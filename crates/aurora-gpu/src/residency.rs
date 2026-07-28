//! GPU tile residency: a fixed-size, tile-aligned atlas texture that
//! slides over the (potentially unbounded) document, with toroidal slot
//! addressing so panning invalidates one row/column of GPU uploads
//! instead of the whole texture (`spike/FINDINGS.md` finding #4).

use std::collections::HashMap;

use aurora_tile::{TILE, TileId, TileStore};

/// The `Canvas` uniform `canvas.wgsl` expects: `uv_offset`/`uv_scale`,
/// two `vec2<f32>`s, 16 bytes total.
const UNIFORM_SIZE: u64 = 16;

/// A GPU-resident window over a tile store: a tile-aligned atlas texture
/// sized to a viewport (plus one tile of margin), whose slots are
/// addressed toroidally (`tile index modulo grid size`) so that panning
/// by one tile invalidates one row or column of uploads, not the whole
/// texture. Ported from `spike/vertical-slice`'s `Renderer` (real,
/// measured — `spike/FINDINGS.md`), generalized to build against the
/// real `aurora_tile::TileStore` API rather than the spike's own
/// throwaway store.
///
/// Deliberately does not handle window resize — recreating the atlas at
/// a new size is separate, still-open M1.2 work (surface configuration
/// and resize).
pub struct TileResidency {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    /// Slots across, slots down.
    grid: (u32, u32),
    /// Which tile currently occupies each modulo-addressed slot.
    slots: HashMap<(u32, u32), TileId>,
    /// Top-left visible tile.
    origin: TileId,
}

impl TileResidency {
    /// Sizes the atlas to `viewport_px`, rounded up to whole tiles plus
    /// one tile of margin (matches the spike's `ct = viewport/TILE + 1`
    /// exactly), and establishes an initial origin of `(0, 0)`.
    #[must_use]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, viewport_px: (u32, u32)) -> Self {
        let grid = (
            viewport_px.0.div_ceil(TILE) + 1,
            viewport_px.1.div_ceil(TILE) + 1,
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile-residency"),
            size: wgpu::Extent3d {
                width: grid.0 * TILE,
                height: grid.1 * TILE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            // COPY_SRC beyond the two production-path flags (TEXTURE_BINDING
            // for sampling, COPY_DST for uploads) so the atlas can be read
            // back for verification -- real capability, not test-only scope
            // creep: debugging/inspection tooling will want this too.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // The wraparound is the hardware sampler's job, not WGSL's --
        // matches the spike exactly (`AddressMode::Repeat` both axes).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tile-residency"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile-residency-uniform"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let residency = Self {
            texture,
            view,
            sampler,
            uniform_buffer,
            grid,
            slots: HashMap::new(),
            origin: TileId { x: 0, y: 0 },
        };
        residency.write_uniform(queue, viewport_px);
        residency
    }

    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    #[must_use]
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// For building a bind group externally — `TileResidency` doesn't
    /// know about bind group layouts, the same boundary `PipelineCache`
    /// already draws.
    #[must_use]
    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    #[must_use]
    pub const fn origin(&self) -> TileId {
        self.origin
    }

    /// Call when the visible top-left tile changes (panning). Updates
    /// the UV uniform immediately; the texture itself is only touched by
    /// the next [`Self::sync`].
    pub fn set_origin(&mut self, queue: &wgpu::Queue, origin: TileId, viewport_px: (u32, u32)) {
        self.origin = origin;
        self.write_uniform(queue, viewport_px);
    }

    fn write_uniform(&self, queue: &wgpu::Queue, viewport_px: (u32, u32)) {
        let tex_w = (self.grid.0 * TILE) as f32;
        let tex_h = (self.grid.1 * TILE) as f32;
        // Absolute scroll (origin in pixels), wrapped by the repeat
        // sampler -- slot addressing is toroidal, so the texture is a
        // sliding window over the document, exactly as in the spike.
        let scroll = (self.origin.x * TILE, self.origin.y * TILE);
        let u = (scroll.0 % (self.grid.0 * TILE)) as f32 / tex_w;
        let v = (scroll.1 % (self.grid.1 * TILE)) as f32 / tex_h;
        let uv_scale = [viewport_px.0 as f32 / tex_w, viewport_px.1 as f32 / tex_h];
        let mut bytes = Vec::with_capacity(16);
        for value in [u, v, uv_scale[0], uv_scale[1]] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniform_buffer, 0, &bytes);
    }

    /// Uploads every visible slot that doesn't already hold the correct,
    /// clean tile. Returns the number of tiles actually uploaded — this
    /// count is the direct, measurable proof that panning by one tile
    /// invalidates one row/column, not the whole texture.
    ///
    /// `force`: re-upload every visible slot unconditionally, for the
    /// future resize case (not exercised by anything yet, since resize
    /// itself is separate, still-open M1.2 work — the parameter exists
    /// now because retrofitting it later is exactly what this bullet is
    /// about avoiding).
    pub fn sync(&mut self, queue: &wgpu::Queue, store: &mut TileStore, force: bool) -> u32 {
        let mut uploads = 0;
        for gy in 0..self.grid.1 {
            for gx in 0..self.grid.0 {
                let id = TileId {
                    x: self.origin.x + gx,
                    y: self.origin.y + gy,
                };
                let slot = (id.x % self.grid.0, id.y % self.grid.1);
                let resident = self.slots.get(&slot) == Some(&id);
                let dirty = store.take_dirty(id).is_some();
                if !force && resident && !dirty {
                    continue;
                }
                let tile = match store.get(id) {
                    Ok(tile) => tile,
                    Err(err) => {
                        // One bad tile shouldn't abort uploading the
                        // rest of the visible grid this frame; there is
                        // nothing more localized to retry against here.
                        tracing::warn!(?id, %err, "skipping tile for this frame's upload");
                        continue;
                    }
                };
                let mut bytes = Vec::with_capacity(tile.texels().len() * 2);
                for sample in tile.texels() {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: slot.0 * TILE,
                            y: slot.1 * TILE,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(TILE * 8),
                        rows_per_image: Some(TILE),
                    },
                    wgpu::Extent3d {
                        width: TILE,
                        height: TILE,
                        depth_or_array_layers: 1,
                    },
                );
                self.slots.insert(slot, id);
                uploads += 1;
            }
        }
        uploads
    }

    /// Exposes the atlas texture for `residency_test.rs`'s real
    /// pixel-readback check — not part of the public API (real callers
    /// only need [`Self::view`]/[`Self::sampler`] to draw).
    #[cfg(test)]
    pub(crate) fn texture_for_test(&self) -> &wgpu::Texture {
        &self.texture
    }
}

impl std::fmt::Debug for TileResidency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileResidency")
            .field("grid", &self.grid)
            .field("origin", &self.origin)
            .field("resident_slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::TileResidency;
    use crate::test_support::{real_context, real_tile_store};
    use aurora_tile::TileId;
    use half::f16;

    /// Paints tile `id` a known solid colour and marks it dirty, exactly
    /// as a real edit would.
    fn paint(store: &mut aurora_tile::TileStore, id: TileId, rgba: [f32; 4]) {
        let tile = match store.get_mut(id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        let samples = tile.texels_mut();
        for (i, sample) in samples.iter_mut().enumerate() {
            let Some(&channel) = rgba.get(i % 4) else {
                unreachable!("i % 4 is always in range 0..4");
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }

    #[test]
    fn toroidal_addressing_uploads_only_the_newly_exposed_column() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // A 256x256 viewport -> grid = (256/256 + 1, 256/256 + 1) = (2, 2).
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        assert_eq!(residency.grid, (2, 2));

        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 0.0, 0.0, 1.0]);
            }
        }

        // Nothing resident yet: every visible slot must upload.
        let first = residency.sync(context.queue(), &mut store, false);
        assert_eq!(first, 4, "first sync must upload the whole visible grid");

        // Unchanged: nothing should re-upload.
        let second = residency.sync(context.queue(), &mut store, false);
        assert_eq!(
            second, 0,
            "unchanged, resident, clean tiles must not re-upload"
        );

        // Pan by exactly one tile on the x axis. Paint the newly-visible
        // column so it has real content to upload.
        paint(&mut store, TileId { x: 2, y: 0 }, [0.0, 1.0, 0.0, 1.0]);
        paint(&mut store, TileId { x: 2, y: 1 }, [0.0, 1.0, 0.0, 1.0]);
        residency.set_origin(context.queue(), TileId { x: 1, y: 0 }, viewport);
        let third = residency.sync(context.queue(), &mut store, false);
        assert_eq!(
            third, 2,
            "panning by one tile must invalidate exactly one column, not the whole grid"
        );
    }
}
