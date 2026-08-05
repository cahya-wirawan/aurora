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
}

impl CanvasView {
    /// A fresh view: 100% zoom, document `(0, 0)` at the canvas area's
    /// own top-left corner.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pan: (0.0, 0.0),
            zoom: DEFAULT_ZOOM,
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

    /// Zooms to `new_zoom` (clamped to [`MIN_ZOOM`]/[`MAX_ZOOM`]),
    /// keeping the document point currently under `screen_anchor` at the
    /// same screen position — "zoom toward the cursor," not toward the
    /// canvas area's own top-left corner.
    pub fn zoom_at(&mut self, screen_anchor: (f32, f32), new_zoom: f32) {
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let doc_anchor = self.to_document(screen_anchor);
        self.zoom = new_zoom;
        self.pan = (
            screen_anchor.0 - doc_anchor.0 * new_zoom,
            screen_anchor.1 - doc_anchor.1 * new_zoom,
        );
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
}
