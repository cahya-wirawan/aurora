//! The display texture (task 142): the whole composited document held on
//! the GPU, so a zoom or a pan changes only a uniform.
//!
//! The canvas atlas ([`crate::TileResidency`]) is a sliding window: every
//! pan that crosses a tile boundary uploads a new row or column through the
//! CPU, and the zoomed-out view (`aurora-app`'s `zoomed_view`) does the same
//! over a CPU-built pyramid of downsampled tiles. This type instead keeps
//! every composite tile of a document resident, at level 0 of one or more
//! `Rgba16Float` textures ("pages") whose mip chains are built on the GPU
//! ([`DisplayTexture::regenerate_mips`], `shaders/display_mips.wgsl`): a 2 x 2
//! box mean of premultiplied texels, redone only inside the tiles that
//! changed. Once every tile is uploaded a frame uploads nothing, whatever
//! the zoom or pan; a dab uploads its own tiles and rebuilds their mips.
//!
//! **Texels.** Level 0 holds exactly the bytes the atlas would hold for the
//! same tile (the same [`crate::residency`] serializer, premultiplied
//! `f16`), so at or above 100 % the canvas shader's `fs_display` (sampling at
//! level of detail 0, a magnification, `Nearest`) shows the atlas's texel
//! for texel. Texels of a tile that lie outside the document are written as
//! transparent, so a mip texel at the document's right or bottom edge
//! averages in transparency, as the zoomed-out pyramid's do
//! (`aurora_render::downsample_quad`'s `valid`).
//!
//! **Pages.** A document wider or taller than the device's
//! `max_texture_dimension_2d` is split into pages ([`plan_pages`]). Each page
//! owns a range of tiles and also holds one tile of gutter on every side it
//! shares with another page, so a page's filter footprint at its owned edge
//! reads the neighbour's texels exactly as one big texture would: every
//! tile's mip chain depends on that tile alone up to level 8 (a tile is
//! `256 >> k` texels at level `k`), so a tile held by two pages has the same
//! texels in both. A frame draws each page in its own scissor
//! ([`DisplayTexture::page_scissors`]), a partition of the canvas's pixels.
//!
//! **Memory** ([`DisplayTexture::bytes`]): 8 bytes a texel at level 0 plus
//! the chain (about a third more), for whole tiles; a 60 MP photo is about
//! 640 MiB. [`display_budget`] decides what a device may hold.

use aurora_tile::{SAMPLES, TILE, TileId};
use half::f16;

use crate::error::GpuError;
use crate::residency::serialize_premultiplied_le_bytes;

/// Bytes a texel costs at any level (`Rgba16Float`).
const BYTES_PER_TEXEL: u64 = 8;

/// Bytes one tile's level 0 upload is.
const TILE_BYTES: usize = SAMPLES * 2;

/// The deepest mip chain a display texture builds: level 8 is one texel a
/// tile, past which a tile's chain would depend on its neighbours.
pub const MAX_DISPLAY_MIP_LEVELS: u32 = 9;

/// The texel cap a discrete GPU gets (150 MP of level 0, about 1.6 GiB with
/// whole tiles and the chain): an engineering default, not measured against
/// any card's free memory (wgpu reports none).
pub const DISCRETE_TEXEL_CAP: u64 = 150_000_000;

/// The share of physical RAM a GPU that shares system memory (integrated,
/// software, virtual or unknown) may use for the display texture: one
/// eighth.
pub const SHARED_MEMORY_DIVISOR: u64 = 8;

/// What an adapter may hold in its display texture, in bytes (level 0 and
/// the chain, as [`DisplayTexture::bytes`] counts them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayBudget {
    pub max_bytes: u64,
}

/// The budget for an adapter of `device_type` on a machine with
/// `ram_bytes` of physical memory (`None`: unknown). A discrete GPU gets
/// [`DISCRETE_TEXEL_CAP`] texels' worth; any other kind shares system
/// memory and gets the smaller of that and `ram / `[`SHARED_MEMORY_DIVISOR`]
/// (`small_machine` halves it again); unknown RAM on a shared-memory
/// adapter gets 512 MiB. Pure, so it is tested without a device.
#[must_use]
pub fn display_budget(
    device_type: wgpu::DeviceType,
    ram_bytes: Option<u64>,
    small_machine: bool,
) -> DisplayBudget {
    let cap = chain_bytes_for_texels(DISCRETE_TEXEL_CAP);
    let max_bytes = match device_type {
        wgpu::DeviceType::DiscreteGpu => cap,
        wgpu::DeviceType::IntegratedGpu
        | wgpu::DeviceType::Cpu
        | wgpu::DeviceType::VirtualGpu
        | wgpu::DeviceType::Other => {
            let share = ram_bytes.map_or(512 << 20, |ram| ram / SHARED_MEMORY_DIVISOR);
            cap.min(share)
        }
    };
    DisplayBudget {
        max_bytes: if small_machine {
            max_bytes / 2
        } else {
            max_bytes
        },
    }
}

/// Level 0 plus a full chain for `texels`, the `4 / 3` bound.
fn chain_bytes_for_texels(texels: u64) -> u64 {
    texels.saturating_mul(BYTES_PER_TEXEL).saturating_mul(4) / 3
}

/// Bytes a `width` x `height` texture with `levels` mip levels takes.
#[must_use]
pub fn texture_bytes(width: u32, height: u32, levels: u32) -> u64 {
    (0..levels.max(1))
        .map(|level| {
            let w = u64::from((width >> level.min(31)).max(1));
            let h = u64::from((height >> level.min(31)).max(1));
            w * h * BYTES_PER_TEXEL
        })
        .sum()
}

/// One page of a display texture, in tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageLayout {
    /// The first document tile the page holds (its texel `(0, 0)`).
    pub first: (u32, u32),
    /// Tiles it holds per axis (its texture is `tiles * TILE` texels).
    pub tiles: (u32, u32),
    /// The tiles it draws: `[owned_first, owned_end)` per axis, a
    /// partition of the document's tiles over the pages.
    pub owned_first: (u32, u32),
    pub owned_end: (u32, u32),
}

impl PageLayout {
    /// Whether the page holds document tile `id` (owned or gutter).
    #[must_use]
    pub fn holds(&self, id: TileId) -> bool {
        id.x >= self.first.0
            && id.y >= self.first.1
            && id.x - self.first.0 < self.tiles.0
            && id.y - self.first.1 < self.tiles.1
    }

    /// The page's texture size in texels.
    #[must_use]
    pub fn texels(&self) -> (u32, u32) {
        (
            self.tiles.0.saturating_mul(TILE),
            self.tiles.1.saturating_mul(TILE),
        )
    }
}

/// One axis split into `(first, len, owned_first, owned_end)` segments of
/// at most `limit` tiles each, one tile of gutter on every inner side.
fn axis_segments(n: u32, limit: u32) -> Option<Vec<(u32, u32, u32, u32)>> {
    if n == 0 || limit == 0 {
        return None;
    }
    if n <= limit {
        return Some(vec![(0, n, 0, n)]);
    }
    // A split page holds its owned tiles plus at most one gutter tile on
    // each side.
    let stride = limit.checked_sub(2).filter(|&s| s > 0)?;
    let mut out = Vec::new();
    let mut start = 0;
    while start < n {
        let end = start.saturating_add(stride).min(n);
        let first = start.saturating_sub(1);
        let last = end.saturating_add(1).min(n);
        out.push((first, last - first, start, end));
        start = end;
    }
    Some(out)
}

/// The pages a document of `doc_tiles` tiles needs on a device whose
/// largest texture side is `limit_tiles` tiles (`None` when it cannot be
/// split: an empty document, or a limit under three tiles with a document
/// over it).
#[must_use]
pub fn plan_pages(doc_tiles: (u32, u32), limit_tiles: u32) -> Option<Vec<PageLayout>> {
    let xs = axis_segments(doc_tiles.0, limit_tiles)?;
    let ys = axis_segments(doc_tiles.1, limit_tiles)?;
    Some(
        ys.iter()
            .flat_map(|&(fy, ly, oy0, oy1)| {
                xs.iter().map(move |&(fx, lx, ox0, ox1)| PageLayout {
                    first: (fx, fy),
                    tiles: (lx, ly),
                    owned_first: (ox0, oy0),
                    owned_end: (ox1, oy1),
                })
            })
            .collect(),
    )
}

/// Tiles per axis of a document of `doc_size` pixels.
#[must_use]
pub fn doc_tiles(doc_size: (u32, u32)) -> (u32, u32) {
    (doc_size.0.div_ceil(TILE), doc_size.1.div_ceil(TILE))
}

/// The mip levels a display texture over pages of `pages` gets when
/// `wanted` are asked for: at most [`MAX_DISPLAY_MIP_LEVELS`], at least 1.
#[must_use]
pub fn mip_levels_for(wanted: u32) -> u32 {
    wanted.clamp(1, MAX_DISPLAY_MIP_LEVELS)
}

/// What a document of `doc_size` costs as a display texture with
/// `mip_levels` levels on a device whose largest texture side is
/// `max_dimension` (`None` when it cannot be paged).
#[must_use]
pub fn display_bytes(doc_size: (u32, u32), max_dimension: u32, mip_levels: u32) -> Option<u64> {
    let pages = plan_pages(doc_tiles(doc_size), max_dimension / TILE)?;
    let levels = mip_levels_for(mip_levels);
    Some(
        pages
            .iter()
            .map(|page| {
                let (w, h) = page.texels();
                texture_bytes(w, h, levels)
            })
            .sum(),
    )
}

/// The guarded zoom: a non-finite or non-positive one is `1.0`, as the
/// atlas's.
fn guard_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    }
}

/// The level of detail a frame at physical `zoom` samples: `-log2(zoom)`,
/// clamped to `[0, levels - 1]` (`0` at and above 100 %).
#[must_use]
pub fn lod_for_zoom(zoom: f32, levels: u32) -> f32 {
    let lod = -guard_zoom(zoom).log2();
    #[allow(clippy::cast_precision_loss)]
    let top = levels.saturating_sub(1) as f32;
    if lod > 0.0 { lod.min(top) } else { 0.0 }
}

/// The level of detail the atlas-filter sampler is clamped to: under 0.5,
/// so its nearest mip is always level 0, and above 0, so a minified sample
/// stays a minification (`Linear`), as the atlas's.
pub const LEVEL0_LOD_CLAMP: f32 = 0.49;

/// What a frame at physical `zoom` samples, for a canvas whose 1:1 atlas
/// floor is `atlas_floor` (task 142 review: the picture must not change
/// filter where the display texture and the atlas meet): `(level of
/// detail, atlas filter)`.
///
/// - At or above the floor: the true `-log2(zoom)` (0 at or above 100 %)
///   with the atlas's own filter (`true`): level 0, bilinear when minified,
///   nearest when magnified, exactly what the atlas samples, so a frame
///   that falls back to the atlas there shows the same filter.
/// - Below it: trilinear (`false`), the level ramped from 0 at the floor to
///   1 at 50 % (the atlas's filter until the ramp reaches
///   [`MIN_TRILINEAR_LOD`], since a level that small reads as a
///   magnification in hardware) (`(lod - lod_floor) / (1 - lod_floor)`), the true level
///   below 50 %, so crossing the floor does not jump and every
///   power-of-two zoom at or below 50 % samples exactly one level. A floor
///   at or below 50 % (a viewport under two tiles) has no ramp.
///
/// Clamped to `[0, levels - 1]`. Pure.
#[must_use]
pub fn display_lod(zoom: f32, atlas_floor: f32, levels: u32) -> (f32, bool) {
    let zoom = guard_zoom(zoom);
    let floor = if atlas_floor.is_finite() && atlas_floor > 0.0 {
        atlas_floor.min(1.0)
    } else {
        1.0
    };
    let lod = -zoom.log2();
    #[allow(clippy::cast_precision_loss)]
    let top = levels.saturating_sub(1) as f32;
    if zoom >= floor {
        return (if lod > 0.0 { lod } else { 0.0 }, true);
    }
    let lod_floor = -floor.log2();
    let ramped = if lod_floor < 1.0 && lod <= 1.0 {
        (lod - lod_floor) / (1.0 - lod_floor)
    } else {
        lod
    };
    // Hardware holds the level of detail in fixed point: a level under
    // about 1/256 above 0 reads as 0, a magnification (`Nearest`), measured
    // on the RTX 3090. Until the ramp is clearly positive the atlas's filter
    // keeps drawing (the true level, so a minification).
    if ramped < MIN_TRILINEAR_LOD {
        return (if lod > 0.0 { lod } else { 0.0 }, true);
    }
    (ramped.clamp(0.0, top), false)
}

/// The smallest ramped level of detail drawn trilinearly
/// ([`display_lod`]): below it the atlas's filter draws. At it the picture
/// steps by this share of level 1 (about 3 %).
pub const MIN_TRILINEAR_LOD: f32 = 1.0 / 32.0;

/// The first canvas pixel (of `extent`) whose centre lies at or past
/// document position `boundary` for a view whose first pixel's left edge
/// is document position `origin` at `zoom` (`(boundary - origin) * zoom -
/// 0.5`, rounded up, clamped to `[0, extent]`). Neighbouring pages use the
/// same value for their shared boundary, so their scissors partition the
/// canvas.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn first_pixel_at(boundary: f64, origin: f64, zoom: f64, extent: u32) -> u32 {
    let at = ((boundary - origin) * zoom - 0.5).ceil();
    if at.is_nan() || at <= 0.0 {
        0
    } else if at >= f64::from(extent) {
        extent
    } else {
        at as u32
    }
}

/// The canvas pixels each page draws for a view whose top-left pixel is at
/// document position `origin`, at physical `zoom`, in a canvas of
/// `viewport` pixels: `(page index, x0, y0, x1, y1)`, pixel ranges `[x0,
/// x1)`, `[y0, y1)` in the canvas's own coordinates, empty ranges left out.
/// The first and last page of each axis reach the canvas's edges (the
/// document clip discards what lies outside the document). Pure.
#[must_use]
pub fn page_scissors(
    pages: &[PageLayout],
    origin: (f32, f32),
    viewport: (u32, u32),
    zoom: f32,
    doc_tiles: (u32, u32),
) -> Vec<(usize, u32, u32, u32, u32)> {
    let zoom = f64::from(guard_zoom(zoom));
    let tile = f64::from(TILE);
    let range = |first: u32, end: u32, total: u32, origin: f32, extent: u32| {
        let start = if first == 0 {
            0
        } else {
            first_pixel_at(f64::from(first) * tile, f64::from(origin), zoom, extent)
        };
        let stop = if end >= total {
            extent
        } else {
            first_pixel_at(f64::from(end) * tile, f64::from(origin), zoom, extent)
        };
        (start, stop)
    };
    pages
        .iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let (x0, x1) = range(
                page.owned_first.0,
                page.owned_end.0,
                doc_tiles.0,
                origin.0,
                viewport.0,
            );
            let (y0, y1) = range(
                page.owned_first.1,
                page.owned_end.1,
                doc_tiles.1,
                origin.1,
                viewport.1,
            );
            (x1 > x0 && y1 > y0).then_some((index, x0, y0, x1, y1))
        })
        .collect()
}

/// The `Canvas` uniform values (`canvas.wgsl`) of one page: `uv` mapping,
/// document clip, channel gain, the partner weight `0.0` and the level of
/// detail. `origin` is the document position at the canvas's top-left
/// corner (not clamped: a zoomed-out view's can be negative), `page_origin`
/// the page's texel `(0, 0)` in document pixels, `page_texels` its size.
/// The clip uses the atlas's own formula (`(x - origin) * zoom /
/// viewport`), so at the same origin and zoom it is the same rectangle.
/// Pure.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
pub fn page_uniform(
    origin: (f32, f32),
    viewport: (u32, u32),
    zoom: f32,
    page_origin: (f32, f32),
    page_texels: (u32, u32),
    doc_clip: Option<(f32, f32, f32, f32)>,
    gain: [f32; 3],
    lod: f32,
) -> [f32; 16] {
    let zoom = guard_zoom(zoom);
    let (vw, vh) = (viewport.0.max(1) as f32, viewport.1.max(1) as f32);
    let (tw, th) = (page_texels.0.max(1) as f32, page_texels.1.max(1) as f32);
    let clip = match doc_clip {
        None => [-1.0e9, -1.0e9, 1.0e9, 1.0e9],
        Some((x0, y0, x1, y1)) if [x0, y0, x1, y1].iter().all(|v| v.is_finite()) => {
            let to_t = |p: f32, scroll: f32, extent: f32| (p - scroll) * zoom / extent;
            [
                to_t(x0, origin.0, vw),
                to_t(y0, origin.1, vh),
                to_t(x1, origin.0, vw),
                to_t(y1, origin.1, vh),
            ]
        }
        Some(_) => [0.0; 4],
    };
    let gain = gain.map(|g| if g.is_nan() { 1.0 } else { g.clamp(0.0, 1.0) });
    [
        (origin.0 - page_origin.0) / tw,
        (origin.1 - page_origin.1) / th,
        (vw / zoom) / tw,
        (vh / zoom) / th,
        clip[0],
        clip[1],
        clip[2],
        clip[3],
        gain[0],
        gain[1],
        gain[2],
        1.0,
        0.0,
        lod,
        0.0,
        0.0,
    ]
}

/// Zeroes the texels of a serialized tile (`bytes`, `TILE` rows of `TILE`
/// texels, 8 bytes each) at or past `valid` texels per axis: the part of a
/// right- or bottom-edge tile outside the document.
fn mask_outside(bytes: &mut [u8], valid: (u32, u32)) {
    let row_bytes = TILE as usize * BYTES_PER_TEXEL as usize;
    let keep = (valid.0.min(TILE) as usize) * BYTES_PER_TEXEL as usize;
    for (y, row) in bytes.chunks_exact_mut(row_bytes).enumerate() {
        if y >= valid.1 as usize {
            row.fill(0);
        } else if let Some(rest) = row.get_mut(keep..) {
            rest.fill(0);
        }
    }
}

/// The level-`level` rectangles to rebuild for `tiles` (page-local tile
/// coordinates): runs of horizontally adjacent tiles of a row merged, `(x,
/// y, w, h)` in level texels. Pure.
#[must_use]
pub fn mip_rects(tiles: &[(u32, u32)], level: u32) -> Vec<(u32, u32, u32, u32)> {
    let side = (TILE >> level.min(8)).max(1);
    let mut sorted: Vec<(u32, u32)> = tiles.iter().map(|&(x, y)| (y, x)).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut out: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut run: Option<(u32, u32, u32)> = None; // (row, first x, last x)
    for (y, x) in sorted {
        run = match run {
            Some((ry, x0, x1)) if ry == y && x == x1 + 1 => Some((ry, x0, x)),
            Some((ry, x0, x1)) => {
                out.push((x0 * side, ry * side, (x1 - x0 + 1) * side, side));
                Some((y, x, x))
            }
            None => Some((y, x, x)),
        };
    }
    if let Some((ry, x0, x1)) = run {
        out.push((x0 * side, ry * side, (x1 - x0 + 1) * side, side));
    }
    out
}

struct Page {
    layout: PageLayout,
    texture: wgpu::Texture,
    /// Every level, for sampling.
    view: wgpu::TextureView,
    /// One view per level, for the mip passes' render targets.
    level_views: Vec<wgpu::TextureView>,
    /// `mip_bind_groups[k - 1]` binds level `k - 1` as the source of level
    /// `k`'s pass.
    mip_bind_groups: Vec<wgpu::BindGroup>,
    uniform: wgpu::Buffer,
    /// Page-local tiles uploaded since the last [`DisplayTexture::regenerate_mips`].
    dirty: Vec<(u32, u32)>,
}

/// The whole composited document on the GPU (see the module doc).
pub struct DisplayTexture {
    doc_size: (u32, u32),
    doc_tiles: (u32, u32),
    mip_levels: u32,
    pages: Vec<Page>,
    sampler: wgpu::Sampler,
    /// The atlas's own filter, for frames at or above its floor.
    level0_sampler: wgpu::Sampler,
    mip_pipeline: wgpu::RenderPipeline,
    staging: Vec<u8>,
    bytes: u64,
    /// Tile uploads since creation (for tests and measurements).
    uploads: u64,
    /// Mip passes recorded since creation.
    mip_passes: u64,
    /// Tiles copied from another texture since creation.
    copies: u64,
}

const MIP_SHADER: &str = include_str!("shaders/display_mips.wgsl");

impl DisplayTexture {
    /// A display texture for a document of `doc_size` pixels with
    /// `mip_levels` levels ([`mip_levels_for`]), paged for the device's
    /// largest texture side. Every texel starts transparent (wgpu zeroes a
    /// texture before its first use).
    ///
    /// # Errors
    ///
    /// [`GpuError::DisplayTooLarge`] when the document cannot be paged on
    /// this device (an empty document, or a texture limit under three
    /// tiles). Out-of-memory is a wgpu error, which a caller catches with
    /// an error scope.
    pub fn new(
        device: &wgpu::Device,
        doc_size: (u32, u32),
        mip_levels: u32,
    ) -> Result<Self, GpuError> {
        Self::with_max_dimension(device, doc_size, mip_levels, u32::MAX)
    }

    /// [`Self::new`] with pages no larger than `max_dimension` texels a
    /// side (and the device's limit): what lets a test split a small
    /// document into pages.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_max_dimension(
        device: &wgpu::Device,
        doc_size: (u32, u32),
        mip_levels: u32,
        max_dimension: u32,
    ) -> Result<Self, GpuError> {
        let tiles = doc_tiles(doc_size);
        let limit = device.limits().max_texture_dimension_2d.min(max_dimension);
        let layouts = plan_pages(tiles, limit / TILE).ok_or(GpuError::DisplayTooLarge)?;
        let mip_levels = mip_levels_for(mip_levels);
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display-mips"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let mip_pipeline = build_mip_pipeline(device, &mip_layout);
        let mut bytes = 0;
        let pages = layouts
            .into_iter()
            .map(|layout| {
                let (w, h) = layout.texels();
                bytes += texture_bytes(w, h, mip_levels);
                build_page(device, &mip_layout, layout, mip_levels)
            })
            .collect();
        // Trilinear: linear within a level and between two. `Nearest`
        // magnification is the atlas's (and makes level 0 at or above
        // 100 % its texel for texel); clamped, since nothing lies outside
        // the document's pages.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("display"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        // The atlas's filter at or above its floor (task 142 review): level
        // 0 only, `Linear` when minified, `Nearest` when magnified. The
        // level of detail stays positive below 100 % (minification) but is
        // clamped under 0.5, so the nearest mip is always level 0.
        let level0_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("display-level0"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_max_clamp: LEVEL0_LOD_CLAMP,
            ..wgpu::SamplerDescriptor::default()
        });
        Ok(Self {
            doc_size,
            doc_tiles: tiles,
            mip_levels,
            pages,
            sampler,
            level0_sampler,
            mip_pipeline,
            staging: vec![0; TILE_BYTES],
            bytes,
            uploads: 0,
            mip_passes: 0,
            copies: 0,
        })
    }

    /// The document size it was built for.
    #[must_use]
    pub fn doc_size(&self) -> (u32, u32) {
        self.doc_size
    }

    /// Tiles per axis.
    #[must_use]
    pub fn doc_tiles(&self) -> (u32, u32) {
        self.doc_tiles
    }

    /// Mip levels each page has.
    #[must_use]
    pub fn mip_levels(&self) -> u32 {
        self.mip_levels
    }

    /// The pages' layouts.
    #[must_use]
    pub fn pages(&self) -> Vec<PageLayout> {
        self.pages.iter().map(|page| page.layout).collect()
    }

    /// GPU memory the pages take (every level, whole tiles).
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Tile uploads since creation.
    #[must_use]
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// Mip passes recorded since creation.
    #[must_use]
    pub fn mip_passes(&self) -> u64 {
        self.mip_passes
    }

    /// Page `index`'s texture (every level; `COPY_SRC` for reading back).
    #[must_use]
    pub fn page_texture(&self, index: usize) -> Option<&wgpu::Texture> {
        self.pages.get(index).map(|page| &page.texture)
    }

    /// Page `index`'s sampled view, sampler and uniform, for a canvas bind
    /// group ([`crate::CanvasPipeline::display_bind_group`]).
    #[must_use]
    pub fn page_bindings(
        &self,
        index: usize,
        atlas_filter: bool,
    ) -> Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)> {
        let sampler = if atlas_filter {
            &self.level0_sampler
        } else {
            &self.sampler
        };
        self.pages
            .get(index)
            .map(|page| (&page.view, sampler, &page.uniform))
    }

    /// Uploads composite tile `id`'s straight-alpha `texels` to level 0 of
    /// every page holding it (premultiplied, as the atlas does; the part
    /// outside the document transparent), and marks its mips for the next
    /// [`Self::regenerate_mips`]. Returns the bytes written.
    ///
    /// # Errors
    ///
    /// [`GpuError::InvalidTileUpload`] for a wrong sample count,
    /// [`GpuError::DisplayTileOutside`] for a tile outside the document.
    pub fn upload_tile(
        &mut self,
        queue: &wgpu::Queue,
        id: TileId,
        texels: &[f16],
    ) -> Result<u64, GpuError> {
        if texels.len() != SAMPLES {
            return Err(GpuError::InvalidTileUpload {
                mip_level: 0,
                expected: SAMPLES,
                actual: texels.len(),
            });
        }
        if id.x >= self.doc_tiles.0 || id.y >= self.doc_tiles.1 {
            return Err(GpuError::DisplayTileOutside);
        }
        serialize_premultiplied_le_bytes(texels, &mut self.staging);
        let valid = (
            self.doc_size.0.saturating_sub(id.x.saturating_mul(TILE)),
            self.doc_size.1.saturating_sub(id.y.saturating_mul(TILE)),
        );
        if valid.0 < TILE || valid.1 < TILE {
            mask_outside(&mut self.staging, valid);
        }
        let mut written = 0;
        for page in &mut self.pages {
            if !page.layout.holds(id) {
                continue;
            }
            let local = (id.x - page.layout.first.0, id.y - page.layout.first.1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: local.0 * TILE,
                        y: local.1 * TILE,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &self.staging,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TILE * 8),
                    rows_per_image: Some(TILE),
                },
                wgpu::Extent3d {
                    width: TILE,
                    height: TILE,
                    depth_or_array_layers: 1,
                },
            );
            page.dirty.push(local);
            written += TILE_BYTES as u64;
        }
        self.uploads += 1;
        Ok(written)
    }

    /// Whether tile `id` lies wholly inside the document (no texel to mask),
    /// so its level 0 can be copied from another texture as it is.
    #[must_use]
    pub fn tile_is_interior(&self, id: TileId) -> bool {
        id.x < self.doc_tiles.0
            && id.y < self.doc_tiles.1
            && (id.x + 1).saturating_mul(TILE) <= self.doc_size.0
            && (id.y + 1).saturating_mul(TILE) <= self.doc_size.1
    }

    /// Copies interior tiles' level 0 from `source` (an `Rgba16Float`
    /// texture holding them premultiplied, as the atlas does: `(id, texel
    /// origin in source)`) on the GPU, into every page holding them, and
    /// marks their mips for the next [`Self::regenerate_mips`]. A tile that
    /// is not interior is skipped (its outside texels must be masked, which
    /// only [`Self::upload_tile`] does). Returns the tiles copied. The copies
    /// are one submission of their own (not batched with the frame's
    /// encoder): `aurora-app` calls this only on a GPU-path frame that fell
    /// back to the atlas, which needs a visible tile with no composite yet.
    pub fn copy_tiles_from(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        tiles: &[(TileId, (u32, u32))],
    ) -> u32 {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("display-adopt"),
        });
        let mut copied = 0;
        for &(id, (sx, sy)) in tiles {
            if !self.tile_is_interior(id)
                || sx.saturating_add(TILE) > source.width()
                || sy.saturating_add(TILE) > source.height()
            {
                continue;
            }
            for page in &mut self.pages {
                if !page.layout.holds(id) {
                    continue;
                }
                let local = (id.x - page.layout.first.0, id.y - page.layout.first.1);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: source,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: sx, y: sy, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &page.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: local.0 * TILE,
                            y: local.1 * TILE,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: TILE,
                        height: TILE,
                        depth_or_array_layers: 1,
                    },
                );
                page.dirty.push(local);
            }
            copied += 1;
        }
        if copied > 0 {
            queue.submit(std::iter::once(encoder.finish()));
            self.copies += u64::from(copied);
        }
        copied
    }

    /// Tiles adopted by [`Self::copy_tiles_from`] since creation.
    #[must_use]
    pub fn copies(&self) -> u64 {
        self.copies
    }

    /// Clears every level of every page to transparent on the GPU (one
    /// clear pass each, no upload) and drops pending mip work: what a
    /// texture whose source has been replaced wholesale needs.
    pub fn clear(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("display-clear"),
        });
        for page in &mut self.pages {
            page.dirty.clear();
            for view in &page.level_views {
                let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("display-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Whether any uploaded tile still waits for its mips.
    #[must_use]
    pub fn mips_pending(&self) -> bool {
        self.pages.iter().any(|page| !page.dirty.is_empty())
    }

    /// Rebuilds levels `1..` inside the tiles uploaded since the last call,
    /// one render pass per page and level, scissored to runs of those
    /// tiles, and submits them (the uploads, queued writes, land first).
    /// Returns the passes recorded.
    pub fn regenerate_mips(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> u32 {
        if !self.mips_pending() {
            return 0;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("display-mips"),
        });
        let mut passes = 0;
        for page in &mut self.pages {
            if page.dirty.is_empty() {
                continue;
            }
            let tiles = std::mem::take(&mut page.dirty);
            for level in 1..self.mip_levels {
                let (Some(target), Some(bind_group)) = (
                    page.level_views.get(level as usize),
                    page.mip_bind_groups.get(level as usize - 1),
                ) else {
                    continue;
                };
                let (w, h) = page.layout.texels();
                let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("display-mips"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.mip_pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                for (x, y, rw, rh) in mip_rects(&tiles, level) {
                    if x >= lw || y >= lh {
                        continue;
                    }
                    pass.set_scissor_rect(x, y, rw.min(lw - x), rh.min(lh - y));
                    pass.draw(0..3, 0..1);
                }
                passes += 1;
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
        self.mip_passes += u64::from(passes);
        passes
    }

    /// Writes every page's uniform for a view whose top-left canvas pixel
    /// is at document position `origin`, at physical `zoom`, in a canvas of
    /// `viewport` pixels, clipped to `doc_clip` (document pixels, `None`
    /// for no clip), with the Channels panel's `gain`, for a canvas whose
    /// 1:1 atlas floor is `atlas_floor`. Returns whether the frame must draw
    /// with the atlas's filter ([`display_lod`]; the bind group to use).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn set_view(
        &self,
        queue: &wgpu::Queue,
        origin: (f32, f32),
        viewport: (u32, u32),
        zoom: f32,
        doc_clip: Option<(f32, f32, f32, f32)>,
        gain: [f32; 3],
        atlas_floor: f32,
    ) -> bool {
        let (lod, atlas_filter) = display_lod(zoom, atlas_floor, self.mip_levels);
        for page in &self.pages {
            #[allow(clippy::cast_precision_loss)]
            let page_origin = (
                (page.layout.first.0 * TILE) as f32,
                (page.layout.first.1 * TILE) as f32,
            );
            let values = page_uniform(
                origin,
                viewport,
                zoom,
                page_origin,
                page.layout.texels(),
                doc_clip,
                gain,
                lod,
            );
            let mut bytes = Vec::with_capacity(64);
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            queue.write_buffer(&page.uniform, 0, &bytes);
        }
        atlas_filter
    }

    /// [`page_scissors`] for this texture's pages.
    #[must_use]
    pub fn page_scissors(
        &self,
        origin: (f32, f32),
        viewport: (u32, u32),
        zoom: f32,
    ) -> Vec<(usize, u32, u32, u32, u32)> {
        page_scissors(&self.pages(), origin, viewport, zoom, self.doc_tiles)
    }
}

impl std::fmt::Debug for DisplayTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayTexture")
            .field("doc_size", &self.doc_size)
            .field("pages", &self.pages.len())
            .field("mip_levels", &self.mip_levels)
            .field("bytes", &self.bytes)
            .field("uploads", &self.uploads)
            .finish_non_exhaustive()
    }
}

fn build_mip_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("display-mips"),
        source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("display-mips"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("display-mips"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_mip"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_mip"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        multiview_mask: None,
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        cache: None,
    })
}

fn build_page(
    device: &wgpu::Device,
    mip_layout: &wgpu::BindGroupLayout,
    layout: PageLayout,
    mip_levels: u32,
) -> Page {
    let (w, h) = layout.texels();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("display-page"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("display-page"),
        ..wgpu::TextureViewDescriptor::default()
    });
    let level_views: Vec<wgpu::TextureView> = (0..mip_levels)
        .map(|level| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("display-level"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..wgpu::TextureViewDescriptor::default()
            })
        })
        .collect();
    let mip_bind_groups = level_views
        .iter()
        .take(mip_levels.saturating_sub(1) as usize)
        .map(|source| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("display-mips"),
                layout: mip_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                }],
            })
        })
        .collect();
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("display-uniform"),
        size: 64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Page {
        layout,
        texture,
        view,
        level_views,
        mip_bind_groups,
        uniform,
        dirty: Vec::new(),
    }
}

#[cfg(test)]
#[path = "display_test.rs"]
mod tests;
