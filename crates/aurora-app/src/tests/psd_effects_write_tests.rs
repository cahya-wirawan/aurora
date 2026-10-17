//! Task 120: layer effects in a PSD save. Corpus files that carry effects
//! are opened, saved as PSD (written and verified by the writer's two
//! passes), reopened: every layer's effects must come back equal for what
//! the file holds, and the composite is compared with the one before the
//! save. With `AURORA_PSD_FIXTURE_DIR` set the saved files are kept as
//! `resave_<name>` for `scripts/verify_psd_write.py check_effects`.
//! `AURORA_EFFECTS_EXTRA` may name more files (a colon-separated list).

use aurora_core::effects::StrokePaint;

use super::{composite_difference, open_and_composite_psd, real_tile_store};

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

#[test]
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
            continue;
        };
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (scratch, mut store) = real_tile_store();
        let (doc1, img1) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
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
