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
    return vec4<f32>(c.rgb + bg * (1.0 - c.a), 1.0);
}
