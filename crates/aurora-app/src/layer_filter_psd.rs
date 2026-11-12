//! Saving a document with editable layer filters to a PSD (task 176).
//!
//! Aurora does not write Photoshop Smart Filters, so a filtered layer is
//! saved as its **filtered pixels**: the writer is handed a copy of the
//! layer tree without the filters and a scratch tile store that shares
//! every tile of the document and holds the filtered tiles of each
//! filtered layer in place of the stored ones. The document itself is not
//! touched. The merged image is composited from the real document (filters
//! on), so it equals what the screen shows.
//!
//! The filtered tiles are computed on the layer's own tile grid
//! ([`crate::layer_filter_app::filtered_layer_texels`]), for the tiles the
//! layer holds and one tile beyond on every side (the blur's spill). The
//! baked layer's rectangle grows by one tile on every side so the spill is
//! saved, the grid staying aligned (every baked tile moves one tile right
//! and down). If the grown origin would leave the document's range, the
//! layer keeps its rectangle and the report says, for that layer only,
//! that its blur spill is not fully saved. The mask is not applied here:
//! the writer saves it as the layer's mask, as for a layer without filters.
//!
//! The save report lists the layers under Not saved: they are saved
//! filtered but are no longer editable filters in Photoshop.

use std::collections::HashSet;
use std::sync::Arc;

use aurora_io::{IoError, ReportCategory, ReportEntry};
use aurora_tile::{TileId, TileShare, TileStore};

use crate::{CompositeBudget, layer_filter_app};

/// A document copy ready to write: no filters, filtered pixels.
pub(crate) struct BakedFilters {
    pub(crate) layers: aurora_doc::LayerTree,
    pub(crate) store: TileStore,
    /// The names of the filtered layers, for the report.
    pub(crate) names: Vec<String>,
    /// The layers whose blur spills past what could be saved for them.
    pub(crate) clipped: Vec<String>,
}

impl BakedFilters {
    /// The Not saved entries for the report: the layers saved filtered,
    /// and (only when some spill was actually cut) the layers whose blur
    /// spill is not fully saved.
    pub(crate) fn report_entries(&self) -> Vec<ReportEntry> {
        let mut entries = vec![
            ReportEntry::new(
                ReportCategory::NotSaved,
                "Layer filters are not saved as editable filters; the filtered pixels were saved \
                 instead.",
            )
            .with_affected(&self.names),
        ];
        if !self.clipped.is_empty() {
            entries.push(
                ReportEntry::new(
                    ReportCategory::NotSaved,
                    "The blur of these layers spreads past the area Photoshop can hold for them \
                     here; the part beyond it is not saved.",
                )
                .with_affected(&self.clipped),
            );
        }
        entries
    }
}

/// Builds the copy for a document whose layers have visible filters;
/// `Ok(None)` when none does (the writer then reads the document itself).
///
/// # Errors
///
/// An [`IoError`] when the scratch store cannot be made or a tile cannot
/// be read.
#[allow(clippy::too_many_lines)]
pub(crate) fn bake_filters(
    layers: &aurora_doc::LayerTree,
    store: &mut TileStore,
) -> Result<Option<BakedFilters>, IoError> {
    let filtered: Vec<aurora_doc::LayerId> = crate::layer_ids_in_order(layers)
        .into_iter()
        .filter(|&id| layer_filter_app::has_visible_filters(layers, id))
        .collect();
    if filtered.is_empty() {
        return Ok(None);
    }
    let other = |what: &str| IoError::Io(std::io::Error::other(what.to_owned()));
    let mut baked = crate::background_composite::new_mirror(store)
        .ok_or_else(|| other("couldn't create a scratch store for the filtered layers"))?;
    let held = store.held_tile_versions();
    for ((surface, id), _version) in &held {
        if let Some(share) = store.share_tile(*surface, *id)? {
            baked.insert_share(*surface, *id, share);
        }
    }
    let mut out = layers.clone();
    let mut names = Vec::new();
    let t = i64::from(aurora_tile::TILE);
    let mut clipped = Vec::new();
    for layer in filtered {
        let (Some(surface), Some(bounds)) = (layers.surface_id(layer), layers.bounds(layer)) else {
            continue;
        };
        let origin = (bounds.x, bounds.y);
        let (tiles_x, tiles_y) = (
            i64::from(bounds.width.div_ceil(aurora_tile::TILE)),
            i64::from(bounds.height.div_ceil(aurora_tile::TILE)),
        );
        // The baked layer's rectangle grows by one tile on every side (the
        // reach is at most one tile) so the blur's spill is saved; the
        // layer's tile grid stays aligned, every baked tile moving one
        // tile right and down. If the grown origin leaves the document
        // range the layer keeps its rectangle and the spill outside it is
        // reported instead.
        let grown = aurora_core::Rect {
            x: bounds.x - t,
            y: bounds.y - t,
            width: bounds.width.saturating_add(2 * aurora_tile::TILE),
            height: bounds.height.saturating_add(2 * aurora_tile::TILE),
        };
        let extended = out.set_bounds(layer, grown).is_ok();
        let shift = i64::from(extended);
        let (lo, hi_x, hi_y) = (-shift, tiles_x - 1 + shift, tiles_y - 1 + shift);
        // The tiles the layer holds and one more on every side.
        let mine: HashSet<TileId> = held
            .iter()
            .filter(|((s, _), _)| *s == surface)
            .map(|((_, id), _)| *id)
            .collect();
        let Some(x0) = mine.iter().map(|id| i64::from(id.x)).min() else {
            // Nothing stored: nothing to spill either.
            if extended {
                let _ = out.set_bounds(layer, bounds);
            }
            drop_filters(&mut out, layer, layers, &mut names);
            continue;
        };
        let x1 = mine.iter().map(|id| i64::from(id.x)).max().unwrap_or(x0);
        let y0 = mine.iter().map(|id| i64::from(id.y)).min().unwrap_or(0);
        let y1 = mine.iter().map(|id| i64::from(id.y)).max().unwrap_or(y0);
        for id in &mine {
            baked.forget_tile(surface, *id);
        }
        let mut budget = CompositeBudget::for_pass(layers);
        let mut spill_lost = false;
        for ty in (y0 - 1)..=(y1 + 1) {
            for tx in (x0 - 1)..=(x1 + 1) {
                let doc_origin = (origin.0 + tx * t, origin.1 + ty * t);
                // A tile left of or above the layer's origin has no id in
                // its own grid: read it as a window of a shifted grid.
                let (read_id, reference) = match (u32::try_from(tx), u32::try_from(ty)) {
                    (Ok(x), Ok(y)) => (TileId { x, y }, origin),
                    _ => (TileId { x: 0, y: 0 }, (origin.0 + 1, origin.1)),
                };
                let filtered_tile = layer_filter_app::filtered_layer_texels(
                    layer,
                    layers,
                    store,
                    read_id,
                    doc_origin,
                    reference,
                    &mut budget,
                );
                let Some(texels) = filtered_tile else {
                    continue;
                };
                let inside = (lo..=hi_x).contains(&tx) && (lo..=hi_y).contains(&ty);
                match (inside, u32::try_from(tx + shift), u32::try_from(ty + shift)) {
                    (true, Ok(x), Ok(y)) => {
                        baked.insert_share(
                            surface,
                            TileId { x, y },
                            TileShare::Texels(Arc::new(texels)),
                        );
                    }
                    _ => spill_lost = true,
                }
            }
        }
        if budget.store_errors > 0 {
            return Err(other("a layer tile could not be read to apply the filters"));
        }
        if spill_lost {
            clipped.push(layers.name(layer).unwrap_or_default().to_owned());
        }
        drop_filters(&mut out, layer, layers, &mut names);
    }
    Ok(Some(BakedFilters {
        layers: out,
        store: baked,
        names,
        clipped,
    }))
}

fn drop_filters(
    out: &mut aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    original: &aurora_doc::LayerTree,
    names: &mut Vec<String>,
) {
    if out.set_filters(layer, None).is_ok() {
        names.push(original.name(layer).unwrap_or_default().to_owned());
    }
}
