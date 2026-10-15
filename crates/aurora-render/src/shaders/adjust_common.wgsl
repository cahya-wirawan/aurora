// Helpers shared by the adjustment passes (Curves 0.159.0; Levels and
// Hue/Saturation 0.206.0; Vibrance, Exposure, Photo Filter, Black & White
// and Color Balance 0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and followed by one adjustment's own file
// (`curves.wgsl`, `levels.wgsl`, `hue_saturation.wgsl`), so it can call
// the blend-math helpers and `straight_backdrop` defined there. They were
// `curves_f16` and `curves_blend` in `curves.wgsl` until 0.206.0 moved
// them here unchanged (renamed only), so that the blend switch has ONE
// copy for every adjustment pass.
//
// Rounds to the nearest `f16` (`pack2x16float` leaves the rounding
// direction to the implementation: nearest or either neighbour, so it
// matches the CPU's round-to-nearest-even to within one `f16` step). The input is
// clamped to the finite `f16` range first: WGSL leaves `pack2x16float`'s
// result indeterminate outside it, and an out-of-gamut backdrop can push a
// blend term there (Overlay/HardLight of a `Cb` near 65504 reach ~1.3e5).
// The CPU's `half::f16::from_f32` would give infinity there instead; both
// are far outside anything the tolerance is about, and the clamp only
// makes the GPU's answer defined.
fn adjustment_f16(c_in: vec3<f32>) -> vec3<f32> {
    let c = clamp(c_in, vec3<f32>(-65504.0), vec3<f32>(65504.0));
    let rg = unpack2x16float(pack2x16float(c.rg));
    let b = unpack2x16float(pack2x16float(vec2<f32>(c.b, 0.0)));
    return vec3<f32>(rg.x, rg.y, b.x);
}

// The CPU ops' input rule (`|v| if v.is_nan() { 0.0 } else { v.clamp(0.0,
// 1.0) }`) for the five 0.239.0/0.240.0 adjustments: NaN and negatives read
// 0, above 1 reads 1. `!(v >= 0.0)` is true for NaN as well as negatives, so
// no comparison with NaN is relied on for anything else. (`min` after the
// zero test sees only a non-NaN operand, so WGSL's undefined `min` on NaN
// is never reached.)
fn clean_unit(c: vec3<f32>) -> vec3<f32> {
    var v = c;
    if (!(v.r >= 0.0)) { v.r = 0.0; }
    if (!(v.g >= 0.0)) { v.g = 0.0; }
    if (!(v.b >= 0.0)) { v.b = 0.0; }
    return min(v, vec3<f32>(1.0));
}

// Rec.709 luma, `fma(0.2126, r, fma(0.7152, g, 0.0722 * b))` exactly as the
// CPU's `luma` (Photo Filter and Color Balance).
fn adjust_luma(c: vec3<f32>) -> f32 {
    return fma(0.2126, c.r, fma(0.7152, c.g, 0.0722 * c.b));
}

// `blend_rgb(mode, cb, cs)` for the nineteen modes `aurora-render`'s
// `curves_mode_code` admits. Each arm calls the very helper that mode's own
// `fs_composite_*` entry point in `composite.wgsl` calls (0.159.0 review:
// one copy of every formula, so a fix to a mode cannot miss the Curves
// pass; since 0.206.0 the Levels and Hue/Saturation passes too). Codes are
// `curves_mode_code`'s, and
// `curves_switch_matches_curves_mode_code_and_all_blend_passes` checks
// that every arm calls `blend_<mode>` under its own code. An unknown code
// never reaches here (the Rust side refuses it) and falls back to Normal.
fn adjustment_blend(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 1u: { return blend_multiply(cb, cs); }
        case 2u: { return blend_darken(cb, cs); }
        case 3u: { return blend_lighten(cb, cs); }
        case 4u: { return blend_screen(cb, cs); }
        case 5u: { return blend_difference(cb, cs); }
        case 6u: { return blend_linear_dodge(cb, cs); }
        case 7u: { return blend_linear_burn(cb, cs); }
        case 8u: { return blend_color_burn(cb, cs); }
        case 9u: { return blend_color_dodge(cb, cs); }
        case 10u: { return blend_overlay(cb, cs); }
        case 11u: { return blend_hard_light(cb, cs); }
        case 12u: { return blend_linear_light(cb, cs); }
        case 13u: { return blend_vivid_light(cb, cs); }
        case 14u: { return blend_hard_mix(cb, cs); }
        case 15u: { return blend_pin_light(cb, cs); }
        case 16u: { return blend_soft_light(cb, cs); }
        case 17u: { return blend_subtract(cb, cs); }
        case 18u: { return blend_divide(cb, cs); }
        default: { return cs; }                                        // Normal (0)
    }
}

