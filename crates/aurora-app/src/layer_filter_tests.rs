// ---- task 176: editable layer filters (Gaussian Blur) ----
//
// Included into `fill_and_overlay_tests` (it uses that module's helpers:
// `painted`, `composite_tile_at`, `composite_roots_into_tile`, `texel`,
// `alpha_at`, `near`, `ok`, `TOL`).

fn blur(radius: f32) -> aurora_core::LayerFilters {
    aurora_core::LayerFilters {
        filters: vec![aurora_core::LayerFilter::new(
            aurora_core::FilterKind::GaussianBlur { radius },
        )],
    }
}

fn set_blur(layers: &mut aurora_doc::LayerTree, id: aurora_doc::LayerId, radius: f32) {
    ok(layers.set_filters(id, Some(blur(radius))));
}

/// The one-shot reference: premultiplied white-square coverage over the
/// document `0..512` squared, blurred in one window `reach` larger on
/// every side, cropped. What every tile must equal.
fn blur_reference(rect: (i64, i64, i64, i64), radius: f32) -> Vec<f32> {
    let Some(spec) = aurora_core::GaussianSpec::new(radius) else {
        unreachable!("radius in range");
    };
    let reach = i64::from(spec.reach());
    let side = 512 + 2 * reach;
    let side_u = usize::try_from(side).unwrap_or(0);
    let mut window = vec![0.0_f32; side_u * side_u * 4];
    for y in rect.1..rect.1 + rect.3 {
        for x in rect.0..rect.0 + rect.2 {
            let i = usize::try_from(((y + reach) * side + x + reach) * 4).unwrap_or(0);
            for v in window.iter_mut().skip(i).take(4) {
                *v = 1.0;
            }
        }
    }
    aurora_filters::gaussian_blur_window(&mut window, side_u, &spec);
    aurora_filters::crop_window(&window, side_u, 512)
}

fn ref_at(reference: &[f32], x: usize, y: usize) -> [f32; 4] {
    let i = (y * 512 + x) * 4;
    let g = |k: usize| reference.get(i + k).copied().unwrap_or(f32::NAN);
    [g(0), g(1), g(2), g(3)]
}

#[test]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn a_blurred_layer_composites_to_the_reference_across_tiles() {
    for radius in [3.0_f32, 20.0] {
        // Straddles x = 256 and y = 256.
        let rect = (230_i64, 240_i64, 60_i64, 40_i64);
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let id = painted(&mut layers, &mut store, (0, 0), rect);
        set_blur(&mut layers, id, radius);
        let reference = blur_reference(rect, radius);
        let mut spill = 0;
        for (tx, ty) in [(0_i64, 0_i64), (1, 0), (0, 1), (1, 1)] {
            let acc = composite_tile_at(&layers, &mut store, tx, ty, (0, 0));
            for y in 0..256_usize {
                for x in 0..256_usize {
                    let (gx, gy) = (tx as usize * 256 + x, ty as usize * 256 + y);
                    let want = ref_at(&reference, gx, gy);
                    near(texel(&acc, x, y), want);
                    let outside = !(rect.0..rect.0 + rect.2).contains(&(gx as i64))
                        || !(rect.1..rect.1 + rect.3).contains(&(gy as i64));
                    if outside && want[3] > 0.01 {
                        spill += 1;
                    }
                }
            }
        }
        assert!(spill > 100, "radius {radius}: the blur spilled {spill} texels");
    }
}

#[test]
fn filtered_tiles_are_bit_identical_to_a_one_shot_blur() {
    for radius in [2.0_f32, 12.5] {
        let rect = (200_i64, 200_i64, 112_i64, 112_i64);
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let id = painted(&mut layers, &mut store, (0, 0), rect);
        set_blur(&mut layers, id, radius);
        let reference = blur_reference(rect, radius);
        for (tx, ty) in [(0_u32, 0_u32), (1, 0), (0, 1), (1, 1)] {
            let mut budget = CompositeBudget::for_pass(&layers);
            let got = crate::layer_filter_app::filtered_layer_texels(
                id,
                &layers,
                &mut store,
                aurora_tile::TileId { x: tx, y: ty },
                (i64::from(tx) * 256, i64::from(ty) * 256),
                (0, 0),
                &mut budget,
            );
            let mut tile = Vec::new();
            for y in 0..256_usize {
                for x in 0..256_usize {
                    tile.extend(ref_at(
                        &reference,
                        tx as usize * 256 + x,
                        ty as usize * 256 + y,
                    ));
                }
            }
            let want = crate::layer_filter_app::to_straight_half(&tile);
            assert_eq!(
                got.as_ref().map(|v| v.iter().map(|h| h.to_bits()).collect::<Vec<_>>()),
                want.as_ref().map(|v| v.iter().map(|h| h.to_bits()).collect::<Vec<_>>()),
                "radius {radius} tile ({tx}, {ty})"
            );
        }
    }
}

#[test]
fn a_blur_lands_on_tiles_the_layer_never_painted() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (200, 100, 50, 50));
    set_blur(&mut layers, id, 20.0);
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    assert!(!store.contains_tile(surface, aurora_tile::TileId { x: 1, y: 0 }));
    let mut budget = CompositeBudget::for_pass(&layers);
    let (tile, folded) = composite_roots_into_tile(
        &layers,
        &mut store,
        aurora_tile::TileId { x: 1, y: 0 },
        (256, 0),
        (0, 0),
        &mut budget,
    );
    assert_eq!(folded, 1);
    assert!(alpha_at(&tile, 0, 125) > 0.02, "6 px past the edge");
    assert!(alpha_at(&tile, 200, 125).abs() < TOL, "past the reach");
}

#[test]
fn a_hidden_filter_changes_nothing_and_a_visible_one_does() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 40, 40));
    let plain = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    set_blur(&mut layers, id, 6.0);
    let blurred = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_ne!(plain, blurred);
    let mut hidden = blur(6.0);
    if let Some(f) = hidden.filters.get_mut(0) {
        f.visible = false;
    }
    ok(layers.set_filters(id, Some(hidden)));
    let off = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(
        plain.iter().map(|h| h.to_bits()).collect::<Vec<_>>(),
        off.iter().map(|h| h.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn a_visible_filter_refuses_the_gpu_path_and_a_hidden_one_does_not() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 40, 40));
    assert!(document_qualifies_for_gpu_compositing(&layers));
    set_blur(&mut layers, id, 6.0);
    assert!(!document_qualifies_for_gpu_compositing(&layers));
    let mut hidden = blur(6.0);
    if let Some(f) = hidden.filters.get_mut(0) {
        f.visible = false;
    }
    ok(layers.set_filters(id, Some(hidden)));
    assert!(document_qualifies_for_gpu_compositing(&layers));
}

#[test]
fn a_dab_recomposites_the_neighbours_within_the_blur_reach() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (250, 100, 1, 1));
    let own = aurora_tile::TileId { x: 0, y: 0 };
    let near_tile = aurora_tile::TileId { x: 1, y: 0 };
    let far_tile = aurora_tile::TileId { x: 3, y: 0 };
    for filtered in [false, true] {
        if filtered {
            set_blur(&mut layers, id, 10.0);
        }
        let mut cache = CompositeCache::default();
        for t in [own, near_tile, far_tile] {
            cache.mark_current(t);
        }
        invalidate_painted_tiles(&mut cache, &layers, id, &[own], (0, 0));
        assert!(!cache.is_current(own));
        assert_eq!(cache.is_current(near_tile), !filtered, "filtered {filtered}");
        assert!(cache.is_current(far_tile));
    }
    // Pixel undo/redo goes the same way.
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let CompositeInvalidation::Regions(rects) = stroke_invalidation(&layers, surface, &[own])
    else {
        unreachable!("a pixel stroke is narrow");
    };
    assert!(rects.len() > 1, "{rects:?}");
}

#[test]
fn filtered_tiles_are_cached_and_a_dab_misses_only_its_reach() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 40, 40));
    set_blur(&mut layers, id, 6.0);
    let computes = || crate::layer_filter_app::FILTER_COMPUTES.with(std::cell::Cell::get);
    let before = computes();
    let first = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 1);
    // Opacity is not in the key: a hit, same colours scaled.
    ok(layers.set_opacity(id, 0.5));
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 1, "an opacity edit hits the cache");
    ok(layers.set_opacity(id, 1.0));
    let again = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(first, again);
    assert_eq!(computes() - before, 1);
    // A change of the radius misses.
    set_blur(&mut layers, id, 7.0);
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 2);
    // Painting a texel in the layer misses (the tile's content version).
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let Ok(tile) = store.get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }) else {
        unreachable!("writable");
    };
    if let Some(v) = tile.texels_mut().get_mut(3) {
        *v = half::f16::ONE;
    }
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 3);
}

#[test]
fn a_layer_effect_sees_the_blurred_pixels() {
    // A drop shadow of a blurred square: the shadow's source alpha spreads.
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 20, 20));
    let fx = aurora_core::LayerEffects {
        outer_glows: vec![aurora_core::effects::OuterGlow {
            size: 4.0,
            ..aurora_core::effects::OuterGlow::default()
        }],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    let sharp = alpha_at(&composite_tile_at(&layers, &mut store, 0, 0, (0, 0)), 70, 110);
    set_blur(&mut layers, id, 15.0);
    let soft = alpha_at(&composite_tile_at(&layers, &mut store, 0, 0, (0, 0)), 70, 110);
    // 30 px left of the square: only the blurred source reaches it.
    assert!(soft > sharp + 0.01, "{soft} vs {sharp}");
}

#[test]
fn a_translucent_coloured_layer_blurs_in_premultiplied_space() {
    // Straight (0.8, 0.2, 0.4) at alpha 0.5 over a 100 x 100 square: the
    // blur must keep the straight colour everywhere the result has alpha
    // (the edge fades alpha only), and leave the flat interior untouched.
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted_rgba(&mut layers, &mut store, (60, 60, 100, 100), [0.8, 0.2, 0.4, 0.5]);
    set_blur(&mut layers, id, 6.0);
    let mut budget = CompositeBudget::for_pass(&layers);
    let tile = crate::layer_filter_app::filtered_layer_texels(
        id,
        &layers,
        &mut store,
        aurora_tile::TileId { x: 0, y: 0 },
        (0, 0),
        (0, 0),
        &mut budget,
    )
    .unwrap_or_default();
    let at = |x: usize, y: usize| -> [f32; 4] {
        let i = (y * 256 + x) * 4;
        let g = |k: usize| tile.get(i + k).map_or(f32::NAN, |v| v.to_f32());
        [g(0), g(1), g(2), g(3)]
    };
    // Interior: exactly the stored straight colour and alpha (to f16).
    let inner = at(110, 110);
    for (got, want) in inner.iter().zip([0.8_f32, 0.2, 0.4, 0.5]) {
        assert!((got - want).abs() < 2e-3, "{inner:?}");
    }
    // The edge (x = 60) and just outside (x = 55): alpha fades, hue stays.
    for x in [60_usize, 55, 66] {
        let p = at(x, 110);
        assert!(p[3] > 0.01 && p[3] < 0.5, "alpha at {x}: {p:?}");
        for (got, want) in p.iter().take(3).zip([0.8_f32, 0.2, 0.4]) {
            assert!((got - want).abs() < 4e-3, "straight colour at {x}: {p:?}");
        }
    }
}
