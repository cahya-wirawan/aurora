//! 0.241.0: Vibrance, Exposure, Photo Filter, Black & White and Color
//! Balance layers in PSD files. A child of `lib.rs`'s `tests` module: the
//! corpus comparison against Photoshop's own merged images (with the
//! five kinds imported, and with them hidden, which is what "left out"
//! composites as), and the save-then-reopen composite equality per kind.

use super::{
    TinyLayer, composite_difference, composite_document, open_and_composite_psd, real_tile_store,
    tiny_psd,
};

const FIVE: [&str; 5] = [
    "Vibrance",
    "Exposure",
    "Photo Filter",
    "Black & White",
    "Color Balance",
];

fn is_five(a: &aurora_doc::Adjustment) -> bool {
    FIVE.contains(&a.label())
}

/// `(count of imported five-kind layers, composite error vs Photoshop's
/// merged image with them, the same with them hidden)`, each error
/// `(per-channel max, per-channel mean)` in 8-bit levels.
type Errors = ([f64; 3], [f64; 3]);

fn measure(relative: &str) -> Option<(usize, Errors, Errors, Vec<String>)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures")
        .join(relative);
    let Ok(bytes) = std::fs::read(&path) else {
        println!("SKIPPED: corpus file not present at {}", path.display());
        return None;
    };
    let merged = match aurora_io::decode_psd_merged_image(&bytes) {
        Ok(image) => image,
        Err(err) => unreachable!("{relative}: {err:?}"),
    };
    let (_scratch, mut store) = real_tile_store();
    let (mut document, image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let with = composite_difference(&image, &merged);
    let mut ids = Vec::new();
    let mut pending = document.layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        pending.extend(document.layers.children(id).unwrap_or(&[]));
        if document.layers.adjustment(id).is_some_and(is_five) {
            ids.push(id);
        }
    }
    for &id in &ids {
        if document.layers.set_visible(id, false).is_err() {
            unreachable!("{relative}: hide");
        }
    }
    let (w, h) = document.canvas_size;
    let hidden = match composite_document(&document.layers, &mut store, w, h) {
        Ok(image) => image,
        Err(err) => unreachable!("{err:?}"),
    };
    let without = composite_difference(&hidden, &merged);
    Some((ids.len(), with, without, document.report.items))
}

/// The files that carry the five kinds: how many layers of those kinds
/// each holds, and the error of Aurora's composite against Photoshop's
/// merged image with them imported, as measured on 0.241.0 (8-bit levels
/// over every pixel; the CPU compositor is deterministic): per-channel
/// maximum and mean. They are regression guards, not claims of a match;
/// what differs, and why, is in the comment on each row. The "left out"
/// error (the same file with the five kinds hidden) is printed next to
/// each by the test.
type Row = (&'static str, usize, [f64; 3], [f64; 3]);
const FILES: [Row; 14] = [
    // A single neutral layer over white: nothing to differ.
    ("layers/vibrance.psd", 1, [0.0; 3], [0.0; 3]),
    ("layers/exposure.psd", 1, [0.0; 3], [0.0; 3]),
    ("layers/photo-filter.psd", 1, [0.0; 3], [0.0; 3]),
    ("layers/black-and-white.psd", 1, [0.0; 3], [0.0; 3]),
    ("layers/color-balance.psd", 1, [0.0; 3], [0.0; 3]),
    // Four masked Exposure layers over a photograph. Left out: mean 59-63.
    // Aurora's linear-light formula is within a mean of 1.4-1.6 levels
    // (max 21) of Photoshop's.
    (
        "adjustments/exposure_rgb.psd",
        4,
        [21.0; 3],
        [1.434, 1.554, 1.392],
    ),
    // Five kinds among 13 others (8 still left out, and shape layers drawn
    // as pixels): not a test of the five. Left out: mean 125.9 / 51.5 / 32.0.
    (
        "fill_adjustments.psd",
        5,
        [204.0, 175.0, 178.0],
        [106.63, 30.50, 26.04],
    ),
    // The one Color Balance sits in a hidden group: no effect, the error is
    // the file's other unsupported content (identical with it left out).
    (
        "clipping-mask2.psd",
        1,
        [86.0, 86.0, 78.0],
        [23.99, 25.33, 24.69],
    ),
    // Exposure -3.53 in a Pass Through group whose Fill is 40 %: Aurora
    // applies it at full Fill (reported), Photoshop shows almost none of
    // it. Worse than left out (mean 6.8 / 15.1 / 15.1): a finding about the
    // group Fill, not about Exposure.
    (
        "passthrough_fill_adjustment.psd",
        1,
        [172.0; 3],
        [165.25, 156.45, 156.45],
    ),
    // Left out: mean 86 / 70 / 70.
    (
        "passthrough_opacity.psd",
        1,
        [0.0, 4.0, 4.0],
        [0.0, 1.19, 1.19],
    ),
    // Left out: mean 31.8 / 17.9 / 15.1.
    (
        "passthrough_vector_mask.psd",
        1,
        [8.0, 7.0, 7.0],
        [4.02, 4.23, 4.15],
    ),
    // Two Exposure -3.53 layers in masked groups, a clipped layer shown
    // unclipped (reported): slightly worse than left out (mean 49.7 /
    // 34.6 / 34.6), same cause as the Fill file.
    (
        "passthrough_clipping_mask_adjustment.psd",
        2,
        [171.0, 89.0, 89.0],
        [54.83, 39.12, 39.12],
    ),
    // Left out: mean 14.1 / 13.1 / 0.1, max 137.
    (
        "adjustments/adjustment_nested_composition_1.psd",
        1,
        [23.0, 19.0, 19.0],
        [1.22, 0.51, 0.89],
    ),
    // Left out: mean 54.1 / 58 / 58, max 113.
    (
        "adjustments/adjustment_nested_composition_4.psd",
        1,
        [18.0, 2.0, 2.0],
        [1.94, 2.01, 2.01],
    ),
];

#[test]
fn the_corpus_files_with_the_five_kinds_import_them_and_report_the_error_against_photoshop() {
    let mut ran = 0;
    for (file, expected, max_bound, mean_bound) in FILES {
        let Some((count, with, without, report)) = measure(file) else {
            continue;
        };
        ran += 1;
        println!(
            "{file}: {count} layers of the five kinds\n  imported:  max {:?} mean {:.3?}\n  left out:  max {:?} mean {:.3?}\n  report {report:?}",
            with.0, with.1, without.0, without.1
        );
        assert_eq!(count, expected, "{file}: every layer is imported");
        for c in 0..3 {
            let (Some(max), Some(mean), Some(max_want), Some(mean_want)) = (
                with.0.get(c),
                with.1.get(c),
                max_bound.get(c),
                mean_bound.get(c),
            ) else {
                unreachable!("three channels");
            };
            assert!(max <= max_want, "{file} channel {c}: {with:?}");
            assert!(mean <= mean_want, "{file} channel {c}: {with:?}");
        }
        // "5 Exposure layers were left out" style lines are gone.
        for line in &report {
            for kind in FIVE {
                assert!(!line.contains(&format!("{kind} layer")), "{file}: {line}");
            }
        }
    }
    if ran == 0 {
        println!("SKIPPED: corpus absent");
    }
}

// ---------------------------------------------------------------------
// Save, reopen, composite
// ---------------------------------------------------------------------

fn gradient_layer() -> TinyLayer {
    let rect = (0, 0, 8, 8);
    let plane =
        |f: &dyn Fn(u8, u8) -> u8| -> Vec<u8> { (0..64).map(|i| f(i % 8, i / 8)).collect() };
    TinyLayer {
        rect,
        channels: vec![
            (-1, vec![255; 64]),
            (0, plane(&|x, _| 20 + x * 30)),
            (1, plane(&|_, y| 235 - y * 28)),
            (2, plane(&|x, y| 40 + (x + y) * 12)),
        ],
        mask: None,
        blocks: Vec::new(),
    }
}

fn ok<T, E>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(_) => unreachable!("valid params"),
    }
}

/// One adjustment per kind, each moving the gradient visibly.
fn adjustments() -> Vec<aurora_doc::Adjustment> {
    use aurora_core::{
        BlackWhiteParams, ColorBalanceParams, ExposureParams, PhotoFilterParams, VibranceParams,
    };
    vec![
        aurora_doc::Adjustment::Vibrance(ok(VibranceParams::new(60.0, -25.0))),
        aurora_doc::Adjustment::Exposure(ok(ExposureParams::new(0.8, 0.02, 1.3))),
        aurora_doc::Adjustment::PhotoFilter(ok(PhotoFilterParams::new(
            [0.0, 109.0 / 255.0, 1.0],
            60.0,
            true,
        ))),
        aurora_doc::Adjustment::BlackWhite(ok(BlackWhiteParams::new(
            [100.0, 20.0, -50.0, 60.0, 120.0, 0.0],
            true,
            210.0,
            35.0,
        ))),
        aurora_doc::Adjustment::ColorBalance(ok(ColorBalanceParams::new(
            [[30.0, -20.0, 10.0], [-40.0, 25.0, 60.0], [15.0, 0.0, -35.0]],
            false,
        ))),
    ]
}

/// For each kind: a document with the adjustment over a gradient, saved to
/// PSD through the verified writer, reopened by the reader and composited,
/// equals the composite before the save to within the 16-bit file's own
/// quantisation (and the adjustment is not the identity: the layer
/// visibly does something).
#[test]
fn each_kind_saved_to_psd_and_reopened_composites_the_same() {
    for adjustment in adjustments() {
        let label = adjustment.label();
        let bytes = tiny_psd(8, 8, 3, &[gradient_layer()]);
        let (_scratch, mut store) = real_tile_store();
        let (mut document, plain) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let id =
            match document
                .layers
                .add_adjustment_layer_at("Adjust", adjustment.clone(), None, 0)
            {
                Ok(id) => id,
                Err(err) => unreachable!("{label}: {err:?}"),
            };
        let (w, h) = document.canvas_size;
        let before = match composite_document(&document.layers, &mut store, w, h) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let (moved, _) = composite_difference(&before, &plain);
        assert!(
            moved.iter().any(|&m| m >= 8.0),
            "{label} must visibly change the gradient: {moved:?}"
        );
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let target = dir.path().join("kind.psd");
        let outcome = aurora_io::replace_verified(
            &target,
            |file| {
                let mut source: &aurora_io::Image = &before;
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
            unreachable!("{label}: save failed")
        };
        // The tint colour and Lab colour come back within a fraction of a
        // level, which the report does not itemise.
        assert!(
            outcome.report.items.is_empty(),
            "{label}: {:?}",
            outcome.report.items
        );
        let Ok(written) = std::fs::read(&target) else {
            unreachable!("{label}: written file")
        };
        let (_scratch2, mut store2) = real_tile_store();
        let (reopened, after) = open_and_composite_psd(
            &written,
            &mut store2,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        assert!(
            reopened.report.is_empty(),
            "{label}: {:?}",
            reopened.report.items
        );
        let (max, mean) = composite_difference(&after, &before);
        println!("{label}: reopened vs before save: max {max:?} mean {mean:.4?}");
        assert!(max.iter().all(|&m| m <= 1.0), "{label}: {max:?}");
        assert!(mean.iter().all(|&m| m <= 0.1), "{label}: {mean:?}");
        let found = reopened
            .layers
            .roots()
            .iter()
            .find_map(|&r| reopened.layers.adjustment(r).cloned());
        assert_eq!(
            found.as_ref().map(aurora_doc::Adjustment::label),
            Some(label)
        );
        let _ = id;
    }
}

// ---------------------------------------------------------------------
// The corpus files, saved and reopened
// ---------------------------------------------------------------------

/// Whether two adjustments are the same up to what the file cannot hold
/// exactly: a Photo Filter colour (Lab hundredths) and a Black & White
/// tint colour (stored as an RGB colour) come back within a fraction of a
/// level; everything else must be equal.
#[allow(clippy::float_cmp)] // integers held in floats compare exactly
fn close(a: &aurora_doc::Adjustment, b: &aurora_doc::Adjustment) -> bool {
    use aurora_doc::Adjustment as A;
    match (a, b) {
        (A::PhotoFilter(x), A::PhotoFilter(y)) => {
            x.colour()
                .iter()
                .zip(y.colour())
                .all(|(p, q)| (p - q).abs() < 1.0 / 765.0)
                && x.density() == y.density()
                && x.preserve_luminosity() == y.preserve_luminosity()
        }
        (A::BlackWhite(x), A::BlackWhite(y)) => {
            x.weights() == y.weights()
                && x.tint() == y.tint()
                && (x.tint_hue() - y.tint_hue()).abs() < 0.01
                && (x.tint_saturation() - y.tint_saturation()).abs() < 0.01
        }
        _ => a == b,
    }
}

/// The corpus files with the five kinds, opened, saved as PSD (written and
/// verified by the writer's two passes), reopened: every adjustment
/// survives (exact, or within the tolerance of [`close`]) and the
/// composite changes by at most the 16-bit file's own one level. Prints
/// the numbers; with `AURORA_PSD_FIXTURE_DIR` set the saved files are kept
/// as `resave_<name>` for `scripts/verify_psd_write.py` (psd-tools).
#[test]
fn corpus_files_with_the_five_kinds_survive_a_psd_save_and_reopen() {
    let keep = std::env::var_os("AURORA_PSD_FIXTURE_DIR").map(std::path::PathBuf::from);
    let mut ran = 0;
    for (relative, _, _, _) in FILES {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpora/psd/reference/psd-tools-fixtures")
            .join(relative);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: corpus file not present at {}", path.display());
            continue;
        };
        ran += 1;
        let (scratch, mut store) = real_tile_store();
        let (doc1, img1) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let name = relative.rsplit('/').next().unwrap_or(relative);
        let target = keep.as_ref().map_or_else(
            || scratch.path().join(format!("resave_{name}")),
            |dir| dir.join(format!("resave_{name}")),
        );
        let outcome = aurora_io::replace_verified(
            &target,
            |file| {
                let mut source: &aurora_io::Image = &img1;
                aurora_io::write_psd(
                    &mut *file,
                    &doc1.layers,
                    doc1.canvas_size,
                    &mut source,
                    &mut store,
                    aurora_io::PsdSaveOptions::default(),
                )
            },
            |written, outcome| aurora_io::verify_psd_file(written, &outcome.expected),
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{relative}: {err:?}"),
        };
        let Ok(written) = std::fs::read(&target) else {
            unreachable!("{relative}: written file")
        };
        let (_scratch2, mut store2) = real_tile_store();
        let (doc2, img2) = open_and_composite_psd(
            &written,
            &mut store2,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let list = |doc: &aurora_io::PsdDocument| {
            let mut found = Vec::new();
            let mut pending = doc.layers.roots().to_vec();
            while let Some(id) = pending.pop() {
                pending.extend(doc.layers.children(id).unwrap_or(&[]));
                if let Some(a) = doc.layers.adjustment(id).filter(|a| is_five(a)) {
                    found.push((
                        doc.layers.name(id).unwrap_or_default().to_owned(),
                        a.clone(),
                    ));
                }
            }
            found.sort_by(|a, b| a.0.cmp(&b.0));
            found
        };
        let (before, after) = (list(&doc1), list(&doc2));
        let (max, mean) = composite_difference(&img2, &img1);
        println!(
            "{relative}: {} layers of the five kinds; reopened vs before save: max {max:?} mean {mean:.4?}; save report {:?}",
            before.len(),
            outcome.report.items
        );
        assert!(!before.is_empty(), "{relative}");
        assert_eq!(before.len(), after.len(), "{relative}");
        for ((n1, a1), (n2, a2)) in before.iter().zip(&after) {
            assert_eq!(n1, n2, "{relative}");
            assert!(close(a1, a2), "{relative}: {n1}: {a1:?} vs {a2:?}");
        }
        assert!(max.iter().all(|&m| m <= 1.0), "{relative}: {max:?}");
        assert!(mean.iter().all(|&m| m <= 0.1), "{relative}: {mean:?}");
        // None of the five kinds appears in the save report: Photoshop's
        // own values need no rounding (the report may name other things).
        for line in &outcome.report.items {
            for kind in FIVE {
                assert!(
                    !line.contains(&format!("{kind} layer")),
                    "{relative}: {line}"
                );
            }
        }
    }
    if ran == 0 {
        println!("SKIPPED: corpus absent");
    }
}
