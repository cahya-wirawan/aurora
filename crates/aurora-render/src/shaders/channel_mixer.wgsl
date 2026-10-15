// The Channel Mixer adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::ChannelMixerOp::apply`,
// line for line): clamp to [0, 1] (NaN reads 0), each output channel is
// `clamp(fma(wr, r, fma(wg, g, fma(wb, b, k))), 0, 1)` from its own row
// (weights and constant already divided by 100 on the CPU, in `f32`);
// monochrome writes the first row's result to all three. An identity mixer
// needs no flag: its rows reproduce the clamped input exactly
// (`fma(1, r, fma(0, g, fma(0, b, 0))) = r`), which is what the CPU's
// `apply` returns for it. No division, no `pow`: nothing is undefined.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct ChannelMixerUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `ChannelMixerOp::packed_for_gpu`'s layout:
    //   a = (monochrome, 0, 0, 0)
    //   rows[i] = (from red, from green, from blue, constant) of output i
    a: vec4<f32>,
    rows: array<vec4<f32>, 3>,
};

@group(0) @binding(4) var<uniform> cm_u: ChannelMixerUniform;

// One output channel of `ChannelMixerOp::apply` (its `mix` closure).
fn cm_mix(row: vec4<f32>, p: vec3<f32>) -> f32 {
    return clamp(fma(row.x, p.r, fma(row.y, p.g, fma(row.z, p.b, row.w))), 0.0, 1.0);
}

// `ChannelMixerOp::apply`.
fn cm_apply(c: vec3<f32>) -> vec3<f32> {
    let p = clean_unit(c);
    let red = cm_mix(cm_u.rows[0], p);
    if (cm_u.a.x != 0.0) {
        return vec3<f32>(red);
    }
    return vec3<f32>(red, cm_mix(cm_u.rows[1], p), cm_mix(cm_u.rows[2], p));
}

@fragment
fn fs_composite_channel_mixer(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(cm_apply(cb));
    let b = adjustment_blend(cm_u.mode, cb, cs);
    let a = cm_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
