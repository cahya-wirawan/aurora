//! Open-time tests and measurements (0.189.0, round R5 of the
//! large-template performance plan).

use std::time::Instant;

/// The user's Nyepi template, if it is on this machine: `AURORA_EFFECTS_PSD`
/// or the checkout's `images/` copy. `None` (the test prints SKIPPED) when
/// neither names an existing file — CI has the directory without the files.
fn nyepi_path() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("AURORA_EFFECTS_PSD") {
        let path = std::path::PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../images/flat-design-nyepi-template/6739968.psd");
    path.is_file().then_some(path)
}

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn proc_kib(name: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|line| line.starts_with(name))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse().ok())
        .unwrap_or(0)
}

fn new_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(crate::TILE_BUDGET) else {
        unreachable!("non-zero");
    };
    let store = ok(aurora_tile::TileStore::new(
        dir.path().to_path_buf(),
        budget,
    ));
    (dir, store)
}

/// The Nyepi open's breakdown: the decode thread's streamed open
/// (`PreparedPsd::stream`) split into the sink's encode and stage time and
/// the rest, the UI-thread install, and peak RSS. Run with
/// `cargo test -p aurora-app --release -- --ignored --nocapture nyepi_open_breakdown`.
#[test]
#[ignore = "measurement on the user's template"]
fn nyepi_open_breakdown() {
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    for run in 0..3 {
        let (_dir, mut store) = new_store();
        let base = proc_kib("VmRSS:");
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let started = Instant::now();
        let document = ok(crate::PreparedPsd::stream(
            &path,
            Some(&store.staging_root()),
        ));
        let wall = started.elapsed();
        let peak = proc_kib("VmHWM:").saturating_sub(base);
        let t = *document.timings;
        let crate::PreparedPsd {
            layers,
            pixels,
            masks,
            ..
        } = document;
        let started = Instant::now();
        let (_freed, failed) = crate::replace_document_pixels_prepared(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &layers,
            pixels,
            masks,
        );
        let install = started.elapsed();
        println!(
            "NYEPI open run {run} ({threads} hw threads): stream {:?}: waited for encode {:?} + stage {:?} \
             + the rest (read, parse, decode, overlapped encode) {:?}; {} tiles; peak RSS +{} MiB; UI install {:?}; failed {failed}",
            wall,
            t.encode,
            t.stage,
            wall.saturating_sub(t.encode + t.stage),
            t.tiles,
            peak / 1024,
            install,
        );
    }
}

/// FNV-1a 64 over every prepared tile, in order: layer, mask, tile id,
/// encoded length, encoded bytes. A staged tile's bytes are read back
/// from its file.
fn fingerprint(document: &crate::PreparedPsd) -> (u64, usize) {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut tiles = 0_usize;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for layer in &document.pixels {
        eat(&format!("L{:?}", layer.layer).into_bytes());
        match &layer.tiles {
            Ok(list) => {
                for (id, tile) in list {
                    tiles += 1;
                    eat(&id.x.to_le_bytes());
                    eat(&id.y.to_le_bytes());
                    eat(&(tile.len() as u64).to_le_bytes());
                    eat(&tile.bytes());
                }
            }
            Err(err) => eat(&format!("E{err:?}").into_bytes()),
        }
    }
    for mask in &document.masks {
        eat(&format!("M{:?}", mask.layer).into_bytes());
        for (id, tile) in &mask.tiles {
            tiles += 1;
            eat(&id.x.to_le_bytes());
            eat(&id.y.to_le_bytes());
            eat(&(tile.len() as u64).to_le_bytes());
            eat(&tile.bytes());
        }
    }
    (hash, tiles)
}

fn stream_fingerprint(path: &std::path::Path, threads: usize) -> (u64, usize) {
    let (_dir, store) = new_store();
    let document = ok(crate::PreparedPsd::stream_with_threads(
        path,
        Some(&store.staging_root()),
        threads,
    ));
    fingerprint(&document)
}

/// The tiles 0.188.0's `PreparedPsd::stream` produced for the Nyepi
/// template, fingerprinted by running this test's [`fingerprint`] on a
/// checkout of 0.188.0 (before the lookup-table decode, the zero-tile
/// reuse and the parallel encode): the new path must not change a byte.
const NYEPI_FINGERPRINT_0_188_0: (u64, usize) = (6_569_619_393_056_216_672, 4021);

#[test]
fn nyepi_tiles_are_the_0_188_0_tiles_on_any_thread_count() {
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let one = stream_fingerprint(&path, 1);
    assert_eq!(stream_fingerprint(&path, 7), one, "7 threads");
    assert_eq!(
        one, NYEPI_FINGERPRINT_0_188_0,
        "against the recorded 0.188.0 tiles"
    );
}

#[test]
fn a_noisy_psd_streams_to_the_same_tiles_on_any_thread_count() {
    for depth in [8_u16, 16] {
        let dir = ok(tempfile::tempdir());
        let path = dir.path().join("noisy.psd");
        ok(crate::tests::write_noisy_psd(&path, 600, depth));
        let one = stream_fingerprint(&path, 1);
        assert!(one.1 > 0);
        for threads in [2, 3, 8] {
            assert_eq!(
                stream_fingerprint(&path, threads),
                one,
                "{depth}-bit, {threads} threads"
            );
        }
    }
}

/// A layer image with zero, nearly-zero and noisy tiles, at an unaligned
/// offset: the pool path returns the sequential path's tiles in the
/// sequential path's order, for several pool sizes.
#[test]
fn the_pool_encodes_a_layers_tiles_in_the_sequential_order() {
    use aurora_tile::TILE;
    let side = TILE * 4 + 9;
    let mut samples = vec![half::f16::ZERO; side as usize * side as usize * 4];
    let mut seed = 7_u32;
    for (i, slot) in samples.iter_mut().enumerate() {
        // Noise in a band of rows and in one stray texel, zero elsewhere.
        let row = i / 4 / side as usize;
        if (TILE as usize..2 * TILE as usize).contains(&row) || i == 5 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *slot = half::f16::from_f32((seed >> 8) as f32 / (1_u32 << 24) as f32);
        }
    }
    let image = ok(aurora_io::Image::new(
        side,
        side,
        aurora_color::IccProfile::srgb(),
        samples,
    ));
    let sequential = ok(crate::prepared_pixels::encode_layer_tiles(
        None, &image, 11, 300,
    ));
    for threads in [2, 5] {
        let pool = crate::prepared_pixels::build_encode_pool(threads);
        assert!(pool.is_some());
        let pooled = ok(crate::prepared_pixels::encode_layer_tiles(
            pool.as_deref(),
            &image,
            11,
            300,
        ));
        assert_eq!(pooled.len(), sequential.len());
        for ((a, ta), (b, tb)) in pooled.iter().zip(&sequential) {
            assert_eq!(a, b);
            assert_eq!(ta.bytes(), tb.bytes(), "tile {a:?}");
        }
    }
    // A placement past the u32 range is refused on both paths.
    let pool = crate::prepared_pixels::build_encode_pool(2);
    assert!(
        crate::prepared_pixels::encode_layer_tiles(pool.as_deref(), &image, u32::MAX, 0).is_err()
    );
}

/// The fingerprint and the overlapped-layer count of one streamed open.
fn overlap_fingerprint(
    path: &std::path::Path,
    threads: usize,
    overlap: bool,
) -> ((u64, usize), usize) {
    let (_dir, store) = new_store();
    let document = ok(crate::PreparedPsd::stream_with_options(
        path,
        Some(&store.staging_root()),
        threads,
        overlap,
    ));
    (fingerprint(&document), document.timings.overlapped_layers)
}

/// 0.192.0: a small machine encodes each layer in place; the tiles are the
/// overlapped open's, byte for byte, and the overlap really is off.
#[test]
fn a_noisy_psd_streams_to_the_same_tiles_with_the_overlap_off() {
    for depth in [8_u16, 16] {
        let dir = ok(tempfile::tempdir());
        let path = dir.path().join("noisy.psd");
        ok(crate::tests::write_noisy_psd(&path, 600, depth));
        let (overlapped, overlapped_layers) = overlap_fingerprint(&path, 4, true);
        let (in_place, in_place_layers) = overlap_fingerprint(&path, 4, false);
        assert!(overlapped.1 > 0);
        // Four layers: each one that decodes while the last still encodes
        // is overlapped (all four are, the pool being there).
        assert_eq!(overlapped_layers, 4, "{depth}-bit: the overlap ran");
        assert_eq!(in_place_layers, 0, "{depth}-bit: the overlap is off");
        assert_eq!(in_place, overlapped, "{depth}-bit: tiles");
        // And against no pool at all, the old sequential open.
        assert_eq!(overlap_fingerprint(&path, 1, true).0, overlapped);
    }
}

/// `PreparedPsd::stream` — the open's real entry — follows the machine:
/// no overlap when it is small, the same tiles either way.
#[test]
fn the_real_open_overlaps_only_on_a_large_machine_and_never_changes_a_tile() {
    let dir = ok(tempfile::tempdir());
    let path = dir.path().join("noisy.psd");
    ok(crate::tests::write_noisy_psd(&path, 600, 16));
    let open = |small: bool| {
        let _guard = crate::small_machine::force_small_machine(small);
        let (_dir, store) = new_store();
        let document = ok(crate::PreparedPsd::stream(
            &path,
            Some(&store.staging_root()),
        ));
        (fingerprint(&document), document.timings.overlapped_layers)
    };
    let (small, small_overlapped) = open(true);
    let (large, _large_overlapped) = open(false);
    assert_eq!(small_overlapped, 0, "a small machine encodes in place");
    assert_eq!(small, large);
}

/// 0.192.0: the user's template opens to the recorded 0.188.0 tiles with
/// the overlap off, as on a small machine.
#[test]
fn nyepi_tiles_are_the_0_188_0_tiles_with_the_overlap_off() {
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let (in_place, overlapped_layers) = overlap_fingerprint(&path, 7, false);
    assert_eq!(overlapped_layers, 0);
    assert_eq!(in_place, NYEPI_FINGERPRINT_0_188_0);
}

/// Populating the Layers panel for the Nyepi template's ~223 layers in 9
/// nested groups (0.195.0, round L2): the per-row widgets must keep the
/// population well under one frame. Run with `cargo test -p aurora-app
/// --release -- --ignored --nocapture nyepi_layers_panel_populate`.
#[test]
#[ignore = "measurement on the user's template"]
fn nyepi_layers_panel_populate() {
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let (_dir, store) = new_store();
    let document = ok(crate::PreparedPsd::stream(
        &path,
        Some(&store.staging_root()),
    ));
    let layers = &document.layers;
    let scales = ok(crate::load_scales());
    let mut workspace = aurora_ui::build_workspace(&scales);
    let mut best = std::time::Duration::MAX;
    let mut rows = 0;
    for _ in 0..7 {
        let started = Instant::now();
        let map = ok(aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            layers,
        ));
        let elapsed = started.elapsed();
        rows = map.len();
        best = best.min(elapsed);
    }
    workspace.tree.compute_layout(1600.0, 900.0);
    let started = Instant::now();
    workspace.tree.compute_layout(1600.0, 900.0);
    let layout = started.elapsed();
    println!(
        "NYEPI layers panel: {rows} rows, populate best of 7 {best:?}, full relayout {layout:?}, {} widgets",
        workspace.tree.len()
    );
}

/// Re-filtering the Layers panel on the Nyepi template (0.199.0, round
/// L5b): the cost of one keystroke (a full filtered population) for a
/// typical search, a search matching nothing and the empty search. Run
/// with `cargo test -p aurora-app --release -- --ignored --nocapture
/// nyepi_layers_filter`.
#[test]
#[ignore = "measurement on the user's template"]
fn nyepi_layers_filter() {
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let (_dir, store) = new_store();
    let document = ok(crate::PreparedPsd::stream(
        &path,
        Some(&store.staging_root()),
    ));
    let layers = &document.layers;
    let scales = ok(crate::load_scales());
    let mut workspace = aurora_ui::build_workspace(&scales);
    for text in ["post", "zzzz", "", "p"] {
        workspace.layer_view.filter = aurora_ui::LayerFilter {
            text: text.to_owned(),
            ..aurora_ui::LayerFilter::default()
        };
        let mut best = std::time::Duration::MAX;
        let mut rows = 0;
        for _ in 0..7 {
            let started = Instant::now();
            let map = ok(aurora_ui::populate_layers_panel_view(
                &mut workspace.tree,
                workspace.layers,
                &scales,
                layers,
                &mut workspace.layer_view,
            ));
            best = best.min(started.elapsed());
            rows = map.len();
        }
        println!("NYEPI filter {text:?}: {rows} rows, populate best of 7 {best:?}");
    }
}

/// Layer thumbnails on the Nyepi template (0.197.0, round L4): the first
/// visible rows of the Layers panel, timed on the UI thread and in total,
/// with the tiles shared, the store's paging counters and the RSS change.
/// Run with `cargo test -p aurora-app --release -- --ignored --nocapture
/// nyepi_layer_thumbnails`.
#[test]
#[ignore = "measurement on the user's template"]
fn nyepi_layer_thumbnails() {
    use crate::layer_thumbnails::LayerThumbnails;
    let Some(path) = nyepi_path() else {
        println!("SKIPPED: the Nyepi template is not present");
        return;
    };
    let (_dir, mut store) = new_store();
    let document = ok(crate::PreparedPsd::stream(
        &path,
        Some(&store.staging_root()),
    ));
    let crate::PreparedPsd {
        layers,
        pixels,
        masks,
        ..
    } = document;
    let (_freed, failed) = crate::replace_document_pixels_prepared(
        &mut store,
        aurora_doc::LayerTree::new(),
        aurora_doc::History::new(),
        &layers,
        pixels,
        masks,
    );
    assert_eq!(failed, 0);
    let scales = ok(crate::load_scales());
    let mut workspace = aurora_ui::build_workspace(&scales);
    let rows = ok(aurora_ui::populate_layers_panel(
        &mut workspace.tree,
        workspace.layers,
        &scales,
        &layers,
    ));
    workspace.tree.compute_layout(1600.0, 900.0);
    let visible =
        aurora_ui::visible_layer_rows(&workspace.tree, workspace.layers, &rows, &scales, 2);
    let ids: Vec<aurora_doc::LayerId> = visible.iter().map(|&(_, id)| id).collect();
    let service_doc = (crate::document_session::DocumentId::next(), store.serial());
    let mut service = LayerThumbnails::new(std::sync::Arc::new(|| {}));
    let before = (
        store.resident_len(),
        store.stats().faults,
        store.stats().pending_hits,
        proc_kib("VmRSS:"),
    );
    let started = Instant::now();
    let mut ui_time = std::time::Duration::ZERO;
    let mut now = Instant::now();
    loop {
        let t = Instant::now();
        let _ = service.update(service_doc, &layers, &mut store, &ids, now);
        ui_time += t.elapsed();
        let t = Instant::now();
        let _ = service.drain();
        let changed = service.apply(&mut workspace.tree, &visible);
        ui_time += t.elapsed();
        let _ = changed;
        if service.pending_len() == 0
            && service.stats.jobs_submitted
                >= ids.len().min(
                    ids.iter()
                        .filter(|&&id| {
                            matches!(layers.kind(id), Some(aurora_doc::LayerKind::Pixel { .. }))
                        })
                        .count(),
                )
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        now += std::time::Duration::from_millis(1);
    }
    let total = started.elapsed();
    let stats = store.stats();
    let total_layers = layers.len();
    println!(
        "NYEPI thumbnails: {} visible rows ({} layers in the document), {} jobs, {} tiles shared \
         (<= {} per layer); UI-thread time {:?}, wall {:?}; resident tiles {} -> {}; faults +{}, \
         pending hits +{}; RSS {} -> {} MiB; cache {} pictures",
        visible.len(),
        total_layers,
        service.stats.jobs_submitted,
        service.stats.tiles_shared,
        crate::layer_thumbnails::MAX_TILES,
        ui_time,
        total,
        before.0,
        store.resident_len(),
        stats.faults - before.1,
        stats.pending_hits - before.2,
        before.3 / 1024,
        proc_kib("VmRSS:") / 1024,
        service.cache().len(),
    );
}
