//! End-to-end proof that [`crate::PathPipeline`]/[`crate::GpuMesh`]
//! together actually draw correct pixels — not just "it compiled and
//! didn't panic" (`render.rs`'s own unit tests), a real rendered-output
//! check, matching `aurora_gpu::render_test`'s own discipline (and this
//! project's general practice: `spike/FINDINGS.md`, `aurora-tile`'s
//! bit-exact round-trip tests). This sandbox has no real GPU adapter
//! (`real_context` skips, logged, every time this file's own tests run
//! here) — genuinely unverified against real hardware yet, the same
//! "written correctly, not yet proven on a real GPU" state
//! `aurora_gpu`'s own render tests were in before a real desktop
//! session first ran them.

#![cfg(test)]

use crate::render::{
    GlyphAtlas, GpuColorMesh, GpuMesh, GpuPaintOp, GradientPipeline, PathPipeline, TextPipeline,
    draw_paint_ops,
};
use crate::test_support::real_context;
use aurora_gpu::GpuContext;
use aurora_vector::{
    ColorMesh, DEFAULT_GRADIENT_CELLS, GradientCorners, Mesh, Point, bilinear_rect,
    horizontal_strip,
};

const TARGET_SIZE: (u32, u32) = (64, 64);

// One linear setup-render-readback flow for a real GPU render+readback
// helper -- splitting it further would just relocate the same lines
// without reducing the actual complexity a real render pass needs
// (bind group, pipeline, target texture, pass, copy, map, read), the
// same precedent `aurora_gpu::render_test`'s own analogous function
// already sets.
#[allow(clippy::too_many_lines)]
fn render_and_sample_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &mut PathPipeline,
    mesh: &GpuMesh,
    color: [f32; 4],
    sample: (u32, u32),
) -> [u8; 4] {
    let bind_group = path.bind_group(
        device,
        queue,
        (TARGET_SIZE.0 as f32, TARGET_SIZE.1 as f32),
        color,
    );
    let pipeline = path.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: TARGET_SIZE.0,
            height: TARGET_SIZE.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("path"),
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
        path.draw(&mut pass, mesh);
    }

    let bytes_per_row = TARGET_SIZE.0 * 4;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(TARGET_SIZE.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TARGET_SIZE.1),
            },
        },
        wgpu::Extent3d {
            width: TARGET_SIZE.0,
            height: TARGET_SIZE.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let (sx, sy) = sample;
    let offset = (sy as usize) * (bytes_per_row as usize) + (sx as usize) * 4;
    let Some(pixel) = data.get(offset..offset + 4) else {
        unreachable!("sample is well within the readback buffer's own bounds");
    };
    let result = match pixel {
        &[r, g, b, a] => [r, g, b, a],
        _ => unreachable!("sliced exactly 4 bytes"),
    };
    drop(data);
    readback_buffer.unmap();
    result
}

#[test]
fn path_pipeline_fills_a_triangle_with_its_own_solid_colour() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    // A triangle covering the whole 64x64 target -- (0,0), (64,0),
    // (0,64) -- so the center sample is unconditionally inside it,
    // regardless of exactly how the tessellator (trivial here, already
    // a triangle) or the rasterizer's own fill-rule edges behave.
    let mesh = Mesh {
        vertices: vec![
            Point::new(0.0, 0.0),
            Point::new(64.0, 0.0),
            Point::new(0.0, 64.0),
        ],
        indices: vec![0, 1, 2],
    };
    let gpu_mesh = GpuMesh::upload(device, queue, &mesh);

    let mut path = PathPipeline::new(device);
    // Solid, opaque blue.
    let color = [0.0, 0.0, 1.0, 1.0];
    let pixel = render_and_sample_pixel(device, queue, &mut path, &gpu_mesh, color, (16, 16));
    assert_eq!(
        pixel,
        [0, 0, 255, 255],
        "the sampled point is inside the triangle and must show its own fill colour"
    );

    // Outside the triangle (past its own hypotenuse, near the target's
    // opposite corner) must show the pass's own clear colour instead --
    // proof this pipeline only fills what it was actually given, not
    // the whole render target.
    let outside = render_and_sample_pixel(device, queue, &mut path, &gpu_mesh, color, (60, 60));
    assert_eq!(
        outside,
        [0, 0, 0, 255],
        "outside the triangle must show the pass's own black clear colour, not the fill"
    );
}

#[test]
fn path_pipeline_draws_nothing_for_an_empty_mesh() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    let gpu_mesh = GpuMesh::upload(device, queue, &Mesh::default());
    let mut path = PathPipeline::new(device);
    let pixel = render_and_sample_pixel(
        device,
        queue,
        &mut path,
        &gpu_mesh,
        [1.0, 0.0, 0.0, 1.0],
        (32, 32),
    );
    assert_eq!(
        pixel,
        [0, 0, 0, 255],
        "an empty Mesh must draw zero triangles, leaving the clear colour untouched"
    );
}

// ---------------------------------------------------------------------
// Gradient primitive (0.124.0): `GradientPipeline`/`GpuColorMesh`
// through `draw_paint_ops`, read back as real pixels.
// ---------------------------------------------------------------------

/// Largest per-channel difference, in 8-bit steps, a gradient probe may
/// show against its analytic expectation: 8-bit quantization (half a
/// step), the rasterizer's own interpolation precision, and
/// `bilinear_rect`'s documented piecewise-linear error (under half a
/// step at 16 cells) together stay well inside it.
const GRADIENT_TOLERANCE: i32 = 3;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Every channel of every corner differs, and the true bilinear centre
/// is exactly `(0.5, 0.5, 0.5)`, where a single undivided quad would
/// show `(0.0, 0.5, 0.5)` (its diagonal runs top-right to bottom-left).
const CORNERS: GradientCorners = GradientCorners {
    top_left: RED,
    top_right: GREEN,
    bottom_left: BLUE,
    bottom_right: WHITE,
};

/// The hue strip a colour picker draws: red, yellow, green, cyan, blue,
/// magenta, red.
const HUE_STOPS: [[f32; 4]; 7] = [
    RED,
    [1.0, 1.0, 0.0, 1.0],
    GREEN,
    [0.0, 1.0, 1.0, 1.0],
    BLUE,
    [1.0, 0.0, 1.0, 1.0],
    RED,
];

/// The sRGB decode `aurora_color::srgb_to_linear` performs (this crate
/// does not depend on `aurora-color`), for the solid colours a real
/// sRGB-target caller linearizes (`aurora-app`'s
/// `linearize_paint_color`). Sign-symmetric, like the original: a
/// negative channel decodes to a negative linear value, not a positive
/// one.
fn srgb_to_linear(encoded: f32) -> f32 {
    let magnitude = encoded.abs();
    let linear = if magnitude <= 0.04045 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    encoded.signum() * linear
}

fn linearized(color: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = color;
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
}

/// A solid axis-aligned rectangle as a two-triangle [`Mesh`].
fn rect_mesh(x: f32, y: f32, width: f32, height: f32) -> Mesh {
    Mesh {
        vertices: vec![
            Point::new(x, y),
            Point::new(x + width, y),
            Point::new(x, y + height),
            Point::new(x + width, y + height),
        ],
        indices: vec![0, 1, 2, 1, 3, 2],
    }
}

fn solid(context: &GpuContext, mesh: &Mesh, color: [f32; 4]) -> GpuPaintOp {
    GpuPaintOp::Solid(
        GpuMesh::upload(context.device(), context.queue(), mesh),
        color,
    )
}

fn gradient(context: &GpuContext, mesh: &ColorMesh) -> GpuPaintOp {
    GpuPaintOp::Gradient(GpuColorMesh::upload(
        context.device(),
        context.queue(),
        mesh,
    ))
}

/// A rendered target, tightly packed RGBA8 rows.
pub(crate) struct Frame {
    width: u32,
    bytes: Vec<u8>,
}

impl Frame {
    pub(crate) fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        match self.bytes.get(offset..offset + 4) {
            Some(&[red, green, blue, alpha]) => [red, green, blue, alpha],
            _ => unreachable!("pixel ({x}, {y}) is outside the {}-wide frame", self.width),
        }
    }
}

/// Clears a `size` target of `format` to black, draws `ops` through
/// [`draw_paint_ops`] with fresh pipelines, and reads every pixel back.
/// `size.0 * 4` must be a multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`
/// (every caller picks such a width, the same restriction the solid
/// helper above has). An sRGB target reads back its *encoded* bytes.
fn render_ops(
    context: &GpuContext,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    ops: &[GpuPaintOp],
) -> Frame {
    #[allow(clippy::cast_precision_loss)]
    let viewport_size = (size.0 as f32, size.1 as f32);
    render_ops_with_viewport(context, format, size, viewport_size, ops)
}

/// [`render_ops`] with an explicit `viewport_size` (the uniform the
/// mesh's own coordinates are measured against). Passing half the
/// target's physical size is exactly what `aurora-app` does at a scale
/// factor of `2.0`: mesh coordinates are logical, the target physical.
fn render_ops_with_viewport(
    context: &GpuContext,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    viewport_size: (f32, f32),
    ops: &[GpuPaintOp],
) -> Frame {
    let atlas = GlyphAtlas::new(context.device());
    render_ops_with_atlas(context, format, size, viewport_size, ops, &atlas)
}

/// [`render_ops_with_viewport`] against a caller's own glyph atlas — the
/// one its `GpuPaintOp::Text` ops were prepared against.
#[allow(clippy::too_many_lines)]
pub(crate) fn render_ops_with_atlas(
    context: &GpuContext,
    format: wgpu::TextureFormat,
    size: (u32, u32),
    viewport_size: (f32, f32),
    ops: &[GpuPaintOp],
    atlas: &GlyphAtlas,
) -> Frame {
    let device = context.device();
    let queue = context.queue();
    let (width, height) = size;
    assert_eq!((width * 4) % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gradient-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut path = PathPipeline::new(device);
    let mut gradient_pipeline = GradientPipeline::new(device);
    let mut text_pipeline = TextPipeline::new(device);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gradient-render"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gradient"),
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
        draw_paint_ops(
            &mut pass,
            &mut path,
            &mut gradient_pipeline,
            &mut text_pipeline,
            atlas,
            device,
            queue,
            format,
            viewport_size,
            ops,
        );
    }

    let bytes_per_row = width * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gradient-readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let bytes = data.to_vec();
    drop(data);
    readback.unmap();
    Frame { width, bytes }
}

fn to_bytes(color: [f32; 4]) -> [i32; 4] {
    color.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as i32)
}

/// Asserts `pixel`'s RGB is within `tolerance` 8-bit steps of `expected`.
fn assert_rgb_near(pixel: [u8; 4], expected: [f32; 4], tolerance: i32, what: &str) {
    let want = to_bytes(expected);
    for channel in 0..3 {
        let (Some(&got), Some(&want)) = (pixel.get(channel), want.get(channel)) else {
            unreachable!("channel {channel} < 4");
        };
        assert!(
            (i32::from(got) - want).abs() <= tolerance,
            "{what}: pixel {pixel:?} vs expected {:?} (channel {channel}, tolerance {tolerance})",
            to_bytes(expected)
        );
    }
}

/// The largest RGB difference between two pixels, in 8-bit steps.
fn max_rgb_diff(a: [u8; 4], b: [u8; 4]) -> i32 {
    a.iter()
        .zip(b.iter())
        .take(3)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
        .max()
        .unwrap_or(0)
}

/// The true bilinear colour of `corners` at normalized `(u, v)`.
fn bilinear(corners: GradientCorners, u: f32, v: f32) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (index, channel) in out.iter_mut().enumerate() {
        let get = |color: [f32; 4]| color.get(index).copied().unwrap_or(0.0);
        let top = get(corners.top_left) * (1.0 - u) + get(corners.top_right) * u;
        let bottom = get(corners.bottom_left) * (1.0 - u) + get(corners.bottom_right) * u;
        *channel = top * (1.0 - v) + bottom * v;
    }
    out
}

/// The piecewise-linear colour of evenly spaced `stops` at normalized
/// `t` in `[0, 1]`.
fn strip_at(stops: &[[f32; 4]], t: f32) -> [f32; 4] {
    let segments = (stops.len() - 1) as f32;
    let scaled = (t * segments).clamp(0.0, segments);
    // `scaled` is clamped to `[0, segments]` above, so the cast loses no sign.
    #[allow(clippy::cast_sign_loss)]
    let segment = (scaled.floor() as usize).min(stops.len() - 2);
    let local = scaled - segment as f32;
    let (Some(a), Some(b)) = (stops.get(segment), stops.get(segment + 1)) else {
        unreachable!("segment {segment} has two stops");
    };
    let mut out = [0.0; 4];
    for ((channel, x), y) in out.iter_mut().zip(a).zip(b) {
        *channel = x * (1.0 - local) + y * local;
    }
    out
}

/// A pixel's centre as a fraction of `extent` pixels.
fn centre(pixel: u32, extent: u32) -> f32 {
    (pixel as f32 + 0.5) / extent as f32
}

// G1
#[test]
fn gradient_bilinear_rect_corners_show_their_own_colours() {
    let Some(context) = real_context() else {
        return;
    };
    let mesh = bilinear_rect(0.0, 0.0, 256.0, 256.0, CORNERS, DEFAULT_GRADIENT_CELLS);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (256, 256),
        &[gradient(&context, &mesh)],
    );
    for (x, y, corner) in [
        (0, 0, RED),
        (255, 0, GREEN),
        (0, 255, BLUE),
        (255, 255, WHITE),
    ] {
        assert_rgb_near(frame.pixel(x, y), corner, GRADIENT_TOLERANCE, "corner");
    }
}

// G2
#[test]
fn gradient_bilinear_rect_centre_is_the_bilinear_mean_not_a_diagonal() {
    let Some(context) = real_context() else {
        return;
    };
    let mesh = bilinear_rect(0.0, 0.0, 256.0, 256.0, CORNERS, DEFAULT_GRADIENT_CELLS);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (256, 256),
        &[gradient(&context, &mesh)],
    );
    for (x, y) in [(127, 127), (128, 128)] {
        assert_rgb_near(
            frame.pixel(x, y),
            [0.5, 0.5, 0.5, 1.0],
            GRADIENT_TOLERANCE,
            "centre",
        );
    }
    // And a coarse sweep of the whole square against the true bilinear
    // surface, not just its centre.
    for y in (0..256).step_by(17) {
        for x in (0..256).step_by(17) {
            let expected = bilinear(CORNERS, centre(x, 256), centre(y, 256));
            assert_rgb_near(frame.pixel(x, y), expected, GRADIENT_TOLERANCE, "sweep");
        }
    }
}

// G3
#[test]
fn gradient_bilinear_rect_top_row_is_monotonic() {
    let Some(context) = real_context() else {
        return;
    };
    let mesh = bilinear_rect(0.0, 0.0, 256.0, 256.0, CORNERS, DEFAULT_GRADIENT_CELLS);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (256, 256),
        &[gradient(&context, &mesh)],
    );
    for x in 1..256 {
        let (previous, current) = (frame.pixel(x - 1, 0), frame.pixel(x, 0));
        assert!(current[0] <= previous[0], "red rises at x = {x}");
        assert!(current[1] >= previous[1], "green falls at x = {x}");
    }
    // Monotonic is necessary, not sufficient: every top-row pixel must
    // also sit on the analytic red-to-green edge at its own centre.
    for x in 0..256 {
        assert_rgb_near(
            frame.pixel(x, 0),
            bilinear(CORNERS, centre(x, 256), centre(0, 256)),
            GRADIENT_TOLERANCE,
            "top row follows the analytic top edge",
        );
    }
}

// G3b
#[test]
fn gradient_saturation_value_square_matches_hsv() {
    let Some(context) = real_context() else {
        return;
    };
    let hue = RED;
    let corners = GradientCorners {
        top_left: WHITE,
        top_right: hue,
        bottom_left: BLACK,
        bottom_right: BLACK,
    };
    let mesh = bilinear_rect(0.0, 0.0, 256.0, 256.0, corners, DEFAULT_GRADIENT_CELLS);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (256, 256),
        &[gradient(&context, &mesh)],
    );
    for y in [64, 128, 192] {
        for x in [64, 128, 192] {
            let saturation = centre(x, 256);
            let value = 1.0 - centre(y, 256);
            let mut expected = [0.0, 0.0, 0.0, 1.0];
            for (channel, h) in expected.iter_mut().zip(hue).take(3) {
                *channel = value * ((1.0 - saturation) + saturation * h);
            }
            assert_rgb_near(frame.pixel(x, y), expected, GRADIENT_TOLERANCE, "SV square");
        }
    }
}

fn hue_strip_frame(context: &GpuContext, format: wgpu::TextureFormat) -> Frame {
    let mesh = horizontal_strip(0.0, 0.0, 384.0, 8.0, &HUE_STOPS);
    render_ops(context, format, (384, 8), &[gradient(context, &mesh)])
}

/// Mean green of the two pixels either side of the first segment's
/// midpoint (x = 32), in 8-bit steps.
fn first_segment_mid_green(frame: &Frame) -> i32 {
    i32::midpoint(
        i32::from(frame.pixel(31, 4)[1]),
        i32::from(frame.pixel(32, 4)[1]),
    )
}

// G4
#[test]
fn gradient_hue_strip_hits_every_stop_and_ramps_in_gamma_space() {
    let Some(context) = real_context() else {
        return;
    };
    let frame = hue_strip_frame(&context, wgpu::TextureFormat::Rgba8Unorm);
    for (k, stop) in HUE_STOPS.iter().enumerate() {
        let k = k as u32;
        if k > 0 {
            assert_rgb_near(
                frame.pixel(64 * k - 1, 4),
                *stop,
                GRADIENT_TOLERANCE,
                "stop",
            );
        }
        if k < 6 {
            assert_rgb_near(frame.pixel(64 * k, 4), *stop, GRADIENT_TOLERANCE, "stop");
        }
    }
    for x in 0..384 {
        let expected = strip_at(&HUE_STOPS, centre(x, 384));
        assert_rgb_near(frame.pixel(x, 4), expected, GRADIENT_TOLERANCE, "strip");
    }
    let mid = first_segment_mid_green(&frame);
    assert!((mid - 128).abs() <= 2, "red-to-yellow midpoint green {mid}");
    assert_rgb_near(
        frame.pixel(31, 4),
        [1.0, 0.5, 0.0, 1.0],
        GRADIENT_TOLERANCE,
        "midpoint",
    );
}

// G5
#[test]
fn gradient_on_an_srgb_target_interpolates_in_gamma_space_too() {
    let Some(context) = real_context() else {
        return;
    };
    let plain = hue_strip_frame(&context, wgpu::TextureFormat::Rgba8Unorm);
    let srgb = hue_strip_frame(&context, wgpu::TextureFormat::Rgba8UnormSrgb);
    let mid = first_segment_mid_green(&srgb);
    assert!(
        (mid - 128).abs() <= 3,
        "red-to-yellow midpoint on an sRGB target reads green {mid}; 128 is gamma-space \
         interpolation, about 188 would be linear-light (the fragment was not linearized)"
    );
    for x in 0..384 {
        let (a, b) = (plain.pixel(x, 4), srgb.pixel(x, 4));
        assert!(
            max_rgb_diff(a, b) <= 2,
            "x = {x}: plain {a:?} vs sRGB {b:?} -- the same mesh must look the same"
        );
    }
}

/// A flat grey gradient must encode to the same byte a solid shape does
/// when its caller linearizes the colour the way `aurora-app` does.
/// `0.5` and `0.2`: a `2.2` exponent in place of `2.4` moves `0.2` by
/// about eight steps.
#[test]
fn flat_gradient_matches_a_linearized_solid_on_an_srgb_target() {
    let Some(context) = real_context() else {
        return;
    };
    for grey in [0.5_f32, 0.2] {
        let colour = [grey, grey, grey, 1.0];
        let flat = GradientCorners {
            top_left: colour,
            top_right: colour,
            bottom_left: colour,
            bottom_right: colour,
        };
        let frame = render_ops(
            &context,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            (64, 8),
            &[
                gradient(&context, &bilinear_rect(0.0, 0.0, 32.0, 8.0, flat, 4)),
                solid(
                    &context,
                    &rect_mesh(32.0, 0.0, 32.0, 8.0),
                    linearized(colour),
                ),
            ],
        );
        let (from_gradient, from_solid) = (frame.pixel(16, 4), frame.pixel(48, 4));
        assert!(
            max_rgb_diff(from_gradient, from_solid) <= 1,
            "grey {grey}: gradient {from_gradient:?} vs solid {from_solid:?}"
        );
        assert_rgb_near(
            from_gradient,
            colour,
            1,
            "flat gradient encodes back to its grey",
        );
    }
}

// G6
#[test]
fn draw_paint_ops_interleaves_solids_and_gradients_in_paint_order() {
    let Some(context) = real_context() else {
        return;
    };
    let green_to_white = horizontal_strip(0.0, 0.0, 64.0, 64.0, &[GREEN, WHITE]);
    let corner_strip = horizontal_strip(96.0, 48.0, 32.0, 16.0, &[WHITE, WHITE]);
    // Solid, gradient, solid, gradient: every kind switch happens.
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (128, 64),
        &[
            solid(&context, &rect_mesh(0.0, 0.0, 128.0, 64.0), RED),
            gradient(&context, &green_to_white),
            solid(&context, &rect_mesh(16.0, 16.0, 32.0, 32.0), BLUE),
            gradient(&context, &corner_strip),
        ],
    );
    assert_rgb_near(
        frame.pixel(100, 20),
        RED,
        0,
        "solid red beside the gradient",
    );
    assert_rgb_near(
        frame.pixel(32, 32),
        BLUE,
        0,
        "solid blue on top of the gradient",
    );
    assert_rgb_near(
        frame.pixel(8, 56),
        strip_at(&[GREEN, WHITE], centre(8, 64)),
        GRADIENT_TOLERANCE,
        "gradient over the red, outside the blue",
    );
    assert_rgb_near(
        frame.pixel(60, 4),
        strip_at(&[GREEN, WHITE], centre(60, 64)),
        GRADIENT_TOLERANCE,
        "gradient over the red, outside the blue",
    );
    assert_rgb_near(
        frame.pixel(110, 56),
        WHITE,
        0,
        "the last gradient drew on top",
    );

    // Gradient first, then a solid on top of it.
    let full = horizontal_strip(0.0, 0.0, 128.0, 64.0, &[GREEN, WHITE]);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (128, 64),
        &[
            gradient(&context, &full),
            solid(&context, &rect_mesh(16.0, 16.0, 32.0, 32.0), BLUE),
        ],
    );
    assert_rgb_near(frame.pixel(32, 32), BLUE, 0, "solid on top of the gradient");
    assert_rgb_near(
        frame.pixel(100, 8),
        strip_at(&[GREEN, WHITE], centre(100, 128)),
        GRADIENT_TOLERANCE,
        "gradient outside the solid",
    );
}

// G7
#[test]
fn an_empty_gradient_mesh_draws_nothing() {
    let Some(context) = real_context() else {
        return;
    };
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (64, 8),
        &[gradient(&context, &ColorMesh::default())],
    );
    assert!(
        frame
            .bytes
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 0, 255]),
        "an empty ColorMesh must leave the black clear untouched"
    );
}

// G8
#[test]
fn gradient_alpha_is_interpolated_and_blended_straight() {
    let Some(context) = real_context() else {
        return;
    };
    let ramp = horizontal_strip(0.0, 0.0, 256.0, 8.0, &[[1.0, 1.0, 1.0, 0.0], WHITE]);
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8Unorm,
        (256, 8),
        &[gradient(&context, &ramp)],
    );
    for x in [0, 64, 127, 128, 192, 255] {
        let alpha = centre(x, 256);
        assert_rgb_near(
            frame.pixel(x, 4),
            [alpha, alpha, alpha, 1.0],
            GRADIENT_TOLERANCE,
            "white over black at the interpolated alpha",
        );
    }
}

// G8, on an sRGB target: the documented difference, recorded rather than
// hidden. A translucent gradient blends in the target's own blend space,
// exactly as a solid fill does -- so the "same bytes on both targets"
// promise is for opaque fragments only.
#[test]
fn translucent_gradient_blends_in_the_targets_own_space_like_a_solid() {
    let Some(context) = real_context() else {
        return;
    };
    let half_white = [1.0, 1.0, 1.0, 0.5];
    let flat = GradientCorners {
        top_left: half_white,
        top_right: half_white,
        bottom_left: half_white,
        bottom_right: half_white,
    };
    let draw = |format: wgpu::TextureFormat, solid_colour: [f32; 4]| {
        render_ops(
            &context,
            format,
            (64, 8),
            &[
                gradient(&context, &bilinear_rect(0.0, 0.0, 32.0, 8.0, flat, 4)),
                solid(&context, &rect_mesh(32.0, 0.0, 32.0, 8.0), solid_colour),
            ],
        )
    };
    // Plain target: blended in gamma space, white at 0.5 over black is 128.
    let plain = draw(wgpu::TextureFormat::Rgba8Unorm, half_white);
    assert_rgb_near(
        plain.pixel(16, 4),
        [0.5, 0.5, 0.5, 1.0],
        1,
        "plain target: half-alpha white over black blends in gamma space",
    );
    // sRGB target: blended in linear light, then encoded -- linear 0.5
    // encodes to about 0.735, byte 188, not 128.
    let srgb = draw(wgpu::TextureFormat::Rgba8UnormSrgb, linearized(half_white));
    let (from_gradient, from_solid) = (srgb.pixel(16, 4), srgb.pixel(48, 4));
    for channel in from_gradient.iter().take(3) {
        assert!(
            (i32::from(*channel) - 188).abs() <= 1,
            "sRGB target: half-alpha white over black must blend in linear light \
             (about 188), got {from_gradient:?}"
        );
    }
    assert!(
        max_rgb_diff(from_gradient, from_solid) <= 1,
        "a translucent gradient blends exactly as a linearized solid does: \
         gradient {from_gradient:?} vs solid {from_solid:?}"
    );
}

// Out-of-range colours: a finite channel outside [0, 1] is decoded with
// the same sign-symmetric curve the solid path uses, then clamped by the
// target on store. A negative channel must stay negative (byte 0): the
// shader's `sign(c) * l` is load-bearing, and dropping the `sign(c)`
// would decode -1.0 as +1.0 and store 255.
#[test]
fn out_of_range_flat_gradient_matches_a_sign_symmetrically_linearized_solid() {
    let Some(context) = real_context() else {
        return;
    };
    let colour = [2.0, -1.0, 0.5, 1.0];
    let flat = GradientCorners {
        top_left: colour,
        top_right: colour,
        bottom_left: colour,
        bottom_right: colour,
    };
    let frame = render_ops(
        &context,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        (64, 8),
        &[
            gradient(&context, &bilinear_rect(0.0, 0.0, 32.0, 8.0, flat, 4)),
            solid(
                &context,
                &rect_mesh(32.0, 0.0, 32.0, 8.0),
                linearized(colour),
            ),
        ],
    );
    let (from_gradient, from_solid) = (frame.pixel(16, 4), frame.pixel(48, 4));
    assert!(
        max_rgb_diff(from_gradient, from_solid) <= 1,
        "out-of-range gradient {from_gradient:?} vs linearized solid {from_solid:?}"
    );
    assert_eq!(
        from_gradient[1], 0,
        "a negative channel must decode negative and clamp to 0, got {from_gradient:?}"
    );
    assert_eq!(
        from_gradient[0], 255,
        "a channel above one must clamp to 255"
    );
}

// HiDPI: at a scale factor of 2 the app passes a logical viewport half
// the physical target's size. A solid and a gradient over the same
// logical rectangle must cover exactly the same physical pixels, and
// that must be the doubled rectangle.
#[test]
fn solid_and_gradient_cover_the_same_physical_pixels_at_scale_factor_two() {
    let Some(context) = real_context() else {
        return;
    };
    let (physical, logical) = ((128, 64), (64.0, 32.0));
    let (x, y, width, height) = (5.0, 3.0, 21.0, 11.0);
    let covered = |op: GpuPaintOp| -> Vec<bool> {
        render_ops_with_viewport(
            &context,
            wgpu::TextureFormat::Rgba8Unorm,
            physical,
            logical,
            &[op],
        )
        .bytes
        .chunks_exact(4)
        .map(|pixel| pixel.iter().take(3).any(|&channel| channel != 0))
        .collect()
    };
    let from_solid = covered(solid(&context, &rect_mesh(x, y, width, height), WHITE));
    let from_gradient = covered(gradient(
        &context,
        &horizontal_strip(x, y, width, height, &[WHITE, WHITE]),
    ));
    assert_eq!(
        from_solid, from_gradient,
        "solid and gradient must rasterize the same logical rect identically"
    );
    let expected: Vec<bool> = (0..physical.1)
        .flat_map(|row| (0..physical.0).map(move |column| (row, column)))
        .map(|(row, column)| (10..52).contains(&column) && (6..28).contains(&row))
        .collect();
    assert_eq!(
        from_gradient, expected,
        "logical (5, 3, 21, 11) at scale 2 must cover physical x 10..52, y 6..28"
    );
}

// ---- Text (0.132.0) -------------------------------------------------

mod text {
    use super::{Frame, rect_mesh, render_ops_with_atlas, solid};
    use crate::paint::{PaintOp, paint_widget_ops};
    use crate::render::{GlyphAtlas, GpuPaintOp, TextPipeline, upload_paint_ops};
    use crate::test_support::real_context;
    use crate::text::{HAlign, TextRun, label_style, resolve_text};
    use crate::widgets::{insert_button, new_tree, test_scales};
    use aurora_core::Rect;
    use aurora_gpu::GpuContext;
    use aurora_text::TextEngine;
    use aurora_theme::{Palette, Theme, ThemeSet};

    const SIZE: (u32, u32) = (128, 64);
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    fn engine() -> TextEngine {
        match TextEngine::new() {
            Ok(engine) => engine,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn dark_theme() -> Theme {
        let Ok(palette) =
            Palette::from_toml_str(include_str!("../../../design/tokens/palette.toml"))
        else {
            unreachable!()
        };
        let mut themes = ThemeSet::new();
        if themes
            .register(include_str!("../../../design/themes/dark.toml"))
            .is_err()
        {
            unreachable!()
        }
        match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn run(text: &str, rect: (f32, f32, f32, f32), clip: Rect) -> TextRun {
        TextRun {
            text: text.to_owned(),
            style: label_style(&test_scales()),
            color: WHITE,
            rect,
            align: HAlign::Start,
            clip,
            field: None,
        }
    }

    fn whole() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: SIZE.0,
            height: SIZE.1,
        }
    }

    fn render(context: &GpuContext, atlas: &mut GlyphAtlas, ops: Vec<PaintOp>) -> Frame {
        let mut engine = engine();
        let gpu = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            atlas,
            ops,
            1.0,
            |c| c,
        );
        #[allow(clippy::cast_precision_loss)]
        let viewport = (SIZE.0 as f32, SIZE.1 as f32);
        render_ops_with_atlas(
            context,
            wgpu::TextureFormat::Rgba8Unorm,
            SIZE,
            viewport,
            &gpu,
            atlas,
        )
    }

    fn lit(frame: &Frame, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        let mut count = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if frame.pixel(x, y)[0] > 0 {
                    count += 1;
                }
            }
        }
        count
    }

    /// Renders `text` white-on-black at `scale` (a 128x64 physical
    /// target, logical viewport `SIZE / scale`) clipped to `clip`
    /// (logical), and compares every pixel against the CPU coverage mask
    /// of the glyph quad it falls in — `engine.glyph(key).alpha`, trimmed
    /// by the quad's `src_offset` exactly as the shader must read it.
    /// Returns (pixels compared inside quads, of which unlit, of which
    /// nearly fully covered).
    fn assert_glyph_pixels_match_the_cpu_masks(
        context: &GpuContext,
        text: &str,
        scale: f32,
        clip: Rect,
    ) -> (usize, usize, usize) {
        let mut engine = engine();
        let mut atlas = GlyphAtlas::new(context.device());
        let label = run(text, (4.0, 4.0, 100.0, 20.0), clip);
        let quads = resolve_text(&mut engine, &label, scale);
        assert!(!quads.is_empty(), "{text} at {scale} draws something");
        let gpu = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            &mut atlas,
            vec![PaintOp::Text(label)],
            scale,
            |c| c,
        );
        #[allow(clippy::cast_precision_loss)]
        let viewport = (SIZE.0 as f32 / scale, SIZE.1 as f32 / scale);
        let frame = render_ops_with_atlas(
            context,
            wgpu::TextureFormat::Rgba8Unorm,
            SIZE,
            viewport,
            &gpu,
            &atlas,
        );
        let mut expected = vec![None::<u8>; (SIZE.0 * SIZE.1) as usize];
        for quad in &quads {
            let Some(mask) = engine.glyph(quad.key) else {
                unreachable!("a quad is only emitted for an inked glyph")
            };
            let [x0, y0, x1, y1] = quad.dst;
            let [ox, oy] = quad.src_offset;
            for y in y0..y1 {
                for x in x0..x1 {
                    let (Ok(px), Ok(py)) = (u32::try_from(x), u32::try_from(y)) else {
                        continue;
                    };
                    if px >= SIZE.0 || py >= SIZE.1 {
                        continue;
                    }
                    let (Ok(mx), Ok(my)) = (u32::try_from(x - x0), u32::try_from(y - y0)) else {
                        unreachable!()
                    };
                    let index = ((my + oy) * mask.width + (mx + ox)) as usize;
                    let Some(&coverage) = mask.alpha.get(index) else {
                        unreachable!("src window inside the mask")
                    };
                    if let Some(slot) = expected.get_mut((py * SIZE.0 + px) as usize) {
                        // Overlapping quads (never in these strings) would
                        // blend; keep the larger coverage as a bound.
                        *slot = Some(slot.map_or(coverage, |c| c.max(coverage)));
                    }
                }
            }
        }
        let (mut compared, mut unlit, mut full) = (0, 0, 0);
        for y in 0..SIZE.1 {
            for x in 0..SIZE.0 {
                let got = frame.pixel(x, y)[0];
                match expected.get((y * SIZE.0 + x) as usize).copied().flatten() {
                    Some(want) => {
                        assert!(
                            got.abs_diff(want) <= 2,
                            "{text} @{scale}: pixel ({x}, {y}) is {got}, mask says {want}"
                        );
                        compared += 1;
                        if want == 0 {
                            unlit += 1;
                        }
                        if want >= 200 {
                            full += 1;
                        }
                    }
                    None => assert_eq!(
                        got, 0,
                        "{text} @{scale}: ink at ({x}, {y}) outside every quad"
                    ),
                }
            }
        }
        (compared, unlit, full)
    }

    #[test]
    fn glyph_pixels_match_the_cpu_coverage_mask_texel_for_texel() {
        let Some(context) = real_context() else {
            return;
        };
        for scale in [1.0, 2.0] {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let clip = Rect {
                x: 0,
                y: 0,
                width: (SIZE.0 as f32 / scale) as u32,
                height: (SIZE.1 as f32 / scale) as u32,
            };
            for text in ["A", "HAWK"] {
                let (compared, unlit, full) =
                    assert_glyph_pixels_match_the_cpu_masks(&context, text, scale, clip);
                assert!(compared > 50, "{text} @{scale}: {compared} pixels compared");
                assert!(
                    unlit > 5,
                    "{text} @{scale}: an A's counter and corners stay unlit"
                );
                assert!(
                    full > 5,
                    "{text} @{scale}: some stem pixels are fully covered"
                );
            }
        }
    }

    #[test]
    fn a_clip_through_a_glyph_reads_the_trimmed_part_of_its_mask() {
        let Some(context) = real_context() else {
            return;
        };
        for scale in [1.0, 2.0] {
            // A narrow window starting inside "H", "A" and "W": every quad
            // is trimmed on the left (and "HAWK"'s tops by the y clip).
            let clip = Rect {
                x: 7,
                y: 9,
                width: 14,
                height: 20,
            };
            let (compared, _, full) =
                assert_glyph_pixels_match_the_cpu_masks(&context, "HAWK", scale, clip);
            assert!(
                compared > 20,
                "@{scale}: {compared} clipped pixels compared"
            );
            assert!(full > 0, "@{scale}: the clipped stem still draws");
        }
    }

    #[test]
    fn a_frames_runs_share_one_glyph_buffer_and_an_empty_run_uploads_nothing() {
        let Some(context) = real_context() else {
            return;
        };
        let mut engine = engine();
        let mut atlas = GlyphAtlas::new(context.device());
        let grey = [0.25, 0.25, 0.25, 1.0];
        let mut ops = vec![PaintOp::Text(run("One", (4.0, 4.0, 100.0, 20.0), whole()))];
        ops.push(PaintOp::Text(run("   ", (4.0, 24.0, 100.0, 20.0), whole())));
        ops.push(PaintOp::Solid((rect_mesh(0.0, 40.0, 10.0, 50.0), grey)));
        ops.push(PaintOp::Text(run("Two", (4.0, 40.0, 100.0, 20.0), whole())));
        let gpu = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            &mut atlas,
            ops,
            1.0,
            |c| c,
        );
        let meshes: Vec<&crate::render::GpuGlyphMesh> = gpu
            .iter()
            .filter_map(|op| match op {
                GpuPaintOp::Text(mesh, _) => Some(mesh),
                _ => None,
            })
            .collect();
        assert_eq!(gpu.len(), 3, "the all-space run uploads no op");
        assert!(
            matches!(gpu.get(1), Some(GpuPaintOp::Solid(..))),
            "paint order kept"
        );
        let [first, second] = meshes.as_slice() else {
            unreachable!("two inked runs")
        };
        assert!(
            first.shares_buffers_with(second),
            "one vertex + index buffer per frame"
        );
        assert_eq!(first.indices(), 0..18);
        assert_eq!(second.indices(), 18..36);
        // A frame of nothing but spaces uploads no text op at all.
        let spaces = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            &mut atlas,
            vec![PaintOp::Text(run("  ", (4.0, 4.0, 100.0, 20.0), whole()))],
            1.0,
            |c| c,
        );
        assert!(spaces.is_empty());
    }

    #[test]
    fn warm_frame_text_collect_time_is_reported() {
        let Some(context) = real_context() else {
            return;
        };
        let mut engine = engine();
        let mut atlas = GlyphAtlas::new(context.device());
        let labels: Vec<PaintOp> = (0..60)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let y = (i % 3) as f32 * 20.0;
                PaintOp::Text(run(&format!("Label {i}"), (4.0, y, 120.0, 20.0), whole()))
            })
            .collect();
        let mut collect = || {
            engine.begin_frame();
            let start = std::time::Instant::now();
            let ops = upload_paint_ops(
                context.device(),
                context.queue(),
                &mut engine,
                &mut atlas,
                labels.clone(),
                1.0,
                |c| c,
            );
            (start.elapsed(), ops.len())
        };
        let (cold, n) = collect();
        let (warm, m) = collect();
        assert_eq!(n, m);
        // Informational only -- no budget is claimed or asserted.
        eprintln!("text collect: 60 labels, cold {cold:?}, warm {warm:?}");
    }

    #[test]
    fn text_pipeline_builds_for_every_target_format() {
        let Some(context) = real_context() else {
            return;
        };
        let mut text = TextPipeline::new(context.device());
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            let _ = text.pipeline(context.device(), format);
        }
        assert_eq!(text.cached_pipelines(), 4);
    }

    #[test]
    fn a_text_run_draws_coverage_inside_its_rect() {
        let Some(context) = real_context() else {
            return;
        };
        let mut atlas = GlyphAtlas::new(context.device());
        let frame = render(
            &context,
            &mut atlas,
            vec![PaintOp::Text(run(
                "Hello",
                (4.0, 4.0, 100.0, 20.0),
                whole(),
            ))],
        );
        let inside = lit(&frame, 4, 4, 60, 24);
        assert!(inside > 40, "only {inside} lit pixels for \"Hello\"");
        assert!(
            (4..24).any(|y| (4..60).any(|x| frame.pixel(x, y)[0] >= 200)),
            "a stem of H is (nearly) fully covered"
        );
        assert_eq!(
            lit(&frame, 0, 30, SIZE.0, SIZE.1),
            0,
            "nothing below the line"
        );
    }

    #[test]
    fn no_text_pixels_outside_the_clip_rect() {
        let Some(context) = real_context() else {
            return;
        };
        let mut atlas = GlyphAtlas::new(context.device());
        let clip = Rect {
            x: 12,
            y: 0,
            width: 14,
            height: 16,
        };
        let frame = render(
            &context,
            &mut atlas,
            vec![PaintOp::Text(run("Hello", (4.0, 4.0, 100.0, 20.0), clip))],
        );
        let total = lit(&frame, 0, 0, SIZE.0, SIZE.1);
        let inside = lit(&frame, 12, 0, 26, 16);
        assert!(inside > 0, "some ink survives inside the clip");
        assert_eq!(total, inside, "no ink outside the clip rect");
    }

    #[test]
    fn text_draws_after_its_background_and_under_a_later_popover() {
        let Some(context) = real_context() else {
            return;
        };
        let mut engine = engine();
        let mut atlas = GlyphAtlas::new(context.device());
        let grey = [0.25, 0.25, 0.25, 1.0];
        let blue = [0.0, 0.0, 1.0, 1.0];
        let label = run("HHHH", (4.0, 4.0, 100.0, 20.0), whole());
        let ops = vec![PaintOp::Text(label)];
        let text_ops = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            &mut atlas,
            ops,
            1.0,
            |c| c,
        );
        let mut gpu = vec![solid(&context, &rect_mesh(0.0, 0.0, 128.0, 32.0), grey)];
        gpu.extend(text_ops);
        // A "popover" covering the right half of the label.
        gpu.push(solid(&context, &rect_mesh(20.0, 0.0, 108.0, 32.0), blue));
        let frame = render_ops_with_atlas(
            &context,
            wgpu::TextureFormat::Rgba8Unorm,
            SIZE,
            (128.0, 64.0),
            &gpu,
            &atlas,
        );
        let white_on_left = (4..24).any(|y| (4..20).any(|x| frame.pixel(x, y)[0] > 200));
        assert!(white_on_left, "the label draws over its grey background");
        for y in 0..32 {
            for x in 20..128 {
                let [r, g, b, _] = frame.pixel(x, y);
                assert_eq!(
                    (r, g, b),
                    (0, 0, 255),
                    "popover covers the label at ({x}, {y})"
                );
            }
        }
        assert!(matches!(gpu.get(1), Some(GpuPaintOp::Text(..))));
    }

    #[test]
    fn atlas_reset_on_full_still_draws_the_current_frame() {
        let Some(context) = real_context() else {
            return;
        };
        let mut atlas = GlyphAtlas::with_size(context.device(), 32);
        let _ = render(
            &context,
            &mut atlas,
            vec![PaintOp::Text(run("MWQ", (4.0, 4.0, 100.0, 20.0), whole()))],
        );
        let frame = render(
            &context,
            &mut atlas,
            vec![PaintOp::Text(run("BDKR", (4.0, 4.0, 100.0, 20.0), whole()))],
        );
        assert!(atlas.layout().resets() >= 1, "the small atlas really reset");
        assert!(
            lit(&frame, 4, 4, 60, 24) > 40,
            "the post-reset frame still draws"
        );
    }

    #[test]
    fn button_label_pixels_differ_from_the_fill_inside_the_label_rect() {
        let Some(context) = real_context() else {
            return;
        };
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let place = |tree: &mut crate::tree::WidgetTree<crate::widgets::WidgetKind>, id, w, h| {
            if tree
                .set_bounds(
                    id,
                    Rect {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    },
                )
                .is_err()
            {
                unreachable!()
            }
        };
        place(&mut tree, root, 128, 64);
        let Ok(button) = insert_button(&mut tree, root, &scales, "Apply") else {
            unreachable!()
        };
        place(&mut tree, button, 128, 32);
        let theme = dark_theme();
        let Ok(ops) = paint_widget_ops(&tree, button, &theme, &scales, 1.0) else {
            unreachable!()
        };
        let without_text: Vec<PaintOp> = ops
            .iter()
            .filter(|op| !matches!(op, PaintOp::Text(_)))
            .cloned()
            .collect();
        let mut atlas = GlyphAtlas::new(context.device());
        let with = render(&context, &mut atlas, ops);
        let without = render(&context, &mut atlas, without_text);
        let mut differing = 0;
        let mut outside_differing = 0;
        for y in 0..32 {
            for x in 0..128 {
                if with.pixel(x, y) != without.pixel(x, y) {
                    if (40..88).contains(&x) {
                        differing += 1;
                    } else {
                        outside_differing += 1;
                    }
                }
            }
        }
        assert!(
            differing > 40,
            "only {differing} label pixels in the centred label box"
        );
        assert_eq!(
            outside_differing, 0,
            "a centred 'Apply' stays near the middle"
        );
    }

    // ---- Text fields (0.133.0) ----------------------------------------

    /// `to_srgb_f32` of a token, quantized the way an `Rgba8Unorm` target
    /// stores it.
    fn token_u8(color: aurora_theme::Color) -> [u8; 3] {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        color
            .to_srgb_f32()
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    fn near(pixel: [u8; 4], want: [u8; 3], tolerance: u8) -> bool {
        pixel
            .iter()
            .zip(want)
            .all(|(have, want)| have.abs_diff(want) <= tolerance)
    }

    /// A focused text field holding `content`, laid out at logical
    /// (4, 4, 56, 24), painted by `paint_widget_ops_frame` at `scale`,
    /// rendered, and resolved again on the CPU for the expected geometry.
    fn render_field(
        context: &aurora_gpu::GpuContext,
        content: &str,
        cursor: usize,
        anchor: Option<usize>,
        scale: f32,
    ) -> (Frame, Vec<crate::text::Resolved>, [i32; 4]) {
        use crate::widgets::{insert_text_field, with_text_field_mut};
        let (mut tree, root) = new_tree(taffy::Style::default());
        let scales = test_scales();
        let theme = dark_theme();
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        if tree.set_bounds(root, rect(0, 0, 64, 32)).is_err() {
            unreachable!()
        }
        let Ok(id) = insert_text_field(&mut tree, root, &scales, "Field", content) else {
            unreachable!()
        };
        // The field's real laid-out height (`insert_text_field`'s style
        // is `row_height` tall), not an arbitrary taller box (C3).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let height = crate::widgets::row_height(&scales) as u32;
        if tree.set_bounds(id, rect(4, 4, 56, height)).is_err()
            || with_text_field_mut(&mut tree, id, |s| {
                s.cursor = cursor;
                s.selection_anchor = anchor;
            })
            .is_err()
        {
            unreachable!()
        }
        let Ok(ops) =
            crate::paint::paint_widget_ops_frame(&tree, id, None, Some(id), &theme, &scales, scale)
        else {
            unreachable!()
        };
        let mut engine = engine();
        let pieces: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                PaintOp::Text(run) => Some(crate::text::resolve_run(&mut engine, run, scale)),
                _ => None,
            })
            .flatten()
            .collect();
        let mut atlas = GlyphAtlas::new(context.device());
        let gpu = upload_paint_ops(
            context.device(),
            context.queue(),
            &mut engine,
            &mut atlas,
            ops,
            scale,
            |c| c,
        );
        #[allow(clippy::cast_precision_loss)]
        let viewport = (SIZE.0 as f32 / scale, SIZE.1 as f32 / scale);
        let frame = render_ops_with_atlas(
            context,
            wgpu::TextureFormat::Rgba8Unorm,
            SIZE,
            viewport,
            &gpu,
            &atlas,
        );
        let pad = scales.spacing.sm;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let clip = [
            ((4 + pad) as f32 * scale).round() as i32,
            // Inset by the 1 px control outline vertically.
            (5.0 * scale).round() as i32,
            ((60 - pad) as f32 * scale).round() as i32,
            ((4 + height - 1) as f32 * scale).round() as i32,
        ];
        (frame, pieces, clip)
    }

    fn last_rect(pieces: &[crate::text::Resolved]) -> [i32; 4] {
        match pieces.last() {
            Some(crate::text::Resolved::Rect(rect, _)) => *rect,
            _ => unreachable!("the caret is the last piece"),
        }
    }

    fn pixels(rect: [i32; 4]) -> impl Iterator<Item = (u32, u32)> {
        let [x0, y0, x1, y1] = rect.map(|v| u32::try_from(v).unwrap_or(0));
        (y0..y1).flat_map(move |y| (x0..x1).map(move |x| (x, y)))
    }

    /// The caret really reaches the screen: every pixel of the caret's
    /// column (at the physical x `resolve_run` computes from the shaped
    /// line's `caret_x`) is `text.primary`, and the column just right of
    /// it — past the end of the line — is the field's `surface.sunken`
    /// fill, at scale 1 and 2.
    #[test]
    fn a_focused_fields_caret_column_is_text_primary_on_surface_sunken() {
        let Some(context) = real_context() else {
            return;
        };
        let theme = dark_theme();
        let primary = token_u8(theme.text.primary);
        let sunken = token_u8(theme.surface.sunken);
        for scale in [1.0_f32, 2.0] {
            let (frame, pieces, _) = render_field(&context, "Hi", 2, None, scale);
            let caret = last_rect(&pieces);
            #[allow(clippy::cast_possible_truncation)]
            let width = (scale.round()) as i32;
            assert_eq!(caret[2] - caret[0], width, "scale {scale}");
            let mut count = 0;
            for (x, y) in pixels(caret) {
                count += 1;
                assert!(
                    near(frame.pixel(x, y), primary, 2),
                    "caret pixel ({x},{y}) at scale {scale}: {:?}",
                    frame.pixel(x, y)
                );
            }
            assert!(count >= 10, "a caret of real height: {count}");
            for (x, y) in pixels([caret[2], caret[1], caret[2] + 1, caret[3]]) {
                assert!(
                    near(frame.pixel(x, y), sunken, 2),
                    "right of the caret ({x},{y}) at scale {scale}: {:?}",
                    frame.pixel(x, y)
                );
            }
        }
    }

    /// Inside the selection highlight every pixel lies on the
    /// `accent.primary` → `text.on_accent` blend line (the fill, or the
    /// selected glyphs redrawn over it), some of them are inked, and the
    /// highlight really is `accent.primary` where there is no ink.
    #[test]
    fn a_selection_is_accent_primary_with_on_accent_glyphs() {
        let Some(context) = real_context() else {
            return;
        };
        let theme = dark_theme();
        let accent = token_u8(theme.accent.primary).map(f32::from);
        let on_accent = token_u8(theme.text.on_accent).map(f32::from);
        for scale in [1.0_f32, 2.0] {
            let (frame, pieces, _) = render_field(&context, "Hello", 4, Some(0), scale);
            let Some(&crate::text::Resolved::Rect(fill, _)) = pieces.get(1) else {
                unreachable!("the selection highlight follows the line");
            };
            let (mut plain, mut inked) = (0, 0);
            for (x, y) in pixels(fill) {
                let [red, green, blue, _] = frame.pixel(x, y);
                let pixel = [f32::from(red), f32::from(green), f32::from(blue)];
                // Distance from the accent→on_accent segment.
                let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
                let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let d = sub(on_accent, accent);
                let v = sub(pixel, accent);
                let t = (dot(v, d) / dot(d, d)).clamp(0.0, 1.0);
                let rest = sub(v, d.map(|c| c * t));
                let off = dot(rest, rest).sqrt();
                assert!(
                    off <= 6.0,
                    "({x},{y}) at {scale} is off the blend line: {pixel:?}"
                );
                if t < 0.02 {
                    plain += 1;
                } else if t > 0.3 {
                    inked += 1;
                }
            }
            assert!(
                plain > 0 && inked > 0,
                "scale {scale}: {plain} plain, {inked} inked"
            );
        }
    }

    /// A line wider than its field is scrolled so the caret at its end
    /// sits inside the inner box's right edge, and no pixel left of the
    /// inner box (the field's own padding) is inked.
    #[test]
    fn an_overflowing_field_shows_its_caret_inside_and_no_ink_in_its_padding() {
        let Some(context) = real_context() else {
            return;
        };
        let theme = dark_theme();
        let sunken = token_u8(theme.surface.sunken);
        let text = "The quick brown fox jumps";
        for scale in [1.0_f32, 2.0] {
            let (frame, pieces, clip) = render_field(&context, text, text.len(), None, scale);
            let caret = last_rect(&pieces);
            assert!(
                caret[2] <= clip[2] && caret[0] >= clip[0],
                "{caret:?} in {clip:?}"
            );
            assert!(
                caret[2] >= clip[2] - (2.0 * scale) as i32,
                "pinned to the right edge: {caret:?} {clip:?}"
            );
            // The padding strip left of the inner box, inside the field's
            // outline: only the field's own fill.
            let inset = (2.0 * scale) as i32;
            for (x, y) in pixels([clip[0] - inset, caret[1], clip[0], caret[3]]) {
                assert!(
                    near(frame.pixel(x, y), sunken, 2),
                    "padding ({x},{y}) at {scale}: {:?}",
                    frame.pixel(x, y)
                );
            }
            for (x, y) in pixels(caret) {
                assert!(near(frame.pixel(x, y), token_u8(theme.text.primary), 2));
            }
        }
    }

    /// M18b: `upload_paint_ops` makes the whole frame's glyphs resident in
    /// ONE `prepare`. A per-run prepare would, on a frame whose second run
    /// overflows an atlas still holding the previous frame's glyphs,
    /// reset the atlas and strand the first run's glyphs; with one
    /// prepare every run's glyph maps to a slot and draws.
    #[test]
    fn one_prepare_per_frame_keeps_every_runs_glyphs_across_an_atlas_reset() {
        use crate::render::AtlasLayout;
        use aurora_text::GlyphKey;
        let Some(context) = real_context() else {
            return;
        };
        let mut engine = engine();
        let style = aurora_text::TextStyle {
            size_px: 40.0,
            ..label_style(&test_scales())
        };
        let run = |text: &str| TextRun {
            text: text.to_owned(),
            style,
            color: WHITE,
            rect: (0.0, 0.0, 2000.0, 60.0),
            align: HAlign::Start,
            clip: Rect {
                x: 0,
                y: 0,
                width: 2000,
                height: 60,
            },
            field: None,
        };
        let (stale, a, b) = ("ABCDEFGHIJKLMNOP", "abcdefgh", "qrstuvwxyz");
        let keys = |engine: &mut TextEngine, text: &str| -> Vec<GlyphKey> {
            resolve_text(engine, &run(text), 1.0)
                .iter()
                .map(|q| q.key)
                .collect()
        };
        let (ks, ka, kb) = (
            keys(&mut engine, stale),
            keys(&mut engine, a),
            keys(&mut engine, b),
        );
        let kab: Vec<GlyphKey> = ka.iter().chain(&kb).copied().collect();
        let resident = |l: &AtlasLayout, k: &[GlyphKey]| k.iter().all(|k| l.slot(*k).is_some());
        // The smallest atlas where: the stale frame fits; the stale
        // frame plus run A fits (so a per-run prepare places A without
        // resetting); run B then does not (so a per-run prepare resets
        // and evicts A); yet A and B together fit a fresh atlas.
        let size = (64..=2048).step_by(8).find(|&size| {
            let mut per_run = AtlasLayout::new(size);
            let fits_stale = per_run.place_all(&mut engine, &ks).1;
            let fits_a = per_run.place_all(&mut engine, &ka).1 && resident(&per_run, &ks);
            let _ = per_run.place_all(&mut engine, &kb);
            let a_evicted = !resident(&per_run, &ka);
            let mut fresh = AtlasLayout::new(size);
            fits_stale && fits_a && a_evicted && fresh.place_all(&mut engine, &kab).1
        });
        let Some(size) = size else {
            unreachable!("some atlas size separates per-run from per-frame prepare")
        };
        let mut atlas = GlyphAtlas::with_size(context.device(), size);
        let upload = |engine: &mut TextEngine, atlas: &mut GlyphAtlas, texts: &[&str]| {
            upload_paint_ops(
                context.device(),
                context.queue(),
                engine,
                atlas,
                texts.iter().map(|t| PaintOp::Text(run(t))).collect(),
                1.0,
                |c| c,
            )
        };
        let _ = upload(&mut engine, &mut atlas, &[stale]);
        assert!(resident(atlas.layout(), &ks));
        let ops = upload(&mut engine, &mut atlas, &[a, b]);
        let counts: Vec<u32> = ops
            .iter()
            .filter_map(|op| match op {
                GpuPaintOp::Text(mesh, _) => Some(mesh.index_count()),
                _ => None,
            })
            .collect();
        #[allow(clippy::cast_possible_truncation)]
        let expected = vec![ka.len() as u32 * 6, kb.len() as u32 * 6];
        assert_eq!(counts, expected, "atlas {size}: every run's glyphs draw");
        assert!(resident(atlas.layout(), &kab));
    }
}
