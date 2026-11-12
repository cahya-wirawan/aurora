//! Editable layer filters at draw time (task 176).
//!
//! A pixel layer's filters ([`aurora_core::LayerFilters`]) are computed when
//! the layer is drawn, never baked (invariant §7.3.2). The one hook is
//! [`crate::pixel_layer_texels`]: for a layer with a visible filter, the
//! texels it hands to the fold (and to the effect, cover and clip paths,
//! which all read through it) are the filtered ones, ahead of the layer's
//! mask, blend mode, opacity, Fill and layer effects. So a drop shadow or
//! stroke sees the blurred pixels, and an export, a histogram and the CPU
//! canvas all draw the same thing.
//!
//! **Per tile.** The filtered tile at composite tile `T` is computed from a
//! `(256 + 2R)`-pixel window of the layer's *stored* pixels around `T` —
//! `R` the visible filters' total reach (at most one tile, so at most the
//! 3 x 3 neighbouring tiles, [`aurora_core::MAX_FILTER_REACH`]) — read cell
//! by cell through [`crate::raw_layer_texels`] (cells with nothing stored
//! cost no page-in), premultiplied, run through each filter in order and
//! cropped. The arithmetic is exact integer ([`aurora_filters::blur`]), so
//! neighbouring tiles agree bit for bit where they meet. A filter reaches
//! tiles the layer never painted: the blur's spill is drawn there.
//!
//! **Cache.** Filtered tiles are cached process-wide, keyed on everything
//! they depend on: the store, the layer's surface and origin, the tile,
//! the visible filters' settings and the `content_version` of every layer
//! tile under the window. A dab, a pixel undo, a Move or a filter edit
//! changes the key, so nothing stale can be served and no eviction call can
//! be forgotten; opacity, blend mode, mask, visibility and other layers are
//! not in it, so those edits hit. The cache holds at most
//! [`crate::small_machine::shadow_cache_bytes`] worth of tiles (half-float
//! RGBA, 512 KiB each), least recently used out first.
//!
//! **Limits to know.** The filter is exact for any chain within its reach
//! cap, but the layer effects drawn from the filtered pixels are bounded
//! separately: a shadow, stroke or glow reads its source through a window
//! of at most `MAX_SHADOW_APRON` pixels (25 composite cells), and that
//! window now has to cover the effect's own apron *plus* the filter's
//! reach. Where the sum goes past the bound the effect's source window is
//! clipped, so a far edge of a very large shadow or glow of a heavily
//! blurred layer can be cut short. Nothing is reported for it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use crate::{CompositeBudget, TileSource};

const T: usize = aurora_tile::TILE as usize;
const CHANNELS: usize = aurora_tile::CHANNELS;

/// One cached filtered tile's size in bytes (half-float RGBA).
const ENTRY_BYTES: usize = T * T * CHANNELS * std::mem::size_of::<half::f16>();

/// Whether `id` is a pixel layer with at least one visible filter.
#[must_use]
pub(crate) fn has_visible_filters(layers: &aurora_doc::LayerTree, id: aurora_doc::LayerId) -> bool {
    matches!(layers.kind(id), Some(aurora_doc::LayerKind::Pixel { .. }))
        && layers
            .filters(id)
            .is_some_and(aurora_core::LayerFilters::any_visible)
}

/// How many pixels the layer's visible filters reach (`0` without any):
/// what an edit of the layer's pixels changes around itself.
#[must_use]
pub(crate) fn filter_reach(layers: &aurora_doc::LayerTree, id: aurora_doc::LayerId) -> u32 {
    if has_visible_filters(layers, id) {
        layers
            .filters(id)
            .map_or(0, aurora_core::LayerFilters::visible_reach)
    } else {
        0
    }
}

/// The visible filters' settings as a cache key part: kind tag and the
/// setting's bits. Empty without visible filters.
#[must_use]
pub(crate) fn filter_signature(
    layers: &aurora_doc::LayerTree,
    id: aurora_doc::LayerId,
) -> Vec<(u8, u32)> {
    if !has_visible_filters(layers, id) {
        return Vec::new();
    }
    layers
        .filters(id)
        .map(|f| {
            f.visible()
                .map(|filter| match filter.kind {
                    aurora_core::FilterKind::GaussianBlur { radius } => (0, radius.to_bits()),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FilterCacheKey {
    store: u64,
    surface: aurora_tile::SurfaceId,
    layer_origin: (i64, i64),
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    filters: Vec<(u8, u32)>,
    versions: Vec<(bool, u32, u32, bool, u64)>,
}

/// Least-recently-used cache of filtered tiles. An empty vector is a tile
/// known to filter to nothing.
struct FilterCache {
    cap_entries: usize,
    entries: HashMap<FilterCacheKey, (u64, Arc<Vec<half::f16>>)>,
    order: BTreeMap<u64, FilterCacheKey>,
    clock: u64,
}

impl FilterCache {
    fn with_cap_bytes(bytes: usize) -> Self {
        Self {
            cap_entries: (bytes / ENTRY_BYTES).max(1),
            entries: HashMap::new(),
            order: BTreeMap::new(),
            clock: 0,
        }
    }

    fn get(&mut self, key: &FilterCacheKey) -> Option<Arc<Vec<half::f16>>> {
        self.clock += 1;
        let stamp = self.clock;
        let (old, tile) = self.entries.get_mut(key)?;
        let previous = std::mem::replace(old, stamp);
        let tile = Arc::clone(tile);
        self.order.remove(&previous);
        self.order.insert(stamp, key.clone());
        Some(tile)
    }

    fn insert(&mut self, key: FilterCacheKey, tile: Arc<Vec<half::f16>>) {
        if let Some((old, _)) = self.entries.remove(&key) {
            self.order.remove(&old);
        }
        while self.entries.len() >= self.cap_entries {
            let Some((&oldest, _)) = self.order.iter().next() else {
                break;
            };
            if let Some(victim) = self.order.remove(&oldest) {
                self.entries.remove(&victim);
            }
        }
        self.clock += 1;
        self.order.insert(self.clock, key.clone());
        self.entries.insert(key, (self.clock, tile));
    }
}

static FILTER_CACHE: LazyLock<Mutex<FilterCache>> = LazyLock::new(|| {
    Mutex::new(FilterCache::with_cap_bytes(
        crate::small_machine::shadow_cache_bytes(),
    ))
});

fn with_cache<R>(f: impl FnOnce(&mut FilterCache) -> R) -> R {
    let mut cache = match FILTER_CACHE.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut cache)
}

#[cfg(test)]
thread_local! {
    /// Test-only: filtered tiles computed (cache misses) on this thread.
    pub(crate) static FILTER_COMPUTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The layer's stored pixels on composite tile `tile_id`, run through its
/// visible filters: straight RGBA like [`crate::raw_layer_texels`], with no
/// mask. `None` when nothing is stored in the tile's neighbourhood or
/// everything filters to transparent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn filtered_layer_texels(
    id: aurora_doc::LayerId,
    layers: &aurora_doc::LayerTree,
    store: &mut dyn TileSource,
    tile_id: aurora_tile::TileId,
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    budget: &mut CompositeBudget,
) -> Option<Vec<half::f16>> {
    let surface = layers.surface_id(id)?;
    let reach = filter_reach(layers, id);
    let layer_origin = layers.bounds(id).map_or((0, 0), |b| (b.x, b.y));
    let tile = i64::from(aurora_tile::TILE);
    let reach_i = i64::from(reach);
    let side = tile + 2 * reach_i;
    let mut versions = Vec::new();
    crate::surface_window_versions(
        store,
        surface,
        layer_origin,
        (doc_origin.0 - reach_i, doc_origin.1 - reach_i),
        side,
        false,
        &mut versions,
    );
    let key = FilterCacheKey {
        store: store.serial(),
        surface,
        layer_origin,
        doc_origin,
        reference_origin,
        filters: filter_signature(layers, id),
        versions,
    };
    if let Some(hit) = with_cache(|cache| cache.get(&key)) {
        return (!hit.is_empty()).then(|| hit.to_vec());
    }
    #[cfg(test)]
    FILTER_COMPUTES.with(|n| n.set(n.get() + 1));
    let errors_before = budget.store_errors;
    let computed = compute(
        id,
        layers,
        store,
        tile_id,
        doc_origin,
        reference_origin,
        budget,
    );
    // A neighbour tile that failed to page in was skipped, so this result
    // is degraded and its versions have not changed: do not keep it.
    if budget.store_errors == errors_before {
        let stored = Arc::new(computed.clone().unwrap_or_default());
        with_cache(|cache| cache.insert(key, stored));
    }
    computed
}

/// [`filtered_layer_texels`], uncached.
fn compute(
    id: aurora_doc::LayerId,
    layers: &aurora_doc::LayerTree,
    store: &mut dyn TileSource,
    tile_id: aurora_tile::TileId,
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    budget: &mut CompositeBudget,
) -> Option<Vec<half::f16>> {
    let filters = layers.filters(id)?;
    let reach = usize::try_from(filters.visible_reach()).ok()?;
    let side = T + 2 * reach;
    let tile = i64::from(aurora_tile::TILE);
    let reach_i = i64::try_from(reach).ok()?;
    let window_origin = (doc_origin.0 - reach_i, doc_origin.1 - reach_i);
    let same_origin = layers.bounds(id).map_or((0, 0), |b| (b.x, b.y)) == reference_origin;
    let cells = i64::try_from(reach.div_ceil(T)).ok()?;
    let mut window = vec![0.0_f32; side * side * CHANNELS];
    let mut any = false;
    for dy in -cells..=cells {
        for dx in -cells..=cells {
            let cell_origin = (doc_origin.0 + dx * tile, doc_origin.1 + dy * tile);
            let cell_id = match (
                u32::try_from(i64::from(tile_id.x) + dx),
                u32::try_from(i64::from(tile_id.y) + dy),
            ) {
                (Ok(x), Ok(y)) => aurora_tile::TileId { x, y },
                // Nothing is stored left of or above a same-origin layer;
                // a windowed layer ignores the id.
                _ if same_origin => continue,
                _ => aurora_tile::TileId { x: 0, y: 0 },
            };
            if let Some(cell) = crate::raw_layer_texels(
                id,
                layers,
                store,
                cell_id,
                cell_origin,
                reference_origin,
                budget,
            ) {
                any = true;
                blit_premultiplied(&mut window, side, window_origin, &cell, cell_origin);
            }
        }
    }
    if !any {
        return None;
    }
    for filter in filters.visible() {
        match filter.kind {
            aurora_core::FilterKind::GaussianBlur { radius } => {
                let spec = aurora_core::GaussianSpec::new(radius)?;
                aurora_filters::gaussian_blur_window(&mut window, side, &spec);
            }
        }
    }
    let centre = aurora_filters::crop_window(&window, side, T);
    to_straight_half(&centre)
}

/// Copies the part of `cell` (a straight RGBA tile whose top-left sits at
/// document `cell_origin`) that lies inside the window into `window`
/// (premultiplied `f32`, `side` x `side`, top-left at `window_origin`).
fn blit_premultiplied(
    window: &mut [f32],
    side: usize,
    window_origin: (i64, i64),
    cell: &[half::f16],
    cell_origin: (i64, i64),
) {
    let t = i64::from(aurora_tile::TILE);
    let side_i = i64::try_from(side).unwrap_or(0);
    let x_lo = cell_origin.0.max(window_origin.0);
    let x_hi = (cell_origin.0 + t).min(window_origin.0 + side_i);
    let y_lo = cell_origin.1.max(window_origin.1);
    let y_hi = (cell_origin.1 + t).min(window_origin.1 + side_i);
    for y in y_lo..y_hi {
        for x in x_lo..x_hi {
            let src = usize::try_from((y - cell_origin.1) * t + (x - cell_origin.0))
                .map_or(0, |i| i * CHANNELS);
            let dst = usize::try_from((y - window_origin.1) * side_i + (x - window_origin.0))
                .map_or(0, |i| i * CHANNELS);
            let (Some(texel), Some(out)) = (
                cell.get(src..src + CHANNELS),
                window.get_mut(dst..dst + CHANNELS),
            ) else {
                continue;
            };
            let a = texel.get(3).map_or(0.0, |v| v.to_f32());
            for (c, slot) in out.iter_mut().enumerate().take(3) {
                *slot = texel.get(c).map_or(0.0, |v| v.to_f32()) * a;
            }
            if let Some(slot) = out.get_mut(3) {
                *slot = a;
            }
        }
    }
}

/// Premultiplied `f32` RGBA back to straight half-float tile texels
/// (`rgb / a`; no alpha is no colour). `None` when every texel is
/// transparent.
pub(crate) fn to_straight_half(premultiplied: &[f32]) -> Option<Vec<half::f16>> {
    let mut out = Vec::with_capacity(premultiplied.len());
    let mut any = false;
    for texel in premultiplied.chunks_exact(CHANNELS) {
        let [r, g, b, a] = texel else {
            continue;
        };
        if a.is_finite() && *a > 0.0 {
            any = true;
            out.push(half::f16::from_f32(r / a));
            out.push(half::f16::from_f32(g / a));
            out.push(half::f16::from_f32(b / a));
            out.push(half::f16::from_f32(*a));
        } else {
            out.extend([half::f16::ZERO; CHANNELS]);
        }
    }
    (any && out.len() == T * T * CHANNELS).then_some(out)
}
