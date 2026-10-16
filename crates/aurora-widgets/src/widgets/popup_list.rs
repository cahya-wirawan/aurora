//! The scaffolding a popup list (an in-window [`menu`](super::menu) or a
//! [`dropdown`](super::dropdown)'s option list) shares to stay on screen
//! (0.259.0, the command palette's recipe of 0.258.0 applied to the other
//! two lists).
//!
//! **Cause it fixes.** Both lists used fixed-height rows (so rows never
//! overlapped) but had no maximum height and no scrolling, and a popover
//! is only hit-test-clamped to the window: a menu of 25 items (about
//! 525 px) or 94 (a Select menu with 64 saved selections) simply hung off
//! the bottom of a short window, and the items there could not be reached.
//!
//! **Shape.** The popup's frame (the `Menu` or `DropdownList` widget, a
//! [`PaintLayer::Popover`](crate::PaintLayer) root) is a `Row` that clips
//! its overflow and has a `max-height`; it holds a scroll container
//! (`body`, a `Column`: the rows, each exactly its own height and never
//! shrunk) and a linked scrollbar, hidden while everything fits.
//!
//! **How tall it may be** is [`fit_popup`], a pure function of where the
//! popup hangs, how much window is left, one `spacing.sm` margin, and the
//! rows' total height. It prefers to open *below* its anchor; a dropdown
//! (`allow_flip`) whose anchor leaves less than [`MIN_VISIBLE_ROWS`] rows
//! below and more room above opens *upward* instead (bottom edge on the
//! control's top). A menu never flips (its anchor is the menu bar at the
//! top of the window). **Limit:** when neither side has
//! [`MIN_VISIBLE_ROWS`] rows of room (a window shorter than about four
//! rows) the popup is given those rows anyway and overflows the window;
//! nothing smarter is attempted.
//!
//! The window is the tree root's last-laid-out bounds, so a tree that has
//! never been laid out (a headless unit test) gets no cap at all.

use accesskit::{Node, Orientation, Role};
use taffy::prelude::TaffyZero;
use taffy::style_helpers::{auto, length};
use taffy::{Dimension, FlexDirection, Overflow, Style};

use super::WidgetKind;
use super::scrollbar::{ScrollbarRange, insert_scrollbar_thick, link_scrollbar};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The fewest rows a popup is given even where the window has less room:
/// below this a list is not usable, so it overflows instead.
pub(crate) const MIN_VISIBLE_ROWS: f32 = 3.0;

/// Where a popup goes and how tall it may be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PopupFit {
    /// The frame's maximum height in logical px.
    pub max_height: f32,
    /// Whether the popup opens upward (its bottom on the anchor's top).
    pub above: bool,
}

/// The pure placement rule (see the module docs). All lengths are
/// logical px in window coordinates; `window_top`/`window_bottom` bound
/// the window, `margin` is kept free at the window edge, `content` is the
/// rows' total height and `min_visible` the height below which a popup is
/// not usable. A non-finite input is treated as no room on that side.
pub(crate) fn fit_popup(
    (anchor_top, anchor_bottom): (f32, f32),
    (window_top, window_bottom): (f32, f32),
    margin: f32,
    content: f32,
    min_visible: f32,
    allow_flip: bool,
) -> PopupFit {
    let room = |value: f32| {
        if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        }
    };
    let below = room(window_bottom - margin - anchor_bottom);
    let above = room(anchor_top - margin - window_top);
    let need = room(content).min(room(min_visible));
    let flip = allow_flip && below < need && above > below;
    let available = if flip { above } else { below };
    PopupFit {
        max_height: available.max(need),
        above: flip,
    }
}

/// The linked scrollbar's thickness (the same token a dock panel's uses).
pub(crate) fn scrollbar_thickness(scales: &aurora_theme::Scales) -> f32 {
    super::scrollbar::thickness(scales)
}

/// The window's vertical span `(top, bottom)` in logical px: the tree
/// root's last-laid-out bounds. `None` for a tree never laid out (or laid
/// out with no area), where no cap can be computed.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn window_span(tree: &WidgetTree<WidgetKind>) -> Option<(f32, f32)> {
    let bounds = tree.bounds(tree.root())?;
    if bounds.width == 0 || bounds.height == 0 {
        return None;
    }
    Some((bounds.y as f32, bounds.bottom() as f32))
}

/// A popup frame's layout: a clipping `Row` capped at `max_height`
/// (`None` = uncapped, the unlaid-out case).
pub(crate) fn cap_frame(style: &mut Style, max_height: Option<f32>) {
    style.flex_direction = FlexDirection::Row;
    style.overflow = taffy::Point {
        x: Overflow::Hidden,
        y: Overflow::Hidden,
    };
    style.max_size.height = max_height.map_or_else(auto, length);
}

/// The scroll container's layout: a `Column` filling the frame beside the
/// scrollbar, its rows stacking at their own height.
fn body_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        flex_grow: 1.0,
        flex_basis: Dimension::ZERO,
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// Adds the scroll container and its linked scrollbar to `frame`;
/// returns `(body, scrollbar)`.
pub(crate) fn insert_parts(
    tree: &mut WidgetTree<WidgetKind>,
    frame: WidgetId,
    scrollbar_thickness: f32,
) -> Result<(WidgetId, WidgetId), WidgetError> {
    // Presentational: the frame already is the named `Menu`/`ListBox`
    // and each row declares its own position in the set.
    let body = tree.insert(
        frame,
        body_style(),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    tree.set_scrollable(body, true)?;
    let scrollbar = insert_scrollbar_thick(
        tree,
        frame,
        scrollbar_thickness,
        Orientation::Vertical,
        None,
        0.0,
        ScrollbarRange {
            min: 0.0,
            max: 0.0,
            page_size: 0.0,
        },
    )?;
    link_scrollbar(tree, scrollbar, body)?;
    Ok((body, scrollbar))
}

/// Scrolls `row` into its list's viewport: now when the list has been
/// laid out, and again after the next layout (a list built a moment ago
/// has no bounds and no scroll range yet).
pub(crate) fn reveal(tree: &mut WidgetTree<WidgetKind>, row: WidgetId) {
    tree.scroll_into_view(row);
    tree.request_scroll_into_view(row);
}

/// The popup frame (`Menu` or `DropdownList`) a list row belongs to: the
/// row's parent is the scroll container and its parent the frame.
pub(crate) fn frame_of_row(tree: &WidgetTree<WidgetKind>, row: WidgetId) -> Option<WidgetId> {
    let frame = tree.parent(tree.parent(row)?)?;
    matches!(
        tree.payload(frame),
        Some(WidgetKind::Menu(_) | WidgetKind::DropdownList)
    )
    .then_some(frame)
}

/// The scroll container of a popup `frame` (its only scrollable child).
pub(crate) fn body_of_frame(tree: &WidgetTree<WidgetKind>, frame: WidgetId) -> Option<WidgetId> {
    tree.children(frame)?
        .iter()
        .copied()
        .find(|&child| tree.is_scrollable(child) == Some(true))
}

/// The scroll container a wheel event at `point` should move while a menu
/// or dropdown list is open (0.259.0): the list under the pointer — its
/// rows, its border or its scrollbar. `None` when the pointer is not over
/// an open popup, so the ordinary routing runs. (`scroll_container_at`
/// deliberately ignores popovers: an open list floats above the panel
/// and is not part of its scrolling content.)
#[must_use]
pub fn popup_scroll_target(tree: &WidgetTree<WidgetKind>, point: (f32, f32)) -> Option<WidgetId> {
    let hit = tree.hit_test(point)?;
    let frame = tree.popover_root_of(hit)?;
    if !matches!(
        tree.payload(frame),
        Some(WidgetKind::Menu(_) | WidgetKind::DropdownList)
    ) {
        return None;
    }
    body_of_frame(tree, frame)
}

#[cfg(test)]
mod tests {
    use super::{PopupFit, fit_popup};

    const WINDOW: (f32, f32) = (0.0, 480.0);

    #[test]
    fn a_popup_that_fits_below_is_capped_to_the_room_below_minus_the_margin() {
        let fit = fit_popup((30.0, 30.0), WINDOW, 8.0, 900.0, 63.0, false);
        assert_eq!(
            fit,
            PopupFit {
                max_height: 480.0 - 8.0 - 30.0,
                above: false
            }
        );
    }

    #[test]
    fn a_popup_flips_up_only_when_allowed_below_the_minimum_and_roomier_above() {
        // The control's box is 400..421: 51 px below the margin, 392 above.
        let anchor = (400.0, 421.0);
        let flipped = fit_popup(anchor, WINDOW, 8.0, 900.0, 63.0, true);
        assert!(flipped.above);
        assert_eq!(flipped.max_height, 400.0 - 8.0);
        let not_allowed = fit_popup(anchor, WINDOW, 8.0, 900.0, 63.0, false);
        assert!(!not_allowed.above);
        assert_eq!(not_allowed.max_height, 63.0, "the minimum, overflowing");
        // Enough room below for the minimum: stays below.
        let roomy = fit_popup((300.0, 321.0), WINDOW, 8.0, 900.0, 63.0, true);
        assert!(!roomy.above);
        assert_eq!(roomy.max_height, 480.0 - 8.0 - 321.0);
    }

    #[test]
    fn no_flip_when_the_side_above_is_no_roomier() {
        // 10 px below, 5 px above: stays below at the minimum.
        let fit = fit_popup((13.0, 462.0), WINDOW, 8.0, 900.0, 63.0, true);
        assert!(!fit.above);
        let fit = fit_popup((13.0, 20.0), (0.0, 38.0), 8.0, 900.0, 63.0, true);
        assert!(!fit.above, "10 below beats 5 above");
    }

    #[test]
    fn a_short_list_needs_less_than_the_minimum() {
        // Two rows (42 px) of content: the minimum is the content.
        let fit = fit_popup((460.0, 460.0), WINDOW, 8.0, 42.0, 63.0, false);
        assert_eq!(fit.max_height, 42.0);
    }

    #[test]
    fn non_finite_inputs_never_produce_a_non_finite_cap() {
        let fit = fit_popup((f32::NAN, f32::NAN), WINDOW, 8.0, 100.0, 63.0, true);
        assert!(fit.max_height.is_finite());
        let fit = fit_popup((10.0, 10.0), (0.0, f32::INFINITY), 8.0, 100.0, 63.0, true);
        assert!(fit.max_height.is_finite());
    }
}
