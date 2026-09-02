//! Test-only fixtures shared across this crate's own test modules.
//!
//! `#[cfg(test)]`, deliberately not a Cargo feature: nothing outside
//! this crate's own tests needs it, and a feature would put it on the
//! `--all-features` build surface for no benefit (the same reasoning
//! `aurora-doc`'s own `test-support` feature had to justify, and this
//! one cannot).

use std::num::NonZeroUsize;

use aurora_tile::{SurfaceId, TileId, TileStore};

/// The single surface every fixture here uses.
pub fn surface() -> SurfaceId {
    SurfaceId::from_raw(0)
}

/// A store in which `broken` is permanently unreadable and `healthy` is
/// fine — the only portable way to make a `TileStore` read fail on
/// demand from outside `aurora-tile` (the same technique `undo`'s own
/// tests and `aurora-app`'s export-refusal tests already use).
///
/// Budget 2, three tiles touched: `broken` first, then `healthy`, then
/// one far-away filler, so exactly `broken` is evicted and exactly one
/// scratch file exists. [`TileStore::flush`] is the load-bearing call —
/// without it the store's own `pending` map still holds the good bytes
/// and the corrupted file is never read, so the "broken" tile would
/// quietly read back fine.
///
/// The returned `TempDir` must be kept alive for as long as the store
/// is used; dropping it deletes the scratch directory underneath it.
pub fn store_with_a_broken_tile(broken: TileId, healthy: TileId) -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(2) else {
        unreachable!("2 is non-zero");
    };
    let mut store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err:?}"),
    };

    // Order matters: `broken` is touched first, so it is the LRU victim
    // the third touch below evicts. `TileStore::make_room` writes every
    // victim out whether or not it was modified, so no fill is needed.
    for tile in [broken, healthy, TileId { x: 50, y: 50 }] {
        if let Err(err) = store.get_mut(surface(), tile) {
            unreachable!("a fresh store must accept a first touch of {tile:?}: {err:?}");
        }
    }
    if let Err(err) = store.flush() {
        unreachable!("test-local scratch disk must accept the write: {err:?}");
    }
    break_the_only_scratch_file(&dir);
    (dir, store)
}

/// Truncates the one file in `dir` to half its length, leaving a
/// well-formed-but-short ATIL file — exactly what
/// `aurora_tile::codec::decode` rejects, and therefore a tile whose
/// every subsequent read fails rather than only its first (0.52.2's own
/// `TileStore::ensure_resident` fix). The one copy in this crate:
/// [`store_with_a_broken_tile`] builds on it, and `undo`'s own tests
/// call it directly to break a store they built themselves.
pub fn break_the_only_scratch_file(dir: &tempfile::TempDir) {
    let Ok(entries) = std::fs::read_dir(dir.path()) else {
        unreachable!("the scratch directory must be readable");
    };
    // Filtered by extension, not counted raw (0.68.5): a scratch
    // directory may hold `aurora_tile::LOCK_FILE_NAME` beside its tiles,
    // and an exact-one-element destructure over *every* entry would then
    // fail on a file this function has no business truncating. The seven
    // other enumerators in this workspace were hardened this way in
    // 0.67.0; this one was missed.
    let files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tile"))
        .collect();
    let [victim] = files.as_slice() else {
        unreachable!("exactly one tile should have been evicted: {files:?}");
    };
    let Ok(bytes) = std::fs::read(victim) else {
        unreachable!("the evicted tile file must be readable");
    };
    let Some(truncated) = bytes.get(..bytes.len() / 2) else {
        unreachable!("half of a slice's own length is always in range");
    };
    if let Err(err) = std::fs::write(victim, truncated) {
        unreachable!("test-local scratch disk must accept the write: {err:?}");
    }
}
