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
