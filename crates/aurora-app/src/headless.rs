//! A timed, headless open -> composite -> export run (the
//! `aurora-headless` binary, `src/bin/aurora-headless.rs`).
//!
//! It exists to measure Aurora from the outside on the same files as
//! another editor (`spike/photocraft-compare/`), so it runs the app's
//! own code, not a reimplementation:
//!
//! * **open**: [`crate::decode_chosen_file_with`]'s routes (for a PSD/PSB
//!   [`PreparedPsd::stream_with_options`], the streamed open with its
//!   staging area, given `--threads` as its encode-pool size), then the
//!   install's store half, [`crate::replace_document_pixels_prepared`];
//!   a `.aur` goes through [`crate::precheck_aur`] and
//!   [`crate::read_prechecked_aur`] as in the app;
//! * **composite**: `export` (the default) is [`crate::composite_document`],
//!   exactly what Save As PNG/JPEG/TIFF composites with (one thread);
//!   `pool` folds the same tiles on a rayon pool of `--threads` threads
//!   with the background compositor's per-tile code (`SharedMirror` +
//!   [`crate::composite_roots_into_tile`]); `--gpu` composites each tile
//!   with the canvas's GPU path ([`crate::begin_gpu_composite_tile`]) and
//!   folds on the CPU where that path refuses, as the canvas does;
//! * **export**: `colour_app::export_image` (no output profile, as after
//!   an open), [`aurora_io::encode_by_extension`] (8-bit, dithered; or
//!   `--depth 16` for a 16-bit PNG through `aurora_io::png::encode_16`),
//!   then [`crate::export_save::write_export_verified`].
//!
//! One JSON object goes to stdout; logs go to stderr. Nothing here is
//! reachable from the window: the app never calls [`main_from_args`].

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::background_open::DecodedFile;
use crate::prepared_pixels::{PreparedLayer, PreparedPsd};

/// How the document is composited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositeMode {
    /// The app's export composite (`composite_document`), one thread.
    Export,
    /// The background compositor's per-tile fold on a pool.
    Pool,
    /// The canvas's GPU tile path, CPU fold where it refuses.
    Gpu,
}

impl CompositeMode {
    fn name(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Pool => "pool",
            Self::Gpu => "gpu",
        }
    }
}

/// One run's options.
#[derive(Clone, Debug)]
pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub scratch: PathBuf,
    pub threads: usize,
    pub mode: CompositeMode,
    pub depth16: bool,
    pub reference_out: Option<PathBuf>,
}

/// Per-phase wall times and counts of one run.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub layers: usize,
    pub decode: Duration,
    pub install: Duration,
    pub composite: Duration,
    pub encode: Duration,
    pub write: Duration,
    pub total: Duration,
    pub layer_tiles: Option<usize>,
    pub doc_tiles: usize,
    pub gpu_tiles: usize,
    pub cpu_tiles: usize,
    pub gpu_adapter: Option<String>,
    pub psd_encode_ms: Option<f64>,
    pub psd_stage_ms: Option<f64>,
    pub open_notes: usize,
    pub reference_written: bool,
    /// A second reference, the merged image in the file's own profile
    /// (`<reference>.stored.png`), written when that profile is not sRGB.
    pub reference_stored_written: bool,
    pub output_bytes: usize,
}

const USAGE: &str = "usage: aurora-headless <input> --out <file.png|.tif|.jpg> \
[--scratch DIR] [--threads N] [--composite export|pool] [--gpu|--cpu] \
[--depth 8|16] [--reference-out ref.png]";

/// Parses `args` (without the program name).
///
/// # Errors
///
/// A usage message when an argument is missing, unknown or malformed.
pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut scratch: Option<PathBuf> = None;
    let mut threads = crate::background_composite::composite_threads();
    let mut mode = CompositeMode::Export;
    let mut gpu = false;
    let mut depth16 = false;
    let mut reference_out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--out" => output = Some(PathBuf::from(value()?)),
            "--scratch" => scratch = Some(PathBuf::from(value()?)),
            "--threads" => {
                threads = value()?
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| format!("--threads needs a whole number >= 1\n{USAGE}"))?;
            }
            "--composite" => {
                mode = match value()?.as_str() {
                    "export" => CompositeMode::Export,
                    "pool" => CompositeMode::Pool,
                    other => return Err(format!("unknown composite mode {other}\n{USAGE}")),
                };
            }
            "--gpu" => gpu = true,
            "--cpu" => gpu = false,
            "--depth" => {
                depth16 = match value()?.as_str() {
                    "8" => false,
                    "16" => true,
                    other => return Err(format!("unknown depth {other}\n{USAGE}")),
                };
            }
            "--reference-out" => reference_out = Some(PathBuf::from(value()?)),
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag {other}\n{USAGE}"));
            }
            other => {
                if input.is_some() {
                    return Err(format!("only one input file\n{USAGE}"));
                }
                input = Some(PathBuf::from(other));
            }
        }
    }
    let input = input.ok_or_else(|| USAGE.to_owned())?;
    let output = output.ok_or_else(|| format!("--out is required\n{USAGE}"))?;
    if gpu {
        mode = CompositeMode::Gpu;
    }
    let is_png = output
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if depth16 && !is_png {
        return Err(format!(
            "--depth 16 needs a .png output (Aurora has no 16-bit TIFF/JPEG encoder)\n{USAGE}"
        ));
    }
    Ok(Options {
        input,
        output,
        scratch: scratch.unwrap_or_else(std::env::temp_dir),
        threads,
        mode,
        depth16,
        reference_out,
    })
}

/// A failed phase and why.
#[derive(Debug)]
pub struct Failure {
    pub phase: &'static str,
    pub message: String,
}

fn fail(phase: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        phase,
        message: message.into(),
    }
}

/// A scratch directory of this run's own, removed when dropped.
struct RunScratch(PathBuf);

impl RunScratch {
    fn create(parent: &Path) -> Result<Self, Failure> {
        std::fs::create_dir_all(parent)
            .map_err(|e| fail("scratch", format!("{}: {e}", parent.display())))?;
        let dir = parent.join(format!("aurora-headless-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| fail("scratch", format!("{}: {e}", dir.display())))?;
        }
        std::fs::create_dir(&dir)
            .map_err(|e| fail("scratch", format!("{}: {e}", dir.display())))?;
        Ok(Self(dir))
    }
}

impl Drop for RunScratch {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(dir = %self.0.display(), %err, "couldn't remove the scratch directory");
        }
    }
}

/// The opened document: its tree and canvas size.
struct Opened {
    layers: aurora_doc::LayerTree,
    size: (u32, u32),
}

fn open_document(
    options: &Options,
    store: &mut aurora_tile::TileStore,
    report: &mut Report,
) -> Result<Opened, Failure> {
    let path = options.input.as_path();
    let staging = store.staging_root();
    let started = Instant::now();
    // The same routes `decode_chosen_file_with` takes; a PSD gets the
    // requested encode-pool size instead of `composite_threads()`.
    let decoded = match crate::open_route(path) {
        crate::OpenRoute::Psd => {
            "psd".clone_into(&mut report.format);
            PreparedPsd::stream_with_options(
                path,
                Some(&staging),
                options.threads,
                crate::small_machine::overlap_allowed(),
            )
            .map(DecodedFile::Psd)
        }
        crate::OpenRoute::Aur => {
            "aur".clone_into(&mut report.format);
            crate::decode_chosen_file_with(path, Some(&staging))
        }
        crate::OpenRoute::Image => {
            "image".clone_into(&mut report.format);
            crate::decode_chosen_file_with(path, Some(&staging))
        }
    }
    .map_err(|e| fail("open", format!("{e:?}")))?;
    report.decode = started.elapsed();
    let started = Instant::now();
    let opened = match decoded {
        DecodedFile::Psd(psd) => {
            let PreparedPsd {
                layers,
                history,
                canvas_size,
                pixels,
                masks,
                report: psd_report,
                timings,
                ..
            } = psd;
            report.layer_tiles = Some(timings.tiles);
            report.psd_encode_ms = Some(ms(timings.encode));
            report.psd_stage_ms = Some(ms(timings.stage));
            report.open_notes = psd_report.items.len();
            drop(history);
            let (_freed, failed) = crate::replace_document_pixels_prepared(
                store,
                aurora_doc::LayerTree::new(),
                aurora_doc::History::new(),
                &layers,
                pixels,
                masks,
            );
            if failed > 0 {
                return Err(fail(
                    "install",
                    format!("{failed} tiles could not be written"),
                ));
            }
            Opened {
                layers,
                size: canvas_size,
            }
        }
        DecodedFile::Image(image) => {
            let name = path
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("Image");
            let size = image.size;
            report.open_notes = image.colour.report.warnings.len();
            let (layers, _history, layer_id) = crate::document_from_size(name, size);
            let (_freed, failed) = crate::replace_document_pixels_prepared(
                store,
                aurora_doc::LayerTree::new(),
                aurora_doc::History::new(),
                &layers,
                vec![PreparedLayer {
                    layer: layer_id,
                    tiles: image.tiles,
                }],
                Vec::new(),
            );
            if failed > 0 {
                return Err(fail(
                    "install",
                    format!("{failed} tiles could not be written"),
                ));
            }
            Opened { layers, size }
        }
        DecodedFile::Aur(bytes) => {
            let document = crate::read_prechecked_aur(&bytes, store)
                .map_err(|e| fail("install", format!("{e:?}")))?;
            Opened {
                layers: document.layers,
                size: document.canvas_size,
            }
        }
    };
    report.install = started.elapsed();
    Ok(opened)
}

/// Every document tile, row by row.
fn document_tiles(width: u32, height: u32) -> Vec<aurora_tile::TileId> {
    let tile = aurora_tile::TILE;
    let mut tiles = Vec::new();
    for y in 0..height.div_ceil(tile) {
        for x in 0..width.div_ceil(tile) {
            tiles.push(aurora_tile::TileId { x, y });
        }
    }
    tiles
}

/// Copies one composited tile into the flat image buffer.
fn place_tile(
    samples: &mut [half::f16],
    (width, height): (u32, u32),
    tile_id: aurora_tile::TileId,
    texels: &[half::f16],
) {
    let tile = aurora_tile::TILE;
    let channels = aurora_tile::CHANNELS;
    let origin_x = tile_id.x * tile;
    let origin_y = tile_id.y * tile;
    let w = width.saturating_sub(origin_x).min(tile) as usize;
    let h = height.saturating_sub(origin_y).min(tile);
    for ly in 0..h {
        let dst = ((origin_y + ly) as usize * width as usize + origin_x as usize) * channels;
        let src = (ly * tile) as usize * channels;
        if let (Some(d), Some(s)) = (
            samples.get_mut(dst..dst + w * channels),
            texels.get(src..src + w * channels),
        ) {
            d.copy_from_slice(s);
        }
    }
}

fn image_from(
    width: u32,
    height: u32,
    samples: Vec<half::f16>,
) -> Result<aurora_io::Image, Failure> {
    aurora_io::Image::new(width, height, aurora_color::IccProfile::srgb(), samples)
        .map_err(|e| fail("composite", format!("{e:?}")))
}

/// The background compositor's per-tile fold, on a pool of `threads`.
fn composite_on_pool(
    layers: &aurora_doc::LayerTree,
    store: aurora_tile::TileStore,
    size: (u32, u32),
    threads: usize,
) -> Result<(aurora_io::Image, aurora_tile::TileStore), Failure> {
    use rayon::prelude::*;
    let (width, height) = size;
    let tiles = document_tiles(width, height);
    let shadows = crate::new_shared_shadow_cache(crate::small_machine::shadow_cache_bytes());
    let pool = crate::background_composite::build_pool(threads, &shadows)
        .ok_or_else(|| fail("composite", "couldn't start the thread pool"))?;
    let mirror = Mutex::new(store);
    let serial = crate::background_composite::lock(&mirror).serial();
    let results: Vec<(aurora_tile::TileId, Vec<half::f16>)> = pool.install(|| {
        tiles
            .par_iter()
            .with_max_len(1)
            .map(|&tile_id| {
                let mut source = crate::background_composite::SharedMirror::new(&mirror, serial);
                let doc_origin = crate::composite_tile_doc_origin((0, 0), tile_id);
                let mut budget = crate::CompositeBudget::for_tile_of_pass(layers);
                let (texels, _folded) = crate::composite_roots_into_tile(
                    layers,
                    &mut source,
                    tile_id,
                    doc_origin,
                    (0, 0),
                    &mut budget,
                );
                source.finish();
                (tile_id, texels)
            })
            .collect()
    });
    let store = mirror
        .into_inner()
        .map_err(|_| fail("composite", "the tile store's lock was poisoned"))?;
    let mut samples =
        vec![half::f16::from_f32(0.0); width as usize * height as usize * aurora_tile::CHANNELS];
    for (tile_id, texels) in results {
        place_tile(&mut samples, size, tile_id, &texels);
    }
    Ok((image_from(width, height, samples)?, store))
}

/// The canvas's GPU tile path; tiles it refuses fold on the CPU.
fn composite_on_gpu(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    size: (u32, u32),
    report: &mut Report,
) -> Result<aurora_io::Image, Failure> {
    // At most this many readbacks (256 x 256 x 8 bytes each) wait at once.
    const BATCH: usize = 64;
    let gpu = aurora_gpu::GpuContext::new().map_err(|e| fail("gpu", format!("{e:?}")))?;
    let info = gpu.adapter_info();
    report.gpu_adapter = Some(format!(
        "{} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    ));
    let mut compositor = aurora_render::TileCompositor::new(gpu.device());
    let qualifies = crate::document_qualifies_for_gpu_compositing(layers);
    let (width, height) = size;
    let mut samples =
        vec![half::f16::from_f32(0.0); width as usize * height as usize * aurora_tile::CHANNELS];
    let mut budget = crate::CompositeBudget::for_pass(layers);
    let tiles = document_tiles(width, height);
    for chunk in tiles.chunks(BATCH) {
        let mut pending = Vec::new();
        for &tile_id in chunk {
            let doc_origin = crate::composite_tile_doc_origin((0, 0), tile_id);
            budget.next_tile(layers);
            let issued = if qualifies {
                crate::begin_gpu_composite_tile(
                    &gpu,
                    &mut compositor,
                    layers,
                    store,
                    tile_id,
                    doc_origin,
                    (0, 0),
                    &mut budget,
                )
            } else {
                None
            };
            if let Some(p) = issued {
                pending.push(p);
            } else {
                let (texels, _) = crate::composite_roots_into_tile(
                    layers,
                    store,
                    tile_id,
                    doc_origin,
                    (0, 0),
                    &mut budget,
                );
                place_tile(&mut samples, size, tile_id, &texels);
                report.cpu_tiles += 1;
            }
        }
        if !pending.is_empty() {
            let _ = gpu.device().poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
        }
        for p in pending {
            let tile_id = p.tile_id;
            if let Some(texels) = crate::finish_tile_readback(p) {
                place_tile(&mut samples, size, tile_id, &texels);
                report.gpu_tiles += 1;
            } else {
                let doc_origin = crate::composite_tile_doc_origin((0, 0), tile_id);
                budget.next_tile(layers);
                let (texels, _) = crate::composite_roots_into_tile(
                    layers,
                    store,
                    tile_id,
                    doc_origin,
                    (0, 0),
                    &mut budget,
                );
                place_tile(&mut samples, size, tile_id, &texels);
                report.cpu_tiles += 1;
            }
        }
    }
    if let Some((skipped, _first)) = budget.store_error() {
        return Err(fail(
            "composite",
            format!("{skipped} source tiles could not be read"),
        ));
    }
    image_from(width, height, samples)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

/// Opens, composites and exports `options.input`.
///
/// # Errors
///
/// The phase that failed and why; the scratch directory is removed
/// either way.
pub fn run(options: &Options) -> Result<Report, Failure> {
    let total = Instant::now();
    let mut report = Report::default();
    let scratch = RunScratch::create(&options.scratch)?;
    let mut store = aurora_tile::TileStore::new_shared(scratch.0.clone(), crate::tile_pool())
        .map_err(|e| fail("scratch", format!("tile store: {e:?}")))?;
    let opened = open_document(options, &mut store, &mut report)?;
    let (width, height) = opened.size;
    report.width = width;
    report.height = height;
    report.layers = opened.layers.len();
    report.doc_tiles = document_tiles(width, height).len();

    let started = Instant::now();
    let image = match options.mode {
        CompositeMode::Export => {
            report.cpu_tiles = report.doc_tiles;
            crate::composite_document(&opened.layers, &mut store, width, height)
                .map_err(|e| fail("composite", format!("{e:?}")))?
        }
        CompositeMode::Pool => {
            report.cpu_tiles = report.doc_tiles;
            let (image, back) =
                composite_on_pool(&opened.layers, store, opened.size, options.threads)?;
            store = back;
            image
        }
        CompositeMode::Gpu => {
            composite_on_gpu(&opened.layers, &mut store, opened.size, &mut report)?
        }
    };
    report.composite = started.elapsed();
    drop(store);

    let started = Instant::now();
    let image = match crate::colour_app::export_image(image, None) {
        Ok(image) | Err(image) => image,
    };
    let bytes = if options.depth16 {
        aurora_io::png::encode_16(&image)
    } else {
        aurora_io::encode_by_extension(&options.output, &image)
    }
    .map_err(|e| fail("encode", format!("{e:?}")))?;
    report.encode = started.elapsed();
    report.output_bytes = bytes.len();
    let started = Instant::now();
    crate::export_save::write_export_verified(
        &options.output,
        &bytes,
        image.width(),
        image.height(),
    )
    .map_err(|e| fail("write", format!("{e:?}")))?;
    report.write = started.elapsed();
    drop(bytes);
    drop(image);
    report.total = total.elapsed();

    if let (Some(reference), "psd") = (&options.reference_out, report.format.as_str()) {
        // The audit's reference: the file's own merged image, in the
        // working space (psd_audit_tests.rs), and beside it, when the
        // file carries another profile, the merged image as stored (for
        // a tool that keeps the source profile). Outside the timed phases.
        let bytes = std::fs::read(&options.input).map_err(|e| fail("reference", e.to_string()))?;
        let write = |path: &Path, image: Result<aurora_io::Image, aurora_io::IoError>| {
            let png = image
                .and_then(|img| aurora_io::png::encode_16(&img))
                .map_err(|e| fail("reference", format!("{e:?}")))?;
            std::fs::write(path, png).map_err(|e| fail("reference", e.to_string()))
        };
        let working = aurora_io::decode_psd_merged_image_in_working_space(&bytes);
        // The merged image as stored differs from the working-space one
        // exactly when the open converts the file's colours.
        let stored = aurora_io::decode_psd_merged_image(&bytes)
            .ok()
            .filter(|stored| {
                working
                    .as_ref()
                    .is_ok_and(|working| working.samples() != stored.samples())
            });
        match write(reference, working) {
            Ok(()) => report.reference_written = true,
            Err(failure) => tracing::warn!(message = %failure.message, "no merged image"),
        }
        if let Some(image) = stored {
            let raw = reference.with_extension("stored.png");
            match write(&raw, Ok(image)) {
                Ok(()) => report.reference_stored_written = true,
                Err(failure) => {
                    tracing::warn!(message = %failure.message, "no stored merged image");
                }
            }
        }
    }
    drop(scratch);
    Ok(report)
}

/// This process's peak resident set (Linux `VmHWM`), in KiB.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn opt_num<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

/// The run's one JSON object.
#[must_use]
pub fn to_json(options: &Options, result: &Result<Report, Failure>) -> String {
    let mut o = String::from("{\"tool\":\"aurora\",\"version\":");
    json_string(&mut o, env!("CARGO_PKG_VERSION"));
    o.push_str(",\"input\":");
    json_string(&mut o, &options.input.display().to_string());
    o.push_str(",\"output\":");
    json_string(&mut o, &options.output.display().to_string());
    let _ = write!(
        o,
        ",\"composite_mode\":\"{}\",\"threads\":{},\"depth\":{}",
        options.mode.name(),
        options.threads,
        if options.depth16 { 16 } else { 8 }
    );
    match result {
        Ok(r) => {
            o.push_str(",\"ok\":true,\"format\":");
            json_string(&mut o, &r.format);
            let _ = write!(
                o,
                ",\"width\":{},\"height\":{},\"layers\":{},\"doc_tiles\":{},\"layer_tiles\":{},\
\"gpu_tiles\":{},\"cpu_tiles\":{},\"open_notes\":{},\"output_bytes\":{},\
\"ms\":{{\"decode\":{},\"install\":{},\"open\":{},\"composite\":{},\"encode\":{},\"write\":{},\"total\":{}}},\
\"psd_encode_ms\":{},\"psd_stage_ms\":{},\"reference_written\":{},\"reference_stored_written\":{}",
                r.width,
                r.height,
                r.layers,
                r.doc_tiles,
                opt_num(r.layer_tiles),
                r.gpu_tiles,
                r.cpu_tiles,
                r.open_notes,
                r.output_bytes,
                ms(r.decode),
                ms(r.install),
                ms(r.decode + r.install),
                ms(r.composite),
                ms(r.encode),
                ms(r.write),
                ms(r.total),
                opt_num(r.psd_encode_ms),
                opt_num(r.psd_stage_ms),
                r.reference_written,
                r.reference_stored_written,
            );
            o.push_str(",\"gpu_adapter\":");
            match &r.gpu_adapter {
                Some(a) => json_string(&mut o, a),
                None => o.push_str("null"),
            }
        }
        Err(f) => {
            o.push_str(",\"ok\":false,\"phase\":");
            json_string(&mut o, f.phase);
            o.push_str(",\"error\":");
            json_string(&mut o, &f.message);
        }
    }
    let _ = write!(o, ",\"peak_rss_kib\":{}}}", opt_num(peak_rss_kib()));
    o
}

/// The binary's entry point: parse, run, print one JSON line.
#[must_use]
pub fn main_from_args(args: &[String]) -> std::process::ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(usage) => {
            eprintln!("{usage}");
            return std::process::ExitCode::from(2);
        }
    };
    let result = run(&options);
    println!("{}", to_json(&options, &result));
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
#[path = "headless_tests.rs"]
mod headless_tests;
