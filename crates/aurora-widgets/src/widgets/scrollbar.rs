//! A scrollbar: a bounded position along one axis, plus the size of the
//! visible page within that range.
//!
//! **Scope, stated honestly. This is a position *model*, not scrolling.**
//! Nothing in this crate scrolls any content: there is no viewport, no
//! clip rect, no content-offset transform, and no widget observes a
//! [`ScrollbarState`] to move itself. `widgets`' own module doc comment
//! lists "scrolling for scrollbars/trees" as infrastructure that does
//! not exist yet, and it still does not — what landed here is the
//! accessibility node, the layout style, the clamped value arithmetic,
//! and the paint geometry, which is exactly the boundary every other
//! widget in this module already keeps ("layout + content, no
//! behaviour"). A real scrolling container is separate, later work.
//!
//! No component-gallery golden covers this widget either. The gallery's
//! goldens are pixel comparisons that need a human to bless them on real
//! GPU hardware (CLAUDE.md: "a green test run is not evidence that
//! canvas or UI work is correct"), which is out of scope for the round
//! that added this file. `paint.rs`'s own three scrollbar unit tests
//! cover the shapes, ordering, thumb travel, and disabled dimming
//! without claiming a visual review happened.
//!
//! **Why `Role::ScrollBar` plus the numeric-value vocabulary** — value,
//! min, max, and the `SetValue`/`Increment`/`Decrement` actions — rather
//! than `accesskit`'s `ScrollX`/`ScrollXMin`/`ScrollXMax` and
//! `SetScrollOffset`: the scroll-offset properties are read by no
//! shipping `accesskit` platform adapter, whereas the numeric-value
//! properties are what drive the Windows UIA `RangeValue` pattern and the
//! macOS/AT-SPI Value interfaces. The scroll-offset vocabulary describes
//! a *scrollable container's* own state, which is precisely the thing
//! this crate does not have yet.

use accesskit::{Action, Node, Orientation, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length};
use taffy::{Size, Style};

use super::{WidgetKind, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The three numbers that travel together whenever a scrollbar's own
/// extent is described: the bounds of its position, and how much of the
/// scrolled content is visible at once.
///
/// Grouped into a struct rather than passed as three more parameters
/// only because the workspace's own `too_many_arguments` lint is a real
/// bound and [`insert_scrollbar`] would otherwise sit past it — the same
/// reasoning `aurora_io`'s own `WritePolicy` already records.
///
/// No `Eq`: the fields are `f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarRange {
    pub min: f64,
    pub max: f64,
    /// How much of the scrolled content is visible at once, in the same
    /// units as `min`/`max` — what sets the thumb's own *length*
    /// relative to its track, the one thing that distinguishes a
    /// scrollbar's paint from a slider's. `0.0` is legal and means "no
    /// proportional information," which paints a minimum-length thumb.
    pub page_size: f64,
}

/// A scrollbar's own state. No `Eq` (the value/range fields are `f64`),
/// and no label field: a scrollbar is named by the region it scrolls,
/// not by chrome of its own, and no such region exists in this crate yet
/// — see this module's own doc comment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarState {
    /// `accesskit::Orientation` directly, not a second, parallel enum —
    /// the same "reuse `accesskit`'s own vocabulary" discipline
    /// [`super::CheckboxState`]'s own `accesskit::Toggled` already
    /// established.
    pub orientation: Orientation,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub page_size: f64,
    pub disabled: bool,
}

fn node(state: &ScrollbarState) -> Node {
    let mut node = Node::new(Role::ScrollBar);
    node.set_orientation(state.orientation);
    node.set_numeric_value(state.value);
    node.set_min_numeric_value(state.min);
    node.set_max_numeric_value(state.max);
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    node
}

fn style(scales: &Scales, orientation: Orientation) -> Style {
    // Fills the available space along its own scrolling axis (a
    // scrollbar is sized by the region it sits beside, not by its own
    // content), with a fixed cross-axis thickness grounded in the type
    // scale -- see `type_size`'s own doc comment on why, the same
    // reasoning `checkbox::style`/`slider::style` already use. Which
    // axis gets the fixed thickness is the whole point of branching
    // here: a vertical scrollbar that took `slider::style` unchanged
    // would be a 13px-tall horizontal bar.
    let thickness = length(type_size(scales.typography.size.md));
    let size = match orientation {
        Orientation::Vertical => Size {
            width: thickness,
            height: auto(),
        },
        Orientation::Horizontal => Size {
            width: auto(),
            height: thickness,
        },
    };
    Style {
        flex_grow: 1.0,
        size,
        ..Default::default()
    }
}

/// Adds a new, enabled scrollbar as the last child of `parent`, with
/// `value` (already clamped to `range.min..=range.max`) as its starting
/// position.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Assumes `range.min <= range.max` (the caller's responsibility — there
/// is no sensible way to validate a scrollbar's own range against
/// anything external, the same precondition [`super::insert_slider`]
/// already documents).
pub fn insert_scrollbar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    orientation: Orientation,
    value: f64,
    range: ScrollbarRange,
) -> Result<WidgetId, WidgetError> {
    let state = ScrollbarState {
        orientation,
        value: value.clamp(range.min, range.max),
        min: range.min,
        max: range.max,
        page_size: range.page_size,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales, orientation),
        node(&state),
        WidgetKind::Scrollbar(state),
    )
}

/// Sets `id` (a scrollbar) to `value`, clamped to its own
/// `min..=max`. Returns the clamped value actually stored.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a scrollbar,
/// or [`WidgetError::WidgetDisabled`] if it's disabled.
pub fn set_scrollbar_value(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    value: f64,
) -> Result<f64, WidgetError> {
    let mut result = 0.0;
    with_scrollbar_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        state.value = value.clamp(state.min, state.max);
        result = state.value;
        Ok(())
    })?;
    Ok(result)
}

/// Sets whether `id` (a scrollbar) is disabled.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a scrollbar.
pub fn set_scrollbar_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_scrollbar_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_scrollbar_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut ScrollbarState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Scrollbar(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::Scrollbar(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a Scrollbar above");
    };
    // `set_accessibility` marks the widget dirty itself -- no separate
    // damage call, the same as `with_slider_mut`.
    tree.set_accessibility(id, node(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ScrollbarRange, insert_scrollbar, set_scrollbar_disabled, set_scrollbar_value};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::{Action, Orientation};
    use taffy::Style;

    fn range() -> ScrollbarRange {
        ScrollbarRange {
            min: 0.0,
            max: 100.0,
            page_size: 20.0,
        }
    }

    #[test]
    // Exact-literal round-trip, no arithmetic -- same reasoning
    // `slider::tests`/`tree::tests` already document for their own
    // float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_clamps_an_out_of_range_starting_value() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            150.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Scrollbar(state)) => assert_eq!(state.value, 100.0),
            other => unreachable!("expected Scrollbar, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.numeric_value(), Some(100.0));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn insert_scrollbar_declares_its_own_orientation_and_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            25.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::ScrollBar);
        // `orientation()` is an `Option<Orientation>`, not a bare value.
        assert_eq!(accessibility.orientation(), Some(Orientation::Vertical));
        assert_eq!(accessibility.numeric_value(), Some(25.0));
        assert_eq!(accessibility.min_numeric_value(), Some(0.0));
        assert_eq!(accessibility.max_numeric_value(), Some(100.0));
        assert!(accessibility.supports_action(Action::Focus));
        assert!(accessibility.supports_action(Action::SetValue));
        assert!(accessibility.supports_action(Action::Increment));
        assert!(accessibility.supports_action(Action::Decrement));
        assert!(!accessibility.is_disabled());
    }

    #[test]
    fn a_horizontal_scrollbar_declares_horizontal_orientation() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Horizontal,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.orientation(), Some(Orientation::Horizontal));
    }

    #[test]
    fn insert_scrollbar_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        // Same bogus-id precedent `tree`'s own tests use -- never
        // inserted into this tree.
        let bogus = accesskit::NodeId(999);
        match insert_scrollbar(
            &mut tree,
            bogus,
            &scales,
            Orientation::Vertical,
            0.0,
            range(),
        ) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn set_scrollbar_value_clamps_to_the_scrollbars_own_range() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            50.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match set_scrollbar_value(&mut tree, id, 75.0) {
            Ok(v) => assert_eq!(v, 75.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_scrollbar_value(&mut tree, id, -10.0) {
            Ok(v) => assert_eq!(v, 0.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_scrollbar_value(&mut tree, id, 1000.0) {
            Ok(v) => assert_eq!(v, 100.0),
            Err(err) => unreachable!("{err:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.numeric_value(),
            Some(100.0),
            "the accessibility node must carry the clamped value, not the raw one"
        );
    }

    #[test]
    fn set_scrollbar_value_marks_the_widget_dirty() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();
        if let Err(err) = set_scrollbar_value(&mut tree, id, 50.0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
    }

    #[test]
    fn set_scrollbar_value_rejects_a_disabled_scrollbar() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_scrollbar_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match set_scrollbar_value(&mut tree, id, 50.0) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn set_scrollbar_disabled_clears_the_accesskit_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_scrollbar(
            &mut tree,
            root,
            &scales,
            Orientation::Vertical,
            0.0,
            range(),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_scrollbar_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert!(accessibility.is_disabled());
        assert!(!accessibility.supports_action(Action::Focus));
        assert!(!accessibility.supports_action(Action::SetValue));
        assert!(!accessibility.supports_action(Action::Increment));
        assert!(!accessibility.supports_action(Action::Decrement));
    }

    #[test]
    fn scrollbar_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_scrollbar_value(&mut tree, root, 1.0) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
        match set_scrollbar_disabled(&mut tree, root, true) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn scrollbar_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match set_scrollbar_value(&mut tree, bogus, 1.0) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        match set_scrollbar_disabled(&mut tree, bogus, true) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
