// HSL helpers shared by the Hue/Saturation, Vibrance and Black & White
// passes (moved out of `hue_saturation.wgsl` unchanged in 0.242.0 so that
// three passes have ONE copy).
//
// NOT a standalone module: `composite.rs` appends this file (after
// `composite.wgsl` and `adjust_common.wgsl`) to the pass that needs it.
// They port `aurora_filters::hue_saturation`'s `to_hsl`, `from_hsl`,
// `adjust_lightness` and `adjust_saturation` line for line. Every division
// is behind the branch that makes its divisor positive, and no `select()`
// is used where an arm would divide by zero: WGSL leaves float division by
// zero indeterminate.

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
