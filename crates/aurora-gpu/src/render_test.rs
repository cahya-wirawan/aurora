//! End-to-end proof that the shader library + pipeline cache actually
//! draw correct pixels — not just "it compiled," a real rendered-output
//! check, matching this project's general practice of measuring rather
//! than assuming (`spike/FINDINGS.md`, `aurora-tile`'s bit-exact
//! round-trip tests).

#![cfg(test)]

use crate::pipeline::{Blend, PipelineCache, PipelineKey};
use crate::shader::ShaderLibrary;
use crate::test_support::{build_canvas_pipeline, canvas_bind_group_layout, real_context};
use half::f16;

/// 64 px × 4 bytes/px (`Rgba8Unorm`) = 256 bytes/row -- exactly wgpu's
/// texture-to-buffer copy alignment requirement, so this test sidesteps
/// row-padding logic entirely rather than fighting it.
const SIZE: u32 = 64;

#[test]
// One linear setup-render-readback flow for a single real GPU test --
// splitting it into helper functions would just relocate the same lines
// without reducing the actual complexity of what a real render pass
// needs (texture, sampler, uniform buffer, bind group, pass, readback).
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_renders_correct_pixels() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    let library = ShaderLibrary::new(device);
    let Some(shader) = library.get("canvas") else {
        unreachable!("ShaderLibrary::new always compiles the canvas shader");
    };
    let layout = canvas_bind_group_layout(device);

    let key = PipelineKey {
        shader: "canvas",
        vertex_entry: "vs_canvas",
        fragment_entry: "fs_canvas",
        target_format: wgpu::TextureFormat::Rgba8Unorm,
        blend: Blend::None,
    };
    let mut cache = PipelineCache::new();
    let pipeline = cache.get_or_create_with(key.clone(), || {
        build_canvas_pipeline(device, &layout, shader, &key)
    });

    // A 1x1 canvas texture, solid opaque red -- with alpha = 1, the
    // checkerboard-behind-transparency logic must contribute nothing, so
    // the expected output is exactly this colour, unconditionally.
    let canvas_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let red = [
        f16::from_f32(1.0),
        f16::from_f32(0.0),
        f16::from_f32(0.0),
        f16::from_f32(1.0),
    ];
    let mut red_bytes = Vec::with_capacity(8);
    for sample in red {
        red_bytes.extend_from_slice(&sample.to_le_bytes());
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &canvas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &red_bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let canvas_view = canvas_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());

    // uv_offset = (0, 0), uv_scale = (1, 1) -- irrelevant to the result
    // here since the 1x1 canvas texture is the same colour everywhere,
    // but a real, non-garbage uniform value, not a placeholder.
    let uniform_floats: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    let mut uniform_bytes = Vec::with_capacity(16);
    for value in uniform_floats {
        uniform_bytes.extend_from_slice(&value.to_le_bytes());
    }
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("canvas-uniform"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&uniform_buffer, 0, &uniform_bytes);

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("canvas"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&canvas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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
            label: Some("canvas"),
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

    let bytes_per_row = SIZE * 4;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(SIZE),
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
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    // Matches spike/vertical-slice's own headless-mode poll pattern
    // (main.rs's `headless_bench`, which awaits presentation the same
    // way): blocks until the queue is idle, which also drives the
    // map_async callback above to completion.
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
    let Some(first_pixel) = data.get(0..4) else {
        unreachable!("a 64x64 Rgba8Unorm readback buffer is at least 4 bytes");
    };
    match first_pixel {
        [r, g, b, a] => {
            assert_eq!(*r, 255, "red channel");
            assert_eq!(*g, 0, "green channel");
            assert_eq!(*b, 0, "blue channel");
            assert_eq!(
                *a, 255,
                "alpha channel (checkerboard must not show through opaque content)"
            );
        }
        _ => unreachable!("sliced exactly 4 bytes"),
    }
    drop(data);
    readback_buffer.unmap();
}
