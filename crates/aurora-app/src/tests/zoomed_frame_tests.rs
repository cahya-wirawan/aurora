//! Zoomed-out view (task 136): frame costs on 24-60 MP photos.
//!
//! The documents are round 9's (`photo_frame_tests::build`), seen in a
//! 1920 x 1080 physical view through the zoomed-out atlas
//! (`zoomed_view`), exactly as `App::prepare_zoomed_frame` drives it:
//! `plan_blended` and `ZoomedAtlases::prepare` (task 140: the level and
//! the next one it blends toward, the slots forgotten on a key change,
//! `advance`
//! (GPU shapes: synchronous recomposites under the frame budget; CPU
//! shapes: the background worker, drained and paced at 16.7 ms, or the
//! synchronous pool), `sync` of the shown level, the render pass, submit
//! and wait. Each result is one `R10 ` line:
//!
//! - **cold**: Fit on Screen from a cold composite and pyramid: frames
//!   and milliseconds to the first complete picture, the UI thread's
//!   per-frame time (mean, p90, max), pyramid tiles and bytes.
//! - **fit**: 40 more frames at the fitted zoom with nothing changed.
//! - **pan**: 40 frames panning at 0.3 (level 1) from a warm 100 % cache
//!   of nothing (the newly seen tiles composited as they come).
//! - **wheel**: zoom from fit to 1.0 and back in 1.1x wheel steps.
//! - **dab**: one dab at fit, frames and ms until the level is current.
//!
//! All `#[ignore]`d; run one with
//! `cargo test -p aurora-app --release --lib r10_ -- --ignored --nocapture`.
//! `AURORA_R9_SHAPES` limits the shapes as in round 9.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::time::{Duration, Instant};

use super::photo_frame_tests::{Photo, Shape, build};
use super::{frame_timing_target, real_gpu_context};
use crate::zoomed_view::{self, Level0};

const VIEWPORT: (u32, u32) = (1920, 1080);
const VSYNC: Duration = Duration::from_micros(16_667);

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn stats(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p90 = sorted
        .get(((sorted.len() - 1) as f64 * 0.9).round() as usize)
        .copied()
        .unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    (mean, p90, max)
}

struct Rig {
    atlases: zoomed_view::ZoomedAtlases,
    canvas: aurora_gpu::CanvasPipeline,
    compositor: aurora_render::TileCompositor,
    target: wgpu::TextureView,
    _texture: wgpu::Texture,
    cache: crate::CompositeCache,
    worker: Option<crate::background_composite::CompositeWorker>,
    /// Frames that wanted the blend toward the next level and did not draw
    /// it (task 140 review: a dab or a pan must not switch it off).
    unblended: u32,
}

/// What one rig frame did.
struct Step {
    ms: f64,
    advance: zoomed_view::Advance,
    /// The frame wanted a blend (task 140) and drew it.
    blend_wanted: bool,
    blend_live: bool,
}

impl Rig {
    fn new(gpu: &aurora_gpu::GpuContext, worker: bool) -> Self {
        let device = gpu.device();
        let queue = gpu.queue();
        let atlases = zoomed_view::ZoomedAtlases::new(device, queue, VIEWPORT);
        let mut canvas = aurora_gpu::CanvasPipeline::new(device);
        let _ = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
        let mut compositor = aurora_render::TileCompositor::new(device);
        let _ = compositor.prewarm(device, false).ok();
        let _ = crate::warm_up_gpu_composite(gpu, &mut compositor);
        let texture = frame_timing_target(device, VIEWPORT);
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            atlases,
            canvas,
            compositor,
            target,
            _texture: texture,
            cache: crate::CompositeCache::default(),
            worker: worker.then(|| {
                crate::background_composite::CompositeWorker::new(std::sync::Arc::new(|| {}))
            }),
            unblended: 0,
        }
    }

    /// One frame at physical `zoom` with document point `origin` at the
    /// canvas's corner; the UI thread's time and what `advance` did.
    fn frame(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        photo: &mut Photo,
        zoom: f32,
        origin: (f32, f32),
    ) -> Step {
        let device = gpu.device();
        let queue = gpu.queue();
        let t0 = Instant::now();
        let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
        if let Some(worker) = self.worker.as_mut() {
            let mut target = crate::background_composite::CompositeTarget {
                layers: &photo.layers,
                reference_origin,
                store: &mut photo.store,
                cache: &mut self.cache,
            };
            let _ = worker.drain(Some(&mut target));
        }
        let limit = gpu.device().limits().max_texture_dimension_2d;
        let mut frame = zoomed_view::plan_blended(
            origin,
            zoom,
            VIEWPORT,
            photo.size,
            zoomed_view::zoomed_floor(VIEWPORT, limit),
            zoomed_view::atlas_floor(VIEWPORT, limit),
        );
        let blend_wanted = frame.blend.is_some();
        let level0 = match self.worker.as_mut() {
            Some(worker) => Level0::Worker(worker),
            None => Level0::Sync(Some(gpu), Some(&mut self.compositor)),
        };
        let advance = self.atlases.prepare(
            device,
            queue,
            &mut frame,
            zoomed_view::FrameDocument {
                cache_id: self.cache.id(),
                canvas: photo.size,
                layers: &photo.layers,
                active_layer: Some(photo.active),
                store: &mut photo.store,
                cache: &mut self.cache,
            },
            false,
            [1.0, 1.0, 1.0],
            level0,
            zoomed_view::PYRAMID_FRAME_BUDGET,
        );
        let blend_live = frame.blend.is_some();
        if blend_wanted && !blend_live {
            self.unblended += 1;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("r10-frame"),
        });
        {
            let bind_group = self.atlases.bind_group(&self.canvas, device, &frame);
            let pipeline = self
                .canvas
                .pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r10-frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target,
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
            #[allow(clippy::cast_precision_loss)]
            pass.set_viewport(
                frame.shift_px.0 as f32,
                frame.shift_px.1 as f32,
                frame.viewport_px.0 as f32,
                frame.viewport_px.1 as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let spent = t0.elapsed();
        if self.worker.is_some()
            && let Some(rest) = VSYNC.checked_sub(spent)
        {
            std::thread::sleep(rest);
        }
        Step {
            ms: ms(spent),
            advance,
            blend_wanted,
            blend_live,
        }
    }

    /// Frames until the shown level is complete (at most `cap`): that
    /// count, its milliseconds and the frame times; then, since task 140,
    /// frames and milliseconds until the blend toward the next level is
    /// live too (the same when the zoom does not blend).
    fn until_complete(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        photo: &mut Photo,
        zoom: f32,
        origin: (f32, f32),
        cap: u32,
    ) -> (u32, f64, Vec<f64>, (u32, f64)) {
        let started = Instant::now();
        let mut times = Vec::new();
        let mut complete = None;
        for n in 1..=cap {
            let step = self.frame(gpu, photo, zoom, origin);
            if complete.is_none() {
                times.push(step.ms);
            }
            if step.advance.complete && complete.is_none() {
                complete = Some((n, ms(started.elapsed())));
            }
            if let Some((frames, total)) = complete
                && step.blend_live == step.blend_wanted
            {
                return (frames, total, times, (n, ms(started.elapsed())));
            }
        }
        let (frames, total) = complete.unwrap_or((cap, ms(started.elapsed())));
        (frames, total, times, (cap, ms(started.elapsed())))
    }
}

/// The fitted physical zoom and centred origin for `size` in the view.
fn fit(size: (u32, u32)) -> (f32, (f32, f32)) {
    let view = (VIEWPORT.0 as f32, VIEWPORT.1 as f32);
    let zoom = zoomed_view::fit_zoom(view, size).unwrap_or(1.0);
    let pan = zoomed_view::centred_pan((0.0, 0.0), zoom, view, size, 1.0);
    (zoom, (-pan.0 / zoom, -pan.1 / zoom))
}

fn run(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, path: &str, how: &str) {
    let worker = how == "worker";
    crate::frame_composite::set_parallel_enabled_for_tests(how != "sequential");
    let mut rig = Rig::new(gpu, worker);
    let (zoom, origin) = fit(photo.size);
    let level = zoomed_view::level_for_zoom(zoom);

    let (frames, total, times, (blend_frames, blend_ms)) =
        rig.until_complete(gpu, photo, zoom, origin, 2_000);
    let (mean, p90, max) = stats(&times);
    let pyramid = rig.cache.pyramid.current_len();
    // The level-0 composite the pyramid is built from stays in the store
    // too (512 KiB a tile, pageable).
    let level0 = rig.cache.current.len();
    println!(
        "R10 cold doc={label} path={path} how={how} zoom={zoom:.4} level={level} frames={frames} \
         ms={total:.0} ui_mean={mean:.2} ui_p90={p90:.2} ui_max={max:.2} pyramid_tiles={pyramid} \
         pyramid_mib={:.1} level0_tiles={level0} level0_mib={:.1} held={} \
         blend_frames={blend_frames} blend_ms={blend_ms:.0}",
        pyramid as f64 * 0.5,
        level0 as f64 * 0.5,
        photo.store.held_tile_count_bound()
    );

    let times: Vec<f64> = (0..40)
        .map(|_| rig.frame(gpu, photo, zoom, origin).ms)
        .collect();
    let (mean, p90, max) = stats(times.get(1..).unwrap_or_default());
    println!(
        "R10 fit doc={label} path={path} how={how} n=39 mean={mean:.2} p90={p90:.2} max={max:.2}"
    );

    // A dab at fit: frames and ms until the level is current again.
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let dab_unblended = rig.unblended;
    let mut dab_ms = Vec::new();
    let mut dab_frames = Vec::new();
    for i in 0..10 {
        let centre = (
            photo.size.0 as f32 * (0.2 + 0.06 * i as f32),
            photo.size.1 as f32 * 0.5,
        );
        let started = Instant::now();
        let outcome = aurora_brush::stamp_dab(
            &mut photo.store,
            photo.active_surface,
            centre,
            crate::BRUSH_RADIUS,
            [0.95, 0.62, 0.25],
            None,
        );
        crate::invalidate_painted_tiles(
            &mut rig.cache,
            &photo.layers,
            photo.active,
            outcome.painted(),
            reference_origin,
        );
        let (frames, _, _, _) = rig.until_complete(gpu, photo, zoom, origin, 600);
        dab_ms.push(ms(started.elapsed()));
        dab_frames.push(f64::from(frames));
    }
    let (mean, p90, max) = stats(&dab_ms);
    let (fmean, _, fmax) = stats(&dab_frames);
    println!(
        "R10 dab doc={label} path={path} how={how} n=10 to_current_mean={mean:.2} p90={p90:.2} \
         max={max:.2} frames_mean={fmean:.1} frames_max={fmax:.0} unblended_frames={}",
        rig.unblended - dab_unblended
    );

    // Wheel: fit -> 1.0 physical floor of the 1:1 atlas and back, 1.1x a
    // step about the view centre (stops at the 1:1 floor: above it the
    // canvas atlas draws).
    let floor = aurora_gpu::TileResidency::min_zoom_for_viewport(VIEWPORT);
    let centre_doc = (photo.size.0 as f32 / 2.0, photo.size.1 as f32 / 2.0);
    let mut steps = Vec::new();
    let mut z = zoom;
    while z * 1.1 < floor {
        z *= 1.1;
        steps.push(z);
    }
    let back: Vec<f32> = steps.iter().rev().copied().collect();
    steps.extend(back);
    let mut wheel = Vec::new();
    let mut incomplete = 0;
    let mut unblended = 0;
    let mut changes = Vec::new();
    let mut last_level = zoomed_view::level_for_zoom(zoom);
    for z in &steps {
        let origin = (
            (centre_doc.0 - VIEWPORT.0 as f32 / 2.0 / z).max(0.0),
            (centre_doc.1 - VIEWPORT.1 as f32 / 2.0 / z).max(0.0),
        );
        let step = rig.frame(gpu, photo, *z, origin);
        wheel.push(step.ms);
        if zoomed_view::level_for_zoom(*z) != last_level {
            changes.push(step.ms);
        }
        last_level = zoomed_view::level_for_zoom(*z);
        if !step.advance.complete {
            incomplete += 1;
        }
        if step.blend_wanted && !step.blend_live {
            unblended += 1;
        }
    }
    let (mean, p90, max) = stats(&wheel);
    let (change_mean, _, change_max) = stats(&changes);
    println!(
        "R10 levelchange doc={label} path={path} how={how} n={} mean={change_mean:.2} \
         max={change_max:.2}",
        changes.len()
    );
    println!(
        "R10 wheel doc={label} path={path} how={how} n={} mean={mean:.2} p90={p90:.2} \
         max={max:.2} incomplete_frames={incomplete} unblended_frames={unblended}",
        wheel.len()
    );

    let pan_unblended = rig.unblended;
    // Pan at 0.3 (level 1), 40 frames of (96, 48) px.
    let mut pan = Vec::new();
    let mut incomplete = 0;
    for step in 0..40_u32 {
        let origin = (
            64.0 + step as f32 * 96.0 / 0.3,
            32.0 + step as f32 * 48.0 / 0.3,
        );
        let step = rig.frame(gpu, photo, 0.3, origin);
        pan.push(step.ms);
        if !step.advance.complete {
            incomplete += 1;
        }
    }
    let (mean, p90, max) = stats(pan.get(1..).unwrap_or_default());
    println!(
        "R10 pan doc={label} path={path} how={how} zoom=0.3 n=39 mean={mean:.2} p90={p90:.2} \
         max={max:.2} incomplete_frames={incomplete} pyramid_tiles={} unblended_frames={}",
        rig.cache.pyramid.current_len(),
        rig.unblended - pan_unblended
    );
    if let Some(mut worker) = rig.worker.take() {
        worker.shutdown();
    }
    crate::frame_composite::set_parallel_enabled_for_tests(false);
}

fn measure(size: (u32, u32), size_label: &str) {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for shape in Shape::selected() {
        let label = format!("{size_label}_{}", shape_name(shape));
        let gpu_path = {
            let photo = build(shape, size);
            crate::document_qualifies_for_gpu_compositing(&photo.layers)
        };
        let path = if gpu_path { "gpu" } else { "cpu" };
        let hows: &[&str] = if gpu_path {
            &["sync"]
        } else {
            &["worker", "pool"]
        };
        for how in hows {
            // A fresh document each way, so every run starts cold.
            let mut photo = build(shape, size);
            run(&context, &mut photo, &label, path, how);
        }
    }
}

fn shape_name(shape: Shape) -> &'static str {
    match shape {
        Shape::Photo => "photo",
        Shape::PhotoCurves => "photo_curves",
        Shape::PhotoMaskedCurves => "photo_masked_curves",
        Shape::FourCpu => "four_cpu",
        Shape::FourGpu => "four_gpu",
    }
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r10_zoomed_out_24mp() {
    measure((6000, 4000), "24mp");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r10_zoomed_out_60mp() {
    measure((9504, 6336), "60mp");
}
