//! A modal dialog: a title, a message, and a row of labeled actions —
//! the generic mechanism `aurora-app`'s crash-recovery prompt (PLAN.md
//! M1.8's "crash recovery UI" bullet) is built from, kept here because
//! nothing about it is document-specific. A confirmation/alert dialog is
//! exactly the kind of reusable interaction shape this crate already
//! covers — [`super::command_palette`] is the nearest precedent: a real,
//! focusable overlay inserted into the tree on demand, not a
//! hidden-flag widget.
//!
//! `Role::AlertDialog`, not the plainer `Role::Dialog` — every dialog
//! this crate can build today is an urgent, blocking prompt (a crash
//! was detected; something needs a decision before the user continues),
//! which is exactly what ARIA's `alertdialog` role names. A quieter,
//! non-urgent `Role::Dialog` variant is real, separate follow-on work if
//! this crate ever needs one.
//!
//! **Action *handling* is deliberately not this module's job**: it
//! builds the buttons and returns their ids; deciding what happens when
//! one is activated (closing the dialog, taking some action) is the
//! caller's — the same "generic mechanism, caller owns behaviour" split
//! [`super::command_palette`] already draws between its own tree
//! structure and `aurora-app`'s own command matching.
//!
//! **Click routing is the caller's job, same as keyboard**: a dialog's
//! action buttons are real `Button`s with `Action::Click` declared, and
//! this module builds them and returns their ids via
//! [`DialogHandle::action_id`] — deciding what a click (or `Enter` on a
//! focused button) actually *does* stays outside this module, the same
//! split this doc comment's own "action handling" paragraph already
//! draws for the keyboard. `aurora-app`'s `handle_dialog_pointer`
//! (added once that crate had real pointer input at all, PLAN.md M1.9)
//! is the first caller to actually do this.

use accesskit::{Node, Role};
use aurora_theme::Scales;
use taffy::Style;

use super::WidgetKind;
use super::button::insert_button;
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// One dialog action button — e.g. `("recover", "Recover Document")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogAction {
    /// Opaque to this crate — the caller defines what its own ids mean,
    /// the same "id is caller-defined" convention
    /// `super::command_palette::CommandEntry::id` already uses.
    pub id: String,
    pub label: String,
}

impl DialogAction {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// One inserted dialog's own widget ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogHandle {
    /// The dialog's own root — a labeled, modal `Role::AlertDialog`.
    pub root: WidgetId,
    /// A plain text node holding the dialog's own message body.
    pub message: WidgetId,
    /// Each action's own id (as passed to [`insert_dialog`]) paired with
    /// the real, focusable button widget built for it, in the same
    /// order they were given.
    pub actions: Vec<(String, WidgetId)>,
}

impl DialogHandle {
    /// The first action's own button, if any — the usual place to move
    /// focus to when a dialog opens.
    #[must_use]
    pub fn first_action(&self) -> Option<WidgetId> {
        self.actions.first().map(|(_, id)| *id)
    }

    /// The action id `button` belongs to, if `button` is one of this
    /// dialog's own action buttons — `None` for any other widget,
    /// including this dialog's own root or message.
    #[must_use]
    pub fn action_id(&self, button: WidgetId) -> Option<&str> {
        self.actions
            .iter()
            .find(|(_, id)| *id == button)
            .map(|(action_id, _)| action_id.as_str())
    }
}

/// Inserts a new modal dialog as the last child of `parent`: a labeled,
/// modal `Role::AlertDialog` holding a message and one real, focusable
/// button per entry in `actions`, in order.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_dialog(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    title: impl Into<String>,
    message: impl Into<String>,
    actions: Vec<DialogAction>,
) -> Result<DialogHandle, WidgetError> {
    let mut root_node = Node::new(Role::AlertDialog);
    root_node.set_label(title.into());
    root_node.set_modal();
    let root = tree.insert(parent, Style::default(), root_node, WidgetKind::Container)?;

    let mut message_node = Node::new(Role::Label);
    message_node.set_label(message.into());
    let message_id = tree.insert(root, Style::default(), message_node, WidgetKind::Container)?;

    let mut action_ids = Vec::with_capacity(actions.len());
    for action in actions {
        let button = insert_button(tree, root, scales, action.label)?;
        action_ids.push((action.id, button));
    }

    Ok(DialogHandle {
        root,
        message: message_id,
        actions: action_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::{DialogAction, insert_dialog};
    use crate::WidgetError;
    use crate::widgets::{new_tree, test_scales};
    use accesskit::Action;
    use taffy::Style;

    #[test]
    fn insert_dialog_builds_a_labeled_modal_alert_with_message_and_ordered_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Aurora Didn't Close Properly",
            "The previous session didn't shut down cleanly.",
            vec![
                DialogAction::new("recover", "Recover Document"),
                DialogAction::new("discard", "Discard"),
            ],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(accessibility) = tree.accessibility(handle.root) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::AlertDialog);
        assert_eq!(accessibility.label(), Some("Aurora Didn't Close Properly"));
        assert!(accessibility.is_modal());

        let Some(message_accessibility) = tree.accessibility(handle.message) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            message_accessibility.label(),
            Some("The previous session didn't shut down cleanly.")
        );

        assert_eq!(handle.actions.len(), 2);
        let Some((first_id, first_button)) = handle.actions.first() else {
            unreachable!("just asserted len() == 2");
        };
        assert_eq!(first_id, "recover");
        let Some(button_accessibility) = tree.accessibility(*first_button) else {
            unreachable!("just inserted");
        };
        assert_eq!(button_accessibility.label(), Some("Recover Document"));
        assert!(button_accessibility.supports_action(Action::Focus));
        assert!(button_accessibility.supports_action(Action::Click));

        assert_eq!(handle.first_action(), Some(*first_button));
        assert_eq!(handle.action_id(*first_button), Some("recover"));
    }

    #[test]
    fn first_action_is_none_for_a_dialog_with_no_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(&mut tree, root, &scales, "Title", "Message", vec![]) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(handle.first_action(), None);
    }

    #[test]
    fn action_id_returns_none_for_a_widget_that_is_not_one_of_this_dialogs_buttons() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let handle = match insert_dialog(
            &mut tree,
            root,
            &scales,
            "Title",
            "Message",
            vec![DialogAction::new("ok", "OK")],
        ) {
            Ok(handle) => handle,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(handle.action_id(handle.message), None);
        assert_eq!(handle.action_id(root), None);
    }

    #[test]
    fn insert_dialog_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        let bogus = accesskit::NodeId(999);
        match insert_dialog(&mut tree, bogus, &scales, "Title", "Message", vec![]) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
