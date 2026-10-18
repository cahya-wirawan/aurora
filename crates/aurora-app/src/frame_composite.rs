//! The frame path's CPU recomposite, in parallel across tiles (60 FPS
//! rounds 7 and 8).
//!
//! [`crate::recomposite_tiles`] composites a CPU-path document's stale
//! visible tiles one after another on the calling (UI) thread. When more
//! than [`crate::background_composite::SYNC_TILE_LIMIT`] tiles are stale
//! ([`MIN_PARALLEL_TILES`]) and no GPU arm can run, it hands them here.
//!
//! **The calling thread does the work; helpers only help (round 8).**
//! Round 7 handed every tile to a rayon pool and blocked in
//! `ThreadPool::install` until the pool had done them all. Under CPU load
//! that wait is a scheduling cost, not a compositing one: the caller
//! sleeps, the pool's threads have to be woken and scheduled, and the
//! caller has to be scheduled again when the last one finishes. On a
//! loaded Apple M2 Pro that made the pooled frame 2-5x slower than the
//! sequential loop (PLAN.md, round 8). Now:
//!
//! - The caller claims tiles one at a time from a shared atomic index
//!   and composites each itself, against the store, with the sequential
//!   loop's own code and its single pass budget.
//! - [`FramePool::helpers`] helper tasks are spawned (never waited for to
//!   start) and claim tiles from the same index. A helper reads a
//!   **snapshot** of the store taken at the start of the pass (every held
//!   tile's content version and, for a resident tile, its copy-on-write
//!   texels: an `Arc`, no copy) and the pass's own clone of the layer
//!   tree, so it never touches the store and owns everything it reads:
//!   the caller can return while a helper is still running.
//! - A helper that needs a tile the snapshot has no texels for (evicted,
//!   not resident), or whose tile would log a bound breach, hands the
//!   tile back: the caller composites it itself, so every page-in and
//!   every report happens on the caller, once per pass as in the
//!   sequential loop.
//! - Once the index is exhausted, the caller installs the results in
//!   tile order. For a tile a helper claimed and has not finished, it
//!   waits only while the helper is plausibly still working on it
//!   ([`STRAGGLER_WAIT_TILES`] mean tile times since the helper started
//!   it), spinning, never sleeping; past that, or with no tile time known
//!   yet, it composites the tile itself. A helper that never starts costs
//!   nothing; one preempted mid-tile costs at most that wait plus a tile.
//!
//! So the caller never waits for a tile no helper has started, and the
//! worst case is the sequential loop plus the pass's setup (a layer-tree
//! clone and the snapshot, [`Pass::new`]) plus that bounded wait per
//! straggling helper. A tile's composite is a pure function of the
//! store's content and the layer tree, so whoever computes it the result
//! is bit-identical to the sequential loop; only the store's LRU order
//! (the caller promotes what a helper read when it installs that tile)
//! and which pages are resident can differ, which no pixel sees.
//!
//! **Bounded.** A helper claims a tile only while it is fewer than
//! [`HELPER_WINDOW_TILES`] ahead of the caller's installed prefix, so at
//! most about that many results (512 KiB each, 8 MiB for 16) wait for
//! installation. The helpers share one drop-shadow coverage cache of
//! [`crate::small_machine::shadow_cache_bytes`], like the background
//! compositor's pool; the caller keeps its own thread's. A helper still
//! running when the pass ends holds the snapshot's reference counts
//! until it finishes its tile; a write to such a tile in that window
//! copies it once (copy-on-write).
//!
//! **Not for a dab.** The redraw handler composites a brush dab's one to
//! three stale tiles itself (`SYNC_TILE_LIMIT`), and they keep the
//! sequential loop. More stale tiles reach this path only without a
//! background worker, when its dispatch fails, or for a GPU-qualifying
//! document without a GPU.
//!
//! **Threads.** The caller plus at most [`FRAME_COMPOSITE_MAX_THREADS`]
//! − 1 helpers, and never more helpers than the machine's parallelism −
//! 1; a machine with one core (or whose parallelism cannot be read) gets
//! no pool and keeps the sequential loop. The background compositor's
//! pool is separate and may run at the same time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use aurora_tile::{SurfaceId, TileId, TileStore};

/// The most threads a frame recomposite uses: the calling thread and
/// three helpers. Four: on the 4-core/8-thread development machine a fifth
/// thread or more gains little on the two-root fixture (see PLAN.md, 60
/// FPS round 7).
pub(crate) const FRAME_COMPOSITE_MAX_THREADS: usize = 4;

/// The fewest stale tiles [`crate::recomposite_tiles`] hands to the pool:
/// one more than [`crate::background_composite::SYNC_TILE_LIMIT`], the
/// most a brush dab composites synchronously, so a dab never pays the
/// pass's setup.
pub(crate) const MIN_PARALLEL_TILES: usize =
    crate::background_composite::SYNC_TILE_LIMIT.saturating_add(1);

/// How far a helper may run ahead of the caller's installed prefix.
pub(crate) const HELPER_WINDOW_TILES: usize = 16;

/// The most tiles a store may hold (by
/// [`TileStore::held_tile_count_bound`]) for a pass to use the helpers
/// (60 FPS round 8). A pass's [`Snapshot`] walks every held tile on the
/// calling thread, so on a larger store the caller keeps the sequential
/// loop. Measured (release, i3-10100, `time_a_pass_over_a_large_store`)
/// at 0.13-0.16 µs a held tile (1.3 ms for 10,000, 8 ms for 50,000), so
/// this bounds the walk to about 0.6 ms, less than three helpers save on
/// a four-tile pass of a plain two-layer document (PLAN.md, round 8). In
/// the live app a large document's bigger recomposites go to the
/// background worker anyway; this path runs only without one.
pub(crate) const MAX_SNAPSHOT_TILES: usize = 4_096;

/// Whether a pass over `store` may use the helpers: its snapshot is
/// bounded ([`MAX_SNAPSHOT_TILES`]).
pub(crate) fn snapshot_is_bounded(store: &TileStore) -> bool {
    #[cfg(test)]
    let limit = SNAPSHOT_LIMIT.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let limit = MAX_SNAPSHOT_TILES;
    store.held_tile_count_bound() <= limit
}

/// How long, in mean tile times, the caller waits for a tile a helper
/// started before compositing it itself.
const STRAGGLER_WAIT_TILES: u32 = 2;

/// How many threads a frame recomposite uses, the calling thread
/// included: the machine's parallelism, capped at
/// [`FRAME_COMPOSITE_MAX_THREADS`]; `1` (no helpers) when it cannot be
/// read.
#[must_use]
pub(crate) fn frame_composite_threads() -> usize {
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    #[cfg(test)]
    if let Some(forced) = std::env::var("AURORA_R7_FRAME_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        return forced.max(1);
    }
    available.clamp(1, FRAME_COMPOSITE_MAX_THREADS)
}

/// The helpers' thread pool and the drop-shadow cache they share.
pub(crate) struct FramePool {
    pool: rayon::ThreadPool,
    shadows: crate::SharedShadowCache,
    helpers: usize,
}

impl FramePool {
    /// How many helper tasks a pass spawns at most (the pool's threads).
    #[cfg(test)]
    pub(crate) fn helpers(&self) -> usize {
        self.helpers
    }
}

/// The frame pool, built on first use; `None` with one thread, or if rayon
/// cannot start it (the caller then keeps the sequential loop).
pub(crate) fn frame_pool() -> Option<&'static FramePool> {
    static POOL: OnceLock<Option<FramePool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let helpers = frame_composite_threads().saturating_sub(1);
        if helpers == 0 {
            return None;
        }
        let shadows = crate::new_shared_shadow_cache(crate::small_machine::shadow_cache_bytes());
        let for_threads = Arc::clone(&shadows);
        match rayon::ThreadPoolBuilder::new()
            .num_threads(helpers)
            .thread_name(|index| format!("aurora-frame-composite-{index}"))
            .start_handler(move |_| {
                crate::use_shared_shadow_cache(Some(Arc::clone(&for_threads)));
            })
            .build()
        {
            Ok(pool) => Some(FramePool {
                pool,
                shadows,
                helpers,
            }),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "couldn't start the frame compositor's thread pool; compositing on one thread"
                );
                None
            }
        }
    })
    .as_ref()
}

#[cfg(test)]
thread_local! {
    /// Test-only: whether [`crate::recomposite_tiles`] may use the frame
    /// pool on this thread. Off by default in tests, because many tests
    /// read per-thread counters ([`crate::COMPOSITE_PROFILE`], the
    /// straighten and cull tallies, `aurora_render`'s skip counter) that
    /// the helpers' threads would not update on the caller; the tests of
    /// this module and the frame measurements turn it on.
    static PARALLEL_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test-only: tiles this thread installed through [`composite_tiles`].
    static POOLED_TILES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Test-only: what [`composite_tiles`] did on this thread, summed
    /// over passes since the last [`take_diagnostics`].
    static DIAGNOSTICS: std::cell::Cell<Diagnostics> = const {
        std::cell::Cell::new(Diagnostics::ZERO)
    };
    /// Test-only: [`snapshot_is_bounded`]'s limit on this thread (the
    /// large-store timing raises it to measure the unbounded walk).
    static SNAPSHOT_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_SNAPSHOT_TILES) };
    /// Test-only: hooks the next passes on this thread run with.
    static HOOKS: std::cell::Cell<TestHooks> = const { std::cell::Cell::new(TestHooks::NONE) };
}

/// Test-only switches for [`composite_tiles`]'s protocol.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TestHooks {
    /// Spawn no helper at all (helpers that never start).
    pub(crate) no_helpers: bool,
    /// A helper sleeps this long (or until its pass ends) after claiming
    /// each tile, before compositing it (a helper preempted mid-tile).
    pub(crate) helper_stall: Option<Duration>,
    /// The caller claims nothing until the helpers have claimed every
    /// tile they may (all, or a window's worth) or this long has passed,
    /// so the helpers composite the tiles a differential compares.
    pub(crate) caller_yields: Option<Duration>,
}

#[cfg(test)]
impl TestHooks {
    pub(crate) const NONE: Self = Self {
        no_helpers: false,
        helper_stall: None,
        caller_yields: None,
    };
}

/// Test-only: sets [`snapshot_is_bounded`]'s limit on this thread.
#[cfg(test)]
pub(crate) fn set_snapshot_limit_for_tests(limit: usize) {
    SNAPSHOT_LIMIT.with(|slot| slot.set(limit));
}

/// Test-only: how long a pass's [`Snapshot`] of `store` takes to build.
#[cfg(test)]
pub(crate) fn time_snapshot(store: &TileStore) -> (Duration, usize) {
    let began = Instant::now();
    let snapshot = Snapshot::of(store);
    (began.elapsed(), snapshot.tiles.len())
}

/// Test-only: sets the hooks the next passes on this thread run with.
#[cfg(test)]
pub(crate) fn set_test_hooks(hooks: TestHooks) {
    HOOKS.with(|slot| slot.set(hooks));
}

/// Test-only: what [`composite_tiles`] did, summed over passes.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Diagnostics {
    pub(crate) passes: u64,
    /// Tiles the caller claimed and composited.
    pub(crate) caller_tiles: u64,
    /// Tiles a helper composited and the caller installed.
    pub(crate) helper_tiles: u64,
    /// Tiles a helper claimed and the caller composited itself because
    /// the helper had not finished in time.
    pub(crate) duplicates: u64,
    /// Tiles a helper handed back (a tile it could not read, or a bound
    /// breach to report).
    pub(crate) handed_back: u64,
    /// Nanoseconds the caller spent setting passes up (tree clone,
    /// snapshot, spawning helpers).
    pub(crate) setup_ns: u64,
    /// Nanoseconds the caller spent waiting on helpers' tiles.
    pub(crate) wait_ns: u64,
    /// The most tiles helpers had claimed by the time a
    /// [`TestHooks::caller_yields`] wait ended (the window's bound).
    pub(crate) claimed_while_yielding: u64,
    /// Passes in which some helper claimed a tile, and the nanoseconds
    /// from each such pass's start to its first helper claim, summed.
    pub(crate) helper_claimed_passes: u64,
    pub(crate) first_claim_ns: u64,
}

#[cfg(test)]
impl Diagnostics {
    const ZERO: Self = Self {
        passes: 0,
        caller_tiles: 0,
        helper_tiles: 0,
        duplicates: 0,
        handed_back: 0,
        claimed_while_yielding: 0,
        setup_ns: 0,
        wait_ns: 0,
        helper_claimed_passes: 0,
        first_claim_ns: 0,
    };
}

/// Reads and resets this thread's [`Diagnostics`].
#[cfg(test)]
pub(crate) fn take_diagnostics() -> Diagnostics {
    DIAGNOSTICS.with(|d| d.replace(Diagnostics::ZERO))
}

#[cfg(test)]
fn note(f: impl FnOnce(&mut Diagnostics)) {
    DIAGNOSTICS.with(|cell| {
        let mut d = cell.get();
        f(&mut d);
        cell.set(d);
    });
}

#[cfg(test)]
fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Reads and resets this thread's count of tiles installed through the
/// frame pool's path (the caller's and the helpers').
#[cfg(test)]
pub(crate) fn take_pooled_tiles() -> u64 {
    POOLED_TILES.with(|n| n.replace(0))
}

/// Turns the parallel frame recomposite on or off on this thread (tests).
#[cfg(test)]
pub(crate) fn set_parallel_enabled_for_tests(enabled: bool) {
    PARALLEL_ENABLED.with(|flag| flag.set(enabled));
}

/// Whether [`crate::recomposite_tiles`] may use the frame pool: always in
/// a shipping build.
#[cfg(test)]
pub(crate) fn parallel_enabled() -> bool {
    PARALLEL_ENABLED.with(std::cell::Cell::get)
}

/// Whether [`crate::recomposite_tiles`] may use the frame pool: always in
/// a shipping build.
#[cfg(not(test))]
pub(crate) const fn parallel_enabled() -> bool {
    true
}

type TileKey = (SurfaceId, TileId);

/// A held tile's content version and, when resident, its texels.
type SnapshotEntry = (u64, Option<Arc<Vec<half::f16>>>);

/// Every tile a store holds at the start of a pass: its content version
/// and, when resident, its texels (a reference count on the store's
/// copy-on-write buffer). The composite surface's texels are left out, so
/// installing a composite never copies a buffer a snapshot shares (the
/// fold never reads that surface).
struct Snapshot {
    serial: u64,
    tiles: HashMap<TileKey, SnapshotEntry>,
}

impl Snapshot {
    fn of(store: &TileStore) -> Self {
        let composite = crate::composite_surface_id();
        let tiles = store
            .held_tile_versions()
            .into_iter()
            .map(|(key, version)| {
                let texels = if key.0 == composite {
                    None
                } else {
                    store.peek_shared(key.0, key.1)
                };
                (key, (version, texels))
            })
            .collect();
        Self {
            serial: store.serial(),
            tiles,
        }
    }
}

/// A helper's view of the store: the pass's [`Snapshot`]. A read the
/// snapshot cannot answer marks the tile missed (the helper then hands
/// it back) and fails, as a store read failure would.
struct SnapshotSource<'a> {
    snapshot: &'a Snapshot,
    missed: bool,
    hits: Vec<TileKey>,
}

impl crate::TileSource for SnapshotSource<'_> {
    fn contains_tile(&self, surface: SurfaceId, id: TileId) -> bool {
        self.snapshot.tiles.contains_key(&(surface, id))
    }

    fn content_version(&self, surface: SurfaceId, id: TileId) -> u64 {
        self.snapshot
            .tiles
            .get(&(surface, id))
            .map_or(0, |entry| entry.0)
    }

    fn serial(&self) -> u64 {
        self.snapshot.serial
    }

    fn texels(
        &mut self,
        surface: SurfaceId,
        id: TileId,
    ) -> Result<crate::SourceTexels<'_>, aurora_tile::TileError> {
        if let Some((_, Some(texels))) = self.snapshot.tiles.get(&(surface, id)) {
            self.hits.push((surface, id));
            return Ok(crate::SourceTexels::Shared(Arc::clone(texels)));
        }
        self.missed = true;
        Err(aurora_tile::TileError::Io {
            surface,
            id,
            source: std::io::Error::other("not resident when the frame pass began"),
        })
    }
}

/// A tile a helper composited.
struct HelperTile {
    texels: Vec<half::f16>,
    folded: usize,
    /// The resident tiles it read, for the caller's LRU promotion.
    hits: Vec<TileKey>,
    /// How long it took, for the caller's wait estimate.
    took: Duration,
    #[cfg(test)]
    profile: crate::CompositeProfile,
}

/// What a helper left for one tile it claimed.
#[derive(Default)]
struct Slot {
    /// When the helper started it.
    started: Option<Instant>,
    /// Its result, once done.
    result: Option<HelperTile>,
    /// The helper handed it back (the caller composites it).
    handed_back: bool,
}

/// One pass, shared by the caller and its helpers.
struct Pass {
    tiles: Vec<TileId>,
    layers: aurora_doc::LayerTree,
    reference_origin: (i64, i64),
    snapshot: Snapshot,
    /// The next tile index nobody has claimed.
    next: AtomicUsize,
    /// How many tiles the caller has installed (the helpers' window).
    installed: AtomicUsize,
    /// The pass is over: helpers stop claiming and drop their results.
    closed: AtomicBool,
    slots: Vec<Mutex<Slot>>,
    #[cfg(test)]
    began: Instant,
    #[cfg(test)]
    first_claim_ns: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    hooks: TestHooks,
}

impl Pass {
    fn new(
        tiles: &[TileId],
        layers: &aurora_doc::LayerTree,
        store: &TileStore,
        reference_origin: (i64, i64),
    ) -> Self {
        Self {
            tiles: tiles.to_vec(),
            layers: layers.clone(),
            reference_origin,
            snapshot: Snapshot::of(store),
            next: AtomicUsize::new(0),
            installed: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            slots: tiles.iter().map(|_| Mutex::new(Slot::default())).collect(),
            #[cfg(test)]
            began: Instant::now(),
            #[cfg(test)]
            first_claim_ns: std::sync::atomic::AtomicU64::new(u64::MAX),
            #[cfg(test)]
            hooks: HOOKS.with(std::cell::Cell::get),
        }
    }

    fn slot(&self, index: usize) -> Option<MutexGuard<'_, Slot>> {
        self.slots.get(index).map(|slot| match slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        })
    }

    /// Claims the next tile for a helper, if one is left within the
    /// window; `None` when the pass is over or every tile is claimed.
    /// Sleeps while the window is full (a helper is never on the critical
    /// path).
    fn claim_for_helper(&self) -> Option<usize> {
        loop {
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            let index = self.next.load(Ordering::Acquire);
            if index >= self.tiles.len() {
                return None;
            }
            let limit = self
                .installed
                .load(Ordering::Acquire)
                .saturating_add(HELPER_WINDOW_TILES);
            if index >= limit {
                std::thread::sleep(Duration::from_micros(50));
                continue;
            }
            if self
                .next
                .compare_exchange(index, index + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(index);
            }
        }
    }
}

/// A helper task: claims and composites tiles until none is left or the
/// pass is over.
fn run_helper(pass: &Pass) {
    while let Some(index) = pass.claim_for_helper() {
        #[cfg(test)]
        {
            let _ = pass
                .first_claim_ns
                .fetch_min(nanos(pass.began.elapsed()), Ordering::AcqRel);
        }
        let started = Instant::now();
        if let Some(mut slot) = pass.slot(index) {
            slot.started = Some(started);
        }
        #[cfg(test)]
        if let Some(stall) = pass.hooks.helper_stall {
            // Stalls until the pass is over (or `stall` passes), so a
            // stalled helper frees its pool thread with its pass.
            while started.elapsed() < stall && !pass.closed.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let Some(&tile_id) = pass.tiles.get(index) else {
            return;
        };
        let result = helper_composite(pass, tile_id, started);
        if pass.closed.load(Ordering::Acquire) {
            return;
        }
        if let Some(mut slot) = pass.slot(index) {
            match result {
                Some(tile) => slot.result = Some(tile),
                None => slot.handed_back = true,
            }
        }
    }
}

/// One tile on a helper, or `None` to hand it back to the caller.
fn helper_composite(pass: &Pass, tile_id: TileId, started: Instant) -> Option<HelperTile> {
    #[cfg(test)]
    let before = crate::COMPOSITE_PROFILE.with(std::cell::Cell::get);
    let mut source = SnapshotSource {
        snapshot: &pass.snapshot,
        missed: false,
        hits: Vec::new(),
    };
    let doc_origin = crate::composite_tile_doc_origin(pass.reference_origin, tile_id);
    let mut budget = crate::CompositeBudget::quiet_for_tile(&pass.layers);
    let (texels, folded) = crate::profiled(crate::ProfileSlot::TileComposite, || {
        crate::composite_roots_into_tile(
            &pass.layers,
            &mut source,
            tile_id,
            doc_origin,
            pass.reference_origin,
            &mut budget,
        )
    });
    // `missed` is the direct signal; with today's fold a missed read also
    // goes through a report (`wanted_report`), so either alone would hand
    // the tile back. Both are kept: a future read path that skips a
    // failed tile without reporting must still not keep its result.
    if source.missed || budget.wanted_report() {
        return None;
    }
    Some(HelperTile {
        texels,
        folded,
        hits: source.hits,
        took: started.elapsed(),
        #[cfg(test)]
        profile: profile_delta(before, crate::COMPOSITE_PROFILE.with(std::cell::Cell::get)),
    })
}

/// A finished tile on the caller's side.
enum Done {
    Own(Vec<half::f16>, usize),
    Helper(Box<HelperTile>),
}

/// Tile times seen this pass (the caller's and installed helpers'), for
/// the straggler wait.
#[derive(Default)]
struct Timing {
    total: Duration,
    tiles: u32,
}

impl Timing {
    fn add(&mut self, took: Duration) {
        self.total += took;
        self.tiles = self.tiles.saturating_add(1);
    }

    fn mean(&self) -> Option<Duration> {
        (self.tiles > 0).then(|| self.total / self.tiles)
    }
}

/// Composites `tiles` (distinct, stale, all on the CPU path) on the
/// calling thread with `frame`'s helpers and calls `install` for each in
/// `tiles` order with the composite and its fold count, exactly what the
/// sequential loop computes for that tile.
pub(crate) fn composite_tiles(
    frame: &FramePool,
    tiles: &[TileId],
    layers: &aurora_doc::LayerTree,
    store: &mut TileStore,
    reference_origin: (i64, i64),
    mut install: impl FnMut(&mut TileStore, TileId, Vec<half::f16>, usize),
) {
    let count = tiles.len();
    if count == 0 {
        return;
    }
    #[cfg(test)]
    let setup_began = Instant::now();
    // A new pass for the helpers' shared drop-shadow cache, and (through
    // `for_pass`) for this thread's, as the sequential loop begins one.
    match frame.shadows.lock() {
        Ok(mut cache) => cache.begin_pass(),
        Err(poisoned) => poisoned.into_inner().begin_pass(),
    }
    let mut budget = crate::CompositeBudget::for_pass(layers);
    let pass = Arc::new(Pass::new(tiles, layers, store, reference_origin));
    // Closes the pass however this function ends, an unwind included, so
    // no helper is left claiming (or sleeping on the window) for a pass
    // nobody will install.
    let _close = ClosePass(Arc::clone(&pass));
    let helpers = frame.helpers.min(count.saturating_sub(1));
    #[cfg(test)]
    let helpers = if pass.hooks.no_helpers { 0 } else { helpers };
    for _ in 0..helpers {
        let pass = Arc::clone(&pass);
        frame.pool.spawn(move || run_helper(&pass));
    }
    #[cfg(test)]
    {
        let setup = nanos(setup_began.elapsed());
        note(|d| {
            d.passes += 1;
            d.setup_ns = d.setup_ns.saturating_add(setup);
        });
        // Nothing to yield to without a helper (a pass of one tile).
        if let Some(limit) = pass.hooks.caller_yields.filter(|_| helpers > 0) {
            let until = Instant::now() + limit;
            // Helpers stop at the window while nothing is installed.
            let claimable = count.min(HELPER_WINDOW_TILES);
            while pass.next.load(Ordering::Acquire) < claimable && Instant::now() < until {
                std::thread::sleep(Duration::from_micros(100));
            }
            // Leave a stray claim in progress time to land, so the bound
            // is what is observed.
            std::thread::sleep(Duration::from_millis(20));
            let claimed = pass.next.load(Ordering::Acquire) as u64;
            note(|d| d.claimed_while_yielding = d.claimed_while_yielding.max(claimed));
        }
    }

    let mut done: Vec<Option<Done>> = (0..count).map(|_| None).collect();
    let mut installed = 0_usize;
    let mut timing = Timing::default();

    // Claim and composite until every tile is claimed, installing the
    // finished prefix as it grows.
    loop {
        let index = pass.next.fetch_add(1, Ordering::AcqRel);
        let Some(&tile_id) = tiles.get(index) else {
            break;
        };
        let began = Instant::now();
        let (texels, folded) = own_composite(layers, store, tile_id, reference_origin, &mut budget);
        timing.add(began.elapsed());
        #[cfg(test)]
        note(|d| d.caller_tiles += 1);
        if let Some(slot) = done.get_mut(index) {
            *slot = Some(Done::Own(texels, folded));
        }
        installed = install_ready(
            &pass,
            &mut done,
            installed,
            store,
            &mut timing,
            &mut install,
        );
    }

    // Every tile is claimed: finish the ones helpers hold, in order.
    while installed < count {
        let Some(&tile_id) = tiles.get(installed) else {
            break;
        };
        let entry = if let Some(tile) = wait_for_helper(&pass, installed, timing.mean()) {
            Done::Helper(Box::new(tile))
        } else {
            let (texels, folded) =
                own_composite(layers, store, tile_id, reference_origin, &mut budget);
            Done::Own(texels, folded)
        };
        if let Some(slot) = done.get_mut(installed) {
            *slot = Some(entry);
        }
        installed = install_ready(
            &pass,
            &mut done,
            installed,
            store,
            &mut timing,
            &mut install,
        );
    }
    #[cfg(test)]
    {
        let first = pass.first_claim_ns.load(Ordering::Acquire);
        if first != u64::MAX {
            note(|d| {
                d.helper_claimed_passes += 1;
                d.first_claim_ns = d.first_claim_ns.saturating_add(first);
            });
        }
    }
}

/// Marks its pass closed when dropped (the end of [`composite_tiles`],
/// normal or not): helpers stop claiming and drop what they finish.
struct ClosePass(Arc<Pass>);

impl Drop for ClosePass {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
    }
}

/// The caller's own composite of one tile: the sequential loop's.
fn own_composite(
    layers: &aurora_doc::LayerTree,
    store: &mut TileStore,
    tile_id: TileId,
    reference_origin: (i64, i64),
    budget: &mut crate::CompositeBudget,
) -> (Vec<half::f16>, usize) {
    budget.next_tile(layers);
    let doc_origin = crate::composite_tile_doc_origin(reference_origin, tile_id);
    crate::profiled(crate::ProfileSlot::TileComposite, || {
        crate::composite_roots_into_tile(
            layers,
            store,
            tile_id,
            doc_origin,
            reference_origin,
            budget,
        )
    })
}

/// Waits for the helper that claimed tile `index` while it is plausibly
/// still working on it ([`STRAGGLER_WAIT_TILES`] × `mean` since it
/// started, spinning), and returns its result; `None` when the caller
/// should composite the tile itself (handed back, too slow, or no tile
/// time known yet).
fn wait_for_helper(pass: &Pass, index: usize, mean: Option<Duration>) -> Option<HelperTile> {
    #[cfg(test)]
    let waited_from = Instant::now();
    let mut deadline: Option<Instant> = None;
    let mut handed_back = false;
    let outcome = loop {
        {
            let mut slot = pass.slot(index)?;
            if let Some(tile) = slot.result.take() {
                break Some(tile);
            }
            if slot.handed_back {
                handed_back = true;
                break None;
            }
            if deadline.is_none() {
                let Some(mean) = mean else {
                    break None;
                };
                let started = slot.started.unwrap_or_else(Instant::now);
                // An unrepresentable deadline (it cannot happen with real
                // tile times) means "do not wait".
                let Some(at) = mean
                    .checked_mul(STRAGGLER_WAIT_TILES)
                    .and_then(|wait| started.checked_add(wait))
                else {
                    break None;
                };
                deadline = Some(at);
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break None;
        }
        for _ in 0..64 {
            std::hint::spin_loop();
        }
    };
    #[cfg(test)]
    {
        let waited = nanos(waited_from.elapsed());
        note(|d| {
            d.wait_ns = d.wait_ns.saturating_add(waited);
            if handed_back {
                d.handed_back += 1;
            } else if outcome.is_none() {
                d.duplicates += 1;
            }
        });
    }
    #[cfg(not(test))]
    let _ = handed_back;
    outcome
}

/// Installs the finished prefix from `installed` on, in order, and
/// returns the new prefix length (also published to the helpers' window).
fn install_ready(
    pass: &Pass,
    done: &mut [Option<Done>],
    mut installed: usize,
    store: &mut TileStore,
    timing: &mut Timing,
    install: &mut impl FnMut(&mut TileStore, TileId, Vec<half::f16>, usize),
) -> usize {
    loop {
        let entry = match done.get_mut(installed) {
            Some(entry @ Some(_)) => entry.take(),
            Some(None) => pass
                .slot(installed)
                .and_then(|mut slot| slot.result.take())
                .map(|tile| Done::Helper(Box::new(tile))),
            None => None,
        };
        let Some(entry) = entry else {
            break;
        };
        let Some(&tile_id) = pass.tiles.get(installed) else {
            break;
        };
        let (texels, folded) = match entry {
            Done::Own(texels, folded) => (texels, folded),
            Done::Helper(tile) => {
                let tile = *tile;
                // The LRU recency the sequential loop's reads would have
                // given the resident tiles this helper read.
                for (surface, id) in tile.hits {
                    if store.is_resident(surface, id) {
                        let _ = store.get(surface, id);
                    }
                }
                timing.add(tile.took);
                #[cfg(test)]
                {
                    merge_profile(tile.profile);
                    note(|d| d.helper_tiles += 1);
                }
                (tile.texels, tile.folded)
            }
        };
        #[cfg(test)]
        POOLED_TILES.with(|n| n.set(n.get().saturating_add(1)));
        install(store, tile_id, texels, folded);
        installed += 1;
        pass.installed.store(installed, Ordering::Release);
    }
    installed
}

#[cfg(test)]
fn profile_delta(
    before: crate::CompositeProfile,
    after: crate::CompositeProfile,
) -> crate::CompositeProfile {
    let mut delta = after;
    for (slot, old) in delta.slots.iter_mut().zip(before.slots) {
        slot.0 = slot.0.saturating_sub(old.0);
        slot.1 = slot.1.saturating_sub(old.1);
    }
    delta
}

/// Adds a helper's profile to the calling thread's, so a measurement on
/// the caller sees the work its helpers did (CPU time summed over
/// threads).
#[cfg(test)]
fn merge_profile(delta: crate::CompositeProfile) {
    crate::COMPOSITE_PROFILE.with(|profile| {
        let mut current = profile.get();
        for (slot, add) in current.slots.iter_mut().zip(delta.slots) {
            slot.0 = slot.0.saturating_add(add.0);
            slot.1 = slot.1.saturating_add(add.1);
        }
        profile.set(current);
    });
}
