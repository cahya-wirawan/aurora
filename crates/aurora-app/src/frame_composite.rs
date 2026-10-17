//! The frame path's CPU recomposite, in parallel across tiles (60 FPS
//! round 7).
//!
//! [`crate::recomposite_tiles`] composites a CPU-path document's stale
//! visible tiles one after another on the calling (UI) thread. When more
//! than one tile is stale and no GPU arm can run, it hands them here: a
//! small rayon pool ([`frame_composite_threads`] threads) folds each tile
//! with the very same [`crate::composite_roots_into_tile`], reading the
//! document's store through [`crate::background_composite::SharedMirror`]
//! (the background compositor's own `TileSource` over a `Mutex`), and the
//! caller installs the results in tile order. A tile's composite is a pure
//! function of the store's content and the layer tree, so the result is
//! bit-identical to the sequential loop; only the store's LRU order and
//! which pages are resident can differ, which no pixel sees.
//!
//! **Bounded.** Tiles go in batches of four per thread (at least
//! `MIN_BATCH_TILES`, 16), so at most that many composite results
//! (512 KiB each, 8 MiB for 16) wait for installation.
//! The pool's threads share one drop-shadow coverage cache of
//! [`crate::small_machine::shadow_cache_bytes`], like the background
//! compositor's pool; the UI thread keeps its own for single-tile work, so
//! this adds at most one more cache of that cap.
//!
//! **Threads.** At most [`FRAME_COMPOSITE_MAX_THREADS`], never more than
//! the machine's parallelism; a machine with one core (or whose
//! parallelism cannot be read) gets no pool and keeps the sequential loop.
//! The calling thread waits while the pool works, so the frame uses at most
//! that many cores. The background compositor's pool is separate and may
//! run at the same time.

use std::sync::{Mutex, OnceLock};

use aurora_tile::{TileId, TileStore};
use rayon::prelude::*;

use crate::background_composite::SharedMirror;

/// The most threads the frame pool folds on. Four: on the 4-core/8-thread
/// development machine a fifth thread or more gains little on the
/// two-root fixture (see PLAN.md, 60 FPS round 7), and a four-core machine
/// then still gets one thread per core while the UI thread waits.
pub(crate) const FRAME_COMPOSITE_MAX_THREADS: usize = 4;

/// The fewest tiles a parallel batch holds (60 FPS round 7): a batch is
/// four tiles per thread or this many, whichever is more, so a typical
/// view (nine to sixteen tiles) is one batch and no thread idles while a
/// short last batch runs. At most this many composites (512 KiB each)
/// wait for installation on a machine with four threads or fewer.
const MIN_BATCH_TILES: usize = 16;

/// How many threads the frame pool has: the machine's parallelism, capped
/// at [`FRAME_COMPOSITE_MAX_THREADS`]; `1` (no pool) when it cannot be read.
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

/// The frame pool, built on first use; `None` with one thread, or if rayon
/// cannot start it (the caller then keeps the sequential loop).
pub(crate) fn frame_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = frame_composite_threads();
        if threads < 2 {
            return None;
        }
        let shadows = crate::new_shared_shadow_cache(crate::small_machine::shadow_cache_bytes());
        match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("aurora-frame-composite-{index}"))
            .start_handler(move |_| {
                crate::use_shared_shadow_cache(Some(std::sync::Arc::clone(&shadows)));
            })
            .build()
        {
            Ok(pool) => Some(pool),
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
    /// the pool's threads would not update on the caller; the tests of
    /// this module and the frame measurements turn it on.
    static PARALLEL_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

/// Composites `tiles` (distinct, stale, all on the CPU path) on `pool` and
/// calls `install` for each in `tiles` order with the composite and its
/// fold count, exactly what the sequential loop computes for that tile.
pub(crate) fn composite_tiles(
    pool: &rayon::ThreadPool,
    tiles: &[TileId],
    layers: &aurora_doc::LayerTree,
    store: &mut TileStore,
    reference_origin: (i64, i64),
    mut install: impl FnMut(&mut TileStore, TileId, Vec<half::f16>, usize),
) {
    // A new pass for the pool's shared drop-shadow cache, as
    // `CompositeBudget::for_pass` begins one on the calling thread's.
    pool.install(|| {
        let _ = crate::with_shadow_cache(crate::ShadowCache::begin_pass);
    });
    // One "already reported" flag for the whole pass, shared by every
    // tile's budget, so a malformed document warns once per pass as in
    // the sequential loop.
    let pass_reported = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let batch_len = pool
        .current_num_threads()
        .saturating_mul(4)
        .max(MIN_BATCH_TILES);
    for batch in tiles.chunks(batch_len) {
        let serial = store.serial();
        let shared = Mutex::new(&mut *store);
        let results: Vec<TileResult> = pool.install(|| {
            batch
                .par_iter()
                .with_max_len(1)
                .map(|&tile_id| {
                    composite_one(
                        &shared,
                        serial,
                        layers,
                        tile_id,
                        reference_origin,
                        &pass_reported,
                    )
                })
                .collect()
        });
        let store: &mut TileStore = match shared.into_inner() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (&tile_id, result) in batch.iter().zip(results) {
            #[cfg(test)]
            merge_profile(result.profile);
            install(store, tile_id, result.texels, result.folded);
        }
    }
}

/// One tile's result, plus (in tests) the profile its thread recorded.
struct TileResult {
    texels: Vec<half::f16>,
    folded: usize,
    #[cfg(test)]
    profile: crate::CompositeProfile,
}

fn composite_one(
    shared: &Mutex<&mut TileStore>,
    serial: u64,
    layers: &aurora_doc::LayerTree,
    tile_id: TileId,
    reference_origin: (i64, i64),
    pass_reported: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> TileResult {
    #[cfg(test)]
    let before = crate::COMPOSITE_PROFILE.with(std::cell::Cell::get);
    let mut source = SharedMirror::new(shared, serial);
    let doc_origin = crate::composite_tile_doc_origin(reference_origin, tile_id);
    let mut budget = crate::CompositeBudget::for_tile_of_shared_pass(layers, pass_reported);
    let (texels, folded) = crate::profiled(crate::ProfileSlot::TileComposite, || {
        crate::composite_roots_into_tile(
            layers,
            &mut source,
            tile_id,
            doc_origin,
            reference_origin,
            &mut budget,
        )
    });
    source.finish();
    TileResult {
        texels,
        folded,
        #[cfg(test)]
        profile: profile_delta(before, crate::COMPOSITE_PROFILE.with(std::cell::Cell::get)),
    }
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

/// Adds a pool task's profile to the calling thread's, so a measurement
/// on the caller sees the work its pool did (CPU time summed over threads).
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
