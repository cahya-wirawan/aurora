//! The document strip's overflow menu (0.260.0): the "More documents"
//! button at the strip's end opens the toolkit's `Menu` listing every
//! open document, and choosing one switches to it.
//!
//! Like [`crate::menu_bar_state`], this is a set of free-function
//! transitions over borrowed state, so it is tested on a headless
//! workspace; `App` only calls it (`document_overflow_app`). One menu at a
//! time: the button toggles it, a press anywhere else closes it
//! (`light_dismiss`), `Escape` (the toolkit's `MenuCancelled`) closes it
//! and returns focus to the button, a modal closes it, and any change of
//! the open documents closes it (`close`, from the strip's resync) because
//! its rows would name documents that may be gone.
//!
//! The menu's rows carry the same labels the tabs draw (with the "• "
//! unsaved marker) in tab order; the active document's row is not marked
//! (a named gap).

use aurora_theme::Scales;
use aurora_ui::Workspace;
use aurora_widgets::widgets::MenuItem;
use aurora_widgets::{ActionOutcome, FocusManager, FocusOrigin, PointerOutcome, WidgetId};

use crate::document_session::{DocumentId, DocumentSession, DocumentShelf};

/// The open menu: its popup and the documents its rows name, in row order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenOverflow {
    pub(crate) menu: WidgetId,
    pub(crate) documents: Vec<DocumentId>,
}

/// What the overflow menu is doing.
#[derive(Debug, Default)]
pub(crate) struct OverflowState {
    pub(crate) open: Option<OpenOverflow>,
}

/// What a routed outcome on the button or its popup asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverflowAction {
    None,
    /// The button was activated: open the menu, or close it when open.
    Toggle,
    /// A row was chosen: switch to this document.
    Switch(DocumentId),
}

impl OverflowState {
    /// Forgets a popup the tree no longer holds.
    pub(crate) fn forget_stale(&mut self, workspace: &Workspace) {
        if self
            .open
            .as_ref()
            .is_some_and(|open| !workspace.tree.contains(open.menu))
        {
            self.open = None;
        }
    }

    /// Closes the menu; with `restore_focus`, focus that was inside it
    /// goes to the button. `true` when one was open.
    pub(crate) fn close(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        restore_focus: bool,
    ) -> bool {
        let Some(open) = self.open.take() else {
            return false;
        };
        let focus_inside = focus
            .focused()
            .is_none_or(|focused| workspace.tree.is_within(open.menu, focused));
        if workspace.tree.contains(open.menu)
            && let Err(err) = aurora_widgets::widgets::close_menu(&mut workspace.tree, open.menu)
        {
            tracing::warn!(?err, "failed to close the document overflow menu");
        }
        focus.validate(&workspace.tree);
        if restore_focus && focus_inside {
            focus_button(workspace, focus);
        }
        true
    }

    /// Opens the menu listing every document of `shelf` and `active` in tab
    /// order, `widest_text` px being the widest row's text. `false` when
    /// a modal is open, the button is not shown, or the toolkit refuses.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
        shelf: &DocumentShelf,
        active: &DocumentSession,
        widest_text: f32,
        modal_open: bool,
    ) -> bool {
        self.forget_stale(workspace);
        let shown = workspace
            .tree
            .style(workspace.document_overflow)
            .is_some_and(|style| style.display != taffy::Display::None);
        if modal_open || !shown {
            return false;
        }
        self.close(workspace, focus, false);
        let documents = shelf.tab_order(active);
        let (labels, _) = crate::document_tabs::tab_labels(shelf, active);
        if documents.len() != labels.len() {
            return false;
        }
        let items: Vec<MenuItem> = labels.iter().map(|label| MenuItem::action(label)).collect();
        let width = aurora_ui::menu_popup_width(scales, widest_text);
        let menu = match aurora_ui::open_document_overflow_menu(workspace, scales, width, items) {
            Ok(menu) => menu,
            Err(err) => {
                tracing::warn!(?err, "failed to open the document overflow menu");
                return false;
            }
        };
        if let Err(err) = focus.focus_with(&mut workspace.tree, menu, FocusOrigin::Keyboard) {
            tracing::warn!(?err, "failed to focus the document overflow menu");
        }
        self.open = Some(OpenOverflow { menu, documents });
        true
    }

    /// A press outside the button and the popup closes the menu.
    pub(crate) fn light_dismiss(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        point: (f32, f32),
    ) -> bool {
        self.forget_stale(workspace);
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        let inside = workspace.tree.hit_test(point).is_some_and(|hit| {
            workspace.tree.is_within(open.menu, hit) || hit == workspace.document_overflow
        });
        !inside && self.close(workspace, focus, false)
    }

    /// Per-turn housekeeping: forgets a popup the tree dropped and closes
    /// an open menu under a modal, or when the open documents (their set or
    /// order) are no longer the ones its rows name. `true` when it closed.
    pub(crate) fn sync(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        shelf: &DocumentShelf,
        active: &DocumentSession,
        modal_open: bool,
    ) -> bool {
        self.forget_stale(workspace);
        let changed = self
            .open
            .as_ref()
            .is_some_and(|open| open.documents != shelf.tab_order(active));
        if changed {
            self.close(workspace, focus, false)
        } else {
            self.close_for_modal(workspace, focus, modal_open)
        }
    }

    /// A modal (a dialog or the palette) closes an open menu.
    pub(crate) fn close_for_modal(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        modal_open: bool,
    ) -> bool {
        modal_open && self.close(workspace, focus, false)
    }

    /// What `outcome` means for the overflow menu. A chosen row's popup is
    /// already removed by the toolkit; a cancelled one too.
    pub(crate) fn action_for(
        &mut self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        outcome: &PointerOutcome,
    ) -> OverflowAction {
        match outcome {
            PointerOutcome::Action(ActionOutcome::Activated(id))
                if *id == workspace.document_overflow =>
            {
                self.forget_stale(workspace);
                OverflowAction::Toggle
            }
            PointerOutcome::Action(ActionOutcome::MenuActivated { menu, index }) => {
                let Some(open) = self.open.take_if(|open| open.menu == *menu) else {
                    return OverflowAction::None;
                };
                focus.validate(&workspace.tree);
                focus_button(workspace, focus);
                open.documents
                    .get(*index)
                    .map_or(OverflowAction::None, |&id| OverflowAction::Switch(id))
            }
            PointerOutcome::MenuCancelled { menu } => {
                if self.open.take_if(|open| open.menu == *menu).is_some() {
                    focus.validate(&workspace.tree);
                    focus_button(workspace, focus);
                }
                OverflowAction::None
            }
            _ => OverflowAction::None,
        }
    }
}

/// Focus on the overflow button, if it can take it (shown).
fn focus_button(workspace: &mut Workspace, focus: &mut FocusManager) {
    if let Err(err) = focus.focus_with(
        &mut workspace.tree,
        workspace.document_overflow,
        FocusOrigin::Keyboard,
    ) {
        tracing::debug!(?err, "the document overflow button is not focusable");
    }
}
