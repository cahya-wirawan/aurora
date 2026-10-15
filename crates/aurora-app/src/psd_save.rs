//! Save As Photoshop (0.177.0, PSD write round R2): the free functions
//! `App::save_psd_file` is built from, so the whole flow is testable
//! without a window.
//!
//! A *lossless* PSD save (an empty report, nothing carried from the open)
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
//! The merged image is the whole-canvas `composite_document` image, held
//! in memory (a banded composite is round R5), and the write runs on the
//! UI thread (a background write is later work).

use std::path::Path;

use aurora_widgets::widgets::DialogAction;

use crate::document_session::DocumentId;

pub(crate) const PSD_SAVE_WITH_CHANGES_TITLE: &str = "Save With Changes?";
pub(crate) const PSD_SAVE_ANYWAY: &str = "psd.save.anyway";
pub(crate) const PSD_SAVE_CANCEL: &str = "psd.save.cancel";

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

/// The dialog's message: the report's items, one per line, in the
/// report's own fixed order, then what Save Anyway does and does not do.
pub(crate) fn psd_save_message(file_name: &str, report: &aurora_io::PsdWriteReport) -> String {
    let mut message = format!("Saving \u{201c}{file_name}\u{201d} as a Photoshop file changes it:");
    for item in &report.items {
        message.push_str("\n\u{2022} ");
        message.push_str(item);
    }
    message.push_str(
        "\nSave Anyway writes the file with these changes; your document is not changed. \
         Nothing has been written yet.",
    );
    message
}

/// The heading of the report's "carried from open" block (H-1).
pub(crate) const CARRIED_HEADING: &str =
    "This file was opened with changes; saving writes it as Aurora shows it:";

/// The report lines carried from the document's own open (0.177.0 review
/// H-1): its "Opened With Changes" lines under [`CARRIED_HEADING`], and a
/// line for tiles a `.aur` open found missing. Empty for a faithful open.
pub(crate) fn carried_losses(
    import_losses: Option<&[String]>,
    skipped_tiles: &aurora_io::SkippedTiles,
) -> Vec<String> {
    let mut lines = Vec::new();
    let opened: &[String] = import_losses.unwrap_or_default();
    if !opened.is_empty() {
        lines.push(CARRIED_HEADING.to_owned());
        lines.extend(opened.iter().map(|line| format!("\u{2013} {line}")));
    }
    if !skipped_tiles.is_empty() {
        lines.push(
            "Some of this document's tiles were missing when it was opened; they are saved as \
             transparent."
                .to_owned(),
        );
    }
    lines
}

/// The one report line for a document that has **saved named selections**
/// (0.218.1): they are the user's data and a Photoshop file cannot carry
/// them yet (PSD alpha channels wait for the Channels panel), so the save
/// would leave them out. `None` when there are none. The *active*
/// selection alone adds nothing: it is transient state, and listing it
/// would make every PSD save of a document with a marquee "lossy" and ask
/// first. (It is still not written to the PSD: a documented gap.)
pub(crate) fn selection_loss_line(selection: &aurora_doc::SelectionSet) -> Option<String> {
    (selection.saved_count() > 0)
        .then(|| "Saved selections are not saved in a Photoshop file.".to_owned())
}

/// The report line for paths (0.233.0): a PSD does not carry them.
pub(crate) fn paths_loss_line(selection: &aurora_doc::SelectionSet) -> Option<String> {
    (!selection.paths().is_empty()).then(|| "Paths are not saved in a Photoshop file.".to_owned())
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
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) canvas: (u32, u32),
    pub(crate) store: &'a mut aurora_tile::TileStore,
    pub(crate) doc_id: DocumentId,
    pub(crate) revision: crate::UndoRevision,
    /// [`carried_losses`] for this document, appended to the report.
    pub(crate) carried: Vec<String>,
}

/// Runs the preflight, composites the document, writes and verifies the
/// PSD into a temp file beside `path` (a symbolic link is followed), and
/// either swaps it in (an empty report) or hands it back to wait for
/// Save Anyway.
pub(crate) fn begin_psd_save(
    source: PsdSaveSource<'_>,
    path: &Path,
) -> Result<PsdSaveStep, PsdSaveFailure> {
    if is_psb_save_path(path) {
        return Err(PsdSaveFailure::Refused(PSB_NOT_YET.to_owned()));
    }
    let opts = aurora_io::PsdSaveOptions::default();
    if let Err(refusal) = aurora_io::psd_save_preflight(source.layers, source.canvas, opts) {
        return Err(PsdSaveFailure::Refused(refusal.to_string()));
    }
    let (width, height) = source.canvas;
    let image = crate::composite_document(source.layers, source.store, width, height)
        .map_err(PsdSaveFailure::Composite)?;
    let layers = source.layers;
    let store = source.store;
    let written = aurora_io::write_verified_temp(
        path,
        |file| {
            let mut composite = &image;
            aurora_io::write_psd(
                &mut *file,
                layers,
                source.canvas,
                &mut composite,
                store,
                opts,
            )
        },
        |temp, outcome| aurora_io::verify_psd_file(temp, &outcome.expected),
    );
    let (mut outcome, temp) = match written {
        Ok(written) => written,
        Err(aurora_io::IoError::PsdSaveRefused(refusal)) => {
            return Err(PsdSaveFailure::Refused(refusal.to_string()));
        }
        Err(err) => return Err(PsdSaveFailure::Write(err)),
    };
    // The fixed-order "carried from open" block goes last (category C).
    outcome.report.items.extend(source.carried);
    if outcome.report.is_empty() {
        let advisory = temp.persist().map_err(PsdSaveFailure::Write)?;
        return Ok(if advisory.is_some() {
            PsdSaveStep::SavedNotConfirmed
        } else {
            PsdSaveStep::Saved
        });
    }
    Ok(PsdSaveStep::NeedsConfirmation(PendingPsdSave {
        temp,
        doc_id: source.doc_id,
        revision: source.revision,
        report: outcome.report,
    }))
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
