//! GPU tile residency: a fixed-size, tile-aligned atlas texture that
//! slides over the (potentially unbounded) document, with toroidal slot
//! addressing so panning invalidates one row/column of GPU uploads
//! instead of the whole texture (`spike/FINDINGS.md` finding #4).

use std::collections::HashMap;

use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::error::GpuError;

/// The `Canvas` uniform `canvas.wgsl` expects: `uv_offset`/`uv_scale`,
/// two `vec2<f32>`s, 16 bytes total.
const UNIFORM_SIZE: u64 = 16;

/// Bytes one tile upload costs — `f16` samples, `SAMPLES` per tile.
const TILE_BYTES: usize = SAMPLES * 2;

/// Mip levels the atlas carries: level 0 is full resolution ([`TILE`] ×
/// [`TILE`]), each level above halves the side length. Fixed at 4 rather
/// than configurable — nothing needs more, and this crate doesn't depend
/// on `aurora-render`'s `MipLevel` enum, but the correspondence is exact
/// by convention: 0 = Full, 1 = Half, 2 = Quarter, 3 = Eighth.
const MIP_LEVELS: u32 = 4;

/// The result of one [`TileResidency::sync`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub uploaded: u32,
    pub bytes_uploaded: u64,
    /// Tiles that still need upload after this call — budget-limited or
    /// errored. Non-zero means: request another frame, don't consider
    /// this view fully caught up yet.
    pub remaining: u32,
    /// Subset of `remaining` that failed to load (not merely
    /// budget-skipped) — a distinct, exceptional condition worth
    /// surfacing separately from "just not enough budget this frame."
    pub errors: u32,
}

/// A GPU-resident window over a tile store: a tile-aligned atlas texture
/// sized to a viewport (plus one tile of margin), whose slots are
/// addressed toroidally (`tile index modulo grid size`) so that panning
/// by one tile invalidates one row or column of uploads, not the whole
/// texture. Ported from `spike/vertical-slice`'s `Renderer` (real,
/// measured — `spike/FINDINGS.md`), generalized to build against the
/// real `aurora_tile::TileStore` API rather than the spike's own
/// throwaway store.
///
/// Handles window resize via [`Self::resize`], which rebuilds the atlas
/// texture at the new size and resets slot occupancy — see that
/// method's own doc comment for exactly what carries over and what
/// doesn't.
pub struct TileResidency {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    /// Slots across, slots down.
    grid: (u32, u32),
    /// Which tile currently occupies each modulo-addressed slot.
    slots: HashMap<(u32, u32), TileId>,
    /// Top-left visible tile.
    origin: TileId,
}

impl TileResidency {
    /// Sizes the atlas to `viewport_px`, rounded up to whole tiles plus
    /// one tile of margin (matches the spike's `ct = viewport/TILE + 1`
    /// exactly), and establishes an initial origin of `(0, 0)`.
    #[must_use]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, viewport_px: (u32, u32)) -> Self {
        let grid = (
            viewport_px.0.div_ceil(TILE) + 1,
            viewport_px.1.div_ceil(TILE) + 1,
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile-residency"),
            size: wgpu::Extent3d {
                width: grid.0 * TILE,
                height: grid.1 * TILE,
                depth_or_array_layers: 1,
            },
            mip_level_count: MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            // COPY_SRC beyond the two production-path flags (TEXTURE_BINDING
            // for sampling, COPY_DST for uploads) so the atlas can be read
            // back for verification -- real capability, not test-only scope
            // creep: debugging/inspection tooling will want this too.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // The wraparound is the hardware sampler's job, not WGSL's --
        // matches the spike exactly (`AddressMode::Repeat` both axes).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tile-residency"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile-residency-uniform"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let residency = Self {
            texture,
            view,
            sampler,
            uniform_buffer,
            grid,
            slots: HashMap::new(),
            origin: TileId { x: 0, y: 0 },
        };
        residency.write_uniform(queue, viewport_px, 1.0);
        residency
    }

    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    #[must_use]
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// For building a bind group externally — `TileResidency` doesn't
    /// know about bind group layouts, the same boundary `PipelineCache`
    /// already draws.
    #[must_use]
    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    #[must_use]
    pub const fn origin(&self) -> TileId {
        self.origin
    }

    /// Every [`TileId`] currently within this atlas's own visible grid,
    /// document-space, in the same fixed row-major order [`Self::sync`]
    /// itself iterates — what a caller computing its own per-tile
    /// content (rather than reading a single [`TileStore`] surface
    /// [`Self::sync`] can already do directly) needs to know what to
    /// actually produce. `aurora-render`'s multi-layer CPU compositing
    /// (`composite_tile_cpu`) is the first real consumer: its own
    /// orchestration (in `aurora-app`, which depends on both this crate
    /// and `aurora-doc`) must run *before* the next [`Self::sync`] call
    /// each frame, so the surface `sync` then reads has fresh,
    /// already-composited content.
    pub fn visible_tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        let origin = self.origin;
        let grid = self.grid;
        (0..grid.1).flat_map(move |gy| {
            (0..grid.0).map(move |gx| TileId {
                x: origin.x + gx,
                y: origin.y + gy,
            })
        })
    }

    /// Call when the visible top-left tile changes (panning) or `zoom`
    /// itself changes. Updates the UV uniform immediately; the texture
    /// itself is only touched by the next [`Self::sync`].
    ///
    /// `zoom`: document pixels per logical screen pixel, matching
    /// `aurora_ui::CanvasView::zoom`'s own convention (`1.0` = 100%,
    /// `> 1.0` magnifies). Shrinks `uv_scale` by this factor — at 200%
    /// zoom, half as many atlas texels stretch across the same
    /// viewport, magnifying them — the shader-side scaling this
    /// texture-sliding-window design needs instead of an actual bigger
    /// upload (the atlas itself is still sized in document pixels, one
    /// tile of margin at 100%, unrelated to `zoom`). Callers must pass a
    /// positive `zoom`; `aurora_ui::CanvasView` already clamps to
    /// `[MIN_ZOOM, MAX_ZOOM]`, both comfortably positive, so there is no
    /// zero/negative case to guard against here.
    pub fn set_origin(
        &mut self,
        queue: &wgpu::Queue,
        origin: TileId,
        viewport_px: (u32, u32),
        zoom: f32,
    ) {
        self.origin = origin;
        self.write_uniform(queue, viewport_px, zoom);
    }

    /// Rebuilds the atlas at a new `viewport_px` — the real fix for the
    /// limitation this struct's own doc comment used to name. There is
    /// no in-place way to resize a `wgpu::Texture`, so this reconstructs
    /// `texture`/`view`/`sampler`/`uniform_buffer` via [`Self::new`]'s
    /// own construction logic (`*self = Self::new(...)`) rather than
    /// duplicating it, which also resets `slots` to empty exactly as a
    /// freshly-constructed atlas starts. That reset matters for
    /// correctness, not just tidiness: every slot coordinate in the old
    /// `HashMap` was computed against the *old* `grid` (`tile index
    /// modulo grid size`) and is meaningless — even out of bounds — for
    /// the new one, so the next [`Self::sync`] call must re-upload every
    /// visible tile fresh rather than trusting stale bookkeeping.
    ///
    /// The document-space `origin` (which tile is top-left) carries over
    /// unchanged — a resize changes how much of the document is
    /// visible, not *which* part is being viewed. `zoom` isn't carried
    /// over (this method has no way to know the caller's current value,
    /// and `TileResidency` doesn't store it between calls), so the
    /// uniform is rewritten at `zoom = 1.0` same as [`Self::new`]; a
    /// caller that cares about zoom being exactly right for the one
    /// frame between a resize and its next [`Self::set_origin`] call
    /// should pass its current zoom there too. `aurora-app`'s real usage
    /// calls `set_origin` every frame before `sync` regardless, so both
    /// `origin` and `zoom` are corrected before anything is drawn.
    ///
    /// No-ops on a zero-sized request (a minimized window can report
    /// `0x0`), mirroring [`crate::GpuSurface::resize`]'s own guard
    /// against calling into wgpu with an invalid size, which panics.
    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport_px: (u32, u32)) {
        if viewport_px.0 == 0 || viewport_px.1 == 0 {
            return;
        }
        let origin = self.origin;
        *self = Self::new(device, queue, viewport_px);
        self.origin = origin;
        self.write_uniform(queue, viewport_px, 1.0);
    }

    fn write_uniform(&self, queue: &wgpu::Queue, viewport_px: (u32, u32), zoom: f32) {
        let tex_w = (self.grid.0 * TILE) as f32;
        let tex_h = (self.grid.1 * TILE) as f32;
        // Absolute scroll (origin in pixels), wrapped by the repeat
        // sampler -- slot addressing is toroidal, so the texture is a
        // sliding window over the document, exactly as in the spike.
        let scroll = (self.origin.x * TILE, self.origin.y * TILE);
        let u = (scroll.0 % (self.grid.0 * TILE)) as f32 / tex_w;
        let v = (scroll.1 % (self.grid.1 * TILE)) as f32 / tex_h;
        let uv_scale = [
            viewport_px.0 as f32 / zoom / tex_w,
            viewport_px.1 as f32 / zoom / tex_h,
        ];
        let mut bytes = Vec::with_capacity(16);
        for value in [u, v, uv_scale[0], uv_scale[1]] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniform_buffer, 0, &bytes);
    }

    /// Uploads visible slots that don't already hold the correct, clean
    /// tile, up to `byte_budget` bytes this call — a fast pan can expose
    /// far more tiles than fit in one frame's bandwidth
    /// (`spike/FINDINGS.md` finding #3: "~18 MB per screenful"), so this
    /// caps the cost per call rather than uploading everything at once.
    ///
    /// Tiles that don't fit in the budget (or fail to load) aren't
    /// marked resident, so the *next* call's own resident check finds
    /// them again automatically — no separate pending-tile queue needed.
    /// Iterating the grid in the same fixed order every call means a
    /// budget smaller than the full backlog fills in from the start of
    /// that order forward, one call at a time, converging to
    /// `remaining == 0` rather than starving any tile.
    ///
    /// `force`: re-upload every visible slot unconditionally. Still not
    /// exercised by [`Self::resize`] — that method clears `slots`
    /// directly (every slot already reads as non-resident against the
    /// new, empty map, so the ordinary resident check already forces a
    /// full re-upload on the next `sync` without needing this flag) —
    /// so this remains without a real caller, kept for a future case
    /// that wants a full re-sync without a slot-mapping change.
    ///
    /// `surface`: which of `store`'s surfaces this atlas is showing
    /// (ADR 0010 — `store` may hold many). This crate has no document
    /// assembly yet to say *which* surface that should be (a single
    /// layer's own preview vs. a whole document's composited result) —
    /// real, separate follow-on work; today's only callers (this
    /// crate's own tests) just pick one.
    pub fn sync(
        &mut self,
        queue: &wgpu::Queue,
        store: &mut TileStore,
        surface: SurfaceId,
        force: bool,
        byte_budget: usize,
    ) -> SyncStats {
        let mut stats = SyncStats::default();
        let mut bytes_left = byte_budget;
        for gy in 0..self.grid.1 {
            for gx in 0..self.grid.0 {
                let id = TileId {
                    x: self.origin.x + gx,
                    y: self.origin.y + gy,
                };
                let slot = (id.x % self.grid.0, id.y % self.grid.1);
                let resident = self.slots.get(&slot) == Some(&id);
                let dirty = store.take_dirty(surface, id).is_some();
                if !force && resident && !dirty {
                    continue;
                }
                if bytes_left < TILE_BYTES {
                    stats.remaining += 1;
                    continue;
                }
                let tile = match store.get(surface, id) {
                    Ok(tile) => tile,
                    Err(err) => {
                        // One bad tile shouldn't abort uploading the
                        // rest of the visible grid this frame; there is
                        // nothing more localized to retry against here.
                        // Still needs a real upload attempt later, same
                        // as a budget-skipped tile.
                        tracing::warn!(?id, %err, "skipping tile for this frame's upload");
                        stats.remaining += 1;
                        stats.errors += 1;
                        continue;
                    }
                };
                let mut bytes = Vec::with_capacity(tile.texels().len() * 2);
                for sample in tile.texels() {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: slot.0 * TILE,
                            y: slot.1 * TILE,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bytes,
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
                self.slots.insert(slot, id);
                bytes_left -= TILE_BYTES;
                stats.uploaded += 1;
                stats.bytes_uploaded += TILE_BYTES as u64;
            }
        }
        stats
    }

    /// Writes `texels` into the region of the atlas that `id`'s current
    /// slot occupies at `mip_level`, using the same toroidal slot
    /// addressing [`Self::sync`] uses. `mip_level` 0 is full resolution
    /// ([`TILE`] × [`TILE`]); each level above halves the side length.
    ///
    /// This is the GPU half of progressive rendering
    /// (`spike/FINDINGS.md` finding #3: "render a lower-resolution mip
    /// while panning fast, refining when motion stops"). The caller
    /// (`aurora-render`'s `mip::downsample`) produces the
    /// lower-resolution texels; this method lands them in the atlas at
    /// the matching mip level.
    ///
    /// Deliberately doesn't touch `slots` or consult tile
    /// dirtiness the way [`Self::sync`] does — this is a direct,
    /// caller-driven write for a resolution the caller has already
    /// decided to show, not part of the budgeted full-resolution
    /// catch-up loop. Real callers should keep using [`Self::sync`] for
    /// full-resolution (mip level 0) uploads and this only for the lower
    /// levels progressive rendering needs.
    ///
    /// # Errors
    ///
    /// Returns [`GpuError::InvalidMipLevel`] if `mip_level` is not in
    /// `0..4`, or [`GpuError::InvalidTileUpload`] if `texels`'s length
    /// doesn't match what that level's tile size expects.
    pub fn upload_mip(
        &self,
        queue: &wgpu::Queue,
        id: TileId,
        mip_level: u32,
        texels: &[f16],
    ) -> Result<(), GpuError> {
        if mip_level >= MIP_LEVELS {
            return Err(GpuError::InvalidMipLevel(mip_level));
        }
        let size = TILE >> mip_level;
        let expected = (size as usize) * (size as usize) * CHANNELS;
        if texels.len() != expected {
            return Err(GpuError::InvalidTileUpload {
                mip_level,
                expected,
                actual: texels.len(),
            });
        }

        let slot = (id.x % self.grid.0, id.y % self.grid.1);
        let mut bytes = Vec::with_capacity(texels.len() * 2);
        for sample in texels {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level,
                origin: wgpu::Origin3d {
                    x: slot.0 * size,
                    y: slot.1 * size,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// The atlas texture itself, beyond the [`Self::view`]/[`Self::sampler`]
    /// pair real drawing needs — for reading it back (`residency_test.rs`'s
    /// own pixel-readback checks, and `aurora-render`'s progressive-rendering
    /// tests, both real consumers) or copying into a different target.
    /// A real, non-test-only accessor: the atlas texture is created with
    /// `COPY_SRC` specifically so this is possible.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

impl std::fmt::Debug for TileResidency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileResidency")
            .field("grid", &self.grid)
            .field("origin", &self.origin)
            .field("resident_slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{TILE_BYTES, TileResidency};
    use crate::test_support::{real_context, real_tile_store};
    use aurora_tile::{SurfaceId, TileId};
    use half::f16;

    /// The one surface every test in this module addresses — nothing
    /// here exercises multi-surface behaviour (that's `aurora-tile`'s
    /// own job); this crate just needs *a* valid `SurfaceId` to call the
    /// store's API with.
    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    /// Paints tile `id` a known solid colour and marks it dirty, exactly
    /// as a real edit would.
    fn paint(store: &mut aurora_tile::TileStore, id: TileId, rgba: [f32; 4]) {
        let tile = match store.get_mut(surface(), id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        let samples = tile.texels_mut();
        for (i, sample) in samples.iter_mut().enumerate() {
            let Some(&channel) = rgba.get(i % 4) else {
                unreachable!("i % 4 is always in range 0..4");
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }

    #[test]
    fn visible_tiles_covers_exactly_the_grid_from_the_current_origin() {
        let Some(context) = real_context() else {
            return;
        };
        // A 256x256 viewport -> grid = (2, 2), same math
        // `toroidal_addressing_uploads_only_the_newly_exposed_column`
        // below already establishes.
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let tiles: Vec<TileId> = residency.visible_tiles().collect();
        assert_eq!(
            tiles,
            vec![
                TileId { x: 0, y: 0 },
                TileId { x: 1, y: 0 },
                TileId { x: 0, y: 1 },
                TileId { x: 1, y: 1 },
            ],
            "row-major from the origin, matching sync's own iteration order"
        );
    }

    #[test]
    fn visible_tiles_shifts_with_the_origin() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        residency.set_origin(context.queue(), TileId { x: 5, y: 3 }, (256, 256), 1.0);
        let tiles: Vec<TileId> = residency.visible_tiles().collect();
        assert_eq!(
            tiles,
            vec![
                TileId { x: 5, y: 3 },
                TileId { x: 6, y: 3 },
                TileId { x: 5, y: 4 },
                TileId { x: 6, y: 4 },
            ]
        );
    }

    #[test]
    fn toroidal_addressing_uploads_only_the_newly_exposed_column() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // A 256x256 viewport -> grid = (256/256 + 1, 256/256 + 1) = (2, 2).
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        assert_eq!(residency.grid, (2, 2));

        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 0.0, 0.0, 1.0]);
            }
        }

        // Nothing resident yet: every visible slot must upload.
        let first = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            first.uploaded, 4,
            "first sync must upload the whole visible grid"
        );
        assert_eq!(
            first.remaining, 0,
            "unlimited budget must leave nothing pending"
        );

        // Unchanged: nothing should re-upload.
        let second = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            second.uploaded, 0,
            "unchanged, resident, clean tiles must not re-upload"
        );

        // Pan by exactly one tile on the x axis. Paint the newly-visible
        // column so it has real content to upload.
        paint(&mut store, TileId { x: 2, y: 0 }, [0.0, 1.0, 0.0, 1.0]);
        paint(&mut store, TileId { x: 2, y: 1 }, [0.0, 1.0, 0.0, 1.0]);
        residency.set_origin(context.queue(), TileId { x: 1, y: 0 }, viewport, 1.0);
        let third = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            third.uploaded, 2,
            "panning by one tile must invalidate exactly one column, not the whole grid"
        );
    }

    #[test]
    fn budget_limited_sync_converges_over_multiple_calls() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // 2x2 grid, 4 tiles total, all painted up front.
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [0.0, 0.0, 1.0, 1.0]);
            }
        }

        // Budget for exactly 2 tiles' worth of bytes.
        let budget = TILE_BYTES * 2;

        let first = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(first.uploaded, 2, "budget must cap uploads to what fits");
        assert_eq!(
            first.remaining, 2,
            "the other two must be reported as still pending"
        );
        assert_eq!(first.bytes_uploaded, (TILE_BYTES * 2) as u64);
        assert_eq!(first.errors, 0);

        // Same small budget again, nothing else changed: must pick up
        // exactly the two left over, not re-touch the first two.
        let second = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(
            second.uploaded, 2,
            "second call must finish the backlog, not restart it"
        );
        assert_eq!(
            second.remaining, 0,
            "fully caught up after two budget-limited calls"
        );

        // Steady state: nothing left to do, even with the same tight budget.
        let third = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(third.uploaded, 0);
        assert_eq!(third.remaining, 0);
    }

    #[test]
    fn upload_mip_rejects_an_out_of_range_level() {
        let Some(context) = real_context() else {
            return;
        };
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let texels = vec![f16::from_f32(0.0); 4];
        match residency.upload_mip(context.queue(), TileId { x: 0, y: 0 }, 4, &texels) {
            Err(crate::GpuError::InvalidMipLevel(4)) => {}
            other => unreachable!("expected InvalidMipLevel(4), got {other:?}"),
        }
    }

    #[test]
    fn upload_mip_rejects_a_mismatched_texel_count() {
        let Some(context) = real_context() else {
            return;
        };
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        // Level 1 (Half) expects (TILE/2)^2 * 4 samples, not 4.
        let texels = vec![f16::from_f32(0.0); 4];
        match residency.upload_mip(context.queue(), TileId { x: 0, y: 0 }, 1, &texels) {
            Err(crate::GpuError::InvalidTileUpload {
                mip_level: 1,
                actual: 4,
                ..
            }) => {}
            other => unreachable!("expected InvalidTileUpload, got {other:?}"),
        }
    }

    #[test]
    fn resize_changes_the_atlas_texture_dimensions() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let before = residency.texture().size();
        assert_eq!(before.width, 512, "grid (2,2) -> 512x512 atlas");
        assert_eq!(before.height, 512);

        residency.resize(context.device(), context.queue(), (512, 512));
        let after = residency.texture().size();
        assert_eq!(
            residency.grid,
            (3, 3),
            "512.div_ceil(256) + 1 == 3 on both axes"
        );
        assert_eq!(after.width, 768, "grid (3,3) -> 768x768 atlas");
        assert_eq!(after.height, 768);
        assert_ne!(
            (before.width, before.height),
            (after.width, after.height),
            "resize must actually change the real GPU texture's dimensions"
        );
    }

    #[test]
    fn resize_resets_slots_so_a_smaller_grid_does_not_leak_stale_occupancy() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // 512x512 viewport -> grid (3, 3): slot (2, 2) is occupied.
        let mut residency = TileResidency::new(context.device(), context.queue(), (512, 512));
        assert_eq!(residency.grid, (3, 3));
        for gy in 0..3 {
            for gx in 0..3 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 1.0, 0.0, 1.0]);
            }
        }
        let first = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(first.uploaded, 9, "first sync fills the whole 3x3 grid");
        assert_eq!(residency.slots.len(), 9);

        // Shrink to a 256x256 viewport -> grid (2, 2). Slot (2, 2) from
        // the old grid is now out of bounds for the new one -- if
        // `resize` didn't clear `slots`, that stale entry would simply
        // sit unread (harmless) but any slot coordinate the old map
        // shared with the new grid (e.g. (0, 0), (1, 0), ...) would
        // wrongly read as still-resident and get skipped.
        residency.resize(context.device(), context.queue(), (256, 256));
        assert_eq!(residency.grid, (2, 2));
        assert!(
            residency.slots.is_empty(),
            "resize must reset slot occupancy, not carry over old-grid coordinates"
        );

        // Nothing marked dirty since the paint above (tiles are clean in
        // the store), but every visible slot must still upload because
        // `slots` was reset -- proves the resident check isn't trusting
        // stale bookkeeping across the resize.
        let second = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            second.uploaded, 4,
            "post-resize sync must re-upload every visible slot in the new (2,2) grid"
        );
        assert_eq!(second.errors, 0, "no panic, no wrong-tile-shown");
    }

    #[test]
    fn resize_is_a_no_op_on_a_zero_sized_request() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        residency.set_origin(context.queue(), TileId { x: 3, y: 7 }, (256, 256), 1.0);
        let before_size = residency.texture().size();
        let before_grid = residency.grid;
        let before_origin = residency.origin;

        residency.resize(context.device(), context.queue(), (0, 256));
        residency.resize(context.device(), context.queue(), (256, 0));
        residency.resize(context.device(), context.queue(), (0, 0));

        assert_eq!(
            residency.texture().size(),
            before_size,
            "a zero-sized resize request must leave the real atlas texture untouched"
        );
        assert_eq!(residency.grid, before_grid);
        assert_eq!(
            residency.origin, before_origin,
            "a no-op resize must not disturb existing origin/pan state either"
        );
    }

    #[test]
    fn resize_preserves_the_document_space_origin() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        residency.set_origin(context.queue(), TileId { x: 5, y: 9 }, (256, 256), 1.0);
        assert_eq!(residency.origin(), TileId { x: 5, y: 9 });

        residency.resize(context.device(), context.queue(), (512, 512));

        assert_eq!(
            residency.origin(),
            TileId { x: 5, y: 9 },
            "resize changes how much of the document is visible, not which part"
        );
    }
}
