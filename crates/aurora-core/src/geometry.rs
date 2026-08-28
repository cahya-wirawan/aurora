//! Document-space geometry: validated extents and rectangles.

use crate::error::CoreError;

/// The document/layer size ceiling (ADR 0002), matching Adobe PSB.
///
/// `u16` was explicitly considered and rejected in ADR 0002 — this value
/// needs at least `u32` to represent, which is what every coordinate type
/// in this module uses.
pub const MAX_DOCUMENT_EXTENT: u32 = 300_000;

/// How far a [`Rect`]'s own origin may sit from the document origin, on
/// either axis and in either direction: `x`/`y` must be within
/// `-MAX_DOCUMENT_ORIGIN..=MAX_DOCUMENT_ORIGIN`.
///
/// One whole document extent, not zero and not a joint check against
/// the rectangle's own width/height. Both halves of that are
/// deliberate:
///
/// - **Negative origins stay legal.** A layer can be dragged partially
///   or fully off the canvas, which is exactly what [`Rect`]'s signed
///   `x`/`y` exist for — see `rect_negative_coordinates_are_allowed`.
///   A layer one whole document-width to the left of the canvas is
///   still a layer a user can drag back; anything past that is not a
///   position, it is a corrupt number.
/// - **Extent is not folded in.** Checking `x + width` against the
///   ceiling would refuse a legal maximal-width layer nudged one pixel
///   right, which is an ordinary edit. Extent is already bounded where
///   it is owned — [`Size::new`] for a document, `aurora-io`'s own
///   `tile_grid` for a `.aur` manifest's layer bounds.
///
/// Bounding the origin alone keeps every derived **`i64`** coordinate
/// far from overflow: with `|x| <= 300_000` and `width <= u32::MAX`,
/// [`Rect::right`] is at most ~4.3 × 10^9, nowhere near `i64`'s own
/// ~9.2 × 10^18 ceiling.
///
/// That is a narrower claim than "every derived coordinate is in
/// range", and deliberately so. Extent is *not* bounded by this
/// constant, nor by `aurora_doc`'s live-edit API, so a `Rect` that
/// passed [`Rect::origin_in_document_range`] can still be far wider
/// than any document: `Rect { x: 300_000, width: u32::MAX }.right()`
/// is 4,295,267,295 — an exact, non-saturating `i64`, and also about
/// 14,000 document widths. Extent is bounded where it is owned
/// ([`Size::new`] for a document, `aurora-io`'s own `tile_grid` for a
/// `.aur` manifest's layer bounds), and a `Rect` reaching neither of
/// those keeps only the `i64` guarantee.
///
/// This constant is the shared *number*; it does not enforce itself.
/// [`Rect::origin_in_document_range`] is the shared predicate, and each
/// consuming crate maps a `false` from it to its own error type —
/// `aurora_doc::DocError::LayerOriginOutOfRange` for a live edit,
/// `aurora_io::IoError::LayerOriginOutOfRange` for a `.aur` manifest
/// being read.
pub const MAX_DOCUMENT_ORIGIN: i64 = MAX_DOCUMENT_EXTENT as i64;

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
/// Signed does not mean unbounded. A legitimate origin sits within
/// ±[`MAX_DOCUMENT_ORIGIN`] on each axis, which
/// [`Self::origin_in_document_range`] tests. Unlike [`Size`], this type
/// does *not* enforce that itself — it is a plain, publicly
/// constructible struct, so there is no constructor to enforce it in.
/// The producers do: `aurora_doc::LayerTree`'s own editing API
/// (`add_pixel_layer`, `set_bounds`, `add_mask`) refuses an out-of-range
/// origin on the way in, and `aurora-io`'s `.aur` reader refuses one on
/// the way off disk. [`Self::right`]/[`Self::bottom`]/[`Self::union`]
/// still saturate rather than overflow, so a `Rect` assembled some third
/// way is wrong but never a panic.
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
    /// Whether this rectangle's own origin is within
    /// ±[`MAX_DOCUMENT_ORIGIN`] on both axes — the shared predicate
    /// behind `aurora_doc::DocError::LayerOriginOutOfRange` and
    /// `aurora_io::IoError::LayerOriginOutOfRange`.
    ///
    /// Each axis is tested independently, and `width`/`height` are not
    /// consulted at all — see [`MAX_DOCUMENT_ORIGIN`] for why. This
    /// answers a question; it does not enforce anything. The two crates
    /// named above are what turn a `false` into a refusal, each with
    /// its own error type, because the policy boundary belongs to them
    /// rather than to this type.
    #[must_use]
    pub const fn origin_in_document_range(&self) -> bool {
        self.x >= -MAX_DOCUMENT_ORIGIN
            && self.x <= MAX_DOCUMENT_ORIGIN
            && self.y >= -MAX_DOCUMENT_ORIGIN
            && self.y <= MAX_DOCUMENT_ORIGIN
    }

    /// The first column *past* this rectangle (half-open: a `Rect` with
    /// `x = 0, width = 10` spans columns `0..10`).
    ///
    /// Saturating, not wrapping: for an origin within
    /// [`MAX_DOCUMENT_ORIGIN`] the sum cannot come close to `i64`'s own
    /// range, so saturation is unobservable for every legitimate
    /// rectangle. It matters for one assembled some other way — a
    /// crafted file read by a caller that skipped its validator, a
    /// hand-built fixture — where a plain `+` overflows: silently
    /// wrapping to a wrong picture in release, and panicking in debug,
    /// which this workspace's `panic`-denying lints exist to keep out of
    /// shipping code. Returning `i64::MAX` is wrong too, but it is
    /// wrong *and* inert.
    #[must_use]
    pub const fn right(&self) -> i64 {
        self.x.saturating_add(self.width as i64)
    }

    /// The first row *past* this rectangle. See [`Self::right`] — same
    /// half-open convention, same saturating rationale.
    #[must_use]
    pub const fn bottom(&self) -> i64 {
        self.y.saturating_add(self.height as i64)
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
    ///
    /// **"The smallest rectangle containing both" holds only while the
    /// result's own extent fits a `u32`.** It usually does — two
    /// origins inside [`MAX_DOCUMENT_ORIGIN`] and two document-sized
    /// extents span at most ~9 × 10^5 — but extent is not bounded by
    /// that constant (see it for why), so the case is reachable through
    /// the validated API rather than only through a corrupt `Rect`:
    /// `Rect { x: -300_000, width: u32::MAX }` unioned with
    /// `Rect { x: 300_000, width: u32::MAX }` spans 4,295,567,295
    /// columns, which a `u32` cannot hold. The width then saturates to
    /// `u32::MAX` and the result no longer contains the right-hand
    /// operand's own far edge. What the saturated case still
    /// guarantees is the weaker, defensive one: an origin that is the
    /// true minimum, an extent that is the largest representable, and
    /// no overflow, wrap, or truncation anywhere in getting there.
    ///
    /// Saturating throughout, for the reason [`Self::right`] gives. Two
    /// rectangles whose origins are both in range produce a span well
    /// inside `u32`, so nothing here is observable for a legitimate
    /// pair. For a pathological one — a near-`i64::MIN` origin unioned
    /// with a near-`i64::MAX` one — the span genuinely does not fit a
    /// `u32`, and the honest answers are the saturated ones: a bare
    /// `right - x` would overflow (`i64::MAX` minus a negative), and a
    /// bare `as u32` would truncate the difference to an arbitrary
    /// small number, which is *worse* than `u32::MAX` because it
    /// reports a tiny dirty rectangle for an enormous change.
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
            width: u32::try_from(right.saturating_sub(x)).unwrap_or(u32::MAX),
            height: u32::try_from(bottom.saturating_sub(y)).unwrap_or(u32::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreError, MAX_DOCUMENT_EXTENT, MAX_DOCUMENT_ORIGIN, Rect, Size};

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

    #[test]
    fn rect_right_and_bottom_saturate_instead_of_overflowing() {
        // Pre-fix this panicked outright ("attempt to add with
        // overflow") in a debug build, and wrapped to a large negative
        // number in release -- measured, not assumed. Neither is
        // acceptable in a workspace that denies `panic` precisely
        // because it holds a professional's unsaved work.
        let r = Rect {
            x: i64::MAX,
            y: i64::MAX,
            width: 10,
            height: 10,
        };
        assert_eq!(r.right(), i64::MAX);
        assert_eq!(r.bottom(), i64::MAX);
    }

    #[test]
    fn rect_right_and_bottom_are_exact_for_ordinary_and_negative_origins() {
        // The other half of the saturating rewrite: it must not have
        // changed a single answer anywhere real rectangles live. The
        // negative case is the one `rect_negative_coordinates_are_allowed`
        // guards, restated here alongside the ceiling case so both sit
        // next to the saturation test they constrain.
        let ordinary = Rect {
            x: 20,
            y: 7,
            width: 100,
            height: 3,
        };
        assert_eq!(ordinary.right(), 120);
        assert_eq!(ordinary.bottom(), 10);

        let negative = Rect {
            x: -50,
            y: -50,
            width: 100,
            height: 100,
        };
        assert_eq!(negative.right(), 50);
        assert_eq!(negative.bottom(), 50);

        let at_limit = Rect {
            x: MAX_DOCUMENT_ORIGIN,
            y: -MAX_DOCUMENT_ORIGIN,
            width: MAX_DOCUMENT_EXTENT,
            height: MAX_DOCUMENT_EXTENT,
        };
        assert_eq!(at_limit.right(), 600_000);
        assert_eq!(at_limit.bottom(), 0);
    }

    #[test]
    fn rect_right_and_bottom_are_exact_for_an_in_range_origin_with_a_maximal_extent() {
        // The one shape where saturating and plain `+` could actually
        // have parted company on a rectangle the *validated* API can
        // still produce. Origin is bounded to +/-300,000; extent is
        // deliberately not bounded at all here (see
        // `MAX_DOCUMENT_ORIGIN`), so this rect passes
        // `origin_in_document_range` while being ~14,000 document widths
        // across. The answer must be the exact sum, not `i64::MAX`:
        // saturation is supposed to be unobservable everywhere the
        // arithmetic genuinely fits.
        let wide = Rect {
            x: MAX_DOCUMENT_ORIGIN,
            y: -MAX_DOCUMENT_ORIGIN,
            width: u32::MAX,
            height: u32::MAX,
        };
        assert!(wide.origin_in_document_range());
        assert_eq!(wide.right(), 4_295_267_295);
        assert_eq!(wide.bottom(), 4_294_667_295);
    }

    #[test]
    fn rect_union_saturates_the_width_when_two_in_range_origins_span_past_u32() {
        // Reachable through the validated API, unlike
        // `rect_union_of_saturated_rects_does_not_overflow_or_wrap`
        // above: both origins are inside `MAX_DOCUMENT_ORIGIN`, and only
        // the unbounded *extent* pushes the span past `u32::MAX`. This
        // is the case `union`'s own doc comment now discloses -- the
        // result is *not* the smallest rectangle containing both.
        let left = Rect {
            x: -MAX_DOCUMENT_ORIGIN,
            y: -MAX_DOCUMENT_ORIGIN,
            width: u32::MAX,
            height: u32::MAX,
        };
        let right = Rect {
            x: MAX_DOCUMENT_ORIGIN,
            y: MAX_DOCUMENT_ORIGIN,
            width: u32::MAX,
            height: u32::MAX,
        };
        assert!(left.origin_in_document_range());
        assert!(right.origin_in_document_range());
        // The true span, which is 600,000 past what a `u32` can hold.
        assert_eq!(right.right() - left.x, 4_295_567_295);
        assert!(4_295_567_295_i64 > i64::from(u32::MAX));

        let u = left.union(&right);
        assert_eq!(u.x, -MAX_DOCUMENT_ORIGIN);
        assert_eq!(u.y, -MAX_DOCUMENT_ORIGIN);
        assert_eq!(
            u.width,
            u32::MAX,
            "the span must saturate to the largest representable extent, \
             not truncate or wrap to a small one"
        );
        assert_eq!(u.height, u32::MAX);
        // And the disclosed consequence, pinned rather than left to the
        // doc comment: the saturated union really does fall short of
        // the right-hand operand's own far edge.
        assert!(u.right() < right.right());
    }

    #[test]
    fn rect_union_of_saturated_rects_does_not_overflow_or_wrap() {
        // Pre-fix this panicked in `right()` before `union`'s own
        // arithmetic was even reached. Post-fix both halves have to
        // hold: `right()` saturates, and then `right - x` (`i64::MAX`
        // minus a very negative `x`) must not overflow in turn.
        let low = Rect {
            x: i64::MIN,
            y: i64::MIN,
            width: 10,
            height: 10,
        };
        let high = Rect {
            x: i64::MAX,
            y: i64::MAX,
            width: 10,
            height: 10,
        };
        let u = low.union(&high);
        assert_eq!(u.x, i64::MIN);
        assert_eq!(u.y, i64::MIN);
        assert_eq!(
            u.width,
            u32::MAX,
            "a span that genuinely does not fit a u32 must saturate, not truncate"
        );
        assert_eq!(u.height, u32::MAX);
    }

    #[test]
    fn rect_origin_in_document_range_accepts_the_negative_and_positive_limits() {
        for (x, y) in [
            (MAX_DOCUMENT_ORIGIN, MAX_DOCUMENT_ORIGIN),
            (-MAX_DOCUMENT_ORIGIN, -MAX_DOCUMENT_ORIGIN),
            (MAX_DOCUMENT_ORIGIN, -MAX_DOCUMENT_ORIGIN),
            (0, 0),
            // The exact case `rect_negative_coordinates_are_allowed`
            // exists to protect: a layer dragged off the top-left edge
            // is ordinary, not a defect.
            (-50, -50),
        ] {
            let r = Rect {
                x,
                y,
                width: 100,
                height: 100,
            };
            assert!(
                r.origin_in_document_range(),
                "({x}, {y}) is a legitimate origin"
            );
        }
    }

    #[test]
    fn rect_origin_in_document_range_rejects_one_past_each_limit() {
        // Each axis independently, in both directions -- a rect out of
        // range on `y` alone must be refused just as one out of range on
        // `x` alone is.
        for bad in [
            MAX_DOCUMENT_ORIGIN + 1,
            -MAX_DOCUMENT_ORIGIN - 1,
            i64::MAX,
            i64::MIN,
        ] {
            let on_x = Rect {
                x: bad,
                y: 0,
                width: 100,
                height: 100,
            };
            assert!(
                !on_x.origin_in_document_range(),
                "x = {bad} is past the document range"
            );
            let on_y = Rect {
                x: 0,
                y: bad,
                width: 100,
                height: 100,
            };
            assert!(
                !on_y.origin_in_document_range(),
                "y = {bad} is past the document range"
            );
        }
    }

    #[test]
    fn max_document_origin_matches_the_document_extent_ceiling() {
        // Pins the derivation rather than the number: the origin bound
        // is *one document extent*, so moving the extent ceiling moves
        // this with it instead of leaving a stale literal behind.
        assert_eq!(MAX_DOCUMENT_ORIGIN, i64::from(MAX_DOCUMENT_EXTENT));
    }
}
