// Canvas and UI are drawn by the same device into the same frame (PRD §7.3.8).
// Two pipelines, one render pass, no interop layer and no compositing seam.

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

// --- UI ---------------------------------------------------------------------
// Rects in normalized device coords, colours already resolved. In the real
// toolkit these come from design tokens (PRD FR-027); here they are hardcoded,
// which is exactly the thing invariant §7.3.10 forbids in production.

struct Rect {
    ndc: vec4<f32>,      // x0, y0, x1, y1
    colour: vec4<f32>,   // straight alpha
};

@group(0) @binding(0) var<uniform> rects: array<Rect, 32>;

struct UiOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_ui(@builtin(vertex_index) v: u32, @builtin(instance_index) inst: u32) -> UiOut {
    let r = rects[inst];
    // Two triangles per rect.
    var xs = array<f32, 6>(r.ndc.x, r.ndc.z, r.ndc.x, r.ndc.x, r.ndc.z, r.ndc.z);
    var ys = array<f32, 6>(r.ndc.y, r.ndc.y, r.ndc.w, r.ndc.w, r.ndc.y, r.ndc.w);
    var out: UiOut;
    out.pos = vec4<f32>(xs[v], ys[v], 0.0, 1.0);
    out.colour = r.colour;
    return out;
}

@fragment
fn fs_ui(in: UiOut) -> @location(0) vec4<f32> {
    return in.colour;
}
