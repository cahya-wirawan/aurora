//! GPU-side tile compositing: blends a source tile over a destination
//! tile using the GPU's fixed-function alpha blend unit, replacing the
//! CPU per-pixel merge `spike/FINDINGS.md` finding #1 measured at ~20ms
//! and named as the actual compositing bottleneck (not disk I/O, which
//! the same spike found fast). PLAN.md M1.3.

use aurora_gpu::{Blend, GpuContext, PipelineCache, PipelineKey};

const COMPOSITE_SHADER: &str = include_str!("shaders/composite.wgsl");
const LABEL: &str = "composite";

/// Composites tile-sized `Rgba16Float` textures on the GPU. Owns its own
/// shader module, bind group layout, sampler, and pipeline cache —
/// self-contained, the same shape `aurora_gpu::TileResidency` already
/// uses, since nothing yet coordinates multiple GPU passes across a
/// frame (that's still-open M1.3 scope: progressive rendering, async
/// evaluation).
///
/// Deliberately minimal: blends exactly one source tile over one
/// destination tile via straight-alpha "source-over"
/// (`Blend::AlphaBlending`). No blend-mode or opacity parameter — those
/// are a layer's properties, and the layer model (`aurora-doc`) doesn't
/// exist yet; `aurora-render` sits below it in the layering (PRD §7.2)
/// and has no way to know either. This is the primitive real layer
/// compositing will call once that model exists, not a full compositor
/// on its own.
pub struct TileCompositor {
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shader: wgpu::ShaderModule,
    pipelines: PipelineCache,
}

impl TileCompositor {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(LABEL),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..wgpu::SamplerDescriptor::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        Self {
            bind_group_layout,
            sampler,
            shader,
            pipelines: PipelineCache::new(),
        }
    }

    /// Blends `src` over `dst` in place: `dst`'s existing content is
    /// preserved (`LoadOp::Load`, not cleared) and `src` is drawn on top
    /// with straight-alpha "source-over" blending. Both views must be
    /// `Rgba16Float`, the same size, and `dst`'s owning texture must
    /// include `RENDER_ATTACHMENT` usage.
    pub fn composite_over(
        &mut self,
        context: &GpuContext,
        dst: &wgpu::TextureView,
        src: &wgpu::TextureView,
    ) {
        let device = context.device();
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_composite",
            fragment_entry: "fs_composite",
            target_format: wgpu::TextureFormat::Rgba16Float,
            blend: Blend::AlphaBlending,
        };
        let layout = &self.bind_group_layout;
        let shader = &self.shader;
        let pipeline = self.pipelines.get_or_create_with(key.clone(), || {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(LABEL),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(LABEL),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(key.vertex_entry),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(key.fragment_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.target_format,
                        blend: key.blend.to_wgpu(),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                multiview_mask: None,
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                cache: None,
            })
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(LABEL) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(LABEL),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
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
        context.queue().submit(std::iter::once(encoder.finish()));
    }
}

impl std::fmt::Debug for TileCompositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileCompositor")
            .field("cached_pipelines", &self.pipelines.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::TileCompositor;
    use crate::test_support::real_context;
    use aurora_tile::TILE;
    use half::f16;

    /// A `TILE`x`TILE` `Rgba16Float` texture, pre-filled solid `rgba` via
    /// `write_texture` (the same upload technique `aurora_gpu::TileResidency`
    /// uses), with whichever `usage` flags the caller needs on top of the
    /// two every test here needs (`TEXTURE_BINDING` for sampling as a
    /// composite source, `COPY_DST` to seed it).
    fn solid_tile(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: [f32; 4],
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-tile"),
            size: wgpu::Extent3d {
                width: TILE,
                height: TILE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: usage | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut texel = Vec::with_capacity(8);
        for channel in rgba {
            texel.extend_from_slice(&f16::from_f32(channel).to_le_bytes());
        }
        let mut row = Vec::with_capacity(texel.len() * TILE as usize);
        for _ in 0..TILE {
            row.extend_from_slice(&texel);
        }
        let mut bytes = Vec::with_capacity(row.len() * TILE as usize);
        for _ in 0..TILE {
            bytes.extend_from_slice(&row);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
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
        texture
    }

    /// Reads back the first texel of `texture` as `(r, g, b, a)` floats.
    fn read_first_texel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
    ) -> (f32, f32, f32, f32) {
        let bytes_per_row = TILE * 8; // Rgba16Float, already a multiple of wgpu's 256-byte alignment.
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite-readback"),
            size: u64::from(bytes_per_row) * u64::from(TILE),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
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
        let Some(texel) = data.get(0..8) else {
            unreachable!("a TILE x TILE Rgba16Float readback buffer is at least 8 bytes");
        };
        let result = match texel {
            [r0, r1, g0, g1, b0, b1, a0, a1] => (
                f16::from_le_bytes([*r0, *r1]).to_f32(),
                f16::from_le_bytes([*g0, *g1]).to_f32(),
                f16::from_le_bytes([*b0, *b1]).to_f32(),
                f16::from_le_bytes([*a0, *a1]).to_f32(),
            ),
            _ => unreachable!("sliced exactly 8 bytes"),
        };
        drop(data);
        readback.unmap();
        result
    }

    /// Reads back the whole `TILE`x`TILE` texture as `Rgba8` (each `f16`
    /// channel clamped to `0.0..=1.0` and rounded) — what a golden-image
    /// comparison needs, unlike [`read_first_texel`]'s single-pixel
    /// sanity check.
    fn read_rgba8(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
        let bytes_per_row = TILE * 8; // Rgba16Float, already 256-byte aligned.
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("golden-readback"),
            size: u64::from(bytes_per_row) * u64::from(TILE),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("golden-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
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
        let rgba8 = data
            .chunks_exact(2)
            .map(|bytes| {
                let Ok(bytes) = <[u8; 2]>::try_from(bytes) else {
                    unreachable!("chunks_exact(2) always yields a 2-byte slice");
                };
                let value = f16::from_le_bytes(bytes).to_f32().clamp(0.0, 1.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (value * 255.0).round() as u8
                }
            })
            .collect();
        drop(data);
        readback.unmap();
        rgba8
    }

    /// A real golden-image regression test, `aurora-testkit`'s first
    /// consumer (PLAN.md 0.2, "golden-image diff harness ... needed
    /// before the first filter"): renders the same source-over blend
    /// [`composite_over_blends_source_over_destination`] already proved
    /// correct via a pixel-math assertion, but here compares the *whole*
    /// composited tile against a checked-in golden PNG
    /// (`tests/golden/composite_basic.png`) instead of reading back one
    /// texel. Tolerance is `1` (out of 255): `0.5` and `1.0` round
    /// trip exactly through `f16`, so any real driver/GPU numerical
    /// noise would still need to be at least 1/255 to matter here, and
    /// this is not asserting bit-exactness the way the plain pixel-math
    /// test does.
    #[test]
    fn composite_over_matches_the_golden_image() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let dst = solid_tile(
            device,
            queue,
            [0.0, 0.0, 1.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 0.5],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        compositor.composite_over(&context, &dst_view, &src_view);

        let rgba8 = read_rgba8(device, queue, &dst);
        let actual = match aurora_testkit::Image::new(TILE, TILE, rgba8) {
            Ok(image) => image,
            Err(err) => unreachable!("read_rgba8 always returns TILE*TILE*4 bytes: {err}"),
        };
        let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden/composite_basic.png");
        if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &actual, 1) {
            unreachable!("{err}");
        }
    }

    #[test]
    fn composite_over_blends_source_over_destination() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Opaque blue destination, half-transparent red source -- a case
        // that distinguishes correct source-over math from both "no
        // blending" (would just overwrite with the raw src colour) and
        // "wrong load op" (would blend against a cleared/black dst
        // instead of the real one).
        let dst = solid_tile(
            device,
            queue,
            [0.0, 0.0, 1.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 0.5],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        compositor.composite_over(&context, &dst_view, &src_view);

        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        // Straight-alpha "over": result = src*src.a + dst*(1-src.a).
        assert_eq!((r, g, b, a), (0.5, 0.0, 0.5, 1.0));
    }

    #[test]
    fn composite_over_with_fully_transparent_source_leaves_destination_unchanged() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let dst = solid_tile(
            device,
            queue,
            [0.0, 0.0, 1.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(
            device,
            queue,
            [1.0, 1.0, 1.0, 0.0],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        compositor.composite_over(&context, &dst_view, &src_view);

        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        assert_eq!((r, g, b, a), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn composite_over_reuses_the_cached_pipeline() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let dst = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(
            device,
            queue,
            [1.0, 1.0, 1.0, 1.0],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        assert_eq!(compositor.pipelines.len(), 0);
        compositor.composite_over(&context, &dst_view, &src_view);
        assert_eq!(compositor.pipelines.len(), 1);
        compositor.composite_over(&context, &dst_view, &src_view);
        assert_eq!(
            compositor.pipelines.len(),
            1,
            "a second call with the same key must not rebuild"
        );
    }
}
