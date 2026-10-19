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
//! already uses. It goes one step further than that type's
//! `composite_over`, though: `composite_over` opens its own render pass
//! into a caller-supplied `wgpu::CommandEncoder` (it stopped submitting
//! one of its own in 0.86.0, which batched a whole composite tile into
//! one submit — this line said "draw-and-submit" until 0.86.1), whereas
//! the caller here (`aurora-app`) needs to draw the canvas *within* one
//! existing render pass alongside its own background clear (a
//! viewport-restricted draw into the canvas dock area, not the whole
//! window). So this exposes
//! [`CanvasPipeline::pipeline`]/[`CanvasPipeline::bind_group`] for the
//! caller's own pass instead of opening a pass at all.

use crate::pipeline::{Blend, PipelineCache, PipelineKey};
use crate::residency::TileResidency;

const CANVAS_SHADER: &str = include_str!("shaders/canvas.wgsl");
const LABEL: &str = "canvas";

/// The two shades of the checkerboard the canvas shows behind transparent
/// areas (0.197.0 names them; they were literals in `canvas.wgsl`),
/// `[dark, light]`, sRGB-encoded greys. Other places that draw "transparent"
/// (the Layers panel's layer thumbnails) take their checkerboard from here,
/// so the two never drift. Pinned against the shader's own source by
/// `checkerboard_shades_match_the_canvas_shader`. Not design tokens: the
/// shader's values predate the token system, and adding tokens is the design
/// owner's decision.
pub const CHECKERBOARD_SHADES: [f32; 2] = [0.18, 0.24];

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
        let entries = layout_entries();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &entries,
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
        self.pipeline_for(device, target_format, "fs_canvas")
    }

    fn pipeline_for(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        fragment_entry: &'static str,
    ) -> &wgpu::RenderPipeline {
        // `SRGB_TARGET` is derived from `target_format` below, so the
        // pipeline cache key needs no entry of its own for it.
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_canvas",
            fragment_entry,
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
                    // An sRGB target re-encodes on store, so the shader
                    // hands it linear light (0.192.1, `SRGB_TARGET`).
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[(
                            "SRGB_TARGET",
                            if key.target_format.is_srgb() {
                                1.0
                            } else {
                                0.0
                            },
                        )],
                        ..wgpu::PipelineCompilationOptions::default()
                    },
                }),
                primitive: wgpu::PrimitiveState::default(),
                multiview_mask: None,
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                cache: None,
            })
        })
    }

    /// The display texture's pipeline for `target_format` (task 142): the
    /// same shader module, layout and `SRGB_TARGET` rule as
    /// [`Self::pipeline`], with `fs_display` (sampling at the uniform's level
    /// of detail) as its fragment entry. Cached beside it.
    pub fn display_pipeline(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        self.pipeline_for(device, target_format, "fs_display")
    }

    /// A bind group drawing page `page` of `display` (task 142): its view,
    /// sampler (the atlas's filter when `atlas_filter`, see
    /// [`crate::display_lod`]) and uniform in the atlas's slots, and again in the partner
    /// slots (`fs_display` never reads them). `None` for no such page.
    #[must_use]
    pub fn display_bind_group(
        &self,
        device: &wgpu::Device,
        display: &crate::DisplayTexture,
        page: usize,
        atlas_filter: bool,
    ) -> Option<wgpu::BindGroup> {
        let (view, sampler, uniform) = display.page_bindings(page, atlas_filter)?;
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas-display"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform.as_entire_binding(),
                },
            ],
        }))
    }

    /// Builds a fresh bind group sampling `residency`'s own atlas view,
    /// sampler, and pan/zoom uniform — cheap enough to rebuild every
    /// frame (three references, no data copy) rather than caching, the
    /// same "just build it" choice `TileCompositor::composite_over`
    /// already makes for its own per-call bind group. The partner slots
    /// (task 140) hold `residency` itself, so whatever its blend weight,
    /// nothing but its own atlas is ever shown.
    #[must_use]
    pub fn bind_group(&self, device: &wgpu::Device, residency: &TileResidency) -> wgpu::BindGroup {
        self.bind_group_blended(device, residency, residency)
    }

    /// [`Self::bind_group`] with `partner` bound as the second atlas (task
    /// 140): the canvas shows `mix(primary, partner, w)` in premultiplied
    /// space, `w` being `primary`'s
    /// [`TileResidency::set_blend_weight`]. `partner` is sampled through
    /// `primary`'s sampler, at the `uv` its own uniform maps the viewport
    /// to; the document clip, checkerboard and channel gain are
    /// `primary`'s. With `w == 0.0` the partner is never sampled.
    #[must_use]
    pub fn bind_group_blended(
        &self,
        device: &wgpu::Device,
        primary: &TileResidency,
        partner: &TileResidency,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(primary.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(primary.sampler()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: primary.uniform_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(partner.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: partner.uniform_buffer().as_entire_binding(),
                },
            ],
        })
    }
}

/// The canvas bind group's layout: the atlas, its sampler and uniform, and
/// (task 140) the partner atlas and its uniform. One definition, used by
/// [`CanvasPipeline::new`] and the tests' own pipeline builder.
#[must_use]
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    [
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
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        // Task 140: the partner atlas (the next pyramid level) and
        // its own uniform, whose `uv` mapping places it.
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
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
    use super::{CANVAS_SHADER, CHECKERBOARD_SHADES, CanvasPipeline};
    use crate::TileResidency;
    use crate::test_support::real_context;

    /// The constant and the shader's `select(dark, light, ...)` literals are
    /// one decision in two places; this fails if either moves alone.
    #[test]
    fn checkerboard_shades_match_the_canvas_shader() {
        let wanted = format!(
            "select({:?}, {:?}, (sq.x + sq.y) % 2.0 == 0.0)",
            CHECKERBOARD_SHADES[0], CHECKERBOARD_SHADES[1]
        );
        assert!(
            CANVAS_SHADER.contains(&wanted),
            "canvas.wgsl no longer contains `{wanted}`"
        );
    }

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
