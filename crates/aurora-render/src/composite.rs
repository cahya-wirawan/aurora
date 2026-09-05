//! GPU-side tile compositing: blends a source tile over a destination
//! tile using the GPU's fixed-function alpha blend unit, replacing the
//! CPU per-pixel merge `spike/FINDINGS.md` finding #1 measured at ~20ms
//! and named as the actual compositing bottleneck (not disk I/O, which
//! the same spike found fast). PLAN.md M1.3.

use aurora_gpu::{Blend, GpuContext, PipelineCache, PipelineKey};
use aurora_tile::{CHANNELS, SAMPLES};
use half::f16;
use half::slice::HalfFloatSliceExt;

const COMPOSITE_SHADER: &str = include_str!("shaders/composite.wgsl");
const LABEL: &str = "composite";
/// The bind group layout that carries the backdrop texture at binding 3
/// ([`blend_bind_group_layout`]). Deliberately *not* the bare [`LABEL`]
/// the two older layouts use: three same-shaped layouts all labelled
/// `"composite"` leave a `wgpu` validation message or a frame
/// capture unable to say which one is at fault, and there will be more
/// of them as the remaining blend modes land.
const LABEL_BLEND_LAYOUT: &str = "composite.blend.layout";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_multiply_over_with_opacity`], for the
/// same reason as [`LABEL_BLEND_LAYOUT`].
const LABEL_MULTIPLY: &str = "composite.multiply";
/// That method's own per-call uniform buffer.
const LABEL_MULTIPLY_UNIFORM: &str = "composite.multiply.opacity";
/// That method's own per-call bind group.
const LABEL_MULTIPLY_BIND_GROUP: &str = "composite.multiply.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_MULTIPLY_PASS: &str = "composite.multiply.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_darken_over_with_opacity`] — the same
/// four-label set `Multiply` above carries, for the same reason: two
/// blend-math pipelines sharing one `"composite"` label would leave a
/// `wgpu` validation message or a frame capture unable to say which
/// blend mode is at fault.
const LABEL_DARKEN: &str = "composite.darken";
/// That method's own per-call uniform buffer.
const LABEL_DARKEN_UNIFORM: &str = "composite.darken.opacity";
/// That method's own per-call bind group.
const LABEL_DARKEN_BIND_GROUP: &str = "composite.darken.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_DARKEN_PASS: &str = "composite.darken.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_lighten_over_with_opacity`] (0.95.0) —
/// the same four-label set `Multiply` and `Darken` above each carry, for
/// the same reason: several blend-math pipelines sharing one
/// `"composite"` label would leave a `wgpu` validation message or a frame
/// capture unable to say which blend mode is at fault.
const LABEL_LIGHTEN: &str = "composite.lighten";
/// That method's own per-call uniform buffer.
const LABEL_LIGHTEN_UNIFORM: &str = "composite.lighten.opacity";
/// That method's own per-call bind group.
const LABEL_LIGHTEN_BIND_GROUP: &str = "composite.lighten.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_LIGHTEN_PASS: &str = "composite.lighten.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_screen_over_with_opacity`] (0.102.0) —
/// the same four-label set `Multiply`, `Darken` and `Lighten` above each
/// carry, for the same reason: several blend-math pipelines sharing one
/// `"composite"` label would leave a `wgpu` validation message or a frame
/// capture unable to say which blend mode is at fault.
const LABEL_SCREEN: &str = "composite.screen";
/// That method's own per-call uniform buffer.
const LABEL_SCREEN_UNIFORM: &str = "composite.screen.opacity";
/// That method's own per-call bind group.
const LABEL_SCREEN_BIND_GROUP: &str = "composite.screen.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_SCREEN_PASS: &str = "composite.screen.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_difference_over_with_opacity`] (0.104.0) —
/// the same four-label set `Multiply`, `Darken`, `Lighten` and `Screen`
/// above each carry, for the same reason: five blend-math pipelines
/// sharing one `"composite"` label would leave a `wgpu` validation message
/// or a frame capture unable to say which blend mode is at fault.
const LABEL_DIFFERENCE: &str = "composite.difference";
/// That method's own per-call uniform buffer.
const LABEL_DIFFERENCE_UNIFORM: &str = "composite.difference.opacity";
/// That method's own per-call bind group.
const LABEL_DIFFERENCE_BIND_GROUP: &str = "composite.difference.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_DIFFERENCE_PASS: &str = "composite.difference.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_linear_dodge_over_with_opacity`] (0.105.0)
/// — the same four-label set `Multiply`, `Darken`, `Lighten`, `Screen`
/// and `Difference` above each carry, for the same reason: six blend-math
/// pipelines sharing one `"composite"` label would leave a `wgpu`
/// validation message or a frame capture unable to say which blend mode is
/// at fault.
const LABEL_LINEAR_DODGE: &str = "composite.linear_dodge";
/// That method's own per-call uniform buffer.
const LABEL_LINEAR_DODGE_UNIFORM: &str = "composite.linear_dodge.opacity";
/// That method's own per-call bind group.
const LABEL_LINEAR_DODGE_BIND_GROUP: &str = "composite.linear_dodge.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_LINEAR_DODGE_PASS: &str = "composite.linear_dodge.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_linear_burn_over_with_opacity`] (0.106.0)
/// — the same four-label set `Multiply`, `Darken`, `Lighten`, `Screen`,
/// `Difference` and `LinearDodge` above each carry, for the same reason:
/// eight blend-math pipelines sharing one `"composite"` label would leave
/// a `wgpu` validation message or a frame capture unable to say which
/// blend mode is at fault. `"linear_burn"` against `"linear_dodge"` is
/// deliberately spelled out in full rather than abbreviated — the two
/// modes are mirror images and a capture naming only `"linear"` would be
/// no help at all.
const LABEL_LINEAR_BURN: &str = "composite.linear_burn";
/// That method's own per-call uniform buffer.
const LABEL_LINEAR_BURN_UNIFORM: &str = "composite.linear_burn.opacity";
/// That method's own per-call bind group.
const LABEL_LINEAR_BURN_BIND_GROUP: &str = "composite.linear_burn.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_LINEAR_BURN_PASS: &str = "composite.linear_burn.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_color_burn_over_with_opacity`] (0.107.0)
/// — the same four-label set the seven modes above each carry, for the
/// same reason: eight blend-math pipelines sharing one `"composite"`
/// label would leave a `wgpu` validation message or a frame capture
/// unable to say which blend mode is at fault. `"color_burn"` against
/// `"linear_burn"` is spelled out in full for the reason
/// `"linear_burn"` against `"linear_dodge"` already is — the two are the
/// burn family's two members, they are adjacent dispatch arms in
/// `aurora-app`, and a capture naming only `"burn"` would be no help at
/// all.
const LABEL_COLOR_BURN: &str = "composite.color_burn";
/// That method's own per-call uniform buffer.
const LABEL_COLOR_BURN_UNIFORM: &str = "composite.color_burn.opacity";
/// That method's own per-call bind group.
const LABEL_COLOR_BURN_BIND_GROUP: &str = "composite.color_burn.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_COLOR_BURN_PASS: &str = "composite.color_burn.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_color_dodge_over_with_opacity`] (0.108.0)
/// — the same four-label set the eight modes above each carry, for the
/// same reason: nine blend-math pipelines sharing one `"composite"` label
/// would leave a `wgpu` validation message or a frame capture unable to
/// say which blend mode is at fault. `"color_dodge"` against
/// `"color_burn"` is spelled out in full for the reason `"linear_burn"`
/// against `"linear_dodge"` already is, and here the argument is stronger
/// than a shared word: the two are the *guarded-division* pair, their
/// `aurora-app` dispatch arms are adjacent, and a capture naming only
/// `"color"` would point at both.
const LABEL_COLOR_DODGE: &str = "composite.color_dodge";
/// That method's own per-call uniform buffer.
const LABEL_COLOR_DODGE_UNIFORM: &str = "composite.color_dodge.opacity";
/// That method's own per-call bind group.
const LABEL_COLOR_DODGE_BIND_GROUP: &str = "composite.color_dodge.bind_group";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_COLOR_DODGE_PASS: &str = "composite.color_dodge.pass";

/// Everything that differs between one shader-computed blend mode's
/// composite pass and another's: the `shaders/composite.wgsl` fragment
/// entry point, and the four `wgpu` debug labels that name its pipeline,
/// uniform buffer, bind group and render pass.
///
/// This is the whole variation between every one of
/// [`TileCompositor`]'s blend-math `composite_*_over_with_opacity`
/// methods — `composite_multiply_over_with_opacity` and its
/// `darken`/`lighten`/`screen`/`difference`/`linear_dodge`/`linear_burn`/
/// `color_burn`/`color_dodge`
/// siblings,
/// deliberately named as a family rather than relisted here, since the
/// list grows by one every time a mode is ported — five
/// `&'static str`s. Everything else those methods do is
/// `composite_blend_over_with_opacity`, which they all
/// delegate to; see that method for why the collapse was safe to
/// make at two modes rather than deferred to a third. (`Lighten`, the
/// third, landed in 0.95.0 as one `BlendPass` const and one wrapper,
/// `Screen`, the fourth, in 0.102.0 as exactly the same two additions,
/// `Difference`, the fifth, in 0.104.0 likewise, `LinearDodge`, the
/// sixth, in 0.105.0 likewise again, `LinearBurn`, the seventh, in
/// 0.106.0, `ColorBurn`, the eighth, in 0.107.0, and `ColorDodge`, the
/// ninth, in 0.108.0 — which is what the
/// collapse was
/// betting on. `ColorBurn` is the first whose *shader* needed more than
/// one changed line, its formula being three calls to a helper rather
/// than one componentwise expression; the Rust side was still exactly one
/// `BlendPass` const, one wrapper and four labels. `ColorDodge` is the
/// second of that shape, and cost the Rust side exactly the same.)
///
/// There is deliberately **no encoder label here any more** (0.86.0).
/// These methods no longer create a `wgpu::CommandEncoder` at all — the
/// caller supplies one and they only record into it — so the encoder's
/// label now belongs to whoever opened it (`aurora-app`'s
/// `begin_gpu_composite_tile` names its single per-tile encoder), not to
/// one pass recorded inside it. `LABEL_MULTIPLY_ENCODER` and
/// `LABEL_DARKEN_ENCODER` were deleted with the encoders they named.
///
/// Carrying `fragment_entry` here rather than as a parameter is still
/// deliberate: it keeps the shared method's argument list one shorter
/// than it would otherwise be. That no longer stays under
/// `clippy::too_many_arguments`'s own seven-argument limit — the
/// caller-supplied encoder is an eighth slot, so the method now carries
/// an explicit `allow` — but a mode is still a `const` here rather than
/// yet another argument at every call site.
struct BlendPass {
    /// The `shaders/composite.wgsl` `@fragment` entry point that
    /// computes this mode's formula. It is also the discriminating
    /// field of the [`PipelineKey`] this pass caches under, so two modes
    /// must never share one — see [`composite_pipeline`]'s own note on
    /// `bind_group_layout` not being part of the key.
    fragment_entry: &'static str,
    /// Pipeline layout and render pipeline.
    pipeline: &'static str,
    /// The per-call opacity uniform buffer.
    uniform: &'static str,
    /// The per-call bind group.
    bind_group: &'static str,
    /// The render pass itself.
    pass: &'static str,
}

/// [`BlendMode::Multiply`]'s entry point and labels — field for field
/// what `composite_multiply_over_with_opacity` passed inline before the
/// two bodies were merged.
const BLEND_PASS_MULTIPLY: BlendPass = BlendPass {
    fragment_entry: "fs_composite_multiply",
    pipeline: LABEL_MULTIPLY,
    uniform: LABEL_MULTIPLY_UNIFORM,
    bind_group: LABEL_MULTIPLY_BIND_GROUP,
    pass: LABEL_MULTIPLY_PASS,
};

/// [`BlendMode::Darken`]'s, likewise.
const BLEND_PASS_DARKEN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_darken",
    pipeline: LABEL_DARKEN,
    uniform: LABEL_DARKEN_UNIFORM,
    bind_group: LABEL_DARKEN_BIND_GROUP,
    pass: LABEL_DARKEN_PASS,
};

/// [`BlendMode::Lighten`]'s, likewise (0.95.0).
const BLEND_PASS_LIGHTEN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_lighten",
    pipeline: LABEL_LIGHTEN,
    uniform: LABEL_LIGHTEN_UNIFORM,
    bind_group: LABEL_LIGHTEN_BIND_GROUP,
    pass: LABEL_LIGHTEN_PASS,
};

/// [`BlendMode::Screen`]'s, likewise (0.102.0).
const BLEND_PASS_SCREEN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_screen",
    pipeline: LABEL_SCREEN,
    uniform: LABEL_SCREEN_UNIFORM,
    bind_group: LABEL_SCREEN_BIND_GROUP,
    pass: LABEL_SCREEN_PASS,
};

/// [`BlendMode::Difference`]'s, likewise (0.104.0).
const BLEND_PASS_DIFFERENCE: BlendPass = BlendPass {
    fragment_entry: "fs_composite_difference",
    pipeline: LABEL_DIFFERENCE,
    uniform: LABEL_DIFFERENCE_UNIFORM,
    bind_group: LABEL_DIFFERENCE_BIND_GROUP,
    pass: LABEL_DIFFERENCE_PASS,
};

/// [`BlendMode::LinearDodge`]'s, likewise (0.105.0).
const BLEND_PASS_LINEAR_DODGE: BlendPass = BlendPass {
    fragment_entry: "fs_composite_linear_dodge",
    pipeline: LABEL_LINEAR_DODGE,
    uniform: LABEL_LINEAR_DODGE_UNIFORM,
    bind_group: LABEL_LINEAR_DODGE_BIND_GROUP,
    pass: LABEL_LINEAR_DODGE_PASS,
};

/// [`BlendMode::LinearBurn`]'s, likewise (0.106.0). Every field differs
/// from [`BLEND_PASS_LINEAR_DODGE`]'s directly above, `fragment_entry`
/// included — the two modes are mirror images, so a `fragment_entry` copied
/// rather than written would silently run the *other* formula through this
/// mode's labels. That is mutation (f) of this round's set, and it is
/// killed by every `composite_linear_burn_*` test.
const BLEND_PASS_LINEAR_BURN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_linear_burn",
    pipeline: LABEL_LINEAR_BURN,
    uniform: LABEL_LINEAR_BURN_UNIFORM,
    bind_group: LABEL_LINEAR_BURN_BIND_GROUP,
    pass: LABEL_LINEAR_BURN_PASS,
};

/// [`BlendMode::ColorBurn`]'s, likewise (0.107.0). Every field differs
/// from [`BLEND_PASS_LINEAR_BURN`]'s directly above — the two are the
/// burn family's two members and their `aurora-app` dispatch arms are
/// adjacent, so a `fragment_entry` copied rather than written would
/// silently run the *other* burn's formula through this mode's labels.
/// That is mutation (k) of this round's set, and it is killed by every
/// `composite_color_burn_*` test.
const BLEND_PASS_COLOR_BURN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_color_burn",
    pipeline: LABEL_COLOR_BURN,
    uniform: LABEL_COLOR_BURN_UNIFORM,
    bind_group: LABEL_COLOR_BURN_BIND_GROUP,
    pass: LABEL_COLOR_BURN_PASS,
};

/// [`BlendMode::ColorDodge`]'s, likewise (0.108.0). Every field differs
/// from [`BLEND_PASS_COLOR_BURN`]'s directly above — the two are the
/// guarded-division pair, structural mirror images of each other, and
/// their `aurora-app` dispatch arms are adjacent, so a `fragment_entry`
/// copied rather than written would silently run the *other* guarded
/// division through this mode's labels. That is mutation (i) of this
/// round's set, and it is killed by every `composite_color_dodge_*` test.
const BLEND_PASS_COLOR_DODGE: BlendPass = BlendPass {
    fragment_entry: "fs_composite_color_dodge",
    pipeline: LABEL_COLOR_DODGE,
    uniform: LABEL_COLOR_DODGE_UNIFORM,
    bind_group: LABEL_COLOR_DODGE_BIND_GROUP,
    pass: LABEL_COLOR_DODGE_PASS,
};

/// The byte size of `composite_over_with_opacity`'s own uniform buffer —
/// a real `f32` opacity value plus 12 bytes of padding, matching
/// `shaders/composite.wgsl`'s own `Opacity` struct exactly.
const OPACITY_UNIFORM_SIZE: u64 = 16;

/// Texels converted per vectorized chunk in [`composite_layer_into`] —
/// the same 64 `aurora_gpu::residency`'s own serializer chose in 0.92.0
/// and for the same reason: 64 texels is [`CHUNK_SAMPLES`] = 256
/// samples, so each of that function's two `f32` scratch arrays is 1 KiB
/// and stays in L1, while [`SAMPLES`] `/ CHUNK_SAMPLES` is exactly 1,024
/// — a real whole-tile fold never reaches the scalar remainder at all.
/// Pinned by `the_chunk_constants_divide_a_whole_tile_evenly` rather
/// than left implied.
///
/// Declared here rather than shared with `aurora_gpu::residency` because
/// that crate's copy is private, and reaching into another crate's
/// internals for a tuning constant would be worse than two documented
/// declarations of the same number. (`aurora-render` does depend on
/// `aurora-gpu`, so the *dependency* would be legal under PRD §7.2 —
/// what makes it wrong is coupling this loop's cache tuning to an
/// unrelated module's, not the layering.)
const CHUNK_TEXELS: usize = 64;
/// [`CHUNK_TEXELS`] texels' worth of `f16` samples — the length of both
/// `f32` scratch arrays in [`composite_layer_into`], and the granularity
/// its two input slices are split at.
const CHUNK_SAMPLES: usize = CHUNK_TEXELS * CHANNELS;

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
/// time, falling the one not-yet-implemented mode (`Dissolve` — this
/// family's own explicit remainder, see [`composite_tile_cpu`]'s own
/// doc comment) back to [`Self::Normal`] as an honest, documented
/// degrade — not a bug, since `Dissolve`'s real math (stochastic
/// per-pixel selection, needing its own reproducibility design
/// decision) is separate, still-open follow-on work.
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
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    // -- Non-separable modes: each is a function of the
    // whole (R,G,B) triple, not a per-channel function of one channel in
    // isolation -- see `blend_rgb` below, not `blend_channel`, for their
    // real math.
    Hue,
    Saturation,
    Color,
    Luminosity,
    // -- DarkerColor/LighterColor (this round): also whole-colour,
    // non-separable modes, but a different shape from the 4 above --
    // those blend via SetLum/SetSat (a real mixed result); these two
    // instead *select* one whole input colour outright, by comparing
    // `lum(Cb)` against `lum(Cs)`, and return that colour's exact
    // triple unchanged -- never a per-channel hybrid the way separable
    // `Darken`/`Lighten` above (`min`/`max` of each channel
    // independently) can produce. See `blend_rgb` below.
    DarkerColor,
    LighterColor,
}

/// `SoftLight`'s own `D(x)` helper (W3C Compositing and Blending Level 1),
/// nested two-branch logic distinct from `SoftLight`'s own `Cs <= 0.5`
/// branch: a polynomial below `x = 0.25`, `sqrt(x)` at and above it.
/// Continuous at that boundary — confirmed by hand, not just asserted: at
/// `x = 0.25`, the polynomial gives
/// `((16*0.25-12)*0.25+4)*0.25 = ((4-12)*0.25+4)*0.25 = (-2+4)*0.25 = 0.5`,
/// and `sqrt(0.25) = 0.5` — the two branches agree exactly at the
/// boundary, so `SoftLight` itself has no discontinuity there (also
/// exercised through the real per-texel path, not just this helper in
/// isolation, by
/// `composite_tile_cpu_soft_light_has_no_discontinuity_across_the_d_helpers_own_branch_boundary`
/// below).
#[must_use]
fn soft_light_d(x: f32) -> f32 {
    if x <= 0.25 {
        ((16.0 * x - 12.0) * x + 4.0) * x
    } else {
        x.sqrt()
    }
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
// `clippy::match_same_arms` wants the `Hue`/`Saturation`/`Color`/
// `Luminosity` arm merged into the literal `Normal` arm above (both
// return `cs`) -- rejected deliberately, the same reasoning
// `aurora-app`'s own `translate_blend_mode` already documents for its
// own identical-bodied arms: collapsing them would blur "this is
// Normal's own real mapping" from "this mode has no per-channel mapping
// at all, and this arm exists purely for exhaustiveness", even though
// both currently produce the same value.
#[allow(clippy::match_same_arms)]
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
        // HardLight (W3C spec): branches on the *source*. Composed from
        // the two families already implemented above rather than
        // re-derived: `Cs <= 0.5` is `Multiply(Cb, 2*Cs)`, else
        // `Screen(Cb, 2*Cs-1)`.
        BlendMode::HardLight => {
            if cs <= 0.5 {
                blend_channel(BlendMode::Multiply, cb, 2.0 * cs)
            } else {
                blend_channel(BlendMode::Screen, cb, 2.0 * cs - 1.0)
            }
        }
        // Overlay (W3C spec): `Overlay(Cb, Cs) = HardLight(Cs, Cb)` --
        // the exact same two formulas as HardLight above, just branching
        // on the backdrop instead of the source. Expressed literally as
        // that relationship (swap the two channel arguments into
        // HardLight's own arm above) rather than as an independently
        // re-derived pair of branches, so the "same shape, one flipped"
        // relationship is visible in the code, not just in a comment.
        BlendMode::Overlay => blend_channel(BlendMode::HardLight, cs, cb),
        // VividLight: branches on the source, reusing ColorBurn/
        // ColorDodge's own already-implemented 0/1 edge-case handling
        // rather than re-deriving it. `2*Cs` (Cs<=0.5, range [0,1]) and
        // `2*Cs-1` (Cs>0.5, range (0,1]) are both valid inputs to those
        // arms as-is.
        BlendMode::VividLight => {
            if cs <= 0.5 {
                blend_channel(BlendMode::ColorBurn, cb, 2.0 * cs)
            } else {
                blend_channel(BlendMode::ColorDodge, cb, 2.0 * cs - 1.0)
            }
        }
        // LinearLight: the branch form is `Cs <= 0.5 -> LinearBurn(Cb,
        // 2*Cs) = max(Cb+2*Cs-1, 0)`, else `LinearDodge(Cb, 2*Cs-1) =
        // min(Cb+2*Cs-1, 1)`. Using the algebraically equivalent
        // single-expression simplification instead (see this crate's own
        // tests for a numeric proof against the branch form): in the
        // `Cs<=0.5` branch, `2*Cs` is in `[0,1]` so `Cb+2*Cs-1` tops out
        // at `Cb <= 1` -- the `min(...,1)` clamp the other branch applies
        // is never actually reachable here, only the `max(...,0)` one is.
        // Symmetrically, in the `Cs>0.5` branch, `2*Cs-1` is in `(0,1]`
        // so `Cb+2*Cs-1` never goes below `0` -- only the `min(...,1)`
        // clamp is reachable. A plain `clamp(Cb+2*Cs-1, 0, 1)` applies
        // both bounds unconditionally, but since each branch only ever
        // needs the one bound it would have applied anyway, the two
        // forms agree on every input in `[0,1]^2`.
        BlendMode::LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
        // PinLight: branches on the source, reusing Darken/Lighten's own
        // already-implemented per-channel min/max rather than
        // re-deriving it.
        BlendMode::PinLight => {
            if cs <= 0.5 {
                blend_channel(BlendMode::Darken, cb, 2.0 * cs)
            } else {
                blend_channel(BlendMode::Lighten, cb, 2.0 * cs - 1.0)
            }
        }
        // HardMix: a hard threshold on VividLight's own result, reusing
        // that arm directly rather than re-deriving its branch logic.
        BlendMode::HardMix => {
            if blend_channel(BlendMode::VividLight, cb, cs) < 0.5 {
                0.0
            } else {
                1.0
            }
        }
        // SoftLight (W3C spec): the most mathematically distinct mode in
        // this family -- no reuse of another arm above, real cross-term
        // math via its own `soft_light_d` helper.
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                cb + (2.0 * cs - 1.0) * (soft_light_d(cb) - cb)
            }
        }
        // `Hue`/`Saturation`/`Color`/`Luminosity`/`DarkerColor`/
        // `LighterColor` are all non-separable -- no per-channel value
        // of `B(Cb,Cs)` can express a property of a whole colour (see
        // this function's own module-level doc comment, and
        // `blend_rgb`'s own doc comment, for why). Named individually
        // rather than folded into a wildcard so the match stays
        // exhaustive (the same discipline `aurora-app`'s own
        // `translate_blend_mode` already uses), but
        // [`blend_rgb`] intercepts all 6 before ever reaching this
        // function, so these arms are never actually exercised by any
        // real caller in this crate -- they exist purely so this match
        // still compiles against the shared, now-26-variant
        // [`BlendMode`] enum. Each degrades to the same pass-through
        // `cs` the `Normal` arm above already returns, rather than
        // introducing a new path that could panic if some future
        // caller ever did reach this function directly with one of
        // these 6 modes -- the same "honest fallback over panicking"
        // discipline `aurora-app`'s own `translate_blend_mode` already
        // uses at its own translation boundary for these same 6 modes.
        BlendMode::Hue
        | BlendMode::Saturation
        | BlendMode::Color
        | BlendMode::Luminosity
        | BlendMode::DarkerColor
        | BlendMode::LighterColor => cs,
    }
}

/// `Lum(C)`, the W3C Compositing and Blending Level 1 spec's own
/// luminance weighting for exactly this family of blend modes:
/// `0.3*r + 0.59*g + 0.11*b` — NTSC-luma-style weights, **not**
/// `aurora_color`'s WCAG relative-luminance weights (`0.2126`/`0.7152`/
/// `0.0722`) already used elsewhere in this codebase for contrast
/// checking. A different, unrelated weighting the spec defines
/// specifically for `Hue`/`Saturation`/`Color`/`Luminosity` below — not
/// interchangeable with, and not to be confused with, that other one.
#[must_use]
// Single-letter names (`c`, `r`/`g`/`b`) throughout this helper and its
// siblings below (`sat`, `clip_color`, `set_lum`, `set_sat`) are the
// W3C spec's own literal variable names -- kept as-is so the code reads
// side-by-side against the spec text quoted in each doc comment, rather
// than renamed to something clippy would consider more descriptive but
// that no longer lines up with the source of truth.
#[allow(clippy::many_single_char_names)]
fn lum(c: [f32; 3]) -> f32 {
    let [r, g, b] = c;
    0.3 * r + 0.59 * g + 0.11 * b
}

/// `Sat(C) = max(r,g,b) - min(r,g,b)`, the W3C spec's own saturation
/// measure for this family of blend modes.
#[must_use]
fn sat(c: [f32; 3]) -> f32 {
    let [r, g, b] = c;
    r.max(g).max(b) - r.min(g).min(b)
}

/// `ClipColor(C)`, the W3C spec's own gamut-remapping step: `SetLum`
/// shifts every channel by the same additive delta to hit a target
/// luminance, which can push a channel outside `0.0..=1.0`; this pulls
/// it back in while preserving that luminance exactly. Two independent
/// clip conditions, in the spec's own literal order — `n < 0` (a
/// channel went negative) applied first, then `x > 1` (a channel
/// overshot `1.0`) applied to *that* branch's own result, not to the
/// original `c` — both may fire on the same input (see `blend_color`'s
/// own worked example in this module's tests, where only the `x > 1`
/// branch fires, and the module-level doc comment on this file's own
/// non-separable-mode tests for one where neither does). `l`, `n`, and
/// `x` are each computed once from the original input before either
/// branch runs; only the three channel values themselves carry forward
/// from the first branch into the second.
///
/// **Both divisions are guarded against a zero denominator** (0.87.1),
/// the same way [`set_sat`]'s own `max > min` check below already is,
/// and for the same underlying reason. [`lum`]'s weights sum to very
/// nearly `1.0` in `f32` (`0.3 + 0.59 + 0.11` is not bit-exact), so
/// `n <= l <= x` always holds to within rounding — equal on *both*
/// sides, in exact arithmetic, precisely when every channel is equal.
/// An achromatic input therefore makes `l - n` and `x - l` each either
/// exactly `0.0`, or a tiny nonzero rounding residue too small to change
/// the guard's outcome in practice — and where it *is* exactly `0.0`,
/// the numerators `(r - l) * …` are exactly `0.0` too, so the division
/// was `0.0 / 0.0` — NaN — for any achromatic colour with a channel outside
/// `0.0..=1.0`, which is exactly the case that makes a branch fire at
/// all. That was reachable from ordinary content: [`blend_color`] is
/// `SetLum(Cs, Lum(Cb))`, so any achromatic *source* (grey, white,
/// black) over a backdrop whose luminance falls outside `[0,1]` — an
/// unclamped HDR import, which invariant §7.3.1b's `f16` pipeline
/// deliberately permits — produced NaN, as did `Hue`/`Saturation` over
/// any achromatic HDR backdrop for *any* source, since [`set_sat`]
/// collapses an achromatic input to `[0, 0, 0]` whatever `s` is. And
/// NaN does not get absorbed downstream: [`composite_layer_into`]
/// scales the blend result by `alpha`, and `0.0 * NaN` is NaN in
/// IEEE-754, so even a fully transparent layer in one of those modes
/// poisoned the entire composited tile.
///
/// When a denominator is zero the guard clamps each channel into
/// `0.0..=1.0` instead of dividing. That gives up this function's
/// luminance-preserving property, unavoidably: the target luminance is
/// out of gamut and an achromatic colour has no chromatic direction to
/// redistribute along, so the closest in-gamut colour is the clamp. It
/// is unreachable for any input that was already well-defined — the
/// guard fires only where the old code produced NaN — which
/// `clip_color_leaves_an_in_gamut_achromatic_input_exactly_alone` pins
/// directly.
#[must_use]
#[allow(clippy::many_single_char_names)]
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let [r, g, b] = c;
    let n = r.min(g).min(b);
    let x = r.max(g).max(b);
    let clamped = |[r, g, b]: [f32; 3]| [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
    let [r, g, b] = if n < 0.0 {
        let d = l - n;
        if d > 0.0 {
            [
                l + (r - l) * l / d,
                l + (g - l) * l / d,
                l + (b - l) * l / d,
            ]
        } else {
            clamped([r, g, b])
        }
    } else {
        [r, g, b]
    };
    if x > 1.0 {
        let d = x - l;
        if d > 0.0 {
            [
                l + (r - l) * (1.0 - l) / d,
                l + (g - l) * (1.0 - l) / d,
                l + (b - l) * (1.0 - l) / d,
            ]
        } else {
            clamped([r, g, b])
        }
    } else {
        [r, g, b]
    }
}

/// `SetLum(C, l)`: shifts `C` by the same additive delta on every
/// channel so its own [`lum`] becomes exactly `l`, then [`clip_color`]s
/// the (possibly now out-of-gamut) result back into range.
#[must_use]
#[allow(clippy::many_single_char_names)]
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    let [r, g, b] = c;
    clip_color([r + d, g + d, b + d])
}

/// `SetSat(C, s)`: reassigns `C`'s own [`sat`] to `s` while preserving
/// which channel is largest/smallest. The W3C spec states this as a
/// three-branch, channel-*identifying* algorithm ("find whichever
/// channel currently holds the max/mid/min value, assign each a
/// different expression, reassemble in R/G/B order"); this instead
/// applies one formula, `(v - min) * s / (max - min)`, to every channel
/// `v` directly, whichever R/G/B position it happens to occupy — no
/// explicit identification or reassembly step. These are algebraically
/// the same function: substituting `v = max` gives exactly `s` (the
/// spec's own `Cmax = s`), `v = min` gives exactly `0` (the spec's own
/// `Cmin = 0`), and `v = mid` gives exactly `(Cmid - Cmin) * s /
/// (Cmax - Cmin)` (the spec's own `Cmid` formula) — so running the one
/// formula across all three channels reproduces the spec's three
/// per-role assignments without ever needing to know which channel
/// holds which role. Proved numerically against the spec's own literal
/// three-branch form for several inputs, including one where the
/// max/mid/min channels are not in R/G/B order, by
/// `set_sat_matches_the_specs_explicit_max_mid_min_form` below — the
/// same "simplify, then prove the simplification numerically"
/// discipline `blend_channel`'s own `LinearLight` arm already uses in
/// this file.
///
/// `max == min` (every channel equal — a fully achromatic input, the
/// only case where `Cmax > Cmin` is false) divides by zero in that
/// formula; guarded explicitly to return `(0.0, 0.0, 0.0)`, matching
/// the spec's own `else` branch — there is no "direction" to
/// redistribute saturation into an achromatic colour, so zeroing every
/// channel is the spec's own defined behaviour here, not a bug.
#[must_use]
#[allow(clippy::many_single_char_names)]
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let [r, g, b] = c;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max > min {
        let scale = s / (max - min);
        [(r - min) * scale, (g - min) * scale, (b - min) * scale]
    } else {
        [0.0, 0.0, 0.0]
    }
}

/// The 4 non-separable blend functions, character for character the
/// W3C spec's own definitions in terms of [`set_lum`]/[`set_sat`]/
/// [`lum`]/[`sat`] above. Unlike every [`blend_channel`] arm, each of
/// these needs the whole `(R,G,B)` triple of both backdrop and source
/// at once — `Hue`/`Saturation`/`Color`/`Luminosity` are properties of
/// a whole colour, not of one channel in isolation, so no per-channel
/// formula can express them.
#[must_use]
fn blend_hue(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    set_lum(set_sat(cs, sat(cb)), lum(cb))
}

#[must_use]
fn blend_saturation(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    set_lum(set_sat(cb, sat(cs)), lum(cb))
}

#[must_use]
fn blend_color(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    set_lum(cs, lum(cb))
}

#[must_use]
fn blend_luminosity(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    set_lum(cb, lum(cs))
}

/// `DarkerColor(Cb, Cs) = Lum(Cb) <= Lum(Cs) ? Cb : Cs` — picks
/// whichever *whole* input colour has the lower overall [`lum`] and
/// returns that triple exactly, unmodified. Not part of the W3C
/// Compositing and Blending Level 1 spec (`Hue`/`Saturation`/`Color`/
/// `Luminosity` above are; `DarkerColor`/`LighterColor` are Photoshop-
/// specific extensions) — but the same "whole-colour, not per-channel"
/// shape, so it lives alongside them here rather than in
/// [`blend_channel`].
///
/// **Tie-breaking, a deliberate convention, not spec-mandated (there is
/// no spec for these two modes) and not an accidental artifact of
/// `<=`**: when `Lum(Cb) == Lum(Cs)` exactly, this resolves to `Cb`,
/// the backdrop. Chosen because it's symmetric with
/// [`blend_lighter_color`]'s own tie-break (that one resolves to `Cb`
/// too, via `>=`) — both modes agree on what a tie means: "leave the
/// backdrop in place" — rather than the two modes disagreeing about
/// which input wins an exact tie.
#[must_use]
fn blend_darker_color(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    if lum(cb) <= lum(cs) { cb } else { cs }
}

/// `LighterColor(Cb, Cs) = Lum(Cb) >= Lum(Cs) ? Cb : Cs` — the mirror
/// image of [`blend_darker_color`]: picks whichever whole input colour
/// has the *higher* overall [`lum`]. Same tie-break convention, stated
/// there: `Lum(Cb) == Lum(Cs)` resolves to `Cb`.
#[must_use]
fn blend_lighter_color(cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    if lum(cb) >= lum(cs) { cb } else { cs }
}

/// The whole-triple counterpart of [`blend_channel`]: `B(Cb, Cs)` for
/// `mode`, given the backdrop's and source's own full `(R,G,B)` triples
/// rather than one channel each. Exists because the 4 non-separable HSL
/// modes ([`blend_hue`], [`blend_saturation`], [`blend_color`],
/// [`blend_luminosity`]) and the 2 whole-colour-selection modes
/// ([`blend_darker_color`], [`blend_lighter_color`]) genuinely cannot
/// be expressed as [`blend_channel`]'s own per-channel signature —
/// every one of the 20 separable modes, by contrast, delegates straight
/// back to [`blend_channel`] three times, once per channel, unchanged
/// from before this function existed: [`composite_tile_cpu`] below now
/// calls this once per texel instead of calling [`blend_channel`] three
/// times, but for those 20 modes the actual arithmetic performed is
/// identical, so their own results are bit-for-bit unchanged (see this
/// file's own
/// `composite_tile_cpu_multiply_blends_two_mid_greys_to_a_quarter_grey`-
/// style tests, re-asserted after this refactor with no changes to
/// their expected values).
///
/// Deliberately an exhaustive match, no wildcard arm, for the same
/// reason `aurora-app`'s own `translate_blend_mode` gives for its own
/// exhaustive match: a future [`BlendMode`] addition should force this
/// function to be revisited, not silently fall through some default.
#[must_use]
fn blend_rgb(mode: BlendMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => blend_hue(cb, cs),
        BlendMode::Saturation => blend_saturation(cb, cs),
        BlendMode::Color => blend_color(cb, cs),
        BlendMode::Luminosity => blend_luminosity(cb, cs),
        BlendMode::DarkerColor => blend_darker_color(cb, cs),
        BlendMode::LighterColor => blend_lighter_color(cb, cs),
        BlendMode::Normal
        | BlendMode::Darken
        | BlendMode::Multiply
        | BlendMode::Lighten
        | BlendMode::Screen
        | BlendMode::Difference
        | BlendMode::Exclusion
        | BlendMode::Subtract
        | BlendMode::Divide
        | BlendMode::ColorDodge
        | BlendMode::LinearDodge
        | BlendMode::ColorBurn
        | BlendMode::LinearBurn
        | BlendMode::Overlay
        | BlendMode::SoftLight
        | BlendMode::HardLight
        | BlendMode::VividLight
        | BlendMode::LinearLight
        | BlendMode::PinLight
        | BlendMode::HardMix => {
            let [br, bg, bb] = cb;
            let [sr, sg, sb] = cs;
            [
                blend_channel(mode, br, sr),
                blend_channel(mode, bg, sg),
                blend_channel(mode, bb, sb),
            ]
        }
    }
}

/// One texel of [`composite_layer_into`]'s fold, in `f32` — the single
/// spelling of the blend math. The vectorized chunk loop and the scalar
/// remainder loop both call it, so they cannot drift apart the way two
/// hand-written copies would (the same reason
/// `aurora_gpu::residency`'s `write_texel_le_bytes` exists).
///
/// `opacity` is already clamped by the caller. Every expression below is
/// verbatim what [`composite_layer_into`]'s pre-0.98.0 body computed,
/// with the `to_f32()`/`from_f32()` calls lifted out to its callers —
/// the arithmetic, its operand order, and the `backdrop_alpha > 0.0`
/// branch are unchanged, which is what makes the vectorized path
/// bit-exact with the scalar one it replaces. See
/// [`composite_layer_into`]'s own "Vectorized conversion" section for
/// the full bit-exactness argument and its one disclosed caveat.
#[inline]
fn fold_texel(
    dst: [f32; CHANNELS],
    src: [f32; CHANNELS],
    opacity: f32,
    mode: BlendMode,
) -> [f32; CHANNELS] {
    let [dr, dg, db, da] = dst;
    let [sr, sg, sb, sa] = src;
    let alpha = sa * opacity;
    let inverse = 1.0 - alpha;
    let backdrop_alpha = da;
    let backdrop_inverse = 1.0 - backdrop_alpha;
    // Recover the backdrop's true straight-alpha colour before handing
    // it to `blend_rgb` as `Cb` -- see `composite_layer_into`'s own doc
    // comment for why the raw accumulator state isn't always already
    // straight alpha.
    let straight_backdrop = if backdrop_alpha > 0.0 {
        [
            dr / backdrop_alpha,
            dg / backdrop_alpha,
            db / backdrop_alpha,
        ]
    } else {
        [0.0, 0.0, 0.0]
    };
    let [br, bg, bb] = blend_rgb(mode, straight_backdrop, [sr, sg, sb]);
    let blended_r = backdrop_inverse * sr + backdrop_alpha * br;
    let blended_g = backdrop_inverse * sg + backdrop_alpha * bg;
    let blended_b = backdrop_inverse * sb + backdrop_alpha * bb;
    [
        inverse * dr + alpha * blended_r,
        inverse * dg + alpha * blended_g,
        inverse * db + alpha * blended_b,
        alpha + da * inverse,
    ]
}

/// Folds exactly one layer into `out`, a running accumulator, **in
/// place** — the per-layer step [`composite_tile_cpu`] runs once per
/// layer, exposed as its own primitive so that a caller which resolves
/// its layers one at a time (`aurora-app`'s own `resolve_tile`, walking
/// a real `aurora_doc::LayerTree` group) can drop each layer's buffer
/// before resolving the next, instead of holding one full
/// [`aurora_tile::SAMPLES`]-length buffer per sibling alive at once.
/// `out` is the already-accumulated backdrop (bottom), `texels` is one
/// tile's own full `f16` texel buffer being composited over it, with
/// that layer's own `opacity` and [`BlendMode`].
///
/// Per texel, per RGB channel: `Co = (1-as)*Cb + as*[(1-ab)*Cs +
/// ab*B(Cb,Cs)]`, where `Cb`/`Cs` are the backdrop/source channel,
/// `as = src_a * opacity` (clamped to `0.0..=1.0`), `ab` is the
/// backdrop's own alpha, and `B` is `blend_channel`'s own per-mode
/// function. The alpha channel itself is blend-mode-independent:
/// `result_a = as + dst_a * (1 - as)`, unchanged from before blend
/// modes existed and the same formula [`TileCompositor::composite_over`]'s
/// own GPU blend unit computes (proven by that function's own
/// `composite_over_blends_source_over_destination` test) — this
/// function is that same math run on the CPU, generalized by
/// blend mode, and with an opacity factor the fixed-function blend unit
/// has no way to express.
///
/// No state whatsoever carries from one call to the next except `out`
/// itself: every intermediate above (`alpha`, `inverse`,
/// `backdrop_alpha`, the recovered straight backdrop) is recomputed
/// per-texel from `out`, `texels`, `opacity`, and `mode`. That is what
/// makes folding N layers via N calls identical to one
/// [`composite_tile_cpu`] call over the same N layers in the same
/// order — **by construction**, not by test: no state carries from one
/// *call* to the next, so there is nothing a batch call could carry
/// across layers that N separate calls could not. (Corrected in 0.98.1:
/// this paragraph used to say "the loop below reads no state at all
/// beyond `dst`, `src`, `opacity` and `mode`", which 0.98.0's
/// vectorization made literally false. The loop *does* now carry two
/// call-local `[f32; CHUNK_SAMPLES]` scratch arrays, `dst_wide` and
/// `src_wide`, across chunk iterations. They are declared inside this
/// function, so nothing crosses a call boundary; what makes reusing them
/// across chunks correct is that `convert_to_f32_slice` overwrites
/// *every* lane of its destination on every chunk — a `half`-crate
/// guarantee this code relies on rather than one it establishes locally
/// — so no lane of a previous chunk can survive into the next. A
/// violation would show up as a bit mismatch in
/// `vectorized_fold_is_bit_identical_to_the_scalar_reference`, whose
/// fixture varies per texel *and* per channel across the chunk boundary
/// precisely so a stale lane cannot hide behind uniform data; see
/// `varied_samples_from`'s own comment.) What pins the math itself is
/// `composite_layer_into_folded_matches_hand_computed_golden_values`,
/// which fixes a three-layer fold to values derived by hand from the
/// formula above rather than from any call this module makes.
///
/// `composite_layer_into_folded_one_at_a_time_matches_the_batch_composite`
/// is a consistency smoke test, **not** evidence of either: since
/// [`composite_tile_cpu`] is now *defined* as this fold, both sides of
/// that assertion are literally the same sequence of calls on the same
/// data, and it cannot fail while that definition holds. Its only real
/// job is to fail loudly if some future edit reintroduces a bespoke
/// per-layer loop into [`composite_tile_cpu`] that drifts from this one.
///
/// **Length mismatches are still silently swallowed here — that is not
/// a safety property of this function, it is just no longer reachable.**
/// A `texels` (or `out`) slice whose length isn't a multiple of
/// [`CHANNELS`] has its trailing partial texel dropped (`chunks_exact`),
/// and because the two slices are *zipped*, a length mismatch between
/// them composites only the shorter one's worth of texels, with no error
/// and no way for a caller to notice. Nothing about that changed. What
/// changed (0.52.1) is that no caller in this workspace can produce a
/// mismatched pair any more — not through one unified mechanism, but
/// because every producer of a tile-shaped `&[f16]` in this workspace
/// independently either allocates exactly [`SAMPLES`] or preserves its
/// input's own length. As of 0.52.1 that is, exhaustively:
///
/// * [`transparent_tile`] — `vec![…; SAMPLES]`. Both accumulators
///   (`aurora-app`'s `composite_roots_into_tile` and `resolve_tile`'s
///   own per-group `isolated`) start here, which is what fixes `out`.
/// * `aurora-app`'s `read_layer_window` — allocates
///   `TILE * TILE * CHANNELS` directly and *copies into* it, so a
///   missing or unreadable source tile leaves transparent pixels rather
///   than a short buffer.
/// * `aurora_tile::TileStore::get` → `Tile::texels()` — every `Tile` is
///   `Tile::blank` (`SAMPLES`-long by construction) or
///   `Tile::from_texels` fed only by `aurora_tile::codec::decode`, which
///   since 0.52.1 rejects any decoded length other than exactly one
///   whole tile. That is the one that closed the real, reachable hole: a
///   truncated or corrupted scratch-disk file used to page in as a short
///   slice, and is now a `TileError` at its source.
/// * `aurora-app`'s `dissolve_gate` and `apply_mask` — both allocate
///   `vec![…; texels.len()]`, so they are length-preserving whatever
///   they are handed. (`apply_mask` was called `apply_mask_clip` until
///   0.70.0 gave layer masks real per-pixel coverage; the mask
///   coverage window it now reads is a separate buffer and does not
///   change the length of what it returns.)
/// * `aurora-app`'s `decode_f16_samples` (the GPU readback path) —
///   enforces `out.len() == aurora_tile::SAMPLES` itself and returns
///   `None` otherwise, entirely independently of this crate.
///
/// So the honest statement is "five separate enforcement points, one of
/// which was fixed", not "one invariant guarantees it". Treat this list
/// as a note about *why* the zip is currently harmless — and as
/// something to re-check when a sixth producer appears — not as licence
/// to hand this function slices of differing lengths.
///
/// **Scope**: [`BlendMode`] covers 26 of `aurora_doc::BlendMode`'s
/// real 27 variants. The inventory, the `Dissolve` boundary, and why
/// this is a CPU implementation at all are on [`composite_tile_cpu`]'s
/// own doc comment, which is this family's entry point.
///
/// **The accumulator's own backdrop colour, recovered before blending —
/// not assumed straight-alpha as-is.** `blend_rgb`/`blend_channel`'s
/// own math (`Multiply`, `Screen`, the HSL family, ...) is only correct
/// when the `Cb` (backdrop) colour it receives is a *true straight-alpha*
/// colour. That is automatically true for every layer composited over an
/// already-*opaque* backdrop (`backdrop_alpha == 1.0`) — the case every
/// test in this module exercises, since each seeds an opaque bottom layer
/// first — because a straight colour and its premultiplied form are
/// identical at `alpha = 1.0`. It is **not** true when the accumulator
/// (`out`, here) is itself still translucent partway through a
/// multi-layer accumulation: the running `dr`/`dg`/`db` at that point
/// hold this function's own straight-alpha "over" accumulation, which is
/// a *premultiplied* colour whenever the accumulated alpha is fractional
/// (e.g. a lone 50%-opacity layer composited alone onto a starting
/// fully-transparent `out` leaves `dr = 0.5` for a source whose own true
/// colour is `1.0`, while `da` correctly holds `0.5`). A second layer
/// then blending against that raw, still-premultiplied state via a
/// non-`Normal` mode would hand `blend_rgb` the wrong `Cb`. So: before
/// calling `blend_rgb`, the current backdrop colour is divided by
/// `backdrop_alpha` to recover its true straight colour (guarded against
/// `backdrop_alpha == 0.0`, where there is no meaningful colour to
/// recover — `[0.0, 0.0, 0.0]` is used instead, the same guard shape
/// `aurora-app`'s own `resolve_tile` already uses for its own, later,
/// un-premultiply step). This changes nothing about the alpha
/// accumulation or the final blended-RGB accumulation formulas below,
/// both already correct — only `blend_rgb`'s own input. See
/// `composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator`
/// for a worked example, and `aurora-app`'s own `resolve_tile` doc
/// comment for how this closes the gap that function's group-isolation
/// path used to leave open.
///
/// # Vectorized conversion (0.98.0)
///
/// The blend math itself is unchanged and still scalar; what changed is
/// how `f16` samples reach and leave it. The pre-0.98.0 body called
/// `f16::to_f32` once per *use* and `f16::from_f32` once per written
/// channel, so a single texel paid on the order of nineteen scalar
/// `vcvtph2ps`/`vcvtps2ph` instructions on the `backdrop_alpha > 0.0`
/// arm (sixteen on the transparent arm) — eight distinct inputs, several
/// read more than once, plus four narrowing writes. Those are now done a
/// chunk at a time through [`half::slice::HalfFloatSliceExt`]'s
/// `convert_to_f32_slice`/`convert_from_f32_slice`, which reach the same
/// F16C instructions eight lanes at a time behind one feature-detection
/// check per call rather than per sample. This is the exact precedent
/// `aurora_gpu::residency`'s own serializer set in 0.92.0 (see
/// `serialize_premultiplied_le_bytes`), applied to the other CPU-side
/// `f16` hot loop in the workspace.
///
/// **This is not progress against the 60 FPS gate.** It shrinks a
/// constant factor on the CPU compositing *fallback* path; it does not
/// change the measured pan-while-painting numbers in CLAUDE.md, and it
/// ~~composes with — rather than replaces — the still-open conditional-
/// parallelization question 0.97.1 left registered~~ **— corrected
/// 0.101.0: it did not compose with that question, it *closed* it.
/// 0.97.1's conditional GO rested on the `fold_onto_opaque` column
/// clearing a pre-registered 2.0 ms-per-call bar; this batching dropped
/// that column ~16–17 %, to 1.71–1.73 ms, so ~~no document shape clears
/// the bar any longer and~~ the `rayon` question for this function is
/// closed NO-GO. See PLAN.md's 0.101.0 entry.**
///
/// **Two scope corrections, 0.101.1 — read them before quoting that
/// NO-GO.** (1) The "no document shape" half is withdrawn: the bar was
/// read off `Normal` and `Multiply` only, and six costlier
/// `fold_onto_opaque` modes (`Hue`, `Saturation`, `Color`, `Luminosity`,
/// `Overlay`, `SoftLight`) still clear 2.0 ms with confidence intervals
/// wholly above it. The NO-GO holds for the two modes it was registered
/// against, which is the right default, but it is not universal.
/// (2) The 2.0 ms bar is a *risk-adjusted contention-survival proxy*, not
/// a measurement that no idle win exists — PLAN.md's 0.101.0 entry
/// discloses this in full, including that the bar's ~8× multiplier was
/// derived from a bandwidth-bound workload and its transfer to this
/// compute-bound kernel is assumed, not measured. Also note that
/// parallelizing *across whole tiles* is not closed: only the framing that
/// shares a `TileStore` across workers is, and PLAN.md's 0.101.1
/// correction names the hoisted-store framing that is not.
///
/// Read the real
/// before/after numbers off PLAN.md's own 0.98.0 entry, not off this
/// comment.
///
/// **Why the two slices are split at one shared offset, not chunked
/// independently.** `aurora_gpu::residency`'s serializer chunks *one*
/// input slice and zips a separately-derived output sink, so it can take
/// that single slice's own `chunks_exact(..).remainder()` for its tail.
/// This function chunks *two* slices that must stay in lockstep, and
/// `out` and `texels` are not guaranteed to be the same length (see the
/// length-mismatch section above — five separate producers, not one
/// invariant). Taking each side's own `remainder()` would start the two
/// tails at *different* offsets whenever the two slices have different
/// whole-chunk counts, silently compositing texel *i* of one slice
/// against texel *j* of the other and processing a different number of
/// texels than the original single `zip` did. So the split point is
/// computed once, from `min(out.len(), texels.len())` rounded down to a
/// whole chunk, and applied to both slices.
/// `mismatched_length_folds_match_the_scalar_reference` pins that
/// against a verbatim copy of the pre-0.98.0 loop.
///
/// **Bit-exactness.** `convert_to_f32_slice`/`convert_from_f32_slice`
/// are the same `half` conversions `to_f32`/`from_f32` perform, batched;
/// the private `fold_texel` above holds the arithmetic verbatim, in the same operand
/// order; and no sample is ever round-tripped `f16` → `f32` → `f16`
/// without being computed, because all four of a processed texel's
/// channels are computed values. (That last point is where the 0.92.0
/// "take alpha from the original chunk, never the round-tripped scratch
/// buffer" rule does *not* transfer: there is no passthrough channel
/// inside a texel here. Its real analogue is the region *outside* the
/// fold — nothing past `min(out.len(), texels.len())` may be read or
/// written, which the shared split point gives for free and
/// `an_over_long_accumulator_keeps_its_tail_bits_untouched` pins.)
/// `vectorized_fold_is_bit_identical_to_the_scalar_reference` asserts
/// `to_bits()` equality against that verbatim scalar copy across every
/// [`BlendMode`], six opacities including two that exercise the clamp,
/// and fixtures carrying both signed zeros, both infinities, both
/// subnormal extremes, and a quiet and a signalling NaN.
///
/// **Carried-forward caveat, not chased here.** 0.92.1 measured that two
/// different auto-vectorizations of the same `f32` arithmetic can pick a
/// different operand's payload when *both* operands of an operation are
/// NaN, and that which one wins is release-profile-dependent. That risk
/// applies to this loop for the same reason and is disclosed rather than
/// closed: the bit-exactness test above is run in both profiles, and any
/// divergence found is expected to be narrowed at the specific assertion
/// with a named reason, exactly as 0.92.1 did — not papered over by
/// weakening the test.
///
/// **Panics: none, by construction.**
///
/// * The `let [dr, dg, db, da] = dst else { continue }` slice patterns
///   are matched against chunks yielded by `chunks_exact(CHANNELS)` /
///   `chunks_exact_mut(CHANNELS)`, which by definition yields slices of
///   exactly [`CHANNELS`] elements — the `else { continue }` arm is
///   unreachable and exists only because the workspace denies
///   `indexing_slicing` and `unwrap`, so a refutable pattern needs a
///   fallback. This is the same shape the pre-0.98.0 loop already used.
/// * `convert_to_f32_slice`/`convert_from_f32_slice`'s only failure mode
///   is a length-mismatch assertion. Both operands of every call below
///   are a `CHUNK_SAMPLES`-length chunk from
///   `chunks_exact(CHUNK_SAMPLES)` and a fixed-size
///   `[f32; CHUNK_SAMPLES]` array, so the lengths are equal by
///   construction and that assertion is unreachable.
/// * `split_at_checked`/`split_at_mut_checked` are used instead of the
///   panicking `split_at`/`split_at_mut`: the split point is
///   `min(len, len)` rounded down, so it is always in bounds, but the
///   checked API means there is no panic path to reason about at all —
///   which matters more than usual given the release profile's
///   `panic = "abort"`.
pub fn composite_layer_into(out: &mut [f16], texels: &[f16], opacity: f32, mode: BlendMode) {
    let opacity = opacity.clamp(0.0, 1.0);
    // One split point for *both* slices, so the two tails begin at the
    // same offset and exactly as many texels are folded as the
    // pre-0.98.0 `zip` folded. See this function's "Why the two slices
    // are split at one shared offset" section: each side's own
    // `chunks_exact(..).remainder()` -- the spelling
    // `aurora_gpu::residency`'s single-input serializer can use -- would
    // misalign the tail whenever the two chunk counts differ.
    let vectorized = (out.len().min(texels.len()) / CHUNK_SAMPLES) * CHUNK_SAMPLES;
    // Both `else` arms below are unreachable by construction: `vectorized`
    // is `min(out.len(), texels.len())` rounded *down*, so it is <= both
    // lengths and every split point is in bounds (the proof is in this
    // function's "Panics: none, by construction" section). `return` --
    // fold nothing -- is chosen over attempting a partial fold precisely
    // because a case that cannot occur has no principled partial answer:
    // if the split point were somehow out of bounds, the length
    // relationship the whole lockstep argument rests on would already be
    // broken, and folding *some* prefix would be guessing. Do not
    // "handle" these arms; they exist only because the workspace denies
    // `unwrap` and the panicking `split_at`/`split_at_mut`.
    let Some((head_src, tail_src)) = texels.split_at_checked(vectorized) else {
        return;
    };
    let Some((head_out, tail_out)) = out.split_at_mut_checked(vectorized) else {
        return;
    };

    // `wide` names the sample width: these hold one chunk's `f16`
    // samples widened to `f32` for the duration of that chunk's fold.
    let mut dst_wide = [0f32; CHUNK_SAMPLES];
    let mut src_wide = [0f32; CHUNK_SAMPLES];
    for (dst_chunk, src_chunk) in head_out
        .chunks_exact_mut(CHUNK_SAMPLES)
        .zip(head_src.chunks_exact(CHUNK_SAMPLES))
    {
        // Two vectorized f16 -> f32 passes: 8 lanes per `vcvtph2ps`, one
        // feature-detection check per slice rather than per sample.
        dst_chunk.convert_to_f32_slice(&mut dst_wide);
        src_chunk.convert_to_f32_slice(&mut src_wide);
        for (dst, src) in dst_wide
            .chunks_exact_mut(CHANNELS)
            .zip(src_wide.chunks_exact(CHANNELS))
        {
            let [dr, dg, db, da] = dst else { continue };
            let [sr, sg, sb, sa] = src else { continue };
            let [nr, ng, nb, na] =
                fold_texel([*dr, *dg, *db, *da], [*sr, *sg, *sb, *sa], opacity, mode);
            *dr = nr;
            *dg = ng;
            *db = nb;
            *da = na;
        }
        // One vectorized f32 -> f16 pass: 8 lanes per `vcvtps2ph`. Every
        // sample written back is a computed value, so unlike 0.92.0's
        // serializer there is no channel that must come from the
        // original `f16` chunk instead.
        dst_chunk.convert_from_f32_slice(&dst_wide);
    }

    // The scalar tail. Unreachable for a real whole-tile fold
    // (`SAMPLES % CHUNK_SAMPLES == 0`); it exists for the defensive and
    // test-only lengths the length-mismatch section above describes.
    for (dst, src) in tail_out
        .chunks_exact_mut(CHANNELS)
        .zip(tail_src.chunks_exact(CHANNELS))
    {
        let [dr, dg, db, da] = dst else { continue };
        let [sr, sg, sb, sa] = src else { continue };
        let [nr, ng, nb, na] = fold_texel(
            [dr.to_f32(), dg.to_f32(), db.to_f32(), da.to_f32()],
            [sr.to_f32(), sg.to_f32(), sb.to_f32(), sa.to_f32()],
            opacity,
            mode,
        );
        *dr = f16::from_f32(nr);
        *dg = f16::from_f32(ng);
        *db = f16::from_f32(nb);
        *da = f16::from_f32(na);
    }
}

/// A fresh, [`aurora_tile::SAMPLES`]-length `f16` buffer of fully
/// transparent black — the starting state every accumulation begins
/// from, whether it's [`composite_tile_cpu`]'s own or a caller folding
/// its layers in one at a time via [`composite_layer_into`]. Factored
/// out so those two paths cannot drift apart on what "empty" means:
/// a document (or group) with no visible pixel layers composites to
/// exactly this.
#[must_use]
pub fn transparent_tile() -> Vec<f16> {
    vec![f16::from_f32(0.0); SAMPLES]
}

/// The largest finite value an `f16` can hold, as `f32` — the clamp
/// [`un_premultiply_in_place`]'s division saturates to, so a very small
/// alpha can never turn a finite colour channel into `inf`.
const F16_MAX: f32 = 65504.0;

/// One channel of [`un_premultiply_in_place`]'s division, saturated to
/// the finite `f16` range rather than allowed to overflow to `inf` —
/// see that function's own doc comment for why the clamp is load-bearing
/// and not cosmetic.
fn straighten_channel(channel: f16, alpha: f32) -> f16 {
    f16::from_f32((channel.to_f32() / alpha).clamp(-F16_MAX, F16_MAX))
}

/// Converts an accumulator buffer's texels from premultiplied alpha back
/// to straight alpha in place: each texel's `r`/`g`/`b` divided by its
/// own `a`, guarded against `a == 0.0` (a fully transparent texel has no
/// meaningful colour to recover, so its colour channels are zeroed
/// rather than divided by zero) and clamped to the finite `f16` range.
///
/// **Why the clamp, on top of the `a == 0.0` guard.** `f16`'s smallest
/// positive subnormal is ~`5.96e-8`, and a tile buffer can legitimately
/// hold an alpha that small. Dividing a large-but-finite colour channel
/// by it overflows `f16`'s ~`65504` ceiling, and an unclamped
/// `f16::from_f32` turns the overflowing `f32` into `inf` — which then
/// travels silently into an exported PNG/TIFF, the eyedropper, and the
/// canvas atlas as corrupt data rather than failing loudly. Each
/// quotient is therefore saturated to `±65504.0` before the conversion.
/// That is also what the GPU already did for the same inputs, since a
/// fixed-function `Rgba16Float` render target saturates rather than
/// overflowing, so this matches measured hardware behaviour rather than
/// inventing a third one.
///
/// **The design invariant this exists to make explicit.** The low-level
/// accumulation primitives in this module — [`composite_layer_into`] and
/// its batch form [`composite_tile_cpu`] — deliberately **keep** their
/// premultiplied-out contract. Folding straight-alpha "over" math onto a
/// starting-*transparent* destination leaves a premultiplied result
/// whenever the accumulated alpha ends up fractional (a lone
/// `opacity = 0.5` opaque-white layer alone on transparent gives
/// `(0.5, 0.5, 0.5, 0.5)`, not the straight `(1.0, 1.0, 1.0, 0.5)`), and
/// that is not an accident to be fixed there: the fold math itself reads
/// its own accumulator back mid-fold and *depends* on it being
/// premultiplied — see [`composite_layer_into`]'s backdrop-recovery
/// step, which divides the running accumulator by its own alpha to
/// recover the true `Cb` it hands `blend_rgb`. Straightening the
/// accumulator in place after every layer would make that recovery a
/// double division.
///
/// So straightening happens exactly **once**, at the *top* of an
/// accumulation — at the moment a buffer stops being an internal
/// accumulator and becomes a finished result handed to something that
/// expects straight alpha (a caller one level up whose own
/// `composite_layer_into` call takes straight-alpha inputs, an exported
/// PNG/TIFF/`.aur` file, the eyedropper, or the tile store the canvas
/// atlas uploads from). `aurora-app` calls it at exactly three such
/// points, **all three on the CPU**: `resolve_tile`'s `Group` arm (a
/// group's isolated buffer becoming a pseudo-layer),
/// `composite_roots_into_tile` (the finished root-level composite), and
/// `finish_tile_readback` (the GPU compositing path's own readback
/// decode — the GPU leaves a premultiplied accumulator in its render
/// target, exactly as [`TileCompositor::composite_over`]'s own contract says it
/// should, and the CPU straightens the decoded samples once as they stop
/// being that accumulator and become a finished tile). Doing it in one
/// implementation rather than two means the CPU and GPU compositing
/// paths cannot disagree about the division *by construction* — 0.52.0's
/// first shape ran a separate WGSL sibling of this loop as an extra
/// render pass, and the two implementations were measured to disagree at
/// very small alphas (the GPU's fixed-function target saturated where
/// this loop overflowed to `inf`).
///
/// **Straightening a near-transparent texel is intrinsically lossy**, and
/// that is a property of `f16` storage rather than a bug in the
/// division: `f16`'s quantization step near zero is *absolute*
/// (~`5.96e-8`), so a premultiplied colour stored at, say, `alpha = 1e-6`
/// has already lost most of its significant bits before this function
/// sees it, and dividing by that alpha amplifies the residual error by
/// `1/alpha`. Below roughly `alpha = 3e-6` the recovered colour should be
/// treated as indicative only. Worth knowing when an eyedropper reading
/// or an exported value on a nearly invisible pixel looks surprising —
/// the surprise is upstream, in what `f16` could represent at all.
///
/// Texels are processed in [`aurora_tile::CHANNELS`]-wide chunks; a
/// trailing partial chunk (which a correctly sized
/// [`aurora_tile::SAMPLES`]-length buffer never has) is left untouched.
///
/// **One of those three `aurora-app` call sites is conditional as of
/// 0.94.0.** `composite_roots_into_tile` skips this call for a tile no
/// root folded into, because on such a buffer — [`transparent_tile`]'s
/// own untouched output — this function is a bitwise identity: every
/// alpha is `0.0`, so the `alpha > 0.0` arm never runs and the else-arm
/// writes back the `0x0000` already there. That is a claim about *this*
/// function's behaviour, so it is pinned by tests in this module rather
/// than at the call site — `un_premultiply_in_place_is_a_bitwise_identity_on_a_transparent_tile`
/// and `transparent_tile_is_all_canonical_positive_zero_bits`. Changing
/// the zero-alpha arm to write anything other than what it read must
/// fail those, since the caller's skip would otherwise silently stop
/// being output-identical.
pub fn un_premultiply_in_place(texels: &mut [f16]) {
    for texel in texels.chunks_exact_mut(CHANNELS) {
        let [r, g, b, a] = texel else { continue };
        let alpha = a.to_f32();
        if alpha > 0.0 {
            *r = straighten_channel(*r, alpha);
            *g = straighten_channel(*g, alpha);
            *b = straighten_channel(*b, alpha);
        } else {
            *r = f16::from_f32(0.0);
            *g = f16::from_f32(0.0);
            *b = f16::from_f32(0.0);
        }
    }
}

/// Composites `layers` (bottom-to-top; each entry is one tile's own full
/// [`aurora_tile::SAMPLES`]-length `f16` texel buffer, that layer's own
/// opacity, and that layer's own [`BlendMode`]) into one tile.
///
/// The entry point to this crate's CPU compositing family, and a thin
/// orchestration over the two primitives that hold the actual
/// behaviour: it starts from [`transparent_tile`]'s own fully
/// transparent black buffer and makes one [`composite_layer_into`] call
/// per layer, in the given order. The per-texel formula and the
/// backdrop-colour-recovery rule live on [`composite_layer_into`]'s own
/// doc comment — read that one for what a single layer actually
/// computes. What the whole family is *for*, and where its edges are,
/// is here.
///
/// Straight-alpha *inputs*, generalized to a per-blend-mode source
/// colour — the CPU-side sibling of [`TileCompositor::composite_over`]'s
/// own GPU shader math, needed because the actual orchestration this
/// crate can't do itself (walking a real `aurora_doc::LayerTree` to
/// decide *which* layers, in what order, at what opacity and blend
/// mode) can't live here: `aurora-render` and `aurora-doc` are sibling
/// crates in PRD §7.2's layering (neither depends on the other), so
/// `aurora-app`, which depends on both, is where that walk actually
/// happens (`translate_blend_mode` converts a real
/// `aurora_doc::BlendMode` into this crate's own [`BlendMode`] at that
/// boundary) — this family is the pure per-tile math it calls once it
/// has real layer data in hand, exactly what this module's own
/// [`TileCompositor`] doc comment already anticipated ("the primitive
/// real layer compositing will call once that model exists"). It
/// reaches that math through [`composite_layer_into`] rather than
/// through this batch form, one layer at a time, for the memory reason
/// at the bottom of this comment.
///
/// **Scope, stated honestly**: [`BlendMode`] implements 26 of
/// `aurora_doc::BlendMode`'s real 27 variants — `Normal`, the
/// "simple separable" family (`Darken`, `Multiply`, `Lighten`,
/// `Screen`, `Difference`, `Exclusion`, `Subtract`, `Divide`), each a
/// pure per-channel function of backdrop and source with no cross-
/// channel or midpoint-branching logic, the "dodge and burn"
/// family (`ColorDodge`, `LinearDodge`, `ColorBurn`, `LinearBurn`),
/// each also a pure per-channel function but with real 0/1 edge-case
/// branches (division by a zero or saturated channel must not produce
/// `NaN`/`Infinity`), the "overlay and light" family (`Overlay`,
/// `SoftLight`, `HardLight`, `VividLight`, `LinearLight`, `PinLight`,
/// `HardMix`), each a source-or-backdrop-midpoint-branching function
/// that (`SoftLight` aside) composes directly from the two families
/// above rather than needing new math of its own, the
/// HSL non-separable family (`Hue`, `Saturation`, `Color`, `Luminosity`),
/// each a whole-`(R,G,B)`-triple function via `blend_rgb` rather
/// than `blend_channel` (see that function's own doc comment for
/// why), and the whole-colour-selection family (`DarkerColor`,
/// `LighterColor`), also dispatched via `blend_rgb` but selecting one
/// entire input colour by comparing overall luminosity rather than
/// blending the two via `SetLum`/`SetSat` the way the HSL family does.
/// The sole remaining variant, `Dissolve`, is this family's own
/// explicit boundary — stochastic per-pixel selection, not a
/// deterministic blend function at all, so it needs its own
/// reproducibility design decision (does a given pixel's outcome need
/// to be stable across re-renders? seeded by what?) before any
/// implementation, not just new math — separate, still-open follow-on
/// work. A layer using `Dissolve` falls back to `Normal` at the
/// `aurora-app` translation boundary (`translate_blend_mode`), not
/// here. This is a CPU implementation specifically because the
/// orchestration crate (`aurora-app`) needs to run it per visible tile,
/// per layer, every time any constituent layer changes —
/// GPU-accelerated multi-layer compositing (reusing [`TileCompositor`]
/// properly, with a real opacity/blend-mode-aware shader) is separate,
/// still-open follow-on work.
///
/// Returns a fresh, [`aurora_tile::SAMPLES`]-length buffer; an empty
/// `layers` composites to fully transparent black, exactly matching
/// what a document with no visible pixel layers should show.
///
/// Peak memory here is proportional to the number of `layers` the
/// *caller* is holding buffers for, which is why `aurora-app`'s own
/// `resolve_tile` calls [`composite_layer_into`] directly rather than
/// collecting every sibling's buffer to pass here in one batch. This
/// batch form stays for callers that genuinely have all their slices in
/// hand already (and for the tests in this module).
#[must_use]
pub fn composite_tile_cpu(layers: &[(&[f16], f32, BlendMode)]) -> Vec<f16> {
    let mut out = transparent_tile();
    for &(texels, opacity, mode) in layers {
        composite_layer_into(&mut out, texels, opacity, mode);
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
/// Deliberately minimal: [`Self::composite_over`] blends exactly one
/// source tile over one destination tile via straight-alpha
/// "source-over" (`Blend::AlphaBlending`), no blend-mode or opacity
/// parameter of its own — those are a layer's properties, and when this
/// method was first written the layer model (`aurora-doc`) didn't exist
/// yet, nor could it: `aurora-render` sits below it in the layering (PRD
/// §7.2) and has no way to know either.
///
/// [`Self::composite_over_with_opacity`] is the real, additive opacity-
/// aware sibling that followed once a caller (`aurora-app`, which
/// depends on both `aurora-render` and `aurora-doc`) actually had a
/// per-layer opacity to apply — `composite_over` itself is unchanged, so
/// every existing caller/test keeps its exact prior behaviour. Neither
/// of those two methods knows about blend *modes* at all: both express
/// `Normal` and only `Normal`, since the fixed-function
/// `Blend::AlphaBlending` unit they drive has no other formula.
///
/// [`Self::composite_multiply_over_with_opacity`] (0.83.0) is the first
/// method here that does express a blend *mode*:
/// `aurora_doc::BlendMode::Multiply`, computed in WGSL against a
/// separately-sampled backdrop rather than by the fixed-function unit.
/// [`Self::composite_darken_over_with_opacity`] (0.85.0) is the second,
/// [`Self::composite_lighten_over_with_opacity`] (0.95.0) the third,
/// [`Self::composite_screen_over_with_opacity`] (0.102.0) the fourth, and
/// [`Self::composite_difference_over_with_opacity`] (0.104.0) the fifth,
/// [`Self::composite_linear_dodge_over_with_opacity`] (0.105.0) the
/// sixth, [`Self::composite_linear_burn_over_with_opacity`] (0.106.0)
/// the seventh,
/// [`Self::composite_color_burn_over_with_opacity`] (0.107.0) the eighth,
/// and [`Self::composite_color_dodge_over_with_opacity`] (0.108.0) the
/// ninth,
/// each built to exactly the same shape.
/// The remaining 17 modes have no dedicated blend-math WGSL entry point
/// of their own, and wait on one — this crate's own `BlendMode` enum
/// has 26 variants (it excludes `Dissolve`, which is a pre-composite
/// gate, never a per-pixel formula this crate would need to port), so
/// 17 is "26 minus the nine, `Multiply`, `Darken`, `Lighten`, `Screen`,
/// `Difference`, `LinearDodge`, `LinearBurn`, `ColorBurn` and
/// `ColorDodge`, done so
/// far."
///
/// **`Normal` is one of those 17 and is *not* CPU-only**, so read the
/// figure as "no blend-math shader", never as "no GPU path":
/// [`Self::composite_over_with_opacity`]'s fixed-function
/// `Blend::AlphaBlending` unit already expresses `Normal` on the GPU,
/// which is exactly why it needs no formula in
/// `shaders/composite.wgsl`. The modes genuinely left to
/// `composite_tile_cpu` are therefore the other **16**, and that is
/// precisely `aurora-app`'s own count of what its GPU predicate
/// rejects: 27 real `aurora_doc::BlendMode` variants minus the eleven it
/// admits (`Normal`, `Multiply`, `Darken`, `Lighten`, `Screen`,
/// `Difference`, `LinearDodge`, `LinearBurn`, `ColorBurn`, `ColorDodge`
/// and `Dissolve`). The two
/// figures — 17
/// here, 16 there —
/// count different
/// things, and the one mode between them is `Normal`. `Dissolve` is in
/// neither set: absent from this crate's enum altogether, and *admitted*
/// at the app's predicate (0.84.1) without ever needing a formula here.
/// See PLAN.md's 0.84.1 addendum if the two numbers
/// look inconsistent side by side; they're counting different things,
/// not disagreeing. See `aurora-app`'s own
/// `begin_gpu_composite_tile`/`document_qualifies_for_gpu_compositing`
/// (a higher crate — `aurora-render` cannot name it directly, PRD
/// §7.2's layering) for exactly which whole documents those primitives
/// can and can't correctly composite between them.
pub struct TileCompositor {
    bind_group_layout: wgpu::BindGroupLayout,
    /// The [`Self::composite_over_with_opacity`]-only sibling of
    /// `bind_group_layout` above: the same texture + sampler pair, plus
    /// a third binding for the opacity uniform buffer.
    /// [`Self::composite_over`] itself never touches this — kept
    /// entirely separate so that method's own layout, and therefore its
    /// exact prior pipeline shape/behaviour, is untouched by this
    /// addition.
    bind_group_layout_opacity: wgpu::BindGroupLayout,
    /// The blend-math sibling of
    /// `bind_group_layout_opacity` above, shared by every
    /// `composite_*_over_with_opacity` blend-math method — as of 0.105.0
    /// [`Self::composite_multiply_over_with_opacity`] and its
    /// `darken`/`lighten`/`screen`/`difference`/`linear_dodge` siblings,
    /// named as a family rather than relisted, because a new mode joins
    /// the list every time one is ported and this layout is untouched by
    /// that: the same texture + sampler +
    /// opacity-uniform triple, plus a fourth binding for the *backdrop*
    /// texture the shader samples instead of reaching it through the
    /// fixed-function blend unit. Kept entirely separate from both
    /// layouts above for the same reason those two are separate from
    /// each other — neither existing method's pipeline shape or
    /// behaviour is touched by this addition.
    bind_group_layout_blend: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shader: wgpu::ShaderModule,
    pipelines: PipelineCache,
}

/// Builds [`TileCompositor::bind_group_layout_blend`]: the opacity
/// layout's three bindings plus the backdrop texture at binding 3, which
/// is what makes real in-shader blend math possible at all — the
/// fixed-function blend unit never hands a shader the backdrop's colour.
/// Shared, unchanged, by every such entry point in
/// `shaders/composite.wgsl`: `fs_composite_multiply` (0.83.0),
/// `fs_composite_darken` (0.85.0), `fs_composite_lighten` (0.95.0),
/// `fs_composite_screen` (0.102.0), `fs_composite_difference`
/// (0.104.0) and `fs_composite_linear_dodge` (0.105.0) so
/// far. A newly ported mode needs its
/// own entry point and its own `PipelineKey`, but no new layout.
///
/// A free function rather than more lines inside
/// [`TileCompositor::new`] purely to keep that constructor under
/// `clippy::too_many_lines`; the two older layouts are deliberately left
/// inline and untouched rather than refactored alongside it.
fn blend_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(LABEL_BLEND_LAYOUT),
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
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // The backdrop texture — the one binding that makes real
            // in-shader blend math possible at all.
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
        ],
    })
}

/// Builds the one render pipeline shape every entry point in
/// `shaders/composite.wgsl` uses: the fullscreen-triangle vertex stage,
/// one `Rgba16Float` colour target, no depth, no multisampling, and
/// `key`'s own blend state. Everything that actually differs between the
/// composite paths is already in `key` (fragment entry point,
/// blend state) or in `bind_group_layout`.
///
/// *Historical:* extracted in 0.83.1, back when this file had three call
/// sites and only `Multiply` was ported — the bet being that each later
/// blend mode would add a `PipelineKey` rather than another copy of this
/// descriptor. It has held so far (`Darken`, 0.85.0, `Lighten`, 0.95.0,
/// and `Screen`, 0.102.0, each added exactly
/// that). Those counts are the 0.83.1 ones and are not maintained here;
/// the live numbers are [`TileCompositor`]'s own doc comment and
/// `aurora-app`'s `document_qualifies_for_gpu_compositing`.
/// It is a pure extraction: the descriptors it builds for
/// [`TileCompositor::composite_over`] and
/// [`TileCompositor::composite_over_with_opacity`] are field-for-field
/// what those two methods built inline before, `label` included.
///
/// **`bind_group_layout` is not part of `key`, and `get_or_create_with`
/// caches by `key` alone.** Today that's safe only because each
/// `fragment_entry` this crate ever calls with is paired with exactly
/// one layout, at exactly one call site. A future blend mode that reused
/// an existing `fragment_entry` string against a *different* layout
/// would silently receive the wrong cached pipeline — wrong bind-group
/// count, not a compile error. Keep that one-entry-point-one-layout
/// pairing intact, or fold the layout into `PipelineKey` (an
/// `aurora-gpu` API change, not attempted here) before it stops holding.
fn composite_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    key: &PipelineKey,
    bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
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
}

/// Uploads `opacity` into a fresh [`OPACITY_UNIFORM_SIZE`]-byte uniform
/// buffer laid out exactly like `shaders/composite.wgsl`'s own `Opacity`
/// struct: the `f32` value followed by 12 bytes of padding (three plain
/// scalar fields, *not* a `vec3<f32>` — see that struct's own comment for
/// the `wgpu` validation error that distinction caused).
///
/// The caller is responsible for clamping `opacity` before calling: the
/// shader deliberately does not re-clamp, because
/// [`composite_layer_into`] clamps the *opacity*, not the
/// `src_alpha * opacity` product.
///
/// Extracted in 0.83.1 alongside [`composite_pipeline`], for the same
/// reason — this block was byte-identical in both opacity-aware paths.
fn opacity_uniform_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    opacity: f32,
    label: &str,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: OPACITY_UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut bytes = Vec::with_capacity(OPACITY_UNIFORM_SIZE as usize);
    bytes.extend_from_slice(&opacity.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 12]);
    queue.write_buffer(&buffer, 0, &bytes);
    buffer
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
        let bind_group_layout_opacity =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let bind_group_layout_blend = blend_bind_group_layout(device);
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
            bind_group_layout_opacity,
            bind_group_layout_blend,
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
    ///
    /// **Records into `encoder`; does not submit** (0.86.0). Nothing on
    /// the GPU happens until the caller finishes `encoder` and submits
    /// it — see [`Self::composite_over_with_opacity`]'s own doc comment
    /// for the full account of why every compositor method here is a
    /// pure recorder now.
    pub fn composite_over(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
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
            composite_pipeline(device, shader, &key, layout, LABEL)
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
    }

    /// Blends `src` over `dst` in place exactly like [`Self::composite_over`]
    /// (`LoadOp::Load`, straight-alpha "source-over" via the same
    /// `Blend::AlphaBlending` fixed-function state), except `src`'s own
    /// alpha channel is scaled by `opacity` (clamped to `0.0..=1.0`)
    /// before the blend unit ever sees it — the GPU counterpart of
    /// [`composite_tile_cpu`]'s own `alpha = src_alpha * opacity` step.
    /// The fixed-function blend unit itself has no uniform input, so
    /// this runs a real, separate fragment shader entry point
    /// (`fs_composite_opacity`, `shaders/composite.wgsl`) that computes
    /// the scaled alpha in the shader instead; the *same* blend state
    /// then does the rest, unchanged. Both views must be `Rgba16Float`,
    /// the same size, and `dst`'s owning texture must include
    /// `RENDER_ATTACHMENT` usage — identical preconditions to
    /// `composite_over`.
    ///
    /// A separate bind group layout/pipeline from `composite_over`'s own
    /// (a third binding, the opacity uniform buffer) — `composite_over`
    /// itself is otherwise unchanged by this method's existence: same
    /// shader entry point, same pipeline key.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0). Until this
    /// method returned an implicitly-submitted command buffer of its
    /// own, so a caller compositing N layers onto one accumulator paid N
    /// `queue.submit` calls, plus one for its clear and one for its
    /// readback. `aurora-app`'s `begin_gpu_composite_tile` — the only
    /// production caller — issued three submits per tile even for the
    /// simplest single-layer document. It now opens one
    /// `wgpu::CommandEncoder` per tile, hands it to every method here in
    /// turn, and submits once. Two consequences a caller must know:
    ///
    /// - **Nothing observable happens until the caller submits.** A
    ///   readback recorded into a *different*, earlier-submitted encoder
    ///   sees the destination as it was before this call. That is the
    ///   subject of this module's own
    ///   `composite_over_with_opacity_records_into_the_encoder_without_submitting_it` test.
    /// - **Ordering within one command buffer is still guaranteed.**
    ///   Passes recorded back to back into one encoder execute in
    ///   recording order, so a later pass sampling a texture an earlier
    ///   pass rendered into reads the earlier pass's finished result —
    ///   which is exactly what `begin_gpu_composite_tile`'s ping-pong
    ///   accumulator pair relies on, and what the chained
    ///   `composite_multiply_*`/`composite_darken_*` tests below pin.
    ///
    /// `context` is still needed: `device()` builds the pipeline and
    /// bind group, and `queue()` uploads the opacity uniform via
    /// `write_buffer`. That upload is a queue-level write flushed by
    /// whichever submit comes next, and it targets a buffer created
    /// fresh inside this call, so no ordering hazard follows from it.
    pub fn composite_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
        src: &wgpu::TextureView,
        opacity: f32,
    ) {
        let device = context.device();
        let opacity = opacity.clamp(0.0, 1.0);
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_composite",
            fragment_entry: "fs_composite_opacity",
            target_format: wgpu::TextureFormat::Rgba16Float,
            blend: Blend::AlphaBlending,
        };
        let layout = &self.bind_group_layout_opacity;
        let shader = &self.shader;
        let pipeline = self.pipelines.get_or_create_with(key.clone(), || {
            composite_pipeline(device, shader, &key, layout, LABEL)
        });

        // 16 bytes: a real `f32` opacity value plus 12 bytes of padding,
        // matching `shaders/composite.wgsl`'s own `Opacity` struct
        // (`value: f32, _pad0: f32, _pad1: f32, _pad2: f32` — plain
        // scalar padding fields, not a `vec3<f32>`; see that struct's
        // own doc comment for why) byte for byte — the same "pad a
        // small scalar uniform to 16 bytes for defensive cross-backend
        // alignment" shape `aurora-widgets`' own
        // `PathPipeline::bind_group` already uses.
        let uniform_buffer = opacity_uniform_buffer(device, context.queue(), opacity, LABEL);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(LABEL),
            layout: &self.bind_group_layout_opacity,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
            ],
        });

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
    }

    /// Composites `src` over `backdrop` with **`BlendMode::Multiply`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// This is the first GPU path in the workspace that expresses a
    /// non-`Normal` blend mode at all. [`Self::composite_over`] and
    /// [`Self::composite_over_with_opacity`] both leave the "over" math
    /// to the fixed-function blend unit, which can only ever express
    /// `Normal`: it has no way to hand the backdrop's *colour* to a
    /// formula like `Cb * Cs`. So this method instead binds the
    /// accumulator as a sampled texture (binding 3), computes the entire
    /// composite — including the "over" — in `fs_composite_multiply`
    /// (`shaders/composite.wgsl`), and writes the finished premultiplied
    /// texel with `Blend::None` (a plain replace).
    ///
    /// **Read-and-write are separate textures on purpose.** Sampling the
    /// same texture a pass is rendering into is undefined, so `dst` must
    /// not alias `backdrop`; the colour attachment is therefore cleared
    /// (`LoadOp::Clear`) rather than loaded, since nothing in `dst` is
    /// being accumulated onto. A caller that wants the accumulator
    /// updated in place needs a ping-pong pair or a copy of its own —
    /// deliberately not decided here.
    ///
    /// **Multiply only, no `mode` parameter.** This method is one mode's
    /// name and nothing else: the body is
    /// `composite_blend_over_with_opacity` applied to
    /// `BLEND_PASS_MULTIPLY`. Read *that* method's doc comment for the
    /// current reasoning on why the four ported modes are still four
    /// public
    /// entry points rather than one taking a `mode` argument. (Until
    /// 0.85.1 this paragraph said "exactly one mode is ported, so there
    /// is nothing to dispatch on yet" — true when it was written at
    /// 0.83.0, falsified by `Darken` at 0.85.0.)
    ///
    /// **The application calls this now** (0.84.0). `aurora-app`'s own
    /// `document_qualifies_for_gpu_compositing` admits `Multiply`
    /// alongside `Normal`, and `begin_gpu_composite_tile` dispatches to
    /// this method for every `Multiply` root layer, ping-ponging between
    /// two accumulator textures to satisfy the aliasing rule above. The
    /// shader math is proven against [`composite_tile_cpu`]'s own results
    /// by this module's `composite_multiply_*` tests independently of
    /// that wiring.
    ///
    /// **Parameter order is inputs first, output last** — `(src,
    /// backdrop, dst)`, deliberately *not*
    /// [`Self::composite_over_with_opacity`]'s `(dst, src)` shape. Three
    /// consecutive `&wgpu::TextureView` parameters are trivially
    /// swappable at a call site, and the second parameter's *meaning*
    /// differs between the two methods (there it is the source; here the
    /// accumulator would sit in that slot under the old order), so the
    /// two signatures are shaped differently on purpose: a reader cannot
    /// pattern-match one onto the other from muscle memory.
    ///
    /// **The aliasing failure mode is a raw `wgpu` panic.** Passing the
    /// same view for `dst` and `backdrop` trips `wgpu`'s own validation
    /// rather than corrupting pixels silently — but nothing in this
    /// workspace installs an error scope or an uncaptured-error handler
    /// (`push_error_scope`/`on_uncaptured_error` appear nowhere under
    /// `crates/`), so that validation failure reaches the user as
    /// `wgpu`'s default panic, in a workspace whose lints otherwise deny
    /// `panic!` precisely because a panic loses unsaved work. That gap is
    /// **pre-existing and workspace-wide**; this method inherits it and
    /// neither introduces nor fixes it. Closing it is a separate
    /// decision about every `wgpu` call site, not this one.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`, exactly as
    /// `composite_over_with_opacity` clamps it.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0), like every
    /// other compositor method here — see
    /// [`Self::composite_over_with_opacity`] for what that means for a
    /// caller and why the change was made. The ping-pong this method
    /// exists to serve is *why* it is safe: `backdrop` is read by a pass
    /// recorded after the pass that wrote it, in the same command
    /// buffer, and within one command buffer passes execute in recording
    /// order.
    pub fn composite_multiply_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_MULTIPLY,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::Darken`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The second blend mode ported to the GPU, and a deliberate
    /// line-for-line mirror of
    /// [`Self::composite_multiply_over_with_opacity`] above: same
    /// `bind_group_layout_blend`, same `Blend::None` replace, same
    /// `Opacity` uniform, same `(src, backdrop, dst)` parameter order,
    /// same clamp. **Read that method's doc comment for all of the
    /// shared "why"** — why the fixed-function blend unit cannot express
    /// a real blend mode at all, why the accumulator must arrive as a
    /// sampled texture, why `dst` must not alias `backdrop`, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of that reasoning is
    /// mode-specific, and it is not repeated here.
    ///
    /// **What is specific to this method** is one line of WGSL:
    /// `blend_rgb(Darken, Cb, Cs)` is `blend_channel`'s `cb.min(cs)`
    /// applied independently per channel — a *separable* mode with no
    /// guards, no branches and no division, so `fs_composite_darken`
    /// differs from `fs_composite_multiply` only in a single `min()`
    /// where the other has a `*`. It is emphatically **not**
    /// `DarkerColor`, which picks one whole `(R, G, B)` triple by
    /// luminosity and is a different, still-CPU-only mode.
    ///
    /// **No `mode` dispatch parameter, still — but no duplicated body
    /// either** (0.85.1). This method and
    /// [`Self::composite_multiply_over_with_opacity`] are both one-line
    /// wrappers over
    /// `composite_blend_over_with_opacity`, differing only in the
    /// `BlendPass` const they hand it. 0.85.0 shipped them as two
    /// ~85-line near-copies and deferred the merge on the grounds that
    /// "two samples is too thin a basis for the right abstraction";
    /// review of that round's own diff did not support the claim, since
    /// the entire variation was already six `&'static str`s and the
    /// shared scaffolding (`composite_pipeline`,
    /// `opacity_uniform_buffer`, `blend_bind_group_layout`) had needed
    /// no changes at all to take a second mode. The merge landed in
    /// 0.85.1 as a pure extraction instead.
    ///
    /// The *public* shape is still five named methods rather than one
    /// `mode: BlendMode` parameter, and that part is a real deferral: a
    /// `mode` parameter would have to say what happens for the 21 modes
    /// with no *blend-math* WGSL entry point behind them — `Normal`
    /// among them, which needs none, since it composites through the
    /// fixed-function unit
    /// ([`Self::composite_over_with_opacity`]) rather than through a
    /// shader formula (panic — denied here; return
    /// a `Result` no caller can act on; silently do `Normal`), and the
    /// answer belongs with whatever ports enough of them to make the
    /// question concrete. Two total functions per mode is the cost until
    /// then.
    ///
    /// **The application calls this** (0.85.0), the same way it calls the
    /// `Multiply` sibling: `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `Darken`, and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `Darken` root layer, through the *same* single ping-pong spare
    /// accumulator a `Multiply` layer would use. The shader math is
    /// proven against [`composite_tile_cpu`]'s own results by this
    /// module's `composite_darken_*` tests independently of that wiring.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — the same
    /// as its `Multiply` sibling directly above, for the same reasons;
    /// see [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_darken_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_DARKEN,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::Lighten`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The third blend mode ported to the GPU, and the exact mirror of
    /// [`Self::composite_darken_over_with_opacity`] above: same
    /// `bind_group_layout_blend`, same `Blend::None` replace, same
    /// `Opacity` uniform, same `(src, backdrop, dst)` parameter order,
    /// same clamp, same caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the
    /// accumulator must arrive as a sampled texture, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of it is mode-specific.
    ///
    /// What is specific to this method is one line of WGSL:
    /// `blend_rgb(Lighten, Cb, Cs)` is `blend_channel`'s `cb.max(cs)`
    /// per channel, so `fs_composite_lighten` differs from
    /// `fs_composite_darken` only in a single `max()` where that one has
    /// a `min()`. It is emphatically **not** `LighterColor`, which picks
    /// one whole `(R, G, B)` triple by luminosity and is still CPU-only.
    ///
    /// **The application calls this** (0.95.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `Lighten` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `Lighten` root layer, through the *same* single ping-pong spare
    /// accumulator a `Multiply` or `Darken` layer would use. Unlike its
    /// two predecessors, its dispatch arm is instrumented from day one
    /// (`GpuBlendDispatch::Lighten` in that crate), so no test here can be
    /// satisfied by a silent CPU fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_lighten_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_LIGHTEN,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::Screen`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The fourth blend mode ported to the GPU, and built to exactly the
    /// shape of the three above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the
    /// accumulator must arrive as a sampled texture, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of it is mode-specific.
    ///
    /// What is specific to this method is one line of WGSL:
    /// `blend_rgb(Screen, Cb, Cs)` is `blend_channel`'s
    /// `cb + cs - cb * cs` per channel — the first ported mode whose
    /// formula is real arithmetic on both operands rather than one
    /// intrinsic (`Multiply` is a `*`, `Darken` a `min()`, `Lighten` a
    /// `max()`). `fs_composite_screen` writes that sum literally rather
    /// than the algebraically-equal `1 - (1 - Cb)(1 - Cs)`, so it stays
    /// line-for-line comparable against `blend_channel`'s own arm; see
    /// the entry point's own comment.
    ///
    /// **The application calls this** (0.102.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `Screen` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `Screen` root layer, through the *same* single ping-pong spare
    /// accumulator a `Multiply`, `Darken` or `Lighten` layer would use.
    /// Like `Lighten` and unlike `Multiply`, its dispatch arm was
    /// instrumented from day one (`GpuBlendDispatch::Screen` in that
    /// crate; `Multiply`'s was retrofitted in 0.103.0, so every mode the
    /// app admits other than `Normal` — eight of them since `LinearBurn`
    /// landed in 0.106.0 — now carries one), so no test here can be
    /// satisfied by a silent CPU fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_screen_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_SCREEN,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::Difference`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The fifth blend mode ported to the GPU, and built to exactly the
    /// shape of the four above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the
    /// accumulator must arrive as a sampled texture, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of it is mode-specific.
    ///
    /// What is specific to this method is one line of WGSL:
    /// `blend_rgb(Difference, Cb, Cs)` is `blend_channel`'s
    /// `(cb - cs).abs()` per channel, which `fs_composite_difference`
    /// writes as a single componentwise `abs()` on the `vec3<f32>`
    /// difference. It is deliberately **not** `max(Cb - Cs, 0)` — that is
    /// `Subtract`, a different and still-CPU-only mode that agrees with
    /// this one only where `Cb >= Cs`; see the entry point's own comment,
    /// and the `composite_difference_*` fixtures below, every one of which
    /// has a channel where `Cb < Cs` precisely so the two cannot be
    /// confused.
    ///
    /// **`Difference` is symmetric in `Cb`/`Cs`** (`|Cb - Cs| =
    /// |Cs - Cb|`), exactly as `Screen` is, so a transposed
    /// `src`/`backdrop` binding is not caught by the blend term alone —
    /// only by the surrounding, asymmetric "over" and by the spatial
    /// per-texel differential. Disclosed rather than left implied.
    ///
    /// **The application calls this** (0.104.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `Difference` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `Difference` root layer, through the *same* single ping-pong spare
    /// accumulator a `Multiply`, `Darken`, `Lighten` or `Screen` layer
    /// would use. Its dispatch arm was instrumented from day one
    /// (`GpuBlendDispatch::Difference` in that crate), so no test here can
    /// be satisfied by a silent CPU fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_difference_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_DIFFERENCE,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::LinearDodge`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The sixth blend mode ported to the GPU, and built to exactly the
    /// shape of the five above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the
    /// accumulator must arrive as a sampled texture, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of it is mode-specific.
    ///
    /// What is specific to this method is one line of WGSL:
    /// `blend_rgb(LinearDodge, Cb, Cs)` is `blend_channel`'s
    /// `(cb + cs).min(1.0)` per channel, which `fs_composite_linear_dodge`
    /// writes as `min(cb + s.rgb, vec3<f32>(1.0))` — the `vec3` splat
    /// because WGSL's `min` needs both operands to share a type.
    ///
    /// **The second ported mode whose formula is real arithmetic on both
    /// operands** rather than one intrinsic (`Screen` was the first;
    /// `Multiply` is a `*`, `Darken` a `min()`, `Lighten` a `max()`,
    /// `Difference` an `abs()`). What separates it from `Screen` is that
    /// **the clamp is part of the mode, not a defensive guard**:
    /// `Cb + Cs` is unbounded above and `LinearDodge` is *defined* as the
    /// clamped sum (Photoshop's "Add"), so dropping the `min` computes a
    /// different function everywhere the sum exceeds `1.0` rather than
    /// merely widening a range.
    ///
    /// **Deliberately not `max(Cb + Cs - 1, 0)`** — that is
    /// `LinearBurn`, this mode's exact mirror image (same sum, opposite
    /// offset, opposite clamp direction) and a different mode, which as of
    /// 0.106.0 has its own GPU entry point
    /// ([`Self::composite_linear_burn_over_with_opacity`]) rather than
    /// being CPU-only — so the copy-paste hazard between the two now runs
    /// in both directions. It is also not `Cb + Cs - Cb*Cs`, which is `Screen`, its
    /// nearest arithmetic neighbour, nor `ColorDodge`, the other
    /// dodge-family mode (`min(1, Cb / (1 - Cs))`) — which as of 0.108.0
    /// likewise has its own GPU entry point
    /// ([`Self::composite_color_dodge_over_with_opacity`]) rather than
    /// being CPU-only, so that hazard now runs in both directions too. And
    /// the resemblance between *those* two is more than a shared word:
    /// `ColorDodge` clamps exactly when `Cb + Cs >= 1`, which is exactly
    /// when this mode clamps, so **no clamped channel can ever tell them
    /// apart** — a fixture meant to discriminate the two needs an
    /// unclamped channel. See
    /// the entry point's own comment, and the `composite_linear_dodge_*`
    /// fixtures below, every one of which has at least one channel whose
    /// sum stays strictly under `1.0` and at least one whose sum exceeds
    /// it, so neither the clamp nor its absence can pass by accident.
    ///
    /// **`LinearDodge` is symmetric in `Cb`/`Cs`** (`Cb + Cs = Cs + Cb`),
    /// exactly as `Screen` and `Difference` are, so a transposed
    /// `src`/`backdrop` binding is not caught by the blend term alone —
    /// only by the surrounding, asymmetric "over" and by the spatial
    /// per-texel differential. Disclosed rather than left implied.
    ///
    /// **The application calls this** (0.105.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `LinearDodge` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `LinearDodge` root layer, through the *same* single ping-pong spare
    /// accumulator a `Multiply`, `Darken`, `Lighten`, `Screen` or
    /// `Difference` layer would use. Its dispatch arm was instrumented
    /// from day one (`GpuBlendDispatch::LinearDodge` in that crate), so no
    /// test here can be satisfied by a silent CPU fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_linear_dodge_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_LINEAR_DODGE,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::LinearBurn`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The seventh blend mode ported to the GPU, and built to exactly the
    /// shape of the six above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the
    /// accumulator must arrive as a sampled texture, and why the
    /// `sa * opacity` product is deliberately left unclamped while
    /// `opacity` itself is clamped. None of it is mode-specific.
    ///
    /// What is specific to this method is one line of WGSL:
    /// `blend_rgb(LinearBurn, Cb, Cs)` is `blend_channel`'s
    /// `(cb + cs - 1.0).max(0.0)` per channel, which
    /// `fs_composite_linear_burn` writes as
    /// `max(cb + s.rgb - 1.0, vec3<f32>(0.0))` — the `vec3` splat because
    /// WGSL's `max` needs both operands to share a type, while the `- 1.0`
    /// broadcasts as a bare scalar.
    ///
    /// **The exact mirror image of
    /// [`Self::composite_linear_dodge_over_with_opacity`]** (0.105.0):
    /// same sum, opposite offset, opposite clamp direction. Written side
    /// by side the two blend lines differ by three characters, so this
    /// one's was derived from `blend_channel`'s own Rust arm rather than
    /// copied from that entry point and edited — which is the mutation
    /// this round's (b), (f) and (g) each perform for real, and every
    /// `composite_linear_burn_*` test below kills.
    ///
    /// **The clamp is part of the mode, not a defensive guard**, the same
    /// way `LinearDodge`'s is: `Cb + Cs - 1` reaches `-1`, and dropping
    /// the `max` computes a different function everywhere the sum falls
    /// under `1.0` — and emits negative colour channels while doing it.
    ///
    /// **Deliberately not `Cb * Cs`**, which is `Multiply`, this mode's
    /// nearest neighbour in behaviour rather than spelling (both darken;
    /// both give `0` for a zero backdrop; they agree exactly where
    /// `(1 - Cb) * (1 - Cs) == 0`), nor `1 - (1 - Cb) / Cs`, which is
    /// `ColorBurn`, the other burn-family mode — **on the GPU itself as
    /// of 0.107.0**, and the dispatch arm directly below this one's in
    /// `aurora-app`, so that hazard is now live in both directions rather
    /// than pointing at a mode that does not exist here. See
    /// the entry point's own comment, and the `composite_linear_burn_*`
    /// fixtures below, every one of which has at least one channel whose
    /// sum stays strictly above `1.0` and at least one whose sum falls
    /// under it, so neither the clamp nor its absence can pass by
    /// accident.
    ///
    /// **`LinearBurn` is symmetric in `Cb`/`Cs`** (`Cb + Cs = Cs + Cb`),
    /// exactly as `Screen`, `Difference` and `LinearDodge` are, so a
    /// transposed `src`/`backdrop` binding is not caught by the blend term
    /// alone — only by the surrounding, asymmetric "over" and by the
    /// spatial per-texel differential. Disclosed rather than left implied.
    ///
    /// **The application calls this** (0.106.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `LinearBurn` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `LinearBurn` root layer, through the *same* single ping-pong spare
    /// accumulator any other blend-math layer would use. Its dispatch arm
    /// was instrumented from day one
    /// (`GpuBlendDispatch::LinearBurn` in that crate), so no test here can
    /// be satisfied by a silent CPU fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_linear_burn_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_LINEAR_BURN,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::ColorBurn`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The eighth blend mode ported to the GPU, and built to exactly the
    /// shape of the seven above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the accumulator
    /// must arrive as a sampled texture, and why the `sa * opacity`
    /// product is deliberately left unclamped while `opacity` itself is
    /// clamped. None of it is mode-specific.
    ///
    /// **What is specific to this method, and it is more than one line of
    /// WGSL for the first time.** `blend_rgb(ColorBurn, Cb, Cs)` is
    /// `blend_channel`'s own three-branch arm per channel:
    ///
    /// ```text
    /// if Cb == 1 { 1 } else if Cs == 0 { 0 }
    /// else { 1 - min(1, (1 - Cb) / Cs) }
    /// ```
    ///
    /// Two of those three branches are per-channel *conditions* rather
    /// than arithmetic, so — unlike every mode ported before it — this one
    /// cannot be one componentwise expression on `vec3<f32>`. The shader
    /// factors the arm into a `color_burn_channel(cb, cs)` helper (one of
    /// that file's two per-channel blend helpers -- it was its first
    /// non-entry-point function when 0.107.0 added it, but 0.109.0's
    /// `straight_backdrop()`/`fold_over()` now precede it, so the ordinal
    /// is dropped rather than re-counted) and calls it three times to
    /// build `b`.
    ///
    /// **Both guards are arithmetically redundant under IEEE-754 and both
    /// are still required.** Drop the `Cb == 1` guard and
    /// `(1 - 1) / Cs = 0` gives `1 - min(1, 0) = 1`, the same answer — for
    /// every `Cs > 0`; at `Cs == 0` the expression becomes `0 / 0`. Drop
    /// the `Cs == 0` guard and `(1 - Cb) / 0` is `+inf`, `min(1, inf)` is
    /// `1`, and the result is `0.0`, again the same answer — *if* the
    /// backend divides like IEEE. **WGSL does not promise that**: division
    /// by zero yields an indeterminate value, which may be `NaN`, and a
    /// `NaN` is not absorbed here because the `ab == 0` half of a tile
    /// multiplies `b` by zero and `0.0 * NaN` is `NaN`. So the guards make
    /// this entry point *defined* rather than merely correct on one
    /// adapter, and 0.107.0 measured exactly that: deleting the first
    /// guard is killed deterministically (the second then fires in its
    /// place), while deleting the second survived every test in this crate
    /// on Vulkan/NVIDIA. That survival is the disclosed, expected result
    /// of a portability guard on IEEE hardware, not a missing test — see
    /// PLAN.md's 0.107.0 entry.
    ///
    /// **Branch order is load-bearing**, and this is the first ported mode
    /// where any is. `Cb == 1` is tested first, so a fully white backdrop
    /// under a fully black source — both conditions true at once, and an
    /// ordinary pixel, not a contrived one — yields `1.0` and not `0.0`.
    ///
    /// **Deliberately not `min(1, Cb / (1 - Cs))`**, which is
    /// `ColorDodge`, the *other* guarded-division mode: its branch
    /// conditions are `Cb == 0` and `Cs == 1` rather than this one's
    /// `Cb == 1` and `Cs == 0`, and as of 0.108.0 it has its own GPU entry
    /// point ([`Self::composite_color_dodge_over_with_opacity`], the
    /// method directly below) rather than being CPU-only — so this hazard
    /// now runs in both directions between two adjacent dispatch arms.
    /// (Until 0.108.0 this sentence printed that formula with a spurious
    /// outer `1 -`, which is *this* mode's shape rather than
    /// `ColorDodge`'s. The distinction it drew — the branch conditions and
    /// the operand order — was right; the formula was not, and was wrong
    /// identically at six sites, all six corrected in that round — though
    /// that round's own count said five, having missed `aurora-app`'s
    /// `begin_gpu_composite_tile` `ColorBurn` dispatch-arm comment, which
    /// was fixed but not counted; 0.108.1 corrected the count.) And
    /// not `max(Cb + Cs - 1, 0)`, which is
    /// [`Self::composite_linear_burn_over_with_opacity`] directly above —
    /// the two share half a name and nothing about the arithmetic is
    /// close, but their `aurora-app` dispatch arms are adjacent, which is
    /// where that hazard actually lives.
    ///
    /// **`ColorBurn` is *not* symmetric in `Cb`/`Cs`, and it is the first
    /// ported mode that is not.** `Multiply`, `Darken`, `Lighten`,
    /// `Screen`, `Difference`, `LinearDodge` and `LinearBurn` each
    /// disclose the opposite: their blend term cannot see a transposed
    /// `src`/`backdrop` binding at all, leaving only the asymmetric "over"
    /// to catch it. Here the blend term itself catches it —
    /// `B(Cb, Cs) != B(Cs, Cb)` in general — so a transpose is observable
    /// even at effective alpha `1.0`, which 0.107.0 confirmed by running
    /// that mutation for real at both `0.5` and `1.0`. `aurora-app`'s
    /// standing `every_gpu_blend_math_dispatch_arm_has_a_fixture_that_
    /// could_see_a_transposed_argument` guard is deliberately *not*
    /// special-cased for this: non-unit opacity is now
    /// sufficient-but-not-necessary for this one mode, and a conservative
    /// guard that still demands it costs nothing and keeps the rule
    /// uniform.
    ///
    /// **The application calls this** (0.107.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `ColorBurn` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `ColorBurn` root layer, through the *same* single ping-pong spare
    /// accumulator any other blend-math layer would use. Its dispatch arm
    /// was instrumented from day one (`GpuBlendDispatch::ColorBurn` in
    /// that crate), so no test here can be satisfied by a silent CPU
    /// fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_color_burn_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_COLOR_BURN,
        );
    }

    /// Composites `src` over `backdrop` with **`BlendMode::ColorDodge`**
    /// math computed in the shader, writing the finished result into
    /// `dst` — a *different* view from `backdrop`.
    ///
    /// The ninth blend mode ported to the GPU, and built to exactly the
    /// shape of the eight above: same `bind_group_layout_blend`, same
    /// `Blend::None` replace, same `Opacity` uniform, same
    /// `(src, backdrop, dst)` parameter order, same clamp, same
    /// caller-supplied encoder. Read
    /// [`Self::composite_multiply_over_with_opacity`]'s doc comment for
    /// all of the shared "why" — the aliasing rule, why the accumulator
    /// must arrive as a sampled texture, and why the `sa * opacity`
    /// product is deliberately left unclamped while `opacity` itself is
    /// clamped. None of it is mode-specific.
    ///
    /// **What is specific to this method.** `blend_rgb(ColorDodge, Cb, Cs)`
    /// is `blend_channel`'s own three-branch arm per channel:
    ///
    /// ```text
    /// if Cb == 0 { 0 } else if Cs == 1 { 1 }
    /// else { min(1, Cb / (1 - Cs)) }
    /// ```
    ///
    /// Two of those three branches are per-channel *conditions* rather
    /// than arithmetic, so — as with `ColorBurn`, and unlike the seven
    /// modes ported before that one — this cannot be one componentwise
    /// expression on `vec3<f32>`. The shader factors the arm into a
    /// `color_dodge_channel(cb, cs)` helper (the other of that file's two
    /// per-channel blend helpers -- second of them, though no longer that
    /// file's second non-entry-point function, since 0.109.0's
    /// `straight_backdrop()`/`fold_over()` precede both) and calls it
    /// three times to build `b`.
    ///
    /// **Both guards are arithmetically redundant under IEEE-754, both are
    /// still required, and — unlike `ColorBurn`'s — they are redundant for
    /// two *different* reasons.** Drop the `Cb == 0` guard and
    /// `0 / (1 - Cs)` is a real, well-defined `+0` for every `Cs < 1`, so
    /// `min(1, 0) = 0` is the same answer with no division-by-zero
    /// semantics involved anywhere; it changes the result only at
    /// `Cs == 1`, where the *second* guard then fires in its place and
    /// returns `1.0` instead of `0.0`. That makes deleting the first guard
    /// killable deterministically on every backend, and 0.108.0 measured
    /// exactly that. Drop the `Cs == 1` guard and `Cb / 0` is `+inf`,
    /// `min(1, inf)` is `1`, again the same answer — *if* the backend
    /// divides like IEEE. **WGSL does not promise that**: division by zero
    /// yields an indeterminate value, which may be `NaN` — and that `NaN`
    /// reaches the output, though by a *different* route than in
    /// `ColorBurn`'s otherwise-identical note above. Here a `NaN` can only
    /// arise when `Cb != 0`, since `Cb == 0` returns from the *first* guard
    /// before any division is reached; and `Cb != 0` requires `ab > 0`,
    /// because the shader forces `cb` to exactly `vec3(0, 0, 0)` on the
    /// `ab == 0` half. So it propagates through `ab * b` with `ab`
    /// **strictly positive**, directly. It is *not* the
    /// zero-fails-to-absorb-it argument `ColorBurn` makes: that mode's
    /// first guard is `Cb == 1`, which does not fire at `Cb == 0`, so on
    /// its `ab == 0` half the division is genuinely reached and genuinely
    /// multiplied by zero. The conclusion is the same either way — the
    /// second guard makes this
    /// entry point *defined* rather than merely correct on one adapter, and
    /// 0.108.0 measured its deletion surviving every test in this crate on
    /// Vulkan/NVIDIA — the disclosed, expected result of a portability
    /// guard on IEEE hardware, not a missing test. See PLAN.md's 0.108.0
    /// entry.
    ///
    /// **Branch order is load-bearing**, and it is the *mirror* of
    /// `ColorBurn`'s rather than the same. `Cb == 0` is tested first, so a
    /// fully black backdrop under a fully white source — both conditions
    /// true at once, and an ordinary pixel, not a contrived one — yields
    /// `0.0` and not `1.0`.
    ///
    /// **Deliberately not `1 - min(1, (1 - Cb) / Cs)`**, which is
    /// [`Self::composite_color_burn_over_with_opacity`] directly above:
    /// the two are the guarded-division pair, structural mirror images,
    /// and their `aurora-app` dispatch arms are *adjacent*, which is where
    /// that hazard actually lives. This method's shader was therefore
    /// derived from `blend_channel`'s own Rust arm rather than copied from
    /// `fs_composite_color_burn` and edited. And not `min(Cb + Cs, 1)`,
    /// which is `LinearDodge`, the other dodge-family mode — likewise on
    /// the GPU. That resemblance is worth a second sentence, because it is
    /// more than a shared name: `min(1, Cb / (1 - Cs))` clamps exactly when
    /// `Cb + Cs >= 1`, which is exactly when `min(Cb + Cs, 1)` clamps, so
    /// **a clamped channel can never distinguish the two**. Every claim
    /// below about discriminating this mode from `LinearDodge` is therefore
    /// a claim about at most two channels, never three.
    ///
    /// **`ColorDodge` is *not* symmetric in `Cb`/`Cs`, and it is the second
    /// ported mode that is not** (`ColorBurn`, 0.107.0, was the first).
    /// `Multiply`, `Darken`, `Lighten`, `Screen`, `Difference`,
    /// `LinearDodge` and `LinearBurn` each disclose the opposite: their
    /// blend term cannot see a transposed `src`/`backdrop` binding at all,
    /// leaving only the asymmetric "over" to catch it. Here the blend term
    /// itself catches it — `B(Cb, Cs) != B(Cs, Cb)` in general — so a
    /// transpose is observable even at effective alpha `1.0`, which 0.108.0
    /// confirmed by running that mutation for real at both `0.5` and `1.0`.
    /// `aurora-app`'s standing `every_gpu_blend_math_dispatch_arm_has_a_
    /// fixture_that_could_see_a_transposed_argument` guard is deliberately
    /// *not* special-cased for either mode: non-unit opacity is now
    /// sufficient-but-not-necessary for two of the nine, and a conservative
    /// guard that still demands it costs nothing and keeps the rule
    /// uniform.
    ///
    /// **The application calls this** (0.108.0): `aurora-app`'s
    /// `document_qualifies_for_gpu_compositing` admits `ColorDodge` and
    /// `begin_gpu_composite_tile` dispatches to this method for every
    /// `ColorDodge` root layer, through the *same* single ping-pong spare
    /// accumulator any other blend-math layer would use. Its dispatch arm
    /// was instrumented from day one (`GpuBlendDispatch::ColorDodge` in
    /// that crate), so no test here can be satisfied by a silent CPU
    /// fallback.
    ///
    /// All three views must be `Rgba16Float` and the same size; `dst`'s
    /// owning texture must include `RENDER_ATTACHMENT` usage, and both
    /// `src`'s and `backdrop`'s must include `TEXTURE_BINDING`.
    /// `opacity` is clamped to `0.0..=1.0`.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for the full account.
    pub fn composite_color_dodge_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            encoder,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_COLOR_DODGE,
        );
    }

    /// The one body behind every shader-computed blend mode: build (or
    /// reuse) `pass.fragment_entry`'s pipeline, upload the clamped
    /// opacity, bind `src`/backdrop/uniform, and draw one fullscreen
    /// triangle into `dst`.
    ///
    /// **Every "why" lives on
    /// [`Self::composite_multiply_over_with_opacity`]** — the aliasing
    /// rule (`dst` must not be `backdrop`), the `Blend::None` replace,
    /// the `(src, backdrop, dst)` parameter order, the clamped `opacity`
    /// against the deliberately unclamped `sa * opacity` product, the
    /// `Rgba16Float`/usage requirements, and the inherited `wgpu`
    /// validation-panic gap. None of it is mode-specific, which is
    /// exactly why this method exists.
    ///
    /// **A pure extraction (0.85.1), not a redesign.** It is field for
    /// field what `composite_multiply_over_with_opacity` and
    /// `composite_darken_over_with_opacity` each built inline at 0.85.0,
    /// with the six values that differed lifted into [`BlendPass`] —
    /// six as of that extraction; **five now**, since 0.86.0 deleted the
    /// per-mode encoder label along with the encoders those methods used
    /// to create (see [`BlendPass`]'s own doc comment, which states the
    /// live count). The "six" here is deliberately left as the
    /// historical number this extraction actually moved, so a reader
    /// comparing it against the struct's five fields is not left
    /// guessing which one is wrong —
    /// and it took `composite_lighten_over_with_opacity` (0.95.0, the
    /// third caller), `composite_screen_over_with_opacity` (0.102.0,
    /// the fourth), `composite_difference_over_with_opacity` (0.104.0,
    /// the fifth), `composite_linear_dodge_over_with_opacity` (0.105.0,
    /// the sixth), `composite_linear_burn_over_with_opacity` (0.106.0,
    /// the seventh), `composite_color_burn_over_with_opacity` (0.107.0,
    /// the eighth) and `composite_color_dodge_over_with_opacity` (0.108.0,
    /// the ninth) without a line of change, which is the bet that
    /// extraction was making — the
    /// same discipline, and the same "no existing test needed to change"
    /// bar, 0.83.1 used when it extracted [`composite_pipeline`] and
    /// [`opacity_uniform_buffer`] out from under those same callers. The
    /// `composite_multiply_*`, `composite_darken_*`,
    /// `composite_lighten_*`, `composite_screen_*`,
    /// `composite_difference_*`, `composite_linear_dodge_*`,
    /// `composite_linear_burn_*`, `composite_color_burn_*` and
    /// `composite_color_dodge_*`
    /// differentials in
    /// this module's tests, each checking the shader's output against
    /// [`composite_tile_cpu`]'s own on real hardware, are what makes that
    /// checkable rather than asserted.
    ///
    /// **Private, and staying private.** A caller outside this crate
    /// picks a mode by picking a method; handing it a [`BlendPass`]
    /// would let it name a `fragment_entry` that does not exist (a
    /// pipeline-creation failure, not a compile error) or pair an entry
    /// point with the wrong labels.
    ///
    /// **Records into `encoder`; does not submit** (0.86.0) — see
    /// [`Self::composite_over_with_opacity`] for what that means and why
    /// it changed.
    // `too_many_arguments`: eight, against clippy's own seven-argument
    // limit. The eighth slot is the caller-supplied `encoder` that
    // replaced this method's own internal `create_command_encoder` +
    // `queue.submit` pair in 0.86.0, which is the whole point of the
    // change: one encoder and one submit per composite tile instead of
    // one per pass. Dropping a parameter to get back under the limit
    // would mean either re-creating an encoder here (undoing the change)
    // or bundling `src`/`backdrop`/`dst` into a struct whose only
    // purpose is to satisfy a lint.
    #[allow(clippy::too_many_arguments)]
    fn composite_blend_over_with_opacity(
        &mut self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
        blend_pass: &BlendPass,
    ) {
        let device = context.device();
        let opacity = opacity.clamp(0.0, 1.0);
        let key = PipelineKey {
            shader: LABEL,
            vertex_entry: "vs_composite",
            fragment_entry: blend_pass.fragment_entry,
            target_format: wgpu::TextureFormat::Rgba16Float,
            // The shader computes the whole composite itself, so the
            // fixed-function unit must do nothing at all -- a plain
            // replace. This is the one thing that makes real blend-mode
            // math expressible on the GPU.
            blend: Blend::None,
        };
        let layout = &self.bind_group_layout_blend;
        let shader = &self.shader;
        let pipeline = self.pipelines.get_or_create_with(key.clone(), || {
            composite_pipeline(device, shader, &key, layout, blend_pass.pipeline)
        });

        // The same `Opacity` upload `composite_over_with_opacity` does,
        // through the same helper — see it for why the 12 bytes of
        // padding are three scalars rather than a `vec3<f32>`.
        let uniform_buffer =
            opacity_uniform_buffer(device, context.queue(), opacity, blend_pass.uniform);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(blend_pass.bind_group),
            layout: &self.bind_group_layout_blend,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(backdrop),
                },
            ],
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(blend_pass.pass),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Not `Load`: `dst` is a separate destination,
                        // not the accumulator, and the fullscreen
                        // triangle replaces every texel anyway.
                        //
                        // This is also why `aurora-app`'s "one shared
                        // spare accumulator, not one per mode" property
                        // is a memory-footprint claim and not a
                        // pixel-observable one: whatever `dst` held
                        // before this pass is discarded here, so giving
                        // each mode its own `dst` texture would produce
                        // byte-identical output. Only `backdrop`, which
                        // is sampled, carries the prior fold.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
    use super::{
        BlendMode, CHUNK_SAMPLES, CHUNK_TEXELS, TileCompositor, blend_channel, blend_color,
        blend_darker_color, blend_hue, blend_lighter_color, blend_luminosity, blend_rgb,
        blend_saturation, clip_color, composite_layer_into, composite_tile_cpu, lum, sat, set_lum,
        set_sat, soft_light_d, transparent_tile, un_premultiply_in_place,
    };
    use crate::test_support::real_context;
    use aurora_tile::{CHANNELS, SAMPLES, TILE};
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

    /// The real point of `un_premultiply_in_place`: a fractional-alpha
    /// accumulator holds premultiplied colour, and straightening it
    /// recovers the true colour. `(0.5, 0.5, 0.5, 0.5)` is exactly what
    /// `composite_tile_cpu` leaves for a lone opaque-white layer at 50%
    /// opacity (see
    /// `composite_tile_cpu_applies_layer_opacity_on_top_of_the_texels_own_alpha`,
    /// which asserts that value and must keep doing so — this function
    /// is the *caller's* step, not a change to that contract).
    #[test]
    fn un_premultiply_in_place_straightens_a_fractional_alpha_texel() {
        let mut texels = solid_texels([0.5, 0.5, 0.5, 0.5]);
        un_premultiply_in_place(&mut texels);
        assert_eq!(first_texel(&texels), (1.0, 1.0, 1.0, 0.5));
    }

    /// At `a == 1.0` the division is by one, so this is an identity
    /// operation — which is exactly why every existing fully-opaque
    /// fixture in this workspace stays bit-identical across this change,
    /// and why an opaque-only test can never catch the premultiplied-
    /// alpha gap this function closes.
    #[test]
    fn un_premultiply_in_place_leaves_a_fully_opaque_texel_unchanged() {
        let mut texels = solid_texels([0.25, 0.5, 0.75, 1.0]);
        un_premultiply_in_place(&mut texels);
        assert_eq!(first_texel(&texels), (0.25, 0.5, 0.75, 1.0));
    }

    /// The divide-by-zero guard: a fully transparent texel has no
    /// meaningful colour to recover, so its colour channels are zeroed
    /// rather than producing `inf`/`NaN`.
    #[test]
    fn un_premultiply_in_place_zeroes_the_colour_of_a_fully_transparent_texel() {
        let mut texels = solid_texels([0.4, 0.6, 0.8, 0.0]);
        un_premultiply_in_place(&mut texels);
        assert_eq!(first_texel(&texels), (0.0, 0.0, 0.0, 0.0));
    }

    /// **A premise `aurora-app` depends on, pinned in the crate that owns
    /// the function.** `transparent_tile` is spelled
    /// `vec![f16::from_f32(0.0); SAMPLES]`, and `f16::from_f32(0.0)` is
    /// canonical *positive* zero — bit pattern `0x0000`, never `-0.0`
    /// (`0x8000`), a subnormal, or a `NaN`. As of 0.94.0
    /// `aurora-app`'s `composite_roots_into_tile` skips
    /// [`un_premultiply_in_place`] entirely for a tile no root folded
    /// into, and its argument that the skip is *output-identical* rather
    /// than an approximation starts here: the buffer it skips over is
    /// this one, untouched.
    ///
    /// Should someone change `transparent_tile` to seed anything else —
    /// `-0.0`, or a sentinel — this test fails here, in this crate,
    /// rather than silently changing what a caller two crates up writes
    /// into the composite surface.
    #[test]
    fn transparent_tile_is_all_canonical_positive_zero_bits() {
        let tile = transparent_tile();
        assert_eq!(tile.len(), SAMPLES);
        assert!(
            tile.iter().all(|sample| sample.to_bits() == 0),
            "transparent_tile must be all canonical +0.0 bits"
        );
    }

    /// **The other half of the same premise**, and the reason
    /// `aurora-app`'s 0.94.0 skip removes *work* rather than changing a
    /// result: run on an all-`0x0000` buffer, this function is a bitwise
    /// identity. Every texel's `alpha` is `0.0`, so `if alpha > 0.0` is
    /// false for all of them, the else-arm writes the identical `0x0000`
    /// back into `r`/`g`/`b`, and `a` is never assigned at all.
    ///
    /// Note this is a strictly stronger claim than
    /// `un_premultiply_in_place_zeroes_the_colour_of_a_fully_transparent_texel`
    /// above, which starts from *coloured* transparent texels and so only
    /// shows the output is zero — not that the input was already the
    /// output. The skip needs the identity, so the identity is what is
    /// asserted, over a whole real-length buffer and on bits rather than
    /// on `f32` values (`-0.0 == 0.0` would let a sign-bit change pass a
    /// value comparison).
    ///
    /// A future change to the zero-alpha arm — writing a sentinel,
    /// normalizing `a`, anything at all — must therefore fail *here*,
    /// which is the point of the test living in this crate.
    ///
    /// **Do not read this more broadly than it is written** (0.94.1). The
    /// claim is about [`transparent_tile`]'s own canonical all-`0x0000`
    /// output, which is the only buffer `aurora-app`'s skip is ever applied
    /// to — *not* "this function is a bitwise identity on any buffer whose
    /// alpha is zero", which is false. Give a texel `r = -0.0` (bit pattern
    /// `0x8000`) alongside `a = +0.0` and the else-arm canonicalizes the
    /// sign bit away, so the bits change while the values do not. That is
    /// why the test starts from `transparent_tile()` itself rather than a
    /// synthetic zero-alpha stand-in.
    #[test]
    fn un_premultiply_in_place_is_a_bitwise_identity_on_a_transparent_tile() {
        let before = transparent_tile();
        let mut after = transparent_tile();
        un_premultiply_in_place(&mut after);
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(b, a)| b.to_bits() == a.to_bits()),
            "un_premultiply_in_place must be a bitwise identity on an all-zero buffer"
        );
    }

    /// The *other* divide-by-alpha hazard, and the one the `a == 0.0`
    /// guard above does not cover: an alpha that is nonzero but far
    /// smaller than the colour channels it divides.
    ///
    /// `f16`'s smallest positive subnormal is ~`5.96e-8`, so a
    /// premultiplied texel can legitimately hold an alpha of `5e-5`
    /// alongside a colour channel at `f16`'s own `65504.0` ceiling (HDR
    /// or crafted content; ordinary SDR painting never gets there).
    /// The exact quotient is then ~`1.3e9`, roughly twenty thousand
    /// times what an `f16` can represent, and an unclamped
    /// `f16::from_f32` turns it into `inf` — which then travels silently
    /// into an exported PNG/TIFF, the eyedropper, and the canvas atlas
    /// as corrupt data rather than failing loudly.
    ///
    /// The assertion is therefore two-part: the result must be *finite*,
    /// and it must be the saturated `65504.0` rather than the true (and
    /// unrepresentable) mathematical quotient. Saturating is also what
    /// the GPU already did for the same inputs — a fixed-function
    /// `Rgba16Float` render target saturates rather than overflowing —
    /// so this makes the CPU agree with the hardware rather than
    /// inventing a third behaviour.
    #[test]
    fn un_premultiply_in_place_saturates_rather_than_overflowing_at_a_tiny_alpha() {
        let mut texels = solid_texels([65504.0, 65504.0, 65504.0, 5e-5]);
        un_premultiply_in_place(&mut texels);
        let (r, g, b, a) = first_texel(&texels);
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite(),
            "straightening must never produce an infinity: got ({r}, {g}, {b}, {a})"
        );
        assert_eq!(
            (r, g, b),
            (65504.0, 65504.0, 65504.0),
            "the quotient is ~1.3e9, far outside f16's range, so it must clamp to f16::MAX"
        );
        assert!(a > 0.0, "the alpha channel itself is left untouched");
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
    // Regression test for the bug named in `composite_tile_cpu`'s own doc
    // comment ("The accumulator's own backdrop colour, recovered before
    // blending") and, before this fix, in `aurora-app`'s `resolve_tile`
    // doc comment and three `PLAN.md` M1.9 entries as "the non-`Normal`
    // blend-mode-against-each-other math inside a translucent group
    // isolation pass" -- genuinely open until now. Every other blend-mode
    // test in this module (including the `Multiply` test just above)
    // seeds an *opaque* bottom layer first, so `backdrop_alpha == 1.0`
    // when the second layer's blend mode runs and the accumulator's raw
    // state already equals its true straight colour -- this test is the
    // one exception, seeding a *translucent* bottom layer instead, so the
    // accumulator itself is still premultiplied when `Multiply` reacts to
    // it.
    //
    // Layer 1 (bottom, `Normal`, opacity 0.5): straight colour
    // (1.0, 0.5, 0.25). Composited onto the starting fully-transparent
    // accumulator (backdrop_alpha = 0.0, so `blend_rgb` returns Cs
    // unchanged): alpha = 1.0*0.5 = 0.5, dr = 0.5*1.0 = 0.5,
    // dg = 0.5*0.5 = 0.25, db = 0.5*0.25 = 0.125, da = 0.5 -- a
    // *premultiplied* accumulator, (0.5, 0.25, 0.125, 0.5). Its true
    // straight-alpha colour, recovered by dividing by da = 0.5, is
    // (1.0, 0.5, 0.25) -- exactly Layer 1's own source colour, as it must
    // be (a lone layer's own colour is never altered by compositing over
    // nothing).
    //
    // Layer 2 (top, `Multiply`, opacity 1.0, fully opaque): straight
    // colour (0.5, 0.5, 0.75). `Multiply` is `Cb * Cs` per channel; with
    // the *correct*, recovered Cb = (1.0, 0.5, 0.25):
    // (1.0*0.5, 0.5*0.5, 0.25*0.75) = (0.5, 0.25, 0.1875). Layer 2 is
    // fully opaque (alpha = 1.0, inverse = 0.0), so the general "over"
    // formula collapses to
    // `Co = (1 - backdrop_alpha)*Cs + backdrop_alpha*B(Cb,Cs)`:
    //   R: 0.5*0.5   + 0.5*0.5    = 0.5
    //   G: 0.5*0.5   + 0.5*0.25   = 0.375
    //   B: 0.5*0.75  + 0.5*0.1875 = 0.46875
    // da = alpha + da*inverse = 1.0 + 0.5*0.0 = 1.0. Correct result:
    // (0.5, 0.375, 0.46875, 1.0).
    //
    // The pre-fix bug used the *raw*, still-premultiplied accumulator,
    // (0.5, 0.25, 0.125), as Cb directly instead: `Multiply` gives
    // (0.5*0.5, 0.25*0.5, 0.125*0.75) = (0.25, 0.125, 0.09375), and the
    // same "over" collapse gives
    //   R: 0.5*0.5  + 0.5*0.25    = 0.375
    //   G: 0.5*0.5  + 0.5*0.125   = 0.3125
    //   B: 0.5*0.75 + 0.5*0.09375 = 0.421875
    // -- (0.375, 0.3125, 0.421875, 1.0), silently wrong in every channel.
    fn composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator()
     {
        let bottom = solid_texels([1.0, 0.5, 0.25, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 0.5, BlendMode::Normal),
            (&top, 1.0, BlendMode::Multiply),
        ]);
        assert_eq!(
            first_texel(&out),
            (0.5, 0.375, 0.46875, 1.0),
            "Multiply against a still-translucent accumulator must use its true \
             straight-alpha colour as Cb, not its raw premultiplied state"
        );
        assert_ne!(
            first_texel(&out),
            (0.375, 0.3125, 0.421_875, 1.0),
            "this is the pre-fix value: Multiply run directly against the raw \
             premultiplied accumulator instead of its recovered straight colour"
        );
    }

    #[test]
    // A consistency smoke test, and deliberately *not* a proof of
    // anything. `composite_tile_cpu` is now literally defined as
    // `transparent_tile()` plus one `composite_layer_into` call per
    // layer, so both sides of the assertion below are the same three
    // calls on the same data: this test cannot fail while that
    // definition holds, no matter what the underlying math does. An
    // earlier version of this comment called it "the load-bearing
    // proof" that the split was pure code motion, which was a
    // tautology -- review 2026-08-24 caught it, and the assertion that
    // actually pins the math is
    // `composite_layer_into_folded_matches_hand_computed_golden_values`
    // below.
    //
    // What it is still worth keeping for: it fails loudly if some
    // future edit reintroduces a bespoke per-layer loop into
    // `composite_tile_cpu` -- exactly the shape this change removed --
    // and lets it drift from the fold. `f16` is `PartialEq`, so this is
    // an exact whole-buffer comparison with no epsilon.
    fn composite_layer_into_folded_one_at_a_time_matches_the_batch_composite() {
        let bottom = solid_texels([1.0, 0.5, 0.25, 1.0]);
        let middle = solid_texels([0.5, 0.5, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.75, 1.0, 0.5]);
        let batched = composite_tile_cpu(&[
            (&bottom, 0.5, BlendMode::Normal),
            (&middle, 1.0, BlendMode::Multiply),
            (&top, 0.75, BlendMode::Screen),
        ]);

        let mut folded = transparent_tile();
        composite_layer_into(&mut folded, &bottom, 0.5, BlendMode::Normal);
        composite_layer_into(&mut folded, &middle, 1.0, BlendMode::Multiply);
        composite_layer_into(&mut folded, &top, 0.75, BlendMode::Screen);

        assert_eq!(
            folded, batched,
            "folding one layer at a time must be bit-identical to the batch composite \
             over the same layers in the same order"
        );
    }

    #[test]
    // The real, non-vacuous assertion that folding layers in one at a
    // time computes the documented formula: a fixed three-layer stack
    // pinned to expected values derived **by hand from
    // `composite_layer_into`'s own doc comment**, not from any call this
    // module makes. Nothing here reads back from `composite_tile_cpu`,
    // so unlike the smoke test above this one fails if the per-layer
    // math is wrong, whatever the two functions' relationship to each
    // other happens to be.
    //
    // Every value below is exactly representable in `f16` (each is a
    // dyadic rational with at most ten mantissa bits), so each stage's
    // stored result is exact and no rounding accumulates across the
    // three folds -- the same reasoning
    // `composite_tile_cpu_a_single_layer_at_full_opacity_reproduces_it_over_transparent`
    // already documents for its own exact-literal comparison.
    //
    // Fold 1 -- bottom (1.0, 0.5, 0.25, 1.0), opacity 0.5, `Normal`,
    // over the fully transparent start. `as = 1.0 * 0.5 = 0.5`,
    // `ab = 0.0` so `blend_rgb` contributes nothing and the bracket
    // collapses to `Cs`:
    //   R: 0.5*0.0 + 0.5*1.0  = 0.5
    //   G: 0.5*0.0 + 0.5*0.5  = 0.25
    //   B: 0.5*0.0 + 0.5*0.25 = 0.125
    //   A: 0.5 + 0.0*0.5      = 0.5
    // -> (0.5, 0.25, 0.125, 0.5), a *premultiplied* accumulator.
    //
    // Fold 2 -- middle (0.5, 0.5, 0.75, 1.0), opacity 1.0, `Multiply`.
    // `as = 1.0`, `ab = 0.5`. Backdrop recovered by dividing by `ab`:
    // (1.0, 0.5, 0.25). `Multiply` is `Cb*Cs`:
    // (0.5, 0.25, 0.1875). With `as = 1.0` the outer mix keeps only the
    // bracket, `(1-ab)*Cs + ab*B`:
    //   R: 0.5*0.5  + 0.5*0.5    = 0.5
    //   G: 0.5*0.5  + 0.5*0.25   = 0.375
    //   B: 0.5*0.75 + 0.5*0.1875 = 0.46875
    //   A: 1.0 + 0.5*0.0         = 1.0
    // -> (0.5, 0.375, 0.46875, 1.0).
    //
    // Fold 3 -- top (0.25, 0.75, 1.0, 0.5), opacity 0.75, `Screen`.
    // `as = 0.5 * 0.75 = 0.375`, `ab = 1.0`, so the backdrop is already
    // straight and the bracket keeps only `B`. `Screen` is
    // `Cb + Cs - Cb*Cs`:
    //   R: 0.5     + 0.25 - 0.125     = 0.625
    //   G: 0.375   + 0.75 - 0.28125   = 0.84375
    //   B: 0.46875 + 1.0  - 0.46875   = 1.0
    // then `Co = (1-as)*Cb + as*B`:
    //   R: 0.625*0.5     + 0.375*0.625   = 0.546875
    //   G: 0.625*0.375   + 0.375*0.84375 = 0.55078125
    //   B: 0.625*0.46875 + 0.375*1.0     = 0.66796875
    //   A: 0.375 + 1.0*0.625             = 1.0
    fn composite_layer_into_folded_matches_hand_computed_golden_values() {
        let bottom = solid_texels([1.0, 0.5, 0.25, 1.0]);
        let middle = solid_texels([0.5, 0.5, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.75, 1.0, 0.5]);

        let mut folded = transparent_tile();
        assert_eq!(
            first_texel(&folded),
            (0.0, 0.0, 0.0, 0.0),
            "an accumulation starts from fully transparent black"
        );

        composite_layer_into(&mut folded, &bottom, 0.5, BlendMode::Normal);
        assert_eq!(
            first_texel(&folded),
            (0.5, 0.25, 0.125, 0.5),
            "fold 1: a half-opacity Normal layer over transparent leaves a \
             premultiplied accumulator"
        );

        composite_layer_into(&mut folded, &middle, 1.0, BlendMode::Multiply);
        assert_eq!(
            first_texel(&folded),
            (0.5, 0.375, 0.46875, 1.0),
            "fold 2: Multiply must run against the *recovered* straight backdrop \
             (1.0, 0.5, 0.25), not the raw premultiplied (0.5, 0.25, 0.125)"
        );

        composite_layer_into(&mut folded, &top, 0.75, BlendMode::Screen);
        assert_eq!(
            first_texel(&folded),
            (0.546_875, 0.550_781_25, 0.667_968_75, 1.0),
            "fold 3: Screen over the now-opaque accumulator, at an effective \
             alpha of 0.5 * 0.75 = 0.375"
        );
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

    // -- Blend-mode math: the 7 "overlay and light" modes added this
    // round (`Overlay`, `SoftLight`, `HardLight`, `VividLight`,
    // `LinearLight`, `PinLight`, `HardMix`), reusing the "simple
    // separable" and "dodge and burn" families' own already-tested arms
    // above wherever the spec formula decomposes that way (`HardLight`
    // reuses `Multiply`/`Screen`, `Overlay` reuses `HardLight` itself,
    // `VividLight` reuses `ColorBurn`/`ColorDodge`, `PinLight` reuses
    // `Darken`/`Lighten`, `HardMix` reuses `VividLight`) -- only
    // `SoftLight` has genuinely new per-mode math. All literals below are
    // exact eighths/sixteenths (like the prior two rounds' own eighths/
    // quarters, never decimal tenths), so both the constructed input
    // texels and the hand-computed expected outputs round-trip bit-exact
    // through `f16`.

    #[test]
    // HardLight (W3C spec), Cs <= 0.5 branch: B = Multiply(Cb, 2*Cs).
    // Backdrop 0.75, source 0.25 (Cs <= 0.5) -> Multiply(0.75, 2*0.25) =
    // Multiply(0.75, 0.5) = 0.375.
    fn composite_tile_cpu_hard_light_uses_multiply_when_the_source_is_at_or_below_half() {
        let bottom = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::HardLight),
        ]);
        assert_eq!(first_texel(&out), (0.375, 0.375, 0.375, 1.0));
    }

    #[test]
    // HardLight, Cs > 0.5 branch: B = Screen(Cb, 2*Cs - 1). Backdrop
    // 0.25, source 0.75 (Cs > 0.5) -> Screen(0.25, 2*0.75-1) =
    // Screen(0.25, 0.5) = 0.25 + 0.5 - 0.25*0.5 = 0.75 - 0.125 = 0.625.
    fn composite_tile_cpu_hard_light_uses_screen_when_the_source_is_above_half() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::HardLight),
        ]);
        assert_eq!(first_texel(&out), (0.625, 0.625, 0.625, 1.0));
    }

    #[test]
    // Overlay, Cb <= 0.5 branch: B = 2*Cb*Cs. Backdrop 0.25 (Cb <= 0.5),
    // source 0.75 -> 2*0.25*0.75 = 0.375.
    fn composite_tile_cpu_overlay_uses_the_direct_multiply_form_when_the_backdrop_is_at_or_below_half()
     {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Overlay),
        ]);
        assert_eq!(first_texel(&out), (0.375, 0.375, 0.375, 1.0));
    }

    #[test]
    // Overlay, Cb > 0.5 branch: B = 1 - 2*(1-Cb)*(1-Cs). Backdrop 0.75
    // (Cb > 0.5), source 0.25 -> 1 - 2*0.25*0.75 = 1 - 0.375 = 0.625.
    fn composite_tile_cpu_overlay_uses_the_inverse_screen_form_when_the_backdrop_is_above_half() {
        let bottom = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Overlay),
        ]);
        assert_eq!(first_texel(&out), (0.625, 0.625, 0.625, 1.0));
    }

    #[test]
    // The stated relationship, checked directly rather than just claimed
    // in a doc comment: `Overlay(Cb, Cs) == HardLight(Cs, Cb)`, i.e.
    // swapping which layer is "backdrop" and which is "source" between
    // the two modes must produce the same result. Backdrop 0.625, source
    // 0.375 through Overlay (Cb=0.625 > 0.5, so the inverse-screen
    // branch fires): 1 - 2*(1-0.625)*(1-0.375) = 1 - 2*0.375*0.625 =
    // 1 - 0.46875 = 0.53125. The mirrored document -- backdrop 0.375,
    // source 0.625 through HardLight (Cs=0.625 > 0.5, so its own
    // Screen branch fires): Screen(0.375, 2*0.625-1) = Screen(0.375,
    // 0.25) = 0.375 + 0.25 - 0.375*0.25 = 0.625 - 0.09375 = 0.53125 --
    // the same value, confirming the two computations (not just the two
    // formulas on paper) genuinely agree.
    fn composite_tile_cpu_overlay_and_hard_light_agree_when_their_arguments_are_swapped() {
        let overlay_bottom = solid_texels([0.625, 0.625, 0.625, 1.0]);
        let overlay_top = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let overlay_out = composite_tile_cpu(&[
            (&overlay_bottom, 1.0, BlendMode::Normal),
            (&overlay_top, 1.0, BlendMode::Overlay),
        ]);

        let hard_light_bottom = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let hard_light_top = solid_texels([0.625, 0.625, 0.625, 1.0]);
        let hard_light_out = composite_tile_cpu(&[
            (&hard_light_bottom, 1.0, BlendMode::Normal),
            (&hard_light_top, 1.0, BlendMode::HardLight),
        ]);

        assert_eq!(first_texel(&overlay_out), (0.53125, 0.53125, 0.53125, 1.0));
        assert_eq!(
            first_texel(&overlay_out),
            first_texel(&hard_light_out),
            "Overlay(Cb, Cs) and HardLight(Cs, Cb) must agree exactly, not just approximately"
        );
    }

    #[test]
    // VividLight, Cs <= 0.5 branch: B = ColorBurn(Cb, 2*Cs). Backdrop
    // 0.875, source 0.25 (Cs <= 0.5) -> ColorBurn(0.875, 0.5) =
    // 1 - min(1, (1-0.875)/0.5) = 1 - min(1, 0.25) = 0.75 -- the same
    // ColorBurn inputs/output this file's own
    // `composite_tile_cpu_color_burn_computes_the_clamped_per_channel_ratio`
    // proves in isolation, reused here through VividLight's own
    // composition rather than re-derived.
    fn composite_tile_cpu_vivid_light_uses_color_burn_when_the_source_is_at_or_below_half() {
        let bottom = solid_texels([0.875, 0.875, 0.875, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::VividLight),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // VividLight, Cs > 0.5 branch: B = ColorDodge(Cb, 2*Cs - 1). Backdrop
    // 0.375, source 0.75 (Cs > 0.5) -> ColorDodge(0.375, 0.5) =
    // min(1, 0.375/0.5) = 0.75 -- the same ColorDodge inputs/output this
    // file's own `composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio`
    // proves in isolation, reused here through VividLight's own
    // composition rather than re-derived.
    fn composite_tile_cpu_vivid_light_uses_color_dodge_when_the_source_is_above_half() {
        let bottom = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::VividLight),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // LinearLight via the simplified single-expression form:
    // clamp(Cb + 2*Cs - 1, 0, 1). Backdrop 0.5, source 0.625 (Cs > 0.5,
    // so the branch form would use LinearDodge) -> 0.5 + 1.25 - 1 = 0.75,
    // comfortably inside the clamp on both sides -- a genuine non-
    // boundary result, not one where the clamp is doing the work (that's
    // what the dedicated equivalence test below is for).
    fn composite_tile_cpu_linear_light_computes_the_clamped_sum() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([0.625, 0.625, 0.625, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::LinearLight),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // Proves the algebraic equivalence claim in `blend_channel`'s own
    // `LinearLight` arm numerically rather than just asserting it in a
    // doc comment: for each `(cb, cs)` pair below, computes the
    // branch-form value independently right here in the test (not by
    // calling the implementation) and compares it against
    // `blend_channel`'s actual (simplified single-expression) output.
    // Covers both branches (`cs <= 0.5` and `cs > 0.5`) and the `cs ==
    // 0.5` boundary itself, plus cases on each side of both the lower
    // and upper clamp.
    //
    // Both forms here are exact dyadic-rational arithmetic (add,
    // multiply by 2, subtract 1, clamp) over the same eighths-only
    // inputs `blend_channel` itself is proven against elsewhere in this
    // file, so an exact `f32` equality is the right check, not a
    // rounding-tolerant one -- the same reasoning `blend_channel`'s own
    // `float_cmp` allow already documents.
    #[allow(clippy::float_cmp)]
    fn linear_light_simplified_form_matches_the_branch_form_for_several_inputs() {
        let cases: [(f32, f32); 6] = [
            (0.25, 0.25), // cs <= 0.5 branch, clamps down to 0
            (0.75, 0.75), // cs > 0.5 branch, clamps up to 1
            (0.5, 0.5),   // the cs == 0.5 boundary itself
            (0.125, 0.875),
            (0.875, 0.125),
            (0.375, 0.625),
        ];
        for (cb, cs) in cases {
            let branch_form = if cs <= 0.5 {
                (cb + 2.0 * cs - 1.0).max(0.0)
            } else {
                (cb + 2.0 * cs - 1.0).min(1.0)
            };
            let simplified_form = blend_channel(BlendMode::LinearLight, cb, cs);
            assert_eq!(
                branch_form, simplified_form,
                "branch and simplified forms must agree exactly for cb={cb}, cs={cs}"
            );
        }
    }

    #[test]
    // PinLight, Cs <= 0.5 branch: B = Darken(Cb, 2*Cs) = min(Cb, 2*Cs).
    // Backdrop 0.375, source 0.125 (Cs <= 0.5) -> min(0.375, 0.25) =
    // 0.25.
    fn composite_tile_cpu_pin_light_uses_darken_when_the_source_is_at_or_below_half() {
        let bottom = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let top = solid_texels([0.125, 0.125, 0.125, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::PinLight),
        ]);
        assert_eq!(first_texel(&out), (0.25, 0.25, 0.25, 1.0));
    }

    #[test]
    // PinLight, Cs > 0.5 branch: B = Lighten(Cb, 2*Cs - 1) = max(Cb,
    // 2*Cs - 1). Backdrop 0.625, source 0.875 (Cs > 0.5) ->
    // max(0.625, 0.75) = 0.75.
    fn composite_tile_cpu_pin_light_uses_lighten_when_the_source_is_above_half() {
        let bottom = solid_texels([0.625, 0.625, 0.625, 1.0]);
        let top = solid_texels([0.875, 0.875, 0.875, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::PinLight),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    #[test]
    // HardMix's own defining property: its result is *always* exactly
    // `0.0` or `1.0`, never an intermediate value, since it thresholds
    // VividLight's own continuous result at `0.5`. Exercises six pairs
    // via `blend_channel` directly (no texel/f16 round-trip needed here,
    // since the property under test -- "the output is one of exactly two
    // values" -- holds in plain `f32` and doesn't depend on `f16`
    // storage), including two pairs (`(0.6, 0.39)` and `(0.6, 0.41)`)
    // deliberately chosen so VividLight's own intermediate result lands
    // just below and just above `0.5`: VividLight(0.6, 0.39) uses the
    // `Cs <= 0.5` branch, `ColorBurn(0.6, 0.78) = 1 - min(1, 0.4/0.78)`
    // ~= 1 - 0.513 = 0.487 (just under 0.5, so HardMix must floor to
    // `0.0`); VividLight(0.6, 0.41) uses the same branch,
    // `ColorBurn(0.6, 0.82) = 1 - min(1, 0.4/0.82)` ~= 1 - 0.488 = 0.512
    // (just over 0.5, so HardMix must ceiling to `1.0`) -- proving the
    // threshold is a genuine hard cut, not a value that merely tends
    // toward the extremes.
    //
    // `HardMix`'s own arm returns the literal `0.0`/`1.0` constants, not
    // an accumulated-rounding-error value, so an exact comparison is the
    // right check here -- the same reasoning `blend_channel`'s own
    // `float_cmp` allow already documents.
    #[allow(clippy::float_cmp)]
    fn hard_mix_produces_only_pure_black_or_white() {
        let cases: [((f32, f32), f32); 6] = [
            ((0.875, 0.25), 1.0), // VividLight = 0.75
            ((0.25, 0.25), 0.0),  // VividLight = 0.0
            ((0.6, 0.39), 0.0),   // VividLight just under 0.5
            ((0.6, 0.41), 1.0),   // VividLight just over 0.5
            ((0.3, 0.9), 1.0),    // VividLight's ColorDodge branch, = 1.0
            ((0.1, 0.6), 0.0),    // VividLight's ColorDodge branch, < 0.5
        ];
        for ((cb, cs), expected) in cases {
            let result = blend_channel(BlendMode::HardMix, cb, cs);
            assert!(
                result == 0.0 || result == 1.0,
                "HardMix({cb}, {cs}) = {result} must be exactly 0.0 or 1.0, never an intermediate value"
            );
            assert_eq!(
                result, expected,
                "HardMix({cb}, {cs}) landed on the wrong side of the threshold"
            );
        }
    }

    #[test]
    // SoftLight (W3C spec), Cs <= 0.5 branch: B = Cb - (1-2*Cs)*Cb*(1-Cb).
    // Backdrop 0.5, source 0.25 (Cs <= 0.5) -> 0.5 - (1-0.5)*0.5*0.5 =
    // 0.5 - 0.5*0.25 = 0.5 - 0.125 = 0.375.
    fn composite_tile_cpu_soft_light_darkens_when_the_source_is_at_or_below_half() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::SoftLight),
        ]);
        assert_eq!(first_texel(&out), (0.375, 0.375, 0.375, 1.0));
    }

    #[test]
    // SoftLight, Cs > 0.5 branch: B = Cb + (2*Cs-1)*(D(Cb)-Cb), where
    // `D` is `soft_light_d`'s own polynomial branch (Cb = 0.0625 <=
    // 0.25). D(0.0625): x=1/16, 16x=1, 16x-12=-11, (16x-12)*x=-11/16,
    // +4 = 53/16, *x = 53/256 = 0.20703125. Backdrop 0.0625, source 0.75
    // (Cs > 0.5): 2*Cs-1 = 0.5; D(Cb)-Cb = 53/256 - 16/256 = 37/256;
    // 0.5 * 37/256 = 37/512; B = 1/16 + 37/512 = 32/512 + 37/512 =
    // 69/512 = 0.134765625 -- a dyadic rational (denominator a power of
    // two) chosen deliberately so the exact result round-trips bit-exact
    // through `f16`, the same discipline every other hand-computed test
    // in this file already follows.
    fn composite_tile_cpu_soft_light_lightens_via_the_d_helper_when_the_source_is_above_half() {
        let bottom = solid_texels([0.0625, 0.0625, 0.0625, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::SoftLight),
        ]);
        assert_eq!(
            first_texel(&out),
            (0.134_765_63, 0.134_765_63, 0.134_765_63, 1.0)
        );
    }

    #[test]
    // SoftLight with the backdrop exactly at `soft_light_d`'s own
    // `x = 0.25` branch boundary (Cs > 0.5, so `D` is actually invoked):
    // D(0.25) = 0.5 (proven exactly, both branches, by
    // `soft_light_d_agrees_at_and_around_its_own_branch_boundary` below).
    // B = Cb + (2*Cs-1)*(D(Cb)-Cb) with Cb=0.25, Cs=0.75 ->
    // 0.25 + 0.5*(0.5-0.25) = 0.25 + 0.125 = 0.375 -- a plain, exact
    // eighth, confirming the boundary itself produces a well-defined,
    // unsurprising result through the real per-texel path, not just
    // through `soft_light_d` in isolation.
    fn composite_tile_cpu_soft_light_at_the_d_helpers_own_branch_boundary() {
        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::SoftLight),
        ]);
        assert_eq!(first_texel(&out), (0.375, 0.375, 0.375, 1.0));
    }

    #[test]
    // `soft_light_d`'s own two-branch continuity, checked directly
    // rather than only in a doc comment. At the boundary itself
    // (x = 0.25), both formulas are computed independently right here
    // (not by calling the two branches of the real implementation, which
    // only ever takes one of them) and must agree exactly: the
    // polynomial ((16*0.25-12)*0.25+4)*0.25 = ((4-12)*0.25+4)*0.25 =
    // (-2+4)*0.25 = 0.5, and sqrt(0.25) = 0.5. Also checks a pair of
    // values straddling the boundary (0.249 via the polynomial branch,
    // 0.251 via the sqrt branch, each 0.001 off the boundary) stay close
    // together rather than jumping -- proving "no discontinuity"
    // empirically around the boundary, not just exactly at it. Both
    // branches also have derivative exactly `1` at `x = 0.25`
    // (`d/dx[(16x-12)x+4)x] = 48x^2-24x+4`, which is `1` at `x=0.25`;
    // `d/dx[sqrt(x)] = 1/(2*sqrt(x))`, also `1` at `x=0.25`) -- so `D` is
    // not just continuous but `C1` there, which is why a `0.001`-wide
    // straddle only moves the value by about `0.001` on each side.
    //
    // The boundary-value comparisons below are both `0.5` computed two
    // independent, exact ways (a literal and this crate's own helper),
    // not an accumulated-rounding-error comparison -- the same reasoning
    // `blend_channel`'s own `float_cmp` allow already documents.
    #[allow(clippy::float_cmp)]
    fn soft_light_d_agrees_at_and_around_its_own_branch_boundary() {
        let polynomial_at_boundary = ((16.0f32 * 0.25 - 12.0) * 0.25 + 4.0) * 0.25;
        let sqrt_at_boundary = 0.25f32.sqrt();
        assert_eq!(polynomial_at_boundary, 0.5);
        assert_eq!(sqrt_at_boundary, 0.5);
        assert_eq!(polynomial_at_boundary, sqrt_at_boundary);
        assert_eq!(soft_light_d(0.25), 0.5);

        let just_below = soft_light_d(0.249); // polynomial branch
        let just_above = soft_light_d(0.251); // sqrt branch
        assert!(
            (just_above - just_below).abs() < 0.005,
            "soft_light_d must not jump across its own branch boundary: D(0.249)={just_below}, D(0.251)={just_above}"
        );
    }

    /// Opens one throwaway [`wgpu::CommandEncoder`], lets `record` write
    /// into it, and submits it — the two lines every compositor method
    /// here used to run internally, before 0.86.0 turned them all into
    /// pure recorders so `aurora-app` could fold a whole composite
    /// tile's passes into a single submit.
    ///
    /// Used at the single-call test sites below precisely so those tests
    /// keep the submission structure they had before that change (one
    /// submit per compositor call), and so what they assert stays a
    /// statement about the *pixels* rather than about batching. The two
    /// "chained through a ping-pong pair" tests deliberately do **not**
    /// use it — see their own doc comments.
    fn submit_one(
        context: &aurora_gpu::GpuContext,
        record: impl FnOnce(&mut wgpu::CommandEncoder),
    ) {
        let mut encoder =
            context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("composite-test"),
                });
        record(&mut encoder);
        context.queue().submit(std::iter::once(encoder.finish()));
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
        submit_one(&context, |encoder| {
            compositor.composite_over(&context, encoder, &dst_view, &src_view);
        });

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
        submit_one(&context, |encoder| {
            compositor.composite_over(&context, encoder, &dst_view, &src_view);
        });

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
        submit_one(&context, |encoder| {
            compositor.composite_over(&context, encoder, &dst_view, &src_view);
        });

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
        submit_one(&context, |encoder| {
            compositor.composite_over(&context, encoder, &dst_view, &src_view);
        });
        assert_eq!(compositor.pipelines.len(), 1);
        submit_one(&context, |encoder| {
            compositor.composite_over(&context, encoder, &dst_view, &src_view);
        });
        assert_eq!(
            compositor.pipelines.len(),
            1,
            "a second call with the same key must not rebuild"
        );
    }

    // -- `composite_over_with_opacity`: the opacity-aware GPU primitive
    // real multi-layer compositing (`aurora-app`'s
    // `begin_gpu_composite_tile`)
    // needs. Every hand-computed expected value below uses exact powers
    // of two (0.25, 0.5, 0.75, 1.0) deliberately -- these round-trip
    // bit-exactly through both `f16` and every intermediate `f32`
    // multiply/add this formula performs, so `assert_eq!` below is a
    // real bit-exact check, not a "close enough" one, the same
    // "0.25/0.5/0.75, powers of two round-trip exactly" reasoning this
    // file's own `composite_tile_cpu` tests already document.

    #[test]
    // Opaque blue dst, opaque red src, opacity 0.25 -> effective alpha
    // 1.0*0.25 = 0.25. Straight-alpha "over":
    // r = (1-0.25)*0 + 0.25*1 = 0.25, g = 0,
    // b = (1-0.25)*1 + 0.25*0 = 0.75, a = 0.25 + 1.0*(1-0.25) = 1.0.
    fn composite_over_with_opacity_scales_the_sources_own_alpha() {
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
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(&context, encoder, &dst_view, &src_view, 0.25);
        });

        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        assert_eq!((r, g, b, a), (0.25, 0.0, 0.75, 1.0));
    }

    #[test]
    // A fully opaque source at zero opacity must leave the destination
    // completely unchanged -- the opacity-driven counterpart of
    // `composite_over_with_fully_transparent_source_leaves_destination_unchanged`,
    // which proves the same property via the source's own alpha instead.
    fn composite_over_with_opacity_of_zero_leaves_destination_unchanged() {
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
            [1.0, 1.0, 1.0, 1.0],
            wgpu::TextureUsages::empty(),
        );
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(&context, encoder, &dst_view, &src_view, 0.0);
        });

        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        assert_eq!((r, g, b, a), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    // An opacity above 1.0 must clamp, not overshoot -- the GPU-path
    // counterpart of `composite_tile_cpu_clamps_an_out_of_range_opacity`.
    fn composite_over_with_opacity_clamps_an_out_of_range_opacity() {
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
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(&context, encoder, &dst_view, &src_view, 5.0);
        });

        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        assert_eq!(
            (r, g, b, a),
            (1.0, 1.0, 1.0, 1.0),
            "an opacity above 1.0 must clamp, not overshoot"
        );
    }

    #[test]
    /// The GPU/CPU parity proof `composite_over_with_opacity`'s own doc
    /// comment promises: the exact same layer data run through this
    /// method and through [`composite_tile_cpu`] (`Normal` blend mode,
    /// full-opacity backdrop drawn first) must land on the identical
    /// result. Uses the same exact-power-of-two values as this module's
    /// other `composite_over_with_opacity` tests above, so the two
    /// independently-implemented formulas (one a hardware fixed-function
    /// blend unit fed by a real fragment shader, the other a plain CPU
    /// loop) are expected to agree bit-for-bit here, not just within a
    /// tolerance -- and do, confirmed by this test actually running
    /// against real GPU hardware.
    fn composite_over_with_opacity_matches_composite_tile_cpus_own_normal_mode_formula() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let dst_rgba = [0.0, 0.0, 1.0, 1.0];
        let src_rgba = [1.0, 0.0, 0.0, 1.0];
        let opacity = 0.25;

        let dst = solid_tile(
            device,
            queue,
            dst_rgba,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(device, queue, src_rgba, wgpu::TextureUsages::empty());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor
                .composite_over_with_opacity(&context, encoder, &dst_view, &src_view, opacity);
        });
        let gpu_result = read_first_texel(device, queue, &dst);

        let dst_texels = solid_texels(dst_rgba);
        let src_texels = solid_texels(src_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&dst_texels, 1.0, BlendMode::Normal),
            (&src_texels, opacity, BlendMode::Normal),
        ]);
        let cpu_result = first_texel(&cpu_out);

        assert_eq!(
            gpu_result, cpu_result,
            "the GPU shader path and the CPU path must agree exactly on this Normal-mode, \
             exact-power-of-two case"
        );
    }

    /// **The compositor methods record; they do not submit** (0.86.0) —
    /// the half of that change no pixel differential and no submit
    /// counter can see.
    ///
    /// Every other test in this module submits the encoder it opened
    /// before reading anything back, so all of them would pass just as
    /// happily if `composite_over_with_opacity` had quietly kept its own
    /// internal `queue.submit`: the pixels would be identical either
    /// way, and `aurora-app`'s own per-tile submit counter only ever
    /// sees the submit `begin_gpu_composite_tile` itself issues, never
    /// one reintroduced down here. That is the exact mutation this test
    /// exists to kill.
    ///
    /// It works by reading the destination twice through
    /// [`read_first_texel`], which opens, submits and polls a command
    /// buffer entirely of its own:
    ///
    /// 1. Record the composite into an encoder and *hold* it, unfinished.
    ///    Read the destination — it must still be the seeded colour,
    ///    because nothing has been submitted.
    /// 2. Submit that encoder. Read again — now it must be the blended
    ///    result.
    ///
    /// A method that submitted internally would fail step 1. A method
    /// that recorded nothing at all would fail step 2, so neither half
    /// is vacuous on its own.
    ///
    /// The two colours are chosen so the blend is unmistakable: an
    /// opaque blue destination and an opaque red source at full opacity,
    /// which "over" replaces outright. Exact-power-of-two channels, so
    /// the comparisons are equalities rather than tolerances, matching
    /// this module's sibling tests.
    #[test]
    fn composite_over_with_opacity_records_into_the_encoder_without_submitting_it() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let dst_rgba = [0.0, 0.0, 1.0, 1.0];
        let src_rgba = [1.0, 0.0, 0.0, 1.0];

        let dst = solid_tile(
            device,
            queue,
            dst_rgba,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let src = solid_tile(device, queue, src_rgba, wgpu::TextureUsages::empty());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite-records-without-submitting"),
        });
        compositor.composite_over_with_opacity(&context, &mut encoder, &dst_view, &src_view, 1.0);

        // Step 1. `read_first_texel` submits and polls its own command
        // buffer, so the GPU has genuinely caught up with everything
        // that *was* submitted -- and the composite above is not part of
        // it, because `encoder` is still open right here.
        let before = read_first_texel(device, queue, &dst);
        assert_eq!(
            before,
            (0.0, 0.0, 1.0, 1.0),
            "the destination must still hold its seeded colour: the composite was recorded into \
             an encoder that has not been submitted, so it cannot have run yet -- a failure here \
             means a compositor method reintroduced an internal queue.submit, which is exactly \
             the per-tile batching this round removed"
        );

        queue.submit(std::iter::once(encoder.finish()));

        // Step 2. The same recording, now submitted, must produce the
        // real blend -- otherwise "it didn't run yet" above would be
        // satisfied just as well by a method that recorded nothing.
        let after = read_first_texel(device, queue, &dst);
        assert_eq!(
            after,
            (1.0, 0.0, 0.0, 1.0),
            "submitting the recorded encoder must produce the real source-over result"
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 1 of the
    // blend-mode port: `Multiply` only, via
    // `TileCompositor::composite_multiply_over_with_opacity` and the
    // `fs_composite_multiply` entry point.
    //
    // These tests exist to answer one specific question the rest of
    // this workspace had never answered: **can a shader sample a texture
    // that a previous render pass wrote to as a colour attachment?**
    // Nothing here had ever done it --
    // every prior GPU test in this file seeds its sampled textures with
    // `queue.write_texture` and only ever *writes* to a render
    // attachment. Real blend-mode math has no alternative: the
    // fixed-function blend unit can express `Normal` and nothing else,
    // so `Cb` has to arrive as a sampled texture.
    //
    // Every one of them therefore builds its accumulator with a real
    // `composite_over_with_opacity` render pass and then hands that same
    // texture to the multiply pass as `backdrop`. Seeding it with
    // `write_texture` instead would pass just as easily and prove
    // nothing about the mechanism.
    //
    // Each of those two passes still goes through its own `submit_one`
    // here (0.86.0), so these tests keep exercising the across-
    // submissions case they always did. The *within-one-command-buffer*
    // case -- the one `aurora-app` now actually relies on -- is what the
    // two "chained through a ping-pong pair" tests below pin, and they
    // are deliberately the only tests here that batch.
    //
    // What each covers, and why none is redundant (0.83.1 added the
    // last four after review found the original two collectively blind
    // to a binding transpose, to any spatial-addressing bug, and to
    // opacity):
    //
    // - `..._multiplies_a_half_grey_backdrop_by_a_quarter_grey_source`:
    //   the arithmetic, with *asymmetric* src and backdrop so a
    //   transposed binding 0/3 fails here on its own.
    // - `..._matches_the_cpu_against_a_translucent_accumulator`: the
    //   un-premultiply branch, against a fractional accumulator alpha.
    // - `..._matches_the_cpu_across_a_spatially_varying_tile`: every
    //   texel of a patterned tile, which is the only one of these that
    //   can catch a V-flip, a transpose, or a half-texel UV offset.
    // - `..._at_half_opacity_matches_the_cpu`: a non-1.0 opacity.
    // - `..._over_a_fully_transparent_backdrop_is_the_source_alone`: the
    //   `ab > 0.0` guard's *untaken* branch, where a naive divide would
    //   be 0/0.
    // - `..._chained_through_a_ping_pong_pair_matches_three_cpu_layers`:
    //   two chained blend passes, each writing the texture the previous
    //   one sampled.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one
    // backend on one vendor: Metal and DX12 remain unverified for this
    // path -- see PLAN.md's 0.83.x entry.

    /// A `TILE`x`TILE` `Rgba16Float` texture seeded from an explicit
    /// `SAMPLES`-length texel buffer, so a test can hand the *same*
    /// pattern to the GPU and to [`composite_tile_cpu`]. The solid-colour
    /// [`solid_tile`] above is the degenerate case of this; both are kept
    /// because most tests here genuinely only need a solid tile.
    fn tile_from_texels(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texels: &[f16],
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-tile-patterned"),
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
        let mut bytes = Vec::with_capacity(texels.len() * 2);
        for channel in texels {
            bytes.extend_from_slice(&channel.to_le_bytes());
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

    /// A deliberately spatially-varying `SAMPLES`-length tile, built so
    /// that *any* wrong-texel-sampled bug shows up somewhere:
    ///
    /// - red varies with `x % 4` and green with `y % 4`, so a shift of
    ///   one to three texels in either axis changes almost every texel
    ///   (and a transpose swaps red with green);
    /// - blue encodes the quadrant (`x >= TILE/2`, `y >= TILE/2`), so a
    ///   V-flip, an H-flip, or a shift that happens to be a multiple of
    ///   four is still caught.
    ///
    /// Every value is a multiple of `0.25`, so it round-trips exactly
    /// through `f16` and lands exactly on an `Rgba8` value after
    /// [`read_rgba8`]'s own rounding. `seed` offsets the pattern so two
    /// layers built from this are never accidentally identical.
    fn patterned_texels(seed: u32, alpha: f32) -> Vec<f16> {
        let mut out = Vec::with_capacity(SAMPLES);
        for y in 0..TILE {
            for x in 0..TILE {
                let quarters = |n: u32| match n % 4 {
                    0 => 0.0,
                    1 => 0.25,
                    2 => 0.5,
                    _ => 0.75,
                };
                let r = quarters(x + seed);
                let g = quarters(y + seed);
                let half = TILE / 2;
                let b = if x >= half { 0.5 } else { 0.0 } + if y >= half { 0.25 } else { 0.0 };
                for channel in [r, g, b, alpha] {
                    out.push(f16::from_f32(channel));
                }
            }
        }
        out
    }

    /// [`read_rgba8`]'s own quantisation, applied to a CPU-side
    /// `SAMPLES`-length buffer, so a whole-tile GPU/CPU comparison
    /// compares like with like.
    fn rgba8_of(texels: &[f16]) -> Vec<u8> {
        texels
            .iter()
            .map(|channel| {
                let value = channel.to_f32().clamp(0.0, 1.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (value * 255.0).round() as u8
                }
            })
            .collect()
    }

    #[test]
    /// The plain-arithmetic case: an opaque 50% grey accumulator, a 25%
    /// grey source at its own `0.5` alpha. The blend itself is
    /// `Multiply(0.5, 0.25) = 0.125`; the "over" around it then folds
    /// that in at the source's effective alpha, giving
    /// `0.5 * 0.5 + 0.5 * 0.125 = 0.3125` per channel, alpha `1.0`.
    ///
    /// **The two inputs are deliberately asymmetric, in colour *and* in
    /// alpha.** Until 0.83.1 this test handed the identical opaque
    /// mid-grey texel to both `src` and `backdrop`, which made it blind
    /// to the single most likely mistake the next 25 blend modes can
    /// make: transposing bindings 0 and 3 in a copy-pasted bind group.
    /// Different *colours* alone are not enough to catch that --
    /// `Multiply` is commutative, and with both alphas at `1.0` and
    /// `opacity` at `1.0` the surrounding "over" terms collapse to the
    /// blend itself, so an opaque `0.25` source over an opaque `0.5`
    /// backdrop yields `0.125` either way round. Giving the source its
    /// own `0.5` alpha breaks that symmetry: transposed, this case
    /// computes `0.375` rather than `0.3125`, so the assertion below
    /// fails on a transpose on its own, without relying on a sibling
    /// test to notice.
    ///
    /// The *accumulator* is still fully opaque, so its premultiplied and
    /// straight colours coincide and the shader's backdrop-recovery
    /// divide is an identity -- deliberately still the simple case, so a
    /// failure means the mechanism (sampling a former render attachment,
    /// `Blend::None`, the bind group) is wrong rather than the
    /// un-premultiply branch. The fractional-*accumulator* sibling below
    /// exercises that branch.
    ///
    /// Every value here is an exact binary fraction, so `assert_eq!` is
    /// a real bit-exact check, not a "close enough" one.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_multiply_over_with_opacity_multiplies_a_half_grey_backdrop_by_a_quarter_grey_source()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.5, 0.5, 1.0];
        let top_rgba = [0.25, 0.25, 0.25, 0.5];

        // The accumulator, built by a real render pass rather than
        // seeded -- that is the whole point of this test.
        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.5, 0.5, 0.5, 1.0),
            "setup: the first pass must really have produced the mid-grey accumulator the \
             second pass then samples"
        );
        let (r, g, b, a) = read_first_texel(device, queue, &dst);
        assert_eq!(
            (r, g, b, a),
            (0.3125, 0.3125, 0.3125, 1.0),
            "Multiply(0.5, 0.25) = 0.125, folded in at the source's own 0.5 alpha: \
             0.5 * 0.5 + 0.5 * 0.125 = 0.3125. Transposing src and backdrop would give 0.375."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case, which is what actually
    /// exercises the shader's backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`) -- the mirror of
    /// [`composite_layer_into`]'s own `straight_backdrop` divide, and
    /// the same gap
    /// `composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator`
    /// covers on the CPU. A fully opaque backdrop can never catch a
    /// missing un-premultiply, because premultiplied and straight
    /// colours are identical at `alpha == 1.0`.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers, so the
    /// GPU and CPU formulas cannot drift apart behind a stale literal.
    ///
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning `aurora-app`'s own GPU-vs-CPU parity test uses:
    /// the two paths fold in different orders and precisions, and Vulkan
    /// permits an `f32` multiply-add a couple of ULP of latitude, so
    /// one-ULP-at-its-own-magnitude disagreement is expected rather than
    /// a defect. It is still tight enough that a genuinely wrong blend,
    /// or a missing un-premultiply, fails.
    fn composite_multiply_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let grey = [0.5, 0.5, 0.5, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, grey, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, grey, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(grey);
        let top_texels = solid_texels(grey);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Multiply),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Multiply path and composite_tile_cpu diverged \
                 by more than {tolerance} against a translucent accumulator ({gpu} vs {cpu}) -- \
                 that is a real finding to report, not a reason to loosen this assertion. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test.** Every other GPU test in this
    /// file composites uniform-colour tiles and reads back texel 0,
    /// which proves the arithmetic and nothing about *which* texel the
    /// shader sampled. A V-flip in the fullscreen-triangle UVs, a
    /// transposed axis, a half-texel offset, or a bind-group transpose
    /// would all sail straight through those tests -- and ruling that
    /// class of bug out is the entire point of a round whose claim is
    /// "sampling a former render target works correctly", so it cannot
    /// rest on a single uniform texel.
    ///
    /// So: both layers are [`patterned_texels`] with *different* seeds
    /// (red varies with `x`, green with `y`, blue with the quadrant),
    /// the accumulator is still built by a real
    /// `composite_over_with_opacity` render pass rather than seeded, and
    /// the **whole** `TILE`x`TILE` result is compared against
    /// [`composite_tile_cpu`]'s own output for the same two layers via
    /// [`read_rgba8`] and its CPU twin [`rgba8_of`].
    ///
    /// The top layer's own alpha is `0.75`, not `1.0`, on purpose:
    /// `Multiply` is commutative, so at two opaque layers and
    /// `opacity == 1.0` the whole composite collapses to `Cb * Cs` and
    /// a transposed src/backdrop binding would still pass. A fractional
    /// source alpha breaks that symmetry per texel.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents: every input
    /// is a multiple of `0.25`, so real disagreement would have to
    /// exceed a whole `Rgba8` step to show up at all.
    fn composite_multiply_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_eq!(
            gpu_accumulator.len(),
            expected_accumulator.len(),
            "setup: readback and CPU reference must describe the same tile"
        );
        let accumulator_mismatches = gpu_accumulator
            .iter()
            .zip(&expected_accumulator)
            .filter(|(gpu, cpu)| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .count();
        assert_eq!(
            accumulator_mismatches, 0,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the multiply comparison below cannot \
             attribute a spatial failure"
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Multiply),
        ]);
        let expected = rgba8_of(&cpu_out);
        let actual = read_rgba8(device, queue, &dst);
        assert_eq!(
            actual.len(),
            expected.len(),
            "readback and CPU reference must describe the same tile"
        );
        let first_mismatch = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .find(|(_, (gpu, cpu))| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .map(|(index, (gpu, cpu))| {
                let texel = index / CHANNELS;
                let tile = TILE as usize;
                (texel % tile, texel / tile, index % CHANNELS, *gpu, *cpu)
            });
        assert!(
            first_mismatch.is_none(),
            "the in-shader Multiply path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile -- first mismatch (x, y, channel, gpu, cpu): \
             {first_mismatch:?}. A whole-tile disagreement of this kind is a wrong-texel bug \
             (V-flip, transpose, UV offset, transposed binding), not precision."
        );
    }

    #[test]
    /// A non-`1.0` opacity on the Multiply path. Both of the original
    /// 0.83.0 tests passed `opacity: 1.0`, which never exercises the
    /// `s.a * opacity.value` scale the shader's own doc comment says it
    /// relies on the Rust caller to have clamped -- while the sibling
    /// `composite_over_with_opacity` has dedicated `0.25`, `0.0` and
    /// `5.0` cases.
    ///
    /// The expected value is **not hand-derived**: it comes from the
    /// real [`composite_tile_cpu`] with the same two layers and the same
    /// `0.5`, so the two implementations cannot drift apart behind a
    /// stale literal. Non-grey, per-channel-distinct colours are used so
    /// a channel swizzle anywhere in the path fails here too.
    fn composite_multiply_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Multiply),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Multiply path and composite_tile_cpu diverged \
                 by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// `fs_composite_multiply` deliberately does not clamp `sa *
    /// opacity.value` -- only `opacity` is clamped Rust-side, mirroring
    /// `composite_layer_into`'s own `let opacity = opacity.clamp(0.0,
    /// 1.0)` followed by an unclamped `sa * opacity`. `f16` can legally
    /// hold a source alpha above `1.0`, and nothing pinned that this
    /// method preserves rather than silently clamps it.
    fn composite_multiply_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Multiply),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// The Multiply-path mirror of
    /// `composite_over_with_opacity_clamps_an_out_of_range_opacity`: an
    /// opacity above `1.0` must clamp to `1.0` on this path too, not
    /// overshoot the source's own contribution.
    fn composite_multiply_over_with_opacity_clamps_an_out_of_range_opacity() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        // 5.0, clamped Rust-side before it ever reaches the uniform --
        // if the clamp were missing, `a` would come out > 1.0 and the
        // final `inv = 1.0 - a` would go negative.
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                5.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Multiply),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: an opacity above 1.0 must clamp to 1.0, matching the \
                 opacity-1.0 result, not overshoot it ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch, on real hardware.
    ///
    /// A fully transparent accumulator makes the straight-backdrop
    /// recovery `bd.rgb / ab` a `0.0 / 0.0`, which is why the guard
    /// exists -- but a guard only helps if the compiler does not
    /// flatten the branch and evaluate both sides, and whether it does
    /// is a property of the shader compiler, i.e. of the backend. This
    /// was checked adversarially during review of 0.83.0 and found
    /// clean; 0.83.1 makes it a committed test rather than a review-only
    /// finding, so a future backend or `naga` change cannot regress it
    /// silently.
    ///
    /// **Since 0.109.0 the guard is shared, not per entry point, and this
    /// is one of only three of the nine per-mode versions of this test
    /// that still detect its removal** — with `screen`'s and
    /// `difference`'s. `Multiply`'s `cb * s.rgb` propagates a `NaN`
    /// instead of laundering it through a `min`/`max`, which is exactly
    /// why. See `composite.wgsl`'s disclosure beside `straight_backdrop()`
    /// and PLAN.md's 0.109.0 entry.
    ///
    /// With `ab == 0.0` the whole composite reduces to the source alone,
    /// so the result is also asserted to be exactly that -- a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_multiply_over_with_opacity_over_a_fully_transparent_backdrop_is_the_source_alone()
    {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one.
        let top_rgba = [0.25, 0.5, 0.75, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass that leaves the accumulator empty: an
        // opaque white layer at zero opacity contributes nothing, so the
        // backdrop stays fully transparent while still having been
        // produced by the mechanism under test.
        let bottom = solid_tile(
            device,
            queue,
            [1.0, 1.0, 1.0, 1.0],
            wgpu::TextureUsages::empty(),
        );
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                0.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_multiply_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful against a genuinely zero-alpha accumulator"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[(
            &top_texels,
            1.0,
            BlendMode::Multiply,
        )]));
        assert_eq!(
            gpu_result, cpu_result,
            "over a fully transparent accumulator the composite is the source alone, exactly as \
             composite_tile_cpu computes it"
        );
    }

    #[test]
    /// **Two chained blend passes, ping-ponged.** 0.83.0 proved a single
    /// hop: pass K samples a texture pass K-1 *wrote*. Real multi-layer
    /// accumulation needs the other half of that -- pass K+1 writing a
    /// texture pass K *sampled from* (a write-after-read hazard, not a
    /// read-after-write one), which nothing in that round exercised.
    /// Since `composite_multiply_over_with_opacity` cannot accumulate in
    /// place (`dst` must not alias `backdrop`), a ping-pong pair is the
    /// shape any real caller will have to use, so it is worth proving
    /// before a later round designs against it.
    ///
    /// Three layers, three passes: `Normal` into A, `Multiply` A -> B,
    /// `Multiply` B -> A. The third pass therefore renders into the very
    /// texture the second pass bound as a sampled backdrop. The final
    /// contents of A are compared against a single three-layer
    /// [`composite_tile_cpu`] call.
    ///
    /// It works, with no barrier, copy or explicit synchronisation of
    /// any kind -- on Vulkan/NVIDIA. Metal and DX12 are unverified.
    ///
    /// **Since 0.86.0 all three passes share one encoder and one
    /// submit**, which strengthens what this proves rather than merely
    /// restating it. Until then each compositor call submitted its own
    /// command buffer, so the ordering the test relied on was whatever
    /// `wgpu` inserts *between submissions*; now it is recording order
    /// *within a single command buffer* — which is exactly the guarantee
    /// `aurora-app`'s `begin_gpu_composite_tile` depends on, since it
    /// folds a whole tile's clear, layer passes and readback into one
    /// encoder. This test is the concrete evidence for that dependency.
    fn composite_multiply_over_with_opacity_chained_through_a_ping_pong_pair_matches_three_cpu_layers()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let layer1 = [0.5, 0.75, 1.0, 1.0];
        let layer2 = [0.5, 0.5, 0.5, 1.0];
        let layer3 = [0.25, 1.0, 0.5, 1.0];
        let opacity3 = 0.5;

        let accumulator_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let ping = solid_tile(device, queue, [0.0, 0.0, 0.0, 0.0], accumulator_usage);
        let pong = solid_tile(device, queue, [0.0, 0.0, 0.0, 0.0], accumulator_usage);
        let l1 = solid_tile(device, queue, layer1, wgpu::TextureUsages::empty());
        let l2 = solid_tile(device, queue, layer2, wgpu::TextureUsages::empty());
        let l3 = solid_tile(device, queue, layer3, wgpu::TextureUsages::empty());
        let ping_view = ping.create_view(&wgpu::TextureViewDescriptor::default());
        let pong_view = pong.create_view(&wgpu::TextureViewDescriptor::default());
        let l1_view = l1.create_view(&wgpu::TextureViewDescriptor::default());
        let l2_view = l2.create_view(&wgpu::TextureViewDescriptor::default());
        let l3_view = l3.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // All three passes go into **one** encoder and **one** submit
        // (0.86.0) -- deliberately, and unlike every single-call test in
        // this module, which keeps its own `submit_one`. This is the
        // arrangement `aurora-app`'s `begin_gpu_composite_tile` actually
        // has now, so this test is what pins that both hazards below
        // resolve correctly *inside a single command buffer*, where
        // ordering comes from recording order rather than from `wgpu`'s
        // between-submission synchronisation:
        //
        // - read-after-write: pass 2 samples `ping`, which pass 1 wrote;
        // - write-after-read: pass 3 renders into `ping`, which pass 2
        //   sampled.
        //
        // Before 0.86.0 each pass carried its own internal submit, so
        // this test proved the same thing only across submissions.
        submit_one(&context, |encoder| {
            // Pass 1: build the accumulator in `ping`.
            compositor.composite_over_with_opacity(&context, encoder, &ping_view, &l1_view, 1.0);
            // Pass 2: sample `ping`, write `pong`.
            compositor.composite_multiply_over_with_opacity(
                &context, encoder, &l2_view, &ping_view, &pong_view, 1.0,
            );
            // Pass 3: sample `pong`, write back into `ping` -- the
            // texture pass 2 read from. This is the hop 0.83.0 never
            // took.
            compositor.composite_multiply_over_with_opacity(
                &context, encoder, &l3_view, &pong_view, &ping_view, opacity3,
            );
        });

        let t1 = solid_texels(layer1);
        let t2 = solid_texels(layer2);
        let t3 = solid_texels(layer3);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&t1, 1.0, BlendMode::Normal),
            (&t2, 1.0, BlendMode::Multiply),
            (&t3, opacity3, BlendMode::Multiply),
        ]));
        let gpu_result = read_first_texel(device, queue, &ping);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a two-hop ping-pong chain diverged from composite_tile_cpu's \
                 own three-layer result by more than {tolerance} ({gpu} vs {cpu}). If this is a \
                 real second-hop failure rather than precision, it is an important finding for \
                 the whole blend-mode epic, not something to work around. Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    // -- Real in-shader blend-mode math on the GPU, slice 2 of the
    // blend-mode port: `Darken`, via
    // `TileCompositor::composite_darken_over_with_opacity` and the
    // `fs_composite_darken` entry point.
    //
    // One-for-one siblings of the `composite_multiply_*` suite above,
    // covering exactly the same seven concerns (the arithmetic against
    // an asymmetric pair, the un-premultiply branch, spatial addressing,
    // a non-1.0 opacity, an unclamped source alpha above 1.0, a clamped
    // out-of-range opacity, and the `ab > 0.0` guard's untaken branch),
    // plus one the `Multiply` suite could not have: a **mixed**-mode
    // ping-pong chain, which is what actually proves the ping-pong
    // mechanism generalises past a single blend mode rather than merely
    // repeating one.
    //
    // Fixture values are *not* copied from the `Multiply` siblings.
    // `Darken` collapses to `Normal` whenever the source is darker than
    // the backdrop in every channel (and to a no-op whenever it is
    // lighter in every channel), so a fixture that happens to sit on
    // either side of that boundary in all three channels would pass just
    // as well with the wrong arm dispatched. Every fixture below
    // therefore takes its minimum from the *backdrop* in at least one
    // channel and from the *source* in at least one other.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one
    // backend on one vendor: Metal and DX12 remain unverified for this
    // path -- see PLAN.md's 0.85.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `Darken` sibling of
    /// `composite_multiply_over_with_opacity_multiplies_a_half_grey_backdrop_by_a_quarter_grey_source`.
    ///
    /// An opaque `(0.75, 0.25, 0.5)` accumulator under a
    /// `(0.25, 0.75, 0.5)` source at its own `0.5` alpha. The blend is
    /// `min` per channel — `(0.25, 0.25, 0.5)`, taking red from the
    /// *source*, green from the *backdrop*, and blue from either since
    /// they agree — and the "over" then folds that in at the source's
    /// effective alpha: `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.5, 0.25, 0.5)` at alpha `1.0`.
    ///
    /// **Every one of the three plausible wrong answers is a different
    /// value here**, which is why the colours are per-channel distinct
    /// and the source's alpha is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.5, 0.5, 0.5)`;
    /// - the `Multiply` arm dispatched by mistake:
    ///   `(0.46875, 0.21875, 0.375)`;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group:
    ///   `(0.625, 0.25, 0.5)`.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_darken_over_with_opacity_takes_the_per_channel_minimum_of_backdrop_and_source() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.75, 0.25, 0.5, 1.0];
        let top_rgba = [0.25, 0.75, 0.5, 0.5];

        // The accumulator, built by a real render pass rather than
        // seeded -- the same mechanism the Multiply suite above proves,
        // re-exercised through the second entry point.
        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.75, 0.25, 0.5, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Darken),
        ]));
        assert_eq!(
            cpu_result,
            (0.5, 0.25, 0.5, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.5, 0.25, 0.5, 1.0),
            "Darken(Cb, Cs) = min per channel = (0.25, 0.25, 0.5), folded in at the source's own \
             0.5 alpha. The Normal arm would give (0.5, 0.5, 0.5), the Multiply arm \
             (0.46875, 0.21875, 0.375), and a transposed src/backdrop binding (0.625, 0.25, 0.5)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `Darken` sibling of
    /// `composite_multiply_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising the shader's backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The grey-on-grey fixture the `Multiply` sibling uses would be
    /// worthless here: `min(0.5, 0.5)` is `0.5`, which is also what
    /// `Normal` produces, so a wrong-arm dispatch would pass. The
    /// backdrop is `(0.75, 0.25, 0.5)` at half opacity and the source
    /// `(0.25, 0.75, 0.5)` instead, so the minimum comes from the source
    /// in red and the backdrop in green.
    ///
    /// A missing un-premultiply still fails loudly: the raw accumulator
    /// is `(0.375, 0.125, 0.25)`, and taking the minimum against *that*
    /// rather than the recovered straight `(0.75, 0.25, 0.5)` gives a
    /// different answer in green and blue.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Multiply` sibling documents.
    fn composite_darken_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.75, 0.25, 0.5, 1.0];
        let top_rgba = [0.25, 0.75, 0.5, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Darken),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Darken path and composite_tile_cpu diverged \
                 by more than {tolerance} against a translucent accumulator ({gpu} vs {cpu}) -- \
                 that is a real finding to report, not a reason to loosen this assertion. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `Darken` entry point**, the
    /// sibling of
    /// `composite_multiply_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `Darken` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, so
    /// red varies with `x`, green with `y`, and blue with the quadrant,
    /// and the per-channel minimum genuinely comes from different layers
    /// in different texels. The accumulator is built by a real
    /// `composite_over_with_opacity` render pass rather than seeded, and
    /// the **whole** `TILE`x`TILE` result is compared against
    /// [`composite_tile_cpu`]'s own output via [`read_rgba8`] and its CPU
    /// twin [`rgba8_of`].
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_darken_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_eq!(
            gpu_accumulator.len(),
            expected_accumulator.len(),
            "setup: readback and CPU reference must describe the same tile"
        );
        let accumulator_mismatches = gpu_accumulator
            .iter()
            .zip(&expected_accumulator)
            .filter(|(gpu, cpu)| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .count();
        assert_eq!(
            accumulator_mismatches, 0,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the Darken comparison below cannot \
             attribute a spatial failure"
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Darken),
        ]);
        let expected = rgba8_of(&cpu_out);
        let actual = read_rgba8(device, queue, &dst);
        assert_eq!(
            actual.len(),
            expected.len(),
            "readback and CPU reference must describe the same tile"
        );
        let first_mismatch = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .find(|(_, (gpu, cpu))| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .map(|(index, (gpu, cpu))| {
                let texel = index / CHANNELS;
                let tile = TILE as usize;
                (texel % tile, texel / tile, index % CHANNELS, *gpu, *cpu)
            });
        assert!(
            first_mismatch.is_none(),
            "the in-shader Darken path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile -- first mismatch (x, y, channel, gpu, cpu): \
             {first_mismatch:?}. A whole-tile disagreement of this kind is a wrong-texel bug \
             (V-flip, transpose, UV offset, transposed binding), not precision."
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `Darken` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The sibling of
    /// `composite_multiply_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`. Non-grey,
    /// per-channel-distinct colours are used so a channel swizzle
    /// anywhere in the path fails here too, and the minimum is taken
    /// from the source in red and green but from the backdrop in blue.
    fn composite_darken_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Darken),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Darken path and composite_tile_cpu diverged \
                 by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// `fs_composite_darken` deliberately does not clamp
    /// `sa * opacity.value` -- only `opacity` is clamped Rust-side,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0`. The sibling of
    /// `composite_multiply_over_with_opacity_does_not_clamp_a_source_alpha_above_one`.
    fn composite_darken_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Darken),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// The `Darken`-path mirror of
    /// `composite_multiply_over_with_opacity_clamps_an_out_of_range_opacity`:
    /// an opacity above `1.0` must clamp to `1.0` on this path too, not
    /// overshoot the source's own contribution.
    fn composite_darken_over_with_opacity_clamps_an_out_of_range_opacity() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        // 5.0, clamped Rust-side before it ever reaches the uniform --
        // if the clamp were missing, `a` would come out > 1.0 and the
        // final `inv = 1.0 - a` would go negative.
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                5.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Darken),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: an opacity above 1.0 must clamp to 1.0, matching the \
                 opacity-1.0 result, not overshoot it ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_darken`, on real hardware — the sibling of
    /// `composite_multiply_over_with_opacity_over_a_fully_transparent_backdrop_is_the_source_alone`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_multiply` does
    /// not prove it here: this is a second, separately-compiled
    /// function, and `min(NaN, x)` is exactly the kind of expression
    /// whose NaN handling differs between backends.
    ///
    /// **0.109.0/0.109.1 turned that last clause from a caution into a
    /// measurement, and it cuts against this test.** The guard now lives
    /// once in `composite.wgsl`'s shared `straight_backdrop()`, and on
    /// Vulkan/NVIDIA `min(NaN, x)` returns `x` — so with the guard deleted
    /// this test still *passes*: `Darken` is one of the six modes for which
    /// removing it is output-equivalent rather than merely undetected. What
    /// this test still pins per entry point is that this mode's own `b`
    /// line and fold reduce to the source alone at `ab == 0.0`. See
    /// `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    ///
    /// With `ab == 0.0` the whole composite reduces to the source alone,
    /// so the result is asserted to be exactly that -- a `NaN` leaking
    /// out of the untaken divide would fail both the finiteness check
    /// and the value check, and (`NaN != NaN`) could not be mistaken for
    /// a pass.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_darken_over_with_opacity_over_a_fully_transparent_backdrop_is_the_source_alone() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one.
        let top_rgba = [0.25, 0.5, 0.75, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass that leaves the accumulator empty: an
        // opaque white layer at zero opacity contributes nothing, so the
        // backdrop stays fully transparent while still having been
        // produced by the mechanism under test.
        let bottom = solid_tile(
            device,
            queue,
            [1.0, 1.0, 1.0, 1.0],
            wgpu::TextureUsages::empty(),
        );
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                0.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_darken_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful against a genuinely zero-alpha accumulator"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[(
            &top_texels,
            1.0,
            BlendMode::Darken,
        )]));
        assert_eq!(
            gpu_result, cpu_result,
            "over a fully transparent accumulator the composite is the source alone, exactly as \
             composite_tile_cpu computes it"
        );
    }

    #[test]
    /// **The mixed-mode ping-pong chain — the test the `Multiply` suite
    /// could not have written.** Every prior chained test here ping-pongs
    /// through repeated instances of *one* blend mode, which proves the
    /// two accumulators can trade places but says nothing about whether
    /// a *second* mode can reuse the same pair. That question is what
    /// `aurora-app`'s own `begin_gpu_composite_tile` actually depends on:
    /// its `spare` accumulator is a single shared texture created by
    /// whichever blend-math arm reaches a tile first, and every later arm
    /// — of any mode — renders into that same texture.
    ///
    /// Three layers, three passes: `Normal` into A, `Multiply` A -> B,
    /// then `Darken` B -> A. The `Darken` pass therefore renders into
    /// the very texture the `Multiply` pass bound as a sampled backdrop,
    /// and samples the one `Multiply` wrote — a write-after-read hazard
    /// *across two different pipelines*, which is the case a per-mode
    /// pipeline cache or a stale bind group would get wrong.
    ///
    /// The fixture is chosen so the third layer's mode is genuinely
    /// observable: after the `Multiply` pass the accumulator holds
    /// `(0.375, 0.5, 0.5)`, and the top layer is `(0.5, 0.25, 0.75)`, so
    /// `Darken` yields `(0.375, 0.25, 0.5)` — taking its minimum from
    /// the backdrop in red and blue but from the source in green. A
    /// third `Multiply` would have given `(0.1875, 0.125, 0.375)` and a
    /// `Normal` `(0.5, 0.25, 0.75)`. The `assert_ne!` below pins that
    /// distinguishability rather than leaving it as a claim in prose, so
    /// a future edit to the fixture cannot quietly make the differential
    /// vacuous.
    ///
    /// Compared against a single three-layer [`composite_tile_cpu`] call
    /// — differential, not hand-derived, for the same reason its
    /// `Multiply` sibling is.
    fn composite_darken_and_multiply_chained_through_one_ping_pong_pair_match_three_cpu_layers() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let layer1 = [0.75, 0.5, 1.0, 1.0];
        let layer2 = [0.5, 1.0, 0.5, 1.0];
        let layer3 = [0.5, 0.25, 0.75, 1.0];

        let accumulator_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let ping = solid_tile(device, queue, [0.0, 0.0, 0.0, 0.0], accumulator_usage);
        let pong = solid_tile(device, queue, [0.0, 0.0, 0.0, 0.0], accumulator_usage);
        let l1 = solid_tile(device, queue, layer1, wgpu::TextureUsages::empty());
        let l2 = solid_tile(device, queue, layer2, wgpu::TextureUsages::empty());
        let l3 = solid_tile(device, queue, layer3, wgpu::TextureUsages::empty());
        let ping_view = ping.create_view(&wgpu::TextureViewDescriptor::default());
        let pong_view = pong.create_view(&wgpu::TextureViewDescriptor::default());
        let l1_view = l1.create_view(&wgpu::TextureViewDescriptor::default());
        let l2_view = l2.create_view(&wgpu::TextureViewDescriptor::default());
        let l3_view = l3.create_view(&wgpu::TextureViewDescriptor::default());

        // One compositor, so both blend pipelines come out of the same
        // `PipelineCache` -- the arrangement `aurora-app` actually has,
        // and the one in which a cache keyed on too little would hand
        // the second mode the first mode's pipeline.
        let mut compositor = TileCompositor::new(device);
        // One encoder, one submit, all three passes (0.86.0) -- the same
        // deliberate exception its `Multiply`-only sibling above makes,
        // and for the same reason: this is the shape
        // `begin_gpu_composite_tile` now has, so the read-after-write
        // (pass 2 samples what pass 1 wrote) and write-after-read (pass
        // 3 renders into what pass 2 sampled) hazards are proven to
        // resolve from recording order *within one command buffer*, not
        // merely across submissions. Two blend modes sharing that one
        // buffer is the additional thing this test, and not its sibling,
        // pins.
        submit_one(&context, |encoder| {
            // Pass 1: build the accumulator in `ping`.
            compositor.composite_over_with_opacity(&context, encoder, &ping_view, &l1_view, 1.0);
            // Pass 2: sample `ping`, write `pong`.
            compositor.composite_multiply_over_with_opacity(
                &context, encoder, &l2_view, &ping_view, &pong_view, 1.0,
            );
            // Pass 3: a *different* blend mode, sampling `pong` and
            // writing back into `ping` -- the same shared pair, no third
            // texture.
            compositor.composite_darken_over_with_opacity(
                &context, encoder, &l3_view, &pong_view, &ping_view, 1.0,
            );
        });

        let t1 = solid_texels(layer1);
        let t2 = solid_texels(layer2);
        let t3 = solid_texels(layer3);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&t1, 1.0, BlendMode::Normal),
            (&t2, 1.0, BlendMode::Multiply),
            (&t3, 1.0, BlendMode::Darken),
        ]));
        let if_third_were_multiply = first_texel(&composite_tile_cpu(&[
            (&t1, 1.0, BlendMode::Normal),
            (&t2, 1.0, BlendMode::Multiply),
            (&t3, 1.0, BlendMode::Multiply),
        ]));
        let if_third_were_normal = first_texel(&composite_tile_cpu(&[
            (&t1, 1.0, BlendMode::Normal),
            (&t2, 1.0, BlendMode::Multiply),
            (&t3, 1.0, BlendMode::Normal),
        ]));
        assert_ne!(
            cpu_result, if_third_were_multiply,
            "setup: this fixture must distinguish a Darken third layer from a Multiply one, or \
             the differential below would pass with the wrong pipeline dispatched"
        );
        assert_ne!(
            cpu_result, if_third_were_normal,
            "setup: this fixture must distinguish a Darken third layer from a Normal one"
        );

        let gpu_result = read_first_texel(device, queue, &ping);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a mixed Multiply-then-Darken ping-pong chain diverged from \
                 composite_tile_cpu's own three-layer result by more than {tolerance} ({gpu} vs \
                 {cpu}). Compare against the Multiply-only ({if_third_were_multiply:?}) and \
                 Normal-only ({if_third_were_normal:?}) alternatives to see whether the wrong \
                 pipeline ran. Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    // -- Real in-shader blend-mode math on the GPU, slice 3 of the
    // blend-mode port: `Lighten`, via
    // `TileCompositor::composite_lighten_over_with_opacity` and the
    // `fs_composite_lighten` entry point (0.95.0).
    //
    // **Six tests, not the `Darken` suite's seven** (corrected in
    // 0.95.1 -- it was five, on a justification that only covered one of
    // the two dropped tests; see below).
    //
    // The one the `Darken` suite has and this one legitimately does not
    // is its clamped-out-of-range-opacity case. Since 0.85.1's merge
    // that property lives in a *single* shared line --
    // `composite_blend_over_with_opacity`'s own `let opacity =
    // opacity.clamp(0.0, 1.0)`, which every mode's wrapper reaches
    // through -- and the `Multiply` and `Darken` suites already pin it on
    // real hardware. Re-asserting one shared Rust line once per ported
    // mode would grow linearly in the number of modes while covering
    // nothing new. That is a disclosed reduction in coverage, not an
    // equivalence claim.
    //
    // **0.95.0 dropped a second test on that same justification, and
    // was wrong to.** `composite_darken_over_with_opacity_does_not_
    // clamp_a_source_alpha_above_one` does not test the shared clamp at
    // all: it tests that the *shader's* own `let a = s.a * opacity.value`
    // deliberately does **not** clamp its product, only the Rust-side
    // `opacity` factor having been clamped before it arrives. Each WGSL
    // fragment function is separately compiled, so that is a
    // per-entry-point property -- exactly the argument this same section
    // makes to justify *keeping* the transparent-backdrop test. A
    // copy-pasted `min(s.a * opacity.value, 1.0)` in
    // `fs_composite_lighten` alone would have passed the whole 0.95.0
    // suite. `composite_lighten_over_with_opacity_does_not_clamp_a_
    // source_alpha_above_one` below closes that, so the count is six.
    //
    // The six kept each exercise something that really is
    // per-entry-point: this mode's own arithmetic, its own un-premultiply
    // branch, its own spatial addressing, its own opacity-scaled fold,
    // its own unclamped `s.a * opacity` product, and this mode's own
    // collapse to the source alone where the accumulator alpha is zero.
    //
    // **That last one used to read "its own separately-compiled
    // `ab > 0.0` guard", and 0.109.1 corrected it here and at the six
    // sibling section headers below.** Since 0.109.0 the guard is written
    // *once*, in `composite.wgsl`'s `straight_backdrop()`, and shared by
    // all nine blend-math entry points, so guard independence is not a
    // per-entry-point property any more. It is worse than that for this
    // mode specifically: with the guard deleted, `composite_lighten_over_
    // with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`
    // still *passes*, because `max()` launders the resulting NaN into the
    // finite operand before it reaches the fold. Only `multiply`,
    // `screen` and `difference` can detect the guard's removal at all;
    // for the other six modes -- this one included -- removing it is
    // output-equivalent on Vulkan/NVIDIA, not merely undetected, so no
    // fixture change of the 0.105.1/0.105.2 kind could close it. What the
    // test does still pin per entry point is that *this* mode's `b` line
    // and its `fold_over` call reduce to the source alone at `ab == 0.0`.
    // `composite.wgsl`'s disclosure beside `straight_backdrop()` has the
    // full account, and PLAN.md's 0.109.0 entry has the two isolating
    // experiments behind it; neither is repeated at the sibling headers.
    //
    // Fixture values are again *not* copied from the `Darken` siblings.
    // `Lighten` collapses to a no-op whenever the source is darker than
    // the backdrop in every channel (and to `Normal` whenever it is
    // lighter in every channel), so every fixture below takes its maximum
    // from the *backdrop* in at least one channel and from the *source*
    // in at least one other.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one
    // backend on one vendor: Metal and DX12 remain unverified for this
    // path -- see PLAN.md's 0.95.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `Lighten` mirror of
    /// `composite_darken_over_with_opacity_takes_the_per_channel_minimum_of_backdrop_and_source`.
    ///
    /// An opaque `(0.25, 0.75, 0.5)` accumulator under a
    /// `(0.75, 0.25, 0.5)` source at its own `0.5` alpha. The blend is
    /// `max` per channel — `(0.75, 0.75, 0.5)`, taking red from the
    /// *source*, green from the *backdrop*, and blue from either since
    /// they agree — and the "over" then folds that in at the source's
    /// effective alpha: `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.5, 0.75, 0.5)` at alpha `1.0`.
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct and the source's alpha
    /// is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.5, 0.5, 0.5)`;
    /// - the `Darken` arm — the realistic copy-paste, since this entry
    ///   point is that one with `min` swapped for `max`:
    ///   `(0.25, 0.5, 0.5)`;
    /// - the `Multiply` arm: `(0.21875, 0.46875, 0.375)`;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group:
    ///   `(0.875, 0.75, 0.75)`.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_lighten_over_with_opacity_takes_the_per_channel_maximum_of_backdrop_and_source() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.75, 0.25, 0.5, 0.5];

        // The accumulator, built by a real render pass rather than
        // seeded -- the same mechanism the two suites above prove,
        // re-exercised through the third entry point.
        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.25, 0.75, 0.5, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Lighten),
        ]));
        assert_eq!(
            cpu_result,
            (0.5, 0.75, 0.5, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.5, 0.75, 0.5, 1.0),
            "Lighten(Cb, Cs) = max per channel = (0.75, 0.75, 0.5), folded in at the source's \
             own 0.5 alpha. The Normal arm would give (0.5, 0.5, 0.5), the Darken arm \
             (0.25, 0.5, 0.5), the Multiply arm (0.21875, 0.46875, 0.375), and a transposed \
             src/backdrop binding (0.875, 0.75, 0.75)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `Lighten` mirror of
    /// `composite_darken_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.25, 0.75, 0.5)` at half opacity and the source
    /// `(0.75, 0.25, 0.5)`, so the maximum comes from the source in red
    /// and the backdrop in green — a wrong-arm dispatch cannot pass on a
    /// fixture that is one-sided in every channel.
    ///
    /// A missing un-premultiply still fails loudly: the raw accumulator
    /// is `(0.125, 0.375, 0.25)`, and taking the maximum against *that*
    /// rather than the recovered straight `(0.25, 0.75, 0.5)` gives a
    /// different answer in green.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Darken` sibling documents.
    fn composite_lighten_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.75, 0.25, 0.5, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Lighten),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Lighten path and composite_tile_cpu diverged \
                 by more than {tolerance} against a translucent accumulator ({gpu} vs {cpu}) -- \
                 that is a real finding to report, not a reason to loosen this assertion. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `Lighten` entry point**, the
    /// mirror of
    /// `composite_darken_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `Lighten` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, so
    /// red varies with `x`, green with `y`, and blue with the quadrant,
    /// and the per-channel maximum genuinely comes from different layers
    /// in different texels. The accumulator is built by a real
    /// `composite_over_with_opacity` render pass rather than seeded, and
    /// the **whole** `TILE`x`TILE` result is compared against
    /// [`composite_tile_cpu`]'s own output via [`read_rgba8`] and its CPU
    /// twin [`rgba8_of`].
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_lighten_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_eq!(
            gpu_accumulator.len(),
            expected_accumulator.len(),
            "setup: readback and CPU reference must describe the same tile"
        );
        let accumulator_mismatches = gpu_accumulator
            .iter()
            .zip(&expected_accumulator)
            .filter(|(gpu, cpu)| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .count();
        assert_eq!(
            accumulator_mismatches, 0,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the Lighten comparison below cannot \
             attribute a spatial failure"
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Lighten),
        ]);
        let expected = rgba8_of(&cpu_out);
        let actual = read_rgba8(device, queue, &dst);
        assert_eq!(
            actual.len(),
            expected.len(),
            "readback and CPU reference must describe the same tile"
        );
        let first_mismatch = actual
            .iter()
            .zip(&expected)
            .enumerate()
            .find(|(_, (gpu, cpu))| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .map(|(index, (gpu, cpu))| {
                let texel = index / CHANNELS;
                let tile = TILE as usize;
                (texel % tile, texel / tile, index % CHANNELS, *gpu, *cpu)
            });
        assert!(
            first_mismatch.is_none(),
            "the in-shader Lighten path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile -- first mismatch (x, y, channel, gpu, cpu): \
             {first_mismatch:?}. A whole-tile disagreement of this kind is a wrong-texel bug \
             (V-flip, transpose, UV offset, transposed binding), not precision."
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `Lighten` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The mirror of
    /// `composite_darken_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`. Non-grey,
    /// per-channel-distinct colours are used so a channel swizzle
    /// anywhere in the path fails here too, and the maximum is taken
    /// from the backdrop in red and green but from the source in blue.
    fn composite_lighten_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 1.0, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Lighten),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Lighten path and composite_tile_cpu diverged \
                 by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_lighten` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The mirror
    /// of
    /// `composite_darken_over_with_opacity_does_not_clamp_a_source_alpha_above_one`.
    ///
    /// **Restored in 0.95.1.** 0.95.0 dropped this test on the grounds
    /// that opacity clamping is one shared line since 0.85.1's merge.
    /// That is true of the *out-of-range-opacity* test and not of this
    /// one: the property here is the `let a = s.a * opacity.value;` line
    /// inside this entry point, and each WGSL fragment function is
    /// separately compiled — the same per-entry-point argument that keeps
    /// the transparent-backdrop test below. See this section's header
    /// comment.
    ///
    /// **Why the fixture is shaped this way.** With a source alpha of
    /// `2.0` the fold's `inv = 1.0 - a` goes *negative*, so the clamped
    /// and unclamped answers differ by exactly `b - cb` per channel —
    /// which for `Lighten` is zero in every channel the backdrop already
    /// won. The backdrop is therefore chosen to win only *one* channel
    /// (red, per this section's own two-sided-maximum rule), leaving
    /// green and blue to separate the two answers:
    ///
    /// - `cb = (0.5, 0.25, 0.375)`, `Cs = (0.25, 0.75, 0.5)`, so
    ///   `b = max(cb, Cs) = (0.5, 0.75, 0.5)`;
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2b = (0.5, 1.25, 0.625)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped (`a = 1.0`, `inv = 0.0`): `b = (0.5, 0.75, 0.5)`, at the
    ///   same alpha `1.0` — so **alpha alone cannot catch this**, and the
    ///   colour channels are what the assertion rests on.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_lighten_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.25, 0.375, 1.0];
        let top_rgba = [0.25, 0.75, 0.5, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Lighten),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_lighten`
        // yields (0.5, 0.75, 0.5, 1.0) instead -- red and alpha agree,
        // which is why this is asserted per channel rather than as a
        // single texel comparison whose message would not say where.
        for (gpu, expected, channel) in [
            (gr, 0.5, "r"),
            (gg, 1.25, "g"),
            (gb, 0.625, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.5, 0.75, 0.5, 1.0) would mean fs_composite_lighten clamped the \
                 s.a * opacity product. Full texel: {gpu_result:?}"
            );
        }
    }

    /// A `SAMPLES`-length tile that is fully transparent in its left half
    /// and opaque `(0.75, 0.25, 0.5)` in its right — the fixture
    /// `composite_lighten_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`
    /// and its `Screen` counterpart
    /// `composite_screen_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`
    /// both need, to exercise the `ab > 0.0` guard's untaken branch *and*
    /// stay sensitive to the blend formula in the same tile.
    ///
    /// The opaque colour is chosen so `Lighten` takes red from the
    /// backdrop and green and blue from the source (this section's
    /// two-sided-maximum rule), and so that `min` and `max` disagree in
    /// every channel there. It suits `Screen` unchanged: no channel of it
    /// is `0.0` or `1.0`, the two values at which `Screen` degenerates
    /// (to `Normal` and to a constant `1.0` respectively), so its own
    /// answer in the opaque half is distinct from every other ported
    /// mode's — see that test's own enumeration.
    fn half_transparent_texels() -> Vec<f16> {
        let mut out = Vec::with_capacity(SAMPLES);
        for _y in 0..TILE {
            for x in 0..TILE {
                let texel = if x >= TILE / 2 {
                    [0.75, 0.25, 0.5, 1.0]
                } else {
                    [0.0, 0.0, 0.0, 0.0]
                };
                for channel in texel {
                    out.push(f16::from_f32(channel));
                }
            }
        }
        out
    }

    /// Asserts two [`read_rgba8`]-shaped buffers agree everywhere within
    /// `1` of 255, naming the first disagreeing `(x, y, channel)` rather
    /// than dumping two 256 KiB vectors. The same comparison the
    /// spatially-varying tests above spell out inline; factored out here
    /// because the half-transparent test needs it twice (once on the
    /// accumulator as a setup check, once on the result).
    fn assert_whole_tile_matches(actual: &[u8], expected: &[u8], context: &str) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "{context} -- readback and CPU reference must describe the same tile"
        );
        let first_mismatch = actual
            .iter()
            .zip(expected)
            .enumerate()
            .find(|(_, (gpu, cpu))| u16::from(**gpu).abs_diff(u16::from(**cpu)) > 1)
            .map(|(index, (gpu, cpu))| {
                let texel = index / CHANNELS;
                let tile = TILE as usize;
                (texel % tile, texel / tile, index % CHANNELS, *gpu, *cpu)
            });
        assert!(
            first_mismatch.is_none(),
            "{context} -- first mismatch (x, y, channel, gpu, cpu): {first_mismatch:?}"
        );
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_lighten`, on real hardware — the mirror of
    /// `composite_darken_over_with_opacity_over_a_fully_transparent_backdrop_is_the_source_alone`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_darken` does not
    /// prove it here: this is a third, separately-compiled function, and
    /// `max(NaN, x)` is exactly the kind of expression whose NaN handling
    /// differs between backends — and it is a *different* expression from
    /// `min(NaN, x)`, which the `Darken` sibling covers.
    ///
    /// **Measured since (0.109.0/0.109.1), and it cuts against this test.**
    /// The guard now lives once in `composite.wgsl`'s shared
    /// `straight_backdrop()`, and on Vulkan/NVIDIA `max(NaN, x)` returns
    /// `x` — so with the guard deleted this test still *passes*: `Lighten`
    /// is one of the six modes for which removing it is output-equivalent
    /// rather than merely undetected. What this test still pins per entry
    /// point is that this mode's own `b` line and fold reduce to the source
    /// alone where `ab == 0.0`. See `composite.wgsl`'s disclosure beside
    /// `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that -- a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so
    /// (0.95.1).** The `Darken`/`Multiply` siblings both use a uniformly
    /// zero-alpha accumulator, and red-team proved by execution that this
    /// makes them the one test in each suite that a `min`/`max` swap
    /// survives: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so no such fixture can
    /// distinguish the two intrinsics. That is inherent to a *uniform*
    /// fixture and not to the property under test — the untaken
    /// `ab > 0.0` branch only needs *some* texels at zero alpha. So the
    /// bottom layer here is transparent in its left half and opaque
    /// `(0.75, 0.25, 0.5)` in its right, and the whole tile is compared:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.25, 0.5, 0.75, 1.0)`;
    /// - right half (`ab == 1`): `out = max(cb, Cs) = (0.75, 0.5, 0.75)`,
    ///   where `min` would give `(0.25, 0.25, 0.5)` — all three channels
    ///   differ, so the swap now fails here too.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_lighten_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one.
        let top_rgba = [0.25, 0.5, 0.75, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_lighten_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        // ... and the whole accumulator must reproduce the bottom layer,
        // so the opaque half is genuinely opaque and the halves are where
        // this test believes they are.
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.25, 0.5, 0.75, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Lighten),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader Lighten path and composite_tile_cpu disagree across a half-transparent \
             backdrop. In the opaque half a min/max swap shows up here; in the transparent half a \
             NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 4 of the
    // blend-mode port: `Screen`, via
    // `TileCompositor::composite_screen_over_with_opacity` and the
    // `fs_composite_screen` entry point (0.102.0).
    //
    // **Six tests, mirroring the `Lighten` suite's own six**, and
    // omitting the same one the `Darken` suite has: an
    // out-of-range-*opacity* case. Since 0.85.1's merge that property
    // lives in a single shared Rust line -- `composite_blend_over_with_
    // opacity`'s own `let opacity = opacity.clamp(0.0, 1.0)`, which every
    // mode's wrapper reaches through -- and the `Multiply` and `Darken`
    // suites already pin it on real hardware. Re-asserting one shared
    // line once per ported mode grows linearly in the number of modes
    // while covering nothing new. That is a disclosed reduction in
    // coverage, not an equivalence claim.
    //
    // The unclamped *source-alpha* case is emphatically **not** omitted:
    // that one tests `fs_composite_screen`'s own `let a = s.a *
    // opacity.value` line, each WGSL fragment function is separately
    // compiled, and 0.95.0 dropping it for `Lighten` on the opacity-clamp
    // argument was the mistake 0.95.1 had to correct. See the `Lighten`
    // section header above for that account.
    //
    // The six each exercise something genuinely per-entry-point: this
    // mode's own arithmetic, its own un-premultiply branch, its own
    // spatial addressing, its own opacity-scaled fold, its own unclamped
    // `s.a * opacity` product, and this mode's own collapse to the source
    // alone at a zero accumulator alpha -- **not** "its own
    // separately-compiled `ab > 0.0` guard", which 0.109.0's shared
    // `straight_backdrop()` made false and 0.109.1 corrected here.
    // `Screen` is, however, one of only three modes whose
    // transparent-backdrop test still detects that guard's removal: its
    // formula is arithmetic on `cb`, so a NaN propagates instead of being
    // laundered by a `min`/`max`. See the `Lighten` section header above
    // and `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    //
    // **Fixture values are chosen against `Screen`'s own two degeneracies,
    // which are different from every prior mode's.** `Screen(0, Cs) = Cs`
    // -- indistinguishable from `Normal` -- and `Screen(Cb, 1) =
    // Screen(1, Cs) = 1` -- indistinguishable from a saturating bug. So
    // every operand in every fixture below is strictly inside `(0, 1)` in
    // every channel. That rules out reusing the `Lighten` half-opacity
    // fixture verbatim, whose source has a `1.0` blue channel.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for this path --
    // see PLAN.md's 0.102.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `Screen` counterpart of
    /// `composite_lighten_over_with_opacity_takes_the_per_channel_maximum_of_backdrop_and_source`.
    ///
    /// An opaque `(0.25, 0.75, 0.5)` accumulator under a
    /// `(0.75, 0.25, 0.5)` source at its own `0.5` alpha. The blend is
    /// `Cb + Cs - Cb*Cs` per channel — `(0.8125, 0.8125, 0.75)` — and the
    /// "over" then folds that in at the source's effective alpha:
    /// `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.53125, 0.78125, 0.625)` at alpha `1.0`.
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct, none of them is `0.0`
    /// or `1.0`, and the source's alpha is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.5, 0.5, 0.5)`;
    /// - the `Lighten` arm — the realistic copy-paste, since this entry
    ///   point was written from that one: `(0.5, 0.75, 0.5)`;
    /// - the `Darken` arm: `(0.25, 0.5, 0.5)`;
    /// - the `Multiply` arm: `(0.21875, 0.46875, 0.375)`;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group:
    ///   `(0.8125, 0.8125, 0.75)`. (That it coincides with `B` itself is
    ///   an arithmetic accident of this fixture — `Screen` is commutative
    ///   in `Cb`/`Cs` — and it is still distinct from the golden, which
    ///   is what the assertion needs.)
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_screen_over_with_opacity_takes_the_per_channel_inverse_multiply() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.75, 0.25, 0.5, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.25, 0.75, 0.5, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Screen),
        ]));
        assert_eq!(
            cpu_result,
            (0.53125, 0.78125, 0.625, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.53125, 0.78125, 0.625, 1.0),
            "Screen(Cb, Cs) = Cb + Cs - Cb*Cs per channel = (0.8125, 0.8125, 0.75), folded in \
             at the source's own 0.5 alpha. The Normal arm would give (0.5, 0.5, 0.5), the \
             Lighten arm (0.5, 0.75, 0.5), the Darken arm (0.25, 0.5, 0.5), the Multiply arm \
             (0.21875, 0.46875, 0.375), and a transposed src/backdrop binding \
             (0.8125, 0.8125, 0.75)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `Screen` counterpart of
    /// `composite_lighten_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.25, 0.75, 0.5)` at half opacity and the source
    /// `(0.75, 0.25, 0.5)` — per-channel distinct on both sides, and no
    /// channel at `0.0` or `1.0`, so neither of `Screen`'s degeneracies
    /// is in play.
    ///
    /// A missing un-premultiply fails loudly in **all three** channels
    /// here, not just one: the raw premultiplied accumulator is
    /// `(0.125, 0.375, 0.25)`, and screening against *that* rather than
    /// the recovered straight `(0.25, 0.75, 0.5)` gives
    /// `B = (0.78125, 0.53125, 0.625)` where the correct `B` is
    /// `(0.8125, 0.8125, 0.75)`.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Darken` and `Lighten` siblings document.
    fn composite_screen_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.75, 0.25, 0.5, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Screen),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Screen path and composite_tile_cpu diverged \
                 by more than {tolerance} against a translucent accumulator ({gpu} vs {cpu}) -- \
                 that is a real finding to report, not a reason to loosen this assertion. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `Screen` entry point**, the
    /// counterpart of
    /// `composite_lighten_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `Screen` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, so
    /// red varies with `x`, green with `y`, and blue with the quadrant,
    /// and the result genuinely varies texel to texel. The accumulator is
    /// built by a real `composite_over_with_opacity` render pass rather
    /// than seeded, and the **whole** `TILE`x`TILE` result is compared
    /// against [`composite_tile_cpu`]'s own output via [`read_rgba8`] and
    /// its CPU twin [`rgba8_of`].
    ///
    /// A bind-group transpose is worth calling out for this mode in
    /// particular: `Screen`'s own `B` is symmetric in `Cb`/`Cs`, so a
    /// transpose is *not* caught by the blend term alone — it is caught
    /// by the surrounding "over", which is not symmetric, and by the
    /// per-texel spatial comparison here. That is why this test matters
    /// more for `Screen` than the commutativity might suggest.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_screen_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the Screen comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Screen),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader Screen path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `Screen` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The counterpart of
    /// `composite_lighten_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`. Non-grey,
    /// per-channel-distinct colours are used so a channel swizzle
    /// anywhere in the path fails here too.
    ///
    /// **The `Lighten` sibling's fixture could not be reused.** Its
    /// source is `(0.25, 0.5, 1.0)`, and `Screen(Cb, 1.0) = 1.0` for
    /// every `Cb` — the blue channel would have gone constant and stopped
    /// discriminating anything. The source's blue is `0.75` here instead,
    /// which keeps all three channels inside `(0, 1)`. The resulting
    /// golden `(0.5625, 0.8125, 0.53125)` differs from `Normal`'s
    /// `(0.375, 0.625, 0.5)`, `Multiply`'s `(0.3125, 0.5625, 0.21875)`,
    /// `Darken`'s `(0.375, 0.625, 0.25)` and `Lighten`'s
    /// `(0.5, 0.75, 0.5)` in every channel.
    fn composite_screen_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.25, 1.0];
        let top_rgba = [0.25, 0.5, 0.75, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Screen),
        ]));
        assert_eq!(
            cpu_result,
            (0.5625, 0.8125, 0.53125, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Screen path and composite_tile_cpu diverged \
                 by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full texels: \
                 {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_screen` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The
    /// counterpart of
    /// `composite_lighten_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore that one: this asserts a
    /// line *inside this entry point*, and each WGSL fragment function is
    /// separately compiled, so no other mode's suite covers it.
    ///
    /// **Why the fixture separates all three channels.** With a source
    /// alpha of `2.0` the fold's `inv = 1.0 - a` goes negative, so the
    /// clamped and unclamped answers differ by exactly `b - cb` per
    /// channel. For `Screen` that difference is `Cs * (1 - Cb)`, which
    /// vanishes only where `Cs == 0` or `Cb == 1` — so unlike `Lighten`,
    /// where the backdrop winning a channel silently zeroed the
    /// difference there, any fixture strictly inside `(0, 1)` separates
    /// every channel. This one does:
    ///
    /// - `cb = (0.5, 0.25, 0.375)`, `Cs = (0.25, 0.75, 0.5)`, so
    ///   `b = Cb + Cs - Cb*Cs = (0.625, 0.8125, 0.6875)`;
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2b = (0.75, 1.375, 1.0)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped (`a = 1.0`, `inv = 0.0`): `b = (0.625, 0.8125, 0.6875)`,
    ///   at the same alpha `1.0` — so **alpha alone cannot catch this**,
    ///   and the colour channels are what the assertion rests on.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_screen_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.25, 0.375, 1.0];
        let top_rgba = [0.25, 0.75, 0.5, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Screen),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_screen`
        // yields (0.625, 0.8125, 0.6875, 1.0) instead -- alpha agrees,
        // which is why this is asserted per channel rather than as a
        // single texel comparison whose message would not say where.
        for (gpu, expected, channel) in [
            (gr, 0.75, "r"),
            (gg, 1.375, "g"),
            (gb, 1.0, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.625, 0.8125, 0.6875, 1.0) would mean fs_composite_screen clamped the \
                 s.a * opacity product. Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_screen`, on real hardware — the counterpart of
    /// `composite_lighten_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_lighten` does not
    /// prove it here: this is a fourth, separately-compiled function, and
    /// its blend line is *arithmetic* on `cb` rather than a `min`/`max`
    /// intrinsic — `NaN + x - NaN * x` propagates rather than being
    /// selected away, which if anything makes this the most likely of the
    /// four entry points to leak a `NaN` if the branch is flattened.
    ///
    /// **That reasoning was confirmed by measurement in 0.109.0.** The
    /// guard now lives once in `composite.wgsl`'s shared
    /// `straight_backdrop()`, and deleting it fails exactly three of the
    /// nine per-mode versions of this test — `multiply`'s, this one, and
    /// `difference`'s — precisely because those three propagate the `NaN`
    /// while the other six launder it through a `min`/`max`. So this test
    /// is one of the three that genuinely protects the shared guard. See
    /// `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so**
    /// — the reason 0.95.1 gives for the `Lighten` sibling applies
    /// verbatim: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so a uniform fixture cannot
    /// distinguish this entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)`
    /// and a `(0.25, 0.5, 0.75)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.25, 0.5, 0.75, 1.0)`;
    /// - right half (`ab == 1`): `out = Cb + Cs - Cb*Cs =
    ///   (0.8125, 0.625, 0.875)`, where `Normal` gives
    ///   `(0.25, 0.5, 0.75)`, `Multiply` `(0.1875, 0.125, 0.375)`,
    ///   `Darken` `(0.25, 0.25, 0.5)` and `Lighten` `(0.75, 0.5, 0.75)` —
    ///   every one of them differing in all three channels.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_screen_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, and strictly inside
        // (0, 1) so neither of Screen's degeneracies is in play.
        let top_rgba = [0.25, 0.5, 0.75, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_screen_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.25, 0.5, 0.75, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Screen),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader Screen path and composite_tile_cpu disagree across a half-transparent \
             backdrop. In the opaque half a wrong blend formula shows up here; in the \
             transparent half a NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 5 of the
    // blend-mode port: `Difference`, via
    // `TileCompositor::composite_difference_over_with_opacity` and the
    // `fs_composite_difference` entry point (0.104.0).
    //
    // **Six tests, mirroring the `Screen` suite's own six**, and omitting
    // the same one the `Darken` and `Screen` suites omit: an
    // out-of-range-*opacity* case. Since 0.85.1's merge that property
    // lives in a single shared Rust line -- `composite_blend_over_with_
    // opacity`'s own `let opacity = opacity.clamp(0.0, 1.0)`, which every
    // mode's wrapper reaches through -- and the `Multiply` and `Darken`
    // suites already pin it on real hardware. Re-asserting one shared
    // line once per ported mode grows linearly in the number of modes
    // while covering nothing new. That is a disclosed reduction in
    // coverage, not an equivalence claim.
    //
    // The unclamped *source-alpha* case is emphatically **not** omitted:
    // that one tests `fs_composite_difference`'s own `let a = s.a *
    // opacity.value` line, each WGSL fragment function is separately
    // compiled, and 0.95.0 dropping it for `Lighten` on the opacity-clamp
    // argument was the mistake 0.95.1 had to correct. See the `Lighten`
    // section header above for that account.
    //
    // The six each exercise something genuinely per-entry-point: this
    // mode's own arithmetic, its own un-premultiply branch, its own
    // spatial addressing, its own opacity-scaled fold, its own unclamped
    // `s.a * opacity` product, and this mode's own collapse to the source
    // alone at a zero accumulator alpha -- **not** "its own
    // separately-compiled `ab > 0.0` guard", which 0.109.0's shared
    // `straight_backdrop()` made false and 0.109.1 corrected here.
    // `Difference` is, with `Multiply` and `Screen`, one of only three
    // modes whose transparent-backdrop test still detects that guard's
    // removal -- `abs(NaN - x)` propagates rather than laundering. See the
    // `Lighten` section header above and `composite.wgsl`'s disclosure
    // beside `straight_backdrop()`.
    //
    // **Fixture values are chosen against `Difference`'s own degeneracies,
    // which are different again from every prior mode's.** There are three
    // that matter:
    //
    //   1. `Difference(Cb, Cb) = 0` -- the mode collapses wherever the two
    //      operands are *equal in a channel*, and a zero blend term is
    //      indistinguishable from several unrelated bugs. So no fixture
    //      below has `Cb == Cs` in any channel, with one deliberate,
    //      disclosed exception: `patterned_texels`' blue channel in the
    //      spatial test, whose own doc comment says so and explains what
    //      still discriminates there.
    //   2. `Difference` agrees with `Subtract` (`max(Cb - Cs, 0)`) in every
    //      channel where `Cb >= Cs`, and with a plain, `abs()`-less
    //      `Cb - Cs` in exactly the same channels. So every fixture below
    //      has at least one channel with `Cb < Cs` -- the only place those
    //      two wrong shaders are observable at all. Several have channels
    //      on *both* sides of the sign change, which is stronger.
    //   3. `Difference(0, Cs) = Cs` -- indistinguishable from `Normal` --
    //      and `Difference(1, Cs) = 1 - Cs`. So every operand in the four
    //      *solid-colour* fixtures below is strictly inside `(0, 1)` in
    //      every channel, as the `Screen` suite's already were for its own
    //      two degeneracies, and so is every operand in the opaque half of
    //      the half-transparent fixture. **Two disclosed exceptions, both
    //      in fixtures that are not solid colours** (0.104.1 added the
    //      first of them; the header previously stated the "strictly
    //      inside" rule as absolute, which it is not):
    //
    //      - the spatial test's `patterned_texels` emits exactly `0.0` in
    //        red wherever `(x + seed) % 4 == 0` and in green wherever
    //        `(y + seed) % 4 == 0`, so roughly a quarter of that tile's
    //        columns and a quarter of its rows sit on
    //        `Difference(0, Cs) = Cs` in one channel. (Its blue is `0.0`
    //        throughout the top-left quadrant too, though blue there is
    //        already the degeneracy-1 exception below.) The test still
    //        discriminates the formula on the three-in-four columns and
    //        rows where the operand is nonzero, and it compares the whole
    //        tile rather than one texel -- it is a confirmed mutation kill
    //        for the formula mutations either way (PLAN.md's 0.104.0
    //        table, rows b/c/e/f).
    //      - the half-transparent test's zero-alpha half is `(0, 0, 0, 0)`
    //        by construction. That one is the *point* of the test -- it
    //        exercises the `ab > 0.0` guard's untaken branch -- and its
    //        opaque half is what carries the formula discrimination; its
    //        own doc comment says so.
    //
    // A fourth consideration, weaker than a degeneracy but worth stating
    // because it is what actually ruled out inheriting the `Screen` suite's
    // fixtures verbatim: this mode's blend term is a *magnitude*, so two
    // channels with the same `|Cb - Cs|` produce the same blend value even
    // from different operands, and a channel swizzle between those two is
    // then invisible in `B`. The `Screen` half-opacity pair
    // `(0.5, 0.75, 0.25)` / `(0.25, 0.5, 0.75)` is exactly that case --
    // strictly inside `(0, 1)`, no channel equal, but `|Cb - Cs|` is
    // `(0.25, 0.25, 0.5)`, so red and green share a magnitude. (Nothing
    // about it is *degenerate* for `Difference`; it is simply weaker than a
    // fixture with three distinct magnitudes.) Each fixture below was
    // therefore derived fresh rather than inherited, with three distinct
    // per-channel differences; the doc comments say what each one
    // separates.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for this path --
    // see PLAN.md's 0.104.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `Difference` counterpart of
    /// `composite_screen_over_with_opacity_takes_the_per_channel_inverse_multiply`.
    ///
    /// An opaque `(0.25, 0.75, 0.5)` accumulator under an
    /// `(0.875, 0.25, 0.625)` source at its own `0.5` alpha. The blend is
    /// `|Cb - Cs|` per channel — `(0.625, 0.5, 0.125)` — and the "over"
    /// then folds that in at the source's effective alpha:
    /// `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.4375, 0.625, 0.3125)` at alpha `1.0`.
    ///
    /// **The fixture straddles the sign change**: `Cb < Cs` in red *and*
    /// blue (`0.25 < 0.875` and `0.5 < 0.625`), `Cb > Cs` in green alone
    /// (`0.75 > 0.25`). That is what makes the two closest wrong shaders
    /// observable — an `abs()`-less `Cb - Cs` and `Subtract`'s
    /// `max(Cb - Cs, 0)` agree with `Difference` only in green, the one
    /// channel where `Cb >= Cs`, and differ in red *and* blue; neither
    /// would differ anywhere if every channel had `Cb > Cs`.
    ///
    /// (0.104.1 corrected this paragraph, which previously put blue on the
    /// wrong side of the sign change and so claimed one failing channel
    /// where there are two. The fixture is more discriminating than the
    /// old wording said, not less: the wrong-arm values listed below were
    /// and are correct, and both `Subtract`'s `(0.125, 0.625, 0.25)` and
    /// the dropped `abs()`'s `(-0.1875, 0.625, 0.1875)` differ from the
    /// golden `(0.4375, 0.625, 0.3125)` in exactly red and blue.)
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct, none of them is `0.0`
    /// or `1.0`, no channel has `Cb == Cs`, and the source's alpha is `0.5`
    /// rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.5625, 0.5, 0.5625)`;
    /// - the `Screen` arm — the realistic copy-paste, since this entry
    ///   point was written from that one:
    ///   `(0.578125, 0.78125, 0.65625)`;
    /// - the `Lighten` arm: `(0.5625, 0.75, 0.5625)`;
    /// - the `Darken` arm: `(0.25, 0.5, 0.5)`;
    /// - the `Multiply` arm: `(0.234375, 0.46875, 0.40625)`;
    /// - a dropped `abs()` (`Cb - Cs`): `(-0.1875, 0.625, 0.1875)`;
    /// - `Subtract`'s `max(Cb - Cs, 0)`: `(0.125, 0.625, 0.25)`;
    /// - `Exclusion`, the "softer `Difference`" it is most confusable
    ///   with: `(0.46875, 0.6875, 0.5)`;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group: not
    ///   caught by the blend term, which is symmetric
    ///   (`|Cb - Cs| = |Cs - Cb|`) exactly as `Screen`'s is. It is caught
    ///   by the surrounding, asymmetric "over" and by the spatial test
    ///   below. Disclosed rather than claimed away.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_difference_over_with_opacity_takes_the_per_channel_absolute_difference() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.875, 0.25, 0.625, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.25, 0.75, 0.5, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Difference),
        ]));
        assert_eq!(
            cpu_result,
            (0.4375, 0.625, 0.3125, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.4375, 0.625, 0.3125, 1.0),
            "Difference(Cb, Cs) = |Cb - Cs| per channel = (0.625, 0.5, 0.125), folded in at the \
             source's own 0.5 alpha. The Normal arm would give (0.5625, 0.5, 0.5625), the Screen \
             arm (0.578125, 0.78125, 0.65625), the Lighten arm (0.5625, 0.75, 0.5625), the \
             Darken arm (0.25, 0.5, 0.5), the Multiply arm (0.234375, 0.46875, 0.40625), a \
             dropped abs() (-0.1875, 0.625, 0.1875), Subtract's max(Cb - Cs, 0) \
             (0.125, 0.625, 0.25) and Exclusion (0.46875, 0.6875, 0.5)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `Difference` counterpart
    /// of
    /// `composite_screen_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.25, 0.75, 0.5)` at half opacity and the source
    /// `(0.875, 0.25, 0.625)` — per-channel distinct on both sides, no
    /// channel at `0.0` or `1.0`, no channel with `Cb == Cs`, and `Cb < Cs`
    /// in red *and* blue against `Cb > Cs` in green alone (0.104.1: this
    /// last clause previously misnamed blue), so none of `Difference`'s
    /// three degeneracies is in play.
    ///
    /// A missing un-premultiply fails loudly in **all three** channels
    /// here, not just one: the raw premultiplied accumulator is
    /// `(0.125, 0.375, 0.25)`, and differencing against *that* rather than
    /// the recovered straight `(0.25, 0.75, 0.5)` gives
    /// `B = (0.75, 0.125, 0.375)` where the correct `B` is
    /// `(0.625, 0.5, 0.125)`.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Darken`, `Lighten` and `Screen` siblings
    /// document. (For the record it is `(0.75, 0.375, 0.375, 1.0)`, since
    /// `ab_inv * Cs + ab * B` at `ab = 0.5` is
    /// `0.5 * (0.875, 0.25, 0.625) + 0.5 * (0.625, 0.5, 0.125)`, and the
    /// source's own `a = 1.0` makes `inv = 0.0` — but the assertion goes
    /// through the CPU reference, not through that literal.)
    fn composite_difference_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.5, 1.0];
        let top_rgba = [0.875, 0.25, 0.625, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Difference),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Difference path and composite_tile_cpu \
                 diverged by more than {tolerance} against a translucent accumulator ({gpu} vs \
                 {cpu}) -- that is a real finding to report, not a reason to loosen this \
                 assertion. Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `Difference` entry point**,
    /// the counterpart of
    /// `composite_screen_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `Difference` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, and the
    /// result genuinely varies texel to texel. The accumulator is
    /// built by a real `composite_over_with_opacity` render pass rather
    /// than seeded, and the **whole** `TILE`x`TILE` result is compared
    /// against [`composite_tile_cpu`]'s own output via [`read_rgba8`] and
    /// its CPU twin [`rgba8_of`].
    ///
    /// **Three disclosures specific to this mode, none of them a claim
    /// that all three channels discriminate at every texel.** (The third
    /// was added in 0.104.1; the suite header had stated its
    /// "every operand strictly inside `(0, 1)`" rule as absolute, and this
    /// test is the fixture that does not obey it.)
    ///
    /// 1. **[`patterned_texels`]' blue channel is seed-independent** — it
    ///    is a pure function of the texel's quadrant
    ///    (`if x >= half { 0.5 } + if y >= half { 0.25 }`), with no `seed`
    ///    term at all, unlike red (`quarters(x + seed)`) and green
    ///    (`quarters(y + seed)`). So the two layers' blue channels are
    ///    *equal at every texel*, `Cb == Cs` there, and this mode's blend
    ///    term is identically `0` in blue across the whole tile — the
    ///    `Difference(Cb, Cb) = 0` degeneracy, hit deliberately in one
    ///    channel rather than hidden. Blue still varies spatially in the
    ///    *output* (via the `inv * bd.rgb` term), so a wrong-texel bug is
    ///    still caught there; what blue cannot do here is discriminate the
    ///    blend formula.
    /// 2. **Red and green are what actually exercise `abs()` spatially**,
    ///    and they do it in *both* directions: with seeds `0` and `1`, the
    ///    top layer's red is `quarters(x + 1)` against the bottom's
    ///    `quarters(x)`, so `Cb < Cs` at `x % 4 ∈ {0, 1, 2}` and
    ///    `Cb > Cs` at `x % 4 == 3` (where `quarters` wraps `0.75` back to
    ///    `0.0`). Green does the same in `y`. Every texel of the tile
    ///    therefore sits on one side or the other of the sign change, and
    ///    both sides occur — which is exactly what a dropped `abs()` or a
    ///    `Subtract`-shaped `max(…, 0)` fails on, in three of every four
    ///    columns and rows.
    /// 3. **A zero operand does occur here, in red and green** — the
    ///    suite header's third degeneracy, `Difference(0, Cs) = Cs`.
    ///    `quarters` returns exactly `0.0` at `n % 4 == 0`, so the
    ///    accumulator's red is `0.0` in one column of every four
    ///    (`x % 4 == 0` at seed `0`) and its green is `0.0` in one row of
    ///    every four. In those texels that channel's blend term is
    ///    indistinguishable from `Normal`'s. This is the same three-of-four
    ///    fraction disclosure 2 already turns on, from the other side: the
    ///    columns and rows that discriminate the sign change are the ones
    ///    with a nonzero operand, and the whole-tile comparison covers all
    ///    of them at once. It is why this test is a real kill for the
    ///    formula mutations (PLAN.md's 0.104.0 table, rows b/c/e/f) despite
    ///    the degeneracy, not in spite of an undisclosed one.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_difference_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the Difference comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Difference),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader Difference path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `Difference` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The counterpart of
    /// `composite_screen_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`, and is also asserted
    /// as an absolute golden: `Cb = (0.5, 0.875, 0.25)`,
    /// `Cs = (0.375, 0.5, 0.75)`, so `B = |Cb - Cs| = (0.125, 0.375, 0.5)`
    /// and the fold at `a = 0.5` over an opaque accumulator gives
    /// `0.5 * Cb + 0.5 * B = (0.3125, 0.625, 0.375)` at alpha `1.0`.
    /// Non-grey, per-channel-distinct colours are used so a channel
    /// swizzle anywhere in the path fails here too.
    ///
    /// **The `Screen` sibling's fixture was not reused, and the reason is
    /// weaker than a degeneracy — stated precisely rather than overclaimed.**
    /// Its bottom/top pair is `(0.5, 0.75, 0.25)` / `(0.25, 0.5, 0.75)`.
    /// Nothing about that is degenerate for `Difference`: every channel is
    /// strictly inside `(0, 1)`, no channel has `Cb == Cs`, and blue sits on
    /// the far side of the sign change. What is weaker is that its
    /// per-channel differences are `(0.25, 0.25, 0.5)` — red and green
    /// share a magnitude, and since this mode's blend term *is* that
    /// magnitude, a red/green swizzle inside the blend line would survive
    /// it. This fixture keeps the same sign pattern (`Cb > Cs` in red and
    /// green, `Cb < Cs` in blue) but gives three *distinct* magnitudes
    /// `(0.125, 0.375, 0.5)`. A dropped `abs()` then shows up as
    /// `(0.3125, 0.625, -0.125)` and `Subtract`'s `max(…, 0)` as
    /// `(0.3125, 0.625, 0.125)` — both wrong in blue, and neither hidden by
    /// a coincidence between channels.
    /// The golden `(0.3125, 0.625, 0.375)` also differs from `Normal`'s
    /// `(0.4375, 0.6875, 0.5)`, `Multiply`'s `(0.34375, 0.65625, 0.21875)`,
    /// `Darken`'s `(0.4375, 0.6875, 0.25)`, `Lighten`'s `(0.5, 0.875, 0.5)`
    /// and `Screen`'s `(0.59375, 0.90625, 0.53125)`.
    fn composite_difference_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.875, 0.25, 1.0];
        let top_rgba = [0.375, 0.5, 0.75, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Difference),
        ]));
        assert_eq!(
            cpu_result,
            (0.3125, 0.625, 0.375, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader Difference path and composite_tile_cpu \
                 diverged by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_difference` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The
    /// counterpart of
    /// `composite_screen_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore the `Lighten` one: this
    /// asserts a line *inside this entry point*, and each WGSL fragment
    /// function is separately compiled, so no other mode's suite covers it.
    ///
    /// **The source alpha is `2.0` and the `opacity` argument is `1.0`, not
    /// the other way round.** Passing `opacity = 2.0` would prove nothing:
    /// `composite_blend_over_with_opacity` clamps that argument to `1.0`
    /// before it ever reaches the uniform, so `a` would come out as `1.0`
    /// and this test would assert the *clamped* answer. The unclamped
    /// product is only reachable through a source alpha the tile itself
    /// carries.
    ///
    /// **Why the fixture separates all three channels.** With a source
    /// alpha of `2.0` the fold's `inv = 1.0 - a` goes negative, so the
    /// clamped and unclamped answers differ by exactly `b - cb` per
    /// channel — which for `Difference` vanishes only where
    /// `|Cb - Cs| == Cb`, i.e. where `Cs` is `0` or `2*Cb`. Neither holds
    /// in any channel here:
    ///
    /// - `cb = (0.625, 0.25, 0.375)`, `Cs = (0.25, 0.75, 0.5)`, so
    ///   `b = |Cb - Cs| = (0.375, 0.5, 0.125)`;
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2b = (0.125, 0.75, -0.125)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped (`a = 1.0`, `inv = 0.0`): `b = (0.375, 0.5, 0.125)`,
    ///   at the same alpha `1.0` — so **alpha alone cannot catch this**,
    ///   and the colour channels are what the assertion rests on.
    ///
    /// **The unclamped golden's blue channel is negative** (`-0.125`), and
    /// that is the point rather than an accident: neither
    /// `composite_layer_into` nor `fs_composite_difference` clamps its
    /// output, `Rgba16Float` stores a negative `f16` exactly, and
    /// [`read_first_texel`] does not clamp on the way back — so the GPU and
    /// CPU are expected to agree on it. A disagreement here would be a real
    /// finding about one of those three, not a reason to move the fixture.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_difference_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.625, 0.25, 0.375, 1.0];
        let top_rgba = [0.25, 0.75, 0.5, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::Difference),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_difference`
        // yields (0.375, 0.5, 0.125, 1.0) instead -- alpha agrees, which
        // is why this is asserted per channel rather than as a single
        // texel comparison whose message would not say where. Blue is
        // deliberately negative; see the doc comment.
        for (gpu, expected, channel) in [
            (gr, 0.125, "r"),
            (gg, 0.75, "g"),
            (gb, -0.125, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.375, 0.5, 0.125, 1.0) would mean fs_composite_difference clamped the \
                 s.a * opacity product, and a 0.0 in blue would mean something clamped the \
                 *output* to non-negative. Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_difference`, on real hardware — the counterpart of
    /// `composite_screen_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_screen` does not
    /// prove it here: this is a fifth, separately-compiled function. Like
    /// `Screen`'s and unlike `Darken`'s or `Lighten`'s, its blend line is
    /// *arithmetic* on `cb` rather than a `min`/`max` intrinsic —
    /// `abs(NaN - x)` is `NaN`, propagated rather than selected away.
    ///
    /// **Confirmed by measurement in 0.109.0.** The guard now lives once in
    /// `composite.wgsl`'s shared `straight_backdrop()`, and deleting it
    /// fails exactly three of the nine per-mode versions of this test —
    /// `multiply`'s, `screen`'s and this one — for exactly that reason,
    /// while the other six launder the `NaN` through a `min`/`max`. So this
    /// test is one of the three that genuinely protects the shared guard.
    /// See `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so**
    /// — the reason 0.95.1 gives for the `Lighten` sibling applies
    /// verbatim: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so a uniform fixture cannot
    /// distinguish this entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)`
    /// and a `(0.125, 0.75, 0.875)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.125, 0.75, 0.875, 1.0)`;
    /// - right half (`ab == 1`): `out = |Cb - Cs| =
    ///   (0.625, 0.5, 0.375)`, where `Normal` gives
    ///   `(0.125, 0.75, 0.875)`, `Multiply` `(0.09375, 0.1875, 0.4375)`,
    ///   `Darken` `(0.125, 0.25, 0.5)`, `Lighten` `(0.75, 0.75, 0.875)`
    ///   and `Screen` `(0.78125, 0.8125, 0.9375)` — every one of them
    ///   differing in all three channels.
    ///
    /// **The source was chosen, not inherited — on magnitude grounds, not
    /// sign grounds.** (0.104.1 corrected the sign clause here, which was
    /// wrong on computation; the magnitude argument, which is the load-
    /// bearing one, was and is right.) The `Screen` sibling's
    /// `(0.25, 0.5, 0.75)` against this fixture's opaque
    /// `(0.75, 0.25, 0.5)` has `Cb > Cs` in red (`0.75 > 0.25`) and
    /// `Cb < Cs` in green *and* blue (`0.25 < 0.5`, `0.5 < 0.75`) — the
    /// same `(>, <, <)` pattern this source's `(0.125, 0.75, 0.875)`
    /// gives, so neither source is better than the other on sign grounds.
    /// What separates them is that the inherited source's differences
    /// `(0.5, 0.25, 0.25)` repeat a magnitude, and this mode's blend term
    /// *is* that magnitude, so a green/blue swizzle inside the blend line
    /// would survive it. This source's `(0.625, 0.5, 0.375)` are three
    /// distinct values. It straddles the sign change either way
    /// (`Cb > Cs` in red, `Cb < Cs` in green and blue), so a dropped
    /// `abs()` — `(0.625, -0.5, -0.375)` — and `Subtract`'s `max(…, 0)` —
    /// `(0.625, 0.0, 0.0)` — each fail in two channels of the opaque half.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_difference_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, strictly inside (0, 1),
        // and straddling the Cb/Cs sign change in the opaque half -- see
        // the doc comment for why this is not the Screen sibling's source.
        let top_rgba = [0.125, 0.75, 0.875, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_difference_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.125, 0.75, 0.875, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::Difference),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader Difference path and composite_tile_cpu disagree across a \
             half-transparent backdrop. In the opaque half a wrong blend formula shows up here; \
             in the transparent half a NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 6 of the
    // blend-mode port: `LinearDodge`, via
    // `TileCompositor::composite_linear_dodge_over_with_opacity` and the
    // `fs_composite_linear_dodge` entry point (0.105.0).
    //
    // **Six tests, mirroring the `Difference` suite's own six**, and
    // omitting the same one every suite since `Darken` omits: an
    // out-of-range-*opacity* case. Since 0.85.1's merge that property
    // lives in a single shared Rust line -- `composite_blend_over_with_
    // opacity`'s own `let opacity = opacity.clamp(0.0, 1.0)`, which every
    // mode's wrapper reaches through -- and the `Multiply` and `Darken`
    // suites already pin it on real hardware. Re-asserting one shared
    // line once per ported mode grows linearly in the number of modes
    // while covering nothing new. That is a disclosed reduction in
    // coverage, not an equivalence claim.
    //
    // The unclamped *source-alpha* case is emphatically **not** omitted:
    // that one tests `fs_composite_linear_dodge`'s own `let a = s.a *
    // opacity.value` line, each WGSL fragment function is separately
    // compiled, and 0.95.0 dropping it for `Lighten` on the opacity-clamp
    // argument was the mistake 0.95.1 had to correct. See the `Lighten`
    // section header above for that account.
    //
    // The six each exercise something genuinely per-entry-point: this
    // mode's own arithmetic *and its clamp*, its own un-premultiply
    // branch, its own spatial addressing, its own opacity-scaled fold,
    // its own unclamped `s.a * opacity` product, and this mode's own
    // collapse to the source alone at a zero accumulator alpha -- **not**
    // "its own separately-compiled `ab > 0.0` guard", which 0.109.0's
    // shared `straight_backdrop()` made false and 0.109.1 corrected here.
    // `LinearDodge` is one of the six modes whose transparent-backdrop
    // test does *not* detect that guard's removal, because its `min(...,
    // 1.0)` clamp launders the NaN. See the `Lighten` section header above
    // and `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    //
    // **Fixture values are chosen against `LinearDodge`'s own
    // degeneracies, which are different again from every prior mode's.**
    // There are four that matter, the fourth being a consequence of the
    // clamp rather than of the sum:
    //
    //   1. `LinearDodge(0, Cs) = Cs` -- indistinguishable from `Normal`,
    //      and from `Screen`, wherever the backdrop channel is zero. So no
    //      operand in the four *solid-colour* fixtures below is `0.0` in
    //      any channel.
    //   2. `LinearDodge(1, Cs) = 1` for every `Cs` -- a saturated
    //      backdrop channel erases the source entirely. So no operand in
    //      those fixtures is `1.0` in any channel either. Together with
    //      degeneracy 1 that is the same "strictly inside `(0, 1)`" rule
    //      the `Screen` and `Difference` suites already keep, for their
    //      own reasons.
    //   3. A channel whose sum exceeds `1.0` is **clamped**, so its output
    //      carries no information about how far past the boundary the
    //      operands were: `(0.5, 0.75)` and `(0.9, 0.9)` both give `1.0`.
    //      A fixture whose every channel clamps therefore cannot
    //      discriminate the operands at all -- only the clamp. So every
    //      solid-colour fixture below has **at least one channel whose sum
    //      stays strictly under `1.0`** (which is what discriminates
    //      `LinearDodge` from `Screen`, its nearest arithmetic neighbour)
    //      **and at least one whose sum exceeds it** (which is what
    //      discriminates it from an unclamped `Cb + Cs`).
    //   4. No channel has `Cb == Cs` in the solid-colour fixtures, so a
    //      transposed operand pair inside the blend line cannot be hidden
    //      by an accidental equality -- though see the symmetry disclosure
    //      below for why that is not what a transpose test rests on here.
    //
    // **"Well past the boundary" here means an excess of ~0.625, with both
    // operands strictly inside `(0, 1)`** -- test 1's blue channel,
    // `0.875 + 0.75 = 1.625`. That is deliberately weaker than
    // `composite_tile_cpu_linear_burn_subtracts_and_clamps_to_zero`, this
    // mode's mirror-image CPU sibling, which reaches an excess of `1.0`
    // (`0.0 + 0.0 - 1.0 = -1.0`) -- but it does so with **both operands at
    // exactly `0.0`**, which degeneracies 1 and 2 above forbid here. With
    // both operands strictly inside `(0, 1)` the sum is strictly under
    // `2.0`, so an excess of `1.0` is unreachable in principle and `0.625`
    // is close to the practical maximum for exact-binary-fraction
    // operands at these magnitudes. Disclosed rather than left as an
    // apparent inconsistency between two sibling suites.
    //
    // **Symmetry, disclosed rather than assumed.** `Cb + Cs = Cs + Cb`, so
    // this mode's blend term is symmetric in backdrop and source, exactly
    // as `Screen`'s and `Difference`'s are. A transposed src/backdrop
    // binding is therefore **not** caught by the blend term alone; what
    // catches it is the surrounding, asymmetric "over" and the per-texel
    // spatial differential in test 3 below.
    //
    // **What is not confused with what.** `min(Cb + Cs, 1)` is this mode.
    // `max(Cb + Cs - 1, 0)` is `LinearBurn`, its exact mirror image and a
    // different mode -- the realistic copy-paste hazard,
    // since the two differ by three characters. As of 0.106.0 `LinearBurn`
    // has its own entry point and its own suite directly below, so that
    // hazard now runs in both directions between two modes that both
    // exist on the GPU; it is no longer "a mode that isn't ported yet".
    // `Cb + Cs - Cb*Cs` is
    // `Screen`. `Cb / (1 - Cs)` is `ColorDodge`, the *other* dodge-family
    // mode, also still CPU-only. Every doc comment below names the wrong
    // answers each of those would give for its own fixture.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for
    // `fs_composite_linear_dodge` -- see PLAN.md's 0.105.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `LinearDodge` counterpart of
    /// `composite_difference_over_with_opacity_takes_the_per_channel_absolute_difference`.
    ///
    /// An opaque `(0.25, 0.75, 0.875)` accumulator under a
    /// `(0.5, 0.5, 0.75)` source at its own `0.5` alpha. The per-channel
    /// sums are `(0.75, 1.25, 1.625)`, so the blend is
    /// `B = (0.75, 1.0, 1.0)` — red under the clamp, green and blue over
    /// it — and the "over" then folds that in at the source's effective
    /// alpha: `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.5, 0.875, 0.9375)` at alpha `1.0`.
    ///
    /// **The fixture straddles the clamp in both directions**, which is
    /// what makes the two closest wrong shaders observable at once:
    ///
    /// - red's sum (`0.75`) is strictly under `1.0`, so the clamp does
    ///   nothing there and red is where `Screen`'s correction term
    ///   (`-Cb*Cs`) shows up as a real difference (`0.4375` against the
    ///   golden `0.5`);
    /// - green's (`1.25`) and blue's (`1.625`) are over `1.0`, so those two
    ///   are where a *dropped* clamp shows up (`1.0` and `1.25` against
    ///   `0.875` and `0.9375`). Blue is `0.625` past the boundary — see
    ///   the suite header on why that is this mode's practical maximum
    ///   with operands strictly inside `(0, 1)`.
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct, none of them is `0.0`
    /// or `1.0`, no channel has `Cb == Cs`, and the source's alpha is
    /// `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.375, 0.625, 0.8125)`;
    /// - the `Screen` arm — the nearest arithmetic neighbour, and this
    ///   mode's own sum with a correction term instead of a clamp:
    ///   `(0.4375, 0.8125, 0.921875)`;
    /// - `LinearBurn`'s `max(Cb + Cs - 1, 0)` — the mirror-image
    ///   copy-paste: `(0.125, 0.5, 0.75)`;
    /// - the `Lighten` arm: `(0.375, 0.75, 0.875)`;
    /// - the `Darken` arm: `(0.25, 0.625, 0.8125)`;
    /// - the `Multiply` arm: `(0.1875, 0.5625, 0.765625)`;
    /// - the `Difference` arm: `(0.25, 0.5, 0.5)`;
    /// - a dropped clamp (`Cb + Cs`): `(0.5, 1.0, 1.25)` — note red
    ///   *agrees*, which is exactly why the fixture needs a clamped
    ///   channel as well as an unclamped one;
    /// - the clamp bound mistyped as `0.5`: `(0.375, 0.625, 0.6875)`;
    /// - the clamp direction reversed (`max(Cb + Cs, 1)`):
    ///   `(0.625, 1.0, 1.25)`;
    /// - `Exclusion`: `(0.375, 0.625, 0.59375)`;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group: not
    ///   caught by the blend term, which is symmetric (`Cb + Cs =
    ///   Cs + Cb`) exactly as `Screen`'s and `Difference`'s are. It is
    ///   caught by the surrounding, asymmetric "over" and by the spatial
    ///   test below. Disclosed rather than claimed away.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_linear_dodge_over_with_opacity_adds_and_clamps_per_channel() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.25, 0.75, 0.875, 1.0];
        let top_rgba = [0.5, 0.5, 0.75, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.25, 0.75, 0.875, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearDodge),
        ]));
        assert_eq!(
            cpu_result,
            (0.5, 0.875, 0.9375, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.5, 0.875, 0.9375, 1.0),
            "LinearDodge(Cb, Cs) = min(Cb + Cs, 1) per channel: sums (0.75, 1.25, 1.625) clamp \
             to (0.75, 1.0, 1.0), folded in at the source's own 0.5 alpha. The Normal arm would \
             give (0.375, 0.625, 0.8125), the Screen arm (0.4375, 0.8125, 0.921875), LinearBurn's \
             max(Cb + Cs - 1, 0) (0.125, 0.5, 0.75), the Lighten arm (0.375, 0.75, 0.875), the \
             Darken arm (0.25, 0.625, 0.8125), the Multiply arm (0.1875, 0.5625, 0.765625), the \
             Difference arm (0.25, 0.5, 0.5), a dropped clamp (0.5, 1.0, 1.25), a clamp bound of \
             0.5 (0.375, 0.625, 0.6875), a reversed clamp max(Cb + Cs, 1) (0.625, 1.0, 1.25) and \
             Exclusion (0.375, 0.625, 0.59375)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `LinearDodge`
    /// counterpart of
    /// `composite_difference_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.5, 0.75, 0.375)` at half opacity and the source
    /// `(0.25, 0.5, 0.25)` — per-channel distinct on both sides, no
    /// channel at `0.0` or `1.0`, no channel with `Cb == Cs`, and the sums
    /// `(0.75, 1.25, 0.625)` straddle the clamp (green over it, red and
    /// blue under), so none of this mode's four degeneracies is in play.
    ///
    /// A missing un-premultiply fails loudly in **all three** channels
    /// here, not just one: the raw premultiplied accumulator is
    /// `(0.25, 0.375, 0.1875)`, and summing against *that* rather than the
    /// recovered straight `(0.5, 0.75, 0.375)` gives
    /// `B = (0.5, 0.875, 0.4375)` where the correct `B` is
    /// `(0.75, 1.0, 0.625)`. Note it also moves green off the clamp
    /// entirely (`0.875 < 1.0`), so the wrong answer is wrong in kind as
    /// well as in value.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Darken`, `Lighten`, `Screen` and `Difference`
    /// siblings document. (For the record it is
    /// `(0.5, 0.75, 0.4375, 1.0)`, since `ab_inv * Cs + ab * B` at
    /// `ab = 0.5` is `0.5 * (0.25, 0.5, 0.25) + 0.5 * (0.75, 1.0, 0.625)`,
    /// and the source's own `a = 1.0` makes `inv = 0.0` — but the
    /// assertion goes through the CPU reference, not through that
    /// literal.)
    fn composite_linear_dodge_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator()
    {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.5, 0.75, 0.375, 1.0];
        let top_rgba = [0.25, 0.5, 0.25, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearDodge),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader LinearDodge path and composite_tile_cpu \
                 diverged by more than {tolerance} against a translucent accumulator ({gpu} vs \
                 {cpu}) -- that is a real finding to report, not a reason to loosen this \
                 assertion. A missing un-premultiply gives (0.375, 0.6875, 0.34375) here. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `LinearDodge` entry point**,
    /// the counterpart of
    /// `composite_difference_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `LinearDodge` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, and
    /// the result genuinely varies texel to texel. The accumulator is
    /// built by a real `composite_over_with_opacity` render pass rather
    /// than seeded, and the **whole** `TILE`x`TILE` result is compared
    /// against [`composite_tile_cpu`]'s own output via [`read_rgba8`] and
    /// its CPU twin [`rgba8_of`].
    ///
    /// The top layer's alpha is `0.75`, so `a = 0.75` and `inv = 0.25`:
    /// **both** terms of `out = inv * d.rgb + a * B` are live, and the
    /// largest output this fixture reaches is `0.9375` rather than a
    /// saturated `1.0` — a wrong-but-clamped value therefore still
    /// differs from the right one in 8-bit.
    ///
    /// **Three disclosures specific to this mode, none of them a claim
    /// that all three channels discriminate at every texel:**
    ///
    /// 1. **[`patterned_texels`]' blue channel is seed-independent** — it
    ///    is a pure function of the texel's quadrant
    ///    (`if x >= half { 0.5 } + if y >= half { 0.25 }`), with no `seed`
    ///    term at all, unlike red (`quarters(x + seed)`) and green
    ///    (`quarters(y + seed)`). So the two layers' blue channels are
    ///    *equal at every texel* and blue's blend term is
    ///    `min(2*Cb, 1)` across the whole tile. Unlike `Difference`,
    ///    whose blend term collapses to `0` under that equality, this one
    ///    stays informative: blue still separates `LinearDodge` from
    ///    `Screen` in three of the four quadrants (`0.4375`/`0.875`/
    ///    `0.9375` against `0.390625`/`0.6875`/`0.890625`; only the
    ///    top-left quadrant, where `Cb == Cs == 0`, agrees — degeneracy 1),
    ///    and it is **genuinely clamped in the bottom-right**, where
    ///    `0.75 + 0.75 = 1.5`. The top-right quadrant sits exactly *on*
    ///    the boundary (`0.5 + 0.5 = 1.0`), which is why the bottom-right
    ///    one is what a dropped clamp fails on in blue.
    /// 2. **Red and green each clamp in one column/row of four and hit
    ///    degeneracy 1 in one more.** With seeds `0` and `1` the top
    ///    layer's red is `quarters(x + 1)` against the bottom's
    ///    `quarters(x)`, so per `x % 4` the sums are
    ///    `0.25 / 0.75 / 1.25 / 0.75`: the third clamps, the first has
    ///    `Cb == 0` (`LinearDodge(0, Cs) = Cs`, indistinguishable from
    ///    `Normal` *and* from `Screen`), and the fourth has `Cs == 0`
    ///    (`quarters` wraps `0.75` back to `0.0`), which is the same
    ///    degeneracy from the other side. That leaves **two of every four**
    ///    columns fully discriminating against `Screen`, and the clamping
    ///    one discriminating against a dropped clamp (`0.875` correct
    ///    against `1.0625`). Green does the same in `y`, so every row and
    ///    column of the tile contributes something and the whole-tile
    ///    comparison covers all of them at once.
    /// 3. **A zero operand does occur here**, in red and green — the
    ///    suite header's degeneracy 1, which the four solid-colour
    ///    fixtures avoid absolutely and this one cannot, since
    ///    `patterned_texels` is shared with five other suites and emits
    ///    exactly `0.0` at `n % 4 == 0`. Disclosed rather than left to be
    ///    discovered; it is why the claim above is "two of four columns",
    ///    not "every texel".
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_linear_dodge_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the LinearDodge comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearDodge),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader LinearDodge path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `LinearDodge` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The counterpart of
    /// `composite_difference_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`, and is also asserted
    /// as an absolute golden: `Cb = (0.375, 0.5, 0.75)`,
    /// `Cs = (0.25, 0.875, 0.875)`, so the sums are
    /// `(0.625, 1.375, 1.625)`, `B = min(sum, 1) = (0.625, 1.0, 1.0)`, and
    /// the fold at `a = 0.5` over an opaque accumulator gives
    /// `0.5 * Cb + 0.5 * B = (0.5, 0.75, 0.875)` at alpha `1.0`.
    /// Non-grey, per-channel-distinct colours are used so a channel
    /// swizzle anywhere in the path fails here too.
    ///
    /// **A second fixture that straddles the clamp, with the split in a
    /// different place than test 1's.** Here red is the only unclamped
    /// channel (`0.625`) and *both* green and blue are over the boundary
    /// (`1.375`, `1.625`), against test 1's one-under/two-over split at
    /// different magnitudes — so the two tests do not share a single
    /// arrangement of which channels the clamp bites in. A dropped clamp
    /// gives `(0.5, 0.9375, 1.1875)` (red agrees, green and blue do not),
    /// `LinearBurn`'s `max(Cb + Cs - 1, 0)` gives
    /// `(0.1875, 0.4375, 0.6875)`, and `Screen` gives
    /// `(0.453125, 0.71875, 0.859375)` — all three channels differ from
    /// the golden for the last two.
    /// The golden also differs from `Normal`'s
    /// `(0.3125, 0.6875, 0.8125)`, `Multiply`'s
    /// `(0.234375, 0.46875, 0.703125)`, `Darken`'s `(0.3125, 0.5, 0.75)`,
    /// `Lighten`'s `(0.375, 0.6875, 0.8125)` and `Difference`'s
    /// `(0.25, 0.4375, 0.4375)`.
    fn composite_linear_dodge_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.375, 0.5, 0.75, 1.0];
        let top_rgba = [0.25, 0.875, 0.875, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::LinearDodge),
        ]));
        assert_eq!(
            cpu_result,
            (0.5, 0.75, 0.875, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.5, 0.75, 0.875, 1.0),
            "min(Cb + Cs, 1) at opacity 0.5: a dropped clamp gives (0.5, 0.9375, 1.1875), \
             LinearBurn's max(Cb + Cs - 1, 0) gives (0.1875, 0.4375, 0.6875) and Screen gives \
             (0.453125, 0.71875, 0.859375)."
        );

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader LinearDodge path and composite_tile_cpu \
                 diverged by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_linear_dodge` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The
    /// counterpart of
    /// `composite_difference_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore the `Lighten` one: this
    /// asserts a line *inside this entry point*, and each WGSL fragment
    /// function is separately compiled, so no other mode's suite covers
    /// it.
    ///
    /// **The source alpha is `2.0` and the `opacity` argument is `1.0`,
    /// not the other way round.** Passing `opacity = 2.0` would prove
    /// nothing: `composite_blend_over_with_opacity` clamps that argument
    /// to `1.0` before it ever reaches the uniform, so `a` would come out
    /// as `1.0` and this test would assert the *clamped* answer. The
    /// unclamped product is only reachable through a source alpha the tile
    /// itself carries.
    ///
    /// **Why the fixture separates all three channels.** With a source
    /// alpha of `2.0` the fold's `inv = 1.0 - a` goes negative, so the
    /// clamped and unclamped answers differ by exactly `b - cb` per
    /// channel — which vanishes only where `min(Cb + Cs, 1) == Cb`, i.e.
    /// where `Cs` is `0` or where `Cb` is already `1`. Neither holds in
    /// any channel here:
    ///
    /// - `cb = (0.375, 0.25, 0.75)`, `Cs = (0.25, 0.625, 0.875)`, so the
    ///   sums are `(0.625, 0.875, 1.625)` and
    ///   `b = min(sum, 1) = (0.625, 0.875, 1.0)` — blue clamped, red and
    ///   green not;
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2b = (0.875, 1.5, 1.25)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped-alpha counterfactual (`a = 1.0`, `inv = 0.0`):
    ///   `b = (0.625, 0.875, 1.0)`, at the same alpha `1.0` — so **alpha
    ///   alone cannot catch this**, and the colour channels are what the
    ///   assertion rests on.
    ///
    /// **Two of the unclamped golden's channels are above `1.0`**
    /// (`1.5` and `1.25`), and that is the point rather than an accident.
    /// It is the mirror image of the `Difference` sibling's negative blue:
    /// this mode's *blend term* is bounded above by `1.0` by construction,
    /// but the *fold* around it is not, and with `inv` negative the fold
    /// overshoots upward instead of undershooting below zero. Neither
    /// `composite_layer_into` nor `fs_composite_linear_dodge` clamps its
    /// output, `Rgba16Float` stores `1.5` and `1.25` exactly, and
    /// [`read_first_texel`] does not clamp on the way back — so the GPU
    /// and CPU are expected to agree on them. A disagreement here would be
    /// a real finding about one of those three, not a reason to move the
    /// fixture. (Confirmed empirically on first run, following the
    /// `Difference` round's own precedent.)
    ///
    /// A dropped *blend* clamp is also visible here, in blue alone:
    /// `b = 1.625` would fold to `2.5` rather than `1.25`.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_linear_dodge_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.375, 0.25, 0.75, 1.0];
        let top_rgba = [0.25, 0.625, 0.875, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::LinearDodge),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_linear_dodge`
        // yields (0.625, 0.875, 1.0, 1.0) instead -- alpha agrees, which
        // is why this is asserted per channel rather than as a single
        // texel comparison whose message would not say where. Green and
        // blue are deliberately above 1.0; see the doc comment.
        for (gpu, expected, channel) in [
            (gr, 0.875, "r"),
            (gg, 1.5, "g"),
            (gb, 1.25, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.625, 0.875, 1.0, 1.0) would mean fs_composite_linear_dodge clamped the \
                 s.a * opacity product, a 1.0 in green or blue would mean something clamped the \
                 *output* to [0, 1], and 2.5 in blue would mean the blend clamp was dropped. \
                 Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_linear_dodge`, on real hardware — the counterpart of
    /// `composite_difference_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_difference` does
    /// not prove it here: this is a sixth, separately-compiled function.
    /// Like `Screen`'s and `Difference`'s and unlike `Darken`'s or
    /// `Lighten`'s, its blend line is *arithmetic* on `cb` rather than a
    /// bare `min`/`max` of the two operands — `min(NaN + x, 1.0)` is
    /// implementation-defined at best and `NaN` in practice, propagated
    /// rather than selected away.
    ///
    /// **"`NaN` in practice" was wrong, and 0.109.0/0.109.1 measured it.**
    /// The guard now lives once in `composite.wgsl`'s shared
    /// `straight_backdrop()`, and on Vulkan/NVIDIA `min(NaN, 1.0)` returns
    /// `1.0` — the clamp launders it. So with the guard deleted this test
    /// still *passes*: `LinearDodge` is one of the six modes for which
    /// removing it is output-equivalent rather than merely undetected,
    /// grouped with `Darken`/`Lighten` here and not with `Screen`'s or
    /// `Difference`'s clamp-free arithmetic. What this test still pins per
    /// entry point is that this mode's own `b` line and fold reduce to the
    /// source alone where `ab == 0.0`. See `composite.wgsl`'s disclosure
    /// beside `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so**
    /// — the reason 0.95.1 gives for the `Lighten` sibling applies
    /// verbatim: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so a uniform fixture cannot
    /// distinguish this entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)`
    /// and a `(0.875, 0.375, 0.25)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.875, 0.375, 0.25, 1.0)`;
    /// - right half (`ab == 1`): the sums are `(1.625, 0.625, 0.75)`, so
    ///   `out = B = (1.0, 0.625, 0.75)`, where `Normal` gives
    ///   `(0.875, 0.375, 0.25)`, `Screen` `(0.96875, 0.53125, 0.625)`,
    ///   `LinearBurn` `(0.625, 0, 0)`, `Multiply`
    ///   `(0.65625, 0.09375, 0.125)`, `Darken` `(0.75, 0.25, 0.25)`,
    ///   `Lighten` `(0.875, 0.375, 0.5)` and `Difference`
    ///   `(0.125, 0.125, 0.25)` — every one of them differing in all
    ///   three channels.
    ///
    /// **REQUIRED DISCLOSURE: this test cannot detect a dropped clamp.**
    /// Red's sum is `1.625`, so an unclamped shader writes `1.625` where
    /// the correct answer is `1.0` — but both sides of this test's
    /// whole-tile comparison quantise through a `[0, 1]` clamp
    /// ([`read_rgba8`]'s on the GPU side, [`rgba8_of`]'s on the CPU
    /// reference's), so `1.625` and `1.0` both land on `255` and the
    /// difference is invisible here. The `read_first_texel` assertion is
    /// no help either: texel 0 is in the *transparent* half, where `b` is
    /// multiplied by zero. This is a real, accepted coverage gap in this
    /// one test, stated rather than papered over — the other five tests in
    /// this suite all kill that mutation (tests 1, 2, 4 and 5 read
    /// unclamped `f16` back, and test 3's clamped channel folds to `0.875`
    /// against `1.0625`, which are distinct in 8-bit). Widening this test
    /// to catch it would mean giving up either the whole-tile 8-bit
    /// comparison or the transparent-half `NaN` check, both of which are
    /// what this test is *for*.
    ///
    /// **The source was chosen, not inherited.** The `Difference`
    /// sibling's `(0.125, 0.75, 0.875)` against this fixture's opaque
    /// `(0.75, 0.25, 0.5)` gives sums `(0.875, 1.0, 1.375)`: green lands
    /// *exactly* on the boundary, where the clamp and its absence agree,
    /// so that source would leave only one channel (blue) able to see a
    /// dropped clamp at all, and its red would be the sole unclamped
    /// channel. This source's `(1.625, 0.625, 0.75)` puts two channels
    /// strictly under the boundary and one strictly over, with nothing
    /// sitting on it — the arrangement the suite header's degeneracy 3
    /// asks for. (That the over-boundary channel's advantage is then
    /// erased by the 8-bit clamp is the disclosure above; the *under*
    /// -boundary channels still discriminate every other mode, which is
    /// what this test's opaque half is really for.)
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_linear_dodge_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, strictly inside (0, 1),
        // and straddling the clamp boundary in the opaque half without
        // landing on it -- see the doc comment for why this is not the
        // Difference sibling's source.
        let top_rgba = [0.875, 0.375, 0.25, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.875, 0.375, 0.25, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearDodge),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader LinearDodge path and composite_tile_cpu disagree across a \
             half-transparent backdrop. In the opaque half a wrong blend formula shows up here \
             (except a dropped clamp -- see this test's doc comment); in the transparent half a \
             NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 7 of the
    // blend-mode port: `LinearBurn`, via
    // `TileCompositor::composite_linear_burn_over_with_opacity` and the
    // `fs_composite_linear_burn` entry point (0.106.0).
    //
    // **Six tests, mirroring the `LinearDodge` suite's own six** -- this
    // mode's exact mirror image, directly above -- and omitting the same
    // one every suite since `Darken` omits: an out-of-range-*opacity*
    // case. Since 0.85.1's merge that property lives in a single shared
    // Rust line -- `composite_blend_over_with_opacity`'s own `let opacity
    // = opacity.clamp(0.0, 1.0)`, which every mode's wrapper reaches
    // through -- and the `Multiply` and `Darken` suites already pin it on
    // real hardware. Re-asserting one shared line once per ported mode
    // grows linearly in the number of modes while covering nothing new.
    // That is a disclosed reduction in coverage, not an equivalence claim.
    //
    // The unclamped *source-alpha* case is emphatically **not** omitted:
    // that one tests `fs_composite_linear_burn`'s own `let a = s.a *
    // opacity.value` line, each WGSL fragment function is separately
    // compiled, and 0.95.0 dropping it for `Lighten` on the opacity-clamp
    // argument was the mistake 0.95.1 had to correct. See the `Lighten`
    // section header above for that account.
    //
    // The six each exercise something genuinely per-entry-point: this
    // mode's own arithmetic *and its clamp*, its own un-premultiply
    // branch, its own spatial addressing, its own opacity-scaled fold,
    // its own unclamped `s.a * opacity` product, and this mode's own
    // collapse to the source alone at a zero accumulator alpha -- **not**
    // "its own separately-compiled `ab > 0.0` guard", which 0.109.0's
    // shared `straight_backdrop()` made false and 0.109.1 corrected here.
    // `LinearBurn` is one of the six modes whose transparent-backdrop test
    // does *not* detect that guard's removal, because its `max(..., 0.0)`
    // clamp launders the NaN. See the `Lighten` section header above and
    // `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    //
    // **Fixture values are chosen against `LinearBurn`'s own
    // degeneracies, and there are six of them -- one more than any prior
    // mode, and the fifth is specific to this mode and was found by
    // testing naive fixtures rather than by reasoning:**
    //
    //   1. `LinearBurn(0, Cs) = 0` for every `Cs <= 1` -- a zero backdrop
    //      channel erases the source entirely, which is the mirror of
    //      `LinearDodge`'s degeneracy 2. So no operand in the four
    //      *solid-colour* fixtures below is `0.0` in any channel.
    //   2. `LinearBurn(1, Cs) = Cs` -- indistinguishable from `Normal`
    //      (and from `Darken`, `Lighten` and `Screen`) wherever the
    //      backdrop channel is saturated, the mirror of `LinearDodge`'s
    //      degeneracy 1. So no operand in those fixtures is `1.0` either.
    //      Together that is the same "strictly inside `(0, 1)`" rule the
    //      `Screen`, `Difference` and `LinearDodge` suites already keep.
    //   3. A channel whose sum falls *under* `1.0` is **clamped** to `0`,
    //      so its output carries no information about how far below the
    //      boundary the operands were: `(0.25, 0.5)` and `(0.1, 0.1)` both
    //      give `0.0`. A fixture whose every channel clamps therefore
    //      cannot discriminate the operands at all -- only the clamp. So
    //      every solid-colour fixture below has **at least one channel
    //      whose sum stays strictly above `1.0`** (which is what
    //      discriminates `LinearBurn` from a shader that dropped the
    //      clamp) **and at least one whose sum falls under it** (which is
    //      what discriminates the clamp from its absence).
    //   4. No channel has `Cb == Cs` in the solid-colour fixtures, so a
    //      transposed operand pair inside the blend line cannot be hidden
    //      by an accidental equality -- though see the symmetry disclosure
    //      below for why that is not what a transpose test rests on here.
    //   5. **New in this round, specific to this mode, and the reason
    //      several otherwise-natural fixtures were rejected.** In an
    //      *unclamped* channel, `Cb + Cs - 1 == |Cb - Cs|` exactly when
    //      `Cb == 0.5` (if `Cs > Cb`) or `Cs == 0.5` (if `Cb > Cs`): the
    //      algebra is `Cb + Cs - 1 = Cs - Cb  <=>  Cb = 0.5`. So an
    //      unclamped channel with *either* operand at exactly `0.5` cannot
    //      discriminate this mode from `Difference`, which is itself on
    //      the GPU (`fs_composite_difference`) and therefore a live
    //      wrong-arm candidate rather than a hypothetical one. **No
    //      unclamped channel in any solid-colour fixture below has an
    //      operand at `0.5`.** Test 3, whose texels come from the shared
    //      [`patterned_texels`] and cannot be chosen, does hit this in one
    //      column and one row -- disclosed in its own doc comment.
    //   6. With both operands strictly inside `(0, 1)` -- which
    //      degeneracies 1 and 2 force -- the sum is strictly above `0.0`,
    //      so a *deficit* of `1.0` is unreachable in principle. That is
    //      the mirror of the `LinearDodge` header's own note, and for the
    //      same reason: this mode's CPU sibling
    //      `composite_tile_cpu_linear_burn_subtracts_and_clamps_to_zero`
    //      reaches a deficit of `1.0` only by putting **both operands at
    //      exactly `0.0`**, which degeneracy 1 forbids here. `0.625` --
    //      test 1's red channel, `0.75 + 0.875 = 1.625` -- is close to the
    //      practical maximum for exact-binary-fraction operands at these
    //      magnitudes. Disclosed rather than left as an apparent
    //      inconsistency between two sibling suites.
    //
    // **Symmetry, disclosed rather than assumed.** `Cb + Cs = Cs + Cb`, so
    // this mode's blend term is symmetric in backdrop and source, exactly
    // as `Screen`'s, `Difference`'s and `LinearDodge`'s are. A transposed
    // src/backdrop binding is therefore **not** caught by the blend term
    // alone; what catches it is the surrounding, asymmetric "over" and the
    // per-texel spatial differential in test 3 below.
    //
    // **What is not confused with what.** `max(Cb + Cs - 1, 0)` is this
    // mode. `min(Cb + Cs, 1)` is `LinearDodge`, its exact mirror image and
    // -- as of 0.105.0 -- **also on the GPU**, which is what makes it the
    // realistic copy-paste hazard here rather than a mode that does not
    // yet exist: the two blend lines differ by three characters, and this
    // one was written from `blend_channel`'s Rust arm rather than copied
    // from that entry point and edited. `Cb * Cs` is `Multiply`, this
    // mode's nearest neighbour in behaviour rather than spelling (both
    // darken; both give `0` for a zero backdrop; they agree exactly where
    // `(1 - Cb) * (1 - Cs) == 0`). `1 - (1 - Cb) / Cs` is `ColorBurn`, the
    // *other* burn-family mode -- **on the GPU as of 0.107.0**, with its
    // own suite directly below this one, so that hazard now runs in both
    // directions between two modes that both exist here. Every doc comment below
    // names the wrong answers each of those would give for its own
    // fixture.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for
    // `fs_composite_linear_burn` -- see PLAN.md's 0.106.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `LinearBurn` counterpart of
    /// `composite_linear_dodge_over_with_opacity_adds_and_clamps_per_channel`.
    ///
    /// An opaque `(0.75, 0.625, 0.25)` accumulator under a
    /// `(0.875, 0.75, 0.125)` source at its own `0.5` alpha. The
    /// per-channel sums are `(1.625, 1.375, 0.375)`, so the blend is
    /// `B = (0.625, 0.375, 0.0)` — red and green over the boundary, blue
    /// under it — and the "over" then folds that in at the source's
    /// effective alpha: `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.6875, 0.5, 0.125)` at alpha `1.0`.
    ///
    /// **The fixture straddles the clamp in both directions**, which is
    /// what makes the two closest wrong shaders observable at once:
    ///
    /// - red's sum (`1.625`) and green's (`1.375`) are strictly above
    ///   `1.0`, so the clamp does nothing there and those two are where
    ///   `Multiply`'s product shows up as a real difference
    ///   (`0.703125`/`0.546875` against the golden `0.6875`/`0.5`). Red is
    ///   `0.625` past the boundary — see the suite header on why that is
    ///   this mode's practical maximum with operands strictly inside
    ///   `(0, 1)`;
    /// - blue's (`0.375`) is under `1.0`, so blue is where a *dropped*
    ///   clamp shows up (`-0.1875` against the golden `0.125`).
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct, none of them is `0.0`
    /// or `1.0`, no channel has `Cb == Cs`, no unclamped channel has an
    /// operand at `0.5` (suite-header degeneracy 5), and the source's
    /// alpha is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.8125, 0.6875, 0.1875)`;
    /// - `LinearDodge`'s `min(Cb + Cs, 1)` — the mirror-image copy-paste,
    ///   and the entry point directly above this one in the shader:
    ///   `(0.875, 0.8125, 0.3125)`;
    /// - the `Multiply` arm — the nearest *behavioural* neighbour:
    ///   `(0.703125, 0.546875, 0.140625)`;
    /// - the `Lighten` arm: `(0.8125, 0.6875, 0.25)`;
    /// - the `Darken` arm: `(0.75, 0.625, 0.1875)`;
    /// - the `Screen` arm: `(0.859375, 0.765625, 0.296875)`;
    /// - the `Difference` arm: `(0.4375, 0.375, 0.1875)`;
    /// - `Exclusion`: `(0.53125, 0.53125, 0.28125)`;
    /// - `Subtract`'s `max(Cb - Cs, 0)`: `(0.375, 0.3125, 0.1875)`;
    /// - a dropped clamp (`Cb + Cs - 1`): `(0.6875, 0.5, -0.1875)` — note
    ///   red and green *agree*, which is exactly why the fixture needs a
    ///   clamped channel as well as unclamped ones. Test 4 below has the
    ///   opposite split, so between them all three channels see it;
    /// - the clamp direction reversed (`min(Cb + Cs - 1, 0)`):
    ///   `(0.375, 0.3125, -0.1875)`;
    /// - the `- 1.0` offset dropped (`max(Cb + Cs, 0)`):
    ///   `(1.1875, 1.0, 0.3125)`;
    /// - the offset mistyped as `0.5`: `(0.9375, 0.75, 0.125)` — note blue
    ///   agrees, so red and green are what catch that one;
    /// - bindings 0 and 3 transposed in a copy-pasted bind group: not
    ///   caught by the blend term, which is symmetric (`Cb + Cs =
    ///   Cs + Cb`) exactly as `Screen`'s, `Difference`'s and
    ///   `LinearDodge`'s are. It is caught by the surrounding, asymmetric
    ///   "over" and by the spatial test below. Disclosed rather than
    ///   claimed away.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction, so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_linear_burn_over_with_opacity_subtracts_and_clamps_per_channel() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.75, 0.625, 0.25, 1.0];
        let top_rgba = [0.875, 0.75, 0.125, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.75, 0.625, 0.25, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearBurn),
        ]));
        assert_eq!(
            cpu_result,
            (0.6875, 0.5, 0.125, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.6875, 0.5, 0.125, 1.0),
            "LinearBurn(Cb, Cs) = max(Cb + Cs - 1, 0) per channel: sums (1.625, 1.375, 0.375) \
             give (0.625, 0.375, 0.0), folded in at the source's own 0.5 alpha. The Normal arm \
             would give (0.8125, 0.6875, 0.1875), LinearDodge's min(Cb + Cs, 1) \
             (0.875, 0.8125, 0.3125), the Multiply arm (0.703125, 0.546875, 0.140625), the \
             Lighten arm (0.8125, 0.6875, 0.25), the Darken arm (0.75, 0.625, 0.1875), the \
             Screen arm (0.859375, 0.765625, 0.296875), the Difference arm \
             (0.4375, 0.375, 0.1875), Exclusion (0.53125, 0.53125, 0.28125), Subtract's \
             max(Cb - Cs, 0) (0.375, 0.3125, 0.1875), a dropped clamp \
             (0.6875, 0.5, -0.1875), a reversed clamp min(Cb + Cs - 1, 0) \
             (0.375, 0.3125, -0.1875), a dropped -1.0 offset (1.1875, 1.0, 0.3125) and an \
             offset mistyped as 0.5 (0.9375, 0.75, 0.125)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `LinearBurn`
    /// counterpart of
    /// `composite_linear_dodge_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.75, 0.625, 0.375)` at half opacity and the
    /// source `(0.875, 0.75, 0.125)` — per-channel distinct on both sides,
    /// no channel at `0.0` or `1.0`, no channel with `Cb == Cs`, and the
    /// sums `(1.625, 1.375, 0.5)` straddle the clamp (red and green above
    /// it, blue under), so none of this mode's six degeneracies is in play:
    /// the two unclamped channels have operands
    /// `(0.75, 0.875)` and `(0.625, 0.75)`, neither carrying a `0.5`.
    ///
    /// **REQUIRED DISCLOSURE, and this is where `LinearBurn` does *not*
    /// mirror `LinearDodge`.** A missing un-premultiply fails in **red and
    /// green only**; blue is *structurally* blind to it. The raw
    /// premultiplied accumulator is `(0.375, 0.3125, 0.1875)`, and summing
    /// against *that* rather than the recovered straight
    /// `(0.75, 0.625, 0.375)` gives `B = (0.25, 0.0625, 0.0)` where the
    /// correct `B` is `(0.625, 0.375, 0.0)` — so the wrong texel is
    /// `(0.5625, 0.40625, 0.0625)` against the golden
    /// `(0.75, 0.5625, 0.0625)`, and blue is **identical**. That is not
    /// bad luck in the fixture: halving `cb` can only push a channel
    /// *further below* the `1.0` boundary, so any channel already clamped
    /// with the correct `cb` stays clamped with the halved one, and this
    /// mode's clamp erases the difference. The `LinearDodge` sibling's own
    /// doc comment claims all three channels fail there, and it is right
    /// for that mode — halving `cb` moves a channel *off* an upper clamp,
    /// which is visible — but the claim does not carry over and is not
    /// copied here. Red and green are what this test rests on; blue would
    /// need an unclamped channel whose halved `cb` still cleared `1.0`,
    /// i.e. `Cs > 1 - Cb/2` with `Cb + Cs > 1`, which is reachable but
    /// would cost the straddle degeneracy 3 asks for.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers.
    /// Compared within `2 * f16::EPSILON`, the same tolerance and the
    /// same reasoning the `Darken`, `Lighten`, `Screen`, `Difference` and
    /// `LinearDodge` siblings document. (For the record it is
    /// `(0.75, 0.5625, 0.0625, 1.0)`, since `ab_inv * Cs + ab * B` at
    /// `ab = 0.5` is
    /// `0.5 * (0.875, 0.75, 0.125) + 0.5 * (0.625, 0.375, 0.0)`, and the
    /// source's own `a = 1.0` makes `inv = 0.0` — but the assertion goes
    /// through the CPU reference, not through that literal.)
    fn composite_linear_burn_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.75, 0.625, 0.375, 1.0];
        let top_rgba = [0.875, 0.75, 0.125, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearBurn),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader LinearBurn path and composite_tile_cpu \
                 diverged by more than {tolerance} against a translucent accumulator ({gpu} vs \
                 {cpu}) -- that is a real finding to report, not a reason to loosen this \
                 assertion. A missing un-premultiply gives (0.5625, 0.40625, 0.0625) here, which \
                 differs in red and green and *agrees* in blue -- see this test's doc comment for \
                 why blue is structurally blind to that mutation in this mode. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `LinearBurn` entry point**,
    /// the counterpart of
    /// `composite_linear_dodge_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `LinearBurn` test here that can catch a V-flip, a
    /// transposed axis, a half-texel UV offset, or a bind-group
    /// transpose: every other one composites uniform tiles and reads
    /// back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, and
    /// the result genuinely varies texel to texel. The accumulator is
    /// built by a real `composite_over_with_opacity` render pass rather
    /// than seeded, and the **whole** `TILE`x`TILE` result is compared
    /// against [`composite_tile_cpu`]'s own output via [`read_rgba8`] and
    /// its CPU twin [`rgba8_of`].
    ///
    /// The top layer's alpha is `0.75`, so `a = 0.75` and `inv = 0.25`:
    /// **both** terms of `out = inv * d.rgb + a * B` are live, which
    /// matters more for this mode than for its siblings, because `B`
    /// itself is `0` across most of this fixture (see below) and the
    /// `inv * d.rgb` term is then the only thing that varies.
    ///
    /// **Three disclosures specific to this mode, none of them a claim
    /// that all three channels discriminate at every texel:**
    ///
    /// 1. **This fixture clamps far more than the `LinearDodge` sibling's
    ///    does, and that is inherent, not a poor choice.**
    ///    [`patterned_texels`] emits only `0.0`/`0.25`/`0.5`/`0.75`, so
    ///    with seeds `0` and `1` the per-`x % 4` red sums are
    ///    `0.25 / 0.75 / 1.25 / 0.75` — **exactly one column in four**
    ///    clears `1.0` and is unclamped, against `LinearDodge`'s two
    ///    columns that stay strictly under its own upper boundary. Green
    ///    does the same in `y`. Blue, which is seed-independent (a pure
    ///    function of the quadrant, `if x >= half { 0.5 } + if y >= half
    ///    { 0.25 }`, so the two layers' blue channels are *equal* at every
    ///    texel and blue's blend term is `max(2*Cb - 1, 0)`), is unclamped
    ///    **only in the bottom-right quadrant**, where `0.75 + 0.75 =
    ///    1.5`; the top-right quadrant sits exactly *on* the boundary
    ///    (`0.5 + 0.5 = 1.0`, where the clamp and its absence agree) and
    ///    the two left quadrants are under it.
    /// 2. **The one unclamped red column, and the one unclamped green row,
    ///    are exactly where suite-header degeneracy 5 bites.** That column
    ///    has `Cb = 0.5, Cs = 0.75`, so `Cb + Cs - 1 = 0.25 = |Cb - Cs|`
    ///    and `Difference` agrees with this mode there. `patterned_texels`
    ///    is shared with six other suites and its values cannot be chosen,
    ///    so this is disclosed rather than avoided. It costs nothing
    ///    overall: the *clamped* columns still separate the two modes
    ///    (`x % 4 == 0` gives `0` against `Difference`'s `48/255`,
    ///    `x % 4 == 3` gives `48/255` against `191/255`), which is why the
    ///    whole-tile comparison still kills a `Difference` mis-dispatch.
    /// 3. **A zero operand does occur here**, in red, green and blue — the
    ///    suite header's degeneracy 1, which the four solid-colour
    ///    fixtures avoid absolutely and this one cannot.
    ///
    /// **What this test does and does not kill, measured rather than
    /// asserted.** Every candidate wrong shader differs from the correct
    /// one in a large majority of the tile's 65,536 texels: `Normal` in
    /// 98.4%, `Lighten`/`Screen`/`LinearDodge`/`Exclusion` in 100%,
    /// `Darken`/`Multiply` in 93.8%, `Difference` in 95.3%, a dropped
    /// clamp and `Subtract` in 81.2%, a reversed clamp and a mistyped
    /// offset in 96.9%, a dropped offset in 100%, and a transposed
    /// src/backdrop binding in 71.9%. A dropped clamp survives in the
    /// unclamped column (where it is a no-op) *and* wherever the negative
    /// result quantises to `0` alongside a correct `0`, but not in
    /// `x % 4 == 1` or `x % 4 == 3`, where the fold's `0.25 * Cb` term
    /// keeps the correct answer positive (`16/255` and `48/255`) while the
    /// unclamped one goes negative and quantises to `0`. So the clamp *is*
    /// covered here, through the fold rather than through `B` alone.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_linear_burn_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the LinearBurn comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearBurn),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader LinearBurn path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `LinearBurn` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The counterpart of
    /// `composite_linear_dodge_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`, and is also asserted
    /// as an absolute golden: `Cb = (0.875, 0.375, 0.25)`,
    /// `Cs = (0.625, 0.25, 0.375)`, so the sums are
    /// `(1.5, 0.625, 0.625)`, `B = max(sum - 1, 0) = (0.5, 0.0, 0.0)`, and
    /// the fold at `a = 0.5` over an opaque accumulator gives
    /// `0.5 * Cb + 0.5 * B = (0.6875, 0.1875, 0.125)` at alpha `1.0`.
    /// Non-grey, per-channel-distinct colours are used so a channel
    /// swizzle anywhere in the path fails here too.
    ///
    /// **A second fixture that straddles the clamp, with the split the
    /// exact opposite of test 1's.** Here red is the only *unclamped*
    /// channel (`1.5`) and both green and blue fall under the boundary
    /// (`0.625`, `0.625`), against test 1's two-over/one-under split — so
    /// the two tests do not share a single arrangement of which channels
    /// the clamp bites in, and between them a dropped clamp is caught in
    /// all three. Here it gives `(0.6875, 0.0, -0.0625)`: red agrees (it
    /// is the unclamped channel), green and blue do not.
    /// `LinearDodge`'s `min(Cb + Cs, 1)` gives `(0.9375, 0.5, 0.4375)`,
    /// `Multiply` gives `(0.7109375, 0.234375, 0.171875)` and `Screen`
    /// gives `(0.9140625, 0.453125, 0.390625)` — all three channels differ
    /// from the golden for each. The golden also differs from `Normal`'s
    /// `(0.75, 0.3125, 0.3125)`, `Darken`'s `(0.75, 0.3125, 0.25)`,
    /// `Lighten`'s `(0.875, 0.375, 0.3125)`, `Difference`'s
    /// `(0.5625, 0.25, 0.1875)`, `Exclusion`'s
    /// `(0.640625, 0.40625, 0.34375)`, `Subtract`'s
    /// `(0.5625, 0.25, 0.125)`, a reversed clamp's
    /// `(0.4375, 0.0, -0.0625)`, a dropped offset's `(1.1875, 0.5, 0.4375)`
    /// and an offset mistyped as `0.5`'s `(0.9375, 0.25, 0.1875)`.
    ///
    /// Note the unclamped channel's operands are `(0.875, 0.625)` —
    /// neither is `0.5`, so suite-header degeneracy 5 does not apply and
    /// red genuinely separates this mode from `Difference` (`0.6875`
    /// against `0.5625`).
    fn composite_linear_burn_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.875, 0.375, 0.25, 1.0];
        let top_rgba = [0.625, 0.25, 0.375, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::LinearBurn),
        ]));
        assert_eq!(
            cpu_result,
            (0.6875, 0.1875, 0.125, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.6875, 0.1875, 0.125, 1.0),
            "max(Cb + Cs - 1, 0) at opacity 0.5: a dropped clamp gives (0.6875, 0.0, -0.0625), \
             LinearDodge's min(Cb + Cs, 1) gives (0.9375, 0.5, 0.4375), Multiply gives \
             (0.7109375, 0.234375, 0.171875) and Screen gives (0.9140625, 0.453125, 0.390625)."
        );

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader LinearBurn path and composite_tile_cpu \
                 diverged by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_linear_burn` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The
    /// counterpart of
    /// `composite_linear_dodge_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore the `Lighten` one: this
    /// asserts a line *inside this entry point*, and each WGSL fragment
    /// function is separately compiled, so no other mode's suite covers
    /// it.
    ///
    /// **The source alpha is `2.0` and the `opacity` argument is `1.0`,
    /// not the other way round.** Passing `opacity = 2.0` would prove
    /// nothing: `composite_blend_over_with_opacity` clamps that argument
    /// to `1.0` before it ever reaches the uniform, so `a` would come out
    /// as `1.0` and this test would assert the *clamped* answer. The
    /// unclamped product is only reachable through a source alpha the tile
    /// itself carries.
    ///
    /// **Why the fixture separates all three channels.** With a source
    /// alpha of `2.0` the fold's `inv = 1.0 - a` goes negative, so the
    /// clamped and unclamped answers differ by exactly `b - cb` per
    /// channel — which vanishes only where `max(Cb + Cs - 1, 0) == Cb`,
    /// i.e. where `Cs` is `1` or where `Cb` is already `0`. Neither holds
    /// in any channel here:
    ///
    /// - `cb = (0.75, 0.375, 0.25)`, `Cs = (0.875, 0.25, 0.375)`, so the
    ///   sums are `(1.625, 0.625, 0.625)` and
    ///   `b = max(sum - 1, 0) = (0.625, 0.0, 0.0)` — red unclamped, green
    ///   and blue clamped;
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2b = (0.5, -0.375, -0.25)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped-alpha counterfactual (`a = 1.0`, `inv = 0.0`):
    ///   `b = (0.625, 0.0, 0.0)`, at the same alpha `1.0` — so **alpha
    ///   alone cannot catch this**, and the colour channels are what the
    ///   assertion rests on.
    ///
    /// **Two of the unclamped golden's channels are below `0.0`**
    /// (`-0.375` and `-0.25`), and that is the point rather than an
    /// accident. It is the exact mirror of the `LinearDodge` sibling's
    /// two channels above `1.0`: this mode's *blend term* is bounded
    /// *below* by `0.0` by construction, but the *fold* around it is not,
    /// and with `inv` negative the fold undershoots below zero instead of
    /// overshooting above one. Neither `composite_layer_into` nor
    /// `fs_composite_linear_burn` clamps its output, `Rgba16Float` stores
    /// `-0.375` and `-0.25` exactly, and [`read_first_texel`] does not
    /// clamp on the way back — so the GPU and CPU are expected to agree on
    /// them. A disagreement here would be a real finding about one of
    /// those three, not a reason to move the fixture. (Confirmed
    /// empirically on first run, following the `Difference` and
    /// `LinearDodge` rounds' own precedent.)
    ///
    /// A dropped *blend* clamp is also visible here, in green and blue:
    /// `b = (0.625, -0.375, -0.375)` would fold to
    /// `(0.5, -1.125, -1.0)` rather than `(0.5, -0.375, -0.25)`. Red is
    /// the unclamped channel and agrees.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_linear_burn_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.75, 0.375, 0.25, 1.0];
        let top_rgba = [0.875, 0.25, 0.375, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::LinearBurn),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_linear_burn`
        // yields (0.625, 0.0, 0.0, 1.0) instead -- alpha agrees, which
        // is why this is asserted per channel rather than as a single
        // texel comparison whose message would not say where. Green and
        // blue are deliberately below 0.0; see the doc comment.
        for (gpu, expected, channel) in [
            (gr, 0.5, "r"),
            (gg, -0.375, "g"),
            (gb, -0.25, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.625, 0.0, 0.0, 1.0) would mean fs_composite_linear_burn clamped the \
                 s.a * opacity product, a 0.0 in green or blue would mean something clamped the \
                 *output* to [0, 1], and (-1.125, -1.0) in green and blue would mean the blend \
                 clamp was dropped. Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_linear_burn`, on real hardware — the counterpart of
    /// `composite_linear_dodge_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_linear_dodge` does
    /// not prove it here: this is a seventh, separately-compiled function.
    /// Like `Screen`'s and `Difference`'s and `LinearDodge`'s, and unlike
    /// `Darken`'s or `Lighten`'s, its blend line is *arithmetic* on `cb`
    /// rather than a bare `min`/`max` of the two operands —
    /// `max(NaN + x - 1, 0)` is implementation-defined at best and `NaN`
    /// in practice, propagated rather than selected away.
    ///
    /// **"`NaN` in practice" was wrong, and 0.109.0/0.109.1 measured it.**
    /// The guard now lives once in `composite.wgsl`'s shared
    /// `straight_backdrop()`, and on Vulkan/NVIDIA `max(NaN, 0.0)` returns
    /// `0.0` — the clamp launders it, exactly as `LinearDodge`'s `min` does.
    /// So with the guard deleted this test still *passes*: `LinearBurn` is
    /// one of the six modes for which removing it is output-equivalent
    /// rather than merely undetected. What this test still pins per entry
    /// point is that this mode's own `b` line and fold reduce to the source
    /// alone where `ab == 0.0`. See `composite.wgsl`'s disclosure beside
    /// `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so**
    /// — the reason 0.95.1 gives for the `Lighten` sibling applies
    /// verbatim: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so a uniform fixture cannot
    /// distinguish this entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)`
    /// and a `(0.625, 0.875, 0.25)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.625, 0.875, 0.25, 1.0)`;
    /// - right half (`ab == 1`): the sums are `(1.375, 1.125, 0.75)`, so
    ///   `out = B = (0.375, 0.125, 0.0)`, where `Normal` gives
    ///   `(0.625, 0.875, 0.25)`, `LinearDodge` `(1.0, 1.0, 0.75)`,
    ///   `Multiply` `(0.46875, 0.21875, 0.125)`, `Screen`
    ///   `(0.90625, 0.90625, 0.625)`, `Darken` `(0.625, 0.25, 0.25)`,
    ///   `Lighten` `(0.75, 0.875, 0.5)`, `Difference`
    ///   `(0.125, 0.625, 0.25)` and `Subtract` `(0.125, 0.0, 0.25)` — every
    ///   one of them differing in at least two channels, and most in all
    ///   three.
    ///
    /// **REQUIRED DISCLOSURE: this test cannot detect a dropped clamp.**
    /// Blue's sum is `0.75`, so an unclamped shader writes `-0.25` where
    /// the correct answer is `0.0` — but both sides of this test's
    /// whole-tile comparison quantise through a `[0, 1]` clamp
    /// ([`read_rgba8`]'s on the GPU side, [`rgba8_of`]'s on the CPU
    /// reference's), so `-0.25` and `0.0` both land on `0` and the
    /// difference is invisible here. Red and green are the *unclamped*
    /// channels, where the mutation is a no-op by definition. The
    /// `read_first_texel` assertion is no help either: texel 0 is in the
    /// *transparent* half, where `b` is multiplied by zero. This is a
    /// real, accepted coverage gap in this one test, stated rather than
    /// papered over — tests 1, 3, 4 and 5 in this suite all kill that
    /// mutation (1, 4 and 5 read unclamped `f16` back; 3's `x % 4 == 1`
    /// and `x % 4 == 3` columns fold to `16/255` and `48/255` against an
    /// unclamped `0`). Widening this test to catch it would mean giving up
    /// either the whole-tile 8-bit comparison or the transparent-half
    /// `NaN` check, both of which are what this test is *for*. It is the
    /// mirror of the `LinearDodge` sibling's own disclosure, where the
    /// same gap appears at the other end of the range.
    ///
    /// **The source's blue channel was chosen as the clamped one, on
    /// purpose.** The backdrop's opaque-half blue is exactly `0.5`, so by
    /// suite-header degeneracy 5 *any* source blue that left the channel
    /// unclamped would make this mode agree with `Difference` there —
    /// `Cb + Cs - 1 == |Cb - Cs|` whenever `Cb == 0.5` and `Cs > Cb`.
    /// Taking `Cs = 0.25` puts blue's sum at `0.75`, safely under the
    /// boundary, and leaves red (`0.75 + 0.625`, neither operand `0.5`)
    /// and green (`0.25 + 0.875`, likewise) as two genuinely unclamped,
    /// genuinely discriminating channels. That is the arrangement
    /// degeneracy 3 asks for — at least one channel each side of the
    /// boundary — reached without tripping degeneracy 5.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_linear_burn_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, strictly inside (0, 1),
        // and straddling the clamp boundary in the opaque half without
        // landing on it -- and with blue as the clamped channel, because
        // the backdrop's own blue is exactly 0.5 and an unclamped blue
        // would collide with Difference (suite-header degeneracy 5). See
        // the doc comment.
        let top_rgba = [0.625, 0.875, 0.25, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_linear_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.625, 0.875, 0.25, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::LinearBurn),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader LinearBurn path and composite_tile_cpu disagree across a \
             half-transparent backdrop. In the opaque half a wrong blend formula shows up here \
             (except a dropped clamp -- see this test's doc comment); in the transparent half a \
             NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 8 of the
    // blend-mode port: `ColorBurn`, via
    // `TileCompositor::composite_color_burn_over_with_opacity` and the
    // `fs_composite_color_burn` entry point (0.107.0).
    //
    // **Eight tests, two more than any suite before it**, and the two
    // extras are the whole reason this mode is different: its formula is
    // the first ported one with *branches*, so it has two edge cases that
    // are not reachable by varying magnitudes at all. The six shared with
    // the `LinearBurn` suite directly above cover this mode's own
    // arithmetic and its `min` clamp, its own un-premultiply branch, its
    // own spatial addressing, its own opacity-scaled fold, its own
    // unclamped `s.a * opacity` product, and this mode's own collapse to
    // the source alone at a zero accumulator alpha -- **not** "its own
    // separately-compiled `ab > 0.0` guard", which 0.109.0's shared
    // `straight_backdrop()` made false and 0.109.1 corrected here.
    // `ColorBurn` is one of the six modes whose transparent-backdrop test
    // does *not* detect that guard's removal, because its guarded branches
    // end in `min(1.0, ...)`, which launders the NaN. See the `Lighten`
    // section header above and `composite.wgsl`'s disclosure beside
    // `straight_backdrop()`. Tests 7 and 8 cover the `Cb == 1` and
    // `Cs == 0` branches respectively, which nothing else here reaches.
    //
    // The same one every suite since `Darken` omits is omitted again: an
    // out-of-range-*opacity* case, which since 0.85.1's merge lives in a
    // single shared Rust line (`composite_blend_over_with_opacity`'s own
    // `let opacity = opacity.clamp(0.0, 1.0)`) that the `Multiply` and
    // `Darken` suites already pin on real hardware. That is a disclosed
    // reduction in coverage, not an equivalence claim. The unclamped
    // *source-alpha* case is emphatically **not** omitted, for the reason
    // the `Lighten` section header gives.
    //
    // **The formula, and it is the first ported one that is not a single
    // expression.** `blend_channel`'s own arm is
    //
    //     if cb == 1.0 { 1.0 }
    //     else if cs == 0.0 { 0.0 }
    //     else { 1.0 - ((1.0 - cb) / cs).min(1.0) }
    //
    // and `color_burn_channel` in `shaders/composite.wgsl` is that,
    // branch for branch, called once per channel. Two of the three
    // branches are per-channel *conditions* rather than arithmetic, which
    // is why a componentwise `vec3` form is not available here: it would
    // need per-lane selects over a division that is undefined in exactly
    // the lanes the conditions exist to exclude.
    //
    // **Both guards are arithmetically redundant under IEEE-754 and both
    // are still required**, and this round measured that rather than
    // reasoning about it. Deleting the `cb == 1.0` guard is killed
    // deterministically by test 7's green channel (the `cs == 0.0` guard
    // then fires where the first should have, returning `0.0` instead of
    // `1.0` -- no division-by-zero semantics involved). Deleting the
    // `cs == 0.0` guard **survives every test in this crate** on
    // Vulkan/NVIDIA, because `(1 - cb) / 0` is `+inf` there,
    // `min(1, inf)` is `1`, and `1 - 1` is the `0.0` the guard would have
    // returned. That survival is the *disclosed, expected* result of a
    // portability guard on IEEE hardware, not a hole in this suite: WGSL
    // specifies division by zero as yielding an indeterminate value, not
    // `+inf`, so on a backend that produces `NaN` instead the guard is
    // what keeps the entry point defined -- and a `NaN` is not absorbed
    // here, because the `ab == 0` half of a tile multiplies `b` by zero
    // and `0.0 * NaN` is `NaN`. No test in this crate can distinguish the
    // two, because no test can make this adapter divide differently. See
    // PLAN.md's 0.107.0 entry.
    //
    // **Branch order is load-bearing**, the first ported mode where any
    // is. `cb == 1.0` is tested first, so the one input where both
    // conditions hold -- a fully white backdrop under a fully black
    // source, an ordinary pixel -- yields `1.0`. Test 7's green channel
    // is the only assertion in this crate that sees the two swapped.
    //
    // **Fixture values are chosen against `ColorBurn`'s own
    // degeneracies:**
    //
    //   1. `ColorBurn(1, Cs) = 1` and `ColorBurn(Cb, 0) = 0` are the two
    //      branch results, and each agrees with a whole family of other
    //      modes (see tests 7 and 8, which disclose exactly which). So
    //      the *arithmetic* fixtures -- tests 1, 2, 4, 5 -- keep every
    //      operand strictly inside `(0, 1)`, and tests 7 and 8 put the
    //      edge case in **one** channel and leave the other two as the
    //      real discriminators.
    //   2. A channel whose quotient `(1 - Cb) / Cs` reaches or exceeds
    //      `1.0` is **clamped** to a blend of `0.0`, so its output
    //      carries no information about how far past the boundary the
    //      operands were. Every solid-colour fixture below therefore has
    //      at least one clamped channel (which discriminates the clamp
    //      from its absence) and at least one unclamped one (which
    //      discriminates the operands).
    //   3. No channel has `Cb == Cs` in the solid-colour fixtures.
    //   4. The quotient must be an exact binary fraction for the golden
    //      to be assertable with `assert_eq!`, which constrains the
    //      fixtures hard: `(1 - Cb) / Cs` is a *division*, so the pair has
    //      to be chosen so it terminates. `1 - Cb = 0.125, Cs = 0.5`
    //      (quotient `0.25`) and `1 - Cb = 0.3125, Cs = 0.625` (quotient
    //      `0.5`) are the two workhorses below. Every such fixture was
    //      hand-derived and then cross-checked against the real
    //      [`composite_tile_cpu`], so a stale literal cannot outlive a
    //      change to either implementation.
    //
    // **Asymmetry, and this is the first ported mode that has it.**
    // `B(Cb, Cs) != B(Cs, Cb)`, so unlike `Multiply`, `Darken`,
    // `Lighten`, `Screen`, `Difference`, `LinearDodge` and `LinearBurn`
    // -- every one of whose suite headers discloses the opposite -- a
    // transposed src/backdrop binding is caught by this mode's *blend
    // term itself*, not only by the asymmetric "over" around it. Test 3's
    // per-texel spatial differential still exists and still catches it
    // (along with a V-flip, a transposed axis and a half-texel UV
    // offset); what changes is that the solid-colour fixtures now catch
    // it too, at any opacity.
    //
    // **What is not confused with what.** `1 - min(1, (1 - Cb) / Cs)` is
    // this mode. `min(1, Cb / (1 - Cs))` is `ColorDodge`, the *other*
    // guarded-division mode, whose branch conditions are `Cb == 0` and
    // `Cs == 1` rather than this one's `Cb == 1` and `Cs == 0`, and which
    // has its own suite (`composite_color_dodge_*`) and its own entry
    // point as of 0.108.0 rather than being CPU-only. (This comment
    // printed that formula with a spurious outer `1 -` until then -- this
    // mode's shape wearing the other's operands -- one of six sites that
    // made the same slip, all corrected in that round, though that round's
    // own count said five: it missed `aurora-app`'s
    // `begin_gpu_composite_tile` `ColorBurn` dispatch-arm comment, fixed
    // but uncounted, and 0.108.1 corrected the count. The distinction it
    // drew was always the right one; the formula was not.)
    // `max(Cb + Cs - 1, 0)` is `LinearBurn`, the other
    // burn-family mode, whose suite is directly above and whose
    // `aurora-app` dispatch arm is directly adjacent -- which is where
    // that hazard actually lives, since nothing about the two blend lines
    // is close. Every doc comment below names the wrong answers each of
    // those would give for its own fixture.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for
    // `fs_composite_color_burn` -- and for this mode that gap is wider
    // than for its predecessors, because the division-by-zero semantics
    // the `cs == 0.0` guard exists to defend against are precisely a
    // per-backend property. See PLAN.md's 0.107.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `ColorBurn` counterpart of
    /// `composite_linear_burn_over_with_opacity_subtracts_and_clamps_per_channel`.
    ///
    /// An opaque `(0.875, 0.6875, 0.25)` accumulator under a
    /// `(0.5, 0.625, 0.375)` source at its own `0.5` alpha. The
    /// per-channel quotients `(1 - Cb) / Cs` are
    /// `(0.125/0.5, 0.3125/0.625, 0.75/0.375) = (0.25, 0.5, 2.0)`, so
    /// `B = 1 - min(1, q) = (0.75, 0.5, 0.0)` — red and green under the
    /// clamp boundary, blue past it — and the "over" then folds that in at
    /// the source's effective alpha: `0.5 * Cb + 0.5 * B` per channel,
    /// giving `(0.8125, 0.59375, 0.125)` at alpha `1.0`.
    ///
    /// **The fixture straddles the clamp in both directions**, which is
    /// what makes the two closest wrong shaders observable at once: red
    /// and green are where a wrong *formula* shows up as a real
    /// difference, and blue (`q = 2.0`) is where a **dropped `min`**
    /// shows up — `1 - 2 = -1`, folding to `-0.375` against the golden
    /// `0.125`.
    ///
    /// **Every plausible wrong answer is a different value here**, which
    /// is why the colours are per-channel distinct, none of the six
    /// operands is `0.0` or `1.0` (so neither branch of this mode's
    /// formula fires — that is tests 7 and 8's job), no channel has
    /// `Cb == Cs`, and the source's alpha is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.6875, 0.65625, 0.3125)`;
    /// - `LinearBurn`'s `max(Cb + Cs - 1, 0)` — the other burn-family mode
    ///   and the adjacent dispatch arm: `B = (0.375, 0.3125, 0.0)`, so
    ///   `(0.625, 0.5, 0.125)`. Note **blue agrees**: both modes clamp
    ///   there. Red and green are what separate the two burns here;
    /// - `ColorDodge`'s `min(1, Cb / (1 - Cs))` — the other
    ///   guarded-division mode: `q = (1.75, 1.833.., 0.4)`, so
    ///   `B = (1.0, 1.0, 0.4)` and `(0.9375, 0.84375, 0.325)`;
    /// - the `Multiply` arm: `(0.65625, 0.55859375, 0.171875)`;
    /// - the `Lighten` arm: `(0.875, 0.6875, 0.3125)`;
    /// - the `Darken` arm: `(0.6875, 0.65625, 0.25)`;
    /// - the `Screen` arm: `(0.90625, 0.78515625, 0.390625)`;
    /// - the `Difference` arm: `(0.625, 0.375, 0.1875)`;
    /// - `LinearDodge`'s `min(Cb + Cs, 1)`: `(0.9375, 0.84375, 0.4375)`;
    /// - a dropped `min` clamp (`1 - q`): `(0.8125, 0.59375, -0.375)` —
    ///   note red and green *agree*, which is exactly why the fixture
    ///   needs a clamped channel as well as unclamped ones;
    /// - the outer `1 -` dropped (`min(1, q)`): `(0.5625, 0.59375, 0.625)`;
    /// - the quotient transposed (`(1 - Cs) / Cb`):
    ///   `q = (0.5/0.875, 0.375/0.6875, 0.625/0.25) =
    ///   (0.571.., 0.545.., 2.5)`, so `B = (0.428.., 0.454.., 0.0)` and
    ///   roughly `(0.651.., 0.571.., 0.125)` — **caught, and this is the
    ///   first ported mode where the blend term itself catches a
    ///   transpose**, because `B` is not symmetric in `Cb`/`Cs`. Every
    ///   suite above discloses the opposite for its own mode. **Red and
    ///   green are what catch it here; blue does not** — the transposed
    ///   `2.5` and the correct `2.0` both exceed the clamp boundary, so
    ///   blue lands on exactly the golden `0.125`. (Corrected in 0.107.1,
    ///   which also fixed four rival greens above: this bullet had the
    ///   blue quotient as `0.15625/0.25 = 0.625`, but `0.15625` is
    ///   `(1 - Cs) * Cb` rather than `1 - Cs`, and it therefore claimed a
    ///   three-channel discriminator where there are two. The bullet's
    ///   conclusion is unchanged — red and green were always doing the
    ///   work.)
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale
    /// literal cannot outlive a change to either implementation. Every
    /// value is an exact binary fraction — the two quotients terminate by
    /// construction (see suite-header degeneracy 4) — so both are
    /// bit-exact `assert_eq!`s rather than tolerance comparisons. That is
    /// sound despite a GPU divide being permitted 2.5 ULP of error,
    /// because the result round-trips through `f16` tile storage, whose
    /// spacing at these magnitudes is orders of magnitude coarser than an
    /// `f32` ULP: an exactly-representable quotient cannot be perturbed
    /// far enough to land on a different `f16`.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_color_burn_over_with_opacity_burns_and_clamps_per_channel() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.875, 0.6875, 0.25, 1.0];
        let top_rgba = [0.5, 0.625, 0.375, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.875, 0.6875, 0.25, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]));
        assert_eq!(
            cpu_result,
            (0.8125, 0.59375, 0.125, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.8125, 0.59375, 0.125, 1.0),
            "ColorBurn(Cb, Cs) = 1 - min(1, (1 - Cb) / Cs) per channel: quotients \
             (0.25, 0.5, 2.0) give B = (0.75, 0.5, 0.0), folded in at the source's own 0.5 \
             alpha. The Normal arm would give (0.6875, 0.65625, 0.3125), LinearBurn's \
             max(Cb + Cs - 1, 0) -- the other burn and the adjacent dispatch arm -- \
             (0.625, 0.5, 0.125) (agreeing in blue, where both clamp), ColorDodge's \
             min(1, Cb / (1 - Cs)) (0.9375, 0.84375, 0.325), the Multiply arm \
             (0.65625, 0.55859375, 0.171875), the Lighten arm (0.875, 0.6875, 0.3125), the \
             Darken arm (0.6875, 0.65625, 0.25), the Screen arm \
             (0.90625, 0.78515625, 0.390625), the Difference arm (0.625, 0.375, 0.1875), \
             LinearDodge's min(Cb + Cs, 1) (0.9375, 0.84375, 0.4375), a dropped min clamp \
             (0.8125, 0.59375, -0.375) and a dropped outer 1 - (0.5625, 0.59375, 0.625)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `ColorBurn` counterpart
    /// of
    /// `composite_linear_burn_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.875, 0.6875, 0.25)` at half opacity and the
    /// source `(0.5, 0.625, 0.375)` at its own alpha `1.0` — the same two
    /// colours as test 1, deliberately, so the *only* thing that differs
    /// between the two tests is which of the shader's own lines is
    /// exercised. Per-channel distinct on both sides, no operand at `0.0`
    /// or `1.0`, no channel with `Cb == Cs`, and the quotients
    /// `(0.25, 0.5, 2.0)` straddle the clamp.
    ///
    /// **REQUIRED DISCLOSURE, and it is the mirror image of the
    /// `LinearBurn` sibling's own.** A missing un-premultiply fails in
    /// **red and green only**; blue is *structurally* blind to it. The raw
    /// premultiplied accumulator is `(0.4375, 0.34375, 0.125)`, and
    /// dividing against *that* rather than the recovered straight
    /// `(0.875, 0.6875, 0.25)` gives quotients
    /// `(0.5625/0.5, 0.65625/0.625, 0.875/0.375) = (1.125, 1.05, 2.33..)`,
    /// so `B` clamps to `(0.0, 0.0, 0.0)` where the correct `B` is
    /// `(0.75, 0.5, 0.0)` — and blue is **identical**. That is not bad luck
    /// in the fixture: halving `cb` can only *increase* `1 - cb` and so
    /// only push a quotient *further past* the `1.0` boundary, so any
    /// channel already clamped with the correct `cb` stays clamped with
    /// the halved one, and this mode's clamp erases the difference. Red
    /// and green are what this test rests on.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers. Compared
    /// within `2 * f16::EPSILON`, the same tolerance and the same
    /// reasoning the `Darken`, `Lighten`, `Screen`, `Difference`,
    /// `LinearDodge` and `LinearBurn` siblings document. (For the record
    /// it is `(0.625, 0.5625, 0.1875, 1.0)`, since `ab_inv * Cs + ab * B`
    /// at `ab = 0.5` is
    /// `0.5 * (0.5, 0.625, 0.375) + 0.5 * (0.75, 0.5, 0.0)`, and the
    /// source's own `a = 1.0` makes `inv = 0.0` — but the assertion goes
    /// through the CPU reference, not through that literal.)
    fn composite_color_burn_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.875, 0.6875, 0.25, 1.0];
        let top_rgba = [0.5, 0.625, 0.375, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader ColorBurn path and composite_tile_cpu \
                 diverged by more than {tolerance} against a translucent accumulator ({gpu} vs \
                 {cpu}) -- that is a real finding to report, not a reason to loosen this \
                 assertion. A missing un-premultiply gives (0.25, 0.3125, 0.1875) here, which \
                 differs in red and green and *agrees* in blue -- see this test's doc comment \
                 for why blue is structurally blind to that mutation in this mode. Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `ColorBurn` entry point**,
    /// the counterpart of
    /// `composite_linear_burn_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `ColorBurn` test here that can catch a V-flip, a
    /// transposed axis or a half-texel UV offset: every other one
    /// composites uniform tiles and reads back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, and
    /// the result genuinely varies texel to texel. The accumulator is
    /// built by a real `composite_over_with_opacity` render pass rather
    /// than seeded, and the **whole** `TILE`x`TILE` result is compared
    /// against [`composite_tile_cpu`]'s own output via [`read_rgba8`] and
    /// its CPU twin [`rgba8_of`].
    ///
    /// The top layer's alpha is `0.75`, so `a = 0.75` and `inv = 0.25`:
    /// **both** terms of `out = inv * d.rgb + a * B` are live, which
    /// matters more for this mode than for most of its siblings, because
    /// `B` is `0` across most of this fixture (see below) and the
    /// `inv * d.rgb` term is then the only thing that varies.
    ///
    /// **Three disclosures specific to this fixture:**
    ///
    /// 1. **It reaches this mode's `cs == 0.0` branch but *not* its
    ///    `cb == 1.0` branch.** [`patterned_texels`] emits only
    ///    `0.0`/`0.25`/`0.5`/`0.75`, so a backdrop channel is never `1.0`
    ///    and the first branch is unreachable here — test 7 is the only
    ///    thing in this crate that exercises it. A source channel *is*
    ///    `0.0` in one red column in four (`x % 4 == 3`, where
    ///    `quarters(x + 1)` wraps to `0.0`) and one green row in four, so
    ///    the second branch runs for real, on a spatially-varying tile,
    ///    alongside the arithmetic branch in the other three quarters.
    /// 2. **Most channels clamp**, which is inherent to a divisor drawn
    ///    from `{0.25, 0.5, 0.75}` against a numerator `1 - Cb >= 0.25`:
    ///    the per-`x % 4` red quotients are
    ///    `1/0.25 = 4`, `0.75/0.5 = 1.5`, `0.5/0.75 = 0.667` and
    ///    `cs == 0` — so **exactly one column in four** is unclamped, and
    ///    green does the same in `y`. Blue is seed-independent (a pure
    ///    function of the quadrant, so the two layers' blue channels are
    ///    *equal* at every texel and blue's blend term is
    ///    `1 - min(1, (1 - Cb) / Cb)`), and is unclamped only in the
    ///    bottom-right quadrant, where `0.25 / 0.75 = 0.333`.
    /// 3. **A zero operand does occur here**, in every channel — the
    ///    suite header's degeneracy 1, which the four solid-colour
    ///    fixtures avoid absolutely and this one cannot.
    ///
    /// **What this test kills, measured rather than asserted:** it is
    /// where mutation (b) of this round's set (transposing the quotient's
    /// operands) and mutation (c) (dropping the `min`) were both confirmed
    /// killed. The dropped-`min` case is worth spelling out, because the
    /// naive expectation is that an 8-bit whole-tile comparison cannot see
    /// it: at `x % 4 == 1` the correct `B` is `0.0` and the fold gives
    /// `0.25 * Cb = 0.0625` → `16/255`, while an unclamped `1 - 1.5 =
    /// -0.5` folds to `0.0625 - 0.375 < 0` → `0`. So the clamp is covered
    /// here *through the fold*, not through `B` alone.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_color_burn_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the ColorBurn comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader ColorBurn path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `ColorBurn` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller
    /// to have clamped. The counterpart of
    /// `composite_linear_burn_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`]
    /// with the same two layers and the same `0.5`, and is also asserted
    /// as an absolute golden: `Cb = (0.9375, 0.6875, 0.25)`,
    /// `Cs = (0.25, 0.625, 0.5)`, so the quotients `(1 - Cb) / Cs` are
    /// `(0.0625/0.25, 0.3125/0.625, 0.75/0.5) = (0.25, 0.5, 1.5)`,
    /// `B = 1 - min(1, q) = (0.75, 0.5, 0.0)`, and the fold at `a = 0.5`
    /// over an opaque accumulator gives `0.5 * Cb + 0.5 * B =
    /// (0.84375, 0.59375, 0.125)` at alpha `1.0`. Non-grey,
    /// per-channel-distinct colours are used so a channel swizzle
    /// anywhere in the path fails here too.
    ///
    /// **A second fixture that straddles the clamp, and deliberately not
    /// at the same distance from the boundary as test 1's.** Blue's
    /// quotient here is `1.5` against test 1's `2.0`, so a dropped `min`
    /// gives `1 - 1.5 = -0.5` folding to `0.5 * 0.25 + 0.5 * (-0.5) =
    /// -0.125` — a different wrong value from test 1's `-0.375`, which
    /// means a shader that clamped to some *other* bound than `1.0` (say
    /// `2.0`) could not agree with both tests at once. Red and green are
    /// the unclamped channels and agree under that mutation, which is why
    /// the fixture needs blue.
    ///
    /// Rival arms, re-derived for this fixture:
    /// `Normal` `(0.59375, 0.65625, 0.375)`,
    /// `LinearBurn`'s `max(Cb + Cs - 1, 0)` `(0.5625, 0.5, 0.125)`
    /// (agreeing in blue, where both clamp — red and green separate the
    /// two burns), `ColorDodge`'s `min(1, Cb / (1 - Cs))`
    /// `(0.96875, 0.84375, 0.375)`, `Multiply` `(0.5859375, 0.55859375,
    /// 0.1875)`, `Screen` `(0.9453125, 0.78515625, 0.4375)`, `Darken`
    /// `(0.59375, 0.65625, 0.25)`, `Lighten` `(0.9375, 0.6875, 0.375)`,
    /// `Difference` `(0.8125, 0.375, 0.25)`, `LinearDodge`'s
    /// `min(Cb + Cs, 1)` `(0.96875, 0.84375, 0.5)`, a dropped outer `1 -`
    /// `(0.59375, 0.59375, 0.625)`, and the quotient transposed
    /// (`(1 - Cs) / Cb`, whose `q` is `(0.8, 0.545.., 2.0)`)
    /// `(0.56875, 0.571.., 0.125)` — every one of them
    /// differing from the golden in at least two channels.
    ///
    /// **Four of those rivals were wrong as first written, and were
    /// recomputed in exact rationals in 0.107.1** (`Multiply` red and
    /// green, `Screen` red and green, `Lighten` green, `LinearDodge` red
    /// and green, plus all three channels of the transposed quotient).
    /// Two things are worth carrying forward from that. First, the green
    /// operands `(Cb, Cs) = (0.6875, 0.625)` are *identical* in this
    /// fixture and test 1's, and the same four wrong greens appeared in
    /// both — one slip copied between siblings, not two independent ones,
    /// which is the argument for deriving such a list mechanically if it
    /// ever grows again. Second, the transposed quotient's blue **agrees**
    /// with the golden (`q' = 2.0` and the correct `q = 1.5` both clamp),
    /// so that row is a two-channel discriminator; the quoted `0.375`
    /// claimed three. The "at least two channels" claim above was
    /// re-checked against every corrected value and still holds, with
    /// `LinearBurn`, the dropped outer `1 -` and the transposed quotient
    /// as the three rows that sit exactly at two. `LinearDodge` and
    /// `ColorDodge` now agree in red and green and differ only in blue —
    /// true of the corrected values, and not a problem, since each is
    /// still distinct from the golden and from each other.
    fn composite_color_burn_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.9375, 0.6875, 0.25, 1.0];
        let top_rgba = [0.25, 0.625, 0.5, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::ColorBurn),
        ]));
        assert_eq!(
            cpu_result,
            (0.84375, 0.59375, 0.125, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.84375, 0.59375, 0.125, 1.0),
            "1 - min(1, (1 - Cb) / Cs) at opacity 0.5: a dropped min clamp gives \
             (0.84375, 0.59375, -0.125), a dropped outer 1 - gives (0.59375, 0.59375, 0.625), \
             LinearBurn's max(Cb + Cs - 1, 0) gives (0.5625, 0.5, 0.125), ColorDodge's \
             min(1, Cb / (1 - Cs)) gives (0.96875, 0.84375, 0.375), Multiply gives \
             (0.5859375, 0.55859375, 0.1875) and Screen gives \
             (0.9453125, 0.78515625, 0.4375)."
        );

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader ColorBurn path and composite_tile_cpu \
                 diverged by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_color_burn` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it
    /// is clamped Rust-side in `composite_blend_over_with_opacity`,
    /// mirroring `composite_layer_into`'s own `let opacity =
    /// opacity.clamp(0.0, 1.0)` followed by an unclamped `sa * opacity`.
    /// `f16` can legally hold a source alpha above `1.0` (invariant
    /// §7.3.1b), so this is a real input, not a synthetic one. The
    /// counterpart of
    /// `composite_linear_burn_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore the `Lighten` one: this
    /// asserts a line *inside this entry point*, and each WGSL fragment
    /// function is separately compiled, so no other mode's suite covers
    /// it.
    ///
    /// **The source alpha is `2.0` and the `opacity` argument is `1.0`,
    /// not the other way round.** Passing `opacity = 2.0` would prove
    /// nothing: `composite_blend_over_with_opacity` clamps that argument
    /// to `1.0` before it ever reaches the uniform, so `a` would come out
    /// as `1.0` and this test would assert the *clamped* answer. The
    /// unclamped product is only reachable through a source alpha the tile
    /// itself carries.
    ///
    /// **Why the fixture separates all three channels.** With a source
    /// alpha of `2.0` the fold's `inv = 1.0 - a` goes negative, so the
    /// clamped and unclamped answers differ by exactly `b - cb` per
    /// channel — which vanishes only where `B == Cb`. Test 1's colours are
    /// reused (`Cb = (0.875, 0.6875, 0.25)`, `Cs = (0.5, 0.625, 0.375)`,
    /// `B = (0.75, 0.5, 0.0)`), and `B != Cb` in all three:
    ///
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2B = (0.625, 0.3125, -0.25)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped-alpha counterfactual (`a = 1.0`, `inv = 0.0`):
    ///   `B = (0.75, 0.5, 0.0)`, at the same alpha `1.0` — so **alpha
    ///   alone cannot catch this**, and the colour channels are what the
    ///   assertion rests on.
    ///
    /// **One of the unclamped golden's channels is below `0.0`**
    /// (`-0.25`), and that is the point rather than an accident: this
    /// mode's *blend term* is bounded to `[0, 1]` by construction (the
    /// `min` bounds it below, and `1 - min(1, q)` cannot exceed `1` for
    /// non-negative `q`), but the *fold* around it is not, and with `inv`
    /// negative the fold undershoots. Neither `composite_layer_into` nor
    /// `fs_composite_color_burn` clamps its output, `Rgba16Float` stores
    /// `-0.25` exactly, and [`read_first_texel`] does not clamp on the way
    /// back — so the GPU and CPU are expected to agree on it. A
    /// disagreement here would be a real finding about one of those three,
    /// not a reason to move the fixture. (Confirmed empirically on first
    /// run, following the `Difference`, `LinearDodge` and `LinearBurn`
    /// rounds' own precedent.)
    ///
    /// A dropped *`min`* clamp is also visible here, in blue:
    /// `B = (0.75, 0.5, -1.0)` would fold to `(0.625, 0.3125, -2.25)`
    /// rather than `(0.625, 0.3125, -0.25)`. Red and green are the
    /// unclamped channels and agree.
    ///
    /// Every value is an exact binary fraction, and the absolute golden
    /// is asserted alongside the [`composite_tile_cpu`] differential so a
    /// clamp added to *both* implementations could not pass either.
    fn composite_color_burn_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.875, 0.6875, 0.25, 1.0];
        let top_rgba = [0.5, 0.625, 0.375, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::ColorBurn),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_color_burn`
        // yields (0.75, 0.5, 0.0, 1.0) instead -- alpha agrees, which
        // is why this is asserted per channel rather than as a single
        // texel comparison whose message would not say where. Blue is
        // deliberately below 0.0; see the doc comment.
        for (gpu, expected, channel) in [
            (gr, 0.625, "r"),
            (gg, 0.3125, "g"),
            (gb, -0.25, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.75, 0.5, 0.0, 1.0) would mean fs_composite_color_burn clamped the \
                 s.a * opacity product, a 0.0 in blue would mean something clamped the \
                 *output* to [0, 1], and -2.25 in blue would mean the min clamp inside the \
                 blend was dropped. Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_color_burn`, on real hardware — the counterpart of
    /// `composite_linear_burn_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of
    /// the entry point, so proving it for `fs_composite_linear_burn` does
    /// not prove it here: this is an eighth, separately-compiled function.
    /// And this mode has **two** divisions inside it rather than one —
    /// the backdrop-recovery `bd.rgb / ab` that the guard protects, and
    /// `color_burn_channel`'s own `(1 - cb) / cs` downstream of it — so a
    /// `NaN` produced by a flattened guard would then be fed straight into
    /// a second division and a `min`, where `min(1.0, NaN)` is itself
    /// implementation-defined. That makes this mode's version of this test
    /// strictly more load-bearing than its siblings', not a formality
    /// copied across.
    ///
    /// **That last claim is the one 0.109.0/0.109.1 overturned.** The guard
    /// now lives once in `composite.wgsl`'s shared `straight_backdrop()`,
    /// and on Vulkan/NVIDIA `min(1.0, NaN)` returns `1.0` — so the second
    /// division's `min` is precisely what *launders* the `NaN` away, and
    /// with the guard deleted this test still *passes*. `ColorBurn` is one
    /// of the six modes for which removing it is output-equivalent rather
    /// than merely undetected; only `multiply`'s, `screen`'s and
    /// `difference`'s versions detect it. What this test still pins per
    /// entry point is that this mode's own three-call `b` and fold reduce to
    /// the source alone where `ab == 0.0`. See `composite.wgsl`'s disclosure
    /// beside `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone,
    /// so that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be
    /// mistaken for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so**
    /// — the reason 0.95.1 gives for the `Lighten` sibling applies
    /// verbatim: with `ab == 0` everywhere, the mode-dependent term `b`
    /// is multiplied by zero in every texel, so a uniform fixture cannot
    /// distinguish this entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)`
    /// and a `(0.625, 0.875, 0.25)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.625, 0.875, 0.25, 1.0)`;
    /// - right half (`ab == 1`): the quotients are
    ///   `(0.25/0.625, 0.75/0.875, 0.5/0.25) = (0.4, 0.857.., 2.0)`, so
    ///   `out = B = (0.6, 0.142.., 0.0)`, where `Normal` gives
    ///   `(0.625, 0.875, 0.25)`, `LinearBurn` `(0.375, 0.125, 0.0)`
    ///   (agreeing in blue, where both clamp), `ColorDodge`
    ///   `(1.0, 1.0, 0.666..)`, `Multiply` `(0.46875, 0.21875, 0.125)`,
    ///   `Screen` `(0.90625, 0.90625, 0.625)`, `Darken`
    ///   `(0.625, 0.25, 0.25)`, `Lighten` `(0.75, 0.875, 0.5)` and
    ///   `Difference` `(0.125, 0.625, 0.25)`.
    ///
    /// **Neither half's values are exact binary fractions in red and
    /// green** (`0.4` and `6/7`), which is why the right half is checked
    /// only through the 8-bit whole-tile differential against
    /// [`composite_tile_cpu`] and never as a literal. Texel 0, in the
    /// transparent half, *is* exact and is asserted with `assert_eq!`.
    ///
    /// **REQUIRED DISCLOSURE: this test cannot detect a dropped `min`
    /// clamp.** Blue's quotient is `2.0`, so an unclamped shader writes
    /// `1 - 2 = -1.0` where the correct answer is `0.0` — but both sides
    /// of this test's whole-tile comparison quantise through a `[0, 1]`
    /// clamp ([`read_rgba8`]'s on the GPU side, [`rgba8_of`]'s on the CPU
    /// reference's), so `-1.0` and `0.0` both land on `0` and the
    /// difference is invisible here. Red and green are the *unclamped*
    /// channels, where the mutation is a no-op by definition. The
    /// `read_first_texel` assertion is no help either: texel 0 is in the
    /// *transparent* half, where `b` is multiplied by zero. This is a
    /// real, accepted coverage gap in this one test, stated rather than
    /// papered over — tests 1, 3, 4 and 5 in this suite all kill that
    /// mutation, and so does `aurora-app`'s own app-level golden, which
    /// reads unclamped `f16` back. Widening this test to catch it would
    /// mean giving up either the whole-tile 8-bit comparison or the
    /// transparent-half `NaN` check, both of which are what this test is
    /// *for*. It is the mirror of the `LinearBurn` and `LinearDodge`
    /// siblings' own disclosures.
    ///
    /// **Neither of this mode's two branches fires anywhere in this
    /// fixture**, which is also disclosed rather than left implied: no
    /// backdrop channel is `1.0` and no source channel is `0.0`. Tests 7
    /// and 8 are what cover those.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile
    /// comparison as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_color_burn_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, strictly inside (0, 1)
        // so neither branch of the formula fires here, and straddling the
        // clamp boundary in the opaque half without landing on it.
        let top_rgba = [0.625, 0.875, 0.25, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.625, 0.875, 0.25, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader ColorBurn path and composite_tile_cpu disagree across a \
             half-transparent backdrop. In the opaque half a wrong blend formula shows up here \
             (except a dropped min clamp -- see this test's doc comment); in the transparent \
             half a NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    #[test]
    /// **The `cb == 1.0` branch, and the only test in either crate that
    /// reaches it** (0.107.0). `ColorBurn`'s formula tests
    /// `Cb == 1` *first*, before `Cs == 0`, so a saturated backdrop
    /// channel yields `1.0` regardless of the source — including where
    /// the source is `0.0` and the second branch would have said `0.0`.
    /// This test's **green** channel is that exact input: `Cb = 1.0`,
    /// `Cs = 0.0`, both conditions true at once.
    ///
    /// **The accumulator must be fully opaque, and that is a requirement
    /// rather than a convenience.** The shader recovers `cb` as
    /// `bd.rgb / ab`, and the branch is a bit-exact `== 1.0`. Only
    /// `ab == 1.0` makes that division an identity; against a fractional
    /// `ab` the quotient need not land exactly on `1.0` and the branch
    /// might not fire at all, which would make this test silently measure
    /// the arithmetic path instead of the branch it exists for. The
    /// translucent-accumulator case is test 2's job.
    ///
    /// Backdrop `(1.0, 1.0, 0.9375)` opaque under a `(0.5, 0.0, 0.25)`
    /// source at opacity `1.0`, so `a = 1.0`, `inv = 0.0` and the whole
    /// result is `B`:
    ///
    /// - red: `Cb == 1`, so `1.0` (and the arithmetic branch would agree —
    ///   `(1 - 1) / 0.5 = 0`, `1 - 0 = 1`, which is *why* the first guard
    ///   is arithmetically redundant on IEEE hardware);
    /// - green: `Cb == 1` **and** `Cs == 0`, so branch order decides:
    ///   `1.0`, not `0.0`;
    /// - blue: neither, so `1 - min(1, 0.0625/0.25) = 0.75`.
    ///
    /// Golden `(1.0, 1.0, 0.75, 1.0)`, every value an exact binary
    /// fraction.
    ///
    /// **REQUIRED DISCLOSURE: red and green agree with a whole family of
    /// other modes here, and blue is the real discriminator of the
    /// *formula*.** `Lighten`, `Screen`, `LinearDodge` and `ColorDodge`
    /// all give `1.0` in a channel with a saturated backdrop, so this
    /// fixture on its own cannot say this is `ColorBurn` rather than any
    /// of them — blue does that (`0.75` against `Lighten`'s `0.9375`,
    /// `Screen`'s `0.953125`, `LinearDodge`'s `1.0`, `ColorDodge`'s
    /// `1.0`, `LinearBurn`'s `0.1875`, `Multiply`'s `0.234375`,
    /// `Darken`'s `0.25` and `Difference`'s `0.6875`). What red and green
    /// *are* for is the branch structure, which no other test reaches:
    ///
    /// - green kills a **swapped branch order** (`Cs == 0` tested first
    ///   would give `0.0`) — the only assertion in either crate that does;
    /// - green also kills the **`cb == 1.0` branch deleted outright**, for
    ///   the same reason and by the same value: the `cs == 0.0` guard then
    ///   fires in its place. Note this is *not* a division-by-zero
    ///   question — no `0/0` is ever evaluated on that path — so unlike
    ///   the `cs == 0.0` guard, deleting the first guard is killed
    ///   deterministically on every backend, which 0.107.0 confirmed by
    ///   running the mutation for real;
    /// - red and green together kill the **`cb == 1.0` branch returning
    ///   `0.0`** instead of `1.0`.
    ///
    /// A dropped `min` clamp is *not* visible here: no channel's quotient
    /// exceeds `1.0` (red's is `0.0`, green's never evaluated, blue's is
    /// `0.25`). Tests 1, 3, 4 and 5 cover that.
    ///
    /// The golden is cross-checked against the real
    /// [`composite_tile_cpu`], whose `blend_channel` arm this branch order
    /// is copied from, and a finiteness check runs first: a `0/0` in the
    /// arithmetic branch would be the plausible failure if the guard were
    /// gone *and* the second guard with it.
    fn composite_color_burn_over_with_opacity_yields_one_where_the_backdrop_is_saturated() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Fully opaque -- see the doc comment: only ab == 1.0 makes the
        // shader's own `bd.rgb / ab` recovery exact enough for a bit-exact
        // `cb == 1.0` branch to reliably fire.
        let bottom_rgba = [1.0, 1.0, 0.9375, 1.0];
        let top_rgba = [0.5, 0.0, 0.25, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (1.0, 1.0, 0.9375, 1.0),
            "setup: the accumulator must be exactly saturated in red and green AND fully \
             opaque, or the shader's own `bd.rgb / ab` recovery need not land on exactly 1.0 \
             and the `cb == 1.0` branch this test exists for might never fire"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]));
        assert_eq!(
            cpu_result,
            (1.0, 1.0, 0.75, 1.0),
            "setup: the golden below must be what composite_tile_cpu's own three-branch \
             blend_channel arm computes -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the ColorBurn branches: {gpu_result:?}. Green's operands \
             are Cb = 1.0 and Cs = 0.0, which is a 0/0 in the arithmetic branch -- reaching it \
             at all means both guards are gone."
        );
        assert_eq!(
            gpu_result,
            (1.0, 1.0, 0.75, 1.0),
            "a saturated backdrop channel must burn to 1.0, and green (Cb = 1.0 AND Cs = 0.0) \
             must take the *first* branch: (1.0, 0.0, 0.75, 1.0) would mean the two branches \
             were tested in the wrong order or the cb == 1.0 branch was deleted, and \
             (0.0, 0.0, 0.75, 1.0) that the cb == 1.0 branch returns 0.0. Blue is what \
             discriminates the formula: Lighten gives 0.9375 there, Screen 0.953125, \
             LinearDodge and ColorDodge 1.0, LinearBurn 0.1875, Multiply 0.234375, Darken 0.25 \
             and Difference 0.6875 -- while red and green agree with several of those, which is \
             why this fixture is about branch structure and blue is about arithmetic."
        );
    }

    #[test]
    /// **The `cs == 0.0` branch** (0.107.0): a zero source channel burns
    /// the backdrop to `0.0`, regardless of how bright the backdrop was —
    /// provided the backdrop is not itself saturated, which is what makes
    /// this test's fixture the complement of test 7's rather than a
    /// variation on it.
    ///
    /// Backdrop `(0.75, 0.6875, 0.90625)` opaque under a
    /// `(0.0, 0.625, 0.375)` source at opacity `1.0`, so `a = 1.0`,
    /// `inv = 0.0` and the whole result is `B`:
    ///
    /// - red: `Cs == 0` and `Cb != 1`, so `0.0` — the branch this test
    ///   exists for;
    /// - green: `1 - min(1, 0.3125/0.625) = 0.5`;
    /// - blue: `1 - min(1, 0.09375/0.375) = 0.75`.
    ///
    /// Golden `(0.0, 0.5, 0.75, 1.0)`, every value an exact binary
    /// fraction.
    ///
    /// **REQUIRED DISCLOSURE: red agrees with a whole family of other
    /// modes here, and green and blue are the real discriminators.**
    /// `Multiply`, `Darken` and `LinearBurn` all give `0.0` in a channel
    /// with a zero source, so this fixture's red channel on its own cannot
    /// say this is `ColorBurn`. Green and blue do (`(0.5, 0.75)` against
    /// `LinearBurn`'s `(0.3125, 0.28125)`, `Multiply`'s
    /// `(0.4296875, 0.33984375)`, `Darken`'s `(0.625, 0.375)`, `Screen`'s
    /// `(0.8828125, 0.94140625)`, `Lighten`'s `(0.6875, 0.90625)`,
    /// `Difference`'s `(0.0625, 0.53125)`, `LinearDodge`'s
    /// `(1.0, 1.0)` and `ColorDodge`'s `(1.0, 1.0)`).
    ///
    /// **What red is for is the branch, and specifically the mutation
    /// where it returns `1.0` instead of `0.0`** — mutation (g) of this
    /// round's set, which this test's red channel is the only assertion in
    /// either crate to kill.
    ///
    /// **What red is *not* able to do, and this is the round's headline
    /// disclosure.** Deleting the `cs == 0.0` guard **entirely** leaves
    /// this test green on Vulkan/NVIDIA, because `(1 - 0.75) / 0.0` is
    /// `+inf` there, `min(1.0, inf)` is `1.0`, and `1 - 1` is exactly the
    /// `0.0` the guard would have returned. That was run for real, not
    /// predicted. The guard is a **portability** guard, not a correctness
    /// one on this adapter: WGSL specifies division by zero as yielding an
    /// indeterminate value, so a backend producing `NaN` instead would
    /// propagate it (`min(1.0, NaN)` is implementation-defined, and the
    /// `ab == 0` half of a tile multiplies `b` by zero, where
    /// `0.0 * NaN` is `NaN`). **No test in this crate can distinguish the
    /// guarded shader from the unguarded one, because no test can make
    /// this adapter divide differently** — stated here rather than papered
    /// over with an assertion that would not mean what it claimed. See
    /// PLAN.md's 0.107.0 entry.
    ///
    /// A dropped `min` clamp is not visible here either: no channel's
    /// quotient exceeds `1.0`. Tests 1, 3, 4 and 5 cover that.
    ///
    /// The golden is cross-checked against the real
    /// [`composite_tile_cpu`], and a finiteness check runs first.
    fn composite_color_burn_over_with_opacity_yields_zero_where_the_source_is_zero() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Red's backdrop is deliberately *not* 1.0: with a zero source
        // channel, a saturated backdrop would take the *first* branch and
        // yield 1.0, which is test 7's green channel. This test is about
        // the second branch, so it needs Cb != 1 in the channel where
        // Cs == 0.
        let bottom_rgba = [0.75, 0.6875, 0.90625, 1.0];
        let top_rgba = [0.0, 0.625, 0.375, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_burn_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.75, 0.6875, 0.90625, 1.0),
            "setup: red's backdrop must be strictly below 1.0, or the cb == 1.0 branch would \
             fire there instead and this test would be a duplicate of test 7"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorBurn),
        ]));
        assert_eq!(
            cpu_result,
            (0.0, 0.5, 0.75, 1.0),
            "setup: the golden below must be what composite_tile_cpu's own three-branch \
             blend_channel arm computes -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the ColorBurn branches: {gpu_result:?}. Red's Cs is 0.0, \
             so an unguarded arithmetic branch divides 0.25 by zero there -- which this adapter \
             resolves to +inf and thence to the correct 0.0, but another need not."
        );
        assert_eq!(
            gpu_result,
            (0.0, 0.5, 0.75, 1.0),
            "a zero source channel over a non-saturated backdrop must burn to 0.0: \
             (1.0, 0.5, 0.75, 1.0) would mean the cs == 0.0 branch returns 1.0. Green and blue \
             are what discriminate the formula, since Multiply, Darken and LinearBurn all give \
             0.0 in red too: LinearBurn gives (0.3125, 0.28125) there, Multiply \
             (0.4296875, 0.33984375), Darken (0.625, 0.375), Screen (0.8828125, 0.94140625), \
             Lighten (0.6875, 0.90625), Difference (0.0625, 0.53125), and both LinearDodge and \
             ColorDodge (1.0, 1.0)."
        );
    }

    // -- Real in-shader blend-mode math on the GPU, slice 9 of the
    // blend-mode port: `ColorDodge`, via
    // `TileCompositor::composite_color_dodge_over_with_opacity` and the
    // `fs_composite_color_dodge` entry point (0.108.0).
    //
    // **Eight tests, the same eight the `ColorBurn` suite directly above
    // carries and for the same reason**: this mode's formula has
    // *branches*, so it has two edge cases no amount of varying magnitudes
    // reaches. Six cover this mode's own arithmetic and its `min` clamp,
    // its own un-premultiply branch, its own spatial addressing, its own
    // opacity-scaled fold, its own unclamped `s.a * opacity` product, and
    // this mode's own collapse to the source alone at a zero accumulator
    // alpha -- **not** "its own separately-compiled `ab > 0.0` guard",
    // which 0.109.0's shared `straight_backdrop()` made false and 0.109.1
    // corrected here. `ColorDodge` is one of the six modes whose
    // transparent-backdrop test does *not* detect that guard's removal,
    // because its guarded branches end in `min(1.0, ...)`, which launders
    // the NaN. See the `Lighten` section header above and
    // `composite.wgsl`'s disclosure beside `straight_backdrop()`. Tests 7
    // and 8 cover the `Cb == 0` and `Cs == 1` branches respectively, which
    // nothing else here reaches.
    //
    // The same one every suite since `Darken` omits is omitted again: an
    // out-of-range-*opacity* case, which since 0.85.1's merge lives in a
    // single shared Rust line (`composite_blend_over_with_opacity`'s own
    // `let opacity = opacity.clamp(0.0, 1.0)`) that the `Multiply` and
    // `Darken` suites already pin on real hardware. That is a disclosed
    // reduction in coverage, not an equivalence claim. The unclamped
    // *source-alpha* case is emphatically **not** omitted, for the reason
    // the `Lighten` section header gives.
    //
    // **The formula.** `blend_channel`'s own arm is
    //
    //     if cb == 0.0 { 0.0 }
    //     else if cs == 1.0 { 1.0 }
    //     else { (cb / (1.0 - cs)).min(1.0) }
    //
    // and `color_dodge_channel` in `shaders/composite.wgsl` is that,
    // branch for branch, called once per channel. Note there is **no outer
    // `1 -`** -- that belongs to `ColorBurn`, and six doc comments in this
    // workspace wrote `ColorDodge`'s formula with one until 0.108.0
    // corrected them. (0.108.0 fixed all six but counted only five,
    // omitting `aurora-app`'s `begin_gpu_composite_tile` `ColorBurn`
    // dispatch-arm comment; 0.108.1 corrected the count, not the fixes.)
    //
    // **Both guards are arithmetically redundant under IEEE-754, both are
    // still required, and the two are redundant for different reasons** --
    // which is the substantive difference from the `ColorBurn` sibling,
    // whose guards are both redundant *through* division-by-zero
    // semantics. Deleting the `cb == 0.0` guard is killed deterministically
    // by test 7's green channel, and **no division by zero is involved on
    // the surviving path at all**: `0 / (1 - cs)` is a real, well-defined
    // `+0` for every `cs < 1`, so `min(1, 0)` is the `0.0` the guard would
    // have returned. It changes the answer only at `cs == 1`, where the
    // *second* guard fires in its place and returns `1.0`. Deleting the
    // `cs == 1.0` guard **survives every test in this crate** on
    // Vulkan/NVIDIA, because `cb / 0` is `+inf` there and `min(1, inf)` is
    // the `1.0` the guard would have returned. That survival is the
    // *disclosed, expected* result of a portability guard on IEEE
    // hardware, not a hole in this suite: WGSL specifies division by zero
    // as yielding an indeterminate value, not `+inf`, so on a backend that
    // produces `NaN` instead the guard is what keeps the entry point
    // defined -- and that `NaN` does reach the output, though by a
    // *different* route than the `ColorBurn` suite header's note describes.
    // Here a `NaN` can only arise when `cb != 0`, since `cb == 0` returns
    // from the *first* guard before any division; and `cb != 0` requires
    // `ab > 0`, because `fs_composite_color_dodge` forces `cb` to exactly
    // zero on the `ab == 0` half. It therefore propagates through `ab * b`
    // with `ab` strictly positive, not by surviving a multiplication by
    // zero. (`ColorBurn`'s first guard is `cb == 1`, which does not fire at
    // `cb == 0`, so *that* mode's `ab == 0` half really does reach the
    // division and really does multiply the result by zero -- which is why
    // its note is written the way it is and this one is not.) No
    // test in this crate can distinguish the two, because no test can make
    // this adapter divide differently. Both results were measured, not
    // predicted; see PLAN.md's 0.108.0 entry.
    //
    // **Branch order is load-bearing, and it is the mirror of
    // `ColorBurn`'s.** `cb == 0.0` is tested first, so the one input where
    // both conditions hold -- a fully black backdrop under a fully white
    // source, an ordinary pixel -- yields `0.0`. Test 7's green channel is
    // the only assertion in this crate that sees the two swapped.
    //
    // **Fixture values are chosen against `ColorDodge`'s own
    // degeneracies:**
    //
    //   1. `ColorDodge(0, Cs) = 0` and `ColorDodge(Cb, 1) = 1` are the two
    //      branch results, and each agrees with a whole family of other
    //      modes (see tests 7 and 8, which disclose exactly which). So the
    //      *arithmetic* fixtures -- tests 1, 2, 4, 5 -- keep every operand
    //      strictly inside `(0, 1)`, and tests 7 and 8 put the edge case in
    //      **one** channel and leave the other two as the real
    //      discriminators.
    //   2. A channel whose quotient `Cb / (1 - Cs)` reaches or exceeds
    //      `1.0` is **clamped** to a blend of `1.0`, so its output carries
    //      no information about how far past the boundary the operands
    //      were. Every solid-colour fixture below therefore has at least
    //      one clamped channel (which discriminates the clamp from its
    //      absence) and at least one unclamped one (which discriminates the
    //      operands).
    //   3. **New for this mode, and checked against every channel of every
    //      fixture here:** `ColorDodge(Cb, Cs) == Cs` exactly when
    //      `Cb == Cs * (1 - Cs)`, so such a channel is indistinguishable
    //      from `Normal`. No solid-colour fixture below has one. Test 3's
    //      patterned fixture *does*, unavoidably, at `x % 4 == 1` -- see
    //      that test's own disclosure.
    //   4. **Unavoidable, and stated once here rather than argued per
    //      test:** this mode clamps exactly when `Cb + Cs >= 1`, which is
    //      exactly when `LinearDodge`'s `min(Cb + Cs, 1)` clamps. A clamped
    //      channel therefore can **never** distinguish `ColorDodge` from
    //      `LinearDodge`. Every claim below about separating the two is a
    //      claim about at most two channels, never three.
    //   5. No channel has `Cb == Cs` in the solid-colour fixtures.
    //   6. The quotient must be an exact binary fraction for the golden to
    //      be assertable with `assert_eq!`, which constrains the fixtures
    //      hard: `Cb / (1 - Cs)` is a *division*, so `1 - Cs` is a power of
    //      two in every solid-colour fixture below -- `Cs` is drawn from
    //      `{0.5, 0.75, 0.875}` throughout, giving divisors
    //      `{0.5, 0.25, 0.125}`. Every such fixture was hand-derived in
    //      exact rationals and then cross-checked against the real
    //      [`composite_tile_cpu`], so a stale literal cannot outlive a
    //      change to either implementation.
    //
    // **Asymmetry.** `B(Cb, Cs) != B(Cs, Cb)`, so -- as with `ColorBurn`,
    // and unlike `Multiply`, `Darken`, `Lighten`, `Screen`, `Difference`,
    // `LinearDodge` and `LinearBurn`, every one of whose suite headers
    // discloses the opposite -- a transposed src/backdrop binding is caught
    // by this mode's *blend term itself*, not only by the asymmetric "over"
    // around it. Test 3's per-texel spatial differential still exists and
    // still catches it (along with a V-flip, a transposed axis and a
    // half-texel UV offset); what changes is that the solid-colour
    // fixtures catch it too, at any opacity.
    //
    // **What is not confused with what.** `min(1, Cb / (1 - Cs))` is this
    // mode. `1 - min(1, (1 - Cb) / Cs)` is `ColorBurn`, the *other*
    // guarded-division mode, whose suite is directly above, whose branch
    // conditions are the mirror of this one's, and whose `aurora-app`
    // dispatch arm is directly adjacent -- which is where that hazard
    // actually lives. `min(Cb + Cs, 1)` is `LinearDodge`, the other
    // dodge-family mode, whose clamp boundary this mode's coincides with
    // exactly (degeneracy 4 above). Every doc comment below names the wrong
    // answers each of those would give for its own fixture.
    //
    // All of them ran on real hardware (`AURORA_REQUIRE_GPU=1`,
    // NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu). That is one backend
    // on one vendor: Metal and DX12 remain unverified for
    // `fs_composite_color_dodge` -- and for this mode that gap is as wide
    // as for `ColorBurn`, because the division-by-zero semantics the
    // `cs == 1.0` guard exists to defend against are precisely a
    // per-backend property. See PLAN.md's 0.108.0 entry.

    #[test]
    /// The plain-arithmetic case, and the `ColorDodge` counterpart of
    /// `composite_color_burn_over_with_opacity_burns_and_clamps_per_channel`.
    ///
    /// An opaque `(0.375, 0.0625, 0.1875)` accumulator under a
    /// `(0.5, 0.75, 0.875)` source at its own `0.5` alpha. The per-channel
    /// quotients `Cb / (1 - Cs)` are
    /// `(0.375/0.5, 0.0625/0.25, 0.1875/0.125) = (0.75, 0.25, 1.5)`, so
    /// `B = min(1, q) = (0.75, 0.25, 1.0)` — red and green under the clamp
    /// boundary, blue past it — and the "over" then folds that in at the
    /// source's effective alpha: `0.5 * Cb + 0.5 * B` per channel, giving
    /// `(0.5625, 0.15625, 0.59375)` at alpha `1.0`.
    ///
    /// **The fixture straddles the clamp in both directions**, which is
    /// what makes the two closest wrong shaders observable at once: red and
    /// green are where a wrong *formula* shows up as a real difference, and
    /// blue (`q = 1.5`) is where a **dropped `min`** shows up — folding to
    /// `0.09375 + 0.75 = 0.84375` against the golden `0.59375`.
    ///
    /// **Blue's quotient is `1.5` rather than a rounder `2.0` on purpose**,
    /// and this is the one place this suite's fixtures differ from the
    /// `ColorBurn` sibling's beyond the formula. Test 2 reuses these exact
    /// colours against a *half-opacity* accumulator, which halves `cb` and
    /// therefore halves the quotient; at `1.5` the halved quotient is
    /// `0.75`, comfortably unclamped, so blue can see a missing
    /// un-premultiply. At `2.0` the halved quotient would be exactly `1.0`,
    /// still clamping, and blue would have been blind to it — which is
    /// precisely the structural blindness the `ColorBurn` sibling has to
    /// disclose. Here it does not, and the difference is one fixture value.
    ///
    /// **Every plausible wrong answer is a different value here**, which is
    /// why the colours are per-channel distinct, none of the six operands
    /// is `0.0` or `1.0` (so neither branch of this mode's formula fires —
    /// that is tests 7 and 8's job), no channel has `Cb == Cs`, no channel
    /// has `Cb == Cs * (1 - Cs)` (suite-header degeneracy 3), and the
    /// source's alpha is `0.5` rather than `1.0`:
    ///
    /// - the `Normal` arm dispatched by mistake: `(0.4375, 0.40625, 0.53125)`
    ///   — and the `Lighten` arm gives *exactly the same triple* here,
    ///   since `Cs > Cb` in all three channels;
    /// - `ColorBurn`'s `1 - min(1, (1 - Cb) / Cs)` — the other
    ///   guarded-division mode and the adjacent dispatch arm:
    ///   `q = (1.25, 1.25, 0.9285..)`, so `B = (0.0, 0.0, 0.0714..)` and
    ///   roughly `(0.1875, 0.03125, 0.1294..)`;
    /// - `LinearDodge`'s `min(Cb + Cs, 1)` — the other dodge-family mode:
    ///   `(0.625, 0.4375, 0.59375)`. **Blue agrees exactly**, and by
    ///   suite-header degeneracy 4 it always will in a clamped channel.
    ///   Red and green are what separate the two dodges here;
    /// - the `Multiply` arm: `(0.28125, 0.0546875, 0.17578125)`;
    /// - the `Darken` arm: `(0.375, 0.0625, 0.1875)` — which is exactly
    ///   `Cb`, this fixture having `Cb < Cs` everywhere;
    /// - the `Screen` arm: `(0.53125, 0.4140625, 0.54296875)`;
    /// - the `Difference` arm: `(0.25, 0.375, 0.4375)`;
    /// - `LinearBurn`'s `max(Cb + Cs - 1, 0)`: `(0.1875, 0.03125, 0.125)`;
    /// - a dropped `min` clamp: `(0.5625, 0.15625, 0.84375)` — note red and
    ///   green *agree*, which is exactly why the fixture needs a clamped
    ///   channel as well as unclamped ones;
    /// - the quotient transposed (`Cs / (1 - Cb)`):
    ///   `q = (0.5/0.625, 0.75/0.9375, 0.875/0.8125) =
    ///   (0.8, 0.8, 1.0769..)`, so `B = (0.8, 0.8, 1.0)` and
    ///   `(0.5875, 0.43125, 0.59375)` — **caught in red and green; blue
    ///   does not catch it**, since the transposed `1.0769..` and the
    ///   correct `1.5` both exceed the boundary and blue lands on exactly
    ///   the golden. A two-channel discriminator, stated as such rather
    ///   than claimed as three.
    ///
    /// The golden is asserted *and* cross-checked against the real
    /// [`composite_tile_cpu`] for the same two layers, so a stale literal
    /// cannot outlive a change to either implementation. Every value is an
    /// exact binary fraction — the three quotients terminate by
    /// construction (see suite-header degeneracy 6) — so both are bit-exact
    /// `assert_eq!`s rather than tolerance comparisons. That is sound
    /// despite a GPU divide being permitted 2.5 ULP of error, because the
    /// result round-trips through `f16` tile storage, whose spacing at
    /// these magnitudes is orders of magnitude coarser than an `f32` ULP:
    /// an exactly-representable quotient cannot be perturbed far enough to
    /// land on a different `f16`.
    ///
    /// `dst` is seeded opaque red first, so a pass that silently wrote
    /// nothing would fail rather than accidentally read as a pass.
    fn composite_color_dodge_over_with_opacity_dodges_and_clamps_per_channel() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.375, 0.0625, 0.1875, 1.0];
        let top_rgba = [0.5, 0.75, 0.875, 0.5];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.375, 0.0625, 0.1875, 1.0),
            "setup: the first pass must really have produced the accumulator the second pass \
             then samples"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]));
        assert_eq!(
            cpu_result,
            (0.5625, 0.15625, 0.59375, 1.0),
            "setup: the hand-derived golden below must be what composite_tile_cpu itself \
             computes for these two layers -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.5625, 0.15625, 0.59375, 1.0),
            "ColorDodge(Cb, Cs) = min(1, Cb / (1 - Cs)) per channel -- no outer 1 -, that is \
             ColorBurn's: quotients (0.75, 0.25, 1.5) give B = (0.75, 0.25, 1.0), folded in at \
             the source's own 0.5 alpha. The Normal arm would give (0.4375, 0.40625, 0.53125) \
             (and so would Lighten, Cs exceeding Cb everywhere here), ColorBurn's \
             1 - min(1, (1 - Cb) / Cs) -- the other guarded division and the adjacent dispatch \
             arm -- roughly (0.1875, 0.03125, 0.1294), LinearDodge's min(Cb + Cs, 1) \
             (0.625, 0.4375, 0.59375) (agreeing in blue, where both clamp -- they always do, \
             the two clamp boundaries being identical), the Multiply arm \
             (0.28125, 0.0546875, 0.17578125), the Darken arm (0.375, 0.0625, 0.1875) (which is \
             Cb itself), the Screen arm (0.53125, 0.4140625, 0.54296875), the Difference arm \
             (0.25, 0.375, 0.4375), LinearBurn's max(Cb + Cs - 1, 0) (0.1875, 0.03125, 0.125), \
             a dropped min clamp (0.5625, 0.15625, 0.84375) and a transposed quotient \
             (0.5875, 0.43125, 0.59375)."
        );
    }

    #[test]
    /// The fractional-accumulator-alpha case: the `ColorDodge` counterpart
    /// of
    /// `composite_color_burn_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator`,
    /// exercising this entry point's own backdrop-recovery branch
    /// (`if (ab > 0.0) { cb = bd.rgb / ab; }`).
    ///
    /// The backdrop is `(0.375, 0.0625, 0.1875)` at half opacity and the
    /// source `(0.5, 0.75, 0.875)` at its own alpha `1.0` — the same two
    /// colours as test 1, deliberately, so the *only* thing that differs
    /// between the two tests is which of the shader's own lines is
    /// exercised. Per-channel distinct on both sides, no operand at `0.0`
    /// or `1.0`, no channel with `Cb == Cs` or `Cb == Cs * (1 - Cs)`, and
    /// the quotients `(0.75, 0.25, 1.5)` straddle the clamp.
    ///
    /// **REQUIRED DISCLOSURE, and it is the *opposite* of the `ColorBurn`
    /// sibling's — this test's own derivation, not that one's inherited.** A
    /// missing un-premultiply fails in **all three channels** here. The raw
    /// premultiplied accumulator is `(0.1875, 0.03125, 0.09375)`, and
    /// dividing against *that* rather than the recovered straight
    /// `(0.375, 0.0625, 0.1875)` halves every quotient — because `1 - Cs` is
    /// untouched and `cb` is halved — giving
    /// `(0.375, 0.125, 0.75)` where the correct quotients are
    /// `(0.75, 0.25, 1.5)`. So `B` becomes `(0.375, 0.125, 0.75)` against
    /// the correct `(0.75, 0.25, 1.0)`, and `blended = 0.5 * Cs + 0.5 * B`
    /// comes out `(0.4375, 0.4375, 0.8125)` against `(0.625, 0.5, 0.9375)`.
    ///
    /// **Why the direction is opposite, and why it is not luck.** For
    /// `ColorBurn` the numerator is `1 - cb`, so halving `cb` *raises* the
    /// quotient and pushes an already-clamped channel further past the
    /// boundary — its clamp then erases the difference, which is what makes
    /// that sibling's blue structurally blind. Here the numerator *is* `cb`,
    /// so halving it *lowers* the quotient, and a clamped channel can become
    /// **unclamped** and therefore visible. That is a property of the
    /// formula, but it is not automatic: at `ab = 0.5` a channel stays blind
    /// exactly when its correct quotient is at least `2.0`. Blue's is `1.5`,
    /// chosen for that reason (see test 1's own note), so `0.75` lands well
    /// clear of the boundary and blue does real work here.
    ///
    /// The expected value is **not hand-derived**: it comes from calling
    /// the real [`composite_tile_cpu`] with the same two layers. Compared
    /// within `2 * f16::EPSILON`, the same tolerance and the same reasoning
    /// the `Darken`, `Lighten`, `Screen`, `Difference`, `LinearDodge`,
    /// `LinearBurn` and `ColorBurn` siblings document. (For the record it is
    /// `(0.625, 0.5, 0.9375, 1.0)`, since `ab_inv * Cs + ab * B` at
    /// `ab = 0.5` is `0.5 * (0.5, 0.75, 0.875) + 0.5 * (0.75, 0.25, 1.0)`,
    /// and the source's own `a = 1.0` makes `inv = 0.0` — but the assertion
    /// goes through the CPU reference, not through that literal.)
    fn composite_color_dodge_over_with_opacity_matches_the_cpu_against_a_translucent_accumulator() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.375, 0.0625, 0.1875, 1.0];
        let top_rgba = [0.5, 0.75, 0.875, 1.0];
        let bottom_opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        // A half-opacity bottom layer leaves a *premultiplied*
        // accumulator whose alpha is 0.5 -- exactly the state whose raw
        // colour is not its straight colour.
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                bottom_opacity,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_accumulator = first_texel(&composite_tile_cpu(&[(
            &bottom_texels,
            bottom_opacity,
            BlendMode::Normal,
        )]));
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, bottom_opacity, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]));

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator, cpu_accumulator,
            "setup: the accumulator the second pass samples must be the premultiplied, \
             fractional-alpha state the CPU path also reaches"
        );
        assert!(
            gpu_accumulator.3 > 0.0 && gpu_accumulator.3 < 1.0,
            "setup: this test is only meaningful with a fractional accumulator alpha, got \
             {gpu_accumulator:?}"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader ColorDodge path and composite_tile_cpu \
                 diverged by more than {tolerance} against a translucent accumulator ({gpu} vs \
                 {cpu}) -- that is a real finding to report, not a reason to loosen this \
                 assertion. A missing un-premultiply gives (0.4375, 0.4375, 0.8125) here, which \
                 differs in *all three* colour channels -- unlike the ColorBurn sibling, where \
                 blue is structurally blind; see this test's doc comment for why the direction \
                 is opposite for this mode. Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **The spatial-addressing test for the `ColorDodge` entry point**,
    /// the counterpart of
    /// `composite_color_burn_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile`
    /// and the only `ColorDodge` test here that can catch a V-flip, a
    /// transposed axis or a half-texel UV offset: every other one
    /// composites uniform tiles and reads back texel 0.
    ///
    /// Both layers are [`patterned_texels`] with *different* seeds, and the
    /// result genuinely varies texel to texel. The accumulator is built by a
    /// real `composite_over_with_opacity` render pass rather than seeded,
    /// and the **whole** `TILE`x`TILE` result is compared against
    /// [`composite_tile_cpu`]'s own output via [`read_rgba8`] and its CPU
    /// twin [`rgba8_of`].
    ///
    /// The top layer's alpha is `0.75`, so `a = 0.75` and `inv = 0.25`:
    /// **both** terms of `out = inv * d.rgb + a * B` are live.
    ///
    /// **Four disclosures specific to this fixture, every one of them
    /// derived from the real pattern rather than assumed:**
    ///
    /// 1. **It reaches this mode's `cb == 0.0` branch but *not* its
    ///    `cs == 1.0` branch.** [`patterned_texels`] emits only
    ///    `0.0`/`0.25`/`0.5`/`0.75`, so a source channel is never `1.0` and
    ///    the second branch is unreachable here — test 8 is the only thing
    ///    in this crate that exercises it. A backdrop channel *is* `0.0` in
    ///    one red column in four (`x % 4 == 0`), one green row in four, and
    ///    the whole top-left blue quadrant, so the first branch runs for
    ///    real, on a spatially-varying tile, alongside the arithmetic branch
    ///    elsewhere. **And because `cs` is never `1.0` here, no `0 / 0` is
    ///    ever evaluated even with the first guard deleted** — which is
    ///    exactly why deleting it survives this test and is killed only by
    ///    test 7.
    /// 2. **Exactly one red column in four clamps**, which is the reverse
    ///    proportion of the `ColorBurn` sibling's and worth stating rather
    ///    than inheriting. The per-`x % 4` red quotients `Cb / (1 - Cs)` are
    ///    `cb == 0` (the guard), `0.25/0.5 = 0.5`, `0.5/0.25 = 2.0` and
    ///    `0.75/1.0 = 0.75` — so two of the four are unclamped, one is
    ///    clamped and one takes the guard. Green does the same in `y`. Blue
    ///    is seed-independent (a pure function of the quadrant, so the two
    ///    layers' blue channels are *equal* at every texel and blue's blend
    ///    term is `min(1, Cb / (1 - Cb))`): `0` in the top-left (the guard),
    ///    `0.25/0.75 = 0.333` unclamped in the bottom-left, exactly `1.0` in
    ///    the top-right and `3.0` in the bottom-right.
    /// 3. **A zero operand does occur here**, in every channel — the
    ///    suite header's degeneracy 1, which the solid-colour fixtures avoid
    ///    absolutely and this one cannot.
    /// 4. **Degeneracy 3 fires here, in one column in four.** At
    ///    `x % 4 == 1` the operands are `Cb = 0.25, Cs = 0.5`, and
    ///    `Cs * (1 - Cs) = 0.25 == Cb` — so `ColorDodge` returns exactly
    ///    `Cs` there and that column is indistinguishable from `Normal`
    ///    (both fold to `0.4375`). This is the only fixture in the suite
    ///    where that happens, it is unavoidable in a pattern drawn from
    ///    quarters, and the other three columns discriminate: at
    ///    `x % 4 == 2` the correct fold is `0.875` against `Normal`'s
    ///    `0.6875`, and at `x % 4 == 3` it is `0.75` against `0.1875`.
    ///
    /// **What this test kills, measured rather than asserted:** it is where
    /// mutation (b) of this round's set (transposing the quotient's
    /// operands) and mutation (c) (dropping the `min`) were both confirmed
    /// killed. Both are worth spelling out in 8-bit terms, because the naive
    /// expectation is that a quantised whole-tile comparison cannot see
    /// either. The dropped `min` shows at `x % 4 == 2`: the correct fold is
    /// `0.25 * 0.5 + 0.75 * 1.0 = 0.875` → `223`, while an unclamped
    /// `q = 2.0` folds to `1.625`, which [`read_rgba8`] clamps to `255`. The
    /// transposed quotient shows in three of the four red columns —
    /// `0 → 48` at `x % 4 == 0`, `112 → 143` at `1`, `191 → 48` at `3`.
    ///
    /// Tolerance is `1` out of 255, the same reasoning
    /// `composite_over_matches_the_golden_image` documents.
    fn composite_color_dodge_over_with_opacity_matches_the_cpu_across_a_spatially_varying_tile() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_texels = patterned_texels(0, 1.0);
        let top_texels = patterned_texels(1, 0.75);

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = tile_from_texels(device, queue, &top_texels, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // The accumulator itself must have survived its render pass
        // texel-for-texel first, or a spatial failure downstream would
        // be ambiguous between the two passes.
        let gpu_accumulator = read_rgba8(device, queue, &backdrop);
        let expected_accumulator = rgba8_of(&bottom_texels);
        assert_whole_tile_matches(
            &gpu_accumulator,
            &expected_accumulator,
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             patterned bottom layer texel for texel, or the ColorDodge comparison below cannot \
             attribute a spatial failure",
        );

        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader ColorDodge path and composite_tile_cpu disagree somewhere on a \
             spatially-varying tile. A whole-tile disagreement of this kind is a wrong-texel \
             bug (V-flip, transpose, UV offset, transposed binding), not precision.",
        );
    }

    #[test]
    /// A non-`1.0` opacity on the `ColorDodge` path, exercising the
    /// `s.a * opacity.value` scale the shader relies on the Rust caller to
    /// have clamped. The counterpart of
    /// `composite_color_burn_over_with_opacity_at_half_opacity_matches_the_cpu`.
    ///
    /// The expected value comes from the real [`composite_tile_cpu`] with
    /// the same two layers and the same `0.5`, and is also asserted as an
    /// absolute golden: `Cb = (0.4375, 0.125, 0.375)`,
    /// `Cs = (0.5, 0.75, 0.875)`, so the quotients `Cb / (1 - Cs)` are
    /// `(0.4375/0.5, 0.125/0.25, 0.375/0.125) = (0.875, 0.5, 3.0)`,
    /// `B = min(1, q) = (0.875, 0.5, 1.0)`, and the fold at `a = 0.5` over
    /// an opaque accumulator gives
    /// `0.5 * Cb + 0.5 * B = (0.65625, 0.3125, 0.6875)` at alpha `1.0`.
    /// Non-grey, per-channel-distinct colours are used so a channel
    /// swizzle anywhere in the path fails here too.
    ///
    /// **A second fixture that straddles the clamp, and deliberately not at
    /// the same distance from the boundary as test 1's.** Blue's quotient
    /// here is `3.0` against test 1's `1.5`, so a dropped `min` gives
    /// `0.5 * 0.375 + 0.5 * 3.0 = 1.6875` — a different wrong value from
    /// test 1's `0.84375`, which means a shader that clamped to some *other*
    /// bound than `1.0` (say `2.0`) could not agree with both tests at once.
    /// Red and green are the unclamped channels and agree under that
    /// mutation, which is why the fixture needs blue.
    ///
    /// **Red's quotient is `0.875`, deliberately just under the boundary.**
    /// Test 1's unclamped channels sit at `0.75` and `0.25`; putting one at
    /// `0.875` means a shader that clamped at some value slightly below
    /// `1.0` would be caught here and nowhere else in the suite.
    ///
    /// No channel has `Cb == Cs` or `Cb == Cs * (1 - Cs)` (`Cs * (1 - Cs)`
    /// is `(0.25, 0.1875, 0.109375)` against
    /// `Cb = (0.4375, 0.125, 0.375)`), and no operand is `0.0` or `1.0`.
    ///
    /// Rival arms, re-derived in exact rationals for this fixture:
    /// `Normal` `(0.46875, 0.4375, 0.625)` (and `Lighten` gives the same,
    /// `Cs` exceeding `Cb` everywhere), `ColorBurn`'s
    /// `1 - min(1, (1 - Cb) / Cs)` roughly `(0.21875, 0.0625, 0.3303..)`,
    /// `LinearDodge`'s `min(Cb + Cs, 1)` `(0.6875, 0.5, 0.6875)` (agreeing
    /// in blue, where both clamp — they always do; suite-header degeneracy
    /// 4), `Multiply` `(0.328125, 0.109375, 0.3515625)`, `Screen`
    /// `(0.578125, 0.453125, 0.6484375)`, `Darken`
    /// `(0.4375, 0.125, 0.375)` (which is `Cb`), `Difference`
    /// `(0.25, 0.375, 0.4375)`, `LinearBurn`'s `max(Cb + Cs - 1, 0)`
    /// `(0.21875, 0.0625, 0.3125)`, a dropped `min` clamp
    /// `(0.65625, 0.3125, 1.6875)`, and the quotient transposed
    /// (`Cs / (1 - Cb)`, whose `q` is `(0.888.., 0.857.., 1.4)`)
    /// roughly `(0.6631.., 0.4910.., 0.6875)` — a **two-channel**
    /// discriminator, blue coinciding because both quotients clamp.
    ///
    /// Every one of those differs from the golden in at least two channels
    /// except the **dropped `min` clamp, which differs in blue alone** —
    /// red and green are the unclamped channels and agree under that
    /// mutation, exactly as disclosed two paragraphs above. `LinearDodge`
    /// and the transposed quotient sit at exactly two, each coinciding in
    /// blue where both clamp. All three are disclosed above rather than
    /// counted as a fuller separation than they are.
    fn composite_color_dodge_over_with_opacity_at_half_opacity_matches_the_cpu() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.4375, 0.125, 0.375, 1.0];
        let top_rgba = [0.5, 0.75, 0.875, 1.0];
        let opacity = 0.5;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::ColorDodge),
        ]));
        assert_eq!(
            cpu_result,
            (0.65625, 0.3125, 0.6875, 1.0),
            "setup: the golden named in this test's doc comment must be what composite_tile_cpu \
             itself computes -- if this fails, the literal is stale, not the GPU"
        );
        let gpu_result = read_first_texel(device, queue, &dst);
        assert_eq!(
            gpu_result,
            (0.65625, 0.3125, 0.6875, 1.0),
            "min(1, Cb / (1 - Cs)) at opacity 0.5: a dropped min clamp gives \
             (0.65625, 0.3125, 1.6875), LinearDodge's min(Cb + Cs, 1) gives \
             (0.6875, 0.5, 0.6875) (agreeing in blue, where both clamp), ColorBurn's \
             1 - min(1, (1 - Cb) / Cs) roughly (0.21875, 0.0625, 0.3303), the Normal arm \
             (0.46875, 0.4375, 0.625) (and Lighten the same), Multiply \
             (0.328125, 0.109375, 0.3515625), Screen (0.578125, 0.453125, 0.6484375), Darken \
             (0.4375, 0.125, 0.375), Difference (0.25, 0.375, 0.4375), and LinearBurn's \
             max(Cb + Cs - 1, 0) (0.21875, 0.0625, 0.3125)."
        );

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: the in-shader ColorDodge path and composite_tile_cpu \
                 diverged by more than {tolerance} at opacity {opacity} ({gpu} vs {cpu}). Full \
                 texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }
    }

    #[test]
    /// **`fs_composite_color_dodge` deliberately does not clamp
    /// `s.a * opacity.value`** — only `opacity` itself is clamped, and it is
    /// clamped Rust-side in `composite_blend_over_with_opacity`, mirroring
    /// `composite_layer_into`'s own `let opacity = opacity.clamp(0.0, 1.0)`
    /// followed by an unclamped `sa * opacity`. `f16` can legally hold a
    /// source alpha above `1.0` (invariant §7.3.1b), so this is a real
    /// input, not a synthetic one. The counterpart of
    /// `composite_color_burn_over_with_opacity_does_not_clamp_a_source_alpha_above_one`,
    /// kept for the reason 0.95.1 had to restore the `Lighten` one: this
    /// asserts a line *inside this entry point*, and each WGSL fragment
    /// function is separately compiled, so no other mode's suite covers it.
    ///
    /// **The source alpha is `2.0` and the `opacity` argument is `1.0`, not
    /// the other way round.** Passing `opacity = 2.0` would prove nothing:
    /// `composite_blend_over_with_opacity` clamps that argument to `1.0`
    /// before it ever reaches the uniform, so `a` would come out as `1.0`
    /// and this test would assert the *clamped* answer. The unclamped
    /// product is only reachable through a source alpha the tile itself
    /// carries.
    ///
    /// **Why the fixture separates all three channels.** With a source alpha
    /// of `2.0` the fold's `inv = 1.0 - a` goes negative, so the clamped and
    /// unclamped answers differ by exactly `b - cb` per channel — which
    /// vanishes only where `B == Cb`. Test 1's colours are reused
    /// (`Cb = (0.375, 0.0625, 0.1875)`, `Cs = (0.5, 0.75, 0.875)`,
    /// `B = (0.75, 0.25, 1.0)`), and `B != Cb` in all three:
    ///
    /// - unclamped (`a = 2.0`, `inv = -1.0`):
    ///   `-cb + 2B = (1.125, 0.4375, 1.8125)` at alpha `2.0 - 1.0 = 1.0`;
    /// - clamped-alpha counterfactual (`a = 1.0`, `inv = 0.0`):
    ///   `B = (0.75, 0.25, 1.0)`, at the same alpha `1.0` — so **alpha alone
    ///   cannot catch this**, and the colour channels are what the assertion
    ///   rests on.
    ///
    /// **Two of the unclamped golden's channels are above `1.0`**
    /// (`1.125` and `1.8125`), and that is the point rather than an
    /// accident — and it is the *mirror* of the `ColorBurn` sibling, whose
    /// equivalent fixture undershoots below `0.0` instead. This mode's
    /// *blend term* is bounded to `[0, 1]` by construction (the `min` bounds
    /// it above, and a quotient of non-negative operands cannot go below
    /// `0`), but the *fold* around it is not, and with `inv` negative a
    /// dodge — whose `B` exceeds `Cb` wherever it brightens — overshoots.
    /// Neither `composite_layer_into` nor `fs_composite_color_dodge` clamps
    /// its output, `Rgba16Float` stores `1.125` and `1.8125` exactly, and
    /// [`read_first_texel`] does not clamp on the way back — so the GPU and
    /// CPU are expected to agree on them. A disagreement here would be a
    /// real finding about one of those three, not a reason to move the
    /// fixture. (Confirmed empirically on first run, following the
    /// `Difference`, `LinearDodge`, `LinearBurn` and `ColorBurn` rounds' own
    /// precedent.)
    ///
    /// A dropped *`min`* clamp is also visible here, in blue:
    /// `B = (0.75, 0.25, 1.5)` would fold to `(1.125, 0.4375, 2.8125)`
    /// rather than `(1.125, 0.4375, 1.8125)`. Red and green are the
    /// unclamped channels and agree.
    ///
    /// Every value is an exact binary fraction, and the absolute golden is
    /// asserted alongside the [`composite_tile_cpu`] differential so a clamp
    /// added to *both* implementations could not pass either.
    fn composite_color_dodge_over_with_opacity_does_not_clamp_a_source_alpha_above_one() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        let bottom_rgba = [0.375, 0.0625, 0.1875, 1.0];
        let top_rgba = [0.5, 0.75, 0.875, 2.0]; // alpha > 1.0, legal in f16
        let opacity = 1.0;

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                opacity,
            );
        });

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, opacity, BlendMode::ColorDodge),
        ]));
        let gpu_result = read_first_texel(device, queue, &dst);

        let tolerance = 2.0 * f32::from(f16::EPSILON);
        let (gr, gg, gb, ga) = gpu_result;
        let (cr, cg, cb, ca) = cpu_result;
        for (gpu, cpu, channel) in [(gr, cr, "r"), (gg, cg, "g"), (gb, cb, "b"), (ga, ca, "a")] {
            assert!(
                (gpu - cpu).abs() <= tolerance,
                "channel {channel}: a source alpha above 1.0 must reach composite_tile_cpu's \
                 own formula unclamped, not silently clamped to 1.0 first ({gpu} vs {cpu}). \
                 Full texels: {gpu_result:?} vs {cpu_result:?}"
            );
        }

        // The absolute golden, hand-derived in the doc comment above.
        // A `min(s.a * opacity.value, 1.0)` in `fs_composite_color_dodge`
        // yields (0.75, 0.25, 1.0, 1.0) instead -- alpha agrees, which is
        // why this is asserted per channel rather than as a single texel
        // comparison whose message would not say where. Red and blue are
        // deliberately above 1.0; see the doc comment.
        for (gpu, expected, channel) in [
            (gr, 1.125, "r"),
            (gg, 0.4375, "g"),
            (gb, 1.8125, "b"),
            (ga, 1.0, "a"),
        ] {
            assert!(
                (gpu - expected).abs() <= tolerance,
                "channel {channel}: expected {expected} from the unclamped fold; got {gpu}. \
                 (0.75, 0.25, 1.0, 1.0) would mean fs_composite_color_dodge clamped the \
                 s.a * opacity product, a 1.0 in red or blue would mean something clamped the \
                 *output* to [0, 1], and 2.8125 in blue would mean the min clamp inside the \
                 blend was dropped. Full texel: {gpu_result:?}"
            );
        }
    }

    #[test]
    /// The `if (ab > 0.0)` guard's **untaken** branch in
    /// `fs_composite_color_dodge`, on real hardware — the counterpart of
    /// `composite_color_burn_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.
    ///
    /// Whether a shader compiler flattens that branch and evaluates the
    /// `0.0 / 0.0` on both sides is a property of the *backend*, not of the
    /// entry point, so proving it for `fs_composite_color_burn` does not
    /// prove it here: this is a ninth, separately-compiled function. And
    /// this mode has **two** divisions inside it rather than one — the
    /// backdrop-recovery `bd.rgb / ab` that the guard protects, and
    /// `color_dodge_channel`'s own `cb / (1 - cs)` downstream of it — so a
    /// `NaN` produced by a flattened guard would then be fed straight into a
    /// second division and a `min`, where `min(1.0, NaN)` is itself
    /// implementation-defined. That makes this mode's version of this test
    /// strictly more load-bearing than its commutative siblings', not a
    /// formality copied across.
    ///
    /// **That last claim is the one 0.109.0/0.109.1 overturned**, exactly as
    /// for the `ColorBurn` sibling. The guard now lives once in
    /// `composite.wgsl`'s shared `straight_backdrop()`, and on
    /// Vulkan/NVIDIA `min(1.0, NaN)` returns `1.0`, so the second division's
    /// `min` *launders* the `NaN` and this test still passes with the guard
    /// deleted. `ColorDodge` is one of the six modes for which removing it
    /// is output-equivalent rather than merely undetected. What this test
    /// still pins per entry point is that this mode's own three-call `b` and
    /// fold reduce to the source alone where `ab == 0.0`. See
    /// `composite.wgsl`'s disclosure beside `straight_backdrop()`.
    ///
    /// Where `ab == 0.0` the whole composite reduces to the source alone, so
    /// that half of the tile is asserted to be exactly that — a `NaN`
    /// leaking out of the untaken divide would fail both the finiteness
    /// check and the value check, and (`NaN != NaN`) could not be mistaken
    /// for a pass.
    ///
    /// **The backdrop is deliberately half transparent, not uniformly so** —
    /// the reason 0.95.1 gives for the `Lighten` sibling applies verbatim:
    /// with `ab == 0` everywhere, the mode-dependent term `b` is multiplied
    /// by zero in every texel, so a uniform fixture cannot distinguish this
    /// entry point's formula from any other's. With
    /// [`half_transparent_texels`]'s opaque half at `(0.75, 0.25, 0.5)` and
    /// a `(0.125, 0.625, 0.875)` source:
    ///
    /// - left half (`ab == 0`): `blended = Cs`, `out = Cs` — the untaken
    ///   branch, `(0.125, 0.625, 0.875, 1.0)`;
    /// - right half (`ab == 1`): the quotients `Cb / (1 - Cs)` are
    ///   `(0.75/0.875, 0.25/0.375, 0.5/0.125) = (0.857.., 0.666.., 4.0)`, so
    ///   `out = B = (0.857.., 0.666.., 1.0)`, where `Normal` gives
    ///   `(0.125, 0.625, 0.875)`, `ColorBurn` `(0.0, 0.0, 0.428..)`,
    ///   `LinearDodge` `(0.875, 0.875, 1.0)` (agreeing in blue, where both
    ///   clamp), `Multiply` `(0.09375, 0.15625, 0.4375)`, `Screen`
    ///   `(0.78125, 0.71875, 0.9375)`, `Darken` `(0.125, 0.25, 0.5)`,
    ///   `Lighten` `(0.75, 0.625, 0.875)`, `Difference`
    ///   `(0.625, 0.375, 0.375)` and `LinearBurn` `(0.0, 0.0, 0.375)`.
    ///
    /// **The source's red is `0.125`, not a mid value, and that is
    /// deliberate.** The opaque half's `Cb` is fixed by
    /// [`half_transparent_texels`], so the only way to give this fixture an
    /// *unclamped* channel is to pick a source channel small enough that
    /// `1 - Cs` exceeds `Cb`. `Cs = 0.125` puts red at `0.857..` and
    /// `Cs = 0.625` puts green at `0.666..`, leaving only blue clamped — two
    /// unclamped channels against the `ColorBurn` sibling's one.
    ///
    /// **Neither half's values are exact binary fractions in red and green**
    /// (`6/7` and `2/3`), which is why the right half is checked only through
    /// the 8-bit whole-tile differential against [`composite_tile_cpu`] and
    /// never as a literal. Texel 0, in the transparent half, *is* exact and
    /// is asserted with `assert_eq!`.
    ///
    /// **REQUIRED DISCLOSURE: this test cannot detect a dropped `min`
    /// clamp.** Blue's quotient is `4.0`, so an unclamped shader writes
    /// `4.0` where the correct answer is `1.0` — but both sides of this
    /// test's whole-tile comparison quantise through a `[0, 1]` clamp
    /// ([`read_rgba8`]'s on the GPU side, [`rgba8_of`]'s on the CPU
    /// reference's), so `4.0` and `1.0` both land on `255` and the
    /// difference is invisible here. Red and green are the *unclamped*
    /// channels, where the mutation is a no-op by definition. The
    /// `read_first_texel` assertion is no help either: texel 0 is in the
    /// *transparent* half, where `b` is multiplied by zero. This is a real,
    /// accepted coverage gap in this one test, stated rather than papered
    /// over — tests 1, 3, 4 and 5 in this suite all kill that mutation, and
    /// so does `aurora-app`'s own app-level golden, which reads unclamped
    /// `f16` back. Widening this test to catch it would mean giving up
    /// either the whole-tile 8-bit comparison or the transparent-half `NaN`
    /// check, both of which are what this test is *for*. It is the mirror of
    /// the `ColorBurn`, `LinearBurn` and `LinearDodge` siblings' own
    /// disclosures — with the one difference that this mode's unclamped
    /// value overshoots `1.0` rather than undershooting `0.0`, and the
    /// quantiser erases both.
    ///
    /// **Neither of this mode's two branches fires anywhere in this
    /// fixture**, which is also disclosed rather than left implied: no
    /// backdrop channel is `0.0` in the opaque half and no source channel is
    /// `1.0`. Tests 7 and 8 are what cover those.
    ///
    /// A `NaN` in the left half is still caught by the whole-tile comparison
    /// as well as by the explicit finiteness check on texel 0:
    /// [`read_rgba8`]'s `clamp` maps `NaN` to `0`, which cannot match the
    /// CPU reference's real value there.
    ///
    /// Verified on Vulkan/NVIDIA only. Metal's and DX12's own shader
    /// compilers are unverified for this specific branch.
    fn composite_color_dodge_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent()
     {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Deliberately non-symmetric across channels, so a contaminated
        // channel cannot hide behind an equal one, strictly inside (0, 1)
        // so neither branch of the formula fires here, and leaving two
        // channels unclamped and one clamped in the opaque half.
        let top_rgba = [0.125, 0.625, 0.875, 1.0];
        let bottom_texels = half_transparent_texels();

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        // A real render pass builds the accumulator, rather than seeding
        // it: the zero-alpha half is produced by the same mechanism under
        // test, not written directly.
        let bottom = tile_from_texels(device, queue, &bottom_texels, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        // Texel 0 is in the transparent half, and `f16` equality pins its
        // alpha at exactly zero -- something the 8-bit whole-tile
        // comparison below cannot do, since a tiny non-zero alpha would
        // quantise to 0 there.
        let gpu_accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            gpu_accumulator,
            (0.0, 0.0, 0.0, 0.0),
            "setup: this test is only meaningful if the accumulator's left half is genuinely \
             zero-alpha"
        );
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &backdrop),
            &rgba8_of(&bottom_texels),
            "setup: the Normal-blend pass that builds the accumulator must reproduce the \
             half-transparent bottom layer texel for texel, or neither half's assertion below \
             means what it claims",
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the untaken `ab > 0.0` branch: {gpu_result:?}. That is a \
             real finding about this backend's shader compiler, not a reason to relax this test."
        );
        assert_eq!(
            gpu_result,
            (0.125, 0.625, 0.875, 1.0),
            "where the accumulator is empty the composite is the source alone"
        );

        let top_texels = solid_texels(top_rgba);
        let cpu_out = composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]);
        assert_whole_tile_matches(
            &read_rgba8(device, queue, &dst),
            &rgba8_of(&cpu_out),
            "the in-shader ColorDodge path and composite_tile_cpu disagree across a \
             half-transparent backdrop. In the opaque half a wrong blend formula shows up here \
             (except a dropped min clamp -- see this test's doc comment); in the transparent \
             half a NaN out of the untaken `ab > 0.0` branch does.",
        );
    }

    #[test]
    /// **The `cb == 0.0` branch, and the only test in either crate that
    /// reaches it** (0.108.0). `ColorDodge`'s formula tests `Cb == 0`
    /// *first*, before `Cs == 1`, so a zero backdrop channel yields `0.0`
    /// regardless of the source — including where the source is `1.0` and
    /// the second branch would have said `1.0`. This test's **green** channel
    /// is that exact input: `Cb = 0.0`, `Cs = 1.0`, both conditions true at
    /// once.
    ///
    /// **The accumulator must be fully opaque, and that is a requirement
    /// rather than a convenience.** The shader recovers `cb` as
    /// `bd.rgb / ab`, and the branch is a bit-exact `== 0.0`. A fractional
    /// `ab` would make blue's own quotient depend on that division landing
    /// exactly, which would make this test silently measure something other
    /// than the branch it exists for. (Red and green would survive it — `0`
    /// divided by anything positive is still `0` — but blue is this
    /// fixture's only formula discriminator, so the whole test rests on it.)
    /// The translucent-accumulator case is test 2's job.
    ///
    /// Backdrop `(0.0, 0.0, 0.375)` opaque under a `(0.5, 1.0, 0.25)` source
    /// at opacity `1.0`, so `a = 1.0`, `inv = 0.0` and the whole result is
    /// `B`:
    ///
    /// - red: `Cb == 0`, so `0.0` (and the arithmetic branch would agree —
    ///   `0 / 0.5 = 0`, `min(1, 0) = 0`, which is *why* the first guard is
    ///   arithmetically redundant, and note this is a perfectly ordinary
    ///   division rather than a `0/0`);
    /// - green: `Cb == 0` **and** `Cs == 1`, so branch order decides:
    ///   `0.0`, not `1.0`;
    /// - blue: neither, so `min(1, 0.375/0.75) = 0.5`.
    ///
    /// Golden `(0.0, 0.0, 0.5, 1.0)`, every value an exact binary fraction.
    ///
    /// **REQUIRED DISCLOSURE: red and green agree with a whole family of
    /// other modes here, and blue is the real discriminator of the
    /// *formula*.** `Multiply`, `Darken`, `LinearBurn` and `ColorBurn` all
    /// give `0.0` in a channel with a zero backdrop, so this fixture on its
    /// own cannot say this is `ColorDodge` rather than any of them — blue
    /// does that (`0.5` against `Normal`'s `0.25`, `Darken`'s `0.25`,
    /// `Lighten`'s `0.375`, `Screen`'s `0.53125`, `Multiply`'s `0.09375`,
    /// `Difference`'s `0.125`, `LinearDodge`'s `0.625`, and both
    /// `LinearBurn`'s and `ColorBurn`'s `0.0`). Note blue is deliberately
    /// *unclamped* here (`Cb + Cs = 0.625 < 1`), which is the only way it
    /// could separate `ColorDodge` from `LinearDodge` at all — suite-header
    /// degeneracy 4. What red and green *are* for is the branch structure,
    /// which no other test reaches:
    ///
    /// - green kills a **swapped branch order** (`Cs == 1` tested first
    ///   would give `1.0`) — the only assertion in either crate that does;
    /// - green also kills the **`cb == 0.0` branch deleted outright**, for
    ///   the same reason and by the same value: the `cs == 1.0` guard then
    ///   fires in its place. Note this is *not* a division-by-zero question
    ///   — no `0/0` is ever evaluated on that path, since `1 - cs` is
    ///   non-zero in every channel the deleted guard used to catch — so
    ///   unlike the `cs == 1.0` guard, deleting the first guard is killed
    ///   deterministically on every backend, which 0.108.0 confirmed by
    ///   running the mutation for real;
    /// - red and green together kill the **`cb == 0.0` branch returning
    ///   `1.0`** instead of `0.0`.
    ///
    /// **Green is also the one channel in this suite where degeneracy 3's
    /// test (`Cb == Cs * (1 - Cs)`) is satisfied and does *not* mean what it
    /// usually means**: `Cs * (1 - Cs) = 1 * 0 = 0 = Cb`, but that
    /// identity characterises the *arithmetic* branch, and green never
    /// reaches it. `ColorDodge(0, 1)` is `0.0` while `Normal` gives `1.0`, so
    /// green discriminates the two perfectly well. Stated here so a later
    /// reader auditing the degeneracy does not treat this channel as a hole.
    ///
    /// A dropped `min` clamp is *not* visible here: no channel's quotient
    /// exceeds `1.0` (red's is `0.0`, green's never evaluated, blue's is
    /// `0.5`). Tests 1, 3, 4 and 5 cover that.
    ///
    /// The golden is cross-checked against the real [`composite_tile_cpu`],
    /// whose `blend_channel` arm this branch order is copied from, and a
    /// finiteness check runs first.
    fn composite_color_dodge_over_with_opacity_yields_zero_where_the_backdrop_is_zero() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Fully opaque -- see the doc comment: only ab == 1.0 makes the
        // shader's own `bd.rgb / ab` recovery exact, which blue's quotient
        // (this fixture's only formula discriminator) depends on.
        let bottom_rgba = [0.0, 0.0, 0.375, 1.0];
        let top_rgba = [0.5, 1.0, 0.25, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.0, 0.0, 0.375, 1.0),
            "setup: the accumulator must be exactly zero in red and green AND fully opaque, or \
             the shader's own `bd.rgb / ab` recovery need not land on exactly 0.375 in blue -- \
             the one channel that discriminates this mode's formula here"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]));
        assert_eq!(
            cpu_result,
            (0.0, 0.0, 0.5, 1.0),
            "setup: the golden below must be what composite_tile_cpu's own three-branch \
             blend_channel arm computes -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the ColorDodge branches: {gpu_result:?}. Green's operands \
             are Cb = 0.0 and Cs = 1.0, which is a 0/0 in the arithmetic branch -- reaching it \
             at all means both guards are gone."
        );
        assert_eq!(
            gpu_result,
            (0.0, 0.0, 0.5, 1.0),
            "a zero backdrop channel must dodge to 0.0, and green (Cb = 0.0 AND Cs = 1.0) must \
             take the *first* branch: (0.0, 1.0, 0.5, 1.0) would mean the two branches were \
             tested in the wrong order or the cb == 0.0 branch was deleted, and \
             (1.0, 1.0, 0.5, 1.0) that the cb == 0.0 branch returns 1.0. Blue is what \
             discriminates the formula: Normal gives 0.25 there, Darken 0.25, Lighten 0.375, \
             Screen 0.53125, Multiply 0.09375, Difference 0.125, LinearDodge 0.625, and both \
             LinearBurn and ColorBurn 0.0 -- while red and green agree with several of those, \
             which is why this fixture is about branch structure and blue is about arithmetic."
        );
    }

    #[test]
    /// **The `cs == 1.0` branch** (0.108.0): a saturated source channel
    /// dodges the backdrop to `1.0`, regardless of how dark the backdrop was
    /// — provided the backdrop is not itself zero, which is what makes this
    /// test's fixture the complement of test 7's rather than a variation on
    /// it.
    ///
    /// Backdrop `(0.75, 0.125, 0.375)` opaque under a `(1.0, 0.75, 0.5)`
    /// source at opacity `1.0`, so `a = 1.0`, `inv = 0.0` and the whole
    /// result is `B`:
    ///
    /// - red: `Cs == 1` and `Cb != 0`, so `1.0` — the branch this test
    ///   exists for;
    /// - green: `min(1, 0.125/0.25) = 0.5`;
    /// - blue: `min(1, 0.375/0.5) = 0.75`.
    ///
    /// Golden `(1.0, 0.5, 0.75, 1.0)`, every value an exact binary fraction.
    ///
    /// **REQUIRED DISCLOSURE: red agrees with a whole family of other modes
    /// here, and green and blue are the real discriminators.** `Normal`,
    /// `Lighten`, `Screen` and `LinearDodge` all give `1.0` in a channel with
    /// a saturated source, so this fixture's red channel on its own cannot
    /// say this is `ColorDodge`. Green and blue do (`(0.5, 0.75)` against
    /// `Normal`'s and `Lighten`'s `(0.75, 0.5)`, `Screen`'s
    /// `(0.78125, 0.6875)`, `Multiply`'s `(0.09375, 0.1875)`, `Darken`'s
    /// `(0.125, 0.375)`, `Difference`'s `(0.625, 0.125)`, `LinearDodge`'s
    /// `(0.875, 0.875)`, and both `LinearBurn`'s and `ColorBurn`'s
    /// `(0.0, 0.0)`). Both are unclamped, which is what lets them separate
    /// this mode from `LinearDodge` at all — suite-header degeneracy 4.
    ///
    /// **What red is for is the branch, and specifically the mutation where
    /// it returns `0.0` instead of `1.0`** — mutation (h) of this round's
    /// set, which this test's red channel is the only assertion in either
    /// crate to kill.
    ///
    /// **What red is *not* able to do, and this is the round's headline
    /// disclosure.** Deleting the `cs == 1.0` guard **entirely** leaves this
    /// test green on Vulkan/NVIDIA, because `0.75 / 0.0` is `+inf` there and
    /// `min(1.0, inf)` is exactly the `1.0` the guard would have returned.
    /// That was run for real, not predicted. The guard is a **portability**
    /// guard, not a correctness one on this adapter: WGSL specifies division
    /// by zero as yielding an indeterminate value, so a backend producing
    /// `NaN` instead would propagate it (`min(1.0, NaN)` is
    /// implementation-defined, and this fixture's red channel has
    /// `Cb = 0.75` under a full-alpha backdrop, so `ab * b` scales that
    /// `NaN` by a *strictly positive* `ab` and it lands in the output
    /// directly. Note this is not `ColorBurn`'s zero-absorption argument
    /// wearing new operands: for `ColorDodge` the `ab == 0` half forces
    /// `cb` to zero, which the *first* guard catches before any division,
    /// so no `NaN` can originate there at all). **No test in this crate can
    /// distinguish the guarded shader from the unguarded one, because no
    /// test can make this adapter divide differently** — stated here rather
    /// than papered over with an assertion that would not mean what it
    /// claimed. Note the asymmetry with test 7: *that* guard's deletion is
    /// killed deterministically, because its redundancy does not route
    /// through a division by zero at all. See PLAN.md's 0.108.0 entry.
    ///
    /// Red's backdrop is deliberately `0.75` rather than `0.0`: with a
    /// saturated source channel, a zero backdrop would take the *first*
    /// branch and yield `0.0`, which is test 7's green channel.
    ///
    /// A dropped `min` clamp is not visible here either: no channel's
    /// quotient exceeds `1.0` (red's is never evaluated, green's is `0.5`,
    /// blue's `0.75`). Tests 1, 3, 4 and 5 cover that.
    ///
    /// The golden is cross-checked against the real [`composite_tile_cpu`],
    /// and a finiteness check runs first.
    fn composite_color_dodge_over_with_opacity_yields_one_where_the_source_is_saturated() {
        let Some(context) = real_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();

        // Red's backdrop is deliberately *not* 0.0: with a saturated source
        // channel, a zero backdrop would take the *first* branch and yield
        // 0.0, which is test 7's green channel. This test is about the
        // second branch, so it needs Cb != 0 in the channel where Cs == 1.
        let bottom_rgba = [0.75, 0.125, 0.375, 1.0];
        let top_rgba = [1.0, 0.75, 0.5, 1.0];

        let backdrop = solid_tile(
            device,
            queue,
            [0.0, 0.0, 0.0, 0.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let bottom = solid_tile(device, queue, bottom_rgba, wgpu::TextureUsages::empty());
        let top = solid_tile(device, queue, top_rgba, wgpu::TextureUsages::empty());
        let dst = solid_tile(
            device,
            queue,
            [1.0, 0.0, 0.0, 1.0],
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let backdrop_view = backdrop.create_view(&wgpu::TextureViewDescriptor::default());
        let bottom_view = bottom.create_view(&wgpu::TextureViewDescriptor::default());
        let top_view = top.create_view(&wgpu::TextureViewDescriptor::default());
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());

        let mut compositor = TileCompositor::new(device);
        submit_one(&context, |encoder| {
            compositor.composite_over_with_opacity(
                &context,
                encoder,
                &backdrop_view,
                &bottom_view,
                1.0,
            );
        });
        submit_one(&context, |encoder| {
            compositor.composite_color_dodge_over_with_opacity(
                &context,
                encoder,
                &top_view,
                &backdrop_view,
                &dst_view,
                1.0,
            );
        });

        let accumulator = read_first_texel(device, queue, &backdrop);
        assert_eq!(
            accumulator,
            (0.75, 0.125, 0.375, 1.0),
            "setup: red's backdrop must be strictly above 0.0, or the cb == 0.0 branch would \
             fire there instead and this test would be a duplicate of test 7"
        );

        let bottom_texels = solid_texels(bottom_rgba);
        let top_texels = solid_texels(top_rgba);
        let cpu_result = first_texel(&composite_tile_cpu(&[
            (&bottom_texels, 1.0, BlendMode::Normal),
            (&top_texels, 1.0, BlendMode::ColorDodge),
        ]));
        assert_eq!(
            cpu_result,
            (1.0, 0.5, 0.75, 1.0),
            "setup: the golden below must be what composite_tile_cpu's own three-branch \
             blend_channel arm computes -- if this fails, the literal is stale, not the GPU"
        );

        let gpu_result = read_first_texel(device, queue, &dst);
        let (r, g, b, a) = gpu_result;
        assert!(
            r.is_finite() && g.is_finite() && b.is_finite() && a.is_finite(),
            "a NaN or infinity escaped the ColorDodge branches: {gpu_result:?}. Red's Cs is 1.0, \
             so an unguarded arithmetic branch divides 0.75 by zero there -- which this adapter \
             resolves to +inf and thence to the correct 1.0, but another need not."
        );
        assert_eq!(
            gpu_result,
            (1.0, 0.5, 0.75, 1.0),
            "a saturated source channel over a non-zero backdrop must dodge to 1.0: \
             (0.0, 0.5, 0.75, 1.0) would mean the cs == 1.0 branch returns 0.0. Green and blue \
             are what discriminate the formula, since Normal, Lighten, Screen and LinearDodge \
             all give 1.0 in red too: Normal and Lighten give (0.75, 0.5) there, Screen \
             (0.78125, 0.6875), Multiply (0.09375, 0.1875), Darken (0.125, 0.375), Difference \
             (0.625, 0.125), LinearDodge (0.875, 0.875), and both LinearBurn and ColorBurn \
             (0.0, 0.0)."
        );
    }

    // -- Non-separable blend modes (this round): `Hue`, `Saturation`,
    // `Color`, `Luminosity`, each a function of the whole `(R,G,B)`
    // triple via `blend_hue`/`blend_saturation`/`blend_color`/
    // `blend_luminosity` and dispatched through `blend_rgb` --
    // `blend_channel`'s own per-channel signature cannot express them.
    // Unlike every prior round's tests in this file, the W3C spec's own
    // weights (0.3/0.59/0.11) are not exact binary fractions, so exact
    // `assert_eq!` isn't achievable here; `assert_close`/
    // `assert_texel_close` below use a small epsilon instead -- a
    // deliberate, reasoned exception to this file's otherwise bit-exact
    // test discipline (the same real floating-point/precision-limit
    // reasoning `aurora_testkit::compare_to_golden`'s own tolerance
    // already uses elsewhere in this codebase), not a lowering of
    // rigor. `1e-3` is roughly `f16`'s own precision at these
    // magnitudes.

    /// Epsilon-tolerance comparison of an `[f32; 3]` triple, used by
    /// every non-separable-mode test below.
    fn assert_close(actual: [f32; 3], expected: [f32; 3], epsilon: f32) {
        let [ar, ag, ab] = actual;
        let [er, eg, eb] = expected;
        assert!(
            (ar - er).abs() < epsilon && (ag - eg).abs() < epsilon && (ab - eb).abs() < epsilon,
            "expected {expected:?} within {epsilon}, got {actual:?}"
        );
    }

    /// Epsilon-tolerance comparison of a whole texel's own `(r,g,b,a)`,
    /// the [`assert_close`] above's sibling for tests that go through
    /// [`composite_tile_cpu`]'s full per-texel path rather than calling
    /// a blend function directly.
    fn assert_texel_close(
        actual: (f32, f32, f32, f32),
        expected: (f32, f32, f32, f32),
        epsilon: f32,
    ) {
        let (ar, ag, ab, aa) = actual;
        let (er, eg, eb, ea) = expected;
        assert!(
            (ar - er).abs() < epsilon
                && (ag - eg).abs() < epsilon
                && (ab - eb).abs() < epsilon
                && (aa - ea).abs() < epsilon,
            "expected {expected:?} within {epsilon}, got {actual:?}"
        );
    }

    #[test]
    fn lum_uses_the_specs_own_ntsc_weighted_average_not_wcag_weights() {
        // Isolate each weight via a pure primary, then a mixed case --
        // confirms these are the spec's own 0.3/0.59/0.11, not
        // `aurora_color`'s own WCAG 0.2126/0.7152/0.0722 weights used
        // elsewhere in this codebase for contrast checking.
        assert!((lum([1.0, 0.0, 0.0]) - 0.3).abs() < 1e-6);
        assert!((lum([0.0, 1.0, 0.0]) - 0.59).abs() < 1e-6);
        assert!((lum([0.0, 0.0, 1.0]) - 0.11).abs() < 1e-6);
        assert!((lum([0.6, 0.3, 0.1]) - 0.368).abs() < 1e-6);
    }

    #[test]
    fn sat_is_the_max_minus_min_channel() {
        assert!((sat([0.6, 0.3, 0.1]) - 0.5).abs() < 1e-6);
        assert!((sat([0.2, 0.8, 0.5]) - 0.6).abs() < 1e-6);
        assert!(
            sat([0.4, 0.4, 0.4]).abs() < 1e-6,
            "an achromatic triple has zero saturation"
        );
    }

    #[test]
    fn clip_color_leaves_an_in_gamut_color_unchanged() {
        let c = [0.3, 0.5, 0.7];
        assert_close(clip_color(c), c, 1e-6);
    }

    #[test]
    // Only the `n < 0` branch fires: min channel is -0.2, max channel
    // is 0.9 (in [0,1], so the `x > 1` branch does not fire).
    // Independently computed via a from-scratch Python
    // re-implementation of the spec (not this crate's own Rust code).
    // The min channel must land at exactly 0 -- a general property of
    // this branch, since `n + (n-l)*l/(l-n)... ` reduces to `0` for the
    // channel equal to `n` itself -- and luminance must be preserved
    // exactly; both checked directly, not just the raw output values.
    fn clip_color_pulls_a_negative_channel_up_to_zero_preserving_luminance() {
        let c = [-0.2, 0.5, 0.9];
        let result = clip_color(c);
        assert_close(result, [0.0, 0.437_827_7, 0.688_015], 1e-3);
        assert!(
            (lum(result) - lum(c)).abs() < 1e-3,
            "ClipColor must preserve luminance exactly"
        );
    }

    #[test]
    // Only the `x > 1` branch fires -- exactly the input
    // `set_lum_shifts_and_clips_matching_the_worked_color_example`
    // below produces internally (`SetLum(Cs=(1,0,0), l=0.5)` computes
    // `C'=(1.2,0.2,0.2)` before clipping).
    fn clip_color_pulls_an_overshot_channel_down_to_one_preserving_luminance() {
        let c = [1.2, 0.2, 0.2];
        let result = clip_color(c);
        assert_close(result, [1.0, 2.0 / 7.0, 2.0 / 7.0], 1e-3);
        assert!((lum(result) - lum(c)).abs() < 1e-3);
    }

    #[test]
    // Both branches fire on the same input: the `n < 0` branch (per
    // the spec's own literal order) runs first, and the `x > 1` branch
    // then runs on *that* branch's own already-updated channel values,
    // not the original `c` -- `clip_color`'s own doc comment names this
    // ordering explicitly. Independently computed via Python.
    fn clip_color_applies_both_branches_in_the_specs_own_order_when_both_fire() {
        let c = [-0.5, 0.9, 1.3];
        let result = clip_color(c);
        assert_close(result, [0.202_577, 0.642_022, 0.767_578], 1e-3);
        assert!((lum(result) - lum(c)).abs() < 1e-3);
    }

    // -- The achromatic-denominator regression (0.87.1). ------------
    //
    // `clip_color` divides by `l - n` and by `x - l`. `lum`'s own
    // weights sum to exactly 1.0, so `n <= l <= x` always, with
    // equality on *both* sides precisely when every channel is equal.
    // An achromatic input therefore makes both denominators exactly
    // `0.0`, and the numerators `(r - l) * ...` are exactly `0.0` too
    // -- so the division was `0.0 / 0.0`, i.e. NaN, whenever an
    // achromatic input also had a channel outside `0.0..=1.0` (which
    // is what makes one of the two branches fire in the first place).
    //
    // That is reachable from ordinary content, not just a contrived
    // one: `blend_color` is `SetLum(Cs, Lum(Cb))`, so any achromatic
    // *source* -- grey, white, black -- over a backdrop whose
    // luminance is outside `[0,1]` (an HDR TIFF import, which
    // `aurora-io` deliberately does not clamp, per invariant §7.3.1b)
    // hit it; and `set_sat` returns an achromatic triple for *any*
    // achromatic backdrop, so `blend_hue`/`blend_saturation` hit it
    // for any source at all over such a backdrop. A NaN there is not
    // absorbed downstream either: `composite_layer_into` scales the
    // blend result by `alpha`, and `0.0 * NaN` is NaN in IEEE-754, so
    // even a fully transparent layer poisoned the whole tile -- which
    // then survives un-premultiplication, export and the eyedropper
    // with no error surfaced anywhere.

    #[test]
    fn clip_color_stays_finite_for_an_out_of_gamut_achromatic_input() {
        for (c, expected) in [
            ([1.5f32, 1.5, 1.5], [1.0f32, 1.0, 1.0]),
            ([-0.5, -0.5, -0.5], [0.0, 0.0, 0.0]),
            ([2.0, 2.0, 2.0], [1.0, 1.0, 1.0]),
        ] {
            let result = clip_color(c);
            assert!(
                result.iter().all(|channel| channel.is_finite()),
                "clip_color({c:?}) must not produce NaN/inf, got {result:?}"
            );
            // Luminance cannot be preserved here -- the target is
            // outside the gamut and the colour has no chromatic
            // direction to redistribute along -- so the guard clamps
            // instead, which is the closest in-gamut colour.
            assert_close(result, expected, 1e-6);
        }
    }

    #[test]
    // An in-gamut achromatic input fires neither branch, so the guard
    // must be invisible to it: this pins that the fix did not start
    // clamping (or otherwise touching) colours that were always fine.
    fn clip_color_leaves_an_in_gamut_achromatic_input_exactly_alone() {
        for c in [[0.0f32, 0.0, 0.0], [0.5, 0.5, 0.5], [1.0, 1.0, 1.0]] {
            let result = clip_color(c);
            // Compared as bit patterns rather than by `==` on the
            // arrays: the claim really is "exactly unchanged, not merely
            // close", and `clippy::float_cmp` rightly refuses a direct
            // float equality that is not against a literal.
            assert_eq!(
                result.map(f32::to_bits),
                c.map(f32::to_bits),
                "clip_color({c:?}) must be exactly unchanged, got {result:?}"
            );
        }
    }

    #[test]
    // The same regression one level up, at the entry point every
    // non-separable mode actually calls.
    fn set_lum_stays_finite_for_an_achromatic_input_with_an_out_of_gamut_target() {
        for (c, l) in [
            ([0.0f32, 0.0, 0.0], 1.5f32),
            ([0.5, 0.5, 0.5], 2.0),
            ([0.0, 0.0, 0.0], -0.5),
            ([1.0, 1.0, 1.0], -1.0),
            ([2.0, 2.0, 2.0], 3.0),
            ([0.5, 0.5, 0.5], f32::from(half::f16::MAX)),
        ] {
            let result = set_lum(c, l);
            assert!(
                result.iter().all(|channel| channel.is_finite()),
                "set_lum({c:?}, {l}) must not produce NaN/inf, got {result:?}"
            );
        }
    }

    #[test]
    // The three modes the bug was actually reachable through, at the
    // two realistic shapes: an HDR (out-of-`[0,1]`) achromatic
    // backdrop under any source (`Hue`/`Saturation`, via `set_sat`'s
    // achromatic collapse), and an achromatic source over an HDR
    // backdrop (`Color`, via `SetLum(Cs, Lum(Cb))`).
    // `Luminosity` never exhibited it and is included to keep all four
    // non-separable modes covered by one test.
    fn every_non_separable_mode_stays_finite_for_hdr_and_achromatic_inputs() {
        let cases: [([f32; 3], [f32; 3]); 6] = [
            ([4.0, 4.0, 4.0], [0.2, 0.5, 0.9]),
            ([4.0, 4.0, 4.0], [0.5, 0.5, 0.5]),
            ([-2.0, -2.0, -2.0], [0.2, 0.5, 0.9]),
            ([2.0, 3.0, 4.0], [0.5, 0.5, 0.5]),
            ([2.0, 3.0, 4.0], [1.0, 1.0, 1.0]),
            ([0.6, 0.3, 0.1], [0.0, 0.0, 0.0]),
        ];
        for (cb, cs) in cases {
            for (name, result) in [
                ("hue", blend_hue(cb, cs)),
                ("saturation", blend_saturation(cb, cs)),
                ("color", blend_color(cb, cs)),
                ("luminosity", blend_luminosity(cb, cs)),
            ] {
                assert!(
                    result.iter().all(|channel| channel.is_finite()),
                    "blend_{name}(Cb={cb:?}, Cs={cs:?}) must not produce NaN/inf, got {result:?}"
                );
            }
        }
    }

    #[test]
    // End to end through the real compositing path, and the part that
    // makes this a *silent corruption* bug rather than a cosmetic one:
    // the offending layer here is fully transparent, so it must change
    // nothing at all -- but `alpha * NaN` is NaN, so before the fix it
    // replaced the whole backdrop with NaN.
    fn a_fully_transparent_color_layer_over_an_hdr_backdrop_leaves_it_untouched() {
        let backdrop = solid_texels([4.0, 4.0, 4.0, 1.0]);
        let ghost = solid_texels([0.0, 0.0, 0.0, 0.0]);
        let out = composite_tile_cpu(&[
            (&backdrop, 1.0, BlendMode::Normal),
            (&ghost, 1.0, BlendMode::Color),
        ]);
        let (r, g, b, a) = first_texel(&out);
        for (channel, value) in [("r", r), ("g", g), ("b", b), ("a", a)] {
            assert!(
                value.is_finite(),
                "channel {channel} of an HDR backdrop under a fully transparent Color layer \
                 must stay finite, got {value}"
            );
        }
        assert_texel_close((r, g, b, a), (4.0, 4.0, 4.0, 1.0), 1e-2);
    }

    #[test]
    // Cb=(0.5,0.5,0.5), target l=Lum(Cs=(1,0,0))=0.3: d=-0.2,
    // C'=(0.3,0.3,0.3), already in gamut -- no clipping needed.
    fn set_lum_shifts_and_clips_matching_the_worked_luminosity_example() {
        assert_close(set_lum([0.5, 0.5, 0.5], 0.3), [0.3, 0.3, 0.3], 1e-3);
    }

    #[test]
    // Cs=(1,0,0), target l=Lum(Cb=(0.5,0.5,0.5))=0.5: d=0.2,
    // C'=(1.2,0.2,0.2), clipped by `ClipColor`'s own `x > 1` branch
    // down to (1.0, 2/7, 2/7) -- this round's own worked `Color`
    // example.
    fn set_lum_shifts_and_clips_matching_the_worked_color_example() {
        assert_close(
            set_lum([1.0, 0.0, 0.0], 0.5),
            [1.0, 2.0 / 7.0, 2.0 / 7.0],
            1e-3,
        );
    }

    #[test]
    // The task's own worked non-R/G/B-order example: for C=(0.2, 0.8,
    // 0.5), max is G (0.8), mid is B (0.5), min is R (0.2) -- the "max"
    // role is held by the *middle* array slot, not the first, proving
    // `set_sat`'s channel handling is genuinely value-based rather than
    // accidentally tied to array position. scale = s/(max-min) =
    // 0.6/(0.8-0.2) = 1.0; R (min) -> 0, G (max) -> s = 0.6, B (mid) ->
    // (0.5-0.2)*1.0 = 0.3.
    fn set_sat_reassigns_saturation_when_the_max_mid_min_order_is_not_r_g_b() {
        assert_close(set_sat([0.2, 0.8, 0.5], 0.6), [0.0, 0.6, 0.3], 1e-6);
    }

    #[test]
    // The spec's own defined degenerate case: an achromatic (all-equal)
    // input has no "direction" to redistribute saturation into, so
    // every channel becomes exactly 0, regardless of `s` -- the spec's
    // own `else` branch, not a bug to special-case around. `set_sat`'s
    // own achromatic arm returns the literal `[0.0, 0.0, 0.0]` array,
    // not an accumulated-rounding-error value, so an exact comparison
    // is the right check here -- the same reasoning `blend_channel`'s
    // own `float_cmp` allow already documents.
    #[allow(clippy::float_cmp)]
    fn set_sat_of_an_equal_channel_input_is_pure_zero() {
        assert_eq!(set_sat([0.4, 0.4, 0.4], 0.6), [0.0, 0.0, 0.0]);
        assert_eq!(set_sat([0.0, 0.0, 0.0], 1.0), [0.0, 0.0, 0.0]);
    }

    #[test]
    // Proves `set_sat`'s own single-formula shape against the W3C
    // spec's literal three-branch, channel-*identifying* form -- the
    // same "simplify, then prove the simplification numerically"
    // discipline `blend_channel`'s own `LinearLight` arm already uses
    // in this file (see
    // `linear_light_simplified_form_matches_the_branch_form_for_several_inputs`).
    // The spec form here is computed via a completely independent
    // method (sort the three channels by value to find which R/G/B
    // slot holds the max/mid/min role, then assign each its own spec
    // expression and reassemble) -- not by calling or mirroring any
    // part of `set_sat`'s own implementation. Covers an R/G/B-ordered
    // input, a not-R/G/B-ordered input, and two tie cases (a max/mid
    // tie and a mid/min tie) -- ties are provably safe for this
    // simplification (see `set_sat`'s own doc comment) since the
    // formula gives the same value to both tied channels either way.
    fn set_sat_matches_the_specs_explicit_max_mid_min_form_for_several_inputs() {
        let cases: [([f32; 3], f32); 4] = [
            ([0.6, 0.3, 0.1], 0.5), // max=R, mid=G, min=B: R/G/B order
            ([0.2, 0.8, 0.5], 0.6), // max=G, mid=B, min=R: not R/G/B order
            ([0.9, 0.9, 0.1], 0.4), // a max/mid tie
            ([0.5, 0.2, 0.2], 0.3), // a mid/min tie
        ];
        for (c, s) in cases {
            let [r, g, b] = c;
            let mut channels = [("r", r), ("g", g), ("b", b)];
            channels.sort_by(|a, b| a.1.total_cmp(&b.1));
            let (min_name, min_v) = channels[0];
            let (mid_name, mid_v) = channels[1];
            let (max_name, max_v) = channels[2];
            let assign = |name: &str| -> f32 {
                if max_v <= min_v || name == min_name {
                    0.0
                } else if name == mid_name {
                    (mid_v - min_v) * s / (max_v - min_v)
                } else {
                    debug_assert_eq!(name, max_name);
                    s
                }
            };
            let expected = [assign("r"), assign("g"), assign("b")];
            assert_close(set_sat(c, s), expected, 1e-6);
        }
    }

    #[test]
    // Luminosity(Cb=(0.5,0.5,0.5), Cs=(1,0,0)), this round's own worked
    // example: Lum(Cb)=0.5, Lum(Cs)=0.3, d=-0.2, C'=(0.3,0.3,0.3),
    // already in gamut -> exactly (0.3, 0.3, 0.3).
    fn composite_tile_cpu_luminosity_matches_the_worked_example() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([1.0, 0.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Luminosity),
        ]);
        assert_texel_close(first_texel(&out), (0.3, 0.3, 0.3, 1.0), 1e-3);
    }

    #[test]
    // Color(Cb=(0.5,0.5,0.5), Cs=(1,0,0)): SetLum(Cs, l=0.5), d=0.2,
    // C'=(1.2,0.2,0.2), clipped by ClipColor's own `x > 1` branch to
    // (1.0, 2/7, 2/7).
    fn composite_tile_cpu_color_matches_the_worked_example() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([1.0, 0.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Color),
        ]);
        assert_texel_close(first_texel(&out), (1.0, 2.0 / 7.0, 2.0 / 7.0, 1.0), 1e-3);
    }

    #[test]
    // Saturation and Hue both degenerate to plain gray for this
    // specific input, because Cb=(0.5,0.5,0.5) is perfectly achromatic
    // (Sat(Cb)=0): `set_sat`'s own degenerate case
    // (`set_sat_of_an_equal_channel_input_is_pure_zero`) makes
    // `SetSat(Cb, s)` for *any* `s` collapse to `(0,0,0)` when `Cb`
    // itself is achromatic, and `SetLum((0,0,0), 0.5)` then yields
    // `(0.5, 0.5, 0.5)`.
    fn composite_tile_cpu_saturation_matches_the_worked_example() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([1.0, 0.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Saturation),
        ]);
        assert_texel_close(first_texel(&out), (0.5, 0.5, 0.5, 1.0), 1e-3);
    }

    #[test]
    fn composite_tile_cpu_hue_matches_the_worked_example() {
        let bottom = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let top = solid_texels([1.0, 0.0, 0.0, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Hue),
        ]);
        assert_texel_close(first_texel(&out), (0.5, 0.5, 0.5, 1.0), 1e-3);
    }

    // -- General (non-degenerate) cases for all 4 non-separable modes,
    // all three channels genuinely different in both Cb and Cs:
    // Cb=(0.6,0.3,0.1), Cs=(0.2,0.5,0.9). Independently computed via a
    // from-scratch Python re-implementation of the W3C spec (not by
    // trusting this crate's own Rust output): Lum(Cb)=0.368,
    // Lum(Cs)=0.454, Sat(Cb)=0.5, Sat(Cs)=0.7. Calls the blend
    // functions directly (not through `composite_tile_cpu`) so no
    // `f16` round-trip of these non-dyadic inputs adds a second source
    // of imprecision on top of the spec's own non-dyadic weights.

    #[test]
    // d = Lum(Cs) - Lum(Cb) = 0.454 - 0.368 = 0.086; Cb + d stays in
    // [0,1] on every channel, so no clipping fires: (0.686, 0.386,
    // 0.186).
    fn blend_luminosity_matches_an_independently_computed_general_case() {
        let cb = [0.6, 0.3, 0.1];
        let cs = [0.2, 0.5, 0.9];
        assert_close(blend_luminosity(cb, cs), [0.686, 0.386, 0.186], 1e-3);
    }

    #[test]
    // d = Lum(Cb) - Lum(Cs) = 0.368 - 0.454 = -0.086; Cs + d stays in
    // [0,1] on every channel, so no clipping fires: (0.114, 0.414,
    // 0.814).
    fn blend_color_matches_an_independently_computed_general_case() {
        let cb = [0.6, 0.3, 0.1];
        let cs = [0.2, 0.5, 0.9];
        assert_close(blend_color(cb, cs), [0.114, 0.414, 0.814], 1e-3);
    }

    #[test]
    // SetSat(Cb, Sat(Cs)=0.7) then SetLum(..., Lum(Cb)=0.368):
    // (0.686567..., 0.274627..., 0.0).
    fn blend_saturation_matches_an_independently_computed_general_case() {
        let cb = [0.6, 0.3, 0.1];
        let cs = [0.2, 0.5, 0.9];
        assert_close(blend_saturation(cb, cs), [0.686_567, 0.274_627, 0.0], 1e-3);
    }

    #[test]
    // SetSat(Cs, Sat(Cb)=0.5) then SetLum(..., Lum(Cb)=0.368):
    // (0.186571..., 0.400857..., 0.686571...).
    fn blend_hue_matches_an_independently_computed_general_case() {
        let cb = [0.6, 0.3, 0.1];
        let cs = [0.2, 0.5, 0.9];
        assert_close(blend_hue(cb, cs), [0.186_571, 0.400_857, 0.686_571], 1e-3);
    }

    #[test]
    // Regression check for the `blend_rgb` refactor: re-asserts a
    // handful of already-landed separable-mode values (Multiply,
    // Overlay, ColorDodge -- one from each of the three previously
    // added families) unchanged now that `composite_tile_cpu`'s inner
    // loop calls `blend_rgb` once per texel instead of calling
    // `blend_channel` three times directly. `blend_rgb`'s own
    // separable-mode arm just delegates back to `blend_channel` per
    // channel, unchanged, so these must remain bit-for-bit identical to
    // the values already proven (before this round) by
    // `composite_tile_cpu_multiply_blends_two_mid_greys_to_a_quarter_grey`,
    // `composite_tile_cpu_overlay_uses_the_direct_multiply_form_when_the_backdrop_is_at_or_below_half`,
    // and
    // `composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio`.
    fn composite_tile_cpu_separable_modes_are_bit_identical_after_the_blend_rgb_refactor() {
        let grey = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&grey, 1.0, BlendMode::Normal),
            (&grey, 1.0, BlendMode::Multiply),
        ]);
        assert_eq!(first_texel(&out), (0.25, 0.25, 0.25, 1.0));

        let bottom = solid_texels([0.25, 0.25, 0.25, 1.0]);
        let top = solid_texels([0.75, 0.75, 0.75, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::Overlay),
        ]);
        assert_eq!(first_texel(&out), (0.375, 0.375, 0.375, 1.0));

        let bottom = solid_texels([0.375, 0.375, 0.375, 1.0]);
        let top = solid_texels([0.5, 0.5, 0.5, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::ColorDodge),
        ]);
        assert_eq!(first_texel(&out), (0.75, 0.75, 0.75, 1.0));
    }

    // -- DarkerColor/LighterColor (this round): whole-colour-selection
    // modes, non-separable like Hue/Saturation/Color/Luminosity above
    // but a different shape -- no SetLum/SetSat blending, just a
    // straight comparison of `lum(Cb)` against `lum(Cs)` that returns
    // one whole input triple unchanged. `blend_darker_color`/
    // `blend_lighter_color` are called directly (not through
    // `composite_tile_cpu`'s f16 texel round-trip) for the tests that
    // need bit-exact equality, since the function performs no
    // arithmetic on the selected triple -- it returns `cb` or `cs`
    // verbatim, so there is no rounding to tolerate; epsilon comparisons
    // are used only where indicated.

    #[test]
    // The real distinguishing property between DarkerColor (whole-
    // colour selection) and the already-implemented, separable Darken
    // (per-channel `min`): Cb=(0.8, 0.1, 0.1) is a fairly dark-average
    // red (high R, low G/B); Cs=(0.1, 0.8, 0.1) is a fairly dark-average
    // green. Lum(Cb) = 0.3*0.8 + 0.59*0.1 + 0.11*0.1 = 0.24 + 0.059 +
    // 0.011 = 0.31; Lum(Cs) = 0.3*0.1 + 0.59*0.8 + 0.11*0.1 = 0.03 +
    // 0.472 + 0.011 = 0.513 -- independently confirmed by actually
    // running this exact pair through this crate's own `lum` (real f32
    // arithmetic, not hand-rounded): `lum(cb) = 0.3100000024`,
    // `lum(cs) = 0.5129999518`, `lum(cb) < lum(cs)`. Since Lum(Cb) is
    // lower, DarkerColor must return `Cb` **whole** -- exactly
    // `(0.8, 0.1, 0.1)`, bit-for-bit the same triple passed in as `cb`,
    // not a recomputed value. The per-channel minimum of the same two
    // inputs would instead be `(min(0.8,0.1), min(0.1,0.8),
    // min(0.1,0.1)) = (0.1, 0.1, 0.1)` -- a hybrid colour equal to
    // *neither* input -- asserted against directly below to prove the
    // distinction is real and checkable, not just claimed in a comment.
    #[allow(clippy::float_cmp)]
    fn blend_darker_color_picks_the_whole_lower_luminance_colour_not_a_per_channel_hybrid() {
        let cb = [0.8, 0.1, 0.1];
        let cs = [0.1, 0.8, 0.1];
        assert!(
            (lum(cb) - 0.31).abs() < 1e-6,
            "Lum(Cb) must be 0.31, got {}",
            lum(cb)
        );
        assert!(
            (lum(cs) - 0.513).abs() < 1e-6,
            "Lum(Cs) must be 0.513, got {}",
            lum(cs)
        );
        assert!(
            lum(cb) < lum(cs),
            "this test requires a genuinely non-tied Lum(Cb) < Lum(Cs)"
        );

        let result = blend_darker_color(cb, cs);
        assert_eq!(
            result, cb,
            "DarkerColor must pick the whole backdrop colour when it has the lower Lum"
        );

        let per_channel_min = [cb[0].min(cs[0]), cb[1].min(cs[1]), cb[2].min(cs[2])];
        assert_eq!(
            per_channel_min,
            [0.1, 0.1, 0.1],
            "sanity check on the hybrid this test must NOT produce"
        );
        assert_ne!(
            result, per_channel_min,
            "DarkerColor must not degrade to Darken's own per-channel minimum"
        );
    }

    #[test]
    // The mirror image of the DarkerColor test above, same pair: since
    // Lum(Cb)=0.31 < Lum(Cs)=0.513, LighterColor must pick the whole
    // *source* colour, `Cs = (0.1, 0.8, 0.1)` exactly -- and must not
    // degrade to Lighten's own per-channel maximum,
    // `(max(0.8,0.1), max(0.1,0.8), max(0.1,0.1)) = (0.8, 0.8, 0.1)`,
    // a hybrid equal to neither input.
    #[allow(clippy::float_cmp)]
    fn blend_lighter_color_picks_the_whole_higher_luminance_colour_not_a_per_channel_hybrid() {
        let cb = [0.8, 0.1, 0.1];
        let cs = [0.1, 0.8, 0.1];
        assert!(lum(cb) < lum(cs));

        let result = blend_lighter_color(cb, cs);
        assert_eq!(
            result, cs,
            "LighterColor must pick the whole source colour when it has the higher Lum"
        );

        let per_channel_max = [cb[0].max(cs[0]), cb[1].max(cs[1]), cb[2].max(cs[2])];
        assert_eq!(per_channel_max, [0.8, 0.8, 0.1]);
        assert_ne!(
            result, per_channel_max,
            "LighterColor must not degrade to Lighten's own per-channel maximum"
        );
    }

    #[test]
    // The documented tie-breaking convention (`blend_darker_color`'s
    // own doc comment): when `Lum(Cb) == Lum(Cs)` exactly, both
    // DarkerColor and LighterColor resolve to `Cb`, the backdrop.
    // Cb=(0.6, 0.3, 0.1) and Cs=(0.659, 0.27, 0.1) are two genuinely
    // *different* colours (not `Cb == Cs`, which would only prove the
    // functions return early on object identity, not that they compare
    // by Lum) engineered to share the same Lum: starting from
    // Cb=(0.6,0.3,0.1) (this file's own pre-existing general-case Lum,
    // 0.368 -- see `blend_luminosity_matches_an_independently_computed_general_case`
    // and friends), shift R up and G down by amounts that keep the
    // weighted sum fixed: `0.3*dr + 0.59*dg = 0` requires
    // `dr = 0.59*k`, `dg = -0.3*k` for any `k`; taking `k = 0.1` gives
    // `dr = 0.059`, `dg = -0.03`, so
    // `Cs = (0.6+0.059, 0.3-0.03, 0.1) = (0.659, 0.27, 0.1)`.
    // `Lum(Cs) = 0.3*0.659 + 0.59*0.27 + 0.11*0.1 = 0.1977 + 0.1593 +
    // 0.011 = 0.368`, matching `Lum(Cb) = 0.3*0.6 + 0.59*0.3 + 0.11*0.1
    // = 0.18 + 0.177 + 0.011 = 0.368` in exact real-number arithmetic --
    // and independently confirmed to be **bit-exactly** equal under
    // this crate's own real `f32` `lum` (not just equal on paper): both
    // evaluate to `0.3680000007` (`0x3ebc6a7f`), verified by actually
    // compiling and running this exact pair through `lum` before
    // relying on it here, since `0.3`/`0.59`/`0.11` are not exact binary
    // fractions and two different real-valued constructions are not
    // guaranteed to round to the same `f32` bit pattern in general --
    // this specific pair was checked, not assumed.
    #[allow(clippy::float_cmp)]
    fn blend_darker_color_and_blend_lighter_color_break_an_exact_luminance_tie_in_favour_of_the_backdrop()
     {
        let cb = [0.6, 0.3, 0.1];
        let cs = [0.659, 0.27, 0.1];
        assert_ne!(cb, cs, "the two colours must be genuinely different");
        assert_eq!(
            lum(cb),
            lum(cs),
            "this test requires bit-exact equal Lum, not merely close"
        );

        assert_eq!(
            blend_darker_color(cb, cs),
            cb,
            "on an exact Lum tie, DarkerColor must resolve to the backdrop"
        );
        assert_eq!(
            blend_lighter_color(cb, cs),
            cb,
            "on an exact Lum tie, LighterColor must also resolve to the backdrop"
        );
    }

    #[test]
    // DarkerColor, a plain non-tied case run through the real per-texel
    // `composite_tile_cpu` path (not `blend_darker_color` directly),
    // verified against `lum`: backdrop (bottom) Cb=(0.5, 0.2, 0.9) --
    // Lum(Cb) = 0.3*0.5 + 0.59*0.2 + 0.11*0.9 = 0.15 + 0.118 + 0.099 =
    // 0.367 -- versus source (top) Cs=(0.4, 0.4, 0.4), an achromatic
    // grey whose own Lum always equals its own channel value (the
    // spec's weights sum to exactly 0.3+0.59+0.11=1.0), so
    // Lum(Cs)=0.4. Lum(Cb)=0.367 < Lum(Cs)=0.4, so DarkerColor must
    // pick the whole backdrop `(0.5, 0.2, 0.9)` -- not the per-channel
    // minimum `(min(0.5,0.4), min(0.2,0.4), min(0.9,0.4)) =
    // (0.4, 0.2, 0.4)`, a different, hybrid result Darken would give for
    // these same two inputs. Epsilon tolerance (not `assert_eq!`): like
    // the non-separable HSL family's own tests, `0.3`/`0.59`/`0.11`
    // aren't exact binary fractions, so this goes through both `lum`'s
    // own rounding and an `f16` texel round-trip.
    fn composite_tile_cpu_darker_color_picks_the_whole_lower_luminance_backdrop() {
        let bottom = solid_texels([0.5, 0.2, 0.9, 1.0]);
        let top = solid_texels([0.4, 0.4, 0.4, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::DarkerColor),
        ]);
        assert_texel_close(first_texel(&out), (0.5, 0.2, 0.9, 1.0), 1e-3);
    }

    #[test]
    // LighterColor's own mirror image of the test directly above, same
    // two layers: since Lum(Cb)=0.367 < Lum(Cs)=0.4, LighterColor must
    // pick the whole *source* colour, `(0.4, 0.4, 0.4)` -- distinct from
    // both DarkerColor's own result for this pair (`(0.5, 0.2, 0.9)`)
    // and from what Lighten's own per-channel maximum would give
    // (`(max(0.5,0.4), max(0.2,0.4), max(0.9,0.4)) = (0.5, 0.4, 0.9)`,
    // a hybrid equal to neither input).
    fn composite_tile_cpu_lighter_color_picks_the_whole_higher_luminance_source() {
        let bottom = solid_texels([0.5, 0.2, 0.9, 1.0]);
        let top = solid_texels([0.4, 0.4, 0.4, 1.0]);
        let out = composite_tile_cpu(&[
            (&bottom, 1.0, BlendMode::Normal),
            (&top, 1.0, BlendMode::LighterColor),
        ]);
        assert_texel_close(first_texel(&out), (0.4, 0.4, 0.4, 1.0), 1e-3);
    }

    // ---------------------------------------------------------------
    // 0.98.0: `composite_layer_into`'s batched `f16` <-> `f32`
    // conversion. Everything below compares the real function against a
    // *verbatim copy of its own pre-0.98.0 body* (`composite_layer_into_scalar`,
    // immediately below) rather than against hand-derived values: the
    // change claims to be a pure restructuring of how samples reach the
    // blend math, so the only assertion that means anything is
    // bit-for-bit agreement with the loop it replaced.
    // ---------------------------------------------------------------

    /// A literal copy of `composite_layer_into`'s pre-0.98.0 body — the
    /// scalar, per-sample-conversion loop, kept here as the reference
    /// every test below measures the vectorized implementation against.
    ///
    /// Copied verbatim on purpose rather than re-derived from the
    /// formula: a re-derivation could drift toward whatever the new
    /// implementation happens to do, and then agree with it for the
    /// wrong reason. Do not "tidy" this function — its value is that it
    /// is the old code.
    fn composite_layer_into_scalar(out: &mut [f16], texels: &[f16], opacity: f32, mode: BlendMode) {
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
            let straight_backdrop = if backdrop_alpha > 0.0 {
                [
                    dr.to_f32() / backdrop_alpha,
                    dg.to_f32() / backdrop_alpha,
                    db.to_f32() / backdrop_alpha,
                ]
            } else {
                [0.0, 0.0, 0.0]
            };
            let [br, bg, bb] = blend_rgb(
                mode,
                straight_backdrop,
                [sr.to_f32(), sg.to_f32(), sb.to_f32()],
            );
            let blended_r = backdrop_inverse * sr.to_f32() + backdrop_alpha * br;
            let blended_g = backdrop_inverse * sg.to_f32() + backdrop_alpha * bg;
            let blended_b = backdrop_inverse * sb.to_f32() + backdrop_alpha * bb;
            *dr = f16::from_f32(inverse * dr.to_f32() + alpha * blended_r);
            *dg = f16::from_f32(inverse * dg.to_f32() + alpha * blended_g);
            *db = f16::from_f32(inverse * db.to_f32() + alpha * blended_b);
            *da = f16::from_f32(alpha + da.to_f32() * inverse);
        }
    }

    /// Every real [`BlendMode`] variant — the same 26 the benchmark
    /// enumerates. Spelled out rather than derived so that adding a
    /// variant to the enum without adding it here is at least a visible
    /// omission in one place, and so that the bit-exactness test below
    /// genuinely covers the division- and NaN-prone modes (`Divide`,
    /// `ColorDodge`, `ColorBurn`, `HardMix`) and the whole non-separable
    /// HSL family, not just `Normal`.
    const ALL_MODES: [BlendMode; 26] = [
        BlendMode::Normal,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::ColorDodge,
        BlendMode::LinearDodge,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::DarkerColor,
        BlendMode::LighterColor,
    ];

    /// Opacities the fold is checked at. `1.5` and `-0.5` are there to
    /// exercise the `clamp` — which lives in the *caller*, not in
    /// `fold_texel`, so a restructuring that dropped or moved it would
    /// show up here.
    const OPACITIES: [f32; 6] = [1.0, 0.75, 0.5, 0.0, 1.5, -0.5];

    /// The `f16` bit patterns a conversion bug hides in that are still
    /// **finite**: both signed zeros, both subnormal extremes, the
    /// largest finite value, and `0.5` as an ordinary anchor. Used by
    /// the unqualified bit-exactness test, which holds with no
    /// exceptions whatsoever — measured in both the dev and the release
    /// profile.
    const FINITE_EXTREMES: [u16; 6] = [
        0x0000, // +0
        0x8000, // -0
        0x0001, // smallest subnormal
        0x03ff, // largest subnormal
        0x7bff, // largest finite (65504)
        0x3800, // 0.5, an ordinary anchor
    ];

    /// [`FINITE_EXTREMES`] plus both infinities, a quiet NaN and a
    /// **signalling** NaN — the adversarial fixture, reachable in
    /// practice because `aurora-io`'s 16-bit-float TIFF reader takes raw
    /// `f16` samples verbatim with no NaN filtering. Used by the
    /// NaN-tolerant test and by the untouched-tail tests.
    ///
    /// The signalling NaN matters specifically: `f16` -> `f32` -> `f16`
    /// is not the identity for it (`0x7c01` quiets to `0x7e01`), which is
    /// exactly the asymmetry 0.92.0's "take alpha from the original
    /// chunk" rule existed to protect. Here every channel of a processed
    /// texel is a *computed* value, so that rule has nothing to protect
    /// inside the fold — but the untouched tail past the shorter slice's
    /// end must still never be round-tripped, and these bits are what
    /// prove it.
    const EXTREMES: [u16; 10] = [
        0x0000, // +0
        0x8000, // -0
        0x0001, // smallest subnormal
        0x03ff, // largest subnormal
        0x7bff, // largest finite (65504)
        0x7c00, // +infinity
        0xfc00, // -infinity
        0x7e00, // quiet NaN
        0x7c01, // signalling NaN — quieted by an f16 -> f32 -> f16 trip
        0x3800, // 0.5, an ordinary anchor
    ];

    /// Ordinary values, so most texels take the arithmetically normal
    /// path and a bug cannot hide behind an all-NaN fixture where every
    /// result is NaN regardless.
    const ORDINARY: [u16; 6] = [
        0x3c00, // 1.0
        0x3800, // 0.5
        0x3400, // 0.25
        0xbc00, // -1.0
        0x3555, // ~0.3333
        0x0000, // +0.0
    ];

    /// `samples` samples whose value varies per texel *and* per channel,
    /// with every third texel drawn from `extremes` instead of
    /// [`ORDINARY`]. Varying per channel is what makes a swapped
    /// channel, a stale scratch-buffer lane carried between chunks, or a
    /// chunk written at the wrong offset show up as a bit mismatch
    /// rather than hiding behind uniform data.
    fn varied_samples_from(extremes: &[u16], seed: usize, samples: usize) -> Vec<f16> {
        let mut out = Vec::with_capacity(samples);
        for index in 0..samples {
            let texel = index / CHANNELS;
            let pick = index + seed;
            let bits = if (texel + seed).is_multiple_of(3) {
                extremes.get(pick % extremes.len().max(1)).copied()
            } else {
                ORDINARY.get(pick % ORDINARY.len()).copied()
            };
            // A modulo of a non-empty slice's length is always in
            // bounds; the fallback exists only because the workspace
            // denies `unwrap` and `indexing_slicing`.
            out.push(f16::from_bits(bits.unwrap_or(0x3800)));
        }
        out
    }

    /// [`varied_samples_from`] over the finite extremes — the fixture
    /// every unqualified bit-exactness assertion uses.
    fn varied_samples(seed: usize, samples: usize) -> Vec<f16> {
        varied_samples_from(&FINITE_EXTREMES, seed, samples)
    }

    /// [`varied_samples_from`] over the full extremes, infinities and
    /// NaNs included.
    fn varied_hostile_samples(seed: usize, samples: usize) -> Vec<f16> {
        varied_samples_from(&EXTREMES, seed, samples)
    }

    /// Two whole vectorized chunks plus a 3-texel remainder, so a single
    /// fixture exercises the chunk loop, the chunk boundary, *and* the
    /// scalar tail. (A real whole-tile fold has no tail at all — see
    /// `the_chunk_constants_divide_a_whole_tile_evenly` — which is
    /// precisely why the tail needs a fixture built for it.)
    fn boundary_fixture(seed: usize) -> Vec<f16> {
        varied_samples(seed, BOUNDARY_SAMPLES)
    }

    /// [`boundary_fixture`]'s adversarial sibling: same shape, infinities
    /// and NaNs included.
    fn hostile_boundary_fixture(seed: usize) -> Vec<f16> {
        varied_hostile_samples(seed, BOUNDARY_SAMPLES)
    }

    /// Two whole chunks plus three texels, in samples.
    const BOUNDARY_SAMPLES: usize = (CHUNK_TEXELS * 2 + 3) * CHANNELS;

    /// Raw bits, not values: `f16: PartialEq` makes `NaN != NaN`, so
    /// comparing `Vec<f16>` directly would be *vacuous* exactly on the
    /// fixtures that matter most.
    fn bits(texels: &[f16]) -> Vec<u16> {
        texels.iter().map(|sample| sample.to_bits()).collect()
    }

    /// **The headline test for 0.98.0, and it is unqualified.** The
    /// vectorized fold must be bit-for-bit identical to the scalar loop
    /// it replaced — every sample, no exceptions, no tolerance — for
    /// every blend mode, at every opacity including two that exercise
    /// the clamp, on a fixture straddling the chunk boundary and
    /// carrying both signed zeros, both subnormal extremes and the
    /// largest finite `f16`, and on an all-transparent accumulator,
    /// where every texel takes the `backdrop_alpha == 0.0` arm that
    /// skips the straightening divisions (the *only* arm a real
    /// single-root document ever takes, per 0.97.1's correction).
    ///
    /// Its fixture is deliberately **finite**: no infinity, no NaN. That
    /// is not a fixture chosen to make the test pass — the
    /// infinity-and-NaN case is covered, and covered adversarially, by
    /// `a_hostile_fixture_folds_identically_except_for_the_nan_payload_operand`
    /// immediately below, which documents exactly what it can and cannot
    /// assert and why. Splitting them this way keeps *this* assertion
    /// absolute rather than blanket-weakening one test to accommodate a
    /// case that only affects NaN payloads.
    #[test]
    fn vectorized_fold_is_bit_identical_to_the_scalar_reference() {
        let source = boundary_fixture(1);
        let samples = source.len();
        // Seed 0: a varied accumulator, so the `backdrop_alpha > 0.0`
        // arm is reached. Seed 1: a genuinely transparent accumulator
        // derived from the real `transparent_tile`, so it is not just
        // "zeros I typed" but the same buffer every real accumulation
        // starts from.
        let mut transparent = transparent_tile();
        transparent.truncate(samples);
        assert_eq!(
            transparent.len(),
            samples,
            "a whole tile is longer than this fixture, so the truncation is exact"
        );
        let accumulators = [varied_samples(0, samples), transparent];

        for accumulator in &accumulators {
            for mode in ALL_MODES {
                for opacity in OPACITIES {
                    let mut vectorized = accumulator.clone();
                    let mut scalar = accumulator.clone();
                    composite_layer_into(&mut vectorized, &source, opacity, mode);
                    composite_layer_into_scalar(&mut scalar, &source, opacity, mode);
                    assert_eq!(
                        bits(&vectorized),
                        bits(&scalar),
                        "vectorized fold diverged from the scalar reference \
                         for {mode:?} at opacity {opacity}"
                    );
                }
            }
        }
    }

    /// The adversarial half of the bit-exactness claim: the same sweep,
    /// on a fixture that also carries both infinities, a quiet NaN and a
    /// signalling NaN. Every sample must still agree bit-for-bit
    /// **except** where the scalar reference itself produced a NaN, in
    /// which case the vectorized path is required only to produce *a*
    /// NaN, of any sign and payload.
    ///
    /// **That exception is a measured, disclosed limitation, not a
    /// convenience.** It is the same class 0.92.1 found and documented
    /// for `aurora_gpu::residency` (see that module's
    /// `serialize_premultiplied_le_bytes` doc comment): when *both*
    /// operands of a floating-point operation are NaN, which operand's
    /// payload survives is an operand-order detail of whatever code LLVM
    /// emits, pinned by neither IEEE 754 nor this source, and two
    /// different auto-vectorizations of the same arithmetic can choose
    /// differently. Measured here (this sandbox, `x86_64` with F16C,
    /// over all 26 modes × 6 opacities × 2 accumulators):
    ///
    /// | profile | diverging samples | of which both-NaN | of which anything else |
    /// |---|---|---|---|
    /// | dev (`opt-level = 1`) | 96 | 96 | **0** |
    /// | `--release` | 5,808 | 5,808 | **0** |
    ///
    /// **Read those two counts as a one-time manual measurement, not as
    /// something this tree re-derives** (disclosed 0.98.1). They were
    /// produced by temporarily instrumenting this test's loop to count and
    /// classify every mismatch, in each profile, and that instrumentation
    /// was not kept: nothing committed here counts or prints them, so
    /// re-running the suite confirms the *property* below without
    /// reproducing the numbers. What the committed test does enforce
    /// permanently is the load-bearing half — the "anything else" column
    /// being **0** — because it still asserts full `to_bits()` equality
    /// everywhere the scalar reference did not produce a NaN. The exact
    /// counts are profile-, LLVM-version- and target-dependent and would
    /// drift with any of the three, which is why they are recorded as a
    /// dated observation rather than pinned by an assertion.
    ///
    /// So the divergence is *entirely* NaN-payload selection and never a
    /// value: not one sample differed where either side was a number.
    /// The dev-profile cases are all `Luminosity`, all on the red
    /// channel, all where the source texel carries a NaN in R *and* the
    /// blend result is NaN too — `sr + 0.0 * br`, both operands NaN. The
    /// release profile simply auto-vectorizes more of the arithmetic and
    /// so hits the same class far more often, including flipping the NaN
    /// *sign* bit, which is equally unspecified.
    ///
    /// The two conversion routines are **not** the cause, checked
    /// directly rather than assumed: `f16::to_f32` /
    /// `half::slice::HalfFloatSliceExt::convert_to_f32_slice` and
    /// `f16::from_f32` / `convert_from_f32_slice` were measured to agree
    /// bit-for-bit on every NaN, infinity and subnormal pattern in
    /// [`EXTREMES`], in both directions. Nothing about batching the
    /// conversions changes what a conversion does.
    ///
    /// Consequence, bounded: the pixel was a NaN before and is a NaN
    /// after, so this turns garbage into different garbage rather than
    /// corrupting a good pixel. It is reachable — `aurora-io`'s
    /// 16-bit-float TIFF reader takes raw `f16` samples verbatim with no
    /// NaN filtering — and it is carried forward as a disclosed risk, not
    /// chased, exactly as 0.92.1 decided for the same class.
    #[test]
    fn a_hostile_fixture_folds_identically_except_for_the_nan_payload_operand() {
        let source = hostile_boundary_fixture(1);
        let samples = source.len();
        let mut transparent = transparent_tile();
        transparent.truncate(samples);
        let accumulators = [varied_hostile_samples(0, samples), transparent];

        for accumulator in &accumulators {
            for mode in ALL_MODES {
                for opacity in OPACITIES {
                    let mut vectorized = accumulator.clone();
                    let mut scalar = accumulator.clone();
                    composite_layer_into(&mut vectorized, &source, opacity, mode);
                    composite_layer_into_scalar(&mut scalar, &source, opacity, mode);
                    assert_eq!(vectorized.len(), scalar.len());
                    for (index, (got, want)) in vectorized.iter().zip(scalar.iter()).enumerate() {
                        if want.is_nan() {
                            // The one documented exception, and it is
                            // still an assertion: a NaN must stay a NaN.
                            assert!(
                                got.is_nan(),
                                "sample {index} became {:#06x}, not a NaN, for {mode:?} \
                                 at opacity {opacity} where the scalar reference gave \
                                 {:#06x}",
                                got.to_bits(),
                                want.to_bits()
                            );
                        } else {
                            assert_eq!(
                                got.to_bits(),
                                want.to_bits(),
                                "sample {index} diverged on a non-NaN value for {mode:?} \
                                 at opacity {opacity} -- this is outside the documented \
                                 NaN-payload exception and is a real regression"
                            );
                        }
                    }
                }
            }
        }
    }

    /// An accumulator longer than the source: the composited prefix must
    /// still match the scalar reference, **and** every sample past the
    /// source's own length must keep its exact input bits. The tail is
    /// seeded with a signalling NaN among other extremes precisely
    /// because an `f16` -> `f32` -> `f16` round trip would quiet it — so
    /// a version that widened the *whole* `out` slice instead of just
    /// the vectorized head would be caught here even though the
    /// arithmetic it performed was correct.
    #[test]
    fn an_over_long_accumulator_keeps_its_tail_bits_untouched() {
        let source = boundary_fixture(1);
        let extra = 7 * CHANNELS;
        let mut accumulator = varied_samples(0, source.len());
        // Deliberately extreme tail bits, cycled so the signalling NaN
        // lands in it more than once.
        accumulator.extend(
            EXTREMES
                .iter()
                .cycle()
                .take(extra)
                .map(|sample| f16::from_bits(*sample)),
        );
        assert_eq!(accumulator.len(), source.len() + extra);
        let original = bits(&accumulator);

        let mut vectorized = accumulator.clone();
        let mut scalar = accumulator.clone();
        for mode in ALL_MODES {
            vectorized.clone_from(&accumulator);
            scalar.clone_from(&accumulator);
            composite_layer_into(&mut vectorized, &source, 0.75, mode);
            composite_layer_into_scalar(&mut scalar, &source, 0.75, mode);
            assert_eq!(
                bits(&vectorized),
                bits(&scalar),
                "over-long accumulator diverged from the scalar reference for {mode:?}"
            );
            let tail_start = source.len();
            assert_eq!(
                bits(&vectorized).get(tail_start..),
                original.get(tail_start..),
                "samples past the source's own length must be untouched for {mode:?}"
            );
        }
    }

    /// The mirror: a source longer than the accumulator composites only
    /// the accumulator's own worth of texels. `out` is the shorter slice
    /// here, so there is no tail to check on the output side — what this
    /// pins is that the extra *source* samples cannot change the result.
    /// (Deliberately *not* "and reads nothing past it": safe Rust gives
    /// this test no way to observe a read, so that is not something it
    /// proves. Corrected in 0.98.1.)
    #[test]
    fn an_over_long_source_composites_only_the_shorter_prefix() {
        let accumulator = boundary_fixture(0);
        let mut source = varied_samples(1, accumulator.len());
        let prefix = source.clone();
        source.extend(EXTREMES.iter().map(|bits| f16::from_bits(*bits)));
        source.extend(EXTREMES.iter().map(|bits| f16::from_bits(*bits)));
        assert!(source.len() > accumulator.len());

        for mode in ALL_MODES {
            let mut vectorized = accumulator.clone();
            let mut scalar = accumulator.clone();
            let mut truncated = accumulator.clone();
            composite_layer_into(&mut vectorized, &source, 0.75, mode);
            composite_layer_into_scalar(&mut scalar, &source, 0.75, mode);
            // The same fold against a source truncated to exactly the
            // accumulator's length: the extra samples must make no
            // difference at all.
            composite_layer_into(&mut truncated, &prefix, 0.75, mode);
            assert_eq!(
                bits(&vectorized),
                bits(&scalar),
                "over-long source diverged from the scalar reference for {mode:?}"
            );
            assert_eq!(
                bits(&vectorized),
                bits(&truncated),
                "source samples past the accumulator's length changed the result for {mode:?}"
            );
        }
    }

    /// **The test that catches the naive port.** Chunking `out` and
    /// `texels` independently and taking each side's own
    /// `chunks_exact(CHUNK_SAMPLES).remainder()` — the spelling
    /// `aurora_gpu::residency`'s *single*-input serializer can legally
    /// use — starts the two tails at different offsets whenever the two
    /// slices have different whole-chunk counts. Every pair below is
    /// chosen so that they do.
    ///
    /// Worked example, the first row: `out` is 3 whole chunks and
    /// `texels` is 1 chunk plus 37 samples. `out`'s own remainder is
    /// *empty* (768 is a multiple of 256) while `texels`' is 37 samples
    /// starting at 256, so the naive version folds 64 texels and stops;
    /// the correct answer, and what the pre-0.98.0 `zip` gave, is
    /// `min(768, 293) = 293` samples, i.e. 73 texels. The third row is
    /// worse than a short count: `out`'s tail would start at 256 while
    /// `texels`' started at 512, compositing texel *i* against texel
    /// *j*.
    #[test]
    fn mismatched_length_folds_match_the_scalar_reference() {
        let lengths = [
            // Different whole-chunk counts, `out` longer.
            (CHUNK_SAMPLES * 3, CHUNK_SAMPLES + 37),
            // The reverse.
            (CHUNK_SAMPLES + 37, CHUNK_SAMPLES * 3),
            // Both tails non-empty and at different offsets — the
            // misaligned-tail case, not merely a short one.
            (CHUNK_SAMPLES + 37, CHUNK_SAMPLES * 2 + 5),
            (CHUNK_SAMPLES * 2 + 5, CHUNK_SAMPLES + 37),
            // Lengths that are not multiples of `CHANNELS`, so a
            // trailing partial texel is dropped on one or both sides.
            (CHUNK_SAMPLES * 2 + 3, CHUNK_SAMPLES * 2 + 1),
            (CHUNK_SAMPLES - 1, CHUNK_SAMPLES * 2),
            // Wholly below one chunk: everything goes down the scalar
            // tail, nothing is vectorized.
            (CHUNK_SAMPLES - 1, CHUNK_SAMPLES - 3),
            (CHANNELS, CHANNELS * 5),
            // Degenerate: empty on either or both sides.
            (0, CHUNK_SAMPLES),
            (CHUNK_SAMPLES, 0),
            (0, 0),
            // Equal lengths, as a control that the interesting rows
            // above are not the only ones passing.
            (CHUNK_SAMPLES * 4, CHUNK_SAMPLES * 4),
        ];

        for (out_len, source_len) in lengths {
            let accumulator = varied_samples(0, out_len);
            let source = varied_samples(1, source_len);
            for mode in [
                BlendMode::Normal,
                BlendMode::Multiply,
                BlendMode::Divide,
                BlendMode::HardMix,
                BlendMode::Color,
            ] {
                let mut vectorized = accumulator.clone();
                let mut scalar = accumulator.clone();
                composite_layer_into(&mut vectorized, &source, 0.75, mode);
                composite_layer_into_scalar(&mut scalar, &source, 0.75, mode);
                assert_eq!(
                    bits(&vectorized),
                    bits(&scalar),
                    "mismatched fold ({out_len} out samples, {source_len} source samples) \
                     diverged from the scalar reference for {mode:?}"
                );
            }
        }
    }

    /// The chunk constants' load-bearing arithmetic, pinned rather than
    /// left implied: a whole tile is an exact whole number of chunks, so
    /// a real fold never touches the scalar remainder loop, and a chunk
    /// is an exact whole number of texels, so the inner
    /// `chunks_exact(CHANNELS)` never drops one.
    #[test]
    fn the_chunk_constants_divide_a_whole_tile_evenly() {
        assert_ne!(CHUNK_SAMPLES, 0, "a zero-length chunk would divide by zero");
        assert_eq!(CHUNK_SAMPLES, CHUNK_TEXELS * CHANNELS);
        assert_eq!(
            CHUNK_SAMPLES % CHANNELS,
            0,
            "a chunk must be a whole number of texels"
        );
        assert_eq!(
            SAMPLES % CHUNK_SAMPLES,
            0,
            "a whole tile must be a whole number of chunks, or a real fold \
             would reach the scalar remainder loop"
        );
    }
}
