// The Gradient Map adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::GradientMapOp::apply` and
// `colour_at`, line for line): clamp to [0, 1] (NaN reads 0), the grey
// `0.30 R + 0.59 G + 0.11 B` (unfused, clamped), `t = 1 - grey` with
// Reverse, then the gradient's colour at `t`: the first stop's colour before
// the first stop, the last stop's at or after the last, the linear
// interpolation in encoded sRGB between the two stops around `t`, the LATER
// stop winning at a shared position. `partition_point(|s| s.pos <= t)` is a
// binary search over the stops (at most 64, so at most 7 halvings; the loop
// is bounded at 8 and its exit is the `lo >= hi` test).
//
// The stops ride in the uniform as up to 64 `vec4`s `(position, r, g, b)`
// (std140: a `vec4` array has a 16-byte stride, so no padding to track); the
// header's count says how many are real.
//
// **WGSL-undefined cases, guarded.** The count is clamped to [0, 64] before
// `u32()` (a float-to-int conversion of an out-of-range value is
// indeterminate) and a zero count returns black, as the CPU's `_ => [0; 3]`
// arm does (unreachable for a validated gradient: it has at least two
// stops). The interpolation divides by `b.pos - a.pos`, which is strictly
// positive because `a.pos <= t < b.pos`; the `span > 0` test is a guard for
// a malformed uniform and cannot fire for a valid one (it returns the
// earlier stop's colour). The interpolation is left as `a + (b - a) * u`; a
// driver may fuse it into an `fma`, a difference of an ulp.
//
// A hard step (two stops at one position) is DISCONTINUOUS in `t`: a texel
// whose grey differs by an ulp between the CPU's unfused sum and the GPU
// (possibly fused) can land on the other side of the step.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct GradientMapUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `GradientMapOp::packed_for_gpu`'s layout:
    //   a = (reverse, stop count, 0, 0)
    //   stops[i] = (position, red, green, blue), in order of position
    a: vec4<f32>,
    stops: array<vec4<f32>, 64>,
};

@group(0) @binding(4) var<uniform> gm_u: GradientMapUniform;

const GM_MAX_STOPS: u32 = 64u;

// `GradientMapOp::colour_at`.
fn gm_colour_at(t_in: f32) -> vec3<f32> {
    var t = t_in;
    // `!(t >= 0)` is true for NaN as well as negatives: both read 0.
    if (!(t >= 0.0)) {
        t = 0.0;
    }
    t = min(t, 1.0);
    let count = min(u32(clamp(gm_u.a.y, 0.0, 64.0)), GM_MAX_STOPS);
    if (count == 0u) {
        return vec3<f32>(0.0);
    }
    // `partition_point`: the number of stops at or before `t`.
    var lo = 0u;
    var hi = count;
    for (var i = 0u; i < 8u; i = i + 1u) {
        if (lo >= hi) {
            break;
        }
        let mid = (lo + hi) / 2u;
        if (gm_u.stops[mid].x <= t) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    if (lo == 0u) {
        return gm_u.stops[0].yzw;
    }
    let a = gm_u.stops[lo - 1u];
    if (lo >= count) {
        return a.yzw;
    }
    let b = gm_u.stops[lo];
    let span = b.x - a.x;
    if (!(span > 0.0)) {
        return a.yzw;
    }
    let u = (t - a.x) / span;
    return a.yzw + (b.yzw - a.yzw) * u;
}

// `GradientMapOp::apply`.
fn gm_apply(c: vec3<f32>) -> vec3<f32> {
    let p = clean_unit(c);
    let grey = clamp(0.30 * p.r + 0.59 * p.g + 0.11 * p.b, 0.0, 1.0);
    if (gm_u.a.x != 0.0) {
        return gm_colour_at(1.0 - grey);
    }
    return gm_colour_at(grey);
}

@fragment
fn fs_composite_gradient_map(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(gm_apply(cb));
    let b = adjustment_blend(gm_u.mode, cb, cs);
    let a = gm_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
