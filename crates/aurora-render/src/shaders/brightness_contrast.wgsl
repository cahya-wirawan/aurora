// The Brightness/Contrast adjustment pass for the Use Legacy line (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// Only the LINEAR cases are computed here. The default mode is a
// 16,385-sample table (`aurora_filters::CurvesLut`, a natural cubic spline
// sampled once on the CPU): `aurora-app` hands that layer to the existing
// Curves pass (`composite_curves_with_opacity`, the same storage-buffer
// upload cached per parameter set), so the GPU interpolates the CPU's own
// samples and no spline is ever evaluated on the GPU.
//
// The CPU path is the reference (`aurora_filters::BrightnessContrastOp::
// apply`, line for line). `a.x` is the kind: 0 = not applied (an exact
// passthrough, even out of gamut), 1 = the line `moved = x + shift - mean;
// out = clamp(fma(moved, slope, mean), 0, 1)`, 2 = the contrast-100 step
// (`moved >= 0` is white, else black; the CPU's infinite slope never
// reaches the GPU -- `slope` is sent as 0). Input is clamped to [0, 1]
// first (NaN reads 0). No division, no `pow`: nothing is undefined.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct BrightnessContrastUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `BrightnessContrastOp::packed_for_gpu`'s layout:
    //   a = (kind, shift, mean, slope)
    a: vec4<f32>,
};

@group(0) @binding(4) var<uniform> bc_u: BrightnessContrastUniform;

// `BrightnessContrastOp::channel` for the legacy mode.
fn bc_channel(v: f32) -> f32 {
    let moved = v + bc_u.a.y - bc_u.a.z;
    if (bc_u.a.x == 2.0) {
        if (moved >= 0.0) {
            return 1.0;
        }
        return 0.0;
    }
    return clamp(fma(moved, bc_u.a.w, bc_u.a.z), 0.0, 1.0);
}

// `BrightnessContrastOp::apply`.
fn bc_apply(c: vec3<f32>) -> vec3<f32> {
    if (bc_u.a.x == 0.0) {
        return c;
    }
    let v = clean_unit(c);
    return vec3<f32>(bc_channel(v.r), bc_channel(v.g), bc_channel(v.b));
}

@fragment
fn fs_composite_brightness_contrast(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(bc_apply(cb));
    let b = adjustment_blend(bc_u.mode, cb, cs);
    let a = bc_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
