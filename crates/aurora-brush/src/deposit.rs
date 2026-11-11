//! EXPERIMENTAL (task 170): a batch deposit of plain soft dabs, tile by tile.
//!
//! [`deposit_soft_dabs`] stamps a list of soft Brush dabs (one pen sample of
//! the bristle brush) and leaves **exactly the tile bits** that calling
//! [`crate::stamp_dab_amount`] on each dab in order would (no coverage, no
//! transparency lock, not the Pencil), with the same stroke snapshot and
//! stroke coverage. It is only faster: the dabs are binned by the tiles their
//! bounding boxes reach, each tile is acquired from the store once per batch
//! instead of once per dab, its stroke coverage is taken from the snapshot's
//! map once, each dab visits only the texel rows and columns its disc can
//! reach instead of its whole bounding box, a texel's four channels are
//! converted to and from `f16` together, and a big batch runs on a small
//! pool in bands of [`BAND_ROWS`] rows ([`Mode`]).
//!
//! **Why the result is the same bits.** Every quantity the per-dab path reads
//! for a texel is either per texel or per tile:
//!
//! * the stored texel and its stroke coverage `A` are per texel, and every
//!   dab that reaches a texel is applied to it in the original order, because
//!   each tile's dab list is in input order and a dab touches a texel at most
//!   once;
//! * the stroke-start content is the snapshot's raw copy once the tile is
//!   captured, otherwise the live tile; both paths look it up per (dab, tile)
//!   in the same order, so a dab that captures a tile sees the live texel
//!   (unchanged until its own write) and every later dab sees the raw copy;
//! * the capture happens at the first changing write of the first dab that
//!   changes the tile, from the tile as it is at that moment; no earlier dab
//!   of the batch changed that tile (it would have captured it), so the
//!   captured content is the same;
//! * the per-texel arithmetic is a copy of `stamp_dab_impl`'s soft,
//!   unlocked, uncovered branch: the same `texel_alpha`, the same
//!   `accumulate`, the same `f32` expressions, the same `f16` rounding (the
//!   four-channel conversions are pinned to `to_f32`/`from_f32` bit for bit
//!   by a test over every `f16` and the rounding midpoints), the same bit
//!   comparison against the current texel. The stored value
//!   must stay `f16` between dabs because the "did anything change" decision
//!   (capture, painted list, dirty rectangle, and with no snapshot the next
//!   dab's start) is made against the stored bits; an `f32` working tile
//!   would change those decisions, so it is deliberately not used;
//! * the row and column limits skip only texels whose distance from the
//!   centre is certainly above the radius (the disc grown by two texels, and
//!   only where every coordinate is below 2^22 so `f32` texel centres are
//!   exact; otherwise the whole box is walked as before). Such texels have
//!   falloff 0, which the per-dab path also skips before touching anything;
//! * the parallel path splits a tile into bands of rows; a dab's texels are
//!   independent of each other, so bands are independent, and each band
//!   applies its tile's dabs in order (see `deposit_parallel` for the start
//!   content and the capture there).
//!
//! Tiles are created, paged in and given a new content version by
//! `TileStore::get_mut` for every tile any dab's bounding box reaches, as the
//! per-dab path does (so the same tiles exist afterwards, blank ones
//! included); the version **numbers** differ because the per-dab path bumps a
//! tile once per dab and this path once per batch (twice for a tile the
//! parallel path writes back). A tile's dirty rectangle is the union of the
//! boxes of the dabs that changed it, as before. LRU recency and therefore
//! which tiles a small budget evicts differ too (never the bits).
//!
//! **Not supported, refused by the types:** selection coverage, the
//! transparency lock and the Pencil's hard dab. [`SoftDab`] has no field for
//! any of them; a caller that needs one must use the per-dab entry points.
//!
//! **One difference on failure.** A tile whose page-in fails is skipped for
//! every dab of the batch (the per-dab path retries the page-in for every
//! dab, so a transient failure could let a later dab through). Every dab
//! whose box reaches a failed tile is counted in
//! [`BatchOutcome::failed_dabs`].

#![allow(dead_code)]

use std::ops::Range;
use std::sync::Arc;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;
use half::slice::HalfFloatSliceExt;
use half::vec::HalfBitsVecExt;

use crate::StrokeSnapshot;
use crate::amount::PaintAmount;
use crate::stamp::{
    DabOutcome, Texel, is_paintable, malformed_tile, stores_the_same_bits, texel_alpha,
    texel_index, tile_range, usable_snapshot,
};
use crate::stroke_cover::{CoverTile, accumulate, cover_index, zeroed_tile};

/// One plain soft Brush dab: what [`crate::stamp_dab_amount`] takes with no
/// coverage, no transparency lock and `hard = false`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoftDab {
    pub center: (f32, f32),
    pub radius: f32,
    /// Straight RGB.
    pub colour: [f32; 3],
    pub amount: PaintAmount,
}

/// What one [`deposit_soft_dabs`] call did.
#[derive(Debug, Default)]
#[must_use]
pub struct BatchOutcome {
    /// Every tile changed, once each (tile order, not dab order), and every
    /// tile that could not be acquired.
    pub tiles: DabOutcome,
    /// Dabs whose bounding box reaches a tile that could not be acquired.
    pub failed_dabs: usize,
}

/// How [`deposit_soft_dabs_with`] schedules the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One tile after another on the calling thread, in place.
    Serial,
    /// On the deposit pool, in bands of [`BAND_ROWS`] rows, whenever there
    /// is more than one band of work (tests and measurements).
    Parallel,
    /// Parallel when the batch's dab boxes hold at least
    /// [`PARALLEL_MIN_TEXELS`] texels; serial otherwise.
    Auto,
}

/// The dab-box texels (summed over dabs and tiles) below which a batch stays
/// on the calling thread: the parallel path copies the reached bands of a
/// tile out and the changed ones back, and pays a pool round trip, about the
/// cost of a few thousand texel visits. An engineering default.
pub const PARALLEL_MIN_TEXELS: u64 = 32 * 1024;

/// Rows per band of the parallel path: four bands per tile. Bands of one
/// tile hold disjoint texels, so they are as independent as tiles are.
pub const BAND_ROWS: u32 = 64;

/// Most threads the deposit pool runs.
const POOL_THREADS: usize = 4;

/// Coordinates below this keep `f32` texel centres exact, which the row and
/// column limits rely on.
const EXACT_COORD_LIMIT: f32 = 4_194_304.0; // 2^22

/// Samples in one band.
const BAND_SAMPLES: usize = (BAND_ROWS * TILE) as usize * CHANNELS;
/// Coverage values in one band.
const BAND_COVER: usize = (BAND_ROWS * TILE) as usize;
/// Bands per tile.
const BANDS: u32 = TILE / BAND_ROWS;

/// A dab with its texel box, computed once.
#[derive(Debug, Clone, Copy)]
struct Prepared {
    dab: SoftDab,
    /// Inclusive surface texel box of the dab (the per-dab path's
    /// `min_x..=max_x`, `min_y..=max_y`).
    min: (u32, u32),
    max: (u32, u32),
    /// Whether the disc limits may be used.
    exact: bool,
}

impl Prepared {
    /// The dab's tile-local inclusive texel box in tile `id` (the per-dab
    /// path's `lx0..=lx1`, `ly0..=ly1`).
    fn tile_box(&self, id: TileId) -> (u32, u32, u32, u32) {
        let origin = (id.x * TILE, id.y * TILE);
        (
            self.min.0.saturating_sub(origin.0).min(TILE - 1),
            self.min.1.saturating_sub(origin.1).min(TILE - 1),
            self.max.0.saturating_sub(origin.0).min(TILE - 1),
            self.max.1.saturating_sub(origin.1).min(TILE - 1),
        )
    }
}

/// Stamps `dabs` in order onto `surface` of `store`, as
/// `stamp_dab_amount(store, surface, d.center, d.radius, d.colour, snapshot,
/// None, false, false, d.amount)` for each `d` would, tile by tile
/// ([`Mode::Auto`]). See the module documentation for the exactness
/// argument and the one difference (a failed page-in).
pub fn deposit_soft_dabs(
    store: &mut TileStore,
    surface: SurfaceId,
    dabs: &[SoftDab],
    snapshot: Option<&mut StrokeSnapshot>,
) -> BatchOutcome {
    deposit_soft_dabs_with(store, surface, dabs, snapshot, Mode::Auto)
}

/// [`deposit_soft_dabs`] with the scheduling chosen ([`Mode`]); every mode
/// leaves the same bits.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn deposit_soft_dabs_with(
    store: &mut TileStore,
    surface: SurfaceId,
    dabs: &[SoftDab],
    snapshot: Option<&mut StrokeSnapshot>,
    mode: Mode,
) -> BatchOutcome {
    let mut out = BatchOutcome::default();
    let snapshot = usable_snapshot(snapshot, surface);
    let mut prepared: Vec<Prepared> = Vec::with_capacity(dabs.len());
    // (tile, dab index) pairs, sorted below so each tile's dabs are in order.
    let mut bins: Vec<(TileId, u32)> = Vec::new();
    for dab in dabs {
        if !is_paintable(dab.center, dab.radius) {
            continue;
        }
        let (cx, cy) = dab.center;
        let r = dab.radius;
        let min = (
            (cx - r).floor().max(0.0) as u32,
            (cy - r).floor().max(0.0) as u32,
        );
        let max = (
            (cx + r).ceil().max(0.0) as u32,
            (cy + r).ceil().max(0.0) as u32,
        );
        let exact = cx.abs() < EXACT_COORD_LIMIT
            && cy.abs() < EXACT_COORD_LIMIT
            && r < EXACT_COORD_LIMIT
            && max.0 < EXACT_COORD_LIMIT as u32
            && max.1 < EXACT_COORD_LIMIT as u32;
        let Ok(index) = u32::try_from(prepared.len()) else {
            break;
        };
        prepared.push(Prepared {
            dab: *dab,
            min,
            max,
            exact,
        });
        let (t0, t1) = tile_range(dab.center, r);
        for ty in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                bins.push((TileId { x: tx, y: ty }, index));
            }
        }
    }
    bins.sort_unstable_by_key(|&(id, i)| (id.y, id.x, i));
    // Each tile with the range of `bins` holding its dabs, in order.
    let mut groups: Vec<(TileId, Range<usize>)> = Vec::new();
    for (k, &(id, _)) in bins.iter().enumerate() {
        match groups.last_mut() {
            Some((last, range)) if *last == id => range.end = k + 1,
            _ => groups.push((id, k..k + 1)),
        }
    }
    let plan = Plan {
        surface,
        prepared: &prepared,
        bins: &bins,
    };
    let mut failed_dab = vec![false; prepared.len()];
    let parallel = match mode {
        Mode::Serial => false,
        Mode::Parallel => !groups.is_empty(),
        Mode::Auto => plan.work() >= PARALLEL_MIN_TEXELS,
    };
    let pool = if parallel { deposit_pool() } else { None };
    match pool {
        Some(pool) => deposit_parallel(
            store,
            &plan,
            &groups,
            snapshot,
            pool,
            &mut out,
            &mut failed_dab,
        ),
        None => deposit_serial(store, &plan, &groups, snapshot, &mut out, &mut failed_dab),
    }
    out.failed_dabs = failed_dab.iter().filter(|&&f| f).count();
    out
}

/// The deposit's own pool, built on first use; `None` when it cannot be
/// built (the deposit then runs serially) or the machine has one core.
fn deposit_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let threads = cores.min(POOL_THREADS);
        if threads < 2 {
            return None;
        }
        match rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("aurora-deposit-{i}"))
            .build()
        {
            Ok(pool) => Some(pool),
            Err(err) => {
                tracing::warn!(%err, "no deposit pool; the batch deposit runs serially");
                None
            }
        }
    })
    .as_ref()
}

/// The batch, binned.
struct Plan<'a> {
    surface: SurfaceId,
    prepared: &'a [Prepared],
    /// `(tile, dab index)` sorted by tile, then dab order.
    bins: &'a [(TileId, u32)],
}

/// The rows of one tile a kernel call may write: `texels` and `cover` start
/// at tile row `row0` and hold `count` rows.
struct Rows<'a> {
    texels: &'a mut [f16],
    /// The stroke coverage of the same rows (`None`: not tracked).
    cover: Option<&'a mut [f32]>,
    row0: u32,
    count: u32,
}

impl Plan<'_> {
    /// The dab-box texels the batch will walk, summed over tiles.
    fn work(&self) -> u64 {
        self.bins
            .iter()
            .filter_map(|&(id, i)| self.prepared.get(i as usize).map(|p| (id, p)))
            .map(|(id, p)| {
                let (lx0, ly0, lx1, ly1) = p.tile_box(id);
                u64::from(lx1 - lx0 + 1) * u64::from(ly1 - ly0 + 1)
            })
            .sum()
    }

    fn dabs(&self, range: Range<usize>) -> impl Iterator<Item = &Prepared> {
        self.bins
            .get(range)
            .unwrap_or(&[])
            .iter()
            .filter_map(|&(_, i)| self.prepared.get(i as usize))
    }

    fn mark_failed(&self, range: Range<usize>, failed_dab: &mut [bool]) {
        for &(_, i) in self.bins.get(range).unwrap_or(&[]) {
            if let Some(flag) = failed_dab.get_mut(i as usize) {
                *flag = true;
            }
        }
    }

    /// Applies the tile's dabs (`range` of the bins) in order to `rows`.
    /// Returns the union of the tile boxes of the dabs that changed a texel
    /// of these rows, whether a capture was refused, and whether any texel
    /// was within a dab's footprint (the per-dab path creates the stroke
    /// coverage tile exactly then). `fixed_start` is the stroke-start
    /// content of the whole tile; `None` looks it up in `snapshot` per dab,
    /// capturing at the first change (serial path, whole tile only).
    #[allow(clippy::too_many_arguments)]
    fn paint_rows(
        &self,
        id: TileId,
        range: Range<usize>,
        rows: &mut Rows<'_>,
        fixed_start: Option<&[f16]>,
        mut snapshot: Option<&mut StrokeSnapshot>,
    ) -> (Option<Rect>, bool, bool) {
        let mut dirty: Option<Rect> = None;
        let mut failed = false;
        let mut reached = false;
        for p in self.dabs(range) {
            let tbox = p.tile_box(id);
            let (touched, capture_failed) = if let Some(start) = fixed_start {
                dab_into_rows(rows, Some(start), None, &mut reached, id, tbox, p)
            } else {
                match snapshot.as_deref().and_then(|s| s.raw_content(id)) {
                    Some(raw) => dab_into_rows(rows, Some(raw), None, &mut reached, id, tbox, p),
                    None => dab_into_rows(
                        rows,
                        None,
                        snapshot.as_deref_mut(),
                        &mut reached,
                        id,
                        tbox,
                        p,
                    ),
                }
            };
            if capture_failed {
                // As the per-dab path: this dab abandons the tile before its
                // first write and the tile is reported; later dabs carry on
                // (and meet the same refusal at their first write).
                failed = true;
                continue;
            }
            if touched {
                let (lx0, ly0, lx1, ly1) = tbox;
                let rect = Rect {
                    x: i64::from(lx0),
                    y: i64::from(ly0),
                    width: lx1 - lx0 + 1,
                    height: ly1 - ly0 + 1,
                };
                dirty = Some(dirty.map_or(rect, |d| d.union(&rect)));
            }
        }
        (dirty, failed, reached)
    }
}

/// Tile after tile, in place, on the calling thread.
fn deposit_serial(
    store: &mut TileStore,
    plan: &Plan<'_>,
    groups: &[(TileId, Range<usize>)],
    mut snapshot: Option<&mut StrokeSnapshot>,
    out: &mut BatchOutcome,
    failed_dab: &mut [bool],
) {
    let surface = plan.surface;
    let track = snapshot.is_some();
    for (id, range) in groups.iter().cloned() {
        let tile = match store.get_mut(surface, id) {
            Ok(tile) => tile,
            Err(err) => {
                plan.mark_failed(range, failed_dab);
                out.tiles.fail(id, err);
                continue;
            }
        };
        let texels = tile.texels_mut();
        let held = if track {
            snapshot.as_deref_mut().and_then(|s| s.cover_mut().take(id))
        } else {
            None
        };
        let had_cover = held.is_some();
        let mut cover: Option<CoverTile> = if track {
            Some(held.unwrap_or_else(zeroed_tile))
        } else {
            None
        };
        let (dirty, failed, reached) = {
            let mut rows = Rows {
                texels: &mut *texels,
                cover: cover.as_deref_mut(),
                row0: 0,
                count: TILE,
            };
            plan.paint_rows(id, range, &mut rows, None, snapshot.as_deref_mut())
        };
        let samples = texels.len();
        // The per-dab path keeps a coverage tile it held, and creates one
        // at the first texel inside a dab's footprint.
        if let (Some(cover), Some(stroke)) = (cover, snapshot.as_deref_mut())
            && (had_cover || reached)
        {
            stroke.cover_mut().put(id, cover);
        }
        if failed {
            out.tiles.fail(id, malformed_tile(surface, id, samples));
            continue;
        }
        if let Some(rect) = dirty {
            out.tiles.painted.push(id);
            tile.mark_dirty(rect);
        }
    }
}

/// One tile of the parallel path.
struct Job {
    id: TileId,
    range: Range<usize>,
    /// The tile as the batch found it (the store's own shared buffer).
    original: Arc<Vec<f16>>,
    had_cover: bool,
    cover: Option<CoverTile>,
    /// The painted copy; only the bands some dab reaches are filled.
    work: Vec<f16>,
    /// Which bands some dab's box reaches.
    reached_bands: [bool; BANDS as usize],
}

/// One band's result.
#[derive(Default, Clone, Copy)]
struct BandResult {
    dirty: Option<Rect>,
    reached: bool,
}

/// Every reached band of every tile on the deposit pool, each on its own
/// copy, then written back tile by tile on the calling thread.
///
/// Why the same bits as the serial path: a dab reads and writes only the
/// texels (and stroke coverage) of its own footprint, each texel on its own,
/// so disjoint bands of rows are independent, and within a band each tile's
/// dabs run in order. With a snapshot, the stroke-start content of a tile is
/// the snapshot's raw copy if an earlier batch captured it, else the tile as
/// this batch found it (`original`): until its first write a tile *is* its
/// own start, and after it the per-dab path reads the raw copy captured from
/// exactly that content. With no snapshot the start is the live texel, here
/// the copy. The capture, the second version bump and the dirty mark happen
/// on the calling thread afterwards, from `original`, for exactly the tiles
/// that changed.
#[allow(clippy::too_many_lines)]
fn deposit_parallel(
    store: &mut TileStore,
    plan: &Plan<'_>,
    groups: &[(TileId, Range<usize>)],
    mut snapshot: Option<&mut StrokeSnapshot>,
    pool: &rayon::ThreadPool,
    out: &mut BatchOutcome,
    failed_dab: &mut [bool],
) {
    use rayon::prelude::*;

    let surface = plan.surface;
    let track = snapshot.is_some();
    let mut jobs: Vec<Job> = Vec::with_capacity(groups.len());
    for (id, range) in groups.iter().cloned() {
        // `get_mut`, as the per-dab path: creates or pages in the tile and
        // gives it a new content version.
        if let Err(err) = store.get_mut(surface, id) {
            plan.mark_failed(range, failed_dab);
            out.tiles.fail(id, err);
            continue;
        }
        let Some(original) = store.peek_shared(surface, id) else {
            // Just made resident by `get_mut`; unreachable, but never paint
            // a tile whose content is not in hand.
            plan.mark_failed(range, failed_dab);
            out.tiles.fail(id, malformed_tile(surface, id, 0));
            continue;
        };
        let held = if track {
            snapshot.as_deref_mut().and_then(|s| s.cover_mut().take(id))
        } else {
            None
        };
        let had_cover = held.is_some();
        let cover = if track {
            Some(held.unwrap_or_else(zeroed_tile))
        } else {
            None
        };
        let mut reached_bands = [false; BANDS as usize];
        for p in plan.dabs(range.clone()) {
            let (_, ly0, _, ly1) = p.tile_box(id);
            for band in (ly0 / BAND_ROWS)..=(ly1 / BAND_ROWS) {
                if let Some(flag) = reached_bands.get_mut(band as usize) {
                    *flag = true;
                }
            }
        }
        jobs.push(Job {
            id,
            range,
            original,
            had_cover,
            cover,
            // Zeroed by the allocator (`u16`), not by a fill loop.
            work: vec![0_u16; aurora_tile::SAMPLES].reinterpret_into(),
            reached_bands,
        });
    }
    let shared: Option<&StrokeSnapshot> = snapshot.as_deref();
    let results: Vec<Vec<BandResult>> = pool.install(|| {
        jobs.par_iter_mut()
            .map(|job| {
                let Job {
                    id,
                    range,
                    original,
                    cover,
                    work,
                    reached_bands,
                    ..
                } = job;
                let start: Option<&[f16]> = if track {
                    Some(
                        shared
                            .and_then(|s| s.raw_content(*id))
                            .unwrap_or(original.as_slice()),
                    )
                } else {
                    None
                };
                let covers: Vec<Option<&mut [f32]>> = match cover.as_deref_mut() {
                    Some(c) => c.chunks_mut(BAND_COVER).map(Some).collect(),
                    None => (0..BANDS).map(|_| None).collect(),
                };
                work.par_chunks_mut(BAND_SAMPLES)
                    .zip(original.par_chunks(BAND_SAMPLES))
                    .zip(covers)
                    .enumerate()
                    .map(|(band, ((texels, from), cover))| {
                        if !reached_bands.get(band).copied().unwrap_or(false) {
                            return BandResult::default();
                        }
                        texels.copy_from_slice(from);
                        let row0 = u32::try_from(band).unwrap_or(0) * BAND_ROWS;
                        let mut rows = Rows {
                            texels,
                            cover,
                            row0,
                            count: BAND_ROWS,
                        };
                        let (dirty, _refused, reached) =
                            plan.paint_rows(*id, range.clone(), &mut rows, start, None);
                        BandResult { dirty, reached }
                    })
                    .collect()
            })
            .collect()
    });
    for (job, bands) in jobs.into_iter().zip(results) {
        let Job {
            id,
            range,
            original,
            had_cover,
            cover,
            work,
            ..
        } = job;
        let reached = bands.iter().any(|b| b.reached);
        if let (Some(cover), Some(stroke)) = (cover, snapshot.as_deref_mut())
            && (had_cover || reached)
        {
            stroke.cover_mut().put(id, cover);
        }
        let Some(rect) = bands
            .iter()
            .filter_map(|b| b.dirty)
            .reduce(|a, b| a.union(&b))
        else {
            continue;
        };
        let tile = match store.get_mut(surface, id) {
            Ok(tile) => tile,
            Err(err) => {
                plan.mark_failed(range, failed_dab);
                out.tiles.fail(id, err);
                continue;
            }
        };
        if let Some(stroke) = snapshot.as_deref_mut()
            && !stroke.record_content(id, &original)
        {
            out.tiles
                .fail(id, malformed_tile(surface, id, original.len()));
            continue;
        }
        // Released before the write so the store's buffer is not shared
        // (no copy-on-write clone).
        drop(original);
        let texels = tile.texels_mut();
        // Only the bands a dab changed are copied back; the others of
        // `work` may not even hold the tile.
        for ((dst, src), band) in texels
            .chunks_mut(BAND_SAMPLES)
            .zip(work.chunks(BAND_SAMPLES))
            .zip(&bands)
        {
            if band.dirty.is_some() && dst.len() == src.len() {
                dst.copy_from_slice(src);
            }
        }
        out.tiles.painted.push(id);
        tile.mark_dirty(rect);
    }
}

/// The tile-local columns `lx0..=lx1` of row `py` (a texel centre) that the
/// dab's disc grown by two texels can reach; `None` when the row is
/// wholly outside it. Only for `exact` dabs.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn row_span(p: &Prepared, py: f32, origin_x: u32, lx0: u32, lx1: u32) -> Option<(u32, u32)> {
    let (cx, cy) = p.dab.center;
    // The disc grown by two texels: a texel outside it is more than one
    // texel beyond the radius even after the `f32` rounding of its offset
    // (at most a quarter texel below 2^22), so its `hypot` is above the
    // radius and its falloff 0.
    let grown = f64::from(p.dab.radius) + 2.0;
    let dy = f64::from(py - cy);
    if dy.abs() > grown {
        return None;
    }
    let w = (grown * grown - dy * dy).max(0.0).sqrt();
    let lo = (f64::from(cx) - w - 0.5 - f64::from(origin_x)).floor();
    let hi = (f64::from(cx) + w - 0.5 - f64::from(origin_x)).ceil();
    if hi < f64::from(lx0) || lo > f64::from(lx1) {
        return None;
    }
    let lo = if lo <= f64::from(lx0) { lx0 } else { lo as u32 };
    let hi = if hi >= f64::from(lx1) { lx1 } else { hi as u32 };
    (lo <= hi).then_some((lo, hi))
}

/// One dab into the rows `rows` of one tile: `stamp_dab_impl`'s soft,
/// unlocked, uncovered branch, texel for texel. `start` is indexed over the
/// whole tile; a capture into `snap` needs `rows` to be the whole tile.
/// Returns `(touched, capture_failed)` and sets `reached` when a texel is
/// inside the footprint.
#[allow(clippy::cast_precision_loss, clippy::similar_names)]
fn dab_into_rows(
    rows: &mut Rows<'_>,
    start: Option<&[f16]>,
    mut snap: Option<&mut StrokeSnapshot>,
    reached: &mut bool,
    id: TileId,
    tbox: (u32, u32, u32, u32),
    p: &Prepared,
) -> (bool, bool) {
    let dab = &p.dab;
    let flow = dab.amount.flow();
    let opacity_ceiling = dab.amount.opacity();
    let [red, green, blue] = dab.colour;
    let (lx0, ly0, lx1, ly1) = tbox;
    let origin = (id.x * TILE, id.y * TILE);
    let row_end = rows.row0 + rows.count - 1;
    if ly1 < rows.row0 || ly0 > row_end {
        return (false, false);
    }
    let mut touched = false;
    for ly in ly0.max(rows.row0)..=ly1.min(row_end) {
        let py = (origin.1 + ly) as f32 + 0.5;
        let (from, to) = if p.exact {
            match row_span(p, py, origin.0, lx0, lx1) {
                Some(span) => span,
                None => continue,
            }
        } else {
            (lx0, lx1)
        };
        let local_y = ly - rows.row0;
        for lx in from..=to {
            let px = (origin.0 + lx) as f32 + 0.5;
            let a = texel_alpha(px, py, dab.center, dab.radius, false);
            if a <= 0.0 {
                continue;
            }
            *reached = true;
            let amount_here = a * flow;
            let index = texel_index(lx, local_y);
            let Some(current) = rows.texels.get(index..index + CHANNELS) else {
                continue;
            };
            let ci = cover_index(lx, local_y);
            let a_old = rows
                .cover
                .as_deref()
                .and_then(|c| c.get(ci).copied())
                .unwrap_or(0.0);
            let a_new = accumulate(a_old, opacity_ceiling, amount_here);
            if a_new.partial_cmp(&a_old) != Some(std::cmp::Ordering::Greater) {
                continue;
            }
            let whole = texel_index(lx, ly);
            let began = start
                .and_then(|s| s.get(whole..whole + CHANNELS))
                .unwrap_or(current);
            // The four channels converted together (one SIMD conversion each
            // way where the hardware has one): the same per-channel values
            // and `f16` bits as `to_f32`/`from_f32` one at a time, see
            // `the_four_channel_conversions_match_one_at_a_time` in the tests.
            let mut s = [0.0_f32; CHANNELS];
            if began.len() == CHANNELS {
                began.convert_to_f32_slice(&mut s);
            }
            let keep = 1.0 - a_new;
            let mixed = [
                s[0] * keep + red * a_new,
                s[1] * keep + green * a_new,
                s[2] * keep + blue * a_new,
                s[3] * keep + 1.0 * a_new,
            ];
            let mut fresh: Texel = [f16::ZERO; CHANNELS];
            fresh.convert_from_f32_slice(&mixed);
            if stores_the_same_bits(current, fresh) {
                set_cover(rows.cover.as_deref_mut(), ci, a_new);
                continue;
            }
            if !touched {
                if let Some(stroke) = snap.as_mut()
                    && !stroke.record_content(id, rows.texels)
                {
                    return (false, true);
                }
                touched = true;
            }
            set_cover(rows.cover.as_deref_mut(), ci, a_new);
            if let Some(dst) = rows.texels.get_mut(index..index + CHANNELS) {
                dst.copy_from_slice(&fresh);
            }
        }
    }
    (touched, false)
}

fn set_cover(cover: Option<&mut [f32]>, index: usize, value: f32) {
    if let Some(slot) = cover.and_then(|c| c.get_mut(index)) {
        *slot = value;
    }
}
