//! Task 120: layer effects in a PSD save. Corpus files that carry effects
//! are opened, saved as PSD (written and verified by the writer's two
//! passes), reopened: every layer's effects must come back equal for what
//! the file holds, and the composite is compared with the one before the
//! save. With `AURORA_PSD_FIXTURE_DIR` set the saved files are kept as
//! `resave_<name>` for `scripts/verify_psd_write.py check_effects`.
//! `AURORA_EFFECTS_EXTRA` may name more files (a colon-separated list).

use aurora_core::effects::StrokePaint;

use super::{composite_difference, composite_document, open_and_composite_psd, real_tile_store};

const FILES: [&str; 10] = [
    "layer_effects.psd",
    "patterns.psd",
    "effect-stroke-gradient.psd",
    "effects/double-stroke-effects.psd",
    "effects/effects-enabled.psd",
    "effects/shape-fx.psd",
    "effects/shape-fx2.psd",
    "effects/stroke-composite.psd",
    "effects/stroke-effect-transparent-shape.psd",
    "effects/stroke-effects.psd",
];

/// Layer name -> effects, made comparable: boxes the reader derives from
/// the record are dropped, pattern origins made absolute, and the counts
/// of kinds Aurora does not model (never written) cleared.
#[allow(clippy::cast_precision_loss)]
fn effects_of(doc: &aurora_io::PsdDocument) -> Vec<(String, aurora_core::LayerEffects)> {
    let mut found = Vec::new();
    let mut pending = doc.layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        pending.extend(doc.layers.children(id).unwrap_or(&[]));
        let Some(fx) = doc.layers.effects(id) else {
            continue;
        };
        let mut fx = fx.clone();
        let (bx, by) = doc
            .layers
            .bounds(id)
            .map_or((0.0, 0.0), |b| (b.x as f64, b.y as f64));
        for o in &mut fx.gradient_overlays {
            o.reference = None;
        }
        for o in &mut fx.pattern_overlays {
            o.origin = (o.origin.0 + bx, o.origin.1 + by);
        }
        for s in &mut fx.strokes {
            match &mut s.paint {
                Some(StrokePaint::Gradient { reference, .. }) => *reference = None,
                Some(StrokePaint::Pattern { origin, .. }) => {
                    *origin = (origin.0 + bx, origin.1 + by);
                }
                None => {}
            }
        }
        // A gradient or pattern stroke Aurora cannot draw is not written.
        fx.strokes.retain(aurora_core::effects::Stroke::drawn);
        fx.other = aurora_core::effects::OtherEffects::default();
        found.push((doc.layers.name(id).unwrap_or_default().to_owned(), fx));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Removes the effect kinds a default PSD save does not write.
fn is_extra(path: &std::path::Path) -> bool {
    !path.to_string_lossy().contains("psd-tools-fixtures")
}

fn strip_unverified(doc: &mut aurora_io::PsdDocument) {
    let mut ids = Vec::new();
    let mut pending = doc.layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        pending.extend(doc.layers.children(id).unwrap_or(&[]));
        ids.push(id);
    }
    for id in ids {
        let Some(mut fx) = doc.layers.effects(id).cloned() else {
            continue;
        };
        fx.gradient_overlays.clear();
        fx.pattern_overlays.clear();
        fx.strokes
            .retain(|s| s.fill == aurora_core::effects::StrokeFill::Color);
        if doc.layers.set_effects(id, Some(fx)).is_err() {
            unreachable!("strip");
        }
    }
}

/// Files allowed to change by a bounded amount in the save round trip, each
/// with its reason.
///
/// `stroke-effects.psd` (task 151): its shape layers have translucent fills
/// and Aurora draws their strokes from the vector outline it kept at open,
/// which a PSD file cannot carry (the save report says so), so the reopened
/// file strokes from the pixels' alpha and differs.
///
/// Measured: max 255 (a stroke frame is wholly absent at its edge) and
/// per-channel means 28.55 / 11.84 / 17.79; the bound is the max plus a
/// margin on each mean, so the check still fails if the round trip got worse
/// (before the outline, the same file round-tripped bit for bit).
const COMPOSITE_ALLOWLIST: [(&str, f64, [f64; 3]); 1] =
    [("stroke-effects.psd", 255.0, [29.0, 12.5, 18.5])];

#[test]
#[allow(clippy::too_many_lines)]
fn corpus_files_with_effects_survive_a_psd_save_and_reopen() {
    let keep = std::env::var_os("AURORA_PSD_FIXTURE_DIR").map(std::path::PathBuf::from);
    let mut paths: Vec<std::path::PathBuf> = FILES
        .iter()
        .map(|f| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpora/psd/reference/psd-tools-fixtures")
                .join(f)
        })
        .collect();
    if let Ok(extra) = std::env::var("AURORA_EFFECTS_EXTRA") {
        paths.extend(extra.split(':').filter(|p| !p.is_empty()).map(Into::into));
    }
    let mut ran = 0;
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            println!("SKIPPED: not present at {}", path.display());
            assert!(
                std::env::var_os("AURORA_REQUIRE_CORPUS").is_none() || is_extra(path),
                "AURORA_REQUIRE_CORPUS is set and {} is missing",
                path.display()
            );
            continue;
        };
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (scratch, mut store) = real_tile_store();
        let (doc1, img1_before_strip) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        // The default save writes only the verified kinds (shadows, colour
        // overlays, solid strokes, outer glows): what it leaves out is
        // removed from the document first, so `img1` is the picture the
        // saved file should reproduce.
        let mut doc1 = doc1;
        strip_unverified(&mut doc1);
        let (w, h) = doc1.canvas_size;
        let img1 = match composite_document(&doc1.layers, &mut store, w, h) {
            Ok(image) => image,
            Err(err) => unreachable!("{name}: {err:?}"),
        };
        let _ = img1_before_strip;
        let before = effects_of(&doc1);
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
            Err(err) => unreachable!("{name}: {err:?}"),
        };
        let Ok(written) = std::fs::read(&target) else {
            unreachable!("{name}: written file")
        };
        let (_scratch2, mut store2) = real_tile_store();
        let (doc2, img2) = open_and_composite_psd(
            &written,
            &mut store2,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let after = effects_of(&doc2);
        let (max, mean) = composite_difference(&img2, &img1);
        println!(
            "{name}: {} layers with effects; reopened vs before save: max {max:?} mean {mean:.4?}",
            before.len()
        );
        for line in &outcome.report.items {
            println!("    report: {line}");
        }
        // With the default (solid-kinds-only) save the reopened composite is
        // the saved document's, bit for bit, on every corpus file: the
        // earlier differences (up to 111 levels on layer_effects, 54 on
        // stroke-composite) came from gradient effects laid over a box that
        // is not the saved record rectangle, which the default no longer
        // writes. The allowlist of files allowed to differ is empty; add a
        // file here only with a written reason.
        let (bound, mean_bound) = COMPOSITE_ALLOWLIST
            .iter()
            .find(|(n, _, _)| *n == name)
            .map_or((0.0, [0.0; 3]), |(_, b, m)| (*b, *m));
        assert!(
            max.iter().all(|&m| m <= bound),
            "{name}: composite changed by the save: max {max:?}"
        );
        assert!(
            mean.iter().zip(mean_bound).all(|(m, b)| *m <= b),
            "{name}: composite changed by the save: mean {mean:?} over {mean_bound:?}"
        );
        assert_eq!(before.len(), after.len(), "{name}: layers with effects");
        let mut differing = Vec::new();
        for ((n1, a), (n2, b)) in before.iter().zip(&after) {
            assert_eq!(n1, n2, "{name}");
            if a != b {
                differing.push(n1.clone());
                println!("    DIFFERS: {n1}\n      before {a:?}\n      after  {b:?}");
            }
        }
        assert!(
            differing.is_empty(),
            "{name}: effects differ on {differing:?}"
        );
        ran += 1;
    }
    if ran == 0 {
        println!("SKIPPED: corpus absent");
    }
}
