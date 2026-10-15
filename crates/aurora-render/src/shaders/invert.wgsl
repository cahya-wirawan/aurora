// The Invert adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::InvertOp::apply`, line for
// line): clamp each channel to [0, 1] (NaN reads 0 -- `clean_unit`), then
// `1 - x`. Invert is never an identity and has no parameters, so the uniform
// is the four-word header alone. Nothing here can be undefined: no
// division, no `pow`, and `clean_unit` already keeps NaN out of `min`.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct InvertUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(4) var<uniform> inv_u: InvertUniform;

// `InvertOp::apply`.
fn inv_apply(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) - clean_unit(c);
}

@fragment
fn fs_composite_invert(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(inv_apply(cb));
    let b = adjustment_blend(inv_u.mode, cb, cs);
    let a = inv_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
