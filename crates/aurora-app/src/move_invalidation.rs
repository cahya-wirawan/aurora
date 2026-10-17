//! Narrow composite invalidation for a live Move (60 FPS round 2).
//!
//! Before this, every pointer event of a Move drag bumped the whole
//! [`CompositeCache`], so each event recomposited every visible tile even
//! when the moved layer covered a corner of the view. Now a move
//! invalidates only the composite tiles the move can have changed, and
//! still bumps the whole cache when the composite grid's anchor moves.
//!
//! # Why this is sound
//!
//! CLAUDE.md records why structural undo narrowing was reverted in 0.73.1:
//! a layer's declared `bounds` is a position hint, not an enforced extent
//! of its content, so a rectangle built from `bounds` can miss real
//! pixels. This module never reads `bounds` for the extent. It asks the
//! tile store which tiles the layer's surface really holds
//! ([`aurora_tile::TileStore::contains_tile`]): a layer's content is
//! exactly its stored tiles, placed at the layer's origin (a tile id is
//! unsigned, so nothing lies left of or above the origin), and an absent
//! tile reads as transparent. A blank tile a read materialized is held and
//! counted, which only over-invalidates.
//!
//! A move changes the layer's origin and nothing else: no tile, no mask
//! (a mask keeps its own document-space `bounds`), no effect, mode,
//! opacity, clipping flag or tree position. So the document after the move
//! differs from the one before only in the moved layer's value at the
//! document pixels its stored content covers at the old origin or at the
//! new one; everywhere else the layer is transparent before and after.
//!
//! Every composite pixel is a function of the layers' values at that same
//! pixel (blend modes, opacity, Fill, masks, isolated and Pass Through
//! groups, clip groups, Dissolve, and every adjustment layer, which maps
//! the composite below it pixel by pixel), with one exception: the layer
//! effects that reach beyond their source pixel. Those are a pixel layer's
//! drop shadows (shifted by their offset, grown by their apron), its
//! strokes and Softer outer glows (grown by their apron), computed from
//! **that layer's own shape** (its alpha times its mask). Interior effects
//! (colour, gradient and pattern overlays, inside strokes) stay inside the
//! shape. Group effects are not drawn. A clipped layer's effects are
//! computed from the clipped layer's own shape; the clip base enters only
//! through the clip group's alpha, pixel by pixel ([`fold_clip_group`]).
//! So the only effects whose source the move changes are the moved
//! layer's own, and [`effect_reaches`] lists them exactly as a dab's
//! invalidation does ([`effect_reach_rects`]).
//!
//! Hence a composite tile `C` can change only if some stored tile of the
//! moved layer, placed at the old or the new origin, meets `C` itself or
//! the window `C` shifted back by an effect's offset and grown by its
//! apron. Every cached tile (current, or in flight on the background
//! compositor) that passes that test is invalidated; the rest keep their
//! pixels, which the move cannot have changed.
//!
//! Two more things a move can change are handled by bumping everything:
//! the grid anchor ([`composite_reference_origin`] is `min(origin, 0)` per
//! axis of the active layer, so a tile id names other pixels once it
//! moves), and a surface the store cannot be asked about.
//!
//! Culling ([`fold_plan_for_tile`]) uses `bounds` to choose candidates but
//! is bit-identical to the plain fold, so a composite tile is still a
//! function of the document's values alone.
//!
//! Cost: one walk of the cached tiles, each asking at most
//! `(1 + reaches) * 2` windows of a few `contains_tile` lookups (a window
//! wider than [`MAX_WINDOW_TILES`] tiles counts as a hit without looking).
//!
//! [`fold_clip_group`]: crate::fold_clip_group
//! [`effect_reach_rects`]: crate::effect_reach_rects
//! [`composite_reference_origin`]: crate::composite_reference_origin
//! [`fold_plan_for_tile`]: crate::fold_plan_for_tile

use crate::CompositeCache;

/// A source window with more layer tiles than this is treated as a hit
/// without looking (conservative; an apron of [`MAX_SHADOW_APRON`] needs at
/// most 5 x 5).
///
/// [`MAX_SHADOW_APRON`]: aurora_core::effects::MAX_SHADOW_APRON
const MAX_WINDOW_TILES: i64 = 64;

/// `(offset, apron)` of every effect of `layer` that is drawn and reaches
/// beyond its source pixel: a drop shadow's own, a stroke's apron with no
/// offset (0.190.0), a Softer outer glow's apron (0.191.0). Empty for a
/// layer that draws no effect ([`crate::layer_renders_effects`]).
pub(crate) fn effect_reaches(
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
) -> Vec<((i64, i64), u32)> {
    let Some(fx) = layers.effects(layer) else {
        return Vec::new();
    };
    if !crate::layer_renders_effects(layers, layer) {
        return Vec::new();
    }
    let mut reaches: Vec<((i64, i64), u32)> = fx
        .drop_shadows
        .iter()
        .filter(|s| fx.shows(s.gate))
        .map(|shadow| {
            let g = shadow.geometry();
            (g.offset, g.apron)
        })
        .collect();
    reaches.extend(
        fx.strokes
            .iter()
            .filter(|s| fx.shows(s.gate) && crate::stroke_paintable(s, layers))
            .map(|stroke| ((0, 0), stroke.geometry().apron)),
    );
    reaches.extend(
        fx.outer_glows
            .iter()
            .filter(|g| fx.shows(g.gate) && !g.precise)
            .map(|glow| ((0, 0), glow.geometry().apron)),
    );
    reaches
}

/// What [`invalidate_moved_layer`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveInvalidation {
    /// Only these many cached tiles were invalidated.
    Narrow(usize),
    /// The whole cache was bumped.
    Everything,
}

/// Invalidates what moving `layer` from `old_origin` to `new_origin` can
/// have changed (see the module documentation for why that is sound).
/// `layers` is the tree after the move; `reference_before` and
/// `reference_after` are the composite grid anchors before and after it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn invalidate_moved_layer(
    cache: &mut CompositeCache,
    layers: &aurora_doc::LayerTree,
    store: Option<&aurora_tile::TileStore>,
    layer: aurora_doc::LayerId,
    old_origin: (i64, i64),
    new_origin: (i64, i64),
    reference_before: (i64, i64),
    reference_after: (i64, i64),
) -> MoveInvalidation {
    if reference_before != reference_after {
        cache.bump();
        return MoveInvalidation::Everything;
    }
    let (Some(store), Some(surface)) = (store, layers.surface_id(layer)) else {
        cache.bump();
        return MoveInvalidation::Everything;
    };
    if old_origin == new_origin {
        return MoveInvalidation::Narrow(0);
    }
    let windows: Vec<((i64, i64), i64)> = std::iter::once(((0, 0), 0))
        .chain(
            effect_reaches(layers, layer)
                .into_iter()
                .map(|(offset, apron)| (offset, i64::from(apron))),
        )
        .collect();
    let t = i64::from(aurora_tile::TILE);
    let hit: Vec<aurora_tile::TileId> = cache
        .current
        .iter()
        .chain(cache.in_flight.iter())
        .copied()
        .filter(|&id| {
            let x0 = reference_after
                .0
                .saturating_add(i64::from(id.x).saturating_mul(t));
            let y0 = reference_after
                .1
                .saturating_add(i64::from(id.y).saturating_mul(t));
            windows.iter().any(|&(offset, apron)| {
                [old_origin, new_origin].iter().any(|&origin| {
                    // Source pixels `p` whose effect lands in the tile:
                    // `p + offset +- apron` meets `[x0, x0 + t)`, so
                    // `p` is in `[x0 - offset - apron, x0 + t - offset + apron)`,
                    // in layer-local pixels by subtracting the origin.
                    let sx0 = x0
                        .saturating_sub(offset.0)
                        .saturating_sub(apron)
                        .saturating_sub(origin.0);
                    let sx1 = x0
                        .saturating_add(t)
                        .saturating_sub(offset.0)
                        .saturating_add(apron)
                        .saturating_sub(origin.0);
                    let sy0 = y0
                        .saturating_sub(offset.1)
                        .saturating_sub(apron)
                        .saturating_sub(origin.1);
                    let sy1 = y0
                        .saturating_add(t)
                        .saturating_sub(offset.1)
                        .saturating_add(apron)
                        .saturating_sub(origin.1);
                    layer_holds_tile_in(store, surface, (sx0, sx1), (sy0, sy1))
                })
            })
        })
        .collect();
    cache.stamp = cache.stamp.wrapping_add(1);
    for &id in &hit {
        cache.note_invalidated(id);
    }
    MoveInvalidation::Narrow(hit.len())
}

/// Whether `surface` holds a tile meeting the layer-local half-open pixel
/// window `[x.0, x.1) x [y.0, y.1)`. A window wider than
/// [`MAX_WINDOW_TILES`] tiles answers `true` without looking.
fn layer_holds_tile_in(
    store: &aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    x: (i64, i64),
    y: (i64, i64),
) -> bool {
    let t = i64::from(aurora_tile::TILE);
    let max = i64::from(u32::MAX);
    // Nothing is stored left of or above the layer's origin.
    if x.1 <= 0 || y.1 <= 0 || x.1 <= x.0 || y.1 <= y.0 {
        return false;
    }
    let (lx, hx) = (x.0.max(0).div_euclid(t), (x.1 - 1).div_euclid(t).min(max));
    let (ly, hy) = (y.0.max(0).div_euclid(t), (y.1 - 1).div_euclid(t).min(max));
    if lx > hx || ly > hy {
        return false;
    }
    if (hx - lx + 1).saturating_mul(hy - ly + 1) > MAX_WINDOW_TILES {
        return true;
    }
    (ly..=hy).any(|ty| {
        (lx..=hx).any(|tx| {
            match (u32::try_from(tx), u32::try_from(ty)) {
                (Ok(x), Ok(y)) => store.contains_tile(surface, aurora_tile::TileId { x, y }),
                // Out of the id range: no tile can be there.
                _ => false,
            }
        })
    })
}

/// The live document half of a Move drag's pointer event: sets `layer`'s
/// `bounds` and invalidates what that changed ([`invalidate_moved_layer`]).
/// `active` is the document's active layer (the grid anchor's layer).
/// On an error the tree is unchanged and nothing is invalidated.
pub(crate) fn move_layer_live(
    layers: &mut aurora_doc::LayerTree,
    cache: &mut CompositeCache,
    store: Option<&aurora_tile::TileStore>,
    active: Option<aurora_doc::LayerId>,
    layer: aurora_doc::LayerId,
    bounds: aurora_core::Rect,
) -> Result<MoveInvalidation, aurora_doc::DocError> {
    let before = layers.bounds(layer);
    let reference_before = crate::composite_reference_origin(layers, active);
    layers.set_bounds(layer, bounds)?;
    let reference_after = crate::composite_reference_origin(layers, active);
    let Some(old) = before else {
        cache.bump();
        return Ok(MoveInvalidation::Everything);
    };
    Ok(invalidate_moved_layer(
        cache,
        layers,
        store,
        layer,
        (old.x, old.y),
        (bounds.x, bounds.y),
        reference_before,
        reference_after,
    ))
}
