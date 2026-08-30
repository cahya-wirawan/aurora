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
    // **The atlas holds straight alpha**, the tile store's universal
    // convention everywhere else in the workspace (a 50%-alpha white
    // texel is (1, 1, 1, 0.5), not the premultiplied (0.5, 0.5, 0.5,
    // 0.5)), so this is the straight-alpha "over" formula:
    // `Cs * as + Cb * (1 - as)`. This is the one place in the codebase
    // where straight alpha is converted for display.
    //
    // It used to be `c.rgb + bg * (1.0 - c.a)` -- the *premultiplied*
    // "over" formula -- and that was accidentally correct, because until
    // 0.52.0 `composite_roots_into_tile` and `begin_gpu_composite_tile`
    // both skipped the un-premultiply step that `resolve_tile`'s own
    // `Group` arm had always run, so the composite surface really did
    // hold premultiplied texels. Two bugs cancelled on screen while the
    // same wrong values went straight into every export and every
    // eyedropper read. Fixing those two entry points without also
    // fixing this line would have turned an accidentally-correct display
    // into a visibly wrong one (translucent content rendering too bright,
    // clipping to white), which is why all three landed together --
    // `canvas_pipeline_blends_a_translucent_tile_against_the_checkerboard`
    // in render_test.rs is the test that catches this line being missed.
    return vec4<f32>(c.rgb * c.a + bg * (1.0 - c.a), 1.0);
}
