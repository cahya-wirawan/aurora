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
            // `blend-and-clipping.psd`'s merged image is a placeholder and its
            // eight shape layers store no pixels (task 108 draws them), so
            // its error against the file's merged image is not a fidelity
            // measure; the other clipped files keep the check.
            if name == "blend-and-clipping.psd" {
                continue;
            }
            for (after, before) in a.mean.iter().zip(b.mean) {
                assert!(*after <= before + 0.05, "{name}: clipping made {name} worse");
            }
        }
    }
    if !any {
        println!("SKIPPED: no corpus file with a clipped layer present");
    }
}
