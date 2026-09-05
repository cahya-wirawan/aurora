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
// fs_composite_screen, fs_composite_difference,
// fs_composite_linear_dodge, fs_composite_linear_burn,
// fs_composite_color_burn and fs_composite_color_dodge -- use it,
// through the
// one bind group layout they share
// (`TileCompositor::bind_group_layout_blend`), so neither
// fixed-function entry point's own layout gains an entry.
//
// Named `backdrop_tex` after the Rust parameter it is actually bound to
// -- the `backdrop` of every `TileCompositor::composite_*_over_with_
// opacity` blend-math method (as of 0.108.0
// `composite_multiply_over_with_opacity` and its
// `darken`/`lighten`/`screen`/`difference`/`linear_dodge`/`linear_burn`/
// `color_burn`/`color_dodge`
// siblings, named
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
// (0.102.0), `Difference` (0.104.0), `LinearDodge` (0.105.0),
// `LinearBurn` (0.106.0), `ColorBurn` (0.107.0) and `ColorDodge`
// (0.108.0) have
// since landed, and the live numbers live in `TileCompositor`'s own doc
// comment.
@group(0) @binding(3) var backdrop_tex: texture_2d<f32>;

// The two halves of the blend-math "over" that every blend-math entry
// point below shares verbatim, extracted in 0.109.0. Each statement in
// both bodies is byte-identical to the line it replaced in those entry
// points -- moved, not retyped -- which is what makes "bit-for-bit
// identical output" a property provable by text diff rather than by
// trusting arithmetic reasoning about equivalent rewrites. Do not
// "clean up" either body: `a + bd.a * inv` is deliberately not
// `a + ab * inv` (numerically identical, textually not), the `if`
// guard is deliberately not a `select()` (which would evaluate both
// arms and make the `0.0 / 0.0` divide unconditional, defeating the
// nine `..._is_the_source_alone_where_the_backdrop_is_transparent`
// tests), and no expression may be reordered, since float addition is
// not associative and the CPU differential tests assert exact equality.

// `composite_layer_into`'s `straight_backdrop = d.rgb / backdrop_alpha,
// or [0,0,0] if da == 0` -- the premultiplied accumulator's straight
// colour. The `if (ab > 0.0)` guard is that function's own
// `backdrop_alpha > 0.0` guard and the zero fallback is its
// `[0.0, 0.0, 0.0]`; as of 0.109.0 the guard lives here rather than
// once per entry point.
fn straight_backdrop(bd: vec4<f32>) -> vec3<f32> {
    let ab = bd.a;
    var cb = vec3<f32>(0.0, 0.0, 0.0);
    if (ab > 0.0) {
        cb = bd.rgb / ab;
    }
    return cb;
}

// The blend-mode-independent "over" fold around an already-computed
// `b = blend_rgb(mode, cb, cs)`: `composite_layer_into`'s `alpha`,
// `inverse`, `backdrop_alpha`, `backdrop_inverse`, `blended`, `out.rgb`
// and `out.a` lines, in that order. The result is premultiplied, as the
// CPU accumulator is.
fn fold_over(s: vec4<f32>, bd: vec4<f32>, b: vec3<f32>) -> vec4<f32> {
    let a = s.a * opacity.value;
    let inv = 1.0 - a;
    let ab = bd.a;
    let ab_inv = 1.0 - ab;
    let blended = ab_inv * s.rgb + ab * b;
    return vec4<f32>(inv * bd.rgb + a * blended, a + bd.a * inv);
}

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
// channel. As of 0.109.0 those lines are no longer all *below*: the
// `straight_backdrop` line lives in `straight_backdrop()` above, and the
// `alpha`/`inverse`/`backdrop_alpha`/`backdrop_inverse`/`blended`/
// `out.rgb`/`out.a` lines live in `fold_over()` above, both still in
// that same order. What remains below is the two `textureSample` lines,
// this mode's own `b = ...`, and the two calls.
//
// `backdrop_tex` holds the *premultiplied* accumulator, exactly as the CPU
// accumulator does; its straight colour is recovered by dividing by its
// own alpha before blending — the `if (ab > 0.0)` guard is
// `composite_layer_into`'s own `backdrop_alpha > 0.0` guard, and the
// zero fallback is its `[0.0, 0.0, 0.0]`. The result is written back
// premultiplied.
//
// As of 0.109.0 that guard belongs to `straight_backdrop()`, not to each
// entry point. The guard's existence and behaviour are unchanged, but the
// several test comments in `composite.rs` that quote
// `if (ab > 0.0) { cb = bd.rgb / ab; }` and attribute it to a *specific*
// entry point are now stale about its **location** only -- disclosed here,
// in one place, rather than by editing nine test comments to say the same
// thing.
//
// `opacity.value` is not re-clamped here because the Rust caller
// (`TileCompositor::composite_multiply_over_with_opacity`) already
// clamps it to `0.0..=1.0` before uploading it, exactly as
// `composite_over_with_opacity` does — and `composite_layer_into`
// itself clamps the *opacity*, not the `sa * opacity` product, so
// clamping the product here would be a real (if narrow) divergence from
// the reference for a source alpha above 1.0, which an `f16` tile can
// legitimately hold.
//
// **Deferred, then landed in 0.109.0** (the deferral was recorded in
// 0.106.1): the straighten-backdrop and "over" fold lines were
// byte-identical across every blend-math entry point in this file, each of
// which differed from this one in its `let b = ...` line and nothing else
// (the bodies were 13 lines each; 12 were shared verbatim, measured, not
// eyeballed). **Two exceptions since 0.108.0**, and they did not change
// the plan: in `fs_composite_color_burn` and `fs_composite_color_dodge`
// the `let b = ...` spans five lines rather than one, because each formula
// is three calls to a per-channel helper (`color_burn_channel`,
// `color_dodge_channel`) rather than one componentwise expression. Stated
// mode-agnostically so the next port does not have to re-sweep this note:
// *every guarded-division entry point's `let b = ...` spans five lines*,
// and that is the whole of the exception -- the *other* 12 lines were
// byte-identical in both, which is the property this note was about.
//
// `straight_backdrop()` and `fold_over()` above now hold **10 of those 12
// lines, not 12 of 12** -- the honest number. The two `textureSample`
// lines stay inline in every entry point because they need `in.uv`, which
// neither helper takes, and threading it through a third helper was
// rejected as buying nothing. Bodies went 13 lines -> 5 for the seven
// one-line-`b` modes and 17 -> 9 for the two guarded-division ones. This
// mirrors the Rust-side collapse 0.85.1 already made in `composite.rs`,
// and it landed as its own round with no mode port in it, so the
// refactor's diff is not entangled with a new formula's -- which is also
// why the next port does not get to fold a cleanup into itself.
@fragment
fn fs_composite_multiply(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    let b = cb * s.rgb;                       // blend_rgb(Multiply, cb, cs)
    return fold_over(s, bd, b);
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
    let cb = straight_backdrop(bd);
    let b = min(cb, s.rgb);                   // blend_rgb(Darken, cb, cs)
    return fold_over(s, bd, b);
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
    let cb = straight_backdrop(bd);
    let b = max(cb, s.rgb);                   // blend_rgb(Lighten, cb, cs)
    return fold_over(s, bd, b);
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
    let cb = straight_backdrop(bd);
    let b = cb + s.rgb - cb * s.rgb;          // blend_rgb(Screen, cb, cs)
    return fold_over(s, bd, b);
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
    let cb = straight_backdrop(bd);
    let b = abs(cb - s.rgb);                  // blend_rgb(Difference, cb, cs)
    return fold_over(s, bd, b);
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
//   - **`max(cb + s.rgb - 1.0, 0.0)` is `LinearBurn`**, a different
//     mode -- the exact mirror image of this one, same
//     sum, opposite offset and opposite clamp direction. Written out
//     side by side the two differ by three characters, which is why it
//     is named here rather than left to be noticed. As of 0.106.0 it is
//     `fs_composite_linear_burn` directly below, so the copy-paste
//     hazard now runs in both directions between two entry points that
//     both exist rather than one existing and one not.
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
    let cb = straight_backdrop(bd);
    let b = min(cb + s.rgb, vec3<f32>(1.0)); // blend_rgb(LinearDodge, cb, cs)
    return fold_over(s, bd, b);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::LinearBurn` only -- the seventh blend mode
// ported to the GPU, and structurally identical to the six entry points
// above in every line but one.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` line below differs.
//
// `blend_rgb(LinearBurn, cb, cs)` is `blend_channel`'s
// `(cb + cs - 1.0).max(0.0)` applied per channel, through `blend_rgb`'s
// own generic per-channel arm (a separable mode, not one of the six
// whole-triple ones). WGSL's `max()` on two `vec3<f32>`s is
// componentwise, so `max(cb + s.rgb - 1.0, vec3<f32>(0.0))` is exactly
// those three independent per-channel offset-and-clamped sums. The
// `vec3<f32>(0.0)` splat rather than a bare `0.0` is not cosmetic: WGSL's
// `max` requires both operands to have the same type, so the scalar form
// does not type-check here. The `- 1.0` scalar *does* type-check, because
// WGSL's `-` broadcasts an abstract-float scalar against a `vec3<f32>`;
// the asymmetry between the two lines is real and deliberate.
//
// **The clamp is part of the mode, not a defensive guard.** `Cb + Cs - 1`
// is unbounded below (it reaches `-1` at `Cb == Cs == 0`), and
// `LinearBurn` is *defined* as the clamped difference -- Photoshop's
// "Linear Burn". Dropping the `max` would not merely widen a range; it
// would compute a different function everywhere the sum falls under
// `1.0`, which is most of this mode's interesting domain, and would emit
// negative colour channels. This is the second ported entry point whose
// formula has an operation existing purely to bound the result (the first
// is `fs_composite_linear_dodge` above, which bounds the other end).
//
// **Three things this is not, all near enough to be real copy-paste
// risks:**
//
//   - **`min(cb + s.rgb, 1.0)` is `LinearDodge`** (fs_composite_linear_
//     dodge directly above), this mode's exact mirror image: same sum,
//     opposite offset and opposite clamp direction. Written out side by
//     side the two differ by three characters. It is **also on the GPU**
//     as of 0.105.0, so the hazard is now a live one in both directions
//     rather than a mode that does not yet exist here -- which is exactly
//     why this function's blend line was derived from `blend_channel`'s
//     Rust arm rather than copied from that one and edited.
//   - **`cb * s.rgb` is `Multiply`** (fs_composite_multiply above), this
//     mode's nearest arithmetic neighbour in *behaviour* rather than in
//     spelling: both darken, both give `0` for a zero backdrop, and the
//     two agree wherever `Cb + Cs - 1 == Cb * Cs`, i.e. where
//     `(1 - Cb) * (1 - Cs) == 0`.
//   - **`ColorBurn` is the other "burn"** (`1 - (1 - cb) / cs`), and as
//     of 0.107.0 it is **also on the GPU**, as
//     `fs_composite_color_burn` directly below. Sharing half a name is
//     the whole risk there; nothing about the arithmetic is close, and
//     the hazard lives in the two adjacent `aurora-app` dispatch arms
//     rather than in these two blend lines.
//
// **Six degeneracies, disclosed because they constrain every fixture in
// this crate's `composite_linear_burn_*` tests:**
//
//   1. `LinearBurn(0, Cs) = 0` for every `Cs <= 1` -- a zero backdrop
//      channel erases the source entirely.
//   2. `LinearBurn(1, Cs) = Cs` -- identical to `Normal`, and to
//      `Darken` and `Lighten` and `Screen`, wherever the backdrop channel
//      is saturated.
//   3. A channel whose sum falls under `1.0` is **clamped**, so its
//      output carries no information about how far below the boundary the
//      operands were: `(0.25, 0.5)` and `(0.1, 0.1)` both give `0.0`.
//      A clamped channel discriminates the clamp, not the operands.
//   4. `Cb == Cs` in a channel lets a transposed operand pair hide behind
//      an accidental equality, so the solid-colour fixtures avoid it.
//   5. **Specific to this mode, and the reason several otherwise-natural
//      fixtures were rejected:** in an *unclamped* channel,
//      `Cb + Cs - 1 == |Cb - Cs|` exactly when `Cb == 0.5` (if
//      `Cs > Cb`) or `Cs == 0.5` (if `Cb > Cs`) -- the algebra is
//      `Cb + Cs - 1 = Cs - Cb  <=>  Cb = 0.5`. So an unclamped channel
//      with either operand at exactly `0.5` cannot discriminate this mode
//      from `Difference`, which is also on the GPU. No unclamped channel
//      in any solid-colour fixture here has an operand at `0.5`.
//   6. With both operands strictly inside `(0, 1)` -- which degeneracies
//      1 and 2 force -- the sum is strictly above `0.0`, so a deficit of
//      `1.0` is unreachable in principle; `0.625` is close to the
//      practical maximum at exact-binary-fraction magnitudes.
//
// **Symmetry, disclosed rather than assumed.** `Cb + Cs = Cs + Cb`, so
// this mode's blend term is symmetric in backdrop and source and a
// transposed src/backdrop binding is **not** caught by the blend term
// alone -- exactly the property `fs_composite_screen`,
// `fs_composite_difference` and `fs_composite_linear_dodge` above
// disclose for the same reason. What catches a transpose is the
// surrounding "over", which is not symmetric, and the per-texel spatial
// differential in
// `composite_linear_burn_over_with_opacity_matches_the_cpu_across_a_
// spatially_varying_tile`.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2)
// and `TileCompositor::bind_group_layout_blend` with the six entry
// points above; no new binding, no new layout.
@fragment
fn fs_composite_linear_burn(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    let b = max(cb + s.rgb - 1.0, vec3<f32>(0.0)); // blend_rgb(LinearBurn, cb, cs)
    return fold_over(s, bd, b);
}

// `blend_channel`'s own `BlendMode::ColorBurn` arm (src/composite.rs), one
// channel at a time -- this file's first non-entry-point function, and the
// first ported mode whose formula cannot be one componentwise expression
// on `vec3<f32>`: two of its three branches are per-channel *conditions*,
// not arithmetic, so a `vec3` form would need per-lane selects over a
// division that is undefined in the very lanes the conditions exist to
// exclude. Three calls to this is the honest shape.
//
// **The literal 0/1 comparisons are the W3C Compositing and Blending
// spec's own boundary conditions**, exactly as `blend_channel`'s Rust arm
// documents under its `#[allow(clippy::float_cmp)]`: `cb`/`cs` arrive from
// `f16` storage, where `0.0` and `1.0` round-trip bit-exact, and the spec
// requires these two literals rather than an epsilon band.
//
// **Branch order is load-bearing.** `cb == 1.0` is tested first, so the one
// input where both conditions hold -- a fully white backdrop under a fully
// black source, an ordinary pixel -- yields `1.0`, not `0.0`. Swapping the
// two is a real mutation, killed only by the saturated-backdrop test's
// green channel.
//
// **Both guards are arithmetically redundant under IEEE-754 and are still
// required.** Without the first, `(1 - 1)/cs = 0` and `1 - min(1, 0) = 1`
// -- the same answer for every `cs > 0`; it changes the result only at
// `cs == 0`, where the expression is `0/0`. Without the second,
// `(1 - cb)/0` is `+inf`, `min(1, inf)` is `1`, and the result is `0.0` --
// again the same answer, *if* the backend divides like IEEE. WGSL does not
// promise that: division by zero yields an indeterminate value, which may
// be `NaN`. And `NaN` here is not harmless, because the `ab == 0` half of a
// tile multiplies `b` by zero and `0.0 * NaN` is `NaN`. The guards are what
// make this entry point defined rather than merely correct on one adapter.
// Measured, not reasoned about: deleting the *first* guard is killed
// deterministically (the second then fires where the first should have, so
// a white backdrop under a zero source returns `0.0` instead of `1.0`),
// while deleting the *second* survives every test in this crate on
// Vulkan/NVIDIA, which is exactly the adapter-dependence this comment
// claims. See PLAN.md's 0.107.0 entry for both real results.
fn color_burn_channel(cb: f32, cs: f32) -> f32 {
    if (cb == 1.0) {
        return 1.0;
    }
    if (cs == 0.0) {
        return 0.0;
    }
    return 1.0 - min(1.0, (1.0 - cb) / cs);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::ColorBurn` only -- the eighth blend mode ported
// to the GPU. Structurally the seven entry points above, with one
// difference beyond the formula: its `b` is a three-call `vec3`
// construction rather than a single componentwise expression, for the
// reason `color_burn_channel` above gives.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent.
//
// **Deliberately not `min(1, cb / (1 - cs))`** -- that is `ColorDodge`,
// the other guarded-division mode, whose branch conditions are `cb == 0`
// and `cs == 1` rather than this one's `cb == 1` and `cs == 0`, and which
// has its own entry point (`fs_composite_color_dodge`, below) as of
// 0.108.0. (Until then this comment wrote that formula with a spurious
// outer `1 -`, i.e. as *this* mode's shape with `ColorDodge`'s operands.
// The distinction it drew was still the right one -- the branch
// conditions and the operand order -- but the formula it printed was
// wrong, and it was wrong identically at six sites; 0.108.0 corrected
// all six, though its own count said five -- it missed `aurora-app`'s
// `begin_gpu_composite_tile` `ColorBurn` dispatch-arm comment, which was
// fixed but not counted, and 0.108.1 corrected the count.) And not
// `max(cb + cs - 1, 0)`, which is `LinearBurn`
// (`fs_composite_linear_burn` directly above, on the GPU since 0.106.0):
// the two share half a name and nothing about the arithmetic is close, but
// the *dispatch arms* are adjacent, which is where that hazard actually
// lives.
//
// **Asymmetry, and this is the first ported mode that has it.**
// `B(Cb, Cs) != B(Cs, Cb)`, so unlike `Multiply`, `Darken`, `Lighten`,
// `Screen`, `Difference`, `LinearDodge` and `LinearBurn` -- every one of
// which discloses the opposite -- a transposed src/backdrop binding is
// caught by the blend term itself, not only by the asymmetric "over"
// around it.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the seven entry points
// above; no new binding, no new layout.
@fragment
fn fs_composite_color_burn(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    let b = vec3<f32>(
        color_burn_channel(cb.r, s.r),
        color_burn_channel(cb.g, s.g),
        color_burn_channel(cb.b, s.b),
    );
    return fold_over(s, bd, b);
}

// `blend_channel`'s own `BlendMode::ColorDodge` arm (src/composite.rs), one
// channel at a time -- this file's shaders' second non-entry-point
// function, and the exact structural sibling of `color_burn_channel`
// above: two of its three branches are per-channel *conditions* rather
// than arithmetic, so a componentwise `vec3` form would need per-lane
// selects over a division that is undefined in the very lanes the
// conditions exist to exclude. Three calls to this is the honest shape,
// for the same reason.
//
// **The literal 0/1 comparisons are the W3C Compositing and Blending
// spec's own boundary conditions**, exactly as `blend_channel`'s Rust arm
// documents under its `#[allow(clippy::float_cmp)]`: `cb`/`cs` arrive from
// `f16` storage, where `0.0` and `1.0` round-trip bit-exact, and the spec
// requires these two literals rather than an epsilon band.
//
// **Branch order is load-bearing**, and it is the *mirror* of
// `color_burn_channel`'s rather than the same. `cb == 0.0` is tested
// first, so the one input where both conditions hold -- a fully black
// backdrop under a fully white source, an ordinary pixel -- yields `0.0`,
// not `1.0`. Swapping the two is a real mutation, killed only by
// `composite_color_dodge_over_with_opacity_yields_zero_where_the_backdrop_
// is_zero`'s green channel.
//
// **Both guards are arithmetically redundant under IEEE-754 and are still
// required, and the two are redundant for *different* reasons.** Without
// the first, `0 / (1 - cs)` is a real, well-defined `+0` for every
// `cs < 1` -- not a `0/0` -- so `min(1, 0)` is the `0.0` the guard would
// have returned, and there is no division-by-zero semantics involved at
// all. It changes the result only at `cs == 1`, where the *second* guard
// then fires in its place and returns `1.0` instead of `0.0`. Without the
// second, `cb / 0` is `+inf`, `min(1, inf)` is `1`, and the result is the
// `1.0` the guard would have returned -- again the same answer, *if* the
// backend divides like IEEE. WGSL does not promise that: division by zero
// yields an indeterminate value, which may be `NaN`. And `NaN` here is not
// harmless -- but for a *different* reason than in `color_burn_channel`
// above, which is why this paragraph is written out rather than inherited
// from the sibling. A `NaN` can arise here only when `cb != 0.0`, because
// `cb == 0.0` returns from the *first* guard before any division is
// reached; and `cb != 0.0` requires `ab > 0`, since
// `fs_composite_color_dodge` forces `cb` to exactly `vec3(0.0, 0.0, 0.0)`
// on the `ab == 0` half. So the `NaN` reaches the output through `ab * b`
// with `ab` **strictly positive**, propagating directly. It is *not* the
// "a zero fails to absorb it" question `color_burn_channel`'s own note
// describes -- there the first guard is `cb == 1.0`, which does not fire
// at `cb == 0`, so on that mode's `ab == 0` half the division really is
// reached and really is multiplied by zero. The guards are what make this
// entry point defined rather than merely correct on one adapter. Measured,
// not reasoned about: deleting the *first* guard is killed deterministically,
// while deleting the *second* survives every test in this crate on
// Vulkan/NVIDIA, which is exactly the adapter-dependence this comment
// claims. See PLAN.md's 0.108.0 entry for both real results.
fn color_dodge_channel(cb: f32, cs: f32) -> f32 {
    if (cb == 0.0) {
        return 0.0;
    }
    if (cs == 1.0) {
        return 1.0;
    }
    return min(1.0, cb / (1.0 - cs));
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::ColorDodge` only -- the ninth blend mode ported
// to the GPU. Structurally identical to `fs_composite_color_burn` directly
// above: its `b` is a three-call `vec3` construction rather than one
// componentwise expression, for the reason `color_dodge_channel` gives.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent.
//
// **Deliberately not `1 - min(1, (1 - cb) / cs)`** -- that is `ColorBurn`,
// the other guarded-division mode (`fs_composite_color_burn` directly
// above, on the GPU since 0.107.0), whose branch conditions are `cb == 1`
// and `cs == 0` rather than this one's `cb == 0` and `cs == 1`. The two
// are structural mirror images and their *dispatch arms in `aurora-app`
// are adjacent*, which is where that hazard actually lives; this entry
// point's body was therefore derived from `blend_channel`'s own Rust arm
// rather than copied from its sibling and edited. And not
// `min(cb + cs, 1)`, which is `LinearDodge` -- the other dodge-family
// mode, likewise on the GPU. That one is worth a second sentence, because
// the two are *not* merely similar names: `min(1, cb / (1 - cs))` clamps
// exactly when `cb + cs >= 1`, which is exactly when `min(cb + cs, 1)`
// clamps, so **a clamped channel can never tell the two apart**. Any
// fixture meant to discriminate them needs an unclamped channel.
//
// **Asymmetry**: `B(Cb, Cs) != B(Cs, Cb)`, so -- as with `ColorBurn`, and
// unlike the seven commutative modes before it -- a transposed
// src/backdrop binding is caught by the blend term itself, not only by the
// asymmetric "over" around it.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the eight entry points
// above; no new binding, no new layout.
@fragment
fn fs_composite_color_dodge(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    let b = vec3<f32>(
        color_dodge_channel(cb.r, s.r),
        color_dodge_channel(cb.g, s.g),
        color_dodge_channel(cb.b, s.b),
    );
    return fold_over(s, bd, b);
}
