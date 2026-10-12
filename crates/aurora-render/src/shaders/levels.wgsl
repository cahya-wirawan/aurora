// The Levels adjustment pass (0.206.0).
//
// NOT a standalone module: `composite.rs` compiles this file appended to
// `composite.wgsl` and `adjust_common.wgsl` (`concat!`), so it can call the
// blend-math helpers, `straight_backdrop`, `adjustment_f16` and
// `adjustment_blend` defined there. Its own `wgpu::ShaderModule`, so
// `composite.wgsl`'s entry-point roster is untouched.
//
// The CPU path is the reference (`aurora-app`'s `apply_adjustment_layer`
// with `aurora_filters::LevelsLut`), step for step, exactly as
// `curves.wgsl` follows it: `Cb` = the accumulator un-premultiplied and
// rounded to `f16`; `Cs` = the Levels map of `Cb`, rounded to `f16`;
// `mixed = lerp(Cb, B(Cb, Cs), opacity)` rounded to `f16`; written back
// premultiplied at the backdrop's own alpha.
//
// **Direct math, no table.** `LevelsLut` tabulates only the gamma power
// `t^(1/gamma)` (2^14 intervals, the first 16 exact) because the CPU wants
// a cheap per-pixel lookup; the affine input and output stages are exact
// arithmetic. The GPU evaluates `pow` itself per pixel, so there is no
// storage buffer and no per-parameter-set cache: the four records ride in
// the pass's uniform. The CPU's table is within 2.1e-5 of the exact power
// (`aurora-filters`' module docs); the GPU's `pow` is within a few ULP of
// it, far below the `2 * f16::EPSILON` tolerance class.
//
// Bindings: 1 (the shared sampler) and 3 (`backdrop_tex`) are
// `composite.wgsl`'s; 4 is this pass's own.

struct LevelsUniform {
    opacity: f32,
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    // `aurora_filters::LevelsLut::packed_for_gpu`'s layout: four records
    // (red, green, blue, master), two `vec4`s each:
    //   [2r]     = (present, input_black, input_white, 1 / gamma)
    //   [2r + 1] = (output_black, output_white, has_gamma, 0)
    // `present` is 0 for an identity record (an exact passthrough, as
    // `LevelsLut` skips it); `has_gamma` is 0 when gamma is exactly 1.
    records: array<vec4<f32>, 8>,
};

@group(0) @binding(4) var<uniform> levels_u: LevelsUniform;

// `Channel::lookup` of record `r`: `LevelsChannel::normalised_input`
// (clamped affine, NaN reads 0), the gamma power, `output_of` (fma, then
// clamp to [0, 1]).
fn levels_record(r: u32, value: f32) -> f32 {
    let head = levels_u.records[2u * r];
    let tail = levels_u.records[2u * r + 1u];
    if (head.x == 0.0) {
        return value;
    }
    var x = value;
    // `!(x >= 0)` is true for NaN as well as negatives: both read 0.
    if (!(x >= 0.0)) {
        x = 0.0;
    }
    // The span is validated positive (`MIN_INPUT_SPAN`); the branch keeps
    // the division from ever seeing a zero divisor.
    let span = head.z - head.y;
    var t = 0.0;
    if (span > 0.0) {
        t = clamp((x - head.y) / span, 0.0, 1.0);
    } else if (x >= head.z) {
        t = 1.0;
    }
    var g = t;
    if (tail.z != 0.0) {
        // `pow(0, p)` is defined for p > 0 but is guarded anyway; the
        // result is 0 there, as `0^p` is.
        if (t > 0.0) {
            g = pow(t, head.w);
        } else {
            g = 0.0;
        }
    }
    return clamp(fma(g, tail.y - tail.x, tail.x), 0.0, 1.0);
}

// `LevelsLut::map_channel` for each channel: its own record first, then the
// master (record 3).
fn levels_apply(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        levels_record(3u, levels_record(0u, c.r)),
        levels_record(3u, levels_record(1u, c.g)),
        levels_record(3u, levels_record(2u, c.b)),
    );
}

@fragment
fn fs_composite_levels(in: VsOut) -> @location(0) vec4<f32> {
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = adjustment_f16(clamp(straight_backdrop(bd), vec3<f32>(-65504.0), vec3<f32>(65504.0)));
    let cs = adjustment_f16(levels_apply(cb));
    let b = adjustment_blend(levels_u.mode, cb, cs);
    let a = levels_u.opacity;
    let mixed = adjustment_f16((1.0 - a) * cb + a * b);
    return vec4<f32>(mixed * bd.a, bd.a);
}
