// ---- task 111: pattern overlays ----
//
// Included into `fill_and_overlay_tests` next to the gradient overlay
// tests (same helpers: `solid`, `composite`, `texel`, `near`, `ok`,
// `real_tile_store`, `bounds` (a 10 x 10 layer at the origin)).

use aurora_core::effects::PatternOverlay;
use aurora_core::pattern::Pattern;

/// A 2 x 2 pattern: red, green / blue, white, opaque, `id`.
fn quad(id: &str) -> Pattern {
    let m = 65_535u16;
    let texels = vec![m, 0, 0, m, 0, m, 0, m, 0, 0, m, m, m, m, m, m];
    match Pattern::new(id.to_owned(), String::new(), 2, 2, texels) {
        Ok(p) => p,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn pat_overlay(id: &str, scale: f32, origin: (f64, f64)) -> PatternOverlay {
    PatternOverlay {
        gate: aurora_core::effects::EffectGate::default(),
        blend_mode: EffectBlendMode::Normal,
        opacity: 1.0,
        pattern: id.to_owned(),
        scale,
        origin,
    }
}

/// A 10 x 10 opaque `base` layer carrying `overlay` over a pool holding `quad("q")`.
fn patterned(
    store: &mut aurora_tile::TileStore,
    base: [f32; 4],
    overlay: PatternOverlay,
) -> (aurora_doc::LayerTree, aurora_doc::LayerId) {
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, store, None, base);
    ok(layers.add_pattern(quad("q")));
    let fx = aurora_core::LayerEffects {
        pattern_overlays: vec![overlay],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    (layers, id)
}

const PRED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const PGREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const PBLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const PWHITE: [f32; 4] = [1.0; 4];

#[test]
fn a_pattern_tiles_from_the_origin_at_whole_texels_at_100_percent() {
    let (_d, mut store) = real_tile_store();
    let (layers, _) = patterned(&mut store, [0.5, 0.5, 0.5, 1.0], pat_overlay("q", 1.0, (0.0, 0.0)));
    let acc = composite(&layers, &mut store);
    for y in 0..10 {
        for x in 0..10 {
            let want = [PRED, PGREEN, PBLUE, PWHITE][(y % 2) * 2 + x % 2];
            near(texel(&acc, x, y), want);
        }
    }
}

#[test]
fn the_origin_shifts_the_tiling_and_is_pixel_exact() {
    let (_d, mut store) = real_tile_store();
    // Origin (1, 3): pixel (1, 3) is texel (0, 0), so pixel (0, 0) is texel (1, 1).
    let (layers, _) = patterned(&mut store, [0.5, 0.5, 0.5, 1.0], pat_overlay("q", 1.0, (1.0, 3.0)));
    let acc = composite(&layers, &mut store);
    near(texel(&acc, 1, 3), PRED);
    near(texel(&acc, 2, 3), PGREEN);
    near(texel(&acc, 1, 4), PBLUE);
    near(texel(&acc, 0, 0), PWHITE);
    near(texel(&acc, 0, 1), PGREEN);
}

#[test]
fn a_scaled_pattern_interpolates_between_texel_centres() {
    let (_d, mut store) = real_tile_store();
    // 200 %: texel centres every 2 pixels from the origin, so pixel x = 1 is
    // half way between texels 0 and 1 (red and green) on row 0.
    let (layers, _) = patterned(&mut store, [0.5, 0.5, 0.5, 1.0], pat_overlay("q", 2.0, (0.0, 0.0)));
    let acc = composite(&layers, &mut store);
    near(texel(&acc, 0, 0), PRED);
    near(texel(&acc, 2, 0), PGREEN);
    near(texel(&acc, 1, 0), [0.5, 0.5, 0.0, 1.0]);
    near(texel(&acc, 0, 1), [0.5, 0.0, 0.5, 1.0]);
    // The period is 4 pixels.
    near(texel(&acc, 4, 0), PRED);
}

#[test]
fn a_pattern_overlay_pixel_function_follows_the_layers_bounds() {
    let pattern = quad("q");
    let o = pat_overlay("q", 1.0, (0.0, 0.0));
    let run = |x: i64, y: i64| {
        let mut out = vec![half::f16::from_f32(0.0); aurora_tile::SAMPLES];
        let bounds = aurora_core::Rect { x, y, width: 10, height: 10 };
        crate::pattern_overlay_texels(&mut out, &pattern, &o, bounds, 0, (0, 0));
        out
    };
    // Layer at (5, 7): its top-left pixel is texel (0, 0). The tile is read
    // from document pixel (0, 0), so pixel (5, 7) is sample index 7 * 256 + 5.
    let moved = run(5, 7);
    let at = |v: &[half::f16], x: usize, y: usize| {
        let i = (y * 256 + x) * aurora_tile::CHANNELS;
        [0, 1, 2, 3].map(|c| v.get(i + c).map_or(f32::NAN, |h| h.to_f32()))
    };
    near(at(&moved, 5, 7), PRED);
    near(at(&moved, 6, 7), PGREEN);
    near(at(&moved, 5, 8), PBLUE);
    // The same overlay on a layer at the origin starts there instead.
    near(at(&run(0, 0), 0, 0), PRED);
    near(at(&run(0, 0), 5, 7), PWHITE);
}

#[test]
fn opacity_and_blend_mode_are_the_overlays_own() {
    let (_d, mut store) = real_tile_store();
    let mut o = pat_overlay("q", 1.0, (0.0, 0.0));
    o.opacity = 0.25;
    let (layers, _) = patterned(&mut store, [0.0, 0.0, 0.0, 1.0], o);
    near(texel(&composite(&layers, &mut store), 0, 0), [0.25, 0.0, 0.0, 1.0]);
    let mut o = pat_overlay("q", 1.0, (0.0, 0.0));
    o.blend_mode = EffectBlendMode::Multiply;
    let (layers, _) = patterned(&mut store, [0.5, 0.5, 0.5, 1.0], o);
    // Multiply: red x 0.5 grey.
    near(texel(&composite(&layers, &mut store), 0, 0), [0.5, 0.0, 0.0, 1.0]);
}

#[test]
fn fill_does_not_fade_the_pattern_and_the_shape_clips_it() {
    let (_d, mut store) = real_tile_store();
    let (mut layers, id) = patterned(&mut store, [0.0, 0.0, 0.0, 1.0], pat_overlay("q", 1.0, (0.0, 0.0)));
    ok(layers.set_fill_opacity(id, 0.0));
    let acc = composite(&layers, &mut store);
    // The content is gone (Fill 0) but the pattern shows in full.
    near(texel(&acc, 0, 0), PRED);
    near(texel(&acc, 1, 1), PWHITE);
    // Clipped by the layer's shape: over transparent content nothing is
    // drawn.
    let (clipped, _) = patterned(&mut store, [0.0; 4], pat_overlay("q", 1.0, (0.0, 0.0)));
    near(texel(&composite(&clipped, &mut store), 0, 0), [0.0; 4]);
}

#[test]
fn a_pattern_missing_from_the_pool_draws_nothing_and_a_shown_overlay_refuses_the_gpu() {
    let (_d, mut store) = real_tile_store();
    let (layers, id) = patterned(&mut store, PBLUE, pat_overlay("absent", 1.0, (0.0, 0.0)));
    near(texel(&composite(&layers, &mut store), 3, 3), PBLUE);
    assert!(crate::layer_renders_effects(&layers, id));
    // A disabled overlay does not refuse it.
    let mut o = pat_overlay("q", 1.0, (0.0, 0.0));
    o.gate.enabled = false;
    let (layers, id) = patterned(&mut store, PBLUE, o);
    assert!(!crate::layer_renders_effects(&layers, id));
    near(texel(&composite(&layers, &mut store), 3, 3), PBLUE);
}

const PATTERN_OVERLAY_FIDELITY: [(&str, f64); 4] = [
    // Photoshop's own merged images: mean error over the channels, in
    // levels (before this task: 13.28, 16.84, 34.18, 6.75).
    ("patterns.psd", 0.1),
    ("transparency/fill-opacity.psd", 0.1),
    ("fill_adjustments.psd", 13.5),
    ("layer_effects.psd", 5.9),
];

#[test]
fn pattern_overlays_match_photoshops_merged_images() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound) in PATTERN_OVERLAY_FIDELITY {
        let path = root.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: corpus file not present at {}", path.display());
            continue;
        };
        let Ok(merged) = aurora_io::decode_psd_merged_image(&bytes) else {
            unreachable!("{file}: no merged image");
        };
        let (_scratch, mut store) = real_tile_store();
        let (document, after) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let drawn = crate::layer_ids_in_order(&document.layers)
            .into_iter()
            .filter_map(|id| document.layers.effects(id))
            .map(aurora_core::LayerEffects::shown_pattern_overlays)
            .sum::<usize>();
        assert!(drawn > 0, "{file}: no pattern overlay was read");
        assert!(!document.layers.patterns().is_empty(), "{file}: no pattern pool");
        let Some((mean, _max)) = audit_diff(&after, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        let mean = mean.iter().sum::<f64>() / 3.0;
        println!("PATTERN-OVERLAY {file}: mean {mean:.3}");
        assert!(mean <= mean_bound, "{file}: mean {mean} over {mean_bound}");
    }
}

#[test]
fn the_pattern_pool_and_overlays_survive_a_save_and_reopen_of_an_imported_psd() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    let Ok(bytes) = std::fs::read(root.join("patterns.psd")) else {
        println!("SKIPPED: corpus file not present");
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, before) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    assert_eq!(document.layers.patterns().len(), 6);
    let mut saved = std::io::Cursor::new(Vec::new());
    ok(aurora_io::write_aur(
        &mut saved,
        &document.layers,
        &document.history,
        (256, 256),
        None,
        &aurora_io::SkippedTiles::new(),
        &mut store,
    ));
    let (_scratch2, mut fresh) = real_tile_store();
    let reopened = ok(aurora_io::read_aur(
        std::io::Cursor::new(saved.into_inner()),
        &mut fresh,
    ));
    assert_eq!(reopened.layers.patterns().len(), 6);
    let after = match composite_document(&reopened.layers, &mut fresh, 256, 256) {
        Ok(image) => image,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some((mean, max)) = audit_diff(&after, &before) else {
        unreachable!("sizes differ");
    };
    assert!(mean.iter().all(|m| *m == 0.0) && max == 0.0, "{mean:?} {max}");
}
