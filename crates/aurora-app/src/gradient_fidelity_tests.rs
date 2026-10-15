// ---- 0.248.0: Gradient Map and Color Lookup against Photoshop's merged
// images ----
//
// Included into `fill_and_overlay_tests` (uses `real_tile_store`,
// `open_and_composite_psd`, `audit_diff`). **No fixture holds a picture a
// Gradient Map changed**: the four `layers/gradient-map*.psd` files hold the
// adjustment over no pixels (their merged image is empty) and the Gradient Map
// in `fill_adjustments.psd` is hidden, so showing the layer cannot move a
// pixel and this test pins exactly that (and the residual error the other,
// still-unmatched layers of `fill_adjustments.psd` leave). It does NOT
// validate the formula. Each file is composited twice, with the Gradient Map
// layers hidden and as opened, and both are compared with the merged image
// Photoshop stored. Prints SKIPPED when the corpus is absent.

/// `(file, bound on the mean error after, bound on the max error after)`.
const GRADIENT_FIDELITY: [(&str, f64, f64); 6] = [
    ("layers/gradient-map.psd", 0.0, 0.0),
    ("layers/gradient-map-v3-classic.psd", 0.0, 0.0),
    ("layers/gradient-map-v3-linear.psd", 0.0, 0.0),
    ("layers/gradient-map-v3-perceptual.psd", 0.0, 0.0),
    ("layers/color-lookup.psd", 0.0, 0.0),
    ("fill_adjustments.psd", 60.0, 255.0),
];

fn gradient_hide(layers: &mut aurora_doc::LayerTree) {
    for id in crate::layer_ids_in_order(layers) {
        if layers
            .adjustment(id)
            .is_some_and(|a| matches!(a, aurora_doc::Adjustment::GradientMap(_)))
        {
            let _ = layers.set_visible(id, false);
        }
    }
}

#[test]
fn gradient_map_files_match_photoshops_merged_images_as_far_as_they_can() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound, max_bound) in GRADIENT_FIDELITY {
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
        let shown = crate::layer_ids_in_order(&document.layers)
            .into_iter()
            .filter(|&id| {
                document
                    .layers
                    .adjustment(id)
                    .is_some_and(|a| matches!(a, aurora_doc::Adjustment::GradientMap(_)))
            })
            .count();
        let expected = match file {
            "layers/gradient-map.psd"
            | "layers/gradient-map-v3-classic.psd"
            | "fill_adjustments.psd" => 1,
            _ => 0,
        };
        assert_eq!(shown, expected, "{file}: Gradient Map layers opened");
        let Some((mean_after, max_after)) = audit_diff(&after, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        gradient_hide(&mut document.layers);
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
            "GRADIENT-MAP {file}: before (Gradient Map hidden) mean {:.3} max {max_before:.0} | \
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
        assert_eq!(
            (mean_after, max_after.to_bits()),
            (mean_before, max_before.to_bits()),
            "{file}: the only Gradient Map layers in the corpus change no pixel"
        );
    }
}

/// Diagnostic (run with `--ignored --nocapture`): the error of
/// `fill_adjustments.psd` with each adjustment layer hidden in turn, to see
/// which layer the 54-level mean error comes from.
#[test]
#[ignore = "diagnostic: prints the error with each layer hidden in turn"]
fn fill_adjustments_error_by_hidden_layer() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/fill_adjustments.psd");
    let Ok(bytes) = std::fs::read(&path) else {
        println!("SKIPPED: corpus file not present");
        return;
    };
    let merged = match aurora_io::decode_psd_merged_image(&bytes) {
        Ok(image) => image,
        Err(err) => unreachable!("{err:?}"),
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, _) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let (w, h) = document.canvas_size;
    let mut layers = document.layers;
    for id in crate::layer_ids_in_order(&layers) {
        let Some(label) = layers.adjustment(id).map(|a| a.label()) else {
            continue;
        };
        let was = layers.visible(id);
        let _ = layers.set_visible(id, false);
        let image = match composite_document(&layers, &mut store, w, h) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Some((mean, max)) = audit_diff(&image, &merged) {
            println!(
                "HIDDEN {label} (was visible: {was:?}): mean {:.3} max {max:.0}",
                mean.iter().sum::<f64>() / 3.0
            );
        }
        let _ = layers.set_visible(id, was.unwrap_or(true));
    }
}
