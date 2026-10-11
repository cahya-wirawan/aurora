// ---- 0.191.0 (E5): outer glows ----
//
// Included into `fill_and_overlay_tests` (it uses that module's helpers:
// `painted`, `composite_tile_at`, `texel`, `alpha_at`, `near`, `TOL`).

use crate::{glow_tile_coverage, stroke_invalidation as glow_stroke_invalidation};

fn glow(size: f32, spread: f32, rgb: [f32; 3]) -> aurora_core::effects::OuterGlow {
    aurora_core::effects::OuterGlow {
        blend_mode: aurora_core::effects::EffectBlendMode::Normal,
        color: aurora_core::effects::EffectColor {
            r: rgb[0],
            g: rgb[1],
            b: rgb[2],
        },
        opacity: 1.0,
        size,
        spread,
        ..aurora_core::effects::OuterGlow::default()
    }
}

fn set_glows(
    layers: &mut aurora_doc::LayerTree,
    id: aurora_doc::LayerId,
    glows: Vec<aurora_core::effects::OuterGlow>,
) {
    let fx = aurora_core::LayerEffects {
        outer_glows: glows,
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
}

/// An opaque `rgba` rectangle (document space) on a new pixel layer whose
/// tile grid starts at `origin`.
fn painted_rgba(
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    rect: (i64, i64, i64, i64),
    rgba: [f32; 4],
) -> aurora_doc::LayerId {
    let id = painted(layers, store, (0, 0), (0, 0, 0, 0));
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let t = i64::from(aurora_tile::TILE);
    for ty in (rect.1 / t)..=((rect.1 + rect.3 - 1) / t) {
        for tx in (rect.0 / t)..=((rect.0 + rect.2 - 1) / t) {
            let tile_id = aurora_tile::TileId {
                x: u32::try_from(tx).unwrap_or(0),
                y: u32::try_from(ty).unwrap_or(0),
            };
            let Ok(tile) = store.get_mut(surface, tile_id) else {
                unreachable!("writable");
            };
            for y in rect.1.max(ty * t)..(rect.1 + rect.3).min(ty * t + t) {
                for x in rect.0.max(tx * t)..(rect.0 + rect.2).min(tx * t + t) {
                    let i = usize::try_from(((y - ty * t) * t + (x - tx * t)) * 4).unwrap_or(0);
                    for (k, v) in tile.texels_mut().iter_mut().skip(i).take(4).enumerate() {
                        *v = half::f16::from_f32(rgba.get(k).copied().unwrap_or(0.0));
                    }
                }
            }
            tile.mark_dirty(aurora_core::Rect {
                x: 0,
                y: 0,
                width: aurora_tile::TILE,
                height: aurora_tile::TILE,
            });
        }
    }
    id
}

/// The glow plane over the document square `0..512`, from a one-shot
/// blur of `rect` (opaque) in a window `apron` larger on every side —
/// what every tile's plane must equal bit for bit.
fn reference_plane(rect: (i64, i64, i64, i64), size: f32, spread: f32) -> Vec<f32> {
    let g = glow(size, spread, [1.0, 0.0, 0.0]).geometry();
    let a = i64::from(g.apron);
    let side = 512 + 2 * a;
    let side_u = usize::try_from(side).unwrap_or(0);
    let mut alpha = vec![0.0_f32; side_u * side_u];
    for y in rect.1..rect.1 + rect.3 {
        for x in rect.0..rect.0 + rect.2 {
            let (wx, wy) = (x + a, y + a);
            if (0..side).contains(&wx)
                && (0..side).contains(&wy)
                && let Some(v) = alpha.get_mut(usize::try_from(wy * side + wx).unwrap_or(0))
            {
                *v = 1.0;
            }
        }
    }
    aurora_filters::shadow_coverage(
        &alpha,
        side_u,
        g.choke as usize,
        g.blur_radius as usize,
        512,
    )
}

fn plane_at(plane: &[f32], x: usize, y: usize) -> f32 {
    plane.get(y * 512 + x).copied().unwrap_or(0.0)
}

/// 40 x 40 white square at (100, 100) with one glow.
fn glowing_square(
    glows: Vec<aurora_core::effects::OuterGlow>,
) -> (
    tempfile::TempDir,
    aurora_tile::TileStore,
    aurora_doc::LayerTree,
    aurora_doc::LayerId,
) {
    let (dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 40, 40));
    set_glows(&mut layers, id, glows);
    (dir, store, layers, id)
}

#[test]
fn glow_is_the_shape_dilated_and_blurred_on_every_side() {
    let (_d, mut store, layers, _) = glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
    let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    let plane = reference_plane((100, 100, 40, 40), 10.0, 0.0);
    // Red at coverage c over transparent is premultiplied (c, 0, 0, c).
    for (x, y) in [
        (99, 120),
        (97, 120),
        (140, 120),
        (120, 99),
        (120, 141),
        (90, 90),
    ] {
        let c = plane_at(&plane, x, y);
        near(texel(&acc, x, y), [c, 0.0, 0.0, c]);
    }
    // Symmetric: no offset. 1 px outside each edge has the same coverage.
    let left = alpha_at(&acc, 99, 120);
    for (x, y) in [(140, 120), (120, 99), (120, 140)] {
        assert!((alpha_at(&acc, x, y) - left).abs() < TOL, "{x},{y}");
    }
    assert!(left > 0.3 && left < 0.7, "edge coverage {left}");
    // Past the reach: nothing. Inside: the white content on top.
    assert!(alpha_at(&acc, 100 - 40, 120).abs() < TOL);
    near(texel(&acc, 120, 120), [1.0; 4]);
}

#[test]
fn spread_100_is_a_hard_edged_dilation() {
    let (_d, mut store, layers, _) = glowing_square(vec![glow(8.0, 1.0, [1.0, 0.0, 0.0])]);
    let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    near(texel(&acc, 100 - 8, 120), [1.0, 0.0, 0.0, 1.0]);
    assert!(alpha_at(&acc, 100 - 10, 120).abs() < TOL);
    near(texel(&acc, 140 + 7, 120), [1.0, 0.0, 0.0, 1.0]);
    // A partial spread widens the solid band without a softer rim.
    let (_d, mut store, layers, _) = glowing_square(vec![glow(20.0, 0.0, [1.0, 0.0, 0.0])]);
    let soft = alpha_at(
        &composite_tile_at(&layers, &mut store, 0, 0, (0, 0)),
        90,
        120,
    );
    let (_d, mut store, layers, _) = glowing_square(vec![glow(20.0, 0.5, [1.0, 0.0, 0.0])]);
    let spread = alpha_at(
        &composite_tile_at(&layers, &mut store, 0, 0, (0, 0)),
        90,
        120,
    );
    assert!(spread > soft, "{spread} vs {soft}");
}

#[test]
fn glow_is_seamless_across_tile_boundaries_even_at_exactly_one_apron() {
    // Size 40: radius 18, apron 54. A shape whose last column is 201 is
    // one pixel outside tile (1, 0)'s window (starts at 256 - 54 = 202);
    // one whose last column is 202 is the first one inside it.
    let size = 40.0;
    let apron = i64::from(glow(size, 0.0, [1.0; 3]).geometry().apron);
    assert_eq!(apron, 54);
    for width in [52_i64, 53] {
        for origin in [(0_i64, 0_i64), (-300, -300)] {
            let (_dir, mut store) = real_tile_store();
            let mut layers = aurora_doc::LayerTree::new();
            let rect = (150, 100, width, 40);
            let id = painted(&mut layers, &mut store, origin, rect);
            set_glows(&mut layers, id, vec![glow(size, 0.0, [1.0, 0.0, 0.0])]);
            let g = glow(size, 0.0, [1.0, 0.0, 0.0]).geometry();
            let reference = reference_plane(rect, size, 0.0);
            let mut seen_tile_1 = false;
            for (tx, ty) in [(0_i64, 0_i64), (1, 0), (0, 1), (1, 1)] {
                let mut budget = CompositeBudget::for_pass(&layers);
                let at = CompositeTileAt {
                    doc_origin: (tx * 256, ty * 256),
                    reference_origin: (0, 0),
                };
                let cov = glow_tile_coverage(id, &layers, &mut store, at, None, g, &mut budget);
                let (ox, oy) = (
                    usize::try_from(tx).unwrap_or(0) * 256,
                    usize::try_from(ty).unwrap_or(0) * 256,
                );
                match cov {
                    Some(cov) => {
                        if (tx, ty) == (1, 0) && cov.iter().any(|v| *v > 0.0) {
                            seen_tile_1 = true;
                        }
                        for (i, v) in cov.iter().enumerate() {
                            assert_eq!(
                                v.to_bits(),
                                plane_at(&reference, ox + i % 256, oy + i / 256).to_bits(),
                                "width {width} {origin:?} tile ({tx}, {ty}) texel {i}"
                            );
                        }
                    }
                    None => {
                        for i in 0..256 * 256 {
                            assert!(
                                plane_at(&reference, ox + i % 256, oy + i / 256) <= 0.0,
                                "width {width} {origin:?} tile ({tx}, {ty}) texel {i}"
                            );
                        }
                    }
                }
            }
            // The glow reaches tile (1, 0) exactly when the shape is within
            // one apron of it.
            assert_eq!(seen_tile_1, width == 53, "width {width}");
        }
    }
}

#[test]
fn glow_lands_on_tiles_the_layer_never_painted() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (200, 100, 51, 56));
    set_glows(&mut layers, id, vec![glow(20.0, 0.0, [1.0, 0.0, 0.0])]);
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
    assert!(
        alpha_at(&tile, 3, 128) > 0.05,
        "x = 259 is 9 px past the edge"
    );
    assert!(alpha_at(&tile, 200, 128).abs() < TOL, "past the reach");
}

#[test]
fn glow_reads_are_bounded_and_the_size_is_capped() {
    // The largest valid size reaches 339 px and is not capped.
    let largest = glow(aurora_core::effects::MAX_EFFECT_SIZE, 0.0, [1.0; 3]).geometry();
    assert_eq!((largest.apron, largest.capped), (339, false));
    let huge = glow(100_000.0, 0.1, [1.0; 3]);
    let g = huge.geometry();
    assert!(g.capped && g.apron <= aurora_core::effects::MAX_SHADOW_APRON);
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    // Every tile of a 10 x 10 block stored.
    let id = painted(&mut layers, &mut store, (0, 0), (0, 0, 2560, 2560));
    APRON_CELL_READS.with(|n| n.set(0));
    let mut budget = CompositeBudget::for_pass(&layers);
    let at = CompositeTileAt {
        doc_origin: (5 * 256, 5 * 256),
        reference_origin: (0, 0),
    };
    let _ = glow_tile_coverage(id, &layers, &mut store, at, None, g, &mut budget);
    let reads = APRON_CELL_READS.with(std::cell::Cell::get);
    assert!((1..=25).contains(&reads), "{reads} cells");
    // `set_effects` refuses the oversize glow; the largest valid one works.
    let fx = aurora_core::LayerEffects {
        outer_glows: vec![huge],
        ..aurora_core::LayerEffects::default()
    };
    assert!(layers.set_effects(id, Some(fx)).is_err());
}

#[test]
fn glow_uses_its_own_mode_and_opacity() {
    // Opaque 0.5 grey backdrop; a glow of colour (1, 0.5, 0) at 0.6.
    let rgb = [1.0_f32, 0.5, 0.0];
    let plane = reference_plane((100, 100, 20, 20), 10.0, 0.0);
    let c = plane_at(&plane, 99, 110);
    assert!(c > 0.2, "{c}");
    let a = c * 0.6;
    let b = 0.5_f32;
    for (mode, blend) in [
        (
            aurora_core::effects::EffectBlendMode::Normal,
            (|_b: f32, s: f32| s) as fn(f32, f32) -> f32,
        ),
        (
            aurora_core::effects::EffectBlendMode::Screen,
            |b: f32, s: f32| b + s - b * s,
        ),
        (
            aurora_core::effects::EffectBlendMode::Multiply,
            |b: f32, s: f32| b * s,
        ),
    ] {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let _back = painted_rgba(&mut layers, &mut store, (0, 0, 256, 256), [b, b, b, 1.0]);
        let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 20, 20));
        let mut g = glow(10.0, 0.0, rgb);
        g.blend_mode = mode;
        g.opacity = 0.6;
        set_glows(&mut layers, id, vec![g]);
        let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
        let want = rgb.map(|s| b + (blend(b, s) - b) * a);
        near(texel(&acc, 99, 110), [want[0], want[1], want[2], 1.0]);
    }
}

#[test]
fn layer_opacity_fades_the_glow_and_fill_does_not() {
    let probe = |opacity: f32, fill: f32| {
        let (_d, mut store, mut layers, id) =
            glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
        ok(layers.set_opacity(id, opacity));
        ok(layers.set_fill_opacity(id, fill));
        alpha_at(
            &composite_tile_at(&layers, &mut store, 0, 0, (0, 0)),
            99,
            120,
        )
    };
    let full = probe(1.0, 1.0);
    assert!(full > 0.3);
    assert!((probe(1.0, 0.25) - full).abs() < TOL, "Fill must not fade");
    assert!((probe(1.0, 0.0) - full).abs() < TOL, "Fill 0 keeps it");
    assert!((probe(0.5, 1.0) - full * 0.5).abs() < TOL, "opacity fades");
    assert!((probe(0.5, 0.25) - full * 0.5).abs() < TOL, "both");
}

#[test]
fn the_layer_does_not_knock_the_glow_out() {
    // A Fill-0 layer paints nothing of its own; the glow still fills the
    // interior (a shadow's default knockout would clear it).
    let (_d, mut store, mut layers, id) = glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
    ok(layers.set_fill_opacity(id, 0.0));
    let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    near(texel(&acc, 120, 120), [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn a_glow_sits_above_the_shadow_and_beneath_the_content() {
    let (_d, mut store, mut layers, id) = glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
    let shadow = aurora_core::effects::DropShadow {
        color: aurora_core::effects::EffectColor {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        },
        ..shadow(90.0, 0.0, 10.0, 0.0)
    };
    let fx = aurora_core::LayerEffects {
        drop_shadows: vec![shadow],
        outer_glows: vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    let c = plane_at(&reference_plane((100, 100, 40, 40), 10.0, 0.0), 99, 120);
    // Red over black over nothing: premultiplied red is c itself (glow on
    // top). Swapped, black on top would leave c (1 - c).
    let got = texel(&acc, 99, 120);
    assert!(c > 0.3 && c < 0.7, "{c}");
    assert!((got[0] - c).abs() < TOL, "red {} vs {c}", got[0]);
    assert!(
        (got[3] - (c + c * (1.0 - c))).abs() < TOL,
        "alpha {}",
        got[3]
    );
    // The white content still covers both.
    near(texel(&acc, 120, 120), [1.0; 4]);
}

#[test]
fn editing_a_glowing_layer_invalidates_the_apron_tiles() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, &mut store, (0, 0), (250, 100, 1, 1));
    let neighbour = aurora_tile::TileId { x: 1, y: 0 };
    let own = aurora_tile::TileId { x: 0, y: 0 };
    for glowing in [false, true] {
        if glowing {
            set_glows(&mut layers, id, vec![glow(20.0, 0.0, [1.0, 0.0, 0.0])]);
        }
        let mut cache = CompositeCache::default();
        cache.mark_current(own);
        cache.mark_current(neighbour);
        // A dab.
        invalidate_painted_tiles(&mut cache, &layers, id, &[own], (0, 0));
        assert!(!cache.is_current(own));
        assert_eq!(cache.is_current(neighbour), !glowing, "glowing {glowing}");
    }
    // Pixel undo/redo goes the same way.
    let Some(surface) = layers.surface_id(id) else {
        unreachable!("pixel layer");
    };
    let CompositeInvalidation::Regions(rects) = glow_stroke_invalidation(&layers, surface, &[own])
    else {
        unreachable!("a pixel stroke is narrow");
    };
    assert!(rects.len() > 1, "{rects:?}");
    let reach = effect_reach_rects(
        &layers,
        id,
        &[aurora_core::Rect {
            x: 250,
            y: 100,
            width: 1,
            height: 1,
        }],
    );
    let apron = i64::from(glow(20.0, 0.0, [1.0; 3]).geometry().apron);
    assert!(
        reach
            .iter()
            .any(|r| r.x == 250 - apron && r.width == u32::try_from(1 + 2 * apron).unwrap_or(0)),
        "{reach:?}"
    );
    // A Precise glow draws nothing, so reaches nothing.
    let mut precise = glow(20.0, 0.0, [1.0, 0.0, 0.0]);
    precise.precise = true;
    set_glows(&mut layers, id, vec![precise]);
    assert_eq!(effect_reach_rects(&layers, id, &[]).len(), 0);
    assert!(!layer_renders_effects(&layers, id));
}

#[test]
fn a_layer_with_a_shown_glow_refuses_the_gpu_fast_path() {
    let (_dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [0.0, 1.0, 0.0, 1.0]);
    assert!(document_qualifies_for_gpu_compositing(&layers));
    set_glows(
        &mut layers,
        id,
        vec![aurora_core::effects::OuterGlow::default()],
    );
    assert!(layer_renders_effects(&layers, id));
    assert!(!document_qualifies_for_gpu_compositing(&layers));
    // Switched off, Precise (not drawn) or master off: the GPU path stays.
    let mut off = aurora_core::effects::OuterGlow::default();
    off.gate.enabled = false;
    let precise = aurora_core::effects::OuterGlow {
        precise: true,
        ..aurora_core::effects::OuterGlow::default()
    };
    for glows in [vec![off], vec![precise]] {
        set_glows(&mut layers, id, glows);
        assert!(document_qualifies_for_gpu_compositing(&layers));
    }
    let fx = aurora_core::LayerEffects {
        enabled: false,
        outer_glows: vec![aurora_core::effects::OuterGlow::default()],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    assert!(document_qualifies_for_gpu_compositing(&layers));
}

#[test]
fn a_precise_glow_is_not_drawn_and_leaves_the_layer_bit_identical() {
    let (_d, mut store, mut layers, id) = glowing_square(vec![]);
    let plain = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    let mut precise = glow(10.0, 0.0, [1.0, 0.0, 0.0]);
    precise.precise = true;
    set_glows(&mut layers, id, vec![precise]);
    let with = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert!(
        plain
            .iter()
            .zip(&with)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

#[test]
fn glow_coverage_is_cached_apart_from_a_shadows_and_hit_by_colour_edits() {
    clear_shadow_cache();
    let (_d, mut store, mut layers, id) = glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
    let shadow = shadow(90.0, 0.0, 10.0, 0.0);
    let fx = aurora_core::LayerEffects {
        drop_shadows: vec![shadow],
        outer_glows: vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx.clone())));
    let before = computes();
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    // Same geometry, same offset: still two entries (one per kind).
    assert_eq!(computes() - before, 2);
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 2, "second pass is all hits");
    // Colour, opacity and mode edits hit.
    let mut edited = fx.clone();
    if let Some(g) = edited.outer_glows.first_mut() {
        g.color.g = 0.5;
        g.opacity = 0.3;
        g.blend_mode = aurora_core::effects::EffectBlendMode::Screen;
    }
    ok(layers.set_effects(id, Some(edited)));
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(computes() - before, 2, "colour/opacity/mode edits hit");
    // A size edit misses.
    let mut bigger = fx;
    if let Some(g) = bigger.outer_glows.first_mut() {
        g.size = 20.0;
    }
    ok(layers.set_effects(id, Some(bigger)));
    let _ = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    assert_eq!(
        computes() - before,
        3,
        "a size edit recomputes the glow only"
    );
}

// ---- the reference: Photoshop's merged image ----

/// Mean abs error per channel (8-bit levels) over `x0..x1, y0..y1`.
fn region_mean(
    image: &aurora_io::Image,
    merged: &aurora_io::Image,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
) -> [f64; 3] {
    let mut sum = [0.0_f64; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let got = image_pixel(image, x, y);
            let want = image_pixel(merged, x, y);
            let a = f64::from(got[3]);
            for (c, s) in sum.iter_mut().enumerate() {
                let g = f64::from(got.get(c).copied().unwrap_or(0.0)) * a + (1.0 - a);
                let w = f64::from(want.get(c).copied().unwrap_or(0.0));
                *s += ((g * 255.0).round() - (w * 255.0).round()).abs();
            }
            n += 1.0;
        }
    }
    sum.map(|s| s / n)
}

fn find_layer_named(layers: &aurora_doc::LayerTree, name: &str) -> Option<aurora_doc::LayerId> {
    let mut pending = layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        if layers.name(id) == Some(name) {
            return Some(id);
        }
        pending.extend(layers.children(id).unwrap_or(&[]));
    }
    None
}

/// `layer_effects.psd`'s "Outer Glow" text layer (Softer, white, Screen,
/// 35 %, size 41, no spread) against Photoshop's merged image over the
/// two margins left and right of its text (49 x 48 px each, glow only), with the
/// glow removed and drawn, and the box radius swept (printed).
const GLOW_REGIONS: [(u32, u32, u32, u32); 2] = [(50, 98, 99, 146), (499, 98, 548, 146)];

fn glow_region_mean(image: &aurora_io::Image, merged: &aurora_io::Image) -> [f64; 3] {
    let a = region_mean(image, merged, GLOW_REGIONS[0]);
    let b = region_mean(image, merged, GLOW_REGIONS[1]);
    let mut out = [0.0; 3];
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = f64::midpoint(*x, y);
    }
    out
}

type GlowSweep = Vec<(u32, [f64; 3])>;

fn glow_reference_errors(per_radius: &[u32]) -> Option<([f64; 3], GlowSweep)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/layer_effects.psd");
    let bytes = std::fs::read(&path).ok()?;
    let merged = aurora_io::decode_psd_merged_image(&bytes).ok()?;
    let (_scratch, mut store) = real_tile_store();
    let (mut document, image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let id = find_layer_named(&document.layers, "Outer Glow")?;
    let fx = document.layers.effects(id)?.clone();
    fx.outer_glows.first()?;
    let (w, h) = document.canvas_size;
    let with = glow_region_mean(&image, &merged);
    let mut stripped = fx.clone();
    stripped.outer_glows.clear();
    ok(document.layers.set_effects(id, Some(stripped)));
    let without = ok(composite_document(&document.layers, &mut store, w, h));
    let without = glow_region_mean(&without, &merged);
    let mut sweep = vec![(u32::MAX, with)];
    for &r in per_radius {
        let mut f = fx.clone();
        // size * 0.45 rounds to r.
        if let Some(g) = f.outer_glows.first_mut() {
            g.size = (r as f32 + 0.1) / aurora_core::effects::GLOW_BLUR_RADIUS_PER_PX;
        }
        ok(document.layers.set_effects(id, Some(f)));
        let image = ok(composite_document(&document.layers, &mut store, w, h));
        sweep.push((r, glow_region_mean(&image, &merged)));
    }
    Some((without, sweep))
}

#[test]
fn the_fitted_glow_radius_reduces_the_error_against_photoshops_merged_image() {
    let radii = [10_u32, 14, 16, 18, 20, 22, 26, 30];
    let Some((without, sweep)) = glow_reference_errors(&radii) else {
        println!("SKIPPED: layer_effects.psd (or its Outer Glow layer) not present");
        return;
    };
    println!("GLOW FIT without glow: {without:?}");
    for (r, e) in &sweep {
        println!("GLOW FIT radius {r}: {e:?}");
    }
    let drawn = sweep.first().map_or([f64::MAX; 3], |(_, e)| *e);
    // The glow improves every channel over drawing none.
    for (d, w) in drawn.iter().zip(without) {
        assert!(*d < w, "{drawn:?} vs {without:?}");
    }
    // Pinned (measured 0.191.0).
    for (got, want) in drawn.iter().zip(GLOW_REGION_MEAN) {
        assert!(*got <= want + 1e-3, "{drawn:?} vs {GLOW_REGION_MEAN:?}");
    }
}

/// Mean error per channel over [`GLOW_REGIONS`] with the glow drawn
/// (0.191.0): 4.3527 with it removed. Box radius sweep over the same
/// margins (radius: mean): 10: 2.903, 14: 2.426, 16: 2.239, **18: 2.163**
/// (the shared 0.45 mapping), 20: 2.181, 22: 2.268, 26: 2.536, 30: 2.800.
const GLOW_REGION_MEAN: [f64; 3] = [2.1631, 2.1631, 2.1631];

#[test]
fn opaque_content_hides_its_glow_at_layer_opacity_as_one_unit() {
    // With a glow, glow and content are composed as one unit and then faded
    // by the layer's opacity, so an opaque layer at 50 % hides its own glow.
    let (_d, mut store, mut layers, id) = glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
    ok(layers.set_opacity(id, 0.5));
    let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
    near(texel(&acc, 120, 120), [0.5, 0.5, 0.5, 0.5]);
}

#[test]
fn a_dissolve_layers_glow_is_faded_by_its_opacity_directly() {
    // A Dissolve layer's dither carries its own opacity, so it is not one
    // faded unit; its glow is faded by the opacity itself.
    let probe = |opacity: f32| {
        let (_d, mut store, mut layers, id) =
            glowing_square(vec![glow(10.0, 0.0, [1.0, 0.0, 0.0])]);
        ok(layers.set_blend_mode(id, aurora_doc::BlendMode::Dissolve));
        ok(layers.set_opacity(id, opacity));
        alpha_at(
            &composite_tile_at(&layers, &mut store, 0, 0, (0, 0)),
            99,
            120,
        )
    };
    let full = probe(1.0);
    assert!(full > 0.3);
    assert!((probe(0.5) - full * 0.5).abs() < TOL);
}
