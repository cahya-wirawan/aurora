//! End-to-end proof that [`crate::CanvasPipeline`] and
//! [`crate::TileResidency`] together actually draw correct pixels — not
//! just "it compiled," a real rendered-output check, matching this
//! project's general practice of measuring rather than assuming
//! (`spike/FINDINGS.md`, `aurora-tile`'s bit-exact round-trip tests).
//! Exercises the exact same real types `aurora-app`'s own canvas
//! rendering does, not a hand-rolled duplicate of them.

#![cfg(test)]

use crate::test_support::real_context;
use crate::{CanvasPipeline, TileResidency};
use aurora_tile::{SurfaceId, TileId, TileStore};
use half::f16;
use std::num::NonZeroUsize;

/// A 256×256 viewport -- one tile exactly, so a solid tile fills the
/// whole rendered output and the expected pixel colour is unconditional
/// (no partial-tile/checkerboard-edge cases to account for).
const VIEWPORT: (u32, u32) = (256, 256);

fn tile_store() -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(4) else {
        unreachable!("4 is non-zero");
    };
    let store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
    };
    (dir, store)
}

/// Fills tile `id` on `surface` with a solid, straight `rgba` colour —
/// shared by [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`]'s
/// two differently-coloured tiles.
fn paint(store: &mut TileStore, surface: SurfaceId, id: TileId, rgba: [f32; 4]) {
    let tile = match store.get_mut(surface, id) {
        Ok(tile) => tile,
        Err(err) => unreachable!("a fresh store must accept this: {err}"),
    };
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let Some(&channel) = rgba.get(i % 4) else {
            unreachable!("i % 4 is always in range 0..4");
        };
        *sample = f16::from_f32(channel);
    }
}

#[test]
// One linear setup-render-readback flow for a single real GPU test --
// splitting it into helper functions would just relocate the same lines
// without reducing the actual complexity of what a real render pass
// needs (store, residency, pipeline, bind group, pass, readback).
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_renders_a_real_residencys_tile_correctly() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    // Solid, opaque green -- with alpha = 1, the checkerboard-behind-
    // transparency logic must contribute nothing, so the expected
    // output is exactly this colour, unconditionally.
    {
        let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("a fresh store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            let channel = match i % 4 {
                1 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
    }

    let mut residency = TileResidency::new(device, queue, VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 4,
        "a 256x256 viewport needs a 2x2 slot grid"
    );
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let bind_group = canvas.bind_group(device, &residency);
    let pipeline = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
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

    let bytes_per_row = VIEWPORT.0 * 4;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(VIEWPORT.1),
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
                rows_per_image: Some(VIEWPORT.1),
            },
        },
        wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
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
    // Checks the center pixel, not the corner: the atlas is padded to a
    // 2x2-tile (512x512) texture but the render target/viewport is only
    // 256x256, so a center sample is safely away from any UV-wrapping
    // edge case at the sampled region's own boundary.
    let center_row = (VIEWPORT.1 / 2) as usize;
    let center_col = (VIEWPORT.0 / 2) as usize;
    let offset = center_row * (bytes_per_row as usize) + center_col * 4;
    let Some(pixel) = data.get(offset..offset + 4) else {
        unreachable!("offset is well within the readback buffer's own bounds");
    };
    match pixel {
        [r, g, b, a] => {
            assert_eq!(*r, 0, "red channel");
            assert_eq!(*g, 255, "green channel");
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

/// Reads back one pixel from a fresh render of `residency` through
/// `canvas` into a `target_size` render target — the readback half of
/// `canvas_pipeline_renders_a_real_residencys_tile_correctly` above,
/// factored out since [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`]
/// below needs it twice (once per zoom level) and duplicating the full
/// encoder/pass/readback sequence a second time would be real
/// repetition, unlike that test's own single linear pass.
fn render_and_sample_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut CanvasPipeline,
    residency: &TileResidency,
    target_size: (u32, u32),
    sample: (u32, u32),
) -> [u8; 4] {
    let bind_group = canvas.bind_group(device, residency);
    let pipeline = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: target_size.0,
            height: target_size.1,
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

    let bytes_per_row = target_size.0 * 4;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(target_size.1),
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
                rows_per_image: Some(target_size.1),
            },
        },
        wgpu::Extent3d {
            width: target_size.0,
            height: target_size.1,
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
/// The real, visible proof of PLAN.md M1.9's "make zoom visible" work:
/// `TileResidency::set_origin`'s new `zoom` parameter must actually
/// change what the canvas shader draws, not just accept the argument.
///
/// Two adjacent, differently-coloured tiles (green at `(0, 0)`, red at
/// `(1, 0)`), a residency/render-target wide enough (512 px, exactly
/// two tiles) to show both side by side at 100% zoom. At 100% zoom, a
/// sample point near the right edge lands in the red tile; at 200%
/// zoom, `uv_scale` halves (this test's real point), so the same origin
/// now shows only the *left* half of that same atlas region stretched
/// across the whole viewport — the same screen-space sample point that
/// used to land in the red tile now lands back in the green one.
fn canvas_pipeline_reflects_zoom_by_magnifying_the_atlas() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [0.0, 1.0, 0.0, 1.0],
    );
    paint(
        &mut store,
        surface,
        TileId { x: 1, y: 0 },
        [1.0, 0.0, 0.0, 1.0],
    );

    // A 512x256 viewport spans exactly the two painted tiles -- grid
    // becomes (3, 2) (the usual "+1" margin column/row), atlas 768x512.
    let viewport = (512, 256);
    let mut residency = TileResidency::new(device, queue, viewport);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 6, "a 3x2 slot grid");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);

    // Near the right edge of a 512-wide target: at 100% zoom this is
    // deep inside the red tile (uv_scale spans the full two-tile width,
    // so screen x=480/512 maps into atlas x ~ 720, past the 512 boundary
    // between tile 0 and tile 1).
    let sample = (480, 128);
    residency.set_origin(queue, TileId { x: 0, y: 0 }, viewport, 1.0);
    let at_100_percent =
        render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        at_100_percent,
        [255, 0, 0, 255],
        "at 100% zoom the same sample point must show the red tile"
    );

    // At 200% zoom, uv_scale halves -- the same origin now shows only
    // the atlas's own first 256 (of 768) texels stretched across the
    // full 512px target, so screen x=480 maps into atlas x ~ 360, still
    // inside tile 0 (green), not tile 1.
    residency.set_origin(queue, TileId { x: 0, y: 0 }, viewport, 2.0);
    let at_200_percent =
        render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        at_200_percent,
        [0, 255, 0, 255],
        "at 200% zoom the same screen point must now show the green tile -- \
         zoom must actually magnify, not just accept the argument"
    );
}
