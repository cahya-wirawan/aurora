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
// table), `AURORA_AUDIT_DUMP` (a directory for raw 8-bit images of both
// sides, for a script to diff). A file that panics, fails to open or runs over its cap is printed
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
    /// False when the file holds no real merged image: the numbers then
    /// compare Aurora with a placeholder and say nothing about fidelity.
    real_merged: bool,
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

/// Whether the file says its merged image is real (image resource 1057,
/// "version info", has-real-merged-data byte). A file saved without
/// "Maximize Compatibility" stores a placeholder there, so it is no ground
/// truth. `true` when the resource is absent (Photoshop's default).
fn audit_has_real_merged_data(bytes: &[u8]) -> bool {
    let be32 = |at: usize| -> Option<usize> {
        let b = bytes.get(at..at.checked_add(4)?)?;
        Some(u32::from_be_bytes([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?]) as usize)
    };
    // Header 26 bytes, then the colour mode data section, then resources.
    let Some(mode_len) = be32(26) else { return true };
    let Some(res_at) = 30_usize.checked_add(mode_len) else { return true };
    let Some(res_len) = be32(res_at) else { return true };
    let mut at = res_at + 4;
    let end = at.saturating_add(res_len).min(bytes.len());
    while at + 8 <= end {
        if bytes.get(at..at + 4) != Some(b"8BIM") {
            break;
        }
        let id = match bytes.get(at + 4..at + 6) {
            Some([hi, lo]) => u16::from_be_bytes([*hi, *lo]),
            _ => 0,
        };
        let name_len = bytes.get(at + 6).map_or(0, |b| usize::from(*b));
        let data_at = at + 6 + ((1 + name_len + 1) & !1);
        let Some(len) = be32(data_at) else { break };
        let body = data_at + 4;
        if id == 1057 {
            return bytes.get(body + 4).is_none_or(|b| *b != 0);
        }
        at = body.saturating_add((len + 1) & !1);
    }
    true
}

/// `AURORA_AUDIT_DUMP=dir`: writes `<file>.aurora.rgb` and
/// `<file>.merged.rgb` (8-bit RGB, a transparent pixel over white; the
/// size is in `<file>.size`) for a script to diff. Best effort.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn audit_dump(dir: &str, name: &str, image: &aurora_io::Image, merged: &aurora_io::Image) {
    let flat = |img: &aurora_io::Image| {
        let mut out = Vec::new();
        for y in 0..img.height() {
            for x in 0..img.width() {
                let [r, g, b, a] = image_pixel(img, x, y);
                for c in [r, g, b] {
                    let v = f64::from(c) * f64::from(a) + (1.0 - f64::from(a));
                    out.push((v * 255.0).round().clamp(0.0, 255.0) as u8);
                }
            }
        }
        out
    };
    let stem = std::path::Path::new(dir).join(name.replace('/', "__"));
    let _ = std::fs::create_dir_all(dir);
    let with = |ext: &str| {
        let mut s = stem.clone().into_os_string();
        s.push(ext);
        std::path::PathBuf::from(s)
    };
    let _ = std::fs::write(with(".aurora.rgb"), flat(image));
    let _ = std::fs::write(with(".merged.rgb"), flat(merged));
    let _ = std::fs::write(with(".size"), format!("{} {}", image.width(), image.height()));
}

/// `AURORA_AUDIT_STRIP_PROFILE=1`: the file with its colour profile resource
/// (1039) renamed to an unused id, so it opens through the pre-colour-
/// management pipeline (layers taken as sRGB, nothing converted) while the
/// merged image is still converted to the working space. **The audit's basis
/// changed with colour management:** the opened document is now in sRGB, so
/// it is compared with the converted merged image; this knob measures the old
/// pipeline on that same new basis, which is the only fair before/after.
fn audit_strip_profile(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let be32 = |b: &[u8], at: usize| -> Option<usize> {
        let s = b.get(at..at + 4)?;
        usize::try_from(u32::from_be_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?])).ok()
    };
    let Some(mode_len) = be32(bytes, 26) else {
        return out;
    };
    let section = 30 + mode_len;
    let (Some(len), mut at) = (be32(bytes, section), section + 4) else {
        return out;
    };
    let end = (at + len).min(bytes.len());
    while at + 12 <= end {
        let (Some(id_hi), Some(id_lo)) = (bytes.get(at + 4), bytes.get(at + 5)) else {
            break;
        };
        let id = u16::from_be_bytes([*id_hi, *id_lo]);
        let name = usize::from(bytes.get(at + 6).copied().unwrap_or(0)) + 1;
        let name = name + name % 2;
        let Some(size) = be32(bytes, at + 6 + name) else {
            break;
        };
        if id == 1039
            && let Some(slot) = out.get_mut(at + 4..at + 6)
        {
            slot.copy_from_slice(&0x7FFF_u16.to_be_bytes());
        }
        at += 6 + name + 4 + size + size % 2;
    }
    out
}

/// Opens, composites and measures one file (no caps: the caller applies
/// them). Errors are strings, never panics.
fn audit_one_file(path: &std::path::Path, name: &str) -> Result<AuditRow, String> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable: {e}"))?;
    let merged = aurora_io::decode_psd_merged_image_in_working_space(&bytes)
        .map_err(|e| format!("no merged image: {e:?}"))?;
    let bytes = if std::env::var_os("AURORA_AUDIT_STRIP_PROFILE").is_some() {
        audit_strip_profile(&bytes)
    } else {
        bytes
    };
    let document = aurora_io::read_psd(&bytes).map_err(|e| format!("not opened: {e:?}"))?;
    let report = document.report.items.clone();
    drop(document);
    let (_scratch, mut store) = real_tile_store();
    let (_document, image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    if let Ok(dir) = std::env::var("AURORA_AUDIT_DUMP") {
        audit_dump(&dir, name, &image, &merged);
    }
    let (mean, max) = audit_diff(&image, &merged).ok_or("merged image size differs")?;
    Ok(AuditRow {
        name: name.to_owned(),
        mean,
        max,
        report_lines: report.len(),
        first_report: report.first().cloned().unwrap_or_default(),
        millis: started.elapsed().as_millis(),
        real_merged: audit_has_real_merged_data(&bytes),
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
    use std::fmt::Write;
    let mut out = String::new();
    for row in rows {
        let first: String = row.first_report.chars().take(90).collect();
        let _ = writeln!(
            out,
            "{:7.2} | {:7.2} {:7.2} {:7.2} | max {:3.0} | rep {:2} | {:6} ms | {}{} | {}",
            row.score(),
            row.mean[0],
            row.mean[1],
            row.mean[2],
            row.max,
            row.report_lines,
            row.millis,
            row.name,
            if row.real_merged { "" } else { " [NO REAL MERGED IMAGE]" },
            first
        );
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
    let mut files = audit_collect_files(&dirs);
    // `AURORA_AUDIT_ONLY`: keep the files whose path contains one of these
    // `,`-separated fragments (a quick re-measure of a few files).
    if let Ok(only) = std::env::var("AURORA_AUDIT_ONLY") {
        let keep: Vec<_> = only.split(',').filter(|f| !f.is_empty()).collect();
        files.retain(|f| keep.iter().any(|k| f.to_string_lossy().contains(k)));
    }
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
    let (rows, placeholders): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.real_merged);
    println!("score | mean R G B (levels) | max | report lines | time | file | first report line");
    print!("{table}");
    let b = audit_buckets(&rows);
    println!(
        "AUDIT: {} files measured against a real merged image ({} more have only a placeholder), {} skipped; mean error <1 level: {}, <3: {}, <10: {}, worse: {}",
        rows.len(),
        placeholders.len(),
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
        real_merged: true,
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
    // The version-info resource: absent -> real; flag 0 -> placeholder.
    let file = |version_info: Option<u8>| {
        let mut b = vec![0_u8; 26];
        b.extend_from_slice(&0_u32.to_be_bytes()); // colour mode data
        let mut res = Vec::new();
        // An unrelated resource first, with an odd-length name to pad.
        res.extend_from_slice(b"8BIM\x03\xed\x01a\x00\x00\x00\x03abc\x00");
        if let Some(flag) = version_info {
            res.extend_from_slice(b"8BIM\x04\x21\x00\x00\x00\x00\x00\x07");
            res.extend_from_slice(&[0, 0, 0, 1, flag, 0, 0, 0]);
        }
        b.extend_from_slice(&(res.len() as u32).to_be_bytes());
        b.extend_from_slice(&res);
        b
    };
    assert!(audit_has_real_merged_data(&file(None)));
    assert!(audit_has_real_merged_data(&file(Some(1))));
    assert!(!audit_has_real_merged_data(&file(Some(0))));
    assert!(audit_has_real_merged_data(b"too short"));
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
