//! Painting through the active selection (0.211.0): the document half.
//!
//! The Brush, the Eraser, the Paint Bucket and the Gradient all restrict
//! themselves to the document's active selection. The tools themselves
//! (`aurora_brush`) only know [`aurora_brush::Coverage`] in the painted
//! surface's own (layer-local) texels; this module is the adapter from
//! `aurora_doc::Selection`, which is in **document** space, to it:
//! [`LayerSelection`] shifts every coordinate by the layer's bounds origin.
//!
//! Today a selection is a rectangle (plus an inverted flag), so coverage
//! is exactly `0` or `1`. A raster selection (Lasso, Magic Wand,
//! feathered) will only need to answer `Selection::coverage_at` and
//! friends; nothing here or in the tools changes.
//!
//! With no active selection [`layer_selection`] returns `None` and the
//! tools run their unchanged, selection-free code.

use aurora_brush::{Coverage, CoverageRegion};
use aurora_core::Rect;
use aurora_doc::Selection;

/// A document-space [`Selection`] seen from one layer's own texels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayerSelection {
    selection: Selection,
    /// The layer's position in document space (its bounds' `x`, `y`): the
    /// document pixel of layer-local texel `(0, 0)`.
    origin: (i64, i64),
}

/// The coverage a tool paints through, or `None` when nothing is selected.
/// `bounds` is the painted layer's bounds (its `x`, `y` are the origin).
#[must_use]
pub(crate) fn layer_selection(
    selection: Option<Selection>,
    bounds: Rect,
) -> Option<LayerSelection> {
    selection.map(|selection| LayerSelection {
        selection,
        origin: (bounds.x, bounds.y),
    })
}

/// [`layer_selection`] as the trait object the tools take.
#[must_use]
pub(crate) fn as_coverage(layer: Option<&LayerSelection>) -> Option<&dyn Coverage> {
    layer.map(|c| c as &dyn Coverage)
}

impl LayerSelection {
    fn to_document(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x.saturating_add(self.origin.0),
            y: rect.y.saturating_add(self.origin.1),
            ..rect
        }
    }
}

impl Coverage for LayerSelection {
    fn bounds(&self) -> Option<Rect> {
        self.selection.coverage_bounds().map(|b| Rect {
            x: b.x.saturating_sub(self.origin.0),
            y: b.y.saturating_sub(self.origin.1),
            ..b
        })
    }

    fn at(&self, x: i64, y: i64) -> f32 {
        self.selection.coverage_at(
            x.saturating_add(self.origin.0),
            y.saturating_add(self.origin.1),
        )
    }

    fn region(&self, rect: Rect) -> CoverageRegion {
        let doc = self.to_document(rect);
        if self.selection.fully_excludes(&doc) {
            CoverageRegion::Empty
        } else if self.selection.fully_covers(&doc) {
            CoverageRegion::Full
        } else {
            CoverageRegion::Partial
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn no_selection_is_no_coverage() {
        assert!(layer_selection(None, r(5, 5, 10, 10)).is_none());
        assert!(as_coverage(None).is_none());
    }

    #[test]
    fn a_layer_origin_shifts_every_answer() {
        // Document selection x 100..120, y 50..60; the layer sits at
        // (90, 40), so layer-local texel (10, 10) is document (100, 50).
        let cover = layer_selection(
            Some(Selection::new(r(100, 50, 20, 10))),
            r(90, 40, 500, 500),
        );
        let Some(cover) = cover else {
            unreachable!("a selection was given");
        };
        assert_eq!(cover.bounds(), Some(r(10, 10, 20, 10)));
        assert_eq!(cover.at(10, 10), 1.0);
        assert_eq!(cover.at(29, 19), 1.0);
        assert_eq!(cover.at(30, 19), 0.0, "right edge exclusive");
        assert_eq!(cover.at(29, 20), 0.0, "bottom edge exclusive");
        assert_eq!(cover.at(9, 10), 0.0);
        assert_eq!(
            cover.at(100, 50),
            0.0,
            "a document coordinate is not a local one"
        );
    }

    #[test]
    fn regions_follow_the_selection_in_layer_space() {
        let cover = layer_selection(
            Some(Selection::new(r(100, 100, 100, 100))),
            r(-50, -50, 400, 400),
        );
        let Some(cover) = cover else {
            unreachable!("a selection was given");
        };
        // Selection in layer space is x 150..250.
        assert_eq!(cover.region(r(160, 160, 10, 10)), CoverageRegion::Full);
        assert_eq!(cover.region(r(0, 0, 100, 100)), CoverageRegion::Empty);
        assert_eq!(cover.region(r(200, 200, 100, 100)), CoverageRegion::Partial);
        let inverted = Selection {
            inverted: true,
            ..Selection::new(r(100, 100, 100, 100))
        };
        let Some(inverted) = layer_selection(Some(inverted), r(-50, -50, 400, 400)) else {
            unreachable!("a selection was given");
        };
        assert_eq!(inverted.bounds(), None);
        assert_eq!(inverted.region(r(160, 160, 10, 10)), CoverageRegion::Empty);
        assert_eq!(inverted.region(r(0, 0, 100, 100)), CoverageRegion::Full);
        assert_eq!(inverted.at(160, 160), 0.0);
        assert_eq!(inverted.at(0, 0), 1.0);
    }
}
