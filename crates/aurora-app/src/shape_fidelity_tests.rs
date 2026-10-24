// ---- 0.251.0: shape / fill layer fidelity (task 108) ----
//
// Included into `fill_and_overlay_tests` (uses `real_tile_store`,
// `open_and_composite_psd`, `audit_diff`, `composite_document`). Prints
// SKIPPED when the corpus is absent.

/// Diagnostic (run with `--ignored --nocapture`): for each file named in
/// `AURORA_DIAG_FILES` (`:`-separated, relative to the psd-tools fixtures),
/// the error against Photoshop's merged image as opened and with each layer
/// hidden in turn, so a layer's share of the error is its drop when hidden.
#[test]
#[ignore = "diagnostic: prints the error with each layer hidden in turn"]
fn error_by_hidden_layer_for_named_files() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    let Ok(list) = std::env::var("AURORA_DIAG_FILES") else {
        println!("SKIPPED: AURORA_DIAG_FILES not set");
        return;
    };
    for file in list.split(':').filter(|f| !f.is_empty()) {
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            println!("SKIPPED: {file} not present");
            continue;
        };
        let Ok(merged) = aurora_io::decode_psd_merged_image(&bytes) else {
            println!("SKIPPED: {file} has no merged image");
            continue;
        };
        let (_scratch, mut store) = real_tile_store();
        let (document, base) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let mean_of = |m: [f64; 3]| m.iter().sum::<f64>() / 3.0;
        if let Some((mean, max)) = audit_diff(&base, &merged) {
            println!("DIAG {file}: as opened mean {:.3} max {max:.0}", mean_of(mean));
        }
        let (w, h) = document.canvas_size;
        let mut layers = document.layers;
        for id in crate::layer_ids_in_order(&layers) {
            let name = layers.name(id).unwrap_or("?").to_string();
            let was = layers.visible(id);
            let _ = layers.set_visible(id, false);
            if let Ok(image) = composite_document(&layers, &mut store, w, h)
                && let Some((mean, max)) = audit_diff(&image, &merged)
            {
                println!("DIAG   hide {name:?}: mean {:.3} max {max:.0}", mean_of(mean));
            }
            let _ = layers.set_visible(id, was.unwrap_or(true));
        }
    }
}

/// `(file, mean bound after, max bound after)`: task 108's two fixes
/// against Photoshop's merged images. "Before" (mean levels, 0.250.0):
/// `vector-mask2.psd` 71.0, `4x4_16bit_rgb.psd` 174.6, `4x4_16bit_grayscale.psd`
/// 185.0, `artboard-bgcolor.psd` 66.8. What remains: the gradient's length
/// (Photoshop's is about 16 % shorter than the conventional one on the one
/// linear picture), the 4x4 reflected picture whose symmetry axis does not
/// follow the stored angle, a pattern fill and a pattern/gradient overlay.
/// Task 149 adds `advanced-blending.psd`: 58.26 before, 0.01 after (`infx`
/// and `clbl`); its one-texel maximum is a rounded edge.
const SHAPE_FIDELITY: [(&str, f64, f64); 11] = [
    ("vector-mask2.psd", 5.0, 70.0),
    ("colormodes/4x4_16bit_rgb.psd", 28.0, 135.0),
    ("colormodes/4x4_16bit_grayscale.psd", 18.0, 45.0),
    ("artboard-bgcolor.psd", 0.05, 1.0),
    ("advanced-blending.psd", 0.05, 150.0),
    // 20.60 before the stored pixels of a shape layer replaced its
    // rasterised path (task 149).
    ("effects/double-stroke-effects.psd", 0.05, 3.0),
    // 28.84 before the strokes of a shape layer followed its vector outline
    // instead of its pixels' alpha (task 151); 4.5 after. The rest is the
    // 'Raster *' twins and the gradient and pattern strokes, not
    // investigated.
    ("effects/stroke-effects.psd", 5.0, 255.0),
    // 18.50 before a clipped group was drawn clipped (task 149).
    ("group-clipping/group-clipping.psd", 0.05, 3.0),
    // 18.88 and 19.13 before task 150: the masks' feathers (Gaussian,
    // sigma = feather) and a shape layer whose vector-mask density lets its
    // fill show across the whole canvas. The .psb keeps a larger error
    // (1.2) at one soft edge, not investigated.
    ("layer_mask_data.psd", 0.5, 15.0),
    ("layer_mask_data.psb", 2.0, 100.0),
    // 21.37 before a base's overlays were drawn over its clipped layers
    // (task 150); the rest is not investigated (its Brightness/Contrast
    // layer is the approximate default mode).
    ("clipping-mask2.psd", 2.5, 90.0),
];

#[test]
fn drawn_fills_and_artboard_backgrounds_match_photoshops_merged_images() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound, max_bound) in SHAPE_FIDELITY {
        let path = root.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            crate::open_time_tests::corpus_file_absent(&path);
            continue;
        };
        let Ok(merged) = aurora_io::decode_psd_merged_image(&bytes) else {
            unreachable!("{file}: no merged image");
        };
        let (_scratch, mut store) = real_tile_store();
        let (_document, after) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let Some((mean, max)) = audit_diff(&after, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        let mean = mean.iter().sum::<f64>() / 3.0;
        println!("SHAPE-FIDELITY {file}: mean {mean:.3} max {max:.0}");
        assert!(mean <= mean_bound, "{file}: mean {mean} over {mean_bound}");
        assert!(max <= max_bound, "{file}: max {max} over {max_bound}");
    }
}

/// Task 151: the shape layers of `stroke-effects.psd` keep their outline
/// (their strokes follow it) and the raster twins and plain layers do not.
#[test]
fn stroke_effects_shape_layers_keep_their_outline() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/effects/stroke-effects.psd");
    let Ok(bytes) = std::fs::read(&path) else {
        crate::open_time_tests::corpus_file_absent(&path);
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, _image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let mut with = Vec::new();
    let mut without = Vec::new();
    for id in crate::layer_ids_in_order(&document.layers) {
        if !matches!(
            document.layers.kind(id),
            Some(aurora_doc::LayerKind::Pixel { .. })
        ) {
            continue;
        }
        let name = document.layers.name(id).unwrap_or("?").to_string();
        if document.layers.effect_shape(id).is_some() {
            with.push(name);
        } else {
            without.push(name);
        }
    }
    println!("EFFECT-SHAPES with {with:?} without {without:?}");
    // Every raster twin has no outline; the 'Shape *' layers do.
    assert!(without.iter().any(|n| n.starts_with("Raster")), "{without:?}");
    assert!(with.iter().all(|n| !n.starts_with("Raster")), "{with:?}");
    for name in ["Shape Rectangle", "Shape Ellipse", "Shape OutsetFrame"] {
        assert!(with.iter().any(|n| n == name), "{name} in {with:?}");
    }
}
