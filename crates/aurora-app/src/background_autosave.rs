//! Writing the crash-recovery autosave off the UI thread (0.152.0,
//! invariant §7.3.4).
//!
//! Until 0.152.0 every autosave walked the live `aurora_tile::TileStore`,
//! encoded every tile and wrote the whole `.aur` container on the UI
//! thread: about 1.9 s of the 2.6 s it took to install a 4096x4096
//! four-layer PSD (PLAN.md 0.151.0, review A1). Now the UI thread only
//! takes an [`aurora_io::AurSnapshot`] (`crate::snapshot_autosave`, a
//! copy of every tile the document holds), and this module's one worker
//! thread encodes and writes it, to a temp file that is then renamed
//! over the autosave. The file, its format and crash recovery are
//! exactly what they were; only the thread that writes it changed.
//!
//! The rules this module owns, each pinned by a test below:
//!
//! - **One writer, newest wins.** There is one worker thread and one
//!   queue slot. A request made while a write is in flight waits in the
//!   slot, and a newer request replaces a waiting one (it is dropped,
//!   never written). So at most one write is in flight and writes land
//!   in request order.
//! - **A stale write never lands.** Every request carries a
//!   [`Generation`], and the rename that publishes a write happens under
//!   the state lock and only if no newer generation has landed
//!   ([`land`]). Among worker writes the single-writer rule already
//!   guarantees that; the check is what keeps a worker write from
//!   landing over an autosave written synchronously on the UI thread
//!   (the over-budget fallback), which first calls
//!   [`AutosaveWorker::supersede`] (0.152.0 review R1). So the file
//!   always ends up holding the newest *request*, worker or synchronous;
//!   a superseded worker write is refused at its rename.
//! - **The snapshot is the document.** A job owns its snapshot, taken on
//!   the UI thread in one call that the UI thread does not interleave
//!   with any edit. Nothing on the worker reads the tile store, so an
//!   edit, a second open, or the store being dropped after the snapshot
//!   cannot reach the file. A job belongs to the document it was taken
//!   from: a second open simply submits a newer job.
//! - **Quitting abandons the autosave.** A clean quit deletes the
//!   autosave anyway (`clean_shutdown_cleanup`), so
//!   [`AutosaveWorker::shutdown`] drops a waiting job, cancels the write
//!   in flight (its writer fails at its next write call), and waits at
//!   most [`SHUTDOWN_WAIT_BOUND`] for the thread before detaching it.
//!   Cancelling happens under the state lock, and [`land`] checks it
//!   under that lock, so once `shutdown` returns no rename can happen;
//!   the cleanup that deletes the autosave after it cannot be undone by
//!   a late write. [`land`] also refuses once the process-wide
//!   `SESSION_ENDING` flag (0.151.0) is set.
//! - **A panic is a failed autosave, not a crash** — in builds that
//!   unwind. The write runs under `catch_unwind`; a panic is logged and
//!   the worker goes on serving later requests. The release profile is
//!   `panic = "abort"`, where nothing can be caught (the same limit
//!   `background_open` documents). A failed autosave is logged and
//!   leaves the previous autosave in place, which is what a failed
//!   UI-thread autosave did before; neither is shown to the user.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Which autosave request a write belongs to. Strictly increasing per
/// [`AutosaveWorker`]; the first request is generation `1`.
pub(crate) type Generation = u64;

/// How long quitting waits for the worker after cancelling its write.
/// The write it cancels is about to be deleted, so this only buys a tidy
/// join; a cancelled writer stops at its next write call, well inside it.
pub(crate) const SHUTDOWN_WAIT_BOUND: Duration = Duration::from_millis(500);

/// One autosave to write: where it goes, and what.
#[derive(Debug)]
pub(crate) struct AutosaveJob {
    /// The canonical autosave path (`crate::autosave_path`).
    pub(crate) path: PathBuf,
    /// Where this write lands: `path`, or its partial sibling when the
    /// snapshot had to leave tiles out (`crate::write_autosave`'s rule).
    pub(crate) destination: PathBuf,
    pub(crate) snapshot: aurora_io::AurSnapshot,
}

/// A finished, synced temp file and where it must land: the job's
/// `destination`, or the partial path when the write itself found an
/// evicted tile it could not decode.
#[derive(Debug)]
pub(crate) struct WrittenTemp {
    pub(crate) temp: PathBuf,
    pub(crate) destination: PathBuf,
}

/// Writes a job to a fresh temp file, or returns `None` after logging
/// and removing the temp file. Production uses
/// `crate::write_autosave_temp`; tests substitute gates and failures.
pub(crate) type WriteTemp = dyn Fn(&AutosaveJob, &AtomicBool) -> Option<WrittenTemp> + Send + Sync;

/// What happened to one finished write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Landing {
    /// Renamed into place.
    Landed,
    /// A newer generation had already landed; the temp file was removed.
    Stale,
    /// The worker was shut down or the session is ending; removed.
    Cancelled,
    /// The rename itself failed; removed.
    RenameFailed,
}

#[derive(Debug, Default)]
struct State {
    queued: Option<(Generation, AutosaveJob)>,
    in_flight: Option<Generation>,
    landed: Generation,
    cancelled: bool,
    /// Every [`Landing`] in order, for tests and the shutdown report.
    landings: Vec<(Generation, Landing)>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Read by the cancellable writer on every write call.
    cancel: AtomicBool,
    session_ending: fn() -> bool,
    write: Box<WriteTemp>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means a panic elsewhere while it was held;
        // the state itself is always left consistent, so keep using it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What [`AutosaveWorker::shutdown`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AutosaveShutdown {
    /// A waiting job was dropped unwritten.
    pub(crate) dropped_queued: bool,
    /// A write was in flight when the shutdown began.
    pub(crate) was_writing: bool,
    /// The worker thread was joined within the bound.
    pub(crate) joined: bool,
}

/// The autosave writer; see the module docs.
#[derive(Debug)]
pub(crate) struct AutosaveWorker {
    shared: Arc<Shared>,
    last_generation: Generation,
    thread: Option<JoinHandle<()>>,
}

impl Default for AutosaveWorker {
    fn default() -> Self {
        Self::new(crate::write_autosave_temp, || {
            crate::SESSION_ENDING.load(Ordering::SeqCst)
        })
    }
}

impl AutosaveWorker {
    /// A worker that writes with `write` and refuses to land once
    /// `session_ending` returns `true`. No thread starts until the first
    /// [`Self::submit`].
    pub(crate) fn new(
        write: impl Fn(&AutosaveJob, &AtomicBool) -> Option<WrittenTemp> + Send + Sync + 'static,
        session_ending: fn() -> bool,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
                cancel: AtomicBool::new(false),
                session_ending,
                write: Box::new(write),
            }),
            last_generation: 0,
            thread: None,
        }
    }

    /// Queues `job` for writing and returns its generation, replacing
    /// (dropping) any job still waiting. `None` after
    /// [`Self::shutdown`], when the job is dropped. If the worker thread
    /// cannot be started, the job is written on the calling thread
    /// instead, so a crash after it still recovers.
    pub(crate) fn submit(&mut self, job: AutosaveJob) -> Option<Generation> {
        self.last_generation += 1;
        let generation = self.last_generation;
        {
            let mut state = self.shared.lock();
            if state.cancelled {
                tracing::info!("autosave requested after shutdown; dropped");
                return None;
            }
            if let Some((superseded, _)) = state.queued.replace((generation, job)) {
                tracing::debug!(
                    superseded,
                    generation,
                    "a newer autosave replaced a waiting one"
                );
            }
        }
        self.shared.changed.notify_all();
        if self.thread.is_none() {
            let shared = Arc::clone(&self.shared);
            match std::thread::Builder::new()
                .name("aurora-autosave".to_owned())
                .spawn(move || run(&shared))
            {
                Ok(handle) => self.thread = Some(handle),
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        "could not start the autosave thread; writing on this one"
                    );
                    let queued = self.shared.lock().queued.take();
                    if let Some((generation, job)) = queued {
                        write_and_land(&self.shared, generation, &job);
                    }
                }
            }
        }
        Some(generation)
    }

    /// Makes every job submitted so far unable to land, for a caller
    /// about to write the autosave itself on its own thread (0.152.0
    /// review R1: `crate::request_autosave`'s over-budget fallback).
    /// Under the state lock it drops the waiting job and records a new
    /// generation as already landed, so [`land`] refuses the write in
    /// flight as [`Landing::Stale`]. Taking the lock also waits out a
    /// rename that is already under way, so once this returns no older
    /// write can replace the caller's. The write in flight is not
    /// interrupted (its result is refused at the rename); a later
    /// [`Self::submit`] gets a newer generation and lands normally.
    pub(crate) fn supersede(&mut self) -> Generation {
        self.last_generation += 1;
        let generation = self.last_generation;
        let mut state = self.shared.lock();
        if let Some((dropped, _)) = state.queued.take() {
            tracing::debug!(
                dropped,
                generation,
                "a synchronous autosave superseded a waiting one"
            );
        }
        state.landed = state.landed.max(generation);
        generation
    }

    /// Waits up to `bound` for every submitted job to be written (or
    /// dropped); `true` when the worker is idle.
    #[cfg(test)]
    pub(crate) fn wait_idle(&self, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        let mut state = self.shared.lock();
        loop {
            if state.queued.is_none() && state.in_flight.is_none() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = match self
                .shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(now))
            {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// Every write that finished, in order, with what became of it.
    #[cfg(test)]
    pub(crate) fn landings(&self) -> Vec<(Generation, Landing)> {
        self.shared.lock().landings.clone()
    }

    /// Abandons the autosave for a clean quit: drops a waiting job,
    /// cancels the write in flight, and waits at most `bound` for the
    /// thread. Once this returns no write can land (see [`land`]).
    /// Idempotent.
    pub(crate) fn shutdown(&mut self, bound: Duration) -> AutosaveShutdown {
        let (dropped_queued, was_writing) = {
            let mut state = self.shared.lock();
            state.cancelled = true;
            self.shared.cancel.store(true, Ordering::SeqCst);
            (state.queued.take().is_some(), state.in_flight.is_some())
        };
        self.shared.changed.notify_all();
        let mut joined = false;
        if let Some(handle) = self.thread.take() {
            let deadline = Instant::now() + bound;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if handle.is_finished() {
                joined = handle.join().is_ok();
            } else {
                tracing::info!("quit with an autosave still writing; its thread was detached");
            }
        } else {
            joined = true;
        }
        AutosaveShutdown {
            dropped_queued,
            was_writing,
            joined,
        }
    }
}

/// The worker thread: takes the waiting job, writes it, lands it; until
/// shut down.
fn run(shared: &Shared) {
    loop {
        let (generation, job) = {
            let mut state = shared.lock();
            loop {
                if state.cancelled {
                    return;
                }
                if let Some(queued) = state.queued.take() {
                    state.in_flight = Some(queued.0);
                    break queued;
                }
                state = match shared.changed.wait(state) {
                    Ok(state) => state,
                    Err(poisoned) => poisoned.into_inner(),
                };
            }
        };
        write_and_land(shared, generation, &job);
        drop(job);
        shared.lock().in_flight = None;
        shared.changed.notify_all();
    }
}

/// Writes one job and lands it, catching a panic where the build
/// unwinds.
fn write_and_land(shared: &Shared, generation: Generation, job: &AutosaveJob) {
    let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (shared.write)(job, &shared.cancel)
    }));
    match written {
        Ok(Some(written)) => {
            let landing = land(shared, generation, &written, job);
            if landing != Landing::Landed {
                tracing::info!(generation, ?landing, "an autosave write did not land");
            }
        }
        Ok(None) => {}
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::warn!(generation, %message, "the autosave write panicked; the previous autosave is left in place");
        }
    }
}

/// Publishes a finished temp file, under the state lock: refused (and
/// the temp file removed) once cancelled or the session is ending, or
/// when a newer generation already landed.
fn land(
    shared: &Shared,
    generation: Generation,
    written: &WrittenTemp,
    job: &AutosaveJob,
) -> Landing {
    let mut state = shared.lock();
    let landing = if state.cancelled || (shared.session_ending)() {
        crate::remove_autosave_temp(&written.temp);
        Landing::Cancelled
    } else if generation <= state.landed {
        crate::remove_autosave_temp(&written.temp);
        Landing::Stale
    } else if crate::land_autosave_temp(&written.temp, &written.destination, &job.path) {
        state.landed = generation;
        Landing::Landed
    } else {
        Landing::RenameFailed
    };
    state.landings.push((generation, landing));
    landing
}

/// A writer that fails every call once `cancel` is set, so a cancelled
/// autosave stops at its next tile instead of finishing a file that is
/// about to be deleted. The error is `ErrorKind::Other`, never
/// `Interrupted`: `write_all` retries an `Interrupted` error forever.
pub(crate) struct CancellableWriter<'a, W> {
    pub(crate) inner: W,
    pub(crate) cancel: &'a AtomicBool,
}

impl<W> CancellableWriter<'_, W> {
    fn check(&self) -> std::io::Result<()> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(std::io::Error::other("autosave cancelled"))
        } else {
            Ok(())
        }
    }
}

impl<W: std::io::Write> std::io::Write for CancellableWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}

impl<W: std::io::Seek> std::io::Seek for CancellableWriter<'_, W> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.check()?;
        self.inner.seek(pos)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared by this module's tests and `lib.rs`'s.

    /// A `session_ending` that never ends, so tests never touch the
    /// process-wide flag.
    pub(crate) fn never() -> bool {
        false
    }

    /// One that always has.
    pub(crate) fn always() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    //! Each rule in the module docs, against real snapshots of a real
    //! tile store, real files and real crash recovery.

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use super::test_support::{always, never};
    use super::{AutosaveJob, AutosaveWorker, Landing, WrittenTemp, land};

    const WAIT: Duration = Duration::from_secs(30);
    const ORIGIN: aurora_tile::TileId = aurora_tile::TileId { x: 0, y: 0 };

    /// A document of one 10x10 pixel layer in its own store.
    struct Doc {
        _scratch: tempfile::TempDir,
        store: aurora_tile::TileStore,
        layers: aurora_doc::LayerTree,
        history: aurora_doc::History,
        layer: aurora_doc::LayerId,
    }

    fn store(budget: usize) -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(budget) else {
            unreachable!("budget is non-zero");
        };
        match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => (dir, store),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn rect(width: u32, height: u32) -> aurora_core::Rect {
        aurora_core::Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    impl Doc {
        fn new(name: &str, value: f32) -> Self {
            let (scratch, store) = store(16);
            let mut layers = aurora_doc::LayerTree::new();
            let mut history = aurora_doc::History::new();
            let layer = match history.add_pixel_layer(&mut layers, name, rect(10, 10), None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let mut doc = Self {
                _scratch: scratch,
                store,
                layers,
                history,
                layer,
            };
            doc.paint(value);
            doc
        }

        /// Adds a layer named `name` whose texel (0, 0) red is `value`;
        /// it becomes [`Self::layer`].
        fn add_layer(&mut self, name: &str, value: f32) {
            self.layer =
                match self
                    .history
                    .add_pixel_layer(&mut self.layers, name, rect(10, 10), None)
                {
                    Ok(id) => id,
                    Err(err) => unreachable!("{err:?}"),
                };
            self.paint(value);
        }

        fn surface(&self) -> aurora_tile::SurfaceId {
            match self.layers.surface_id(self.layer) {
                Some(surface) => surface,
                None => unreachable!("a pixel layer"),
            }
        }

        fn paint(&mut self, value: f32) {
            let surface = self.surface();
            let tile = match self.store.get_mut(surface, ORIGIN) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(first) = tile.texels_mut().first_mut() {
                *first = half::f16::from_f32(value);
            }
        }

        fn job(&mut self, path: &Path) -> AutosaveJob {
            match crate::snapshot_autosave(
                path,
                &self.layers,
                &self.history,
                (10, 10),
                &mut aurora_io::SkippedTiles::new(),
                crate::AUTOSAVE_SNAPSHOT_BUDGET_BYTES,
                &mut self.store,
            ) {
                crate::SnapshotOutcome::Taken(job) => job,
                other => unreachable!("{other:?}"),
            }
        }
    }

    /// What `path` recovers to: each root layer's name and its texel
    /// (0, 0) red.
    fn recovered(path: &Path) -> Option<Vec<(String, f32)>> {
        let (_dir, mut fresh) = store(16);
        let document = crate::recover_document(path, &mut fresh)?;
        let mut out = Vec::new();
        for &id in document.layers.roots() {
            let name = document.layers.name(id).unwrap_or_default().to_owned();
            let surface = document.layers.surface_id(id)?;
            let value = match fresh.get(surface, ORIGIN) {
                Ok(tile) => tile.texels().first().map_or(f32::NAN, |s| s.to_f32()),
                Err(err) => unreachable!("{err:?}"),
            };
            out.push((name, value));
        }
        Some(out)
    }

    fn temp_files(dir: &Path) -> Vec<PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
                .collect(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn tempdir() -> tempfile::TempDir {
        match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn real_worker() -> AutosaveWorker {
        AutosaveWorker::new(crate::write_autosave_temp, never)
    }

    /// A writer that, on its first call only, reports it started and
    /// then blocks until released; every call then writes for real.
    fn gated_writer() -> (
        impl Fn(&AutosaveJob, &AtomicBool) -> Option<WrittenTemp> + Send + Sync + 'static,
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
    ) {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let started_tx = Mutex::new(started_tx);
        let release_rx = Mutex::new(release_rx);
        let calls = AtomicUsize::new(0);
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                if let Ok(tx) = started_tx.lock() {
                    let _ = tx.send(());
                }
                if let Ok(rx) = release_rx.lock() {
                    let _ = rx.recv_timeout(WAIT);
                }
            }
            crate::write_autosave_temp(job, cancel)
        };
        (write, started, release)
    }

    #[test]
    fn a_background_autosave_recovers_to_the_snapshotted_document() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let mut doc = Doc::new("ink", 0.5);
        let mut worker = real_worker();
        let job = doc.job(&path);
        assert_eq!(worker.submit(job), Some(1));
        assert!(worker.wait_idle(WAIT));
        assert_eq!(worker.landings(), vec![(1, Landing::Landed)]);
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.5)]));
        assert!(temp_files(dir.path()).is_empty());
    }

    #[test]
    fn a_background_autosave_of_evicted_tiles_matches_the_streaming_write() {
        // Budget 1, four tiles: three are evicted, so the snapshot carries
        // their encoded bytes, never paging them in; the fourth is a
        // materialized blank, which neither writer stores.
        let dir = tempdir();
        let (_scratch, mut store) = store(1);
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let layer = match history.add_pixel_layer(&mut layers, "wide", rect(1024, 10), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("a pixel layer");
        };
        for (x, value) in [(0, 0.25_f32), (1, 0.5), (2, 0.75)] {
            let tile = match store.get_mut(surface, aurora_tile::TileId { x, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(first) = tile.texels_mut().first_mut() {
                *first = half::f16::from_f32(value);
            }
        }
        if let Err(err) = store.get_mut(surface, aurora_tile::TileId { x: 3, y: 0 }) {
            unreachable!("{err:?}");
        }
        let background = dir.path().join("background.aur");
        let streamed = dir.path().join("streamed.aur");
        let job = match crate::snapshot_autosave(
            &background,
            &layers,
            &history,
            (1024, 10),
            &mut aurora_io::SkippedTiles::new(),
            crate::AUTOSAVE_SNAPSHOT_BUDGET_BYTES,
            &mut store,
        ) {
            crate::SnapshotOutcome::Taken(job) => job,
            other => unreachable!("{other:?}"),
        };
        assert_eq!(
            store.stats().faults,
            0,
            "the snapshot must not page tiles in"
        );
        let mut worker = real_worker();
        assert_eq!(worker.submit(job), Some(1));
        assert!(worker.wait_idle(WAIT));
        crate::write_autosave(
            &streamed,
            &layers,
            &history,
            (1024, 10),
            &mut aurora_io::SkippedTiles::new(),
            &mut store,
        );
        let read = |path: &Path| -> Vec<(String, Vec<u8>)> {
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(err) => unreachable!("{err:?}"),
            };
            let mut zip = match zip::ZipArchive::new(file) {
                Ok(zip) => zip,
                Err(err) => unreachable!("{err:?}"),
            };
            (0..zip.len())
                .map(|i| {
                    let mut entry = match zip.by_index(i) {
                        Ok(entry) => entry,
                        Err(err) => unreachable!("{err:?}"),
                    };
                    let mut bytes = Vec::new();
                    if let Err(err) = std::io::Read::read_to_end(&mut entry, &mut bytes) {
                        unreachable!("{err:?}");
                    }
                    (entry.name().to_owned(), bytes)
                })
                .collect()
        };
        let background_entries = read(&background);
        assert_eq!(
            background_entries.len(),
            6,
            "mimetype, manifest, history, three tiles"
        );
        assert_eq!(background_entries, read(&streamed));
    }

    #[test]
    fn a_newer_request_replaces_a_waiting_one_and_lands_last() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (write, started, release) = gated_writer();
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.25);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        doc.paint(0.5);
        assert_eq!(worker.submit(doc.job(&path)), Some(2));
        doc.paint(0.75);
        assert_eq!(worker.submit(doc.job(&path)), Some(3));
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_eq!(
            worker.landings(),
            vec![(1, Landing::Landed), (3, Landing::Landed)],
            "generation 2 was waiting when 3 arrived, so it was never written"
        );
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.75)]));
    }

    #[test]
    fn a_stale_generation_never_lands_over_a_newer_one() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let worker = real_worker();
        let mut doc = Doc::new("ink", 0.25);
        let older = doc.job(&path);
        doc.paint(0.75);
        let newer = doc.job(&path);
        let cancel = AtomicBool::new(false);
        let Some(newer_temp) = crate::write_autosave_temp(&newer, &cancel) else {
            unreachable!("the write succeeds");
        };
        let Some(older_temp) = crate::write_autosave_temp(&older, &cancel) else {
            unreachable!("the write succeeds");
        };
        assert_eq!(
            land(&worker.shared, 2, &newer_temp, &newer),
            Landing::Landed
        );
        assert_eq!(land(&worker.shared, 1, &older_temp, &older), Landing::Stale);
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.75)]));
        assert!(
            !older_temp.temp.exists(),
            "a stale write's temp file is removed"
        );
    }

    #[test]
    fn an_edit_after_the_snapshot_is_not_in_the_written_file() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (write, started, release) = gated_writer();
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.25);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        // While the write is blocked: edit the tile, then throw the whole
        // store (and its scratch directory) away.
        doc.paint(0.75);
        drop(doc);
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.25)]));
    }

    #[test]
    fn opening_another_document_while_an_autosave_is_writing_is_safe() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let side = dir.path().join("first-landed.aur");
        let (write, started, release) = gated_writer();
        let side_copy = side.clone();
        let copies = AtomicUsize::new(0);
        // Keeps a copy of the first file that lands, to read it back
        // after the second has replaced it.
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| {
            let written = write(job, cancel)?;
            if copies.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = std::fs::copy(&written.temp, &side_copy);
            }
            Some(written)
        };
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("first", 0.25);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        // "Open" a second document into the same store: the first's
        // tiles are freed, a new layer is painted, and its autosave is
        // requested while the first is still writing.
        let first_surface = doc.surface();
        let _freed = doc.store.forget_surface(first_surface);
        doc.layers = aurora_doc::LayerTree::new();
        doc.history = aurora_doc::History::new();
        doc.add_layer("second", 0.75);
        assert_eq!(worker.submit(doc.job(&path)), Some(2));
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_eq!(
            worker.landings(),
            vec![(1, Landing::Landed), (2, Landing::Landed)]
        );
        assert_eq!(recovered(&side), Some(vec![("first".to_owned(), 0.25)]));
        assert_eq!(recovered(&path), Some(vec![("second".to_owned(), 0.75)]));
    }

    #[test]
    fn request_autosave_returns_before_the_file_is_written() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (write, started, release) = gated_writer();
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.5);
        crate::request_autosave(
            &mut worker,
            &path,
            &doc.layers,
            &doc.history,
            (10, 10),
            &mut aurora_io::SkippedTiles::new(),
            &mut doc.store,
        );
        assert!(started.recv_timeout(WAIT).is_ok(), "the worker is writing");
        assert!(
            !path.exists(),
            "the request returned before the write finished"
        );
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.5)]));
    }

    #[test]
    fn quitting_while_a_write_is_in_progress_cancels_it() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (started_tx, started) = mpsc::channel();
        let started_tx = Mutex::new(started_tx);
        // Starts, then waits for the shutdown's cancel before it writes
        // its first byte: a write caught mid-flight by the quit.
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| {
            if let Ok(tx) = started_tx.lock() {
                let _ = tx.send(());
            }
            let deadline = std::time::Instant::now() + WAIT;
            while !cancel.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            crate::write_autosave_temp(job, cancel)
        };
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.5);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        assert_eq!(worker.submit(doc.job(&path)), Some(2));
        let report = worker.shutdown(super::SHUTDOWN_WAIT_BOUND);
        assert!(report.was_writing);
        assert!(report.dropped_queued, "the waiting request is abandoned");
        assert!(report.joined, "a cancelled write stops within the bound");
        assert!(worker.landings().is_empty());
        assert!(!path.exists());
        assert!(
            temp_files(dir.path()).is_empty(),
            "the cancelled temp file is removed"
        );
        assert_eq!(
            worker.submit(doc.job(&path)),
            None,
            "no autosave after shutdown"
        );
        assert!(worker.shutdown(super::SHUTDOWN_WAIT_BOUND).joined);
    }

    #[test]
    fn a_write_that_finishes_after_quit_does_not_land() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (started_tx, started) = mpsc::channel();
        let started_tx = Mutex::new(started_tx);
        let shutdown_began = Arc::new(AtomicBool::new(false));
        let began = Arc::clone(&shutdown_began);
        // Finishes its temp file, then holds it until the quit has begun:
        // only the landing step can still stop it.
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| {
            let written = crate::write_autosave_temp(job, cancel);
            if let Ok(tx) = started_tx.lock() {
                let _ = tx.send(());
            }
            let deadline = std::time::Instant::now() + WAIT;
            while !began.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            std::thread::sleep(Duration::from_millis(20));
            written
        };
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.5);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        let quitting = {
            let began = Arc::clone(&shutdown_began);
            move |worker: &mut AutosaveWorker| {
                began.store(true, Ordering::SeqCst);
                worker.shutdown(super::SHUTDOWN_WAIT_BOUND)
            }
        };
        let report = quitting(&mut worker);
        assert!(report.joined);
        assert_eq!(worker.landings(), vec![(1, Landing::Cancelled)]);
        assert!(
            !path.exists(),
            "nothing lands after the quit's cleanup could have run"
        );
        assert!(temp_files(dir.path()).is_empty());
    }

    #[test]
    fn nothing_lands_once_the_session_is_ending() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let mut worker = AutosaveWorker::new(crate::write_autosave_temp, always);
        let mut doc = Doc::new("ink", 0.5);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(worker.wait_idle(WAIT));
        assert_eq!(worker.landings(), vec![(1, Landing::Cancelled)]);
        assert!(!path.exists());
        assert!(temp_files(dir.path()).is_empty());
    }

    #[test]
    fn a_failed_write_leaves_the_previous_autosave_in_place() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let mut doc = Doc::new("ink", 0.25);
        let mut worker = real_worker();
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(worker.wait_idle(WAIT));
        doc.paint(0.75);
        let job = doc.job(&path);
        let cancelled = AtomicBool::new(true);
        assert!(
            crate::write_autosave_temp(&job, &cancelled).is_none(),
            "a cancelled writer refuses its first write"
        );
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.25)]));
        assert!(temp_files(dir.path()).is_empty());
        // A directory that does not exist: the write fails, is logged,
        // and the worker goes on serving later requests.
        let missing = dir.path().join("missing").join("aurora-autosave.aur");
        assert_eq!(worker.submit(doc.job(&missing)), Some(2));
        assert_eq!(worker.submit(doc.job(&path)), Some(3));
        assert!(worker.wait_idle(WAIT));
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.75)]));
    }

    #[test]
    fn a_panicking_write_is_caught_and_the_worker_goes_on() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let calls = AtomicUsize::new(0);
        let write = move |job: &AutosaveJob, cancel: &AtomicBool| {
            assert!(
                calls.fetch_add(1, Ordering::SeqCst) != 0,
                "a deliberate test panic in the autosave writer"
            );
            crate::write_autosave_temp(job, cancel)
        };
        let mut worker = AutosaveWorker::new(write, never);
        let mut doc = Doc::new("ink", 0.25);
        assert_eq!(worker.submit(doc.job(&path)), Some(1));
        assert!(worker.wait_idle(WAIT));
        assert!(!path.exists());
        doc.paint(0.5);
        assert_eq!(worker.submit(doc.job(&path)), Some(2));
        assert!(worker.wait_idle(WAIT), "the worker survived the panic");
        assert_eq!(worker.landings(), vec![(2, Landing::Landed)]);
        assert_eq!(recovered(&path), Some(vec![("ink".to_owned(), 0.5)]));
    }

    #[test]
    fn a_document_past_the_snapshot_budget_is_not_snapshotted() {
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let mut doc = Doc::new("ink", 0.5);
        let outcome = crate::snapshot_autosave(
            &path,
            &doc.layers,
            &doc.history,
            (10, 10),
            &mut aurora_io::SkippedTiles::new(),
            1,
            &mut doc.store,
        );
        assert!(matches!(outcome, crate::SnapshotOutcome::OverBudget));
    }

    #[test]
    fn an_older_write_never_lands_over_a_synchronous_over_budget_autosave() {
        // 0.152.0 review R1: document A's write is in flight (and a second
        // A request is waiting) when document B, too large to snapshot,
        // is autosaved synchronously on the UI thread. A must not land
        // over B afterwards.
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (write, started, release) = gated_writer();
        let mut worker = AutosaveWorker::new(write, never);
        let mut first = Doc::new("first", 0.25);
        assert_eq!(worker.submit(first.job(&path)), Some(1));
        assert!(started.recv_timeout(WAIT).is_ok());
        assert_eq!(worker.submit(first.job(&path)), Some(2));
        let mut second = Doc::new("second", 0.75);
        crate::request_autosave_within(
            &mut worker,
            1,
            &path,
            &second.layers,
            &second.history,
            (10, 10),
            &mut aurora_io::SkippedTiles::new(),
            &mut second.store,
        );
        assert_eq!(recovered(&path), Some(vec![("second".to_owned(), 0.75)]));
        assert!(release.send(()).is_ok());
        assert!(worker.wait_idle(WAIT));
        assert_eq!(
            worker.landings(),
            vec![(1, Landing::Stale)],
            "the in-flight write is refused and the waiting one dropped"
        );
        assert_eq!(recovered(&path), Some(vec![("second".to_owned(), 0.75)]));
        assert!(temp_files(dir.path()).is_empty());
        // A later request is newer than the synchronous write and lands.
        assert_eq!(worker.submit(second.job(&path)), Some(4));
        assert!(worker.wait_idle(WAIT));
        assert_eq!(worker.landings().last(), Some(&(4, Landing::Landed)));
    }

    #[test]
    fn an_over_budget_document_copies_nothing_before_falling_back() {
        // 0.152.0 review R2: the budget is decided from the store's own
        // I/O-free bound before any tile is copied or read.
        let dir = tempdir();
        let path = dir.path().join("aurora-autosave.aur");
        let (_scratch, mut store) = store(1);
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let layer = match history.add_pixel_layer(&mut layers, "wide", rect(512, 10), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("a pixel layer");
        };
        for x in 0..2 {
            let tile = match store.get_mut(surface, aurora_tile::TileId { x, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            if let Some(first) = tile.texels_mut().first_mut() {
                *first = half::f16::from_f32(0.5);
            }
        }
        // Tile 0 is now on the scratch disk, not pending.
        if let Err(err) = store.flush() {
            unreachable!("{err:?}");
        }
        let read_before = store.stats().bytes_read;
        let outcome = crate::snapshot_autosave(
            &path,
            &layers,
            &history,
            (512, 10),
            &mut aurora_io::SkippedTiles::new(),
            // Room for the resident tile only.
            aurora_tile::SAMPLES * 2,
            &mut store,
        );
        assert!(matches!(outcome, crate::SnapshotOutcome::OverBudget));
        assert_eq!(
            store.stats().bytes_read,
            read_before,
            "an over-budget snapshot reads no scratch file"
        );
        // And with room for both, the same document is snapshotted.
        let outcome = crate::snapshot_autosave(
            &path,
            &layers,
            &history,
            (512, 10),
            &mut aurora_io::SkippedTiles::new(),
            aurora_tile::SAMPLES * 2 + aurora_tile::codec::MAX_ENCODED_LEN,
            &mut store,
        );
        assert!(matches!(outcome, crate::SnapshotOutcome::Taken(_)));
    }

    /// The UI-thread snapshot cost on **noisy** layers (0.152.0 review
    /// R2), where every evicted tile is incompressible, so each one costs
    /// a full ~512 KiB scratch-file read. Run with
    /// `cargo test -p aurora-app --lib -- --ignored --nocapture measure_autosave_snapshot_on_noisy`.
    #[test]
    #[ignore = "measurement, allocates over a gigabyte"]
    #[allow(clippy::print_stderr, clippy::cast_precision_loss)]
    fn measure_autosave_snapshot_on_noisy_layers() {
        const SIDE: u32 = 4096;
        for budget in [16, crate::TILE_BUDGET] {
            let dir = tempdir();
            let (_scratch, mut store) = store(budget);
            let mut layers = aurora_doc::LayerTree::new();
            let mut history = aurora_doc::History::new();
            let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
            for name in ["n0", "n1", "n2", "n3"] {
                let layer = match history.add_pixel_layer(&mut layers, name, rect(SIDE, SIDE), None)
                {
                    Ok(id) => id,
                    Err(err) => unreachable!("{err:?}"),
                };
                let Some(surface) = layers.surface_id(layer) else {
                    unreachable!("a pixel layer");
                };
                let tiles = SIDE / aurora_tile::TILE;
                for ty in 0..tiles {
                    for tx in 0..tiles {
                        let tile =
                            match store.get_mut(surface, aurora_tile::TileId { x: tx, y: ty }) {
                                Ok(tile) => tile,
                                Err(err) => unreachable!("{err:?}"),
                            };
                        for sample in tile.texels_mut() {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            // Random bits in [0, 1): incompressible.
                            *sample = half::f16::from_bits((seed & 0x3bff) as u16);
                        }
                    }
                }
            }
            if let Err(err) = store.flush() {
                unreachable!("{err:?}");
            }
            let path = dir.path().join("aurora-autosave.aur");
            let started = std::time::Instant::now();
            let job = match crate::snapshot_autosave(
                &path,
                &layers,
                &history,
                (SIDE, SIDE),
                &mut aurora_io::SkippedTiles::new(),
                crate::AUTOSAVE_SNAPSHOT_BUDGET_BYTES,
                &mut store,
            ) {
                crate::SnapshotOutcome::Taken(job) => job,
                other => unreachable!("{other:?}"),
            };
            let snapshot = started.elapsed();
            let snapshot_mib = job.snapshot.texel_bytes() as f64 / f64::from(1 << 20);
            let mut worker = real_worker();
            let started = std::time::Instant::now();
            assert_eq!(worker.submit(job), Some(1));
            assert!(worker.wait_idle(Duration::from_mins(20)));
            let write = started.elapsed();
            let sync_path = dir.path().join("sync.aur");
            let started = std::time::Instant::now();
            crate::write_autosave(
                &sync_path,
                &layers,
                &history,
                (SIDE, SIDE),
                &mut aurora_io::SkippedTiles::new(),
                &mut store,
            );
            let sync = started.elapsed();
            eprintln!(
                "noisy {SIDE}x{SIDE} x4 layers, store budget {budget} tiles: UI-thread snapshot \
                 {:.1} ms ({snapshot_mib:.0} MiB copied/read); worker write {:.1} ms; old \
                 synchronous autosave {:.1} ms",
                snapshot.as_secs_f64() * 1e3,
                write.as_secs_f64() * 1e3,
                sync.as_secs_f64() * 1e3,
            );
            let _shutdown = worker.shutdown(super::SHUTDOWN_WAIT_BOUND);
        }
    }
}
