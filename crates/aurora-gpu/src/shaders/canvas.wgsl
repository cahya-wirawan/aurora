// Fullscreen-triangle canvas compositor: samples the canvas texture and
// shows a checkerboard behind transparent areas, as any image editor
// does. Ported from spike/vertical-slice/src/shader.wgsl's `vs_canvas`/
// `fs_canvas` pair (real, measured -- spike/FINDINGS.md). The UI-rect
// half of that shader is deliberately not ported here: it hardcodes
// colour values, which is exactly what invariant §7.3.10 forbids in real
// UI code -- that half needs aurora-theme's token system first, which
// doesn't exist yet.

struct Canvas {
    uv_offset: vec2<f32>,
    uv_scale: vec2<f32>,
};

@group(0) @binding(0) var canvas_tex: texture_2d<f32>;
@group(0) @binding(1) var canvas_smp: sampler;
@group(0) @binding(2) var<uniform> canvas: Canvas;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle; no vertex buffer.
@vertex
fn vs_canvas(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y) * canvas.uv_scale + canvas.uv_offset;
    return out;
}

@fragment
fn fs_canvas(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(canvas_tex, canvas_smp, in.uv);
    // Checkerboard behind transparent areas, as any image editor shows.
    let sq = floor(in.pos.xy / 8.0);
    let check = select(0.18, 0.24, (sq.x + sq.y) % 2.0 == 0.0);
    let bg = vec3<f32>(check);
    // **The atlas holds PREMULTIPLIED alpha** (since 0.68.0), so this is
    // the premultiplied "over" formula: `Cs + Cb * (1 - as)`.
    //
    // Read this whole comment before "fixing" this line. It has been
    // both formulas, and each was right for its own build.
    //
    // Where the convention boundary is. `aurora-tile`'s store is
    // **straight** alpha, and so are the CPU/GPU composite surfaces --
    // that is the workspace's universal convention and none of it
    // changed. The conversion happens at *upload*, in
    // `TileResidency::sync` and `TileResidency::upload_mip`
    // (`residency.rs`'s `extend_premultiplied_le_bytes`, which both call
    // as of 0.92.1; `premultiply_rgba` is the same arithmetic in the
    // obvious scalar spelling, kept as the test-only reference and as the
    // place that rationale is written down), so the atlas texture -- and
    // only the atlas texture -- is premultiplied by the time this shader
    // samples it.
    //
    // Why it has to be there rather than here. A `textureSample` with
    // `min_filter: Linear` returns a per-channel weighted average of
    // four texels, computed by fixed-function hardware *before* this
    // line runs. Averaging straight colours weights a fully transparent
    // texel's RGB exactly as heavily as an opaque neighbour's, so a hard
    // opaque/transparent boundary drags the transparent side's arbitrary
    // colour into the visible result -- the classic minification halo.
    // No formula written here can undo that: the information is already
    // gone. Premultiplied RGB carries its own alpha weight, so the same
    // hardware average is the correct alpha-weighted one.
    //
    // The history, so the 0.52.0 bug is not reintroduced by "fixing"
    // this back. This line was `c.rgb + bg * (1.0 - c.a)` before 0.52.0,
    // and it was *accidentally* correct: `composite_roots_into_tile` and
    // `begin_gpu_composite_tile` both skipped the un-premultiply step
    // that `resolve_tile`'s `Group` arm always ran, so the composite
    // surface genuinely held premultiplied texels. Two bugs cancelled on
    // screen while the same wrong values went into every export and
    // every eyedropper read. 0.52.0 fixed those two entry points and had
    // to change this line to `c.rgb * c.a + bg * (1.0 - c.a)` in the
    // same commit, or translucent content would have rendered far too
    // bright and clipped to white.
    //
    // 0.68.0 changed it back -- but for a different reason and with a
    // different mechanism. The composite surfaces are still straight
    // (0.52.0's fix stands, untouched); what changed is that the *atlas*
    // is now premultiplied deliberately, at upload. So: if you are ever
    // tempted to restore `c.rgb * c.a + ...` here, the question to ask
    // first is whether the premultiply still runs on both upload paths
    // (`extend_premultiplied_le_bytes` since 0.92.1; `premultiply_rgba`
    // before that). If it does, this line is correct as written and
    // changing it
    // will double-count alpha in the other direction (translucent
    // content rendering too dark).
    //
    // Two tests in render_test.rs pin this line, and they fail for
    // different reasons on purpose:
    // `canvas_pipeline_blends_a_translucent_tile_against_the_checkerboard`
    // catches the formula and the upload drifting apart (a uniform
    // texel, where premultiply-then-premultiplied-over and straight-over
    // are mathematically identical, so it can only fail if exactly one
    // of the two halves changed), and
    // `canvas_pipeline_does_not_bleed_transparent_black_across_a_hard_alpha_edge`
    // is the negative control for the *domain* -- it fails against the
    // straight-domain formula, measured, not assumed.
    return vec4<f32>(c.rgb + bg * (1.0 - c.a), 1.0);
}
