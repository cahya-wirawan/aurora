//! Real GPU proof that uploads land in the *correct* slot region — a
//! call-count-only test (see `residency.rs`'s own tests) can't catch a
//! wrong-axis or off-by-one slot-offset bug; this reads the atlas
//! texture back and checks actual pixel content, matching this crate's
//! established practice (`render_test.rs`).

#![cfg(test)]

use crate::TileResidency;
use crate::test_support::{real_context, real_tile_store};
use aurora_tile::{SurfaceId, TILE, TileId};
use half::f16;

#[test]
// One linear setup-render-readback flow for a single real GPU test --
// splitting it into helper functions would just relocate the same lines
// without reducing the actual complexity (texture, buffer, bind group,
// pass, readback) -- same call already made for render_test.rs.
#[allow(clippy::too_many_lines)]
fn upload_lands_in_the_correct_slot() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = real_tile_store(64);

    // 256x256 viewport -> grid (2, 2): tile (1, 0) maps to slot (1, 0),
    // i.e. the texture's second column, first row.
    let viewport = (256, 256);
    let mut residency = TileResidency::new(device, queue, viewport);

    let surface = SurfaceId::from_raw(0);
    let target_tile = TileId { x: 1, y: 0 };
    {
        let tile = match store.get_mut(surface, target_tile) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            // R, G, B, A = 0, 1, 0, 1 (opaque green) -- distinct from the
            // zero-initialized blank default so a wrong-slot read is
            // visibly wrong, not accidentally matching.
            let channel = match i % 4 {
                1 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: TILE,
            height: TILE,
        });
    }

    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 4,
        "first sync uploads the whole visible grid"
    );

    // Read back exactly the slot-(1,0) region: x in [256, 512), y in [0, 256).
    let bytes_per_row = TILE * 8; // Rgba16Float = 8 bytes/px; 256*8 = 2048, already
    // a multiple of wgpu's 256-byte copy alignment -- no row padding needed.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-readback"),
        size: u64::from(bytes_per_row) * u64::from(TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: TILE,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TILE),
            },
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
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
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 256x256 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (0.0, 1.0, 0.0, 1.0),
                "slot (1,0) must hold the painted tile's own colour, not a neighbour's"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}

#[test]
// Proves `resize` actually leaves a usable atlas behind, not just a
// struct that no longer panics: after growing the atlas, a real upload
// through `sync` must land in the *new* grid's slot region, matching
// `upload_lands_in_the_correct_slot`'s own readback rigor rather than
// just asserting the resize call didn't panic.
#[allow(clippy::too_many_lines)]
fn resize_then_upload_lands_in_the_correct_slot() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = real_tile_store(64);

    // Start at a 256x256 viewport (grid (2, 2)), then grow to 512x512
    // (grid (3, 3)): tile (2, 0) only exists in the *new* grid, mapping
    // to slot (2, 0), the atlas's third column, first row.
    let mut residency = TileResidency::new(device, queue, (256, 256));
    residency.resize(device, queue, (512, 512));

    let surface = SurfaceId::from_raw(0);
    let target_tile = TileId { x: 2, y: 0 };
    {
        let tile = match store.get_mut(surface, target_tile) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            // R, G, B, A = 1, 0, 1, 1 (opaque magenta) -- distinct from
            // this file's other test colour (green) and from a blank
            // tile, so a wrong-slot read after resize is visibly wrong.
            let channel = match i % 4 {
                0 | 2 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: TILE,
            height: TILE,
        });
    }

    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 9,
        "post-resize sync uploads the whole (3,3) visible grid"
    );

    // Read back exactly the slot-(2,0) region: x in [512, 768), y in [0, 256).
    let bytes_per_row = TILE * 8; // Rgba16Float = 8 bytes/px; 256*8 = 2048, already
    // a multiple of wgpu's 256-byte copy alignment -- no row padding needed.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-resize-readback"),
        size: u64::from(bytes_per_row) * u64::from(TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-resize-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: TILE * 2,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TILE),
            },
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
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
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 256x256 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (1.0, 0.0, 1.0, 1.0),
                "post-resize slot (2,0) must hold the painted tile's own colour, in the new, bigger atlas"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}

#[test]
// Same linear setup-upload-readback shape as `upload_lands_in_the_correct_slot`
// above, targeting `upload_mip`'s non-zero mip level instead of `sync`'s
// mip level 0 -- proves the progressive-rendering GPU wiring lands
// downsampled bytes in the *correct* slot and level, not just that the
// call succeeds.
#[allow(clippy::too_many_lines)]
fn upload_mip_lands_in_the_correct_slot_and_level() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    // 256x256 viewport -> grid (2, 2): tile (1, 0) maps to slot (1, 0).
    let viewport = (256, 256);
    let residency = TileResidency::new(device, queue, viewport);

    // Mip level 1 ("Half" in aurora-render's MipLevel): TILE/2 = 128.
    let level = 1u32;
    let size = TILE / 2;
    let mut texels = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        // Opaque magenta -- distinct from this file's other test colour
        // (green) and from a blank tile, so a wrong slot/level reads as
        // visibly wrong, not coincidentally right.
        for channel in [1.0, 0.0, 1.0, 1.0] {
            texels.push(f16::from_f32(channel));
        }
    }

    let target_tile = TileId { x: 1, y: 0 };
    if let Err(err) = residency.upload_mip(queue, target_tile, level, &texels) {
        unreachable!("a correctly-sized texel buffer at a valid level must upload: {err}");
    }

    // Read back mip level 1's slot-(1,0) region: x in [128, 256), y in [0, 128).
    let bytes_per_row = size * 8; // Rgba16Float = 8 bytes/px; 128*8 = 1024,
    // already a multiple of wgpu's 256-byte copy alignment.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("residency-mip-readback"),
        size: u64::from(bytes_per_row) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("residency-mip-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: residency.texture(),
            mip_level: level,
            origin: wgpu::Origin3d {
                x: size,
                y: 0,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
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
    let Some(first_texel) = data.get(0..8) else {
        unreachable!("a 128x128 Rgba16Float readback buffer is at least 8 bytes");
    };
    match first_texel {
        [r0, r1, g0, g1, b0, b1, a0, a1] => {
            let r = f16::from_le_bytes([*r0, *r1]).to_f32();
            let g = f16::from_le_bytes([*g0, *g1]).to_f32();
            let b = f16::from_le_bytes([*b0, *b1]).to_f32();
            let a = f16::from_le_bytes([*a0, *a1]).to_f32();
            assert_eq!(
                (r, g, b, a),
                (1.0, 0.0, 1.0, 1.0),
                "slot (1,0) at mip level 1 must hold the uploaded preview's own colour"
            );
        }
        _ => unreachable!("sliced exactly 8 bytes"),
    }
    drop(data);
    readback.unmap();
}
