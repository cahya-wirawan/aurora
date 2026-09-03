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
/// That method's own command encoder.
const LABEL_MULTIPLY_ENCODER: &str = "composite.multiply.encoder";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_MULTIPLY_PASS: &str = "composite.multiply.pass";
/// The pipeline layout and render pipeline behind
/// [`TileCompositor::composite_darken_over_with_opacity`] — the same
/// five-label set `Multiply` above carries, for the same reason: two
/// blend-math pipelines sharing one `"composite"` label would leave a
/// `wgpu` validation message or a frame capture unable to say which
/// blend mode is at fault.
const LABEL_DARKEN: &str = "composite.darken";
/// That method's own per-call uniform buffer.
const LABEL_DARKEN_UNIFORM: &str = "composite.darken.opacity";
/// That method's own per-call bind group.
const LABEL_DARKEN_BIND_GROUP: &str = "composite.darken.bind_group";
/// That method's own command encoder.
const LABEL_DARKEN_ENCODER: &str = "composite.darken.encoder";
/// That method's own render pass — the label a `wgpu` validation error
/// or a frame capture actually names.
const LABEL_DARKEN_PASS: &str = "composite.darken.pass";

/// Everything that differs between one shader-computed blend mode's
/// composite pass and another's: the `shaders/composite.wgsl` fragment
/// entry point, and the five `wgpu` debug labels that name its pipeline,
/// uniform buffer, bind group, encoder and render pass.
///
/// This is the whole variation between
/// [`TileCompositor::composite_multiply_over_with_opacity`] and
/// [`TileCompositor::composite_darken_over_with_opacity`] — six
/// `&'static str`s. Everything else those two methods do is
/// `composite_blend_over_with_opacity`, which they now
/// both delegate to; see that method for why the collapse was safe to
/// make at two modes rather than deferred to a third.
///
/// Carrying `fragment_entry` here rather than as a seventh parameter is
/// deliberate: it keeps the shared method at exactly seven arguments
/// (`self` included), which is `clippy::too_many_arguments`'s own limit,
/// so a third mode adds a `const` and not an `allow`.
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
    /// The command encoder.
    encoder: &'static str,
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
    encoder: LABEL_MULTIPLY_ENCODER,
    pass: LABEL_MULTIPLY_PASS,
};

/// [`BlendMode::Darken`]'s, likewise.
const BLEND_PASS_DARKEN: BlendPass = BlendPass {
    fragment_entry: "fs_composite_darken",
    pipeline: LABEL_DARKEN,
    uniform: LABEL_DARKEN_UNIFORM,
    bind_group: LABEL_DARKEN_BIND_GROUP,
    encoder: LABEL_DARKEN_ENCODER,
    pass: LABEL_DARKEN_PASS,
};

/// The byte size of `composite_over_with_opacity`'s own uniform buffer —
/// a real `f32` opacity value plus 12 bytes of padding, matching
/// `shaders/composite.wgsl`'s own `Opacity` struct exactly.
const OPACITY_UNIFORM_SIZE: u64 = 16;

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
#[must_use]
#[allow(clippy::many_single_char_names)]
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let [r, g, b] = c;
    let n = r.min(g).min(b);
    let x = r.max(g).max(b);
    let [r, g, b] = if n < 0.0 {
        [
            l + (r - l) * l / (l - n),
            l + (g - l) * l / (l - n),
            l + (b - l) * l / (l - n),
        ]
    } else {
        [r, g, b]
    };
    if x > 1.0 {
        [
            l + (r - l) * (1.0 - l) / (x - l),
            l + (g - l) * (1.0 - l) / (x - l),
            l + (b - l) * (1.0 - l) / (x - l),
        ]
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
/// order — **by construction**, not by test: the loop below reads no
/// state at all beyond `dst`, `src`, `opacity` and `mode`, so there is
/// nothing a batch call could carry across layers that N separate calls
/// could not. What pins the math itself is
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
pub fn composite_layer_into(out: &mut [f16], texels: &[f16], opacity: f32, mode: BlendMode) {
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
        // Recover the backdrop's true straight-alpha colour before
        // handing it to `blend_rgb` as `Cb` -- see this function's
        // own doc comment above for why the raw accumulator state
        // isn't always already straight alpha.
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
/// built to exactly the same shape.
/// The remaining 24 modes stay CPU-only (`composite_tile_cpu`) until
/// their own formulas are ported — this crate's own `BlendMode` enum
/// has 26 variants (it excludes `Dissolve`, which is a pre-composite
/// gate, never a per-pixel formula this crate would need to port), so
/// 24 is "26 minus the two, `Multiply` and `Darken`, done so far."
/// `aurora-app`'s own
/// count of what its GPU predicate still rejects is 23, one lower,
/// because `Dissolve` is *admitted* there (0.84.1) without ever needing
/// a formula here — see PLAN.md's 0.84.1 addendum if the two numbers
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
    /// `bind_group_layout_opacity` above, shared by
    /// [`Self::composite_multiply_over_with_opacity`] and
    /// [`Self::composite_darken_over_with_opacity`]: the same texture +
    /// sampler +
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
/// `shaders/composite.wgsl`: `fs_composite_multiply` (0.83.0) and
/// `fs_composite_darken` (0.85.0) so far. A newly ported mode needs its
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
/// descriptor. It has held so far (`Darken`, 0.85.0, added exactly
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
    /// itself is entirely unchanged by this method's existence: same
    /// signature, same shader entry point, same pipeline key, so every
    /// caller/test of it keeps its exact prior behaviour.
    pub fn composite_over_with_opacity(
        &mut self,
        context: &GpuContext,
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
    /// current reasoning on why the two ported modes are still two public
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
    pub fn composite_multiply_over_with_opacity(
        &mut self,
        context: &GpuContext,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
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
    /// The *public* shape is still two named methods rather than one
    /// `mode: BlendMode` parameter, and that part is a real deferral: a
    /// `mode` parameter would have to say what happens for the 24 modes
    /// with no WGSL entry point behind them (panic — denied here; return
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
    pub fn composite_darken_over_with_opacity(
        &mut self,
        context: &GpuContext,
        src: &wgpu::TextureView,
        backdrop: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        opacity: f32,
    ) {
        self.composite_blend_over_with_opacity(
            context,
            src,
            backdrop,
            dst,
            opacity,
            &BLEND_PASS_DARKEN,
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
    /// with the six values that differed lifted into [`BlendPass`] — the
    /// same discipline, and the same "no existing test needed to change"
    /// bar, 0.83.1 used when it extracted [`composite_pipeline`] and
    /// [`opacity_uniform_buffer`] out from under those same callers. The
    /// `composite_multiply_*` and `composite_darken_*` differentials in
    /// this module's tests, each checking the shader's output against
    /// [`composite_tile_cpu`]'s own on real hardware, are what makes that
    /// checkable rather than asserted.
    ///
    /// **Private, and staying private.** A caller outside this crate
    /// picks a mode by picking a method; handing it a [`BlendPass`]
    /// would let it name a `fragment_entry` that does not exist (a
    /// pipeline-creation failure, not a compile error) or pair an entry
    /// point with the wrong labels.
    fn composite_blend_over_with_opacity(
        &mut self,
        context: &GpuContext,
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(blend_pass.encoder),
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
    use super::{
        BlendMode, TileCompositor, blend_channel, blend_color, blend_darker_color, blend_hue,
        blend_lighter_color, blend_luminosity, blend_saturation, clip_color, composite_layer_into,
        composite_tile_cpu, lum, sat, set_lum, set_sat, soft_light_d, transparent_tile,
        un_premultiply_in_place,
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
        compositor.composite_over_with_opacity(&context, &dst_view, &src_view, 0.25);

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
        compositor.composite_over_with_opacity(&context, &dst_view, &src_view, 0.0);

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
        compositor.composite_over_with_opacity(&context, &dst_view, &src_view, 5.0);

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
        compositor.composite_over_with_opacity(&context, &dst_view, &src_view, opacity);
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

    // -- Real in-shader blend-mode math on the GPU, slice 1 of the
    // blend-mode port: `Multiply` only, via
    // `TileCompositor::composite_multiply_over_with_opacity` and the
    // `fs_composite_multiply` entry point.
    //
    // These tests exist to answer one specific question the rest of
    // this workspace had never answered: **can a shader sample a texture
    // that a previous render pass wrote to as a colour attachment, after
    // an intervening `queue.submit`?** Nothing here had ever done it --
    // every prior GPU test in this file seeds its sampled textures with
    // `queue.write_texture` and only ever *writes* to a render
    // attachment. Real blend-mode math has no alternative: the
    // fixed-function blend unit can express `Normal` and nothing else,
    // so `Cb` has to arrive as a sampled texture.
    //
    // Every one of them therefore builds its accumulator with a real
    // `composite_over_with_opacity` render pass (which submits on its
    // own) and then hands that same texture to the multiply pass as
    // `backdrop`. Seeding it with `write_texture` instead would pass
    // just as easily and prove nothing about the mechanism.
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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(
            &context,
            &backdrop_view,
            &bottom_view,
            bottom_opacity,
        );
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            opacity,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            opacity,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        // 5.0, clamped Rust-side before it ever reaches the uniform --
        // if the clamp were missing, `a` would come out > 1.0 and the
        // final `inv = 1.0 - a` would go negative.
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            5.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 0.0);
        compositor.composite_multiply_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
    /// It works, with no barrier, copy or synchronisation beyond what
    /// `wgpu` already inserts between submissions -- on Vulkan/NVIDIA.
    /// Metal and DX12 are unverified.
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
        // Pass 1: build the accumulator in `ping`.
        compositor.composite_over_with_opacity(&context, &ping_view, &l1_view, 1.0);
        // Pass 2: sample `ping`, write `pong`.
        compositor
            .composite_multiply_over_with_opacity(&context, &l2_view, &ping_view, &pong_view, 1.0);
        // Pass 3: sample `pong`, write back into `ping` -- the texture
        // pass 2 read from. This is the hop 0.83.0 never took.
        compositor.composite_multiply_over_with_opacity(
            &context, &l3_view, &pong_view, &ping_view, opacity3,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(
            &context,
            &backdrop_view,
            &bottom_view,
            bottom_opacity,
        );
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            opacity,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            opacity,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 1.0);
        // 5.0, clamped Rust-side before it ever reaches the uniform --
        // if the clamp were missing, `a` would come out > 1.0 and the
        // final `inv = 1.0 - a` would go negative.
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            5.0,
        );

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
        compositor.composite_over_with_opacity(&context, &backdrop_view, &bottom_view, 0.0);
        compositor.composite_darken_over_with_opacity(
            &context,
            &top_view,
            &backdrop_view,
            &dst_view,
            1.0,
        );

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
        // Pass 1: build the accumulator in `ping`.
        compositor.composite_over_with_opacity(&context, &ping_view, &l1_view, 1.0);
        // Pass 2: sample `ping`, write `pong`.
        compositor
            .composite_multiply_over_with_opacity(&context, &l2_view, &ping_view, &pong_view, 1.0);
        // Pass 3: a *different* blend mode, sampling `pong` and writing
        // back into `ping` -- the same shared pair, no third texture.
        compositor
            .composite_darken_over_with_opacity(&context, &l3_view, &pong_view, &ping_view, 1.0);

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
}
