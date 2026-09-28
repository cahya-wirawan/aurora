//! A checkbox: a toggle, the second of the three interaction shapes this
//! first widget slice covers.

use accesskit::{Action, Node, Role, Toggled};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Size, Style};

use super::{WidgetKind, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckboxState {
    pub label: String,
    /// `accesskit::Toggled` directly, not a second, parallel tri-state
    /// enum — the same "reuse `accesskit`'s own vocabulary" discipline
    /// [`WidgetId`] and [`crate::FocusManager`] already established.
    /// `Toggled::Mixed` is the indeterminate state (e.g. a "select all"
    /// checkbox over a partial selection).
    pub checked: Toggled,
    pub disabled: bool,
}

fn node(state: &CheckboxState) -> Node {
    let mut node = Node::new(Role::CheckBox);
    node.set_label(state.label.clone());
    node.set_toggled(state.checked);
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
    }
    node
}

fn style(scales: &Scales) -> Style {
    // No dedicated "control size" token exists yet -- grounded in the
    // type scale (about one line of text) rather than an invented
    // literal; see `type_size`'s own doc comment.
    let side = length(type_size(scales.typography.size.md));
    Style {
        size: Size {
            width: side,
            height: side,
        },
        ..Default::default()
    }
}

/// Adds a new, unchecked, enabled checkbox as the last child of `parent`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_checkbox(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
) -> Result<WidgetId, WidgetError> {
    let state = CheckboxState {
        label: label.into(),
        checked: Toggled::False,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::Checkbox(state),
    )
}

/// Toggles `id` (a checkbox): unchecked -> checked, checked -> unchecked,
/// and — matching every mainstream toolkit's own convention for a
/// "select all" style indeterminate checkbox — mixed -> checked. Returns
/// the new state.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a checkbox, or
/// [`WidgetError::WidgetDisabled`] if it's disabled.
pub fn toggle_checkbox(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<Toggled, WidgetError> {
    let mut result = Toggled::False;
    with_checkbox_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        state.checked = match state.checked {
            Toggled::True => Toggled::False,
            Toggled::False | Toggled::Mixed => Toggled::True,
        };
        result = state.checked;
        Ok(())
    })?;
    Ok(result)
}

/// Sets `id`'s checked state directly — an owner-driven change (the
/// document's own value changed, an undo), not a user gesture, so unlike
/// [`toggle_checkbox`] it is allowed on a **disabled** checkbox, the same
/// distinction `set_dropdown_selected` draws. Setting the state it
/// already has changes nothing: no accessibility rebuild, no damage.
/// A real change rebuilds the accessibility node (so `toggled` reports
/// the new state) and marks the control's bounds dirty so it repaints.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a checkbox.
/// Nothing changes when either happens.
pub fn set_checkbox_checked(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    checked: Toggled,
) -> Result<(), WidgetError> {
    match tree.payload(id) {
        None => return Err(WidgetError::UnknownWidget(id)),
        Some(WidgetKind::Checkbox(state)) => {
            if state.checked == checked {
                return Ok(());
            }
        }
        Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
    }
    with_checkbox_mut(tree, id, |state| {
        state.checked = checked;
        Ok(())
    })?;
    tree.mark_dirty(id)
}

/// Sets whether `id` (a checkbox) is disabled.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a checkbox.
pub fn set_checkbox_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_checkbox_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_checkbox_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut CheckboxState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Checkbox(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::Checkbox(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a Checkbox above");
    };
    tree.set_accessibility(id, node(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{insert_checkbox, set_checkbox_checked, set_checkbox_disabled, toggle_checkbox};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::{Action, Toggled};
    use taffy::Style;

    #[test]
    fn insert_checkbox_creates_a_fresh_unchecked_enabled_checkbox() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "Enabled") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Checkbox(state)) => {
                assert_eq!(state.checked, Toggled::False);
                assert!(!state.disabled);
            }
            other => unreachable!("expected Checkbox, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.toggled(), Some(Toggled::False));
        assert!(accessibility.supports_action(Action::Click));
    }

    #[test]
    fn toggle_checkbox_cycles_false_true_false() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match toggle_checkbox(&mut tree, id) {
            Ok(Toggled::True) => {}
            other => unreachable!("expected Ok(Toggled::True), got {other:?}"),
        }
        match toggle_checkbox(&mut tree, id) {
            Ok(Toggled::False) => {}
            other => unreachable!("expected Ok(Toggled::False), got {other:?}"),
        }
    }

    #[test]
    fn toggle_checkbox_resolves_mixed_to_checked() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(WidgetKind::Checkbox(state)) = tree.payload_mut(id) else {
            unreachable!("just inserted");
        };
        state.checked = Toggled::Mixed;

        match toggle_checkbox(&mut tree, id) {
            Ok(Toggled::True) => {}
            other => unreachable!("expected Ok(Toggled::True), got {other:?}"),
        }
    }

    #[test]
    fn toggle_checkbox_marks_the_widget_dirty() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();
        if let Err(err) = toggle_checkbox(&mut tree, id) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
    }

    #[test]
    fn toggle_checkbox_rejects_a_disabled_checkbox() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_checkbox_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match toggle_checkbox(&mut tree, id) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn checkbox_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match toggle_checkbox(&mut tree, root) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }
    #[test]
    fn set_checkbox_checked_sets_the_value_and_the_accessibility_toggle() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "Visible") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_checkbox_checked(&mut tree, id, Toggled::True) {
            unreachable!("{err:?}");
        }
        match tree.payload(id) {
            Some(WidgetKind::Checkbox(state)) => assert_eq!(state.checked, Toggled::True),
            other => unreachable!("expected Checkbox, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.toggled(), Some(Toggled::True));
    }

    #[test]
    fn set_checkbox_checked_is_allowed_on_a_disabled_checkbox() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "Visible") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_checkbox_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        assert!(
            matches!(toggle_checkbox(&mut tree, id), Err(WidgetError::WidgetDisabled(got)) if got == id),
            "a user toggle is refused while disabled"
        );
        if let Err(err) = set_checkbox_checked(&mut tree, id, Toggled::True) {
            unreachable!("an owner-driven set must succeed while disabled: {err:?}");
        }
        match tree.payload(id) {
            Some(WidgetKind::Checkbox(state)) => {
                assert_eq!(state.checked, Toggled::True);
                assert!(state.disabled, "setting the value must not re-enable it");
            }
            other => unreachable!("expected Checkbox, got {other:?}"),
        }
    }

    #[test]
    fn set_checkbox_checked_to_the_same_value_causes_no_damage() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_checkbox(&mut tree, root, &scales, "Visible") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(200.0, 200.0);
        tree.take_damage();
        if let Err(err) = set_checkbox_checked(&mut tree, id, Toggled::False) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.take_damage(),
            None,
            "an unchanged value repaints nothing"
        );
        if let Err(err) = set_checkbox_checked(&mut tree, id, Toggled::True) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
        assert!(
            tree.take_damage().is_some(),
            "a real change reaches the damage region"
        );
    }

    #[test]
    fn set_checkbox_checked_rejects_a_non_checkbox() {
        let (mut tree, root) = new_tree(Style::default());
        assert!(matches!(
            set_checkbox_checked(&mut tree, root, Toggled::True),
            Err(WidgetError::WrongWidgetKind(got)) if got == root
        ));
    }
}
