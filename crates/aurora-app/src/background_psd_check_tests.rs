//! Tests for the background Check PSD Compatibility (task 162).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use aurora_color::IccProfile;
use aurora_core::Rect;
use aurora_doc::{BlendMode, LayerTree};
use half::f16;

use super::*;
use crate::document_session::DocumentId;
use crate::psd_save::{PsdSaveFailure, PsdSaveSource};

const PATIENCE: Duration = Duration::from_secs(20);
const LONG_PATIENCE: Duration = Duration::from_mins(10);

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// A two-layer document in its own store; `budget` tiles stay resident, so
/// a small one pages tiles out to the scratch disk.
struct Doc {
    tree: LayerTree,
    store: aurora_tile::TileStore,
    _dir: tempfile::TempDir,
    id: DocumentId,
    revision: crate::UndoRevision,
    canvas: (u32, u32),
    surfaces: Vec<aurora_tile::SurfaceId>,
}

impl Doc {
    fn new(width: u32, height: u32, budget: usize) -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(budget) else {
            unreachable!("non-zero")
        };
        let scratch = dir.path().join("scratch");
        ok(std::fs::create_dir(&scratch));
        let mut store = ok(aurora_tile::TileStore::new(scratch, budget));
        let mut tree = LayerTree::new();
        let mut surfaces = Vec::new();
        for (name, value) in [("Background", 0.25f32), ("Top", 0.75)] {
            let bounds = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            let id = ok(tree.add_pixel_layer(name, bounds, None));
            let samples: Vec<f16> = (0..width * height)
                .flat_map(|i| [value, 0.5, (i % 8) as f32 / 8.0, 1.0].map(f16::from_f32))
                .collect();
            let image = ok(aurora_io::Image::new(
                width,
                height,
                IccProfile::srgb(),
                samples,
            ));
            let Some(surface) = tree.surface_id(id) else {
                unreachable!("pixel layer")
            };
            surfaces.push(surface);
            ok(aurora_io::write_into_store_at(
                &image, &mut store, surface, 0, 0,
            ));
        }
        Self {
            tree,
            store,
            _dir: dir,
            id: DocumentId::next(),
            revision: crate::UndoRevision(3),
            canvas: (width, height),
            surfaces,
        }
    }

    /// A Dissolve layer: the report has a line.
    fn make_lossy(&mut self) {
        let Some(top) = self.tree.roots().first().copied() else {
            unreachable!()
        };
        ok(self.tree.set_blend_mode(top, BlendMode::Dissolve));
    }

    /// Writes one over-range texel into the top layer's first tile: the
    /// report then counts a clipped colour value, a line that depends on
    /// pixels and not on the tree.
    fn set_hot_texel(&mut self, value: f32) {
        let Some(&surface) = self.surfaces.last() else {
            unreachable!()
        };
        let tile = ok(self
            .store
            .get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }));
        if let Some(texel) = tile.texels_mut().first_mut() {
            *texel = f16::from_f32(value);
        }
    }

    fn name() -> String {
        "Photo".to_owned()
    }

    fn source(&mut self, carried: Vec<aurora_io::ReportEntry>) -> PsdSaveSource<'_> {
        PsdSaveSource {
            saved: Vec::new(),
            layers: &self.tree,
            canvas: self.canvas,
            store: &mut self.store,
            doc_id: self.id,
            revision: self.revision,
            carried,
        }
    }

    fn job(&mut self, carried: Vec<aurora_io::ReportEntry>) -> CheckJob {
        let inputs = CheckInputs {
            name: Self::name(),
            doc_id: self.id,
            revision: self.revision,
            layers: &self.tree,
            canvas: self.canvas,
            saved: Vec::new(),
            carried,
        };
        ok(CheckJob::snapshot(inputs, &mut self.store))
    }
}

fn wait<T>(rx: &mpsc::Receiver<T>) -> T {
    match rx.recv_timeout(PATIENCE) {
        Ok(v) => v,
        Err(err) => unreachable!("timed out: {err:?}"),
    }
}

/// Starts `job` on a worker and waits for its result.
fn run_on_worker(worker: &mut PsdCheckWorker, job: CheckJob) -> FinishedCheck {
    let (tx, rx) = mpsc::channel();
    let _ = ok(worker.start(job, move || {
        let _ = tx.send(());
    }));
    // A large document's check takes tens of seconds in a dev build.
    assert!(
        rx.recv_timeout(LONG_PATIENCE).is_ok(),
        "the check never woke"
    );
    match worker.take_finished() {
        Some(finished) => finished,
        None => unreachable!("no result after the wake"),
    }
}

fn meta(doc_id: DocumentId, revision: u64) -> CheckMeta {
    CheckMeta {
        name: "Photo".to_owned(),
        doc_id,
        revision: crate::UndoRevision(revision),
    }
}

/// A check held open until the returned sender fires (or `PATIENCE`).
fn gated_start(
    worker: &mut PsdCheckWorker,
    meta: CheckMeta,
) -> (mpsc::Sender<()>, mpsc::Receiver<()>, Arc<AtomicBool>) {
    let (release_tx, release) = mpsc::channel::<()>();
    let (woken_tx, woken) = mpsc::channel();
    let seen_cancel = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&seen_cancel);
    let _ = ok(worker.start_with(
        meta,
        move |cancel| {
            let _ = release.recv_timeout(PATIENCE);
            seen.store(cancel.load(Ordering::SeqCst), Ordering::SeqCst);
            Ok(aurora_io::PsdWriteReport::default())
        },
        move || {
            let _ = woken_tx.send(());
        },
    ));
    (release_tx, woken, seen_cancel)
}

// ---- the report is the synchronous one ----

#[test]
fn the_worker_report_and_dialog_equal_the_direct_checks() {
    for (width, height, budget) in [(8, 8, 16), (600, 300, 2)] {
        let mut doc = Doc::new(width, height, budget);
        doc.make_lossy();
        doc.set_hot_texel(2.0);
        let carried = vec![aurora_io::ReportEntry::new(
            aurora_io::ReportCategory::FromOpen,
            "A line carried from the open.".to_owned(),
        )];
        let direct = ok(crate::psd_save::check_psd_compatibility(
            doc.source(carried.clone()),
        ));
        assert!(!direct.entries.is_empty());
        let job = doc.job(carried);
        assert!(job.tile_count() > 0);
        let mut worker = PsdCheckWorker::default();
        let finished = run_on_worker(&mut worker, job);
        let report = ok(finished.result.as_ref().map_err(|_| ()));
        assert_eq!(*report, direct, "{width}x{height} budget {budget}");
        let want = dialog_text(&Doc::name(), &Ok(direct), false);
        let got = dialog_text(&finished.meta.name, &finished.result, false);
        assert_eq!(got, want, "byte for byte");
    }
}

#[test]
fn a_report_is_the_document_as_it_was_when_the_check_started() {
    let mut doc = Doc::new(8, 8, 16);
    doc.set_hot_texel(2.0);
    let before = ok(crate::psd_save::check_psd_compatibility(
        doc.source(Vec::new()),
    ));
    assert!(
        before
            .entries
            .iter()
            .any(|e| e.text.contains("outside the 0–1 range")),
        "{:?}",
        before.entries
    );
    let job = doc.job(Vec::new());
    // Edits after the snapshot: the hot texel is repainted, a layer is
    // made lossy.
    doc.set_hot_texel(0.5);
    doc.make_lossy();
    let after = ok(crate::psd_save::check_psd_compatibility(
        doc.source(Vec::new()),
    ));
    assert_ne!(after, before, "the live document did change");
    let mut worker = PsdCheckWorker::default();
    let finished = run_on_worker(&mut worker, job);
    assert_eq!(ok(finished.result.map_err(|_| ())), before);
}

#[test]
fn an_edit_adds_the_note_to_the_dialog_and_nothing_else_does() {
    let report = aurora_io::PsdWriteReport::default();
    let plain = dialog_text("Photo", &Ok(report.clone()), false);
    let edited = dialog_text("Photo", &Ok(report), true);
    assert_eq!(edited.0, plain.0);
    assert_eq!(edited.1, format!("{}{EDITED_NOTE}", plain.1));
    // A failure gets no note.
    let failed = Err(CheckFailure::Failed(PsdSaveFailure::Refused(
        "x".to_owned(),
    )));
    assert_eq!(
        dialog_text("Photo", &failed, true),
        dialog_text("Photo", &failed, false)
    );
}

#[test]
fn a_refused_document_shows_the_calm_failure_dialog_and_writes_nothing() {
    let mut doc = Doc::new(8, 8, 16);
    let mut job = doc.job(Vec::new());
    job.canvas = (0, 0);
    let mut worker = PsdCheckWorker::default();
    let finished = run_on_worker(&mut worker, job);
    let (title, message) = dialog_text("Photo", &finished.result, false);
    assert_eq!(title, crate::psd_save::PSD_CHECK_FAILED_TITLE);
    assert!(message.contains("No file was written"), "{message}");
}

#[test]
#[allow(clippy::panic)] // the point of the test
fn a_panicking_check_is_a_failed_check_not_a_lost_one() {
    let mut worker = PsdCheckWorker::default();
    let (tx, rx) = mpsc::channel();
    let _ = ok(worker.start_with(
        meta(DocumentId::next(), 1),
        |_| -> Result<aurora_io::PsdWriteReport, CheckFailure> { panic!("boom") },
        move || {
            let _ = tx.send(());
        },
    ));
    wait(&rx);
    let Some(finished) = worker.take_finished() else {
        unreachable!("a result")
    };
    assert!(matches!(finished.result, Err(CheckFailure::Panicked(ref m)) if m == "boom"));
    let (title, message) = dialog_text("Photo", &finished.result, false);
    assert_eq!(title, crate::psd_save::PSD_CHECK_FAILED_TITLE);
    assert!(message.contains("boom"), "{message}");
}

// ---- off the calling thread, single flight ----

#[test]
fn the_check_runs_off_the_calling_thread_and_wakes_after_its_result_is_ready() {
    let caller = std::thread::current().id();
    let ran_on = Arc::new(std::sync::Mutex::new(None));
    let seen = Arc::clone(&ran_on);
    let mut worker = PsdCheckWorker::default();
    let (tx, rx) = mpsc::channel();
    let _ = ok(worker.start_with(
        meta(DocumentId::next(), 1),
        move |_| {
            if let Ok(mut slot) = seen.lock() {
                *slot = Some(std::thread::current().id());
            }
            Ok(aurora_io::PsdWriteReport::default())
        },
        move || {
            let _ = tx.send(());
        },
    ));
    wait(&rx);
    // Result before wake: the wake's receiver always finds it.
    assert!(worker.take_finished().is_some());
    let thread = ran_on.lock().ok().and_then(|slot| *slot);
    assert!(thread.is_some_and(|thread| thread != caller));
}

#[test]
fn a_second_check_while_one_runs_is_refused_and_does_not_start_a_job() {
    let mut worker = PsdCheckWorker::default();
    let (release, woken, _) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    assert!(worker.is_busy());
    let started = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&started);
    let second = worker.start_with(
        meta(DocumentId::next(), 1),
        move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(aurora_io::PsdWriteReport::default())
        },
        || {},
    );
    assert!(matches!(second, Err(StartFailure::Busy)), "{second:?}");
    let _ = release.send(());
    wait(&woken);
    assert!(worker.take_finished().is_some());
    assert_eq!(started.load(Ordering::SeqCst), 0, "no second job ran");
    // Free again once the first is taken and its thread has ended.
    let deadline = std::time::Instant::now() + PATIENCE;
    while worker.is_busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!worker.is_busy());
    assert!(
        worker
            .start_with(
                meta(DocumentId::next(), 1),
                |_| Ok(aurora_io::PsdWriteReport::default()),
                || {}
            )
            .is_ok()
    );
}

#[test]
fn a_cancelled_check_still_winding_down_keeps_the_worker_busy() {
    let mut worker = PsdCheckWorker::default();
    let (release, woken, _) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    assert!(worker.cancel());
    assert!(!worker.is_checking());
    assert!(worker.is_busy(), "its thread still holds a composite");
    let _ = release.send(());
    wait(&woken);
}

// ---- the indicator ----

#[test]
fn the_checking_indicator_appears_while_pending_and_clears_on_every_outcome() {
    let workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let root = workspace.tree.root();
    let update = || workspace.tree.accessibility_update(root);
    let shown = |checking: bool| {
        let u = with_check_status(update(), root, checking);
        let listed = u
            .nodes
            .iter()
            .find(|(id, _)| *id == root)
            .is_some_and(|(_, n)| n.children().contains(&CHECK_STATUS_NODE));
        let node = u
            .nodes
            .iter()
            .find(|(id, _)| *id == CHECK_STATUS_NODE)
            .map(|(_, n)| (n.role(), n.label().map(str::to_owned), n.live()));
        (listed, node)
    };
    assert_eq!(shown(false), (false, None));
    let (listed, node) = shown(true);
    assert!(listed);
    assert_eq!(
        node,
        Some((
            accesskit::Role::Status,
            Some(CHECKING_TEXT.to_owned()),
            Some(accesskit::Live::Polite)
        ))
    );
    assert_eq!(
        window_title_with_check(None, true, "Photo"),
        "Checking PSD compatibility\u{2026} \u{2014} Aurora"
    );
    assert_eq!(
        window_title_with_check(None, false, "Photo"),
        "Photo \u{2014} Aurora"
    );

    // Driven through a real worker: pending shows it, every outcome clears.
    let mut worker = PsdCheckWorker::default();
    assert!(!worker.is_checking());
    let (release, woken, _) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    assert!(worker.is_checking(), "appears");
    let _ = release.send(());
    wait(&woken);
    assert!(worker.take_finished().is_some());
    assert!(!worker.is_checking(), "cleared by the result");

    let deadline = std::time::Instant::now() + PATIENCE;
    while worker.is_busy() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (release, woken, _) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    assert!(worker.is_checking());
    assert!(worker.cancel());
    assert!(!worker.is_checking(), "cleared by a cancel");
    let _ = release.send(());
    wait(&woken);
}

#[test]
fn the_app_shows_the_indicator_through_its_title_and_status_node() {
    let lib = include_str!("lib.rs");
    assert!(lib.contains("self.psd_check_worker.is_checking()"));
    assert!(lib.contains("background_psd_check::window_title_with_check("));
    assert!(lib.contains("background_psd_check::with_check_status("));
    let start = lib
        .find("fn check_psd_compatibility(&mut self)")
        .unwrap_or(0);
    let end = start + lib[start..].find("fn show_check_state").unwrap_or(0);
    let body = &lib[start..end];
    assert!(
        body.contains("self.show_check_state()"),
        "indicator on start"
    );
    assert!(body.contains("self.psd_check_worker.start(job, wake)"));
    assert!(body.contains("BUSY_NOTICE"), "calm refusal");
}

// ---- document switch, modal, shutdown ----

struct Host {
    worker: PsdCheckWorker,
    active: DocumentId,
    revision: crate::UndoRevision,
    modal: bool,
    shown: Vec<(FinishedCheck, bool)>,
    states: usize,
    cancelled: usize,
}

impl Host {
    fn new(active: DocumentId) -> Self {
        Self {
            worker: PsdCheckWorker::default(),
            active,
            revision: crate::UndoRevision(1),
            modal: false,
            shown: Vec::new(),
            states: 0,
            cancelled: 0,
        }
    }
}

impl CheckHost for Host {
    fn check_worker(&mut self) -> &mut PsdCheckWorker {
        &mut self.worker
    }
    fn active_document(&self) -> (DocumentId, crate::UndoRevision) {
        (self.active, self.revision)
    }
    fn modal_open(&self) -> bool {
        self.modal
    }
    fn show_check_state(&mut self) {
        self.states += 1;
    }
    fn check_cancelled(&mut self) {
        self.cancelled += 1;
    }
    fn show_check_result(&mut self, finished: FinishedCheck, edited: bool) {
        self.shown.push((finished, edited));
    }
}

#[test]
fn a_finished_check_is_shown_for_its_own_document_and_flags_an_edit() {
    let doc = DocumentId::next();
    let mut host = Host::new(doc);
    let (release, woken, _) = gated_start(&mut host.worker, meta(doc, 1));
    assert_eq!(background_check_step(&mut host), CheckStep::Idle);
    host.revision = crate::UndoRevision(2); // edited while checking
    let _ = release.send(());
    wait(&woken);
    assert_eq!(background_check_step(&mut host), CheckStep::Finished);
    assert_eq!(host.shown.len(), 1);
    assert!(
        host.shown.first().is_some_and(|(_, edited)| *edited),
        "edited"
    );
    assert_eq!(host.states, 1, "the indicator is refreshed to cleared");
    assert!(!host.worker.is_checking());
}

#[test]
fn a_result_is_dropped_when_the_document_was_switched_away() {
    let doc = DocumentId::next();
    let mut host = Host::new(doc);
    let (release, woken, seen_cancel) = gated_start(&mut host.worker, meta(doc, 1));
    host.active = DocumentId::next(); // a tab switch or a close
    assert_eq!(background_check_step(&mut host), CheckStep::Cancelled);
    assert_eq!(host.cancelled, 1);
    assert!(!host.worker.is_checking(), "indicator cleared");
    let _ = release.send(());
    wait(&woken);
    assert!(
        seen_cancel.load(Ordering::SeqCst),
        "the job was told to stop"
    );
    // Back on the original document: the dropped result never shows.
    host.active = doc;
    assert_eq!(background_check_step(&mut host), CheckStep::Idle);
    assert!(host.shown.is_empty());
}

#[test]
fn a_finished_check_waits_while_a_dialog_is_open() {
    let doc = DocumentId::next();
    let mut host = Host::new(doc);
    host.modal = true;
    let (release, woken, _) = gated_start(&mut host.worker, meta(doc, 1));
    let _ = release.send(());
    wait(&woken);
    assert_eq!(background_check_step(&mut host), CheckStep::Deferred);
    assert!(host.shown.is_empty());
    assert!(host.worker.is_checking(), "still indicated until shown");
    host.modal = false;
    assert_eq!(background_check_step(&mut host), CheckStep::Finished);
    assert_eq!(host.shown.len(), 1);
}

#[test]
fn a_check_under_a_dialog_asks_for_no_redraw_until_it_is_shown() {
    let doc = DocumentId::next();
    let mut host = Host::new(doc);
    host.modal = true;
    let (release, woken, _) = gated_start(&mut host.worker, meta(doc, 1));
    // Still running under the dialog: Idle, so the loop is not kept awake.
    for _ in 0..3 {
        let step = background_check_step(&mut host);
        assert_eq!(step, CheckStep::Idle);
        assert!(!step_needs_redraw(step));
    }
    // Finished but waiting for the dialog: nothing to draw yet either.
    let _ = release.send(());
    wait(&woken);
    for _ in 0..3 {
        let step = background_check_step(&mut host);
        assert_eq!(step, CheckStep::Deferred);
        assert!(!step_needs_redraw(step));
    }
    // The dialog closes: shown, and only now a frame.
    host.modal = false;
    let step = background_check_step(&mut host);
    assert_eq!(step, CheckStep::Finished);
    assert!(step_needs_redraw(step));
    assert!(step_needs_redraw(CheckStep::Cancelled));
    assert!(!step_needs_redraw(CheckStep::Idle));
}

#[test]
fn the_app_redraws_only_for_the_steps_that_change_the_screen() {
    let lib = include_str!("lib.rs");
    let start = lib.find("fn poll_psd_check(&mut self)").unwrap_or(0);
    let body = &lib[start..start + 400.min(lib.len() - start)];
    assert!(body.contains("step_needs_redraw(step)"), "{body}");
}

#[test]
fn the_snapshot_and_start_failures_have_their_own_calm_dialogs() {
    let (title, message) = dialog_text(
        "",
        &Err(CheckFailure::Snapshot(
            "a tile could not be read".to_owned(),
        )),
        false,
    );
    assert_eq!(title, crate::psd_save::PSD_CHECK_FAILED_TITLE);
    assert_eq!(
        message,
        "Aurora couldn't read the document to check it: a tile could not be read. No file was written."
    );
    let (title, message) = dialog_text("", &Err(CheckFailure::CouldNotStart), true);
    assert_eq!(title, crate::psd_save::PSD_CHECK_FAILED_TITLE);
    assert_eq!(
        message,
        "Aurora couldn't start the Photoshop compatibility check. No file was written."
    );
}

#[test]
fn the_result_is_on_the_channel_before_the_wake_runs() {
    let mut worker = PsdCheckWorker::default();
    let (tx, rx) = mpsc::channel();
    let _ = ok(worker.start_with(
        meta(DocumentId::next(), 1),
        |_| Ok(aurora_io::PsdWriteReport::default()),
        move || {
            let _ = tx.send(());
            // A slow wake: a result sent only after it would be missed.
            std::thread::sleep(Duration::from_millis(300));
        },
    ));
    wait(&rx);
    assert!(worker.take_finished().is_some(), "result before wake");
}

#[test]
fn a_cancelled_job_reports_cancelled_even_when_the_writer_fails_first() {
    let mut doc = Doc::new(8, 8, 16);
    // Flag already set: the fill stops at the first tile.
    let job = doc.job(Vec::new());
    let cancel = AtomicBool::new(true);
    assert!(matches!(job.run(&cancel), Err(CheckFailure::Cancelled)));
    // No tiles to fill, flag set: the sink fails the writer, and that
    // failure is still reported as a cancel, not as a broken write.
    let mut job = doc.job(Vec::new());
    job.tiles.clear();
    let cancel = AtomicBool::new(true);
    assert!(matches!(job.run(&cancel), Err(CheckFailure::Cancelled)));
    // Not set: the same job runs to a report.
    let job = doc.job(Vec::new());
    assert!(job.run(&AtomicBool::new(false)).is_ok());
}

#[test]
fn shutdown_with_a_check_in_flight_cancels_and_joins_without_hanging() {
    let mut worker = PsdCheckWorker::default();
    let (release, woken, seen_cancel) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    // The check finishes once cancelled and released.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        let _ = release.send(());
    });
    let started = std::time::Instant::now();
    let report = worker.shutdown(Duration::from_secs(10));
    assert_eq!(
        report,
        ShutdownReport {
            joined: 1,
            detached: 0
        }
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(seen_cancel.load(Ordering::SeqCst), "told to stop");
    assert!(!worker.is_checking() && !worker.is_busy());
    wait(&woken);
    let _ = releaser.join();
}

#[test]
fn shutdown_detaches_a_check_that_will_not_stop_within_the_bound() {
    let mut worker = PsdCheckWorker::default();
    let (release, woken, _) = gated_start(&mut worker, meta(DocumentId::next(), 1));
    let started = std::time::Instant::now();
    let report = worker.shutdown(Duration::from_millis(50));
    assert_eq!(report.detached, 1);
    assert!(started.elapsed() < Duration::from_secs(5), "bounded");
    let _ = release.send(());
    wait(&woken);
}

#[test]
fn the_app_stops_the_check_worker_before_the_scratch_directory_goes() {
    let lib = include_str!("lib.rs");
    let finish = lib.find("fn finish_shutdown(&mut self)").unwrap_or(0);
    let body = &lib[finish
        ..finish
            + lib[finish..]
                .find("fn palette_scrollbar_captured")
                .unwrap_or(0)];
    let stop = body.find(".psd_check_worker").unwrap_or(usize::MAX);
    let cleanup = body.find("run_shutdown_cleanup(self)").unwrap_or(0);
    assert!(stop < cleanup, "stop {stop} before cleanup {cleanup}");
    assert!(body.contains("background_psd_check::SHUTDOWN_JOIN_BOUND"));
}

#[test]
fn the_cancel_flag_stops_the_writer_at_its_next_write() {
    let mut doc = Doc::new(8, 8, 16);
    let cancel = AtomicBool::new(true);
    let result =
        crate::psd_save::check_psd_compatibility_cancellable(doc.source(Vec::new()), Some(&cancel));
    assert!(
        matches!(result, Err(PsdSaveFailure::Write(_))),
        "{result:?}"
    );
    // Not set: the same document checks fine.
    let cancel = AtomicBool::new(false);
    assert!(
        crate::psd_save::check_psd_compatibility_cancellable(doc.source(Vec::new()), Some(&cancel))
            .is_ok()
    );
}

// ---- corpus and measurement ----

/// Direct and worker reports are equal for corpus files (the usual
/// SKIPPED / `AURORA_REQUIRE_CORPUS` convention).
#[test]
fn corpus_files_give_the_same_report_from_the_worker_as_from_the_direct_check() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for name in [
        "effects/effects-enabled.psd",
        "blend-and-clipping.psd",
        "adjustment-mask.psd",
    ] {
        let path = root.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            crate::open_time_tests::corpus_file_absent(&path);
            continue;
        };
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(crate::TILE_BUDGET) else {
            unreachable!()
        };
        let mut store = ok(aurora_tile::TileStore::new(
            dir.path().to_path_buf(),
            budget,
        ));
        let document = ok(crate::open_psd_document(&bytes).map_err(|f| format!("{f:?}")));
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = crate::replace_document_pixels(
            &mut store,
            LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        assert_eq!(failed, 0);
        let saved: Vec<(&str, &aurora_doc::Selection)> = document
            .saved_selections
            .iter()
            .map(|(n, s)| (n.as_str(), s))
            .collect();
        let id = DocumentId::next();
        let revision = crate::UndoRevision(0);
        let direct = crate::psd_save::check_psd_compatibility(PsdSaveSource {
            saved: saved.clone(),
            layers: &document.layers,
            canvas: document.canvas_size,
            store: &mut store,
            doc_id: id,
            revision,
            carried: Vec::new(),
        });
        let job = ok(CheckJob::snapshot(
            CheckInputs {
                name: "f".to_owned(),
                doc_id: id,
                revision,
                layers: &document.layers,
                canvas: document.canvas_size,
                saved,
                carried: Vec::new(),
            },
            &mut store,
        ));
        let mut worker = PsdCheckWorker::default();
        let finished = run_on_worker(&mut worker, job);
        match (direct, finished.result) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{name}"),
            (Err(_), Err(CheckFailure::Failed(_))) => {}
            (a, b) => unreachable!("{name}: {a:?} vs {b:?}"),
        }
    }
}

fn nyepi() -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/flat-design-nyepi-template/6739968.psd");
    path.is_file().then_some(path)
}

/// UI-thread cost of starting a check on the Nyepi template (the
/// snapshot) and the whole check's wall time on the worker, three runs.
/// `cargo test -p aurora-app --release -- --ignored --nocapture nyepi_check_snapshot_cost`
#[test]
#[ignore = "measurement on the user's template"]
fn nyepi_check_snapshot_cost() {
    let Some(path) = nyepi() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(crate::TILE_BUDGET) else {
        unreachable!()
    };
    let mut store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    let document = ok(crate::PreparedPsd::stream(
        &path,
        Some(&store.staging_root()),
    ));
    let crate::PreparedPsd {
        layers,
        pixels,
        masks,
        canvas_size,
        ..
    } = document;
    let (_freed, failed) = crate::replace_document_pixels_prepared(
        &mut store,
        LayerTree::new(),
        aurora_doc::History::new(),
        &layers,
        pixels,
        masks,
    );
    assert_eq!(failed, 0);
    for run in 0..3 {
        let id = DocumentId::next();
        let revision = crate::UndoRevision(0);
        let started = std::time::Instant::now();
        let job = ok(CheckJob::snapshot(
            CheckInputs {
                name: "Nyepi".to_owned(),
                doc_id: id,
                revision,
                layers: &layers,
                canvas: canvas_size,
                saved: Vec::new(),
                carried: Vec::new(),
            },
            &mut store,
        ));
        let snapshot = started.elapsed();
        let tiles = job.tile_count();
        let mut worker = PsdCheckWorker::default();
        let started = std::time::Instant::now();
        let finished = run_on_worker(&mut worker, job);
        let total = started.elapsed();
        println!(
            "NYEPI check run {run}: UI-thread snapshot {snapshot:?} ({tiles} tiles); worker wall {total:?}; ok {}",
            finished.result.is_ok()
        );
        let direct_started = std::time::Instant::now();
        let direct = crate::psd_save::check_psd_compatibility(PsdSaveSource {
            saved: Vec::new(),
            layers: &layers,
            canvas: canvas_size,
            store: &mut store,
            doc_id: id,
            revision,
            carried: Vec::new(),
        });
        println!(
            "  direct (the old UI-thread cost): {:?}",
            direct_started.elapsed()
        );
        if let (Ok(a), Ok(b)) = (direct, finished.result) {
            assert_eq!(a, b, "worker report equals the direct one on Nyepi");
        }
    }
}
