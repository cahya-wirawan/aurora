// The Curves adjustment pass (0.159.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` (`concat!`), so it can call the blend-math helpers and
// `straight_backdrop` defined there. It is a separate file, and a separate
// `wgpu::ShaderModule`, so `composite.wgsl`'s own entry-point roster (and
// the `ALL_BLEND_PASSES` set-equality guard over it) is untouched.
//
// What it computes is `aurora-app`'s `apply_adjustment_layer`, step for
// step (the CPU path is the reference):
//
// 1. `Cb` = the accumulator un-premultiplied (`straight_backdrop`), clamped
//    to the finite `f16` range and rounded to `f16`, as
//    `aurora_render::un_premultiply_in_place` stores it;
// 2. `Cs` = `f(Cb)`: each channel through its own table, then all three
//    through the composite table (`aurora_filters::CurvesLut::map_channel`),
//    rounded to `f16` (the CPU's source tile is `f16`);
// 3. `mixed` = `lerp(Cb, B(Cb, Cs), opacity)` -- `fold_texel` over an
//    opaque backdrop with a source alpha of `1` -- rounded to `f16`;
// 4. written back premultiplied at the backdrop's own alpha.
//
// The `f16` roundings use `pack2x16float`, whose rounding direction WGSL
// leaves to the implementation (nearest or either neighbour), so they
// match the CPU's round-to-nearest-even to within one `f16` step -- they
// exist to keep the GPU on the CPU's quantisation grid, not to be exact.
//
// Bindings: 1 (the shared nearest sampler) and 3 (`backdrop_tex`) are the
// declarations in `composite.wgsl`; 4 and 5 are this pass's own. Bindings
// 0 and 2 are never touched by this entry point, so its bind group layout
// does not carry them.

struct CurvesUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(4) var<uniform> curves_u: CurvesUniform;

// `aurora_filters::CurvesLut::packed_for_gpu`'s layout: a 16-float header
// of four `[present, lo, hi, 0]` records (red, green, blue, composite),
// then four tables of CURVES_SAMPLES `f32` samples each.
@group(0) @binding(5) var<storage, read> curves_lut: array<f32>;

const CURVES_INTERVALS: f32 = 16384.0;
const CURVES_SAMPLES: u32 = 16385u;
const CURVES_HEADER: u32 = 16u;

// `Table::lookup`, for table `t`: input clamp to [0, 1] (NaN reads 0), the
// curve's own input-range clamp (flat beyond a moved endpoint), linear
// interpolation in f32, output clamp to [0, 1]. An identity table is an
// exact passthrough, as `map_channel` skips it.
fn curves_lookup(t: u32, value: f32) -> f32 {
    let base = t * 4u;
    if (curves_lut[base] == 0.0) {
        return value;
    }
    var x = value;
    // `!(x >= 0)` is true for NaN as well as negatives: both read 0.
    if (!(x >= 0.0)) {
        x = 0.0;
    }
    x = min(x, 1.0);
    let lo = curves_lut[base + 1u];
    let hi = max(curves_lut[base + 2u], lo);
    x = min(max(x, lo), hi);
    let position = x * CURVES_INTERVALS;
    // `position` is in [0, INTERVALS]; the last sample's interval is the
    // one before it, at frac == 1.0.
    let index = min(u32(position), CURVES_SAMPLES - 2u);
    let frac = position - f32(index);
    let offset = CURVES_HEADER + t * CURVES_SAMPLES + index;
    let low = curves_lut[offset];
    let high = curves_lut[offset + 1u];
    return clamp(fma(high - low, frac, low), 0.0, 1.0);
}

// `CurvesLut::apply`: per channel first, then composite.
fn curves_apply(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        curves_lookup(3u, curves_lookup(0u, c.r)),
        curves_lookup(3u, curves_lookup(1u, c.g)),
        curves_lookup(3u, curves_lookup(2u, c.b)),
    );
}

// Rounds to the nearest `f16` (see the header for "nearest"). The input is
// clamped to the finite `f16` range first: WGSL leaves `pack2x16float`'s
// result indeterminate outside it, and an out-of-gamut backdrop can push a
// blend term there (Overlay/HardLight of a `Cb` near 65504 reach ~1.3e5).
// The CPU's `half::f16::from_f32` would give infinity there instead; both
// are far outside anything the tolerance is about, and the clamp only
// makes the GPU's answer defined.
fn curves_f16(c_in: vec3<f32>) -> vec3<f32> {
    let c = clamp(c_in, vec3<f32>(-65504.0), vec3<f32>(65504.0));
    let rg = unpack2x16float(pack2x16float(c.rg));
    let b = unpack2x16float(pack2x16float(vec2<f32>(c.b, 0.0)));
    return vec3<f32>(rg.x, rg.y, b.x);
}

// `blend_rgb(mode, cb, cs)` for the nineteen modes `aurora-render`'s
// `curves_mode_code` admits. Each arm calls the very helper that mode's own
// `fs_composite_*` entry point in `composite.wgsl` calls (0.159.0 review:
// one copy of every formula, so a fix to a mode cannot miss the Curves
// pass). Codes are `curves_mode_code`'s, and
// `curves_switch_matches_curves_mode_code_and_all_blend_passes` checks
// that every arm calls `blend_<mode>` under its own code. An unknown code
// never reaches here (the Rust side refuses it) and falls back to Normal.
fn curves_blend(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return blend_multiply(cb, cs); }
        case 2u: { return blend_darken(cb, cs); }
        case 3u: { return blend_lighten(cb, cs); }
        case 4u: { return blend_screen(cb, cs); }
        case 5u: { return blend_difference(cb, cs); }
        case 6u: { return blend_linear_dodge(cb, cs); }
        case 7u: { return blend_linear_burn(cb, cs); }
        case 8u: { return blend_color_burn(cb, cs); }
        case 9u: { return blend_color_dodge(cb, cs); }
        case 10u: { return blend_overlay(cb, cs); }
        case 11u: { return blend_hard_light(cb, cs); }
        case 12u: { return blend_linear_light(cb, cs); }
        case 13u: { return blend_vivid_light(cb, cs); }
        case 14u: { return blend_hard_mix(cb, cs); }
        case 15u: { return blend_pin_light(cb, cs); }
        case 16u: { return blend_soft_light(cb, cs); }
        case 17u: { return blend_subtract(cb, cs); }
        case 18u: { return blend_divide(cb, cs); }
        default: { return cs; }                                        // Normal (0)
    }
}

@fragment
fn fs_composite_curves(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = curves_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = curves_f16(curves_apply(cb));
    let b = curves_blend(curves_u.mode, cb, cs);
    let a = curves_u.opacity;
    let mixed = curves_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
