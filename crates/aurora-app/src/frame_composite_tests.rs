//! Differentials for [`crate::frame_composite`] (60 FPS round 7): the
//! frame path's parallel recomposite installs, for every tile, exactly the
//! bits the sequential loop installs, on random trees (groups, masks,
//! clipping, adjustments, Pass Through, shadows, strokes, glows), on a tree
//! of special `f16` values (NaN payloads, infinities, signed zeros,
//! subnormals), with a store budget small enough to page tiles in during
//! the parallel reads, and on the corpus.

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use crate::frame_composite::{
    FRAME_COMPOSITE_MAX_THREADS, TestHooks, frame_composite_threads, frame_pool,
    set_parallel_enabled_for_tests, set_test_hooks, take_diagnostics,
};
use crate::move_invalidation_tests::{corpus_files, pixel_layers, reaching_effects};
use crate::occlusion_cull_tests::{
    Rng, clear_shadow_cache, layer, mismatches, ok, opaque, random_node, rect, store_with_budget,
    translucent,
};
use crate::{CompositeCache, composite_surface_id, recomposite_tiles};

/// Resets the parallel switch even if a test fails half way.
struct Parallel;

impl Parallel {
    fn set(enabled: bool) -> Self {
        set_parallel_enabled_for_tests(enabled);
        Self
    }
}

impl Drop for Parallel {
    fn drop(&mut self) {
        set_parallel_enabled_for_tests(false);
    }
}

fn grid(side: u32) -> Vec<aurora_tile::TileId> {
    (0..side)
        .flat_map(|y| (0..side).map(move |x| aurora_tile::TileId { x, y }))
        .collect()
}

/// Recomposites `tiles` on a fresh cache, sequentially or in parallel, and
/// returns each tile's installed composite (in `tiles` order) plus how
/// many tiles the cache then calls current.
fn recomposite(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
    parallel: bool,
) -> (Vec<Vec<half::f16>>, usize) {
    clear_shadow_cache();
    let _switch = Parallel::set(parallel);
    // The caller lets the helpers claim first, so they composite most of
    // the tiles compared (on its own the caller would often do them all).
    let _hooks = Hooks::set(TestHooks {
        caller_yields: Some(std::time::Duration::from_secs(20)),
        ..TestHooks::NONE
    });
    let mut cache = CompositeCache::default();
    recomposite_tiles(tiles, layers, None, store, &mut cache, None, None);
    let current = tiles.iter().filter(|&&id| cache.is_current(id)).count();
    let out = tiles
        .iter()
        .map(|&id| ok(store.get(composite_surface_id(), id)).texels().to_vec())
        .collect();
    (out, current)
}

/// Asserts both ways install the same bits; returns the samples compared.
#[track_caller]
fn check(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
    what: &str,
) -> usize {
    let (sequential, seq_current) = recomposite(layers, store, tiles, false);
    // Poison the installed composites, so a tile the parallel path failed
    // to install cannot pass by showing the sequential result.
    for &id in tiles {
        if let Ok(dest) = store.get_mut(composite_surface_id(), id) {
            dest.texels_mut().fill(half::f16::from_bits(0x7E01));
        }
    }
    let (parallel, par_current) = recomposite(layers, store, tiles, true);
    assert_eq!(seq_current, par_current, "{what}: current tiles differ");
    let mut compared = 0;
    for ((id, seq), par) in tiles.iter().zip(&sequential).zip(&parallel) {
        let bad = mismatches(par, seq);
        assert!(
            bad.is_empty(),
            "{what}: tile {id:?}: {} samples differ, first {:?}",
            bad.len(),
            bad.get(..bad.len().min(4))
        );
        compared += seq.len();
    }
    compared
}

fn pool_or_skip() -> bool {
    if frame_pool().is_none() {
        println!("SKIPPED: no frame pool (one core)");
        return false;
    }
    true
}

#[test]
fn the_thread_count_is_bounded() {
    let threads = frame_composite_threads();
    assert!(
        (1..=FRAME_COMPOSITE_MAX_THREADS).contains(&threads)
            || std::env::var_os("AURORA_R7_FRAME_THREADS").is_some(),
        "{threads}"
    );
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    if std::env::var_os("AURORA_R7_FRAME_THREADS").is_none() {
        assert_eq!(threads, available.min(FRAME_COMPOSITE_MAX_THREADS));
        assert_eq!(frame_pool().is_some(), threads > 1);
    }
    if let Some(pool) = frame_pool() {
        assert_eq!(
            pool.helpers(),
            threads - 1,
            "the caller is one of the threads"
        );
    }
}

/// Resets the protocol's test hooks even if a test fails half way.
struct Hooks;

impl Hooks {
    fn set(hooks: TestHooks) -> Self {
        set_test_hooks(hooks);
        let _ = take_diagnostics();
        Self
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        set_test_hooks(TestHooks::NONE);
    }
}

/// A 4 x 4-tile view over an opaque backdrop under a translucent
/// `Exclusion` layer (a CPU-path document).
fn cpu_document(store: &mut aurora_tile::TileStore) -> aurora_doc::LayerTree {
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(&mut layers, store, None, rect(0, 0, 1024, 1024), &opaque);
    let top = layer(
        &mut layers,
        store,
        None,
        rect(40, 30, 900, 800),
        &translucent,
    );
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Exclusion));
    assert!(!crate::document_qualifies_for_gpu_compositing(&layers));
    layers
}

/// The parallel path really composites on the helpers when the caller
/// lets them (the differentials below would pass if the caller quietly
/// did everything), and their tiles are the sequential loop's bits.
#[test]
fn helpers_composite_tiles_the_caller_installs() {
    if !pool_or_skip() {
        return;
    }
    let (_d, mut store) = store_with_budget(256);
    let layers = cpu_document(&mut store);
    let tiles = grid(4);
    let (sequential, _) = recomposite(&layers, &mut store, &tiles, false);
    poison(&mut store, &tiles);
    let _hooks = Hooks::set(TestHooks {
        caller_yields: Some(std::time::Duration::from_secs(20)),
        ..TestHooks::NONE
    });
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    let before = crate::COMPOSITE_STRAIGHTEN_PASSES.with(std::cell::Cell::get);
    let mut costs = crate::RecompositeTileCosts::start();
    assert!(crate::recomposite_tiles_in_parallel(
        &tiles,
        &layers,
        &mut store,
        &mut cache,
        (0, 0),
        &mut costs,
    ));
    let diagnostics = take_diagnostics();
    assert_eq!(diagnostics.passes, 1);
    assert!(diagnostics.helper_tiles > 0, "{diagnostics:?}");
    assert_eq!(
        diagnostics.helper_tiles + diagnostics.caller_tiles + diagnostics.duplicates,
        16,
        "{diagnostics:?}"
    );
    // The helpers' straightening was not done on this thread.
    let straightened = crate::COMPOSITE_STRAIGHTEN_PASSES.with(std::cell::Cell::get) - before;
    assert!(straightened < 16, "{straightened}");
    for &id in &tiles {
        assert!(cache.is_current(id), "{id:?}");
    }
    assert_installed(&mut store, &tiles, &sequential, "helpers");
}

/// Helpers that never start cost the caller nothing: it composites every
/// tile itself, waits for nobody, and installs the sequential loop's bits.
#[test]
fn helpers_that_never_start_leave_the_caller_doing_everything() {
    if !pool_or_skip() {
        return;
    }
    let (_d, mut store) = store_with_budget(256);
    let layers = cpu_document(&mut store);
    let tiles = grid(4);
    let (sequential, _) = recomposite(&layers, &mut store, &tiles, false);
    poison(&mut store, &tiles);
    let _hooks = Hooks::set(TestHooks {
        no_helpers: true,
        ..TestHooks::NONE
    });
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
    let diagnostics = take_diagnostics();
    assert_eq!(diagnostics.caller_tiles, 16, "{diagnostics:?}");
    assert_eq!(diagnostics.helper_tiles, 0);
    assert_eq!(diagnostics.duplicates, 0);
    assert_eq!(diagnostics.wait_ns, 0, "nothing was waited for");
    assert_eq!(crate::frame_composite::take_pooled_tiles(), 16);
    assert_installed(&mut store, &tiles, &sequential, "no helpers");
}

/// A helper preempted in the middle of its tile does not hold the frame:
/// the caller composites that tile itself after a bounded wait and
/// returns long before the helper would have finished.
#[test]
fn a_stalled_helper_is_not_waited_for() {
    if !pool_or_skip() {
        return;
    }
    let (_d, mut store) = store_with_budget(256);
    let layers = cpu_document(&mut store);
    let tiles = grid(4);
    let (sequential, _) = recomposite(&layers, &mut store, &tiles, false);
    poison(&mut store, &tiles);
    // Long enough that a slow (dev, loaded) pass still ends well before
    // it; a stalled helper only occupies a pool thread, which other tests'
    // passes do without (their callers composite instead).
    let stall = std::time::Duration::from_secs(30);
    let _hooks = Hooks::set(TestHooks {
        helper_stall: Some(stall),
        // Let a helper claim a tile before the caller starts, so one is
        // certainly stalled mid-pass.
        caller_yields: Some(std::time::Duration::from_secs(5)),
        ..TestHooks::NONE
    });
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    let started = std::time::Instant::now();
    recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
    let took = started.elapsed();
    let diagnostics = take_diagnostics();
    assert!(took < stall, "waited for the stalled helper: {took:?}");
    assert_eq!(diagnostics.helper_tiles, 0, "{diagnostics:?}");
    // Every tile a stalled helper claimed was composited by the caller.
    if diagnostics.helper_claimed_passes > 0 {
        assert!(diagnostics.duplicates > 0, "{diagnostics:?}");
    }
    assert_eq!(
        diagnostics.caller_tiles + diagnostics.duplicates,
        16,
        "{diagnostics:?}"
    );
    assert_installed(&mut store, &tiles, &sequential, "stalled helper");
}

/// A helper that needs a tile that is not resident hands the tile back,
/// and the caller pages it in and composites it: the same bits.
#[test]
fn a_helper_hands_back_a_tile_it_cannot_read() {
    if !pool_or_skip() {
        return;
    }
    // A budget far below the document's tiles, so most are evicted.
    let (_d, mut store) = store_with_budget(4);
    let layers = cpu_document(&mut store);
    let tiles = grid(4);
    let (sequential, _) = recomposite(&layers, &mut store, &tiles, false);
    poison(&mut store, &tiles);
    let _hooks = Hooks::set(TestHooks {
        caller_yields: Some(std::time::Duration::from_secs(20)),
        ..TestHooks::NONE
    });
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
    let diagnostics = take_diagnostics();
    assert!(diagnostics.handed_back > 0, "{diagnostics:?}");
    assert_installed(&mut store, &tiles, &sequential, "handed back");
}

/// A helper's budget never logs: a breach only records the wish, and the
/// helper hands the tile to the caller, whose single pass budget reports
/// once per pass (`composite_budget_reports_a_breach_once_per_pass...`).
#[test]
fn a_helpers_budget_never_reports() {
    let layers = aurora_doc::LayerTree::new();
    let mut quiet = crate::CompositeBudget::quiet_for_tile(&layers);
    assert!(!quiet.wanted_report());
    assert!(!quiet.should_report(), "a helper logs nothing");
    assert!(
        quiet.wanted_report(),
        "but its tile goes back to the caller"
    );
    let mut loud = crate::CompositeBudget::for_tile_of_pass(&layers);
    assert!(loud.should_report());
    assert!(!loud.wanted_report());
}

/// At most `SYNC_TILE_LIMIT` stale tiles (a dab) keep the sequential
/// loop; current tiles are skipped; a tile named twice is composited once.
#[test]
fn a_dabs_tiles_stay_sequential_and_duplicates_composite_once() {
    if !pool_or_skip() {
        return;
    }
    assert_eq!(
        crate::frame_composite::MIN_PARALLEL_TILES,
        crate::background_composite::SYNC_TILE_LIMIT + 1
    );
    let (_d, mut store) = store_with_budget(256);
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 1280, 256),
        &opaque,
    );
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    let mut costs = crate::RecompositeTileCosts::start();
    let t = |x| aurora_tile::TileId { x, y: 0 };
    let _ = crate::frame_composite::take_pooled_tiles();
    // Three stale tiles, named twice each: the most a dab composites
    // synchronously, so the pool declines and nothing is done.
    assert!(!crate::recomposite_tiles_in_parallel(
        &[t(0), t(1), t(2), t(0), t(1), t(2)],
        &layers,
        &mut store,
        &mut cache,
        (0, 0),
        &mut costs,
    ));
    assert!(!cache.is_current(t(0)), "nothing done when declined");
    // Through `recomposite_tiles`, three stale tiles fold sequentially.
    recomposite_tiles(
        &[t(0), t(1), t(2)],
        &layers,
        None,
        &mut store,
        &mut cache,
        None,
        None,
    );
    assert_eq!(crate::frame_composite::take_pooled_tiles(), 0);
    assert!(cache.is_current(t(2)));
    // Four stale of six named (two current, one named twice): the pool
    // composites exactly the four, once each.
    let composites_before = tile_composites();
    assert!(crate::recomposite_tiles_in_parallel(
        &[t(0), t(3), t(1), t(4), t(5), t(4), t(6), t(2)],
        &layers,
        &mut store,
        &mut cache,
        (0, 0),
        &mut costs,
    ));
    assert_eq!(tile_composites() - composites_before, 4);
    assert_eq!(crate::frame_composite::take_pooled_tiles(), 4);
    for x in 0..7 {
        assert!(cache.is_current(t(x)), "{x}");
    }
}

/// Tile composites recorded on this thread's profile (the pool's merged in).
fn tile_composites() -> u64 {
    crate::COMPOSITE_PROFILE.with(|profile| {
        profile
            .get()
            .slots
            .get(crate::ProfileSlot::TileComposite as usize)
            .map_or(0, |slot| slot.0)
    })
}

/// A document the GPU can composite keeps the GPU arm with the parallel
/// path on: no tile is composited on the CPU.
#[test]
fn a_gpu_document_keeps_the_gpu_arm_with_the_pool_on() {
    if !pool_or_skip() {
        return;
    }
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (_d, mut store) = store_with_budget(256);
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(&mut layers, &mut store, None, rect(0, 0, 512, 512), &opaque);
    assert!(crate::document_qualifies_for_gpu_compositing(&layers));
    let mut compositor = aurora_render::TileCompositor::new(context.device());
    let _switch = Parallel::set(true);
    let mut cache = CompositeCache::default();
    let before = tile_composites();
    recomposite_tiles(
        &grid(2),
        &layers,
        None,
        &mut store,
        &mut cache,
        Some(&context),
        Some(&mut compositor),
    );
    assert_eq!(tile_composites() - before, 0, "every tile went to the GPU");
    for id in grid(2) {
        assert!(cache.is_current(id), "{id:?}");
    }
}

#[test]
fn random_trees_composite_the_same_in_parallel() {
    if !pool_or_skip() {
        return;
    }
    let mut rng = Rng(0x5EED_0127_0007_0001);
    let mut compared = 0;
    let trees = 40;
    for tree in 0..trees {
        // A budget below the working set on some trees, so the parallel
        // reads page tiles in (and evict) under the lock.
        let budget = if tree % 3 == 0 { 24 } else { 1024 };
        let (_d, mut store) = store_with_budget(budget);
        let mut layers = aurora_doc::LayerTree::new();
        for _ in 0..=rng.below(4) {
            random_node(&mut rng, &mut layers, &mut store, None, 1);
        }
        for id in pixel_layers(&layers) {
            if rng.chance(30) {
                ok(layers.set_effects(id, Some(reaching_effects(&mut rng))));
            }
            if rng.chance(20) {
                let _ = layers.set_clipped(id, true);
            }
            if rng.chance(20) && layers.add_mask(id, rect(100, 80, 400, 300)).is_ok() {
                if rng.chance(50) {
                    ok(layers.set_mask_inverted(id, true));
                }
                if rng.chance(30) {
                    ok(layers.set_mask_density(id, 0.5));
                }
            }
        }
        let mut tiles = grid(4);
        tiles.push(aurora_tile::TileId { x: 1, y: 1 });
        compared += check(&layers, &mut store, &tiles, &format!("tree {tree}"));
    }
    println!("parallel frame differential: {trees} trees, {compared} samples");
    assert!(compared > 1_000_000, "{compared}");
}

/// Every special `f16` class in the source: NaNs with distinct payloads
/// and signs, infinities, signed zeros, subnormals, values above one, in
/// two overlapping layers of every separable and non-separable mode.
/// The special `f16` bit patterns the special-values differential plants.
const SPECIALS: [u16; 12] = [
    0x7E00, 0x7C01, 0xFE37, 0x7D55, 0x7C00, 0xFC00, 0x0000, 0x8000, 0x0001, 0x83FF, 0x4400, 0x3C00,
];

#[test]
fn special_values_composite_the_same_in_parallel() {
    if !pool_or_skip() {
        return;
    }
    let mut compared = 0;
    for (index, mode) in aurora_doc::BlendMode::ALL
        .into_iter()
        .filter(|m| *m != aurora_doc::BlendMode::PassThrough)
        .enumerate()
    {
        let (_d, mut store) = store_with_budget(256);
        let mut layers = aurora_doc::LayerTree::new();
        let below = layer(
            &mut layers,
            &mut store,
            None,
            rect(0, 0, 768, 512),
            &translucent,
        );
        let above = layer(&mut layers, &mut store, None, rect(0, 0, 768, 512), &opaque);
        ok(layers.set_blend_mode(above, mode));
        ok(layers.set_opacity(above, 0.75));
        for (salt, id) in [(0_usize, below), (5, above)] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("pixel layer");
            };
            for tile in grid(3) {
                if tile.y > 1 {
                    continue;
                }
                let Ok(dest) = store.get_mut(surface, tile) else {
                    continue;
                };
                for (i, sample) in dest.texels_mut().iter_mut().enumerate() {
                    if (i / 4 + salt + index) % 7 == 0 {
                        let pick = (i + salt * 3 + index) % SPECIALS.len();
                        *sample =
                            half::f16::from_bits(SPECIALS.get(pick).copied().unwrap_or(0x7E00));
                    }
                }
            }
        }
        let tiles: Vec<_> = grid(3).into_iter().filter(|t| t.y < 2).collect();
        compared += check(&layers, &mut store, &tiles, &format!("{mode:?}"));
    }
    assert!(compared > 0);
}

#[test]
fn corpus_composites_the_same_in_parallel() {
    if !pool_or_skip() {
        return;
    }
    let files = corpus_files();
    if files.is_empty() {
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        println!("SKIPPED: corpus absent");
        return;
    }
    let (mut opened, mut compared) = (0, 0);
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(document) = crate::open_psd_document(&bytes) else {
            continue;
        };
        let (_d, mut store) = store_with_budget(512);
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = crate::replace_document_pixels(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        if failed > 0 || pixel_layers(&document.layers).is_empty() {
            continue;
        }
        let (w, h) = document.canvas_size;
        if u64::from(w) * u64::from(h) > 4_000_000 {
            continue;
        }
        let tile = i64::from(aurora_tile::TILE);
        let (cols, rows) = (
            (i64::from(w) + tile - 1) / tile,
            (i64::from(h) + tile - 1) / tile,
        );
        let tiles: Vec<_> = (0..rows)
            .flat_map(|y| {
                (0..cols).map(move |x| aurora_tile::TileId {
                    x: x as u32,
                    y: y as u32,
                })
            })
            .collect();
        opened += 1;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        compared += check(&document.layers, &mut store, &tiles, &name);
    }
    println!("corpus parallel frame differential: {opened} files, {compared} samples");
    assert!(opened > 10, "{opened}");
}

/// Nyepi's whole canvas through the frame path, sequentially and in
/// parallel, equal bit for bit; prints both times (three interleaved
/// runs). Not run by default (minutes in the dev profile).
#[test]
#[ignore = "measures the Nyepi template; run with --ignored"]
fn nyepi_whole_canvas_composites_the_same_in_parallel() {
    if !pool_or_skip() {
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/flat-design-nyepi-template/6739968.psd");
    let Ok(bytes) = std::fs::read(&path) else {
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the Nyepi template is missing"
        );
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let document = ok(crate::open_psd_document(&bytes));
    let (_d, mut store) = store_with_budget(16_384);
    let incoming: Vec<_> = document
        .pixels
        .iter()
        .map(|p| (p.layer, &p.image, p.offset))
        .collect();
    let (_freed, failed) = crate::replace_document_pixels(
        &mut store,
        aurora_doc::LayerTree::new(),
        aurora_doc::History::new(),
        &document.layers,
        &incoming,
        &document.masks,
    );
    assert_eq!(failed, 0);
    let (w, h) = document.canvas_size;
    let tile = aurora_tile::TILE;
    let tiles: Vec<_> = (0..h.div_ceil(tile))
        .flat_map(|y| (0..w.div_ceil(tile)).map(move |x| aurora_tile::TileId { x, y }))
        .collect();
    let compared = check(&document.layers, &mut store, &tiles, "nyepi");
    for run in 0..3 {
        for parallel in [false, true] {
            let started = std::time::Instant::now();
            let _ = recomposite(&document.layers, &mut store, &tiles, parallel);
            println!(
                "nyepi run {run}: {} tiles, parallel {parallel}: {:.0} ms (incl. readback)",
                tiles.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    println!("nyepi parallel frame differential: {compared} samples");
}

/// Poisons the composite tiles `tiles`, so a tile a path fails to install
/// cannot pass by showing an earlier result.
fn poison(store: &mut aurora_tile::TileStore, tiles: &[aurora_tile::TileId]) {
    for &id in tiles {
        if let Ok(dest) = store.get_mut(composite_surface_id(), id) {
            dest.texels_mut().fill(half::f16::from_bits(0x7E01));
        }
    }
}

#[track_caller]
fn assert_installed(
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
    want: &[Vec<half::f16>],
    what: &str,
) {
    for (&id, want) in tiles.iter().zip(want) {
        let got = ok(store.get(composite_surface_id(), id)).texels().to_vec();
        let bad = mismatches(&got, want);
        assert!(
            bad.is_empty(),
            "{what}: tile {id:?}: {} samples differ",
            bad.len()
        );
    }
}

/// The shipping decision path with the pool on (60 FPS round 7): without a
/// worker a whole view folds on the pool; with one, a structural edit goes
/// to the worker and a dab's tiles fold sequentially in the redraw
/// handler. Every path installs the sequential loop's bits.
#[test]
fn the_redraw_decision_uses_the_pool_only_without_a_worker() {
    if !pool_or_skip() {
        return;
    }
    let (_d, mut store) = store_with_budget(256);
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 1024, 1024),
        &opaque,
    );
    let top = layer(
        &mut layers,
        &mut store,
        None,
        rect(40, 30, 900, 800),
        &translucent,
    );
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Exclusion));
    assert!(!crate::document_qualifies_for_gpu_compositing(&layers));
    let tiles = grid(4);
    let (sequential, _) = recomposite(&layers, &mut store, &tiles, false);
    let _switch = Parallel::set(true);
    let _ = crate::frame_composite::take_pooled_tiles();

    // No worker (what `recomposite_or_dispatch` does without one): the
    // pool composites the whole view.
    poison(&mut store, &tiles);
    let mut cache = CompositeCache::default();
    recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
    assert_eq!(crate::frame_composite::take_pooled_tiles(), 16);
    assert_installed(&mut store, &tiles, &sequential, "no worker");

    // A worker: the whole view is dispatched, nothing folds on the pool.
    poison(&mut store, &tiles);
    let waker: crate::background_composite::Waker = std::sync::Arc::new(|| {});
    let mut worker = crate::background_composite::CompositeWorker::with_threads(waker, 2);
    let mut cache = CompositeCache::default();
    let reference_origin = crate::composite_reference_origin(&layers, None);
    let start = std::time::Instant::now();
    while !tiles.iter().all(|&t| cache.is_current(t)) {
        assert!(
            start.elapsed() < std::time::Duration::from_mins(2),
            "never settled"
        );
        crate::dispatch_or_composite_tiles(
            &tiles,
            &layers,
            None,
            &mut store,
            &mut cache,
            &mut worker,
        );
        let _ = worker.wait_until_idle(
            Some(&mut crate::background_composite::CompositeTarget {
                layers: &layers,
                reference_origin,
                store: &mut store,
                cache: &mut cache,
            }),
            std::time::Duration::from_mins(2),
        );
    }
    assert_eq!(crate::frame_composite::take_pooled_tiles(), 0);
    assert_installed(&mut store, &tiles, &sequential, "worker");

    // A dab: three stale tiles, composited in the redraw handler,
    // sequentially.
    let dab: Vec<_> = tiles.iter().copied().take(3).collect();
    poison(&mut store, &dab);
    for &id in &dab {
        cache.note_invalidated(id);
    }
    let composites_before = tile_composites();
    crate::dispatch_or_composite_tiles(&tiles, &layers, None, &mut store, &mut cache, &mut worker);
    assert_eq!(
        tile_composites() - composites_before,
        3,
        "synchronous, here"
    );
    assert_eq!(
        crate::frame_composite::take_pooled_tiles(),
        0,
        "not on the pool"
    );
    assert_installed(&mut store, &dab, &sequential, "dab");
}
