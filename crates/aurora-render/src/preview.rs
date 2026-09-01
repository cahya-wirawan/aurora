//! Progressive rendering's GPU half: uploading a downsampled tile into
//! `aurora_gpu::TileResidency`'s atlas. PLAN.md M1.3.
//!
//! Ties together three pieces that each already exist independently:
//! `aurora_tile::TileStore` (where a tile's real pixels live),
//! [`crate::mip::downsample`] (the box filter), and
//! `aurora_gpu::TileResidency::upload_mip` (landing bytes in the atlas).
//!
//! What this module does not do: decide *when* to request a lower level
//! — that needs an interaction-state signal (is the user actively
//! panning right now) this crate has no source for yet, still open M1.3
//! work.

use aurora_gpu::{GpuError, TileResidency};
use aurora_tile::{SurfaceId, TileError, TileId, TileStore};

use crate::mip::{MipLevel, downsample};

/// Errors from [`upload_preview`]. The store lookup and the GPU upload
/// can each fail independently, so both are represented rather than
/// collapsed into one opaque variant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreviewError {
    /// `level` was [`MipLevel::Full`] — full resolution is
    /// `TileResidency::sync`'s job (dirty tracking, budgeting, LRU
    /// interplay this function doesn't replicate), not this function's.
    /// A caller error, not a runtime failure.
    #[error("MipLevel::Full is uploaded via TileResidency::sync, not upload_preview")]
    FullResolutionNotSupported,
    /// The tile failed to page in from the store.
    #[error(transparent)]
    Tile(#[from] TileError),
    /// The atlas rejected the upload.
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

/// Downsamples the tile at `id` in `store` to `level` and uploads it
/// into `residency`'s atlas at the matching mip level — the full
/// store → downsample → GPU-atlas path progressive rendering needs.
///
/// # Errors
///
/// See [`PreviewError`].
pub fn upload_preview(
    residency: &TileResidency,
    queue: &wgpu::Queue,
    store: &mut TileStore,
    surface: SurfaceId,
    id: TileId,
    level: MipLevel,
) -> Result<(), PreviewError> {
    if level == MipLevel::Full {
        return Err(PreviewError::FullResolutionNotSupported);
    }
    let tile = store.get(surface, id)?;
    let downsampled = downsample(tile.texels(), level);
    residency.upload_mip(queue, id, level.index(), &downsampled)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PreviewError, upload_preview};
    use crate::mip::MipLevel;
    use crate::test_support::real_context;
    use aurora_tile::{SurfaceId, TileId, TileStore};
    use half::f16;
    use std::num::NonZeroUsize;

    fn tile_store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let Some(budget) = NonZeroUsize::new(64) else {
            unreachable!("64 is non-zero");
        };
        let store = match TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
        };
        (dir, store)
    }

    #[test]
    // One linear setup-render-readback flow for a single real GPU test --
    // splitting it into helper functions would just relocate the same
    // lines without reducing the actual complexity, the same call
    // already made for `aurora-gpu`'s own `residency_test.rs`.
    #[allow(clippy::too_many_lines)]
    fn upload_preview_lands_a_downsampled_tile_in_the_atlas() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();
        let (_dir, mut store) = tile_store();

        // 256x256 viewport -> grid (2, 2): tile (1, 0) maps to slot (1, 0).
        let surface = SurfaceId::from_raw(0);
        let id = TileId { x: 1, y: 0 };
        {
            let tile = match store.get_mut(surface, id) {
                Ok(tile) => tile,
                Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
            };
            for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
                // Half-transparent green -- distinct from a blank tile,
                // and *translucent* deliberately (0.68.4): with an opaque
                // fixture, both `downsample`'s premultiplied-domain box
                // filter and `upload_mip`'s own premultiply are the
                // identity, so this whole store -> downsample -> atlas
                // chain asserted nothing about either.
                let channel = match i % 4 {
                    1 => 1.0,
                    3 => 0.5,
                    _ => 0.0,
                };
                *sample = f16::from_f32(channel);
            }
        }

        let residency = aurora_gpu::TileResidency::new(device, queue, (256, 256));
        if let Err(err) = upload_preview(
            &residency,
            queue,
            &mut store,
            surface,
            id,
            MipLevel::Quarter,
        ) {
            unreachable!("a real tile and a non-Full level must upload: {err}");
        }

        // Read back mip level 2 (Quarter, size 64) at slot (1,0): x in
        // [64, 128), y in [0, 64) -- same pattern as
        // `aurora_gpu`'s own residency_test.rs.
        let size = MipLevel::Quarter.size();
        let bytes_per_row = size * 8;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview-readback"),
            size: u64::from(bytes_per_row) * u64::from(size),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: residency.texture(),
                mip_level: MipLevel::Quarter.index(),
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
            unreachable!("a 64x64 Rgba16Float readback buffer is at least 8 bytes");
        };
        match first_texel {
            [r0, r1, g0, g1, b0, b1, a0, a1] => {
                let r = f16::from_le_bytes([*r0, *r1]).to_f32();
                let g = f16::from_le_bytes([*g0, *g1]).to_f32();
                let b = f16::from_le_bytes([*b0, *b1]).to_f32();
                let a = f16::from_le_bytes([*a0, *a1]).to_f32();
                assert_eq!(
                    (r, g, b, a),
                    (0.0, 0.5, 0.0, 0.5),
                    "downsampling a uniform tile must preserve its colour exactly, and the \
                     atlas holds it premultiplied: straight (0, 1, 0, 0.5) lands as \
                     (0, 0.5, 0, 0.5)"
                );
            }
            _ => unreachable!("sliced exactly 8 bytes"),
        }
        drop(data);
        readback.unmap();
    }

    #[test]
    fn upload_preview_rejects_full_resolution() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();
        let (_dir, mut store) = tile_store();
        let residency = aurora_gpu::TileResidency::new(device, queue, (256, 256));

        match upload_preview(
            &residency,
            queue,
            &mut store,
            SurfaceId::from_raw(0),
            TileId { x: 0, y: 0 },
            MipLevel::Full,
        ) {
            Err(PreviewError::FullResolutionNotSupported) => {}
            other => unreachable!("expected FullResolutionNotSupported, got {other:?}"),
        }
    }
}
