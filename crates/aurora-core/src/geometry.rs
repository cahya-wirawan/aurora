//! Document-space geometry: validated extents and rectangles.

use crate::error::CoreError;

/// The document/layer size ceiling (ADR 0002), matching Adobe PSB.
///
/// `u16` was explicitly considered and rejected in ADR 0002 — this value
/// needs at least `u32` to represent, which is what every coordinate type
/// in this module uses.
pub const MAX_DOCUMENT_EXTENT: u32 = 300_000;

/// A validated document or layer extent.
///
/// Rejects zero and anything past [`MAX_DOCUMENT_EXTENT`] — constructing
/// one is the one place that ceiling is actually enforced; everything
/// downstream can assume a `Size` is already in range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    width: u32,
    height: u32,
}

impl Size {
    /// Validates `width`/`height` against [`MAX_DOCUMENT_EXTENT`] and
    /// against zero.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::EmptyDocument`] if either dimension is zero,
    /// or [`CoreError::DocumentTooLarge`] if either exceeds the ceiling.
    pub fn new(width: u32, height: u32) -> Result<Self, CoreError> {
        if width == 0 || height == 0 {
            return Err(CoreError::EmptyDocument);
        }
        if width > MAX_DOCUMENT_EXTENT || height > MAX_DOCUMENT_EXTENT {
            return Err(CoreError::DocumentTooLarge {
                width,
                height,
                max: MAX_DOCUMENT_EXTENT,
            });
        }
        Ok(Self { width, height })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Area in pixels. `u64`, not `u32`: at the ceiling, width × height
    /// alone (9 × 10^10) already overflows `u32::MAX` (~4.29 × 10^9).
    #[must_use]
    pub fn area_px(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// A rectangle in document space.
///
/// `x`/`y` are signed: a layer's bounds can sit partially or fully outside
/// the canvas (moved off-edge, mid-transform) — unlike a document's own
/// extent ([`Size`]), which always starts at the origin.
///
/// `Serialize`/`Deserialize`: a layer's own `bounds` (`LayerKind::Pixel`)
/// and mask bounds travel through `aurora_doc::History::save_journal`
/// (ADR 0009's `.aur` journal encoding).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[must_use]
    pub const fn right(&self) -> i64 {
        self.x + self.width as i64
    }

    #[must_use]
    pub const fn bottom(&self) -> i64 {
        self.y + self.height as i64
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    #[must_use]
    pub fn area_px(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Whether document-space point `(x, y)` lies inside this rectangle.
    ///
    /// Same half-open convention [`Self::right`]/[`Self::bottom`]/
    /// [`Self::intersects`] already establish: `x` in
    /// `[self.x, self.right())`, `y` in `[self.y, self.bottom())` — the
    /// left/top edge is inside, the right/bottom edge is not (a `Rect`
    /// with `x = 0, width = 10` spans columns `0..10`; column `10` is
    /// outside it). An empty rectangle ([`Self::is_empty`]) contains no
    /// point at all — it has no area to speak of.
    #[must_use]
    pub const fn contains_point(&self, x: i64, y: i64) -> bool {
        !self.is_empty() && x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        if self.is_empty() || other.is_empty() {
            return false;
        }
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The smallest rectangle containing both `self` and `other`.
    ///
    /// An empty rectangle acts as the identity — unioning with it returns
    /// the other rectangle unchanged, which is what dirty-rectangle
    /// accumulation (starting from an empty rect) needs.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x,
            y,
            #[allow(clippy::cast_sign_loss)]
            width: (right - x) as u32,
            #[allow(clippy::cast_sign_loss)]
            height: (bottom - y) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreError, MAX_DOCUMENT_EXTENT, Rect, Size};

    #[test]
    fn size_accepts_the_ceiling() {
        // No `.expect()`/`.unwrap()` -- both denied workspace-wide, tests
        // included. `unreachable!()` isn't (clippy's `panic` lint only
        // covers the literal `panic!` macro).
        let size = match Size::new(MAX_DOCUMENT_EXTENT, MAX_DOCUMENT_EXTENT) {
            Ok(size) => size,
            Err(err) => unreachable!("must accept the ceiling: {err:?}"),
        };
        assert_eq!(size.width(), MAX_DOCUMENT_EXTENT);
        assert_eq!(
            size.area_px(),
            u64::from(MAX_DOCUMENT_EXTENT) * u64::from(MAX_DOCUMENT_EXTENT)
        );
    }

    #[test]
    fn size_rejects_past_the_ceiling() {
        assert!(matches!(
            Size::new(MAX_DOCUMENT_EXTENT + 1, 100),
            Err(CoreError::DocumentTooLarge { .. })
        ));
    }

    #[test]
    fn size_rejects_zero() {
        assert!(matches!(Size::new(0, 100), Err(CoreError::EmptyDocument)));
        assert!(matches!(Size::new(100, 0), Err(CoreError::EmptyDocument)));
    }

    #[test]
    fn rect_intersects_overlapping() {
        let a = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        };
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
    }

    #[test]
    fn rect_does_not_intersect_disjoint() {
        let a = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Rect {
            x: 20,
            y: 20,
            width: 10,
            height: 10,
        };
        assert!(!a.intersects(&b));
    }

    #[test]
    fn rect_touching_edges_do_not_intersect() {
        // Half-open convention: [x, right) — adjacent rects that only
        // share a boundary line do not overlap.
        let a = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Rect {
            x: 10,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(!a.intersects(&b));
    }

    #[test]
    fn rect_union_covers_both() {
        let a = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Rect {
            x: 5,
            y: -5,
            width: 10,
            height: 10,
        };
        let u = a.union(&b);
        assert_eq!(
            u,
            Rect {
                x: 0,
                y: -5,
                width: 15,
                height: 15
            }
        );
    }

    #[test]
    fn rect_union_with_empty_is_identity() {
        let a = Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        let empty = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert_eq!(a.union(&empty), a);
        assert_eq!(empty.union(&a), a);
    }

    #[test]
    fn contains_point_true_for_a_point_strictly_inside() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(r.contains_point(5, 5));
    }

    #[test]
    fn contains_point_true_on_the_left_and_top_edges_false_on_the_right_and_bottom() {
        // Half-open convention: [x, right) x [y, bottom) -- the same
        // boundary `rect_touching_edges_do_not_intersect` already
        // establishes for `intersects`.
        let r = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(r.contains_point(0, 0), "left/top edge is inside");
        assert!(r.contains_point(0, 5), "left edge, mid-height, is inside");
        assert!(r.contains_point(5, 0), "top edge, mid-width, is inside");
        assert!(
            !r.contains_point(10, 5),
            "right edge (x == right()) is outside"
        );
        assert!(
            !r.contains_point(5, 10),
            "bottom edge (y == bottom()) is outside"
        );
        assert!(
            !r.contains_point(10, 10),
            "bottom-right corner is outside on both axes"
        );
    }

    #[test]
    fn contains_point_false_for_a_point_outside_bounds() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert!(!r.contains_point(-1, 5));
        assert!(!r.contains_point(5, -1));
        assert!(!r.contains_point(20, 20));
    }

    #[test]
    fn contains_point_is_false_everywhere_for_an_empty_rect() {
        let empty = Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        // An empty rect still has `right() == x` and `bottom() == y`, so
        // without the explicit `is_empty()` guard the half-open test
        // would already fail closed here -- this asserts that stays
        // true, and also checks a point that would otherwise land
        // exactly on the degenerate `x == right()` boundary.
        assert!(!empty.contains_point(0, 0));
        assert!(!empty.contains_point(0, 5));
    }

    #[test]
    fn rect_negative_coordinates_are_allowed() {
        let r = Rect {
            x: -50,
            y: -50,
            width: 100,
            height: 100,
        };
        assert_eq!(r.right(), 50);
        assert_eq!(r.bottom(), 50);
        assert!(!r.is_empty());
    }
}
