//! The background compositor (0.186.0, invariant §7.3.4): a CPU-path
//! recomposite of the visible tiles runs on one worker thread instead of
//! inside the redraw handler, so a structural edit on a large document
//! no longer freezes the UI for the seconds its folds take.
//!
//! **Architecture — a mirrored store, fed by content-version deltas.** The
//! worker owns its own [`aurora_tile::TileStore`] (the *mirror*). Each job
//! the UI thread dispatches carries a clone of the `LayerTree`, the tiles
//! to composite, and only the source tiles whose
//! [`aurora_tile::TileStore::content_version`] changed since the last job
//! sent to this mirror ([`aurora_tile::TileStore::held_tile_versions`]
//! diffed against `sent`). A changed resident tile is shared, not copied
//! ([`aurora_tile::TileShare::Texels`] — the UI store's own `Arc`, copied
//! on the UI side's next write only if the mirror still holds it); an
//! evicted one goes as its encoded bytes. A structural edit changes no
//! pixels, so its job ships no tiles at all. The first job after a store
//! appears (open, document switch) ships every held tile — still only
//! reference counts for resident ones.
//!
//! The worker composites with the very same
//! [`crate::composite_roots_into_tile`] the synchronous path uses, against
//! the mirror, so a result is bit-identical to a synchronous composite of
//! the same document state.
//!
//! **In parallel (0.188.0, ADR 0012's addendum).** The worker owns a
//! rayon pool of [`composite_threads`] threads (`available_parallelism`
//! − 1, at least one, so the UI thread and the brush keep a core) and
//! folds a job's tiles on it, one tile per task. The mirror sits behind a
//! `Mutex` for the duration of a job and each task reads it through a
//! [`SharedMirror`]: a resident tile is a reference count taken under the
//! lock ([`aurora_tile::TileStore::peek_shared`]) and copied outside it;
//! only a tile that is not resident pages in under the lock. The fold path
//! reads every store through [`crate::TileSource`], so it is the same
//! code, and each tile's result is a pure function of the mirror's
//! content and the job's tree: bit-identical to the sequential fold. The
//! pool's threads share one drop-shadow coverage cache
//! ([`crate::SharedShadowCache`], one `small_machine::shadow_cache_bytes` cap for the
//! whole pool, keyed by the mirror's serial); Curves LUTs and scratch
//! buffers stay per thread (bounded, see the addendum).
//!
//! **Installing.** The UI drains results in `about_to_wait`
//! ([`CompositeWorker::drain`]); a result installs only if (1) its job is
//! still the active one (a superseded generation is dropped), (2) the
//! document it was dispatched for is still the current one (cache id and
//! store serial), (3) the composite grid's reference origin is unchanged,
//! and (4) [`crate::CompositeCache::unchanged_since`] says nothing
//! invalidated that tile after the job's stamp — a dab, undo or other edit
//! landing mid-job makes its result for that tile stale, and it is dropped
//! rather than installed over newer content.
//!
//! What stays synchronous is listed on [`SYNC_TILE_LIMIT`] and in PLAN.md's
//! 0.186.0 entry.

use std::borrow::BorrowMut;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use aurora_tile::{SurfaceId, TileId, TileShare, TileStore};
use rayon::prelude::*;

/// At most this many stale tiles are composited synchronously in the
/// redraw handler instead of on the worker — the brush's own case (a dab
/// dirties one to three composite tiles), where a thread hop and a
/// `LayerTree` clone would only add latency, and which is unchanged from
/// 0.185.0. More stale tiles than this (a structural edit, an open, a pan
/// revealing a new row of tiles) go to the worker.
pub(crate) const SYNC_TILE_LIMIT: usize = 3;

/// How many superseded jobs keep their results installable as
/// provisional tiles.
const SUPERSEDED_KEPT: usize = 4;

/// The generation a cancelled worker sees: never a real job's.
const CANCELLED: u64 = u64::MAX;

/// How many threads the background compositor's pool folds tiles on
/// (0.188.0): one fewer than the machine's available parallelism, and at
/// least one, so the UI thread (and the brush, which composites its own
/// dab synchronously) always keeps a core. A machine whose parallelism
/// cannot be read gets one.
#[must_use]
pub(crate) fn composite_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1))
}

type TileKey = (SurfaceId, TileId);

/// One job, built on the UI thread by [`CompositeWorker::dispatch`].
struct CompositeJob {
    generation: u64,
    /// A fresh mirror replacing the worker's current one (a new document
    /// store); `None` keeps the current mirror and applies `updates`.
    reset: Option<TileStore>,
    updates: Vec<(TileKey, TileShare)>,
    forgets: Vec<TileKey>,
    layers: aurora_doc::LayerTree,
    reference_origin: (i64, i64),
    tiles: Vec<TileId>,
}

enum ToWorker {
    Job(Box<CompositeJob>),
    /// Drop the mirror: the store it mirrors is gone or no longer current,
    /// and the mirror's shared tiles would otherwise keep that document's
    /// pixels alive.
    Release,
}

/// What the worker sends back.
pub(crate) enum FromWorker {
    Tile {
        generation: u64,
        tile_id: TileId,
        texels: Vec<half::f16>,
    },
    Done {
        generation: u64,
    },
}

/// The UI's record of the one job in flight.
#[derive(Debug)]
struct ActiveJob {
    generation: u64,
    cache_id: u64,
    store_serial: u64,
    stamp: u64,
    reference_origin: (i64, i64),
    tiles: HashSet<TileId>,
}

/// A wake for the event loop; the real one sends `AppEvent::CompositeReady`.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

/// The document state one dispatch or drain acts on.
pub(crate) struct CompositeTarget<'a> {
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) reference_origin: (i64, i64),
    pub(crate) store: &'a mut TileStore,
    pub(crate) cache: &'a mut crate::CompositeCache,
}

/// The UI-thread half of the background compositor. Owned by `App`; its
/// thread starts at the first dispatch.
pub(crate) struct CompositeWorker {
    waker: Waker,
    tx: Option<mpsc::Sender<ToWorker>>,
    rx: Option<mpsc::Receiver<FromWorker>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// The generation the worker should be compositing; anything else (a
    /// newer job, [`CANCELLED`]) makes it stop between tiles.
    latest: Arc<AtomicU64>,
    /// Coalesces wakes: set by the worker when it wakes the loop, cleared
    /// by [`CompositeWorker::drain`].
    wake_pending: Arc<AtomicBool>,
    next_generation: u64,
    /// The serial of the store the mirror currently mirrors.
    mirror_serial: Option<u64>,
    /// Every source tile the mirror holds, at the version it was sent.
    sent: HashMap<TileKey, u64>,
    active: Option<ActiveJob>,
    /// Jobs superseded by a newer one for the same document, newest last
    /// (at most [`SUPERSEDED_KEPT`]): their finished tiles are installed
    /// *provisionally* (shown, never marked current), so a continuous
    /// structural drag keeps updating the canvas instead of freezing it.
    superseded: Vec<ActiveJob>,
    /// Provisional installs so far (for tests and the PLAN figures).
    provisional_installs: usize,
    /// The mirror's resident tile count, published by the worker after
    /// each job (MEM-1's measurement).
    mirror_resident: Arc<std::sync::atomic::AtomicUsize>,
    /// Set when the active job reports done, taken by
    /// [`CompositeWorker::take_finished`].
    finished: bool,
    shut_down: bool,
    /// Tiles the worker thread has composited, over its whole life.
    composited: Arc<std::sync::atomic::AtomicUsize>,
    /// The pool's thread count ([`composite_threads`] unless a test asked).
    threads: usize,
    /// Test-only: what the last dispatch shipped, for the snapshot-size tests.
    #[cfg(test)]
    pub(crate) last_shipped: usize,
    /// Test-only hooks into the pool's tasks.
    #[cfg(test)]
    hooks: TestHooks,
}

/// Test-only hooks into the pool's tasks: which threads composited a tile,
/// and a callback each task runs just before it folds (after its cancel
/// check), so a test can hold tasks in flight.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct TestHooks {
    pub(crate) threads: Arc<Mutex<HashSet<std::thread::ThreadId>>>,
    pub(crate) before_tile: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The entry cap of the shared drop-shadow cache the worker made
    /// (0.192.0); zero until its thread has started.
    pub(crate) shadow_cap_entries: Arc<std::sync::atomic::AtomicUsize>,
}

impl std::fmt::Debug for CompositeWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeWorker")
            .field("active", &self.active)
            .field("mirror_serial", &self.mirror_serial)
            .field("sent", &self.sent.len())
            .finish_non_exhaustive()
    }
}

impl CompositeWorker {
    pub(crate) fn new(waker: Waker) -> Self {
        Self::with_threads(waker, composite_threads())
    }

    /// A worker whose pool has `threads` threads (at least one).
    pub(crate) fn with_threads(waker: Waker, threads: usize) -> Self {
        Self {
            waker,
            tx: None,
            rx: None,
            handle: None,
            latest: Arc::new(AtomicU64::new(CANCELLED)),
            wake_pending: Arc::new(AtomicBool::new(false)),
            next_generation: 0,
            mirror_serial: None,
            sent: HashMap::new(),
            active: None,
            superseded: Vec::new(),
            provisional_installs: 0,
            mirror_resident: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            finished: false,
            shut_down: false,
            composited: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            threads: threads.max(1),
            #[cfg(test)]
            last_shipped: 0,
            #[cfg(test)]
            hooks: TestHooks::default(),
        }
    }

    /// Test-only: installs `hooks` (before the first dispatch, which
    /// starts the thread).
    #[cfg(test)]
    pub(crate) fn set_hooks(&mut self, hooks: TestHooks) {
        self.hooks = hooks;
    }

    /// Test-only: how many distinct pool threads have composited a tile.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn threads_used(&self) -> usize {
        self.hooks.threads.lock().map_or(0, |threads| threads.len())
    }

    /// Test-only: the entry cap of the worker's shared drop-shadow cache
    /// (zero until its thread has started).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn shadow_cap_entries(&self) -> usize {
        self.hooks.shadow_cap_entries.load(Ordering::SeqCst)
    }

    /// Whether a job is in flight.
    #[must_use]
    pub(crate) fn is_busy(&self) -> bool {
        self.active.is_some()
    }

    /// Tiles of the active job that are still worth waiting for: dispatched
    /// for this very document and not invalidated since. The redraw
    /// handler leaves these alone instead of recompositing them.
    #[must_use]
    pub(crate) fn covers(&self, target: &CompositeTarget<'_>, tile: TileId) -> bool {
        self.active.as_ref().is_some_and(|job| {
            job.cache_id == target.cache.id()
                && job.store_serial == target.store.serial()
                && job.reference_origin == target.reference_origin
                && job.tiles.contains(&tile)
                && target.cache.unchanged_since(tile, job.stamp)
        })
    }

    /// Whether the active job is healthy: same document, same grid, and no
    /// tile of it invalidated since its stamp. A healthy job is left to
    /// finish (so a pan doesn't starve it); an unhealthy one is superseded.
    #[must_use]
    pub(crate) fn active_is_healthy(&self, target: &CompositeTarget<'_>) -> bool {
        self.active.as_ref().is_some_and(|job| {
            job.cache_id == target.cache.id()
                && job.store_serial == target.store.serial()
                && job.reference_origin == target.reference_origin
                && job
                    .tiles
                    .iter()
                    .all(|&t| target.cache.unchanged_since(t, job.stamp))
        })
    }

    /// Whether the active job was dispatched for `target`'s document.
    #[must_use]
    pub(crate) fn belongs_to(&self, target: &CompositeTarget<'_>) -> bool {
        self.active.as_ref().is_some_and(|job| {
            job.cache_id == target.cache.id() && job.store_serial == target.store.serial()
        })
    }

    /// Whether the mirror mirrors a store other than `serial` (or none).
    #[must_use]
    pub(crate) fn mirrors_other_than(&self, serial: Option<u64>) -> bool {
        self.mirror_serial.is_some() && self.mirror_serial != serial
    }

    /// Asks the worker to drop its mirror (document closed or switched
    /// away); the next dispatch builds a fresh one.
    pub(crate) fn release_mirror(&mut self) {
        self.mirror_serial = None;
        self.sent.clear();
        if let Some(tx) = self.tx.as_ref() {
            let _ = tx.send(ToWorker::Release);
        }
    }

    /// Whether a job finished since the last call.
    pub(crate) fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }

    /// Cancels the job in flight (document closed or switched away). Its
    /// results, arriving later, are dropped by generation.
    pub(crate) fn cancel(&mut self) {
        self.latest.store(CANCELLED, Ordering::SeqCst);
        self.active = None;
        self.superseded.clear();
    }

    /// Provisional installs so far.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn provisional_installs(&self) -> usize {
        self.provisional_installs
    }

    /// The mirror's resident tile count after the worker's last job.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn mirror_resident(&self) -> usize {
        self.mirror_resident.load(Ordering::SeqCst)
    }

    fn ensure_thread(&mut self) -> bool {
        if self.shut_down {
            return false;
        }
        if self.tx.is_some() {
            return true;
        }
        let (to_tx, to_rx) = mpsc::channel::<ToWorker>();
        let (from_tx, from_rx) = mpsc::channel::<FromWorker>();
        let latest = Arc::clone(&self.latest);
        let wake_pending = Arc::clone(&self.wake_pending);
        let waker = Arc::clone(&self.waker);
        let composited = Arc::clone(&self.composited);
        let mirror_resident = Arc::clone(&self.mirror_resident);
        let threads = self.threads;
        // Read here, on the caller's thread (0.192.0): a test's forced
        // machine size is per thread, and the answer is the same for the
        // whole process in the app.
        let shadow_bytes = crate::small_machine::shadow_cache_bytes();
        #[cfg(test)]
        let hooks = self.hooks.clone();
        let spawned = std::thread::Builder::new()
            .name("aurora-composite".into())
            .spawn(move || {
                // One drop-shadow cache for this thread and the whole pool.
                let shadows = crate::new_shared_shadow_cache(shadow_bytes);
                #[cfg(test)]
                hooks.shadow_cap_entries.store(
                    shadows.lock().map_or(0, |cache| cache.cap_entries()),
                    Ordering::SeqCst,
                );
                crate::use_shared_shadow_cache(Some(Arc::clone(&shadows)));
                let pool = build_pool(threads, &shadows);
                let context = WorkerContext {
                    latest: &latest,
                    wake_pending: &wake_pending,
                    waker: &waker,
                    composited: &composited,
                    mirror_resident: &mirror_resident,
                    pool: pool.as_ref(),
                    #[cfg(test)]
                    hooks,
                };
                worker_loop(&to_rx, &from_tx, &context);
                crate::use_shared_shadow_cache(None);
            });
        match spawned {
            Ok(handle) => {
                self.tx = Some(to_tx);
                self.rx = Some(from_rx);
                self.handle = Some(handle);
                true
            }
            Err(err) => {
                tracing::warn!(?err, "couldn't start the background compositor");
                self.shut_down = true;
                false
            }
        }
    }

    /// Builds and sends one job for `tiles` (UI thread). Supersedes any
    /// job in flight. Returns `false` if there is no worker (it could not
    /// be started, or was shut down) — the caller then composites
    /// synchronously, exactly as before 0.186.0.
    pub(crate) fn dispatch(
        &mut self,
        target: &mut CompositeTarget<'_>,
        tiles: Vec<TileId>,
    ) -> bool {
        if dispatch_refused_for_tests() || !self.ensure_thread() {
            return false;
        }
        let serial = target.store.serial();
        let reset = if self.mirror_serial == Some(serial) {
            None
        } else {
            let Some(mirror) = new_mirror(target.store) else {
                return false;
            };
            // Whatever the old mirror held is no longer trusted, even if
            // this dispatch fails below.
            self.mirror_serial = None;
            self.sent.clear();
            Some(mirror)
        };
        let composite = crate::composite_surface_id();
        let held = target.store.held_tile_versions();
        // BG-2: only surfaces this tree can composite (its layers' pixels
        // and masks). A tile of any other surface is not shipped, and is
        // forgotten from the mirror if it was there.
        let wanted = crate::tree_surfaces(target.layers);
        // Nothing is recorded in `sent` until the job is really on its way:
        // a failure below leaves the mirror's bookkeeping exactly as it was.
        let mut updates = Vec::new();
        let mut versions: Vec<(TileKey, u64)> = Vec::new();
        let mut live: HashSet<TileKey> = HashSet::with_capacity(held.len());
        for (key, version) in held {
            if key.0 == composite || !wanted.contains(&key.0) {
                continue;
            }
            live.insert(key);
            if self.sent.get(&key) == Some(&version) {
                continue;
            }
            match target.store.share_tile(key.0, key.1) {
                Ok(Some(share)) => {
                    updates.push((key, share));
                    versions.push((key, version));
                }
                Ok(None) => {}
                Err(err) => {
                    // The mirror must not silently diverge: give up on the
                    // worker for this frame and let the caller go synchronous.
                    tracing::warn!(?err, "couldn't share a tile with the background compositor");
                    return false;
                }
            }
        }
        let forgets: Vec<TileKey> = self
            .sent
            .keys()
            .filter(|key| !live.contains(key))
            .copied()
            .collect();
        #[cfg(test)]
        {
            self.last_shipped = updates.len();
        }
        self.next_generation = self.next_generation.wrapping_add(1) % CANCELLED;
        let generation = self.next_generation;
        let forgets_sent = forgets;
        let stamp = target.cache.stamp();
        target.cache.track_in_flight(tiles.iter().copied());
        let job = CompositeJob {
            generation,
            reset,
            updates,
            forgets: forgets_sent.clone(),
            layers: target.layers.clone(),
            reference_origin: target.reference_origin,
            tiles: tiles.clone(),
        };
        self.latest.store(generation, Ordering::SeqCst);
        let sent = self
            .tx
            .as_ref()
            .is_some_and(|tx| tx.send(ToWorker::Job(Box::new(job))).is_ok());
        if !sent {
            self.shut_down = true;
            self.mirror_serial = None;
            self.sent.clear();
            self.active = None;
            return false;
        }
        self.mirror_serial = Some(serial);
        if let Some(previous) = self.active.take() {
            if previous.cache_id == target.cache.id() && previous.store_serial == serial {
                self.superseded.push(previous);
                if self.superseded.len() > SUPERSEDED_KEPT {
                    let _ = self.superseded.remove(0);
                }
            } else {
                self.superseded.clear();
            }
        }
        for key in &forgets_sent {
            self.sent.remove(key);
        }
        self.sent.extend(versions);
        self.active = Some(ActiveJob {
            generation,
            cache_id: target.cache.id(),
            store_serial: serial,
            stamp,
            reference_origin: target.reference_origin,
            tiles: tiles.into_iter().collect(),
        });
        true
    }

    /// Installs every finished tile that is still valid (UI thread, from
    /// `about_to_wait`). `target` is the *current* document; results for
    /// any other are dropped. Returns how many tiles were installed.
    pub(crate) fn drain(&mut self, target: Option<&mut CompositeTarget<'_>>) -> usize {
        self.wake_pending.store(false, Ordering::SeqCst);
        let Some(rx) = self.rx.as_ref() else {
            return 0;
        };
        let messages: Vec<FromWorker> = rx.try_iter().collect();
        let mut installed = 0;
        let mut target = target;
        for message in messages {
            match message {
                FromWorker::Tile {
                    generation,
                    tile_id,
                    texels,
                } => {
                    let Some(target) = target.as_deref_mut() else {
                        continue;
                    };
                    let is_active = self
                        .active
                        .as_ref()
                        .is_some_and(|job| job.generation == generation);
                    if is_active {
                        if self
                            .active
                            .as_ref()
                            .is_some_and(|job| result_is_installable(job, target, tile_id))
                        {
                            crate::install_composited_tile(
                                target.store,
                                target.cache,
                                tile_id,
                                &texels,
                            );
                            installed += 1;
                        }
                        continue;
                    }
                    // A superseded job's tile: shown if it is newer than what
                    // the canvas shows, never marked current, so the newest
                    // job still recomposites it (BG-3).
                    let Some(job) = self.superseded.iter().find(|j| j.generation == generation)
                    else {
                        continue;
                    };
                    if provisional_is_installable(job, target, tile_id) {
                        crate::install_provisional_tile(
                            target.store,
                            target.cache,
                            tile_id,
                            &texels,
                            job.stamp,
                        );
                        self.provisional_installs += 1;
                        installed += 1;
                    }
                }
                FromWorker::Done { generation } => {
                    self.superseded.retain(|job| job.generation != generation);
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|job| job.generation == generation)
                    {
                        self.active = None;
                        self.finished = true;
                        if let Some(target) = target.as_deref_mut() {
                            target.cache.clear_in_flight();
                        }
                    }
                }
            }
        }
        installed
    }

    /// Stops the worker for good (quit): cancels the job in flight, closes
    /// the channel and joins the thread. The pool starts no new tile once
    /// the cancel lands, and finishes only the tiles already in progress
    /// (at most one per pool thread, side by side, so about one tile's
    /// time). Its mirror store is dropped on that thread, before the
    /// session's scratch directory is removed.
    pub(crate) fn shutdown(&mut self) {
        self.shut_down = true;
        self.cancel();
        self.tx = None;
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            tracing::warn!("the background compositor panicked");
        }
        self.rx = None;
        self.mirror_serial = None;
        self.sent.clear();
    }

    /// Test-only: blocks until the active job reports done (or `timeout`).
    #[cfg(test)]
    pub(crate) fn wait_until_idle(
        &mut self,
        mut target: Option<&mut CompositeTarget<'_>>,
        timeout: std::time::Duration,
    ) -> usize {
        let start = std::time::Instant::now();
        let mut installed = 0;
        while self.active.is_some() && start.elapsed() < timeout {
            installed += self.drain(target.as_deref_mut());
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        installed + self.drain(target)
    }

    /// Tiles the worker thread has composited so far.
    #[cfg(test)]
    pub(crate) fn composited(&self) -> usize {
        self.composited.load(Ordering::SeqCst)
    }

    /// Test-only: whether the worker thread has exited.
    #[cfg(test)]
    pub(crate) fn thread_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }
}

impl Drop for CompositeWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A superseded job's result may be *shown*: same document and grid,
/// the tile not current, and the job newer than what the tile shows.
fn provisional_is_installable(job: &ActiveJob, target: &CompositeTarget<'_>, tile: TileId) -> bool {
    job.cache_id == target.cache.id()
        && job.store_serial == target.store.serial()
        && job.reference_origin == target.reference_origin
        && job.tiles.contains(&tile)
        && !target.cache.is_current(tile)
        && target.cache.shows_older_than(tile, job.stamp)
}

fn result_is_installable(job: &ActiveJob, target: &CompositeTarget<'_>, tile: TileId) -> bool {
    job.cache_id == target.cache.id()
        && job.store_serial == target.store.serial()
        && job.reference_origin == target.reference_origin
        && job.tiles.contains(&tile)
        && !target.cache.is_current(tile)
        && target.cache.unchanged_since(tile, job.stamp)
}

/// A fresh mirror store in the session's scratch directory (or the
/// source store's own, in tests), with the source's residency budget:
/// resident tiles are shared, so that budget costs nothing extra for
/// them.
pub(crate) fn new_mirror(source: &mut TileStore) -> Option<TileStore> {
    let dir = crate::composite_mirror_scratch_dir(source)?;
    // MEM-1: a pooled source's mirror joins the same pool, so the two
    // budgets are shares of one total rather than two whole budgets.
    // 0.187.0: it joins the source's own pool group, so it is active
    // exactly when its document is and the two split the active share
    // (`aurora_tile::pool_shares`); the parked documents' shares do not
    // move. The source shrinks to its new share lazily (its own next
    // insertions) plus one eviction burst here.
    let created = if let (Some(pool), Some(group)) = (source.pool(), source.pool_group()) {
        TileStore::new_shared_in(dir, &pool, group)
    } else {
        let budget = std::num::NonZeroUsize::new(source.budget_tiles().max(1))?;
        TileStore::new(dir, budget)
    };
    if source.pool().is_some() {
        let _ = source.enforce_budget(aurora_tile::EVICTION_BURST);
    }
    match created {
        Ok(store) => Some(store),
        Err(err) => {
            tracing::warn!(?err, "couldn't create the background compositor's mirror");
            None
        }
    }
}

/// The pool the worker folds a job's tiles on: [`composite_threads`]
/// threads (or a test's count) whose drop-shadow coverage goes through
/// `shadows`. `None` if rayon cannot start it; the worker then folds
/// sequentially on its own thread, as before 0.188.0.
pub(crate) fn build_pool(
    threads: usize,
    shadows: &crate::SharedShadowCache,
) -> Option<rayon::ThreadPool> {
    let shadows = Arc::clone(shadows);
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .thread_name(|index| format!("aurora-composite-{index}"))
        .start_handler(move |_| crate::use_shared_shadow_cache(Some(Arc::clone(&shadows))))
        .build()
    {
        Ok(pool) => Some(pool),
        Err(err) => {
            tracing::warn!(
                ?err,
                "couldn't start the compositor's thread pool; folding on one thread"
            );
            None
        }
    }
}

/// What the worker thread shares with its pool's tasks.
struct WorkerContext<'a> {
    latest: &'a AtomicU64,
    wake_pending: &'a AtomicBool,
    waker: &'a Waker,
    /// Tiles composited over the worker's whole life.
    composited: &'a std::sync::atomic::AtomicUsize,
    /// The mirror's resident tile count after the last job (MEM-1).
    mirror_resident: &'a std::sync::atomic::AtomicUsize,
    pool: Option<&'a rayon::ThreadPool>,
    #[cfg(test)]
    hooks: TestHooks,
}

impl WorkerContext<'_> {
    /// Wakes the event loop, at most once until the UI drains.
    fn wake(&self) {
        if !self.wake_pending.swap(true, Ordering::SeqCst) {
            (self.waker)();
        }
    }
}

/// Locks the mirror. A poisoned lock (a task panicked while holding it,
/// which a release build's `panic = "abort"` never sees) is used as is:
/// the store's own state is never left half-updated by a reader.
pub(crate) fn lock<S>(store: &Mutex<S>) -> MutexGuard<'_, S> {
    match store.lock() {
        Ok(store) => store,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One pool task's view of the mirror (0.188.0): a [`crate::TileSource`]
/// several threads read at once. A resident tile costs a lock, a lookup
/// and a reference count; its texels are copied by the fold outside the
/// lock. A tile that is not resident pages in under the lock (decoding
/// there, so page-ins are serialized; a warm mirror has none). Reads never
/// bump LRU recency under the lock themselves; the keys read are promoted
/// in one go before a page-in and when the task finishes, so a page-in's
/// eviction spares what the job is using.
///
/// `S` is what the mutex holds (60 FPS round 7): the worker's own mirror
/// store, or the frame path's `&mut TileStore` borrowed for one parallel
/// batch (`crate::frame_composite`).
pub(crate) struct SharedMirror<'a, S: BorrowMut<TileStore> = TileStore> {
    store: &'a Mutex<S>,
    serial: u64,
    hits: Vec<TileKey>,
}

impl<'a, S: BorrowMut<TileStore>> SharedMirror<'a, S> {
    /// A view of `store`, whose [`TileStore::serial`] is `serial`.
    pub(crate) fn new(store: &'a Mutex<S>, serial: u64) -> Self {
        Self {
            store,
            serial,
            hits: Vec::new(),
        }
    }

    /// Promotes the resident tiles this task has read (LRU recency only;
    /// nothing a fold sees).
    fn promote(store: &mut TileStore, hits: &mut Vec<TileKey>) {
        for (surface, id) in hits.drain(..) {
            if store.is_resident(surface, id) {
                let _ = store.get(surface, id);
            }
        }
    }

    /// Ends this task's reads: promotes what it read.
    pub(crate) fn finish(mut self) {
        if !self.hits.is_empty() {
            Self::promote((*lock(self.store)).borrow_mut(), &mut self.hits);
        }
    }
}

impl<S: BorrowMut<TileStore>> crate::TileSource for SharedMirror<'_, S> {
    fn contains_tile(&self, surface: SurfaceId, id: TileId) -> bool {
        lock(self.store).borrow().contains_tile(surface, id)
    }

    fn content_version(&self, surface: SurfaceId, id: TileId) -> u64 {
        lock(self.store).borrow().content_version(surface, id)
    }

    fn serial(&self) -> u64 {
        self.serial
    }

    fn texels(
        &mut self,
        surface: SurfaceId,
        id: TileId,
    ) -> Result<crate::SourceTexels<'_>, aurora_tile::TileError> {
        let mut guard = lock(self.store);
        let store: &mut TileStore = (*guard).borrow_mut();
        if let Some(texels) = store.peek_shared(surface, id) {
            self.hits.push((surface, id));
            return Ok(crate::SourceTexels::Shared(texels));
        }
        if !store.contains_tile(surface, id) {
            // `TileStore::get` would materialize a blank here; the fold
            // path never asks (it checks `contains_tile` first), and a
            // shared mirror must not change what other tasks' `contains_tile`
            // answers mid-job, so it reads the same blank without storing it.
            return Ok(crate::SourceTexels::Shared(Arc::new(vec![
                half::f16::ZERO;
                aurora_tile::SAMPLES
            ])));
        }
        Self::promote(store, &mut self.hits);
        store.get(surface, id)?;
        let texels = match store.peek_shared(surface, id) {
            Some(texels) => texels,
            None => Arc::new(store.get(surface, id)?.texels().to_vec()),
        };
        Ok(crate::SourceTexels::Shared(texels))
    }
}

/// Folds `tiles` against `mirror` on the pool (or, without one, on this
/// thread), sending each finished tile as it completes. A task first
/// checks the job is still the latest: a newer job, a cancel or a
/// shutdown stops the pool from *starting* tiles, and only those already
/// in progress finish. Returns `true` when the UI's receiver is gone.
fn composite_job(
    context: &WorkerContext<'_>,
    job: &JobTiles<'_>,
    mirror: &Mutex<TileStore>,
    tx: &mpsc::Sender<FromWorker>,
) -> bool {
    let serial = lock(mirror).serial();
    let gone = AtomicBool::new(false);
    let one = |tile_id: TileId| {
        if gone.load(Ordering::SeqCst) || context.latest.load(Ordering::SeqCst) != job.generation {
            return;
        }
        #[cfg(test)]
        {
            if let Ok(mut threads) = context.hooks.threads.lock() {
                threads.insert(std::thread::current().id());
            }
            if let Some(hook) = context.hooks.before_tile.as_ref() {
                hook();
            }
        }
        let mut source = SharedMirror::new(mirror, serial);
        let doc_origin = crate::composite_tile_doc_origin(job.reference_origin, tile_id);
        let mut budget = crate::CompositeBudget::for_tile_of_pass(job.layers);
        let (texels, _folded) = crate::composite_roots_into_tile(
            job.layers,
            &mut source,
            tile_id,
            doc_origin,
            job.reference_origin,
            &mut budget,
        );
        source.finish();
        context.composited.fetch_add(1, Ordering::SeqCst);
        if tx
            .send(FromWorker::Tile {
                generation: job.generation,
                tile_id,
                texels,
            })
            .is_err()
        {
            gone.store(true, Ordering::SeqCst);
            return;
        }
        context.wake();
    };
    match context.pool {
        // One tile per task (`with_max_len(1)`): tiles cost very
        // different amounts (an empty one next to a shadowed one), so the
        // pool balances them by stealing single tiles.
        Some(pool) => pool.install(|| {
            job.tiles
                .par_iter()
                .with_max_len(1)
                .for_each(|&tile_id| one(tile_id));
        }),
        None => job.tiles.iter().for_each(|&tile_id| one(tile_id)),
    }
    gone.load(Ordering::SeqCst)
}

/// The parts of a [`CompositeJob`] its pool tasks read (the job itself
/// also carries the mirror deltas, which are not theirs to see).
struct JobTiles<'a> {
    generation: u64,
    layers: &'a aurora_doc::LayerTree,
    reference_origin: (i64, i64),
    tiles: &'a [TileId],
}

fn worker_loop(
    rx: &mpsc::Receiver<ToWorker>,
    tx: &mpsc::Sender<FromWorker>,
    context: &WorkerContext<'_>,
) {
    let mut mirror: Option<TileStore> = None;
    let mut expected: HashSet<TileKey> = HashSet::new();
    while let Ok(first) = rx.recv() {
        // Every queued job's deltas apply in order (they are cumulative);
        // only the newest is composited.
        let mut newest: Option<Box<CompositeJob>> = None;
        for message in std::iter::once(first).chain(rx.try_iter()) {
            let mut job = match message {
                ToWorker::Job(job) => job,
                ToWorker::Release => {
                    mirror = None;
                    context.mirror_resident.store(0, Ordering::SeqCst);
                    expected.clear();
                    newest = None;
                    continue;
                }
            };
            if let Some(fresh) = job.reset.take() {
                mirror = Some(fresh);
                expected.clear();
            }
            if let Some(store) = mirror.as_mut() {
                for key in job.forgets.drain(..) {
                    store.forget_tile(key.0, key.1);
                    expected.remove(&key);
                }
                for (key, share) in job.updates.drain(..) {
                    store.insert_share(key.0, key.1, share);
                    expected.insert(key);
                }
            }
            newest = Some(job);
        }
        let Some(job) = newest else {
            continue;
        };
        let Some(mut store) = mirror.take() else {
            continue;
        };
        // Anything the mirror holds that the UI store doesn't (a tile a
        // previous job's reads materialized blank) goes, so `contains_tile`
        // answers here exactly as it would on the UI store.
        for (key, _) in store.held_tile_versions() {
            if !expected.contains(&key) {
                store.forget_tile(key.0, key.1);
            }
        }
        // A new pass for the (shared) drop-shadow cache's eviction policy.
        let _pass = crate::CompositeBudget::for_pass(&job.layers);
        let shared = Mutex::new(store);
        let gone = composite_job(
            context,
            &JobTiles {
                generation: job.generation,
                layers: &job.layers,
                reference_origin: job.reference_origin,
                tiles: &job.tiles,
            },
            &shared,
            tx,
        );
        let store = match shared.into_inner() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        context
            .mirror_resident
            .store(store.resident_len(), Ordering::SeqCst);
        mirror = Some(store);
        if gone {
            return;
        }
        if tx
            .send(FromWorker::Done {
                generation: job.generation,
            })
            .is_err()
        {
            return;
        }
        context.wake();
    }
}

/// Whether [`FAIL_DISPATCH`] is set (always `false` outside tests).
#[cfg(test)]
fn dispatch_refused_for_tests() -> bool {
    FAIL_DISPATCH.with(std::cell::Cell::get)
}

#[cfg(not(test))]
const fn dispatch_refused_for_tests() -> bool {
    false
}

#[cfg(test)]
thread_local! {
    /// Test hook (task 136 review, ZV-3): makes [`CompositeWorker::dispatch`]
    /// refuse, as a failed thread spawn or mirror would.
    pub(crate) static FAIL_DISPATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
