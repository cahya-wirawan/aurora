//! The sparse tile store: LRU-resident tiles, scratch-disk paging, and
//! per-tile dirty-rectangle tracking.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use aurora_core::Rect;
use lru::LruCache;

use crate::codec;
use crate::error::TileError;
use crate::tile::{Tile, TileId};
use crate::writer::{BackgroundWriter, WriteJob};

/// Counters mirroring `spike/vertical-slice`'s own `Stats` (paging
/// throughput/eviction-cost numbers depend on these being tracked, not
/// just "it works").
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub tiles_created: u64,
    pub evictions: u64,
    pub faults: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
}

/// A sparse, paging, LRU-bounded store of [`Tile`]s.
///
/// Tiles are created lazily on first touch (a read or write of an
/// untouched `TileId` returns/allocates a blank tile) — this is what
/// "sparse" means here: nothing is allocated for the vast majority of a
/// huge, mostly-untouched document.
///
/// **Known limitation, accepted rather than solved here**: a tile's dirty
/// rectangle does not survive eviction. If a tile is evicted while still
/// dirty (its pending changes never consumed, e.g. via GPU upload), that
/// dirty state is lost — the pixel data itself is safely persisted, only
/// the "what changed since last upload" bookkeeping is not. A freshly
/// paged-in tile always starts clean, which is correct *relative to what
/// is on disk*, but a consumer that was relying on an in-flight dirty
/// rect across an eviction would need to re-derive it some other way.
/// Solving this would mean persisting dirty state in the on-disk format,
/// which is real, avoidable complexity for a corner case this milestone
/// does not need to close.
#[derive(Debug)]
pub struct TileStore {
    resident: LruCache<TileId, Tile>,
    paged_out: HashMap<TileId, PathBuf>,
    budget: NonZeroUsize,
    scratch_dir: PathBuf,
    writer: BackgroundWriter,
    stats: Stats,
}

impl TileStore {
    /// Creates a store rooted at `scratch_dir` (created if missing),
    /// holding at most `budget` tiles resident at once (ADR 0005: at the
    /// fixed 256×256 tile size, a tile-count budget is equivalent to a
    /// byte budget).
    ///
    /// # Errors
    ///
    /// Returns [`TileError::ScratchDirUnavailable`] if `scratch_dir`
    /// can't be created.
    pub fn new(scratch_dir: PathBuf, budget: NonZeroUsize) -> Result<Self, TileError> {
        std::fs::create_dir_all(&scratch_dir).map_err(|source| {
            TileError::ScratchDirUnavailable {
                path: scratch_dir.clone(),
                source,
            }
        })?;
        Ok(Self {
            resident: LruCache::new(budget),
            paged_out: HashMap::new(),
            budget,
            scratch_dir,
            writer: BackgroundWriter::spawn(),
            stats: Stats::default(),
        })
    }

    /// Returns the tile at `id`, paging it in or creating it blank if
    /// necessary. Bumps its LRU recency.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if paging in from the scratch disk fails.
    pub fn get(&mut self, id: TileId) -> Result<&Tile, TileError> {
        self.ensure_resident(id)?;
        match self.resident.get(&id) {
            Some(tile) => Ok(tile),
            None => unreachable!("ensure_resident just inserted this id"),
        }
    }

    /// Mutable counterpart of [`Self::get`].
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if paging in from the scratch disk fails.
    pub fn get_mut(&mut self, id: TileId) -> Result<&mut Tile, TileError> {
        self.ensure_resident(id)?;
        match self.resident.get_mut(&id) {
            Some(tile) => Ok(tile),
            None => unreachable!("ensure_resident just inserted this id"),
        }
    }

    /// Takes and clears `id`'s accumulated dirty rectangle, if it is
    /// currently resident and dirty. Returns `None` for a tile that is
    /// not resident, not dirty, or has never been touched.
    pub fn take_dirty(&mut self, id: TileId) -> Option<Rect> {
        self.resident.get_mut(&id).and_then(Tile::take_dirty)
    }

    /// Blocks until every write submitted so far has actually reached
    /// disk (e.g. before a document save) and surfaces the first
    /// failure encountered, if any. Every failure is logged via
    /// `tracing::error!` even though only the first is returned —
    /// dropping the rest silently would contradict the point of
    /// reporting a scratch-disk failure at all.
    ///
    /// # Errors
    ///
    /// Returns the first [`TileError::Io`] encountered among pending
    /// writes, if any failed.
    pub fn flush(&mut self) -> Result<(), TileError> {
        self.writer.flush();
        let mut first_err = None;
        for result in self.writer.drain_results() {
            if let Err(source) = result.outcome {
                tracing::error!(tile = ?result.id, %source, "scratch-disk write failed");
                if first_err.is_none() {
                    first_err = Some(TileError::Io {
                        id: result.id,
                        source,
                    });
                }
            }
        }
        // A fresh writer thread, since `flush` above tore the old one
        // down (`BackgroundWriter::flush` drops the sender and joins) —
        // the store must remain usable for further writes afterward.
        self.writer = BackgroundWriter::spawn();
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    #[must_use]
    pub const fn stats(&self) -> &Stats {
        &self.stats
    }

    #[must_use]
    pub fn resident_len(&self) -> usize {
        self.resident.len()
    }

    fn ensure_resident(&mut self, id: TileId) -> Result<(), TileError> {
        if self.resident.contains(&id) {
            return Ok(());
        }
        if let Some(path) = self.paged_out.remove(&id) {
            self.page_in(id, &path)
        } else {
            self.make_room();
            self.resident.put(id, Tile::blank());
            self.stats.tiles_created += 1;
            Ok(())
        }
    }

    fn page_in(&mut self, id: TileId, path: &Path) -> Result<(), TileError> {
        let bytes = std::fs::read(path).map_err(|source| TileError::Io { id, source })?;
        let texels = codec::decode(&bytes)?;
        self.stats.bytes_read += bytes.len() as u64;
        self.stats.faults += 1;
        self.make_room();
        self.resident.put(id, Tile::from_texels(texels));
        Ok(())
    }

    /// Evicts least-recently-used resident tiles, encoding and handing
    /// each off to the background writer, until there is room for one
    /// more. Encoding (compression) happens here, synchronously, on the
    /// caller's thread — it is fast, in-memory CPU work; only the actual
    /// disk write is offloaded, which is where the real latency is.
    fn make_room(&mut self) {
        while self.resident.len() >= self.budget.get() {
            let Some((victim_id, victim_tile)) = self.resident.pop_lru() else {
                break;
            };
            let bytes = codec::encode(victim_tile.texels());
            let path = self.tile_path(victim_id);
            self.stats.bytes_written += bytes.len() as u64;
            self.stats.evictions += 1;
            self.paged_out.insert(victim_id, path.clone());
            self.writer.submit(WriteJob {
                id: victim_id,
                path,
                bytes,
            });
        }
    }

    fn tile_path(&self, id: TileId) -> PathBuf {
        self.scratch_dir.join(format!("{}_{}.tile", id.x, id.y))
    }
}

#[cfg(test)]
mod tests {
    use super::TileStore;
    use crate::tile::TileId;
    use std::num::NonZeroUsize;

    fn store(budget: usize) -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let Some(budget) = NonZeroUsize::new(budget) else {
            unreachable!("test budgets are always non-zero literals");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
        };
        (dir, store)
    }

    #[test]
    fn first_touch_creates_a_blank_tile() {
        let (_dir, mut store) = store(4);
        let id = TileId { x: 0, y: 0 };
        let tile = match store.get(id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("no prior state exists to fail on: {err}"),
        };
        assert!(tile.texels().iter().all(|s| s.to_f32() == 0.0));
        assert_eq!(store.stats().tiles_created, 1);
    }

    #[test]
    fn eviction_and_page_in_round_trip() {
        let (_dir, mut store) = store(2);
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        let c = TileId { x: 2, y: 0 };

        {
            let tile = match store.get_mut(a) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err}"),
            };
            let samples = tile.texels_mut();
            if let Some(first) = samples.first_mut() {
                *first = half::f16::from_f32(0.5);
            }
        }
        if let Err(err) = store.get_mut(b) {
            unreachable!("{err}");
        }
        // Budget is 2; touching `c` must evict the least-recently-used
        // resident tile (`a`, touched first).
        if let Err(err) = store.get_mut(c) {
            unreachable!("{err}");
        }
        assert_eq!(store.resident_len(), 2);
        assert_eq!(store.stats().evictions, 1);

        // Paging `a` back in must reproduce exactly what was written --
        // bit-exact through compression, same property FINDINGS.md
        // proved for the spike's uncompressed format.
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
        let a_again = match store.get(a) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err}"),
        };
        let Some(first) = a_again.texels().first() else {
            unreachable!("a tile's texel buffer is never empty");
        };
        // Exact comparison is correct, not fragile, here: 0.5 has an
        // exact binary representation in both f16 and f32, so this isn't
        // the "accumulated rounding error" case clippy::float_cmp warns
        // about -- it's the same bit-exact-round-trip property
        // spike/FINDINGS.md already proved for the uncompressed format.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(first.to_f32(), 0.5);
        }
        assert_eq!(store.stats().faults, 1);
    }

    #[test]
    fn dirty_rect_is_taken_and_clears() {
        let (_dir, mut store) = store(4);
        let id = TileId { x: 0, y: 0 };
        let rect = aurora_core::Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        if let Ok(tile) = store.get_mut(id) {
            tile.mark_dirty(rect);
        }
        assert_eq!(store.take_dirty(id), Some(rect));
        assert_eq!(store.take_dirty(id), None);
    }
}
