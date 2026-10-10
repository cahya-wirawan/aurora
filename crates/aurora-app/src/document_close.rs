//! Unsaved changes, Close Document and the quit review (0.174.0,
//! document tabs round R5).
//!
//! A document is *unsaved* ([`DocumentSession::is_dirty`]) when its
//! `UndoOrder` revision is not the one it was last opened or saved as
//! `.aur` at. The tab strip shows it as "• name" with an accessible
//! description of [`UNSAVED_DESCRIPTION`], relabelled only when that
//! changes ([`refresh_dirty_marks`]).
//!
//! Closing ([`begin_close`]) commits the document's live gestures first,
//! then closes a clean document at once and asks about an unsaved one
//! ([`open_close_dialog`]: Save, the default; Don't Save; Cancel, also
//! `Escape`). The dialog names its document by [`DocumentId`], never by
//! position or "the active one", so its answer applies to the document it
//! was opened for. A close ([`close_document`]) activates the next tab,
//! else the previous one, else a fresh "Untitled" document, and only then
//! is the closed session retired ([`retire_session`]): its autosave slot
//! superseded, so no in-flight write can re-create its file, and its tile
//! store's writer joined before its scratch files go.
//!
//! The quit review ([`QuitReview`]) asks about each unsaved document in
//! tab order, one dialog at a time. Cancel, a cancelled picker or a failed
//! save abort the whole quit and touch nothing; only an empty list quits.
//! Everything here is a free function or a plain value so it can be
//! tested without a window; `App` only sequences the dialogs.

use aurora_theme::Scales;
use aurora_widgets::FocusManager;
use aurora_widgets::widgets::DialogAction;

use crate::document_session::{DocumentId, DocumentSession, DocumentShelf};
use crate::document_tabs::{
    SwitchContext, bind_active_document, commit_live_gestures, sync_document_tab_bar,
};
use crate::{DialogPurpose, OpenDialog};

/// The palette and menu id of Close Document (`Ctrl+W`).
pub(crate) const COMMAND_FILE_CLOSE: &str = "file.close";
/// The close and quit dialogs' three actions.
pub(crate) const CLOSE_SAVE: &str = "close.save";
pub(crate) const CLOSE_DONT_SAVE: &str = "close.dont_save";
pub(crate) const CLOSE_CANCEL: &str = "close.cancel";
/// What an unsaved document's tab label starts with: a text bullet, so
/// no new token or icon (the design owner may replace it).
pub(crate) const UNSAVED_MARK: &str = "\u{2022}";
/// An unsaved document's tab's accessible description.
pub(crate) const UNSAVED_DESCRIPTION: &str = "unsaved";

/// What the user chose in a close or quit dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseChoice {
    Save,
    DontSave,
    Cancel,
}

/// The choice an action id names (`None` for any other id).
pub(crate) fn close_choice(action: &str) -> Option<CloseChoice> {
    match action {
        CLOSE_SAVE => Some(CloseChoice::Save),
        CLOSE_DONT_SAVE => Some(CloseChoice::DontSave),
        CLOSE_CANCEL => Some(CloseChoice::Cancel),
        _ => None,
    }
}

/// Save (default, `Enter`), Don't Save, Cancel (`Escape`).
pub(crate) fn close_dialog_actions() -> Vec<DialogAction> {
    vec![
        DialogAction::new(CLOSE_SAVE, "Save").as_default(),
        DialogAction::new(CLOSE_DONT_SAVE, "Don't Save"),
        DialogAction::new(CLOSE_CANCEL, "Cancel").as_cancel(),
    ]
}

/// The dialog's title: "Save changes to “name” before closing?" (or
/// "quitting?").
pub(crate) fn close_dialog_title(name: &str, quitting: bool) -> String {
    let when = if quitting { "quitting" } else { "closing" };
    format!("Save changes to \u{201c}{name}\u{201d} before {when}?")
}

pub(crate) const CLOSE_DIALOG_MESSAGE: &str = "Your changes will be lost if you don't save them.";

/// Opens the close (or, `quitting`, the quit-review) dialog for `id`,
/// named `name`. A no-op returning `false` when a dialog is already open.
pub(crate) fn open_close_dialog(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<OpenDialog>,
    scales: &Scales,
    id: DocumentId,
    name: &str,
    quitting: bool,
) -> bool {
    let purpose = if quitting {
        DialogPurpose::QuitReview(id)
    } else {
        DialogPurpose::CloseDocument(id)
    };
    crate::open_dialog(
        workspace,
        focus,
        dialog,
        scales,
        &close_dialog_title(name, quitting),
        CLOSE_DIALOG_MESSAGE,
        close_dialog_actions(),
        purpose,
    )
}

pub(crate) const EXPORTED_NOT_SAVED_DISMISS: &str = "close.exported.dismiss";

/// "“name” Was Exported, Not Saved" (0.174.0 review Q-2): a close or
/// quit dialog's Save went to a non-`.aur` name, which exports a flat
/// image and leaves the document unsaved — said, not silent.
pub(crate) fn exported_not_saved_title(name: &str) -> String {
    format!("\u{201c}{name}\u{201d} Was Exported, Not Saved")
}

pub(crate) const EXPORTED_NOT_SAVED_MESSAGE: &str = "It still has unsaved changes. To save it, \
     choose Save again and pick a name ending in .aur.";

/// Opens the exported-not-saved alert (a single OK). A no-op returning
/// `false` when a dialog is already open (an export failure's own).
pub(crate) fn open_exported_not_saved_dialog(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<OpenDialog>,
    scales: &Scales,
    name: &str,
) -> bool {
    crate::open_dialog(
        workspace,
        focus,
        dialog,
        scales,
        &exported_not_saved_title(name),
        EXPORTED_NOT_SAVED_MESSAGE,
        vec![crate::acknowledge_action(EXPORTED_NOT_SAVED_DISMISS, "OK")],
        DialogPurpose::ExportedNotSaved,
    )
}

/// What a close or quit dialog's Save does with the picked path (review
/// Q-2): only a `.aur` name is a save; anything else is an export that
/// leaves the document unsaved and is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogSave {
    Aur,
    ExportOnly,
}

pub(crate) fn dialog_save_kind(path: &std::path::Path) -> DialogSave {
    if crate::is_aur_path(path) {
        DialogSave::Aur
    } else {
        DialogSave::ExportOnly
    }
}

/// Whether a window close request that waited for an open dialog
/// (review Q-3) should start the quit review now.
pub(crate) fn pending_quit_ready(pending: bool, dialog_open: bool, reviewing: bool) -> bool {
    pending && !dialog_open && !reviewing
}

/// Whether a shutdown must keep this run's recovery data (review Q-1):
/// the quit was never confirmed by the review (macOS menu Quit, an OS
/// session end, any exit that bypassed `CloseRequested`) and something
/// is unsaved. Then the marker, the index and every session autosave
/// stay, so the next start offers recovery; only scratch tiles go. A
/// confirmed quit (the review reached `QuitStep::Quit`, Don't Save
/// included, or nothing was unsaved) cleans up as before.
pub(crate) fn keeps_unsaved_work(quit_confirmed: bool, unsaved: &[DocumentId]) -> bool {
    !quit_confirmed && !unsaved.is_empty()
}

/// The last autosave of every unsaved session before an unconfirmed
/// exit (review Q-1), written synchronously on this thread: each one's
/// worker slot is superseded first, so no older write lands over it, and
/// the index is told it is complete. Returns how many were written.
pub(crate) fn final_autosave_unsaved(
    worker: &mut crate::background_autosave::AutosaveWorker,
    namespace: &crate::autosave_files::AutosaveNamespace,
    shelf: &mut DocumentShelf,
    active: &mut DocumentSession,
) -> usize {
    let mut written = 0;
    for session in std::iter::once(active).chain(shelf.parked.iter_mut()) {
        if !session.is_dirty() {
            continue;
        }
        let id = session.id.get();
        let Some(store) = session.tile_store.as_mut() else {
            continue;
        };
        let _ = worker.supersede(id);
        crate::write_autosave(
            &namespace.session_path(id),
            &session.layers,
            &session.history,
            session.canvas_size,
            &mut session.skipped_tiles,
            store,
        );
        let _ = worker.note_written(id);
        session.park_autosave = None;
        written += 1;
    }
    written
}

/// What a close request needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseStep {
    /// Clean: close at once, no dialog.
    Close(DocumentId),
    /// Unsaved: ask first.
    Ask(DocumentId),
    /// No open document has this id.
    Unknown,
}

/// Starts closing `id`: commits the active document's live gestures (so a
/// stroke in progress counts as an unsaved change), then decides.
pub(crate) fn begin_close(cx: &mut SwitchContext<'_>, id: DocumentId) -> CloseStep {
    let dirty = if id == cx.doc.id {
        let _ = commit_live_gestures(cx);
        cx.doc.is_dirty()
    } else if let Some(session) = cx.shelf.get(id) {
        session.is_dirty()
    } else {
        return CloseStep::Unknown;
    };
    if dirty {
        CloseStep::Ask(id)
    } else {
        CloseStep::Close(id)
    }
}

/// Closes `id` and returns its session, for [`retire_session`]. The active
/// document is replaced by the next tab, else the previous one, else
/// `replacement` (closing the last document), and the panels, tab strip
/// and focus rebind to it; a parked document just leaves the strip.
/// `None` (nothing changed) for an unknown id, or for the last document
/// with no replacement.
pub(crate) fn close_document(
    cx: &mut SwitchContext<'_>,
    id: DocumentId,
    replacement: Option<DocumentSession>,
) -> Option<DocumentSession> {
    if id == cx.doc.id {
        // Whatever was live belonged to the closing document.
        let _ = commit_live_gestures(cx);
        let closed = cx.shelf.close_active(cx.doc, replacement)?;
        bind_active_document(cx);
        Some(closed)
    } else {
        let closed = cx.shelf.remove(id)?;
        let _ = sync_document_tab_bar(cx.workspace, cx.focus, cx.shelf, cx.doc);
        cx.focus.validate(&cx.workspace.tree);
        Some(closed)
    }
}

/// Retires a closed session: its autosave slot is superseded first (a
/// waiting write is dropped and one in flight is refused at its rename,
/// so nothing re-creates the file the index update then removes), then
/// its tile store is discarded — the store's writer is joined before any
/// of its scratch files is removed. Returns how many scratch files went.
/// The autosave file and index entry themselves go when the caller next
/// tells the worker the open sessions
/// (`document_tabs::sync_autosave_sessions`): the index is rewritten
/// without the session, and its file is deleted once that has landed.
pub(crate) fn retire_session(
    mut closed: DocumentSession,
    worker: Option<&mut crate::background_autosave::AutosaveWorker>,
) -> usize {
    if let Some(worker) = worker {
        let _ = worker.supersede(closed.id.get());
    }
    closed
        .tile_store
        .take()
        .map_or(0, aurora_tile::TileStore::discard)
}

/// Records a save's outcome on `doc` (0.174.0): only a landed `.aur`
/// save makes the document clean and gives it this path and name; an
/// export (any other extension) or a failed save changes nothing.
/// Returns whether the document is now saved.
pub(crate) fn after_save(doc: &mut DocumentSession, path: &std::path::Path, landed: bool) -> bool {
    if !(landed && crate::is_aur_path(path)) {
        return false;
    }
    doc.mark_clean();
    doc.path = Some(path.to_path_buf());
    doc.name = crate::display_file_name(path);
    true
}

/// Every unsaved document's id, in tab order.
pub(crate) fn unsaved_in_tab_order(
    shelf: &DocumentShelf,
    active: &DocumentSession,
) -> Vec<DocumentId> {
    shelf
        .tab_order(active)
        .into_iter()
        .filter(|&id| {
            if id == active.id {
                active.is_dirty()
            } else {
                // Unknown counts as unsaved: when in doubt, ask.
                shelf.get(id).is_none_or(DocumentSession::is_dirty)
            }
        })
        .collect()
}

/// A tab's label: "• name" when unsaved.
pub(crate) fn marked_label(name: &str, dirty: bool) -> String {
    if dirty {
        format!("{UNSAVED_MARK} {name}")
    } else {
        name.to_owned()
    }
}

/// Every tab's accessible description in tab order.
pub(crate) fn tab_descriptions(shelf: &DocumentShelf, active: &DocumentSession) -> Vec<String> {
    shelf
        .tab_order(active)
        .into_iter()
        .map(|id| {
            let dirty = if id == active.id {
                active.is_dirty()
            } else {
                shelf.get(id).is_some_and(DocumentSession::is_dirty)
            };
            if dirty {
                UNSAVED_DESCRIPTION.to_owned()
            } else {
                String::new()
            }
        })
        .collect()
}

/// Brings the tab strip's unsaved marks up to date — cheap enough for
/// every loop iteration: it compares the wanted labels and descriptions
/// with the strip's own state and touches the tree only when they
/// differ. Returns whether it relabelled.
pub(crate) fn refresh_dirty_marks(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    shelf: &mut DocumentShelf,
    active: &DocumentSession,
) -> bool {
    let (labels, _selected) = crate::document_tabs::tab_labels(shelf, active);
    let descriptions = tab_descriptions(shelf, active);
    let current = aurora_widgets::widgets::tab_bar_state(&workspace.tree, workspace.document_tabs)
        .ok()
        .map(|state| (state.labels().to_vec(), state.descriptions().to_vec()));
    if current.as_ref() == Some(&(labels, descriptions)) {
        return false;
    }
    sync_document_tab_bar(workspace, focus, shelf, active)
}

/// The quit review's progress: the unsaved documents still to ask about,
/// in tab order, the first being the one whose dialog is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuitReview {
    remaining: Vec<DocumentId>,
}

/// What the quit review needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitStep {
    /// Show this document and ask about it.
    Ask(DocumentId),
    /// Nothing left unsaved: run the clean-quit cleanup and exit.
    Quit,
    /// Cancelled (or a save did not land): keep running, every document
    /// and every autosave untouched.
    Abort,
}

impl QuitReview {
    /// Starts a review of `unsaved` (tab order): the review and its first
    /// step, or no review and [`QuitStep::Quit`] when nothing is unsaved.
    pub(crate) fn start(unsaved: Vec<DocumentId>) -> (Option<Self>, QuitStep) {
        match unsaved.first().copied() {
            Some(first) => (Some(Self { remaining: unsaved }), QuitStep::Ask(first)),
            None => (None, QuitStep::Quit),
        }
    }

    /// The document being asked about.
    pub(crate) fn current(&self) -> Option<DocumentId> {
        self.remaining.first().copied()
    }

    /// Applies the answer for `id`. `saved` is whether a Save landed (the
    /// document is clean); it is ignored for the other choices. An answer
    /// for any document but the current one aborts — a review never
    /// skips a document it has not asked about.
    pub(crate) fn answer(&mut self, id: DocumentId, choice: CloseChoice, saved: bool) -> QuitStep {
        if self.current() != Some(id) {
            return QuitStep::Abort;
        }
        match choice {
            CloseChoice::Cancel => QuitStep::Abort,
            CloseChoice::Save if !saved => QuitStep::Abort,
            CloseChoice::Save | CloseChoice::DontSave => {
                self.remaining.remove(0);
                self.current().map_or(QuitStep::Quit, QuitStep::Ask)
            }
        }
    }
}

/// What `App` does for a quit step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitEffect {
    /// Switch to this document and open its dialog.
    AskAbout(DocumentId),
    /// Run the clean-quit cleanup (`App::finish_shutdown`) and exit.
    CleanupAndExit,
    /// End the review; run nothing.
    KeepRunning,
}

pub(crate) fn quit_effect(step: QuitStep) -> QuitEffect {
    match step {
        QuitStep::Ask(id) => QuitEffect::AskAbout(id),
        QuitStep::Quit => QuitEffect::CleanupAndExit,
        QuitStep::Abort => QuitEffect::KeepRunning,
    }
}

/// What a close dialog's answer does, given whether a Save landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseEffect {
    /// Close this document (Don't Save, or Save once it landed).
    Close(DocumentId),
    /// Keep it open (Cancel, a cancelled picker, a failed save).
    Keep,
}

pub(crate) fn close_effect(id: DocumentId, choice: CloseChoice, saved: bool) -> CloseEffect {
    match choice {
        CloseChoice::DontSave => CloseEffect::Close(id),
        CloseChoice::Save if saved => CloseEffect::Close(id),
        CloseChoice::Save | CloseChoice::Cancel => CloseEffect::Keep,
    }
}
