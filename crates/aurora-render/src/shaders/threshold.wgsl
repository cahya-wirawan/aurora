// The Threshold adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::ThresholdOp::apply`, line
// for line): clamp to [0, 1] (NaN reads 0), the grey `fma(wr, r, fma(wg, g,
// wb * b))` with Photoshop's weights, white when `grey >= edge`, else black.
// The weights and the edge ride in the uniform as the very `f32`s the CPU
// uses (`ThresholdOp::packed_for_gpu`), so a grey exactly on the edge is
// decided by the same comparison; both sides use a true `fma`, so for the
// same input texel the grey is bit-identical. The step is DISCONTINUOUS: a
// texel whose un-premultiplied `f16` value differs between the adapters'
// `straight_backdrop` division and the CPU's (translucent backdrops only;
// an opaque backdrop divides by exactly 1) can flip to the other colour.
// No division, no `pow`: nothing is undefined for any input.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct ThresholdUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `ThresholdOp::packed_for_gpu`'s layout:
    //   a = (edge, weight_r, weight_g, weight_b)
    a: vec4<f32>,
};

@group(0) @binding(4) var<uniform> th_u: ThresholdUniform;

// `ThresholdOp::apply`.
fn th_apply(c: vec3<f32>) -> vec3<f32> {
    let px = clean_unit(c);
    let grey = fma(th_u.a.y, px.r, fma(th_u.a.z, px.g, th_u.a.w * px.b));
    if (grey >= th_u.a.x) {
        return vec3<f32>(1.0);
    }
    return vec3<f32>(0.0);
}

@fragment
fn fs_composite_threshold(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(th_apply(cb));
    let b = adjustment_blend(th_u.mode, cb, cs);
    let a = th_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
