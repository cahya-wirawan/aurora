// ---- 0.243.0: the PSD fidelity audit ----
//
// Included into `fill_and_overlay_tests` (uses `real_tile_store`,
// `open_and_composite_psd`, `image_pixel`).
//
// A reusable tool, not a gate: `#[ignore]`d, run with
//
//     cargo test -p aurora-app --release psd_fidelity_audit -- --ignored --nocapture
//
// It opens every `.psd`/`.psb` under `corpora/psd` (and `images/`, where a
// maintainer keeps client-like templates), composites each with Aurora's CPU
// compositor, compares with the merged image stored in the file (Photoshop's
// own rendering, the ground truth) and prints a table sorted by error.
// Environment: `AURORA_AUDIT_MAX_MB` (file size cap, default 64),
// `AURORA_AUDIT_SECS` (per-file time cap, default 120), `AURORA_AUDIT_DIRS`
// (extra directories, `:`-separated), `AURORA_AUDIT_OUT` (a TSV copy of the
// table). A file that panics, fails to open or runs over its cap is printed
// and the audit continues.

/// One audited file's result.
#[derive(Clone, Debug)]
struct AuditRow {
    name: String,
    /// Mean absolute error per channel in 8-bit levels, and the maximum.
    mean: [f64; 3],
    max: f64,
    report_lines: usize,
    first_report: String,
    millis: u128,
}

impl AuditRow {
    fn score(&self) -> f64 {
        self.mean.iter().sum::<f64>() / 3.0
    }
}

/// What an audited file turned into.
#[derive(Clone, Debug)]
enum AuditOutcome {
    Row(AuditRow),
    /// Not measured, with the reason (printed, never a failure).
    Skipped(String, String),
}

/// Mean/max error of `image` against `merged` in 8-bit levels, a
/// transparent Aurora pixel compared as if over white (the merged image is
/// opaque). `None` when the sizes differ.
#[allow(clippy::cast_precision_loss)]
fn audit_diff(image: &aurora_io::Image, merged: &aurora_io::Image) -> Option<([f64; 3], f64)> {
    if (image.width(), image.height()) != (merged.width(), merged.height()) {
        return None;
    }
    let (mut sum, mut max) = ([0.0_f64; 3], 0.0_f64);
    for y in 0..image.height() {
        for x in 0..image.width() {
            let [r, g, b, a] = image_pixel(image, x, y);
            let want = image_pixel(merged, x, y);
            let a = f64::from(a);
            for (c, got) in [r, g, b].into_iter().enumerate() {
                let got = f64::from(got) * a + (1.0 - a);
                let want = f64::from(want.get(c).copied().unwrap_or(0.0));
                let error = ((got * 255.0).round() - (want * 255.0).round()).abs();
                max = max.max(error);
                if let Some(s) = sum.get_mut(c) {
                    *s += error;
                }
            }
        }
    }
    let n = (f64::from(image.width()) * f64::from(image.height())).max(1.0);
    Some((sum.map(|s| s / n), max))
}

/// Opens, composites and measures one file (no caps: the caller applies
/// them). Errors are strings, never panics.
fn audit_one_file(path: &std::path::Path, name: &str) -> Result<AuditRow, String> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable: {e}"))?;
    let merged = aurora_io::decode_psd_merged_image(&bytes)
        .map_err(|e| format!("no merged image: {e:?}"))?;
    let document = aurora_io::read_psd(&bytes).map_err(|e| format!("not opened: {e:?}"))?;
    let report = document.report.items.clone();
    drop(document);
    let (_scratch, mut store) = real_tile_store();
    let (_document, image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let (mean, max) = audit_diff(&image, &merged).ok_or("merged image size differs")?;
    Ok(AuditRow {
        name: name.to_owned(),
        mean,
        max,
        report_lines: report.len(),
        first_report: report.first().cloned().unwrap_or_default(),
        millis: started.elapsed().as_millis(),
    })
}

/// Every `.psd`/`.psb` below `dirs`, sorted.
fn audit_collect_files(dirs: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack: Vec<_> = dirs.to_vec();
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Audits one file under the size and time caps and a panic guard.
fn audit_guarded(
    path: &std::path::Path,
    name: &str,
    max_bytes: u64,
    secs: u64,
) -> AuditOutcome {
    let skipped = |why: String| AuditOutcome::Skipped(name.to_owned(), why);
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > max_bytes => {
            return skipped(format!(
                "{} MiB is over the {} MiB size cap",
                meta.len() >> 20,
                max_bytes >> 20
            ));
        }
        Ok(_) => {}
        Err(e) => return skipped(format!("unreadable: {e}")),
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (path_owned, name_owned) = (path.to_path_buf(), name.to_owned());
    let spawned = std::thread::Builder::new()
        .name(format!("audit {name}"))
        .stack_size(64 << 20)
        .spawn(move || {
            let result = std::panic::catch_unwind(|| audit_one_file(&path_owned, &name_owned));
            let _ = tx.send(match result {
                Ok(r) => r,
                Err(_) => Err("panicked".to_owned()),
            });
        });
    if let Err(e) = spawned {
        return skipped(format!("could not start a worker: {e}"));
    }
    match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
        Ok(Ok(row)) => AuditOutcome::Row(row),
        Ok(Err(why)) => skipped(why),
        Err(_) => skipped(format!("over the {secs} s time cap (worker abandoned)")),
    }
}

/// Buckets for the summary: `<1`, `<3`, `<10`, worse.
fn audit_buckets(rows: &[AuditRow]) -> [usize; 4] {
    let mut b = [0; 4];
    for row in rows {
        let s = row.score();
        let i = if s < 1.0 {
            0
        } else if s < 3.0 {
            1
        } else if s < 10.0 {
            2
        } else {
            3
        };
        if let Some(slot) = b.get_mut(i) {
            *slot += 1;
        }
    }
    b
}

fn audit_table(rows: &[AuditRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let first: String = row.first_report.chars().take(90).collect();
        out.push_str(&format!(
            "{:7.2} | {:7.2} {:7.2} {:7.2} | max {:3.0} | rep {:2} | {:6} ms | {} | {}\n",
            row.score(),
            row.mean[0],
            row.mean[1],
            row.mean[2],
            row.max,
            row.report_lines,
            row.millis,
            row.name,
            first
        ));
    }
    out
}

/// The audit. `#[ignore]`d: it walks the whole corpus (minutes).
#[test]
#[ignore = "walks the whole PSD corpus; run on demand"]
fn psd_fidelity_audit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs = vec![root.join("corpora/psd"), root.join("images")];
    if let Ok(extra) = std::env::var("AURORA_AUDIT_DIRS") {
        dirs.extend(extra.split(':').filter(|s| !s.is_empty()).map(Into::into));
    }
    let files = audit_collect_files(&dirs);
    if files.is_empty() {
        println!("SKIPPED: no PSD/PSB corpus present under corpora/psd");
        return;
    }
    let env_u64 = |key: &str, default: u64| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let max_bytes = env_u64("AURORA_AUDIT_MAX_MB", 64) << 20;
    let secs = env_u64("AURORA_AUDIT_SECS", 120);
    let (mut rows, mut skipped) = (Vec::new(), Vec::new());
    for path in &files {
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace("corpora/psd/reference/psd-tools-fixtures/", "psd-tools/")
            .replace('\\', "/");
        match audit_guarded(path, &name, max_bytes, secs) {
            AuditOutcome::Row(row) => rows.push(row),
            AuditOutcome::Skipped(name, why) => {
                println!("SKIPPED {name}: {why}");
                skipped.push((name, why));
            }
        }
    }
    rows.sort_by(|a, b| b.score().total_cmp(&a.score()).then(a.name.cmp(&b.name)));
    let table = audit_table(&rows);
    println!("score | mean R G B (levels) | max | report lines | time | file | first report line");
    print!("{table}");
    let b = audit_buckets(&rows);
    println!(
        "AUDIT: {} files measured, {} skipped; mean error <1 level: {}, <3: {}, <10: {}, worse: {}",
        rows.len(),
        skipped.len(),
        b[0],
        b[1],
        b[2],
        b[3]
    );
    if let Ok(out) = std::env::var("AURORA_AUDIT_OUT") {
        let _ = std::fs::write(out, table);
    }
}

#[test]
fn audit_buckets_and_diff_are_arithmetically_right() {
    let row = |s: f64| AuditRow {
        name: String::new(),
        mean: [s, s, s],
        max: 0.0,
        report_lines: 0,
        first_report: String::new(),
        millis: 0,
    };
    let rows = [row(0.5), row(0.99), row(1.0), row(2.9), row(3.0), row(9.9), row(10.0), row(80.0)];
    assert_eq!(audit_buckets(&rows), [2, 2, 2, 2]);
    // A missing directory is not an error, and an unreadable path is
    // skipped with a reason, never a panic.
    assert!(audit_collect_files(&[std::path::PathBuf::from("/nonexistent-audit-dir")]).is_empty());
    match audit_guarded(std::path::Path::new("/nonexistent/x.psd"), "x", 1 << 20, 5) {
        AuditOutcome::Skipped(name, why) => {
            assert_eq!(name, "x");
            assert!(why.contains("unreadable"), "{why}");
        }
        AuditOutcome::Row(_) => unreachable!("a missing file has no row"),
    }
    // Hostile bytes: skipped, not a panic.
    let dir = tempfile::tempdir().map_err(|e| e.to_string());
    if let Ok(dir) = dir {
        let path = dir.path().join("junk.psd");
        let _ = std::fs::write(&path, b"8BPS not really a psd");
        assert!(matches!(
            audit_guarded(&path, "junk.psd", 1 << 20, 30),
            AuditOutcome::Skipped(..)
        ));
        let _ = std::fs::write(&path, vec![0_u8; 4096]);
        assert!(matches!(
            audit_guarded(&path, "zeros.psd", 1024, 30),
            AuditOutcome::Skipped(_, why) if why.contains("size cap")
        ));
    }
}
