//! Real GPU proof that uploads land in the *correct* slot region — a
//! call-count-only test (see `residency.rs`'s own tests) can't catch a
//! wrong-axis or off-by-one slot-offset bug; this reads the atlas
//! texture back and checks actual pixel content, matching this crate's
//! established practice (`render_test.rs`).

#![cfg(test)]

use crate::TileResidency;
use crate::test_support::{real_context, real_tile_store};
use aurora_tile::{TILE, TileId};
use half::f16;

#[test]
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

    let target_tile = TileId { x: 1, y: 0 };
    {
        let tile = match store.get_mut(target_tile) {
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

    let uploaded = residency.sync(queue, &mut store, false);
    assert_eq!(uploaded, 4, "first sync uploads the whole visible grid");

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
            texture: residency.texture_for_test(),
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
