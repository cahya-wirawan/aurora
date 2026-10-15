// ---- 0.244.0: Invert, Threshold, Posterize and Brightness/Contrast against
// Photoshop's merged images ----
//
// Included into `fill_and_overlay_tests` (uses `real_tile_store`,
// `open_and_composite_psd`, `audit_diff`). Each fixture is composited twice:
// with the four new kinds hidden (what Aurora showed before 0.244.0, when
// these layers were left out) and as opened, and both are compared with the
// merged image Photoshop stored in the file. Prints SKIPPED when the corpus
// is absent. The pinned bounds are the measured values plus a margin.

/// `(file, bound on the mean error after, bound on the max error after)`.
const FOUR_ADJUST_FIDELITY: [(&str, f64, f64); 7] = [
    ("adjustments/invert_rgb.psd", 0.05, 2.0),
    ("adjustments/threshold_rgb.psd", 0.1, 255.0),
    ("adjustments/threshold_16bits_rgb.psd", 1.0, 255.0),
    ("adjustments/posterize_rgb.psd", 1.0, 140.0),
    ("adjustments/posterize_16bits_rgb.psd", 0.5, 140.0),
    ("adjustments/brightnesscontrast_legacy_rgb.psd", 0.2, 2.0),
    ("adjustments/brightnesscontrast_rgb.psd", 10.0, 60.0),
];

fn four_adjust_hide(layers: &mut aurora_doc::LayerTree) {
    for id in crate::layer_ids_in_order(layers) {
        let new_kind = layers.adjustment(id).is_some_and(|a| {
            matches!(
                a,
                aurora_doc::Adjustment::Invert
                    | aurora_doc::Adjustment::Threshold(_)
                    | aurora_doc::Adjustment::Posterize(_)
                    | aurora_doc::Adjustment::BrightnessContrast(_)
            )
        });
        if new_kind {
            let _ = layers.set_visible(id, false);
        }
    }
}

#[test]
fn the_four_adjustments_match_photoshops_merged_images_in_the_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound, max_bound) in FOUR_ADJUST_FIDELITY {
        let path = root.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: corpus file not present at {}", path.display());
            continue;
        };
        let merged = match aurora_io::decode_psd_merged_image(&bytes) {
            Ok(image) => image,
            Err(err) => unreachable!("{file}: {err:?}"),
        };
        let (_scratch, mut store) = real_tile_store();
        let (mut document, after) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let Some((mean_after, max_after)) = audit_diff(&after, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        four_adjust_hide(&mut document.layers);
        let (w, h) = document.canvas_size;
        let before = match composite_document(&document.layers, &mut store, w, h) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some((mean_before, max_before)) = audit_diff(&before, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        let mean_of = |m: [f64; 3]| m.iter().sum::<f64>() / 3.0;
        println!(
            "FOUR-ADJUST {file}: before (layers left out) mean {:.3} max {max_before:.0} | \
             after mean {:.3} (r/g/b {:.3} {:.3} {:.3}) max {max_after:.0}",
            mean_of(mean_before),
            mean_of(mean_after),
            mean_after[0],
            mean_after[1],
            mean_after[2],
        );
        assert!(
            mean_of(mean_after) <= mean_bound,
            "{file}: mean {mean_after:?} over {mean_bound}"
        );
        assert!(max_after <= max_bound, "{file}: max {max_after} over {max_bound}");
        assert!(
            mean_of(mean_after) < mean_of(mean_before),
            "{file}: showing the layers must be closer than leaving them out"
        );
    }
}
