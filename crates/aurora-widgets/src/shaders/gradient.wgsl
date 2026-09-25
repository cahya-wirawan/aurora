// Vertex-coloured gradient renderer: draws an aurora_vector::ColorMesh,
// one straight (unpremultiplied) sRGB-gamma-encoded RGBA colour per
// vertex, interpolated across each triangle. No colour of its own (the
// colours are the caller's content, carried in the vertex buffer).
//
// Interpolation always happens on the gamma-encoded values (the rasterizer
// interpolates exactly what the vertex shader emits). On an sRGB-aware
// target the fragment is then linearized, so the hardware's own re-encode
// lands back on the interpolated gamma value; the Rust side picks the
// fragment entry point from the target format, never the caller.

struct Uniforms {
    // The render target's size in the same pixel space the mesh's
    // positions use (origin top-left, y-down).
    viewport_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Screen-space (non-perspective) interpolation: this is 2-D UI with
    // w == 1 at every vertex, so perspective-correct and linear agree in
    // exact arithmetic; `linear` says so explicitly and skips the divide.
    @location(0) @interpolate(linear) color: vec4<f32>,
};

@vertex
fn vs_gradient(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (input.position.x / uniforms.viewport_size.x) * 2.0 - 1.0;
    // Pixel space is y-down, clip space y-up (same maths as vs_path).
    let ndc_y = 1.0 - (input.position.y / uniforms.viewport_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.color = input.color;
    return out;
}

// The IEC 61966-2-1 decode, numerically equivalent to
// aurora_color::srgb_to_linear for finite inputs (sign-symmetric, same
// 0.04045 threshold, 12.92 slope and 2.4 exponent; GPU `pow` is not
// correctly rounded, so "equivalent" means within a few ULP, not
// bit-identical). A finite out-of-range channel is decoded, not clamped,
// exactly as the solid path's CPU-side linearization does.
fn srgb_to_linear(c: f32) -> f32 {
    let m = abs(c);
    let l = select(pow((m + 0.055) / 1.055, 2.4), m / 12.92, m <= 0.04045);
    return sign(c) * l;
}

// Non-sRGB target: the stored byte is the interpolated gamma value.
@fragment
fn fs_gradient(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}

// sRGB-aware target: linearize so the hardware's encode on write gives
// back the interpolated gamma value. Alpha is a blend coefficient, not a
// gamma-encoded sample, so it passes through.
@fragment
fn fs_gradient_srgb_target(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(
        srgb_to_linear(input.color.r),
        srgb_to_linear(input.color.g),
        srgb_to_linear(input.color.b),
        input.color.a,
    );
}
