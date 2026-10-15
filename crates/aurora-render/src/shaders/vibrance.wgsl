// The Vibrance adjustment pass (0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl`, `adjust_common.wgsl` and `hsl_common.wgsl` (`concat!`),
// so it can call the blend-math helpers, `straight_backdrop`,
// `adjustment_f16`, `adjustment_blend`, `clean_unit` and the `hs_*` HSL
// helpers defined there. Its own `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::VibranceOp::apply`, line
// for line): clamp to [0, 1] (NaN reads 0), RGB -> HSL, the vibrance rule
// (`s (1 + k (1 - s) g(hue))` for a positive slider with the raised-cosine
// skin guard `g`, `s (1 + k)` for a negative one), then the Hue/Saturation
// master saturation rule, then HSL -> RGB. Same four steps as the other
// adjustment passes (`Cb` rounded to `f16`; `Cs` = the map, `f16`;
// `lerp(Cb, B(Cb, Cs), opacity)`, `f16`; written back premultiplied at the
// backdrop's alpha).
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct VibranceUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `VibranceOp::packed_for_gpu`'s layout:
    //   a = (identity, vibrance / 100, saturation / 100, 0)
    // the sliders divided by 100 in f32 exactly as the CPU divides them;
    // `identity` = 1 is an exact passthrough (even out of gamut).
    a: vec4<f32>,
};

@group(0) @binding(4) var<uniform> vb_u: VibranceUniform;

// `skin_guard`: 1 outside 30 +- 30 degrees, else `1 - 0.5 * bump` with the
// raised cosine `bump`. `cos` is only evaluated for |d| < 30, i.e. an
// argument strictly inside (-pi, pi), where WGSL's `cos` is well defined
// and accurate; the early return keeps it away from any larger argument.
fn vb_skin_guard(hue: f32) -> f32 {
    let d = hue - 30.0;
    if (abs(d) >= 30.0) {
        return 1.0;
    }
    let bump = 0.5 * (1.0 + cos(3.14159274101257324 * d / 30.0));
    return fma(0.5, -bump, 1.0);
}

// `vibrance_saturation`.
fn vb_saturation(s: f32, amount: f32, hue: f32) -> f32 {
    if (amount > 0.0) {
        return s * (1.0 + amount * (1.0 - s) * vb_skin_guard(hue));
    }
    return s * (1.0 + amount);
}

// `VibranceOp::apply`.
fn vb_apply(c: vec3<f32>) -> vec3<f32> {
    if (vb_u.a.x != 0.0) {
        return c;
    }
    let hsl = hs_to_hsl(clean_unit(c));
    let boosted = vb_saturation(hsl.y, vb_u.a.y, hsl.x);
    let adjusted = hs_adjust_saturation(boosted, vb_u.a.z);
    return hs_from_hsl(hsl.x, clamp(adjusted, 0.0, 1.0), hsl.z);
}

@fragment
fn fs_composite_vibrance(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(vb_apply(cb));
    let b = adjustment_blend(vb_u.mode, cb, cs);
    let a = vb_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
