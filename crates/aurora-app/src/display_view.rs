//! The display-texture view (task 142).
//!
//! For a document whose composite fits the device's display budget
//! ([`eligible`]), the canvas is drawn from one [`aurora_gpu::DisplayTexture`]
//! holding every composite tile of the document with a mip chain built on
//! the GPU, so a wheel zoom or a pan changes only a uniform and uploads
//! nothing once the texture is current. Documents over the budget keep the
//! 1:1 atlas and the zoomed-out pyramid (`zoomed_view`) exactly as before.
//!
//! **Where the texels come from.** The composite tiles are still the
//! document's own composite surface, made current by the composite cache's
//! usual paths (the background worker on the CPU path, synchronous
//! recomposites otherwise). This view only uploads: a tile is uploaded
//! whenever the store's [`aurora_tile::TileStore::content_version`] for it
//! differs from the one last uploaded, so a dab, an undo, a structural edit
//! or a background install reaches the texture through the same tile writes
//! the atlas follows, and nothing is consumed from the store (its dirty
//! flags stay the atlas's).
//!
//! **Each frame** ([`DisplayView::prepare`]): the visible tiles are made
//! current first, exactly as the 1:1 atlas does at or above its floor (no
//! budget) and as the zoomed-out view does below it (under
//! [`zoomed_view::LEVEL0_FRAME_BUDGET`]), and the changed ones uploaded
//! (all of them at or above the atlas floor, so the frame is never staler
//! than the atlas would be). Only on a frame that had none of that to do
//! (and only once the view has held still for [`QUIET_FRAMES`] frames and,
//! on the CPU path, the worker is idle) is the rest of the document
//! composited in the background
//! ([`BACKGROUND_BUDGET`] of synchronous recomposites, or one worker job)
//! and uploaded under [`UPLOAD_BUDGET`], so painting or panning at 100 %
//! costs what it costs on the atlas, and the build continues when the
//! input pauses. Uploaded tiles get their mips rebuilt on the GPU.
//!
//! **What is drawn.** At or above the atlas floor the frame draws the
//! texture only when every visible tile is uploaded at its current content
//! ([`DisplayAdvance::ready`]); otherwise it draws the atlas, unchanged, and
//! the texture adopts the tiles the atlas has just uploaded by a GPU copy
//! ([`DisplayView::adopt_from_atlas`], GPU compositing path only). On the
//! CPU compositing path (the worker or the frame pool) the atlas always
//! draws at or above its floor and the texture does no visible-tile work
//! there (its own dispatch, uploads and even the GPU copies measured slower
//! than the atlas alone, 1.5-6 ms under load); it is built when the view
//! holds still ([`QUIET_FRAMES`]) and takes over below the floor.
//! Below the floor it always draws the texture: a tile not uploaded yet is
//! transparent there (the checkerboard), as a cold zoomed-out pyramid is.
//!
//! **Lifetime.** One texture, for the active document only: a document
//! switch, a crop (a new canvas size) or a new store builds a new one and
//! re-uploads every tile. A window resize or a lost surface leaves it as it
//! is (its size is the document's, not the window's). **Device loss is not
//! handled**, as nothing in the app handles it: a lost device makes this
//! texture as invalid as the atlas, the pipelines and every other GPU
//! object, and a future recovery must drop `App::display` with them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use aurora_tile::{TILE, TileId};

use crate::zoomed_view::{self, FrameDocument, Level0};

/// Mip levels the texture has: down to the canvas's deepest zoom,
/// `1 / 2^MAX_LEVEL`.
pub(crate) const DISPLAY_MIP_LEVELS: u32 = zoomed_view::MAX_LEVEL + 1;

/// Upload time per frame for tiles not owed this frame.
pub(crate) const UPLOAD_BUDGET: Duration = Duration::from_millis(4);

/// Synchronous recomposite time per frame for tiles outside the view.
pub(crate) const BACKGROUND_BUDGET: Duration = Duration::from_millis(4);

/// Frames the view must hold still, with nothing visible to composite or
/// upload, before the rest of the document is built in the background
/// (about 130 ms at 60 Hz): while the user pans, zooms or paints the frame
/// does only what the atlas would, and the build never competes with the
/// worker for the tiles coming into view.
pub(crate) const QUIET_FRAMES: u32 = 8;

/// The environment variable choosing the path: `0`/`off`/`false`/`no`
/// never uses the display texture, `1`/`on`/`force` uses it for every
/// document the device can page (the budget ignored), anything else (or
/// unset) decides by the budget.
pub(crate) const DISPLAY_ENV: &str = "AURORA_DISPLAY_TEXTURE";

/// How [`DISPLAY_ENV`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    Off,
    Auto,
    Force,
}

/// [`DISPLAY_ENV`]'s value as a [`DisplayMode`]. Pure.
#[must_use]
pub(crate) fn mode_from(value: Option<&str>) -> DisplayMode {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "0" | "off" | "false" | "no") => DisplayMode::Off,
        Some(v) if matches!(v.as_str(), "1" | "on" | "force" | "true" | "yes") => {
            DisplayMode::Force
        }
        _ => DisplayMode::Auto,
    }
}

/// [`mode_from`] of this process's environment, read once.
#[must_use]
pub(crate) fn mode() -> DisplayMode {
    static MODE: std::sync::OnceLock<DisplayMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| mode_from(std::env::var(DISPLAY_ENV).ok().as_deref()))
}

/// Whether a document of `doc_size` is drawn through the display texture
/// on a device whose largest texture side is `max_dimension` with
/// `budget`. Pure.
#[must_use]
pub(crate) fn eligible(
    doc_size: (u32, u32),
    max_dimension: u32,
    budget: aurora_gpu::DisplayBudget,
    mode: DisplayMode,
) -> bool {
    if mode == DisplayMode::Off {
        return false;
    }
    let Some(bytes) = aurora_gpu::display_bytes(doc_size, max_dimension, DISPLAY_MIP_LEVELS) else {
        return false;
    };
    mode == DisplayMode::Force || bytes <= budget.max_bytes
}

/// What a display texture was built for: (composite cache id, store
/// serial, canvas size). Any change builds a new one.
pub(crate) type DisplayKey = (u64, u64, (u32, u32));

/// The document tiles a view samples: those under the canvas, whose top-left
/// pixel is document position `origin` at physical `zoom` in a canvas of
/// `viewport` pixels, plus the reach of the filter at the level it samples
/// (one texel of that level), clamped to the document. Pure.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn visible_tiles(
    origin: (f32, f32),
    zoom: f32,
    viewport: (u32, u32),
    doc_size: (u32, u32),
) -> Vec<TileId> {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        f64::from(zoom)
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation)]
    let lod = aurora_gpu::lod_for_zoom(zoom as f32, DISPLAY_MIP_LEVELS);
    // Trilinear at `lod` reads up to the level above `floor(lod)`: one of
    // its texels is `2^(floor(lod) + 1)` document pixels. At level 0 with
    // nearest magnification nothing is read past the pixel itself.
    let reach = if lod > 0.0 {
        f64::from(1_u32 << (lod.floor() as u32 + 1).min(DISPLAY_MIP_LEVELS))
    } else {
        0.0
    };
    let tile = f64::from(TILE);
    let axis = |origin: f32, extent: u32, doc: u32| -> std::ops::Range<u32> {
        let origin = if origin.is_finite() {
            f64::from(origin)
        } else {
            0.0
        };
        let start = (origin - reach).max(0.0);
        let end = (origin + f64::from(extent) / zoom + reach).min(f64::from(doc));
        if end <= start {
            return 0..0;
        }
        let first = (start / tile).floor() as u32;
        let last = (end / tile).ceil() as u32;
        first..last.min(doc.div_ceil(TILE)).max(first)
    };
    let xs = axis(origin.0, viewport.0, doc_size.0);
    let ys = axis(origin.1, viewport.1, doc_size.1);
    ys.flat_map(|y| xs.clone().map(move |x| TileId { x, y }))
        .collect()
}

#[cfg(test)]
thread_local! {
    /// Test-only: milliseconds spent in [`DisplayView::prepare`]'s phases
    /// (visible composite, visible upload, background, mips), summed.
    pub(crate) static PHASES: std::cell::Cell<[f64; 4]> = const { std::cell::Cell::new([0.0; 4]) };
}

/// A frame drawn from the display texture: where its view is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DisplayFrame {
    /// The document position at the canvas's top-left pixel.
    pub(crate) origin: (f32, f32),
    /// Physical zoom.
    pub(crate) zoom: f32,
}

/// What one [`DisplayView::prepare`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayAdvance {
    /// Tiles uploaded this frame.
    pub(crate) uploaded: u32,
    /// Level-0 tiles recomposited synchronously this frame.
    pub(crate) composited: usize,
    /// Every visible tile is uploaded at its current content (what lets a
    /// frame at or above the atlas floor draw the texture).
    pub(crate) ready: bool,
    /// Work the UI thread can still do on a later frame (a busy worker's
    /// install wakes the loop itself).
    pub(crate) more: bool,
}

/// The display texture of one document and what it holds.
pub(crate) struct DisplayView {
    texture: aurora_gpu::DisplayTexture,
    key: DisplayKey,
    /// The content version each tile was uploaded at.
    uploaded: HashMap<TileId, u64>,
    /// One canvas bind group per page, built on the first draw.
    groups: Vec<wgpu::BindGroup>,
    /// The view (`origin.x`, `origin.y`, zoom) of the last frame.
    last_view: Option<(f32, f32, f32)>,
    /// Frames in a row with that view and no visible work: the background
    /// build waits for [`QUIET_FRAMES`] of them.
    quiet: u32,
}

impl DisplayView {
    /// A view for `key` (its canvas size the document's), every texel
    /// transparent.
    ///
    /// # Errors
    ///
    /// When the document cannot be paged on this device.
    pub(crate) fn new(
        device: &wgpu::Device,
        key: DisplayKey,
    ) -> Result<Self, aurora_gpu::GpuError> {
        Self::with_max_dimension(device, key, u32::MAX)
    }

    /// [`Self::new`] with pages of at most `max_dimension` texels a side.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub(crate) fn with_max_dimension(
        device: &wgpu::Device,
        key: DisplayKey,
        max_dimension: u32,
    ) -> Result<Self, aurora_gpu::GpuError> {
        let texture = aurora_gpu::DisplayTexture::with_max_dimension(
            device,
            key.2,
            DISPLAY_MIP_LEVELS,
            max_dimension,
        )?;
        Ok(Self {
            texture,
            key,
            uploaded: HashMap::new(),
            groups: Vec::new(),
            last_view: None,
            quiet: 0,
        })
    }

    #[must_use]
    pub(crate) fn key(&self) -> DisplayKey {
        self.key
    }

    #[must_use]
    pub(crate) fn texture(&self) -> &aurora_gpu::DisplayTexture {
        &self.texture
    }

    /// Whether tile `id` is uploaded at the store's current content.
    fn uploaded_current(&self, store: &aurora_tile::TileStore, id: TileId) -> bool {
        store.contains_tile(crate::composite_surface_id(), id)
            && self.uploaded.get(&id).copied()
                == Some(store.content_version(crate::composite_surface_id(), id))
    }

    /// Uploads tile `id` if the store holds content for it newer than what
    /// was uploaded; whether it did.
    fn upload_if_changed(
        &mut self,
        queue: &wgpu::Queue,
        store: &mut aurora_tile::TileStore,
        id: TileId,
    ) -> bool {
        let surface = crate::composite_surface_id();
        if !store.contains_tile(surface, id) {
            return false;
        }
        let version = store.content_version(surface, id);
        if self.uploaded.get(&id).copied() == Some(version) {
            return false;
        }
        let Ok(tile) = store.get(surface, id) else {
            return false;
        };
        match self.texture.upload_tile(queue, id, tile.texels()) {
            Ok(_) => {
                self.uploaded.insert(id, version);
                true
            }
            Err(err) => {
                tracing::warn!(?id, %err, "skipping a display tile upload");
                false
            }
        }
    }

    /// One frame's work for showing `visible` (see the module doc).
    /// `one_to_one`: the frame is at or above the 1:1 atlas floor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: FrameDocument<'_>,
        visible: &[TileId],
        level0: Level0<'_>,
        one_to_one: bool,
        gpu_path: bool,
        view: (f32, f32, f32),
        atlas_tiles: &[TileId],
    ) -> DisplayAdvance {
        let FrameDocument {
            canvas,
            layers,
            active_layer,
            store,
            cache,
            ..
        } = doc;
        let mut result = DisplayAdvance::default();
        let tiles = aurora_gpu::doc_tiles(canvas);
        let in_doc = |id: &TileId| id.x < tiles.0 && id.y < tiles.1;
        let visible: Vec<TileId> = visible.iter().copied().filter(in_doc).collect();
        let mut level0 = level0;
        let phase_start = Instant::now();
        // 1. The visible tiles current, as the path drawing them would. At
        // or above the atlas floor that is the atlas's own tile list (its
        // grid, a tile wider than the view), so a frame that falls back to
        // the atlas finds every tile it composites already dispatched.
        let composite_set: Vec<TileId> = if one_to_one {
            let mut set: Vec<TileId> = atlas_tiles.iter().copied().filter(in_doc).collect();
            set.extend(
                visible
                    .iter()
                    .copied()
                    .filter(|id| !atlas_tiles.contains(id)),
            );
            set
        } else {
            visible.clone()
        };
        let stale: Vec<TileId> = composite_set
            .iter()
            .copied()
            .filter(|&id| !cache.is_current(id))
            .collect();
        // On the CPU path the atlas draws at or above its floor (task 142
        // review: the texture's own composite dispatch and uploads there
        // measured slower than the atlas alone), and composites for itself.
        let atlas_draws = one_to_one && !gpu_path;
        if !stale.is_empty() && !atlas_draws {
            match (&mut level0, one_to_one) {
                (Level0::Worker(worker), true) => crate::dispatch_or_composite_tiles(
                    &composite_set,
                    layers,
                    active_layer,
                    store,
                    cache,
                    worker,
                ),
                (Level0::Worker(worker), false) => {
                    result.more |= crate::dispatch_or_composite_tiles_bounded(
                        &visible,
                        layers,
                        active_layer,
                        store,
                        cache,
                        worker,
                        Some(zoomed_view::LEVEL0_FRAME_BUDGET),
                    );
                }
                (Level0::Sync(gpu, compositor), true) => {
                    crate::recomposite_tiles(
                        &composite_set,
                        layers,
                        active_layer,
                        store,
                        cache,
                        *gpu,
                        compositor.as_deref_mut(),
                    );
                    result.composited += stale.len();
                }
                (Level0::Sync(gpu, compositor), false) => {
                    let (done, more) = zoomed_view::recomposite_within_budget(
                        &stale,
                        layers,
                        active_layer,
                        store,
                        cache,
                        *gpu,
                        compositor.as_deref_mut(),
                        zoomed_view::LEVEL0_FRAME_BUDGET,
                    );
                    result.composited += done;
                    result.more |= more;
                }
            }
        }
        let t_composite = phase_start.elapsed();
        // 2. The changed visible tiles uploaded (all of them at or above
        // the atlas floor, so the frame is never staler than the atlas's),
        // unless at or above the floor a visible tile has no content yet:
        // then the atlas draws this frame, and uploading the others here as
        // well would only upload them twice.
        let started = Instant::now();
        let surface = crate::composite_surface_id();
        // There the texture is built by its quiet-frame background build
        // and takes over below the floor.
        let drawable = !atlas_draws
            && (!one_to_one || visible.iter().all(|&id| store.contains_tile(surface, id)));
        let budget = (!one_to_one).then_some(UPLOAD_BUDGET);
        let (uploaded, visible_left) = if drawable {
            self.upload_tiles(queue, store, &visible, budget, started)
        } else {
            (0, false)
        };
        result.uploaded = uploaded;
        result.more |= visible_left;
        let t_upload = phase_start.elapsed();
        // 3. The rest of the document, in the background: only after
        // `QUIET_FRAMES` frames with the view still and nothing visible
        // stale, composited or uploaded, and (CPU path) the worker idle, so
        // a dab or a pan costs what it costs on the atlas.
        let all: Vec<TileId> = (0..tiles.1)
            .flat_map(|y| (0..tiles.0).map(move |x| TileId { x, y }))
            .collect();
        let busy_frame =
            !stale.is_empty() || uploaded > 0 || visible_left || (!drawable && !atlas_draws);
        self.quiet = if busy_frame || self.last_view != Some(view) {
            0
        } else {
            self.quiet.saturating_add(1)
        };
        self.last_view = Some(view);
        let worker_busy = matches!(&level0, Level0::Worker(worker) if worker.is_busy());
        if self.quiet >= QUIET_FRAMES && !worker_busy {
            let (done, more) =
                composite_rest(&all, layers, active_layer, store, cache, &mut level0);
            result.composited += done;
            result.more |= more;
            let (rest, rest_left) =
                self.upload_tiles(queue, store, &all, Some(UPLOAD_BUDGET), started);
            result.uploaded += rest;
            result.more |= rest_left;
        } else {
            // Deferred to a later frame. A busy worker wakes the loop when
            // its result lands; anything else asks for a frame (which is
            // how the quiet frames are counted).
            let pending = all
                .iter()
                .any(|&id| !cache.is_current(id) || !self.uploaded_current(store, id));
            result.more |= pending && !worker_busy;
        }
        let t_rest = phase_start.elapsed();
        // 4. Their mips, on the GPU: when this frame samples them (below
        // 100 %), or on a frame with nothing else to do. A frame at or above
        // 100 % reads level 0 only, so while painting there the mip passes
        // (and their render-target transitions of the whole texture) wait.
        if view.2 < 1.0 || !busy_frame {
            let _ = self.texture.regenerate_mips(device, queue);
        } else if self.texture.mips_pending() {
            result.more = true;
        }
        #[cfg(test)]
        PHASES.with(|p| {
            let mut v = p.get();
            let done = phase_start.elapsed();
            let marks = [
                t_composite,
                t_upload - t_composite,
                t_rest - t_upload,
                done - t_rest,
            ];
            for (sum, mark) in v.iter_mut().zip(marks) {
                *sum += mark.as_secs_f64() * 1000.0;
            }
            p.set(v);
        });
        #[cfg(not(test))]
        let _ = (t_composite, t_upload, t_rest);
        result.ready = !atlas_draws && visible.iter().all(|&id| self.uploaded_current(store, id));
        result
    }

    /// Uploads the changed tiles of `tiles` in order, stopping once
    /// `budget` (from `started`) is spent after at least one upload (`None`:
    /// every one); the count and whether the budget left some.
    fn upload_tiles(
        &mut self,
        queue: &wgpu::Queue,
        store: &mut aurora_tile::TileStore,
        tiles: &[TileId],
        budget: Option<Duration>,
        started: Instant,
    ) -> (u32, bool) {
        let mut uploaded = 0;
        for &id in tiles {
            if self.uploaded_current(store, id)
                || !store.contains_tile(crate::composite_surface_id(), id)
            {
                continue;
            }
            if let Some(budget) = budget
                && uploaded > 0
                && started.elapsed() >= budget
            {
                return (uploaded, true);
            }
            if self.upload_if_changed(queue, store, id) {
                uploaded += 1;
            }
        }
        (uploaded, false)
    }

    /// After the atlas drew a frame in this view's place (at or above its
    /// floor, a visible tile not uploaded here yet): adopts the visible
    /// tiles the atlas now holds at the store's current content by a GPU
    /// copy, so the tiles the atlas has just uploaded are not uploaded a
    /// second time from the CPU when this view takes over. Edge tiles (whose
    /// outside texels this view masks) are left to the ordinary upload.
    /// Returns the tiles adopted.
    pub(crate) fn adopt_from_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &aurora_gpu::TileResidency,
        store: &aurora_tile::TileStore,
        visible: &[TileId],
    ) -> u32 {
        let surface = crate::composite_surface_id();
        let mut wanted = Vec::new();
        let mut versions = Vec::new();
        for &id in visible {
            if !store.contains_tile(surface, id)
                || store.is_dirty(surface, id)
                || self.uploaded_current(store, id)
                || !self.texture.tile_is_interior(id)
            {
                continue;
            }
            if let Some(origin) = atlas.slot_origin(id) {
                wanted.push((id, origin));
                versions.push((id, store.content_version(surface, id)));
            }
        }
        if wanted.is_empty() {
            return 0;
        }
        let copied = self
            .texture
            .copy_tiles_from(device, queue, atlas.texture(), &wanted);
        self.uploaded.extend(versions);
        // Their mips wait for the next `prepare` that needs them (this is a
        // frame at or above 100 %).
        copied
    }

    /// Writes the view's uniforms: `origin` the document position at the
    /// canvas's top-left pixel, `zoom` physical, `viewport` the canvas's
    /// physical size, the document clip and the channel gain.
    pub(crate) fn set_view(
        &self,
        queue: &wgpu::Queue,
        origin: (f32, f32),
        viewport: (u32, u32),
        zoom: f32,
        doc_clip: (f32, f32, f32, f32),
        gain: [f32; 3],
    ) {
        self.texture
            .set_view(queue, origin, viewport, zoom, Some(doc_clip), gain);
    }

    /// The per-page bind groups, built once.
    pub(crate) fn bind_groups(
        &mut self,
        canvas: &aurora_gpu::CanvasPipeline,
        device: &wgpu::Device,
    ) -> &[wgpu::BindGroup] {
        if self.groups.is_empty() {
            self.groups = (0..self.texture.pages().len())
                .filter_map(|page| canvas.display_bind_group(device, &self.texture, page))
                .collect();
        }
        &self.groups
    }

    /// The content version tile `id` was last uploaded at.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn uploaded_version(&self, id: TileId) -> Option<u64> {
        self.uploaded.get(&id).copied()
    }

    /// Tiles uploaded since the view was built.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn uploads(&self) -> u64 {
        self.texture.uploads()
    }
}

/// Composites the tiles of `all` that are not current, in the background
/// (one worker job, or [`BACKGROUND_BUDGET`] of synchronous recomposites);
/// the tiles done synchronously and whether work was left for the UI thread.
fn composite_rest(
    all: &[TileId],
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    store: &mut aurora_tile::TileStore,
    cache: &mut crate::CompositeCache,
    level0: &mut Level0<'_>,
) -> (usize, bool) {
    let rest: Vec<TileId> = all
        .iter()
        .copied()
        .filter(|&id| !cache.is_current(id))
        .collect();
    if rest.is_empty() {
        return (0, false);
    }
    match level0 {
        Level0::Worker(worker) => (
            0,
            crate::dispatch_or_composite_tiles_bounded(
                &rest,
                layers,
                active_layer,
                store,
                cache,
                worker,
                Some(BACKGROUND_BUDGET),
            ),
        ),
        Level0::Sync(gpu, compositor) => zoomed_view::recomposite_within_budget(
            &rest,
            layers,
            active_layer,
            store,
            cache,
            *gpu,
            compositor.as_deref_mut(),
            BACKGROUND_BUDGET,
        ),
    }
}

/// The scissor each page draws in, in surface pixels: [`aurora_gpu::page_scissors`]
/// for a canvas whose viewport starts at surface position `viewport_x`,
/// `viewport_y` (possibly fractional), clipped to `clip` (`x, y, w, h`). A
/// single page draws the whole clip. Pure.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn surface_scissors(
    texture: &aurora_gpu::DisplayTexture,
    origin: (f32, f32),
    zoom: f32,
    viewport_xy: (f32, f32),
    clip: (u32, u32, u32, u32),
) -> Vec<(usize, u32, u32, u32, u32)> {
    let pages = texture.pages();
    if pages.len() == 1 {
        return vec![(0, clip.0, clip.1, clip.2, clip.3)];
    }
    let zoom_ok = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    // Partition the surface's own pixel grid: pixel `floor(vx) + i`'s
    // centre is `origin + (i + 0.5 - frac(vx)) / zoom`.
    let base = (
        viewport_xy.0.floor().max(0.0),
        viewport_xy.1.floor().max(0.0),
    );
    let shift = (viewport_xy.0 - base.0, viewport_xy.1 - base.1);
    let local_origin = (origin.0 - shift.0 / zoom_ok, origin.1 - shift.1 / zoom_ok);
    let extent = (
        (clip.0 + clip.2).saturating_sub(base.0 as u32),
        (clip.1 + clip.3).saturating_sub(base.1 as u32),
    );
    aurora_gpu::page_scissors(&pages, local_origin, extent, zoom_ok, texture.doc_tiles())
        .into_iter()
        .filter_map(|(page, x0, y0, x1, y1)| {
            let x0 = (x0 + base.0 as u32).max(clip.0);
            let y0 = (y0 + base.1 as u32).max(clip.1);
            let x1 = (x1 + base.0 as u32).min(clip.0 + clip.2);
            let y1 = (y1 + base.1 as u32).min(clip.1 + clip.3);
            (x1 > x0 && y1 > y0).then_some((page, x0, y0, x1 - x0, y1 - y0))
        })
        .collect()
}

#[cfg(test)]
#[path = "display_view_tests.rs"]
mod tests;
