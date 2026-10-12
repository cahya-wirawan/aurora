// The Curves adjustment pass (0.159.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`), so it can call the
// blend-math helpers, `straight_backdrop`, `adjustment_f16` and
// `adjustment_blend` defined there. It is a separate file, and a separate
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

@fragment
fn fs_composite_curves(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(curves_apply(cb));
    let b = adjustment_blend(curves_u.mode, cb, cs);
    let a = curves_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
