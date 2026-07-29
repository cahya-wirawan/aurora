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
