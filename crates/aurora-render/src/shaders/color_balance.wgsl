// The Color Balance adjustment pass (0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::ColorBalanceOp::apply`,
// line for line): clamp to [0, 1] (NaN reads 0); the Rec.709 luma `y`
// weighs shadows `(1 - y)^2`, midtones `4 y (1 - y)`, highlights `y^2`;
// each channel moves by `fma(shadows, s, fma(midtones, m, highlights * h))`
// where the slider shifts are `slider / 100 * 0.5` (computed on the CPU in
// f32 and sent); with Preserve Luminosity the luma of those moves is
// subtracted from every channel; clamp to [0, 1]. No division, no `pow`:
// nothing here is undefined for any input.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct ColorBalanceUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `ColorBalanceOp::packed_for_gpu`'s layout:
    //   p[0] = (identity, preserve_luminosity, 0, 0)
    //   p[1] = shadows shifts   (red, green, blue, 0)
    //   p[2] = midtones shifts  (red, green, blue, 0)
    //   p[3] = highlights shifts(red, green, blue, 0)
    // flags are 0 / 1; `identity` = 1 is an exact passthrough.
    p: array<vec4<f32>, 4>,
};

@group(0) @binding(4) var<uniform> cbal_u: ColorBalanceUniform;

// `ColorBalanceOp::apply`.
fn cbal_apply(c: vec3<f32>) -> vec3<f32> {
    if (cbal_u.p[0].x != 0.0) {
        return c;
    }
    let px = clean_unit(c);
    let y = adjust_luma(px);
    let inv = 1.0 - y;
    let shadows = inv * inv;
    let midtones = 4.0 * y * inv;
    let highlights = y * y;
    let delta = vec3<f32>(
        fma(shadows, cbal_u.p[1].x, fma(midtones, cbal_u.p[2].x, highlights * cbal_u.p[3].x)),
        fma(shadows, cbal_u.p[1].y, fma(midtones, cbal_u.p[2].y, highlights * cbal_u.p[3].y)),
        fma(shadows, cbal_u.p[1].z, fma(midtones, cbal_u.p[2].z, highlights * cbal_u.p[3].z)),
    );
    var restore = 0.0;
    if (cbal_u.p[0].y != 0.0) {
        restore = -adjust_luma(delta);
    }
    return clamp(
        (px + delta) + vec3<f32>(restore),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

@fragment
fn fs_composite_color_balance(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(cbal_apply(cb));
    let b = adjustment_blend(cbal_u.mode, cb, cs);
    let a = cbal_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
