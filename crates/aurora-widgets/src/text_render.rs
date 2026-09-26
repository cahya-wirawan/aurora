//! The GPU half of widget text (0.132.0): a glyph atlas, the glyph quad
//! mesh, and the text pipeline — next to [`crate::render::PathPipeline`]
//! because `aurora-text` may not depend on `aurora-gpu`
//! (`scripts/layering.json`), so shaping and rasterization happen there
//! and the texture work happens here.
//!
//! **Atlas.** One `R8Unorm` texture of [`GlyphAtlas::SIZE`]² texels,
//! packed shelf by shelf ([`aurora_text::ShelfPacker`], one texel of
//! padding). A frame's glyphs are made resident all at once
//! ([`GlyphAtlas::prepare`]) *before* any glyph mesh is built, so the
//! atlas can never be reset under a mesh that already points into it.
//! When a frame's glyphs do not fit, the atlas clears and retries once;
//! if they still do not fit (a single frame needing more than the whole
//! atlas) the glyphs that did not fit are skipped and a warning logged.
//! There is no eviction policy beyond that reset.
//!
//! **Quads.** Every quad is on whole physical pixels and the same size as
//! its glyph image, so the shader reads one texel per fragment with
//! `textureLoad` — no sampler, no filtering, no blur. Vertex positions are
//! logical (physical ÷ scale factor), matching every other mesh this crate
//! draws against a logical `viewport_size`.
//!
//! **Batching.** A frame's glyph quads, across every run, go into *one*
//! vertex buffer and *one* index buffer ([`upload_glyph_meshes`]); each
//! run's [`GpuGlyphMesh`] is a range of indices into those shared buffers,
//! and its colour rides in its own vertices, so a frame of text is three
//! buffer allocations (vertices, indices, the viewport uniform) and one
//! bind group (viewport + atlas) however many
//! labels it has — and a frame with no inked glyph allocates nothing.
//! Runs still draw one by one, in paint order, interleaved with the
//! frame's solids and gradients.
//!
//! **Blending** is the path pipeline's straight-alpha blend, of coverage ×
//! the token colour's alpha. On an sRGB-aware target that blend happens in
//! linear light, which draws antialiased text a little lighter and thinner
//! than a gamma-space blend would; this is disclosed, not corrected.

use std::collections::HashMap;

use aurora_gpu::{Blend, PipelineCache, PipelineKey};
use aurora_text::{GlyphKey, ShelfPacker, TextEngine};

use crate::paint::PaintOp;
use crate::render::{GpuColorMesh, GpuMesh, GpuPaintOp};
use crate::text::{QuadGlyph, resolve_text};

const TEXT_SHADER: &str = include_str!("shaders/text.wgsl");
const LABEL: &str = "text";
/// `position: vec2<f32>` + `uv: vec2<f32>` + `color: vec4<f32>`.
const VERTEX_SIZE: u64 = 32;
/// `viewport_size: vec2<f32>` plus 8 bytes of padding.
const UNIFORM_SIZE: u64 = 16;

/// Where one glyph image sits in the atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtlasSlot {
    /// The glyph has no coverage (a space) or can never fit; draws nothing.
    Empty,
    /// The glyph's image occupies `[x, x + w) × [y, y + h)`.
    Placed { x: u32, y: u32, w: u32, h: u32 },
}

/// The CPU-side bookkeeping of a glyph atlas — which glyph is where — kept
/// apart from the texture so its reset logic is testable headlessly.
#[derive(Debug, Clone)]
pub struct AtlasLayout {
    packer: ShelfPacker,
    slots: HashMap<GlyphKey, AtlasSlot>,
    resets: u64,
}

/// A glyph image newly placed by [`AtlasLayout::place_all`], waiting to be
/// written into the texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpload {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

impl AtlasLayout {
    /// An empty layout over a `size` × `size` atlas.
    #[must_use]
    pub fn new(size: u32) -> Self {
        Self {
            packer: ShelfPacker::new(size, size),
            slots: HashMap::new(),
            resets: 0,
        }
    }

    /// How many times the atlas has been cleared because it was full.
    #[must_use]
    pub fn resets(&self) -> u64 {
        self.resets
    }

    /// Where `key` is, if resident.
    #[must_use]
    pub fn slot(&self, key: GlyphKey) -> Option<AtlasSlot> {
        self.slots.get(&key).copied()
    }

    /// How many glyphs (including empty ones) are resident.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether nothing is resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Makes every key in `keys` resident, rasterizing through `engine`.
    /// If they do not all fit, clears everything and tries once more
    /// (every key of *this* call is then placed fresh, so nothing this
    /// frame points at a reused slot). Returns the images to upload, in
    /// order, and whether every key fit.
    pub fn place_all(
        &mut self,
        engine: &mut TextEngine,
        keys: &[GlyphKey],
    ) -> (Vec<PendingUpload>, bool) {
        let mut uploads = Vec::new();
        // A glyph bigger than the whole atlas can never be drawn; it is
        // recorded as empty (so it is not retried every frame) but still
        // reported as not fitting, and it never triggers a reset.
        let mut oversized = false;
        for attempt in 0..2 {
            let mut all_fit = true;
            for key in keys {
                if self.slots.contains_key(key) {
                    continue;
                }
                // Cheap pre-check before rasterizing: at a size four times
                // the atlas side even a glyph a quarter of an em across
                // cannot fit, so it is refused without the rasterization
                // (which, at a pathological size, is the expensive part).
                #[allow(clippy::cast_precision_loss)]
                let side = self.packer.width().min(self.packer.height()) as f32;
                if key.size_phys().is_nan() || key.size_phys() > side * 4.0 {
                    self.slots.insert(*key, AtlasSlot::Empty);
                    oversized = true;
                    continue;
                }
                let Some(mask) = engine.glyph(*key) else {
                    self.slots.insert(*key, AtlasSlot::Empty);
                    continue;
                };
                if mask.width > self.packer.width() || mask.height > self.packer.height() {
                    self.slots.insert(*key, AtlasSlot::Empty);
                    oversized = true;
                    continue;
                }
                let Some((x, y)) = self.packer.insert(mask.width, mask.height) else {
                    all_fit = false;
                    if attempt == 0 {
                        break;
                    }
                    continue;
                };
                self.slots.insert(
                    *key,
                    AtlasSlot::Placed {
                        x,
                        y,
                        w: mask.width,
                        h: mask.height,
                    },
                );
                uploads.push(PendingUpload {
                    x,
                    y,
                    width: mask.width,
                    height: mask.height,
                    alpha: mask.alpha.clone(),
                });
            }
            if all_fit {
                return (uploads, !oversized);
            }
            if attempt == 0 {
                // Full: start over. Uploads queued before the reset point
                // at slots that no longer exist; drop them rather than
                // write bytes the re-placement overwrites anyway.
                uploads.clear();
                self.packer.clear();
                self.slots.clear();
                self.resets = self.resets.saturating_add(1);
            }
        }
        (uploads, false)
    }

    /// The quad vertices (`[x, y, u, v]`, logical position, texel-unit uv)
    /// and indices for `quads` at `scale_factor`. A quad whose glyph is not
    /// resident, or is empty, is skipped.
    #[must_use]
    pub fn mesh_data(&self, quads: &[QuadGlyph], scale_factor: f32) -> (Vec<[f32; 4]>, Vec<u32>) {
        let mut vertices = Vec::with_capacity(quads.len() * 4);
        let mut indices = Vec::with_capacity(quads.len() * 6);
        let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        for quad in quads {
            let Some(AtlasSlot::Placed { x, y, w, h }) = self.slot(quad.key) else {
                continue;
            };
            let [x0, y0, x1, y1] = quad.dst;
            #[allow(clippy::cast_possible_wrap)]
            let (qw, qh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            let [ox, oy] = quad.src_offset;
            let fits = u32::try_from(qw)
                .ok()
                .zip(u32::try_from(qh).ok())
                .is_some_and(|(qw, qh)| ox.saturating_add(qw) <= w && oy.saturating_add(qh) <= h);
            if !fits {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let (u0, v0) = ((x + ox) as f32, (y + oy) as f32);
            #[allow(clippy::cast_precision_loss)]
            let (u1, v1) = (u0 + qw as f32, v0 + qh as f32);
            #[allow(clippy::cast_precision_loss)]
            let (px0, py0, px1, py1) = (
                x0 as f32 / scale,
                y0 as f32 / scale,
                x1 as f32 / scale,
                y1 as f32 / scale,
            );
            let Ok(base) = u32::try_from(vertices.len()) else {
                break;
            };
            vertices.push([px0, py0, u0, v0]);
            vertices.push([px1, py0, u1, v0]);
            vertices.push([px1, py1, u1, v1]);
            vertices.push([px0, py1, u0, v1]);
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (vertices, indices)
    }
}

/// Uploads a frame's [`PaintOp`]s, in order, as [`GpuPaintOp`]s: every
/// text run is resolved ([`resolve_text`]), *all* of the frame's glyphs are
/// made resident in `atlas` in one [`GlyphAtlas::prepare`], and only then
/// are the glyph meshes built — so an atlas reset can never strand a mesh
/// built earlier in the same frame. `color` converts a `Solid`'s or a
/// `Text`'s token colour for the target (identity for a non-sRGB target,
/// linearization for an sRGB one); gradient vertex colours are passed
/// through untouched, as before. A run that resolves to no quads (a fully
/// clipped label, or only spaces) uploads no op.
pub fn upload_paint_ops(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &mut TextEngine,
    atlas: &mut GlyphAtlas,
    ops: Vec<PaintOp>,
    scale_factor: f32,
    color: impl Fn([f32; 4]) -> [f32; 4],
) -> Vec<GpuPaintOp> {
    enum Staged {
        Solid(GpuMesh, [f32; 4]),
        Gradient(GpuColorMesh),
        Text(Vec<QuadGlyph>, [f32; 4]),
    }
    let staged: Vec<Staged> = ops
        .into_iter()
        .map(|op| match op {
            PaintOp::Solid((mesh, c)) => {
                Staged::Solid(GpuMesh::upload(device, queue, &mesh), color(c))
            }
            PaintOp::Gradient(mesh) => Staged::Gradient(GpuColorMesh::upload(device, queue, &mesh)),
            PaintOp::Text(run) => {
                Staged::Text(resolve_text(engine, &run, scale_factor), color(run.color))
            }
        })
        .collect();
    let all_quads: Vec<QuadGlyph> = staged
        .iter()
        .filter_map(|s| match s {
            Staged::Text(quads, _) => Some(quads.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    if !all_quads.is_empty() {
        atlas.prepare(queue, engine, &all_quads);
    }
    let runs: Vec<(&[QuadGlyph], [f32; 4])> = staged
        .iter()
        .filter_map(|s| match s {
            Staged::Text(quads, c) => Some((quads.as_slice(), *c)),
            _ => None,
        })
        .collect();
    let mut meshes = upload_glyph_meshes(device, queue, atlas, &runs, scale_factor).into_iter();
    staged
        .into_iter()
        .filter_map(|s| match s {
            Staged::Solid(mesh, c) => Some(GpuPaintOp::Solid(mesh, c)),
            Staged::Gradient(mesh) => Some(GpuPaintOp::Gradient(mesh)),
            Staged::Text(_, c) => meshes
                .next()
                .flatten()
                .map(|mesh| GpuPaintOp::Text(mesh, c)),
        })
        .collect()
}

/// One frame's glyph geometry, every run packed into one vertex and one
/// index byte stream: `ranges[i]` is run `i`'s index range, or `None` when
/// it has no drawable quad (then it contributes no bytes at all).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlyphBatch {
    pub vertex_bytes: Vec<u8>,
    pub index_bytes: Vec<u8>,
    pub ranges: Vec<Option<std::ops::Range<u32>>>,
}

impl GlyphBatch {
    /// Packs `runs` (quads plus the run's already-converted colour) against
    /// `layout`. Vertices are `[x, y, u, v, r, g, b, a]`, little-endian
    /// `f32`; indices are absolute `u32`s into the shared vertex stream.
    #[must_use]
    pub fn build(
        layout: &AtlasLayout,
        runs: &[(&[QuadGlyph], [f32; 4])],
        scale_factor: f32,
    ) -> Self {
        let mut batch = Self::default();
        let mut vertex_count = 0_u32;
        let mut index_count = 0_u32;
        for (quads, color) in runs {
            let (vertices, indices) = layout.mesh_data(quads, scale_factor);
            if indices.is_empty() {
                batch.ranges.push(None);
                continue;
            }
            let (Ok(nv), Ok(ni)) = (u32::try_from(vertices.len()), u32::try_from(indices.len()))
            else {
                batch.ranges.push(None);
                continue;
            };
            let (Some(next_vertex), Some(next_index)) =
                (vertex_count.checked_add(nv), index_count.checked_add(ni))
            else {
                batch.ranges.push(None);
                continue;
            };
            for vertex in &vertices {
                for value in vertex.iter().chain(color.iter()) {
                    batch.vertex_bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            for index in &indices {
                batch
                    .index_bytes
                    .extend_from_slice(&index.saturating_add(vertex_count).to_le_bytes());
            }
            batch.ranges.push(Some(index_count..next_index));
            vertex_count = next_vertex;
            index_count = next_index;
        }
        batch
    }
}

/// Uploads a frame's text runs as [`GpuGlyphMesh`]es sharing one vertex
/// and one index buffer ([`GlyphBatch`]). Returns one entry per run, in
/// order: `None` for a run with nothing to draw. Allocates no buffer at
/// all when no run has a drawable quad. Call after [`GlyphAtlas::prepare`]
/// has seen every one of these quads this frame.
#[must_use]
pub fn upload_glyph_meshes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas: &GlyphAtlas,
    runs: &[(&[QuadGlyph], [f32; 4])],
    scale_factor: f32,
) -> Vec<Option<GpuGlyphMesh>> {
    let batch = GlyphBatch::build(&atlas.layout, runs, scale_factor);
    if batch.index_bytes.is_empty() {
        return batch.ranges.into_iter().map(|_| None).collect();
    }
    let buffer = |bytes: &[u8], usage| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: bytes.len() as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytes);
        buffer
    };
    let vertex_buffer = buffer(&batch.vertex_bytes, wgpu::BufferUsages::VERTEX);
    let index_buffer = buffer(&batch.index_bytes, wgpu::BufferUsages::INDEX);
    batch
        .ranges
        .into_iter()
        .map(|range| {
            range.map(|indices| GpuGlyphMesh {
                vertex_buffer: vertex_buffer.clone(),
                index_buffer: index_buffer.clone(),
                indices,
            })
        })
        .collect()
}

/// The glyph atlas texture plus its [`AtlasLayout`].
pub struct GlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    layout: AtlasLayout,
}

impl GlyphAtlas {
    /// The atlas's side, in texels.
    pub const SIZE: u32 = 1024;

    /// A fresh, empty atlas.
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_size(device, Self::SIZE)
    }

    /// A fresh, empty atlas of `size` × `size` texels (tests use a small
    /// one to exercise the reset).
    #[must_use]
    pub fn with_size(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            layout: AtlasLayout::new(size),
        }
    }

    /// The CPU-side layout.
    #[must_use]
    pub fn layout(&self) -> &AtlasLayout {
        &self.layout
    }

    /// Makes every glyph `quads` use resident, uploading new images. Call
    /// once per frame with *all* of the frame's quads before building any
    /// [`GpuGlyphMesh`]. Returns whether every glyph fit.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        engine: &mut TextEngine,
        quads: &[QuadGlyph],
    ) -> bool {
        let mut keys: Vec<GlyphKey> = quads.iter().map(|q| q.key).collect();
        // Sort first so `dedup` removes every repeat, not only adjacent
        // ones (`place_all` would skip a resident repeat anyway; this just
        // keeps the warning's count honest).
        keys.sort_by_key(|k| {
            (
                k.glyph_id,
                k.size_phys().to_bits(),
                k.bin.offset().to_bits(),
            )
        });
        keys.dedup();
        let (uploads, all_fit) = self.layout.place_all(engine, &keys);
        for upload in &uploads {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: upload.x,
                        y: upload.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.alpha,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.width),
                    rows_per_image: Some(upload.height),
                },
                wgpu::Extent3d {
                    width: upload.width,
                    height: upload.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        if !all_fit {
            tracing::warn!(
                glyphs = keys.len(),
                "glyph atlas cannot hold one frame's glyphs; some text is not drawn"
            );
        }
        all_fit
    }
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

/// One run's glyph quads: a range of indices into a frame's shared glyph
/// vertex and index buffers ([`upload_glyph_meshes`]). The buffers are
/// reference-counted handles, so every run of a frame holds the same two.
pub struct GpuGlyphMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    indices: std::ops::Range<u32>,
}

impl GpuGlyphMesh {
    /// How many indices (six per drawn glyph) the run has.
    #[must_use]
    pub fn index_count(&self) -> u32 {
        self.indices.end.saturating_sub(self.indices.start)
    }

    /// The run's index range in the shared index buffer.
    #[must_use]
    pub fn indices(&self) -> std::ops::Range<u32> {
        self.indices.clone()
    }

    /// Whether `self` and `other` draw from the same vertex and index
    /// buffers (true for every pair of runs uploaded in one frame).
    #[must_use]
    pub fn shares_buffers_with(&self, other: &Self) -> bool {
        self.vertex_buffer == other.vertex_buffer && self.index_buffer == other.index_buffer
    }
}

impl std::fmt::Debug for GpuGlyphMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuGlyphMesh")
            .field("indices", &self.indices)
            .finish_non_exhaustive()
    }
}

/// Builds and caches the text render pipeline, and the per-draw bind group
/// (viewport size + the atlas; colour rides in the vertices).
pub struct TextPipeline {
    layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    cache: PipelineCache,
}

impl TextPipeline {
    #[must_use]
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(LABEL),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(LABEL),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });
        Self {
            layout,
            shader,
            cache: PipelineCache::new(),
        }
    }

    /// How many target formats have a cached pipeline.
    #[must_use]
    pub fn cached_pipelines(&self) -> usize {
        self.cache.len()
    }

    /// The cached pipeline for `target_format` (built on first use), with
    /// the path pipeline's straight-alpha blend.
    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_text",
            fragment_entry: "fs_text",
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
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                                shader_location: 2,
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

    /// The bind group every glyph run of a frame shares: `viewport_size`
    /// (logical, as for [`crate::render::PathPipeline::bind_group`]) and
    /// `atlas`. Colours ride in the vertices ([`GlyphBatch`]).
    #[must_use]
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_size: (f32, f32),
        atlas: &GlyphAtlas,
    ) -> wgpu::BindGroup {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(LABEL),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&viewport_size.0.to_le_bytes());
        bytes.extend_from_slice(&viewport_size.1.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        queue.write_buffer(&uniform_buffer, 0, &bytes);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
            ],
        })
    }

    /// Draws `mesh` in `pass`; the pipeline and the frame's bind group must
    /// already be set. An empty mesh binds nothing (see `PathPipeline::draw`).
    pub fn draw<'pass>(&self, pass: &mut wgpu::RenderPass<'pass>, mesh: &'pass GpuGlyphMesh) {
        if mesh.index_count() == 0 {
            return;
        }
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(mesh.indices.clone(), 0, 0..1);
    }
}

impl std::fmt::Debug for TextPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextPipeline")
            .field("cached_pipelines", &self.cache.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{AtlasLayout, AtlasSlot, GlyphBatch};
    use crate::text::{HAlign, QuadGlyph, TextRun, label_style, resolve_text};
    use crate::widgets::test_scales;
    use aurora_core::Rect;
    use aurora_text::{GlyphKey, TextEngine};

    fn engine() -> TextEngine {
        match TextEngine::new() {
            Ok(engine) => engine,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn quads(engine: &mut TextEngine, text: &str, scale: f32) -> Vec<QuadGlyph> {
        let run = TextRun {
            text: text.to_owned(),
            style: label_style(&test_scales()),
            color: [1.0; 4],
            rect: (0.0, 0.0, 400.0, 20.0),
            align: HAlign::Start,
            clip: Rect {
                x: 0,
                y: 0,
                width: 400,
                height: 20,
            },
        };
        resolve_text(engine, &run, scale)
    }

    fn keys(quads: &[QuadGlyph]) -> Vec<GlyphKey> {
        quads.iter().map(|q| q.key).collect()
    }

    #[test]
    fn atlas_layout_places_each_glyph_once_and_caches_empty_ones() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(256);
        let q = quads(&mut engine, "abca", 1.0);
        let mut ks = keys(&q);
        let (uploads, fit) = layout.place_all(&mut engine, &ks);
        assert!(fit);
        let mut unique = ks.clone();
        unique.sort_by_key(|k| (k.glyph_id, k.bin.offset().to_bits()));
        unique.dedup();
        assert_eq!(
            uploads.len(),
            unique.len(),
            "each distinct key is placed once"
        );
        assert!(unique.len() >= 3);
        let (again, fit) = layout.place_all(&mut engine, &ks);
        assert!(fit && again.is_empty(), "resident glyphs upload nothing");
        // A space's key (not produced by resolve_text) is cached as empty.
        let line = engine.shape(" ", &label_style(&test_scales()), 1.0);
        let space = GlyphKey::new(
            line.glyphs.first().map(|g| g.glyph_id).unwrap_or_default(),
            line.size_phys,
            aurora_text::SubpixelBin::ZERO,
        );
        ks.push(space);
        let _ = layout.place_all(&mut engine, &ks);
        assert_eq!(layout.slot(space), Some(AtlasSlot::Empty));
    }

    #[test]
    fn atlas_reset_on_full_places_every_glyph_of_the_current_frame() {
        let mut engine = engine();
        // Big glyphs in a small atlas: the first frame nearly fills it.
        let mut layout = AtlasLayout::new(48);
        let first = keys(&quads(&mut engine, "MWQ", 2.0));
        let (_, fit) = layout.place_all(&mut engine, &first);
        assert!(fit);
        let second = keys(&quads(&mut engine, "BDKR", 2.0));
        let (uploads, fit) = layout.place_all(&mut engine, &second);
        assert!(fit, "the second frame alone fits after one reset");
        assert_eq!(layout.resets(), 1);
        // Images placed before the reset are re-uploaded after it, in
        // queue order, so the last `second.len()` uploads are the frame.
        assert!(uploads.len() >= second.len());
        for key in &second {
            assert!(matches!(layout.slot(*key), Some(AtlasSlot::Placed { .. })));
        }
        for key in &first {
            assert_eq!(layout.slot(*key), None, "the reset dropped the old frame");
        }
    }

    #[test]
    fn a_frame_larger_than_the_atlas_reports_not_all_fit() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(24);
        let many = keys(&quads(&mut engine, "ABDEFGHKMNOPQRSUVWXYZ", 2.0));
        let (_, fit) = layout.place_all(&mut engine, &many);
        assert!(!fit);
        assert_eq!(layout.resets(), 1, "reset once, not in a loop");
    }

    #[test]
    fn a_glyph_bigger_than_the_atlas_is_not_fit_empty_and_triggers_no_reset() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(24);
        // "M" at 13 px x 4 = 52 px physical: its mask is wider than 24.
        let big = keys(&quads(&mut engine, "M", 4.0));
        let (uploads, fit) = layout.place_all(&mut engine, &big);
        assert!(!fit, "an oversized glyph is reported, not silently dropped");
        assert!(uploads.is_empty());
        assert_eq!(layout.resets(), 0, "a reset cannot help an oversized glyph");
        for key in &big {
            assert_eq!(layout.slot(*key), Some(AtlasSlot::Empty));
        }
        // Far past the atlas: refused by size alone, before rasterizing.
        let Some(first) = big.first() else {
            unreachable!("M has ink")
        };
        let huge = GlyphKey::new(first.glyph_id, 5000.0, first.bin);
        let (_, fit) = layout.place_all(&mut engine, &[huge]);
        assert!(!fit);
        assert_eq!(layout.slot(huge), Some(AtlasSlot::Empty));
        // A small glyph alongside still places, and reports all-fit alone.
        let small = keys(&quads(&mut engine, "i", 1.0));
        let (_, fit) = layout.place_all(&mut engine, &small);
        assert!(fit);
    }

    #[test]
    fn a_reset_frame_uploads_only_the_current_frames_glyphs() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(48);
        let first = keys(&quads(&mut engine, "MWQ", 2.0));
        let _ = layout.place_all(&mut engine, &first);
        // "B" still fits beside the first frame and is queued before the
        // atlas runs out at "D"; the reset must drop that stale upload.
        let second = keys(&quads(&mut engine, "BDKR", 2.0));
        let (uploads, fit) = layout.place_all(&mut engine, &second);
        assert!(fit);
        assert_eq!(layout.resets(), 1);
        let mut unique = second.clone();
        unique.sort_by_key(|k| (k.glyph_id, k.bin.offset().to_bits()));
        unique.dedup();
        assert_eq!(
            uploads.len(),
            unique.len(),
            "exactly one upload per glyph of the frame, none from before the reset"
        );
        let expected_bytes: usize = unique
            .iter()
            .filter_map(|k| match layout.slot(*k) {
                Some(AtlasSlot::Placed { w, h, .. }) => Some((w * h) as usize),
                _ => None,
            })
            .sum();
        let uploaded: usize = uploads.iter().map(|u| u.alpha.len()).sum();
        assert_eq!(uploaded, expected_bytes);
    }

    #[test]
    fn a_glyph_batch_shares_one_stream_and_an_empty_frame_has_no_bytes() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(256);
        let a = quads(&mut engine, "Hi", 1.0);
        let b = quads(&mut engine, "Ok", 1.0);
        let mut all = keys(&a);
        all.extend(keys(&b));
        let _ = layout.place_all(&mut engine, &all);
        let red = [1.0, 0.0, 0.0, 1.0];
        let blue = [0.0, 0.0, 1.0, 0.5];
        let empty = GlyphBatch::build(&layout, &[(&[], red)], 1.0);
        assert!(empty.vertex_bytes.is_empty() && empty.index_bytes.is_empty());
        assert_eq!(empty.ranges, vec![None]);
        let batch = GlyphBatch::build(&layout, &[(&a, red), (&[], red), (&b, blue)], 1.0);
        assert_eq!(batch.ranges, vec![Some(0..12), None, Some(12..24)]);
        assert_eq!(
            batch.vertex_bytes.len(),
            16 * 32,
            "sixteen vertices of 32 bytes"
        );
        assert_eq!(batch.index_bytes.len(), 24 * 4);
        // The second run's indices point past the first run's vertices.
        let index = |i: usize| {
            let bytes = batch.index_bytes.get(i * 4..i * 4 + 4).unwrap_or_default();
            u32::from_le_bytes(bytes.try_into().unwrap_or_default())
        };
        assert_eq!((index(0), index(12)), (0, 8));
        // Colour rides in each vertex: the last vertex carries blue.
        let tail = batch
            .vertex_bytes
            .get(batch.vertex_bytes.len() - 16..)
            .unwrap_or_default();
        let floats: Vec<f32> = tail
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap_or_default()))
            .collect();
        assert_eq!(floats, blue.to_vec());
    }

    #[test]
    // Exact by construction: whole texel indices and whole physical
    // pixels divided by 2.0 are all exactly representable.
    #[allow(clippy::float_cmp)]
    fn mesh_data_trims_uvs_by_src_offset_and_divides_by_scale() {
        let mut engine = engine();
        let mut layout = AtlasLayout::new(256);
        let q = quads(&mut engine, "H", 2.0);
        let _ = layout.place_all(&mut engine, &keys(&q));
        let Some(mut quad) = q.first().copied() else {
            unreachable!("H has ink");
        };
        let Some(AtlasSlot::Placed { x, y, .. }) = layout.slot(quad.key) else {
            unreachable!("H is placed");
        };
        quad.dst[0] += 3;
        quad.src_offset = [3, 0];
        let (vertices, indices) = layout.mesh_data(&[quad], 2.0);
        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
        let Some(top_left) = vertices.first() else {
            unreachable!()
        };
        #[allow(clippy::cast_precision_loss)]
        let expected = [
            quad.dst[0] as f32 / 2.0,
            quad.dst[1] as f32 / 2.0,
            (x + 3) as f32,
            y as f32,
        ];
        assert_eq!(*top_left, expected);
        // A source window past the glyph's own image is refused.
        quad.src_offset = [1000, 0];
        assert!(layout.mesh_data(&[quad], 2.0).0.is_empty());
    }
}
