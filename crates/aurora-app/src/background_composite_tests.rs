//! Tests for the background compositor (0.186.0): bit-identity with the
//! synchronous path, and the concurrency rules — a stale tile is never
//! installed, a superseded generation is dropped, an edit, a document
//! switch or close, and shutdown during a job all behave.

use super::occlusion_cull_tests::{
    clear_shadow_cache, layer, mismatches, nyepi_shaped, ok, opaque, rect, store_with_budget,
};
use super::*;
use background_composite::{CompositeTarget, CompositeWorker, SYNC_TILE_LIMIT, Waker};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_mins(2);

fn noop_waker() -> Waker {
    Arc::new(|| {})
}

/// A waker that blocks the worker thread — right after it has sent a tile
/// — until the test opens it, so a test can act while a job is
/// genuinely in flight.
#[derive(Default)]
struct Gate {
    state: Mutex<(bool, usize)>,
    changed: Condvar,
}

impl Gate {
    fn waker(gate: &Arc<Self>) -> Waker {
        let gate = Arc::clone(gate);
        Arc::new(move || {
            let Ok(mut state) = gate.state.lock() else {
                return;
            };
            state.1 += 1;
            gate.changed.notify_all();
            let start = Instant::now();
            while !state.0 && start.elapsed() < Duration::from_secs(10) {
                let Ok((next, _)) = gate.changed.wait_timeout(state, Duration::from_millis(50))
                else {
                    return;
                };
                state = next;
            }
        })
    }

    fn wait_blocked(&self) {
        let start = Instant::now();
        let Ok(mut state) = self.state.lock() else {
            unreachable!("gate poisoned");
        };
        while state.1 == 0 {
            assert!(start.elapsed() < WAIT, "the worker never reached the gate");
            let Ok((next, _)) = self.changed.wait_timeout(state, Duration::from_millis(50)) else {
                unreachable!("gate poisoned");
            };
            state = next;
        }
    }

    fn open(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.0 = true;
        }
        self.changed.notify_all();
    }
}

fn grid(size: (u32, u32)) -> Vec<aurora_tile::TileId> {
    let mut tiles = Vec::new();
    for y in 0..size.1.div_ceil(aurora_tile::TILE) {
        for x in 0..size.0.div_ceil(aurora_tile::TILE) {
            tiles.push(aurora_tile::TileId { x, y });
        }
    }
    tiles
}

fn read_composite(
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
) -> Vec<Vec<half::f16>> {
    tiles
        .iter()
        .map(|&t| ok(store.get(composite_surface_id(), t)).texels().to_vec())
        .collect()
}

fn sync_composite(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
) -> Vec<Vec<half::f16>> {
    let _ = store.forget_surface(composite_surface_id());
    clear_shadow_cache();
    let mut cache = CompositeCache::default();
    recomposite_tiles(tiles, layers, None, store, &mut cache, None, None);
    read_composite(store, tiles)
}

fn target<'a>(
    layers: &'a aurora_doc::LayerTree,
    store: &'a mut aurora_tile::TileStore,
    cache: &'a mut CompositeCache,
) -> CompositeTarget<'a> {
    CompositeTarget {
        layers,
        reference_origin: composite_reference_origin(layers, None),
        store,
        cache,
    }
}

/// Drives the redraw handler's own decision until every tile is current.
fn settle(
    worker: &mut CompositeWorker,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    cache: &mut CompositeCache,
    tiles: &[aurora_tile::TileId],
) {
    let start = Instant::now();
    while !tiles.iter().all(|&t| cache.is_current(t)) {
        assert!(start.elapsed() < WAIT, "the composite never settled");
        dispatch_or_composite_tiles(tiles, layers, None, store, cache, worker);
        let _ = worker.wait_until_idle(Some(&mut target(layers, store, cache)), WAIT);
    }
}

fn worker_composite(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tiles: &[aurora_tile::TileId],
) -> (Vec<Vec<half::f16>>, usize) {
    let _ = store.forget_surface(composite_surface_id());
    let mut worker = CompositeWorker::new(noop_waker());
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(layers, store, &mut cache), tiles.to_vec()));
    let installed = worker.wait_until_idle(Some(&mut target(layers, store, &mut cache)), WAIT);
    (read_composite(store, tiles), installed)
}

fn assert_same(worker: &[Vec<half::f16>], sync: &[Vec<half::f16>], what: &str) {
    assert_eq!(worker.len(), sync.len());
    for (i, (w, s)) in worker.iter().zip(sync).enumerate() {
        let bad = mismatches(w, s);
        assert!(
            bad.is_empty(),
            "{what}: tile {i}: {} samples differ",
            bad.len()
        );
    }
}

/// A 3 x 3-tile CPU-path document: shadows in a Pass Through group, a
/// `Screen` text layer, plus an `Exclusion` layer (never on the GPU).
fn fixture(store: &mut aurora_tile::TileStore) -> (aurora_doc::LayerTree, (u32, u32)) {
    let (mut layers, size) = nyepi_shaped(store);
    let top = layer(&mut layers, store, None, rect(40, 30, 500, 400), &|x, y| {
        let [r, g, b, _] = opaque(y, x);
        [r, g, b, 0.5]
    });
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Exclusion));
    (layers, size)
}

#[test]
fn worker_composite_equals_sync_composite() {
    let (_d, mut store) = store_with_budget(256);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    assert!(tiles.len() > SYNC_TILE_LIMIT);
    let (worker, installed) = worker_composite(&layers, &mut store, &tiles);
    assert_eq!(installed, tiles.len(), "every tile came from the worker");
    let sync = sync_composite(&layers, &mut store, &tiles);
    assert_same(&worker, &sync, "synthetic");
}

#[test]
fn worker_composite_equals_sync_composite_with_evicted_sources() {
    // A budget far below the document: most source tiles are shipped to
    // the mirror as encoded bytes, not shared texels.
    let (_d, mut store) = store_with_budget(4);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let (worker, _) = worker_composite(&layers, &mut store, &tiles);
    let sync = sync_composite(&layers, &mut store, &tiles);
    assert_same(&worker, &sync, "evicted sources");
}

#[test]
fn worker_composite_equals_sync_composite_on_the_psd_corpus_subset() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpora/psd/reference");
    if !root.exists() {
        println!("SKIPPED: corpus absent");
        return;
    }
    let root_display = root.display().to_string();
    let mut pending = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("psd"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        println!("SKIPPED: corpus not fetched (no .psd under {root_display})");
        return;
    }
    let mut compared = 0;
    for path in files.iter().take(24) {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(document) = open_psd_document(&bytes) else {
            continue;
        };
        let (_d, mut store) = store_with_budget(256);
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = replace_document_pixels(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        if failed > 0 {
            continue;
        }
        let tiles = grid(document.canvas_size);
        let (worker, _) = worker_composite(&document.layers, &mut store, &tiles);
        let sync = sync_composite(&document.layers, &mut store, &tiles);
        assert_same(&worker, &sync, &path.display().to_string());
        compared += 1;
    }
    println!("corpus subset: {compared} files compared");
    assert!(compared >= 10, "only {compared} corpus files compared");
}

/// Repaints source tile (0, 0) of `id`'s surface and invalidates what it
/// reaches, as a dab does.
fn dab(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    cache: &mut CompositeCache,
    id: aurora_doc::LayerId,
) {
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let tile = ok(store.get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }));
    for texel in tile.texels_mut().chunks_exact_mut(aurora_tile::CHANNELS) {
        for (c, v) in texel.iter_mut().zip([0.9_f32, 0.1, 0.4, 1.0]) {
            *c = half::f16::from_f32(v);
        }
    }
    match stroke_invalidation(layers, surface, &[aurora_tile::TileId { x: 0, y: 0 }]) {
        CompositeInvalidation::Regions(reach) => {
            cache.invalidate_doc_rects(&reach, composite_reference_origin(layers, None));
        }
        CompositeInvalidation::Everything => cache.bump(),
        CompositeInvalidation::None => {}
    }
}

#[test]
fn a_dab_during_a_job_is_never_overwritten_by_the_stale_tile() {
    let (_d, mut store) = store_with_budget(256);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.wait_blocked();
    // Tile (0, 0) has been composited from the old content and sent.
    let Some(&bottom) = layers.roots().first() else {
        unreachable!("the fixture has layers");
    };
    dab(&layers, &mut store, &mut cache, bottom);
    gate.open();
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    let first = aurora_tile::TileId { x: 0, y: 0 };
    assert!(
        !cache.is_current(first),
        "the result computed before the dab must be dropped, not installed"
    );
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "after a dab during a job");
}

#[test]
fn a_superseded_generation_is_dropped() {
    let (_d, mut store) = store_with_budget(256);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.wait_blocked();
    // Job A has sent tile (0, 0). Job B, for the same unchanged document,
    // omits it: if A's result were installed, (0, 0) would become current.
    let first = aurora_tile::TileId { x: 0, y: 0 };
    let rest: Vec<_> = tiles.iter().copied().filter(|&t| t != first).collect();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), rest.clone()));
    gate.open();
    let installed =
        worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    assert!(
        !cache.is_current(first),
        "job A's tile is never installed as current"
    );
    // Since BG-3 it may be *shown* provisionally (at most once), never more.
    assert!(worker.provisional_installs() <= 1);
    assert_eq!(
        installed,
        rest.len() + worker.provisional_installs(),
        "only job B's tiles install as final"
    );
}

#[test]
fn an_undo_during_a_job_composites_the_undone_state() {
    let (_d, mut store) = store_with_budget(256);
    let (mut layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let mut history = aurora_doc::History::new();
    let Some(&text) = layers.roots().get(2) else {
        unreachable!("the fixture has a third root");
    };
    let before = layers.visible(text);
    ok(history.set_visible(&mut layers, text, !before.unwrap_or(true)));
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.wait_blocked();
    // Undo mid-job: a structural change, which bumps the whole cache.
    let _undone = ok(history.undo(&mut layers));
    cache.bump();
    gate.open();
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    assert!(
        tiles.iter().all(|&t| !cache.is_current(t)),
        "nothing composited before the undo may install"
    );
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "after an undo during a job");
}

#[test]
fn a_document_switch_or_close_during_a_job_installs_nothing_into_the_other_document() {
    let (_d, mut store_a) = store_with_budget(256);
    let (layers_a, size) = fixture(&mut store_a);
    let tiles = grid(size);
    let (_e, mut store_b) = store_with_budget(256);
    let (layers_b, _) = fixture(&mut store_b);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache_a = CompositeCache::default();
    let mut cache_b = CompositeCache::default();
    assert!(worker.dispatch(
        &mut target(&layers_a, &mut store_a, &mut cache_a),
        tiles.clone()
    ));
    gate.wait_blocked();
    // Switch to B: A's results arrive while B is current.
    gate.open();
    let installed = worker.wait_until_idle(
        Some(&mut target(&layers_b, &mut store_b, &mut cache_b)),
        WAIT,
    );
    assert_eq!(installed, 0, "A's tiles must never install into B");
    assert!(tiles.iter().all(|&t| !cache_b.is_current(t)));
    assert!(
        tiles
            .iter()
            .all(|&t| !store_b.contains_tile(composite_surface_id(), t))
    );
    // Close A: its store goes; the mirror is released, B composites fresh.
    let serial_b = Some(store_b.serial());
    drop(store_a);
    assert!(worker.mirrors_other_than(serial_b));
    worker.cancel();
    worker.release_mirror();
    settle(&mut worker, &layers_b, &mut store_b, &mut cache_b, &tiles);
    let have = read_composite(&mut store_b, &tiles);
    let want = sync_composite(&layers_b, &mut store_b, &tiles);
    assert_same(&have, &want, "B after A closed");
}

#[test]
fn shutdown_cancels_the_job_in_flight() {
    let (_d, mut store) = store_with_budget(256);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.wait_blocked();
    let opener = {
        let gate = Arc::clone(&gate);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            gate.open();
        })
    };
    worker.shutdown();
    let _ = opener.join();
    assert!(worker.thread_finished(), "shutdown joins the worker");
    assert!(
        worker.composited() < tiles.len(),
        "the job stops at the next tile, not after all {} ({} composited)",
        tiles.len(),
        worker.composited()
    );
    assert!(!worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles));
    assert!(!worker.is_busy());
}

#[test]
fn a_structural_edit_ships_no_tiles_and_a_dab_ships_only_its_own() {
    let (_d, mut store) = store_with_budget(256);
    let (mut layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let mut worker = CompositeWorker::new(noop_waker());
    let mut cache = CompositeCache::default();
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    let first = worker.last_shipped;
    assert!(first > 0, "the first job ships the document");
    let Some(&text) = layers.roots().get(2) else {
        unreachable!("third root");
    };
    ok(layers.set_visible(text, false));
    cache.bump();
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    assert_eq!(
        worker.last_shipped, 0,
        "a visibility toggle changes no pixels"
    );
    let Some(&bottom) = layers.roots().first() else {
        unreachable!("layers");
    };
    dab(&layers, &mut store, &mut cache, bottom);
    cache.bump();
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    assert_eq!(worker.last_shipped, 1, "only the repainted tile ships");
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "after mirror deltas");
}

/// AC-1/AC-4 measurements on the real template (release recommended):
/// UI-thread time of a dispatch, time to final pixels, and a dab.
#[test]
#[ignore = "needs a local PSD named by AURORA_EFFECTS_PSD"]
fn worker_composite_equals_sync_composite_on_nyepi_and_measures_it() {
    let Some(path) = std::env::var_os("AURORA_EFFECTS_PSD") else {
        println!("SKIPPED: AURORA_EFFECTS_PSD not set");
        return;
    };
    let bytes = ok(std::fs::read(&path));
    let document = ok(open_psd_document(&bytes));
    let (_d, mut store) = store_with_budget(16_384);
    let incoming: Vec<_> = document
        .pixels
        .iter()
        .map(|p| (p.layer, &p.image, p.offset))
        .collect();
    let (_freed, failed) = replace_document_pixels(
        &mut store,
        aurora_doc::LayerTree::new(),
        aurora_doc::History::new(),
        &document.layers,
        &incoming,
        &document.masks,
    );
    assert_eq!(failed, 0);
    let mut layers = document.layers;
    let tiles = grid(document.canvas_size);
    let held = store.held_tile_versions().len();

    let started = Instant::now();
    let sync = sync_composite(&layers, &mut store, &tiles);
    let sync_time = started.elapsed();

    let _ = store.forget_surface(composite_surface_id());
    let mut worker = CompositeWorker::new(noop_waker());
    let mut cache = CompositeCache::default();
    clear_shadow_cache();
    let started = Instant::now();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    let first_dispatch = started.elapsed();
    let shipped = worker.last_shipped;
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT * 5);
    let first_final = started.elapsed();
    let have = read_composite(&mut store, &tiles);
    assert_same(&have, &sync, "nyepi");

    // A structural edit: hide the topmost root, recomposite everything.
    let Some(&top) = layers.roots().last() else {
        unreachable!("layers");
    };
    ok(layers.set_visible(top, false));
    cache.bump();
    let started = Instant::now();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    let edit_dispatch = started.elapsed();
    let edit_shipped = worker.last_shipped;
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT * 5);
    let edit_final = started.elapsed();

    // A dab on the bottom layer's tile (0, 0), composited synchronously
    // (at most `SYNC_TILE_LIMIT` tiles), exactly as 0.185.0 did.
    println!(
        "NYEPI background composite: {} tiles, {held} source tiles held; sync full composite \
         {sync_time:?}; first dispatch {first_dispatch:?} (shipped {shipped}), final pixels \
         {first_final:?}; structural edit dispatch {edit_dispatch:?} (shipped {edit_shipped}), \
         final pixels {edit_final:?}",
        tiles.len()
    );
    let painted = (0..u64::try_from(layers.len()).unwrap_or(0).saturating_mul(4)).find_map(|raw| {
        let id = aurora_doc::LayerId::from_raw(raw);
        let surface = layers.surface_id(id)?;
        store
            .contains_tile(surface, aurora_tile::TileId { x: 0, y: 0 })
            .then_some(id)
    });
    let Some(bottom) = painted else {
        println!("SKIPPED dab: no pixel layer holds tile (0, 0)");
        return;
    };
    let started = Instant::now();
    dab(&layers, &mut store, &mut cache, bottom);
    let stale = tiles.iter().filter(|&&t| !cache.is_current(t)).count();
    dispatch_or_composite_tiles(&tiles, &layers, None, &mut store, &mut cache, &mut worker);
    let dab_time = started.elapsed();
    let after = Instant::now();
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT * 5);
    println!(
        "NYEPI dab: {stale} stale tiles, dab + redraw decision on the UI thread {dab_time:?} \
         (worker busy afterwards: {}), worker tail {:?}",
        worker.is_busy(),
        after.elapsed()
    );
}

#[test]
fn a_superseded_generations_stale_tile_never_installs_under_the_new_job() {
    // Job A sends tile (0, 0); a dab then invalidates it and job B is
    // dispatched *after* the dab, so B's stamp is newer than the
    // invalidation. Only the generation check keeps A's stale (0, 0) from
    // passing B's stamp test and being installed in place of B's.
    let (_d, mut store) = store_with_budget(256);
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.wait_blocked();
    let Some(&bottom) = layers.roots().first() else {
        unreachable!("the fixture has layers");
    };
    dab(&layers, &mut store, &mut cache, bottom);
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    gate.open();
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "after a superseded job");
}

/// A waker that blocks the worker on every wake until the test grants a
/// permit — one step at a time, for the drag test.
#[derive(Default)]
struct Steps {
    state: Mutex<(usize, usize)>,
    changed: Condvar,
}

impl Steps {
    fn waker(steps: &Arc<Self>) -> Waker {
        let steps = Arc::clone(steps);
        Arc::new(move || {
            let Ok(mut state) = steps.state.lock() else {
                return;
            };
            state.0 += 1;
            steps.changed.notify_all();
            // Bounded, so a failed assertion (whose unwinding drops and
            // joins the worker) can never deadlock on a closed gate.
            let start = Instant::now();
            while state.1 == 0 && start.elapsed() < Duration::from_secs(10) {
                let Ok((next, _)) = steps.changed.wait_timeout(state, Duration::from_millis(50))
                else {
                    return;
                };
                state = next;
            }
            state.1 = state.1.saturating_sub(1);
        })
    }

    fn wait_wakes(&self, n: usize) {
        let start = Instant::now();
        let Ok(mut state) = self.state.lock() else {
            unreachable!("poisoned");
        };
        while state.0 < n {
            assert!(start.elapsed() < WAIT, "the worker never woke {n} times");
            let Ok((next, _)) = self.changed.wait_timeout(state, Duration::from_millis(50)) else {
                unreachable!("poisoned");
            };
            state = next;
        }
    }

    fn permit(&self, n: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.1 += n;
        }
        self.changed.notify_all();
    }
}

/// The fixture's `Exclusion` layer (the one it adds last), which covers
/// tile (0, 0) — or, for another document, the last root.
fn top_layer(layers: &aurora_doc::LayerTree) -> aurora_doc::LayerId {
    let exclusion = layers
        .roots()
        .iter()
        .copied()
        .find(|&id| layers.blend_mode(id) == Some(aurora_doc::BlendMode::Exclusion));
    let Some(top) = exclusion.or_else(|| layers.roots().last().copied()) else {
        unreachable!("the document has layers");
    };
    top
}

/// BG-1: the App's own seams, end to end and headless — the redraw
/// handler's decision (`dispatch_or_composite_tiles`), the worker's wake
/// (`AppEvent::CompositeReady`, through the same waker shape `App::new`
/// builds), `user_event`'s redraw test (`app_event_requests_redraw`) and
/// `about_to_wait`'s drain (`drain_background_composite_into`), down to the
/// composite tile's dirty mark that `TileResidency::sync` uploads.
#[test]
fn app_wiring_dispatches_wakes_drains_and_installs_a_structural_edit() {
    let (_d, mut store) = store_with_budget(256);
    let (mut layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let (events_tx, events_rx) = std::sync::mpsc::channel::<AppEvent>();
    let events_tx = Mutex::new(events_tx);
    let mut worker = CompositeWorker::new(Arc::new(move || {
        if let Ok(tx) = events_tx.lock() {
            let _ = tx.send(AppEvent::CompositeReady);
        }
    }));
    let mut cache = CompositeCache::default();
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    for &t in &tiles {
        let _ = store.take_dirty(composite_surface_id(), t);
    }
    while events_rx.try_recv().is_ok() {}
    let before = read_composite(&mut store, &tiles);

    // A structural edit, as the opacity slider makes one.
    ok(layers.set_opacity(top_layer(&layers), 0.25));
    cache.bump();
    dispatch_or_composite_tiles(&tiles, &layers, None, &mut store, &mut cache, &mut worker);
    assert!(
        worker.is_busy(),
        "more than {SYNC_TILE_LIMIT} stale tiles go to the worker"
    );
    assert!(
        tiles.iter().all(|&t| !cache.is_current(t)),
        "the UI thread composited nothing"
    );

    let start = Instant::now();
    let mut redraws = 0;
    while worker.is_busy() || tiles.iter().any(|&t| !cache.is_current(t)) {
        assert!(start.elapsed() < WAIT, "the job never finished");
        let Ok(event) = events_rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        assert!(
            app_event_requests_redraw(&event),
            "CompositeReady must request a redraw"
        );
        if drain_background_composite_into(&mut worker, &layers, None, Some(&mut store), &mut cache)
        {
            redraws += 1;
        }
    }
    assert!(
        redraws > 0,
        "draining installed tiles and asked for a redraw"
    );
    assert!(
        tiles.iter().all(|&t| cache.is_current(t)),
        "every tile is current"
    );
    let mut uploaded = 0;
    for &t in &tiles {
        if store.take_dirty(composite_surface_id(), t).is_some() {
            uploaded += 1;
        }
    }
    let have = read_composite(&mut store, &tiles);
    assert!(have != before, "the composite surface was updated");
    assert!(
        uploaded > 0,
        "changed tiles are marked for the atlas upload"
    );
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "app wiring");
}

/// BG-1: a document switch mid-job, through `about_to_wait`'s drain.
#[test]
fn app_wiring_a_switch_mid_job_cancels_and_installs_nothing() {
    let (_d, mut store_a) = store_with_budget(256);
    let (layers_a, size) = fixture(&mut store_a);
    let tiles = grid(size);
    let (_e, mut store_b) = store_with_budget(256);
    let (layers_b, _) = fixture(&mut store_b);
    let gate = Arc::new(Gate::default());
    let mut worker = CompositeWorker::new(Gate::waker(&gate));
    let mut cache_a = CompositeCache::default();
    let mut cache_b = CompositeCache::default();
    dispatch_or_composite_tiles(
        &tiles,
        &layers_a,
        None,
        &mut store_a,
        &mut cache_a,
        &mut worker,
    );
    assert!(worker.is_busy());
    gate.wait_blocked();
    // The App switched to B; its next `about_to_wait` drains.
    let _ = drain_background_composite_into(
        &mut worker,
        &layers_b,
        None,
        Some(&mut store_b),
        &mut cache_b,
    );
    assert!(!worker.is_busy(), "A's job is cancelled");
    assert!(
        !worker.mirrors_other_than(Some(store_b.serial())),
        "A's mirror is released"
    );
    gate.open();
    std::thread::sleep(Duration::from_millis(50));
    let _ = drain_background_composite_into(
        &mut worker,
        &layers_b,
        None,
        Some(&mut store_b),
        &mut cache_b,
    );
    assert!(tiles.iter().all(|&t| !cache_b.is_current(t)));
    assert!(
        tiles
            .iter()
            .all(|&t| !store_b.contains_tile(composite_surface_id(), t))
    );
    // Back to A: nothing from the cancelled job installs; A recomposites.
    let _ = drain_background_composite_into(
        &mut worker,
        &layers_a,
        None,
        Some(&mut store_a),
        &mut cache_a,
    );
    assert!(tiles.iter().all(|&t| !cache_a.is_current(t)));
    settle(&mut worker, &layers_a, &mut store_a, &mut cache_a, &tiles);
    let have = read_composite(&mut store_a, &tiles);
    let want = sync_composite(&layers_a, &mut store_a, &tiles);
    assert_same(&have, &want, "A after switching back");
}

/// BG-3: a continuous structural drag keeps the canvas changing — each
/// superseded job's finished tile is shown provisionally — and a
/// provisional tile is never marked current.
#[test]
fn a_continuous_drag_shows_provisional_tiles_that_are_never_current() {
    let (_d, mut store) = store_with_budget(256);
    let (mut layers, size) = fixture(&mut store);
    let tiles = grid(size);
    let first = aurora_tile::TileId { x: 0, y: 0 };
    let steps = Arc::new(Steps::default());
    let mut worker = CompositeWorker::new(Steps::waker(&steps));
    let mut cache = CompositeCache::default();
    let top = top_layer(&layers);
    ok(layers.set_opacity(top, 0.1));
    cache.bump();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    let mut wakes = 1;
    let mut shown: Vec<Vec<half::f16>> = Vec::new();
    for round in 1..=4_u8 {
        // The job in flight has sent tile (0, 0); the drag moves on.
        steps.wait_wakes(wakes);
        ok(layers.set_opacity(top, 0.1 + 0.2 * f32::from(round)));
        cache.bump();
        assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
        let _ = worker.drain(Some(&mut target(&layers, &mut store, &mut cache)));
        assert_eq!(
            worker.provisional_installs(),
            usize::from(round),
            "round {round}"
        );
        assert!(
            !cache.is_current(first),
            "a provisional tile is never current"
        );
        shown.push(
            ok(store.get(composite_surface_id(), first))
                .texels()
                .to_vec(),
        );
        // Let the superseded job stop (its `Done` wakes once more), then
        // the new job composite its first tile.
        steps.permit(1);
        wakes += 1;
        steps.wait_wakes(wakes);
        let _ = worker.drain(Some(&mut target(&layers, &mut store, &mut cache)));
        steps.permit(1);
        wakes += 1;
    }
    for pair in shown.windows(2) {
        if let [a, b] = pair {
            assert!(a != b, "the shown tile changes every round of the drag");
        }
    }
    steps.permit(usize::MAX / 2);
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    settle(&mut worker, &layers, &mut store, &mut cache, &tiles);
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "after the drag");
}

/// MEM-1: with a pooled source, the mirror joins the same pool, so the
/// two stores' resident tiles stay within the pool's total plus the
/// disclosed slack (one floor's worth, from `max(floor, total / members)`),
/// and the result is still bit-identical while both page.
#[test]
fn the_mirror_shares_the_documents_pool_budget() {
    let dir = ok(tempfile::tempdir());
    let (Some(total), Some(floor)) = (
        std::num::NonZeroUsize::new(24),
        std::num::NonZeroUsize::new(4),
    ) else {
        unreachable!("non-zero");
    };
    let pool = aurora_tile::ResidencyPool::new(total, floor);
    let mut store = ok(aurora_tile::TileStore::new_shared(
        dir.path().to_path_buf(),
        &pool,
    ));
    let (layers, size) = fixture(&mut store);
    let tiles = grid(size);
    assert!(
        store.held_tile_versions().len() > total.get(),
        "the document exceeds the pool"
    );
    let mut worker = CompositeWorker::new(noop_waker());
    let mut cache = CompositeCache::default();
    assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
    assert_eq!(pool.members(), 2, "the mirror is a pool member");
    let _ = worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT);
    let _ = store.enforce_budget(usize::MAX);
    let both = store.resident_len() + worker.mirror_resident();
    assert!(
        both <= total.get() + floor.get(),
        "UI {} + mirror {} resident tiles exceed the pool's {} + {} slack",
        store.resident_len(),
        worker.mirror_resident(),
        total,
        floor
    );
    let have = read_composite(&mut store, &tiles);
    let want = sync_composite(&layers, &mut store, &tiles);
    assert_same(&have, &want, "paging mirror");
    worker.shutdown();
}

/// Resident and peak resident set of this process, in MiB (Linux only).
fn rss_mib() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map_or(0, |kb| kb / 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// MEM-1/BG-2 measurements on the real template: memory over a
/// structural-edit sequence at the default budget, and the first
/// dispatch's UI-thread time at a 2,048-tile budget (half the template
/// paged out).
#[test]
#[ignore = "needs a local PSD named by AURORA_EFFECTS_PSD"]
fn nyepi_mirror_memory_and_first_dispatch_at_2048() {
    let Some(path) = std::env::var_os("AURORA_EFFECTS_PSD") else {
        println!("SKIPPED: AURORA_EFFECTS_PSD not set");
        return;
    };
    let bytes = ok(std::fs::read(&path));
    for total in [16_384_usize, 2_048] {
        let document = ok(open_psd_document(&bytes));
        let dir = ok(tempfile::tempdir());
        let (Some(total_nz), Some(floor)) = (
            std::num::NonZeroUsize::new(total),
            std::num::NonZeroUsize::new(256),
        ) else {
            unreachable!("non-zero");
        };
        let pool = aurora_tile::ResidencyPool::new(total_nz, floor);
        let mut store = ok(aurora_tile::TileStore::new_shared(
            dir.path().to_path_buf(),
            &pool,
        ));
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = replace_document_pixels(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        assert_eq!(failed, 0);
        drop(incoming);
        let mut layers = document.layers;
        let tiles = grid(document.canvas_size);
        let (rss_loaded, _) = rss_mib();
        let mut worker = CompositeWorker::new(noop_waker());
        let mut cache = CompositeCache::default();
        let started = Instant::now();
        assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
        let first_dispatch = started.elapsed();
        let shipped = worker.last_shipped;
        let _ =
            worker.wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT * 5);
        let first_final = started.elapsed();
        let mut peak_tiles = store.resident_len() + worker.mirror_resident();
        let (rss_first, _) = rss_mib();
        let top = top_layer(&layers);
        for opacity in [0.8_f32, 0.6, 0.4] {
            ok(layers.set_opacity(top, opacity));
            cache.bump();
            assert!(worker.dispatch(&mut target(&layers, &mut store, &mut cache), tiles.clone()));
            let _ = worker
                .wait_until_idle(Some(&mut target(&layers, &mut store, &mut cache)), WAIT * 5);
            peak_tiles = peak_tiles.max(store.resident_len() + worker.mirror_resident());
        }
        let (rss_end, hwm) = rss_mib();
        println!(
            "NYEPI mirror @ pool {total}: members {}, share {}; first dispatch {first_dispatch:?} \
             (shipped {shipped}), final {first_final:?}; UI {} + mirror {} resident tiles, peak sum \
             {peak_tiles} ({} MiB counted); RSS loaded {rss_loaded} MiB, after first {rss_first} MiB, \
             after 3 edits {rss_end} MiB, process peak {hwm} MiB",
            pool.members(),
            pool.share(),
            store.resident_len(),
            worker.mirror_resident(),
            peak_tiles / 2
        );
        assert!(
            peak_tiles <= total + 256,
            "UI + mirror within the pool plus one floor"
        );
        worker.shutdown();
    }
}
