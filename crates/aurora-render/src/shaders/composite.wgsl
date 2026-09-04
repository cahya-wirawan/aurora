// Composites one tile-sized texture over another via straight-alpha
// "source-over" blending. The fragment shader just outputs the source
// texel unchanged -- the actual blend math is the GPU's fixed-function
// blend unit (aurora_gpu::Blend::AlphaBlending), not a shader computation.
// This replaces the CPU per-pixel tile merge spike/FINDINGS.md finding #1
// measured at ~20ms and named as the real compositing bottleneck (not
// disk I/O, which the same spike found fast).

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle; no vertex buffer -- same trick as canvas.wgsl in
// aurora-gpu.
@vertex
fn vs_composite(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_smp, in.uv);
}

// The opacity-aware sibling of fs_composite above, for
// TileCompositor::composite_over_with_opacity (aurora-render
// src/composite.rs): the same fixed-function AlphaBlending blend state
// does the "over" math, but src's own alpha channel is scaled by a real
// uniform first -- the GPU counterpart of composite_tile_cpu's own
// `alpha = src_alpha * opacity` step, needed because the fixed-function
// blend unit itself takes no uniform input of its own. Padded to 16
// bytes total (only .value is ever read) purely for defensive cross-
// backend uniform-buffer-size alignment, the same padding shape
// aurora-widgets' src/render.rs's own path.wgsl uniform already uses for
// an analogous small scalar-plus-padding payload -- three plain `f32`
// padding fields, not a `vec3<f32>`: WGSL's uniform-address-space layout
// rules give `vec3<f32>` its own 16-byte alignment, which would push
// this struct's own real size to 32 bytes (4-byte `value` padded out to
// a 16-byte-aligned `vec3`, then rounded up again to the struct's own
// 16-byte alignment) instead of the intended 16 -- confirmed the hard
// way, by a real `wgpu` validation error ("bound with size 16 where the
// shader expects 32") the first time this used `vec3<f32>` padding
// instead.
struct Opacity {
    value: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(2) var<uniform> opacity: Opacity;

@fragment
fn fs_composite_opacity(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(src_tex, src_smp, in.uv);
    return vec4<f32>(color.rgb, color.a * opacity.value);
}

// The accumulator ("backdrop") texture, sampled as a *texture* rather
// than reached through the fixed-function blend unit. Binding 3 is the
// next free slot after src_tex (0), src_smp (1) and the opacity uniform
// (2) declared above; only the real blend-math entry points below --
// fs_composite_multiply, fs_composite_darken, fs_composite_lighten,
// fs_composite_screen, fs_composite_difference and
// fs_composite_linear_dodge -- use it, through the
// one bind group layout they share
// (`TileCompositor::bind_group_layout_blend`), so neither
// fixed-function entry point's own layout gains an entry.
//
// Named `backdrop_tex` after the Rust parameter it is actually bound to
// -- the `backdrop` of every `TileCompositor::composite_*_over_with_
// opacity` blend-math method (as of 0.105.0
// `composite_multiply_over_with_opacity` and its
// `darken`/`lighten`/`screen`/`difference`/`linear_dodge` siblings, named
// as a family rather than relisted, since one more joins them every time
// a mode is ported) --
// deliberately *not* `dst_tex`: `dst` on the Rust side is the render
// target this entry point writes to, which is a different texture
// entirely and must never alias this one. Calling the sampled backdrop
// `dst_tex` read as exactly the aliasing the Rust doc comment warns
// against, so the name was corrected in 0.83.1, while `Multiply` was
// still the only ported mode and the 25 then-unported ones had yet to be
// written against this file. That "25" is the 0.83.1 count and is not
// maintained here; `Darken` (0.85.0), `Lighten` (0.95.0), `Screen`
// (0.102.0), `Difference` (0.104.0) and `LinearDodge` (0.105.0) have
// since landed, and the live numbers live in `TileCompositor`'s own doc
// comment.
@group(0) @binding(3) var backdrop_tex: texture_2d<f32>;

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Multiply` only.
//
// **Why this exists at all.** Every other fragment entry point in this
// file leaves the actual "over" math to the GPU's fixed-function blend
// unit, which can express `Normal` and nothing else: it has no way to
// read the backdrop as a *colour* and run `Cb * Cs` on it. A real blend
// mode therefore has to compute the whole composite in the shader and
// write the finished result with `Blend::None` (a plain replace) — which
// means the accumulator has to arrive as a sampled texture
// (`backdrop_tex`), not as the render target. So this entry point writes
// to a *different*
// view than the one it samples; it cannot accumulate in place the way
// fs_composite/fs_composite_opacity do.
//
// **The formula, step for step against the CPU reference.**
// `composite_layer_into`'s loop body is, per texel:
//
//     alpha             = sa * opacity            (opacity pre-clamped)
//     inverse           = 1 - alpha
//     backdrop_alpha    = da
//     backdrop_inverse  = 1 - backdrop_alpha
//     straight_backdrop = d.rgb / backdrop_alpha, or [0,0,0] if da == 0
//     b                 = blend_rgb(mode, straight_backdrop, s.rgb)
//     blended           = backdrop_inverse * s.rgb + backdrop_alpha * b
//     out.rgb           = inverse * d.rgb + alpha * blended
//     out.a             = alpha + da * inverse
//
// and `blend_rgb(Multiply, cb, cs)` is `blend_channel`'s `cb * cs` per
// channel. Each line below is that line, in the same order.
//
// `backdrop_tex` holds the *premultiplied* accumulator, exactly as the CPU
// accumulator does; its straight colour is recovered by dividing by its
// own alpha before blending — the `if (ab > 0.0)` guard is
// `composite_layer_into`'s own `backdrop_alpha > 0.0` guard, and the
// zero fallback is its `[0.0, 0.0, 0.0]`. The result is written back
// premultiplied.
//
// `opacity.value` is not re-clamped here because the Rust caller
// (`TileCompositor::composite_multiply_over_with_opacity`) already
// clamps it to `0.0..=1.0` before uploading it, exactly as
// `composite_over_with_opacity` does — and `composite_layer_into`
// itself clamps the *opacity*, not the `sa * opacity` product, so
// clamping the product here would be a real (if narrow) divergence from
// the reference for a source alpha above 1.0, which an `f16` tile can
// legitimately hold.
@fragment
fn fs_composite_multiply(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = cb * s.rgb;                       // blend_rgb(Multiply, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Darken` only -- the second blend mode ported
// to the GPU, and structurally identical to `fs_composite_multiply`
// above in every line but one.
//
// Read that entry point's own comment for the full derivation of the
// surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(Darken, cb, cs)` is `blend_channel`'s `cb.min(cs)` applied
// per channel, through `blend_rgb`'s own generic per-channel arm (it is
// a separable mode, not one of the six whole-triple ones). WGSL's
// `min()` on a `vec3<f32>` is componentwise, so one intrinsic is
// exactly those three independent per-channel minima -- not a
// whole-colour selection, which is `DarkerColor`, a different mode.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with
// `fs_composite_multiply`; no new binding, no new layout.
@fragment
fn fs_composite_darken(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = min(cb, s.rgb);                   // blend_rgb(Darken, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Lighten` only -- the third blend mode ported
// to the GPU, and the exact mirror image of `fs_composite_darken`
// above: one intrinsic differs, `max()` where that one has `min()`.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(Lighten, cb, cs)` is `blend_channel`'s `cb.max(cs)` applied
// per channel, through `blend_rgb`'s own generic per-channel arm (a
// separable mode, not one of the six whole-triple ones). WGSL's `max()`
// on a `vec3<f32>` is componentwise, so one intrinsic is exactly those
// three independent per-channel maxima -- not a whole-colour selection,
// which is `LighterColor`, a different, still-CPU-only mode.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with the two entry
// points above; no new binding, no new layout.
@fragment
fn fs_composite_lighten(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = max(cb, s.rgb);                   // blend_rgb(Lighten, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Screen` only -- the fourth blend mode ported
// to the GPU, and structurally identical to the three entry points above
// in every line but one.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(Screen, cb, cs)` is `blend_channel`'s `cb + cs - cb * cs`
// applied per channel, through `blend_rgb`'s own generic per-channel arm
// (a separable mode, not one of the six whole-triple ones). It is the
// first ported mode whose formula is real *arithmetic* on both operands
// rather than a single intrinsic: `Multiply` is one `*`, `Darken` one
// `min()`, `Lighten` one `max()`.
//
// **Written as the literal sum, not as `1.0 - (1.0 - cb) * (1.0 - s.rgb)`.**
// The two are algebraically equal and the inverse-multiply form is the
// more familiar statement of what `Screen` *means*, but `blend_channel`'s
// own Rust arm is `cb + cs - cb * cs`, and every entry point in this file
// is checked by reading it against that function line for line. A form
// that has to be re-derived before it can be compared is a worse
// comment than a form that matches. (They are not bit-identical in
// floating point either, which would put the `assert_eq!`-based
// differentials in this crate's test module at the mercy of which one
// was written.)
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with the three entry
// points above; no new binding, no new layout.
@fragment
fn fs_composite_screen(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = cb + s.rgb - cb * s.rgb;          // blend_rgb(Screen, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Difference` only -- the fifth blend mode
// ported to the GPU, and structurally identical to the four entry points
// above in every line but one.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(Difference, cb, cs)` is `blend_channel`'s `(cb - cs).abs()`
// applied per channel, through `blend_rgb`'s own generic per-channel arm
// (a separable mode, not one of the six whole-triple ones). WGSL's
// `abs()` on a `vec3<f32>` is componentwise, so one intrinsic is exactly
// those three independent per-channel absolute differences.
//
// **`abs()` on the difference, not `max(cb - s.rgb, 0.0)`.** Those two
// agree wherever `Cb >= Cs` and disagree everywhere else, and the second
// is `Subtract`, a different, still-CPU-only mode. The fixtures in this
// crate's `composite_difference_*` tests separate the two deliberately:
// each has at least one channel where `Cb < Cs`, so a `max(..., 0)` in
// place of the `abs()` here fails them rather than passing by accident.
//
// **Symmetry, disclosed rather than assumed.** `|Cb - Cs| = |Cs - Cb|`,
// so this mode's blend term is symmetric in backdrop and source and a
// transposed src/backdrop binding is **not** caught by the blend term
// alone -- exactly the property `fs_composite_screen` above discloses for
// the same reason (`Screen` is likewise commutative). What does catch a
// transpose is the surrounding "over", which is not symmetric, and the
// per-texel spatial differential in
// `composite_difference_over_with_opacity_matches_the_cpu_across_a_
// spatially_varying_tile`.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with the four entry
// points above; no new binding, no new layout.
@fragment
fn fs_composite_difference(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = abs(cb - s.rgb);                  // blend_rgb(Difference, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::LinearDodge` only -- the sixth blend mode
// ported to the GPU, and structurally identical to the five entry points
// above in every line but one.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(LinearDodge, cb, cs)` is `blend_channel`'s
// `(cb + cs).min(1.0)` applied per channel, through `blend_rgb`'s own
// generic per-channel arm (a separable mode, not one of the six
// whole-triple ones). WGSL's `min()` on two `vec3<f32>`s is
// componentwise, so `min(cb + s.rgb, vec3<f32>(1.0))` is exactly those
// three independent per-channel clamped sums. The `vec3<f32>(1.0)`
// splat rather than a bare `1.0` is not cosmetic: WGSL's `min` requires
// both operands to have the same type, so the scalar form does not
// type-check here.
//
// **The clamp is part of the mode, not a defensive guard.** `Cb + Cs`
// is unbounded above, and `LinearDodge` is *defined* as the clamped sum
// -- Photoshop's "Add". Dropping the `min` would not merely widen a
// range; it would compute a different function everywhere the sum
// exceeds `1.0`, which is most of this mode's interesting domain. This
// is the one ported entry point so far whose formula has an operation
// that exists purely to bound the result.
//
// **Three things this is not, all near enough to be real copy-paste
// risks:**
//
//   - **`max(cb + s.rgb - 1.0, 0.0)` is `LinearBurn`**, a different,
//     still-CPU-only mode -- the exact mirror image of this one, same
//     sum, opposite offset and opposite clamp direction. Written out
//     side by side the two differ by three characters, which is why it
//     is named here rather than left to be noticed.
//   - **`cb + s.rgb - cb * s.rgb` is `Screen`** (fs_composite_screen
//     above), this mode's nearest arithmetic neighbour: the same sum
//     with a correction term instead of a clamp. The two agree only
//     where `cb * cs == 0` or where the clamp bites at exactly `1.0`.
//   - **`ColorDodge` is the other "dodge"** (`cb / (1 - cs)`), likewise
//     still CPU-only. Sharing half a name is the whole risk there;
//     nothing about the arithmetic is close.
//
// **Three degeneracies, disclosed because they constrain every fixture
// in this crate's `composite_linear_dodge_*` tests:**
//
//   1. `LinearDodge(0, Cs) = Cs` -- identical to `Normal`, and to
//      `Screen`, wherever the backdrop channel is zero.
//   2. `LinearDodge(1, Cs) = 1` for every `Cs` -- a saturated backdrop
//      channel erases the source entirely.
//   3. A channel whose sum exceeds `1.0` is *clamped*, so it carries no
//      information about how far past the boundary the operands were:
//      `(0.5, 0.75)` and `(0.9, 0.9)` both give `1.0`. That is the mode
//      working correctly, but it means a clamped channel cannot
//      discriminate the operands, only the clamp.
//
// **Symmetry, disclosed rather than assumed.** `Cb + Cs = Cs + Cb`, so
// this mode's blend term is symmetric in backdrop and source and a
// transposed src/backdrop binding is **not** caught by the blend term
// alone -- exactly the property `fs_composite_screen` and
// `fs_composite_difference` above disclose for the same reason. What
// catches a transpose is the surrounding "over", which is not
// symmetric, and the per-texel spatial differential in
// `composite_linear_dodge_over_with_opacity_matches_the_cpu_across_a_
// spatially_varying_tile`.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with the five entry
// points above; no new binding, no new layout.
@fragment
fn fs_composite_linear_dodge(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    let b = min(cb + s.rgb, vec3<f32>(1.0)); // blend_rgb(LinearDodge, cb, cs)
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}
