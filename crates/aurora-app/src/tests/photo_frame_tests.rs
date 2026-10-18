//! 60 FPS round 9: frame costs on photo-sized documents.
//!
//! PLAN.md's speed goal is 60 FPS on 24-60 MP photos, and every frame
//! fixture before this round held a few dozen tiles. These build a 24 MP
//! (6000 x 4000) or a 60 MP (9504 x 6336) document with noise-like
//! content in five shapes and measure, in a 1920 x 1080 physical-pixel
//! view at 100 %:
//!
//! - **frames**: pan while painting, 40 frames, the dab's tiles
//!   invalidated narrowly as `App` does (`invalidate_painted_tiles`), the
//!   newly visible tiles stale because the view moved; the whole frame
//!   (recomposite, upload, render pass, submit and wait) per frame. CPU
//!   shapes run three ways: the frame pool, the sequential loop, and the
//!   background worker (`recomposite_or_dispatch` + `drain`, paced at
//!   16.7 ms like a vsync'd loop, reporting the UI thread's time and the
//!   visible tiles still stale at the end of each frame).
//! - **dab**: one dab's synchronous recomposite (its 1-4 tiles), 200 dabs.
//! - **full**: every document tile recomposited from scratch (what a
//!   whole-document view would cost; the canvas's zoom floor keeps the
//!   live view at about 100 %, see the round's PLAN.md entry), on the
//!   GPU path, the sequential CPU loop, the frame pool and the worker.
//!
//! Every result is one line starting `R9 ` with `key=value` fields. All
//! tests are `#[ignore]`d; run one with
//! `cargo test -p aurora-app --release --lib <name> -- --ignored --nocapture`.
//! `AURORA_R9_SHAPES` (comma-separated: `photo`, `photo_curves`,
//! `photo_masked_curves`, `four_cpu`, `four_gpu`) limits the shapes.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::time::{Duration, Instant};

use aurora_tile::{SurfaceId, TileId, TileStore};

use super::{frame_timing_target, real_gpu_context};

const VIEWPORT: (u32, u32) = (1920, 1080);
const FRAMES: u32 = 40;
const PAN_STEP: (u32, u32) = (96, 48);
const START: (u32, u32) = (64, 32);
/// Store budget in tiles (4 GiB): holds every fixture resident.
const STORE_BUDGET_TILES: usize = 8_192;
const VSYNC: Duration = Duration::from_micros(16_667);

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    /// One opaque photo layer (GPU path).
    Photo,
    /// The photo under a root, unmasked Curves layer (GPU path).
    PhotoCurves,
    /// The photo under a masked Curves layer (CPU path).
    PhotoMaskedCurves,
    /// Photo, a translucent Overlay retouch layer, a Multiply layer at
    /// 60 % and a masked Curves layer (CPU path).
    FourCpu,
    /// The same without the mask (GPU path).
    FourGpu,
}

impl Shape {
    const ALL: [Self; 5] = [
        Self::Photo,
        Self::PhotoCurves,
        Self::PhotoMaskedCurves,
        Self::FourCpu,
        Self::FourGpu,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::PhotoCurves => "photo_curves",
            Self::PhotoMaskedCurves => "photo_masked_curves",
            Self::FourCpu => "four_cpu",
            Self::FourGpu => "four_gpu",
        }
    }

    fn selected() -> Vec<Self> {
        let Ok(list) = std::env::var("AURORA_R9_SHAPES") else {
            return Self::ALL.to_vec();
        };
        Self::ALL
            .into_iter()
            .filter(|shape| list.split(',').any(|name| name.trim() == shape.name()))
            .collect()
    }
}

/// A photo-sized fixture.
pub(super) struct Photo {
    _dir: tempfile::TempDir,
    pub(super) store: TileStore,
    pub(super) layers: aurora_doc::LayerTree,
    /// The layer the per-frame dab paints.
    pub(super) active: aurora_doc::LayerId,
    pub(super) active_surface: SurfaceId,
    pub(super) size: (u32, u32),
}

impl Photo {
    /// Every composite tile of the document.
    pub(super) fn all_tiles(&self) -> Vec<TileId> {
        let (w, h) = (
            self.size.0.div_ceil(aurora_tile::TILE),
            self.size.1.div_ceil(aurora_tile::TILE),
        );
        (0..h)
            .flat_map(|y| (0..w).map(move |x| TileId { x, y }))
            .collect()
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// `count` distinct noise tiles: premultiplied, every colour sample's
/// `f16` bits uniform in `[0x2000, alpha]` (about 0.008 up to the alpha),
/// so `rgb <= a` holds.
fn noise_tiles(count: usize, seed: u64, alpha: half::f16) -> Vec<Vec<half::f16>> {
    let mut rng = Rng(seed | 1);
    let top = u64::from(alpha.to_bits());
    let low = 0x2000_u64.min(top);
    let span = top - low + 1;
    (0..count)
        .map(|_| {
            let mut texels = vec![alpha; aurora_tile::SAMPLES];
            for texel in texels.chunks_exact_mut(aurora_tile::CHANNELS) {
                let bits = rng.next();
                for (i, sample) in texel.iter_mut().take(3).enumerate() {
                    let value = low + ((bits >> (i * 16)) & 0xFFFF) % span;
                    *sample = half::f16::from_bits(value as u16);
                }
            }
            texels
        })
        .collect()
}

fn fill_layer(
    store: &mut TileStore,
    surface: SurfaceId,
    tiles: &[TileId],
    noise: &[Vec<half::f16>],
) {
    for &id in tiles {
        let pick = (id.x as usize * 7 + id.y as usize * 13) % noise.len().max(1);
        let Some(source) = noise.get(pick) else {
            unreachable!("pick is in range");
        };
        let tile = ok(store.get_mut(surface, id));
        tile.texels_mut().copy_from_slice(source);
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }
}

fn curves() -> aurora_doc::Adjustment {
    let points = [
        aurora_core::CurvePoint::new(0.0, 0.05),
        aurora_core::CurvePoint::new(0.4, 0.55),
        aurora_core::CurvePoint::new(1.0, 0.95),
    ];
    aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
        composite: ok(aurora_core::ToneCurve::new(&points)),
        ..aurora_core::CurvesParams::default()
    })
}

/// Builds `shape` at `size` (document pixels).
pub(super) fn build(shape: Shape, size: (u32, u32)) -> Photo {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(STORE_BUDGET_TILES) else {
        unreachable!("non-zero");
    };
    let mut store = ok(TileStore::new(dir.path().to_path_buf(), budget));
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let tiles: Vec<TileId> = {
        let (w, h) = (
            size.0.div_ceil(aurora_tile::TILE),
            size.1.div_ceil(aurora_tile::TILE),
        );
        (0..h)
            .flat_map(|y| (0..w).map(move |x| TileId { x, y }))
            .collect()
    };
    let photo = ok(layers.add_pixel_layer("photo", bounds, None));
    let photo_surface = layers.surface_id(photo).unwrap_or_else(|| unreachable!());
    fill_layer(
        &mut store,
        photo_surface,
        &tiles,
        &noise_tiles(16, 0x5eed, half::f16::ONE),
    );
    let mut active = (photo, photo_surface);
    let four = matches!(shape, Shape::FourCpu | Shape::FourGpu);
    if four {
        let retouch = ok(layers.add_pixel_layer("dodge and burn", bounds, None));
        ok(layers.set_blend_mode(retouch, aurora_doc::BlendMode::Overlay));
        let surface = layers.surface_id(retouch).unwrap_or_else(|| unreachable!());
        fill_layer(
            &mut store,
            surface,
            &tiles,
            &noise_tiles(16, 0xd0d6e, half::f16::from_f32(0.5)),
        );
        let multiply = ok(layers.add_pixel_layer("colour", bounds, None));
        ok(layers.set_blend_mode(multiply, aurora_doc::BlendMode::Multiply));
        ok(layers.set_opacity(multiply, 0.6));
        let surface = layers
            .surface_id(multiply)
            .unwrap_or_else(|| unreachable!());
        fill_layer(
            &mut store,
            surface,
            &tiles,
            &noise_tiles(16, 0xc0105, half::f16::ONE),
        );
        active = (
            retouch,
            layers.surface_id(retouch).unwrap_or_else(|| unreachable!()),
        );
    }
    if shape != Shape::Photo {
        let adjustment = ok(layers.add_adjustment_layer_at("curves", curves(), None, 0));
        if matches!(shape, Shape::PhotoMaskedCurves | Shape::FourCpu) {
            ok(layers.add_mask(adjustment, bounds));
            let mask = layers
                .mask_surface_id(adjustment)
                .unwrap_or_else(|| unreachable!());
            let width = size.0 as f32;
            let _ = ok(aurora_doc::write_mask_coverage_region(
                &mut store,
                mask,
                (0, 0),
                size.0,
                size.1,
                |x, _| x as f32 / width,
            ));
        }
    }
    let gpu = crate::document_qualifies_for_gpu_compositing(&layers);
    let wants_gpu = matches!(shape, Shape::Photo | Shape::PhotoCurves | Shape::FourGpu);
    assert_eq!(
        gpu,
        wants_gpu,
        "{} must take the intended path",
        shape.name()
    );
    Photo {
        _dir: dir,
        store,
        layers,
        active: active.0,
        active_surface: active.1,
        size,
    }
}

/// How a frame recomposites.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// `recomposite_visible_tiles` with the frame pool (CPU shapes).
    Pool,
    /// The same with the sequential loop.
    Sequential,
    /// `recomposite_or_dispatch` with a background worker, paced at vsync.
    Worker,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Pool => "pool",
            Self::Sequential => "sequential",
            Self::Worker => "worker",
        }
    }
}

/// Mean of `values`, `0` when empty.
fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len().saturating_sub(1)) as f64 * p).round() as usize;
    sorted.get(index).copied().unwrap_or(0.0)
}

/// Pan while painting on `photo` for [`FRAMES`] frames and print one line.
fn frames(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, mode: Mode, path: &str) {
    let device = gpu.device();
    let queue = gpu.queue();
    let mut residency = aurora_gpu::TileResidency::new(device, queue, VIEWPORT);
    let mut canvas_pipeline = aurora_gpu::CanvasPipeline::new(device);
    let mut compositor = aurora_render::TileCompositor::new(device);
    let _ = canvas_pipeline.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
    let _ = compositor.prewarm(device, false).ok();
    let _ = crate::warm_up_gpu_composite(gpu, &mut compositor);
    let target = frame_timing_target(device, VIEWPORT);
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut cache = crate::CompositeCache::default();
    let mut worker = (mode == Mode::Worker)
        .then(|| crate::background_composite::CompositeWorker::new(std::sync::Arc::new(|| {})));
    crate::frame_composite::set_parallel_enabled_for_tests(mode == Mode::Pool);
    let _ = crate::frame_composite::take_diagnostics();
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let mut total = Vec::new();
    let mut recomposite = Vec::new();
    let mut upload = Vec::new();
    let mut stale_in = Vec::new();
    let mut stale_left = Vec::new();
    let profile_before = crate::COMPOSITE_PROFILE.with(std::cell::Cell::get);
    let _ = super::take_recomposite_phase_ms();
    let mut phase_sum = [0.0_f64; 3];
    for step in 0..FRAMES {
        let x = START.0 + step * PAN_STEP.0;
        let y = START.1 + step * PAN_STEP.1;
        let t0 = Instant::now();
        if let Some(worker) = worker.as_mut() {
            let mut target = crate::background_composite::CompositeTarget {
                layers: &photo.layers,
                reference_origin,
                store: &mut photo.store,
                cache: &mut cache,
            };
            let _ = worker.drain(Some(&mut target));
        }
        residency.set_origin(queue, (x as f32, y as f32), VIEWPORT, 1.0);
        let centre = (x as f32 + 960.0, y as f32 + 540.0);
        let outcome = aurora_brush::stamp_dab(
            &mut photo.store,
            photo.active_surface,
            centre,
            crate::BRUSH_RADIUS,
            [0.95, 0.62, 0.25],
            None,
        );
        crate::invalidate_painted_tiles(
            &mut cache,
            &photo.layers,
            photo.active,
            outcome.painted(),
            reference_origin,
        );
        stale_in.push(
            residency
                .visible_tiles()
                .filter(|&tile| !cache.is_current(tile))
                .count() as f64,
        );
        let t1 = Instant::now();
        crate::recomposite_or_dispatch(
            &residency,
            &photo.layers,
            Some(photo.active),
            &mut photo.store,
            &mut cache,
            Some(gpu),
            Some(&mut compositor),
            worker.as_mut(),
        );
        let t2 = Instant::now();
        let _ = residency.sync(
            queue,
            &mut photo.store,
            crate::composite_surface_id(),
            false,
            usize::MAX,
        );
        let t3 = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("r9-frame"),
        });
        {
            let bind_group = canvas_pipeline.bind_group(device, &residency);
            let pipeline = canvas_pipeline.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r9-frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let t4 = Instant::now();
        let phases = super::take_recomposite_phase_ms();
        if step > 0 {
            for (sum, phase) in phase_sum.iter_mut().zip(phases) {
                *sum += phase;
            }
        }
        total.push(ms(t4 - t0));
        recomposite.push(ms(t2 - t1));
        upload.push(ms(t3 - t2));
        stale_left.push(
            residency
                .visible_tiles()
                .filter(|&tile| !cache.is_current(tile))
                .count() as f64,
        );
        if mode == Mode::Worker {
            let spent = t0.elapsed();
            if let Some(rest) = VSYNC.checked_sub(spent) {
                std::thread::sleep(rest);
            }
        }
    }
    if let Some(mut worker) = worker {
        worker.shutdown();
    }
    crate::frame_composite::set_parallel_enabled_for_tests(false);
    let pool = crate::frame_composite::take_diagnostics();
    let steady = total.get(1..).unwrap_or_default();
    if std::env::var_os("AURORA_R9_PROFILE").is_some() {
        super::print_composite_profile_per_frame(label, profile_before, FRAMES);
        let n = f64::from(FRAMES - 1);
        println!(
            "R9 phases doc={label} mode={} issue={:.2} poll={:.2} drain={:.2}",
            mode.name(),
            phase_sum[0] / n,
            phase_sum[1] / n,
            phase_sum[2] / n
        );
    }
    println!(
        "R9 frames doc={label} path={path} mode={} n={} frame0={:.2} mean={:.2} p50={:.2} \
         p90={:.2} max={:.2} over16.7={} recomp_mean={:.2} upload_mean={:.2} stale_in={:.2} \
         stale_left={:.2} pool_passes={} held={} bounded={}",
        mode.name(),
        steady.len(),
        total.first().copied().unwrap_or(0.0),
        mean(steady),
        percentile(steady, 0.5),
        percentile(steady, 0.9),
        percentile(steady, 1.0),
        steady.iter().filter(|&&t| t > 16.7).count(),
        mean(recomposite.get(1..).unwrap_or_default()),
        mean(upload.get(1..).unwrap_or_default()),
        mean(stale_in.get(1..).unwrap_or_default()),
        mean(stale_left.get(1..).unwrap_or_default()),
        pool.passes,
        photo.store.held_tile_count_bound(),
        crate::frame_composite::snapshot_is_bounded(&photo.store),
    );
}

/// One dab's synchronous recomposite, 200 dabs, through
/// `recomposite_tiles` over a 9 x 6 view, as the redraw handler runs it.
fn dabs(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, path: &str) {
    let device = gpu.device();
    let mut compositor = aurora_render::TileCompositor::new(device);
    let _ = compositor.prewarm(device, false).ok();
    let _ = crate::warm_up_gpu_composite(gpu, &mut compositor);
    let view: Vec<TileId> = (2..8)
        .flat_map(|y| (2..11).map(move |x| TileId { x, y }))
        .collect();
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let mut cache = crate::CompositeCache::default();
    crate::frame_composite::set_parallel_enabled_for_tests(true);
    crate::recomposite_tiles(
        &view,
        &photo.layers,
        Some(photo.active),
        &mut photo.store,
        &mut cache,
        Some(gpu),
        Some(&mut compositor),
    );
    let runs = 200_u32;
    let mut times = Vec::new();
    let mut stale = 0_usize;
    for run in 0..runs {
        let centre = (
            700.0 + (run % 20) as f32 * 61.0,
            700.0 + (run / 20) as f32 * 53.0,
        );
        let outcome = aurora_brush::stamp_dab(
            &mut photo.store,
            photo.active_surface,
            centre,
            crate::BRUSH_RADIUS,
            [0.2, 0.4, 0.9],
            None,
        );
        crate::invalidate_painted_tiles(
            &mut cache,
            &photo.layers,
            photo.active,
            outcome.painted(),
            reference_origin,
        );
        stale += view.iter().filter(|&&tile| !cache.is_current(tile)).count();
        let started = Instant::now();
        crate::recomposite_tiles(
            &view,
            &photo.layers,
            Some(photo.active),
            &mut photo.store,
            &mut cache,
            Some(gpu),
            Some(&mut compositor),
        );
        times.push(ms(started.elapsed()));
    }
    crate::frame_composite::set_parallel_enabled_for_tests(false);
    println!(
        "R9 dab doc={label} path={path} n={runs} mean={:.3} p50={:.3} p90={:.3} \
         stale_mean={:.2}",
        mean(&times),
        percentile(&times, 0.5),
        percentile(&times, 0.9),
        stale as f64 / f64::from(runs),
    );
}

/// Every document tile recomposited from scratch, each way.
fn full(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, path: &str) {
    let all = photo.all_tiles();
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let (snapshot, entries) = crate::frame_composite::time_snapshot(&photo.store);
    println!(
        "R9 snapshot doc={label} held={} entries={entries} ms={:.3}",
        photo.store.held_tile_count_bound(),
        ms(snapshot),
    );
    let mut run = |how: &str, gpu_arm: bool, parallel: bool, limit: usize| {
        let device = gpu.device();
        let mut compositor = aurora_render::TileCompositor::new(device);
        let _ = compositor.prewarm(device, false).ok();
        let _ = crate::warm_up_gpu_composite(gpu, &mut compositor);
        crate::frame_composite::set_parallel_enabled_for_tests(parallel);
        crate::frame_composite::set_snapshot_limit_for_tests(limit);
        let mut cache = crate::CompositeCache::default();
        let started = Instant::now();
        crate::recomposite_tiles(
            &all,
            &photo.layers,
            Some(photo.active),
            &mut photo.store,
            &mut cache,
            gpu_arm.then_some(gpu),
            if gpu_arm { Some(&mut compositor) } else { None },
        );
        let took = started.elapsed();
        crate::frame_composite::set_parallel_enabled_for_tests(false);
        crate::frame_composite::set_snapshot_limit_for_tests(
            crate::frame_composite::MAX_SNAPSHOT_TILES,
        );
        let current = all.iter().filter(|&&tile| cache.is_current(tile)).count();
        assert_eq!(current, all.len(), "every tile composited");
        println!(
            "R9 full doc={label} path={path} how={how} tiles={} ms={:.1} per_tile={:.3}",
            all.len(),
            ms(took),
            ms(took) / all.len() as f64,
        );
    };
    if path == "gpu" {
        run(
            "gpu",
            true,
            false,
            crate::frame_composite::MAX_SNAPSHOT_TILES,
        );
    }
    run(
        "cpu_sequential",
        false,
        false,
        crate::frame_composite::MAX_SNAPSHOT_TILES,
    );
    run(
        "cpu_pool_shipping",
        false,
        true,
        crate::frame_composite::MAX_SNAPSHOT_TILES,
    );
    run("cpu_pool_unbounded", false, true, usize::MAX);

    // The background worker: one dispatch of every tile, then wait.
    let mut worker = crate::background_composite::CompositeWorker::new(std::sync::Arc::new(|| {}));
    let mut cache = crate::CompositeCache::default();
    let started = Instant::now();
    let mut target = crate::background_composite::CompositeTarget {
        layers: &photo.layers,
        reference_origin,
        store: &mut photo.store,
        cache: &mut cache,
    };
    let dispatched = worker.dispatch(&mut target, all.clone());
    let dispatch_ms = ms(started.elapsed());
    let installed = worker.wait_until_idle(Some(&mut target), Duration::from_secs(600));
    let took = ms(started.elapsed());
    // A second dispatch with nothing changed: the per-dispatch UI cost.
    let again = Instant::now();
    let _ = worker.dispatch(&mut target, all.clone());
    let redispatch_ms = ms(again.elapsed());
    let _ = worker.wait_until_idle(Some(&mut target), Duration::from_secs(600));
    worker.shutdown();
    println!(
        "R9 full doc={label} path={path} how=worker dispatched={dispatched} installed={installed} \
         ms={took:.1} first_dispatch_ui_ms={dispatch_ms:.2} redispatch_ui_ms={redispatch_ms:.3}",
    );
}

fn measure(size: (u32, u32), size_label: &str, what: &str) {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for shape in Shape::selected() {
        let built = Instant::now();
        let mut photo = build(shape, size);
        let label = format!("{size_label}_{}", shape.name());
        let path = if crate::document_qualifies_for_gpu_compositing(&photo.layers) {
            "gpu"
        } else {
            "cpu"
        };
        println!(
            "R9 built doc={label} path={path} held={} build_ms={:.0}",
            photo.store.held_tile_count_bound(),
            ms(built.elapsed())
        );
        match what {
            "frames" => {
                if path == "gpu" {
                    frames(&context, &mut photo, &label, Mode::Pool, path);
                } else {
                    for mode in [Mode::Pool, Mode::Sequential, Mode::Worker] {
                        frames(&context, &mut photo, &label, mode, path);
                    }
                }
            }
            "dab" => dabs(&context, &mut photo, &label, path),
            "full" => full(&context, &mut photo, &label, path),
            _ => unreachable!("a known measurement"),
        }
    }
}

const MP24: (u32, u32) = (6000, 4000);
const MP60: (u32, u32) = (9504, 6336);

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_frames_24mp() {
    measure(MP24, "24mp", "frames");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_frames_60mp() {
    measure(MP60, "60mp", "frames");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_dab_24mp() {
    measure(MP24, "24mp", "dab");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_dab_60mp() {
    measure(MP60, "60mp", "dab");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_full_24mp() {
    measure(MP24, "24mp", "full");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r9_photo_full_60mp() {
    measure(MP60, "60mp", "full");
}

#[test]
#[ignore = "probe"]
fn r9_probe_adjustment_steps() {
    let noise = noise_tiles(1, 7, half::f16::ONE);
    let Some(tile) = noise.first() else {
        unreachable!()
    };
    let aurora_doc::Adjustment::Curves(params) = curves() else {
        unreachable!()
    };
    let lut = crate::curves_lut(&params);
    let n = 50;
    let mut t = [Duration::ZERO; 5];
    for _ in 0..n {
        let s = Instant::now();
        let mut backdrop = tile.clone();
        aurora_render::un_premultiply_in_place(&mut backdrop);
        t[0] += s.elapsed();
        let s = Instant::now();
        let one = half::f16::ONE;
        let mut source = Vec::with_capacity(backdrop.len());
        for texel in backdrop.chunks_exact(4) {
            if let [r, g, b, _] = *texel {
                let [r, g, b] = lut.apply([r.to_f32(), g.to_f32(), b.to_f32()]);
                source.extend([
                    half::f16::from_f32(r),
                    half::f16::from_f32(g),
                    half::f16::from_f32(b),
                    one,
                ]);
            }
        }
        t[1] += s.elapsed();
        let s = Instant::now();
        let mut x = 0.0f32;
        for texel in backdrop.chunks_exact(4) {
            if let [r, g, b, _] = *texel {
                let [r, g, b] = lut.apply([r.to_f32(), g.to_f32(), b.to_f32()]);
                x += r + g + b;
            }
        }
        std::hint::black_box(x);
        t[2] += s.elapsed();
        let s = Instant::now();
        let mut mixed = backdrop;
        aurora_render::composite_layer_into(
            &mut mixed,
            &source,
            0.8,
            aurora_render::BlendMode::Normal,
        );
        t[3] += s.elapsed();
        let s = Instant::now();
        let mut acc = tile.clone();
        for (out, mixed) in acc.chunks_exact_mut(4).zip(mixed.chunks_exact(4)) {
            if let ([r, g, b, alpha], [mr, mg, mb, _]) = (out, mixed) {
                let a = alpha.to_f32();
                *r = half::f16::from_f32(mr.to_f32() * a);
                *g = half::f16::from_f32(mg.to_f32() * a);
                *b = half::f16::from_f32(mb.to_f32() * a);
            }
        }
        std::hint::black_box(&acc);
        t[4] += s.elapsed();
    }
    for (name, d) in ["unpremul", "lut+convert", "lut only", "fold", "writeback"]
        .iter()
        .zip(t)
    {
        println!("R9 probe {name} {:.3} ms/tile", ms(d) / f64::from(n));
    }
}
