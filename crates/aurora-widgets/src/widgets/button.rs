//! A push button: a discrete trigger, the simplest of the three
//! interaction shapes this first widget slice covers.
//!
//! **Toggle buttons (0.160.0).** A button built with
//! [`insert_toggle_button`] also carries an on/off state
//! ([`ButtonState::toggled`]), exposed to assistive technology as
//! accesskit's `toggled` property on a `Role::Button` — the accesskit
//! spelling of a toggle button, which a screen reader announces as
//! "selected"/"pressed". Clicking one does **not** flip it: the caller
//! decides what a click means (the tools panel treats its buttons as a
//! radio group) and sets the state with [`set_button_toggled`]. An "on"
//! toggle is painted like a push button (`accent.primary`, label in
//! `text.on_accent`); an "off" one has no fill, only the control outline,
//! with its label in `text.primary` — both pairs already gated by
//! `design/check_contrast.py`. No hover state exists for any button yet.

use accesskit::{Action, Node, Role, Toggled};
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
    /// `None` for a push button; `Some(on)` for a toggle button
    /// ([`insert_toggle_button`]).
    pub toggled: Option<bool>,
}

impl ButtonState {
    /// Whether the button is painted with the accent fill (and so its
    /// label in `text.on_accent`): a push button always, a toggle button
    /// when on or while held down.
    #[must_use]
    pub const fn fills_accent(&self) -> bool {
        self.pressed || !matches!(self.toggled, Some(false))
    }
}

fn node(state: &ButtonState) -> Node {
    let mut node = Node::new(Role::Button);
    node.set_label(state.label.clone());
    if let Some(on) = state.toggled {
        node.set_toggled(if on { Toggled::True } else { Toggled::False });
    }
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
        toggled: None,
    };
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::Button(state),
    )
}

/// Adds a new **toggle** button, initially `on` or off, as the last child
/// of `parent` — see this module's own doc comment.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_toggle_button(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
    on: bool,
) -> Result<WidgetId, WidgetError> {
    let state = ButtonState {
        label: label.into(),
        pressed: false,
        disabled: false,
        toggled: Some(on),
    };
    // One control row plus its vertical padding, text-blind or measured
    // alike (`measure::measure_widget` gives the same height), so a
    // headless layout and the app's text-aware one agree on height.
    let mut toggle_style = style(scales);
    toggle_style.size.height = length(super::row_height(scales) + 2.0 * spacing(scales.spacing.sm));
    tree.insert(
        parent,
        toggle_style,
        node(&state),
        WidgetKind::Button(state),
    )
}

/// Sets toggle button `id` on or off, returning whether anything changed.
/// Works on a disabled toggle too (its state can still follow the app).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it is not a button or is a plain
/// push button (one with no toggle state to set).
pub fn set_button_toggled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    on: bool,
) -> Result<bool, WidgetError> {
    match tree.payload(id) {
        Some(WidgetKind::Button(state)) => match state.toggled {
            Some(current) if current == on => return Ok(false),
            Some(_) => {}
            None => return Err(WidgetError::WrongWidgetKind(id)),
        },
        Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
        None => return Err(WidgetError::UnknownWidget(id)),
    }
    with_button_mut(tree, id, |state| {
        state.toggled = Some(on);
        Ok(())
    })?;
    Ok(true)
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
    use super::{
        ButtonState, insert_button, insert_toggle_button, set_button_disabled, set_button_pressed,
        set_button_toggled,
    };
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
                toggled: None,
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

    /// 0.160.0: a toggle button reports its state through accesskit's
    /// `toggled` property (a screen reader's "selected"), keeps `Click`,
    /// and follows `set_button_toggled` — which reports a real change only.
    #[test]
    fn a_toggle_button_exposes_and_follows_its_toggled_state() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style::default());
        let Ok(id) = insert_toggle_button(&mut tree, root, &scales, "Brush", false) else {
            unreachable!("root exists");
        };
        let toggled = |tree: &WidgetTree<WidgetKind>| {
            tree.accessibility(id).and_then(accesskit::Node::toggled)
        };
        assert_eq!(toggled(&tree), Some(accesskit::Toggled::False));
        assert!(
            tree.accessibility(id)
                .is_some_and(|node| node.role() == accesskit::Role::Button
                    && node.supports_action(Action::Click))
        );
        assert!(matches!(set_button_toggled(&mut tree, id, true), Ok(true)));
        assert_eq!(toggled(&tree), Some(accesskit::Toggled::True));
        assert!(matches!(set_button_toggled(&mut tree, id, true), Ok(false)));
        match tree.payload(id) {
            Some(WidgetKind::Button(state)) => {
                assert_eq!(state.toggled, Some(true));
                assert!(state.fills_accent());
            }
            other => unreachable!("{other:?}"),
        }
        assert!(matches!(set_button_toggled(&mut tree, id, false), Ok(true)));
        match tree.payload(id) {
            Some(WidgetKind::Button(state)) => assert!(!state.fills_accent()),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn a_push_button_has_no_toggle_state_to_set() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style::default());
        let Ok(id) = insert_button(&mut tree, root, &scales, "OK") else {
            unreachable!("root exists");
        };
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::toggled),
            None
        );
        assert!(matches!(
            set_button_toggled(&mut tree, id, true),
            Err(WidgetError::WrongWidgetKind(wrong)) if wrong == id
        ));
    }
}
