//! Save As Photoshop (0.177.0, PSD write round R2): the free functions
//! `App::save_psd_file` is built from, so the whole flow is testable
//! without a window.
//!
//! A *lossless* PSD save (a report with no line that asks: only
//! [`aurora_io::ReportCategory::Kept`] ones, see [`crate::psd_report`])
//! is a real save: the document becomes clean and takes the `.psd` path
//! and name, and a close or quit Save to it goes ahead (design-owner
//! decision, 2026-10-10, bringing R4's "clean if lossless" forward). A
//! save that changes anything is a copy: the document stays dirty and
//! keeps its path even after Save Anyway (decision 4), and a close or quit
//! does not go ahead. The target is replaced only by a verified temp file,
//! and — when the save changes anything ([`aurora_io::PsdWriteReport`]) —
//! only after the user chose Save Anyway in the "Save With Changes?"
//! dialog, for the same document at the same undo revision it was written
//! from.
//!
//! What the document's own open changed or left out
//! (`DocumentSession::import_losses`, and tiles missing from a `.aur`) is
//! carried into the report ([`carried_losses`], 0.177.0 review H-1), so
//! re-saving an opened PSD over itself never drops that content silently.
//!
//! Saved selections (task 156) are written as PSD alpha channels
//! (`aurora_io::write_psd_with_selections`). The write's report then always
//! carries a "written as alpha channels" line (the channels are not checked
//! in Photoshop), so a save of a document with saved selections is never
//! lossless and always asks "Save With Changes?" first, as saves with
//! effects or text layers do.
//!
//! The merged image is the whole-canvas `composite_document` image, held
//! in memory (a banded composite is round R5), and the write runs on the
//! UI thread (a background write is later work).

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use aurora_widgets::widgets::DialogAction;

use crate::document_session::DocumentId;

pub(crate) const PSD_SAVE_WITH_CHANGES_TITLE: &str = "Save With Changes?";
pub(crate) const PSD_SAVE_ANYWAY: &str = "psd.save.anyway";
pub(crate) const PSD_SAVE_CANCEL: &str = "psd.save.cancel";

/// The command id, palette/menu label and dialogs of "Check PSD
/// Compatibility" (task 157).
pub(crate) const COMMAND_CHECK: &str = "file.check_psd";
#[cfg(any(test, target_os = "macos"))]
pub(crate) const CHECK_TITLE: &str = "Check PSD Compatibility\u{2026}";
pub(crate) const PSD_CHECK_TITLE: &str = "Photoshop Compatibility";
pub(crate) const PSD_CHECK_FAILED_TITLE: &str = "Can't Check Photoshop Compatibility";
pub(crate) const PSD_CHECK_OK: &str = "psd.check.ok";
/// The calm status-bar notice when the check's result cannot be shown because
/// another dialog is open (the result is also logged).
pub(crate) const PSD_CHECK_BUSY_NOTICE: &str = "The Photoshop compatibility check couldn't be shown because a dialog is open. Close it and \
     check again.";

/// The check dialog's single "OK" (default and cancel).
pub(crate) fn psd_check_dialog_actions() -> Vec<DialogAction> {
    vec![
        DialogAction::new(PSD_CHECK_OK, "OK")
            .as_default()
            .as_cancel(),
    ]
}

/// Why a `.psb` name is refused until round R5.
pub(crate) const PSB_NOT_YET: &str = "PSB isn't supported yet. Choose a name ending in .psd, \
     or save as an Aurora document (.aur)";

/// Whether `path` ends in `.psd`, case-insensitively.
pub(crate) fn is_psd_save_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("psd"))
}

/// Whether `path` ends in `.psb`, case-insensitively.
pub(crate) fn is_psb_save_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("psb"))
}

/// The save picker's file-type filters, in the order shown: Aurora's own
/// format first (the only one that is a real save), then Photoshop, then
/// the flat images.
pub(crate) fn save_file_filters() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("Aurora document", &["aur"]),
        ("Photoshop", &["psd"]),
        ("PNG image", &["png"]),
        ("JPEG image", &["jpg", "jpeg"]),
        ("TIFF image", &["tif", "tiff"]),
    ]
}

/// Save Anyway, then Cancel — **Cancel is the default** (`Enter`) as well
/// as the cancel action (`Escape`): Save Anyway replaces a file on disk
/// with one that differs from the document, so it must be a deliberate
/// choice, never the reflex of pressing `Enter` on an alert.
pub(crate) fn psd_save_dialog_actions() -> Vec<DialogAction> {
    vec![
        DialogAction::new(PSD_SAVE_ANYWAY, "Save Anyway"),
        DialogAction::new(PSD_SAVE_CANCEL, "Cancel")
            .as_default()
            .as_cancel(),
    ]
}

/// Polled once per loop iteration (review L-2): `true` — and the flag
/// cleared — the first time a deferred Save With Changes can be shown (a
/// save is pending and no dialog is open). Without a pending save the
/// flag is dropped.
pub(crate) fn take_deferred_dialog(deferred: &mut bool, pending: bool, dialog_open: bool) -> bool {
    if !*deferred {
        return false;
    }
    if !pending {
        *deferred = false;
        return false;
    }
    if dialog_open {
        return false;
    }
    *deferred = false;
    true
}

/// The Save With Changes dialog's two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PsdSaveChoice {
    SaveAnyway,
    Cancel,
}

pub(crate) fn psd_save_choice(action: &str) -> Option<PsdSaveChoice> {
    match action {
        PSD_SAVE_ANYWAY => Some(PsdSaveChoice::SaveAnyway),
        PSD_SAVE_CANCEL => Some(PsdSaveChoice::Cancel),
        _ => None,
    }
}

/// The dialog's message: the report grouped by category with counts
/// ([`crate::psd_report`]), then what Save Anyway does and does not do.
pub(crate) fn psd_save_message(file_name: &str, report: &aurora_io::PsdWriteReport) -> String {
    crate::psd_report::save_message(file_name, report)
}

/// The report entries carried from the document's own open (0.177.0 review
/// H-1; task 157: one [`aurora_io::ReportCategory::FromOpen`] entry per
/// "Opened With Changes" line), and one for tiles a `.aur` open found
/// missing. Empty for a faithful open.
pub(crate) fn carried_losses(
    import_losses: Option<&[String]>,
    skipped_tiles: &aurora_io::SkippedTiles,
) -> Vec<aurora_io::ReportEntry> {
    use aurora_io::{ReportCategory, ReportEntry};
    let mut entries: Vec<ReportEntry> = import_losses
        .unwrap_or_default()
        .iter()
        .map(|line| ReportEntry::new(ReportCategory::FromOpen, line.clone()))
        .collect();
    if !skipped_tiles.is_empty() {
        entries.push(ReportEntry::new(
            ReportCategory::Changed,
            "Some of this document's tiles were missing when it was opened; they are saved as \
             transparent.",
        ));
    }
    entries
}

/// Saved selections are written as PSD alpha channels (task 156) by
/// `aurora_io::write_psd_with_selections`, which itemises what it writes and
/// what it cannot in its own report; the *active* selection alone adds
/// nothing: it is transient state, and listing it would make every PSD save
/// of a document with a marquee "lossy" and ask first. (It is still not
/// written to the PSD: a documented gap.)
/// The report line for paths (0.233.0): a PSD does not carry them.
pub(crate) fn paths_loss_line(selection: &aurora_doc::SelectionSet) -> Option<String> {
    (!selection.paths().is_empty()).then(|| "Paths are not saved in a Photoshop file.".to_owned())
}

/// [`paths_loss_line`] as a Not saved entry.
pub(crate) fn paths_loss_entry(
    selection: &aurora_doc::SelectionSet,
) -> Option<aurora_io::ReportEntry> {
    paths_loss_line(selection)
        .map(|line| aurora_io::ReportEntry::new(aurora_io::ReportCategory::NotSaved, line))
}

/// [`colour_loss_lines`] as Needs attention entries: the profile is not kept.
pub(crate) fn colour_loss_entries(
    colour: &aurora_io::DocumentColour,
) -> Vec<aurora_io::ReportEntry> {
    colour_loss_lines(colour)
        .into_iter()
        .map(|line| aurora_io::ReportEntry::new(aurora_io::ReportCategory::NeedsAttention, line))
        .collect()
}

/// The report lines for a document with colour metadata: a PSD is written in
/// sRGB (the working space, with the sRGB profile as resource 1039), so a
/// document converted from another profile on open does not get that
/// profile back, and an output profile is not applied. Empty for a document
/// with neither. Each line makes the save "lossy", so the user is asked.
pub(crate) fn colour_loss_lines(colour: &aurora_io::DocumentColour) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(source) = &colour.source {
        lines.push(format!(
            "Colours were converted from \u{201c}{}\u{201d} to sRGB on open; the file is saved as \
             sRGB, the original profile is not kept, and out-of-gamut colours are clipped.",
            source.description
        ));
    }
    if let Some(output) = &colour.output {
        lines.push(format!(
            "The output profile \u{201c}{}\u{201d} is not applied to a Photoshop file; it is \
             saved as sRGB.",
            output.description
        ));
    }
    lines
}

/// Records a PSD save on the document: a lossless one is a real save
/// (clean, `.psd` path and name, through `document_close::after_save`);
/// one with changes — Save Anyway included — leaves it untouched.
/// Returns whether the document is now saved.
pub(crate) fn record_psd_save(
    doc: &mut crate::DocumentSession,
    path: &Path,
    lossless: bool,
) -> bool {
    crate::document_close::after_save(doc, path, lossless)
}

/// A written and verified PSD waiting for the user's Save Anyway: the
/// target is untouched until [`finish_psd_save`]; dropping it deletes
/// the temp file.
#[derive(Debug)]
pub(crate) struct PendingPsdSave {
    pub(crate) temp: aurora_io::VerifiedTemp,
    pub(crate) doc_id: DocumentId,
    pub(crate) revision: crate::UndoRevision,
    pub(crate) report: aurora_io::PsdWriteReport,
}

/// Where [`begin_psd_save`] got to.
#[derive(Debug)]
pub(crate) enum PsdSaveStep {
    /// Nothing changed in the save: written, verified and in place.
    Saved,
    /// As [`Self::Saved`], but the directory sync failed after the rename
    /// (0.247.0): the file is in place, its durability unconfirmed.
    SavedNotConfirmed,
    /// Written and verified beside the target; the user must agree.
    NeedsConfirmation(PendingPsdSave),
}

/// Why a PSD save did not happen. Nothing at the target changed.
#[derive(Debug)]
pub(crate) enum PsdSaveFailure {
    /// `.psb`, or the preflight's refusal: shown with its own reason.
    Refused(String),
    /// The layers could not be composited into the merged image.
    Composite(aurora_io::IoError),
    /// Writing, verifying or replacing failed.
    Write(aurora_io::IoError),
    /// Save Anyway for a document that changed (or closed) since the
    /// file was written: the written file no longer matches it.
    DocumentChanged,
}

/// The document a PSD save writes.
#[derive(Debug)]
pub(crate) struct PsdSaveSource<'a> {
    /// The saved selections, written as alpha channels (task 156), sorted
    /// by name ([`aurora_doc::SelectionSet::saved_sorted`]).
    pub(crate) saved: Vec<(&'a str, &'a aurora_doc::Selection)>,
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) canvas: (u32, u32),
    pub(crate) store: &'a mut aurora_tile::TileStore,
    pub(crate) doc_id: DocumentId,
    pub(crate) revision: crate::UndoRevision,
    /// [`carried_losses`] for this document, appended to the report.
    pub(crate) carried: Vec<aurora_io::ReportEntry>,
}

/// Runs the preflight, composites the document, writes and verifies the
/// PSD into a temp file beside `path` (a symbolic link is followed), and
/// either swaps it in (an empty report) or hands it back to wait for
/// Save Anyway.
pub(crate) fn begin_psd_save(
    source: PsdSaveSource<'_>,
    path: &Path,
) -> Result<PsdSaveStep, PsdSaveFailure> {
    begin_psd_save_timed(source, path, &mut PsdSaveTimings::default())
}

/// Wall times of one [`begin_psd_save_timed`] call, per phase (task 154,
/// the headless Save As `.psd` measurement). A phase the call never
/// reached stays zero.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PsdSaveTimings {
    /// `psd_save_preflight`.
    pub(crate) preflight: Duration,
    /// The merged image's composite (`composite_document`).
    pub(crate) composite: Duration,
    /// `write_psd` into the temp file: tile reads, `PackBits` and the writes.
    pub(crate) write: Duration,
    /// The temp file's `sync_all`.
    pub(crate) sync: Duration,
    /// `verify_psd_file`: the reopen by Aurora's own parsers.
    pub(crate) verify: Duration,
    /// The rename and the directory sync, when the report is empty (else
    /// the caller persists, after Save Anyway).
    pub(crate) persist: Duration,
}

/// [`begin_psd_save`], recording each phase's wall time into `timings`.
/// The same steps in the same order; only clocks are read in between.
pub(crate) fn begin_psd_save_timed(
    source: PsdSaveSource<'_>,
    path: &Path,
    timings: &mut PsdSaveTimings,
) -> Result<PsdSaveStep, PsdSaveFailure> {
    if is_psb_save_path(path) {
        return Err(PsdSaveFailure::Refused(PSB_NOT_YET.to_owned()));
    }
    let opts = aurora_io::PsdSaveOptions::default();
    let started = Instant::now();
    let preflight = aurora_io::psd_save_preflight(source.layers, source.canvas, opts);
    timings.preflight = started.elapsed();
    if let Err(refusal) = preflight {
        return Err(PsdSaveFailure::Refused(refusal.to_string()));
    }
    let (width, height) = source.canvas;
    let started = Instant::now();
    let image = crate::composite_document(source.layers, source.store, width, height)
        .map_err(PsdSaveFailure::Composite)?;
    timings.composite = started.elapsed();
    let layers = source.layers;
    let store = source.store;
    let write_time = Cell::new(Duration::ZERO);
    let sync_time = Cell::new(Duration::ZERO);
    let verify_time = Cell::new(Duration::ZERO);
    let written = aurora_io::write_verified_temp_with(
        path,
        |file| {
            let started = Instant::now();
            let mut composite = &image;
            let outcome = aurora_io::write_psd_with_selections(
                &mut *file,
                layers,
                source.canvas,
                &mut composite,
                store,
                opts,
                &source.saved,
            );
            write_time.set(started.elapsed());
            outcome
        },
        |file| {
            let started = Instant::now();
            let synced = file.sync_all();
            sync_time.set(started.elapsed());
            synced
        },
        |temp, outcome| {
            let started = Instant::now();
            let verified = aurora_io::verify_psd_file(temp, &outcome.expected);
            verify_time.set(started.elapsed());
            verified
        },
    );
    timings.write = write_time.get();
    timings.sync = sync_time.get();
    timings.verify = verify_time.get();
    let (mut outcome, temp) = match written {
        Ok(written) => written,
        Err(aurora_io::IoError::PsdSaveRefused(refusal)) => {
            return Err(PsdSaveFailure::Refused(refusal.to_string()));
        }
        Err(err) => return Err(PsdSaveFailure::Write(err)),
    };
    // The lines carried from the open, and the colour and path notes, are
    // entries of their own categories (task 157).
    outcome.report.extend_entries(source.carried);
    if outcome.report.is_empty() {
        let started = Instant::now();
        let advisory = temp.persist();
        timings.persist = started.elapsed();
        let advisory = advisory.map_err(PsdSaveFailure::Write)?;
        return Ok(if advisory.is_some() {
            PsdSaveStep::SavedNotConfirmed
        } else {
            PsdSaveStep::Saved
        });
    }
    tracing::info!(
        report = %crate::psd_report::SaveReport::new(&outcome.report).full_text(),
        "the Photoshop save asks first"
    );
    Ok(PsdSaveStep::NeedsConfirmation(PendingPsdSave {
        temp,
        doc_id: source.doc_id,
        revision: source.revision,
        report: outcome.report,
    }))
}

/// A writer that keeps nothing: it counts its position and length so the
/// PSD writer, which seeks back to patch lengths, runs unchanged. With a
/// `cancel` flag (task 162) a write after the flag is set fails, which
/// stops a check nobody wants any more at the writer's next write.
#[derive(Debug, Default)]
struct CountingSink<'a> {
    pos: u64,
    len: u64,
    cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

impl std::io::Write for CountingSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self
            .cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            // Not `Interrupted`: `write_all` retries that forever.
            return Err(std::io::Error::other("the check was cancelled"));
        }
        self.pos = self.pos.saturating_add(buf.len() as u64);
        self.len = self.len.max(self.pos);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Seek for CountingSink<'_> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = match to {
            std::io::SeekFrom::Start(n) => Some(n),
            std::io::SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            std::io::SeekFrom::End(d) => self.len.checked_add_signed(d),
        };
        let Some(target) = target else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start",
            ));
        };
        self.pos = target;
        Ok(target)
    }
}

/// "Check PSD Compatibility" (task 157): the report a Save As `.psd` would
/// show, without writing a file. It runs the preflight, composites the
/// document and runs the real writer into a sink that keeps nothing, so
/// the measured lines (clipped colours, kept and dropped smart objects and
/// text, alpha channels) are the ones a save would give. Nothing is
/// created on disk and the document is untouched. The cost is a save's
/// without its disk writes and verification (the composite and the tile
/// reads), on the calling thread. The app runs the same check off the UI
/// thread over a snapshot (`background_psd_check`, task 162); this form
/// is what that must equal, and what the automation bridge calls.
pub(crate) fn check_psd_compatibility(
    source: PsdSaveSource<'_>,
) -> Result<aurora_io::PsdWriteReport, PsdSaveFailure> {
    check_psd_compatibility_cancellable(source, None)
}

/// [`check_psd_compatibility`] that stops, with an error, at the writer's
/// next write once `cancel` is set. The result of a cancelled check is
/// meaningless; the caller drops it.
pub(crate) fn check_psd_compatibility_cancellable(
    source: PsdSaveSource<'_>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<aurora_io::PsdWriteReport, PsdSaveFailure> {
    let opts = aurora_io::PsdSaveOptions::default();
    aurora_io::psd_save_preflight(source.layers, source.canvas, opts)
        .map_err(|refusal| PsdSaveFailure::Refused(refusal.to_string()))?;
    let (width, height) = source.canvas;
    let image = crate::composite_document(source.layers, source.store, width, height)
        .map_err(PsdSaveFailure::Composite)?;
    let mut composite = &image;
    let mut sink = CountingSink {
        cancel,
        ..CountingSink::default()
    };
    let outcome = aurora_io::write_psd_with_selections(
        &mut sink,
        source.layers,
        source.canvas,
        &mut composite,
        source.store,
        opts,
        &source.saved,
    )
    .map_err(|err| match err {
        aurora_io::IoError::PsdSaveRefused(refusal) => PsdSaveFailure::Refused(refusal.to_string()),
        other => PsdSaveFailure::Write(other),
    })?;
    let mut report = outcome.report;
    report.extend_entries(source.carried);
    Ok(report)
}

/// The text of the "can't check" dialog for a failed [`check_psd_compatibility`].
pub(crate) fn psd_check_failure_message(failure: &PsdSaveFailure) -> String {
    match failure {
        PsdSaveFailure::Refused(reason) => format!(
            "Aurora can't save this document as a Photoshop file: {reason}. No file was written."
        ),
        PsdSaveFailure::Composite(err) => format!(
            "Aurora couldn't combine the document's layers to check it. Details: {}. No file \
             was written.",
            crate::display_error_detail(err)
        ),
        PsdSaveFailure::Write(err) => format!(
            "Aurora couldn't finish checking the document. Details: {}. No file was written.",
            crate::display_error_detail(err)
        ),
        PsdSaveFailure::DocumentChanged => {
            "The document changed while it was being checked. No file was written.".to_owned()
        }
    }
}

/// Save Anyway: swaps the pending file in — only if `doc_id` and
/// `revision` (the document as it is now) are the ones it was written
/// from. Otherwise the temp file is dropped (deleted) and nothing changes.
pub(crate) fn finish_psd_save(
    pending: PendingPsdSave,
    doc_id: DocumentId,
    revision: crate::UndoRevision,
) -> Result<Option<String>, PsdSaveFailure> {
    if pending.doc_id != doc_id || pending.revision != revision {
        return Err(PsdSaveFailure::DocumentChanged);
    }
    pending.temp.persist().map_err(PsdSaveFailure::Write)
}

/// "Couldn't Save File"'s message for a failed PSD save.
pub(crate) fn psd_save_failure_message(file_name: &str, failure: &PsdSaveFailure) -> String {
    let body = match failure {
        PsdSaveFailure::Refused(reason) => {
            format!("Aurora can't save \u{201c}{file_name}\u{201d} as a Photoshop file: {reason}.")
        }
        PsdSaveFailure::Composite(err) => format!(
            "Aurora couldn't save \u{201c}{file_name}\u{201d}: the document's layers could not be \
             combined into the Photoshop file's merged image. Details: {}.",
            crate::display_error_detail(err)
        ),
        PsdSaveFailure::Write(err) => format!(
            "Aurora couldn't save \u{201c}{file_name}\u{201d}: writing the Photoshop file, or \
             reading it back to check it, failed. Details: {}.",
            crate::display_error_detail(err)
        ),
        PsdSaveFailure::DocumentChanged => format!(
            "Aurora didn't save \u{201c}{file_name}\u{201d}: the document changed after the \
             Photoshop file was prepared. Save it again to include the changes."
        ),
    };
    format!("{body} {}", crate::SAVE_FAILED_UNCHANGED)
}
