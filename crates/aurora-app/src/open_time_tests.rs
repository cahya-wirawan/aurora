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
