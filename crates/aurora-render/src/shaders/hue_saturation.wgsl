// The Hue/Saturation adjustment pass (0.206.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`), so it can call the
// blend-math helpers, `straight_backdrop`, `adjustment_f16` and
// `adjustment_blend` defined there. Its own `wgpu::ShaderModule`.
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

// `f32::rem_euclid`: truncated remainder, then `+ m` when negative.
fn hs_rem_euclid(a: f32, m: f32) -> f32 {
    let r = a % m;
    if (r < 0.0) {
        return r + m;
    }
    return r;
}

// `to_hsl`: (hue in degrees [0, 360), saturation, lightness).
fn hs_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let hi = max(max(c.r, c.g), c.b);
    let lo = min(min(c.r, c.g), c.b);
    let lightness = (hi + lo) * 0.5;
    let delta = hi - lo;
    if (delta <= 0.0) {
        return vec3<f32>(0.0, 0.0, lightness);
    }
    var denominator = 2.0 - hi - lo;
    if (lightness <= 0.5) {
        denominator = hi + lo;
    }
    var saturation = 0.0;
    if (denominator > 0.0) {
        saturation = clamp(delta / denominator, 0.0, 1.0);
    }
    var sector = 0.0;
    if (hi == c.r) {
        sector = hs_rem_euclid((c.g - c.b) / delta, 6.0);
    } else if (hi == c.g) {
        sector = (c.b - c.r) / delta + 2.0;
    } else {
        sector = (c.r - c.g) / delta + 4.0;
    }
    return vec3<f32>(hs_rem_euclid(sector * 60.0, 360.0), saturation, lightness);
}

// `from_hsl`: each channel clamped to [0, 1].
fn hs_from_hsl(hue: f32, saturation: f32, lightness: f32) -> vec3<f32> {
    let chroma = (1.0 - abs(2.0 * lightness - 1.0)) * saturation;
    let sector = hs_rem_euclid(hue, 360.0) / 60.0;
    let second = chroma * (1.0 - abs(hs_rem_euclid(sector, 2.0) - 1.0));
    var rgb = vec3<f32>(chroma, 0.0, second);
    if (sector < 1.0) {
        rgb = vec3<f32>(chroma, second, 0.0);
    } else if (sector < 2.0) {
        rgb = vec3<f32>(second, chroma, 0.0);
    } else if (sector < 3.0) {
        rgb = vec3<f32>(0.0, chroma, second);
    } else if (sector < 4.0) {
        rgb = vec3<f32>(0.0, second, chroma);
    } else if (sector < 5.0) {
        rgb = vec3<f32>(second, 0.0, chroma);
    }
    let offset = lightness - chroma * 0.5;
    return clamp(rgb + vec3<f32>(offset), vec3<f32>(0.0), vec3<f32>(1.0));
}

// `adjust_lightness`: blend towards black (negative) or white.
fn hs_adjust_lightness(lightness: f32, amount: f32) -> f32 {
    if (amount < 0.0) {
        return lightness * (1.0 + amount);
    }
    return fma(1.0 - lightness, amount, lightness);
}

// `adjust_saturation`: `s (1 + k)` down, `min(1, s / (1 - k))` up, and at
// `k >= 1` (no division) 1 for a coloured pixel, 0 for a grey one.
fn hs_adjust_saturation(saturation: f32, amount: f32) -> f32 {
    if (amount <= 0.0) {
        return saturation * (1.0 + amount);
    }
    if (amount >= 1.0) {
        if (saturation > 0.0) {
            return 1.0;
        }
        return 0.0;
    }
    return min(saturation / (1.0 - amount), 1.0);
}

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
