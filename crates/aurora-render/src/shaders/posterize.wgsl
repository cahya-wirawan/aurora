// The Posterize adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::PosterizeOp::apply`, line
// for line): an identity (255 levels) is an exact passthrough, otherwise per
// channel clamp to [0, 1] (NaN reads 0) and `min(floor(x n), n - 1) /
// (n - 1)`. `n` and `n - 1` ride in the uniform as the CPU's own `f32`s.
// For an `f16`-valued input `x n` is exact in `f32` (11 + 8 bits), so the
// bin is chosen identically on both sides; the final division may differ by
// an ulp, which the `f16` rounding hides except at an exact rounding tie.
// The quantisation is DISCONTINUOUS: a texel whose un-premultiplied `f16`
// value differs between the adapters' `straight_backdrop` division and the
// CPU's (translucent backdrops only) can land in the neighbouring bin, a
// disagreement of exactly one bin step `1 / (n - 1)`.
//
// **Division guard.** `top` = n - 1 is at least 1 for every valid level
// count (2..=255); the `top > 0` test keeps the division from ever seeing a
// zero divisor should a malformed uniform arrive (it returns 0 there).
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct PosterizeUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `PosterizeOp::packed_for_gpu`'s layout:
    //   a = (identity, n, n - 1, 0)
    // `identity` = 1 is an exact passthrough (even out of gamut).
    a: vec4<f32>,
};

@group(0) @binding(4) var<uniform> po_u: PosterizeUniform;

// `PosterizeOp::channel` for an input already in [0, 1].
fn po_channel(x: f32) -> f32 {
    if (!(po_u.a.z > 0.0)) {
        return 0.0;
    }
    return min(floor(x * po_u.a.y), po_u.a.z) / po_u.a.z;
}

// `PosterizeOp::apply`.
fn po_apply(c: vec3<f32>) -> vec3<f32> {
    if (po_u.a.x != 0.0) {
        return c;
    }
    let v = clean_unit(c);
    return vec3<f32>(po_channel(v.r), po_channel(v.g), po_channel(v.b));
}

@fragment
fn fs_composite_posterize(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(po_apply(cb));
    let b = adjustment_blend(po_u.mode, cb, cs);
    let a = po_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
