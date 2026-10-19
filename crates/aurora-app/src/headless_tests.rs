//! Tests of the headless timed run (`headless.rs`).

#![allow(clippy::panic, clippy::cast_precision_loss)]

use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// A 300 x 200 RGBA picture with ramps and a varying alpha, written as
/// an 8-bit PNG by Aurora's own encoder.
fn write_test_png(dir: &Path) -> PathBuf {
    let (w, h) = (300_u32, 200_u32);
    let mut samples = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let a = 0.25 + 0.75 * (x as f32 / w as f32);
            samples.push(half::f16::from_f32((x as f32 / w as f32) * a));
            samples.push(half::f16::from_f32((y as f32 / h as f32) * a));
            samples.push(half::f16::from_f32(0.5 * a));
            samples.push(half::f16::from_f32(a));
        }
    }
    let image = aurora_io::Image::new(w, h, aurora_color::IccProfile::srgb(), samples)
        .unwrap_or_else(|e| panic!("test image: {e:?}"));
    let bytes = aurora_io::png::encode(&image).unwrap_or_else(|e| panic!("encode: {e:?}"));
    let path = dir.join("input.png");
    std::fs::write(&path, bytes).unwrap_or_else(|e| panic!("write: {e}"));
    path
}

fn options(input: &Path, output: &Path, scratch: &Path, mode: CompositeMode) -> Options {
    Options {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        scratch: scratch.to_path_buf(),
        threads: 2,
        mode,
        depth16: false,
        reference_out: None,
    }
}

fn decode(path: &Path) -> aurora_io::Image {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    aurora_io::decode_by_extension(path, &bytes).unwrap_or_else(|e| panic!("decode: {e:?}"))
}

fn max_level_difference(a: &aurora_io::Image, b: &aurora_io::Image) -> f32 {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    a.samples()
        .iter()
        .zip(b.samples())
        .map(|(x, y)| (x.to_f32() - y.to_f32()).abs() * 255.0)
        .fold(0.0, f32::max)
}

#[test]
fn parse_args_reads_every_flag_and_refuses_the_rest() {
    let o = parse_args(&args(&[
        "in.psd",
        "--out",
        "o.png",
        "--scratch",
        "s",
        "--threads",
        "3",
        "--composite",
        "pool",
        "--depth",
        "16",
        "--reference-out",
        "r.png",
    ]))
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(o.input, PathBuf::from("in.psd"));
    assert_eq!(o.output, PathBuf::from("o.png"));
    assert_eq!(o.scratch, PathBuf::from("s"));
    assert_eq!(o.threads, 3);
    assert_eq!(o.mode, CompositeMode::Pool);
    assert!(o.depth16);
    assert_eq!(o.reference_out, Some(PathBuf::from("r.png")));
    let gpu =
        parse_args(&args(&["in.psd", "--out", "o.png", "--gpu"])).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(gpu.mode, CompositeMode::Gpu);
    assert!(parse_args(&args(&["in.psd"])).is_err(), "--out is required");
    assert!(
        parse_args(&args(&[
            "in.psd",
            "--out",
            "o.png",
            "--gpu",
            "--composite",
            "pool"
        ]))
        .is_err(),
        "--gpu overriding --composite pool would be silent"
    );
    assert!(parse_args(&args(&["in.psd", "--out", "o.tif", "--depth", "16"])).is_err());
    assert!(parse_args(&args(&["in.psd", "--out", "o.png", "--threads", "0"])).is_err());
    assert!(parse_args(&args(&["in.psd", "--out", "o.png", "--fromat", "png"])).is_err());
    assert!(parse_args(&args(&["a.psd", "b.psd", "--out", "o.png"])).is_err());
}

#[test]
fn a_png_round_trips_through_the_export_composite_and_the_scratch_is_removed() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let input = write_test_png(dir.path());
    let output = dir.path().join("out.png");
    let scratch = dir.path().join("scratch");
    let o = options(&input, &output, &scratch, CompositeMode::Export);
    let result = run(&o);
    let report = result.as_ref().unwrap_or_else(|f| panic!("{f:?}"));
    assert_eq!((report.width, report.height), (300, 200));
    assert_eq!(report.format, "image");
    assert_eq!(report.doc_tiles, 2, "300 x 200 is two 256 px tiles");
    assert_eq!(report.cpu_tiles, 2);
    assert!(report.output_bytes > 0);
    // The picture survives: 8-bit dither moves a sample at most one level,
    // plus the f16 rounding of the premultiplied decode.
    let max = max_level_difference(&decode(&input), &decode(&output));
    assert!(max <= 1.5, "export differs from the input by {max} levels");
    // This run's own scratch directory is gone, its parent stays.
    let leftovers: Vec<_> = std::fs::read_dir(&scratch)
        .unwrap_or_else(|e| panic!("{e}"))
        .collect();
    assert!(leftovers.is_empty(), "scratch left behind: {leftovers:?}");
    let json = to_json(&o, &result);
    assert!(json.starts_with("{\"tool\":\"aurora\""), "{json}");
    assert!(json.contains("\"ok\":true"), "{json}");
    assert!(json.contains("\"composite_mode\":\"export\""), "{json}");
    assert!(json.contains("\"threads\":2"), "{json}");
    assert!(json.contains("\"ms\":{\"decode\":"), "{json}");
    // An sRGB-tagged PNG is not converted at open.
    assert!(report.colour_converted_from.is_none());
    assert!(json.contains("\"colour_converted_from\":null"), "{json}");
    assert!(json.ends_with('}'));
}

#[test]
fn the_pool_composite_is_bit_identical_to_the_export_composite() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let input = write_test_png(dir.path());
    let a = dir.path().join("export.png");
    let b = dir.path().join("pool.png");
    let scratch = dir.path().join("scratch");
    run(&options(&input, &a, &scratch, CompositeMode::Export)).unwrap_or_else(|f| panic!("{f:?}"));
    run(&options(&input, &b, &scratch, CompositeMode::Pool)).unwrap_or_else(|f| panic!("{f:?}"));
    let (a, b) = (std::fs::read(a), std::fs::read(b));
    assert!(
        a.is_ok() && a.ok() == b.ok(),
        "pool and export PNG bytes differ"
    );
}

/// A layered corpus PSD (groups, blend modes): the pool composite equals
/// the export composite byte for byte, and the reference is written.
#[test]
fn a_layered_psd_composites_identically_on_the_pool_and_writes_its_reference() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/blend-modes/rgb-blend-modes.psd");
    if !path.exists() {
        crate::open_time_tests::corpus_file_absent(&path);
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let scratch = dir.path().join("scratch");
    let a = dir.path().join("export.png");
    let b = dir.path().join("pool.png");
    let reference = dir.path().join("reference.png");
    let mut o = options(&path, &a, &scratch, CompositeMode::Export);
    o.reference_out = Some(reference.clone());
    let report = run(&o).unwrap_or_else(|f| panic!("{f:?}"));
    assert_eq!(report.format, "psd");
    assert!(report.layers > 1, "a layered document: {report:?}");
    assert!(report.layer_tiles.is_some_and(|t| t > 0));
    assert!(report.reference_written);
    assert_eq!(
        (decode(&reference).width(), decode(&reference).height()),
        (report.width, report.height)
    );
    let mut o = options(&path, &b, &scratch, CompositeMode::Pool);
    o.threads = 3;
    run(&o).unwrap_or_else(|f| panic!("{f:?}"));
    let (a, b) = (std::fs::read(a), std::fs::read(b));
    assert!(
        a.is_ok() && a.ok() == b.ok(),
        "pool and export PNG bytes differ"
    );
}

/// The GPU mode composites a plain one-layer document on the GPU and
/// matches the export composite within the GPU path's rounding.
#[test]
fn the_gpu_mode_uses_the_gpu_and_matches_the_export_composite() {
    if aurora_gpu::test_support::real_context_or_skip().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let input = write_test_png(dir.path());
    let scratch = dir.path().join("scratch");
    let a = dir.path().join("export.png");
    let b = dir.path().join("gpu.png");
    run(&options(&input, &a, &scratch, CompositeMode::Export)).unwrap_or_else(|f| panic!("{f:?}"));
    let report =
        run(&options(&input, &b, &scratch, CompositeMode::Gpu)).unwrap_or_else(|f| panic!("{f:?}"));
    assert_eq!(report.gpu_tiles, 2, "{report:?}");
    assert_eq!(report.cpu_tiles, 0);
    assert!(report.gpu_adapter.is_some());
    let max = max_level_difference(&decode(&a), &decode(&b));
    assert!(max <= 1.01, "GPU and export differ by {max} levels");
}

#[test]
fn a_missing_file_fails_in_the_open_phase_with_json() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let o = options(
        &dir.path().join("absent.psd"),
        &dir.path().join("o.png"),
        &dir.path().join("scratch"),
        CompositeMode::Export,
    );
    let result = run(&o);
    let failure = result
        .as_ref()
        .err()
        .unwrap_or_else(|| panic!("opened a missing file"));
    assert_eq!(failure.phase, "open");
    let json = to_json(&o, &result);
    assert!(json.contains("\"ok\":false,\"phase\":\"open\""), "{json}");
    assert!(!dir.path().join("o.png").exists());
}

/// A source tile that cannot be read back fails the pool composite (it
/// never exports a picture with a hole), like the export composite.
#[test]
fn the_pool_composite_fails_on_an_unreadable_source_tile() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let budget = std::num::NonZeroUsize::new(1).unwrap_or_else(|| panic!("1 is non-zero"));
    let mut store = aurora_tile::TileStore::new(dir.path().to_path_buf(), budget)
        .unwrap_or_else(|e| panic!("{e:?}"));
    let (layers, _history, layer) = crate::document_from_size("t", (256, 256));
    let surface = layers
        .surface_id(layer)
        .unwrap_or_else(|| panic!("a pixel layer"));
    let tile = aurora_tile::TileId { x: 0, y: 0 };
    let texels = store
        .get_mut(surface, tile)
        .unwrap_or_else(|e| panic!("{e:?}"));
    texels.texels_mut().fill(half::f16::from_f32(1.0));
    // Budget 1: touching another surface evicts the layer's tile to disk.
    store
        .get_mut(aurora_tile::SurfaceId::from_raw(0x7777), tile)
        .unwrap_or_else(|e| panic!("{e:?}"));
    store.flush().unwrap_or_else(|e| panic!("{e:?}"));
    let mut damaged = 0;
    for entry in std::fs::read_dir(dir.path())
        .unwrap_or_else(|e| panic!("{e}"))
        .flatten()
    {
        if entry.path().extension().is_some_and(|e| e == "tile") {
            std::fs::write(entry.path(), b"not a tile").unwrap_or_else(|e| panic!("{e}"));
            damaged += 1;
        }
    }
    assert!(
        damaged >= 1,
        "the layer's tile was evicted to a scratch file"
    );
    let failure = composite_on_pool(&layers, store, (256, 256), 2)
        .err()
        .unwrap_or_else(|| panic!("composited a document with an unreadable tile"));
    assert_eq!(failure.phase, "composite");
    assert!(
        failure.message.contains("could not be read"),
        "{}",
        failure.message
    );
}
