//! A static, one-line text label (0.136.0) — the first widget whose
//! *whole job* is visible text. `aurora-ui`'s Properties panel uses it
//! for the live "Radius 24 px" readout beside its radius slider.
//!
//! **What it is**: a `Role::Label` accessibility node carrying the text,
//! one [`super::row_height`] tall and as wide as its parent lays it out,
//! drawn by [`crate::text_runs`] in `text.secondary` (a pair
//! `design/check_contrast.py` gates at 4.5:1 on every panel surface) or,
//! disabled, `text.disabled`. It paints no solids — no well, no border,
//! no focus ring.
//!
//! **What it is not**: interactive. It exposes no actions (no `Focus`,
//! no `Click`), is never a tab stop, and hit-testing it simply finds the
//! label (a caller routing pointer input treats it as inert). There is
//! no wrapping, no ellipsis and no measure-func sizing: a line wider than
//! its box is clipped, like every other widget label in this crate.
//! Whether a readout like this should be `text.secondary` or
//! `text.primary` is the design owner's call (PRD FR-027 *Ownership*);
//! `text.secondary` is used because a readout is supporting text next
//! to the control that owns the value.

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Dimension, Size, Style};

use super::{WidgetKind, row_height};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// A label's own state: its text, and whether it is shown disabled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelState {
    /// The one line of text drawn and announced.
    pub text: String,
    /// Drawn in `text.disabled` and announced as disabled.
    pub disabled: bool,
}

fn node(state: &LabelState) -> Node {
    let mut node = Node::new(Role::Label);
    node.set_label(state.text.clone());
    if state.disabled {
        node.set_disabled();
    }
    node
}

fn style(scales: &Scales) -> Style {
    Style {
        size: Size {
            width: Dimension::auto(),
            height: length(row_height(scales)),
        },
        flex_shrink: 0.0,
        ..Default::default()
    }
}

/// Adds a new, enabled label showing `text` as the last child of
/// `parent`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_label(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    text: impl Into<String>,
) -> Result<WidgetId, WidgetError> {
    let state = LabelState {
        text: text.into(),
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::Label(state),
    )
}

/// Replaces `id`'s text. Returns whether anything changed: setting the
/// text it already shows touches nothing (no accessibility rebuild, no
/// damage), so a caller can run this on every event-loop iteration. A
/// real change rebuilds the accessibility node (so a screen reader reads
/// the new text) and marks the label's bounds dirty so it repaints.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a label.
pub fn set_label_text(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    text: &str,
) -> Result<bool, WidgetError> {
    if label_state(tree, id)?.text == text {
        return Ok(false);
    }
    with_label_mut(tree, id, |state| text.clone_into(&mut state.text))?;
    Ok(true)
}

/// Sets whether `id` (a label) is shown disabled. Returns whether
/// anything changed; an unchanged state touches nothing.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a label.
pub fn set_label_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<bool, WidgetError> {
    if label_state(tree, id)?.disabled == disabled {
        return Ok(false);
    }
    with_label_mut(tree, id, |state| state.disabled = disabled)?;
    Ok(true)
}

/// `id`'s label state.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a label.
pub fn label_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&LabelState, WidgetError> {
    match tree.payload(id) {
        None => Err(WidgetError::UnknownWidget(id)),
        Some(WidgetKind::Label(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
    }
}

fn with_label_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut LabelState),
) -> Result<(), WidgetError> {
    let rebuilt = {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Label(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state);
        node(state)
    };
    tree.set_accessibility(id, rebuilt)?;
    tree.mark_dirty(id)
}

#[cfg(test)]
mod tests {
    use super::{insert_label, label_state, set_label_disabled, set_label_text};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, insert_button, new_tree, row_height, test_scales};
    use accesskit::Role;
    use taffy::Style;

    #[test]
    fn insert_label_creates_an_enabled_role_label_with_its_text_and_no_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_label(&mut tree, root, &scales, "Size 24 px") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Label(state)) => {
                assert_eq!(state.text, "Size 24 px");
                assert!(!state.disabled);
            }
            other => unreachable!("expected Label, got {other:?}"),
        }
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::Label);
        assert_eq!(node.label(), Some("Size 24 px"));
        assert!(!node.is_disabled());
        assert!(!node.supports_action(accesskit::Action::Focus));
        assert!(!node.supports_action(accesskit::Action::Click));
    }

    #[test]
    fn a_label_is_one_row_tall_and_as_wide_as_its_column() {
        let (mut tree, root) = new_tree(Style {
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: taffy::style_helpers::length(300.0_f32),
                height: taffy::style_helpers::length(200.0_f32),
            },
            ..Style::default()
        });
        let scales = test_scales();
        let id = match insert_label(&mut tree, root, &scales, "x") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 200.0);
        let Some(bounds) = tree.bounds(id) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = row_height(&scales).round() as u32;
        assert_eq!(bounds.height, row);
        assert_eq!(bounds.width, 300, "a label stretches across its column");
    }

    #[test]
    fn set_label_text_updates_the_node_marks_dirty_and_is_a_no_op_when_unchanged() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_label(&mut tree, root, &scales, "Size 24 px") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 200.0);
        let _ = tree.take_damage();
        assert!(matches!(
            set_label_text(&mut tree, id, "Size 24 px"),
            Ok(false)
        ));
        assert!(
            tree.take_damage().is_none(),
            "an unchanged label must not damage anything"
        );
        assert!(matches!(
            set_label_text(&mut tree, id, "Size 40 px"),
            Ok(true)
        ));
        assert_eq!(
            label_state(&tree, id).ok().map(|state| state.text.as_str()),
            Some("Size 40 px")
        );
        assert_eq!(
            tree.accessibility(id).and_then(|node| node.label()),
            Some("Size 40 px")
        );
        assert!(tree.take_damage().is_some(), "a changed label repaints");
    }

    #[test]
    fn set_label_disabled_is_announced_and_idempotent() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_label(&mut tree, root, &scales, "Size") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(matches!(set_label_disabled(&mut tree, id, true), Ok(true)));
        assert!(matches!(set_label_disabled(&mut tree, id, true), Ok(false)));
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_disabled)
        );
        assert!(matches!(set_label_disabled(&mut tree, id, false), Ok(true)));
        assert!(
            !tree
                .accessibility(id)
                .is_some_and(accesskit::Node::is_disabled)
        );
    }

    #[test]
    fn label_setters_reject_unknown_and_wrong_kind_widgets() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let button = match insert_button(&mut tree, root, &scales, "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(matches!(
            set_label_text(&mut tree, button, "x"),
            Err(WidgetError::WrongWidgetKind(got)) if got == button
        ));
        assert!(matches!(
            set_label_disabled(&mut tree, button, true),
            Err(WidgetError::WrongWidgetKind(got)) if got == button
        ));
        assert!(tree.remove(button).is_ok());
        assert!(matches!(
            set_label_text(&mut tree, button, "x"),
            Err(WidgetError::UnknownWidget(got)) if got == button
        ));
    }
}
