// Glyph renderer: draws textured quads whose coverage comes from an R8
// glyph atlas. No hardcoded colour here (invariant §7.3.10) -- `color` is
// a per-vertex attribute carrying the run's `text.*` design-token colour,
// resolved by the caller; every run of a frame shares one vertex buffer.
//
// Quads are placed on whole physical pixels and never scaled, so each
// fragment maps to exactly one atlas texel: `uv` is in texel units and the
// fragment reads it with `textureLoad` (no sampler, no filtering).

struct Uniforms {
    // The render target's size in the same logical pixel units the quad
    // vertices are in (see `path.wgsl`).
    viewport_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Flat: every vertex of a quad carries the same run colour, so the
    // provoking vertex's value is exact (no interpolation rounding).
    @location(1) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_text(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (input.position.x / uniforms.viewport_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (input.position.y / uniforms.viewport_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    return out;
}

@fragment
fn fs_text(input: VertexOutput) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(floor(input.uv));
    let coverage = textureLoad(atlas, texel, 0).r;
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
