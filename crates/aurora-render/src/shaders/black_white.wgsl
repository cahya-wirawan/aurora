// The Black & White adjustment pass (0.242.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl`, `adjust_common.wgsl` and `hsl_common.wgsl` (`concat!`),
// so Tint can use the shared `hs_from_hsl`. Its own `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::BlackWhiteOp::apply`,
// line for line): clamp to [0, 1] (NaN reads 0); sort the channels
// descending with the same three-step network (strict `<`, so equal
// channels keep index order red, green, blue) carrying each value's channel
// index; the primary slider is the largest channel's own (red 0, green 2,
// blue 4), the secondary the slider of the pair of the two largest
// (red+green yellow 1, green+blue cyan 3, red+blue magenta 5), and
// `grey = clamp(fma(first - second, primary, third) + (second - third) *
// secondary, 0, 1)`. With Tint the grey becomes the HSL colour of the tint
// hue and saturation at that lightness.
//
// No division, no `pow`, no `sqrt`: every operation is defined for every
// finite input, so there is nothing to guard beyond the NaN read. The
// slider lookup is a `switch` over a clamped slot (the slot is always 0..5
// by construction; the `default` arm is the CPU's `unwrap_or(0.0)` and is
// unreachable).
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct BlackWhiteUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `BlackWhiteOp::packed_for_gpu`'s layout (the sliders divided by 100
    // in f32 exactly as the CPU divides them):
    //   a = (reds, yellows, greens, cyans)
    //   b = (blues, magentas, tint, tint_hue)
    //   c = (tint_saturation / 100, 0, 0, 0)
    // `tint` is 0 / 1; hue is degrees. There is no identity flag: a Black &
    // White layer is never an identity.
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
};

@group(0) @binding(4) var<uniform> bw_u: BlackWhiteUniform;

// The slider at `slot`: reds, yellows, greens, cyans, blues, magentas.
fn bw_weight(slot: u32) -> f32 {
    switch slot {
        case 0u: { return bw_u.a.x; }
        case 1u: { return bw_u.a.y; }
        case 2u: { return bw_u.a.z; }
        case 3u: { return bw_u.a.w; }
        case 4u: { return bw_u.b.x; }
        case 5u: { return bw_u.b.y; }
        default: { return 0.0; }
    }
}

// `BlackWhiteOp::grey`.
fn bw_grey(c: vec3<f32>) -> f32 {
    let v = clean_unit(c);
    var v0 = v.r;
    var v1 = v.g;
    var v2 = v.b;
    var i0 = 0u;
    var i1 = 1u;
    var i2 = 2u;
    if (v0 < v1) {
        let tv = v0; v0 = v1; v1 = tv;
        let ti = i0; i0 = i1; i1 = ti;
    }
    if (v1 < v2) {
        let tv = v1; v1 = v2; v2 = tv;
        let ti = i1; i1 = i2; i2 = ti;
    }
    if (v0 < v1) {
        let tv = v0; v0 = v1; v1 = tv;
        let ti = i0; i0 = i1; i1 = ti;
    }
    // `secondary_slot(a, b)`: a + b = 1 -> 1, 3 -> 3, else 5.
    var secondary_slot = 5u;
    if (i0 + i1 == 1u) {
        secondary_slot = 1u;
    } else if (i0 + i1 == 3u) {
        secondary_slot = 3u;
    }
    let primary = bw_weight(i0 * 2u);
    let secondary = bw_weight(secondary_slot);
    let grey = fma(v0 - v1, primary, v2) + (v1 - v2) * secondary;
    return clamp(grey, 0.0, 1.0);
}

// `BlackWhiteOp::apply`.
fn bw_apply(c: vec3<f32>) -> vec3<f32> {
    let grey = bw_grey(c);
    if (bw_u.b.z != 0.0) {
        return hs_from_hsl(bw_u.b.w, bw_u.c.x, grey);
    }
    return vec3<f32>(grey);
}

@fragment
fn fs_composite_black_white(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(bw_apply(cb));
    let b = adjustment_blend(bw_u.mode, cb, cs);
    let a = bw_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
