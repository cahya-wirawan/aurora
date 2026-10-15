//! Tests for Save As Photoshop (0.177.0, `psd_save`).

use std::path::{Path, PathBuf};

use aurora_color::IccProfile;
use aurora_core::Rect;
use aurora_doc::{BlendMode, LayerTree};
use aurora_widgets::FocusManager;
use half::f16;

use crate::document_session::DocumentId;
use crate::psd_save::{
    PSD_SAVE_ANYWAY, PSD_SAVE_CANCEL, PsdSaveChoice, PsdSaveFailure, PsdSaveSource, PsdSaveStep,
    begin_psd_save, finish_psd_save, psd_save_dialog_actions, psd_save_failure_message,
    psd_save_message, record_psd_save,
};
use crate::{DialogEffect, DialogPurpose, DialogResult, FileDialogAccess, UndoRevision};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// A two-layer document in its own store and directory.
struct Doc {
    tree: LayerTree,
    store: aurora_tile::TileStore,
    dir: tempfile::TempDir,
    id: DocumentId,
    revision: UndoRevision,
}

impl Doc {
    fn new() -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(16) else {
            unreachable!("non-zero")
        };
        let scratch = dir.path().join("scratch");
        ok(std::fs::create_dir(&scratch));
        let mut store = ok(aurora_tile::TileStore::new(scratch, budget));
        let mut tree = LayerTree::new();
        for (name, value) in [("Background", 0.25f32), ("Top", 0.75)] {
            let id = ok(tree.add_pixel_layer(name, rect(0, 0, 8, 8), None));
            let samples: Vec<f16> = (0..64)
                .flat_map(|i| [value, 0.5, (i % 8) as f32 / 8.0, 1.0].map(f16::from_f32))
                .collect();
            let image = ok(aurora_io::Image::new(8, 8, IccProfile::srgb(), samples));
            let Some(surface) = tree.surface_id(id) else {
                unreachable!("pixel layer")
            };
            ok(aurora_io::write_into_store_at(
                &image, &mut store, surface, 0, 0,
            ));
        }
        Self {
            tree,
            store,
            dir,
            id: DocumentId::next(),
            revision: UndoRevision(7),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn save(&mut self, path: &Path) -> Result<PsdSaveStep, PsdSaveFailure> {
        self.save_carrying(path, Vec::new())
    }

    fn save_carrying(
        &mut self,
        path: &Path,
        carried: Vec<String>,
    ) -> Result<PsdSaveStep, PsdSaveFailure> {
        begin_psd_save(
            PsdSaveSource {
                layers: &self.tree,
                canvas: (8, 8),
                store: &mut self.store,
                doc_id: self.id,
                revision: self.revision,
                carried,
            },
            path,
        )
    }

    /// Makes the save lossy: a Dissolve layer.
    fn make_lossy(&mut self) {
        let top = self.tree.roots().first().copied();
        let Some(top) = top else { unreachable!() };
        ok(self.tree.set_blend_mode(top, BlendMode::Dissolve));
    }

    /// Everything in the directory but the scratch store.
    fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = ok(std::fs::read_dir(self.dir.path()))
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "scratch")
            .collect();
        names.sort();
        names
    }
}

fn names_bottom_up(layers: &LayerTree) -> Vec<String> {
    layers
        .roots()
        .iter()
        .rev()
        .filter_map(|&id| layers.name(id).map(str::to_owned))
        .collect()
}

#[test]
fn psd_save_with_empty_report_persists_without_dialog() {
    let mut doc = Doc::new();
    let path = doc.path("copy.psd");
    let step = ok(doc.save(&path));
    assert!(matches!(step, PsdSaveStep::Saved));
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert_eq!(names_bottom_up(&back.layers), ["Background", "Top"]);
    assert_eq!(doc.listing(), ["copy.psd"], "no temp file left");
}

#[test]
fn psd_save_with_items_opens_dialog_and_target_untouched_until_save_anyway() {
    let mut doc = Doc::new();
    doc.make_lossy();
    let path = doc.path("copy.psd");
    ok(std::fs::write(&path, b"the user's file"));
    let step = ok(doc.save(&path));
    let PsdSaveStep::NeedsConfirmation(pending) = step else {
        unreachable!("needs confirmation")
    };
    assert_eq!(
        pending.report.items,
        ["Dissolve layers will look different in apps other than Photoshop."]
    );
    assert_eq!(ok(std::fs::read(&path)), b"the user's file", "untouched");
    assert_eq!(
        doc.listing().len(),
        2,
        "the verified temp file waits beside it"
    );
    ok(finish_psd_save(pending, doc.id, doc.revision));
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert_eq!(names_bottom_up(&back.layers), ["Background", "Top"]);
    assert_eq!(doc.listing(), ["copy.psd"]);
}

#[test]
fn psd_save_cancel_removes_temp() {
    let mut doc = Doc::new();
    doc.make_lossy();
    let path = doc.path("copy.psd");
    ok(std::fs::write(&path, b"the user's file"));
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&path)) else {
        unreachable!("needs confirmation")
    };
    let temp = pending.temp.path().to_path_buf();
    assert!(temp.exists());
    // Cancel is dropping the pending save (`App::answer_psd_save`).
    drop(pending);
    assert!(!temp.exists());
    assert_eq!(ok(std::fs::read(&path)), b"the user's file");
    assert_eq!(doc.listing(), ["copy.psd"]);
}

#[test]
fn psd_save_anyway_after_revision_change_refuses() {
    for (other_doc, other_revision) in [(false, true), (true, false)] {
        let mut doc = Doc::new();
        doc.make_lossy();
        let path = doc.path("copy.psd");
        let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&path)) else {
            unreachable!("needs confirmation")
        };
        let id = if other_doc {
            DocumentId::next()
        } else {
            doc.id
        };
        let revision = if other_revision {
            UndoRevision(8)
        } else {
            doc.revision
        };
        assert!(matches!(
            finish_psd_save(pending, id, revision),
            Err(PsdSaveFailure::DocumentChanged)
        ));
        assert!(!path.exists(), "nothing written");
        assert!(doc.listing().is_empty(), "temp file removed");
    }
}

#[test]
fn run_dialog_action_maps_psd_save_actions() {
    let actions = psd_save_dialog_actions();
    let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, [PSD_SAVE_ANYWAY, PSD_SAVE_CANCEL]);
    // Cancel is both the default (`Enter`) and the cancel (`Escape`).
    let cancel = actions.get(1);
    assert!(cancel.is_some_and(|a| a.is_default && a.is_cancel));
    assert!(
        actions
            .first()
            .is_some_and(|a| !a.is_default && !a.is_cancel)
    );
    for (action, choice) in [
        (PSD_SAVE_ANYWAY, PsdSaveChoice::SaveAnyway),
        (PSD_SAVE_CANCEL, PsdSaveChoice::Cancel),
    ] {
        let result = DialogResult {
            purpose: DialogPurpose::PsdSaveWithChanges,
            action: action.to_owned(),
        };
        assert_eq!(crate::dialog_effect(&result), DialogEffect::PsdSave(choice));
    }
    let unknown = DialogResult {
        purpose: DialogPurpose::PsdSaveWithChanges,
        action: "nope".to_owned(),
    };
    assert_eq!(crate::dialog_effect(&unknown), DialogEffect::Unknown);

    // Through the real dialog: open it, press `Enter`, get Cancel.
    let scales = crate::test_workspace_scales();
    let mut workspace = aurora_ui::build_workspace(&scales);
    let mut focus = FocusManager::default();
    let mut dialog = None;
    assert!(crate::open_dialog(
        &mut workspace,
        &mut focus,
        &mut dialog,
        &scales,
        crate::psd_save::PSD_SAVE_WITH_CHANGES_TITLE,
        "message",
        psd_save_dialog_actions(),
        DialogPurpose::PsdSaveWithChanges,
    ));
    let chosen = crate::run_dialog_action(
        &mut workspace,
        &mut focus,
        &mut dialog,
        Some(PSD_SAVE_ANYWAY.to_owned()),
    );
    assert_eq!(
        chosen.map(|r| crate::dialog_effect(&r)),
        Some(DialogEffect::PsdSave(PsdSaveChoice::SaveAnyway))
    );
    assert!(dialog.is_none());
}

#[test]
fn save_psd_message_lists_items_in_fixed_order() {
    let report = aurora_io::PsdWriteReport {
        items: vec!["First.".to_owned(), "Second.".to_owned()],
    };
    let message = psd_save_message("art.psd", &report);
    assert_eq!(
        message,
        "Saving \u{201c}art.psd\u{201d} as a Photoshop file changes it:\n\u{2022} First.\n\
         \u{2022} Second.\nSave Anyway writes the file with these changes; your document is not \
         changed. Nothing has been written yet."
    );
    // And a real report's items arrive in the writer's own fixed order.
    let mut doc = Doc::new();
    doc.make_lossy();
    let top = doc.tree.roots().first().copied();
    let Some(top) = top else { unreachable!() };
    ok(doc.tree.set_opacity(top, 0.3333));
    let a = doc.path("a.psd");
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&a)) else {
        unreachable!("needs confirmation")
    };
    let message = psd_save_message("a.psd", &pending.report);
    let opacity = message.find("rounded to Photoshop's 255 steps");
    let dissolve = message.find("Dissolve layers");
    assert!(
        opacity.is_some() && dissolve.is_some() && opacity < dissolve,
        "{message}"
    );
}

/// A dirty session holding `doc`'s layers.
fn dirty_session(doc: Doc) -> crate::DocumentSession {
    let mut session = crate::DocumentSession::new(crate::DocumentContents {
        layers: doc.tree,
        history: aurora_doc::History::new(),
        canvas_size: (8, 8),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: None,
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: Some(doc.store),
    });
    session.saved_revision = None;
    assert!(session.is_dirty(), "fixture is dirty");
    session
}

#[test]
fn psd_names_are_a_save_kind_of_their_own() {
    use crate::document_close::DialogSave;
    for name in ["art.psd", "ART.PSD", "art.psb"] {
        assert_eq!(
            crate::document_close::dialog_save_kind(Path::new(name)),
            DialogSave::Psd
        );
    }
}

#[test]
fn lossless_psd_save_marks_clean_and_sets_path() {
    let mut doc = Doc::new();
    let path = doc.path("art.psd");
    assert!(matches!(ok(doc.save(&path)), PsdSaveStep::Saved));
    let mut session = dirty_session(doc);
    assert!(record_psd_save(&mut session, &path, true));
    assert!(!session.is_dirty());
    assert_eq!(session.path.as_deref(), Some(path.as_path()));
    assert_eq!(session.name, "art.psd");
}

#[test]
fn lossy_psd_save_anyway_keeps_dirty() {
    let mut doc = Doc::new();
    doc.make_lossy();
    let path = doc.path("art.psd");
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&path)) else {
        unreachable!("needs confirmation")
    };
    ok(finish_psd_save(pending, doc.id, doc.revision));
    assert!(path.exists(), "Save Anyway wrote it");
    let mut session = dirty_session(doc);
    // `App::answer_psd_save`: a save with changes is recorded as lossy.
    assert!(!record_psd_save(&mut session, &path, false));
    assert!(session.is_dirty());
    assert_eq!(session.path, None, "path unchanged");
}

#[test]
fn close_save_to_lossless_psd_closes() {
    use crate::document_close::{CloseChoice, CloseEffect, close_effect};
    let mut doc = Doc::new();
    let path = doc.path("art.psd");
    let lossless = matches!(ok(doc.save(&path)), PsdSaveStep::Saved);
    let mut session = dirty_session(doc);
    let saved = record_psd_save(&mut session, &path, lossless) && !session.is_dirty();
    assert!(saved);
    assert_eq!(
        close_effect(session.id, CloseChoice::Save, saved),
        CloseEffect::Close(session.id)
    );
}

#[test]
fn quit_review_psd_lossless_counts_as_saved() {
    use crate::document_close::{CloseChoice, QuitReview, QuitStep};
    let mut doc = Doc::new();
    let path = doc.path("art.psd");
    let lossless = matches!(ok(doc.save(&path)), PsdSaveStep::Saved);
    let mut session = dirty_session(doc);
    let saved = record_psd_save(&mut session, &path, lossless) && !session.is_dirty();
    let (review, _) = QuitReview::start(vec![session.id]);
    let Some(mut review) = review else {
        unreachable!("one unsaved")
    };
    assert_eq!(
        review.answer(session.id, CloseChoice::Save, saved),
        QuitStep::Quit
    );
}

#[test]
fn close_save_to_lossy_psd_keeps_document_open_and_dirty() {
    use crate::document_close::{CloseChoice, CloseEffect, QuitReview, QuitStep, close_effect};
    let mut doc = Doc::new();
    doc.make_lossy();
    let path = doc.path("art.psd");
    let step = ok(doc.save(&path));
    let lossless = matches!(step, PsdSaveStep::Saved);
    assert!(!lossless);
    drop(step);
    let mut session = dirty_session(doc);
    let saved = record_psd_save(&mut session, &path, lossless) && !session.is_dirty();
    assert!(!saved);
    assert!(session.is_dirty());
    assert_eq!(
        close_effect(session.id, CloseChoice::Save, saved),
        CloseEffect::Keep
    );
    let (review, _) = QuitReview::start(vec![session.id]);
    let Some(mut review) = review else {
        unreachable!("one unsaved")
    };
    assert_eq!(
        review.answer(session.id, CloseChoice::Save, saved),
        QuitStep::Abort
    );
}

/// Writes a clean PSD of `doc` and returns its bytes with the first
/// layer record's clipping byte set — a feature the reader opens
/// "with changes" (shown unclipped).
fn psd_opening_with_changes(doc: &mut Doc) -> Vec<u8> {
    let path = doc.path("source.psd");
    assert!(matches!(ok(doc.save(&path)), PsdSaveStep::Saved));
    let mut bytes = ok(std::fs::read(&path));
    let be32 = |b: &[u8], at: usize| -> usize {
        match b.get(at..at + 4) {
            Some([a, b, c, d]) => u32::from_be_bytes([*a, *b, *c, *d]) as usize,
            _ => unreachable!("short"),
        }
    };
    let section = 26 + 4 + 4 + be32(&bytes, 30);
    let record = section + 4 + 4 + 4 + 12 + 2;
    let clipping = record + 16 + 2 + 4 * 6 + 8 + 1;
    if let Some(b) = bytes.get_mut(clipping) {
        *b = 1;
    }
    bytes
}

#[test]
fn a_psd_opened_with_changes_always_asks_before_a_psd_save() {
    let mut doc = Doc::new();
    let bytes = psd_opening_with_changes(&mut doc);
    let opened = ok(aurora_io::read_psd(&bytes));
    assert!(!opened.report.is_empty(), "the open reports a change");
    // `App::open_psd_file` keeps the open's lines on the session.
    let losses = Some(opened.report.items.clone());
    let skipped = aurora_io::SkippedTiles::new();
    let carried = crate::psd_save::carried_losses(losses.as_deref(), &skipped);
    assert_eq!(
        carried.first().map(String::as_str),
        Some(crate::psd_save::CARRIED_HEADING)
    );
    // Saving over the very file it came from: a flat, otherwise lossless
    // document, so only the carried block is in the report.
    let target = doc.path("source.psd");
    ok(std::fs::write(&target, &bytes));
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save_carrying(&target, carried.clone()))
    else {
        unreachable!("needs confirmation")
    };
    assert_eq!(pending.report.items, carried);
    assert_eq!(ok(std::fs::read(&target)), bytes, "target untouched");
    drop(pending);
    assert_eq!(doc.listing(), ["source.psd"], "temp removed");
}

#[test]
fn a_psd_opened_cleanly_saves_without_a_dialog() {
    let mut doc = Doc::new();
    let path = doc.path("clean.psd");
    assert!(matches!(ok(doc.save(&path)), PsdSaveStep::Saved));
    let opened = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert!(opened.report.is_empty());
    let losses = (!opened.report.items.is_empty()).then(|| opened.report.items.clone());
    let carried =
        crate::psd_save::carried_losses(losses.as_deref(), &aurora_io::SkippedTiles::new());
    assert!(carried.is_empty());
    assert!(matches!(
        ok(doc.save_carrying(&path, carried)),
        PsdSaveStep::Saved
    ));
}

#[test]
fn new_and_other_documents_carry_no_losses_of_their_own() {
    // A new session (New, an open, recovery) starts with none.
    let fresh = dirty_session(Doc::new());
    assert_eq!(fresh.import_losses, None);
    // Each session carries only its own: the save reads the active
    // session's field, so switching documents switches what is carried.
    let mut opened = dirty_session(Doc::new());
    opened.import_losses = Some(vec!["Clipping masks are shown unclipped.".to_owned()]);
    let none = aurora_io::SkippedTiles::new();
    assert!(crate::psd_save::carried_losses(fresh.import_losses.as_deref(), &none).is_empty());
    assert_eq!(
        crate::psd_save::carried_losses(opened.import_losses.as_deref(), &none).len(),
        2
    );
}

#[test]
fn a_deferred_save_with_changes_asks_once_the_dialog_closes() {
    use crate::psd_save::take_deferred_dialog;
    let mut deferred = true;
    assert!(!take_deferred_dialog(&mut deferred, true, true), "waits");
    assert!(deferred);
    assert!(take_deferred_dialog(&mut deferred, true, false), "asks");
    assert!(!deferred, "once");
    assert!(!take_deferred_dialog(&mut deferred, true, false));
    let mut deferred = true;
    assert!(
        !take_deferred_dialog(&mut deferred, false, false),
        "nothing pending"
    );
    assert!(!deferred, "dropped");
}

#[test]
fn preflight_refusal_shows_couldnt_save_with_reason() {
    let mut doc = Doc::new();
    // 0.205.0: adjustment layers are written; one whose rounded values the
    // file cannot hold (two curve points on one 8-bit level) is still refused.
    let x = 102.5_f32 / 255.0;
    let squeezed = ok(aurora_core::ToneCurve::new(&[
        aurora_core::CurvePoint::new(x, 0.2),
        aurora_core::CurvePoint::new(x + 1.0 / 256.0, 0.8),
    ]));
    ok(doc.tree.add_adjustment_layer_at(
        "Curves",
        aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
            composite: squeezed,
            ..aurora_core::CurvesParams::identity()
        }),
        None,
        0,
    ));
    let path = doc.path("copy.psd");
    let Err(failure) = doc.save(&path) else {
        unreachable!("refused")
    };
    let message = psd_save_failure_message("copy.psd", &failure);
    assert!(
        message.contains("the Curves layer \u{201c}Curves\u{201d} can't be saved to .psd")
            && message.contains("fewer than two different 8-bit levels"),
        "{message}"
    );
    assert!(message.ends_with(crate::SAVE_FAILED_UNCHANGED), "{message}");
    // A refusal, worded as one — not as a failed write.
    assert!(
        message.starts_with("Aurora can't save \u{201c}copy.psd\u{201d} as a Photoshop file:"),
        "{message}"
    );
    assert!(!path.exists());
    // Oversize: the hint to use .psb.
    let mut doc = Doc::new();
    let big = doc.path("big.psd");
    let result = begin_psd_save(
        PsdSaveSource {
            layers: &doc.tree,
            canvas: (30_001, 8),
            store: &mut doc.store,
            doc_id: doc.id,
            revision: doc.revision,
            carried: Vec::new(),
        },
        &big,
    );
    let Err(failure) = result else {
        unreachable!("refused")
    };
    assert!(psd_save_failure_message("big.psd", &failure).contains("use .psb"));
}

/// 0.239.0: Vibrance, Exposure and Photo Filter layers have no PSD block
/// yet: the whole app flow refuses (no dialog to "Save Anyway", no file, no
/// temp left), naming the layer and the way out, and the same document
/// saves once the layer is gone.
#[test]
fn vibrance_exposure_and_photo_filter_layers_refuse_the_psd_save_by_name() {
    let cases = [
        (
            aurora_doc::Adjustment::Vibrance(aurora_core::VibranceParams::identity()),
            "Vibrance",
        ),
        (
            aurora_doc::Adjustment::Exposure(aurora_core::ExposureParams::identity()),
            "Exposure",
        ),
        (
            aurora_doc::Adjustment::PhotoFilter(aurora_core::PhotoFilterParams::default()),
            "Photo Filter",
        ),
    ];
    for (adjustment, kind) in cases {
        let mut doc = Doc::new();
        let id = ok(doc.tree.add_adjustment_layer_at("My layer", adjustment, None, 0));
        let path = doc.path("nope.psd");
        let Err(failure) = doc.save(&path) else {
            unreachable!("{kind} must be refused")
        };
        let message = psd_save_failure_message("nope.psd", &failure);
        assert!(
            message.contains(&format!("the {kind} layer \u{201c}My layer\u{201d} can't be saved to .psd"))
                && message.contains("Aurora can't write")
                && message.contains("save as .aur"),
            "{message}"
        );
        assert!(!path.exists());
        assert!(doc.listing().is_empty(), "no temp file is left: {:?}", doc.listing());
        ok(doc.tree.remove(id));
        assert!(matches!(ok(doc.save(&path)), PsdSaveStep::Saved));
    }
}

#[test]
fn psb_refused_until_r5() {
    let mut doc = Doc::new();
    let path = doc.path("copy.psb");
    let Err(failure) = doc.save(&path) else {
        unreachable!("refused")
    };
    let message = psd_save_failure_message("copy.psb", &failure);
    assert!(message.contains("PSB isn't supported yet"), "{message}");
    assert!(!path.exists());
    assert!(doc.listing().is_empty());
}

#[test]
fn save_picker_offers_aur_psd_and_flat_formats() {
    let filters = crate::psd_save::save_file_filters();
    let names: Vec<&str> = filters.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "Aurora document",
            "Photoshop",
            "PNG image",
            "JPEG image",
            "TIFF image"
        ]
    );
    assert!(filters.iter().any(|(_, ext)| ext.contains(&"psd")));
    assert!(!filters.iter().any(|(_, ext)| ext.contains(&"psb")));
}

/// A picker that returns one path.
struct OnePath(Option<PathBuf>);

impl FileDialogAccess for OnePath {
    fn pick_file(&mut self) -> Option<PathBuf> {
        None
    }

    fn save_file(&mut self) -> Option<PathBuf> {
        self.0.take()
    }
}

#[test]
fn save_as_psd_end_to_end_through_the_picker_reopens_with_the_same_layers() {
    let mut doc = Doc::new();
    let path = doc.path("picked.psd");
    let scales = crate::test_workspace_scales();
    let mut workspace = aurora_ui::build_workspace(&scales);
    let mut focus = FocusManager::default();
    let mut picker = OnePath(Some(path.clone()));
    let picked = crate::activate_command(
        &mut workspace,
        &mut focus,
        crate::COMMAND_FILE_SAVE,
        &mut picker,
    );
    let Some(crate::ActivatedCommand::SaveFile(chosen)) = picked else {
        unreachable!("save chosen")
    };
    // `App::save_file`'s routing for this name.
    assert!(crate::psd_save::is_psd_save_path(&chosen));
    assert!(!crate::is_aur_path(&chosen));
    assert!(matches!(ok(doc.save(&chosen)), PsdSaveStep::Saved));
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert_eq!(back.canvas_size, (8, 8));
    assert_eq!(names_bottom_up(&back.layers), ["Background", "Top"]);
    assert!(back.report.is_empty(), "{:?}", back.report);
    // Same pixels: the top layer's first pixel.
    let top = back.layers.roots().first().copied();
    let px = back.pixels.iter().find(|p| Some(p.layer) == top);
    let Some(px) = px else { unreachable!() };
    let first: Vec<f32> = px
        .image
        .samples()
        .iter()
        .take(4)
        .map(|v| v.to_f32())
        .collect();
    assert_eq!(first, [0.75, 0.5, 0.0, 1.0]);
}

#[cfg(unix)]
#[test]
fn psd_save_through_a_symlink_replaces_the_real_file() {
    let mut doc = Doc::new();
    let real = doc.path("real.psd");
    ok(std::fs::write(&real, b"old"));
    let link = doc.path("link.psd");
    ok(std::os::unix::fs::symlink(&real, &link));
    assert!(matches!(ok(doc.save(&link)), PsdSaveStep::Saved));
    assert!(
        ok(std::fs::symlink_metadata(&link))
            .file_type()
            .is_symlink()
    );
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&real))));
    assert_eq!(names_bottom_up(&back.layers), ["Background", "Top"]);
}

/// A PSD that opens with changes, and the lines its open reports.
fn lossy_open(doc: &mut Doc) -> (Vec<u8>, Vec<String>) {
    let bytes = psd_opening_with_changes(doc);
    let opened = ok(aurora_io::read_psd(&bytes));
    assert!(!opened.report.is_empty());
    (bytes, opened.report.items)
}

/// After a PSD save of `doc` carrying `losses` over `target` (holding
/// `bytes`): it asks, carries the block, and leaves the target alone.
fn assert_asks(doc: &mut Doc, target: &Path, bytes: &[u8], losses: Option<&[String]>) {
    let carried = crate::psd_save::carried_losses(losses, &aurora_io::SkippedTiles::new());
    assert_eq!(
        carried.first().map(String::as_str),
        Some(crate::psd_save::CARRIED_HEADING)
    );
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save_carrying(target, carried.clone()))
    else {
        unreachable!("needs confirmation")
    };
    assert_eq!(pending.report.items, carried);
    assert_eq!(ok(std::fs::read(target)), bytes, "target untouched");
}

#[test]
fn a_lossy_psd_saved_as_aur_and_reopened_still_asks_before_a_psd_save() {
    let mut doc = Doc::new();
    let (bytes, losses) = lossy_open(&mut doc);
    let target = doc.path("source.psd");
    ok(std::fs::write(&target, &bytes));
    // `App::save_aur_file`'s write, then `App::open_aur_file`'s read.
    let aur = doc.path("work.aur");
    let file = ok(std::fs::File::create(&aur));
    ok(aurora_io::write_aur_with_import_losses(
        file,
        &doc.tree,
        &aurora_doc::History::new(),
        (8, 8),
        None,
        &aurora_io::SkippedTiles::new(),
        Some(&losses),
        None,
        &mut doc.store,
    ));
    let (_dir, mut fresh) = fresh_store();
    let reopened = ok(aurora_io::read_aur(
        ok(std::fs::File::open(&aur)),
        &mut fresh,
    ));
    assert_eq!(reopened.import_losses.as_deref(), Some(losses.as_slice()));
    assert_asks(&mut doc, &target, &bytes, reopened.import_losses.as_deref());
}

fn fresh_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero")
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

#[test]
fn a_lossy_psd_recovered_after_a_crash_still_asks_before_a_psd_save() {
    let mut doc = Doc::new();
    let (bytes, losses) = lossy_open(&mut doc);
    let target = doc.path("source.psd");
    ok(std::fs::write(&target, &bytes));
    let history = aurora_doc::History::new();
    // The background autosave: a snapshot here, written as the worker does.
    let snap_path = doc.path("snapshot.aur");
    let job = match crate::snapshot_autosave(
        &snap_path,
        &doc.tree,
        &history,
        (8, 8),
        &mut aurora_io::SkippedTiles::new(),
        crate::AUTOSAVE_SNAPSHOT_BUDGET_BYTES,
        &mut doc.store,
        Some(&losses),
        None,
    ) {
        crate::SnapshotOutcome::Taken(job) => job,
        other => unreachable!("{other:?}"),
    };
    ok(aurora_io::write_aur_snapshot(
        ok(std::fs::File::create(&snap_path)),
        &job.snapshot,
    ));
    // The over-budget synchronous autosave.
    let sync_path = doc.path("sync.aur");
    crate::write_autosave(
        &sync_path,
        &doc.tree,
        &history,
        (8, 8),
        &mut aurora_io::SkippedTiles::new(),
        &mut doc.store,
        Some(&losses),
        None,
    );
    for path in [&snap_path, &sync_path] {
        let (_dir, mut fresh) = fresh_store();
        let Some(recovered) = crate::recover_document(path, &mut fresh) else {
            unreachable!("recovers")
        };
        assert_eq!(recovered.import_losses.as_deref(), Some(losses.as_slice()));
        assert_asks(
            &mut doc,
            &target,
            &bytes,
            recovered.import_losses.as_deref(),
        );
    }
}

#[test]
fn an_old_aur_without_the_entry_carries_nothing() {
    let mut doc = Doc::new();
    let aur = doc.path("old.aur");
    ok(aurora_io::write_aur(
        ok(std::fs::File::create(&aur)),
        &doc.tree,
        &aurora_doc::History::new(),
        (8, 8),
        None,
        &aurora_io::SkippedTiles::new(),
        &mut doc.store,
    ));
    let (_dir, mut fresh) = fresh_store();
    let reopened = ok(aurora_io::read_aur(
        ok(std::fs::File::open(&aur)),
        &mut fresh,
    ));
    assert_eq!(reopened.import_losses, None);
    let carried = crate::psd_save::carried_losses(
        reopened.import_losses.as_deref(),
        &aurora_io::SkippedTiles::new(),
    );
    assert!(carried.is_empty());
    let target = doc.path("old.psd");
    assert!(matches!(
        ok(doc.save_carrying(&target, carried)),
        PsdSaveStep::Saved
    ));
}

/// The tree's shape, top-to-bottom: `name` or `name(children)`.
fn shape(layers: &LayerTree, ids: &[aurora_doc::LayerId]) -> String {
    ids.iter()
        .map(|&id| {
            let name = layers.name(id).unwrap_or_default().to_owned();
            match layers.kind(id) {
                Some(aurora_doc::LayerKind::Group { children }) => {
                    format!("{name}({})", shape(layers, children))
                }
                _ => name,
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn find(layers: &LayerTree, name: &str) -> aurora_doc::LayerId {
    let mut stack: Vec<aurora_doc::LayerId> = layers.roots().to_vec();
    while let Some(id) = stack.pop() {
        if layers.name(id) == Some(name) {
            return id;
        }
        if let Some(children) = layers.children(id) {
            stack.extend_from_slice(children);
        }
    }
    unreachable!("no layer named {name}")
}

/// 0.178.0 (R3): a document with nested groups and a masked layer goes
/// through Save As `.psd` and comes back with the same tree, the same
/// group and layer properties, the same layer pixels and the same mask.
#[test]
fn save_as_psd_round_trips_nested_groups_and_a_masked_layer() {
    let mut doc = Doc::new();
    // Background, Top at the root; then Outer(Inner(Masked), Loose) on top.
    let outer = ok(doc.tree.add_group("Outer", None));
    ok(doc.tree.set_blend_mode(outer, BlendMode::Multiply));
    ok(doc.tree.set_opacity(outer, 128.0 / 255.0));
    let loose = ok(doc
        .tree
        .add_pixel_layer("Loose", rect(0, 0, 8, 8), Some(outer)));
    let inner = ok(doc.tree.add_group("Inner", Some(outer)));
    ok(doc.tree.set_visible(inner, false));
    let masked = ok(doc
        .tree
        .add_pixel_layer("Masked", rect(0, 0, 8, 8), Some(inner)));
    for (id, value) in [(loose, 0.125f32), (masked, 0.625)] {
        let samples: Vec<f16> = (0..64)
            .flat_map(|i| [value, (i % 8) as f32 / 8.0, 0.5, 1.0].map(f16::from_f32))
            .collect();
        let image = ok(aurora_io::Image::new(8, 8, IccProfile::srgb(), samples));
        let Some(surface) = doc.tree.surface_id(id) else {
            unreachable!("pixel layer")
        };
        ok(aurora_io::write_into_store_at(
            &image,
            &mut doc.store,
            surface,
            0,
            0,
        ));
    }
    ok(doc.tree.add_mask(masked, rect(1, 2, 5, 4)));
    let Some(mask_surface) = doc.tree.mask_surface_id(masked) else {
        unreachable!("mask surface")
    };
    ok(aurora_doc::write_mask_coverage_region(
        &mut doc.store,
        mask_surface,
        (0, 0),
        5,
        4,
        |x, y| ((x + y) % 5) as f32 / 4.0,
    ));
    let want_shape = shape(&doc.tree, doc.tree.roots());
    assert_eq!(want_shape, "Outer(Inner(Masked),Loose),Top,Background");

    let path = doc.path("groups.psd");
    assert!(
        matches!(ok(doc.save(&path)), PsdSaveStep::Saved),
        "lossless"
    );
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert!(back.report.items.is_empty(), "{:?}", back.report);
    assert_eq!(shape(&back.layers, back.layers.roots()), want_shape);

    // Group and layer properties.
    let l = &back.layers;
    let (b_outer, b_inner) = (find(l, "Outer"), find(l, "Inner"));
    assert_eq!(l.blend_mode(b_outer), Some(BlendMode::Multiply));
    assert_eq!(l.opacity(b_outer), Some(128.0 / 255.0));
    assert_eq!(l.visible(b_inner), Some(false));
    assert_eq!(l.visible(b_outer), Some(true));

    // Pixels: every layer's decoded image equals the original exactly.
    for name in ["Background", "Top", "Loose", "Masked"] {
        let (orig, got) = (find(&doc.tree, name), find(l, name));
        let Some(surface) = doc.tree.surface_id(orig) else {
            unreachable!("pixel layer")
        };
        let want = ok(aurora_io::read_from_store(&mut doc.store, surface, 8, 8));
        let pixels = back.pixels.iter().find(|p| p.layer == got);
        let Some(pixels) = pixels else {
            unreachable!("{name} has pixels")
        };
        assert_eq!(pixels.offset, (0, 0), "{name}");
        assert_eq!(pixels.image.samples(), want.samples(), "{name} pixels");
    }

    // The mask: same bounds, enabled, coverage sample for sample.
    let b_masked = find(l, "Masked");
    let mask = l.mask(b_masked).cloned();
    let Some(mask) = mask else {
        unreachable!("mask read back")
    };
    assert_eq!(mask.bounds, rect(1, 2, 5, 4));
    assert!(mask.enabled && !mask.inverted);
    let coverage = back.masks.iter().find(|m| m.layer == b_masked);
    let Some(coverage) = coverage else {
        unreachable!("mask coverage")
    };
    assert_eq!(
        (coverage.offset, coverage.width, coverage.height),
        ((0, 0), 5, 4)
    );
    let want: Vec<f16> = (0..4u32)
        .flat_map(|y| (0..5u32).map(move |x| f16::from_f32(((x + y) % 5) as f32 / 4.0)))
        .collect();
    assert_eq!(coverage.coverage, want);
}

// -- 0.200.0: smart-object preservation (round S1) ---------------------

const SMART_ID: &str = "0001bdab-4207-8a45-9be1-e66466dde9b7";

/// A framed linked-layer entry holding `payload`: the length, `liFD`,
/// version, the Pascal ID, and filler, padded to four.
fn linked_entry(payload: &[u8]) -> Vec<u8> {
    let mut body = b"liFD".to_vec();
    body.extend_from_slice(&7u32.to_be_bytes());
    body.push(SMART_ID.len() as u8);
    body.extend_from_slice(SMART_ID.as_bytes());
    body.extend_from_slice(payload);
    let mut framed = (body.len() as u64).to_be_bytes().to_vec();
    framed.extend_from_slice(&body);
    while !framed.len().is_multiple_of(4) {
        framed.push(0);
    }
    framed
}

impl Doc {
    /// Makes "Top" a smart object whose data is still true.
    fn make_smart(&mut self) -> aurora_doc::LayerId {
        let Some(top) = self.tree.roots().first().copied() else {
            unreachable!("layers")
        };
        let bounds = rect(0, 0, 8, 8);
        let Some(surface) = self.tree.surface_id(top) else {
            unreachable!("pixel layer")
        };
        let fingerprint = ok(aurora_doc::smart_object::surface_fingerprint(
            &mut self.store,
            surface,
            bounds,
        ));
        ok(self.tree.add_linked_file(aurora_doc::LinkedFile {
            list: *b"lnk2",
            unique_id: SMART_ID.to_owned(),
            entry: std::sync::Arc::new(linked_entry(&[9; 33])),
        }));
        ok(self.tree.set_smart_object(
            top,
            Some(aurora_doc::SmartObject {
                unique_id: SMART_ID.to_owned(),
                blocks: vec![aurora_doc::SmartObjectBlock {
                    key: *b"PlLd",
                    data: {
                        let mut d = b"plcL\0\0\0\x03".to_vec();
                        d.push(SMART_ID.len() as u8);
                        d.extend_from_slice(SMART_ID.as_bytes());
                        d.extend_from_slice(&[0, 0, 0]);
                        d
                    },
                }],
                bounds,
                pixel_fingerprint: fingerprint,
            }),
        ));
        top
    }
}

#[test]
fn an_untouched_smart_object_saves_losslessly_and_reopens_as_one() {
    let mut doc = Doc::new();
    doc.make_smart();
    let path = doc.path("smart.psd");
    assert!(matches!(ok(doc.save(&path)), PsdSaveStep::Saved));
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    let kept = back
        .layers
        .roots()
        .iter()
        .filter(|&&id| back.layers.smart_object(id).is_some())
        .count();
    assert_eq!(kept, 1);
    assert_eq!(
        back.layers
            .linked_files()
            .get(SMART_ID)
            .map(|f| f.entry.to_vec()),
        Some(linked_entry(&[9; 33]))
    );
}

#[test]
fn a_brush_dab_on_a_smart_object_layer_drops_it_with_an_itemised_report() {
    let mut doc = Doc::new();
    let top = doc.make_smart();
    let Some(surface) = doc.tree.surface_id(top) else {
        unreachable!("pixel layer")
    };
    // The real brush, as a stroke on the layer would.
    let outcome = aurora_brush::stamp_dab(
        &mut doc.store,
        surface,
        (4.0, 4.0),
        6.0,
        [1.0, 0.0, 0.0],
        None,
    );
    assert!(!outcome.painted().is_empty(), "the dab painted");
    let path = doc.path("smart.psd");
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&path)) else {
        unreachable!("a smart object lost: the user must agree")
    };
    let text = pending.report.items.join("\n");
    assert!(text.contains("painted or erased"), "{text}");
    assert!(text.contains("\u{201c}Top\u{201d}"), "{text}");
    ok(finish_psd_save(pending, doc.id, doc.revision));
    let back = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert!(back.layers.linked_files().is_empty());
    assert!(
        back.layers
            .roots()
            .iter()
            .all(|&id| back.layers.smart_object(id).is_none())
    );
}

#[test]
fn moving_a_smart_object_layer_drops_it_with_an_itemised_report() {
    let mut doc = Doc::new();
    let top = doc.make_smart();
    ok(doc.tree.set_bounds(top, rect(2, 1, 8, 8)));
    let path = doc.path("smart.psd");
    let PsdSaveStep::NeedsConfirmation(pending) = ok(doc.save(&path)) else {
        unreachable!("a smart object lost: the user must agree")
    };
    let text = pending.report.items.join("\n");
    assert!(text.contains("they were moved"), "{text}");
}

/// 0.226.0: the Crop moves every layer's bounds by the negated crop origin
/// and does NOT move a smart object's recorded placement (the `SoLd` data is
/// in document coordinates and would be stale), so a smart object survives a
/// crop whose origin is (0, 0) and is written as ordinary pixels, with a
/// report naming the layer, after any crop that moves the origin; undoing
/// the crop makes it a smart object again. All through the real PSD save.
mod crop_and_smart_objects {
    use super::*;
    use crate::crop::crop_document;
    use crate::{UndoDirection, UndoOrder, step_undo_order};

    /// The document's crop machinery beside the `Doc`.
    struct Cropper {
        history: aurora_doc::History,
        pixel_history: aurora_brush::PixelHistory,
        undo_order: UndoOrder,
        selection: aurora_doc::SelectionSet,
        canvas: (u32, u32),
    }

    impl Cropper {
        fn new() -> Self {
            Self {
                history: aurora_doc::History::new(),
                pixel_history: aurora_brush::PixelHistory::new(),
                undo_order: UndoOrder::default(),
                selection: aurora_doc::SelectionSet::new(),
                canvas: (8, 8),
            }
        }

        fn crop(&mut self, doc: &mut Doc, r: Rect) {
            ok(crop_document(
                &mut doc.tree,
                &mut self.history,
                &mut self.pixel_history,
                &mut self.undo_order,
                &mut self.selection,
                &mut self.canvas,
                false,
                r,
            )
            .map_err(|why| format!("{why:?}")));
        }

        fn undo(&mut self, doc: &mut Doc) {
            assert!(
                step_undo_order(
                    &mut doc.tree,
                    &mut self.history,
                    &mut self.pixel_history,
                    Some(&mut doc.store),
                    &mut self.undo_order,
                    UndoDirection::Undo,
                )
                .is_some()
            );
            crate::crop::apply_pending_crop(
                &mut self.undo_order,
                &mut self.canvas,
                &mut self.selection,
            );
        }

        fn save(&self, doc: &mut Doc, path: &Path) -> PsdSaveStep {
            ok(begin_psd_save(
                PsdSaveSource {
                    layers: &doc.tree,
                    canvas: self.canvas,
                    store: &mut doc.store,
                    doc_id: doc.id,
                    revision: doc.revision,
                    carried: Vec::new(),
                },
                path,
            ))
        }
    }

    fn smart_layers(path: &Path) -> usize {
        let back = ok(aurora_io::read_psd(&ok(std::fs::read(path))));
        back.layers
            .roots()
            .iter()
            .filter(|&&id| back.layers.smart_object(id).is_some())
            .count()
    }

    #[test]
    fn a_crop_at_the_origin_keeps_the_smart_object() {
        let mut doc = Doc::new();
        let top = doc.make_smart();
        let mut c = Cropper::new();
        c.crop(&mut doc, rect(0, 0, 6, 5));
        assert_eq!(c.canvas, (6, 5));
        assert_eq!(
            doc.tree.bounds(top),
            Some(rect(0, 0, 8, 8)),
            "origin unchanged"
        );
        let path = doc.path("origin.psd");
        assert!(matches!(c.save(&mut doc, &path), PsdSaveStep::Saved));
        assert_eq!(smart_layers(&path), 1);
    }

    #[test]
    fn a_crop_that_moves_the_origin_writes_it_as_pixels_and_names_the_layer() {
        for r in [rect(2, 0, 6, 8), rect(0, 3, 8, 5), rect(1, 1, 5, 5)] {
            let mut doc = Doc::new();
            let top = doc.make_smart();
            let mut c = Cropper::new();
            c.crop(&mut doc, r);
            assert_eq!(doc.tree.bounds(top), Some(rect(-r.x, -r.y, 8, 8)));
            let path = doc.path("moved.psd");
            let PsdSaveStep::NeedsConfirmation(pending) = c.save(&mut doc, &path) else {
                unreachable!("a smart object lost: the user must agree ({r:?})")
            };
            let text = pending.report.items.join("\n");
            assert!(text.contains("they were moved"), "{text}");
            assert!(text.contains("\u{201c}Top\u{201d}"), "{text}");
            ok(finish_psd_save(pending, doc.id, doc.revision));
            assert_eq!(smart_layers(&path), 0, "written as ordinary pixels ({r:?})");
        }
    }

    #[test]
    fn undoing_the_crop_makes_it_a_smart_object_again() {
        let mut doc = Doc::new();
        let top = doc.make_smart();
        let mut c = Cropper::new();
        c.crop(&mut doc, rect(2, 1, 6, 7));
        let moved = doc.path("moved.psd");
        assert!(matches!(
            c.save(&mut doc, &moved),
            PsdSaveStep::NeedsConfirmation(_)
        ));
        c.undo(&mut doc);
        assert_eq!(c.canvas, (8, 8));
        assert_eq!(doc.tree.bounds(top), Some(rect(0, 0, 8, 8)));
        let back = doc.path("back.psd");
        assert!(matches!(c.save(&mut doc, &back), PsdSaveStep::Saved));
        assert_eq!(smart_layers(&back), 1);
    }
}

/// 0.205.0: a document with Curves, Levels and Hue/Saturation layers saves
/// to PSD through the whole app flow. Exact values are lossless (no
/// dialog, the target replaced); rounded ones ask first, naming the kind.
#[test]
fn adjustment_layers_save_losslessly_and_rounded_ones_ask_first() {
    let mut doc = Doc::new();
    let curve = ok(aurora_core::ToneCurve::new(&[
        aurora_core::CurvePoint::new(0.0, 0.0),
        aurora_core::CurvePoint::new(64.0 / 255.0, 90.0 / 255.0),
        aurora_core::CurvePoint::new(1.0, 1.0),
    ]));
    ok(doc.tree.add_adjustment_layer_at(
        "Curves",
        aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
            composite: curve,
            ..aurora_core::CurvesParams::identity()
        }),
        None,
        0,
    ));
    ok(doc.tree.add_adjustment_layer_at(
        "Levels",
        aurora_doc::Adjustment::Levels(aurora_core::LevelsParams::identity()),
        None,
        0,
    ));
    ok(doc.tree.add_adjustment_layer_at(
        "Hue",
        aurora_doc::Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
            15.0, -30.0, 10.0, false, 0.0, 25.0, 0.0,
        ))),
        None,
        0,
    ));
    let path = doc.path("adjusted.psd");
    let step = ok(doc.save(&path));
    assert!(matches!(step, PsdSaveStep::Saved), "{step:?}");
    let reopened = ok(aurora_io::read_psd(&ok(std::fs::read(&path))));
    assert_eq!(
        names_bottom_up(&reopened.layers),
        ["Background", "Top", "Curves", "Levels", "Hue"]
    );
    assert_eq!(listing_without_scratch(&doc), ["adjusted.psd"]);

    // A fractional Hue/Saturation value is rounded: Save Anyway first.
    let hue = find(&doc.tree, "Hue");
    ok(doc.tree.set_adjustment(
        hue,
        aurora_doc::Adjustment::HueSaturation(ok(aurora_core::HueSaturationParams::new(
            15.4, -30.0, 10.0, false, 0.0, 25.0, 0.0,
        ))),
    ));
    let target = doc.path("rounded.psd");
    let step = ok(doc.save(&target));
    let PsdSaveStep::NeedsConfirmation(pending) = step else {
        unreachable!("asks first")
    };
    assert!(!target.exists());
    assert!(
        pending
            .report
            .items
            .iter()
            .any(|i| i.contains("Hue/Saturation layer's fractional values")),
        "{:?}",
        pending.report.items
    );
    let message = psd_save_message("rounded.psd", &pending.report);
    assert!(message.contains("rounded to whole numbers"), "{message}");
    ok(finish_psd_save(pending, doc.id, doc.revision));
    assert!(target.exists());
}

fn listing_without_scratch(doc: &Doc) -> Vec<String> {
    doc.listing()
}
