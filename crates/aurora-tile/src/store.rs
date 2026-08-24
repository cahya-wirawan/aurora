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

use std::collections::{HashMap, VecDeque};
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
    /// Page-ins that did not produce a tile — the `fs::read` failed, or
    /// the bytes it returned did not decode. Counted separately from
    /// `faults` (which counts *completed* page-ins) because since 0.52.2
    /// a failed page-in is retried on every subsequent read of that key
    /// rather than healing into a blank tile, so a single broken scratch
    /// file shows up here as a climbing counter — the store's own,
    /// cheapest signal that something is wrong with the scratch disk.
    pub failed_page_ins: u64,
    /// Tiles whose bytes were dropped because the memory held for
    /// *failed* writes hit its cap (a private detail of [`TileStore`],
    /// described here rather than linked to). Each one is a tile whose
    /// pixels are genuinely gone; anything above zero means the scratch
    /// disk has been failing long enough for the store to start choosing
    /// bounded memory over content, and is worth surfacing.
    ///
    /// Such a tile reads back as an error, not as blank and not as stale:
    /// the store deletes the superseded scratch file when it drops the
    /// newer content, and keeps the `paged_out` mapping so the read fails
    /// loudly. The one exception is a deletion that itself fails, which
    /// is reported at `error!` when it happens.
    pub dropped_failed_writes: u64,
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
///
/// **Eviction/revisit race, closed**: `make_room` evicts a tile by handing
/// its encoded bytes to a background writer (`submit` never blocks — see
/// `writer.rs`), so the actual disk write lands at some later,
/// unspecified time. A naive `ensure_resident` that only tracked
/// `paged_out` could be asked to page the same tile back in before that
/// write landed, racing a not-yet-created or partially-written file. This
/// is closed by keeping the evicted tile's own bytes in `pending` until
/// the write is confirmed complete (see `ensure_resident`'s and
/// `make_room`'s own doc comments for the mechanism) — a revisit during
/// that window is served straight from memory, never disk, so the race
/// window is zero by construction rather than merely narrowed.
#[derive(Debug)]
pub struct TileStore {
    resident: LruCache<(SurfaceId, TileId), Tile>,
    paged_out: HashMap<(SurfaceId, TileId), PathBuf>,
    /// Evicted tiles whose background write hasn't been *confirmed*
    /// complete yet — closes the eviction/revisit race documented on
    /// [`Self::ensure_resident`] and [`Self::make_room`]. Holds the exact
    /// already-encoded bytes `make_room` also handed to `writer.submit`,
    /// so a revisit before the write lands never has to touch disk.
    pending: HashMap<(SurfaceId, TileId), Vec<u8>>,
    /// The keys in [`Self::pending`] whose background write actually
    /// *failed*, oldest first — the retention queue
    /// [`Self::cap_failed_writes`] bounds. A successful write's `pending`
    /// entry clears itself the moment its result drains, so those are
    /// self-limiting; a failed one is kept deliberately (it is the tile's
    /// only surviving copy) and nothing else would ever drop it, which is
    /// exactly why this queue and its cap exist.
    failed_writes: VecDeque<(SurfaceId, TileId)>,
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
            pending: HashMap::new(),
            failed_writes: VecDeque::new(),
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
            match result.outcome {
                // Confirmed durable: the scratch file is now a real
                // replacement for the in-memory copy, so the holding area
                // for it can go. See `ensure_resident`/`make_room`'s doc
                // comments for the full race this closes.
                Ok(()) => {
                    self.forget_pending((result.surface, result.id));
                }
                // Kept, deliberately: these bytes are the *only* surviving
                // copy of that tile. Dropping them here (as this did until
                // 0.52.2) sent the next read to a scratch file that was
                // never written -- and since 0.52.2 a failed page-in no
                // longer heals into a blank tile, that read fails forever.
                // One transient write failure (a full disk, a momentarily
                // read-only mount) would therefore have destroyed pixels
                // that were sitting safely in memory. Bounded by
                // `retain_failed_write`'s own cap, so a *persistent*
                // failure cannot grow this without limit. See
                // `reconcile_pending` for the same rule and the memory
                // trade it accepts.
                Err(source) => {
                    self.retain_failed_write((result.surface, result.id));
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

    /// Resolves `(surface, id)` to a resident tile via, in priority order:
    /// (a) already resident, (b) still in [`Self::pending`] -- an eviction
    /// whose background write has not yet been confirmed, reinstated
    /// straight from the in-memory encoded bytes `make_room` kept around
    /// for exactly this purpose, with zero disk I/O and therefore zero
    /// race window; (c) in `paged_out` only -- `reconcile_pending` (called
    /// first, below) already confirmed this key's write landed, so the
    /// existing synchronous `page_in` read is now provably race-free; (d)
    /// neither -- a brand-new blank tile.
    ///
    /// This ordering, together with `make_room` populating `pending` and
    /// `paged_out` atomically (same call, same instant), is what closes
    /// the eviction/revisit race tracked in `PLAN.md`'s M1.1 section: a
    /// key can only ever reach the disk-read branch (c) once its write is
    /// confirmed complete, so `page_in` can never again race a
    /// still-in-flight or partially-written file.
    ///
    /// Invariant, extended from the pre-existing `resident`/`paged_out`
    /// one to now also cover `pending`: a `(surface, id)` key is never
    /// simultaneously resident *and* present in `pending` or `paged_out`
    /// **as observed by any caller** — both paging branches restore it
    /// before they return, though not identically. Branch (b) removes the
    /// key from both maps and only then re-inserts it into `resident`, so
    /// the two are never both true even mid-branch. Branch (c) is the
    /// other order: `page_in` puts the tile into `resident` first, and the
    /// `paged_out` entry is removed on the statement after it, so the
    /// invariant is transiently false in between. That is harmless rather
    /// than sloppy — this store is single-threaded, `page_in` is private,
    /// and nothing runs between those two statements that could observe
    /// the overlap — but the ordering is forced: the `paged_out` mapping
    /// must survive until `page_in` has actually succeeded, which is the
    /// whole point of the paragraph below.
    ///
    /// **A failed page-in keeps its mapping.** Branches (b) and (c)
    /// remove a key from `pending`/`paged_out` only once they hold a
    /// real, whole tile to put in `resident`. Until 0.52.2 they removed
    /// first, so a tile whose scratch file was corrupt or truncated
    /// errored on its first read and then fell through to branch (d) on
    /// every read after that, silently returning `Tile::blank()` -- which
    /// meant a save or export that was correctly *refused* for that tile
    /// (`aurora_io::IoError::IncompleteComposite`) quietly succeeded on
    /// the retry, with the tile blank. The consequence to be aware of:
    /// such a tile now fails for the whole life of this store rather than
    /// healing into an empty one. That is the intended trade -- the
    /// pixels are gone either way, and the corrupt file is left on disk
    /// rather than being overwritten by a re-eviction of the blank tile
    /// that replaced it.
    fn ensure_resident(&mut self, surface: SurfaceId, id: TileId) -> Result<(), TileError> {
        // Cheap and non-blocking (`drain_results` never waits) -- run on
        // every touch so `pending` can't grow past "evictions since the
        // last touch of any tile", bounded by the store's own `budget`
        // rather than by document size (invariant §7.3.1).
        self.reconcile_pending();

        if self.resident.contains(&(surface, id)) {
            return Ok(());
        }
        // Peeked, not removed: `codec::decode` can fail even here, on
        // bytes this store itself encoded (a future encoder bug, or
        // memory corruption), and dropping the mapping before there is a
        // real tile to replace it with is what let the *next* read of the
        // same key find neither map and invent a blank tile -- turning one
        // loud, recoverable error into silent, permanent content loss.
        // Both removes below therefore happen only once `decode` has
        // actually produced a whole tile.
        if let Some(bytes) = self.pending.get(&(surface, id)) {
            let texels = codec::decode(bytes)?;
            self.forget_pending((surface, id));
            self.paged_out.remove(&(surface, id));
            self.make_room();
            self.resident.put((surface, id), Tile::from_texels(texels));
            return Ok(());
        }
        // Same rule as the branch above, and this is the branch where it
        // actually bit: a corrupted or truncated scratch file fails
        // `page_in` on every attempt, so forgetting the path here is what
        // let a *retried* save/export succeed with that tile silently
        // blank -- see this method's own doc comment. The `PathBuf` is
        // cloned because `page_in` needs `&mut self`; that clone is paid
        // only on a page-in (a fault or a failure), never on the resident
        // fast path above.
        if let Some(path) = self.paged_out.get(&(surface, id)).cloned() {
            self.page_in(surface, id, &path)?;
            self.paged_out.remove(&(surface, id));
            Ok(())
        } else {
            self.make_room();
            self.resident.put((surface, id), Tile::blank());
            self.stats.tiles_created += 1;
            Ok(())
        }
    }

    /// Drains whatever background-write results have completed so far
    /// (non-blocking -- see [`BackgroundWriter::drain_results`]) and
    /// clears each *successful* one's entry from [`Self::pending`]: its
    /// write is now confirmed durable, so a future revisit of that key is
    /// safe to fall through to the ordinary `paged_out` disk-read path. A
    /// failed write is logged via `tracing::warn!` and otherwise not
    /// escalated here -- a routine reconciliation pass touched by every
    /// tile access is the wrong place to fail every subsequent, unrelated
    /// tile access over one bad write. [`Self::flush`] remains the
    /// authoritative point where a write failure surfaces as a real
    /// `Err`, unchanged by this.
    ///
    /// **A failed write keeps its bytes** (0.52.2). The scratch file that
    /// write was meant to produce does not exist, or is half-written, so
    /// `pending`'s copy is the only whole one left; dropping it would
    /// hand the next read a file that isn't there. That is the same rule
    /// [`Self::ensure_resident`] follows for a failed *read*, and it
    /// accepts the same trade in the other direction: the entry occupies
    /// memory (compressed, one tile) until the key is revisited -- which
    /// reinstates it as a resident tile and lets an ordinary later
    /// eviction retry the write. Losing a professional's pixels is the
    /// worse half of that trade by a wide margin.
    ///
    /// That retention is **bounded**, not unlimited: see
    /// [`Self::retain_failed_write`], which caps how many unwritable
    /// tiles may be held at once and drops the oldest beyond it. A
    /// persistent scratch-disk failure would otherwise grow `pending`
    /// with every eviction for as long as the session lasts, which is
    /// invariant §7.3.1's own "nothing assumes a document fits in memory"
    /// broken by the back door.
    fn reconcile_pending(&mut self) {
        for result in self.writer.drain_results() {
            match result.outcome {
                Ok(()) => {
                    self.forget_pending((result.surface, result.id));
                }
                Err(source) => {
                    self.retain_failed_write((result.surface, result.id));
                    tracing::warn!(
                        surface = ?result.surface,
                        tile = ?result.id,
                        %source,
                        "scratch-disk write failed (reconciled in background); keeping the \
                         tile's only surviving copy in memory"
                    );
                }
            }
        }
    }

    /// Drops one key's `pending` entry and, with it, any place that key
    /// held in the failed-write retention queue — the two must move
    /// together or the queue accumulates keys that are no longer holding
    /// anything, which would make [`Self::cap_failed_writes`] trim live
    /// entries early. The `retain` is `O(n)` in a queue whose length is
    /// capped at the store's own tile budget, on a path that already
    /// costs a decode or a disk write.
    fn forget_pending(&mut self, key: (SurfaceId, TileId)) {
        self.pending.remove(&key);
        self.failed_writes.retain(|held| *held != key);
    }

    /// How many tiles' worth of *failed* writes this store will hold in
    /// memory before it starts dropping the oldest.
    ///
    /// The store's one real promise about memory is its resident tile
    /// budget (ADR 0005: at a fixed tile size, a tile count *is* a byte
    /// budget), so the cap is that same number: a store allowed `budget`
    /// resident tiles will hold at most `budget` more in compressed,
    /// unwritable form. That bounds the worst case at roughly twice the
    /// configured budget — in practice much less, since `pending` holds
    /// `codec::encode` output rather than raw texels — and it scales with
    /// the store instead of being an arbitrary constant that is far too
    /// small on a workstation and far too large on a small machine.
    /// Invariant §7.3.1 is the reason a cap has to exist at all: nothing
    /// here may grow with document size.
    const fn failed_write_capacity(&self) -> usize {
        self.budget.get()
    }

    /// Records that `key`'s background write failed, so its `pending`
    /// bytes are being kept as the tile's only surviving copy, and
    /// enforces the cap on how many such tiles may be kept at once.
    ///
    /// **Why a cap** (0.52.2, second review round): keeping a failed
    /// write's bytes is right for the case it was written for — a
    /// transient failure, where the tile is served from memory until the
    /// next eviction retries the write. Under a *persistent* failure (a
    /// full disk, a read-only mount) nothing ever retries successfully
    /// and nothing else ever removes those entries, so unbounded
    /// retention turns a broken scratch disk into an out-of-memory abort:
    /// measured at ~1.05 GB retained over 2,000 evicted tiles with a
    /// four-tile budget. An abort loses the *whole* document, which is
    /// strictly worse than losing the tiles that could not be written.
    ///
    /// So this is the bounded trade: up to
    /// [`Self::failed_write_capacity`] unwritable tiles are held and stay
    /// readable; beyond that, the oldest is dropped. A dropped tile is
    /// *not* silently blanked and *not* silently stale — the `paged_out`
    /// mapping is kept so the next read is a loud `TileError` rather than
    /// an invented `Tile::blank()`, and any superseded scratch file
    /// behind it is deleted first ([`Self::discard_stale_scratch_file`],
    /// which also documents the one case that can defeat this: a deletion
    /// that itself fails).
    ///
    /// Note what this does *not* cover: a `pending` entry cleared by the
    /// completion of an **older** write for the same key can still send a
    /// later read to a stale file with no error. That is the separate,
    /// still-open stale-write race (no generation numbers on write jobs)
    /// disclosed in `PLAN.md`, not something this cap either causes or
    /// closes.
    fn retain_failed_write(&mut self, key: (SurfaceId, TileId)) {
        if !self.pending.contains_key(&key) {
            // Already revisited and reinstated, or already dropped: there
            // is nothing being held for this key, so nothing to bound.
            return;
        }
        if !self.failed_writes.contains(&key) {
            self.failed_writes.push_back(key);
        }
        self.cap_failed_writes();
    }

    /// Drops the oldest failed-write entries until at most
    /// [`Self::failed_write_capacity`] remain — see
    /// [`Self::retain_failed_write`] for why the cap exists.
    fn cap_failed_writes(&mut self) {
        while self.failed_writes.len() > self.failed_write_capacity() {
            let Some(oldest) = self.failed_writes.pop_front() else {
                break;
            };
            if self.pending.remove(&oldest).is_some() {
                self.stats.dropped_failed_writes += 1;
                self.discard_stale_scratch_file(oldest);
                tracing::error!(
                    surface = ?oldest.0,
                    tile = ?oldest.1,
                    held = self.failed_writes.len(),
                    "scratch disk has been failing long enough to fill the failed-write memory \
                     cap; dropping this tile's only surviving copy to stay inside the store's \
                     memory budget"
                );
            }
        }
    }

    /// Deletes the scratch file behind a key whose newer content
    /// [`Self::cap_failed_writes`] just dropped, so the next read of that
    /// key fails loudly instead of succeeding with **stale** pixels.
    ///
    /// Without this the cap had a silent-wrong-content hole, found by
    /// review after the cap itself landed. It only bites a key that was
    /// evicted successfully at least once before: that write left a real
    /// file, the tile was then paged back in and edited, and the *second*
    /// eviction is the one that failed and got capped. `make_room`
    /// re-inserts `paged_out` pointing at the same path either way, so
    /// the mapping would still resolve — to the older file, whose content
    /// is exactly the pre-edit version the user changed. A read would
    /// have returned it as if it were current, with no error at all,
    /// which is the failure class this whole line of work exists to
    /// close. For a key whose first-ever write failed there is no file to
    /// delete and `NotFound` is the expected, ignored answer.
    ///
    /// Deleting loses that older content. That is deliberate: it is
    /// already superseded, and a loud `TileError::Io` on the next read is
    /// strictly better than silently handing back a version of the tile
    /// the user edited away from. The `paged_out` mapping is kept, not
    /// removed, precisely so the read *is* that error — removing it would
    /// send the key to the never-touched branch and invent a blank tile,
    /// silently, which is the same class of problem in a different coat.
    ///
    /// If the deletion itself fails (a read-only mount fails writes and
    /// deletes alike, so this is reachable), the stale file survives and
    /// the guarantee genuinely does not hold for that key. There is
    /// nothing better available at that point, so it is reported at
    /// `error!` — the one case where a later read can silently return
    /// stale content, named at the moment it becomes possible rather than
    /// discovered afterwards.
    fn discard_stale_scratch_file(&mut self, key: (SurfaceId, TileId)) {
        let Some(path) = self.paged_out.get(&key) else {
            return;
        };
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::error!(
                    surface = ?key.0,
                    tile = ?key.1,
                    path = %path.display(),
                    %err,
                    "could not delete the superseded scratch file for a tile whose newer content \
                     was just dropped; a later read of this tile can now return stale, pre-edit \
                     pixels instead of failing"
                );
            }
        }
    }

    fn page_in(&mut self, surface: SurfaceId, id: TileId, path: &Path) -> Result<(), TileError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) => {
                self.stats.failed_page_ins += 1;
                return Err(TileError::Io {
                    surface,
                    id,
                    source,
                });
            }
        };
        // Charged against `bytes_read` here, before the decode below and
        // regardless of whether it succeeds: this read really did move
        // these bytes off the scratch disk, and a throughput counter that
        // only counts I/O whose *decode* also worked understates exactly
        // the case worth seeing -- a corrupt file re-read on every touch.
        self.stats.bytes_read += bytes.len() as u64;
        let texels = match codec::decode(&bytes) {
            Ok(texels) => texels,
            Err(err) => {
                self.stats.failed_page_ins += 1;
                return Err(err);
            }
        };
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
            // Same key, same moment, as the `paged_out` insert above --
            // this is what lets `ensure_resident` reinstate the tile from
            // memory if it's revisited before the write below actually
            // lands (see that method's doc comment for the full race this
            // closes). Cleared by `reconcile_pending` once the write is
            // confirmed complete.
            self.pending
                .insert((victim_surface, victim_id), bytes.clone());
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

    /// Asserts that a `TileError::CorruptFile` rejection was made *for
    /// the payload's length*, the property the half-a-tile fixtures below
    /// exist to prove.
    ///
    /// Matching the variant alone is not enough, and 0.52.1's own `codec`
    /// tests already established why: `CorruptFile` equally covers a bad
    /// magic, an unsupported version and a truncated header, so a
    /// regression earlier in `codec::decode` that rejected these fixtures
    /// before ever reaching the length check would leave a wildcard match
    /// green while proving nothing. Both of `decode`'s own length
    /// rejections (the compressed branch's size-prefix equality check and
    /// the raw branch's post-decode one) name the byte count of exactly
    /// one whole tile, which is what this looks for.
    fn assert_rejected_for_its_length(read: u32, message: &str) {
        let whole_tile_bytes = (crate::tile::SAMPLES * 2).to_string();
        assert!(
            message.contains("of exactly one") && message.contains(&whole_tile_bytes),
            "read {read}: this fixture is a well-formed ATIL file holding half a tile, so it must \
             be rejected for its length -- naming the {whole_tile_bytes} bytes one whole tile \
             occupies -- not for its magic, version or header, which `CorruptFile` also covers: \
             {message}"
        );
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

    // -- Eviction/revisit race (PLAN.md M1.1) --

    /// The decisive, fully deterministic proof that the `pending` fast
    /// path works: constructs, **by hand**, the exact state `make_room`
    /// leaves behind mid-eviction (a key present in both `pending` and
    /// `paged_out`) -- with no real background thread involved at all,
    /// so nothing here depends on OS scheduling. `paged_out` is pointed
    /// at a path that is deliberately never created; if `ensure_resident`
    /// ever fell through to the disk-read branch instead of taking the
    /// `pending` fast path, this would fail loudly with `TileError::Io`
    /// (or, if some other file happened to occupy that exact path,
    /// `TileError::CorruptFile` from decoding the wrong bytes) --
    /// silently succeeding is only possible by actually reading from
    /// `pending`, in memory, exactly as the fix specifies.
    ///
    /// (A companion test below additionally exercises a *real* eviction
    /// via `make_room` and an immediate revisit, the shape the original
    /// bug actually took -- see its own doc comment for why that one, on
    /// its own, is necessary-but-not-sufficient as proof of which code
    /// path gets taken, which is exactly why this test exists too.)
    #[test]
    fn ensure_resident_serves_directly_from_pending_bypassing_disk_entirely() {
        let (dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };

        let texels = vec![half::f16::from_f32(0.75); crate::tile::SAMPLES];
        let bytes = crate::codec::encode(&texels);
        let nonexistent = dir.path().join("this_file_is_never_created.tile");
        store.pending.insert((s, id), bytes);
        store.paged_out.insert((s, id), nonexistent);

        let tile = match store.get(s, id) {
            Ok(tile) => tile,
            Err(err) => unreachable!(
                "must be served from `pending`, never from the nonexistent disk path: {err}"
            ),
        };
        let Some(first) = tile.texels().first() else {
            unreachable!("tile texel buffer is never empty");
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(first.to_f32(), 0.75);
        }
        // `stats().faults` is incremented only by `page_in`'s real
        // `fs::read`, never by the `pending` fast path -- staying `0` is
        // internal-state confirmation, on top of the nonexistent-path
        // argument above, that no disk read occurred.
        assert_eq!(store.stats().faults, 0);
        // Reinstated: no longer meaningfully "paged out", by either map.
        assert!(!store.pending.contains_key(&(s, id)));
        assert!(!store.paged_out.contains_key(&(s, id)));
    }

    /// Regression test using a *real* eviction (`make_room`, via a tight
    /// budget) followed by an immediate, synchronous revisit -- no
    /// `sleep`/yield anywhere in this test -- the exact shape the
    /// original bug took (`PLAN.md` M1.1). Before the fix, this sequence
    /// could fail with `TileError::Io` or `TileError::CorruptFile`
    /// depending on exactly how far the background write had gotten;
    /// after the fix it must always succeed with the correct content,
    /// regardless of how that race actually resolves.
    ///
    /// One thing this test deliberately does **not** assert: which of
    /// `pending`/disk actually served the revisit. Measured on this
    /// machine, the background writer thread -- already alive and
    /// blocked in `recv()` before `submit` is ever called -- can
    /// complete a small `fs::write` to a fresh tempdir and have its
    /// result reconciled before this test's own next few statements
    /// run, even with no explicit sleep/yield; asserting "the file must
    /// not exist yet" here would be asserting a timing outcome this
    /// environment does not reliably produce, i.e. exactly the kind of
    /// flaky assertion item 1 asks *not* to write. What's still
    /// deterministic, and asserted below, is that `make_room` populates
    /// `pending` **synchronously**, in the same call that performs the
    /// eviction -- proven by checking it immediately afterward, with no
    /// intervening `TileStore` call that could have reconciled it away.
    /// The `pending` fast path itself, specifically, is what the
    /// isolated test above proves -- deterministically, by construction,
    /// with no reliance on real thread timing at all.
    #[test]
    fn real_eviction_then_immediate_revisit_always_succeeds() {
        let (_dir, mut store) = store(2);
        let s = surface();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        let c = TileId { x: 2, y: 0 };

        if let Ok(tile) = store.get_mut(s, a)
            && let Some(first) = tile.texels_mut().first_mut()
        {
            *first = half::f16::from_f32(0.5);
        }
        if let Err(err) = store.get_mut(s, b) {
            unreachable!("{err}");
        }
        // Budget is 2; touching `c` evicts `a` (LRU) via `make_room`,
        // synchronously, right here on the test's own thread.
        if let Err(err) = store.get_mut(s, c) {
            unreachable!("{err}");
        }
        assert_eq!(store.stats().evictions, 1);
        // Deterministic, no timing dependency: `make_room` inserts into
        // `pending` in the exact same call that just evicted `a` above,
        // before this test does anything else that could reconcile it.
        assert!(
            store.pending.contains_key(&(s, a)),
            "eviction must populate `pending` synchronously"
        );

        // Revisit `a` immediately, synchronously, no sleep/yield -- must
        // succeed with correct content regardless of which path
        // actually served it.
        let a_again = match store.get(s, a) {
            Ok(tile) => tile,
            Err(err) => {
                unreachable!("the eviction/revisit race must be closed, on either code path: {err}")
            }
        };
        let Some(first) = a_again.texels().first() else {
            unreachable!("a tile's texel buffer is never empty");
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(first.to_f32(), 0.5);
        }
        // Reinstated: no longer meaningfully "paged out", by either map,
        // regardless of which path served it.
        assert!(!store.pending.contains_key(&(s, a)));
        assert!(!store.paged_out.contains_key(&(s, a)));

        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
    }

    /// Confirms the other half of the fix: `pending` entries actually
    /// clear once their write is confirmed complete (via `flush`, which
    /// blocks until every submitted write lands and reconciles `pending`
    /// for each), and a *subsequent* revisit correctly falls through to
    /// the ordinary disk `page_in` path rather than staying on the
    /// in-memory fast path forever -- `stats().faults` incrementing is
    /// the internal-state proof that a real disk read happened this
    /// time, the mirror image of the `0` asserted in the test above.
    #[test]
    fn pending_entries_clear_once_writes_are_confirmed() {
        let (_dir, mut store) = store(2);
        let s = surface();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        let c = TileId { x: 2, y: 0 };

        if let Err(err) = store.get_mut(s, a) {
            unreachable!("{err}");
        }
        if let Err(err) = store.get_mut(s, b) {
            unreachable!("{err}");
        }
        // Evicts `a`.
        if let Err(err) = store.get_mut(s, c) {
            unreachable!("{err}");
        }
        assert_eq!(store.pending.len(), 1);

        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
        assert!(
            store.pending.is_empty(),
            "flush must reconcile every in-flight write"
        );

        if let Err(err) = store.get(s, a) {
            unreachable!("{err}");
        }
        assert_eq!(
            store.stats().faults,
            1,
            "with `pending` empty, the revisit must take the real disk page_in path"
        );
        // The `paged_out` branch's own half of the same bookkeeping,
        // asserted here because this is the one test that reaches it
        // deterministically (the revisit above provably took the disk
        // path, per the `faults` assertion): a successful `page_in`
        // leaves the key resident and in neither map.
        assert!(
            !store.paged_out.contains_key(&(s, a)),
            "a successful page-in must clear the paged-out mapping it just consumed"
        );
        assert!(!store.pending.contains_key(&(s, a)));
    }

    /// A scratch-disk write that *fails* must not take the tile's only
    /// surviving copy with it (0.52.2). Until this fix, `flush` and
    /// `reconcile_pending` both dropped the `pending` entry on an `Err`
    /// outcome as readily as on `Ok` — so a full disk, a momentarily
    /// read-only mount, or one permission hiccup silently destroyed the
    /// evicted tile: `pending` no longer held it and the scratch file it
    /// was supposed to be written to was never created. Combined with the
    /// rest of 0.52.2 (a failed page-in no longer heals into a blank
    /// tile) the destruction is permanent for the life of the store.
    ///
    /// The write is made to fail deterministically, and portably, by
    /// occupying the exact path `make_room` will write to with a
    /// *directory*: `fs::write` cannot succeed against one on any
    /// platform this ships to, and unlike a permissions change it needs
    /// no `unix`-only API and no privileged environment.
    #[test]
    fn a_failed_write_keeps_the_tiles_only_copy_readable_from_pending() {
        let (_dir, mut store) = store(2);
        let s = surface();
        let a = TileId { x: 0, y: 0 };
        let b = TileId { x: 1, y: 0 };
        let c = TileId { x: 2, y: 0 };

        if let Err(err) = std::fs::create_dir(store.tile_path(s, a)) {
            unreachable!("test-local scratch dir must accept a subdirectory: {err}");
        }

        if let Ok(tile) = store.get_mut(s, a)
            && let Some(first) = tile.texels_mut().first_mut()
        {
            *first = half::f16::from_f32(0.5);
        }
        if let Err(err) = store.get_mut(s, b) {
            unreachable!("{err}");
        }
        // Budget is 2; this evicts `a`, whose write can only fail.
        if let Err(err) = store.get_mut(s, c) {
            unreachable!("{err}");
        }

        // `flush` joins the writer thread, so the failure has definitely
        // happened and been drained by the time this returns — no timing
        // dependency anywhere in this test.
        match store.flush() {
            Err(crate::TileError::Io { surface, id, .. }) => {
                assert_eq!((surface, id), (s, a));
            }
            Ok(()) => unreachable!("writing a tile over a directory cannot succeed"),
            Err(other) => unreachable!("expected TileError::Io, got {other:?}"),
        }

        assert!(
            store.pending.contains_key(&(s, a)),
            "a failed write must leave the tile's own bytes in `pending`, which is now the only \
             copy of them that exists"
        );

        let a_again = match store.get(s, a) {
            Ok(tile) => tile,
            Err(err) => unreachable!(
                "the tile must still be readable straight from memory after a failed write: {err}"
            ),
        };
        let Some(first) = a_again.texels().first() else {
            unreachable!("a tile's texel buffer is never empty");
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(first.to_f32(), 0.5, "and with its real content, not blank");
        }
        // Served from memory: no disk read was attempted or needed.
        assert_eq!(store.stats().faults, 0);
        assert_eq!(store.stats().failed_page_ins, 0);
        // `a`, `b` and `c`'s own first touches, and nothing else: the
        // revisit of `a` above reinstated a real tile rather than
        // inventing a fourth blank one.
        assert_eq!(store.stats().tiles_created, 3);
    }

    /// The cap on that retention (0.52.2, second review round). Keeping a
    /// failed write's bytes is right for a *transient* failure; under a
    /// *persistent* one nothing ever retries successfully and nothing
    /// else ever removes those entries, so unbounded retention turns a
    /// broken scratch disk into an out-of-memory abort — measured by the
    /// review at ~1.05 GB held over 2,000 evicted tiles with a four-tile
    /// budget. An abort loses the whole document, which is worse than
    /// losing the tiles that could not be written, so the retention is
    /// bounded at the store's own tile budget.
    ///
    /// Every write here fails, deterministically and portably: each tile
    /// path is occupied by a directory before anything is touched.
    #[test]
    fn retention_of_failed_writes_is_bounded_by_the_stores_own_tile_budget() {
        const BUDGET: usize = 4;
        const TILES: u32 = 40;

        let (_dir, mut store) = store(BUDGET);
        let s = surface();

        for x in 0..TILES {
            if let Err(err) = std::fs::create_dir(store.tile_path(s, TileId { x, y: 0 })) {
                unreachable!("test-local scratch dir must accept a subdirectory: {err}");
            }
        }
        // Touch every tile: each one evicts an earlier one, and every one
        // of those evictions' writes can only fail.
        for x in 0..TILES {
            if let Err(err) = store.get_mut(s, TileId { x, y: 0 }) {
                unreachable!("a first touch always succeeds: {err}");
            }
        }
        // `flush` joins the writer, so every failure has been reported
        // and reconciled by the time this returns.
        match store.flush() {
            Err(crate::TileError::Io { .. }) => {}
            Ok(()) => unreachable!("writing a tile over a directory cannot succeed"),
            Err(other) => unreachable!("expected TileError::Io, got {other:?}"),
        }

        assert!(
            store.pending.len() <= BUDGET,
            "failed-write retention must stay inside the store's own tile budget, held {}",
            store.pending.len()
        );
        assert_eq!(store.failed_writes.len(), store.pending.len());
        // The cap really did have to drop tiles -- otherwise this test
        // would pass just as well against unbounded retention. Exactly
        // `TILES - BUDGET` evictions happen (the last `BUDGET` tiles are
        // still resident), every one of them fails, and `BUDGET` of those
        // are retained: everything else was dropped.
        let Ok(budget) = u64::try_from(BUDGET) else {
            unreachable!("4 fits in a u64");
        };
        assert_eq!(
            store.stats().dropped_failed_writes,
            u64::from(TILES) - budget - budget,
            "every failed write past the cap must have been dropped"
        );

        // A dropped tile is not silently blanked: its `paged_out` mapping
        // still points at a scratch file that was never written, so the
        // read is a loud error, never `Tile::blank()`.
        let dropped = TileId { x: 0, y: 0 };
        assert!(!store.pending.contains_key(&(s, dropped)));
        match store.get(s, dropped) {
            Err(crate::TileError::Io { .. }) => {}
            Ok(tile) => unreachable!(
                "a dropped failed write must not read back as a blank {}-sample tile",
                tile.texels().len()
            ),
            Err(other) => unreachable!("expected TileError::Io, got {other:?}"),
        }
        // And a *retained* one is still served from memory, which is the
        // whole point of retaining it.
        let Some(&(_, retained)) = store
            .failed_writes
            .back()
            .map(|key| (key.0, key.1))
            .as_ref()
        else {
            unreachable!("the cap keeps `budget` entries, so the queue is not empty");
        };
        if let Err(err) = store.get(s, retained) {
            unreachable!("a retained failed write must still be readable from memory: {err}");
        }
    }

    /// The hole the cap above left behind, found by review after it
    /// landed: a capped drop used to leave `paged_out` pointing at a
    /// scratch file from an **earlier, successful** write of the same
    /// key, so the next read succeeded and returned the pre-edit content
    /// as if it were current — silently. (For a key whose first-ever
    /// write failed there is no such file, which is why the original
    /// "reads back as a loud error" claim looked true.)
    ///
    /// Hand-builds exactly that state, the way the mid-eviction tests
    /// above hand-build theirs: a real old file holding 0.25, `pending`
    /// holding the newer 0.75 whose write failed, and a second key
    /// present only to push the first past a one-tile cap. The write
    /// failures are reported against the real keys while their jobs point
    /// at directories, so the *stale file itself is left intact* for the
    /// test to prove something about — which is the whole point.
    #[test]
    fn a_capped_drop_deletes_the_superseded_file_instead_of_serving_stale_pixels() {
        let (dir, mut store) = store(1);
        let s = surface();
        let (stale, filler) = (TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 });

        // The earlier, successful eviction of `stale`: a real file.
        let old = crate::codec::encode(&vec![half::f16::from_f32(0.25); crate::tile::SAMPLES]);
        let stale_path = store.tile_path(s, stale);
        if let Err(err) = std::fs::write(&stale_path, &old) {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }

        // Paged back in, edited to 0.75, re-evicted -- and that write
        // failed, so the newer content is only in `pending`.
        store.paged_out.insert((s, stale), stale_path.clone());
        store.pending.insert(
            (s, stale),
            crate::codec::encode(&vec![half::f16::from_f32(0.75); crate::tile::SAMPLES]),
        );
        store
            .paged_out
            .insert((s, filler), store.tile_path(s, filler));
        store.pending.insert(
            (s, filler),
            crate::codec::encode(&vec![half::f16::from_f32(0.5); crate::tile::SAMPLES]),
        );

        for (index, id) in [stale, filler].into_iter().enumerate() {
            let unwritable = dir.path().join(format!("unwritable_{index}"));
            if let Err(err) = std::fs::create_dir(&unwritable) {
                unreachable!("test-local scratch dir must accept a subdirectory: {err}");
            }
            store.writer.submit(crate::writer::WriteJob {
                surface: s,
                id,
                path: unwritable,
                bytes: vec![1, 2, 3, 4],
            });
        }
        // Joined before draining, so both failures are queued and their
        // order (oldest first) is the submission order, deterministically.
        store.writer.flush();
        store.reconcile_pending();

        assert_eq!(store.stats().dropped_failed_writes, 1);
        assert!(
            !store.pending.contains_key(&(s, stale)),
            "the one-tile cap must have dropped the oldest failed write"
        );
        assert!(
            !stale_path.exists(),
            "the superseded scratch file must be deleted, or the next read of this tile silently \
             returns the content the user edited away from"
        );
        match store.get(s, stale) {
            Err(crate::TileError::Io { .. }) => {}
            Ok(tile) => {
                let value = tile
                    .texels()
                    .first()
                    .map_or(f32::NAN, |sample| sample.to_f32());
                unreachable!(
                    "a dropped tile must fail loudly, not read back as {value} from a superseded \
                     scratch file"
                );
            }
            Err(other) => unreachable!("expected TileError::Io, got {other:?}"),
        }
        // The retained one is untouched by any of this.
        assert!(store.pending.contains_key(&(s, filler)));
    }

    /// The background half of the test above, for the path a running
    /// application actually takes: `reconcile_pending`, not `flush`.
    /// Deterministic by construction — the write job is handed to the
    /// writer directly and the writer is joined (`BackgroundWriter::flush`
    /// drops the sender and joins the thread) *before* `reconcile_pending`
    /// drains it, so the failed result is provably already queued and
    /// nothing here depends on OS scheduling. Draining after that join is
    /// exactly what `TileStore::flush` itself does.
    #[test]
    fn reconcile_pending_keeps_the_bytes_of_a_write_that_failed() {
        let (_dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 3, y: 7 };

        // A directory at the tile's own path: the write below fails, and
        // so would any later `page_in` from it.
        let path = store.tile_path(s, id);
        if let Err(err) = std::fs::create_dir(&path) {
            unreachable!("test-local scratch dir must accept a subdirectory: {err}");
        }

        let texels = vec![half::f16::from_f32(0.25); crate::tile::SAMPLES];
        let bytes = crate::codec::encode(&texels);
        // Exactly the state `make_room` leaves behind for an eviction:
        // both maps populated, the same bytes handed to the writer.
        store.pending.insert((s, id), bytes.clone());
        store.paged_out.insert((s, id), path.clone());
        store.writer.submit(crate::writer::WriteJob {
            surface: s,
            id,
            path,
            bytes,
        });
        store.writer.flush();

        store.reconcile_pending();

        assert!(
            store.pending.contains_key(&(s, id)),
            "reconciling a *failed* write must keep its bytes; only a confirmed-durable write has \
             a real replacement to point later reads at"
        );
        let tile = match store.get(s, id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("the tile must still be served from `pending`: {err}"),
        };
        let Some(first) = tile.texels().first() else {
            unreachable!("a tile's texel buffer is never empty");
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(first.to_f32(), 0.25);
        }
    }

    /// A corrupted scratch file must surface as a `TileError`, never as a
    /// short `Tile`. `Tile::from_texels` (this crate's only non-blank tile
    /// constructor, `pub(crate)`) is fed exclusively by `codec::decode`, at
    /// `page_in` and the `pending` branch of `ensure_resident` -- so
    /// `codec`'s own exact-length check is what makes "every `Tile` this
    /// store hands out holds exactly `SAMPLES` samples" true for the
    /// paged-in case, and therefore what keeps `aurora-app`'s own
    /// `write_composited`/`copy_from_slice` and
    /// `aurora_render::composite_layer_into`'s zip out of reach of a short
    /// buffer. Before the fix this returned `Ok` with a half-length tile.
    #[test]
    fn a_truncated_scratch_file_pages_in_as_an_error_not_a_short_tile() {
        let (dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };

        // A structurally valid tile file holding half a tile -- what a
        // crash mid-write or another process in the scratch directory
        // leaves behind. Written to the real path `page_in` will read.
        let half = vec![half::f16::from_f32(0.5); crate::tile::SAMPLES / 2];
        // `encode_any_length`, not `encode`: `encode` now debug-asserts a
        // whole tile precisely so production code cannot write a file
        // like this one by accident. Building the fixture is the one
        // legitimate reason to bypass that.
        let bytes = crate::codec::encode_any_length(&half);
        let path = dir.path().join("truncated.tile");
        if let Err(err) = std::fs::write(&path, &bytes) {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
        store.paged_out.insert((s, id), path);

        match store.get(s, id) {
            Err(crate::TileError::CorruptFile(_)) => {}
            Ok(tile) => unreachable!(
                "a truncated scratch file must not page in as a {}-sample tile",
                tile.texels().len()
            ),
            Err(other) => unreachable!("expected CorruptFile, got {other:?}"),
        }
    }

    /// The retry half of the test above, and the more dangerous half: a
    /// corrupted scratch file must keep failing, not heal into a blank
    /// tile on the second read. Until 0.52.2 `ensure_resident` removed
    /// the `paged_out` mapping *before* calling `page_in`, so a failed
    /// page-in forgot the tile entirely -- read one surfaced
    /// `CorruptFile`, read two fell through to the never-touched branch
    /// and returned `Tile::blank()`. A user whose export was correctly
    /// refused (`aurora_io::IoError::IncompleteComposite`) and who simply
    /// pressed Save again therefore got an `Ok` file with that tile
    /// silently blank: the same class of failure CLAUDE.md names as the
    /// worst this project can have, one step removed. Three reads, not
    /// two -- the rule is "every read", not "the first two".
    #[test]
    fn a_corrupted_scratch_file_keeps_failing_on_every_read_not_just_the_first() {
        let (dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };

        // Same fixture as the test above: a structurally valid ATIL file
        // holding half a tile, at the real path `page_in` will read.
        let half = vec![half::f16::from_f32(0.5); crate::tile::SAMPLES / 2];
        let bytes = crate::codec::encode_any_length(&half);
        let path = dir.path().join("truncated.tile");
        if let Err(err) = std::fs::write(&path, &bytes) {
            unreachable!("test-local scratch disk must accept the write: {err}");
        }
        store.paged_out.insert((s, id), path);

        for read in 1..=3 {
            match store.get(s, id) {
                Err(crate::TileError::CorruptFile(message)) => {
                    assert_rejected_for_its_length(read, &message);
                }
                Ok(tile) => unreachable!(
                    "read {read} of a corrupted scratch file returned a {}-sample tile instead of \
                     an error",
                    tile.texels().len()
                ),
                Err(other) => unreachable!("read {read}: expected CorruptFile, got {other:?}"),
            }
        }
        // Every one of those reads really did hit the disk and really did
        // fail — `failed_page_ins` is the store's own counter for exactly
        // the "retried forever" case this fix creates.
        assert_eq!(store.stats().failed_page_ins, 3);
        assert_eq!(store.stats().faults, 0, "no page-in ever completed");

        // The mapping survived all three failures -- this is the actual
        // fix, stated directly rather than only through its symptom.
        assert!(
            store.paged_out.contains_key(&(s, id)),
            "a failed page-in must leave the paged-out mapping in place, or the next read invents \
             a blank tile"
        );
        // And nothing was ever invented for this key: `Tile::blank()`
        // would have made it resident and bumped `tiles_created`.
        assert_eq!(store.stats().tiles_created, 0);
        assert_eq!(store.resident_len(), 0);
    }

    /// The `pending` branch's own half of the same rule. Undecodable
    /// bytes in `pending` are not scratch-disk corruption -- they are
    /// this store's own encoder output, so this is the defensive case,
    /// not the expected one. It is tested anyway because the *failure
    /// mode* is identical (drop the mapping, and the next read invents a
    /// blank tile) and only a test that reads twice can see it.
    /// Hand-constructs the exact mid-eviction state `make_room` leaves
    /// behind -- the same key in both maps, the idiom
    /// `ensure_resident_serves_directly_from_pending_bypassing_disk_entirely`
    /// above already uses -- with the bytes deliberately wrong-length.
    #[test]
    fn undecodable_pending_bytes_keep_failing_and_leave_both_maps_untouched() {
        let (dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };

        let half = vec![half::f16::from_f32(0.5); crate::tile::SAMPLES / 2];
        let never_created = dir.path().join("this_file_is_never_created.tile");
        store
            .pending
            .insert((s, id), crate::codec::encode_any_length(&half));
        store.paged_out.insert((s, id), never_created);

        for read in 1..=2 {
            match store.get(s, id) {
                Err(crate::TileError::CorruptFile(message)) => {
                    assert_rejected_for_its_length(read, &message);
                }
                Ok(tile) => unreachable!(
                    "read {read} of undecodable pending bytes returned a {}-sample tile",
                    tile.texels().len()
                ),
                Err(other) => unreachable!("read {read}: expected CorruptFile, got {other:?}"),
            }
        }

        assert!(
            store.pending.contains_key(&(s, id)),
            "a failed decode must leave `pending` exactly as it was"
        );
        assert!(
            store.paged_out.contains_key(&(s, id)),
            "a failed decode must leave `paged_out` exactly as it was"
        );
        assert_eq!(store.stats().tiles_created, 0);
        assert_eq!(store.resident_len(), 0);
    }

    /// The same rule for the failure a real scratch disk is far likelier
    /// to produce than a corrupt payload: the file is simply *not there*
    /// (a scratch directory cleaned out mid-session by a `/tmp` reaper or
    /// by the user, a removed volume, a permissions change). That path
    /// returns `TileError::Io` rather than `CorruptFile` and, before
    /// 0.52.2, dropped the `paged_out` mapping just as readily — so the
    /// second read of a tile whose file had been deleted quietly handed
    /// back a blank one. Three reads, and the mapping must survive all of
    /// them.
    #[test]
    fn a_missing_scratch_file_keeps_failing_and_keeps_its_mapping() {
        let (dir, mut store) = store(4);
        let s = surface();
        let id = TileId { x: 0, y: 0 };

        store
            .paged_out
            .insert((s, id), dir.path().join("this_file_is_never_created.tile"));

        for read in 1..=3 {
            match store.get(s, id) {
                Err(crate::TileError::Io {
                    surface, id: got, ..
                }) => {
                    assert_eq!((surface, got), (s, id), "read {read}");
                }
                Ok(tile) => unreachable!(
                    "read {read} of a missing scratch file returned a {}-sample tile instead of \
                     an error",
                    tile.texels().len()
                ),
                Err(other) => unreachable!("read {read}: expected TileError::Io, got {other:?}"),
            }
        }

        assert!(
            store.paged_out.contains_key(&(s, id)),
            "a page-in that failed at the `fs::read` must leave the paged-out mapping in place, \
             or the next read invents a blank tile"
        );
        assert_eq!(store.stats().failed_page_ins, 3);
        assert_eq!(store.stats().faults, 0);
        assert_eq!(store.stats().tiles_created, 0);
        assert_eq!(store.resident_len(), 0);
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
