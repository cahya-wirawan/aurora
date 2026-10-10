//! Several open documents and switching between them (0.172.0, document
//! tabs round R3).
//!
//! `App::doc` is the active [`DocumentSession`]; the others wait, intact,
//! on a [`DocumentShelf`]. Everything here is a free function over borrowed
//! state so it can be tested without a window.
//!
//! A switch ([`switch_document`]) runs in one fixed order:
//!
//! 1. live gestures are committed into the *outgoing* document's own
//!    history — a Layers-panel opacity drag, the Curves editor's gestures,
//!    and a Brush/Eraser stroke or Move, through the same commit path an
//!    undo or a History jump uses — never dropped;
//! 2. the outgoing session is parked at its own tab position and the
//!    target becomes active — after the outgoing session is autosaved
//!    under its own id ([`autosave_outgoing`], review J-1);
//! 3. the Layers and History panels are rebuilt from the incoming
//!    session's own layers, `undo_order` and `active_layer` (never the
//!    defaults, never the topmost layer), and focus left on a removed layer
//!    row moves to the incoming active row;
//! 4. the incoming view is re-clamped (kept, not reset);
//! 5. the composite cache is bumped and the GPU atlas forgets its slots
//!    (F1: composite tiles of two documents share ids, and a clean,
//!    resident tile would otherwise keep showing the previous document);
//! 6. the status bar is re-synced. The caller sets the window title,
//!    pushes accessibility and redraws.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use aurora_theme::Scales;
use aurora_widgets::{FocusManager, WidgetId};

use crate::document_session::{DocumentId, DocumentSession, DocumentShelf};
use crate::{
    ClickTracker, CurvesEdit, CurvesUiState, Drag, LayerControlEdit, LayerControlsState,
    ScrollFollow, commit_drag_into_history, end_curves_gestures, end_layer_control_gestures,
    populate_history_rows, rebuild_layer_rows, sync_layer_controls, sync_status_bar,
};

/// Everything a switch reads or rewrites, borrowed from `App`.
pub(crate) struct SwitchContext<'a> {
    pub(crate) workspace: &'a mut aurora_ui::Workspace,
    pub(crate) focus: &'a mut FocusManager,
    pub(crate) scales: &'a Scales,
    pub(crate) doc: &'a mut DocumentSession,
    pub(crate) shelf: &'a mut DocumentShelf,
    pub(crate) layer_rows: &'a mut HashMap<WidgetId, aurora_doc::LayerId>,
    pub(crate) scroll_follow: &'a mut ScrollFollow,
    pub(crate) drag: &'a mut Option<Drag>,
    pub(crate) layer_controls: &'a mut LayerControlsState,
    pub(crate) curves_ui: &'a mut CurvesUiState,
    pub(crate) tool_controls: Option<aurora_ui::ToolControls>,
    pub(crate) click: &'a mut ClickTracker,
    pub(crate) residency: Option<&'a mut aurora_gpu::TileResidency>,
    /// Where the outgoing document is autosaved when it is parked (J-1).
    /// `None` only in tests that do not exercise autosave.
    pub(crate) autosave: Option<ParkAutosave<'a>>,
    /// The canvas area's logical size, for the pan clamp.
    pub(crate) canvas_area: Option<(f32, f32)>,
    pub(crate) scale_factor: f64,
    /// A dialog or the palette is open, or a panel drag or rail resize is
    /// live: no switch.
    pub(crate) blocked: bool,
}

/// The autosave worker and this run's namespace, for the park autosave.
pub(crate) struct ParkAutosave<'a> {
    pub(crate) worker: &'a mut crate::background_autosave::AutosaveWorker,
    pub(crate) namespace: &'a crate::autosave_files::AutosaveNamespace,
}

/// Autosaves the active (outgoing) session under its own id and store
/// (review J-1, 0.172.0), right after its live gestures are committed and
/// before it is parked: until then a document edited and then left by a
/// switch or an open had no autosave of those edits, though it was still
/// open. The snapshot is taken here, on the UI thread, and written by the
/// worker in the session's own slot, so the incoming document's autosave
/// (another id, another slot) never supersedes it.
pub(crate) fn autosave_outgoing(cx: &mut SwitchContext<'_>) -> bool {
    let Some(park) = cx.autosave.as_mut() else {
        return false;
    };
    let id = cx.doc.id.get();
    let path = park.namespace.session_path(id);
    let Some(store) = cx.doc.tile_store.as_mut() else {
        tracing::warn!(id, "no tile store; the parked document is not autosaved");
        return false;
    };
    crate::request_autosave(
        park.worker,
        &path,
        id,
        &cx.doc.layers,
        &cx.doc.history,
        cx.doc.canvas_size,
        &mut cx.doc.skipped_tiles,
        store,
    );
    true
}

/// What [`switch_document`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOutcome {
    Switched,
    AlreadyActive,
    Blocked,
    Unknown,
}

/// Commits every live gesture into the active (outgoing) document's own
/// history (step 1 above). Returns whether anything was recorded.
pub(crate) fn commit_live_gestures(cx: &mut SwitchContext<'_>) -> bool {
    let mut recorded = end_layer_control_gestures(
        &mut LayerControlEdit {
            workspace: cx.workspace,
            layers: &mut cx.doc.layers,
            history: &mut cx.doc.history,
            pixel_history: &mut cx.doc.pixel_history,
            undo_order: &mut cx.doc.undo_order,
            layer_rows: cx.layer_rows,
            active_layer: cx.doc.active_layer,
            state: cx.layer_controls,
        },
        cx.click,
    );
    recorded |= end_curves_gestures(
        &mut CurvesEdit {
            workspace: cx.workspace,
            layers: &mut cx.doc.layers,
            history: &mut cx.doc.history,
            pixel_history: &mut cx.doc.pixel_history,
            undo_order: &mut cx.doc.undo_order,
            layer_rows: cx.layer_rows,
            active_layer: cx.doc.active_layer,
            controls: cx.tool_controls.map(|controls| controls.curves),
            state: cx.curves_ui,
        },
        cx.click,
    );
    recorded |= commit_drag_into_history(
        cx.workspace,
        cx.drag.take(),
        &cx.doc.layers,
        &mut cx.doc.history,
        &mut cx.doc.pixel_history,
        &mut cx.doc.undo_order,
        &mut cx.doc.canvas_view,
        cx.doc.active_layer,
        cx.canvas_area,
    );
    recorded
}

/// Rebinds the panels, view, caches and status bar to the (just
/// activated) `cx.doc` — steps 3 to 6 above.
pub(crate) fn bind_active_document(cx: &mut SwitchContext<'_>) {
    for body in [cx.workspace.layers.body, cx.workspace.history.body] {
        if let Err(err) = cx.workspace.tree.set_scroll_y(body, 0.0) {
            tracing::warn!(
                ?err,
                "failed to reset a panel's scroll for the incoming document"
            );
        }
    }
    // The session's own active layer is the preferred one; only a layer
    // that no longer exists falls back.
    let preferred = cx.doc.active_layer;
    rebuild_layer_rows(
        cx.workspace,
        cx.focus,
        cx.scales,
        &cx.doc.layers,
        cx.layer_rows,
        &mut cx.doc.active_layer,
        &mut cx.doc.canvas_view,
        preferred,
    );
    *cx.scroll_follow = ScrollFollow::default();
    if let Err(err) = populate_history_rows(cx.workspace, cx.scales, &cx.doc.undo_order) {
        tracing::warn!(
            ?err,
            "failed to rebuild the History panel for the incoming document"
        );
    }
    let _ = sync_layer_controls(
        cx.workspace,
        cx.layer_controls,
        &cx.doc.layers,
        cx.doc.active_layer,
        None,
    );
    // The Curves editor's histogram and shown layer belonged to the
    // outgoing document; the per-frame sync rebuilds them.
    *cx.curves_ui = CurvesUiState::default();
    crate::clamp_pan_to_active_layer(
        &mut cx.doc.canvas_view,
        &cx.doc.layers,
        cx.doc.active_layer,
        cx.canvas_area,
    );
    cx.doc.composite_cache.bump();
    if let Some(residency) = cx.residency.as_deref_mut() {
        residency.forget_slots();
    }
    cx.focus.validate(&cx.workspace.tree);
    let _ = sync_status_bar(
        cx.workspace,
        &cx.doc.canvas_view,
        cx.doc.canvas_size,
        cx.scale_factor,
    );
}

/// Switches to the parked document `target` (see this module's doc
/// comment for the order).
pub(crate) fn switch_document(cx: &mut SwitchContext<'_>, target: DocumentId) -> SwitchOutcome {
    if target == cx.doc.id {
        return SwitchOutcome::AlreadyActive;
    }
    if !cx.shelf.holds(target) {
        return SwitchOutcome::Unknown;
    }
    if cx.blocked {
        return SwitchOutcome::Blocked;
    }
    let _ = commit_live_gestures(cx);
    let _ = autosave_outgoing(cx);
    if !cx.shelf.swap_in(cx.doc, target) {
        return SwitchOutcome::Unknown;
    }
    bind_active_document(cx);
    SwitchOutcome::Switched
}

/// Makes a just-built session the active one, appended to the tab order:
/// live gestures commit into the outgoing document first, then the panels
/// bind to the new one. Returns where the outgoing one was parked.
pub(crate) fn activate_new_document(cx: &mut SwitchContext<'_>, session: DocumentSession) -> usize {
    let _ = commit_live_gestures(cx);
    let _ = autosave_outgoing(cx);
    let parked_at = cx.shelf.push_active(cx.doc, session);
    bind_active_document(cx);
    parked_at
}

/// The next "Untitled N" name for a New Document (`N` from 2: the startup
/// document is plain "Untitled").
pub(crate) fn next_untitled_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(2);
    format!(
        "{} {}",
        crate::document_session::UNTITLED,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The tile store a pending background open will install into (0.172.0):
/// each open decodes into a new session's own store, created when the open
/// starts (its staging root is what the decode thread writes into). Only
/// the newest open's store is kept; a superseded open's store is dropped
/// when the newer one starts, and its decode result is discarded by the
/// open worker's generation check.
#[derive(Default)]
pub(crate) struct PendingOpenStores {
    pending: Option<(u64, Option<aurora_tile::TileStore>)>,
}

impl PendingOpenStores {
    /// Records `store` for the open with `generation`, dropping any older
    /// one's. Returns the dropped generation.
    pub(crate) fn start(
        &mut self,
        generation: u64,
        store: Option<aurora_tile::TileStore>,
    ) -> Option<u64> {
        self.pending
            .replace((generation, store))
            .map(|(dropped, _store)| dropped)
    }

    /// The store for the finished open `generation`: `Some(store)` when it
    /// is the pending one (taken), `None` otherwise (left alone). The
    /// inner `Option` is the store itself, which may be absent (no scratch
    /// directory), the same `Option<TileStore>` a session holds.
    #[allow(clippy::option_option)]
    pub(crate) fn take(&mut self, generation: u64) -> Option<Option<aurora_tile::TileStore>> {
        match self.pending.take() {
            Some((pending, store)) if pending == generation => Some(store),
            other => {
                self.pending = other;
                None
            }
        }
    }

    /// Which open's store is held, if any.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> Option<u64> {
        self.pending.as_ref().map(|(generation, _)| *generation)
    }

    /// Drops the held store (quit: before the scratch directory goes).
    pub(crate) fn clear(&mut self) {
        self.pending = None;
    }
}

/// The palette id prefix of "Document: `name`" (followed by the id).
pub(crate) const COMMAND_DOCUMENT_SWITCH_PREFIX: &str = "document.switch.";
pub(crate) const COMMAND_DOCUMENT_NEXT: &str = "document.next";
pub(crate) const COMMAND_DOCUMENT_PREVIOUS: &str = "document.previous";
pub(crate) const COMMAND_FILE_NEW: &str = "file.new";

/// A document command a palette id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentCommand {
    New,
    Cycle { forward: bool },
    Activate(u64),
}

pub(crate) fn document_command_for(id: &str) -> Option<DocumentCommand> {
    match id {
        COMMAND_FILE_NEW => Some(DocumentCommand::New),
        COMMAND_DOCUMENT_NEXT => Some(DocumentCommand::Cycle { forward: true }),
        COMMAND_DOCUMENT_PREVIOUS => Some(DocumentCommand::Cycle { forward: false }),
        _ => id
            .strip_prefix(COMMAND_DOCUMENT_SWITCH_PREFIX)
            .and_then(|raw| raw.parse().ok())
            .map(DocumentCommand::Activate),
    }
}

/// "Document: `name`" for every open document, in tab order, the active
/// one included (choosing it does nothing).
pub(crate) fn document_palette_entries(
    shelf: &DocumentShelf,
    active: &DocumentSession,
) -> Vec<aurora_widgets::widgets::CommandEntry> {
    shelf
        .tab_order(active)
        .into_iter()
        .filter_map(|id| {
            let name = if id == active.id {
                Some(active.name.as_str())
            } else {
                shelf.get(id).map(|session| session.name.as_str())
            }?;
            Some(aurora_widgets::widgets::CommandEntry::new(
                format!("{COMMAND_DOCUMENT_SWITCH_PREFIX}{}", id.get()),
                format!("Document: {name}"),
            ))
        })
        .collect()
}

/// Appends [`document_palette_entries`] to a palette the key just opened,
/// the same way `workspace_presets::add_workspace_entries_if_opened` adds
/// the workspace entries.
pub(crate) fn add_document_entries_if_opened(
    workspace: &mut aurora_ui::Workspace,
    was_open: bool,
    palette: Option<WidgetId>,
    entries: Vec<aurora_widgets::widgets::CommandEntry>,
) -> bool {
    let Some(root) = palette else {
        return false;
    };
    if was_open {
        return false;
    }
    let Ok(state) = aurora_widgets::widgets::command_palette_state(&workspace.tree, root) else {
        return false;
    };
    if !state.is_filtering() {
        return false;
    }
    let mut commands = state.commands().to_vec();
    commands.extend(entries);
    if let Err(err) =
        aurora_widgets::widgets::set_command_palette_commands(&mut workspace.tree, root, commands)
    {
        tracing::warn!(?err, "failed to add the document commands to the palette");
        return false;
    }
    true
}

/// The autosave index's view of the open documents: every session in tab
/// order with its file name.
pub(crate) fn index_entries(
    shelf: &DocumentShelf,
    active: &DocumentSession,
    key: crate::autosave_files::RunKey,
) -> Vec<crate::autosave_files::IndexEntry> {
    shelf
        .tab_order(active)
        .into_iter()
        .map(|id| crate::autosave_files::IndexEntry {
            id: id.get(),
            file: crate::autosave_files::session_file_name(key, id.get()),
        })
        .collect()
}

/// Tells the autosave worker the open sessions in tab order and the active
/// one (0.172.0) — every time the set or the active document changes.
pub(crate) fn sync_autosave_sessions(
    worker: &mut crate::background_autosave::AutosaveWorker,
    shelf: &DocumentShelf,
    active: &DocumentSession,
    key: crate::autosave_files::RunKey,
) -> bool {
    worker.set_sessions(index_entries(shelf, active, key), Some(active.id.get()))
}
