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
// fs_composite_color_burn, fs_composite_color_dodge,
// fs_composite_overlay, fs_composite_hard_light,
// fs_composite_linear_light and fs_composite_vivid_light -- use it,
// through the
// one bind group layout they share
// (`TileCompositor::bind_group_layout_blend`), so neither
// fixed-function entry point's own layout gains an entry.
//
// Named `backdrop_tex` after the Rust parameter it is actually bound to
// -- the `backdrop` of every `TileCompositor::composite_*_over_with_
// opacity` blend-math method (as of 0.114.0
// `composite_multiply_over_with_opacity` and its
// `darken`/`lighten`/`screen`/`difference`/`linear_dodge`/`linear_burn`/
// `color_burn`/`color_dodge`/`overlay`/`hard_light`/`linear_light`/
// `vivid_light` siblings, named
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
// `LinearBurn` (0.106.0), `ColorBurn` (0.107.0), `ColorDodge` (0.108.0),
// `Overlay` (0.110.0), `HardLight` (0.111.0), `LinearLight` (0.113.0) and
// `VividLight` (0.114.0) have
// since landed, and the live numbers live in `TileCompositor`'s own doc
// comment.
@group(0) @binding(3) var backdrop_tex: texture_2d<f32>;

// The two halves of the blend-math "over" that every blend-math entry
// point below shares verbatim, extracted in 0.109.0. Each statement in
// both bodies is byte-identical to the line it replaced in those entry
// points -- moved, not retyped -- with exactly one exception:
// `straight_backdrop`'s own `return cb;` is new text and replaced
// nothing, because the entry points left the recovered colour in a local
// and carried straight on rather than returning it. Every *other*
// statement is a moved line, which is what makes "bit-for-bit identical
// output" a property provable by text diff rather than by trusting
// arithmetic reasoning about equivalent rewrites. Do not "clean up"
// either body: `a + bd.a * inv` is deliberately not `a + ab * inv`
// (numerically identical, textually not), the `if` guard is deliberately
// not a `select()` (which would evaluate both arms and make the
// `0.0 / 0.0` divide unconditional -- see the guard's own comment below
// for how much of that is actually caught by a test, which is less than
// 0.109.0 first claimed), and no expression may be reordered, since
// float addition is not associative and the CPU differential tests
// assert exact equality.

// `composite_layer_into`'s `straight_backdrop = d.rgb / backdrop_alpha,
// or [0,0,0] if da == 0` -- the premultiplied accumulator's straight
// colour. The `if (ab > 0.0)` guard is that function's own
// `backdrop_alpha > 0.0` guard and the zero fallback is its
// `[0.0, 0.0, 0.0]`; as of 0.109.0 the guard lives here rather than
// once per entry point.
//
// **What actually protects this guard is five tests, not thirteen (measured,
// 0.109.0 for the first three, 0.110.0 for the fourth and 0.111.0 for the
// fifth; explained, 0.109.1).** Deleting the guard -- or replacing it with a
// `select()`, which evaluates both arms -- fails
// exactly five of the thirteen per-mode transparent-backdrop tests:
// `multiply`'s, `screen`'s, `difference`'s, `overlay`'s and
// `hard_light`'s. (Two naming
// shapes are in play: `multiply`'s and `darken`'s are
// `composite_<mode>_over_with_opacity_over_a_fully_transparent_backdrop_is_the_source_alone`,
// the other eleven are
// `composite_<mode>_over_with_opacity_is_the_source_alone_where_the_backdrop_is_transparent`.)
// The other eight -- `darken`, `lighten`, `linear_dodge`, `linear_burn`,
// `color_burn`, `color_dodge`, `linear_light`, `vivid_light` -- pass with the
// guard gone, and not
// because their fixtures are weak: on this backend `min()`/`max()`
// *launder* a NaN operand into the finite one, so those formulas
// turn the NaN into a finite `b` before `fold_over` ever sees it -- and
// `fold_over`'s own `ab == 0.0` would have erased that finite `b` anyway.
// That makes the guard's removal genuinely **output-equivalent**
// for those eight here, not merely undetected. See PLAN.md's 0.109.0 entry
// for the two isolating experiments and for why no fixture change can
// close it.
//
// **`Overlay` (0.110.0) is the fourth detector, and it is the first one
// whose detection was *predicted from the formula* before being run.**
// The mode has neither a `min` nor a `max` anywhere -- both arms of its
// `select()` are pure multiply/add -- so there is nothing to launder the
// `NaN` with. Measured: with the guard deleted, `fs_composite_overlay`'s
// texel 0 reads back literally `(NaN, NaN, NaN, 1.0)`, failing both that
// test's finiteness check and its value check. It also shows the general
// shape of the rule: a mode detects the guard's removal exactly when its
// blend term has no NaN-laundering intrinsic on the path from `cb` to
// `b`, which is a property to check when porting the next mode rather
// than a fact about these four.
//
// **`HardLight` (0.111.0) is the fifth detector, and the rule above is what
// predicted it — correctly, and measured before being written down here.**
// Like its transposed twin `Overlay` it has neither a `min` nor a `max`, so
// nothing launders the `NaN`. One detail differs from the sibling and is
// worth stating because it does *not* change the outcome: this mode's
// `select()` condition tests `s.rgb`, which the guard's removal does not
// touch, so a well-defined arm is still chosen — but `cb` appears in **both**
// arms (`cb * 2*Cs` and `cb + t - cb*t`), so `b` is `NaN` either way, and
// `fold_over`'s `ab * b` is `0.0 * NaN`, which is `NaN`, not `0.0`.
// Measured: with the guard deleted, `fs_composite_hard_light`'s texel 0 reads
// back literally `(NaN, NaN, NaN, 1.0)`, failing both that test's finiteness
// check and its value check. See PLAN.md's 0.111.0 entry.
//
// **Neither `LinearLight` (0.113.0) nor `VividLight` (0.114.0) is a sixth
// detector, and the same rule is
// what predicted that too — the first time the rule predicted a *non*-detection
// for a newly ported mode, and it was then measured rather than assumed.**
// `LinearLight`'s own
// blend term is a single `clamp()`, and WGSL specifies float `clamp(e1, e2, e3)`
// as `min(max(e1, e2), e3)`; so despite containing no literal `min`/`max` token
// it has *two* of them, and it launders the `NaN` into the finite bound exactly
// as `darken` and the other five do. Measured in 0.113.0: with the guard
// deleted, `composite_linear_light_over_with_opacity_is_the_source_alone_where_
// the_backdrop_is_transparent` stays **green**. That is the same
// output-equivalence the six min/max modes have, not a weak fixture — and it is
// why the rule is worth stating as "no NaN-laundering intrinsic on the path from
// `cb` to `b`" rather than as a list of tokens to grep for. **`VividLight`
// (0.114.0) is the second such prediction and it held too**: both of its arms
// end in a `min()` -- inherited from `color_burn_channel` and
// `color_dodge_channel` -- so a `NaN` `cb` is laundered into a finite `0.0` or
// `1.0` before `fold_over` ever sees it. Measured in 0.114.0: with the guard
// deleted, `composite_vivid_light_over_with_opacity_is_the_source_alone_where_
// the_backdrop_is_transparent` stays green.
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
// test comments in `composite.rs` that describe a kept per-mode test's
// rationale are stale in **two** ways, not one -- 0.109.0 disclosed only
// the first, and 0.109.1 corrects that:
//
//   1. **Location.** Comments quoting
//      `if (ab > 0.0) { cb = bd.rgb / ab; }` and attributing it to a
//      *specific* entry point now name the wrong function; the line lives
//      in `straight_backdrop()` above.
//   2. **Independence, which matters more.** Several suite-header comments
//      justify keeping a per-mode transparent-backdrop test by listing
//      "its own separately-compiled `ab > 0.0` guard" among the things
//      that test uniquely exercises. That is no longer true: the guard is
//      written once and shared by all thirteen entry points. (A backend is
//      still free to inline it per call site, so per-entry-point *machine
//      code* is not ruled out -- but the source-level independence the
//      comments leaned on is gone, and the measured 5-of-11 kill above is
//      the direct evidence.)
//
// Those suite-header comments in `composite.rs` were corrected in 0.109.1
// and carry the specific sites; the per-test doc comments there carry a
// one-line back-reference to here rather than repeating this. Only 5 of
// the 13 kept transparent-backdrop tests can currently detect the guard's
// removal at all -- see the `straight_backdrop()` comment above, and
// PLAN.md's 0.109.0 entry for why.
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
// channel at a time -- one of this file's per-channel blend helpers (it was
// the first non-entry-point function here when 0.107.0 added it; 0.109.0's
// `straight_backdrop()`/`fold_over()` now precede it, so the ordinal is
// dropped rather than re-counted), and the
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
// channel at a time -- the other of this file's per-channel blend helpers
// (second of them, and no longer this file's second non-entry-point
// function: 0.109.0's `straight_backdrop()`/`fold_over()` precede both),
// and the exact structural sibling of `color_burn_channel`
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

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs)
// exactly, for `BlendMode::Overlay` only -- the tenth blend mode ported to
// the GPU, and the first whose formula is a real *branch* that is still
// expressible componentwise.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of
// the surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` block below differs.
//
// **The formula, derived from `blend_channel`'s own arm rather than from
// the spec.** That arm is literally
// `BlendMode::Overlay => blend_channel(BlendMode::HardLight, cs, cb)` --
// `HardLight` with the two channel arguments *swapped*. Substituting
// `HardLight`'s own body (which branches on *its* `cs`, i.e. on our `Cb`)
// and then its `Multiply`/`Screen` delegates gives, in Overlay's own
// terms:
//
//     Cb <= 0.5:  B = Multiply(Cs, 2*Cb)      = Cs * (2*Cb)
//     Cb >  0.5:  B = Screen(Cs, 2*Cb - 1)    = Cs + t - Cs * t,  t = 2*Cb - 1
//
// Note the branch tests the **backdrop**, not the source -- that swap is
// the whole difference between `Overlay` and `HardLight`, and branching on
// `s.rgb` here would silently compute `HardLight` instead (mutation (c) of
// this round's set). Each expression is written in `blend_channel`'s own
// evaluation order (`cb * cs` becomes `s.rgb * (2.0 * cb)` because the
// swap makes our source the *first* operand there; `cb + cs - cb * cs`
// becomes `s.rgb + t - s.rgb * t` for the same reason), not in an
// algebraically equivalent rearrangement, because the CPU differential
// tests in `composite.rs` assert **exact** equality and float addition is
// not associative.
//
// **Written with `select()`, and this is the first entry point here that
// uses one.** `color_burn_channel` and `color_dodge_channel` above exist
// precisely *because* a componentwise form was not available to them: a
// `select()` evaluates **both** arms, and their discarded arm contains a
// division that is undefined in exactly the lanes the branch exists to
// exclude, so evaluating it would reintroduce the `0/0` the guards remove.
// `Overlay` has no such operation -- both arms are pure finite
// multiply/add on operands already in `[0, 1]`, so evaluating both is
// harmless and one componentwise `select()` is the honest shape. (The
// same reasoning is why `straight_backdrop()`'s `if` guard above is
// deliberately *not* a `select()`; the distinction is the discarded arm's
// definedness, not a blanket preference.)
//
// `cb <= vec3<f32>(0.5)` splats deliberately: WGSL's comparison operators
// have no scalar/vector mixed overload, so `cb <= 0.5` does not
// type-check, even though `-` and `*` *do* broadcast a scalar (which is
// why `2.0 * cb - 1.0` needs no splat two lines up). That asymmetry is the
// same one `fs_composite_linear_burn` above already documents for
// `max(... - 1.0, vec3<f32>(0.0))`.
//
// **The two branches agree bit-exactly at `Cb == 0.5`, which has a
// consequence worth stating.** The `lo` arm gives `Cs * 1.0 = Cs`; the
// `hi` arm gives `t = 0.0` exactly, so `Cs + 0.0 - Cs * 0.0 = Cs` -- the
// same value, not merely a close one. So `Overlay` is continuous there,
// **and a mutation flipping `<=` to `<` is unkillable in principle**: the
// two arms compute identical bits on the only inputs that distinguish the
// two conditions. `composite_overlay_over_with_opacity_agrees_across_its_
// own_branch_boundary` in `composite.rs` pins the continuity; nothing can
// pin the comparison, and no test there claims to.
//
// **Four degeneracies, disclosed because they constrain every fixture in
// this crate's `composite_overlay_*` tests** (each verified algebraically
// against the two arms above, not assumed):
//
//   1. `Overlay(0.5, Cs) = Cs` -- a backdrop channel at exactly `0.5` is
//      indistinguishable from `Normal` (and from `Screen`'s and
//      `LinearBurn`'s own saturated-backdrop degeneracies).
//   2. `Overlay(Cb, 0.5) = Cb` for **every** `Cb` -- a *source* channel at
//      exactly `0.5` makes this mode a total no-op. Both arms give it: the
//      `lo` arm is `0.5 * 2*Cb = Cb`, and the `hi` arm is
//      `0.5 + t - 0.5*t = 0.5 + 0.5*(2*Cb - 1) = Cb`. This one is easy to
//      hit by accident and erases the source entirely, so no solid-colour
//      fixture here uses `0.5` in any channel of either operand.
//   3. `Overlay(0, Cs) = 0` and `Overlay(1, Cs) = 1` -- a black or white
//      backdrop channel erases the source, exactly as `Multiply` and
//      `Screen` do at those points.
//   4. `Cb == Cs` in a channel lets a transposed operand pair hide behind
//      an accidental equality (see the asymmetry note below for why that
//      matters here and not in the seven commutative modes).
//
// **The `HardLight` collision rule, worked out rather than asserted.**
// `HardLight(Cb, Cs) = Overlay(Cs, Cb)`, so the two modes agree exactly
// wherever `Cb` and `Cs` fall on the **same side** of `0.5`:
//
//   - both `<= 0.5`: `Overlay = Cs * 2*Cb` and
//     `HardLight = Cb * 2*Cs`, both `2*Cb*Cs` -- and bit-identical too,
//     since `2*x` is exact and each form is one rounding of the same
//     product.
//   - both `> 0.5`: `Overlay = 2*Cb + 2*Cs - 1 - 2*Cb*Cs` and
//     `HardLight = 2*Cs + 2*Cb - 1 - 2*Cb*Cs` -- the same value, though
//     reached by different expression sequences, so agreement there is
//     algebraic and only *usually* bit-exact.
//
// They differ only where the two operands **straddle** `0.5`. **As of
// 0.111.0 `HardLight` has its own entry point below
// (`fs_composite_hard_light`) and its own `aurora-app` dispatch arm, so it
// is now a wrong-arm hazard in the fullest sense the burn and dodge pairs
// are, and in one they are not: the hazard here is *bidirectional*, each of
// the two modes being the other's transpose.** (Until 0.111.0 this comment
// said `HardLight` was CPU-only with no entry point, which was true then and
// is exactly the claim that port had to come back and correct.) It remains
// what a transposed `src`/`backdrop` binding computes here — and now also
// what a `fragment_entry` naming the sibling computes — which is why every
// fixture in this crate's `composite_overlay_*` tests, and `aurora-app`'s
// `NORMAL_MULTIPLY_OVERLAY_STACK`, straddles `0.5` in all three channels.
//
// **Asymmetry, and it is *conditional* -- the first of that kind.**
// `ColorBurn` (0.107.0) and `ColorDodge` (0.108.0) are asymmetric
// everywhere; the other seven ported modes are commutative everywhere.
// `Overlay` is neither: `B(Cb, Cs) = B(Cs, Cb)` in every channel whose two
// operands share a side of `0.5` (that is the collision rule above with
// `Overlay` on both sides), and differs only in a straddling channel. So a
// transposed binding is caught by the blend term itself, but **only** in
// straddling channels -- a fixture whose channels all sit below `0.5`
// would see nothing but the asymmetric "over".
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the nine entry points
// above; no new binding, no new layout.
@fragment
fn fs_composite_overlay(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    // blend_rgb(Overlay, cb, cs) == blend_channel(HardLight, cs, cb):
    // Multiply(cs, 2*cb) where cb <= 0.5, else Screen(cs, 2*cb - 1).
    let lo = s.rgb * (2.0 * cb);
    let t = 2.0 * cb - 1.0;
    let hi = s.rgb + t - s.rgb * t;
    let b = select(hi, lo, cb <= vec3<f32>(0.5));
    return fold_over(s, bd, b);
}

// Mirrors `blend_channel(BlendMode::HardLight, cb, cs)` (src/composite.rs)
// exactly. `HardLight` in the shader (0.111.0) — the eleventh blend mode on
// the GPU path, and `fs_composite_overlay` directly above is its **exact
// transposed twin**: `HardLight(Cb, Cs) = Overlay(Cs, Cb)` and
// `Overlay(Cb, Cs) = HardLight(Cs, Cb)`. This is the first entry point in
// this file whose transposed twin is *also* a live entry point here, so the
// wrong-arm hazard is now **bidirectional between two real, shipped GPU
// modes** rather than "computed by mistake but unreachable by dispatch",
// which is what `fs_composite_overlay`'s own comment above could still say
// about `HardLight` at 0.110.0 and can no longer.
//
// **Derived from `blend_channel`'s own `HardLight` arm, not from
// `fs_composite_overlay`.** That discipline is not stylistic here, it is the
// whole defence: the two entry points differ in exactly one *semantic*
// place — which operand the branch tests — and copying the sibling and
// "fixing it up" is precisely how that one place gets left alone.
// `blend_channel`'s arm is:
//
//     BlendMode::HardLight => if cs <= 0.5 {
//         blend_channel(BlendMode::Multiply, cb, 2.0 * cs)
//     } else {
//         blend_channel(BlendMode::Screen, cb, 2.0 * cs - 1.0)
//     }
//
// Substituting `Multiply`'s `cb * cs` and `Screen`'s `cb + cs - cb * cs`
// with **`cb` as the first operand of each delegate** (the opposite of
// `Overlay`'s substitution, where the *source* is first) gives:
//
//     Cs <= 0.5:  B = Multiply(Cb, 2*Cs)   = Cb * 2*Cs
//     Cs >  0.5:  B = Screen(Cb, 2*Cs - 1) = Cb + t - Cb*t,  t = 2*Cs - 1
//
// The branch tests the **source**. Both statements below keep
// `blend_channel`'s own operand order and expression sequence verbatim
// (`cb * (2.0 * s.rgb)`, not `(2.0 * s.rgb) * cb`; `cb + t - cb * t`, not an
// algebraic rearrangement) because the CPU differential tests in
// composite.rs assert **exact** equality and float addition is not
// associative.
//
// **Measured, and it corrects what this comment first claimed.** The round
// that wrote this asserted that `cb * (2.0 * s.rgb)` here, against the
// sibling's `s.rgb * (2.0 * cb)`, "is the evidence this was not copy-pasted".
// It is evidence for a *human reader* and nothing more: mutation (m) of the
// round's matrix rewrote this `lo` arm into the sibling's exact shape and
// **the entire suite stayed green**, on real hardware. That is not a coverage
// gap, it is the collision rule's own "both operands `<= 0.5`" clause applied
// to one arm -- `Cs * 2*Cb` and `Cb * 2*Cs` are both `2*Cb*Cs`, and
// bit-identically so, `2*x` being exact and each form one rounding of the
// same product. (`2*x` is exact for every `x` an `f16`-backed tile can
// actually produce here, which is the claim that matters: it would fail only
// on f32 overflow, and `straight_backdrop`'s divide bounds `cb` to roughly
// `65504 / 5.96e-8` = 1.1e12 -- far below where `2*cb` could overflow.)
// **The `lo` arm's operand order is therefore unobservable in
// principle, and no test can or should claim to pin it.** What *is*
// observable, and was confirmed by running it, is the `hi` arm's base operand
// (writing `s.rgb + t - s.rgb * t` there kills all seven of this mode's
// render tests and the app golden) and the `select()` condition (mutation
// (c), which computes `Overlay` outright). So the derivation-from-the-Rust-arm
// discipline earns its keep on the branch and the `hi` arm; on the `lo` arm it
// buys readability, not correctness.
//
// **Written with `select()`, for the same reason the sibling above is** —
// both arms are pure finite multiply/add on operands already in `[0, 1]`, so
// evaluating the discarded one is harmless, unlike `color_burn_channel`'s
// and `color_dodge_channel`'s, whose discarded arm divides by zero in
// exactly the lanes their branch excludes. The condition is
// `s.rgb <= vec3<f32>(0.5)`, **not** `cb <= vec3<f32>(0.5)`: the latter is
// literally `fs_composite_overlay`, so that one token is the difference
// between this mode and a second, redundant copy of its neighbour. The splat
// is required for the same reason the sibling's is — WGSL's comparison
// operators have no scalar/vector mixed overload, even though `*` and `-`
// broadcast a scalar.
//
// **The two branches agree bit-exactly at `Cs == 0.5`.** The `lo` arm gives
// `Cb * 1.0 = Cb`; the `hi` arm gives `t = 0.0` exactly, so
// `Cb + 0.0 - Cb * 0.0 = Cb` — the same bits. So `HardLight` is continuous
// there, **and a mutation flipping `<=` to `<` is unkillable in principle**,
// exactly as for `Overlay`: the two arms compute identical bits on the only
// input that distinguishes the two conditions.
// `composite_hard_light_over_with_opacity_agrees_across_its_own_branch_
// boundary` in composite.rs pins the continuity; nothing can pin the
// comparison, and no test there claims to.
//
// **Four degeneracies, each verified algebraically against the two arms
// above and each the *transpose* of the sibling's correspondingly-numbered
// one — read them carefully rather than by analogy:**
//
//   1. `HardLight(0.5, Cs) = Cs` for **every** `Cs` — a *backdrop* channel
//      at exactly `0.5` is indistinguishable from `Normal`. Both arms give
//      it: `lo` is `0.5 * 2*Cs = Cs`, and `hi` is
//      `0.5 + t - 0.5*t = 0.5 + 0.5*(2*Cs - 1) = Cs`. Note this is **not**
//      the branch boundary here (the branch is on `Cs`), which is where it
//      differs structurally from the sibling's degeneracy 1.
//   2. `HardLight(Cb, 0.5) = Cb` for **every** `Cb` — a *source* channel at
//      exactly `0.5` makes this mode a total no-op, **and this one *is* the
//      branch boundary**: `lo` gives `Cb * (2*0.5) = Cb` and `hi` gives
//      `t = 0.0`, so `Cb + 0.0 - Cb*0.0 = Cb`. That coincidence is what
//      makes the `<=`/`<` mutation unkillable.
//   3. `HardLight(Cb, 0) = 0` and `HardLight(Cb, 1) = 1` — a black or white
//      *source* channel erases the backdrop (`lo` at `Cs = 0` is `Cb * 0`;
//      `hi` at `Cs = 1` has `t = 1`, so `Cb + 1 - Cb`). The sibling's
//      degeneracy 3 is on the *backdrop* side; do not carry it over.
//      Deliberately **not** true of a `0` or `1` backdrop here:
//      `HardLight(0, Cs)` is `0` on the low arm but `2*Cs - 1` on the high
//      one, and `HardLight(1, Cs)` is `2*Cs` then `1`.
//   4. `Cb == Cs` in a channel lets a transposed operand pair hide behind an
//      accidental equality — and here that is stronger than "a transpose
//      survives": by the collision rule below, `Cb == Cs` makes this mode
//      and `Overlay` produce the *same* value, so such a channel cannot see
//      a branch-on-the-wrong-operand mutation either.
//
// **The two-way collision rule, worked out rather than asserted.** The modes
// agree exactly wherever `Cb` and `Cs` fall on the **same side** of `0.5`:
//
//   - both `<= 0.5`: `HardLight = Cb * 2*Cs` and `Overlay = Cs * 2*Cb`, both
//     `2*Cb*Cs` — and bit-identically, since `2*x` is exact and each form is
//     one rounding of the same product.
//   - both `> 0.5`: `HardLight = 2*Cs + 2*Cb - 1 - 2*Cb*Cs` and
//     `Overlay = 2*Cb + 2*Cs - 1 - 2*Cb*Cs` — the same value, reached by
//     different expression sequences, so agreement there is algebraic and
//     only *usually* bit-exact.
//
// They differ only where the two operands **straddle** `0.5`. Since both
// modes are now live entry points here, that is simultaneously (a) what a
// transposed `src`/`backdrop` binding on *this* pass computes, (b) what a
// branch on `cb` instead of `s.rgb` computes, and (c) what a `fragment_entry`
// naming the sibling computes — three distinct mutations that all land on the
// same wrong answer. Every fixture in this crate's `composite_hard_light_*`
// tests, and `aurora-app`'s `NORMAL_MULTIPLY_HARD_LIGHT_STACK`, therefore
// straddles `0.5` in all three channels.
//
// **Asymmetry: conditional, the second of that kind.** `Overlay` (0.110.0)
// was the first; `ColorBurn`/`ColorDodge` are asymmetric everywhere and the
// other seven ported modes commutative everywhere. `B(Cb, Cs) = B(Cs, Cb)`
// here in every channel whose two operands share a side of `0.5` (that is
// the collision rule with `HardLight` on both sides), and differs only in a
// straddling channel.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the ten entry points above;
// no new binding, no new layout.
@fragment
fn fs_composite_hard_light(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    // blend_channel(HardLight, cb, cs): Multiply(cb, 2*cs) where cs <= 0.5,
    // else Screen(cb, 2*cs - 1). The branch tests the SOURCE -- branching on
    // `cb` here is `fs_composite_overlay`, one token away and directly above.
    let lo = cb * (2.0 * s.rgb);
    let t = 2.0 * s.rgb - 1.0;
    let hi = cb + t - cb * t;
    let b = select(hi, lo, s.rgb <= vec3<f32>(0.5));
    return fold_over(s, bd, b);
}

// `blend_channel`'s own `BlendMode::LinearLight` arm (src/composite.rs),
// componentwise -- the **twelfth** blend mode ported to WGSL (0.113.0), and the
// simplest-shaped one since `LinearBurn`. Derived from that Rust arm, which is
// literally one expression:
//
//     BlendMode::LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
//
// **No branch, no `select()`, and that is the structural news of this round.**
// The two overlay-family modes ported before it (`Overlay`, `HardLight`) are
// two-call delegations that had to become a componentwise `select()` here.
// This mode's CPU arm delegates to nothing: `blend_channel` already carries a
// proof (in its own comment, and in
// `linear_light_simplified_form_matches_the_branch_form_for_several_inputs`)
// that the *branch* form -- `Cs <= 0.5 -> LinearBurn(Cb, 2*Cs)`, else
// `LinearDodge(Cb, 2*Cs - 1)` -- collapses to a single unconditional clamp,
// because each branch only ever reaches the one bound it would have applied
// anyway. So this entry point is shaped like `fs_composite_linear_burn`'s, not
// like the two directly above it.
//
// **This is the first `clamp()` in this file** (every earlier mode reaches for
// `min`, `max`, `abs` or plain arithmetic), which matters for one reason and
// not for correctness: WGSL specifies float `clamp(e1, e2, e3)` as
// `min(max(e1, e2), e3)`, so it **launders a NaN into a finite operand exactly
// as the six min/max modes do** -- the opposite of `Overlay`'s and
// `HardLight`'s behaviour. See `straight_backdrop`'s own comment for the
// measured result of deleting its guard against this mode.
//
// **Near misses, in decreasing order of how easy the slip is:**
//
//   - **Dropping the `2.0 *` gives `LinearBurn` exactly**, not merely
//     something close: `clamp(cb + cs - 1, 0, 1)` never reaches its upper
//     bound (both operands are in `[0, 1]`, so the sum is at most `1`), so it
//     *is* `max(cb + cs - 1, 0)`, which is `fs_composite_linear_burn` above.
//     That is a live GPU entry point and a live dispatch arm, so this is the
//     one slip in this round that silently computes another shipped mode.
//   - **`min(cb + s.rgb, 1)` is `LinearDodge`** (also live, also above), the
//     other half of the pair this mode's branch form is built from.
//   - Dropping either bound of the clamp is a real mutation in its own right:
//     `min(cb + 2*cs - 1, 1)` loses the lower rail and
//     `max(cb + 2*cs - 1, 0)` the upper one. Both are killed, but only by a
//     fixture that actually reaches the rail in question -- see the
//     `composite_linear_light_*` suite header for which fixture covers which.
//
// **Two degeneracies, both verified algebraically:**
//
//   1. `LinearLight(Cb, 0.5) = Cb` for every `Cb` -- a **source** channel at
//      exactly `0.5` makes this mode a total no-op (`Cb + 1 - 1`). No
//      solid-colour fixture in this crate's `composite_linear_light_*` tests
//      uses `0.5` in any *source* channel.
//   2. `LinearLight(Cb, 0) = 0` and `LinearLight(Cb, 1) = 1` -- a black or
//      white **source** channel erases the backdrop, both by clamping.
//
// A `0.5` **backdrop** channel is deliberately *not* degenerate here
// (`LinearLight(0.5, Cs) = clamp(2*Cs - 0.5, 0, 1)`, which still depends on
// `Cs`), unlike `HardLight`, where it is `Normal`. Backdrop `0.5`s are
// therefore usable and are used.
//
// **Asymmetry: UNCONDITIONAL, and this is the fifth asymmetric mode on the
// GPU path -- the third unconditionally so**, after `ColorBurn` (0.107.0) and
// `ColorDodge` (0.108.0). `Overlay` and `HardLight` are asymmetric only where
// their operands straddle `0.5`. Here the algebra is direct:
//
//     B(Cb, Cs) - B(Cs, Cb) = (Cb + 2*Cs) - (Cs + 2*Cb) = Cs - Cb
//
// *before* the clamp -- so the **blend term** differs in every channel whose
// operands differ at all, at any opacity.
//
// **Two separate things launder that, and they are exactly complementary --
// worked out algebraically in this round and then measured, because the
// obvious reading of "unconditional asymmetry" is wrong about both.** Over an
// opaque backdrop at effective alpha `a`, `fold_over` gives
// `out = (1-a)*Cb + a*B`, so:
//
//   - **A railed channel launders it.** If both operand orders drive the sum
//     past the *same* bound, both `B`s are that bound and only the fold's own
//     `(1-a)*Cb` vs `(1-a)*Cs` remains -- which vanishes at `a = 1`. So a
//     saturated channel at full alpha sees nothing.
//   - **A clamp-*interior* channel launders it at `a = 0.5` exactly.** With
//     both orders interior, `out - out_transposed = (Cb - Cs) * (1 - 2a)`,
//     which is **zero at `a = 0.5`** -- there `out = Cb + Cs - 0.5`, symmetric
//     in its two operands. This is the surprising half: the very non-unit
//     opacity `aurora-app`'s transpose guard demands is the one value at which
//     an interior channel goes blind.
//
// So a fixture needs a *railed* channel to catch a transpose at `a = 0.5`, and
// an *interior* channel to catch one at `a = 1`. The
// `composite_linear_light_*` suite header names which of its fixtures supplies
// which, and the app-level fixture carries both.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the eleven entry points
// above; no new binding, no new layout.
@fragment
fn fs_composite_linear_light(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    // blend_channel(LinearLight, cb, cs): clamp(cb + 2*cs - 1, 0, 1), one
    // expression and no branch -- see composite.rs's own arm for why the
    // branch form collapses to this. Dropping the `2.0 *` here is
    // fs_composite_linear_burn exactly.
    let b = clamp(cb + 2.0 * s.rgb - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
    return fold_over(s, bd, b);
}

// `blend_channel`'s own `BlendMode::VividLight` arm (src/composite.rs), one
// channel at a time -- the third per-channel blend helper in this file, and
// the first that computes no arithmetic of its own: it is a branch and two
// delegations, exactly as the Rust arm is.
//
//     BlendMode::VividLight => if cs <= 0.5 { ColorBurn(cb, 2*cs) }
//                             else          { ColorDodge(cb, 2*cs - 1) }
//
// **Why three calls to this and not one componentwise `select()`.**
// `fs_composite_overlay` and `fs_composite_hard_light` are branches written as
// `select()`, which is legitimate *there* because neither arm divides, so
// evaluating the discarded one costs nothing. Here both arms are
// `color_burn_channel`/`color_dodge_channel`, whose guards are **early
// returns**: they exist precisely so a division is never reached in the lanes
// where its divisor is zero. A `select()` would evaluate both arms in every
// lane, which reinstates exactly the division-by-zero the guards were written
// to avoid. Worth stating precisely, because the weaker argument is the one
// that first comes to mind and it is not the argument being made: the
// *discarded* arm here is not itself unsafe -- it is guarded too, and would
// return its guard's value rather than divide. What a `select()` costs is the
// early-return structure, not correctness of the branch taken; and since the
// guards are the file's only defence against WGSL's indeterminate
// division-by-zero, giving that structure up to save two lines is not a trade
// worth making. Three calls is the honest shape, for the same reason
// `color_burn_channel`'s own comment gives.
//
// **The `2.0 * cs` and `2.0 * cs - 1.0` substitutions are bit-faithful.**
// Multiplying by two and subtracting one are exact in IEEE-754 binary for
// operands in `[0, 1]` (a power-of-two scale, then a subtraction of same-sign
// magnitudes at most one exponent apart), so nothing here rounds and all four
// of the callees' guards stay reachable through this substitution:
//
//   | guard | fires when | deleting it |
//   |---|---|---|
//   | `color_burn_channel`'s `cb == 1.0` | `Cs <= 0.5` and `Cb == 1` | killed deterministically (the `cs == 0` guard then fires in its place and returns `0.0` where `1.0` is correct) |
//   | `color_burn_channel`'s `cs == 0.0` | `Cs == 0.0` exactly (`2*Cs` is `0` only there) | **survives on this adapter** -- `(1-Cb)/0` is `+inf` here and `1 - min(1, inf)` is the `0.0` the guard returns |
//   | `color_dodge_channel`'s `cb == 0.0` | `Cs > 0.5` and `Cb == 0` | killed deterministically |
//   | `color_dodge_channel`'s `cs == 1.0` | `Cs == 1.0` exactly (`2*Cs - 1` is `1` only there) | **survives on this adapter** -- `Cb/0` is `+inf` here and `min(1, inf)` is the `1.0` the guard returns |
//
// Both survivals are *inherited*, not new: they are `ColorBurn`'s (0.107.0)
// and `ColorDodge`'s (0.108.0) own disclosed portability gaps, reaching this
// mode through the two helpers it shares with them. WGSL specifies an
// indeterminate value for division by zero, not `+inf`, so those two guards
// are portability guards that no test on this hardware can exercise -- and
// this mode now propagates *both* of them at once, where each sibling
// propagates one.
//
// **The `ab == 0` half of a tile reaches the two branches differently**, and
// the two arguments are genuinely distinct rather than one argument stated
// twice. There `straight_backdrop` forces `cb` to exactly `0.0`. In the
// **dodge** branch that hits `cb == 0.0` and returns before any division, so
// `ColorDodge`'s own NaN-safety argument carries over verbatim. In the
// **burn** branch `cb == 0.0` does *not* match `cb == 1.0`, so the division is
// reached; if `Cs == 0.0` as well, an unguarded form would compute `0.0/0.0`
// and `fold_over`'s `ab * b` would then be `0.0 * NaN`, which is `NaN`, not
// `0.0` -- `ColorBurn`'s hazard shape, governing branch 1, where
// `ColorDodge`'s governs branch 2. Note also that the two `cb` guards test
// **different constants** (`cb == 1` in burn, `cb == 0` in dodge), so there is
// no single "the `cb` guard" for this mode.
//
// **Asymmetry: UNCONDITIONAL, and structurally so.** `B(Cb, Cs)` branches on
// `Cs` while `B(Cs, Cb)` branches on `Cb`, so a pair straddling `0.5` takes
// two different *families* under transposition -- this is `ColorBurn`'s and
// `ColorDodge`'s class, not `Overlay`'s and `HardLight`'s straddle-conditional
// one. It is **not** affine in its operands, so `LinearLight`'s tidy
// `(Cb - Cs)*(1 - 2a)` form has no analogue here.
//
// **What that does *not* buy is freedom from a blind opacity, and 0.114.0's
// first draft of this comment wrongly claimed it did** -- corrected here rather
// than papered over, because the wrong version is the one that reads as
// reassuring. The fold's own transpose gap,
//
//     out - out_transposed = (1 - a)*(Cb - Cs)
//                            + a*(B(Cb, Cs) - B(Cs, Cb)),
//
// is affine in `a` for **every** blend mode, this one included: `B`'s
// non-affinity is in its *operands*, which that expression never varies. So
// writing `D0 = Cb - Cs` and `D1 = B(Cb, Cs) - B(Cs, Cb)`, a channel is blind
// at `a* = D0 / (D0 - D1)`, and `a*` lands in `(0, 1)` whenever `D0` and `D1`
// have opposite signs -- clamp-interior in both operand orders or not.
//
// Two mechanisms therefore launder a transpose here, not one:
//
//   - *rail agreement* in the blend term, in two regimes:
//       - burn branch (`Cs <= 0.5`): `B` rails to `0` when `Cb + 2*Cs <= 1`.
//         Both operand orders rail when `Cb + 2*Cs <= 1` **and**
//         `Cs + 2*Cb <= 1`, and the blend term is blind to a transpose there.
//       - dodge branch (`Cs > 0.5`): `B` rails to `1` when `Cb + 2*Cs >= 2`.
//         Both orders rail when `Cb + 2*Cs >= 2` **and** `Cs + 2*Cb >= 2`.
//   - the per-channel blind `a*` above. Inside the burn branch the algebra
//     closes exactly: for `Cb != Cs`, `D0 + D1 == 0` -- blind at precisely
//     `a = 0.5` -- reduces to `2*Cb*Cs + Cb + Cs == 1`, derived and then
//     confirmed by exhaustive rational search. `Cb = 0.3`, `Cs = 0.4375` sits
//     on that locus with **both** orders burn-*interior* (`B = 0.2` against
//     `B^T = 0.0625`), so such a channel is blind at exactly the `0.5` every
//     roster fixture carries.
//   - and universally, `Cb == Cs` hides one.
//
// **So "clamp-interior in both operand orders" does not imply "observable at
// every alpha".** What establishes observability is arithmetic, not that
// heuristic: `aurora-app`'s
// `every_gpu_blend_math_dispatch_arm_has_a_fixture_that_could_see_a_transposed_
// argument` folds each roster fixture both ways on the CPU (0.113.1's third
// assertion) and measures the gap. For this mode that assertion is
// **necessary**, not defence in depth.
//
// Measured in 0.114.0 rather than argued: the app-level fixture's transpose is
// caught in all three channels at `0.5` and all three at `1.0`. Every one of
// its three channels does have a blind `a*` in `(0, 1)` -- red `~0.8882`,
// green `0.75`, blue `~0.9310` -- and none of them is at `0.5` or `1.0`, which
// is the whole reason both opacities work there.
//
// **Five degeneracies, all verified algebraically, and they constrain every
// fixture in this crate's `composite_vivid_light_*` tests:**
//
//   1. `VividLight(Cb, 0.5) = Cb` for every `Cb` -- a **source** channel at
//      exactly `0.5` is a total no-op. This is also the branch boundary; see
//      below.
//   2. `VividLight(Cb, 0) = 0` except `VividLight(1, 0) = 1`, and
//      `VividLight(Cb, 1) = 1` except `VividLight(0, 1) = 0` -- a black or
//      white **source** channel erases the backdrop, the two guard points
//      excepted.
//   3. `VividLight(0, Cs) = 0` and `VividLight(1, Cs) = 1` for every `Cs` --
//      a black or white **backdrop** channel erases the *source* entirely.
//      Unlike the sibling modes this makes the *composited* backdrop a
//      constraint, not just the bottom layer's own literal.
//   4. A `0.5` **backdrop** channel is *not* degenerate
//      (`VividLight(0.5, Cs)` is `1 - 1/(4*Cs)` below the boundary and
//      `1/(4 - 4*Cs)` above it, clamped), like `LinearLight` and unlike
//      `HardLight`.
//   5. `Cb == Cs` in a channel hides a transposed operand pair.
//
// **The branch boundary is continuous, and the `<=` there is provably
// unkillable** -- the third such disclosure, after `Overlay` (0.110.0) and
// `HardLight` (0.111.0). At `Cs == 0.5` the burn arm computes
// `ColorBurn(Cb, 1.0)`, which is `1 - min(1, (1 - Cb)/1) = Cb`, and the dodge
// arm computes `ColorDodge(Cb, 0.0)`, which is `min(1, Cb/1) = Cb`. That
// includes both arms' guard points (`Cb == 1` gives `1`, `Cb == 0` gives `0`,
// from the guards and from the arithmetic alike). The two arms are therefore
// identical for **every `f16`-representable `Cb` in `[0, 1]`** -- exactly the
// domain `vivid_light_at_its_branch_boundary_is_the_backdrop_for_every_f16_
// value` sweeps, and bit-exactly there, not merely within tolerance -- so `<=`
// against `<` computes the same function and no test in this suite can
// distinguish them.
//
// **Stated precisely, because the unqualified form of that claim is false in
// two ways** (both disclosed in 0.114.1 rather than left implied):
//
//   - the burn arm's value is `1 - (1 - Cb)`, and that is bit-exactly `Cb` for
//     every `f16`-representable `Cb`, but **not** for a general `f32` `Cb` --
//     which is what `straight_backdrop`'s own division yields from a
//     non-opaque premultiplied backdrop. Measured: `bd.rgb/bd.a` of
//     `3.57627869e-07 / 0.999511719` is `3.5780258e-07`, where the burn arm
//     gives `3.5762787e-07` and the dodge arm `3.5780258e-07`. The gap is
//     bounded by two roundings at `1.0`, so at most `2^-25` (measured maximum
//     `2.98e-8`) -- `1/65536` of the suites' own `2 * f16::EPSILON` tolerance.
//     So the mutation is unkillable *at this suite's chosen tolerance*, not
//     because the two arms compute the same function on all inputs.
//   - `straight_backdrop` does not clamp, so an accumulator whose
//     premultiplied `rgb` exceeds its own `a` yields `cb > 1`, where the two
//     arms genuinely diverge: at `Cs == 0.5`, `Cb = 1.5` gives `1.5` from the
//     burn arm (neither of `color_burn_channel`'s guards is an inequality) and
//     `1.0` from the dodge arm. That is off-nominal input for a well-formed
//     premultiplied accumulator, not a reachable path any fixture has.
//
// `composite_vivid_light_over_with_opacity_agrees_across_its_own_branch_boundary`
// pins the *value* at the boundary and deliberately does not claim to test the
// comparison's direction.
//
// **Not a sixth detector of `straight_backdrop`'s guard removal -- predicted,
// then measured** (the second time the rule 0.110.0 wrote down has been used
// to predict a *non*-detection, after `LinearLight`'s). With the guard gone
// `cb` is `0.0/0.0`, a `NaN`. The burn arm computes
// `1 - min(1, (1 - NaN)/cs)`, and `min(1, NaN)` returns `1.0` on this backend,
// so `b` is a finite `0.0`; the dodge arm computes `min(1, NaN/(1 - cs))`,
// likewise a finite `1.0`. **Both arms launder**, so the detector count stays
// at five (`Multiply`, `Screen`, `Difference`, `Overlay`, `HardLight`).
//
// **Near misses, in decreasing order of how easy the slip is:**
//
//   - **Dropping the `2.0 *` in the burn branch computes `ColorBurn(Cb, Cs)`
//     there** -- and `ColorBurn` is a live GPU entry point and a live dispatch
//     arm. Detectable only in a burn channel that is clamp-*interior*, since
//     `Cb + 2*Cs <= 1` implies `Cb + Cs <= 1`: a lower-railed burn channel is
//     `0.0` for both.
//   - **Passing `2.0 * cs` where the dodge branch needs `2.0 * cs - 1.0`**
//     makes `color_dodge_channel`'s divisor `1 - 2*Cs`, which is **negative**
//     for every `Cs > 0.5` -- the whole domain of that branch. So
//     `min(1, Cb / negative)` is that negative quotient, and the branch emits
//     out-of-range negative colour in every channel with `Cb > 0`. **Corrected
//     in this round from a wrong prediction, which is worth recording:** the
//     first analysis had this collapsing to a constant `1.0` via the
//     `cs == 1.0` guard, on the assumption that `2*Cs` would be clamped. It is
//     not clamped, and that guard can only fire at `Cs == 0.5`, which belongs
//     to the *other* branch. The consequence is that this mutation is far
//     easier to kill than predicted -- **every** dodge channel sees it, railed
//     or interior -- and every fixture below did kill it, measured.
//   - **Swapping the two branches**, or **branching on `cb` instead of
//     `s.rgb`** -- the latter is `fs_composite_overlay`'s relationship to
//     `fs_composite_hard_light` reappearing here, except that the mode a
//     `cb`-branch computes is not itself a named PSD mode, so it degrades to
//     nonsense rather than to another shipped formula.
//   - **A `fragment_entry` naming `fs_composite_color_burn` or
//     `fs_composite_color_dodge`** -- the two entry points this one *calls*,
//     both live. Detectable in every channel whose branch differs from the
//     named mode's own answer; see the suite header for which fixture covers
//     which.
//
fn vivid_light_channel(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return color_burn_channel(cb, 2.0 * cs);
    }
    return color_dodge_channel(cb, 2.0 * cs - 1.0);
}

// Mirrors `aurora_render::composite_layer_into` (src/composite.rs) exactly,
// for `BlendMode::VividLight` only -- the **thirteenth** blend mode ported to
// the GPU (0.114.0), and the first whose blend term is built entirely out of
// two *other ported modes'* helpers rather than out of arithmetic.
//
// Read `fs_composite_multiply`'s own comment for the full derivation of the
// surrounding "over": the alpha compositing around `B(Cb, Cs)` is
// blend-mode-independent, so only the `b = ...` block below differs. Read
// `vivid_light_channel` directly above for this mode's own formula, its four
// inherited guard-reachability results, its asymmetry class, its five
// degeneracies, its boundary mutation (unkillable at this suite's tolerance,
// not in principle -- see the derivation above) and its near-miss table.
//
// Shares `backdrop_tex` (binding 3), the `Opacity` uniform (binding 2) and
// `TileCompositor::bind_group_layout_blend` with the twelve entry points
// above; no new binding, no new layout.
@fragment
fn fs_composite_vivid_light(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(src_tex, src_smp, in.uv);
    let bd = textureSample(backdrop_tex, src_smp, in.uv);
    let cb = straight_backdrop(bd);
    // blend_channel(VividLight, cb, cs): ColorBurn(cb, 2*cs) where cs <= 0.5,
    // else ColorDodge(cb, 2*cs - 1). The branch tests the SOURCE. Three calls
    // rather than a select(), because both callees' guards are early returns.
    let b = vec3<f32>(
        vivid_light_channel(cb.r, s.r),
        vivid_light_channel(cb.g, s.g),
        vivid_light_channel(cb.b, s.b),
    );
    return fold_over(s, bd, b);
}
