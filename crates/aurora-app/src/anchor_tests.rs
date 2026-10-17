//! Differentials for the document-anchored composite grid (60 FPS round
//! 3): every document pixel inside the canvas composites to the same bits
//! on the document grid (`(0, 0)`) as on the grid the app used before
//! round 3 (`min(active layer origin, 0)` per axis), on random trees with
//! layers at negative, positive and straddling origins (masks, clipping,
//! adjustments, Pass Through groups, shadows, strokes, glows, overlays)
//! and on the corpus. Export, the Curves histogram and the clipboard
//! already composited from `(0, 0)`, so this is also "the live canvas
//! equals the export".

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use crate::move_invalidation_tests::{corpus_files, pixel_layers, reaching_effects};
use crate::occlusion_cull_tests::{
    Rng, clear_shadow_cache, layer, mismatches, ok, opaque, random_node, rect, store_with_budget,
};
use crate::{CompositeBudget, composite_reference_origin, composite_roots_into_tile};

const T: i64 = aurora_tile::TILE as i64;

/// The pre-round-3 anchor for `active`: its origin capped at the corner.
fn old_anchor(layers: &aurora_doc::LayerTree, active: aurora_doc::LayerId) -> (i64, i64) {
    layers
        .bounds(active)
        .map_or((0, 0), |b| (b.x.min(0), b.y.min(0)))
}

/// The canvas `(w, h)` composited on the grid anchored at `anchor`, as
/// straight rows of `w * 4` samples (fresh: the shadow cache is cleared
/// before every tile so no grid feeds another).
fn canvas_on_grid(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    anchor: (i64, i64),
    (w, h): (u32, u32),
) -> Vec<half::f16> {
    let (w, h) = (i64::from(w), i64::from(h));
    let mut out = vec![half::f16::from_f32(-7.0); (w * h * 4) as usize];
    let first = ((0 - anchor.0).div_euclid(T), (0 - anchor.1).div_euclid(T));
    let last = (
        (w - 1 - anchor.0).div_euclid(T),
        (h - 1 - anchor.1).div_euclid(T),
    );
    for ty in first.1..=last.1 {
        for tx in first.0..=last.0 {
            let id = aurora_tile::TileId {
                x: tx as u32,
                y: ty as u32,
            };
            let origin = (anchor.0 + tx * T, anchor.1 + ty * T);
            clear_shadow_cache();
            let mut budget = CompositeBudget::for_pass(layers);
            budget.next_tile(layers);
            let (tile, _) =
                composite_roots_into_tile(layers, store, id, origin, anchor, &mut budget);
            for row in 0..T {
                let y = origin.1 + row;
                if !(0..h).contains(&y) {
                    continue;
                }
                for col in 0..T {
                    let x = origin.0 + col;
                    if !(0..w).contains(&x) {
                        continue;
                    }
                    let src = ((row * T + col) * 4) as usize;
                    let dst = ((y * w + x) * 4) as usize;
                    if let (Some(s), Some(d)) = (tile.get(src..src + 4), out.get_mut(dst..dst + 4))
                    {
                        d.copy_from_slice(s);
                    }
                }
            }
        }
    }
    out
}

/// Compares the canvas on the document grid against the canvas on each
/// distinct old anchor the pixel layers would have given; returns how
/// many old anchors other than `(0, 0)` were compared.
#[track_caller]
fn check_tree(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    canvas: (u32, u32),
    max_anchors: usize,
    what: &str,
) -> usize {
    let document = composite_reference_origin(layers, None);
    assert_eq!(document, (0, 0), "the grid is the document's");
    let fresh = canvas_on_grid(layers, store, document, canvas);
    let mut anchors: Vec<(i64, i64)> = pixel_layers(layers)
        .into_iter()
        .map(|id| {
            assert_eq!(
                composite_reference_origin(layers, Some(id)),
                document,
                "{what}: the active layer does not move the grid"
            );
            old_anchor(layers, id)
        })
        .filter(|&a| a != (0, 0))
        .collect();
    anchors.sort_unstable();
    anchors.dedup();
    anchors.truncate(max_anchors);
    for &anchor in &anchors {
        let old = canvas_on_grid(layers, store, anchor, canvas);
        let bad = mismatches(&old, &fresh);
        assert!(
            bad.is_empty(),
            "{what}: the old anchor {anchor:?} gives {} different samples, first {:?}",
            bad.len(),
            bad.get(..bad.len().min(4))
        );
    }
    anchors.len()
}

#[test]
fn random_trees_composite_the_same_on_the_document_grid_as_on_the_old_anchor() {
    let mut rng = Rng(0x5EED_0127_0003_0001);
    let (mut compared, mut positive, mut straddling, mut negative) = (0, 0, 0, 0);
    let (mut clipped, mut masked, mut effects) = (0, 0, 0);
    let trees = 48;
    for tree in 0..trees {
        let (_d, mut store) = store_with_budget(1024);
        let mut layers = aurora_doc::LayerTree::new();
        for _ in 0..=rng.below(4) {
            random_node(&mut rng, &mut layers, &mut store, None, 1);
        }
        for id in pixel_layers(&layers) {
            // Origins of every kind: well inside, straddling one edge or
            // both, and wholly left of / above the document.
            if let Some(b) = layers.bounds(id) {
                let (x, y) = match rng.below(4) {
                    0 => (1 + rng.below(400) as i64, 1 + rng.below(300) as i64),
                    1 => (rng.below(300) as i64 - 299, 1 + rng.below(300) as i64),
                    2 => (-(i64::from(b.width)) - rng.below(40) as i64, b.y),
                    _ => (b.x, b.y),
                };
                ok(layers.set_bounds(id, aurora_core::Rect { x, y, ..b }));
            }
            if let Some(b) = layers.bounds(id) {
                if b.x > 0 && b.y > 0 {
                    positive += 1;
                } else if b.x + i64::from(b.width) <= 0 || b.y + i64::from(b.height) <= 0 {
                    negative += 1;
                } else {
                    straddling += 1;
                }
            }
            if rng.chance(30) {
                ok(layers.set_effects(id, Some(reaching_effects(&mut rng))));
                effects += 1;
            }
            if rng.chance(20) && layers.set_clipped(id, true).is_ok() {
                clipped += 1;
            }
            if rng.chance(20) && layers.add_mask(id, rect(-60, 80, 400, 300)).is_ok() {
                masked += 1;
                if rng.chance(50) {
                    ok(layers.set_mask_inverted(id, true));
                }
                if rng.chance(30) {
                    ok(layers.set_mask_density(id, 0.5));
                }
            }
        }
        compared += check_tree(&layers, &mut store, (700, 560), 3, &format!("tree {tree}"));
    }
    println!(
        "anchor differential: {trees} trees, {compared} old anchors compared, origins \
         {positive} positive / {straddling} straddling / {negative} outside, {clipped} clipped, \
         {masked} masked, {effects} with reaching effects"
    );
    assert!(compared > 40, "old anchors compared: {compared}");
    assert!(positive > 10 && straddling > 10 && negative > 5);
    assert!(clipped > 5 && masked > 5 && effects > 10);
}

/// A layer whose shadow reaches from far left of the document into it:
/// the shadow is drawn from content the document grid never composites a
/// tile for, and must still land.
#[test]
fn a_shadow_from_left_of_the_document_lands_on_the_document_grid() {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(
        &mut layers,
        &mut store,
        None,
        rect(-300, 20, 280, 200),
        &opaque,
    );
    let fx = aurora_core::LayerEffects {
        drop_shadows: vec![aurora_core::effects::DropShadow {
            distance: 60.0,
            size: 8.0,
            use_global_light: false,
            angle: 180.0,
            ..aurora_core::effects::DropShadow::default()
        }],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    let canvas = canvas_on_grid(&layers, &mut store, (0, 0), (300, 300));
    let alpha_at = |x: usize, y: usize| canvas.get((y * 300 + x) * 4 + 3).map(|a| a.to_f32());
    assert!(
        alpha_at(10, 120).is_some_and(|a| a > 0.0),
        "the shadow lands inside the document: {:?}",
        alpha_at(10, 120)
    );
    assert_eq!(check_tree(&layers, &mut store, (300, 300), 1, "shadow"), 1);
}

#[test]
fn corpus_composites_the_same_on_the_document_grid_as_on_the_old_anchor() {
    let files = corpus_files();
    if files.is_empty() {
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        println!("SKIPPED: corpus absent");
        return;
    }
    let (mut opened, mut compared) = (0, 0);
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(document) = crate::open_psd_document(&bytes) else {
            continue;
        };
        let (_d, mut store) = store_with_budget(512);
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = crate::replace_document_pixels(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        if failed > 0 || pixel_layers(&document.layers).is_empty() {
            continue;
        }
        let (w, h) = document.canvas_size;
        if u64::from(w) * u64::from(h) > 4_000_000 {
            continue;
        }
        opened += 1;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        compared += check_tree(&document.layers, &mut store, (w, h), 2, &name);
    }
    println!("corpus anchor differential: {opened} files, {compared} old anchors compared");
    assert!(opened > 10 && compared > 5, "{opened} {compared}");
}
