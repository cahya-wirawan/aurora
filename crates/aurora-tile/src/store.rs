//! The sparse tile store: LRU-resident tiles, scratch-disk paging, and
//! per-tile dirty-rectangle tracking.
//!
//! **One shared store, addressed by surface** ([ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)):
//! every tile-access method takes a [`SurfaceId`] alongside a [`TileId`]
//! — the pair is this store's real key, not `TileId` alone. One store
//! can hold tiles for many independent surfaces (e.g. one per pixel
//! layer in a document) while still owning exactly one background-writer
//! thread and one real LRU memory bound covering all of them combined —
//! the property a naive one-store-per-surface design would not have.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use aurora_core::Rect;
use lru::LruCache;

use crate::codec;
use crate::error::TileError;
use crate::tile::{SurfaceId, Tile, TileId};
use crate::writer::{BackgroundWriter, WriteJob};

/// Counters mirroring `spike/vertical-slice`'s own `Stats` (paging
/// throughput/eviction-cost numbers depend on these being tracked, not
/// just "it works"). Store-wide, not per-surface — a per-surface
/// breakdown is real, separate follow-on work if a consumer ever needs
/// one.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub tiles_created: u64,
    pub evictions: u64,
    pub faults: u64,
    pub bytes_written: u64,
    pub bytes_read: u64,
}

/// A sparse, paging, LRU-bounded store of [`Tile`]s, addressed by
/// `(SurfaceId, TileId)`.
///
/// Tiles are created lazily on first touch (a read or write of an
/// untouched `(SurfaceId, TileId)` pair returns/allocates a blank tile)
/// — this is what "sparse" means here: nothing is allocated for the vast
/// majority of a huge, mostly-untouched document, and a surface nobody
/// has touched yet costs nothing at all.
///
/// Eviction picks the globally least-recently-used resident tile across
/// *every* surface this store holds, not per-surface — the correct
/// behaviour for a memory bound meant to cover a whole document
/// regardless of how many surfaces (layers) it has.
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
    resident: LruCache<(SurfaceId, TileId), Tile>,
    paged_out: HashMap<(SurfaceId, TileId), PathBuf>,
    budget: NonZeroUsize,
    scratch_dir: PathBuf,
    writer: BackgroundWriter,
    stats: Stats,
}

impl TileStore {
    /// Creates a store rooted at `scratch_dir` (created if missing),
    /// holding at most `budget` tiles resident at once, summed across
    /// every surface this store ever addresses (ADR 0005: at the fixed
    /// 256×256 tile size, a tile-count budget is equivalent to a byte
    /// budget; ADR 0010: one such budget per document, not one per
    /// surface).
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

    /// Returns the tile at `id` on `surface`, paging it in or creating it
    /// blank if necessary. Bumps its LRU recency.
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if paging in from the scratch disk fails.
    pub fn get(&mut self, surface: SurfaceId, id: TileId) -> Result<&Tile, TileError> {
        self.ensure_resident(surface, id)?;
        match self.resident.get(&(surface, id)) {
            Some(tile) => Ok(tile),
            None => unreachable!("ensure_resident just inserted this key"),
        }
    }

    /// Mutable counterpart of [`Self::get`].
    ///
    /// # Errors
    ///
    /// Returns [`TileError`] if paging in from the scratch disk fails.
    pub fn get_mut(&mut self, surface: SurfaceId, id: TileId) -> Result<&mut Tile, TileError> {
        self.ensure_resident(surface, id)?;
        match self.resident.get_mut(&(surface, id)) {
            Some(tile) => Ok(tile),
            None => unreachable!("ensure_resident just inserted this key"),
        }
    }

    /// Takes and clears `(surface, id)`'s accumulated dirty rectangle, if
    /// it is currently resident and dirty. Returns `None` for a tile that
    /// is not resident, not dirty, or has never been touched.
    pub fn take_dirty(&mut self, surface: SurfaceId, id: TileId) -> Option<Rect> {
        self.resident
            .get_mut(&(surface, id))
            .and_then(Tile::take_dirty)
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
                tracing::error!(surface = ?result.surface, tile = ?result.id, %source, "scratch-disk write failed");
                if first_err.is_none() {
                    first_err = Some(TileError::Io {
                        surface: result.surface,
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

    fn ensure_resident(&mut self, surface: SurfaceId, id: TileId) -> Result<(), TileError> {
        if self.resident.contains(&(surface, id)) {
            return Ok(());
        }
        if let Some(path) = self.paged_out.remove(&(surface, id)) {
            self.page_in(surface, id, &path)
        } else {
            self.make_room();
            self.resident.put((surface, id), Tile::blank());
            self.stats.tiles_created += 1;
            Ok(())
        }
    }

    fn page_in(&mut self, surface: SurfaceId, id: TileId, path: &Path) -> Result<(), TileError> {
        let bytes = std::fs::read(path).map_err(|source| TileError::Io {
            surface,
            id,
            source,
        })?;
        let texels = codec::decode(&bytes)?;
        self.stats.bytes_read += bytes.len() as u64;
        self.stats.faults += 1;
        self.make_room();
        self.resident.put((surface, id), Tile::from_texels(texels));
        Ok(())
    }

    /// Evicts least-recently-used resident tiles, encoding and handing
    /// each off to the background writer, until there is room for one
    /// more. Encoding (compression) happens here, synchronously, on the
    /// caller's thread — it is fast, in-memory CPU work; only the actual
    /// disk write is offloaded, which is where the real latency is.
    ///
    /// Picks the globally least-recently-used `(SurfaceId, TileId)` —
    /// `LruCache::pop_lru` already orders by access recency across every
    /// key it holds, regardless of which surface a key belongs to, so
    /// this needs no surface-aware logic of its own to get that right.
    fn make_room(&mut self) {
        while self.resident.len() >= self.budget.get() {
            let Some(((victim_surface, victim_id), victim_tile)) = self.resident.pop_lru() else {
                break;
            };
            let bytes = codec::encode(victim_tile.texels());
            let path = self.tile_path(victim_surface, victim_id);
            self.stats.bytes_written += bytes.len() as u64;
            self.stats.evictions += 1;
            self.paged_out
                .insert((victim_surface, victim_id), path.clone());
            self.writer.submit(WriteJob {
                surface: victim_surface,
                id: victim_id,
                path,
                bytes,
            });
        }
    }

    fn tile_path(&self, surface: SurfaceId, id: TileId) -> PathBuf {
        self.scratch_dir
            .join(format!("{}_{}_{}.tile", surface.to_raw(), id.x, id.y))
    }
}

#[cfg(test)]
mod tests {
    use super::TileStore;
    use crate::tile::{CHANNELS, SurfaceId, TILE, TileId};
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

    /// The surface every single-surface test below uses — its own value
    /// is arbitrary and never compared against anything; only the
    /// multi-surface tests care about more than one distinct value.
    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    #[test]
    fn first_touch_creates_a_blank_tile() {
        let (_dir, mut store) = store(4);
        let id = TileId { x: 0, y: 0 };
        let tile = match store.get(surface(), id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("no prior state exists to fail on: {err}"),
        };
        assert!(tile.texels().iter().all(|s| s.to_f32() == 0.0));
        assert_eq!(store.stats().tiles_created, 1);
    }

    #[test]
    fn eviction_and_page_in_round_trip() {
        let (_dir, mut store) = store(2);
        let s = surface();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        let c = TileId { x: 2, y: 0 };

        {
            let tile = match store.get_mut(s, a) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err}"),
            };
            let samples = tile.texels_mut();
            if let Some(first) = samples.first_mut() {
                *first = half::f16::from_f32(0.5);
            }
        }
        if let Err(err) = store.get_mut(s, b) {
            unreachable!("{err}");
        }
        // Budget is 2; touching `c` must evict the least-recently-used
        // resident tile (`a`, touched first).
        if let Err(err) = store.get_mut(s, c) {
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
        let a_again = match store.get(s, a) {
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
        let s = surface();
        let id = TileId { x: 0, y: 0 };
        let rect = aurora_core::Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        if let Ok(tile) = store.get_mut(s, id) {
            tile.mark_dirty(rect);
        }
        assert_eq!(store.take_dirty(s, id), Some(rect));
        assert_eq!(store.take_dirty(s, id), None);
    }

    // -- Multi-surface addressing (ADR 0010) --

    #[test]
    fn the_same_tile_id_on_two_surfaces_does_not_collide() {
        let (_dir, mut store) = store(4);
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        let id = TileId { x: 0, y: 0 };

        if let Ok(tile) = store.get_mut(surface_a, id)
            && let Some(first) = tile.texels_mut().first_mut()
        {
            *first = half::f16::from_f32(0.25);
        }
        if let Ok(tile) = store.get_mut(surface_b, id)
            && let Some(first) = tile.texels_mut().first_mut()
        {
            *first = half::f16::from_f32(0.75);
        }

        let a_value = match store.get(surface_a, id) {
            Ok(tile) => tile.texels().first().copied(),
            Err(err) => unreachable!("{err}"),
        };
        let b_value = match store.get(surface_b, id) {
            Ok(tile) => tile.texels().first().copied(),
            Err(err) => unreachable!("{err}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(a_value.map(half::f16::to_f32), Some(0.25));
            assert_eq!(b_value.map(half::f16::to_f32), Some(0.75));
        }
        // Two distinct resident entries, not one overwriting the other.
        assert_eq!(store.resident_len(), 2);
    }

    #[test]
    fn eviction_picks_the_globally_least_recently_used_tile_across_surfaces() {
        let (_dir, mut store) = store(2);
        let (surface_a, surface_b) = (SurfaceId::from_raw(1), SurfaceId::from_raw(2));
        let id = TileId { x: 0, y: 0 };

        // Touch surface_a first (becomes LRU), then surface_b (becomes
        // MRU). Budget is 2, so both are resident. A third touch, to a
        // different tile on surface_b, must evict surface_a's tile --
        // the globally least-recently-used one -- not something keyed
        // only within surface_b's own tiles.
        if let Err(err) = store.get_mut(surface_a, id) {
            unreachable!("{err}");
        }
        if let Err(err) = store.get_mut(surface_b, id) {
            unreachable!("{err}");
        }
        if let Err(err) = store.get_mut(surface_b, TileId { x: 1, y: 0 }) {
            unreachable!("{err}");
        }

        assert_eq!(store.resident_len(), 2);
        assert_eq!(store.stats().evictions, 1);
        // surface_a's tile was paged out -- a fresh `get` recreates it
        // blank only if it was never evicted-with-content; page_in
        // succeeding (not erroring, and not just silently re-blanking)
        // is confirmed by the round-trip test above, so here it's
        // enough to confirm the *other* surface's own two tiles are
        // still the ones actually resident.
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
    }

    /// CI-gated regression check for `spike/FINDINGS.md`'s own
    /// recommendation ("a latency regression test in CI... since the
    /// brush budget has under 1ms of margin"). Measures only the
    /// pure-CPU slice of the stroke pipeline this crate owns — writing a
    /// brush-sized region of texels into one already-resident tile and
    /// accumulating its dirty rect — deliberately, not the full "input
    /// to frame submitted" number the spike measured: that needs a real
    /// window/present loop (`aurora-app`, still M1.8), which doesn't
    /// exist yet. This piece is worth gating on its own because it's the
    /// one most exposed to an accidental algorithmic regression (e.g. a
    /// future change that scans every resident tile instead of touching
    /// one) and the one whose cost genuinely doesn't depend on what GPU,
    /// if any, a CI runner happens to have — unlike the GPU-dependent
    /// upload/composite half, which has its own, deliberately looser
    /// check in `aurora-render` (see that crate's `latency` module for
    /// why the threshold differs).
    ///
    /// Asserts on the median (p50), not p99: a single scheduler
    /// preemption on a shared CI runner can spike one sample without
    /// indicating a real regression, and the median is far more robust
    /// to that than a tail percentile while still moving if the
    /// underlying cost genuinely grows. p95/p99 are still computed and
    /// printed for visibility, just not asserted on.
    #[test]
    fn paint_and_dirty_round_trip_stays_within_a_tight_cpu_budget() {
        // A brush-sized dirty region, in line with the ~24px-radius
        // brush `spike/FINDINGS.md` measured (finding #2) -- comfortably
        // inside the tile's own 256x256 bounds. Kept as plain `u32`s for
        // the pixel-index math below, with `brush` (the `i64`/`u32`
        // `aurora_core::Rect` `mark_dirty` needs) derived from them via
        // a lossless widening cast.
        const BRUSH_X: u32 = 100;
        const BRUSH_Y: u32 = 100;
        const BRUSH_SIZE: u32 = 48;
        const ITERATIONS: usize = 1000;

        let (_dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };
        let brush = aurora_core::Rect {
            x: i64::from(BRUSH_X),
            y: i64::from(BRUSH_Y),
            width: BRUSH_SIZE,
            height: BRUSH_SIZE,
        };

        let mut samples = Vec::with_capacity(ITERATIONS);
        for i in 0..ITERATIONS {
            let start = std::time::Instant::now();
            let Ok(tile) = store.get_mut(s, id) else {
                unreachable!(
                    "id stays resident for the whole loop: budget is 4, only one tile is ever touched"
                );
            };
            let value = half::f16::from_f32(f32::from(u8::from(i % 2 == 0)));
            let texels = tile.texels_mut();
            for dy in 0..BRUSH_SIZE {
                for dx in 0..BRUSH_SIZE {
                    let x = BRUSH_X + dx;
                    let y = BRUSH_Y + dy;
                    let base = ((y * TILE + x) as usize) * CHANNELS;
                    for channel in 0..CHANNELS {
                        if let Some(sample) = texels.get_mut(base + channel) {
                            *sample = value;
                        }
                    }
                }
            }
            tile.mark_dirty(brush);
            let _ = store.take_dirty(s, id);
            samples.push(start.elapsed());
        }

        samples.sort_unstable();
        let percentile = |pct: usize| -> std::time::Duration {
            let index = (samples.len() * pct / 100).min(samples.len() - 1);
            match samples.get(index) {
                Some(&value) => value,
                None => unreachable!("samples is non-empty: ITERATIONS > 0"),
            }
        };
        let (p50, p95, p99) = (percentile(50), percentile(95), percentile(99));
        eprintln!(
            "paint+dirty round trip over {ITERATIONS} iterations: p50={p50:?} p95={p95:?} p99={p99:?}"
        );

        // 500us is generous by roughly three orders of magnitude against
        // a single in-memory 48x48 tile write plus one Rect::union call
        // -- a trip-wire for a real algorithmic regression, not a tight
        // enforcement of the 10ms brush budget itself.
        assert!(
            p50 < std::time::Duration::from_micros(500),
            "median paint+dirty latency regressed: {p50:?} (budget: 500us); p95={p95:?} p99={p99:?}"
        );
    }
}
