// The Selective Color adjustment pass (0.250.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`). Its own
// `wgpu::ShaderModule`.
//
// The CPU path is the reference (`aurora_filters::SelectiveColorOp::apply`
// and `range_weights`, line for line; UNVALIDATED against Photoshop, see
// that module's docs). Clamp to [0, 1] (NaN reads 0); the nine range
// weights from the largest / middle / smallest channels and the HSL-style
// lightness `max + min - 1`; the slider sums `s = sum(weight * slider)`
// (`fma` per range, as the CPU); the CMY complements `c m y`, `k = min(c,
// m, y)`; Relative scales each ink by itself (`c * s_c`, `k * s_k`),
// Absolute adds the slider; a channel whose own change and the black change
// are both exactly zero is returned as the clamped input, else it becomes
// `1 - clamp(ink + delta + d_k, 0, 1)`.
//
// The CPU skips ranges whose weight is exactly zero; this port does not,
// because `fma(0, slider, s) = s` for the finite sliders the parameters
// validate (and the sums start at +0), so the skip never changes a result.
// There is no identity flag for the same reason: all-zero sliders take the
// `delta == 0 && d_k == 0` path on every channel and return the clamped
// input exactly. No division, no `pow`: nothing here is undefined.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct SelectiveColorUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `SelectiveColorOp::packed_for_gpu`'s layout:
    //   a = (absolute, 0, 0, 0)
    //   sliders[i] = (cyan, magenta, yellow, black) of range i, as
    //   fractions; ranges: Reds, Yellows, Greens, Cyans, Blues, Magentas,
    //   Whites, Neutrals, Blacks.
    a: vec4<f32>,
    sliders: array<vec4<f32>, 9>,
};

@group(0) @binding(4) var<uniform> sc_u: SelectiveColorUniform;

// `selective_color::range_weights` for a pixel already in [0, 1].
fn sc_range_weights(p: vec3<f32>) -> array<f32, 9> {
    var v = p;
    // The index of the largest channel and of the middle one, with the CPU's
    // tie-breaking (`>=` keeps the earlier channel).
    var hi = 0u;
    var mid = 0u;
    if (v.r >= v.g) {
        if (v.g >= v.b) {
            hi = 0u; mid = 1u;
        } else if (v.r >= v.b) {
            hi = 0u; mid = 2u;
        } else {
            hi = 2u; mid = 0u;
        }
    } else if (v.r >= v.b) {
        hi = 1u; mid = 0u;
    } else if (v.g >= v.b) {
        hi = 1u; mid = 2u;
    } else {
        hi = 2u; mid = 1u;
    }
    let max_v = v[hi];
    let mid_v = v[mid];
    let min_v = min(min(v.r, v.g), v.b);
    let chroma = max_v - min_v;
    var primary = 4u;
    if (hi == 0u) {
        primary = 0u;
    } else if (hi == 1u) {
        primary = 2u;
    }
    var secondary = 5u;
    if ((hi == 0u && mid == 1u) || (hi == 1u && mid == 0u)) {
        secondary = 1u;
    } else if ((hi == 1u && mid == 2u) || (hi == 2u && mid == 1u)) {
        secondary = 3u;
    }
    var w = array<f32, 9>();
    w[primary] = w[primary] + (max_v - mid_v);
    w[secondary] = w[secondary] + (mid_v - min_v);
    let achromatic = 1.0 - chroma;
    let tone = max_v + min_v - 1.0;
    w[6] = achromatic * max(tone, 0.0);
    w[7] = achromatic * (1.0 - abs(tone));
    w[8] = achromatic * max(-tone, 0.0);
    return w;
}

// `SelectiveColorOp::apply`.
fn sc_apply(c: vec3<f32>) -> vec3<f32> {
    let p = clean_unit(c);
    var w = sc_range_weights(p);
    var s = vec4<f32>(0.0);
    for (var i = 0u; i < 9u; i = i + 1u) {
        s = fma(vec4<f32>(w[i]), sc_u.sliders[i], s);
    }
    let inks = vec3<f32>(1.0) - p;
    let k = min(min(inks.x, inks.y), inks.z);
    var delta = vec4<f32>(inks * s.xyz, k * s.w);
    if (sc_u.a.x != 0.0) {
        delta = s;
    }
    var out = p;
    if (!(delta.x == 0.0 && delta.w == 0.0)) {
        out.r = 1.0 - clamp(inks.x + delta.x + delta.w, 0.0, 1.0);
    }
    if (!(delta.y == 0.0 && delta.w == 0.0)) {
        out.g = 1.0 - clamp(inks.y + delta.y + delta.w, 0.0, 1.0);
    }
    if (!(delta.z == 0.0 && delta.w == 0.0)) {
        out.b = 1.0 - clamp(inks.z + delta.z + delta.w, 0.0, 1.0);
    }
    return out;
}

@fragment
fn fs_composite_selective_color(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(sc_apply(cb));
    let b = adjustment_blend(sc_u.mode, cb, cs);
    let a = sc_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
