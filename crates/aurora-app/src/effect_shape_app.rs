//! The compositor's use of a layer's effect shape (task 151,
//! [`aurora_doc::EffectShape`]).
//!
//! A shape layer's strokes, glows and shadows are drawn from the outline's
//! coverage instead of the alpha of the stored pixels, but only while the
//! layer's pixels are still the ones the outline was recorded beside:
//! [`valid_effect_shape`] compares the layer's pixel fingerprint with the
//! shape's. The comparison pages in every tile of the layer, so its result
//! is remembered per thread, keyed by the store, the surface, the shape and
//! a hash of the layer's tile content versions (which change on every write
//! to a tile and never page anything in); a later call with the same
//! versions answers from the memo. An edit changes a version and the layer
//! is compared again, so painting on the layer ends the shape's use and
//! undoing the edit brings it back.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use aurora_doc::{EffectShape, LayerId, LayerTree};
use aurora_tile::{SurfaceId, TILE, TileId};

use crate::TileSource;

/// The memo's size bound: when it is full it is cleared (a recompute is
/// always correct).
const MEMO_LIMIT: usize = 512;

type MemoKey = (u64, SurfaceId, u64, u64);

thread_local! {
    static MEMO: RefCell<HashMap<MemoKey, bool>> = RefCell::new(HashMap::new());
}

#[cfg(test)]
thread_local! {
    /// Test-only: how many full fingerprint comparisons ran.
    pub(crate) static FINGERPRINT_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The layer's tile grid, from its bounds.
fn grid(layers: &LayerTree, id: LayerId) -> Option<(u32, u32)> {
    let bounds = layers.bounds(id)?;
    Some((bounds.width.div_ceil(TILE), bounds.height.div_ceil(TILE)))
}

/// A hash of which tiles of the layer exist and their content versions.
fn versions_hash(
    store: &dyn TileSource,
    surface: SurfaceId,
    (tiles_x, tiles_y): (u32, u32),
) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let tile = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, tile) {
                continue;
            }
            let v = store.content_version(surface, tile);
            h = (h ^ (u64::from(tx) << 32 | u64::from(ty))).wrapping_mul(0x0100_0000_01B3);
            h = (h ^ v).wrapping_mul(0x0100_0000_01B3);
        }
    }
    h
}

/// The layer's pixel fingerprint, as
/// [`aurora_doc::smart_object::surface_fingerprint`] computes it, over any
/// [`TileSource`]. `None` when a tile cannot be read.
fn fingerprint(
    store: &mut dyn TileSource,
    surface: SurfaceId,
    (tiles_x, tiles_y): (u32, u32),
) -> Option<u64> {
    #[cfg(test)]
    FINGERPRINT_RUNS.with(|n| n.set(n.get() + 1));
    let mut sum = 0u64;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let tile = store.texels(surface, id).ok()?;
            if let Some(h) = aurora_doc::smart_object::texel_fingerprint(&tile) {
                sum = sum.wrapping_add(aurora_doc::smart_object::tile_fingerprint_term(id, h));
            }
        }
    }
    Some(sum)
}

/// The layer's effect shape if it has one, the layer has no enabled mask
/// (the shape is an outline, not a masked one) and its pixels still match
/// the fingerprint the shape was recorded with; `None` otherwise, when the
/// effects use the pixels' alpha as before.
pub(crate) fn valid_effect_shape(
    id: LayerId,
    layers: &LayerTree,
    store: &mut dyn TileSource,
) -> Option<Arc<EffectShape>> {
    let shape = layers.effect_shape(id)?;
    if layers.mask(id).is_some_and(|m| m.enabled) {
        return None;
    }
    let surface = layers.surface_id(id)?;
    let grid = grid(layers, id)?;
    let versions = versions_hash(store, surface, grid);
    let key = (store.serial(), surface, versions, shape.content_hash());
    if let Some(known) = MEMO.with(|memo| memo.borrow().get(&key).copied()) {
        return known.then(|| Arc::clone(shape));
    }
    let valid = fingerprint(store, surface, grid)? == shape.pixel_fingerprint();
    MEMO.with(|memo| {
        let mut memo = memo.borrow_mut();
        if memo.len() >= MEMO_LIMIT {
            memo.clear();
        }
        memo.insert(key, valid);
    });
    valid.then(|| Arc::clone(shape))
}

/// The shape's coverage `0..=1` over the `side` x `side` window whose
/// top-left is document point `window`, for a layer whose bounds origin is
/// `layer_origin`; `None` when the window holds none.
#[allow(clippy::cast_possible_wrap)] // a window row or column is at most a few thousand
pub(crate) fn shape_window(
    shape: &EffectShape,
    layer_origin: (i64, i64),
    window: (i64, i64),
    side: usize,
) -> Option<Vec<f32>> {
    let mut out = vec![0.0f32; side * side];
    let mut any = false;
    for row in 0..side {
        let y = window.1 + row as i64 - layer_origin.1;
        for col in 0..side {
            let x = window.0 + col as i64 - layer_origin.0;
            let c = shape.at(x, y);
            if c != 0
                && let Some(d) = out.get_mut(row * side + col)
            {
                *d = f32::from(c) / 255.0;
                any = true;
            }
        }
    }
    any.then_some(out)
}
