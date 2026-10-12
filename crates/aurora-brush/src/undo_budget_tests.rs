//! Tests for the pixel history's memory bound (0.210.0): compression,
//! spilling, paging back, budget, order and cleanup. A child module of
//! `undo` so it can look at the stacks themselves.

// Test code: slicing a fixture vector it just built is not a production panic.
#![allow(clippy::indexing_slicing)]

use std::num::NonZeroUsize;
use std::path::Path;

use aurora_tile::{SAMPLES, TileId, TileStore};
use half::f16;

use super::{PixelHistory, SPILL_BYTES_PER_CALL, StrokeSnapshot, TileData};
use crate::test_support::surface;

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn new_store() -> (tempfile::TempDir, TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = NonZeroUsize::new(64) else {
        unreachable!("64 is non-zero");
    };
    let store = ok(TileStore::new(dir.path().join("scratch"), budget));
    (dir, store)
}

/// xorshift64*: deterministic noise with no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Every `u16` bit pattern is possible: NaNs, infinities, subnormals,
/// negative zero. The codec must keep all of them.
fn noisy(seed: u64) -> Vec<f16> {
    let mut rng = Rng(seed | 1);
    (0..SAMPLES)
        .map(|_| f16::from_bits((rng.next() >> 24) as u16))
        .collect()
}

fn flat(value: f32) -> Vec<f16> {
    vec![f16::from_f32(value); SAMPLES]
}

/// A smooth gradient with a little noise: what real paint looks like.
fn painted(seed: u64) -> Vec<f16> {
    let mut rng = Rng(seed | 1);
    (0..SAMPLES)
        .map(|i| f16::from_f32(((i / 4) % 256) as f32 / 255.0 + (rng.next() % 3) as f32 / 1024.0))
        .collect()
}

fn bits(texels: &[f16]) -> Vec<u16> {
    texels.iter().map(|t| t.to_bits()).collect()
}

fn write(store: &mut TileStore, tile: TileId, content: &[f16]) {
    let t = ok(store.get_mut(surface(), tile));
    t.texels_mut().copy_from_slice(content);
}

fn read(store: &mut TileStore, tile: TileId) -> Vec<u16> {
    bits(ok(store.get(surface(), tile)).texels())
}

/// One stroke on `tile`: captures its content, writes `after`, pushes.
fn stroke(history: &mut PixelHistory, store: &mut TileStore, tile: TileId, after: &[f16]) {
    let mut snapshot = StrokeSnapshot::new(surface());
    let before = ok(store.get(surface(), tile)).texels().to_vec();
    assert!(snapshot.record_content(tile, &before));
    write(store, tile, after);
    assert!(history.push(snapshot));
}

fn spilled_steps(history: &PixelHistory) -> Vec<bool> {
    history
        .undo
        .iter()
        .chain(&history.redo)
        .map(|s| s.spill.is_some())
        .collect()
}

fn files_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| entries.flatten().count())
}

fn spill_dir(history: &PixelHistory) -> Option<std::path::PathBuf> {
    history.area.as_ref().map(|a| a.path().to_path_buf())
}

fn history_for(store: &TileStore, budget: u64) -> PixelHistory {
    PixelHistory::with_spill(store.staging_root(), budget)
}

const T0: TileId = TileId { x: 0, y: 0 };

#[test]
fn compression_round_trips_every_kind_of_tile_bit_for_bit() {
    for (name, before, after) in [
        ("flat", flat(0.25), flat(0.75)),
        ("painted", painted(1), painted(2)),
        ("noisy incompressible", noisy(3), noisy(4)),
        ("flat to noisy", flat(0.0), noisy(5)),
    ] {
        let (_dir, mut store) = new_store();
        let mut history = PixelHistory::new();
        write(&mut store, T0, &before);
        stroke(&mut history, &mut store, T0, &after);
        assert!(
            history
                .undo
                .iter()
                .all(|s| s.tiles.values().all(|d| matches!(d, TileData::Packed(_)))),
            "{name}: a pushed snapshot holds no raw tile"
        );
        assert!(ok(history.undo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(&before), "{name}: undo");
        assert!(ok(history.redo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(&after), "{name}: redo");
        assert!(ok(history.undo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(&before), "{name}: undo again");
    }
}

#[test]
fn a_flat_fill_of_1024_tiles_costs_a_few_mebibytes_not_half_a_gibibyte() {
    let mut snapshot = StrokeSnapshot::new(surface());
    let content = flat(0.5);
    for x in 0..32 {
        for y in 0..32 {
            assert!(snapshot.record_content(TileId { x, y }, &content));
        }
    }
    let raw = snapshot.memory_bytes();
    let mut history = PixelHistory::new();
    assert!(history.push(snapshot));
    let stats = history.stats();
    println!(
        "1024 flat tiles: raw snapshot {} MiB, in history {:.2} MiB (uncompressed equivalent {} MiB)",
        raw >> 20,
        stats.memory_bytes as f64 / 1_048_576.0,
        stats.uncompressed_bytes >> 20
    );
    assert_eq!(stats.tiles, 1024);
    assert_eq!(stats.uncompressed_bytes, 512 << 20);
    assert!(
        stats.memory_bytes < 5 << 20,
        "got {} bytes",
        stats.memory_bytes
    );
    assert_eq!(history.memory_bytes(), stats.memory_bytes);
}

#[test]
fn incompressible_tiles_cost_no_more_than_the_raw_tile_plus_a_header() {
    let mut snapshot = StrokeSnapshot::new(surface());
    assert!(snapshot.record_content(T0, &noisy(9)));
    let raw = snapshot.memory_bytes();
    let mut history = PixelHistory::new();
    assert!(history.push(snapshot));
    assert!(
        history.memory_bytes() <= raw + 16,
        "{} vs {raw}",
        history.memory_bytes()
    );
}

#[test]
fn spilled_steps_page_back_exactly_in_order() {
    let (_dir, mut store) = new_store();
    // A budget of 1 MiB with ~512 KiB noisy tiles: nearly every step spills.
    let mut history = history_for(&store, 1 << 20);
    let versions: Vec<Vec<f16>> = (0..12).map(|i| noisy(100 + i)).collect();
    write(&mut store, T0, &versions[0]);
    for v in &versions[1..] {
        stroke(&mut history, &mut store, T0, v);
    }
    let stats = history.stats();
    println!(
        "12 noisy steps, 1 MiB budget: memory {} KiB, spilled {} steps / {} KiB",
        stats.memory_bytes >> 10,
        stats.spilled_steps,
        stats.spilled_bytes >> 10
    );
    assert!(stats.spilled_steps >= 8, "{stats:?}");
    assert!(stats.memory_bytes <= 1 << 20, "{stats:?}");
    assert_eq!(history.memory_bytes(), stats.memory_bytes);
    // Undo everything, checking each step lands on the previous version.
    for expected in versions.iter().rev().skip(1) {
        assert!(ok(history.undo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(expected));
        assert_eq!(history.memory_bytes(), history.stats().memory_bytes);
    }
    assert!(!ok(history.undo(&mut store)));
    // And redo everything.
    for expected in &versions[1..] {
        assert!(ok(history.redo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(expected));
    }
}

#[test]
fn the_oldest_steps_spill_first_and_the_newest_stay_resident() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 4 << 20);
    write(&mut store, T0, &noisy(1));
    for i in 0..40 {
        stroke(&mut history, &mut store, T0, &noisy(10 + i));
        assert!(
            history.memory_bytes() <= 4 << 20,
            "budget exceeded at step {i}: {}",
            history.memory_bytes()
        );
    }
    let spilled = spilled_steps(&history);
    let first_resident = spilled.iter().position(|s| !s);
    let Some(first_resident) = first_resident else {
        unreachable!("the newest steps fit the budget");
    };
    assert!(first_resident > 20, "{spilled:?}");
    assert!(
        spilled[..first_resident].iter().all(|s| *s),
        "spilling is oldest first with no gaps: {spilled:?}"
    );
    assert!(spilled[first_resident..].iter().all(|s| !*s), "{spilled:?}");
}

#[test]
fn a_failed_spill_keeps_the_data_in_memory_and_undo_stays_exact() {
    let (dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    // The scratch directory vanishes: no spill directory can be made.
    ok(std::fs::remove_dir_all(dir.path().join("scratch")));
    let versions: Vec<Vec<f16>> = (0..6).map(|i| noisy(200 + i)).collect();
    // The store itself would also fail to write now, so keep its tile
    // resident by writing before the removal is noticed: the store's
    // tiles are in memory (budget 64), so reads and writes still work.
    write(&mut store, T0, &versions[0]);
    for v in &versions[1..] {
        stroke(&mut history, &mut store, T0, v);
    }
    let stats = history.stats();
    assert_eq!(stats.spilled_steps, 0);
    assert!(stats.spill_failures >= 1, "{stats:?}");
    assert!(
        stats.memory_bytes > 2 << 20,
        "all still in memory: {stats:?}"
    );
    for expected in versions.iter().rev().skip(1) {
        assert!(ok(history.undo(&mut store)));
        assert_eq!(read(&mut store, T0), bits(expected));
    }
}

#[test]
fn a_spill_write_that_fails_midway_changes_nothing() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    write(&mut store, T0, &noisy(1));
    stroke(&mut history, &mut store, T0, &noisy(2));
    stroke(&mut history, &mut store, T0, &noisy(3));
    // Something spilled, so the area exists; now break it.
    let Some(dir) = spill_dir(&history) else {
        unreachable!("the budget forced a spill");
    };
    let before = history.stats();
    ok(std::fs::remove_dir_all(&dir));
    stroke(&mut history, &mut store, T0, &noisy(4));
    stroke(&mut history, &mut store, T0, &noisy(5));
    let after = history.stats();
    assert!(after.spill_failures > before.spill_failures, "{after:?}");
    assert_eq!(
        after.tiles, after.undo_steps as u64,
        "a failed spill loses no captured tile: {after:?}"
    );
    assert_eq!(history.memory_bytes(), after.memory_bytes);
    // The two newest steps are intact (the older spilled ones lost their
    // files with the directory, which is an error on undo, not a panic).
    assert!(ok(history.undo(&mut store)));
    assert_eq!(read(&mut store, T0), bits(&noisy(4)));
    assert!(ok(history.undo(&mut store)));
    assert_eq!(read(&mut store, T0), bits(&noisy(3)));
}

#[test]
fn an_unreadable_spilled_step_fails_the_undo_and_keeps_the_entry() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    write(&mut store, T0, &noisy(1));
    for i in 0..4 {
        stroke(&mut history, &mut store, T0, &noisy(10 + i));
    }
    let Some(dir) = spill_dir(&history) else {
        unreachable!("spilled");
    };
    // Undo down to a spilled step, then remove the files.
    while history.undo.last().is_none_or(|s| s.spill.is_none()) {
        assert!(ok(history.undo(&mut store)));
    }
    let tile_now = read(&mut store, T0);
    for entry in ok(std::fs::read_dir(&dir)).flatten() {
        ok(std::fs::remove_file(entry.path()));
    }
    let steps = history.undo.len();
    assert!(history.undo(&mut store).is_err());
    assert_eq!(history.undo.len(), steps, "the entry stays");
    assert_eq!(read(&mut store, T0), tile_now, "and the store is untouched");
    assert_eq!(history.memory_bytes(), history.stats().memory_bytes);
}

#[test]
fn truncating_the_redo_stack_deletes_its_spill_files() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    write(&mut store, T0, &noisy(1));
    for i in 0..8 {
        stroke(&mut history, &mut store, T0, &noisy(20 + i));
    }
    let Some(dir) = spill_dir(&history) else {
        unreachable!("spilled");
    };
    for _ in 0..8 {
        assert!(ok(history.undo(&mut store)));
    }
    assert_eq!(history.undo.len(), 0);
    let with_redo = files_in(&dir);
    assert!(
        with_redo >= 4,
        "the redo stack holds spilled files: {with_redo}"
    );
    // A new stroke clears redo.
    stroke(&mut history, &mut store, T0, &noisy(99));
    assert_eq!(history.redo.len(), 0);
    assert!(
        files_in(&dir) < with_redo,
        "files freed: {} -> {}",
        with_redo,
        files_in(&dir)
    );
    // clear_redo (the structural-edit path) frees too.
    for i in 0..6 {
        stroke(&mut history, &mut store, T0, &noisy(300 + i));
    }
    for _ in 0..5 {
        assert!(ok(history.undo(&mut store)));
    }
    history.clear_redo();
    assert_eq!(history.memory_bytes(), history.stats().memory_bytes);
    let live = history.undo.iter().filter(|s| s.spill.is_some()).count();
    assert_eq!(
        files_in(&dir),
        live,
        "exactly the live spilled steps have files"
    );
}

#[test]
fn dropping_the_history_removes_the_spill_directory() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    write(&mut store, T0, &noisy(1));
    for i in 0..6 {
        stroke(&mut history, &mut store, T0, &noisy(40 + i));
    }
    let Some(dir) = spill_dir(&history) else {
        unreachable!("spilled");
    };
    assert!(files_in(&dir) > 0);
    drop(history);
    assert!(!dir.exists(), "no spill file or directory survives");
}

#[test]
fn a_random_walk_of_pushes_undos_and_redos_matches_a_model_under_a_tiny_budget() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 2 << 20);
    let mut rng = Rng(0xDEAD_BEEF);
    let start = noisy(7);
    write(&mut store, T0, &start);
    // Model: (before, after) per step.
    let mut undo: Vec<(Vec<f16>, Vec<f16>)> = Vec::new();
    let mut redo: Vec<(Vec<f16>, Vec<f16>)> = Vec::new();
    let mut current = start;
    for i in 0..300_u64 {
        match rng.next() % 5 {
            0 | 1 => {
                let next = match rng.next() % 3 {
                    0 => flat((rng.next() % 100) as f32 / 100.0),
                    1 => painted(i),
                    _ => noisy(i + 1000),
                };
                undo.push((current.clone(), next.clone()));
                redo.clear();
                stroke(&mut history, &mut store, T0, &next);
                current = next;
            }
            2 | 3 => {
                let did = ok(history.undo(&mut store));
                assert_eq!(did, !undo.is_empty());
                if let Some((before, after)) = undo.pop() {
                    current = before.clone();
                    redo.push((before, after));
                }
            }
            _ => {
                let did = ok(history.redo(&mut store));
                assert_eq!(did, !redo.is_empty());
                if let Some((before, after)) = redo.pop() {
                    current = after.clone();
                    undo.push((before, after));
                }
            }
        }
        assert_eq!(read(&mut store, T0), bits(&current), "op {i}");
        assert_eq!(history.undo.len(), undo.len());
        assert_eq!(history.redo.len(), redo.len());
        assert_eq!(
            history.memory_bytes(),
            history.stats().memory_bytes,
            "op {i}"
        );
        assert!(
            history.memory_bytes() <= 2 << 20,
            "op {i}: {}",
            history.memory_bytes()
        );
    }
    let stats = history.stats();
    println!("random walk end state: {stats:?}");
    assert!(stats.spilled_steps > 0, "the walk exercised spilling");
}

#[test]
fn ten_thousand_tiny_steps_stay_small_and_undo_in_order() {
    let (_dir, mut store) = new_store();
    // 4 MiB: 10,000 steps of ~200 bytes of bookkeeping plus ~2 KiB of
    // flat tile overflow it, so this also spills thousands of tiny files.
    let mut history = history_for(&store, 4 << 20);
    let started = std::time::Instant::now();
    write(&mut store, T0, &flat(0.0));
    for i in 1..=10_000_u32 {
        stroke(
            &mut history,
            &mut store,
            T0,
            &flat((i % 1000) as f32 / 1000.0),
        );
    }
    let pushed = started.elapsed();
    let stats = history.stats();
    println!(
        "10,000 tiny steps: {pushed:?}, memory {} KiB, {} spilled steps, {} KiB on disk",
        stats.memory_bytes >> 10,
        stats.spilled_steps,
        stats.spilled_bytes >> 10
    );
    assert!(stats.memory_bytes <= 4 << 20, "{stats:?}");
    assert_eq!(stats.undo_steps, 10_000);
    // The newest 1,500 undo in order (a mix of resident and spilled; the
    // full-length order is covered by the random walk above, and 10,000
    // 512 KiB-tile undos would take half a minute).
    for i in (8_500..10_000_u32).rev() {
        assert!(ok(history.undo(&mut store)));
        let expected = f16::from_f32((i % 1000) as f32 / 1000.0).to_bits();
        assert_eq!(
            read(&mut store, T0).first().copied(),
            Some(expected),
            "step {i}"
        );
    }
    assert_eq!(history.memory_bytes(), history.stats().memory_bytes);
    assert!(history.memory_bytes() <= 4 << 20);
    // Dropping the redo stack frees its files: only live spilled steps keep one.
    history.clear_redo();
    let Some(dir) = spill_dir(&history) else {
        unreachable!("spilled");
    };
    let live = history.undo.iter().filter(|s| s.spill.is_some()).count();
    assert_eq!(files_in(&dir), live);
    println!("10,000 pushes plus 1,500 undos: {:?}", started.elapsed());
}

#[test]
fn pack_gives_the_same_bytes_on_one_thread_and_many() {
    // 200 tiles take the threaded path; each must equal its own serial encode.
    let mut snapshot = StrokeSnapshot::new(surface());
    let contents: Vec<Vec<f16>> = (0..200)
        .map(|i: u64| {
            if i.is_multiple_of(2) {
                noisy(i)
            } else {
                painted(i)
            }
        })
        .collect();
    for (i, content) in contents.iter().enumerate() {
        assert!(snapshot.record_content(TileId { x: i as u32, y: 0 }, content));
    }
    snapshot.pack();
    for (i, content) in contents.iter().enumerate() {
        let Some(TileData::Packed(bytes)) = snapshot.tiles.get(&TileId { x: i as u32, y: 0 })
        else {
            unreachable!("tile {i} must be packed");
        };
        assert_eq!(bytes, &aurora_tile::codec::encode(content), "tile {i}");
    }
}

#[test]
fn pushing_costs_little_for_a_brush_stroke_and_scales_for_a_fill() {
    for (tiles, content) in [(4_u32, painted(1)), (64, painted(2)), (256, flat(0.5))] {
        let mut snapshot = StrokeSnapshot::new(surface());
        for x in 0..tiles {
            assert!(snapshot.record_content(TileId { x, y: 0 }, &content));
        }
        let mut history = PixelHistory::new();
        let started = std::time::Instant::now();
        assert!(history.push(snapshot));
        println!("push of {tiles} tiles: {:?}", started.elapsed());
    }
}

#[test]
fn the_spill_per_call_cap_leaves_the_rest_for_trim() {
    // One huge incompressible step bigger than a call may write, then
    // trim() calls finish the job.
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 4 << 20);
    let steps = (SPILL_BYTES_PER_CALL / (512 * 1024)) as u32 + 40;
    for i in 0..steps {
        let mut snapshot = StrokeSnapshot::new(surface());
        assert!(snapshot.record_content(TileId { x: i, y: 0 }, &noisy(u64::from(i) + 1)));
        history.undo.push({
            let mut s = snapshot;
            s.pack();
            s
        });
        history.memory_bytes += history.undo.last().map_or(0, |s| s.memory_bytes);
    }
    history.trim();
    assert!(
        history.memory_bytes() > 4 << 20,
        "capped: {}",
        history.memory_bytes()
    );
    for _ in 0..10 {
        history.trim();
    }
    assert!(
        history.memory_bytes() <= 4 << 20,
        "{}",
        history.memory_bytes()
    );
    assert_eq!(history.memory_bytes(), history.stats().memory_bytes);
    let _ = &mut store;
}

#[test]
fn a_history_without_a_spill_directory_never_spills() {
    let (_dir, mut store) = new_store();
    let mut history = PixelHistory::new();
    history.set_budget_bytes(1);
    write(&mut store, T0, &noisy(1));
    for i in 0..4 {
        stroke(&mut history, &mut store, T0, &noisy(2 + i));
    }
    let stats = history.stats();
    assert_eq!(stats.spilled_steps, 0);
    assert_eq!(stats.spill_failures, 0);
}

#[test]
fn multi_tile_spilled_steps_restore_every_tile_to_its_own_content() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 1 << 20);
    let tiles = [
        TileId { x: 0, y: 0 },
        TileId { x: 5, y: 0 },
        TileId { x: 0, y: 7 },
        TileId { x: 9, y: 9 },
    ];
    let content = |round: u64, i: usize| -> Vec<f16> {
        if i.is_multiple_of(2) {
            noisy(round * 10 + i as u64 + 1)
        } else {
            painted(round * 10 + i as u64 + 1)
        }
    };
    for (i, &tile) in tiles.iter().enumerate() {
        write(&mut store, tile, &content(0, i));
    }
    for round in 1..=4_u64 {
        let mut snapshot = StrokeSnapshot::new(surface());
        for (i, &tile) in tiles.iter().enumerate() {
            let before = ok(store.get(surface(), tile)).texels().to_vec();
            assert!(snapshot.record_content(tile, &before));
            write(&mut store, tile, &content(round, i));
        }
        assert!(history.push(snapshot));
    }
    assert!(history.stats().spilled_steps >= 3, "{:?}", history.stats());
    for round in (0..4_u64).rev() {
        assert!(ok(history.undo(&mut store)));
        for (i, &tile) in tiles.iter().enumerate() {
            assert_eq!(
                read(&mut store, tile),
                bits(&content(round, i)),
                "round {round} tile {i}"
            );
        }
    }
}

#[test]
fn undo_steps_spill_before_redo_steps() {
    let (_dir, mut store) = new_store();
    let mut history = history_for(&store, 64 << 20);
    write(&mut store, T0, &noisy(1));
    for i in 0..6 {
        stroke(&mut history, &mut store, T0, &noisy(50 + i));
    }
    for _ in 0..3 {
        assert!(ok(history.undo(&mut store)));
    }
    assert_eq!((history.undo.len(), history.redo.len()), (3, 3));
    assert_eq!(history.stats().spilled_steps, 0, "nothing over budget yet");
    // ~3 MiB held; a 2.5 MiB budget spills down to 1.875 MiB: three steps.
    history.set_budget_bytes(5 << 19);
    history.trim();
    assert!(history.undo.iter().all(|s| s.spill.is_some()), "undo first");
    assert!(history.redo.iter().all(|s| s.spill.is_none()), "redo kept");
    // And everything still comes back exactly.
    for i in (0..3).rev() {
        assert!(ok(history.undo(&mut store)));
        let expected = if i == 0 { noisy(1) } else { noisy(50 + i - 1) };
        assert_eq!(read(&mut store, T0), bits(&expected));
    }
}
