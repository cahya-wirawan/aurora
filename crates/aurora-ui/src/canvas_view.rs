//! The canvas view transform: pan and zoom between document-pixel space
//! and the canvas area's own logical screen space. PLAN.md M1.8's
//! "Canvas: infinite zoom, rotation, pan, rulers, guides, grid, snap"
//! bullet's first slice — deliberately just pan and zoom, no rotation,
//! rulers, guides, grid, or snap yet, and (matching [`crate::workspace`]'s
//! own "no pixel rendering" boundary) no actual GPU drawing: this is the
//! coordinate transform PLAN.md M1.9's "basic tools" bullet needs to turn
//! a pointer position into a document-space one, built and tested ahead
//! of the GPU-rendering work that will eventually read the same
//! transform to know what to draw.

/// Zoom at which one document pixel occupies exactly one logical screen
/// pixel ("100%") — [`CanvasView::new`]'s own starting zoom.
pub const DEFAULT_ZOOM: f32 = 1.0;

/// Practical zoom bounds — not the literal "infinite" this bullet is
/// named for. A real infinite zoom needs bounds tied to the document's
/// own resolution and the GPU renderer's mip chain (`aurora-render`'s
/// already-built progressive/mip rendering, M1.3), neither of which this
/// crate has a document to read yet. These are round, practical limits
/// (professional raster editors cap out in the same neighbourhood) until
/// that real basis exists — reconsider once rendering is wired in.
pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 64.0;

/// Maps between document-pixel space and the canvas area's own logical
/// screen space: `to_screen(doc) = doc * zoom + pan`.
///
/// `pan` is the screen position (in the canvas area's own logical
/// pixels, i.e. relative to its own top-left, not the window's) that
/// document-space `(0, 0)` currently renders at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasView {
    pan: (f32, f32),
    zoom: f32,
    /// The lower bound [`Self::zoom_at`] clamps to, on top of
    /// [`MIN_ZOOM`] — see [`Self::set_min_zoom`]. [`MIN_ZOOM`] until a
    /// caller sets it, so a `CanvasView` built by anything that doesn't
    /// know about a renderer behaves exactly as it always did.
    min_zoom: f32,
}

impl CanvasView {
    /// A fresh view: 100% zoom, document `(0, 0)` at the canvas area's
    /// own top-left corner.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: DEFAULT_ZOOM,
            min_zoom: MIN_ZOOM,
        }
    }

    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.zoom
    }

    #[must_use]
    pub const fn pan(&self) -> (f32, f32) {
        self.pan
    }

    /// The current effective lower zoom bound — [`MIN_ZOOM`], or
    /// whatever larger floor [`Self::set_min_zoom`] was last given.
    #[must_use]
    pub fn min_zoom(&self) -> f32 {
        self.min_zoom.max(MIN_ZOOM)
    }

    /// Raises this view's own lower zoom bound to `min_zoom`, clamping
    /// the current zoom up to it immediately if it is already below.
    ///
    /// This is [`Self::clamp_pan_to_minimum`]'s counterpart on the zoom
    /// axis, and it exists for the same reason, against the same class
    /// of bug. The renderer that draws this view
    /// (`aurora_gpu::TileResidency`) cannot honour an arbitrarily small
    /// zoom: its atlas is sized from the viewport and holds a fixed
    /// amount of document, so below
    /// `TileResidency::min_zoom_for_viewport` it renders at that floor
    /// rather than shrinking further. If this view kept the smaller
    /// number, [`Self::to_document`] — which is what turns a click into
    /// the document position a brush dab lands on — would divide by a
    /// zoom nothing on screen was drawn at, and paint would land
    /// somewhere other than the pixel under the cursor (measured at
    /// canvas zoom 0.25 on a 1920 px viewport: a click at screen
    /// x = 960 converting to document x ≈ 3840 while the pixel drawn
    /// there was document x ≈ 1152; ~83× apart at [`MIN_ZOOM`]).
    ///
    /// So the bound is applied *here*, at the source, exactly as
    /// [`Self::clamp_pan_to_minimum`] applies the pan bound: the view
    /// cannot hold an unrenderable zoom in the first place, so every
    /// downstream consumer — render and paint alike — reads the same
    /// already-bounded number and they cannot diverge. `aurora-app`
    /// keeps this in step with the atlas (its own `canvas_min_zoom`,
    /// derived from `TileResidency::min_zoom_for_viewport`, called
    /// whenever the canvas area's size or the window's scale factor can
    /// have changed); this crate takes it as a plain parameter rather
    /// than reaching for a renderer, the same way
    /// [`Self::clamp_pan_to_minimum`] takes `min_doc` rather than
    /// reading a document.
    ///
    /// A non-finite or non-positive `min_zoom` is ignored (falls back to
    /// [`MIN_ZOOM`]), and the bound is itself capped at [`MAX_ZOOM`] so
    /// a nonsense floor can never invert the clamp in
    /// [`Self::zoom_at`].
    ///
    /// **That cap is a boundary on the guarantee, stated rather than
    /// hidden.** A floor above [`MAX_ZOOM`] is a floor this view cannot
    /// hold, so the render/paint agreement above does *not* extend past
    /// it: the view will sit at [`MAX_ZOOM`] while its renderer clamps
    /// to something larger, and a click can land off the pixel under it
    /// again. Inverting the clamp instead would be worse (`zoom_at`'s
    /// `clamp(min, MAX_ZOOM)` panics when `min > max`), and refusing the
    /// floor outright would be worse still (no bound at all). The only
    /// way to reach it is an absurd viewport or a misconfigured display
    /// scale factor — `WINIT_X11_SCALE_FACTOR`/`Xft.dpi` set to
    /// something enormous — since the derived floor is at most 1.0 per
    /// physical viewport pixel before that division. So it is logged at
    /// `error` rather than silently absorbed: it is an environment
    /// problem the user can actually fix, and it should not present as
    /// an unexplained paint offset. Making it correct instead of merely
    /// diagnosed needs the atlas sized from zoom (M1.3), the same work
    /// the floor itself is waiting on.
    ///
    /// Raising the zoom goes through [`Self::zoom_at`] anchored at the
    /// canvas area's own top-left corner, which is **not** a detail:
    /// changing `zoom` while leaving `pan` alone moves
    /// `to_document((0, 0))`, and moving it *toward* the origin can push
    /// it back past the boundary [`Self::clamp_pan_to_minimum`] has
    /// already pinned it to — which would reopen the pan-axis version of
    /// exactly the divergence this method exists to close (a layer whose
    /// own origin is not `(0, 0)` is where that bites). Anchoring at
    /// `(0, 0)` holds `to_document((0, 0))` fixed across the raise, so a
    /// view that satisfied the pan bound before still satisfies it
    /// after.
    ///
    /// **This method moves the view**, despite reading like a plain
    /// bounds setter, and a caller holding anything anchored to the old
    /// one has to deal with that (0.57.8). Only `to_document((0, 0))`
    /// is held fixed: for every other point the raise from `z0` to `z1`
    /// shifts `to_document(x)` by `x * (1/z1 - 1/z0)` — zero at the
    /// anchor, growing linearly away from it, so it is *not* one
    /// uniform shift the way a pure pan change would be. `aurora-app`
    /// carries the live-drag case (a stroke held while the window is
    /// resized), where a document-space reference point fixed at the
    /// start of a drag would otherwise be measured against a view that
    /// moved under it; this crate has no drag to re-anchor and
    /// deliberately keeps none.
    pub fn set_min_zoom(&mut self, min_zoom: f32) {
        if min_zoom.is_finite() && min_zoom > MAX_ZOOM {
            tracing::error!(
                requested = min_zoom,
                cap = MAX_ZOOM,
                "the canvas zoom floor exceeds the maximum zoom and is being \
                 capped; the display scale factor or canvas size this was \
                 derived from is out of any sane range, and while capped, \
                 what is painted can land off the pixel under the cursor"
            );
        }
        self.min_zoom = if min_zoom.is_finite() && min_zoom > 0.0 {
            min_zoom.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            MIN_ZOOM
        };
        if self.zoom < self.min_zoom {
            self.zoom_at((0.0, 0.0), self.min_zoom);
        }
    }

    /// Converts a document-space point to the canvas area's own logical
    /// screen space.
    #[must_use]
    pub fn to_screen(&self, doc: (f32, f32)) -> (f32, f32) {
        (
            doc.0 * self.zoom + self.pan.0,
            doc.1 * self.zoom + self.pan.1,
        )
    }

    /// Converts a point in the canvas area's own logical screen space to
    /// document space — the exact inverse of [`Self::to_screen`].
    #[must_use]
    pub fn to_document(&self, screen: (f32, f32)) -> (f32, f32) {
        (
            (screen.0 - self.pan.0) / self.zoom,
            (screen.1 - self.pan.1) / self.zoom,
        )
    }

    /// Pans by `delta` logical screen pixels (e.g. a drag delta) —
    /// dragging right/down moves the view right/down, the direction a
    /// real "hand tool" drag follows.
    pub fn pan_by(&mut self, delta: (f32, f32)) {
        self.pan.0 += delta.0;
        self.pan.1 += delta.1;
    }

    /// Zooms to `new_zoom` (clamped to [`Self::min_zoom`]/[`MAX_ZOOM`]),
    /// keeping the document point currently under `screen_anchor` at the
    /// same screen position — "zoom toward the cursor," not toward the
    /// canvas area's own top-left corner.
    pub fn zoom_at(&mut self, screen_anchor: (f32, f32), new_zoom: f32) {
        let new_zoom = new_zoom.clamp(self.min_zoom(), MAX_ZOOM);
        let doc_anchor = self.to_document(screen_anchor);
        self.zoom = new_zoom;
        self.pan = (
            screen_anchor.0 - doc_anchor.0 * new_zoom,
            screen_anchor.1 - doc_anchor.1 * new_zoom,
        );
    }

    /// Clamps `pan` (per axis, independently) so that
    /// `self.to_document((0.0, 0.0))` — the document-space point that
    /// currently renders at the canvas area's own top-left corner —
    /// never falls below `min_doc` on either axis.
    ///
    /// This is the fix for a real reported bug, not a hypothetical one:
    /// `aurora_tile::TileId` addresses tiles with unsigned `u32` fields,
    /// so there is no way to represent "tile -1" — a document-space
    /// position left of/above a layer's own origin. Before this method
    /// existed, nothing stopped `pan_by`/`zoom_at` from scrolling the
    /// view past that boundary: `aurora_gpu::TileResidency::set_origin`
    /// silently clamped the *render* at the document's own top-left tile
    /// forever, while `to_document` (used directly for pointer-to-
    /// document conversion when painting) kept computing the true,
    /// unbounded — and increasingly negative — position. Render and
    /// paint then silently disagreed for the rest of the session. Rather
    /// than threading signed tile addressing through the whole
    /// compositing pipeline to support true negative-space panning (a
    /// substantially bigger change, deliberately deferred), this clamps
    /// panning itself at the source: the view simply cannot scroll past
    /// the boundary in the first place, so every downstream consumer —
    /// render and paint alike — reads the same, already-bounded `pan`
    /// and can never diverge again.
    ///
    /// Derivation: `to_document((0, 0)) = (-pan.0 / zoom, -pan.1 /
    /// zoom)`. Requiring the x component `>= min_doc.0` gives `pan.0 <=
    /// -min_doc.0 * zoom`; symmetrically for y. So each axis of `pan` is
    /// clamped to at most `-min_doc * zoom` — the exact value that makes
    /// `to_document((0, 0))` land precisely on `min_doc` — and left
    /// untouched when it's already within that bound (panning right/
    /// down is the only direction that can violate it, since it's what
    /// *increases* `pan`, matching `to_screen(doc) = doc * zoom + pan`
    /// and `pan_by`'s own "dragging right/down moves the view right/
    /// down" convention).
    pub fn clamp_pan_to_minimum(&mut self, min_doc: (f32, f32)) {
        let max_pan_x = -min_doc.0 * self.zoom;
        if self.pan.0 > max_pan_x {
            self.pan.0 = max_pan_x;
        }
        let max_pan_y = -min_doc.1 * self.zoom;
        if self.pan.1 > max_pan_y {
            self.pan.1 = max_pan_y;
        }
    }
}

impl Default for CanvasView {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasView, DEFAULT_ZOOM, MAX_ZOOM, MIN_ZOOM};

    #[test]
    // `zoom()` here is a plain field read straight from `new()`'s own
    // literal, never computed -- exact equality is correct, not a float
    // rounding risk `clippy::float_cmp` should warn about.
    #[allow(clippy::float_cmp)]
    fn new_starts_at_100_percent_with_no_pan() {
        let view = CanvasView::new();
        assert_eq!(view.zoom(), DEFAULT_ZOOM);
        assert_eq!(view.pan(), (0.0, 0.0));
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(CanvasView::default(), CanvasView::new());
    }

    #[test]
    fn at_default_zoom_and_pan_screen_and_document_space_coincide() {
        let view = CanvasView::new();
        assert_eq!(view.to_screen((12.0, 34.0)), (12.0, 34.0));
        assert_eq!(view.to_document((12.0, 34.0)), (12.0, 34.0));
    }

    #[test]
    fn to_screen_and_to_document_round_trip() {
        let mut view = CanvasView::new();
        view.zoom_at((100.0, 50.0), 2.5);
        view.pan_by((17.0, -3.0));

        let doc = (123.0, 456.0);
        let screen = view.to_screen(doc);
        let back = view.to_document(screen);
        assert!((back.0 - doc.0).abs() < 1e-3, "{back:?} vs {doc:?}");
        assert!((back.1 - doc.1).abs() < 1e-3, "{back:?} vs {doc:?}");
    }

    #[test]
    fn pan_by_shifts_every_screen_position_by_the_same_delta() {
        let mut view = CanvasView::new();
        let before = view.to_screen((10.0, 10.0));
        view.pan_by((5.0, -2.0));
        let after = view.to_screen((10.0, 10.0));
        assert_eq!(after, (before.0 + 5.0, before.1 - 2.0));
    }

    #[test]
    // `zoom()` was just assigned exactly `4.0` (below `MAX_ZOOM`, so the
    // clamp is a no-op) -- exact equality is the correct check.
    #[allow(clippy::float_cmp)]
    fn zoom_at_keeps_the_anchor_point_fixed_on_screen() {
        let mut view = CanvasView::new();
        let anchor = (400.0, 300.0);
        view.zoom_at(anchor, 4.0);
        assert_eq!(view.zoom(), 4.0);
        let (x, y) = view.to_screen(view.to_document(anchor));
        assert!((x - anchor.0).abs() < 1e-3);
        assert!((y - anchor.1).abs() < 1e-3);

        // The anchor's own screen position must be unchanged by the
        // zoom, not just self-consistent.
        assert!((x - anchor.0).abs() < 1e-3, "{x} vs {}", anchor.0);
    }

    #[test]
    // `zoom()` is clamped to exactly `MIN_ZOOM` by `f32::clamp`, not
    // computed by further arithmetic -- exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn zoom_at_clamps_below_the_minimum() {
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), MIN_ZOOM / 10.0);
        assert_eq!(view.zoom(), MIN_ZOOM);
    }

    #[test]
    // Same reasoning as `zoom_at_clamps_below_the_minimum` above, for
    // `MAX_ZOOM`.
    #[allow(clippy::float_cmp)]
    fn zoom_at_clamps_above_the_maximum() {
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), MAX_ZOOM * 10.0);
        assert_eq!(view.zoom(), MAX_ZOOM);
    }

    #[test]
    fn zoom_at_the_origin_leaves_pan_at_the_origin() {
        // Zooming anchored at the canvas area's own top-left corner
        // means document (0, 0) itself doesn't move on screen.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 8.0);
        assert_eq!(view.pan(), (0.0, 0.0));
    }

    #[test]
    // `to_document((0, 0))` after the clamp is `-max_pan_x / zoom` where
    // `max_pan_x` was itself computed as `-min_doc.0 * zoom` -- an exact
    // round trip through one multiply and one divide at zoom 1.0, not
    // accumulated float error -- exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn clamp_pan_to_minimum_pins_the_boundary_exactly_when_panned_past_it() {
        // Panning right/down by 500px would put document (0, 0) at
        // screen (500, 500), i.e. `to_document((0, 0))` would report the
        // true, negative (-500, -500) -- the exact real bug this method
        // exists to prevent. Clamping must land `to_document((0, 0))`
        // exactly on `min_doc`, not just somewhere no-longer-negative.
        let mut view = CanvasView::new();
        view.pan_by((500.0, 500.0));
        view.clamp_pan_to_minimum((0.0, 0.0));
        assert_eq!(view.to_document((0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    // `pan` is asserted unchanged (still the exact value `pan_by` set,
    // untouched by the clamp) -- exact equality is correct, not a float
    // rounding risk.
    #[allow(clippy::float_cmp)]
    fn clamp_pan_to_minimum_leaves_pan_untouched_when_already_within_bounds() {
        // Panning left/up (negative delta) only ever moves
        // `to_document((0, 0))` further *into* positive document space,
        // away from the boundary -- never past it -- so the clamp must
        // be a complete no-op here.
        let mut view = CanvasView::new();
        view.pan_by((-40.0, -40.0));
        let before = view.pan();
        view.clamp_pan_to_minimum((0.0, 0.0));
        assert_eq!(view.pan(), before);
    }

    #[test]
    // Same reasoning as
    // `clamp_pan_to_minimum_pins_the_boundary_exactly_when_panned_past_it`
    // above -- an exact round trip, not accumulated float error.
    #[allow(clippy::float_cmp)]
    fn clamp_pan_to_minimum_uses_a_non_zero_boundary_for_a_moved_layer() {
        // A layer moved away from the document's own origin (e.g. to
        // (300, 300) via `aurora_doc::LayerTree::set_bounds`) must clamp
        // panning to *its* own top-left corner, not always (0, 0).
        let mut view = CanvasView::new();
        view.pan_by((900.0, 900.0));
        view.clamp_pan_to_minimum((300.0, 300.0));
        assert_eq!(view.to_document((0.0, 0.0)), (300.0, 300.0));
    }

    #[test]
    // Same reasoning as the other `clamp_pan_to_minimum_*` tests above
    // -- both assertions are exact round trips / unchanged values, not
    // accumulated float error.
    #[allow(clippy::float_cmp)]
    fn clamp_pan_to_minimum_clamps_each_axis_independently() {
        // x is panned past the boundary, y is not -- only x should move.
        let mut view = CanvasView::new();
        view.pan_by((500.0, -40.0));
        let y_before = view.pan().1;
        view.clamp_pan_to_minimum((0.0, 0.0));
        assert_eq!(view.to_document((0.0, 0.0)).0, 0.0);
        assert_eq!(
            view.pan().1,
            y_before,
            "the in-bounds axis must be left untouched"
        );
    }

    #[test]
    // Same reasoning as the other `clamp_pan_to_minimum_*` tests above
    // -- exact round trips through one multiply and one divide, not
    // accumulated float error.
    #[allow(clippy::float_cmp)]
    fn clamp_pan_to_minimum_accounts_for_zoom() {
        // At 4x zoom, `to_document` divides by zoom, so the same 500px
        // pan corresponds to a different document-space position than
        // at 1x -- the clamp must account for the view's current zoom,
        // not just its pan.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 4.0);
        view.pan_by((500.0, 500.0));
        view.clamp_pan_to_minimum((0.0, 0.0));
        assert_eq!(view.to_document((0.0, 0.0)), (0.0, 0.0));

        // A partially-out-of-bounds case at the same zoom: the boundary
        // in document space is fixed, but `pan`'s own value that
        // achieves it scales with zoom.
        let mut view2 = CanvasView::new();
        view2.zoom_at((0.0, 0.0), 4.0);
        view2.pan_by((900.0, 900.0));
        view2.clamp_pan_to_minimum((100.0, 100.0));
        assert_eq!(view2.to_document((0.0, 0.0)), (100.0, 100.0));
    }

    #[test]
    // Each assertion lands on a value the clamp assigned directly, not
    // on an accumulated computation.
    #[allow(clippy::float_cmp)]
    fn set_min_zoom_raises_a_zoom_that_is_already_below_the_new_floor() {
        // The renderer cannot honour a zoom below its atlas's own floor,
        // so a view holding one would hand `to_document` a scale nothing
        // was drawn at -- see this method's own doc comment.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.05);
        assert_eq!(view.zoom(), 0.05);

        view.set_min_zoom(0.9375);
        assert_eq!(view.min_zoom(), 0.9375);
        assert_eq!(
            view.zoom(),
            0.9375,
            "a zoom already below the new floor must be raised to it \
             immediately, not left for the next zoom gesture to fix"
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn set_min_zoom_leaves_a_zoom_already_above_the_floor_untouched() {
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 4.0);
        view.set_min_zoom(0.9375);
        assert_eq!(view.zoom(), 4.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn zoom_at_cannot_go_below_the_floor_afterwards() {
        // The floor has to bind every *later* zoom gesture too, or the
        // very next scroll-to-zoom reopens the divergence. This is why
        // the bound is stored on the view rather than applied once by a
        // caller.
        let mut view = CanvasView::new();
        view.set_min_zoom(0.9375);
        for requested in [0.5_f32, 0.01, MIN_ZOOM, MIN_ZOOM / 100.0, 0.9] {
            view.zoom_at((100.0, 100.0), requested);
            assert_eq!(
                view.zoom(),
                0.9375,
                "zooming to {requested} must land on the floor"
            );
        }
        // And zooming back in still works normally.
        view.zoom_at((100.0, 100.0), 2.0);
        assert_eq!(view.zoom(), 2.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_fresh_view_has_no_floor_beyond_the_documented_minimum() {
        // Nothing that doesn't know about a renderer changes behaviour:
        // `MIN_ZOOM` is still the only bound until someone sets one.
        let view = CanvasView::new();
        assert_eq!(view.min_zoom(), MIN_ZOOM);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn set_min_zoom_ignores_degenerate_floors() {
        // A caller computing the floor from a viewport it hasn't laid
        // out yet can produce any of these; none of them may be able to
        // pin the view at an unusable zoom, or invert `zoom_at`'s own
        // clamp.
        let mut view = CanvasView::new();
        for bad in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            view.set_min_zoom(bad);
            assert_eq!(view.min_zoom(), MIN_ZOOM, "floor {bad} must be ignored");
        }
        // Above MAX_ZOOM is capped rather than accepted, so `zoom_at`'s
        // `clamp(min, MAX_ZOOM)` can never see min > max.
        view.set_min_zoom(MAX_ZOOM * 10.0);
        assert_eq!(view.min_zoom(), MAX_ZOOM);
        view.zoom_at((0.0, 0.0), 1.0);
        assert_eq!(view.zoom(), MAX_ZOOM);
    }

    #[test]
    fn the_document_position_under_a_click_is_read_at_the_bounded_zoom() {
        // The whole point, stated as the property that was broken: after
        // the floor is applied, `to_document` divides by the *same* zoom
        // the renderer is given, so the document position a click maps to
        // is the one drawn under the cursor. Before, a view clamped to
        // 0.25 while the atlas rendered 0.9375 put a click at screen
        // x = 960 at document x = 3840 while the pixel there was
        // document x ~= 1024.
        let mut view = CanvasView::new();
        view.set_min_zoom(0.9375);
        view.zoom_at((0.0, 0.0), 0.25);

        let doc = view.to_document((960.0, 0.0)).0;
        let expected = 960.0 / view.zoom();
        assert!(
            (doc - expected).abs() < 1e-3,
            "to_document must divide by the bounded zoom {}, giving {expected}; \
             got {doc}",
            view.zoom()
        );
        assert!(
            doc < 1100.0,
            "with the floor in place a click at screen x = 960 has to land \
             near document x = 1024, not the 3840 an unbounded 0.25 zoom \
             would report; got {doc}"
        );
    }

    #[test]
    fn set_min_zoom_holds_the_top_left_document_position_across_the_raise() {
        // The interaction that would otherwise reopen the *pan*-axis
        // divergence: `clamp_pan_to_minimum` has already pinned
        // `to_document((0, 0))` at or beyond a moved layer's own origin,
        // and raising the zoom without compensating `pan` moves that
        // point back toward the document origin -- under the boundary
        // again, where the renderer clamps and `to_document` does not.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.25);
        view.pan_by((-100.0, -100.0));
        view.clamp_pan_to_minimum((300.0, 300.0));
        let before = view.to_document((0.0, 0.0));
        assert!(before.0 >= 300.0 && before.1 >= 300.0);

        view.set_min_zoom(0.9375);

        let after = view.to_document((0.0, 0.0));
        assert!(
            (after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3,
            "the document position at the canvas's own top-left corner must \
             not move when the zoom is raised to the floor: was {before:?}, \
             now {after:?}"
        );
        assert!(
            after.0 >= 300.0 && after.1 >= 300.0,
            "and it must still satisfy the pan bound the layer's own origin \
             sets; got {after:?}"
        );
    }
}
