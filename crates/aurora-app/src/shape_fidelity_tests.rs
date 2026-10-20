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
const SHAPE_FIDELITY: [(&str, f64, f64); 4] = [
    ("vector-mask2.psd", 5.0, 70.0),
    ("colormodes/4x4_16bit_rgb.psd", 28.0, 135.0),
    ("colormodes/4x4_16bit_grayscale.psd", 18.0, 45.0),
    ("artboard-bgcolor.psd", 0.05, 1.0),
];

#[test]
fn drawn_fills_and_artboard_backgrounds_match_photoshops_merged_images() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound, max_bound) in SHAPE_FIDELITY {
        let path = root.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: corpus file not present at {}", path.display());
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
