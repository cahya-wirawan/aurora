//! The zoomed-out view (0.275.0).
//!
//! The canvas atlas ([`aurora_gpu::TileResidency`]) is a 1:1 sliding
//! window over the composite with one tile of margin, so it cannot render
//! below about 0.94-1.0 physical zoom
//! ([`aurora_gpu::TileResidency::min_zoom_for_viewport`]). Below that floor
//! the frame draws a second atlas instead, built with twice the coverage
//! ([`aurora_gpu::TileResidency::with_coverage`]), over one level of the
//! document's **composite pyramid**:
//!
//! - level 0 is the composite surface itself (the tiles the canvas shows at
//!   100 %, composited exactly as today);
//! - level `L` tile `(x, y)` covers document pixels `[x, x + 1) * TILE *
//!   2^L` per axis and is the premultiplied 2 x 2 mean of level `L - 1`
//!   ([`aurora_render::downsample_quad`]), so level `L` is the export
//!   scaled down by a `2^L` box filter, whatever the layers hold.
//!
//! At physical zoom `z` below the floor the level is `L = floor(log2(1 /
//! z))` (at most [`MAX_LEVEL`]), shown at `z * 2^L` in `(0.5, 1]` with the
//! atlas's bilinear minification, and (task 140) blended toward the next
//! level by [`blend_weight`] (trilinear across two atlases, see
//! [`ZoomedAtlases`]), so a zoom crossing a power of two does not pop and
//! a zoom between two shows a mix of the sharper and the softer level, as
//! a GPU's own trilinear filter would. At a power of two, at the deepest
//! level and on the 1:1 path the weight is exactly zero and the picture
//! is bit-identical to the unblended one.
//!
//! **Validity** lives in [`PyramidState`], inside the
//! [`crate::CompositeCache`]: a level-`L` tile is current only when it was
//! built from four current children, and every way a level-0 tile stops
//! being current (a dab, an undo, a Move, a structural bump) drops its
//! ancestors at every level, so invalidation is exactly the composite
//! cache's own, nothing added beside it.
//!
//! **Work** is incremental and bounded per frame: the level-0 tiles under
//! the visible level-`L` tiles go through the existing composite paths
//! (the background worker on the CPU path, a [`LEVEL0_FRAME_BUDGET`] of
//! synchronous recomposites otherwise), and the downsample runs on the UI
//! thread for at most [`PYRAMID_FRAME_BUDGET`] a frame. Whatever is not
//! built yet shows what the level's tiles last held (transparent when
//! never built), and the frame asks for another.
//!
//! **Memory**: pyramid tiles live in the document's own tile store under
//! the reserved surfaces [`level_surface`] (`aurora_doc::
//! RESERVED_TOP_SURFACES`), so they share the residency budget and page out
//! like any tile; level `L` has at most `1 / 4^L` of level 0's tiles.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use aurora_tile::{SAMPLES, SurfaceId, TILE, TileId};

/// The deepest pyramid level (zoom down to `1 / 2^MAX_LEVEL`).
pub(crate) const MAX_LEVEL: u32 = 5;

/// The smallest physical zoom the canvas reaches: `1 / 2^MAX_LEVEL`, so
/// a document up to 32 viewports across fits on screen.
pub(crate) const MIN_PHYSICAL_ZOOM: f32 = 1.0 / 32.0;

/// How many viewports per axis the zoomed-out atlas covers.
pub(crate) const COVERAGE: u32 = 2;

/// The UI thread's downsample time per frame.
pub(crate) const PYRAMID_FRAME_BUDGET: Duration = Duration::from_millis(6);

/// Synchronous level-0 recomposite time per frame (GPU path, or no
/// worker), checked between chunks of [`LEVEL0_CHUNK`] tiles.
pub(crate) const LEVEL0_FRAME_BUDGET: Duration = Duration::from_millis(8);

/// Level-0 tiles recomposited per synchronous call.
pub(crate) const LEVEL0_CHUNK: usize = 8;

/// The tile-store surface holding pyramid level `level` (0 is the
/// composite surface itself).
#[must_use]
pub(crate) fn level_surface(level: u32) -> SurfaceId {
    if level == 0 {
        crate::composite_surface_id()
    } else {
        SurfaceId::from_raw(u64::MAX - u64::from(level.min(MAX_LEVEL)))
    }
}

/// The pyramid level shown at physical zoom `zoom`: the deepest level `L`
/// with `zoom * 2^L > 0.5`, at most [`MAX_LEVEL`] (`0` for a degenerate
/// zoom).
#[must_use]
pub(crate) fn level_for_zoom(zoom: f32) -> u32 {
    if !(zoom.is_finite() && zoom > 0.0) {
        return 0;
    }
    let mut level = 0;
    while level < MAX_LEVEL && zoom * level_scale(level + 1) <= 1.0 {
        level += 1;
    }
    level
}

/// [`level_for_zoom`] for a zoomed-out atlas whose floor is `floor`
/// (task 136 review, ZV-1): one level deeper while the level zoom would be
/// below the floor, so the atlas never clamps and the render path keeps
/// the zoom the pointer path divides by. With the double-coverage atlas's
/// usual floor (at most 0.5) this is [`level_for_zoom`] exactly; on a
/// canvas area too large for the device's textures the level zoom can be
/// a magnification of up to twice the floor.
#[must_use]
pub(crate) fn level_for_zoom_within(zoom: f32, floor: f32) -> u32 {
    let mut level = level_for_zoom(zoom);
    while level < MAX_LEVEL && zoom * level_scale(level) < floor {
        level += 1;
    }
    level
}

/// The 1:1 canvas atlas's floor on a device whose largest texture side is
/// `limit` (the frame draws the zoomed-out view below it).
#[must_use]
pub(crate) fn atlas_floor(canvas_px: (u32, u32), limit: u32) -> f32 {
    aurora_gpu::TileResidency::min_zoom_for_viewport_within(canvas_px, 1, limit)
}

/// The zoomed-out atlas's floor on a device whose largest texture side is
/// `limit`.
#[must_use]
pub(crate) fn zoomed_floor(canvas_px: (u32, u32), limit: u32) -> f32 {
    aurora_gpu::TileResidency::min_zoom_for_viewport_within(canvas_px, COVERAGE, limit)
}

/// The canvas's physical zoom floor: [`MIN_PHYSICAL_ZOOM`], or higher on a
/// canvas area so large that even the deepest level would have to be shown
/// below the zoomed-out atlas's floor.
#[must_use]
pub(crate) fn physical_floor(canvas_px: (u32, u32), limit: u32) -> f32 {
    MIN_PHYSICAL_ZOOM.max(zoomed_floor(canvas_px, limit) / level_scale(MAX_LEVEL))
}

/// `2^level` as `f32`.
#[must_use]
pub(crate) fn level_scale(level: u32) -> f32 {
    f32::from(1_u16 << level.min(MAX_LEVEL))
}

/// Which pyramid tiles are current, per level `1..=MAX_LEVEL` (level 0 is
/// the composite cache's own `current` set).
#[derive(Debug, Default)]
pub(crate) struct PyramidState {
    current: [HashSet<TileId>; MAX_LEVEL as usize],
    /// The canvas size the pyramid was built against (its border masking
    /// depends on it).
    canvas: (u32, u32),
}

impl PyramidState {
    /// Forgets every pyramid tile (a structural bump).
    pub(crate) fn clear(&mut self) {
        for level in &mut self.current {
            level.clear();
        }
    }

    /// A level-0 tile stopped being current: so did every tile above it.
    pub(crate) fn drop_ancestors(&mut self, id: TileId) {
        let mut id = id;
        for level in &mut self.current {
            id = TileId {
                x: id.x / 2,
                y: id.y / 2,
            };
            level.remove(&id);
        }
    }

    fn is_current(&self, level: u32, id: TileId) -> bool {
        level
            .checked_sub(1)
            .and_then(|index| self.current.get(index as usize))
            .is_some_and(|set| set.contains(&id))
    }

    fn mark_current(&mut self, level: u32, id: TileId) {
        if let Some(set) = level
            .checked_sub(1)
            .and_then(|index| self.current.get_mut(index as usize))
        {
            set.insert(id);
        }
    }

    /// Pyramid tiles currently marked current (all levels).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn current_len(&self) -> usize {
        self.current.iter().map(HashSet::len).sum()
    }
}

/// Whether level-`level` tile `id` has any texel inside a document of
/// `canvas` pixels.
#[must_use]
pub(crate) fn tile_in_document(level: u32, id: TileId, canvas: (u32, u32)) -> bool {
    let span = u64::from(TILE) << level.min(MAX_LEVEL);
    u64::from(id.x) * span < u64::from(canvas.0) && u64::from(id.y) * span < u64::from(canvas.1)
}

/// One zoomed-out frame's geometry, a pure function of the view.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ZoomedFrame {
    pub(crate) level: u32,
    /// The zoom the atlas renders the level at (`zoom * 2^level`).
    pub(crate) level_zoom: f32,
    /// The atlas's origin in level texels (never negative).
    pub(crate) level_origin: (f32, f32),
    /// Physical pixels from the canvas area's top-left corner to where the
    /// atlas's viewport starts (the document is drawn right of / below a
    /// view origin left of / above it, e.g. centred).
    pub(crate) shift_px: (u32, u32),
    /// The atlas viewport: the canvas area less the shift (at least 1).
    pub(crate) viewport_px: (u32, u32),
    /// The document rectangle in level texels.
    pub(crate) doc_clip: (f32, f32, f32, f32),
    /// The level tiles the frame shows, and the ring its bilinear filter
    /// can reach (the only tiles the zoomed-out atlas uploads).
    pub(crate) visible: Vec<TileId>,
    /// The blend toward the next level (task 140), `None` when the weight
    /// is zero ([`plan`] never sets it; [`plan_blended`] does, and
    /// [`ZoomedAtlases::prepare`] clears it while the next level is not
    /// built yet).
    pub(crate) blend: Option<Blend>,
}

/// The partner level a zoomed-out frame mixes in (task 140).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Blend {
    /// `ZoomedFrame::level + 1`.
    pub(crate) level: u32,
    /// Its weight, in `(0, 1]` ([`blend_weight`]).
    pub(crate) weight: f32,
    /// The zoom its atlas renders it at (twice the frame's level zoom).
    pub(crate) level_zoom: f32,
    /// Its atlas's origin in its own level's texels.
    pub(crate) level_origin: (f32, f32),
    /// Its tiles the frame samples, with the bilinear ring.
    pub(crate) visible: Vec<TileId>,
}

/// How much of level `level + 1` a frame showing `level` at `level_zoom`
/// mixes in (task 140): the fractional part of the level of detail,
/// `-log2(level_zoom)`, so the weight is `0` at a power of two (level zoom
/// exactly `1`) and tends to `1` as the level zoom tends to `0.5`, where
/// the next level takes over at its own level zoom `1`. Level 0 ramps from
/// the 1:1 atlas's floor `one_floor` instead of from `1`, so the hand-over
/// from the 1:1 atlas does not jump either. Zero at [`MAX_LEVEL`] (there is
/// no deeper level), for a magnified level (a capped atlas, ZV-1), and for
/// anything non-finite.
#[must_use]
pub(crate) fn blend_weight(level: u32, level_zoom: f32, one_floor: f32) -> f32 {
    if level >= MAX_LEVEL || !(level_zoom.is_finite() && level_zoom > 0.0) {
        return 0.0;
    }
    let lod = -level_zoom.log2();
    let weight = if level == 0 {
        let start = if one_floor.is_finite() && one_floor > 0.0 {
            (-one_floor.log2()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if start >= 1.0 {
            return 0.0;
        }
        (lod - start) / (1.0 - start)
    } else {
        lod
    };
    if weight > 0.0 { weight.min(1.0) } else { 0.0 }
}

/// The frame for a view whose top-left canvas pixel is document point
/// `view_origin` at physical zoom `zoom`, in a canvas area of `canvas_px`
/// physical pixels, over a document of `doc_size` pixels, for a
/// zoomed-out atlas whose floor is `floor` ([`zoomed_floor`]).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn plan(
    view_origin: (f32, f32),
    zoom: f32,
    canvas_px: (u32, u32),
    doc_size: (u32, u32),
    floor: f32,
) -> ZoomedFrame {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    plan_at(
        view_origin,
        zoom,
        canvas_px,
        doc_size,
        level_for_zoom_within(zoom, floor),
    )
}

/// [`plan`] with the blend toward the next level filled in (task 140):
/// `one_floor` is the 1:1 atlas's floor ([`atlas_floor`]), where level 0's
/// weight starts.
#[must_use]
pub(crate) fn plan_blended(
    view_origin: (f32, f32),
    zoom: f32,
    canvas_px: (u32, u32),
    doc_size: (u32, u32),
    floor: f32,
    one_floor: f32,
) -> ZoomedFrame {
    let mut frame = plan(view_origin, zoom, canvas_px, doc_size, floor);
    let weight = blend_weight(frame.level, frame.level_zoom, one_floor);
    // Below one 8-bit step the next level is neither built nor drawn.
    if weight >= BLEND_EPSILON {
        let zoom = frame.level_zoom / level_scale(frame.level);
        let next = plan_at(view_origin, zoom, canvas_px, doc_size, frame.level + 1);
        frame.blend = Some(Blend {
            level: next.level,
            weight,
            level_zoom: next.level_zoom,
            level_origin: next.level_origin,
            visible: next.visible,
        });
    }
    frame
}

/// [`plan`] at a given `level` (a valid `zoom`).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn plan_at(
    view_origin: (f32, f32),
    zoom: f32,
    canvas_px: (u32, u32),
    doc_size: (u32, u32),
    level: u32,
) -> ZoomedFrame {
    let level = level.min(MAX_LEVEL);
    let scale = level_scale(level);
    let axis = |origin: f32, canvas: u32| {
        let origin = if origin.is_finite() { origin } else { 0.0 };
        let shift = ((-origin).max(0.0) * zoom)
            .round()
            .min(canvas.saturating_sub(1) as f32);
        let shift = shift as u32;
        (origin.max(0.0), shift, canvas.saturating_sub(shift).max(1))
    };
    let (ox, sx, vx) = axis(view_origin.0, canvas_px.0);
    let (oy, sy, vy) = axis(view_origin.1, canvas_px.1);
    let span = TILE as f32 * scale;
    let tiles = |origin: f32, viewport: u32, doc: u32| -> std::ops::Range<u32> {
        let end = (origin + viewport as f32 / zoom).min(doc as f32);
        if end <= origin {
            return 0..0;
        }
        let first = (origin / span).floor() as u32;
        // One level texel further: the bilinear filter at the last
        // fragment can reach it, so its tile must be uploaded (blank when
        // it is past the document) and not hold another level's texels.
        let last = ((end + scale) / span).ceil() as u32;
        first..last.max(first)
    };
    let xs = tiles(ox, vx, doc_size.0);
    let ys = tiles(oy, vy, doc_size.1);
    let visible = ys
        .flat_map(|y| xs.clone().map(move |x| TileId { x, y }))
        .collect();
    ZoomedFrame {
        level,
        level_zoom: zoom * scale,
        level_origin: (ox / scale, oy / scale),
        shift_px: (sx, sy),
        viewport_px: (vx, vy),
        doc_clip: (
            0.0,
            0.0,
            doc_size.0 as f32 / scale,
            doc_size.1 as f32 / scale,
        ),
        visible,
        blend: None,
    }
}

/// The logical pan that centres a document of `doc` pixels on any axis
/// where it is narrower than the `canvas` logical area at `zoom`, with the
/// position on whole physical pixels; `pan` on the other axes.
#[must_use]
pub(crate) fn centred_pan(
    pan: (f32, f32),
    zoom: f32,
    canvas: (f32, f32),
    doc: (u32, u32),
    scale_factor: f32,
) -> (f32, f32) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    #[allow(clippy::cast_precision_loss)]
    let axis = |pan: f32, canvas: f32, doc: u32| {
        let extent = doc as f32 * zoom;
        if extent.is_finite() && canvas.is_finite() && extent <= canvas {
            ((canvas - extent) / 2.0 * scale).round() / scale
        } else {
            pan
        }
    };
    (axis(pan.0, canvas.0, doc.0), axis(pan.1, canvas.1, doc.1))
}

/// The logical zoom that fits a document of `doc` pixels in a `canvas`
/// logical area, or `None` for an empty one.
#[must_use]
pub(crate) fn fit_zoom(canvas: (f32, f32), doc: (u32, u32)) -> Option<f32> {
    if doc.0 == 0 || doc.1 == 0 || !(canvas.0 > 0.0 && canvas.1 > 0.0) {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let zoom = (canvas.0 / doc.0 as f32).min(canvas.1 / doc.1 as f32);
    zoom.is_finite().then_some(zoom)
}

/// The zoom the canvas really draws a physical `zoom` at in a canvas area
/// of `canvas_px`: the 1:1 atlas's at or above its floor, the zoomed-out
/// atlas's (its level zoom, back in document scale) below it. The test
/// oracle for "the render path and the pointer path agree".
#[cfg(test)]
#[must_use]
pub(crate) fn rendered_canvas_zoom(canvas_px: (u32, u32), zoom: f32) -> f32 {
    rendered_canvas_zoom_within(canvas_px, zoom, u32::MAX)
}

/// [`rendered_canvas_zoom`] on a device whose largest texture side is
/// `limit`: both atlases capped to it, as `TileResidency::with_coverage`
/// caps them.
#[cfg(test)]
#[must_use]
pub(crate) fn rendered_canvas_zoom_within(canvas_px: (u32, u32), zoom: f32, limit: u32) -> f32 {
    let one = atlas_floor(canvas_px, limit);
    if zoom >= one || !zoom.is_finite() || zoom <= 0.0 {
        return if zoom.is_finite() && zoom > 0.0 {
            zoom.max(one)
        } else {
            one.max(1.0)
        };
    }
    let floor = zoomed_floor(canvas_px, limit);
    let scale = level_scale(level_for_zoom_within(zoom, floor));
    (zoom * scale).max(floor) / scale
}

/// Palette and menu id of View > Fit on Screen (`Primary+0`).
pub(crate) const COMMAND_FIT_ON_SCREEN: &str = "view.fit-on-screen";
/// Its title.
pub(crate) const FIT_ON_SCREEN_TITLE: &str = "Fit on Screen";
/// Palette and menu id of View > 100% (`Primary+1`): one image pixel per
/// physical pixel, Photoshop's physical 100 % (0.163.0's status bar zoom).
pub(crate) const COMMAND_ACTUAL_PIXELS: &str = "view.actual-pixels";
/// Its title.
pub(crate) const ACTUAL_PIXELS_TITLE: &str = "100%";

/// A view zoom command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewZoom {
    /// Fit the whole document in the canvas area, centred.
    Fit,
    /// 100 % physical, about the canvas area's centre.
    ActualPixels,
}

/// Applies `command` to `view` for a document of `doc` pixels in a canvas
/// area of `canvas` logical pixels at `scale_factor`. The zoom is clamped
/// to the view's own floor and ceiling; Fit centres the document (on an
/// axis the redraw's pan bounds then pin, when the fitted zoom is at or
/// above the 1:1 atlas floor, the document stays at its corner).
pub(crate) fn apply_view_zoom(
    view: &mut aurora_ui::CanvasView,
    command: ViewZoom,
    canvas: (f32, f32),
    doc: (u32, u32),
    scale_factor: f32,
) {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    match command {
        ViewZoom::Fit => {
            let Some(zoom) = fit_zoom(canvas, doc) else {
                return;
            };
            view.zoom_at((0.0, 0.0), zoom);
            let pan = centred_pan(view.pan(), view.zoom(), canvas, doc, scale);
            let now = view.pan();
            view.pan_by((pan.0 - now.0, pan.1 - now.1));
        }
        ViewZoom::ActualPixels => {
            view.zoom_at((canvas.0 / 2.0, canvas.1 / 2.0), 1.0 / scale);
        }
    }
}

/// What one [`advance`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Advance {
    /// Level-0 tiles recomposited synchronously this frame.
    pub(crate) composited: usize,
    /// Pyramid tiles built this frame.
    pub(crate) built: usize,
    /// Every visible tile is current.
    pub(crate) complete: bool,
    /// Work the UI thread can still do on a later frame (not counting a
    /// busy worker, whose install wakes the loop itself).
    pub(crate) more: bool,
}

/// How [`advance`] composites the level-0 tiles it needs.
pub(crate) enum Level0<'a> {
    /// The background worker (the CPU path, as the frame does at 100 %).
    Worker(&'a mut crate::background_composite::CompositeWorker),
    /// Synchronous recomposites under [`LEVEL0_FRAME_BUDGET`], on the GPU
    /// when given one (the GPU path, as the frame does at 100 %).
    Sync(
        Option<&'a aurora_gpu::GpuContext>,
        Option<&'a mut aurora_render::TileCompositor>,
    ),
}

impl Level0<'_> {
    /// The same way, borrowed again (a frame advances two levels).
    pub(crate) fn reborrow(&mut self) -> Level0<'_> {
        match self {
            Level0::Worker(worker) => Level0::Worker(worker),
            Level0::Sync(gpu, compositor) => Level0::Sync(*gpu, compositor.as_deref_mut()),
        }
    }
}

/// What an atlas's slots were filled for: (composite cache id, canvas
/// size, level). Any change forgets them.
pub(crate) type AtlasKey = (u64, (u32, u32), u32);

/// The smallest blend weight drawn (task 140 review): below it the next
/// level's contribution is under one 8-bit step, so the frame neither
/// builds nor uploads it.
pub(crate) const BLEND_EPSILON: f32 = 1.0 / 256.0;

/// Frames over which a newly live blend fades in (task 140 review): the
/// first frame a partner becomes complete for its key draws `1 / N` of
/// the weight, the `N`th all of it, so a cold build's switch-on is a ramp
/// of about 100 ms at 60 FPS rather than one step.
pub(crate) const BLEND_FADE_FRAMES: u32 = 6;

/// One zoomed-out atlas and what its slots hold.
struct Atlas {
    residency: aurora_gpu::TileResidency,
    /// What its slots were filled for (`None`: nothing yet).
    key: Option<AtlasKey>,
    /// It has held a complete picture of `key` since the key was set: its
    /// slots may be stale in places (a dab, a pan), but show what those
    /// tiles last held, as the frame's own level does.
    complete: bool,
    /// Frames drawn as a live partner since `complete` was set, at most
    /// [`BLEND_FADE_FRAMES`].
    shown: u32,
}

impl Atlas {
    fn new(residency: aurora_gpu::TileResidency) -> Self {
        Self {
            residency,
            key: None,
            complete: false,
            shown: 0,
        }
    }

    /// Hands the atlas `key`: a different key (or `forget`) forgets its
    /// slots and its latch.
    fn key_to(&mut self, key: AtlasKey, forget: bool) {
        if forget || self.key != Some(key) {
            self.reset(Some(key));
        }
    }

    /// Forgets the slots and the latch, keyed to `key`.
    fn reset(&mut self, key: Option<AtlasKey>) {
        self.residency.forget_slots();
        self.key = key;
        self.complete = false;
        self.shown = 0;
    }
}

/// The zoomed-out view's two atlases (task 140): the frame's level and the
/// next one, which the canvas shader mixes by [`Blend::weight`]. Both are
/// double-coverage atlases, so they swap roles when the level changes: a
/// zoom out past a power of two finds its new level already uploaded in
/// the partner. The partner is built on the first frame that blends.
///
/// **Slot validity.** Each atlas consumes the dirty flags of the level it
/// is keyed to, so at most one atlas is ever keyed to a level (a key is
/// handed over by a swap, never copied), and `forget` (the 1:1 atlas drew
/// last and consumed level 0's) forgets both.
///
/// **The latch** (task 140 review). Once the partner has held a complete
/// next level for its key it keeps being drawn, whatever its tiles are
/// doing: a dab or a pan leaves some of them not current for a frame or
/// more, and they show what they last held (transparent when never built),
/// exactly as the frame's own level does. Only a new key, `forget` or a
/// cold partner turns the blend off, and a cold partner's first complete
/// picture fades in over [`BLEND_FADE_FRAMES`].
pub(crate) struct ZoomedAtlases {
    size: (u32, u32),
    primary: Atlas,
    partner: Option<Atlas>,
}

/// One frame's document side for [`ZoomedAtlases::prepare`].
pub(crate) struct FrameDocument<'a> {
    /// The composite cache's id (part of every [`AtlasKey`]).
    pub(crate) cache_id: u64,
    pub(crate) canvas: (u32, u32),
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) active_layer: Option<aurora_doc::LayerId>,
    pub(crate) store: &'a mut aurora_tile::TileStore,
    pub(crate) cache: &'a mut crate::CompositeCache,
}

impl ZoomedAtlases {
    /// The primary atlas for a canvas area of `canvas_px`; the partner is
    /// built when first needed.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, canvas_px: (u32, u32)) -> Self {
        Self {
            size: canvas_px,
            primary: Atlas::new(aurora_gpu::TileResidency::with_coverage(
                device, queue, canvas_px, COVERAGE,
            )),
            partner: None,
        }
    }

    /// The canvas area the atlases were sized for.
    #[must_use]
    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The atlas showing the frame's level.
    #[must_use]
    pub(crate) fn primary(&self) -> &aurora_gpu::TileResidency {
        &self.primary.residency
    }

    /// The atlas holding the next level, once built.
    #[must_use]
    pub(crate) fn partner(&self) -> Option<&aurora_gpu::TileResidency> {
        self.partner.as_ref().map(|atlas| &atlas.residency)
    }

    /// The keys the two atlases hold (primary, partner).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn keys(&self) -> (Option<AtlasKey>, Option<AtlasKey>) {
        (
            self.primary.key,
            self.partner.as_ref().and_then(|atlas| atlas.key),
        )
    }

    /// The bind group for `frame`: both atlases when it blends.
    #[must_use]
    pub(crate) fn bind_group(
        &self,
        canvas: &aurora_gpu::CanvasPipeline,
        device: &wgpu::Device,
        frame: &ZoomedFrame,
    ) -> wgpu::BindGroup {
        match (frame.blend.as_ref(), self.partner()) {
            (Some(_), Some(partner)) => canvas.bind_group_blended(device, self.primary(), partner),
            _ => canvas.bind_group(device, self.primary()),
        }
    }

    /// The partner atlas, built (unkeyed) when there is none yet.
    fn ensure_partner(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &mut Atlas {
        let size = self.size;
        self.partner.get_or_insert_with(|| {
            Atlas::new(aurora_gpu::TileResidency::with_coverage(
                device, queue, size, COVERAGE,
            ))
        })
    }

    /// Prepares one zoomed-out frame: hands the atlases their levels
    /// (swapping them when that saves an upload), advances the pyramid for
    /// the frame's level and uploads it, and, once that level is complete,
    /// advances the next level too; the next level's dirty tiles are
    /// uploaded every blended frame. The blend is drawn while the partner
    /// is latched complete for its key (see the type's doc), faded in over
    /// [`BLEND_FADE_FRAMES`] the first time; before that `frame.blend` is
    /// cleared and the frame shows its level alone, exactly as before task
    /// 140 (the checkerboard of a cold build included). Returns the frame
    /// level's [`Advance`], with `more` also set while the next level has
    /// work left or the blend is still fading in.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &mut ZoomedFrame,
        doc: FrameDocument<'_>,
        forget: bool,
        gain: [f32; 3],
        mut level0: Level0<'_>,
        budget: Duration,
    ) -> Advance {
        let key = |level: u32| (doc.cache_id, doc.canvas, level);
        let want_primary = key(frame.level);
        let want_partner = frame.blend.as_ref().map(|blend| key(blend.level));
        if want_partner.is_some() {
            // Before the swap: a zoom in past a power of two hands the
            // level just shown over as the partner even on the first frame
            // that blends.
            self.ensure_partner(device, queue);
        }
        if let Some(partner) = self.partner.as_mut() {
            let swap = (self.primary.key != Some(want_primary)
                && partner.key == Some(want_primary))
                || (want_partner.is_some()
                    && partner.key != want_partner
                    && self.primary.key == want_partner);
            if swap {
                std::mem::swap(&mut self.primary, partner);
            }
            if forget {
                partner.reset(None);
            }
        }
        self.primary.key_to(want_primary, forget);
        let advance_doc =
            |level: u32, visible: &[TileId], doc: &mut FrameDocument<'_>, level0: Level0<'_>| {
                advance(
                    level,
                    visible,
                    doc.canvas,
                    doc.layers,
                    doc.active_layer,
                    doc.store,
                    doc.cache,
                    level0,
                    budget,
                )
            };
        let mut doc = doc;
        let mut result = advance_doc(frame.level, &frame.visible, &mut doc, level0.reborrow());
        if result.complete {
            // Drawn complete as the frame's own level: on screen already,
            // so when a zoom in hands it over as the partner it is drawn
            // at its full weight at once, not faded in again (task 140
            // re-review). Only a partner that was cold when keyed fades.
            self.primary.complete = true;
            self.primary.shown = BLEND_FADE_FRAMES;
        }
        let mut weight = 0.0;
        if let (Some(blend), Some(want_partner)) = (frame.blend.as_ref(), want_partner) {
            let atlas = self.ensure_partner(device, queue);
            atlas.key_to(want_partner, false);
            atlas.residency.set_origin(
                queue,
                blend.level_origin,
                frame.viewport_px,
                blend.level_zoom,
            );
            if result.complete {
                let next = advance_doc(blend.level, &blend.visible, &mut doc, level0.reborrow());
                result.more |= next.more;
                if next.complete {
                    atlas.complete = true;
                }
            }
            if atlas.complete {
                let _ = atlas.residency.sync_only(
                    queue,
                    doc.store,
                    level_surface(blend.level),
                    usize::MAX,
                    &blend.visible,
                );
                atlas.shown = (atlas.shown + 1).min(BLEND_FADE_FRAMES);
                if atlas.shown < BLEND_FADE_FRAMES {
                    result.more = true;
                }
                #[allow(clippy::cast_precision_loss)]
                let fade = atlas.shown as f32 / BLEND_FADE_FRAMES as f32;
                weight = if atlas.shown >= BLEND_FADE_FRAMES {
                    blend.weight
                } else {
                    blend.weight * fade
                };
            }
        }
        if weight <= 0.0 {
            frame.blend = None;
        }
        let atlas = &mut self.primary.residency;
        atlas.set_channel_gain(gain);
        atlas.set_document_clip(Some(frame.doc_clip));
        atlas.set_blend_weight(weight);
        atlas.set_origin(
            queue,
            frame.level_origin,
            frame.viewport_px,
            frame.level_zoom,
        );
        let _ = atlas.sync_only(
            queue,
            doc.store,
            level_surface(frame.level),
            usize::MAX,
            &frame.visible,
        );
        result
    }
}

/// The tiles at every level the visible `level` tiles need, top level
/// first: each non-current tile's in-document children are needed one
/// level down.
fn needed(
    level: u32,
    visible: &[TileId],
    canvas: (u32, u32),
    cache: &crate::CompositeCache,
) -> Vec<Vec<TileId>> {
    let mut out: Vec<Vec<TileId>> = Vec::new();
    let mut wanted: Vec<TileId> = visible
        .iter()
        .copied()
        .filter(|&id| tile_in_document(level, id, canvas))
        .collect();
    for l in (0..=level).rev() {
        let mut below = Vec::new();
        if l > 0 {
            for &id in &wanted {
                if cache.pyramid.is_current(l, id) {
                    continue;
                }
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let child = TileId {
                        x: id.x.saturating_mul(2).saturating_add(dx),
                        y: id.y.saturating_mul(2).saturating_add(dy),
                    };
                    if tile_in_document(l - 1, child, canvas) {
                        below.push(child);
                    }
                }
            }
        }
        out.push(std::mem::replace(&mut wanted, below));
    }
    out
}

/// Whether tile `id` of `level` is current (level 0 in the composite
/// cache, the others in the pyramid).
fn current_at(cache: &crate::CompositeCache, level: u32, id: TileId) -> bool {
    if level == 0 {
        cache.is_current(id)
    } else {
        cache.pyramid.is_current(level, id)
    }
}

/// Recomposites `tiles` in chunks of [`LEVEL0_CHUNK`] until `budget` is
/// spent (always at least one chunk, so a frame always progresses);
/// returns the tiles done and whether any were left.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recomposite_within_budget(
    tiles: &[TileId],
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    store: &mut aurora_tile::TileStore,
    cache: &mut crate::CompositeCache,
    gpu: Option<&aurora_gpu::GpuContext>,
    mut compositor: Option<&mut aurora_render::TileCompositor>,
    budget: Duration,
) -> (usize, bool) {
    let started = Instant::now();
    let mut done = 0;
    for chunk in tiles.chunks(LEVEL0_CHUNK) {
        if done > 0 && started.elapsed() >= budget {
            return (done, true);
        }
        crate::recomposite_tiles(
            chunk,
            layers,
            active_layer,
            store,
            cache,
            gpu,
            compositor.as_deref_mut(),
        );
        done += chunk.len();
    }
    (done, false)
}

#[cfg(test)]
thread_local! {
    /// Test hook (ZV-4): makes every pyramid tile write fail, as a store
    /// that cannot page a tile in would.
    pub(crate) static FAIL_PYRAMID_WRITES: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Builds level-`level` tile `id` from its children and marks it current;
/// whether it did (a store error leaves it as it was, not current).
fn build_tile(
    store: &mut aurora_tile::TileStore,
    cache: &mut crate::CompositeCache,
    level: u32,
    id: TileId,
    canvas: (u32, u32),
) -> bool {
    let child_surface = level_surface(level - 1);
    let mut children: [Option<std::sync::Arc<Vec<half::f16>>>; 4] = [None, None, None, None];
    let mut owned: [Option<Vec<half::f16>>; 4] = [None, None, None, None];
    for (index, (dx, dy)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        let child = TileId {
            x: id.x.saturating_mul(2).saturating_add(dx),
            y: id.y.saturating_mul(2).saturating_add(dy),
        };
        if !tile_in_document(level - 1, child, canvas) {
            continue;
        }
        if let Some(shared) = store.peek_shared(child_surface, child) {
            if let Some(slot) = children.get_mut(index) {
                *slot = Some(shared);
            }
        } else if let Ok(tile) = store.get(child_surface, child)
            && let Some(slot) = owned.get_mut(index)
        {
            *slot = Some(tile.texels().to_vec());
        }
    }
    let slice = |index: usize| -> Option<&[half::f16]> {
        children
            .get(index)
            .and_then(|c| c.as_deref().map(Vec::as_slice))
            .or_else(|| owned.get(index).and_then(|o| o.as_deref()))
    };
    // Input-level texels inside the document from this block's origin.
    let child_extent = |extent: u32| {
        let scale = 1_u64 << (level - 1).min(MAX_LEVEL);
        u64::from(extent).div_ceil(scale)
    };
    let origin = |coordinate: u32| u64::from(coordinate) * 2 * u64::from(TILE);
    let valid = (
        u32::try_from(child_extent(canvas.0).saturating_sub(origin(id.x))).unwrap_or(u32::MAX),
        u32::try_from(child_extent(canvas.1).saturating_sub(origin(id.y))).unwrap_or(u32::MAX),
    );
    let mut out = crate::tile_buffers::zeroed();
    if out.len() != SAMPLES {
        out = vec![half::f16::ZERO; SAMPLES];
    }
    aurora_render::downsample_quad([slice(0), slice(1), slice(2), slice(3)], valid, &mut out);
    let full = aurora_core::Rect {
        x: 0,
        y: 0,
        width: TILE,
        height: TILE,
    };
    #[cfg(test)]
    let refused = FAIL_PYRAMID_WRITES.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let refused = false;
    let mut written = false;
    if !refused
        && let Ok(dest) = store.get_mut(level_surface(level), id)
        && dest.texels().len() == out.len()
    {
        dest.texels_mut().copy_from_slice(&out);
        dest.mark_dirty(full);
        cache.pyramid.mark_current(level, id);
        written = true;
    }
    crate::tile_buffers::recycle(out);
    written
}

/// One frame's worth of pyramid work for showing `visible` tiles of
/// `level`: level-0 composites through `level0`, then downsamples bottom
/// up under [`PYRAMID_FRAME_BUDGET`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn advance(
    level: u32,
    visible: &[TileId],
    canvas: (u32, u32),
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    store: &mut aurora_tile::TileStore,
    cache: &mut crate::CompositeCache,
    level0: Level0<'_>,
    budget: Duration,
) -> Advance {
    let mut result = Advance::default();
    if cache.pyramid.canvas != canvas {
        cache.pyramid.clear();
        cache.pyramid.canvas = canvas;
    }
    let need = needed(level, visible, canvas, cache);
    let level0_tiles: Vec<TileId> = need
        .last()
        .map(|tiles| {
            let mut seen = HashSet::new();
            tiles
                .iter()
                .copied()
                .filter(|&id| !cache.is_current(id) && seen.insert(id))
                .collect()
        })
        .unwrap_or_default();
    if !level0_tiles.is_empty() {
        match level0 {
            Level0::Worker(worker) => {
                if crate::dispatch_or_composite_tiles_bounded(
                    &level0_tiles,
                    layers,
                    active_layer,
                    store,
                    cache,
                    worker,
                    Some(LEVEL0_FRAME_BUDGET),
                ) {
                    result.more = true;
                }
            }
            Level0::Sync(gpu, compositor) => {
                let (done, more) = recomposite_within_budget(
                    &level0_tiles,
                    layers,
                    active_layer,
                    store,
                    cache,
                    gpu,
                    compositor,
                    LEVEL0_FRAME_BUDGET,
                );
                result.composited += done;
                result.more |= more;
            }
        }
    }
    let started = Instant::now();
    'levels: for l in 1..=level {
        let index = (level - l) as usize;
        let Some(tiles) = need.get(index) else {
            continue;
        };
        let mut seen = HashSet::new();
        for &id in tiles {
            if !seen.insert(id) || cache.pyramid.is_current(l, id) {
                continue;
            }
            let ready = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .into_iter()
                .all(|(dx, dy)| {
                    let child = TileId {
                        x: id.x.saturating_mul(2).saturating_add(dx),
                        y: id.y.saturating_mul(2).saturating_add(dy),
                    };
                    !tile_in_document(l - 1, child, canvas) || current_at(cache, l - 1, child)
                });
            if !ready {
                continue;
            }
            if started.elapsed() >= budget {
                result.more = true;
                break 'levels;
            }
            // Only a written tile counts (ZV-4): a store that keeps failing
            // must not keep the loop redrawing.
            if build_tile(store, cache, l, id, canvas) {
                result.built += 1;
            }
        }
    }
    result.complete = visible
        .iter()
        .filter(|&&id| tile_in_document(level, id, canvas))
        .all(|&id| current_at(cache, level, id));
    if !result.complete && result.built > 0 {
        result.more = true;
    }
    result
}

/// Forgets every pyramid surface's tiles in `store` (a document's pixels
/// replaced in place).
pub(crate) fn forget_pyramid_surfaces(store: &mut aurora_tile::TileStore) -> usize {
    (1..=MAX_LEVEL)
        .map(|level| store.forget_surface(level_surface(level)))
        .sum()
}

#[cfg(test)]
#[path = "zoomed_view_tests.rs"]
mod tests;
