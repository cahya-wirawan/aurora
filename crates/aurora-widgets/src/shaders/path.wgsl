// Solid-fill vector path renderer: draws a tessellated aurora_vector::Mesh
// (a real vertex/index buffer this time, unlike aurora-gpu's own
// fullscreen-triangle shaders) at one flat colour per draw call. No
// hardcoded colour here (invariant §7.3.10) -- `color` is a real per-draw
// uniform, resolved from a design token by whichever caller sets up the
// bind group; this shader has no opinion on what that colour is.

struct Uniforms {
    // The render target's own size in physical pixels -- what a vertex
    // position (in the same pixel space aurora_vector::Path was built in,
    // origin top-left, y-down) is converted against to reach clip space.
    viewport_size: vec2<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_path(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (input.position.x / uniforms.viewport_size.x) * 2.0 - 1.0;
    // Flips y: pixel space is y-down (origin top-left, matching every
    // other document/screen-space convention this project already uses),
    // clip space is y-up.
    let ndc_y = 1.0 - (input.position.y / uniforms.viewport_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_path(input: VertexOutput) -> @location(0) vec4<f32> {
    return uniforms.color;
}
