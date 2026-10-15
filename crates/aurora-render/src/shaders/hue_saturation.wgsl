// The Hue/Saturation adjustment pass (0.206.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl`, `adjust_common.wgsl` and `hsl_common.wgsl` (`concat!`;
// the `hs_*` HSL helpers moved to the last in 0.242.0 so the Vibrance and
// Black & White passes share them), so it can call the blend-math helpers,
// `straight_backdrop`, `adjustment_f16` and `adjustment_blend` defined
// there. Its own `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora-app`'s `apply_adjustment_layer`
// with `aurora_filters::HueSaturationOp::apply`, the master/Colorize path:
// a layer with active colour ranges stays on the CPU). Same four steps as
// `curves.wgsl` (`Cb` rounded to `f16`; `Cs` = the map of `Cb`, `f16`;
// `lerp(Cb, B(Cb, Cs), opacity)`, `f16`; written back premultiplied at the
// backdrop's alpha), and the map is `HueSaturationOp::apply` line for line:
// clamp to [0, 1] (NaN reads 0), RGB -> HSL (`l = (max + min) / 2`), the
// hue rotation / saturation / lightness rules or Colorize, HSL -> RGB with
// each channel clamped to [0, 1].
//
// Every division is behind the branch that makes its divisor positive
// (`delta > 0`, `denominator > 0`, `1 - amount > 0`), and no `select()` is
// used where an arm would divide by zero: WGSL leaves float division by
// zero indeterminate.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct HueSaturationUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `HueSaturationOp::packed_for_gpu`'s layout:
    //   a = (identity, colorize, hue, saturation)
    //   b = (lightness, colorize_hue, colorize_saturation, colorize_lightness)
    // saturation and lightness are the sliders divided by 100, in f32,
    // exactly as the CPU computes them; hues are degrees. `identity` = 1
    // is an exact passthrough (even out of gamut), as the CPU's.
    a: vec4<f32>,
    b: vec4<f32>,
};

@group(0) @binding(4) var<uniform> hs_u: HueSaturationUniform;

// `HueSaturationOp::apply` for the master / Colorize path.
fn hs_apply(c: vec3<f32>) -> vec3<f32> {
    if (hs_u.a.x != 0.0) {
        return c;
    }
    // NaN and negatives read 0, above 1 reads 1 (`!(v >= 0)` is true for NaN).
    var clamped = c;
    if (!(clamped.r >= 0.0)) { clamped.r = 0.0; }
    if (!(clamped.g >= 0.0)) { clamped.g = 0.0; }
    if (!(clamped.b >= 0.0)) { clamped.b = 0.0; }
    clamped = min(clamped, vec3<f32>(1.0));
    let hsl = hs_to_hsl(clamped);
    if (hs_u.a.y != 0.0) {
        return hs_from_hsl(
            hs_u.b.y,
            hs_u.b.z,
            hs_adjust_lightness(hsl.z, hs_u.b.w),
        );
    }
    return hs_from_hsl(
        hsl.x + hs_u.a.z,
        hs_adjust_saturation(hsl.y, hs_u.a.w),
        hs_adjust_lightness(hsl.z, hs_u.b.x),
    );
}

@fragment
fn fs_composite_hue_saturation(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(hs_apply(cb));
    let b = adjustment_blend(hs_u.mode, cb, cs);
    let a = hs_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
