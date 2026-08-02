//! A push button: a discrete trigger, the simplest of the three
//! interaction shapes this first widget slice covers.

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Rect as LayoutRect, Style};

use super::{WidgetKind, spacing};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonState {
    pub label: String,
    pub pressed: bool,
    pub disabled: bool,
}

fn node(state: &ButtonState) -> Node {
    let mut node = Node::new(Role::Button);
    node.set_label(state.label.clone());
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
    }
    node
}

fn style(scales: &Scales) -> Style {
    Style {
        padding: LayoutRect {
            left: length(spacing(scales.spacing.md)),
            right: length(spacing(scales.spacing.md)),
            top: length(spacing(scales.spacing.sm)),
            bottom: length(spacing(scales.spacing.sm)),
        },
        ..Default::default()
    }
}

/// Adds a new button as the last child of `parent`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_button(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
) -> Result<WidgetId, WidgetError> {
    let state = ButtonState {
        label: label.into(),
        pressed: false,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::Button(state),
    )
}

/// Sets whether `id` (a button) is currently pressed — e.g. while the
/// pointer is held down on it. Does not fire a click; that's the
/// caller's own job (typically: press on pointer-down, and if the
/// pointer is still over the button on pointer-up, treat that as the
/// click and release).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a button, or
/// [`WidgetError::WidgetDisabled`] if it's disabled.
pub fn set_button_pressed(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    pressed: bool,
) -> Result<(), WidgetError> {
    with_button_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        state.pressed = pressed;
        Ok(())
    })
}

/// Sets whether `id` (a button) is disabled. A disabled button loses its
/// `Focus`/`Click` actions and can no longer be pressed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a button.
pub fn set_button_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_button_mut(tree, id, |state| {
        state.disabled = disabled;
        if disabled {
            state.pressed = false;
        }
        Ok(())
    })
}

/// Mutates `id`'s [`ButtonState`] via `f`, then rebuilds and applies its
/// accessibility node from the result — the one place every button
/// mutator in this module goes through, so "update state, then
/// re-derive the node" can't drift out of sync between them.
fn with_button_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut ButtonState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Button(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::Button(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a Button above");
    };
    tree.set_accessibility(id, node(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ButtonState, insert_button, set_button_disabled, set_button_pressed};
    use crate::WidgetError;
    use crate::tree::WidgetTree;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::Action;
    use taffy::Style;

    #[test]
    fn insert_button_creates_a_fresh_enabled_unpressed_button() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.payload(id),
            Some(&WidgetKind::Button(ButtonState {
                label: "OK".to_owned(),
                pressed: false,
                disabled: false,
            }))
        );
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.label(), Some("OK"));
        assert!(accessibility.supports_action(Action::Focus));
        assert!(accessibility.supports_action(Action::Click));
    }

    #[test]
    fn insert_button_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        let bogus = accesskit::NodeId(999);
        match insert_button(&mut tree, bogus, &scales, "OK") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_button_pressed_updates_state_and_marks_dirty() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();

        if let Err(err) = set_button_pressed(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match tree.payload(id) {
            Some(WidgetKind::Button(state)) => assert!(state.pressed),
            other => unreachable!("expected Button, got {other:?}"),
        }
        assert_eq!(tree.is_dirty(id), Some(true));
    }

    #[test]
    fn set_button_pressed_rejects_a_disabled_button() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_button_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match set_button_pressed(&mut tree, id, true) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn set_button_disabled_clears_pressed_and_the_accesskit_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_button_pressed(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_button_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }

        match tree.payload(id) {
            Some(WidgetKind::Button(state)) => {
                assert!(state.disabled);
                assert!(!state.pressed, "disabling must clear a stale pressed state");
            }
            other => unreachable!("expected Button, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert!(accessibility.is_disabled());
        assert!(!accessibility.supports_action(Action::Focus));
        assert!(!accessibility.supports_action(Action::Click));
    }

    #[test]
    fn button_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_button_pressed(&mut tree, root, true) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn button_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = WidgetTree::new(
            accesskit::Node::new(accesskit::Role::GenericContainer),
            Style::default(),
            WidgetKind::Container,
        );
        let bogus = accesskit::NodeId(999);
        match set_button_pressed(&mut tree, bogus, true) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
