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
//! [`zoomed_view::LEVEL0_FRAME_BUDGET`]); then, once they are, the rest of
//! the document is composited in the background ([`BACKGROUND_BUDGET`] of
//! synchronous recomposites, or one worker job). Changed visible tiles are
//! uploaded (all of them at or above the atlas floor, so the frame is never
//! staler than the atlas would be), the rest under [`UPLOAD_BUDGET`], and
//! their mips rebuilt on the GPU.
//!
//! **What is drawn.** At or above the atlas floor the frame draws the
//! texture only when every visible tile is uploaded at its current content
//! ([`DisplayAdvance::ready`]); otherwise it draws the atlas, unchanged.
//! Below the floor it always draws the texture: a tile not uploaded yet is
//! transparent there (the checkerboard), as a cold zoomed-out pyramid is.

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
        // 1. The visible tiles current, as the path drawing them would.
        let stale: Vec<TileId> = visible
            .iter()
            .copied()
            .filter(|&id| !cache.is_current(id))
            .collect();
        if !stale.is_empty() {
            match (&mut level0, one_to_one) {
                (Level0::Worker(worker), true) => crate::dispatch_or_composite_tiles(
                    &visible,
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
                        &stale,
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
        // 2. The rest of the document, once the view is current.
        let all: Vec<TileId> = (0..tiles.1)
            .flat_map(|y| (0..tiles.0).map(move |x| TileId { x, y }))
            .collect();
        if visible.iter().all(|&id| cache.is_current(id)) {
            let (done, more) =
                composite_rest(&all, layers, active_layer, store, cache, &mut level0);
            result.composited += done;
            result.more |= more;
        }
        // 3. Uploads: the visible tiles first (all of them at or above the
        // atlas floor), then the rest under the budget.
        let (uploaded, upload_left) = self.upload_changed(queue, store, &visible, &all, one_to_one);
        result.uploaded = uploaded;
        result.more |= upload_left;
        // 4. Their mips, on the GPU.
        let _ = self.texture.regenerate_mips(device, queue);
        result.ready = visible.iter().all(|&id| self.uploaded_current(store, id));
        result
    }

    /// Uploads the changed tiles of `visible` (every one when `one_to_one`,
    /// else under [`UPLOAD_BUDGET`]), then of `all` under the budget;
    /// returns the count and whether the budget left some.
    fn upload_changed(
        &mut self,
        queue: &wgpu::Queue,
        store: &mut aurora_tile::TileStore,
        visible: &[TileId],
        all: &[TileId],
        one_to_one: bool,
    ) -> (u32, bool) {
        let started = Instant::now();
        let mut uploaded = 0;
        for &id in visible {
            if !one_to_one && uploaded > 0 && started.elapsed() >= UPLOAD_BUDGET {
                return (uploaded, true);
            }
            if self.upload_if_changed(queue, store, id) {
                uploaded += 1;
            }
        }
        for &id in all {
            if self.uploaded_current(store, id)
                || !store.contains_tile(crate::composite_surface_id(), id)
            {
                continue;
            }
            if uploaded > 0 && started.elapsed() >= UPLOAD_BUDGET {
                return (uploaded, true);
            }
            if self.upload_if_changed(queue, store, id) {
                uploaded += 1;
            }
        }
        (uploaded, false)
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
