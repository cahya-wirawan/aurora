//! The GPU path renderer: draws a tessellated [`aurora_vector::Mesh`] —
//! PRD §8's own pre-decided "vector rasterization: `lyon` (tessellation)
//! and a custom GPU path renderer," the half `aurora-vector` itself
//! deliberately doesn't do (that crate depends on `aurora-core` only,
//! staying GPU-agnostic on purpose — see its own doc comment). This
//! crate is where it belongs: the one crate depending on both
//! `aurora-vector` and `aurora-gpu` (`scripts/layering.json`).
//!
//! [`PathPipeline`] follows the same self-contained shape
//! `aurora_gpu::CanvasPipeline` already established (owns its own
//! shader module, bind group layout, and `aurora_gpu::PipelineCache`),
//! exposing [`PathPipeline::pipeline`]/[`PathPipeline::bind_group`] for
//! a caller's own render pass rather than a self-contained draw-and-
//! submit method — a UI frame draws many paths (one per widget) within
//! *one* shared pass, the same reason `CanvasPipeline` itself made this
//! choice.
//!
//! [`GpuMesh`] is the other real, new piece this crate needed: every
//! `wgpu`-touching pipeline in this workspace so far has drawn a fixed
//! fullscreen triangle generated in the vertex shader from
//! `@builtin(vertex_index)` alone (`CanvasPipeline`,
//! `aurora_render::TileCompositor`) — no real vertex *buffer* exists
//! anywhere in this codebase yet, because vector geometry is the first
//! thing that actually varies per draw call rather than being a fixed
//! shape. [`GpuMesh::upload`] is that missing piece: a `Mesh`'s own
//! `Vec<Point>`/`Vec<u32>` uploaded as real `wgpu::Buffer`s, built by
//! hand (`f32::to_le_bytes`/`u32::to_le_bytes`) rather than pulling in
//! a cast-safety crate like `bytemuck` — this workspace already has an
//! established, dependency-free convention for exactly this
//! (`aurora_gpu`'s own `TileResidency::write_uniform`), and matching it
//! beats adding a new dependency for one more struct.
//!
//! **Scope, stated honestly**: solid-fill only — one flat colour per
//! draw call ([`PathPipeline::bind_group`]'s own `color` parameter),
//! resolved by the caller from a real design token (invariant §7.3.10
//! — this shader has no colour of its own opinion baked in). No
//! per-vertex colour or textures in *this* pipeline: gradients (0.124.0)
//! are a separate [`GradientPipeline`] drawing a [`GpuColorMesh`], so
//! the solid path is byte-for-byte what it was. [`draw_paint_ops`]
//! interleaves both in paint order within one pass — and, since 0.132.0,
//! glyph runs too: [`TextPipeline`] draws [`GpuGlyphMesh`] quads from an
//! `R8Unorm` [`GlyphAtlas`], the first texture any widget pipeline samples
//! (`crate::text_render` has the account).
//! `aurora_vector::stroke`
//! already produces a real `Mesh` this pipeline can draw exactly the
//! same way a fill's `Mesh` is drawn — nothing here is fill-specific —
//! but that combination isn't exercised by this module's own tests
//! yet. [`crate::paint_widget`] (added 2026-08-06, `Button`'s own
//! solid rounded-rect background only at first, `Checkbox`/`Slider`
//! since) is the real widget-to-`Mesh` path; `aurora-app::App::redraw`
//! is the real caller that drives it over a whole tree, every frame,
//! feeding the results through this pipeline — both landed after this
//! module did, see PLAN.md's own M1.7 section for the full account.
//!
//! **A real bug, found by real macOS CI (2026-08-07), not this
//! sandbox** (no GPU adapter here — every test in this module skips):
//! `PathPipeline::draw` used to bind an empty `GpuMesh`'s own
//! zero-size vertex/index buffers unconditionally before issuing a
//! `0..0` indexed draw call, on the assumption that a zero-size
//! `wgpu::Buffer` is real and valid (true) and therefore safe to
//! `Buffer::slice(..)` (false — `wgpu` 30 panics, "buffer slice can
//! not be empty"). Fixed with an early return in `draw` itself for
//! `index_count == 0`, before any buffer is touched — see that
//! method's own doc comment. Not reachable through `App::redraw` in
//! the *current* app (checked, not assumed: the only widgets
//! `paint_widget` paints today — `Button`/`Checkbox`/`Slider` — are
//! only ever inserted either before the first `WidgetTree::
//! compute_layout` runs, where a subsequent full layout pass
//! positions them, or never at all mid-session; `open_command_palette`
//! is the one real mid-session insertion path and its own rows are
//! `Container`/`CommandPalette`, neither painted yet), but a real,
//! latent trap for the next widget that *is* inserted mid-session
//! without an intervening layout pass — `WidgetTree::bounds` stays
//! `UNLAID_OUT` (all zero) until `compute_layout` next runs, which
//! `paint_widget` tessellates to an empty `Mesh` today with no special
//! case of its own.

use aurora_gpu::{Blend, PipelineCache, PipelineKey};
use aurora_vector::{ColorMesh, Mesh};

pub use crate::text_render::{
    AtlasLayout, AtlasSlot, GlyphAtlas, GlyphBatch, GpuGlyphMesh, PendingUpload, TextPipeline,
    upload_glyph_meshes, upload_paint_ops,
};

const PATH_SHADER: &str = include_str!("shaders/path.wgsl");
const LABEL: &str = "path";

/// One `Mesh` vertex's own on-GPU size: `position: vec2<f32>` only —
/// see this module's own doc comment for why nothing else yet.
const VERTEX_SIZE: u64 = 8;
/// [`PathPipeline::bind_group`]'s own uniform buffer size — real,
/// WGSL-`uniform`-address-space-compatible layout: `viewport_size:
/// vec2<f32>` (align 8, size 8), padded out to `color`'s own 16-byte
/// alignment (`vec4<f32>`), then `color` itself (size 16) — 32 bytes
/// total, matching WGSL's own implicit struct layout rules exactly
/// (confirmed against the spec's own alignment table, not assumed).
const UNIFORM_SIZE: u64 = 32;

/// A [`Mesh`] uploaded to the GPU — the real vertex/index buffers a
/// [`PathPipeline`] draws directly. Built once per (re)tessellation
/// (whenever a shape's own geometry changes), not rebuilt every frame a
/// caller redraws the same, unchanged shape.
pub struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl GpuMesh {
    /// Uploads `mesh`'s own vertices/indices as real GPU buffers. An
    /// empty `mesh` (no vertices/indices — a degenerate or fully
    /// clamped-away shape) uploads a zero-length buffer rather than
    /// erroring; [`PathPipeline::draw`] on the result draws nothing —
    /// see that method's own doc comment for the real mechanism (an
    /// early return, not a zero-index draw call; `wgpu` doesn't allow
    /// even binding a zero-size buffer, a real, only-recently-checked
    /// finding).
    #[must_use]
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) -> Self {
        let mut vertex_bytes = Vec::with_capacity(mesh.vertices.len() * 8);
        for point in &mesh.vertices {
            vertex_bytes.extend_from_slice(&point.x.to_le_bytes());
            vertex_bytes.extend_from_slice(&point.y.to_le_bytes());
        }
        let mut index_bytes = Vec::with_capacity(mesh.indices.len() * 4);
        for &index in &mesh.indices {
            index_bytes.extend_from_slice(&index.to_le_bytes());
        }

        // `size: 0` is a real, valid `wgpu::Buffer` -- an empty `Mesh`
        // (see this method's own doc comment) uploads one rather than
        // being special-cased away, and `queue.write_buffer` with an
        // empty slice below is likewise a real no-op, not an error.
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, &index_bytes);

        #[allow(clippy::cast_possible_truncation)]
        let index_count = mesh.indices.len() as u32;
        Self {
            vertex_buffer,
            index_buffer,
            index_count,
        }
    }
}

impl std::fmt::Debug for GpuMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuMesh")
            .field("index_count", &self.index_count)
            .finish_non_exhaustive()
    }
}

/// Builds and caches the path render pipeline, and builds the per-draw
/// bind group (viewport size + solid fill colour) a [`GpuMesh`] draws
/// with. See this module's own doc comment for the overall shape and
/// why it matches `aurora_gpu::CanvasPipeline`'s.
pub struct PathPipeline {
    layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    cache: PipelineCache,
}

impl PathPipeline {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(PATH_SHADER.into()),
        });
        Self {
            layout,
            shader,
            cache: PipelineCache::new(),
        }
    }

    /// Returns the cached pipeline for `target_format`, building it (and
    /// caching the result) on first use for that format. Alpha-blended
    /// (`Blend::AlphaBlending`) — a real UI shape's own fill can be, and
    /// often is, semi-transparent (an overlay, a disabled-state scrim),
    /// unlike the canvas pipeline's own opaque-composited atlas.
    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_path",
            fragment_entry: "fs_path",
            target_format,
            blend: Blend::AlphaBlending,
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
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: VERTEX_SIZE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    })],
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

    /// Builds a fresh bind group for one draw: `viewport_size` (the
    /// render target's own size in physical pixels, what
    /// `shaders/path.wgsl`'s own `vs_path` converts a `Mesh` vertex's
    /// pixel-space position against to reach clip space) and `color`
    /// (straight, unpremultiplied RGBA — resolved by the caller from a
    /// real design token, never hardcoded here). Cheap enough to
    /// rebuild every draw (16 bytes) rather than caching, the same
    /// "just build it" choice `CanvasPipeline::bind_group`'s own doc
    /// comment already makes for a per-call bind group.
    #[must_use]
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_size: (f32, f32),
        color: [f32; 4],
    ) -> wgpu::BindGroup {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bytes = Vec::with_capacity(UNIFORM_SIZE as usize);
        bytes.extend_from_slice(&viewport_size.0.to_le_bytes());
        bytes.extend_from_slice(&viewport_size.1.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]); // padding, matching UNIFORM_SIZE's own layout.
        for channel in color {
            bytes.extend_from_slice(&channel.to_le_bytes());
        }
        queue.write_buffer(&uniform_buffer, 0, &bytes);

        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        })
    }

    /// Draws `mesh` within `pass` — `pass` must already have this
    /// pipeline's own [`Self::pipeline`] and a [`Self::bind_group`] set
    /// (`set_pipeline`/`set_bind_group`, group `0`) before calling
    /// this; it only issues the vertex/index buffer binds and the
    /// indexed draw call itself, the one part specific to which `Mesh`
    /// is being drawn.
    ///
    /// An empty `mesh` (`index_count == 0`) returns immediately,
    /// binding nothing — real macOS CI (2026-08-07) found that
    /// `wgpu` 30 panics ("buffer slice can not be empty") on
    /// `Buffer::slice(..)` for the zero-size buffers
    /// [`GpuMesh::upload`] uploads for exactly this case, so binding
    /// them at all, even for an indexed draw call of `0..0` that would
    /// itself have been a real no-op, isn't safe to reach. This
    /// sandbox has no GPU adapter, so this path had never actually run
    /// against real `wgpu` before that CI run — [`GpuMesh::upload`]'s
    /// own doc comment used to describe this as "drawing zero
    /// triangles"; it's an early return instead now, same outcome
    /// (nothing drawn), different, actually-correct mechanism.
    pub fn draw<'pass>(&self, pass: &mut wgpu::RenderPass<'pass>, mesh: &'pass GpuMesh) {
        if mesh.index_count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}

impl std::fmt::Debug for PathPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathPipeline")
            .field("cached_pipelines", &self.cache.len())
            .finish_non_exhaustive()
    }
}

const GRADIENT_SHADER: &str = include_str!("shaders/gradient.wgsl");
const GRADIENT_LABEL: &str = "gradient";

/// One [`ColorMesh`] vertex's on-GPU size: `position: vec2<f32>` at
/// offset 0, then `color: vec4<f32>` at offset 8.
const GRADIENT_VERTEX_SIZE: u64 = 24;
/// Byte offset of a gradient vertex's `color` attribute.
const GRADIENT_COLOR_OFFSET: u64 = 8;
/// [`GradientPipeline::bind_group`]'s uniform buffer size:
/// `viewport_size: vec2<f32>` padded to 16 bytes, WGSL's own minimum
/// uniform struct alignment for this shape.
const GRADIENT_UNIFORM_SIZE: u64 = 16;

/// A [`ColorMesh`] uploaded to the GPU — [`GpuMesh`]'s counterpart for
/// a vertex-coloured gradient, drawn by a [`GradientPipeline`]. Same
/// hand-built little-endian byte layout as [`GpuMesh::upload`] (no
/// `bytemuck`): per vertex, `x, y, r, g, b, a` as six `f32`s.
pub struct GpuColorMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl GpuColorMesh {
    /// Uploads `mesh`'s vertices and indices. Colours are uploaded
    /// exactly as given (straight sRGB-gamma RGBA) — never linearized
    /// here or by any caller; [`GradientPipeline`] handles an sRGB-aware
    /// target itself. An empty `mesh` uploads zero-size buffers that
    /// [`GradientPipeline::draw`] never binds (the same early return
    /// [`PathPipeline::draw`] has, for the same `wgpu` panic).
    #[must_use]
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &ColorMesh) -> Self {
        let mut vertex_bytes =
            Vec::with_capacity(mesh.vertices.len() * GRADIENT_VERTEX_SIZE as usize);
        for vertex in &mesh.vertices {
            vertex_bytes.extend_from_slice(&vertex.position.x.to_le_bytes());
            vertex_bytes.extend_from_slice(&vertex.position.y.to_le_bytes());
            for channel in vertex.color {
                vertex_bytes.extend_from_slice(&channel.to_le_bytes());
            }
        }
        let mut index_bytes = Vec::with_capacity(mesh.indices.len() * 4);
        for &index in &mesh.indices {
            index_bytes.extend_from_slice(&index.to_le_bytes());
        }

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(GRADIENT_LABEL),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, &vertex_bytes);
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(GRADIENT_LABEL),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, &index_bytes);

        // A `ColorMesh` index count past `u32::MAX` would need more than
        // 16 GiB of indices; `aurora_vector`'s builders cap far below it.
        let index_count = mesh.indices.len() as u32;
        Self {
            vertex_buffer,
            index_buffer,
            index_count,
        }
    }
}

impl std::fmt::Debug for GpuColorMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuColorMesh")
            .field("index_count", &self.index_count)
            .finish_non_exhaustive()
    }
}

/// Builds and caches the gradient render pipeline — a separate pipeline
/// from [`PathPipeline`], so the solid path's shader, vertex layout and
/// uniform are untouched. Same caller-owned-pass shape as
/// [`PathPipeline`]: [`Self::pipeline`], [`Self::bind_group`], then
/// [`Self::draw`] per mesh; [`draw_paint_ops`] drives both pipelines.
///
/// **Colour space.** The caller never linearizes gradient colours. The
/// fragment entry point is chosen from the *target format*:
/// `fs_gradient` writes the interpolated gamma value unchanged to a
/// plain target, and `fs_gradient_srgb_target` linearizes it per
/// fragment for an sRGB-aware one (whose hardware encode then stores the
/// same gamma value). Interpolation is therefore always in gamma-encoded
/// sRGB, so an *opaque* mesh stores the same bytes, to within 2 of 255
/// (the hardware sRGB encode's own rounding), on the gallery's
/// `Rgba8Unorm` and an sRGB swapchain. A *translucent* fragment is
/// blended in the target's own blend space (linear light on an sRGB
/// target, gamma-encoded values on a plain one) exactly as a solid
/// fill's is, so half-alpha white over black reads about 188 on an sRGB
/// target and 128 on a plain one, for gradients and solids alike.
/// Linearizing the vertices
/// instead would interpolate in linear light and visibly change the
/// result (a red-to-yellow midpoint would read green 188, not 128).
pub struct GradientPipeline {
    layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    cache: PipelineCache,
}

/// The fragment entry point [`GradientPipeline`] uses for `format`.
fn gradient_fragment_entry(format: wgpu::TextureFormat) -> &'static str {
    if format.is_srgb() {
        "fs_gradient_srgb_target"
    } else {
        "fs_gradient"
    }
}

impl GradientPipeline {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(GRADIENT_LABEL),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(GRADIENT_LABEL),
            source: wgpu::ShaderSource::Wgsl(GRADIENT_SHADER.into()),
        });
        Self {
            layout,
            shader,
            cache: PipelineCache::new(),
        }
    }

    /// Returns the cached pipeline for `target_format`, building it on
    /// first use. Alpha-blended, like [`PathPipeline::pipeline`]; the
    /// fragment entry point depends on whether `target_format` is
    /// sRGB-aware (see this type's doc comment).
    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let key = PipelineKey {
            shader: GRADIENT_LABEL,
            vertex_entry: "vs_gradient",
            fragment_entry: gradient_fragment_entry(target_format),
            target_format,
            blend: Blend::AlphaBlending,
        };
        let layout = &self.layout;
        let shader = &self.shader;
        self.cache.get_or_create_with(key.clone(), || {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(GRADIENT_LABEL),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(GRADIENT_LABEL),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(key.vertex_entry),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: GRADIENT_VERTEX_SIZE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: GRADIENT_COLOR_OFFSET,
                                shader_location: 1,
                            },
                        ],
                    })],
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

    /// Builds a bind group carrying `viewport_size` (the target's size in
    /// the mesh's own pixel units). Unlike [`PathPipeline::bind_group`]
    /// there is no colour uniform, so one bind group serves every
    /// gradient drawn to the same target.
    #[must_use]
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_size: (f32, f32),
    ) -> wgpu::BindGroup {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(GRADIENT_LABEL),
            size: GRADIENT_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bytes = Vec::with_capacity(GRADIENT_UNIFORM_SIZE as usize);
        bytes.extend_from_slice(&viewport_size.0.to_le_bytes());
        bytes.extend_from_slice(&viewport_size.1.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]); // `_pad`, see GRADIENT_UNIFORM_SIZE.
        queue.write_buffer(&uniform_buffer, 0, &bytes);

        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(GRADIENT_LABEL),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        })
    }

    /// Draws `mesh` within `pass`, which must already have this
    /// pipeline's [`Self::pipeline`] and a [`Self::bind_group`] set
    /// (group `0`). An empty mesh returns before binding anything — the
    /// same `wgpu` "buffer slice can not be empty" panic
    /// [`PathPipeline::draw`] documents.
    pub fn draw<'pass>(&self, pass: &mut wgpu::RenderPass<'pass>, mesh: &'pass GpuColorMesh) {
        if mesh.index_count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
    }
}

impl std::fmt::Debug for GradientPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GradientPipeline")
            .field("cached_pipelines", &self.cache.len())
            .finish_non_exhaustive()
    }
}

/// One uploaded paint, ready to draw: the GPU-side form of
/// [`crate::PaintOp`].
///
/// `Solid`'s colour is whatever the target needs — a caller drawing to
/// an sRGB-aware target linearizes it first (`aurora-app`'s
/// `linearize_paint_color`), exactly as before this type existed.
/// `Gradient`'s vertex colours are never linearized by a caller; see
/// [`GradientPipeline`].
#[derive(Debug)]
///
/// `Text` (0.132.0) is one run's glyph quads plus its colour, which a
/// caller linearizes exactly as it does a `Solid`'s. The colour is already
/// baked into the mesh's vertices by [`upload_paint_ops`]; the field
/// records it for inspection and is not re-read when drawing.
pub enum GpuPaintOp {
    Solid(GpuMesh, [f32; 4]),
    Gradient(GpuColorMesh),
    Text(GpuGlyphMesh, [f32; 4]),
}

/// Draws `ops` in order within `pass`, switching between `path` and
/// `gradient` only when consecutive ops differ in kind (a switch resets
/// the pipeline, which is also why a gradient's bind group is re-set on
/// every switch back). A `Text` op binds [`TextPipeline`] with one bind
/// group over `atlas`, which must already hold every glyph those ops use
/// ([`GlyphAtlas::prepare`]). The gradient and text bind groups are each
/// built at most once per call, and only if `ops` contains that kind. `viewport_size` is the
/// target's size in the meshes' own pixel units, as for
/// [`PathPipeline::bind_group`].
// Two pipelines, the device/queue pair, and the target's format and size
// are each needed; a parameter struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn draw_paint_ops<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    path: &mut PathPipeline,
    gradient: &mut GradientPipeline,
    text: &mut TextPipeline,
    atlas: &GlyphAtlas,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    viewport_size: (f32, f32),
    ops: &'pass [GpuPaintOp],
) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Bound {
        Nothing,
        Solid,
        Gradient,
        Text,
    }
    let mut bound = Bound::Nothing;
    let mut gradient_bind_group: Option<wgpu::BindGroup> = None;
    let mut text_bind_group: Option<wgpu::BindGroup> = None;
    for op in ops {
        match op {
            GpuPaintOp::Solid(mesh, color) => {
                if bound != Bound::Solid {
                    pass.set_pipeline(path.pipeline(device, format));
                    bound = Bound::Solid;
                }
                let bind_group = path.bind_group(device, queue, viewport_size, *color);
                pass.set_bind_group(0, &bind_group, &[]);
                path.draw(pass, mesh);
            }
            GpuPaintOp::Gradient(mesh) => {
                if bound != Bound::Gradient {
                    pass.set_pipeline(gradient.pipeline(device, format));
                    let bind_group = gradient_bind_group
                        .get_or_insert_with(|| gradient.bind_group(device, queue, viewport_size));
                    pass.set_bind_group(0, &*bind_group, &[]);
                    bound = Bound::Gradient;
                }
                gradient.draw(pass, mesh);
            }
            GpuPaintOp::Text(mesh, _) => {
                if bound != Bound::Text {
                    pass.set_pipeline(text.pipeline(device, format));
                    let bind_group = text_bind_group.get_or_insert_with(|| {
                        text.bind_group(device, queue, viewport_size, atlas)
                    });
                    pass.set_bind_group(0, &*bind_group, &[]);
                    bound = Bound::Text;
                }
                text.draw(pass, mesh);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuColorMesh, GpuMesh, GradientPipeline, PathPipeline, gradient_fragment_entry};
    use crate::test_support::real_context;
    use aurora_vector::{ColorMesh, Mesh, Point, horizontal_strip};

    fn triangle_mesh() -> Mesh {
        Mesh {
            vertices: vec![
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(0.0, 10.0),
            ],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn pipeline_is_cached_by_target_format() {
        let Some(context) = real_context() else {
            return;
        };
        let mut path = PathPipeline::new(context.device());
        assert_eq!(path.cache.len(), 0);
        let _ = path.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(path.cache.len(), 1);
        let _ = path.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(path.cache.len(), 1, "same format must hit the cache");
        let _ = path.pipeline(context.device(), wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(path.cache.len(), 2, "a different format must rebuild");
    }

    #[test]
    fn bind_group_can_be_built_from_real_parameters() {
        let Some(context) = real_context() else {
            return;
        };
        let path = PathPipeline::new(context.device());
        // No assertion beyond "doesn't panic" -- a `wgpu::BindGroup`
        // exposes nothing else to introspect; the real proof this shape
        // is correct is `render_test.rs`, which actually draws with it.
        let _ = path.bind_group(
            context.device(),
            context.queue(),
            (100.0, 100.0),
            [1.0, 0.0, 0.0, 1.0],
        );
    }

    #[test]
    fn gpu_mesh_upload_of_an_empty_mesh_does_not_panic() {
        let Some(context) = real_context() else {
            return;
        };
        let empty = Mesh::default();
        let uploaded = GpuMesh::upload(context.device(), context.queue(), &empty);
        assert_eq!(uploaded.index_count, 0);
    }

    #[test]
    fn gpu_mesh_upload_of_a_real_triangle_records_its_index_count() {
        let Some(context) = real_context() else {
            return;
        };
        let uploaded = GpuMesh::upload(context.device(), context.queue(), &triangle_mesh());
        assert_eq!(uploaded.index_count, 3);
    }

    #[test]
    fn gradient_fragment_entry_linearizes_only_for_srgb_targets() {
        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
        ] {
            assert_eq!(gradient_fragment_entry(format), "fs_gradient_srgb_target");
        }
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba16Float,
        ] {
            assert_eq!(gradient_fragment_entry(format), "fs_gradient");
        }
    }

    #[test]
    fn gradient_pipeline_is_cached_by_target_format_including_srgb() {
        let Some(context) = real_context() else {
            return;
        };
        let mut gradient = GradientPipeline::new(context.device());
        assert_eq!(gradient.cache.len(), 0);
        let _ = gradient.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        let _ = gradient.pipeline(context.device(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(gradient.cache.len(), 1, "same format must hit the cache");
        let _ = gradient.pipeline(context.device(), wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(
            gradient.cache.len(),
            2,
            "an sRGB target is a separate pipeline"
        );
        let _ = gradient.pipeline(context.device(), wgpu::TextureFormat::Bgra8UnormSrgb);
        assert_eq!(gradient.cache.len(), 3);
    }

    #[test]
    fn gpu_color_mesh_upload_records_its_index_count() {
        let Some(context) = real_context() else {
            return;
        };
        let empty = GpuColorMesh::upload(context.device(), context.queue(), &ColorMesh::default());
        assert_eq!(empty.index_count, 0);
        let strip = horizontal_strip(
            0.0,
            0.0,
            10.0,
            10.0,
            &[[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]],
        );
        let uploaded = GpuColorMesh::upload(context.device(), context.queue(), &strip);
        assert_eq!(uploaded.index_count, 6);
    }
}
