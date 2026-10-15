// The Photo Filter adjustment pass (0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::PhotoFilterOp::apply`,
// line for line): clamp to [0, 1] (NaN reads 0), multiply each channel by
// the per-channel transmission `1 + (colour - 1) * density` (computed on the
// CPU in f32 and sent), and with Preserve Luminosity add back the Rec.709
// luma the multiply took away, clamping to [0, 1]. No division, no `pow`:
// nothing here is undefined for any input.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct PhotoFilterUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `PhotoFilterOp::packed_for_gpu`'s layout:
    //   a = (identity, preserve_luminosity, 0, 0)
    //   b = (transmission_r, transmission_g, transmission_b, 0)
    // flags are 0 / 1; `identity` = 1 is an exact passthrough.
    a: vec4<f32>,
    b: vec4<f32>,
};

@group(0) @binding(4) var<uniform> pf_u: PhotoFilterUniform;

// `PhotoFilterOp::apply`.
fn pf_apply(c: vec3<f32>) -> vec3<f32> {
    if (pf_u.a.x != 0.0) {
        return c;
    }
    let px = clean_unit(c);
    let filtered = px * pf_u.b.xyz;
    if (pf_u.a.y == 0.0) {
        return filtered;
    }
    let luma_shift = adjust_luma(px) - adjust_luma(filtered);
    return clamp(filtered + vec3<f32>(luma_shift), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_composite_photo_filter(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(pf_apply(cb));
    let b = adjustment_blend(pf_u.mode, cb, cs);
    let a = pf_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
