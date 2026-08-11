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
/// time, falling every one of the 3 not-yet-implemented modes
/// (`Dissolve`, `DarkerColor`, `LighterColor` — this family's own
/// explicit remainder, see [`composite_tile_cpu`]'s own doc comment)
/// back to [`Self::Normal`] as an honest, documented degrade — not a
/// bug, since those modes' real math is separate, still-open follow-on
/// work.
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
    // -- Non-separable modes (this round): each is a function of the
    // whole (R,G,B) triple, not a per-channel function of one channel in
    // isolation -- see `blend_rgb` below, not `blend_channel`, for their
    // real math.
    Hue,
    Saturation,
    Color,
    Luminosity,
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
        // `Hue`/`Saturation`/`Color`/`Luminosity` are non-separable --
        // no per-channel value of `B(Cb,Cs)` can express a property of
        // a whole colour (see this function's own module-level doc
        // comment, and `blend_rgb`'s own doc comment, for why). Named
        // individually rather than folded into a wildcard so the match
        // stays exhaustive (the same discipline `aurora-app`'s own
        // `translate_blend_mode` already uses), but
        // [`blend_rgb`] intercepts all 4 before ever reaching this
        // function, so these arms are never actually exercised by any
        // real caller in this crate -- they exist purely so this match
        // still compiles against the shared, now-24-variant
        // [`BlendMode`] enum. Each degrades to the same pass-through
        // `cs` the `Normal` arm above already returns, rather than
        // introducing a new path that could panic if some future
        // caller ever did reach this function directly with one of
        // these 4 modes -- the same "honest fallback over panicking"
        // discipline `aurora-app`'s own `translate_blend_mode` already
        // uses at its own translation boundary for these same 4 modes.
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => cs,
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

/// The whole-triple counterpart of [`blend_channel`]: `B(Cb, Cs)` for
/// `mode`, given the backdrop's and source's own full `(R,G,B)` triples
/// rather than one channel each. Exists because the 4 non-separable
/// modes ([`blend_hue`], [`blend_saturation`], [`blend_color`],
/// [`blend_luminosity`]) genuinely cannot be expressed as
/// [`blend_channel`]'s own per-channel signature — every one of the 20
/// separable modes, by contrast, delegates straight back to
/// [`blend_channel`] three times, once per channel, unchanged from
/// before this function existed: [`composite_tile_cpu`] below now calls
/// this once per texel instead of calling [`blend_channel`] three
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
/// **Scope, stated honestly**: [`BlendMode`] implements 24 of
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
/// above rather than needing new math of its own, and the
/// non-separable family (`Hue`, `Saturation`, `Color`, `Luminosity`),
/// each a whole-`(R,G,B)`-triple function via `blend_rgb` rather
/// than `blend_channel` (see that function's own doc comment for
/// why). The remaining 3 (`Dissolve`, stochastic per-pixel selection
/// rather than a deterministic blend function at all; and
/// `DarkerColor`/`LighterColor`, which compare backdrop and source by
/// overall luminosity rather than blending them) are this family's own
/// explicit boundary — qualitatively different from every mode
/// implemented so far, separate, still-open follow-on work. A layer
/// using one of those 3 falls back to `Normal` at the `aurora-app`
/// translation boundary (`translate_blend_mode`), not here. This is a
/// CPU implementation specifically because the
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
            let [br, bg, bb] = blend_rgb(
                mode,
                [dr.to_f32(), dg.to_f32(), db.to_f32()],
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
    use super::{
        BlendMode, TileCompositor, blend_channel, blend_color, blend_hue, blend_luminosity,
        blend_saturation, clip_color, composite_tile_cpu, lum, sat, set_lum, set_sat, soft_light_d,
    };
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
}
