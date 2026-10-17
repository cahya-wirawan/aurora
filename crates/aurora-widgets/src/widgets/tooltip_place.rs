//! Where a tooltip goes (0.262.0): the pure geometry the [`super::Tooltip`]
//! controller uses to keep a shown tooltip inside the window.
//!
//! Two pure functions, no tree and no text engine:
//!
//! - [`tooltip_size`] turns a measured text width into the tooltip's box:
//!   the text plus its horizontal padding, capped at a maximum width (the
//!   text then ends in an ellipsis), at least the minimum height.
//! - [`place_tooltip`] puts that box beside an anchor (the owner's bounds)
//!   inside a window. It tries the preferred [`TooltipSide`] first, then
//!   the opposite one, then the two perpendicular ones, and takes the
//!   first on which the whole box fits along its own axis; the other axis
//!   is clamped into the window. When no side fits (a window smaller than
//!   the anchor plus the box) it takes the side with the most room and
//!   clamps both axes, so the box is always as far inside the window as it
//!   can be, and always finite.
//!
//! Everything is logical pixels. A non-finite or negative input reads as
//! zero, so a bad measurement can never put a NaN into a layout style.

use aurora_core::Rect;

/// Which edge of its anchor a tooltip hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipSide {
    /// Under the anchor, left edges aligned.
    Below,
    /// Over the anchor, left edges aligned.
    Above,
    /// To the right of the anchor, top edges aligned.
    Right,
    /// To the left of the anchor, top edges aligned.
    Left,
}

impl TooltipSide {
    /// The side across the anchor.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Below => Self::Above,
            Self::Above => Self::Below,
            Self::Right => Self::Left,
            Self::Left => Self::Right,
        }
    }

    /// The order in which sides are tried when `self` is preferred: itself,
    /// its opposite, then the two perpendicular ones (below before above,
    /// right before left).
    #[must_use]
    pub fn order(self) -> [Self; 4] {
        match self {
            Self::Below | Self::Above => [self, self.opposite(), Self::Right, Self::Left],
            Self::Right | Self::Left => [self, self.opposite(), Self::Below, Self::Above],
        }
    }
}

/// A tooltip's top-left corner (window coordinates, logical px) and the
/// side it landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooltipPlacement {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// The side of the anchor it hangs from.
    pub side: TooltipSide,
}

/// A finite, non-negative reading of `value` (anything else is zero).
fn nonneg(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// `value` clamped into `0..=(limit - extent)`, or `0` when the box is
/// larger than the window along this axis.
fn clamp_into(value: f32, extent: f32, limit: f32) -> f32 {
    let max = (limit - extent).max(0.0);
    value.clamp(0.0, max)
}

/// The tooltip's box for a measured text: `text_width + 2 * pad_h` wide,
/// never wider than `max_width` (and never narrower than the padding
/// alone), `max(text_height + 2 * pad_v, min_height)` tall.
#[must_use]
pub fn tooltip_size(
    text_width: f32,
    text_height: f32,
    pad_h: f32,
    pad_v: f32,
    max_width: f32,
    min_height: f32,
) -> (f32, f32) {
    let pad_h = nonneg(pad_h);
    let width = (nonneg(text_width).ceil() + 2.0 * pad_h).min(nonneg(max_width).max(2.0 * pad_h));
    let height = (nonneg(text_height) + 2.0 * nonneg(pad_v)).max(nonneg(min_height));
    (width, height.ceil())
}

/// The room a `size` box has on `side` of `anchor` in `window`, minus
/// `gap`: negative when it does not fit.
fn room(side: TooltipSide, anchor: (f32, f32, f32, f32), window: (f32, f32), gap: f32) -> f32 {
    let (ax, ay, aw, ah) = anchor;
    match side {
        TooltipSide::Below => window.1 - (ay + ah) - gap,
        TooltipSide::Above => ay - gap,
        TooltipSide::Right => window.0 - (ax + aw) - gap,
        TooltipSide::Left => ax - gap,
    }
}

/// Places a `size` (width, height) box beside `anchor` inside `window`
/// (width, height), `gap` logical px away from the anchor, preferring
/// `prefer` — see the module documentation for the order and the clamping.
#[must_use]
pub fn place_tooltip(
    anchor: Rect,
    size: (f32, f32),
    window: (f32, f32),
    gap: f32,
    prefer: TooltipSide,
) -> TooltipPlacement {
    #[allow(clippy::cast_precision_loss)]
    let anchor = (
        anchor.x as f32,
        anchor.y as f32,
        anchor.width as f32,
        anchor.height as f32,
    );
    let size = (nonneg(size.0), nonneg(size.1));
    let window = (nonneg(window.0), nonneg(window.1));
    let gap = nonneg(gap);

    let order = prefer.order();
    let need = |side: TooltipSide| match side {
        TooltipSide::Below | TooltipSide::Above => size.1,
        TooltipSide::Right | TooltipSide::Left => size.0,
    };
    // The first side with room for the whole box; failing that the one
    // with the most room (the earlier in `order` on a tie).
    let mut chosen = None;
    for side in order {
        if room(side, anchor, window, gap) >= need(side) {
            chosen = Some(side);
            break;
        }
    }
    let side = chosen.unwrap_or_else(|| {
        let mut best = prefer;
        let mut best_room = f32::NEG_INFINITY;
        for side in order {
            let r = room(side, anchor, window, gap);
            if r > best_room {
                best_room = r;
                best = side;
            }
        }
        best
    });

    let (ax, ay, aw, ah) = anchor;
    let (x, y) = match side {
        TooltipSide::Below => (ax, ay + ah + gap),
        TooltipSide::Above => (ax, ay - gap - size.1),
        TooltipSide::Right => (ax + aw + gap, ay),
        TooltipSide::Left => (ax - gap - size.0, ay),
    };
    TooltipPlacement {
        x: clamp_into(x, size.0, window.0),
        y: clamp_into(y, size.1, window.1),
        side,
    }
}

#[cfg(test)]
mod tests {
    use aurora_core::Rect;

    use super::{TooltipSide, place_tooltip, tooltip_size};

    const WINDOW: (f32, f32) = (800.0, 600.0);
    const SIZE: (f32, f32) = (120.0, 24.0);
    const GAP: f32 = 4.0;

    fn rect(x: i64, y: i64, w: u32, h: u32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn place(anchor: Rect, prefer: TooltipSide) -> super::TooltipPlacement {
        place_tooltip(anchor, SIZE, WINDOW, GAP, prefer)
    }

    /// The box lies wholly inside the window (when it can).
    fn inside(p: super::TooltipPlacement) -> bool {
        p.x >= 0.0 && p.y >= 0.0 && p.x + SIZE.0 <= WINDOW.0 && p.y + SIZE.1 <= WINDOW.1
    }

    #[test]
    fn sizes_follow_the_text_and_the_cap() {
        // text 50.2 -> 51 + 2 * 8, height 15 + 2 * 4.
        assert_eq!(
            tooltip_size(50.2, 15.0, 8.0, 4.0, 320.0, 10.0),
            (67.0, 23.0)
        );
        // Capped: a long text never exceeds the maximum.
        assert_eq!(tooltip_size(900.0, 15.0, 8.0, 4.0, 320.0, 10.0).0, 320.0);
        // The minimum height wins over a short line.
        assert_eq!(tooltip_size(10.0, 5.0, 8.0, 4.0, 320.0, 30.0).1, 30.0);
        // A cap below the padding alone still holds the padding.
        assert_eq!(tooltip_size(50.0, 15.0, 8.0, 4.0, 4.0, 0.0).0, 16.0);
        // Garbage reads as zero text.
        assert_eq!(
            tooltip_size(f32::NAN, f32::INFINITY, 8.0, 4.0, 320.0, 0.0),
            (16.0, 8.0)
        );
        assert_eq!(tooltip_size(-5.0, -5.0, 8.0, 4.0, 320.0, 0.0), (16.0, 8.0));
    }

    #[test]
    fn open_space_gives_the_preferred_side_flush_with_the_anchor() {
        let a = rect(300, 200, 32, 32);
        let below = place(a, TooltipSide::Below);
        assert_eq!(
            (below.x, below.y, below.side),
            (300.0, 236.0, TooltipSide::Below)
        );
        let above = place(a, TooltipSide::Above);
        assert_eq!(
            (above.x, above.y, above.side),
            (300.0, 200.0 - 4.0 - 24.0, TooltipSide::Above)
        );
        let right = place(a, TooltipSide::Right);
        assert_eq!(
            (right.x, right.y, right.side),
            (336.0, 200.0, TooltipSide::Right)
        );
        let left = place(a, TooltipSide::Left);
        assert_eq!(
            (left.x, left.y, left.side),
            (300.0 - 4.0 - 120.0, 200.0, TooltipSide::Left)
        );
    }

    #[test]
    fn the_bottom_edge_flips_below_to_above() {
        // The Layers toolbar sits on the rail's bottom edge.
        let a = rect(300, 600 - 32, 32, 32);
        let p = place(a, TooltipSide::Below);
        assert_eq!(p.side, TooltipSide::Above);
        assert_eq!(p.y, 568.0 - 4.0 - 24.0);
        assert!(inside(p));
        // One pixel short of fitting below still flips.
        let a = rect(300, 600 - 32 - 4 - 24 + 1, 32, 32);
        assert_eq!(place(a, TooltipSide::Below).side, TooltipSide::Above);
        // Exactly fitting below stays below.
        let a = rect(300, 600 - 32 - 4 - 24, 32, 32);
        assert_eq!(place(a, TooltipSide::Below).side, TooltipSide::Below);
    }

    #[test]
    fn the_top_edge_flips_above_to_below() {
        let a = rect(300, 0, 32, 32);
        let p = place(a, TooltipSide::Above);
        assert_eq!(p.side, TooltipSide::Below);
        assert!(inside(p));
        // Exactly fitting above stays above; one pixel less flips.
        let a = rect(300, 28, 32, 32);
        assert_eq!(place(a, TooltipSide::Above).side, TooltipSide::Above);
        let a = rect(300, 27, 32, 32);
        assert_eq!(place(a, TooltipSide::Above).side, TooltipSide::Below);
    }

    #[test]
    fn the_right_edge_flips_right_to_left_and_clamps_the_cross_axis() {
        let a = rect(800 - 32, 100, 32, 32);
        let p = place(a, TooltipSide::Right);
        assert_eq!(p.side, TooltipSide::Left);
        assert_eq!(p.x, 768.0 - 4.0 - 120.0);
        assert!(inside(p));
        // Exactly fitting to the right stays; one pixel less flips.
        let a = rect(800 - 32 - 4 - 120, 100, 32, 32);
        assert_eq!(place(a, TooltipSide::Right).side, TooltipSide::Right);
        let a = rect(800 - 32 - 4 - 120 + 1, 100, 32, 32);
        assert_eq!(place(a, TooltipSide::Right).side, TooltipSide::Left);
    }

    #[test]
    fn the_left_edge_flips_left_to_right() {
        let a = rect(0, 100, 32, 32);
        let p = place(a, TooltipSide::Left);
        assert_eq!(p.side, TooltipSide::Right);
        assert_eq!(p.x, 36.0);
        assert!(inside(p));
    }

    #[test]
    fn below_near_the_right_edge_slides_left_instead_of_leaving() {
        let a = rect(800 - 16, 100, 16, 16);
        let p = place(a, TooltipSide::Below);
        assert_eq!(p.side, TooltipSide::Below);
        assert_eq!(p.x, 800.0 - 120.0);
        assert!(inside(p));
    }

    #[test]
    fn side_tooltips_near_the_bottom_slide_up_instead_of_leaving() {
        let a = rect(100, 600 - 10, 32, 10);
        let p = place(a, TooltipSide::Right);
        assert_eq!(p.side, TooltipSide::Right);
        assert_eq!(p.y, 600.0 - 24.0);
        assert!(inside(p));
    }

    #[test]
    fn all_four_corners_stay_inside_for_every_preference() {
        let corners = [
            rect(0, 0, 32, 32),
            rect(800 - 32, 0, 32, 32),
            rect(0, 600 - 32, 32, 32),
            rect(800 - 32, 600 - 32, 32, 32),
        ];
        for anchor in corners {
            for prefer in [
                TooltipSide::Below,
                TooltipSide::Above,
                TooltipSide::Right,
                TooltipSide::Left,
            ] {
                let p = place(anchor, prefer);
                assert!(inside(p), "{anchor:?} {prefer:?} -> {p:?}");
            }
        }
        // Top-left prefers Below/Right and never lands Above/Left.
        let p = place(corners[0], TooltipSide::Above);
        assert_eq!(p.side, TooltipSide::Below);
        let p = place(corners[0], TooltipSide::Left);
        assert_eq!(p.side, TooltipSide::Right);
        // Bottom-right flips to Above / Left.
        assert_eq!(
            place(corners[3], TooltipSide::Below).side,
            TooltipSide::Above
        );
        assert_eq!(
            place(corners[3], TooltipSide::Right).side,
            TooltipSide::Left
        );
    }

    #[test]
    fn every_anchor_position_keeps_the_box_inside_the_window() {
        // Exhaustive over a grid of anchors, windows large enough for the
        // box beside any 32 px anchor on at least one side.
        let mut checked = 0;
        for prefer in [
            TooltipSide::Below,
            TooltipSide::Above,
            TooltipSide::Right,
            TooltipSide::Left,
        ] {
            for x in (-40..=840).step_by(20) {
                for y in (-40..=640).step_by(20) {
                    let a = rect(x, y, 32, 32);
                    let p = place(a, prefer);
                    assert!(
                        p.x.is_finite() && p.y.is_finite(),
                        "{a:?} {prefer:?} -> {p:?}"
                    );
                    assert!(inside(p), "{a:?} {prefer:?} -> {p:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 4000);
    }

    #[test]
    fn a_window_too_small_for_any_side_picks_the_roomiest_and_clamps() {
        // 100 x 40 window, 120 x 24 box: it cannot fit sideways at all.
        let p = place_tooltip(rect(10, 4, 20, 10), SIZE, (100.0, 40.0), GAP, TooltipSide::Right);
        // Below has 40 - 14 - 4 = 22 < 24, Above has 0, Right 100-30-4=66,
        // Left 6: the roomiest is Right, clamped to the window.
        assert_eq!(p.side, TooltipSide::Right);
        assert_eq!(p.x, 0.0, "wider than the window: pinned to the left edge");
        assert!(p.y >= 0.0 && p.y + SIZE.1 <= 40.0);
        // A zero window gives (0, 0), finite.
        let z = place_tooltip(rect(0, 0, 1, 1), SIZE, (0.0, 0.0), GAP, TooltipSide::Below);
        assert_eq!((z.x, z.y), (0.0, 0.0));
    }

    #[test]
    fn preference_order_is_preferred_opposite_then_perpendicular() {
        assert_eq!(
            TooltipSide::Below.order(),
            [
                TooltipSide::Below,
                TooltipSide::Above,
                TooltipSide::Right,
                TooltipSide::Left
            ]
        );
        assert_eq!(
            TooltipSide::Left.order(),
            [
                TooltipSide::Left,
                TooltipSide::Right,
                TooltipSide::Below,
                TooltipSide::Above
            ]
        );
        assert_eq!(TooltipSide::Above.opposite(), TooltipSide::Below);
        assert_eq!(TooltipSide::Right.opposite(), TooltipSide::Left);
    }

    #[test]
    fn non_finite_inputs_never_produce_a_non_finite_position() {
        let p = place_tooltip(
            rect(10, 10, 20, 20),
            (f32::NAN, f32::INFINITY),
            (f32::NAN, 600.0),
            f32::NAN,
            TooltipSide::Below,
        );
        assert!(p.x.is_finite() && p.y.is_finite());
    }
}
