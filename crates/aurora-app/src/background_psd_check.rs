//! "Check PSD Compatibility" off the UI thread (task 162, invariant
//! §7.3.4).
//!
//! Task 157's check composites the whole document and runs the real PSD
//! writer into a sink that keeps nothing. On the 277-layer Nyepi template
//! that is about 12 s, which used to run inside the command, with the
//! window frozen and nothing on screen to say why. Now the command only
//! takes a **snapshot** on the UI thread ([`CheckJob::snapshot`]) and the
//! work runs on [`PsdCheckWorker`]'s thread; `about_to_wait` shows the same
//! report dialog when the thread wakes the loop ([`background_check_step`]).
//!
//! **The snapshot.** The writer and the composite read the document
//! through a `&mut TileStore`, so the worker gets a store of its own, the
//! way the background compositor's mirror does ([`new_mirror`]): a clone
//! of the `LayerTree`, the saved selections and the carried report lines,
//! plus one [`aurora_tile::TileShare`] per source tile. A resident tile is
//! the live store's own `Arc` of texels (a reference count, no copy: the
//! live store pays one copy the next time it writes that tile), a pending
//! one its encoded bytes, and a tile that lives only on the scratch disk
//! is read from it, on the UI thread, exactly as the autosave snapshot
//! does. The mirror itself is *filled* on the worker thread. After the
//! snapshot nothing the worker reads can change, so a later edit never
//! tears the report: it is the report of the document as it was when the
//! check started, and the dialog says so when the document was edited
//! meanwhile ([`EDITED_NOTE`]).
//!
//! The rules this module owns, each pinned by a test:
//!
//! - **One check at a time.** A second request while one runs (or one
//!   cancelled but still winding down, whose composite holds the whole
//!   canvas in memory) is refused with [`StartFailure::Busy`], which the
//!   app shows as a calm notice.
//! - **A result belongs to its document.** [`background_check_step`]
//!   cancels a check whose document is no longer the active one (a tab
//!   switch, a close): the cancel flag stops the writer at its next write,
//!   the result is dropped and the indicator clears.
//! - **Never under a modal.** A finished check waits in the worker while a
//!   dialog is open, as a finished open does, so its report is not lost to
//!   the one modal slot.
//! - **A panic is a failed check**, in builds that unwind (see
//!   `background_open`'s notes on `panic = "abort"`).
//! - **Result before wake**, and **quitting never hangs**:
//!   [`PsdCheckWorker::shutdown`] cancels, joins what finishes within a
//!   bound and detaches the rest.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aurora_tile::{SurfaceId, TileId, TileShare, TileStore};

use crate::document_session::DocumentId;
use crate::psd_save::{self, PsdSaveFailure, PsdSaveSource};

/// How long quitting waits for a check still running before detaching it.
pub(crate) const SHUTDOWN_JOIN_BOUND: Duration = Duration::from_millis(100);

/// The window title while a check runs (the "Opening …" pattern).
pub(crate) const CHECKING_TEXT: &str = "Checking PSD compatibility\u{2026}";

/// The calm status-bar notice for a second request while one runs.
pub(crate) const BUSY_NOTICE: &str =
    "A Photoshop compatibility check is already running. Wait for it to finish.";

/// The calm status-bar notice for a check dropped because its document was
/// switched away or closed.
pub(crate) const CANCELLED_NOTICE: &str =
    "The Photoshop compatibility check was cancelled because its document is no longer active.";

/// Appended to the report when the document was edited after the check
/// started.
pub(crate) const EDITED_NOTE: &str = "\n\nThe document was edited while it was being checked. \
     This is the report for the document as it was when the check started.";

/// Which check a result belongs to. Strictly increasing per worker.
pub(crate) type Generation = u64;

type TileKey = (SurfaceId, TileId);

/// Why a check did not produce a report.
#[derive(Debug)]
pub(crate) enum CheckFailure {
    /// The preflight, the composite or the writer failed.
    Failed(PsdSaveFailure),
    /// The snapshot could not be taken (a scratch tile could not be read
    /// or the worker's store could not be made).
    Snapshot(String),
    /// The check panicked; the payload's message.
    Panicked(String),
    /// Dropped on request; never shown.
    Cancelled,
}

/// Why a check could not start.
#[derive(Debug)]
pub(crate) enum StartFailure {
    /// A check is running (or still winding down).
    Busy,
    /// The thread could not be started.
    Spawn(std::io::Error),
}

/// What [`CheckJob::snapshot`] needs from the live document.
pub(crate) struct CheckInputs<'a> {
    /// The sanitised display name the dialog shows.
    pub(crate) name: String,
    pub(crate) doc_id: DocumentId,
    pub(crate) revision: crate::UndoRevision,
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) canvas: (u32, u32),
    /// Sorted by name ([`aurora_doc::SelectionSet::saved_sorted`]).
    pub(crate) saved: Vec<(&'a str, &'a aurora_doc::Selection)>,
    pub(crate) carried: Vec<aurora_io::ReportEntry>,
}

/// Everything one check reads, owned: plain `Send` data, no handle to the
/// live store.
pub(crate) struct CheckJob {
    name: String,
    doc_id: DocumentId,
    revision: crate::UndoRevision,
    layers: aurora_doc::LayerTree,
    canvas: (u32, u32),
    saved: Vec<(String, aurora_doc::Selection)>,
    carried: Vec<aurora_io::ReportEntry>,
    mirror: TileStore,
    tiles: Vec<(TileKey, TileShare)>,
}

impl std::fmt::Debug for CheckJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckJob")
            .field("name", &self.name)
            .field("doc_id", &self.doc_id)
            .field("tiles", &self.tiles.len())
            .finish_non_exhaustive()
    }
}

impl CheckJob {
    /// Takes the snapshot (UI thread): clones the tree and selections,
    /// makes the worker's store and shares every source tile of `store`
    /// into the job. Costs a reference count per resident tile and a file
    /// read per tile that is only on the scratch disk; fills nothing.
    ///
    /// # Errors
    ///
    /// A sentence for the "can't check" dialog when the worker's store
    /// cannot be made or a scratch tile cannot be read.
    pub(crate) fn snapshot(inputs: CheckInputs<'_>, store: &mut TileStore) -> Result<Self, String> {
        let CheckInputs {
            name,
            doc_id,
            revision,
            layers,
            canvas,
            saved,
            carried,
        } = inputs;
        let Some(mirror) = crate::background_composite::new_mirror(store) else {
            return Err("the checking store could not be made".to_owned());
        };
        // Only surfaces this tree can composite, as the compositor's mirror.
        let wanted: HashSet<SurfaceId> = crate::tree_surfaces(layers);
        let composite = crate::composite_surface_id();
        let mut tiles = Vec::new();
        for (key, _version) in store.held_tile_versions() {
            if key.0 == composite || !wanted.contains(&key.0) {
                continue;
            }
            match store.share_tile(key.0, key.1) {
                Ok(Some(share)) => tiles.push((key, share)),
                Ok(None) => {}
                Err(err) => {
                    return Err(format!(
                        "a tile could not be read ({})",
                        crate::display_error_detail(&err)
                    ));
                }
            }
        }
        Ok(Self {
            name,
            doc_id,
            revision,
            layers: layers.clone(),
            canvas,
            saved: saved
                .into_iter()
                .map(|(name, selection)| (name.to_owned(), selection.clone()))
                .collect(),
            carried,
            mirror,
            tiles,
        })
    }

    /// How many tiles the snapshot holds.
    #[cfg(test)]
    pub(crate) fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    fn meta(&self) -> CheckMeta {
        CheckMeta {
            name: self.name.clone(),
            doc_id: self.doc_id,
            revision: self.revision,
        }
    }

    /// The check itself (worker thread): fills the store from the shares
    /// and runs [`psd_save::check_psd_compatibility_cancellable`].
    fn run(self, cancel: &AtomicBool) -> Result<aurora_io::PsdWriteReport, CheckFailure> {
        let Self {
            doc_id,
            revision,
            layers,
            canvas,
            saved,
            carried,
            mut mirror,
            tiles,
            ..
        } = self;
        for (index, (key, share)) in tiles.into_iter().enumerate() {
            if index % 64 == 0 && cancel.load(Ordering::SeqCst) {
                return Err(CheckFailure::Cancelled);
            }
            mirror.insert_share(key.0, key.1, share);
        }
        let source = PsdSaveSource {
            saved: saved
                .iter()
                .map(|(name, selection)| (name.as_str(), selection))
                .collect(),
            layers: &layers,
            canvas,
            store: &mut mirror,
            doc_id,
            revision,
            carried,
        };
        let result = psd_save::check_psd_compatibility_cancellable(source, Some(cancel));
        if cancel.load(Ordering::SeqCst) {
            return Err(CheckFailure::Cancelled);
        }
        result.map_err(CheckFailure::Failed)
    }
}

/// Which document and revision a check was started for.
#[derive(Clone, Debug)]
pub(crate) struct CheckMeta {
    pub(crate) name: String,
    pub(crate) doc_id: DocumentId,
    pub(crate) revision: crate::UndoRevision,
}

/// One check's result.
#[derive(Debug)]
pub(crate) struct FinishedCheck {
    pub(crate) generation: Generation,
    pub(crate) meta: CheckMeta,
    pub(crate) result: Result<aurora_io::PsdWriteReport, CheckFailure>,
    /// How long the check ran on its thread.
    pub(crate) elapsed: Duration,
}

/// The check that is running.
#[derive(Debug)]
struct PendingCheck {
    generation: Generation,
    meta: CheckMeta,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

/// What [`PsdCheckWorker::shutdown`] did with the threads it found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShutdownReport {
    pub(crate) joined: usize,
    pub(crate) detached: usize,
}

/// Runs compatibility checks off the calling thread; see the module docs.
#[derive(Debug)]
pub(crate) struct PsdCheckWorker {
    last_generation: Generation,
    pending: Option<PendingCheck>,
    /// Cancelled checks whose threads have not finished yet.
    abandoned: Vec<JoinHandle<()>>,
    sender: mpsc::Sender<FinishedCheck>,
    receiver: mpsc::Receiver<FinishedCheck>,
}

impl Default for PsdCheckWorker {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            last_generation: 0,
            pending: None,
            abandoned: Vec::new(),
            sender,
            receiver,
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "no message".to_owned()
    }
}

impl PsdCheckWorker {
    /// Starts checking `job` on a new thread. `wake` runs on that thread
    /// once the result is on the channel (in the app, an event-loop proxy
    /// user event).
    ///
    /// # Errors
    ///
    /// [`StartFailure::Busy`] while a check runs or winds down;
    /// [`StartFailure::Spawn`] if the thread cannot start.
    pub(crate) fn start<W>(&mut self, job: CheckJob, wake: W) -> Result<Generation, StartFailure>
    where
        W: FnOnce() + Send + 'static,
    {
        let meta = job.meta();
        self.start_with(meta, move |cancel| job.run(cancel), wake)
    }

    /// [`Self::start`] with the work given as a closure (the seam tests
    /// hold a check open through).
    pub(crate) fn start_with<R, W>(
        &mut self,
        meta: CheckMeta,
        run: R,
        wake: W,
    ) -> Result<Generation, StartFailure>
    where
        R: FnOnce(&AtomicBool) -> Result<aurora_io::PsdWriteReport, CheckFailure> + Send + 'static,
        W: FnOnce() + Send + 'static,
    {
        if self.is_busy() {
            return Err(StartFailure::Busy);
        }
        let generation = self.last_generation.saturating_add(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let sender = self.sender.clone();
        let thread_cancel = Arc::clone(&cancel);
        let thread_meta = meta.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("aurora-psd-check-{generation}"))
            .spawn(move || {
                let started = Instant::now();
                let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&thread_cancel)
                })) {
                    Ok(result) => result,
                    Err(payload) => {
                        let message = panic_message(payload.as_ref());
                        tracing::error!(%message, "the Photoshop compatibility check panicked");
                        Err(CheckFailure::Panicked(message))
                    }
                };
                // A closed channel means the app is quitting.
                let _ = sender.send(FinishedCheck {
                    generation,
                    meta: thread_meta,
                    result,
                    elapsed: started.elapsed(),
                });
                // After the send, never before.
                wake();
            });
        let handle = match spawned {
            Ok(handle) => handle,
            Err(err) => {
                tracing::error!(%err, "failed to start the Photoshop compatibility check thread");
                return Err(StartFailure::Spawn(err));
            }
        };
        self.last_generation = generation;
        self.pending = Some(PendingCheck {
            generation,
            meta,
            cancel,
            handle,
        });
        Ok(generation)
    }

    /// Whether a check is running or a cancelled one is still winding down
    /// (a second would hold a second whole-canvas composite).
    pub(crate) fn is_busy(&mut self) -> bool {
        self.reap();
        self.pending.is_some() || !self.abandoned.is_empty()
    }

    /// Whether a check is running and its result is wanted: what the
    /// "Checking …" indicator shows.
    #[must_use]
    pub(crate) fn is_checking(&self) -> bool {
        self.pending.is_some()
    }

    /// The running check's document and revision.
    #[must_use]
    pub(crate) fn pending_meta(&self) -> Option<&CheckMeta> {
        self.pending.as_ref().map(|pending| &pending.meta)
    }

    /// Stops wanting the running check: its writer is told to stop at its
    /// next write, its result will be dropped, the indicator clears.
    /// Returns whether there was one.
    pub(crate) fn cancel(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        pending.cancel.store(true, Ordering::SeqCst);
        self.abandoned.push(pending.handle);
        true
    }

    /// Drains the channel and returns the running check's result, clearing
    /// the pending state. A result whose check was cancelled is dropped.
    pub(crate) fn take_finished(&mut self) -> Option<FinishedCheck> {
        let mut current = None;
        while let Ok(finished) = self.receiver.try_recv() {
            let is_pending = self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.generation == finished.generation);
            if is_pending {
                if let Some(pending) = self.pending.take() {
                    // Finished except for its `wake` call: reaped later.
                    self.abandoned.push(pending.handle);
                }
                current = Some(finished);
            } else {
                tracing::info!(
                    generation = finished.generation,
                    "dropped a cancelled Photoshop compatibility check's result"
                );
            }
        }
        self.reap();
        current
    }

    fn reap(&mut self) {
        let (finished, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.abandoned)
            .into_iter()
            .partition(JoinHandle::is_finished);
        self.abandoned = running;
        for handle in finished {
            if handle.join().is_err() {
                tracing::warn!("a finished compatibility check thread panicked outside its check");
            }
        }
    }

    /// For quitting: cancels the running check, joins every thread that
    /// finishes within `bound` and detaches the rest. Never blocks for
    /// much longer than `bound`.
    pub(crate) fn shutdown(&mut self, bound: Duration) -> ShutdownReport {
        let mut handles = std::mem::take(&mut self.abandoned);
        if let Some(pending) = self.pending.take() {
            pending.cancel.store(true, Ordering::SeqCst);
            handles.push(pending.handle);
        }
        let deadline = Instant::now() + bound;
        let mut report = ShutdownReport::default();
        while !handles.is_empty() {
            let (finished, running): (Vec<_>, Vec<_>) =
                handles.into_iter().partition(JoinHandle::is_finished);
            handles = running;
            for handle in finished {
                let _ = handle.join();
                report.joined += 1;
            }
            if handles.is_empty() || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        report.detached = handles.len();
        drop(handles);
        report
    }
}

/// The window title while `checking` (the "Opening …" title wins when
/// both are showing).
pub(crate) fn window_title_with_check(
    opening: Option<&std::path::Path>,
    checking: bool,
    document: &str,
) -> String {
    if opening.is_none() && checking {
        return format!("{CHECKING_TEXT} \u{2014} Aurora");
    }
    crate::window_title(opening, document)
}

/// The accessibility node id of the "Checking …" status. Not a widget:
/// widget ids count up from zero, and `u64::MAX` is the "Opening …" node.
pub(crate) const CHECK_STATUS_NODE: accesskit::NodeId = accesskit::NodeId(u64::MAX - 1);

/// Adds the "Checking …" status to `update` while `checking`: a
/// `Role::Status` node with a polite live region, the last child of `root`
/// (the "Opening …" pattern). Unchanged when not checking, and the node
/// leaves the tree.
pub(crate) fn with_check_status(
    mut update: accesskit::TreeUpdate,
    root: aurora_widgets::WidgetId,
    checking: bool,
) -> accesskit::TreeUpdate {
    if !checking {
        return update;
    }
    let Some((_, root_node)) = update.nodes.iter_mut().find(|(id, _)| *id == root) else {
        return update;
    };
    root_node.push_child(CHECK_STATUS_NODE);
    let mut status = accesskit::Node::new(accesskit::Role::Status);
    status.set_label(CHECKING_TEXT);
    status.set_live(accesskit::Live::Polite);
    update.nodes.push((CHECK_STATUS_NODE, status));
    update
}

/// The text of the "can't check" dialog for a failure other than the
/// writer's own ([`psd_save::psd_check_failure_message`]).
fn failure_message(failure: &CheckFailure) -> String {
    match failure {
        CheckFailure::Failed(failure) => psd_save::psd_check_failure_message(failure),
        CheckFailure::Snapshot(why) => {
            format!("Aurora couldn't read the document to check it: {why}. No file was written.")
        }
        CheckFailure::Panicked(message) => format!(
            "The Photoshop compatibility check stopped unexpectedly. Details: {}. No file was \
             written.",
            crate::display_error_detail(&message)
        ),
        CheckFailure::Cancelled => {
            "The Photoshop compatibility check was cancelled. No file was written.".to_owned()
        }
    }
}

/// The title and message of the dialog a finished check shows. One
/// function for the worker path and the direct one, so the two cannot
/// differ. `edited`: the document changed after the check started
/// ([`EDITED_NOTE`] is appended to a report).
pub(crate) fn dialog_text(
    name: &str,
    result: &Result<aurora_io::PsdWriteReport, CheckFailure>,
    edited: bool,
) -> (&'static str, String) {
    match result {
        Ok(report) => {
            let mut message = crate::psd_report::check_message(name, report);
            if edited {
                message.push_str(EDITED_NOTE);
            }
            (psd_save::PSD_CHECK_TITLE, message)
        }
        Err(failure) => (psd_save::PSD_CHECK_FAILED_TITLE, failure_message(failure)),
    }
}

/// What one [`background_check_step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckStep {
    /// Nothing running, or it has not finished yet.
    Idle,
    /// The check's document is no longer the active one: it was cancelled.
    Cancelled,
    /// A check finished but a dialog is up: nothing was taken.
    Deferred,
    /// A finished check was taken and handed to the host.
    Finished,
}

/// The side of the app [`background_check_step`] drives: `App` in
/// production, a recording stand-in in tests.
pub(crate) trait CheckHost {
    fn check_worker(&mut self) -> &mut PsdCheckWorker;
    /// The active document's id and current revision.
    fn active_document(&self) -> (DocumentId, crate::UndoRevision);
    /// Whether a modal dialog is open.
    fn modal_open(&self) -> bool;
    /// Mirrors the worker's pending state into the title and the
    /// accessibility tree.
    fn show_check_state(&mut self);
    /// A check was dropped because its document went away.
    fn check_cancelled(&mut self);
    /// Shows `finished`; `edited`: the document changed since it started.
    fn show_check_result(&mut self, finished: FinishedCheck, edited: bool);
}

/// The loop's per-iteration check step (`about_to_wait`).
pub(crate) fn background_check_step(host: &mut impl CheckHost) -> CheckStep {
    let (active, revision) = host.active_document();
    let wrong_document = host
        .check_worker()
        .pending_meta()
        .is_some_and(|meta| meta.doc_id != active);
    if wrong_document {
        host.check_worker().cancel();
        host.show_check_state();
        host.check_cancelled();
        return CheckStep::Cancelled;
    }
    if host.modal_open() {
        return if host.check_worker().is_checking() {
            CheckStep::Deferred
        } else {
            CheckStep::Idle
        };
    }
    let Some(finished) = host.check_worker().take_finished() else {
        return CheckStep::Idle;
    };
    host.show_check_state();
    let edited = finished.meta.revision != revision;
    host.show_check_result(finished, edited);
    CheckStep::Finished
}

#[cfg(test)]
#[path = "background_psd_check_tests.rs"]
mod tests;
