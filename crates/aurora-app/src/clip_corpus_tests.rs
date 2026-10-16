// ---- 0.201.0: clipping masks against Photoshop's merged images ----
//
// Included into `fill_and_overlay_tests` (uses `psd_merged_diff`,
// `open_and_composite_psd`, `real_tile_store`).

fn clipped_layer_count(bytes: &[u8]) -> usize {
    let Ok(document) = aurora_io::read_psd(bytes) else {
        return 0;
    };
    let mut pending = document.layers.roots().to_vec();
    let mut n = 0;
    while let Some(id) = pending.pop() {
        n += usize::from(document.layers.clip_base(id).is_some());
        pending.extend(document.layers.children(id).unwrap_or(&[]));
    }
    n
}

/// Every PSD/PSB of the corpus (and the user's Nyepi template) with a
/// clipped layer: mean and max error against the file's own merged image
/// with clipping off ("before") and on ("after"). Prints the table PLAN.md
/// records; asserts clipping never makes a file worse by more than noise.
#[test]
fn clipped_files_against_their_merged_images_before_and_after() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    let mut stack = vec![root.join("corpora/psd")];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("psd" | "psb")
            ) {
                files.push(path);
            }
        }
    }
    files.push(root.join("images/flat-design-nyepi-template/6739968.psd"));
    files.sort();
    let mut any = false;
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: {} not present", path.display());
            continue;
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let clipped = clipped_layer_count(&bytes);
        if clipped == 0 {
            continue;
        }
        any = true;
        crate::CLIPPING_DISABLED.with(|c| c.set(true));
        let before = psd_merged_diff(&path, name);
        crate::CLIPPING_DISABLED.with(|c| c.set(false));
        let after = psd_merged_diff(&path, name);
        if let (Some(b), Some(a)) = (before, after) {
            println!(
                "CLIP {name}: {clipped} clipped layers; before mean {:?} max {:?} | after mean {:?} max {:?}",
                b.mean, b.max, a.mean, a.max
            );
            // A file saved without Maximize Compatibility holds a placeholder
            // merged image (`blend-and-clipping.psd`, whose eight shape
            // layers also store no pixels, drawn since task 108), so a
            // per-channel comparison with it measures nothing: the check
            // that clipping did not make the file worse than leaving it
            // unclipped still runs, on the three channels' summed mean, which
            // is the one number a blank-vs-drawing comparison cannot flip
            // channel by channel. A file with a real merged image keeps the
            // per-channel check.
            if audit_has_real_merged_data(&bytes) {
                for (after, before) in a.mean.iter().zip(b.mean) {
                    assert!(*after <= before + 0.05, "{name}: clipping made {name} worse");
                }
            } else {
                let (after, before): (f64, f64) = (a.mean.iter().sum(), b.mean.iter().sum());
                assert!(
                    after <= before + 0.15,
                    "{name}: clipping made {name} worse against its placeholder merged image"
                );
            }
        }
    }
    if !any {
        println!("SKIPPED: no corpus file with a clipped layer present");
    }
}
