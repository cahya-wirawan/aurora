// The Exposure adjustment pass (0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::ExposureOp::apply`, line
// for line): per channel, clamp to [0, 1] (NaN reads 0), sRGB -> linear,
// `x = fma(linear, 2^exposure, offset)`, `y = sign(x) |x|^(1/gamma)`,
// clamp to [0, 1], and back to sRGB (a clipped highlight is exactly 1).
// `2^exposure` and `1/gamma` are computed on the CPU in f32 and ride in the
// uniform, so the GPU does not recompute them.
//
// **`pow` guards.** WGSL's `pow(x, y)` is `exp2(y * log2(x))` and leaves a
// negative base, or a zero base with a non-positive exponent, an
// *indeterminate value*. Every `pow` here has its base tested positive
// first: the sRGB decode's base `(m + 0.055) / 1.055` is above 0.0495 on the
// branch that uses it, the encode's base is above 0.0031308 on its branch,
// and the model's `|x|` is raised only when `x != 0` (exactly `sign(x) *
// |x|^p`: a zero `x` gives 0, as Rust's `0.0_f32.powf(p)` does for the
// positive exponent here). The exponent `1/gamma` is always positive.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct ExposureUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `ExposureOp::packed_for_gpu`'s layout:
    //   a = (identity, 2^exposure, offset, 1 / gamma)
    // `identity` = 1 is an exact passthrough (even out of gamut).
    a: vec4<f32>,
};

@group(0) @binding(4) var<uniform> ex_u: ExposureUniform;

// `aurora_color::srgb_to_linear` for an input in [0, 1].
fn ex_srgb_to_linear(encoded: f32) -> f32 {
    if (encoded <= 0.04045) {
        return encoded / 12.92;
    }
    return pow((encoded + 0.055) / 1.055, 2.4);
}

// `aurora_color::linear_to_srgb` for an input in [0, 1).
fn ex_linear_to_srgb(linear: f32) -> f32 {
    if (linear <= 0.0031308) {
        return linear * 12.92;
    }
    return 1.055 * pow(linear, 1.0 / 2.4) - 0.055;
}

// `ExposureOp::channel`.
fn ex_channel(encoded: f32) -> f32 {
    let x = fma(ex_srgb_to_linear(encoded), ex_u.a.y, ex_u.a.z);
    var y = 0.0;
    if (x > 0.0) {
        y = pow(x, ex_u.a.w);
    } else if (x < 0.0) {
        y = -pow(-x, ex_u.a.w);
    }
    y = clamp(y, 0.0, 1.0);
    if (y >= 1.0) {
        return 1.0;
    }
    return clamp(ex_linear_to_srgb(y), 0.0, 1.0);
}

// `ExposureOp::apply`.
fn ex_apply(c: vec3<f32>) -> vec3<f32> {
    if (ex_u.a.x != 0.0) {
        return c;
    }
    let v = clean_unit(c);
    return vec3<f32>(ex_channel(v.r), ex_channel(v.g), ex_channel(v.b));
}

@fragment
fn fs_composite_exposure(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(ex_apply(cb));
    let b = adjustment_blend(ex_u.mode, cb, cs);
    let a = ex_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
