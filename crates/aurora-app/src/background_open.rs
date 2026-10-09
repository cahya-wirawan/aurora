//! Opening a file off the UI thread (0.151.0, invariant §7.3.4).
//!
//! Until 0.151.0 `App::open_file` read and decoded the chosen file on the
//! UI thread, so a large PSD froze the window for as long as its decode
//! took (PLAN.md 0.150.0 measured hostile files at about 2 s). Now
//! [`OpenWorker::start`] runs the read and decode on its own
//! `std::thread` (the app has no async runtime, and this does not add
//! one) and hands back plain `Send` data ([`FinishedOpen`]); the UI
//! thread only *installs* it, because the live `aurora_tile::TileStore`
//! is owned by `App` and written only from the UI thread.
//!
//! The rules this module owns, each pinned by a test below:
//!
//! - **Newest wins.** A second open while one is still decoding
//!   supersedes it. The superseded thread cannot be interrupted
//!   mid-decode, so it runs to completion (detached) and its result is
//!   thrown away when it arrives: every open carries a [`Generation`],
//!   and [`OpenWorker::take_finished`] returns only the result whose
//!   generation is the one still pending.
//! - **At most one superseded decode stays in flight** beside the
//!   pending one ([`MAX_SUPERSEDED`], 0.151.0 review D1): a big PSD's
//!   decode holds the whole file and document in memory, so a third open
//!   while two decodes still run is refused with
//!   [`BackgroundFailure::Busy`], which the app shows as a refused open.
//! - **Never under a modal** (0.151.0 review E1): [`background_open_step`]
//!   leaves a finished open in the worker while a dialog is open, and
//!   installs it on the first loop iteration after the dialog closes —
//!   whichever path closed it, since every one of them is an event the
//!   loop follows with `about_to_wait`. Installing under the dialog would
//!   swap the document beneath an alert that may concern the old one, and
//!   the install's own report ("Opened With Changes", "Couldn't Open
//!   File") would find the one modal slot taken and be lost.
//! - **A panic is a failed open, not a crash** — in builds that unwind.
//!   The decode runs under `catch_unwind` and a panic comes back as
//!   [`OpenFailure::Background`]. The workspace's release profile is
//!   `panic = "abort"`, under which no panic can be caught at all: there
//!   a decode-thread panic still ends the process, and the protection is
//!   the decoders' own lints instead — `aurora-io`, like every crate,
//!   denies `panic`, `unwrap`, `expect` and `indexing_slicing`, so a
//!   panic there needs an overflow or a library panic rather than an
//!   ordinary bug. Allocation failure (out of memory) aborts the process
//!   in **every** profile; nothing here can catch it.
//! - **Result before wake.** The thread sends its result on the channel
//!   *before* it calls `wake`, so whoever the wake reaches (the event
//!   loop, through an `EventLoopProxy` user event) always finds it.
//! - **Quitting never hangs.** [`OpenWorker::shutdown`] joins every
//!   decode thread that finishes within a bound and detaches the rest.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::OpenFailure;

/// Which open a result belongs to. Strictly increasing per
/// [`OpenWorker`]; the first open is generation `1`.
pub(crate) type Generation = u64;

/// How long quitting waits for a decode still running before detaching
/// it. Short on purpose: the decode's result is going to be thrown away,
/// so waiting buys nothing but a tidy join.
pub(crate) const SHUTDOWN_JOIN_BOUND: Duration = Duration::from_millis(100);

/// How many superseded decodes may still be running beside the pending
/// one before a new open is refused ([`BackgroundFailure::Busy`]).
pub(crate) const MAX_SUPERSEDED: usize = 1;

/// A decoded file, ready for the UI thread to install. Plain `Send`
/// data: no tile-store handle, nothing that needs the UI thread.
#[derive(Debug)]
pub(crate) enum DecodedFile {
    /// A flat PNG/JPEG/TIFF image, its tiles already encoded on the
    /// decode thread (0.153.0).
    Image(crate::PreparedImage),
    /// A whole PSD/PSB document with its import report, its layers' and
    /// masks' tiles already encoded on the decode thread (0.153.0).
    Psd(crate::PreparedPsd),
    /// A `.aur` document's bytes, **already read once into a throwaway
    /// store** on the decode thread (`crate::precheck_aur`, 0.143.1's
    /// safety rule). The UI thread reads them into the live store; the
    /// live store is never touched by bytes that failed that check.
    Aur(Vec<u8>),
}

/// One decode thread's result.
#[derive(Debug)]
pub(crate) struct FinishedOpen {
    pub(crate) generation: Generation,
    pub(crate) path: PathBuf,
    pub(crate) result: Result<DecodedFile, OpenFailure>,
    /// How long the read and decode took on the background thread.
    pub(crate) decode_time: Duration,
}

/// Why the background machinery itself, rather than the file, failed.
#[derive(Debug)]
pub(crate) enum BackgroundFailure {
    /// The decode thread could not be started.
    Spawn(std::io::Error),
    /// The decode panicked; the payload's message, when it had one.
    Panicked(String),
    /// Refused before starting: an open is pending and
    /// [`MAX_SUPERSEDED`] superseded decodes are still running.
    Busy,
}

/// The open that is still decoding.
#[derive(Debug)]
struct PendingOpen {
    generation: Generation,
    path: PathBuf,
    handle: JoinHandle<()>,
}

/// What [`OpenWorker::shutdown`] did with the threads it found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShutdownReport {
    /// Threads that finished within the bound and were joined.
    pub(crate) joined: usize,
    /// Threads still running at the bound, left to the process exit.
    pub(crate) detached: usize,
}

/// Runs file decodes off the calling thread; see the module docs.
#[derive(Debug)]
pub(crate) struct OpenWorker {
    last_generation: Generation,
    pending: Option<PendingOpen>,
    /// Superseded decodes still running (or finished and not yet
    /// reaped). Their results are stale by construction.
    superseded: Vec<JoinHandle<()>>,
    sender: mpsc::Sender<FinishedOpen>,
    receiver: mpsc::Receiver<FinishedOpen>,
}

impl Default for OpenWorker {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            last_generation: 0,
            pending: None,
            superseded: Vec::new(),
            sender,
            receiver,
        }
    }
}

/// A panic payload's message: the `&str` or `String` `panic!` carries,
/// or a fixed sentence for anything else.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "no message".to_owned()
    }
}

impl OpenWorker {
    /// Starts reading and decoding `path` with `decode` on a new thread,
    /// superseding any open still pending (newest wins). `wake` runs on
    /// that thread once the result is on the channel — in the app, an
    /// `EventLoopProxy` user event, so the loop wakes and installs it.
    ///
    /// # Errors
    ///
    /// [`OpenFailure::Background`] with [`BackgroundFailure::Busy`] if an
    /// open is pending and [`MAX_SUPERSEDED`] superseded decodes are still
    /// running, or with [`BackgroundFailure::Spawn`] if the thread could
    /// not be started. Nothing new is pending then, and an open that was
    /// pending before this call is still pending.
    pub(crate) fn start<D, W>(
        &mut self,
        path: PathBuf,
        decode: D,
        wake: W,
    ) -> Result<Generation, OpenFailure>
    where
        D: FnOnce(&Path) -> Result<DecodedFile, OpenFailure> + Send + 'static,
        W: FnOnce() + Send + 'static,
    {
        self.reap_superseded();
        if self.pending.is_some() && self.superseded.len() >= MAX_SUPERSEDED {
            tracing::warn!(
                path = %path.display(),
                running = self.superseded.len() + 1,
                "refused an open: earlier decodes are still running"
            );
            return Err(OpenFailure::Background(BackgroundFailure::Busy));
        }
        let generation = self.last_generation.saturating_add(1);
        let sender = self.sender.clone();
        let thread_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("aurora-open-{generation}"))
            .spawn(move || {
                let started = Instant::now();
                let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    decode(&thread_path)
                })) {
                    Ok(result) => result,
                    Err(payload) => {
                        let message = panic_message(payload.as_ref());
                        tracing::error!(path = %thread_path.display(), %message, "a file decode panicked");
                        Err(OpenFailure::Background(BackgroundFailure::Panicked(
                            message,
                        )))
                    }
                };
                let finished = FinishedOpen {
                    generation,
                    path: thread_path,
                    result,
                    decode_time: started.elapsed(),
                };
                // A closed channel means the app is quitting and dropped
                // the worker: the result has nobody left to go to.
                let _ = sender.send(finished);
                // After the send, never before: see the module docs.
                wake();
            });
        let handle = match spawned {
            Ok(handle) => handle,
            Err(err) => {
                tracing::error!(%err, "failed to start a file decode thread");
                return Err(OpenFailure::Background(BackgroundFailure::Spawn(err)));
            }
        };
        self.last_generation = generation;
        if let Some(previous) = self.pending.replace(PendingOpen {
            generation,
            path,
            handle,
        }) {
            tracing::info!(
                superseded = previous.generation,
                by = generation,
                path = %previous.path.display(),
                "a newer open superseded one still decoding"
            );
            self.superseded.push(previous.handle);
        }
        self.reap_superseded();
        Ok(generation)
    }

    /// The file still decoding, if any — what the "Opening …" state
    /// shows.
    #[must_use]
    pub(crate) fn pending_path(&self) -> Option<&Path> {
        self.pending.as_ref().map(|pending| pending.path.as_path())
    }

    /// The generation still pending, if any.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn pending_generation(&self) -> Option<Generation> {
        self.pending.as_ref().map(|pending| pending.generation)
    }

    /// Drains every result that has arrived and returns the pending
    /// open's, clearing the pending state whether it succeeded or
    /// failed. A result from a superseded open is stale and is dropped
    /// here, never returned — so it can never install over a newer one.
    pub(crate) fn take_finished(&mut self) -> Option<FinishedOpen> {
        let mut current = None;
        while let Ok(finished) = self.receiver.try_recv() {
            let is_pending = self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.generation == finished.generation);
            if is_pending {
                if let Some(pending) = self.pending.take() {
                    // Finished except for its `wake` call: reaped later
                    // rather than joined here, so the UI thread never
                    // waits on it.
                    self.superseded.push(pending.handle);
                }
                current = Some(finished);
            } else {
                tracing::info!(
                    generation = finished.generation,
                    path = %finished.path.display(),
                    "dropped a superseded open's result"
                );
            }
        }
        self.reap_superseded();
        current
    }

    /// Joins every finished thread among the superseded ones.
    fn reap_superseded(&mut self) {
        let (finished, running): (Vec<_>, Vec<_>) = std::mem::take(&mut self.superseded)
            .into_iter()
            .partition(JoinHandle::is_finished);
        self.superseded = running;
        for handle in finished {
            // The closure catches its own panics, so a join error here
            // would be a panic in `send`/`wake` themselves: log only.
            if handle.join().is_err() {
                tracing::warn!("a finished file decode thread panicked outside its decode");
            }
        }
    }

    /// For quitting: forgets the pending open, joins every decode
    /// thread that finishes within `bound`, and detaches the rest (the
    /// process exit ends them). Never blocks for much longer than
    /// `bound`, whatever the threads are doing.
    pub(crate) fn shutdown(&mut self, bound: Duration) -> ShutdownReport {
        let mut handles = std::mem::take(&mut self.superseded);
        if let Some(pending) = self.pending.take() {
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

/// What one [`background_open_step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenStep {
    /// Nothing pending, or the pending open has not finished yet.
    Idle,
    /// An open is pending but a modal dialog is up: nothing was taken.
    Deferred,
    /// A finished open was taken and handed to the installer.
    Installed,
}

/// The side of the app [`background_open_step`] drives — `App` in
/// production, a recording stand-in in tests (the same seam
/// `ShutdownState` gives shutdown).
pub(crate) trait OpenInstaller {
    /// The worker whose results this installs.
    fn open_worker(&mut self) -> &mut OpenWorker;
    /// Whether a modal dialog is open.
    fn modal_open(&self) -> bool;
    /// Mirrors the worker's (now cleared) pending state into the UI.
    fn show_open_state(&mut self);
    /// Installs `finished` — or reports its failure.
    fn install(&mut self, finished: FinishedOpen);
}

/// The event loop's per-iteration open step (`about_to_wait`, 0.151.0):
/// while a modal dialog is up, leaves everything in the worker
/// ([`OpenStep::Deferred`]); otherwise takes the pending open's result if
/// it has arrived, clears the "Opening …" state, and installs it.
pub(crate) fn background_open_step(app: &mut impl OpenInstaller) -> OpenStep {
    if app.modal_open() {
        return if app.open_worker().pending_path().is_some() {
            OpenStep::Deferred
        } else {
            OpenStep::Idle
        };
    }
    let Some(finished) = app.open_worker().take_finished() else {
        return OpenStep::Idle;
    };
    app.show_open_state();
    app.install(finished);
    OpenStep::Installed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Long enough for any real thread to have run on a loaded CI box.
    const PATIENCE: Duration = Duration::from_secs(10);

    fn tiny_image() -> aurora_io::Image {
        let texels = vec![half::f16::from_f32(0.5); 4];
        match aurora_io::Image::new(1, 1, aurora_color::IccProfile::srgb(), texels) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// A wake that reports on a channel, so a test can wait for it.
    fn waker() -> (mpsc::Receiver<()>, impl FnOnce() + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        (rx, move || {
            let _ = tx.send(());
        })
    }

    /// A decode that blocks until the returned sender fires (or is
    /// dropped, or [`PATIENCE`] runs out — so a decode wrongly run on the
    /// calling thread fails these tests instead of hanging them), then
    /// succeeds with a 1×1 image.
    fn gated_decode() -> (
        mpsc::Sender<()>,
        impl FnOnce(&Path) -> Result<DecodedFile, OpenFailure> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel::<()>();
        (tx, move |_: &Path| {
            let _ = rx.recv_timeout(PATIENCE);
            Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image())))
        })
    }

    fn wait(woken: &mpsc::Receiver<()>) {
        assert!(
            woken.recv_timeout(PATIENCE).is_ok(),
            "the decode thread never woke the caller"
        );
    }

    #[test]
    fn a_started_open_decodes_off_the_calling_thread_and_wakes_once_its_result_is_ready() {
        let caller = std::thread::current().id();
        let decoded_on = Arc::new(std::sync::Mutex::new(None));
        let seen = Arc::clone(&decoded_on);
        let mut worker = OpenWorker::default();
        let (woken, wake) = waker();
        let generation = match worker.start(
            PathBuf::from("big.psd"),
            move |path| {
                if let Ok(mut slot) = seen.lock() {
                    *slot = Some((std::thread::current().id(), path.to_path_buf()));
                }
                Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image())))
            },
            wake,
        ) {
            Ok(generation) => generation,
            Err(failure) => unreachable!("{failure:?}"),
        };
        assert_eq!(generation, 1);
        assert_eq!(worker.pending_path(), Some(Path::new("big.psd")));
        wait(&woken);

        let Some(finished) = worker.take_finished() else {
            unreachable!("the woken result must be there to take");
        };
        assert_eq!(finished.generation, 1);
        assert_eq!(finished.path, PathBuf::from("big.psd"));
        assert!(matches!(finished.result, Ok(DecodedFile::Image(_))));
        let decoded_on = decoded_on
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some((thread, path)) = decoded_on else {
            unreachable!("the decode ran");
        };
        assert_ne!(thread, caller, "the decode must not run on the UI thread");
        assert_eq!(path, PathBuf::from("big.psd"));
        assert_eq!(worker.pending_path(), None, "installed: nothing is pending");
        assert!(worker.take_finished().is_none());
    }

    #[test]
    fn a_failed_decode_comes_back_as_its_failure_and_clears_the_pending_state() {
        let mut worker = OpenWorker::default();
        let (woken, wake) = waker();
        if let Err(failure) = worker.start(
            PathBuf::from("gone.png"),
            |_| {
                Err(OpenFailure::Read(std::io::Error::from(
                    std::io::ErrorKind::NotFound,
                )))
            },
            wake,
        ) {
            unreachable!("{failure:?}");
        }
        wait(&woken);
        let Some(finished) = worker.take_finished() else {
            unreachable!("a failure is a result too, and must be returned");
        };
        assert!(
            matches!(&finished.result, Err(OpenFailure::Read(err)) if err.kind() == std::io::ErrorKind::NotFound),
            "{:?}",
            finished.result
        );
        assert_eq!(
            worker.pending_path(),
            None,
            "a failed open must clear the Opening state too"
        );
    }

    #[test]
    #[allow(clippy::panic)] // the panic under test
    fn a_panicking_decode_is_reported_as_a_failed_open_and_does_not_take_the_caller_down() {
        let mut worker = OpenWorker::default();
        let (woken, wake) = waker();
        if let Err(failure) = worker.start(
            PathBuf::from("hostile.psd"),
            |_| panic!("decoder bug at offset 42"),
            wake,
        ) {
            unreachable!("{failure:?}");
        }
        wait(&woken);
        let Some(finished) = worker.take_finished() else {
            unreachable!("a panicked decode must still report");
        };
        match finished.result {
            Err(OpenFailure::Background(BackgroundFailure::Panicked(message))) => {
                assert_eq!(message, "decoder bug at offset 42");
            }
            other => unreachable!("expected a caught panic, got {other:?}"),
        }
        assert_eq!(worker.pending_path(), None);
    }

    #[test]
    fn a_second_open_supersedes_the_first_and_only_the_newest_result_is_returned() {
        let mut worker = OpenWorker::default();
        let (release_first, first) = gated_decode();
        let (first_woken, first_wake) = waker();
        let (second_woken, second_wake) = waker();
        if let Err(failure) = worker.start(PathBuf::from("first.psd"), first, first_wake) {
            unreachable!("{failure:?}");
        }
        let second = match worker.start(
            PathBuf::from("second.png"),
            |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
            second_wake,
        ) {
            Ok(generation) => generation,
            Err(failure) => unreachable!("a second open must start, not be refused: {failure:?}"),
        };
        assert_eq!(second, 2);
        assert_eq!(
            worker.pending_path(),
            Some(Path::new("second.png")),
            "the newest open is the one shown as pending"
        );
        wait(&second_woken);
        let Some(finished) = worker.take_finished() else {
            unreachable!("the newest open's result must be returned");
        };
        assert_eq!(finished.generation, 2);
        assert_eq!(finished.path, PathBuf::from("second.png"));

        // The superseded decode finishes late; its result must not
        // install over the newer document.
        let _ = release_first.send(());
        wait(&first_woken);
        assert!(
            worker.take_finished().is_none(),
            "a superseded open's late result must be dropped"
        );
        assert_eq!(worker.pending_path(), None);
    }

    #[test]
    fn a_stale_generation_is_ignored_even_when_it_arrives_before_the_pending_one() {
        let mut worker = OpenWorker::default();
        let (release_first, first) = gated_decode();
        let (release_second, second) = gated_decode();
        let (first_woken, first_wake) = waker();
        let (second_woken, second_wake) = waker();
        for (path, decode, wake) in [
            (
                "a.psd",
                Box::new(first) as Box<dyn FnOnce(&Path) -> _ + Send>,
                Box::new(first_wake) as Box<dyn FnOnce() + Send>,
            ),
            ("b.psd", Box::new(second), Box::new(second_wake)),
        ] {
            if let Err(failure) = worker.start(PathBuf::from(path), decode, wake) {
                unreachable!("{failure:?}");
            }
        }
        let _ = release_first.send(());
        wait(&first_woken);
        assert!(
            worker.take_finished().is_none(),
            "generation 1 is stale once generation 2 started"
        );
        assert_eq!(
            worker.pending_generation(),
            Some(2),
            "a stale result must not clear the newer open's pending state"
        );
        assert_eq!(worker.pending_path(), Some(Path::new("b.psd")));

        let _ = release_second.send(());
        wait(&second_woken);
        let Some(finished) = worker.take_finished() else {
            unreachable!("generation 2 is the live one");
        };
        assert_eq!(finished.generation, 2);
    }

    #[test]
    fn shutting_down_with_a_decode_still_running_returns_within_its_bound() {
        let mut worker = OpenWorker::default();
        let (release, decode) = gated_decode();
        let (_woken, wake) = waker();
        if let Err(failure) = worker.start(PathBuf::from("huge.psb"), decode, wake) {
            unreachable!("{failure:?}");
        }
        // On another thread, so an unbounded join fails this test
        // instead of hanging the suite.
        let (done_tx, done_rx) = mpsc::channel();
        let quitting = std::thread::spawn(move || {
            let started = Instant::now();
            let report = worker.shutdown(Duration::from_millis(50));
            let _ = done_tx.send((report, started.elapsed()));
            worker
        });
        let Ok((report, took)) = done_rx.recv_timeout(Duration::from_secs(5)) else {
            let _ = release.send(());
            unreachable!("quitting while a decode runs must not hang");
        };
        assert_eq!(
            report,
            ShutdownReport {
                joined: 0,
                detached: 1
            }
        );
        assert!(took < Duration::from_secs(2), "{took:?}");
        let _ = release.send(());
        let Ok(worker) = quitting.join() else {
            unreachable!("shutdown must not panic");
        };
        assert_eq!(worker.pending_path(), None);
    }

    #[test]
    fn shutting_down_after_a_decode_finished_joins_its_thread() {
        let mut worker = OpenWorker::default();
        let (woken, wake) = waker();
        if let Err(failure) = worker.start(
            PathBuf::from("small.png"),
            |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
            wake,
        ) {
            unreachable!("{failure:?}");
        }
        wait(&woken);
        let report = worker.shutdown(PATIENCE);
        assert_eq!(
            report,
            ShutdownReport {
                joined: 1,
                detached: 0
            }
        );
        assert_eq!(worker.pending_path(), None);
        assert!(
            worker.take_finished().is_none(),
            "a result left over at quit is not installed"
        );
    }

    #[test]
    fn a_third_open_while_two_decodes_still_run_is_refused_as_busy() {
        let mut worker = OpenWorker::default();
        let (release_first, first) = gated_decode();
        let (release_second, second) = gated_decode();
        let (_first_woken, first_wake) = waker();
        let (second_woken, second_wake) = waker();
        if let Err(failure) = worker.start(PathBuf::from("a.psd"), first, first_wake) {
            unreachable!("{failure:?}");
        }
        if let Err(failure) = worker.start(PathBuf::from("b.psd"), second, second_wake) {
            unreachable!("one superseded decode is within the cap: {failure:?}");
        }
        let (third_woken, third_wake) = waker();
        match worker.start(
            PathBuf::from("c.psd"),
            |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
            third_wake,
        ) {
            Err(OpenFailure::Background(BackgroundFailure::Busy)) => {}
            other => unreachable!("expected a busy refusal, got {other:?}"),
        }
        assert_eq!(
            worker.pending_path(),
            Some(Path::new("b.psd")),
            "a refused open leaves the pending one pending"
        );
        assert!(
            third_woken
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "a refused open starts no thread"
        );

        // Once the superseded decode has finished, a new open is accepted.
        let _ = release_first.send(());
        let _ = release_second.send(());
        wait(&second_woken);
        let _ = worker.take_finished();
        let _ = worker.shutdown(PATIENCE);
        let (fourth_woken, fourth_wake) = waker();
        if let Err(failure) = worker.start(
            PathBuf::from("d.psd"),
            |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
            fourth_wake,
        ) {
            unreachable!("nothing is running any more: {failure:?}");
        }
        wait(&fourth_woken);
    }

    /// A recording [`OpenInstaller`].
    #[derive(Default)]
    struct Recorder {
        worker: OpenWorker,
        modal: bool,
        shown: usize,
        installed: Vec<Generation>,
    }

    impl OpenInstaller for Recorder {
        fn open_worker(&mut self) -> &mut OpenWorker {
            &mut self.worker
        }
        fn modal_open(&self) -> bool {
            self.modal
        }
        fn show_open_state(&mut self) {
            self.shown += 1;
        }
        fn install(&mut self, finished: FinishedOpen) {
            self.installed.push(finished.generation);
        }
    }

    #[test]
    fn the_open_step_defers_a_finished_open_while_a_modal_is_up_and_installs_it_after() {
        let mut app = Recorder::default();
        assert_eq!(background_open_step(&mut app), OpenStep::Idle);
        let (woken, wake) = waker();
        if let Err(failure) = app.worker.start(
            PathBuf::from("x.psd"),
            |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
            wake,
        ) {
            unreachable!("{failure:?}");
        }
        wait(&woken);
        app.modal = true;
        assert_eq!(background_open_step(&mut app), OpenStep::Deferred);
        assert_eq!(background_open_step(&mut app), OpenStep::Deferred);
        assert!(app.installed.is_empty(), "nothing installs under a modal");
        assert_eq!(app.worker.pending_path(), Some(Path::new("x.psd")));

        app.modal = false;
        assert_eq!(background_open_step(&mut app), OpenStep::Installed);
        assert_eq!(app.installed, vec![1]);
        assert_eq!(
            app.shown, 1,
            "the Opening state is cleared before the install"
        );
        assert_eq!(app.worker.pending_path(), None);
        assert_eq!(background_open_step(&mut app), OpenStep::Idle);
    }

    #[test]
    fn every_open_gets_a_strictly_newer_generation() {
        let mut worker = OpenWorker::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let mut generations = Vec::new();
        for _ in 0..3 {
            // Within the superseded-decode cap: each earlier decode is
            // given time to finish before the next open.
            let _ = worker.shutdown(PATIENCE);
            let wakes = Arc::clone(&wakes);
            match worker.start(
                PathBuf::from("x.png"),
                |_| Ok(DecodedFile::Image(crate::PreparedImage::new(tiny_image()))),
                move || {
                    wakes.fetch_add(1, Ordering::SeqCst);
                },
            ) {
                Ok(generation) => generations.push(generation),
                Err(failure) => unreachable!("{failure:?}"),
            }
        }
        assert_eq!(generations, vec![1, 2, 3]);
        let _ = worker.shutdown(PATIENCE);
        assert_eq!(wakes.load(Ordering::SeqCst), 3, "every thread wakes once");
    }
}
