//! GPU side: one device, one frame, canvas and UI together (PRD §7.3.8).
//!
//! The canvas is a tile-aligned RGBA16F texture covering the viewport plus one
//! tile of margin. Visible tiles are composited (base over stroke — the render
//! graph, such as it is) and uploaded only when dirty. The UI is drawn into the
//! same render pass immediately afterwards.

use crate::doc::Document;
use crate::tiles::{CHANNELS, TEXELS, TILE, TileId};
use half::f16;
use std::collections::{HashMap, HashSet};

pub const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CanvasUniform {
    uv_offset: [f32; 2],
    uv_scale: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Rect {
    ndc: [f32; 4],
    colour: [f32; 4],
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,

    canvas_tex: wgpu::Texture,
    canvas_view: wgpu::TextureView,
    canvas_tiles: (u32, u32),
    /// Top-left visible tile.
    origin: TileId,
    /// Which tile currently occupies each slot. Slots are addressed toroidally
    /// (tile index modulo grid size), so panning by one tile invalidates one
    /// row or column rather than the whole texture.
    slots: HashMap<(u32, u32), TileId>,

    canvas_pipeline: wgpu::RenderPipeline,
    canvas_bind: wgpu::BindGroup,
    canvas_uniform: wgpu::Buffer,

    ui_pipeline: wgpu::RenderPipeline,
    ui_bind: wgpu::BindGroup,
    ui_uniform: wgpu::Buffer,

    scratch: Vec<f16>,
    pub uploads_last_frame: u32,
}

impl Renderer {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
        target_format: wgpu::TextureFormat,
        viewport: (u32, u32),
    ) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .expect("no suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("slice"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .expect("no device");

        // Canvas texture covers the viewport plus a tile of margin, tile-aligned
        // so a whole tile can be uploaded in one write_texture.
        let ct = (viewport.0.div_ceil(TILE) + 1, viewport.1.div_ceil(TILE) + 1);
        let canvas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: ct.0 * TILE,
                height: ct.1 * TILE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let canvas_view = canvas_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let canvas_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas uniform"),
            size: std::mem::size_of::<CanvasUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ui_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui rects"),
            size: (std::mem::size_of::<Rect>() * 32) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slice"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let canvas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas"),
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
        let ui_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui"),
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

        let canvas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas"),
            layout: &canvas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&canvas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: canvas_uniform.as_entire_binding(),
                },
            ],
        });
        let ui_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui"),
            layout: &ui_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ui_uniform.as_entire_binding(),
            }],
        });

        let make_pipeline = |label: &str,
                             layout: &wgpu::BindGroupLayout,
                             vs: &str,
                             fs: &str,
                             blend: Option<wgpu::BlendState>| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                multiview_mask: None,
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                cache: None,
            })
        };

        let canvas_pipeline = make_pipeline("canvas", &canvas_layout, "vs_canvas", "fs_canvas", None);
        let ui_pipeline = make_pipeline(
            "ui",
            &ui_layout,
            "vs_ui",
            "fs_ui",
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        Self {
            device,
            queue,
            adapter,
            canvas_tex,
            canvas_view,
            canvas_tiles: ct,
            origin: TileId { x: 0, y: 0 },
            slots: HashMap::new(),
            canvas_pipeline,
            canvas_bind,
            canvas_uniform,
            ui_pipeline,
            ui_bind,
            ui_uniform,
            scratch: vec![f16::ZERO; TEXELS * CHANNELS],
            uploads_last_frame: 0,
        }
    }

    /// Composite base-over-stroke for any visible tile that needs it, and upload.
    ///
    /// This is the render graph in miniature: two inputs, one output, evaluated
    /// per tile and only where dirty.
    pub fn sync_tiles(&mut self, doc: &mut Document, force: bool) -> std::io::Result<u32> {
        let dirty_base: HashSet<TileId> = doc.base.take_dirty(4096).into_iter().collect();
        let dirty_stroke: HashSet<TileId> = doc.stroke.take_dirty(4096).into_iter().collect();
        let mut uploads = 0;

        for ty in 0..self.canvas_tiles.1 {
            for tx in 0..self.canvas_tiles.0 {
                let id = TileId { x: self.origin.x + tx, y: self.origin.y + ty };
                if id.x >= doc.tiles_across() || id.y >= doc.tiles_down() {
                    continue;
                }
                let slot = (id.x % self.canvas_tiles.0, id.y % self.canvas_tiles.1);
                let resident = self.slots.get(&slot) == Some(&id);
                let needs = force
                    || !resident
                    || dirty_base.contains(&id)
                    || dirty_stroke.contains(&id);
                if !needs {
                    continue;
                }
                if !doc.base.exists(id) && !doc.stroke.exists(id) {
                    continue;
                }

                // base
                match doc.base.get(id)? {
                    Some(t) => self.scratch.copy_from_slice(&t.texels),
                    None => self.scratch.fill(f16::ZERO),
                }
                // stroke over base
                if let Some(s) = doc.stroke.get(id)? {
                    for i in (0..self.scratch.len()).step_by(CHANNELS) {
                        let sa = s.texels[i + 3].to_f32();
                        if sa <= 0.0 {
                            continue;
                        }
                        for c in 0..CHANNELS {
                            let sv = s.texels[i + c].to_f32();
                            let dv = self.scratch[i + c].to_f32();
                            self.scratch[i + c] = f16::from_f32(sv + dv * (1.0 - sa));
                        }
                    }
                }

                let bits: Vec<u16> = self.scratch.iter().map(|t| t.to_bits()).collect();
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.canvas_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: slot.0 * TILE, y: slot.1 * TILE, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&bits),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(TILE * CHANNELS as u32 * 2),
                        rows_per_image: Some(TILE),
                    },
                    wgpu::Extent3d { width: TILE, height: TILE, depth_or_array_layers: 1 },
                );
                self.slots.insert(slot, id);
                uploads += 1;
            }
        }
        self.uploads_last_frame = uploads;
        Ok(uploads)
    }

    pub fn set_view(&mut self, viewport: (u32, u32), scroll: (u32, u32)) {
        self.origin = TileId { x: scroll.0 / TILE, y: scroll.1 / TILE };
        let tex_w = (self.canvas_tiles.0 * TILE) as f32;
        let tex_h = (self.canvas_tiles.1 * TILE) as f32;
        // Absolute scroll, wrapped by the repeat sampler: slot addressing is
        // toroidal, so the texture is a sliding window over the document.
        let u = (scroll.0 % (self.canvas_tiles.0 * TILE)) as f32 / tex_w;
        let v = (scroll.1 % (self.canvas_tiles.1 * TILE)) as f32 / tex_h;
        let uni = CanvasUniform {
            uv_offset: [u, v],
            uv_scale: [viewport.0 as f32 / tex_w, viewport.1 as f32 / tex_h],
        };
        self.queue
            .write_buffer(&self.canvas_uniform, 0, bytemuck::bytes_of(&uni));
    }

    /// A few flat rects standing in for a docked panel. Not a widget toolkit —
    /// just enough to prove UI and canvas share a device and a frame.
    pub fn set_ui(&mut self, viewport: (u32, u32), busy: f32) {
        let px_x = |x: f32| x / viewport.0 as f32 * 2.0 - 1.0;
        let px_y = |y: f32| 1.0 - y / viewport.1 as f32 * 2.0;
        let panel_w = 220.0;
        let mut rects = [Rect { ndc: [0.0; 4], colour: [0.0; 4] }; 32];

        // Panel background.
        rects[0] = Rect {
            ndc: [px_x(0.0), px_y(0.0), px_x(panel_w), px_y(viewport.1 as f32)],
            colour: [0.13, 0.13, 0.145, 0.96],
        };
        // Header.
        rects[1] = Rect {
            ndc: [px_x(0.0), px_y(0.0), px_x(panel_w), px_y(36.0)],
            colour: [0.17, 0.17, 0.19, 1.0],
        };
        // Rows, standing in for layer entries.
        for i in 0..6 {
            let top = 48.0 + i as f32 * 34.0;
            rects[2 + i] = Rect {
                ndc: [px_x(10.0), px_y(top), px_x(panel_w - 10.0), px_y(top + 26.0)],
                colour: if i == 0 { [0.24, 0.26, 0.32, 1.0] } else { [0.17, 0.17, 0.19, 1.0] },
            };
        }
        // Upload-activity meter: a visible sign the canvas is doing work.
        rects[8] = Rect {
            ndc: [
                px_x(10.0),
                px_y(viewport.1 as f32 - 24.0),
                px_x(10.0 + busy.clamp(0.0, 1.0) * (panel_w - 20.0)),
                px_y(viewport.1 as f32 - 12.0),
            ],
            colour: [0.35, 0.55, 0.85, 1.0],
        };

        self.queue
            .write_buffer(&self.ui_uniform, 0, bytemuck::cast_slice(&rects));
    }

    pub fn draw(&self, target: &wgpu::TextureView) {
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.08,
                            g: 0.08,
                            b: 0.09,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.canvas_pipeline);
            pass.set_bind_group(0, &self.canvas_bind, &[]);
            pass.draw(0..3, 0..1);

            // Same pass, same frame — this is invariant §7.3.8 in one line.
            pass.set_pipeline(&self.ui_pipeline);
            pass.set_bind_group(0, &self.ui_bind, &[]);
            pass.draw(0..6, 0..9);
        }
        self.queue.submit(Some(enc.finish()));
    }

    #[allow(dead_code)] // Useful when debugging the canvas texture directly.
    pub fn canvas_view(&self) -> &wgpu::TextureView {
        &self.canvas_view
    }
}
