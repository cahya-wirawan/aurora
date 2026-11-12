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
//! ([`crate::layer_filter_app::filtered_layer_texels`] with the layer's
//! origin as the grid anchor), for the tiles the layer holds and one tile
//! beyond (the blur's spill), and only inside the layer's declared bounds
//! (the writer's extent): spill beyond the layer's bounds, or left of or
//! above its origin, is not saved. The mask is not applied here: the writer
//! saves it as the layer's mask, as for a layer without filters.
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
}

impl BakedFilters {
    /// The Not saved entry for the report, naming the layers.
    pub(crate) fn report_entry(&self) -> ReportEntry {
        ReportEntry::new(
            ReportCategory::NotSaved,
            "Layer filters are not saved as editable filters; the filtered pixels were saved \
             instead.",
        )
        .with_affected(&self.names)
    }
}

/// Builds the copy for a document whose layers have visible filters;
/// `Ok(None)` when none does (the writer then reads the document itself).
///
/// # Errors
///
/// An [`IoError`] when the scratch store cannot be made or a tile cannot
/// be read.
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
    for layer in filtered {
        let (Some(surface), Some(bounds)) = (layers.surface_id(layer), layers.bounds(layer)) else {
            continue;
        };
        let origin = (bounds.x, bounds.y);
        let (tiles_x, tiles_y) = (
            bounds.width.div_ceil(aurora_tile::TILE),
            bounds.height.div_ceil(aurora_tile::TILE),
        );
        // The tile range to write: the tiles the layer holds, one more on
        // every side, inside the layer's bounds.
        let mine: HashSet<TileId> = held
            .iter()
            .filter(|((s, _), _)| *s == surface)
            .map(|((_, id), _)| *id)
            .collect();
        let Some(x0) = mine.iter().map(|id| id.x).min() else {
            // Nothing stored: nothing to spill either.
            drop_filters(&mut out, layer, layers, &mut names);
            continue;
        };
        let x1 = mine.iter().map(|id| id.x).max().unwrap_or(x0);
        let y0 = mine.iter().map(|id| id.y).min().unwrap_or(0);
        let y1 = mine.iter().map(|id| id.y).max().unwrap_or(y0);
        let range = |lo: u32, hi: u32, n: u32| {
            lo.saturating_sub(1)..=hi.saturating_add(1).min(n.saturating_sub(1))
        };
        let mut budget = CompositeBudget::for_pass(layers);
        for ty in range(y0, y1, tiles_y) {
            for tx in range(x0, x1, tiles_x) {
                let id = TileId { x: tx, y: ty };
                let doc_origin = (origin.0 + i64::from(tx) * t, origin.1 + i64::from(ty) * t);
                let filtered_tile = layer_filter_app::filtered_layer_texels(
                    layer,
                    layers,
                    store,
                    id,
                    doc_origin,
                    origin,
                    &mut budget,
                );
                match filtered_tile {
                    Some(texels) => {
                        baked.insert_share(surface, id, TileShare::Texels(Arc::new(texels)));
                    }
                    None => {
                        if baked.contains_tile(surface, id) {
                            baked.forget_tile(surface, id);
                        }
                    }
                }
            }
        }
        if budget.store_errors > 0 {
            return Err(other("a layer tile could not be read to apply the filters"));
        }
        drop_filters(&mut out, layer, layers, &mut names);
    }
    Ok(Some(BakedFilters {
        layers: out,
        store: baked,
        names,
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
