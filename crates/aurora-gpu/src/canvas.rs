//! The canvas render pipeline: draws a [`crate::TileResidency`]'s own
//! atlas texture as a fullscreen triangle (`shaders/canvas.wgsl`),
//! checkerboard behind transparent areas — the on-screen half of
//! `spike/vertical-slice`'s own proven `vs_canvas`/`fs_canvas` shader
//! pair, real production code for the first time (previously exercised
//! only by this crate's own tests, via `test_support`'s private
//! helpers).
//!
//! Self-contained (owns its own shader module, bind group layout, and
//! pipeline cache), the same shape `aurora_render::TileCompositor`
//! already uses — deliberately *not* a self-contained draw-and-submit
//! method the way that type's `composite_over` is, though: the caller
//! (`aurora-app`) needs to draw the canvas within one existing render
//! pass alongside its own background clear (a viewport-restricted draw
//! into the canvas dock area, not the whole window), so this exposes
//! [`CanvasPipeline::pipeline`]/[`CanvasPipeline::bind_group`] for the
//! caller's own pass instead.

use crate::pipeline::{Blend, PipelineCache, PipelineKey};
use crate::residency::TileResidency;

const CANVAS_SHADER: &str = include_str!("shaders/canvas.wgsl");
const LABEL: &str = "canvas";

/// Builds and caches the canvas render pipeline, and builds the
/// per-frame bind group that samples a given [`TileResidency`].
pub struct CanvasPipeline {
    layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    cache: PipelineCache,
}

impl CanvasPipeline {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(CANVAS_SHADER.into()),
        });
        Self {
            layout,
            shader,
            cache: PipelineCache::new(),
        }
    }

    /// Returns the cached pipeline for `target_format`, building it (and
    /// caching the result) on first use for that format.
    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_canvas",
            fragment_entry: "fs_canvas",
            target_format,
            blend: Blend::None,
        };
        let layout = &self.layout;
        let shader = &self.shader;
        self.cache.get_or_create_with(key.clone(), || {
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
        })
    }

    /// Builds a fresh bind group sampling `residency`'s own atlas view,
    /// sampler, and pan/zoom uniform — cheap enough to rebuild every
    /// frame (three references, no data copy) rather than caching, the
    /// same "just build it" choice `TileCompositor::composite_over`
    /// already makes for its own per-call bind group.
    #[must_use]
    pub fn bind_group(&self, device: &wgpu::Device, residency: &TileResidency) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(residency.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(residency.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: residency.uniform_buffer().as_entire_binding(),
                },
            ],
        })
    }
}

impl std::fmt::Debug for CanvasPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasPipeline")
            .field("cached_pipelines", &self.cache.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::CanvasPipeline;
    use crate::TileResidency;
    use crate::test_support::real_context;

    #[test]
    fn pipeline_is_cached_by_target_format() {
        let Some(context) = real_context() else {
            return;
        };
        let mut canvas = CanvasPipeline::new(context.device());
        assert_eq!(canvas.cache.len(), 0);
        let _ = canvas.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(canvas.cache.len(), 1);
        let _ = canvas.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(canvas.cache.len(), 1, "same format must hit the cache");
        let _ = canvas.pipeline(context.device(), wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(canvas.cache.len(), 2, "a different format must rebuild");
    }

    #[test]
    fn bind_group_can_be_built_from_a_real_residency() {
        let Some(context) = real_context() else {
            return;
        };
        let canvas = CanvasPipeline::new(context.device());
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        // No assertion beyond "doesn't panic" -- a `wgpu::BindGroup`
        // exposes nothing else to introspect; the real proof this shape
        // is correct is `canvas_pipeline_renders_correct_pixels`
        // (`render_test.rs`), which actually draws with it.
        let _ = canvas.bind_group(context.device(), &residency);
    }
}
