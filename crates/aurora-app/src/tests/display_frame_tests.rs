//! The display-texture view (task 142) on a real GPU: it becomes current,
//! a zoom or a pan then uploads and composites nothing, a dab re-uploads
//! only its own tiles, a structural edit re-uploads every tile (the visible
//! ones first), and its picture equals the atlas's at 100 % and the
//! zoomed-out pyramid's at the power-of-two zooms. Plus `R11` measurements
//! (`#[ignore]`d) on round 9's 24 and 60 MP photos, the same frames as
//! round 10's zoomed-out view (`zoomed_frame_tests`) and round 9's 1:1 pan
//! while painting (`photo_frame_tests`):
//! `cargo test -p aurora-app --release --lib r11_ -- --ignored --nocapture`.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::time::{Duration, Instant};

use aurora_tile::TileId;

use super::photo_frame_tests::{Photo, Shape, build};
use super::real_gpu_context;
use crate::display_view::{self, DisplayAdvance, DisplayView};
use crate::zoomed_view::{self, Level0};

const VIEWPORT: (u32, u32) = (1920, 1080);
const VSYNC: Duration = Duration::from_micros(16_667);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn stats(values: &[f64]) -> (f64, f64, f64, usize) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p90 = sorted
        .get(((sorted.len() - 1) as f64 * 0.9).round() as usize)
        .copied()
        .unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    let over = values.iter().filter(|&&v| v > 16.7).count();
    (mean, p90, max, over)
}

thread_local! {
    /// Milliseconds from the end of `prepare` to the GPU being idle, summed.
    static DRAW_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    /// With `AURORA_R11_SPLIT`: the GPU wait for `prepare`'s own work.
    static SPLIT_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

struct Rig {
    view: DisplayView,
    canvas: aurora_gpu::CanvasPipeline,
    compositor: aurora_render::TileCompositor,
    target: wgpu::TextureView,
    texture: wgpu::Texture,
    viewport: (u32, u32),
    cache: crate::CompositeCache,
    worker: Option<crate::background_composite::CompositeWorker>,
}

struct Step {
    ms: f64,
    advance: DisplayAdvance,
}

impl Rig {
    fn new(
        gpu: &aurora_gpu::GpuContext,
        photo: &Photo,
        viewport: (u32, u32),
        worker: bool,
        max_dimension: u32,
    ) -> Self {
        let device = gpu.device();
        let cache = crate::CompositeCache::default();
        let key = (cache.id(), photo.store.serial(), photo.size);
        let Ok(view) = DisplayView::with_max_dimension(device, key, max_dimension) else {
            unreachable!("the photo is pageable");
        };
        let mut canvas = aurora_gpu::CanvasPipeline::new(device);
        let _ = canvas.display_pipeline(device, FORMAT);
        let mut compositor = aurora_render::TileCompositor::new(device);
        let _ = compositor.prewarm(device, false).ok();
        let _ = crate::warm_up_gpu_composite(gpu, &mut compositor);
        let texture = target_texture(device, viewport);
        let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            view,
            canvas,
            compositor,
            target,
            texture,
            viewport,
            cache,
            worker: worker.then(|| {
                crate::background_composite::CompositeWorker::new(std::sync::Arc::new(|| {}))
            }),
        }
    }

    /// One frame at physical `zoom` with document point `origin` at the
    /// canvas's corner, as `App::prepare_display_frame` and `redraw` run it.
    fn frame(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        photo: &mut Photo,
        zoom: f32,
        origin: (f32, f32),
    ) -> Step {
        self.frame_with(gpu, photo, zoom, origin, None)
    }

    /// [`Self::frame`], drawing `atlas` instead when the app would.
    fn frame_with(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        photo: &mut Photo,
        zoom: f32,
        origin: (f32, f32),
        atlas: Option<&mut aurora_gpu::TileResidency>,
    ) -> Step {
        let device = gpu.device();
        let queue = gpu.queue();
        let t0 = Instant::now();
        if let Some(worker) = self.worker.as_mut() {
            let reference_origin =
                crate::composite_reference_origin(&photo.layers, Some(photo.active));
            let mut target = crate::background_composite::CompositeTarget {
                layers: &photo.layers,
                reference_origin,
                store: &mut photo.store,
                cache: &mut self.cache,
            };
            let _ = worker.drain(Some(&mut target));
        }
        let limit = device.limits().max_texture_dimension_2d;
        let one_to_one = zoom >= zoomed_view::atlas_floor(self.viewport, limit);
        let visible = display_view::visible_tiles(origin, zoom, self.viewport, photo.size);
        let gpu_path = crate::document_qualifies_for_gpu_compositing(&photo.layers);
        let level0 = match self.worker.as_mut() {
            Some(worker) if !gpu_path => Level0::Worker(worker),
            _ => Level0::Sync(Some(gpu), Some(&mut self.compositor)),
        };
        let advance = self.view.prepare(
            device,
            queue,
            zoomed_view::FrameDocument {
                cache_id: self.cache.id(),
                canvas: photo.size,
                layers: &photo.layers,
                active_layer: Some(photo.active),
                store: &mut photo.store,
                cache: &mut self.cache,
            },
            &visible,
            level0,
            one_to_one,
            (origin.0, origin.1, zoom),
            &aurora_gpu::TileResidency::visible_tiles_at(origin, self.viewport, limit),
        );
        let t_draw = Instant::now();
        if std::env::var_os("AURORA_R11_SPLIT").is_some() {
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            SPLIT_MS.with(|d| d.set(d.get() + ms(t_draw.elapsed())));
        }
        match atlas {
            // As `redraw`: at or above the atlas floor a frame whose view
            // is not uploaded yet draws the atlas.
            Some(residency) if one_to_one && !advance.ready => {
                residency.set_document_clip(Some((
                    0.0,
                    0.0,
                    photo.size.0 as f32,
                    photo.size.1 as f32,
                )));
                residency.set_origin(queue, origin, self.viewport, zoom);
                let worker = if gpu_path { None } else { self.worker.as_mut() };
                crate::recomposite_or_dispatch(
                    residency,
                    &photo.layers,
                    Some(photo.active),
                    &mut photo.store,
                    &mut self.cache,
                    Some(gpu),
                    Some(&mut self.compositor),
                    worker,
                );
                let _ = residency.sync(
                    queue,
                    &mut photo.store,
                    crate::composite_surface_id(),
                    false,
                    usize::MAX,
                );
                self.draw_atlas(gpu, residency);
                let visible = display_view::visible_tiles(origin, zoom, self.viewport, photo.size);
                let _ =
                    self.view
                        .adopt_from_atlas(device, queue, residency, &photo.store, &visible);
            }
            _ => self.draw(gpu, photo.size, origin, zoom),
        }
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        DRAW_MS.with(|d| d.set(d.get() + ms(t_draw.elapsed())));
        let spent = t0.elapsed();
        if self.worker.is_some()
            && let Some(rest) = VSYNC.checked_sub(spent)
        {
            std::thread::sleep(rest);
        }
        Step {
            ms: ms(spent),
            advance,
        }
    }

    fn draw_atlas(&mut self, gpu: &aurora_gpu::GpuContext, residency: &aurora_gpu::TileResidency) {
        let device = gpu.device();
        let bind_group = self.canvas.bind_group(device, residency);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("r11-atlas"),
        });
        {
            let pipeline = self.canvas.pipeline(device, FORMAT);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r11-atlas"),
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
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue().submit(std::iter::once(encoder.finish()));
    }

    fn draw(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        size: (u32, u32),
        origin: (f32, f32),
        zoom: f32,
    ) {
        let device = gpu.device();
        let queue = gpu.queue();
        self.view.set_view(
            queue,
            origin,
            self.viewport,
            zoom,
            (0.0, 0.0, size.0 as f32, size.1 as f32),
            [1.0, 1.0, 1.0],
        );
        let scissors = display_view::surface_scissors(
            self.view.texture(),
            origin,
            zoom,
            (0.0, 0.0),
            (0, 0, self.viewport.0, self.viewport.1),
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("r11-frame"),
        });
        {
            let groups = self.view.bind_groups(&self.canvas, device).to_vec();
            let pipeline = self.canvas.display_pipeline(device, FORMAT);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("r11-frame"),
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
            pass.set_pipeline(pipeline);
            for (page, x, y, w, h) in scissors {
                let Some(group) = groups.get(page) else {
                    continue;
                };
                pass.set_scissor_rect(x, y, w, h);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Frames until every tile of the document is current and uploaded
    /// (at most `cap`); the frame count and the UI thread's frame times.
    fn settle(
        &mut self,
        gpu: &aurora_gpu::GpuContext,
        photo: &mut Photo,
        zoom: f32,
        origin: (f32, f32),
        cap: u32,
    ) -> (u32, Vec<f64>) {
        let mut times = Vec::new();
        for n in 1..=cap {
            let step = self.frame(gpu, photo, zoom, origin);
            times.push(step.ms);
            if self.settled(photo) {
                return (n, times);
            }
        }
        (cap, times)
    }

    fn settled(&self, photo: &Photo) -> bool {
        let surface = crate::composite_surface_id();
        photo.all_tiles().iter().all(|&id| {
            self.cache.is_current(id)
                && self.view.uploaded_version(id) == Some(photo.store.content_version(surface, id))
        })
    }

    fn read_back(&self, gpu: &aurora_gpu::GpuContext) -> Vec<u8> {
        read_texture(gpu, &self.texture, self.viewport)
    }
}

fn read_texture(
    gpu: &aurora_gpu::GpuContext,
    texture: &wgpu::Texture,
    size: (u32, u32),
) -> Vec<u8> {
    let device = gpu.device();
    let queue = gpu.queue();
    let row = size.0 * 4;
    let padded = row.div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("r11-readback"),
        size: u64::from(padded) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("r11-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("mapped once idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("mapped");
    };
    let mut out = Vec::new();
    for line in data.chunks_exact(padded as usize) {
        out.extend_from_slice(line.get(..row as usize).unwrap_or_default());
    }
    drop(data);
    buffer.unmap();
    out
}

/// A canvas frame through the 1:1 atlas, as `redraw` draws it.
fn atlas_picture(
    gpu: &aurora_gpu::GpuContext,
    photo: &mut Photo,
    viewport: (u32, u32),
    origin: (f32, f32),
    zoom: f32,
) -> Vec<u8> {
    let device = gpu.device();
    let queue = gpu.queue();
    let mut residency = aurora_gpu::TileResidency::new(device, queue, viewport);
    residency.set_document_clip(Some((0.0, 0.0, photo.size.0 as f32, photo.size.1 as f32)));
    residency.set_origin(queue, origin, viewport, zoom);
    let _ = residency.sync(
        queue,
        &mut photo.store,
        crate::composite_surface_id(),
        true,
        usize::MAX,
    );
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let bind_group = canvas.bind_group(device, &residency);
    let texture = target_texture(device, viewport);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("atlas-picture"),
    });
    {
        let pipeline = canvas.pipeline(device, FORMAT);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("atlas-picture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
    read_texture(gpu, &texture, viewport)
}

/// A zoomed-out frame through the pyramid (`zoomed_view`), unblended (a
/// power-of-two zoom), built until complete.
fn pyramid_picture(
    gpu: &aurora_gpu::GpuContext,
    photo: &mut Photo,
    cache: &mut crate::CompositeCache,
    viewport: (u32, u32),
    origin: (f32, f32),
    zoom: f32,
) -> Vec<u8> {
    let device = gpu.device();
    let queue = gpu.queue();
    let limit = device.limits().max_texture_dimension_2d;
    let mut atlases = zoomed_view::ZoomedAtlases::new(device, queue, viewport);
    let mut compositor = aurora_render::TileCompositor::new(device);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let texture = target_texture(device, viewport);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    for _ in 0..500 {
        let mut frame = zoomed_view::plan(
            origin,
            zoom,
            viewport,
            photo.size,
            zoomed_view::zoomed_floor(viewport, limit),
        );
        let advance = atlases.prepare(
            device,
            queue,
            &mut frame,
            zoomed_view::FrameDocument {
                cache_id: cache.id(),
                canvas: photo.size,
                layers: &photo.layers,
                active_layer: Some(photo.active),
                store: &mut photo.store,
                cache,
            },
            false,
            [1.0, 1.0, 1.0],
            Level0::Sync(Some(gpu), Some(&mut compositor)),
            Duration::from_secs(10),
        );
        if !advance.complete {
            continue;
        }
        let bind_group = atlases.bind_group(&canvas, device, &frame);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pyramid-picture"),
        });
        {
            let pipeline = canvas.pipeline(device, FORMAT);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pyramid-picture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
        return read_texture(gpu, &texture, viewport);
    }
    unreachable!("the pyramid completes");
}

/// The canvas target: `super::frame_timing_target`'s, readable.
fn target_texture(device: &wgpu::Device, viewport: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("r11-target"),
        size: wgpu::Extent3d {
            width: viewport.0,
            height: viewport.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

const SMALL: (u32, u32) = (1500, 1000);
const SMALL_VIEW: (u32, u32) = (640, 480);

#[test]
fn once_current_a_zoom_or_a_pan_uploads_and_composites_nothing() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for (shape, worker) in [(Shape::Photo, false), (Shape::PhotoMaskedCurves, true)] {
        let mut photo = build(shape, SMALL);
        let mut rig = Rig::new(&context, &photo, SMALL_VIEW, worker, u32::MAX);
        let (frames, _) = rig.settle(&context, &mut photo, 0.4, (-50.0, -40.0), 400);
        assert!(rig.settled(&photo), "{shape:?}: settled in {frames} frames");
        let uploads = rig.view.uploads();
        for (zoom, origin) in [
            (0.3, (-200.0, -100.0)),
            (0.5, (100.0, 50.0)),
            (0.77, (300.0, 200.0)),
            (1.0, (0.0, 0.0)),
            (1.0, (513.0, 257.0)),
            (2.0, (900.0, 600.0)),
            (1.0 / 32.0, (0.0, 0.0)),
        ] {
            let step = rig.frame(&context, &mut photo, zoom, origin);
            assert_eq!(step.advance.uploaded, 0, "{shape:?} {zoom} {origin:?}");
            assert_eq!(step.advance.composited, 0, "{shape:?} {zoom} {origin:?}");
            // On the worker path the atlas draws at or above its floor.
            let limit = context.device().limits().max_texture_dimension_2d;
            let atlas_draws = worker && zoom >= zoomed_view::atlas_floor(SMALL_VIEW, limit);
            assert_eq!(step.advance.ready, !atlas_draws, "{shape:?} {zoom}");
            assert!(!step.advance.more);
        }
        assert_eq!(rig.view.uploads(), uploads, "{shape:?}: no tile uploaded");
        if let Some(mut worker) = rig.worker.take() {
            worker.shutdown();
        }
    }
}

/// At 100 % a frame whose visible tiles are not all uploaded yet is not
/// ready (the app draws the atlas that frame): a fresh CPU-path document
/// whose tiles the worker is still compositing.
#[test]
fn at_100_percent_a_view_not_yet_uploaded_is_not_ready() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut photo = build(Shape::PhotoMaskedCurves, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, true, u32::MAX);
    let first = rig.frame(&context, &mut photo, 1.0, (0.0, 0.0));
    assert!(!first.advance.ready, "nothing composited yet");
    assert_eq!(first.advance.uploaded, 0);
    let (frames, _) = rig.settle(&context, &mut photo, 1.0, (0.0, 0.0), 400);
    let last = rig.frame(&context, &mut photo, 1.0, (0.0, 0.0));
    assert!(
        !last.advance.ready,
        "on the worker path the atlas keeps drawing at 100 % ({frames} frames)"
    );
    assert!(rig.settled(&photo), "and the texture is built behind it");
    let zoomed = rig.frame(&context, &mut photo, 0.5, (0.0, 0.0));
    assert!(zoomed.advance.ready && zoomed.advance.uploaded == 0);
    if let Some(mut worker) = rig.worker.take() {
        worker.shutdown();
    }
}

/// A frame the atlas drew hands its freshly uploaded tiles to the display
/// texture by a GPU copy: the next display frame uploads only the edge
/// tiles (which need masking), and its picture is the atlas's.
#[test]
fn tiles_the_atlas_just_uploaded_are_adopted_by_a_gpu_copy() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    // 1500 x 1000: the last tile column and row are partial.
    let mut photo = build(Shape::Photo, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, u32::MAX);
    let mut residency = aurora_gpu::TileResidency::new(device, queue, SMALL_VIEW);
    let origin = (1000.0, 600.0);
    residency.set_origin(queue, origin, SMALL_VIEW, 1.0);
    crate::recomposite_or_dispatch(
        &residency,
        &photo.layers,
        Some(photo.active),
        &mut photo.store,
        &mut rig.cache,
        Some(&context),
        Some(&mut rig.compositor),
        None,
    );
    let _ = residency.sync(
        queue,
        &mut photo.store,
        crate::composite_surface_id(),
        false,
        usize::MAX,
    );
    let visible = display_view::visible_tiles(origin, 1.0, SMALL_VIEW, photo.size);
    let interior = visible
        .iter()
        .filter(|&&id| rig.view.texture().tile_is_interior(id))
        .count();
    assert!(interior > 0 && interior < visible.len(), "{visible:?}");
    let adopted = rig
        .view
        .adopt_from_atlas(device, queue, &residency, &photo.store, &visible);
    assert_eq!(adopted as usize, interior);
    assert_eq!(rig.view.uploads(), 0, "no CPU upload");
    let step = rig.frame(&context, &mut photo, 1.0, origin);
    assert!(step.advance.ready);
    assert_eq!(
        step.advance.uploaded as usize,
        visible.len() - interior,
        "only the edge tiles uploaded"
    );
    rig.draw(&context, photo.size, origin, 1.0);
    let display = rig.read_back(&context);
    let atlas = atlas_picture(&context, &mut photo, SMALL_VIEW, origin, 1.0);
    assert!(
        display == atlas,
        "the adopted tiles show the atlas's picture"
    );
}

#[test]
fn a_dab_reuploads_only_the_tiles_it_painted() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut photo = build(Shape::Photo, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, u32::MAX);
    let _ = rig.settle(&context, &mut photo, 0.4, (0.0, 0.0), 400);
    let surface = crate::composite_surface_id();
    let before: Vec<(TileId, Option<u64>)> = photo
        .all_tiles()
        .into_iter()
        .map(|id| (id, rig.view.uploaded_version(id)))
        .collect();
    // On a tile corner, so it paints four tiles.
    let outcome = aurora_brush::stamp_dab(
        &mut photo.store,
        photo.active_surface,
        (512.0, 512.0),
        crate::BRUSH_RADIUS,
        [0.9, 0.1, 0.1],
        None,
    );
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    crate::invalidate_painted_tiles(
        &mut rig.cache,
        &photo.layers,
        photo.active,
        outcome.painted(),
        reference_origin,
    );
    let uploads = rig.view.uploads();
    let step = rig.frame(&context, &mut photo, 0.4, (0.0, 0.0));
    let painted: Vec<TileId> = before
        .iter()
        .filter(|(id, version)| rig.view.uploaded_version(*id) != *version)
        .map(|(id, _)| *id)
        .collect();
    let mut expected = vec![
        TileId { x: 1, y: 1 },
        TileId { x: 2, y: 1 },
        TileId { x: 1, y: 2 },
        TileId { x: 2, y: 2 },
    ];
    expected.sort_by_key(|id| (id.y, id.x));
    assert_eq!(painted, expected, "only the dab's tiles were re-uploaded");
    assert_eq!(step.advance.uploaded, 4);
    assert_eq!(rig.view.uploads(), uploads + 4);
    for id in &painted {
        assert_eq!(
            rig.view.uploaded_version(*id),
            Some(photo.store.content_version(surface, *id))
        );
    }
    assert!(rig.settled(&photo));
}

/// A pixel undo re-uploads exactly the tiles the stroke touched, through
/// the app's own undo path (`apply_pixel_step` and `after_undo_redo`).
#[test]
fn a_pixel_undo_reuploads_only_the_tiles_the_stroke_touched() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut photo = build(Shape::Photo, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, u32::MAX);
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let mut history = aurora_brush::PixelHistory::new();
    let mut snapshot = aurora_brush::StrokeSnapshot::new(photo.active_surface);
    let outcome = aurora_brush::stamp_dab(
        &mut photo.store,
        photo.active_surface,
        (300.0, 300.0),
        crate::BRUSH_RADIUS,
        [0.1, 0.9, 0.1],
        Some(&mut snapshot),
    );
    let touched: Vec<TileId> = outcome.painted().to_vec();
    assert!(!touched.is_empty());
    let _ = history.push(snapshot);
    crate::invalidate_painted_tiles(
        &mut rig.cache,
        &photo.layers,
        photo.active,
        outcome.painted(),
        reference_origin,
    );
    let _ = rig.settle(&context, &mut photo, 0.4, (0.0, 0.0), 400);
    let before: Vec<(TileId, Option<u64>)> = photo
        .all_tiles()
        .into_iter()
        .map(|id| (id, rig.view.uploaded_version(id)))
        .collect();
    let Some(invalidation) = crate::apply_pixel_step(
        &photo.layers,
        &mut history,
        &mut photo.store,
        crate::UndoDirection::Undo,
    ) else {
        unreachable!("one stroke to undo");
    };
    let mut view = aurora_ui::CanvasView::default();
    crate::after_undo_redo(
        &mut view,
        &photo.layers,
        Some(photo.active),
        &mut rig.cache,
        None,
        &invalidation,
    );
    let step = rig.frame(&context, &mut photo, 0.4, (0.0, 0.0));
    let changed: Vec<TileId> = before
        .iter()
        .filter(|(id, version)| rig.view.uploaded_version(*id) != *version)
        .map(|(id, _)| *id)
        .collect();
    // The layer is at the document origin, so its tiles are the composite's.
    let mut want = touched.clone();
    want.sort_by_key(|id| (id.y, id.x));
    assert_eq!(changed, want, "only the undone stroke's tiles");
    assert_eq!(step.advance.uploaded as usize, touched.len());
    assert!(rig.settled(&photo));
}

#[test]
fn a_structural_edit_reuploads_every_tile_the_visible_ones_first() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut photo = build(Shape::Photo, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, u32::MAX);
    let _ = rig.settle(&context, &mut photo, 0.4, (0.0, 0.0), 400);
    let before: Vec<(TileId, Option<u64>)> = photo
        .all_tiles()
        .into_iter()
        .map(|id| (id, rig.view.uploaded_version(id)))
        .collect();
    // An opacity change bumps the whole composite cache.
    if photo.layers.set_opacity(photo.active, 0.5).is_err() {
        unreachable!("a pixel layer");
    }
    rig.cache.bump();
    let origin = (300.0, 200.0);
    let visible = display_view::visible_tiles(origin, 1.0, SMALL_VIEW, photo.size);
    let step = rig.frame(&context, &mut photo, 1.0, origin);
    assert!(step.advance.ready, "the visible tiles are current at once");
    for id in &visible {
        let old = before.iter().find(|(t, _)| t == id).and_then(|(_, v)| *v);
        assert_ne!(rig.view.uploaded_version(*id), old, "{id:?} re-uploaded");
    }
    let (frames, _) = rig.settle(&context, &mut photo, 1.0, origin, 400);
    assert!(rig.settled(&photo), "all re-uploaded in {frames} frames");
    for (id, old) in &before {
        assert_ne!(rig.view.uploaded_version(*id), *old, "{id:?}");
    }
}

/// At 100 % and above the display texture draws the atlas's picture, one
/// page or several.
#[test]
fn at_100_percent_the_picture_is_the_atlas_picture() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    for max_dimension in [u32::MAX, 3 * aurora_tile::TILE] {
        let mut photo = build(Shape::PhotoMaskedCurves, SMALL);
        let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, max_dimension);
        let _ = rig.settle(&context, &mut photo, 0.4, (0.0, 0.0), 400);
        for (zoom, origin) in [
            (1.0, (0.0, 0.0)),
            (1.0, (700.0, 420.0)),
            (2.0, (512.0, 300.0)),
        ] {
            rig.draw(&context, photo.size, origin, zoom);
            let display = rig.read_back(&context);
            let atlas = atlas_picture(&context, &mut photo, SMALL_VIEW, origin, zoom);
            let differ = display
                .chunks_exact(4)
                .zip(atlas.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(differ, 0, "{max_dimension}: {zoom} at {origin:?}");
        }
    }
}

/// At the power-of-two zooms the display texture shows what the zoomed-out
/// pyramid shows (both are the composite's 2^L box filter), within one
/// 8-bit step (f16 rounding: the pyramid stores straight texels and
/// premultiplies on upload, the texture's chain is premultiplied
/// throughout).
#[test]
fn at_power_of_two_zooms_the_picture_is_the_pyramid_picture() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let mut photo = build(Shape::FourGpu, SMALL);
    let mut rig = Rig::new(&context, &photo, SMALL_VIEW, false, u32::MAX);
    let _ = rig.settle(&context, &mut photo, 0.4, (0.0, 0.0), 400);
    for (zoom, origin) in [(0.5, (0.0, 0.0)), (0.25, (0.0, 0.0)), (0.5, (256.0, 128.0))] {
        rig.draw(&context, photo.size, origin, zoom);
        let display = rig.read_back(&context);
        let mut cache = crate::CompositeCache::default();
        let pyramid = pyramid_picture(&context, &mut photo, &mut cache, SMALL_VIEW, origin, zoom);
        let worst = display
            .iter()
            .zip(&pyramid)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let differ = display
            .chunks_exact(4)
            .zip(pyramid.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        println!("{zoom} at {origin:?}: {differ} pixels differ, worst {worst}");
        assert!(worst <= 1, "{zoom} at {origin:?}: worst {worst}");
    }
}

// ---- R11 measurements ----

fn fit(size: (u32, u32)) -> (f32, (f32, f32)) {
    let view = (VIEWPORT.0 as f32, VIEWPORT.1 as f32);
    let zoom = zoomed_view::fit_zoom(view, size).unwrap_or(1.0);
    let pan = zoomed_view::centred_pan((0.0, 0.0), zoom, view, size, 1.0);
    (zoom, (-pan.0 / zoom, -pan.1 / zoom))
}

fn run(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, path: &str, how: &str) {
    let worker = how == "worker";
    crate::frame_composite::set_parallel_enabled_for_tests(how != "sequential");
    cold_paint(gpu, photo, label, path, how);
    if std::env::var("AURORA_R11_ONLY").is_ok_and(|v| v == "cold_paint") {
        crate::frame_composite::set_parallel_enabled_for_tests(false);
        return;
    }
    let mut rig = Rig::new(gpu, photo, VIEWPORT, worker, u32::MAX);
    let (zoom, origin) = fit(photo.size);
    let started = Instant::now();
    let (frames, times) = rig.settle(gpu, photo, zoom, origin, 4_000);
    let cold_ms = ms(started.elapsed());
    let (mean, p90, max, over) = stats(&times);
    println!(
        "R11 cold doc={label} path={path} how={how} zoom={zoom:.4} frames={frames} ms={cold_ms:.0} \
         ui_mean={mean:.2} ui_p90={p90:.2} ui_max={max:.2} over16={over} pages={} vram_mib={:.1} \
         uploads={}",
        rig.view.texture().pages().len(),
        rig.view.texture().bytes() as f64 / f64::from(1_u32 << 20),
        rig.view.uploads()
    );

    let times: Vec<f64> = (0..40)
        .map(|_| rig.frame(gpu, photo, zoom, origin).ms)
        .collect();
    let (mean, p90, max, over) = stats(times.get(1..).unwrap_or_default());
    println!(
        "R11 fit doc={label} path={path} how={how} n=39 mean={mean:.2} p90={p90:.2} max={max:.2} \
         over16={over}"
    );

    // A dab at fit: ms until every tile is current and uploaded again.
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
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
        let (frames, _) = rig.settle(gpu, photo, zoom, origin, 600);
        dab_ms.push(ms(started.elapsed()));
        dab_frames.push(f64::from(frames));
    }
    let (mean, p90, max, _) = stats(&dab_ms);
    let (fmean, _, fmax, _) = stats(&dab_frames);
    println!(
        "R11 dab doc={label} path={path} how={how} n=10 to_current_mean={mean:.2} p90={p90:.2} \
         max={max:.2} frames_mean={fmean:.1} frames_max={fmax:.0}"
    );

    // Wheel: round 10's steps (fit -> the 1:1 atlas floor and back, 1.1x
    // about the centre), then on to 2x and back (what round 10's pyramid
    // could not draw past its floor).
    let floor = aurora_gpu::TileResidency::min_zoom_for_viewport(VIEWPORT);
    let centre_doc = (photo.size.0 as f32 / 2.0, photo.size.1 as f32 / 2.0);
    let ladder = |top: f32| {
        let mut steps = Vec::new();
        let mut z = zoom;
        while z * 1.1 < top {
            z *= 1.1;
            steps.push(z);
        }
        let back: Vec<f32> = steps.iter().rev().copied().collect();
        steps.extend(back);
        steps
    };
    for (name, top) in [("wheel", floor), ("wheel2x", 2.0)] {
        let uploads = rig.view.uploads();
        let mut wheel = Vec::new();
        for z in ladder(top) {
            let origin = (
                centre_doc.0 - VIEWPORT.0 as f32 / 2.0 / z,
                centre_doc.1 - VIEWPORT.1 as f32 / 2.0 / z,
            );
            wheel.push(rig.frame(gpu, photo, z, origin).ms);
        }
        let (mean, p90, max, over) = stats(&wheel);
        println!(
            "R11 {name} doc={label} path={path} how={how} n={} mean={mean:.2} p90={p90:.2} \
             max={max:.2} over16={over} uploads={}",
            wheel.len(),
            rig.view.uploads() - uploads
        );
    }

    // Pan at 0.3 (round 10's), 40 frames of (96, 48) px.
    let uploads = rig.view.uploads();
    let mut pan = Vec::new();
    for step in 0..40_u32 {
        let origin = (
            64.0 + step as f32 * 96.0 / 0.3,
            32.0 + step as f32 * 48.0 / 0.3,
        );
        pan.push(rig.frame(gpu, photo, 0.3, origin).ms);
    }
    let (mean, p90, max, over) = stats(pan.get(1..).unwrap_or_default());
    println!(
        "R11 pan doc={label} path={path} how={how} zoom=0.3 n=39 mean={mean:.2} p90={p90:.2} \
         max={max:.2} over16={over} uploads={}",
        rig.view.uploads() - uploads
    );

    // Round 9's 1:1 pan while painting: 40 frames of (96, 48) px, a dab at
    // the view centre each frame.
    let mut frames = Vec::new();
    let mut not_ready = 0;
    for step in 0..40_u32 {
        let (x, y) = (64 + step * 96, 32 + step * 48);
        let outcome = aurora_brush::stamp_dab(
            &mut photo.store,
            photo.active_surface,
            (x as f32 + 960.0, y as f32 + 540.0),
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
        let step = rig.frame(gpu, photo, 1.0, (x as f32, y as f32));
        if !step.advance.ready {
            not_ready += 1;
        }
        frames.push(step.ms);
    }
    let (mean, p90, max, over) = stats(frames.get(1..).unwrap_or_default());
    println!(
        "R11 paint11 doc={label} path={path} how={how} n=39 mean={mean:.2} p90={p90:.2} \
         max={max:.2} over16={over} not_ready={not_ready}"
    );
    if let Some(mut worker) = rig.worker.take() {
        worker.shutdown();
    }
    crate::frame_composite::set_parallel_enabled_for_tests(false);
}

/// Round 9's frames exactly (a cold cache, 1:1, pan while painting, the
/// newly seen tiles stale), as the app runs them with the display texture:
/// a frame not ready draws the atlas (`redraw`'s fallback), and the
/// display's own background build of the rest of the document runs
/// alongside. Compare with `R9 frames`.
fn cold_paint(gpu: &aurora_gpu::GpuContext, photo: &mut Photo, label: &str, path: &str, how: &str) {
    let device = gpu.device();
    let mut rig = Rig::new(gpu, photo, VIEWPORT, how == "worker", u32::MAX);
    let mut residency = aurora_gpu::TileResidency::new(device, gpu.queue(), VIEWPORT);
    let reference_origin = crate::composite_reference_origin(&photo.layers, Some(photo.active));
    let mut frames = Vec::new();
    let mut atlas_frames = 0;
    let mut prepares = Vec::new();
    crate::display_view::PHASES.with(|p| p.set([0.0; 4]));
    for step in 0..40_u32 {
        let (x, y) = (64 + step * 96, 32 + step * 48);
        let t0 = Instant::now();
        let outcome = aurora_brush::stamp_dab(
            &mut photo.store,
            photo.active_surface,
            (x as f32 + 960.0, y as f32 + 540.0),
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
        let dab = ms(t0.elapsed());
        let before = crate::display_view::PHASES.with(std::cell::Cell::get);
        let draw_before = DRAW_MS.with(std::cell::Cell::get);
        let step = rig.frame_with(gpu, photo, 1.0, (x as f32, y as f32), Some(&mut residency));
        let after = crate::display_view::PHASES.with(std::cell::Cell::get);
        let mut phases: Vec<f64> = after.iter().zip(before).map(|(a, b)| a - b).collect();
        phases.push(DRAW_MS.with(std::cell::Cell::get) - draw_before);
        prepares.push(phases);
        if !step.advance.ready {
            atlas_frames += 1;
        }
        // The dab is inside the frame, as round 9's (the worker's vsync
        // pacing is not).
        frames.push(dab + step.ms);
    }
    let phase = |k: usize| {
        let v: Vec<f64> = prepares
            .iter()
            .skip(1)
            .map(|p| p.get(k).copied().unwrap_or(0.0))
            .collect();
        stats(&v).0
    };
    println!(
        "R11 cold_paint11_phases doc={label} how={how} composite={:.2} upload={:.2} rest={:.2} \
         mips={:.2} draw_and_wait={:.2}",
        phase(0),
        phase(1),
        phase(2),
        phase(3),
        phase(4)
    );
    println!(
        "R11 cold_paint11_split doc={label} how={how} prepare_gpu_wait_total={:.2}",
        SPLIT_MS.with(std::cell::Cell::get)
    );
    let (mean, p90, max, over) = stats(frames.get(1..).unwrap_or_default());
    println!(
        "R11 cold_paint11 doc={label} path={path} how={how} n=39 frame0={:.2} mean={mean:.2} \
         p90={p90:.2} max={max:.2} over16={over} atlas_frames={atlas_frames}",
        frames.first().copied().unwrap_or(0.0)
    );
    if let Some(mut worker) = rig.worker.take() {
        worker.shutdown();
    }
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
fn r11_display_24mp() {
    measure((6000, 4000), "24mp");
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture in release"]
fn r11_display_60mp() {
    measure((9504, 6336), "60mp");
}
