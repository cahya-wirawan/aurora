//! Layer thumbnails for the Layers panel (0.197.0, round L4): derived data,
//! never saved, never in History, never a reason for the document to be
//! dirty. ADR 0013 has the account; the short version:
//!
//! - **Lazy.** Only the rows in (or within two rows of) the panel's viewport
//!   are ever computed ([`LayerThumbnails::update`] is handed exactly those
//!   layers), each when first needed, at most [`MAX_JOBS_PER_UPDATE`] per
//!   event-loop turn.
//! - **Bounded reads.** A thumbnail reads at most [`MAX_TILES`] (16) tiles
//!   of its layer, chosen among the tiles that *exist* ([`plan`]: the tile
//!   grid is probed with `contains_tile`, at most [`SCAN_LIMIT`] cells, on a
//!   regular lattice beyond that). With 16 or fewer existing tiles all are
//!   read and the picture is the exact area average of the layer's bounds
//!   (a layer painted in one corner is not blank). With more, a regular
//!   spread of 16 of them is read and each texel over an existing tile takes
//!   an [`SPARSE_WINDOW`]-square window from the nearest sampled tile: a
//!   *sampled* picture, blind to edits inside existing tiles that were not
//!   sampled (a new tile does change the stamp, so it refreshes).
//! - **Off the UI thread.** The UI thread only takes shares of the sampled
//!   tiles (an `Arc` clone of a resident tile's texels, the encoded bytes of
//!   a pending one; never a page-in or decode) and hands them to one worker
//!   thread, which decodes and downsamples. A tile that exists only on the
//!   scratch disk is read on the UI thread, at most
//!   [`SCRATCH_READS_PER_TURN`] (8) per event-loop turn across all layers;
//!   the rest wait for the next turn ([`Gather`]). The worker
//!   owns no tile store: nothing joins the residency pool, and what it
//!   holds is the handful of shares of the jobs queued (at most
//!   [`QUEUE_LIMIT`]) plus one decoded 512 KiB tile at a time.
//! - **Cached by content.** The cache key is a stamp over the sampled tiles'
//!   [`aurora_tile::TileStore::content_version`]s and the layer's size, so a
//!   dab or an undo of pixels changes the stamp of the layer it touched and
//!   no other, and an opacity, visibility or blend edit changes nothing.
//!   Capped at [`CACHE_ENTRIES`] pictures and [`CACHE_BYTES`], least
//!   recently used out.
//! - **Throttled.** A layer whose stamp changed is recomputed at most every
//!   [`REFRESH_INTERVAL`] while it keeps changing (a stroke), and once more
//!   after the last change.
//! - **Generations.** A document switch or close moves the generation: the
//!   worker drops stale jobs, and a result that returns for an old
//!   generation is discarded.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use aurora_doc::{LayerId, LayerKind, LayerTree};
use aurora_tile::{TILE, TileId, TileLookup, TileShare, TileStore};
use aurora_widgets::widgets::{THUMBNAIL_TEXELS, ThumbnailImage, WidgetKind};
use aurora_widgets::{WidgetId, WidgetTree};
use half::f16;

use crate::document_session::DocumentId;

/// Tiles one thumbnail reads, at most.
pub(crate) const MAX_TILES: usize = 16;
/// The side of the window a sparse thumbnail texel averages.
pub(crate) const SPARSE_WINDOW: u32 = 8;
/// Cached pictures, at most.
pub(crate) const CACHE_ENTRIES: usize = 256;
/// Cached picture bytes, at most.
pub(crate) const CACHE_BYTES: usize = 8 << 20;
/// A changing layer's picture is recomputed no more often than this.
pub(crate) const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
/// Jobs one [`LayerThumbnails::update`] submits.
pub(crate) const MAX_JOBS_PER_UPDATE: usize = 4;
/// Jobs the worker's queue holds.
pub(crate) const QUEUE_LIMIT: usize = 8;
/// Tile-grid cells [`plan`] probes for existence, at most.
pub(crate) const SCAN_LIMIT: u64 = 4096;
/// Tiles read from the scratch disk on the UI thread per
/// [`LayerThumbnails::update`], at most.
pub(crate) const SCRATCH_READS_PER_TURN: usize = 8;
/// A layer's plan is re-derived at most this often after the store changed.
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_millis(100);

type Waker = Arc<dyn Fn() + Send + Sync>;

// ---------------------------------------------------------------------
// Sampling (pure)
// ---------------------------------------------------------------------

/// Which tiles of a `width` x `height` layer a thumbnail reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The existing tiles read, at most [`MAX_TILES`], ascending `(y, x)`.
    pub tiles: Vec<TileId>,
    /// Every existing tile (on the probe lattice) is read: the thumbnail is
    /// the exact area average of the layer's bounds.
    pub dense: bool,
    /// Sparse plans only: the existing tiles the probe found.
    pub existing: HashSet<TileId>,
    /// The probe lattice's step in tiles (1: every cell was probed).
    pub step: u32,
    /// Existing tiles the probe found.
    pub existing_count: usize,
}

/// The tiles a thumbnail of a `width` x `height` layer reads, chosen among
/// those `exists` reports: at most [`MAX_TILES`], from at most
/// [`SCAN_LIMIT`] probes (a layer with more grid cells is probed on a
/// regular lattice of step `ceil(sqrt(cells / SCAN_LIMIT))`; tiles between
/// lattice points are unseen).
pub(crate) fn plan(width: u32, height: u32, exists: &mut dyn FnMut(TileId) -> bool) -> Plan {
    let (gw, gh) = (width.div_ceil(TILE).max(1), height.div_ceil(TILE).max(1));
    let cells = u64::from(gw) * u64::from(gh);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let step = if cells <= SCAN_LIMIT {
        1
    } else {
        ((cells as f64 / SCAN_LIMIT as f64).sqrt().ceil() as u32).max(2)
    };
    let mut found: Vec<TileId> = Vec::new();
    for y in (0..gh).step_by(step as usize) {
        for x in (0..gw).step_by(step as usize) {
            let id = TileId { x, y };
            if exists(id) {
                found.push(id);
            }
        }
    }
    let existing_count = found.len();
    if existing_count <= MAX_TILES {
        return Plan {
            tiles: found,
            dense: true,
            existing: HashSet::new(),
            step,
            existing_count,
        };
    }
    #[allow(clippy::cast_possible_truncation)]
    let max = MAX_TILES as u32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let nx =
        (((f64::from(max) * f64::from(gw) / f64::from(gh)).sqrt()) as u32).clamp(1, gw.min(max));
    let ny = (max / nx).clamp(1, gh);
    // One tile per coarse bucket: the existing one nearest the bucket's
    // centre.
    let mut best: HashMap<(u32, u32), (u64, TileId)> = HashMap::new();
    let centre =
        |b: u32, n: u32, g: u32| (2 * u64::from(b) + 1) * u64::from(g) / (2 * u64::from(n));
    for &id in &found {
        #[allow(clippy::cast_possible_truncation)]
        let bucket = (
            (u64::from(id.x) * u64::from(nx) / u64::from(gw)) as u32,
            (u64::from(id.y) * u64::from(ny) / u64::from(gh)) as u32,
        );
        let (cx, cy) = (centre(bucket.0, nx, gw), centre(bucket.1, ny, gh));
        let distance = u64::from(id.x).abs_diff(cx).pow(2) + u64::from(id.y).abs_diff(cy).pow(2);
        let entry = best.entry(bucket).or_insert((distance, id));
        if distance < entry.0 {
            *entry = (distance, id);
        }
    }
    let mut tiles: Vec<TileId> = best.into_values().map(|(_, id)| id).collect();
    tiles.sort_by_key(|id| (id.y, id.x));
    Plan {
        tiles,
        dense: false,
        existing: found.into_iter().collect(),
        step,
        existing_count,
    }
}

/// The content rectangle `(x, y, w, h)` on the 48-texel grid for a layer of
/// `width` x `height`: an aspect-preserving fit, centred.
pub(crate) fn fit(width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let longest = width.max(height);
    if width == 0 || height == 0 {
        return None;
    }
    let scaled = |v: u32| -> u32 {
        let n = (u64::from(v) * u64::from(THUMBNAIL_TEXELS) + u64::from(longest) / 2)
            / u64::from(longest);
        #[allow(clippy::cast_possible_truncation)]
        let n = n as u32;
        n.clamp(1, THUMBNAIL_TEXELS)
    };
    let (w, h) = (scaled(width), scaled(height));
    Some(((THUMBNAIL_TEXELS - w) / 2, (THUMBNAIL_TEXELS - h) / 2, w, h))
}

/// Decoded tiles a sample reads from.
pub(crate) type Decoded = HashMap<TileId, Arc<Vec<f16>>>;

/// One straight-alpha texel as premultiplied `f32`; a non-finite sample
/// (a NaN or infinite half-float a damaged file can hold) counts as 0.
fn premultiply(px: [f16; 4]) -> [f32; 4] {
    let [red, green, blue, alpha] = px.map(|v| {
        let v = v.to_f32();
        if v.is_finite() { v } else { 0.0 }
    });
    let alpha = alpha.clamp(0.0, 1.0);
    [red * alpha, green * alpha, blue * alpha, alpha]
}

/// The pixel of the layer at layer-local `(x, y)`, premultiplied;
/// transparent where no tile is held.
fn premultiplied(tiles: &Decoded, x: u32, y: u32) -> [f32; 4] {
    let id = TileId {
        x: x / TILE,
        y: y / TILE,
    };
    let Some(texels) = tiles.get(&id) else {
        return [0.0; 4];
    };
    let at = ((y % TILE) as usize * TILE as usize + (x % TILE) as usize) * 4;
    match texels.get(at..at + 4).map(<[f16; 4]>::try_from) {
        Some(Ok(px)) => premultiply(px),
        _ => [0.0; 4],
    }
}

/// A `width` x `height` layer's thumbnail from `tiles` (the ones `plan`
/// names). `None` for an empty layer.
pub(crate) fn sample(
    width: u32,
    height: u32,
    plan: &Plan,
    tiles: &Decoded,
) -> Option<ThumbnailImage> {
    let (ox, oy, cw, ch) = fit(width, height)?;
    let texels = if width < cw || height < ch {
        sample_upscaled(width, height, (cw, ch), tiles)
    } else if plan.dense {
        sample_dense(width, height, (cw, ch), plan, tiles)
    } else {
        sample_sparse(width, height, (cw, ch), plan, tiles)
    };
    ThumbnailImage::new(ox, oy, cw, ch, texels)
}

/// A layer smaller than its picture (under 48 px a side): each texel is the
/// layer pixel its centre falls on.
fn sample_upscaled(
    width: u32,
    height: u32,
    (cw, ch): (u32, u32),
    tiles: &Decoded,
) -> Vec<[f32; 4]> {
    let at = |v: u32, n: u32, extent: u32| {
        #[allow(clippy::cast_possible_truncation)]
        let pixel = ((2 * u64::from(v) + 1) * u64::from(extent) / (2 * u64::from(n))) as u32;
        pixel.min(extent - 1)
    };
    (0..ch)
        .flat_map(|j| (0..cw).map(move |i| (i, j)))
        .map(|(i, j)| premultiplied(tiles, at(i, cw, width), at(j, ch, height)))
        .collect()
}

/// The exact area average: every pixel of every existing tile goes into
/// the texel its layer position falls in (a partition: texel `i` takes the
/// pixels with `x * cw / width == i`), divided by the texel's pixel count
/// (missing tiles count as transparent). One pass over at most
/// [`MAX_TILES`] tiles, whatever the layer's size.
#[allow(clippy::cast_precision_loss)]
fn sample_dense(
    width: u32,
    height: u32,
    (cw, ch): (u32, u32),
    plan: &Plan,
    tiles: &Decoded,
) -> Vec<[f32; 4]> {
    let bin =
        |v: u32, n: u32, extent: u32| (u64::from(v) * u64::from(n) / u64::from(extent)) as usize;
    let mut cols = vec![0_u32; cw as usize];
    for x in 0..width {
        if let Some(c) = cols.get_mut(bin(x, cw, width)) {
            *c += 1;
        }
    }
    let mut rows = vec![0_u32; ch as usize];
    for y in 0..height {
        if let Some(r) = rows.get_mut(bin(y, ch, height)) {
            *r += 1;
        }
    }
    let mut sums = vec![[0.0_f32; 4]; cw as usize * ch as usize];
    for id in &plan.tiles {
        let Some(texels) = tiles.get(id) else {
            continue;
        };
        for ty in 0..TILE {
            let y = id.y * TILE + ty;
            if y >= height {
                break;
            }
            let row = bin(y, ch, height) * cw as usize;
            for tx in 0..TILE {
                let x = id.x * TILE + tx;
                if x >= width {
                    break;
                }
                let at = (ty as usize * TILE as usize + tx as usize) * 4;
                let Some(Ok(px)) = texels.get(at..at + 4).map(<[f16; 4]>::try_from) else {
                    continue;
                };
                if let Some(sum) = sums.get_mut(row + bin(x, cw, width)) {
                    for (acc, v) in sum.iter_mut().zip(premultiply(px)) {
                        *acc += v;
                    }
                }
            }
        }
    }
    for (index, sum) in sums.iter_mut().enumerate() {
        let (i, j) = (index % cw as usize, index / cw as usize);
        let n = cols.get(i).copied().unwrap_or(1) as f32 * rows.get(j).copied().unwrap_or(1) as f32;
        if n > 0.0 {
            for v in sum.iter_mut() {
                *v /= n;
            }
        }
    }
    sums
}

/// The sampled picture of a layer with more than [`MAX_TILES`] existing
/// tiles: a texel whose own tile does not exist is transparent; one that
/// does takes a window of at most [`SPARSE_WINDOW`] pixels square at its
/// region's centre, moved into the nearest sampled tile.
fn sample_sparse(
    width: u32,
    height: u32,
    (cw, ch): (u32, u32),
    plan: &Plan,
    tiles: &Decoded,
) -> Vec<[f32; 4]> {
    let mut texels = Vec::with_capacity(cw as usize * ch as usize);
    for j in 0..ch {
        let y0 = (u64::from(j) * u64::from(height) / u64::from(ch)) as u32;
        let y1 = (((u64::from(j) + 1) * u64::from(height)).div_ceil(u64::from(ch)) as u32)
            .clamp(y0 + 1, height);
        for i in 0..cw {
            let x0 = (u64::from(i) * u64::from(width) / u64::from(cw)) as u32;
            let x1 = (((u64::from(i) + 1) * u64::from(width)).div_ceil(u64::from(cw)) as u32)
                .clamp(x0 + 1, width);
            let (cx, cy) = (x0 + (x1 - x0) / 2, y0 + (y1 - y0) / 2);
            let own = TileId {
                x: cx / TILE,
                y: cy / TILE,
            };
            let snapped = TileId {
                x: own.x / plan.step * plan.step,
                y: own.y / plan.step * plan.step,
            };
            if !plan.existing.contains(&snapped) {
                texels.push([0.0; 4]);
                continue;
            }
            let tile = plan
                .tiles
                .iter()
                .copied()
                .min_by_key(|t| t.x.abs_diff(own.x).max(t.y.abs_diff(own.y)))
                .unwrap_or(own);
            let window = |lo: u32, hi: u32, centre: u32, t: u32, limit: u32| {
                let size = (hi - lo).clamp(1, SPARSE_WINDOW);
                let (t0, t1) = (t * TILE, ((t + 1) * TILE).min(limit));
                let start = centre
                    .saturating_sub(size / 2)
                    .clamp(t0, t1.saturating_sub(size).max(t0));
                start..(start + size).min(t1.max(start + 1))
            };
            let (xs, ys) = (
                window(x0, x1, cx, tile.x, width),
                window(y0, y1, cy, tile.y, height),
            );
            let mut sum = [0.0_f32; 4];
            let mut n = 0.0_f32;
            for y in ys {
                for x in xs.clone() {
                    for (acc, v) in sum.iter_mut().zip(premultiplied(tiles, x, y)) {
                        *acc += v;
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                for v in &mut sum {
                    *v /= n;
                }
            }
            texels.push(sum);
        }
    }
    texels
}

// ---------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------

struct Entry {
    stamp: u64,
    image: Arc<ThumbnailImage>,
    bytes: usize,
    used: u64,
}

/// The picture cache: capped in entries and bytes, least recently used out.
pub(crate) struct ThumbnailCache {
    map: HashMap<LayerId, Entry>,
    bytes: usize,
    tick: u64,
    max_entries: usize,
    max_bytes: usize,
}

impl ThumbnailCache {
    pub(crate) fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            tick: 0,
            max_entries,
            max_bytes,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// The stamp `layer`'s cached picture was computed for, without
    /// touching its recency.
    pub(crate) fn stamp(&self, layer: LayerId) -> Option<u64> {
        self.map.get(&layer).map(|entry| entry.stamp)
    }

    /// `layer`'s picture (stale or not), marked used.
    pub(crate) fn get(&mut self, layer: LayerId) -> Option<Arc<ThumbnailImage>> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(&layer).map(|entry| {
            entry.used = tick;
            Arc::clone(&entry.image)
        })
    }

    pub(crate) fn insert(&mut self, layer: LayerId, stamp: u64, image: Arc<ThumbnailImage>) {
        self.tick += 1;
        let bytes = image.byte_size();
        if let Some(old) = self.map.insert(
            layer,
            Entry {
                stamp,
                image,
                bytes,
                used: self.tick,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        while (self.map.len() > self.max_entries || self.bytes > self.max_bytes)
            && self.map.len() > 1
        {
            let Some(victim) = self
                .map
                .iter()
                .filter(|&(&id, _)| id != layer)
                .min_by_key(|&(_, entry)| entry.used)
                .map(|(&id, _)| id)
            else {
                break;
            };
            if let Some(gone) = self.map.remove(&victim) {
                self.bytes -= gone.bytes;
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    /// Drops every entry whose layer `keep` refuses.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(LayerId) -> bool) {
        let mut freed = 0;
        self.map.retain(|&id, entry| {
            let kept = keep(id);
            if !kept {
                freed += entry.bytes;
            }
            kept
        });
        self.bytes -= freed;
    }
}

// ---------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------

struct Job {
    layer: LayerId,
    generation: u64,
    stamp: u64,
    width: u32,
    height: u32,
    plan: Arc<Plan>,
    tiles: Vec<(TileId, TileShare)>,
}

struct Finished {
    layer: LayerId,
    generation: u64,
    stamp: u64,
    image: Option<Arc<ThumbnailImage>>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    shutdown: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// Queues `job`: newest wins for a layer (a queued job of the same layer is
/// replaced), and a full queue refuses a job for a new layer.
fn enqueue(queue: &mut Queue, job: Job) -> bool {
    if let Some(at) = queue
        .jobs
        .iter()
        .position(|queued| queued.layer == job.layer)
    {
        queue.jobs.remove(at);
    } else if queue.jobs.len() >= QUEUE_LIMIT {
        return false;
    }
    queue.jobs.push_back(job);
    true
}

fn compute(job: &Job) -> Option<ThumbnailImage> {
    let mut decoded: Decoded = HashMap::new();
    for (id, share) in &job.tiles {
        let texels = match share {
            TileShare::Texels(texels) => Some(Arc::clone(texels)),
            TileShare::Encoded(bytes) => aurora_tile::codec::decode(bytes).ok().map(Arc::new),
        };
        if let Some(texels) = texels {
            decoded.insert(*id, texels);
        }
    }
    sample(job.width, job.height, &job.plan, &decoded)
}

fn run_worker(
    shared: &Shared,
    latest: &AtomicU64,
    tx: &mpsc::Sender<Finished>,
    wake_pending: &AtomicBool,
    waker: &Waker,
) {
    loop {
        let job = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            loop {
                if queue.shutdown {
                    return;
                }
                if let Some(job) = queue.jobs.pop_front() {
                    break job;
                }
                queue = match shared.wake.wait(queue) {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
            }
        };
        if job.generation != latest.load(Ordering::Acquire) {
            continue;
        }
        let image = compute(&job).map(Arc::new);
        if job.generation != latest.load(Ordering::Acquire) {
            continue;
        }
        let finished = Finished {
            layer: job.layer,
            generation: job.generation,
            stamp: job.stamp,
            image,
        };
        if tx.send(finished).is_err() {
            return;
        }
        if !wake_pending.swap(true, Ordering::AcqRel) {
            waker();
        }
    }
}

// ---------------------------------------------------------------------
// The UI-side service
// ---------------------------------------------------------------------

/// Counters for tests and the PLAN figures.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stats {
    pub jobs_submitted: usize,
    pub tiles_shared: usize,
    pub results_installed: usize,
    pub results_dropped: usize,
    /// Tiles read from the scratch disk on the UI thread, in all.
    pub scratch_reads: usize,
    /// Times a layer's tiles were not all gathered in one turn.
    pub deferred: usize,
}

pub(crate) struct LayerThumbnails {
    waker: Waker,
    shared: Arc<Shared>,
    tx: Option<mpsc::Sender<Finished>>,
    rx: Option<mpsc::Receiver<Finished>>,
    handle: Option<std::thread::JoinHandle<()>>,
    latest: Arc<AtomicU64>,
    wake_pending: Arc<AtomicBool>,
    generation: u64,
    doc_key: Option<(DocumentId, u64)>,
    cache: ThumbnailCache,
    /// Layers with a job out, and the stamp it was made for.
    pending: HashMap<LayerId, u64>,
    last_request: HashMap<LayerId, Instant>,
    memo: HashMap<LayerId, Memo>,
    gathering: HashMap<LayerId, Gather>,
    /// Rows this service has put a picture into.
    shown: Vec<WidgetId>,
    pub(crate) stats: Stats,
}

impl std::fmt::Debug for LayerThumbnails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerThumbnails")
            .field("cached", &self.cache.len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

/// A stamp over a layer's plan: changes when a sampled tile's content
/// version changes, when the set of existing tiles changes (a new tile, a
/// forgotten one) or when the layer's size does.
fn stamp_of(
    store: &TileStore,
    surface: aurora_tile::SurfaceId,
    plan: &Plan,
    width: u32,
    height: u32,
) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        hash ^= v;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(u64::from(width));
    mix(u64::from(height));
    mix(plan.existing_count as u64);
    for &id in &plan.tiles {
        mix(u64::from(id.x) << 32 | u64::from(id.y));
        mix(store.content_version(surface, id).wrapping_add(1));
    }
    hash
}

/// A layer's plan and stamp, valid while the store's content epoch and the
/// layer's size stay as they were.
struct Memo {
    epoch: u64,
    width: u32,
    height: u32,
    plan: Arc<Plan>,
    stamp: u64,
    checked: Instant,
}

/// The shares gathered so far for one layer's job: a layer with tiles on
/// the scratch disk is gathered over several turns.
struct Gather {
    stamp: u64,
    plan: Arc<Plan>,
    tiles: Vec<(TileId, TileShare)>,
    next: usize,
}

impl LayerThumbnails {
    pub(crate) fn new(waker: Waker) -> Self {
        Self::with_caps(waker, CACHE_ENTRIES, CACHE_BYTES)
    }

    pub(crate) fn with_caps(waker: Waker, entries: usize, bytes: usize) -> Self {
        Self {
            waker,
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                wake: Condvar::new(),
            }),
            tx: None,
            rx: None,
            handle: None,
            latest: Arc::new(AtomicU64::new(0)),
            wake_pending: Arc::new(AtomicBool::new(false)),
            generation: 0,
            doc_key: None,
            cache: ThumbnailCache::new(entries, bytes),
            pending: HashMap::new(),
            last_request: HashMap::new(),
            memo: HashMap::new(),
            gathering: HashMap::new(),
            shown: Vec::new(),
            stats: Stats::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn cache(&self) -> &ThumbnailCache {
        &self.cache
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Forgets everything about the previous document and discards its
    /// in-flight work.
    fn reset(&mut self) {
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Release);
        if let Ok(mut queue) = self.shared.queue.lock() {
            queue.jobs.clear();
        }
        self.cache.clear();
        self.pending.clear();
        self.last_request.clear();
        self.memo.clear();
        self.gathering.clear();
        self.shown.clear();
    }

    fn ensure_thread(&mut self) -> bool {
        if self.handle.is_some() {
            return true;
        }
        let (tx, rx) = mpsc::channel();
        let (shared, latest, wake_pending, waker) = (
            Arc::clone(&self.shared),
            Arc::clone(&self.latest),
            Arc::clone(&self.wake_pending),
            Arc::clone(&self.waker),
        );
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("aurora-layer-thumbnails".to_owned())
            .spawn(move || run_worker(&shared, &latest, &worker_tx, &wake_pending, &waker));
        match spawned {
            Ok(handle) => {
                self.handle = Some(handle);
                self.tx = Some(tx);
                self.rx = Some(rx);
                true
            }
            Err(err) => {
                tracing::warn!(?err, "could not start the layer thumbnail worker");
                false
            }
        }
    }

    /// Whether the queue could take a job for `layer` now (checked before
    /// any shares are taken, so refused work is never done).
    fn has_room(&self, layer: LayerId) -> bool {
        self.shared.queue.lock().is_ok_and(|queue| {
            queue.jobs.len() < QUEUE_LIMIT || queue.jobs.iter().any(|queued| queued.layer == layer)
        })
    }

    fn submit(&mut self, job: Job) -> bool {
        if !self.ensure_thread() {
            return false;
        }
        let Ok(mut queue) = self.shared.queue.lock() else {
            return false;
        };
        let accepted = enqueue(&mut queue, job);
        drop(queue);
        self.shared.wake.notify_one();
        accepted
    }

    /// One event-loop turn: notices a different document, forgets deleted
    /// layers, and requests a picture for each of the `visible` layers that
    /// has none for its current content. Returns when to call again, if a
    /// throttled or deferred refresh is waiting.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn update(
        &mut self,
        doc: (DocumentId, u64),
        layers: &LayerTree,
        store: &mut TileStore,
        visible: &[LayerId],
        now: Instant,
    ) -> Option<Instant> {
        if self.doc_key != Some(doc) {
            self.reset();
            self.doc_key = Some(doc);
        }
        self.cache
            .retain(|id| matches!(layers.kind(id), Some(LayerKind::Pixel { .. })));
        self.pending.retain(|&id, _| layers.contains(id));
        self.last_request.retain(|&id, _| layers.contains(id));
        self.memo.retain(|id, _| visible.contains(id));
        self.gathering.retain(|id, _| visible.contains(id));
        let epoch = store.content_epoch();
        let mut scratch_left = SCRATCH_READS_PER_TURN;
        let mut deadline: Option<Instant> = None;
        let mut soonest = |at: Instant| {
            deadline = Some(deadline.map_or(at, |d| d.min(at)));
        };
        let mut submitted = 0;
        for &layer in visible {
            let Some(LayerKind::Pixel { bounds }) = layers.kind(layer) else {
                continue;
            };
            let (width, height) = (bounds.width, bounds.height);
            let Some(surface) = layers.surface_id(layer) else {
                continue;
            };
            if width == 0 || height == 0 {
                continue;
            }
            let current = self
                .memo
                .get(&layer)
                .is_some_and(|m| m.epoch == epoch && m.width == width && m.height == height);
            if !current {
                if let Some(m) = self.memo.get(&layer)
                    && m.width == width
                    && m.height == height
                    && self.cache.stamp(layer).is_some()
                    && now < m.checked + CHECK_INTERVAL
                {
                    // The store changed, but this layer was looked at a
                    // moment ago: look again when the interval ends.
                    soonest(m.checked + CHECK_INTERVAL);
                    continue;
                }
                let plan = Arc::new(plan(width, height, &mut |id| {
                    store.contains_tile(surface, id)
                }));
                let stamp = stamp_of(store, surface, &plan, width, height);
                self.memo.insert(
                    layer,
                    Memo {
                        epoch,
                        width,
                        height,
                        plan,
                        stamp,
                        checked: now,
                    },
                );
            }
            let Some((plan, stamp)) = self
                .memo
                .get(&layer)
                .map(|m| (Arc::clone(&m.plan), m.stamp))
            else {
                continue;
            };
            if self.cache.stamp(layer) == Some(stamp) || self.pending.get(&layer) == Some(&stamp) {
                self.gathering.remove(&layer);
                continue;
            }
            if self.cache.stamp(layer).is_some()
                && let Some(&last) = self.last_request.get(&layer)
                && now < last + REFRESH_INTERVAL
            {
                soonest(last + REFRESH_INTERVAL);
                continue;
            }
            if submitted >= MAX_JOBS_PER_UPDATE {
                soonest(now);
                break;
            }
            if !self.has_room(layer) {
                continue;
            }
            let mut gather = match self.gathering.remove(&layer) {
                Some(gather) if gather.stamp == stamp => gather,
                _ => Gather {
                    stamp,
                    plan: Arc::clone(&plan),
                    tiles: Vec::new(),
                    next: 0,
                },
            };
            while let Some(&id) = gather.plan.tiles.get(gather.next) {
                match store.share_tile_without_io(surface, id) {
                    TileLookup::Shared(share) => gather.tiles.push((id, share)),
                    TileLookup::Absent => {}
                    TileLookup::OnDisk => {
                        if scratch_left == 0 {
                            break;
                        }
                        scratch_left -= 1;
                        self.stats.scratch_reads += 1;
                        if let Ok(Some(share)) = store.share_tile(surface, id) {
                            gather.tiles.push((id, share));
                        }
                    }
                }
                gather.next += 1;
            }
            if gather.next < gather.plan.tiles.len() {
                self.gathering.insert(layer, gather);
                self.stats.deferred += 1;
                soonest(now);
                continue;
            }
            let shared = gather.tiles.len();
            let job = Job {
                layer,
                generation: self.generation,
                stamp,
                width,
                height,
                plan,
                tiles: gather.tiles,
            };
            if self.submit(job) {
                submitted += 1;
                self.pending.insert(layer, stamp);
                self.last_request.insert(layer, now);
                self.stats.jobs_submitted += 1;
                self.stats.tiles_shared += shared;
            }
        }
        deadline
    }

    /// Installs every finished picture that is still wanted; returns the
    /// layers whose picture changed.
    pub(crate) fn drain(&mut self) -> Vec<LayerId> {
        self.wake_pending.store(false, Ordering::Release);
        let mut changed = Vec::new();
        let Some(rx) = self.rx.as_ref() else {
            return changed;
        };
        let results: Vec<Finished> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        for done in results {
            if done.generation != self.generation {
                self.stats.results_dropped += 1;
                continue;
            }
            if self.pending.get(&done.layer) == Some(&done.stamp) {
                self.pending.remove(&done.layer);
            }
            match done.image {
                Some(image) => {
                    self.cache.insert(done.layer, done.stamp, image);
                    self.stats.results_installed += 1;
                    changed.push(done.layer);
                }
                None => self.stats.results_dropped += 1,
            }
        }
        changed
    }

    /// Puts each visible pixel row's cached picture into its slot and blanks
    /// the slots of rows that left the visible set (so widgets never hold
    /// pictures beyond the visible window). Returns whether anything
    /// changed.
    pub(crate) fn apply(
        &mut self,
        tree: &mut WidgetTree<WidgetKind>,
        visible: &[(WidgetId, LayerId)],
    ) -> bool {
        let mut changed = false;
        let now_shown: Vec<WidgetId> = visible.iter().map(|&(row, _)| row).collect();
        for &row in &self.shown {
            if !now_shown.contains(&row) && tree.contains(row) {
                changed |= aurora_ui::set_layer_row_thumbnail(tree, row, None).unwrap_or(false);
            }
        }
        for &(row, layer) in visible {
            let image = self.cache.get(layer);
            changed |= aurora_ui::set_layer_row_thumbnail(tree, row, image).unwrap_or(false);
        }
        self.shown = now_shown;
        changed
    }

    /// Stops the worker and joins it.
    pub(crate) fn shutdown(&mut self) {
        if let Ok(mut queue) = self.shared.queue.lock() {
            queue.shutdown = true;
            queue.jobs.clear();
        }
        self.shared.wake.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.tx = None;
    }
}

impl Drop for LayerThumbnails {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::many_single_char_names
)]
mod tests {
    use super::*;
    use aurora_core::Rect;

    fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn rect(w: u32, h: u32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    fn store() -> (tempfile::TempDir, TileStore) {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(256) else {
            unreachable!("non-zero");
        };
        let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
        (dir, store)
    }

    /// A whole tile of one straight-alpha colour.
    fn tile(rgba: [f32; 4]) -> Arc<Vec<f16>> {
        Arc::new(
            (0..aurora_tile::TEXELS)
                .flat_map(|_| rgba.map(f16::from_f32))
                .collect(),
        )
    }

    /// A tile whose red is `x / 255` and green `y / 255`, opaque.
    fn gradient_tile() -> Arc<Vec<f16>> {
        let mut out = Vec::new();
        for y in 0..TILE {
            for x in 0..TILE {
                out.extend([
                    f16::from_f32(x as f32 / 255.0),
                    f16::from_f32(y as f32 / 255.0),
                    f16::from_f32(0.0),
                    f16::ONE,
                ]);
            }
        }
        Arc::new(out)
    }

    /// `sample` with the plan the given tiles imply (every held tile exists).
    fn sample_of(width: u32, height: u32, tiles: &Decoded) -> Option<ThumbnailImage> {
        let p = plan(width, height, &mut |id| tiles.contains_key(&id));
        sample(width, height, &p, tiles)
    }

    fn one_tile(texels: Arc<Vec<f16>>) -> Decoded {
        HashMap::from([(TileId { x: 0, y: 0 }, texels)])
    }

    fn close(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
    }

    fn texel(image: &ThumbnailImage, cx: u32, cy: u32) -> [f32; 4] {
        image.texel(cx, cy).unwrap_or([f32::NAN; 4])
    }

    // ---- sampling ----

    #[test]
    fn a_solid_opaque_layer_samples_to_its_colour_everywhere() {
        let image =
            ok(sample_of(100, 100, &one_tile(tile([0.25, 0.5, 0.75, 1.0]))).ok_or("no image"));
        assert_eq!(
            (image.x(), image.y(), image.width(), image.height()),
            (0, 0, 48, 48)
        );
        assert!(
            image
                .texels()
                .iter()
                .all(|t| close(*t, [0.25, 0.5, 0.75, 1.0], 1e-3))
        );
    }

    #[test]
    fn a_gradient_samples_to_the_area_average_of_each_texel() {
        // 256 x 256 into 48 x 48: texel i covers x in [i*256/48, (i+1)*256/48).
        let image = ok(sample_of(256, 256, &one_tile(gradient_tile())).ok_or("no image"));
        for i in [0_u32, 1, 17, 47] {
            // Texel i takes the pixels with x * 48 / 256 == i.
            let lo = (u64::from(i) * 256).div_ceil(48) as u32;
            let hi = ((u64::from(i) + 1) * 256).div_ceil(48) as u32;
            let mean: f32 = (lo..hi)
                .map(|x| f16::from_f32(x as f32 / 255.0).to_f32())
                .sum::<f32>()
                / (hi - lo) as f32;
            let got = texel(&image, i, 5);
            assert!(
                (got[0] - mean).abs() < 1e-4,
                "red of column {i}: {got:?} vs {mean}"
            );
        }
        let (first, last) = (texel(&image, 0, 0), texel(&image, 47, 47));
        assert!(first[0] < 0.05 && last[0] > 0.95 && last[1] > 0.95);
    }

    #[test]
    fn a_half_transparent_layer_is_premultiplied() {
        // Straight (1, 0, 0) at alpha 0.5 is premultiplied (0.5, 0, 0, 0.5).
        let image = ok(sample_of(64, 64, &one_tile(tile([1.0, 0.0, 0.0, 0.5]))).ok_or("no image"));
        assert!(
            image
                .texels()
                .iter()
                .all(|t| close(*t, [0.5, 0.0, 0.0, 0.5], 1e-3))
        );
    }

    #[test]
    fn transparent_texels_do_not_bleed_their_colour_into_the_average() {
        // Left half opaque black, right half fully transparent *white*: an
        // alpha-ignoring average would give grey; the right answer is
        // black at alpha 0.5 where they meet.
        let mut texels = Vec::new();
        for _y in 0..TILE {
            for x in 0..TILE {
                let opaque = x < 128;
                texels.extend(if opaque {
                    [0.0, 0.0, 0.0, 1.0].map(f16::from_f32)
                } else {
                    [1.0, 1.0, 1.0, 0.0].map(f16::from_f32)
                });
            }
        }
        let image = ok(sample_of(256, 256, &one_tile(Arc::new(texels))).ok_or("no image"));
        assert!(close(texel(&image, 2, 10), [0.0, 0.0, 0.0, 1.0], 1e-3));
        assert!(close(texel(&image, 45, 10), [0.0, 0.0, 0.0, 0.0], 1e-3));
    }

    #[test]
    fn the_picture_keeps_the_layers_aspect_and_is_centred() {
        let wide = ok(sample_of(200, 100, &one_tile(tile([1.0; 4]))).ok_or("no image"));
        assert_eq!(
            (wide.x(), wide.y(), wide.width(), wide.height()),
            (0, 12, 48, 24)
        );
        let tall = ok(sample_of(100, 400, &one_tile(tile([1.0; 4]))).ok_or("no image"));
        assert_eq!(
            (tall.x(), tall.y(), tall.width(), tall.height()),
            (18, 0, 12, 48)
        );
        assert!(sample_of(0, 10, &Decoded::new()).is_none());
    }

    #[test]
    fn a_layer_larger_than_a_tile_reads_each_of_its_tiles() {
        // 512 x 256: left tile red, right tile blue.
        let tiles: Decoded = HashMap::from([
            (TileId { x: 0, y: 0 }, tile([1.0, 0.0, 0.0, 1.0])),
            (TileId { x: 1, y: 0 }, tile([0.0, 0.0, 1.0, 1.0])),
        ]);
        let image = ok(sample_of(512, 256, &tiles).ok_or("no image"));
        assert_eq!((image.width(), image.height()), (48, 24));
        assert!(close(texel(&image, 3, 3), [1.0, 0.0, 0.0, 1.0], 1e-3));
        assert!(close(texel(&image, 44, 3), [0.0, 0.0, 1.0, 1.0], 1e-3));
    }

    #[test]
    fn a_missing_tile_is_transparent() {
        let tiles: Decoded = HashMap::from([(TileId { x: 0, y: 0 }, tile([1.0; 4]))]);
        let image = ok(sample_of(512, 256, &tiles).ok_or("no image"));
        assert!(close(texel(&image, 3, 3), [1.0; 4], 1e-3));
        assert!(close(texel(&image, 44, 3), [0.0; 4], 1e-6));
    }

    #[test]
    fn a_layer_smaller_than_a_tile_is_scaled_up_not_padded() {
        let image = ok(sample_of(10, 10, &one_tile(tile([0.5, 0.5, 0.5, 1.0]))).ok_or("no image"));
        assert_eq!((image.width(), image.height()), (48, 48));
        assert!(
            image
                .texels()
                .iter()
                .all(|t| close(*t, [0.5, 0.5, 0.5, 1.0], 1e-3))
        );
    }

    // ---- the read bound ----

    #[test]
    fn a_thumbnail_never_reads_more_than_sixteen_tiles_or_probes_more_than_the_scan_limit() {
        for (w, h) in [
            (1, 1),
            (256, 256),
            (1024, 1024),
            (1025, 1024),
            (4096, 4096),
            (300_000, 300_000),
            (300_000, 256),
            (256, 300_000),
            (6000, 2500),
            (7000, 100),
        ] {
            let mut probes = 0_u64;
            let p = plan(w, h, &mut |_| {
                probes += 1;
                true
            });
            assert!(
                (1..=MAX_TILES).contains(&p.tiles.len()),
                "{w}x{h} reads {} tiles",
                p.tiles.len()
            );
            assert!(probes <= SCAN_LIMIT, "{w}x{h} probed {probes} cells");
            let (gw, gh) = (w.div_ceil(TILE), h.div_ceil(TILE));
            assert!(p.tiles.iter().all(|t| t.x < gw && t.y < gh));
            assert_eq!(p.dense, p.existing_count <= MAX_TILES, "{w}x{h}");
        }
        // Exactly 16 existing tiles is dense, 17 is sparse.
        assert!(plan(1024, 1024, &mut |_| true).dense);
        assert!(!plan(1025, 1025, &mut |_| true).dense);
    }

    #[test]
    fn a_big_layer_painted_in_one_corner_is_not_blank() {
        // 3000 x 2000 = 12 x 8 = 96 grid cells; only tile (0, 0) exists.
        let tiles: Decoded = HashMap::from([(TileId { x: 0, y: 0 }, tile([1.0, 0.0, 0.0, 1.0]))]);
        let p = plan(3000, 2000, &mut |id| tiles.contains_key(&id));
        assert!(p.dense && p.tiles == vec![TileId { x: 0, y: 0 }]);
        let image = ok(sample(3000, 2000, &p, &tiles).ok_or("no image"));
        let (w, h) = (image.width(), image.height());
        assert!(
            close(texel(&image, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3),
            "the corner shows"
        );
        assert!(
            close(texel(&image, w - 1, h - 1), [0.0; 4], 1e-6),
            "the rest is empty"
        );
        // Area: tile (0,0) is 256 x 256 of 3000 x 2000 pixels.
        let covered: f32 = image.texels().iter().map(|t| t[3]).sum();
        let expected = 256.0 * 256.0 / (3000.0 * 2000.0) * (w * h) as f32;
        assert!(
            (covered - expected).abs() < expected * 0.35,
            "{covered} vs {expected}"
        );
    }

    #[test]
    fn a_sparse_sample_leaves_texels_over_missing_tiles_transparent() {
        // 20 existing tiles on the top rows of a 3000 x 2000 layer.
        let exist: HashSet<TileId> = (0..12)
            .flat_map(|x| (0..2).map(move |y| TileId { x, y }))
            .collect();
        let p = plan(3000, 2000, &mut |id| exist.contains(&id));
        assert!(!p.dense && p.tiles.len() <= MAX_TILES && p.existing_count == 24);
        assert!(
            p.tiles.iter().all(|t| exist.contains(t)),
            "only existing tiles are read"
        );
        let tiles: Decoded = p
            .tiles
            .iter()
            .map(|&id| (id, tile([0.0, 1.0, 0.0, 1.0])))
            .collect();
        let image = ok(sample(3000, 2000, &p, &tiles).ok_or("no image"));
        let (w, h) = (image.width(), image.height());
        // The top 512 of 2000 rows are painted: about a quarter of the picture.
        assert!(close(texel(&image, w / 2, 1), [0.0, 1.0, 0.0, 1.0], 1e-3));
        assert!(close(texel(&image, w / 2, h - 2), [0.0; 4], 1e-6));
    }

    #[test]
    fn a_non_finite_texel_counts_as_transparent_black() {
        let mut texels = vec![f16::ZERO; aurora_tile::SAMPLES];
        for texel in texels.chunks_exact_mut(4) {
            texel.copy_from_slice(&[f16::NAN, f16::INFINITY, f16::NEG_INFINITY, f16::ONE]);
        }
        let image = ok(sample_of(256, 256, &one_tile(Arc::new(texels))).ok_or("no image"));
        assert!(
            image
                .texels()
                .iter()
                .all(|t| t.iter().all(|v| v.is_finite()))
        );
        assert!(close(texel(&image, 5, 5), [0.0, 0.0, 0.0, 1.0], 1e-6));
    }

    #[test]
    fn a_sparse_sample_of_uniform_tiles_is_uniform() {
        let p = plan(4096, 4096, &mut |_| true);
        let tiles: Decoded = p
            .tiles
            .iter()
            .map(|&id| (id, tile([0.2, 0.4, 0.6, 1.0])))
            .collect();
        let image = ok(sample(4096, 4096, &p, &tiles).ok_or("no image"));
        assert!(
            image
                .texels()
                .iter()
                .all(|t| close(*t, [0.2, 0.4, 0.6, 1.0], 1e-3))
        );
    }

    // ---- cache ----

    fn picture(v: f32) -> Arc<ThumbnailImage> {
        Arc::new(ok(
            ThumbnailImage::new(0, 0, 2, 2, vec![[v; 4]; 4]).ok_or("fits")
        ))
    }

    fn ids(n: usize) -> Vec<LayerId> {
        let mut layers = LayerTree::new();
        (0..n)
            .map(|i| ok(layers.add_pixel_layer(format!("L{i}"), rect(8, 8), None)))
            .collect()
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_entry_over_its_entry_cap() {
        let layer = ids(4);
        let mut cache = ThumbnailCache::new(3, usize::MAX);
        for (i, id) in layer.iter().take(3).enumerate() {
            cache.insert(*id, i as u64, picture(i as f32));
        }
        // Touch the oldest; the next oldest goes.
        assert!(cache.get(layer[0]).is_some());
        cache.insert(layer[3], 9, picture(0.9));
        assert_eq!(cache.len(), 3);
        assert!(
            cache.stamp(layer[1]).is_none(),
            "the least recently used went"
        );
        assert!(cache.stamp(layer[0]).is_some() && cache.stamp(layer[3]).is_some());
    }

    #[test]
    fn the_cache_keeps_its_byte_cap_and_never_evicts_the_entry_just_added() {
        let layer = ids(5);
        let each = picture(0.0).byte_size();
        let mut cache = ThumbnailCache::new(100, each * 2);
        for (i, id) in layer.iter().enumerate() {
            cache.insert(*id, i as u64, picture(i as f32));
            assert!(cache.bytes() <= each * 2, "after {i}: {}", cache.bytes());
            assert!(cache.stamp(*id).is_some());
        }
        assert_eq!(cache.len(), 2);
        // A cap below one picture still keeps the newest.
        let mut tiny = ThumbnailCache::new(100, 1);
        tiny.insert(layer[0], 1, picture(0.0));
        assert_eq!(tiny.len(), 1);
    }

    #[test]
    fn replacing_an_entry_does_not_leak_its_bytes() {
        let layer = ids(1);
        let mut cache = ThumbnailCache::new(8, usize::MAX);
        cache.insert(layer[0], 1, picture(0.1));
        let once = cache.bytes();
        cache.insert(layer[0], 2, picture(0.2));
        assert_eq!((cache.len(), cache.bytes()), (1, once));
        cache.retain(|_| false);
        assert_eq!((cache.len(), cache.bytes()), (0, 0));
    }

    // ---- the queue ----

    fn job(layer: LayerId, generation: u64) -> Job {
        Job {
            layer,
            generation,
            stamp: 0,
            width: 8,
            height: 8,
            plan: Arc::new(plan(8, 8, &mut |_| true)),
            tiles: Vec::new(),
        }
    }

    #[test]
    fn a_queued_job_is_replaced_by_a_newer_one_for_the_same_layer() {
        let layer = ids(2);
        let mut queue = Queue::default();
        assert!(enqueue(&mut queue, job(layer[0], 1)));
        assert!(enqueue(&mut queue, job(layer[1], 1)));
        assert!(enqueue(&mut queue, job(layer[0], 2)));
        assert_eq!(queue.jobs.len(), 2, "coalesced, not appended");
        assert!(
            queue
                .jobs
                .iter()
                .any(|j| j.layer == layer[0] && j.generation == 2)
        );
        assert!(
            !queue
                .jobs
                .iter()
                .any(|j| j.layer == layer[0] && j.generation == 1)
        );
    }

    #[test]
    fn a_full_queue_refuses_a_job_for_a_new_layer_but_takes_a_replacement() {
        let layer = ids(QUEUE_LIMIT + 1);
        let mut queue = Queue::default();
        for id in layer.iter().take(QUEUE_LIMIT) {
            assert!(enqueue(&mut queue, job(*id, 1)));
        }
        assert!(!enqueue(&mut queue, job(layer[QUEUE_LIMIT], 1)));
        assert!(enqueue(&mut queue, job(layer[0], 2)));
        assert_eq!(queue.jobs.len(), QUEUE_LIMIT);
    }

    // ---- the service, with its real worker ----

    struct Fixture {
        scratch: tempfile::TempDir,
        store: TileStore,
        layers: LayerTree,
        service: LayerThumbnails,
        doc: (DocumentId, u64),
        woken: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Fixture {
        fn new(count: usize) -> (Self, Vec<LayerId>) {
            let (dir, mut store) = store();
            let mut layers = LayerTree::new();
            let mut ids = Vec::new();
            for i in 0..count {
                let id = ok(layers.add_pixel_layer(format!("L{i}"), rect(300, 200), None));
                if let Some(surface) = layers.surface_id(id) {
                    for x in 0..2 {
                        let t = ok(store.get_mut(surface, TileId { x, y: 0 }));
                        let shade = f16::from_f32(0.1 + 0.8 * (i % 10) as f32 / 10.0);
                        for texel in t.texels_mut().chunks_exact_mut(4) {
                            texel.copy_from_slice(&[shade, shade, shade, f16::ONE]);
                        }
                    }
                }
                ids.push(id);
            }
            let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counter = Arc::clone(&woken);
            let service = LayerThumbnails::new(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));
            let doc = (DocumentId::next(), store.serial());
            (
                Self {
                    scratch: dir,
                    store,
                    layers,
                    service,
                    doc,
                    woken,
                },
                ids,
            )
        }

        /// Runs update + drain until nothing is pending (or 10 s pass).
        fn settle(&mut self, visible: &[LayerId], mut now: Instant) -> Instant {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let _ = self
                    .service
                    .update(self.doc, &self.layers, &mut self.store, visible, now);
                let _ = self.service.drain();
                if self.service.pending_len() == 0 {
                    return now;
                }
                assert!(Instant::now() < end, "the worker never finished");
                std::thread::sleep(Duration::from_millis(2));
                now += Duration::from_millis(2);
            }
        }
    }

    #[test]
    fn a_visible_layer_gets_a_picture_from_the_worker_and_a_wake() {
        let (mut fx, ids) = Fixture::new(1);
        fx.settle(&ids, Instant::now());
        assert!(fx.service.cache().stamp(ids[0]).is_some());
        assert!(fx.woken.load(Ordering::SeqCst) >= 1, "the loop was woken");
        assert!(fx.service.stats.tiles_shared <= MAX_TILES);
        assert_eq!(fx.service.stats.jobs_submitted, 1);
    }

    #[test]
    fn a_structural_edit_hits_the_cache_and_a_dab_misses_only_its_layer() {
        let (mut fx, ids) = Fixture::new(2);
        let t0 = fx.settle(&ids, Instant::now());
        let jobs = fx.service.stats.jobs_submitted;
        assert_eq!(jobs, 2);
        // Opacity, visibility and blend mode change no pixels.
        ok(fx.layers.set_opacity(ids[0], 0.3));
        ok(fx.layers.set_visible(ids[0], false));
        ok(fx
            .layers
            .set_blend_mode(ids[0], aurora_doc::BlendMode::Multiply));
        fx.settle(&ids, t0 + Duration::from_secs(1));
        assert_eq!(fx.service.stats.jobs_submitted, jobs, "no recompute");
        // A dab on layer 1 only.
        let Some(surface) = fx.layers.surface_id(ids[1]) else {
            unreachable!("pixel layer");
        };
        let t = ok(fx.store.get_mut(surface, TileId { x: 0, y: 0 }));
        if let Some(first) = t.texels_mut().first_mut() {
            *first = f16::ONE;
        }
        let before = fx.service.cache().stamp(ids[1]);
        fx.settle(&ids, t0 + Duration::from_secs(2));
        assert_eq!(
            fx.service.stats.jobs_submitted,
            jobs + 1,
            "only the dabbed layer"
        );
        assert_ne!(fx.service.cache().stamp(ids[1]), before);
    }

    #[test]
    fn a_changing_layer_is_refreshed_at_most_every_250_ms() {
        let (mut fx, ids) = Fixture::new(1);
        let t0 = fx.settle(&ids, Instant::now());
        let Some(surface) = fx.layers.surface_id(ids[0]) else {
            unreachable!("pixel layer");
        };
        let mut deadline = None;
        for k in 1..=5_u64 {
            let t = ok(fx.store.get_mut(surface, TileId { x: 0, y: 0 }));
            if let Some(first) = t.texels_mut().first_mut() {
                *first = f16::from_f32(k as f32 / 10.0);
            }
            deadline = fx.service.update(
                fx.doc,
                &fx.layers,
                &mut fx.store,
                &ids,
                t0 + Duration::from_millis(10 * k),
            );
        }
        assert_eq!(
            fx.service.stats.jobs_submitted, 1,
            "five dabs inside 250 ms: no recompute"
        );
        let due = fx
            .service
            .last_request
            .get(&ids[0])
            .map(|last| *last + REFRESH_INTERVAL);
        assert!(due.is_some_and(|due| due > t0));
        assert!(
            deadline.is_some_and(|d| d <= due.unwrap_or(t0) && d > t0),
            "but a look is due before the refresh interval ends: {deadline:?} vs {due:?}"
        );
        fx.settle(&ids, due.unwrap_or(t0));
        assert_eq!(fx.service.stats.jobs_submitted, 2, "once after the strokes");
    }

    #[test]
    fn a_deleted_layers_entry_is_dropped() {
        let (mut fx, ids) = Fixture::new(2);
        fx.settle(&ids, Instant::now());
        assert_eq!(fx.service.cache().len(), 2);
        ok(fx.layers.remove(ids[0]));
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &ids[1..], Instant::now());
        assert_eq!(fx.service.cache().len(), 1);
        assert!(fx.service.cache().stamp(ids[0]).is_none());
    }

    #[test]
    fn a_document_switch_drops_the_cache_and_discards_an_in_flight_result() {
        let (mut fx, ids) = Fixture::new(1);
        let t0 = Instant::now();
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &ids, t0);
        // Switch before draining: whatever the worker made is for the old one.
        fx.doc = (DocumentId::next(), fx.doc.1);
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[], t0);
        assert_eq!(fx.service.cache().len(), 0);
        assert_eq!(fx.service.pending_len(), 0);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            fx.service.drain().is_empty(),
            "nothing installs for the old document"
        );
        assert_eq!(fx.service.cache().len(), 0);
    }

    #[test]
    fn a_job_of_a_stale_generation_is_skipped_by_the_worker() {
        let (mut fx, ids) = Fixture::new(2);
        let Some(surface) = fx.layers.surface_id(ids[0]) else {
            unreachable!("pixel layer");
        };
        let tiles = |store: &mut TileStore| -> Vec<(TileId, TileShare)> {
            (0..2)
                .filter_map(|x| {
                    let id = TileId { x, y: 0 };
                    store
                        .share_tile(surface, id)
                        .ok()
                        .flatten()
                        .map(|s| (id, s))
                })
                .collect()
        };
        // Establish generation 1, then queue a stale (0) job and a live one.
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[], Instant::now());
        let live = fx.service.generation;
        let mut stale = job(ids[0], live - 1);
        stale.tiles = tiles(&mut fx.store);
        let mut good = job(ids[1], live);
        good.tiles = tiles(&mut fx.store);
        assert!(fx.service.submit(stale));
        assert!(fx.service.submit(good));
        fx.service.pending.insert(ids[1], 0);
        fx.settle(&[], Instant::now());
        let end = Instant::now() + Duration::from_secs(10);
        while fx.service.cache().stamp(ids[1]).is_none() {
            let _ = fx.service.drain();
            assert!(Instant::now() < end, "the live job never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            fx.service.cache().stamp(ids[0]).is_none(),
            "the stale job produced nothing"
        );
        assert_eq!(
            fx.service.stats.results_dropped, 0,
            "and was dropped before being computed"
        );
    }

    #[test]
    fn a_result_that_returns_for_an_old_generation_is_discarded_not_installed() {
        let (mut fx, ids) = Fixture::new(1);
        // Start the worker and establish the generation.
        fx.settle(&ids, Instant::now());
        let live = fx.service.generation;
        let Some(tx) = fx.service.tx.clone() else {
            unreachable!("the worker has started");
        };
        let finished = |generation| Finished {
            layer: ids[0],
            generation,
            stamp: 42,
            image: Some(picture(0.5)),
        };
        assert!(tx.send(finished(live - 1)).is_ok());
        let before = fx.service.stats;
        assert!(fx.service.drain().is_empty());
        assert_eq!(fx.service.stats.results_dropped, before.results_dropped + 1);
        assert_ne!(
            fx.service.cache().stamp(ids[0]),
            Some(42),
            "the stale picture is not shown"
        );
        // The same result for the live generation does install.
        assert!(tx.send(finished(live)).is_ok());
        assert_eq!(fx.service.drain(), vec![ids[0]]);
        assert_eq!(fx.service.cache().stamp(ids[0]), Some(42));
    }

    /// A fixture with one big layer (3000 x 2000: 96 grid cells) holding
    /// only the given tiles.
    fn big_layer(tiles: &[TileId]) -> (Fixture, LayerId) {
        let (mut fx, _) = Fixture::new(0);
        let id = ok(fx.layers.add_pixel_layer("Big", rect(3000, 2000), None));
        for &tile_id in tiles {
            paint_tile(&mut fx, id, tile_id, 0.8);
        }
        (fx, id)
    }

    fn paint_tile(fx: &mut Fixture, layer: LayerId, tile_id: TileId, shade: f32) {
        let Some(surface) = fx.layers.surface_id(layer) else {
            unreachable!("pixel layer");
        };
        let t = ok(fx.store.get_mut(surface, tile_id));
        let v = f16::from_f32(shade);
        for texel in t.texels_mut().chunks_exact_mut(4) {
            texel.copy_from_slice(&[v, v, v, f16::ONE]);
        }
    }

    #[test]
    fn a_stroke_into_a_previously_empty_tile_of_a_big_layer_refreshes_its_picture() {
        let (mut fx, id) = big_layer(&[TileId { x: 0, y: 0 }]);
        let t0 = fx.settle(&[id], Instant::now());
        let first = fx.service.cache.get(id);
        let Some(first) = first else {
            unreachable!("computed");
        };
        let before = fx.service.stats.jobs_submitted;
        // Paint a tile far from the first, in a layer that had one tile.
        paint_tile(&mut fx, id, TileId { x: 9, y: 6 }, 0.3);
        fx.settle(&[id], t0 + Duration::from_secs(1));
        assert_eq!(
            fx.service.stats.jobs_submitted,
            before + 1,
            "the new tile refreshed it"
        );
        let Some(second) = fx.service.cache.get(id) else {
            unreachable!("computed");
        };
        assert_ne!(*first, *second);
        let (w, h) = (second.width(), second.height());
        // The new tile is at columns 9..10 of 12, rows 6..7 of 8.
        let at = |cx: u32, cy: u32| second.texel(cx, cy).map_or(0.0, |t| t[3]);
        assert!(
            at(w * 9 / 12 + 1, h * 6 / 8 + 1) > 0.9,
            "the new tile shows"
        );
        assert!(
            at(w * 5 / 12, h * 3 / 8) < 1e-6,
            "an empty cell stays empty"
        );
    }

    #[test]
    fn a_layer_with_many_tiles_reads_sixteen_existing_ones_and_notices_a_new_tile() {
        let many: Vec<TileId> = (0..12)
            .flat_map(|x| (0..2).map(move |y| TileId { x, y }))
            .collect();
        let (mut fx, id) = big_layer(&many);
        let t0 = fx.settle(&[id], Instant::now());
        assert!(fx.service.stats.tiles_shared <= MAX_TILES);
        let before = fx.service.stats.jobs_submitted;
        paint_tile(&mut fx, id, TileId { x: 3, y: 5 }, 0.5);
        fx.settle(&[id], t0 + Duration::from_secs(1));
        assert_eq!(
            fx.service.stats.jobs_submitted,
            before + 1,
            "a new tile changes the stamp"
        );
    }

    /// A store whose budget is two tiles, with the 4 x 4 tiles of a
    /// 1024 x 1024 layer written and flushed to scratch.
    fn paged_out_layer() -> (Fixture, LayerId) {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(2) else {
            unreachable!("non-zero");
        };
        let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
        let (mut fx, _) = Fixture::new(0);
        fx.scratch = dir;
        fx.store = store;
        fx.doc = (fx.doc.0, fx.store.serial());
        let id = ok(fx.layers.add_pixel_layer("Paged", rect(1024, 1024), None));
        for x in 0..4 {
            for y in 0..4 {
                paint_tile(&mut fx, id, TileId { x, y }, 0.1 + 0.05 * (x + y) as f32);
            }
        }
        ok(fx.store.flush());
        (fx, id)
    }

    #[test]
    fn scratch_reads_on_the_ui_thread_are_capped_per_turn_and_the_rest_deferred() {
        let (mut fx, id) = paged_out_layer();
        let on_disk = (0..4)
            .flat_map(|x| (0..4).map(move |y| TileId { x, y }))
            .filter(|&t| {
                let Some(surface) = fx.layers.surface_id(id) else {
                    return false;
                };
                matches!(
                    fx.store.share_tile_without_io(surface, t),
                    aurora_tile::TileLookup::OnDisk
                )
            })
            .count();
        assert!(
            on_disk > SCRATCH_READS_PER_TURN,
            "{on_disk} tiles on disk: the cap must bite"
        );
        let now = Instant::now();
        let deadline = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[id], now);
        assert_eq!(
            fx.service.stats.scratch_reads, SCRATCH_READS_PER_TURN,
            "capped at 8"
        );
        assert_eq!(
            fx.service.stats.jobs_submitted, 0,
            "not all gathered: no job yet"
        );
        assert_eq!(fx.service.stats.deferred, 1);
        assert_eq!(deadline, Some(now), "asks to be called again at once");
        // The next turn finishes the gather and submits.
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[id], now);
        assert_eq!(
            fx.service.stats.scratch_reads, on_disk,
            "no tile read twice"
        );
        assert_eq!(fx.service.stats.jobs_submitted, 1);
        assert_eq!(fx.service.stats.tiles_shared, 16);
        fx.settle(&[id], now);
        assert!(fx.service.cache().stamp(id).is_some());
    }

    #[test]
    fn the_scratch_budget_is_shared_by_all_layers_of_a_turn() {
        let (mut fx, id) = paged_out_layer();
        let second = ok(fx.layers.add_pixel_layer("Second", rect(1024, 1024), None));
        for x in 0..4 {
            for y in 0..4 {
                paint_tile(&mut fx, second, TileId { x, y }, 0.4);
            }
        }
        ok(fx.store.flush());
        let _ = fx.service.update(
            fx.doc,
            &fx.layers,
            &mut fx.store,
            &[id, second],
            Instant::now(),
        );
        assert!(fx.service.stats.scratch_reads <= SCRATCH_READS_PER_TURN);
    }

    #[test]
    fn a_full_queue_is_checked_before_any_shares_are_taken() {
        let (mut fx, id) = paged_out_layer();
        // Establish the document first (a new document clears the queue).
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[], Instant::now());
        // Fill the queue by hand (no worker thread has started, so nothing
        // drains it) with jobs for other layers.
        let others: Vec<LayerId> = (0..QUEUE_LIMIT)
            .map(|i| ok(fx.layers.add_pixel_layer(format!("Q{i}"), rect(8, 8), None)))
            .collect();
        if let Ok(mut queue) = fx.service.shared.queue.lock() {
            for other in &others {
                assert!(enqueue(&mut queue, job(*other, 0)));
            }
        }
        assert!(!fx.service.has_room(id));
        assert!(
            fx.service.has_room(others[0]),
            "a replacement is always welcome"
        );
        let _ = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &[id], Instant::now());
        assert_eq!(
            fx.service.stats.scratch_reads, 0,
            "no scratch read for a refused job"
        );
        assert_eq!(fx.service.stats.deferred, 0);
        assert_eq!(fx.service.stats.jobs_submitted, 0);
    }

    #[test]
    fn the_stamp_notices_a_change_in_how_many_tiles_exist() {
        let (fx, id) = big_layer(&[TileId { x: 0, y: 0 }]);
        let Some(surface) = fx.layers.surface_id(id) else {
            unreachable!("pixel layer");
        };
        let one = plan(3000, 2000, &mut |t| t == TileId { x: 0, y: 0 });
        let mut more = one.clone();
        more.existing_count += 1;
        assert_ne!(
            stamp_of(&fx.store, surface, &one, 3000, 2000),
            stamp_of(&fx.store, surface, &more, 3000, 2000)
        );
    }

    #[test]
    fn only_the_visible_layers_are_computed_in_a_long_document() {
        let (mut fx, ids) = Fixture::new(100);
        let visible: Vec<LayerId> = ids[40..50].to_vec();
        fx.settle(&visible, Instant::now());
        assert_eq!(fx.service.stats.jobs_submitted, 10);
        assert_eq!(fx.service.cache().len(), 10);
        assert!(fx.service.stats.tiles_shared <= 10 * MAX_TILES);
        for id in &visible {
            assert!(fx.service.cache().stamp(*id).is_some());
        }
        assert!(fx.service.cache().stamp(ids[0]).is_none());
    }

    #[test]
    fn one_update_submits_at_most_four_jobs_and_asks_to_be_called_again() {
        let (mut fx, ids) = Fixture::new(10);
        let deadline = fx
            .service
            .update(fx.doc, &fx.layers, &mut fx.store, &ids, Instant::now());
        assert_eq!(fx.service.stats.jobs_submitted, MAX_JOBS_PER_UPDATE);
        assert!(deadline.is_some(), "the rest wait for the next turn");
    }

    #[test]
    fn shutdown_joins_the_worker_and_is_idempotent() {
        let (mut fx, ids) = Fixture::new(1);
        fx.settle(&ids, Instant::now());
        assert!(fx.service.handle.is_some());
        fx.service.shutdown();
        assert!(fx.service.handle.is_none());
        fx.service.shutdown();
    }

    /// 0.199.0: under a filter only the shown rows exist, so only they are
    /// ever visible to the service and get a request.
    #[test]
    fn a_filtered_panel_requests_thumbnails_for_its_shown_rows_only() {
        let scales = ok(crate::load_scales());
        let (mut fx, ids) = Fixture::new(100);
        let mut ws = aurora_ui::build_workspace(&scales);
        ws.layer_view.filter = aurora_ui::LayerFilter {
            text: "l4".to_owned(),
            ..aurora_ui::LayerFilter::default()
        };
        let rows = ok(aurora_ui::populate_layers_panel_view(
            &mut ws.tree,
            ws.layers,
            &scales,
            &fx.layers,
            &mut ws.layer_view,
        ));
        assert_eq!(rows.len(), 11, "L4 and L40..L49");
        ws.tree.compute_layout(1274.0, 900.0);
        let visible: Vec<LayerId> =
            aurora_ui::visible_layer_rows(&ws.tree, ws.layers, &rows, &scales, 2)
                .into_iter()
                .map(|(_, layer)| layer)
                .collect();
        assert!(
            !visible.is_empty() && visible.len() <= 11,
            "{}",
            visible.len()
        );
        for layer in &visible {
            let name = fx.layers.name(*layer).unwrap_or_default();
            assert!(name.starts_with("L4"), "{name}");
        }
        fx.settle(&visible, Instant::now());
        assert_eq!(fx.service.stats.jobs_submitted, visible.len());
        for hidden in &ids[5..40] {
            assert!(fx.service.cache().stamp(*hidden).is_none());
        }
    }

    #[test]
    fn rows_show_visible_pictures_and_blank_when_they_scroll_away() {
        let scales = ok(crate::load_scales());
        let (mut fx, ids) = Fixture::new(3);
        let mut ws = aurora_ui::build_workspace(&scales);
        let rows = ok(aurora_ui::populate_layers_panel(
            &mut ws.tree,
            ws.layers,
            &scales,
            &fx.layers,
        ));
        let row_of = |layer: LayerId| {
            rows.iter()
                .find(|&(_, &id)| id == layer)
                .map(|(&row, _)| row)
        };
        let (Some(a), Some(b)) = (row_of(ids[0]), row_of(ids[1])) else {
            unreachable!("a row per layer");
        };
        fx.settle(&ids, Instant::now());
        assert!(fx.service.apply(&mut ws.tree, &[(a, ids[0]), (b, ids[1])]));
        let shown = |tree: &WidgetTree<WidgetKind>, row| {
            aurora_ui::layer_row_thumbnail(tree, row)
                .and_then(|slot| aurora_widgets::widgets::thumbnail_state(tree, slot).ok())
                .is_some_and(|state| state.image.is_some())
        };
        assert!(shown(&ws.tree, a) && shown(&ws.tree, b));
        // Row `a` leaves the visible set: blanked. A second apply changes nothing.
        assert!(fx.service.apply(&mut ws.tree, &[(b, ids[1])]));
        assert!(!shown(&ws.tree, a) && shown(&ws.tree, b));
        assert!(!fx.service.apply(&mut ws.tree, &[(b, ids[1])]));
    }
}
