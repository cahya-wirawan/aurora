//! Task 112: Exposure through a Grayscale file's grey profile. A child of
//! `lib.rs`'s `tests` module: the corpus composite against Photoshop's own
//! merged image, with the grey-profile model and (as a control) with the
//! plain sRGB Exposure, and the save-then-reopen behaviour (a PSD carries a
//! plain Exposure, itemised).

use super::{composite_difference, open_and_composite_psd, real_tile_store};

fn fixture(relative: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(relative);
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            println!("SKIPPED: corpus file not present at {}", path.display());
            None
        }
    }
}

/// Opens `bytes`, optionally replaces every grey-profile Exposure by the
/// plain one with the same sliders, and returns the composite error
/// `(max, mean)` per channel against Photoshop's merged image.
fn error_against_merged(bytes: &[u8], plain: bool) -> ([f64; 3], [f64; 3]) {
    let merged = match aurora_io::decode_psd_merged_image(bytes) {
        Ok(image) => image,
        Err(err) => unreachable!("{err:?}"),
    };
    let (_scratch, mut store) = real_tile_store();
    let (mut document, image) = open_and_composite_psd(
        bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    if !plain {
        return composite_difference(&image, &merged);
    }
    let mut pending = document.layers.roots().to_vec();
    let mut ids = Vec::new();
    while let Some(id) = pending.pop() {
        pending.extend(document.layers.children(id).unwrap_or(&[]));
        if let Some(p) = document
            .layers
            .adjustment(id)
            .and_then(aurora_doc::Adjustment::exposure_trc)
        {
            ids.push((id, p.exposure()));
        }
    }
    for (id, sliders) in ids {
        if document
            .layers
            .set_adjustment(id, aurora_doc::Adjustment::Exposure(sliders))
            .is_err()
        {
            unreachable!("swap");
        }
    }
    let (w, h) = document.canvas_size;
    let swapped = match super::composite_document(&document.layers, &mut store, w, h) {
        Ok(image) => image,
        Err(err) => unreachable!("{err:?}"),
    };
    composite_difference(&swapped, &merged)
}

/// Measured on the task's own build: the grey-profile model is within the
/// 8-bit rounding noise of Photoshop's image (mean 0.031, max 2), where the
/// plain sRGB Exposure is 11.4 levels off.
#[test]
fn the_grayscale_fixture_matches_photoshop_through_its_grey_profile() {
    let Some(bytes) = fixture("adjustments/exposure_grayscale.psd") else {
        return;
    };
    let (max, mean) = error_against_merged(&bytes, false);
    let (plain_max, plain_mean) = error_against_merged(&bytes, true);
    println!("grey profile: max {max:?} mean {mean:.3?}");
    println!("plain sRGB:   max {plain_max:?} mean {plain_mean:.3?}");
    for c in 0..3 {
        // Measured on this build: mean 0.031, max 2 (the plain sRGB model:
        // mean 11.43, max 26).
        assert!(
            mean[c] < 0.1 && max[c] <= 3.0,
            "channel {c}: {max:?} {mean:?}"
        );
        assert!(
            plain_mean[c] > 8.0,
            "the control must be clearly worse: {plain_mean:?}"
        );
    }
}

#[test]
fn the_rgb_control_fixture_is_unchanged_by_the_grey_profile_work() {
    let Some(bytes) = fixture("adjustments/exposure_rgb.psd") else {
        return;
    };
    let (max, mean) = error_against_merged(&bytes, false);
    // The 0.241.0 figures (psd_five_adjust_tests): max 21, mean 1.4-1.6.
    for c in 0..3 {
        assert!(max[c] <= 21.0 && mean[c] <= 1.56, "{max:?} {mean:?}");
    }
}

/// A PSD saved from a grey-profile Exposure carries a plain Exposure with
/// the same sliders, and the save report says so.
#[test]
fn a_saved_psd_carries_a_plain_exposure_and_the_report_says_so() {
    let Some(bytes) = fixture("adjustments/exposure_grayscale.psd") else {
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    };
    let target = dir.path().join("grey.psd");
    let outcome = aurora_io::replace_verified(
        &target,
        |file| {
            let mut source: &aurora_io::Image = &image;
            aurora_io::write_psd(
                &mut *file,
                &document.layers,
                document.canvas_size,
                &mut source,
                &mut store,
                aurora_io::PsdSaveOptions::default(),
            )
        },
        |written, outcome| aurora_io::verify_psd_file(written, &outcome.expected),
    );
    let Ok(outcome) = outcome else {
        unreachable!("save failed")
    };
    let text = outcome.report.items.join("\n");
    assert!(
        text.contains("4 Exposure layers use the grey profile of the Grayscale file it came from")
            && text.contains("saved as a plain Exposure"),
        "{text}"
    );
    let Ok(written) = std::fs::read(&target) else {
        unreachable!("written")
    };
    let (_scratch2, mut store2) = real_tile_store();
    let (reopened, _) = open_and_composite_psd(
        &written,
        &mut store2,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let mut pending = reopened.layers.roots().to_vec();
    let (mut plain, mut trc) = (0, 0);
    while let Some(id) = pending.pop() {
        pending.extend(reopened.layers.children(id).unwrap_or(&[]));
        match reopened.layers.adjustment(id) {
            Some(aurora_doc::Adjustment::Exposure(_)) => plain += 1,
            Some(aurora_doc::Adjustment::ExposureTrc(_)) => trc += 1,
            _ => {}
        }
    }
    assert_eq!((plain, trc), (4, 0));
}

/// The CPU-only rule: a grey-profile Exposure never qualifies for the GPU
/// compositing path, and its CPU result is what `AdjustmentLut` computes.
#[test]
fn a_grey_profile_exposure_is_cpu_only_and_matches_its_op() {
    let Some(bytes) = fixture("adjustments/exposure_grayscale.psd") else {
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, _) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let mut pending = document.layers.roots().to_vec();
    let mut seen = 0;
    while let Some(id) = pending.pop() {
        pending.extend(document.layers.children(id).unwrap_or(&[]));
        if let Some(adj) = document.layers.adjustment(id) {
            seen += 1;
            assert!(matches!(adj, aurora_doc::Adjustment::ExposureTrc(_)));
            assert!(
                !super::super::adjustment_layer_qualifies_for_gpu(&document.layers, id),
                "no GPU port"
            );
            let lut = super::super::AdjustmentLut::of(adj);
            let Some(p) = adj.exposure_trc() else {
                unreachable!("grey-profile layer")
            };
            let op = aurora_filters::ExposureTrcOp::new(p);
            for v in [0.0_f32, 0.2, 0.5, 0.8, 1.0] {
                assert_eq!(lut.apply([v; 3]), op.apply([v; 3]));
            }
        }
    }
    assert_eq!(seen, 4);
}
