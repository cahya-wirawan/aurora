//! GPU-side tile compositing: blends a source tile over a destination
//! tile using the GPU's fixed-function alpha blend unit, replacing the
//! CPU per-pixel merge `spike/FINDINGS.md` finding #1 measured at ~20ms
//! and named as the actual compositing bottleneck (not disk I/O, which
//! the same spike found fast). PLAN.md M1.3.

use aurora_gpu::{Blend, GpuContext, PipelineCache, PipelineKey};
use aurora_tile::{CHANNELS, SAMPLES};
use half::f16;

const COMPOSITE_SHADER: &str = include_str!("shaders/composite.wgsl");
const LABEL: &str = "composite";

/// The subset of `aurora_doc::BlendMode`'s real 27-variant, PSD-
/// round-trippable enum this crate actually implements blend math for.
/// `aurora-render` sits below `aurora-doc` in PRD §7.2's layering (the
/// two are siblings — neither may depend on the other), so this can't
/// just be `aurora_doc::BlendMode` reused directly; it's a deliberately
/// narrower, purely additive enum, the same "widen when you actually
/// need to" discipline `aurora_widgets::paint::paint_widget`'s own
/// `Vec<Paint>` return type already established (see that module's own
/// doc comment) — only variants with real, implemented math below, not
/// all 27 with most unimplemented (which would force either a
/// forbidden `panic!`/`unwrap`/`unreachable!` fallback or silently
/// wrong pixels for an unhandled mode).
///
/// `aurora-app`'s `translate_blend_mode` maps a real
/// `aurora_doc::BlendMode` onto this one, one implemented variant at a
/// time, falling every one of the ~14 not-yet-implemented modes back to
/// [`Self::Normal`] as an honest, documented degrade — not a bug, since
/// those modes' real math is separate, still-open follow-on work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    Lighten,
    Screen,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    ColorDodge,
    LinearDodge,
    ColorBurn,
    LinearBurn,
}

/// The per-channel blend function `B(Cb, Cs)` for `mode`, given one
/// backdrop channel `cb` and one source channel `cs`, both straight
/// (unpremultiplied) and in `0.0..=1.0` — the piece of the general
/// compositing formula
/// `Co = (1-as)*Cb + as*[(1-ab)*Cs + ab*B(Cb,Cs)]` that actually varies
/// by blend mode; everything else in that formula is blend-mode-
/// independent alpha compositing, unchanged by `mode`.
///
/// `Normal`'s own `B(Cb, Cs) = Cs` — the generalized formula above
/// reduces to exactly `(1-as)*Cb + as*Cs`, character for character the
/// same formula this module used before blend modes existed, proven by
/// every pre-existing Normal-mode test in this file still passing
/// unchanged against the generalized code.
#[must_use]
// `ColorDodge`/`ColorBurn`'s `cs == 1.0`/`cb == 1.0` branch checks are
// the W3C Compositing and Blending spec's own literal 0/1 boundary
// conditions (guarding a division that would otherwise divide by zero),
// not the "accumulated rounding error" smell `clippy::float_cmp`
// otherwise warns about (the same reasoning `aurora-doc`'s and this
// crate's own other `float_cmp` allows already document) -- `cb`/`cs`
// arrive here as exact `f16`-sourced values (`0.0`/`1.0` round-trip
// bit-exact through `f16`, confirmed by `spike/FINDINGS.md`), and the
// spec requires exactly these two literals, not an epsilon band.
#[allow(clippy::float_cmp)]
fn blend_channel(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Darken => cb.min(cs),
        BlendMode::Multiply => cb * cs,
        BlendMode::Lighten => cb.max(cs),
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        BlendMode::Subtract => (cb - cs).max(0.0),
        BlendMode::Divide => {
            if cs == 0.0 {
                1.0
            } else {
                (cb / cs).min(1.0)
            }
        }
        BlendMode::ColorDodge => {
            if cb == 0.0 {
                0.0
            } else if cs == 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::LinearDodge => (cb + cs).min(1.0),
        BlendMode::ColorBurn => {
            if cb == 1.0 {
                1.0
            } else if cs == 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::LinearBurn => (cb + cs - 1.0).max(0.0),
    }
}

/// Composites `layers` (bottom-to-top; each entry is one tile's own full
/// [`aurora_tile::SAMPLES`]-length `f16` texel buffer, that layer's own
/// opacity, and that layer's own [`BlendMode`]) via straight-alpha
/// compositing generalized to a per-blend-mode source colour — the
/// CPU-side sibling of [`TileCompositor::composite_over`]'s own GPU
/// shader math, needed because the actual orchestration this crate
/// can't do itself (walking a real `aurora_doc::LayerTree` to decide
/// *which* layers, in what order, at what opacity and blend mode)
/// can't live here: `aurora-render` and `aurora-doc` are sibling crates
/// in PRD §7.2's layering (neither depends on the other), so
/// `aurora-app`, which depends on both, is where that walk actually
/// happens (`translate_blend_mode` converts a real
/// `aurora_doc::BlendMode` into this crate's own [`BlendMode`] at that
/// boundary) — this is the pure per-tile math it calls once it has
/// real layer data in hand, exactly what this module's own
/// [`TileCompositor`] doc comment already anticipated ("the primitive
/// real layer compositing will call once that model exists").
///
/// Per texel, per RGB channel: `Co = (1-as)*Cb + as*[(1-ab)*Cs +
/// ab*B(Cb,Cs)]`, where `Cb`/`Cs` are the backdrop/source channel,
/// `as = src_a * opacity` (clamped to `0.0..=1.0`), `ab` is the
/// backdrop's own alpha, and `B` is [`blend_channel`]'s own per-mode
/// function. The alpha channel itself is blend-mode-independent:
/// `result_a = as + dst_a * (1 - as)`, unchanged from before blend
/// modes existed and the same formula [`TileCompositor::composite_over`]'s
/// own GPU blend unit computes (proven by that function's own
/// `composite_over_blends_source_over_destination` test) — this
/// function is that same math run on the CPU, generalized by
/// blend mode, and with an opacity factor the fixed-function blend unit
/// has no way to express.
///
/// A `texels` slice whose length isn't a multiple of [`CHANNELS`] has
/// its trailing partial texel silently dropped (`chunks_exact`) rather
/// than erroring or panicking — real callers always pass a genuine
/// `Tile::texels()` slice, exactly [`aurora_tile::SAMPLES`] long, so
/// this only matters for a malformed, never-real input, and dropping a
/// few trailing samples is a safer failure than indexing past the
/// shorter buffer.
///
/// Returns a fresh, [`aurora_tile::SAMPLES`]-length buffer starting from
/// fully transparent black — an empty `layers` composites to that,
/// exactly matching what a document with no visible pixel layers should
/// show.
///
/// **Scope, stated honestly**: [`BlendMode`] implements 13 of
/// `aurora_doc::BlendMode`'s real 27 variants — `Normal`, the
/// "simple separable" family (`Darken`, `Multiply`, `Lighten`,
/// `Screen`, `Difference`, `Exclusion`, `Subtract`, `Divide`), each a
/// pure per-channel function of backdrop and source with no cross-
/// channel or midpoint-branching logic, and the "dodge and burn"
/// family (`ColorDodge`, `LinearDodge`, `ColorBurn`, `LinearBurn`),
/// each also a pure per-channel function but with real 0/1 edge-case
/// branches (division by a zero or saturated channel must not produce
/// `NaN`/`Infinity`). The remaining ~14
/// (`Overlay`/`SoftLight`/`HardLight`/`VividLight`/`LinearLight`/
/// `PinLight`/`HardMix` "overlay and light"; the non-separable
/// `Hue`/`Saturation`/`Color`/`Luminosity`; `Dissolve`; and
/// `DarkerColor`/`LighterColor`) are separate, still-open follow-on
/// work — a layer using one of those falls back to `Normal` at the
/// `aurora-app` translation boundary (`translate_blend_mode`), not
/// here. This is a CPU implementation specifically because the
/// orchestration crate (`aurora-app`) needs to run it per visible tile,
/// per layer, every time any constituent layer changes —
/// GPU-accelerated multi-layer compositing (reusing [`TileCompositor`]
/// properly, with a real opacity/blend-mode-aware shader) is separate,
/// still-open follow-on work.
#[must_use]
pub fn composite_tile_cpu(layers: &[(&[f16], f32, BlendMode)]) -> Vec<f16> {
    let mut out = vec![f16::from_f32(0.0); SAMPLES];
    for &(texels, opacity, mode) in layers {
        let opacity = opacity.clamp(0.0, 1.0);
        for (dst, src) in out
            .chunks_exact_mut(CHANNELS)
            .zip(texels.chunks_exact(CHANNELS))
        {
            let [dr, dg, db, da] = dst else { continue };
            let [sr, sg, sb, sa] = src else { continue };
            let alpha = sa.to_f32() * opacity;
            let inverse = 1.0 - alpha;
            let backdrop_alpha = da.to_f32();
            let backdrop_inverse = 1.0 - backdrop_alpha;
            let blended_r = backdrop_inverse * sr.to_f32()
                + backdrop_alpha * blend_channel(mode, dr.to_f32(), sr.to_f32());
            let blended_g = backdrop_inverse * sg.to_f32()
                + backdrop_alpha * blend_channel(mode, dg.to_f32(), sg.to_f32());
            let blended_b = backdrop_inverse * sb.to_f32()
                + backdrop_alpha * blend_channel(mode, db.to_f32(), sb.to_f32());
            *dr = f16::from_f32(inverse * dr.to_f32() + alpha * blended_r);
            *dg = f16::from_f32(inverse * dg.to_f32() + alpha * blended_g);
            *db = f16::from_f32(inverse * db.to_f32() + alpha * blended_b);
            *da = f16::from_f32(alpha + da.to_f32() * inverse);
        }
    }
    out
}

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
    use super::{BlendMode, TileCompositor, composite_tile_cpu};
    use crate::test_support::real_context;
    use aurora_tile::{SAMPLES, TILE};
    use half::f16;

    /// A `SAMPLES`-length buffer of one solid `rgba` texel repeated —
    /// the CPU-side sibling of the GPU tests' own `solid_tile` below,
    /// same shape.
    fn solid_texels(rgba: [f32; 4]) -> Vec<f16> {
        let mut out = Vec::with_capacity(SAMPLES);
        for _ in 0..(SAMPLES / 4) {
            for channel in rgba {
                out.push(f16::from_f32(channel));
            }
        }
        out
    }

    /// Reads the first texel back out of a `composite_tile_cpu` result.
    fn first_texel(texels: &[f16]) -> (f32, f32, f32, f32) {
        let [r, g, b, a, ..] = texels else {
            unreachable!("a SAMPLES-length buffer has at least one texel");
        };
        (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32())
    }

    #[test]
    fn composite_tile_cpu_of_no_layers_is_fully_transparent_black() {
        let out = composite_tile_cpu(&[]);
        assert_eq!(out.len(), SAMPLES);
        assert_eq!(first_texel(&out), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn composite_tile_cpu_matches_the_gpu_shaders_own_source_over_math() {
        // Same case `composite_over_blends_source_over_destination`
        // proves on the GPU: opaque blue dst, half-transparent red src
        // -> (0.5, 0.0, 0.5, 1.0).
        let dst = solid_texels([0.0, 0.0, 1.0, 1.0]);
        let src = solid_texels([1.0, 0.0, 0.0, 0.5]);
        let out = composite_tile_cpu(&[
            (&dst, 1.0, BlendMode::Normal),
            (&src, 1.0, BlendMode::Normal),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.0, 0.5, 1.0));
    }

    #[test]
    // Exact-literal round-trip (0.25/0.5/0.75, powers of two -- unlike
    // 0.2/0.4/0.6, these round-trip exactly through f16), same reasoning
    // `aurora-doc`'s own tests already document for their float_cmp
    // allows.
    fn composite_tile_cpu_a_single_layer_at_full_opacity_reproduces_it_over_transparent() {
        let src = solid_texels([0.25, 0.5, 0.75, 1.0]);
        let out = composite_tile_cpu(&[(&src, 1.0, BlendMode::Normal)]);
        // Over fully transparent black, straight-alpha "over" at full
        // opacity reproduces the source exactly.
        assert_eq!(first_texel(&out), (0.25, 0.5, 0.75, 1.0));
    }

    #[test]
    fn composite_tile_cpu_applies_layer_opacity_on_top_of_the_texels_own_alpha() {
        // A fully opaque source at 50% layer opacity must land at 50%
        // effective alpha, not its own texel alpha unmodified.
        let dst = solid_texels([0.0, 0.0, 0.0, 0.0]);
        let src = solid_texels([1.0, 1.0, 1.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&dst, 1.0, BlendMode::Normal),
            (&src, 0.5, BlendMode::Normal),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.5, 0.5, 0.5));
    }

    #[test]
    fn composite_tile_cpu_clamps_an_out_of_range_opacity() {
        let dst = solid_texels([0.0, 0.0, 0.0, 0.0]);
        let src = solid_texels([1.0, 1.0, 1.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&dst, 1.0, BlendMode::Normal),
            (&src, 5.0, BlendMode::Normal),
        ]);
        assert_eq!(
            first_texel(&out),
            (1.0, 1.0, 1.0, 1.0),
            "an opacity above 1.0 must clamp, not overshoot"
        );
    }

    #[test]
    fn composite_tile_cpu_with_a_fully_transparent_top_layer_leaves_the_bottom_unchanged() {
        let dst = solid_texels([0.25, 0.5, 0.75, 1.0]);
        let src = solid_texels([1.0, 1.0, 1.0, 0.0]);
        let out = composite_tile_cpu(&[
            (&dst, 1.0, BlendMode::Normal),
            (&src, 1.0, BlendMode::Normal),
        ]);
        assert_eq!(first_texel(&out), (0.25, 0.5, 0.75, 1.0));
    }

    #[test]
    fn composite_tile_cpu_three_layers_composite_in_the_given_order() {
        // Bottom fully opaque red, middle fully opaque green at 50%
        // layer opacity, top fully transparent (contributes nothing).
        let bottom = solid_texels([1.0, 0.0, 0.0, 1.0]);
        let middle = solid_texels([0.0, 1.0, 0.0, 1.0]);
        let top = solid_texels([0.0, 0.0, 1.0, 0.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&middle, 0.5, BlendMode::Normal),
            (&top, 1.0, BlendMode::Normal),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.5, 0.0, 1.0));
    }

    // -- Blend-mode math: each of the 8 newly-implemented non-Normal
    // modes below, plus Normal itself already proven above. Every test
    // uses a fully opaque backdrop and source at full layer opacity
    // (`as = ab = 1.0`), so the general formula
    // `Co = (1-as)*Cb + as*[(1-ab)*Cs + ab*B(Cb,Cs)]` reduces to exactly
    // `Co = B(Cb,Cs)` -- the bottom layer is drawn `Normal` (over fully
    // transparent black, any mode reproduces the source exactly, so
    // `Normal` there is neutral) purely to seed a real backdrop colour
    // for the top layer's own real blend mode to react against.

    #[test]
    // Darken: min(Cb, Cs) per channel. Bottom (backdrop) 0.25/0.75/0.5,
    // top (source) 0.75/0.25/0.5 -> min(0.25,0.75)=0.25,
    // min(0.75,0.25)=0.25, min(0.5,0.5)=0.5 -> (0.25, 0.25, 0.5, 1.0).
    fn composite_tile_cpu_darken_takes_the_per_channel_minimum() {
        let bottom = solid_texels([0.25, 0.75, 0.5, 1.0]);
        let top = solid_texels([0.75, 0.25, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Darken),
        ]);
        assert_eq!(first_texel(&out), (0.25, 0.25, 0.5, 1.0));
    }

    #[test]
    // Multiply: Cb * Cs. 50% grey multiplied by 50% grey -> 0.5*0.5 =
    // 0.25 per channel, the textbook "multiply darkens" case.
    fn composite_tile_cpu_multiply_blends_two_mid_greys_to_a_quarter_grey() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Multiply),
        ]);
        assert_eq!(first_texel(&out), (0.25, 0.25, 0.25, 1.0));
    }

    #[test]
    // Lighten: max(Cb, Cs) per channel -- the mirror image of Darken's
    // own test case above: (0.25,0.75,0.5) vs (0.75,0.25,0.5) ->
    // max(0.25,0.75)=0.75, max(0.75,0.25)=0.75, max(0.5,0.5)=0.5 ->
    // (0.75, 0.75, 0.5, 1.0).
    fn composite_tile_cpu_lighten_takes_the_per_channel_maximum() {
        let bottom = solid_texels([0.25, 0.75, 0.5, 1.0]);
        let top = solid_texels([0.75, 0.25, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Lighten),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.5, 1.0));
    }

    #[test]
    // Screen: Cb + Cs - Cb*Cs. Backdrop 0.25, source 0.75 ->
    // 0.25 + 0.75 - (0.25*0.75) = 1.0 - 0.1875 = 0.8125.
    fn composite_tile_cpu_screen_lightens_by_the_inverse_multiply_formula() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Screen),
        ]);
        assert_eq!(first_texel(&out), (0.8125, 0.8125, 0.8125, 1.0));
    }

    #[test]
    // Difference: |Cb - Cs|. Backdrop 0.75, source 0.25 ->
    // |0.75 - 0.25| = 0.5.
    fn composite_tile_cpu_difference_is_the_absolute_per_channel_delta() {
        let bottom = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Difference),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    // Exclusion: Cb + Cs - 2*Cb*Cs. Backdrop 0.25, source 0.75 ->
    // 0.25 + 0.75 - 2*(0.25*0.75) = 1.0 - 0.375 = 0.625 -- lower
    // contrast than Difference's own 0.5 for the swapped colour pair
    // above, matching Exclusion's own textbook "softer Difference"
    // description.
    fn composite_tile_cpu_exclusion_is_a_softer_difference() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Exclusion),
        ]);
        assert_eq!(first_texel(&out), (0.625, 0.625, 0.625, 1.0));
    }

    #[test]
    // Subtract: max(Cb - Cs, 0). Backdrop 0.75, source 0.25 ->
    // max(0.75 - 0.25, 0) = 0.5.
    fn composite_tile_cpu_subtract_takes_the_clamped_per_channel_difference() {
        let bottom = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Subtract),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    // Subtract's own clamp: backdrop 0.25, source 0.75 ->
    // 0.25 - 0.75 = -0.5, clamped to 0 rather than going negative.
    fn composite_tile_cpu_subtract_clamps_a_negative_result_to_zero() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Subtract),
        ]);
        assert_eq!(first_texel(&out), (0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    // Divide: min(Cb / Cs, 1.0) for a non-zero source. Backdrop 0.25,
    // source 0.5 -> 0.25 / 0.5 = 0.5, well under the 1.0 clamp.
    fn composite_tile_cpu_divide_computes_the_clamped_per_channel_ratio() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Divide),
        ]);
        assert_eq!(first_texel(&out), (0.5, 0.5, 0.5, 1.0));
    }

    #[test]
    // Divide's own two documented edge cases in one texel, each channel
    // proving one: R has a zero source channel (0.0), which Photoshop's
    // own convention treats as "divide by zero -> white" (1.0), not
    // NaN/infinity -- `half::f16` arithmetic wouldn't panic on a literal
    // 0.0/0.0 either, but the *value* must still be the documented 1.0
    // fallback, which this asserts directly (a stray NaN or +inf would
    // both fail this `assert_eq!`, since neither compares equal to
    // 1.0). B has a non-zero source (0.5) but a larger backdrop (0.75),
    // so 0.75 / 0.5 = 1.5 must clamp down to 1.0 rather than overshoot.
    // G is a plain in-range case (0.25 / 0.5 = 0.5) for contrast against
    // the two edge channels.
    fn composite_tile_cpu_divide_by_a_zero_source_channel_yields_white_not_nan_or_infinity() {
        let bottom = solid_texels([0.5, 0.25, 0.75, 1.0]);
        let top = solid_texels([0.0, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Divide),
        ]);
        let (r, g, b, a) = first_texel(&out);
        assert!(r.is_finite() && g.is_finite() && b.is_finite());
        assert_eq!((r, g, b, a), (1.0, 0.5, 1.0, 1.0));
    }

    // -- Blend-mode math: the 4 "dodge and burn" modes added this round
    // (`ColorDodge`, `LinearDodge`, `ColorBurn`, `LinearBurn`), the same
    // W3C-spec formulas the module-level doc comment on [`blend_channel`]
    // names, following the exact same `as = ab = 1.0` reduction to
    // `Co = B(Cb,Cs)` the 9 pre-existing non-Normal-mode tests above
    // already establish. `ColorDodge`/`ColorBurn` both branch on which of
    // two 0/1 extremes fires first -- the W3C spec (and Photoshop) check
    // the *backdrop*'s own extreme before the *source*'s, so each gets a
    // dedicated test proving the documented branch, not just the smooth
    // in-range formula.

    #[test]
    // ColorDodge in-range case: min(1, Cb / (1 - Cs)). Backdrop 0.375,
    // source 0.5 -> min(1, 0.375 / 0.5) = min(1, 0.75) = 0.75. 0.375 and
    // 0.75 (like 0.25/0.5, unlike 0.4/0.6) are exact eighths, so they
    // round-trip bit-exact through `f16` -- the same discipline this
    // file's own Normal-mode test documents for its own literal choice.
    fn composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio() {
        let bottom = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorDodge),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // ColorDodge's own `Cb == 0` branch, required to fire (and yield 0,
    // not attempt a division) regardless of `Cs` -- proven with three
    // different source values sharing a zero backdrop: R is a plain
    // in-range source (0.3), B is also `Cs == 0` (both formal edge
    // conditions simultaneously zero), and G is `Cs == 1` -- the one
    // input where *both* of `blend_channel`'s `ColorDodge` conditions
    // (`Cb == 0` and `Cs == 1`) are true at once. `Cb` and `Cs` are
    // independent per-channel scalars (one backdrop, one source texel,
    // no relationship enforced between them), so this input is a real,
    // reachable pixel, not a hypothetical -- e.g. a transparent-black
    // backdrop dodge-blended with a fully white source. The W3C order
    // (check the backdrop's own `Cb == 0` extreme first, exactly as
    // `blend_channel` does) resolves it to 0, matching this assertion;
    // checking `Cs == 1` first instead would wrongly yield 1 for that
    // channel -- this test is what would catch that ordering bug.
    fn composite_tile_cpu_color_dodge_with_a_zero_backdrop_yields_zero_regardless_of_source() {
        let bottom = solid_texels([0.0, 0.0, 0.0, 1.0]);
        let top = solid_texels([0.3, 1.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorDodge),
        ]);
        let (r, g, b, a) = first_texel(&out);
        assert!(r.is_finite() && g.is_finite() && b.is_finite());
        assert_eq!((r, g, b, a), (0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    // ColorDodge's own `Cs == 1` branch (backdrop non-zero, so the
    // `Cb == 0` branch above does not fire first): must yield 1 without
    // ever computing `Cb / (1 - Cs)`, which would be a division by zero.
    fn composite_tile_cpu_color_dodge_with_a_saturated_source_yields_one_when_backdrop_is_nonzero()
    {
        let bottom = solid_texels([0.3, 0.6, 0.9, 1.0]);
        let top = solid_texels([1.0, 1.0, 1.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorDodge),
        ]);
        let (r, g, b, a) = first_texel(&out);
        assert!(r.is_finite() && g.is_finite() && b.is_finite());
        assert_eq!((r, g, b, a), (1.0, 1.0, 1.0, 1.0));
    }

    #[test]
    // ColorBurn in-range case: 1 - min(1, (1 - Cb) / Cs). Backdrop 0.875,
    // source 0.5 -> 1 - min(1, 0.125 / 0.5) = 1 - 0.25 = 0.75. 0.875,
    // 0.125, and 0.25 (like 0.25/0.5, unlike 0.6/0.4) are exact
    // eighths/quarters, so they round-trip bit-exact through `f16` --
    // the same discipline this file's own Normal-mode test documents for
    // its own literal choice. The result (0.75) is deliberately distinct
    // from both inputs and from what a Normal blend of the same two
    // layers would produce (0.5), so it can't be mistaken for either.
    fn composite_tile_cpu_color_burn_computes_the_clamped_per_channel_ratio() {
        let bottom = solid_texels([0.875, 0.875, 0.875, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorBurn),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // ColorBurn's own `Cb == 1` branch, required to fire (and yield 1,
    // not attempt a division) regardless of `Cs` -- the mirror image of
    // the ColorDodge zero-backdrop test above. R is a plain in-range
    // source (0.3), B is `Cs == 1`, and G is `Cs == 0` -- the one input
    // where *both* of `blend_channel`'s `ColorBurn` conditions (`Cb == 1`
    // and `Cs == 0`) are true at once (a fully white backdrop burned by a
    // fully black source, a perfectly ordinary real pixel, not a
    // hypothetical). The W3C order (check the backdrop's own `Cb == 1`
    // extreme first, exactly as `blend_channel` does) resolves it to 1,
    // matching this assertion; checking `Cs == 0` first instead would
    // wrongly yield 0 for that channel -- this test is what would catch
    // that ordering bug.
    fn composite_tile_cpu_color_burn_with_a_saturated_backdrop_yields_one_regardless_of_source() {
        let bottom = solid_texels([1.0, 1.0, 1.0, 1.0]);
        let top = solid_texels([0.3, 0.0, 1.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorBurn),
        ]);
        let (r, g, b, a) = first_texel(&out);
        assert!(r.is_finite() && g.is_finite() && b.is_finite());
        assert_eq!((r, g, b, a), (1.0, 1.0, 1.0, 1.0));
    }

    #[test]
    // ColorBurn's own `Cs == 0` branch (backdrop not saturated, so the
    // `Cb == 1` branch above does not fire first): must yield 0 without
    // ever computing `(1 - Cb) / Cs`, which would be a division by zero.
    fn composite_tile_cpu_color_burn_with_a_zero_source_yields_zero_when_backdrop_is_not_saturated()
    {
        let bottom = solid_texels([0.2, 0.5, 0.9, 1.0]);
        let top = solid_texels([0.0, 0.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorBurn),
        ]);
        let (r, g, b, a) = first_texel(&out);
        assert!(r.is_finite() && g.is_finite() && b.is_finite());
        assert_eq!((r, g, b, a), (0.0, 0.0, 0.0, 1.0));
    }

    #[test]
    // LinearDodge: min(Cb + Cs, 1.0) -- plain addition and a clamp, no
    // division so no 0/1 special-casing is needed. All literals are
    // exact quarters/eighths (unlike 0.4/0.7), so they round-trip
    // bit-exact through `f16`. R: 0.25 + 0.5 = 0.75, under the clamp.
    // G: 0.75 + 0.5 = 1.25, clamped down to 1.0. B: 1.0 + 1.0 = 2.0,
    // clamped down to 1.0 -- proving the clamp holds even well past the
    // boundary, not just just-over-1.
    fn composite_tile_cpu_linear_dodge_adds_and_clamps_to_one() {
        let bottom = solid_texels([0.25, 0.75, 1.0, 1.0]);
        let top = solid_texels([0.5, 0.5, 1.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::LinearDodge),
        ]);
        assert_eq!(first_texel(&out), (0.75, 1.0, 1.0, 1.0));
    }

    #[test]
    // LinearBurn: max(Cb + Cs - 1.0, 0.0) -- plain subtraction and a
    // clamp, no division so no 0/1 special-casing is needed. All
    // literals are exact quarters (unlike 0.4/0.7), so they round-trip
    // bit-exact through `f16`. R: 0.25 + 0.5 - 1.0 = -0.25, clamped up
    // to 0.0. G: 0.75 + 0.5 - 1.0 = 0.25, under the clamp (a genuine
    // non-zero result, for contrast against R and B). B:
    // 0.0 + 0.0 - 1.0 = -1.0, clamped up to 0.0 -- proving the clamp
    // holds even well past the boundary, not just just-under-0.
    fn composite_tile_cpu_linear_burn_subtracts_and_clamps_to_zero() {
        let bottom = solid_texels([0.25, 0.75, 0.0, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::LinearBurn),
        ]);
        assert_eq!(first_texel(&out), (0.0, 0.25, 0.0, 1.0));
    }

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
