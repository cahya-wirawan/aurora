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

use crate::render::{GpuMesh, PathPipeline};
use crate::test_support::real_context;
use aurora_vector::{Mesh, Point};

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
