// ---- task 157: the PSD round-trip fidelity report ----
//
// Included into `fill_and_overlay_tests` next to `psd_audit_tests.rs` (uses
// its `audit_*` helpers, `real_tile_store`, `open_and_composite_psd`,
// `image_pixel`). An ignored, on-demand test:
//
//     AURORA_REQUIRE_CORPUS=1 cargo test -p aurora-app --release \
//         psd_roundtrip_report -- --ignored --nocapture
//
// For every corpus PSD it opens the file, builds the save report, saves a
// `.psd` into a temp directory through the app's own save path, reopens the
// result and records: the report's group counts, the reopened composite
// against the original composite, and the original composite against the
// merged image Photoshop stored in the file. It writes a markdown table to
// `docs/psd-roundtrip-report.md` (or `AURORA_ROUNDTRIP_OUT`). Environment:
// `AURORA_AUDIT_MAX_MB` (size cap, default 64), `AURORA_AUDIT_SECS` (per-file
// time cap, default 300), `AURORA_AUDIT_DIRS`, `AURORA_AUDIT_ONLY` (as the
// fidelity audit), `AURORA_AUDIT_DUMP` (a directory that keeps each saved
// `.psd` for inspection). Photoshop has never opened any file this writes.

#[derive(Clone, Debug)]
struct RoundTripRow {
    name: String,
    megabytes: f64,
    /// Report lines per category, in `ReportCategory::ALL` order.
    counts: [usize; 5],
    asks: bool,
    /// Original composite against Photoshop's merged image: mean of the
    /// three channels' mean error and the maximum, in 8-bit levels.
    original_vs_merged: Option<(f64, f64)>,
    /// Reopened composite against the original composite.
    reopened_vs_original: (f64, f64),
    /// Reopened composite against Photoshop's merged image.
    reopened_vs_merged: Option<(f64, f64)>,
    real_merged: bool,
    millis: u128,
}

/// Mean (of the channels) and maximum 8-bit error between two composites,
/// both flattened over white.
#[allow(clippy::cast_precision_loss)]
fn roundtrip_diff(a: &aurora_io::Image, b: &aurora_io::Image) -> Option<(f64, f64)> {
    if (a.width(), a.height()) != (b.width(), b.height()) {
        return None;
    }
    let (mut sum, mut max) = (0.0_f64, 0.0_f64);
    for y in 0..a.height() {
        for x in 0..a.width() {
            let (pa, pb) = (image_pixel(a, x, y), image_pixel(b, x, y));
            let (aa, ab) = (f64::from(pa[3]), f64::from(pb[3]));
            for c in 0..3 {
                let va = f64::from(pa.get(c).copied().unwrap_or(0.0)) * aa + (1.0 - aa);
                let vb = f64::from(pb.get(c).copied().unwrap_or(0.0)) * ab + (1.0 - ab);
                let e = ((va * 255.0).round() - (vb * 255.0).round()).abs();
                max = max.max(e);
                sum += e;
            }
        }
    }
    let n = (f64::from(a.width()) * f64::from(a.height())).max(1.0) * 3.0;
    Some((sum / n, max))
}

fn make_source<'a>(
    document: &'a aurora_io::PsdDocument,
    saved: &[(&'a str, &'a aurora_doc::Selection)],
    carried: &[aurora_io::ReportEntry],
    store: &'a mut aurora_tile::TileStore,
) -> crate::psd_save::PsdSaveSource<'a> {
    crate::psd_save::PsdSaveSource {
        saved: saved.to_vec(),
        layers: &document.layers,
        canvas: document.canvas_size,
        store,
        doc_id: crate::document_session::DocumentId::next(),
        revision: crate::UndoRevision(0),
        carried: carried.to_vec(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn roundtrip_one_file(path: &std::path::Path, name: &str) -> Result<RoundTripRow, String> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable: {e}"))?;
    let merged = aurora_io::decode_psd_merged_image_in_working_space(&bytes).ok();
    let (_scratch, mut store) = real_tile_store();
    let (document, original) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let mut carried = crate::psd_save::carried_losses(
        Some(&document.report.items).filter(|items| !items.is_empty()).map(Vec::as_slice),
        &aurora_io::SkippedTiles::new(),
    );
    carried.extend(crate::psd_save::colour_loss_entries(&aurora_io::DocumentColour {
        source: document.colour.source.clone(),
        output: None,
    }));
    let saved: Vec<(&str, &aurora_doc::Selection)> = document
        .saved_selections
        .iter()
        .map(|(n, s)| (n.as_str(), s))
        .collect();
    let report = crate::psd_save::check_psd_compatibility(make_source(&document, &saved, &carried, &mut store))
        .map_err(|f| format!("could not be saved: {}", crate::psd_save::psd_check_failure_message(&f)))?;
    let grouped = crate::psd_report::SaveReport::new(&report);
    let mut counts = [0; 5];
    for (slot, category) in counts.iter_mut().zip(aurora_io::ReportCategory::ALL) {
        *slot = grouped.count(category);
    }
    let dump = std::env::var("AURORA_AUDIT_DUMP").ok();
    let out_dir = tempfile::tempdir().map_err(|e| format!("temp dir: {e}"))?;
    let target_dir = dump.as_deref().map_or_else(
        || out_dir.path().to_path_buf(),
        std::path::PathBuf::from,
    );
    let file_name = format!("{}.roundtrip.psd", name.replace('/', "_"));
    let target = target_dir.join(&file_name);
    let source = make_source(&document, &saved, &carried, &mut store);
    let (doc_id, revision) = (source.doc_id, source.revision);
    match crate::psd_save::begin_psd_save(source, &target) {
        Ok(crate::psd_save::PsdSaveStep::Saved | crate::psd_save::PsdSaveStep::SavedNotConfirmed) => {}
        Ok(crate::psd_save::PsdSaveStep::NeedsConfirmation(pending)) => {
            crate::psd_save::finish_psd_save(pending, doc_id, revision)
                .map_err(|f| format!("Save Anyway failed: {f:?}"))?;
        }
        Err(f) => return Err(format!("save failed: {f:?}")),
    }
    let saved_bytes = std::fs::read(&target).map_err(|e| format!("saved file unreadable: {e}"))?;
    let (_scratch2, mut store2) = real_tile_store();
    let reopened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_and_composite_psd(
            &saved_bytes,
            &mut store2,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        )
        .1
    }))
    .map_err(|_| "the saved file did not reopen (panic)".to_owned())?;
    let reopened_vs_original =
        roundtrip_diff(&reopened, &original).ok_or("reopened size differs from the original")?;
    Ok(RoundTripRow {
        name: name.to_owned(),
        megabytes: bytes.len() as f64 / (1024.0 * 1024.0),
        counts,
        asks: grouped.asks(),
        original_vs_merged: merged.as_ref().and_then(|m| roundtrip_diff(&original, m)),
        reopened_vs_original,
        reopened_vs_merged: merged.as_ref().and_then(|m| roundtrip_diff(&reopened, m)),
        real_merged: audit_has_real_merged_data(&bytes),
        millis: started.elapsed().as_millis(),
    })
}

fn roundtrip_guarded(
    path: &std::path::Path,
    name: &str,
    secs: u64,
) -> Result<RoundTripRow, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (path_owned, name_owned) = (path.to_path_buf(), name.to_owned());
    std::thread::Builder::new()
        .name(format!("roundtrip {name}"))
        .stack_size(64 << 20)
        .spawn(move || {
            let result = std::panic::catch_unwind(|| roundtrip_one_file(&path_owned, &name_owned));
            let _ = tx.send(result.unwrap_or_else(|_| Err("panicked".to_owned())));
        })
        .map_err(|e| format!("could not start a worker: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .unwrap_or_else(|_| Err(format!("over the {secs} s time cap (worker abandoned)")))
}

/// `(year, month, day)` of a count of days since 1970-01-01 (civil calendar).
fn roundtrip_civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn roundtrip_header(
    max_mb: u64,
    measured: usize,
    skipped: usize,
    failed: usize,
    local: usize,
) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = roundtrip_civil(i64::try_from(secs / 86_400).unwrap_or(0));
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map_or_else(
                || "unknown".to_owned(),
                |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
            )
    };
    let commit = run("git", &["rev-parse", "--short", "HEAD"]);
    let dirty = run("git", &["status", "--porcelain", "--untracked-files=no"]);
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned()))
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZeroUsize::get);
    format!(
        "# PSD round-trip fidelity report\n\n\
         *Generated by the ignored test `psd_roundtrip_report` in `crates/aurora-app`; do not \
         edit by hand.*\n\n\
         - Date: {y:04}-{m:02}-{d:02} (UTC)\n\
         - Git commit: {commit}{}\n\
         - Machine: {os}, {cores} threads\n\
         - Size cap: files over {max_mb} MiB are skipped. {measured} files measured, {skipped} \
         skipped, {failed} failed.\n\
         - Corpus: the files under `corpora/psd` in the repository, plus a local corpus of {local} \
         files that is not in the repository; those are listed as `local file #N` and their \
         paths are not recorded.\n\n\
         ## What this is, and what it is not\n\n\
         For each corpus PSD Aurora opens it, builds the Save As `.psd` report \
         (`psd_report`), saves a `.psd` through its own safe-save path, reopens that \
         file with Aurora's own reader and compares composites. **Photoshop never opened any \
         file this test wrote**, and the \"Photoshop merged\" columns compare Aurora's \
         composite with the merged image Photoshop stored in the *original* file; nothing here \
         says how a saved file looks in Photoshop. Errors are 8-bit levels (mean of the three \
         channels, then the maximum), both sides flattened over white. Aurora reads its own \
         output, so a defect that its reader and writer share would not show.\n\n\
         Report columns count lines per group: **A** needs attention, **N** not saved, **C** \
         changed, **O** already different when opened, **K** kept as they are. **Asks** is \
         whether Save As would show the \"Save With Changes?\" dialog.\n\n",
        if dirty.is_empty() || dirty == "unknown" { "" } else { " (plus uncommitted changes)" },
    )
}

#[allow(clippy::cast_precision_loss)]
fn roundtrip_markdown(
    header: &str,
    rows: &[RoundTripRow],
    skipped: &[(String, String)],
    failed: &[(String, String)],
) -> String {
    use std::fmt::Write;
    let fmt = |v: Option<(f64, f64)>| {
        v.map_or_else(|| "n/a".to_owned(), |(m, x)| format!("{m:.2} / {x:.0}"))
    };
    let mut out = header.to_owned();
    let asks = rows.iter().filter(|r| r.asks).count();
    let exact = rows.iter().filter(|r| r.reopened_vs_original.1 <= 1.0).count();
    let _ = writeln!(
        out,
        "## Summary\n\n- Save As would ask first for {asks} of {} files.\n- Reopened composite within \
         1 level (max) of the original for {exact} of {} files.\n\n## Per file\n",
        rows.len(),
        rows.len()
    );
    let _ = writeln!(
        out,
        "| File | MiB | A | N | C | O | K | Asks | Original vs Photoshop merged (mean / max) | \
         Reopened vs original (mean / max) | Reopened vs Photoshop merged (mean / max) | Time (s) |\n\
         |---|---:|---:|---:|---:|---:|---:|:--:|---:|---:|---:|---:|"
    );
    for r in rows {
        let name = r.name.replace('|', "\\|");
        let ps = if r.real_merged { "" } else { " (placeholder merged image)" };
        let _ = writeln!(
            out,
            "| {name} | {:.1} | {} | {} | {} | {} | {} | {} | {}{ps} | {} | {}{ps} | {:.1} |",
            r.megabytes,
            r.counts[0],
            r.counts[1],
            r.counts[2],
            r.counts[3],
            r.counts[4],
            if r.asks { "yes" } else { "no" },
            fmt(r.original_vs_merged),
            fmt(Some(r.reopened_vs_original)),
            fmt(r.reopened_vs_merged),
            r.millis as f64 / 1000.0,
        );
    }
    for (title, list) in [("Failed to save or reopen", failed), ("Skipped", skipped)] {
        let _ = writeln!(out, "\n## {title}\n");
        if list.is_empty() {
            let _ = writeln!(out, "None.");
        }
        for (name, why) in list {
            let _ = writeln!(out, "- `{name}`: {why}");
        }
    }
    out
}

#[test]
#[ignore = "walks the whole PSD corpus and saves each file; run on demand"]
fn psd_roundtrip_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs = vec![root.join("corpora/psd"), root.join("images")];
    if let Ok(extra) = std::env::var("AURORA_AUDIT_DIRS") {
        dirs.extend(extra.split(':').filter(|s| !s.is_empty()).map(Into::into));
    }
    let mut files = audit_collect_files(&dirs);
    if let Ok(only) = std::env::var("AURORA_AUDIT_ONLY") {
        let keep: Vec<_> = only.split(',').filter(|f| !f.is_empty()).collect();
        files.retain(|f| keep.iter().any(|k| f.to_string_lossy().contains(k)));
    }
    if files.is_empty() {
        println!("SKIPPED: no PSD/PSB corpus present under corpora/psd");
        return;
    }
    let env_u64 = |key: &str, default: u64| {
        std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    let max_mb = env_u64("AURORA_AUDIT_MAX_MB", 64);
    let secs = env_u64("AURORA_AUDIT_SECS", 300);
    let (mut rows, mut skipped, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    let mut local = 0;
    for path in &files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace("corpora/psd/reference/psd-tools-fixtures/", "psd-tools/")
            .replace('\\', "/");
        // Anything outside the repository's own corpus is a local file: its
        // path is not recorded, so the committed report carries none.
        let name = if relative.starts_with("corpora/") || relative.starts_with("psd-tools/") {
            relative
        } else {
            local += 1;
            format!("local file #{local}")
        };
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > max_mb << 20 => {
                let why = format!("{} MiB is over the {max_mb} MiB size cap", meta.len() >> 20);
                println!("SKIPPED {name}: {why}");
                skipped.push((name, why));
                continue;
            }
            Err(e) => {
                skipped.push((name, format!("unreadable: {e}")));
                continue;
            }
            Ok(_) => {}
        }
        match roundtrip_guarded(path, &name, secs) {
            Ok(row) => {
                println!(
                    "{:60} A{} N{} C{} O{} K{} asks={} reopened/original {:.2}/{:.0}",
                    row.name, row.counts[0], row.counts[1], row.counts[2], row.counts[3],
                    row.counts[4], row.asks, row.reopened_vs_original.0, row.reopened_vs_original.1
                );
                rows.push(row);
            }
            Err(why) => {
                println!("FAILED {name}: {why}");
                failed.push((name, why));
            }
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    let header = roundtrip_header(max_mb, rows.len(), skipped.len(), failed.len(), local);
    let markdown = roundtrip_markdown(&header, &rows, &skipped, &failed);
    let out = std::env::var("AURORA_ROUNDTRIP_OUT")
        .map_or_else(|_| root.join("docs/psd-roundtrip-report.md"), Into::into);
    if let Err(e) = std::fs::write(&out, markdown) {
        println!("could not write {}: {e}", out.display());
    }
}

#[test]
fn roundtrip_helpers_are_arithmetically_right() {
    assert_eq!(roundtrip_civil(0), (1970, 1, 1));
    assert_eq!(roundtrip_civil(20_000), (2024, 10, 4));
    let a = {
        let samples = [0.0, 0.0, 0.0, 1.0].map(half::f16::from_f32);
        match aurora_io::Image::new(1, 1, aurora_color::IccProfile::srgb(), samples.to_vec()) {
            Ok(i) => i,
            Err(e) => unreachable!("{e:?}"),
        }
    };
    assert_eq!(roundtrip_diff(&a, &a), Some((0.0, 0.0)));
    let rows = [RoundTripRow {
        name: "x|y.psd".to_owned(),
        megabytes: 1.0,
        counts: [1, 0, 2, 0, 3],
        asks: true,
        original_vs_merged: None,
        reopened_vs_original: (0.5, 3.0),
        reopened_vs_merged: Some((1.0, 2.0)),
        real_merged: true,
        millis: 1,
    }];
    let md = roundtrip_markdown("H\n", &rows, &[], &[("f.psd".to_owned(), "boom".to_owned())]);
    assert!(md.contains("| x\\|y.psd | 1.0 | 1 | 0 | 2 | 0 | 3 | yes | n/a | 0.50 / 3 | 1.00 / 2 | 0.0 |"), "{md}");
    assert!(md.contains("`f.psd`: boom"), "{md}");
}
