// Task 142: one mip level of the display texture from the level below it,
// the premultiplied 2 x 2 box mean (`aurora_gpu::DisplayTexture`). Drawn as
// a fullscreen triangle into level `k`, scissored to the changed tiles, with
// level `k - 1` bound as `src`. The display texture holds premultiplied
// texels (the upload premultiplies, as the atlas's does), so the plain mean
// is the alpha-weighted one, the same mean `aurora_render::downsample_quad`
// takes in f32 for the zoomed-out pyramid. Computed in f32 and stored as the
// target's f16.

@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs_mip(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_mip(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // `pos.xy` is the target texel's centre, `(x + 0.5, y + 0.5)`.
    let p = vec2<i32>(floor(pos.xy)) * 2;
    let a = textureLoad(src, p, 0);
    let b = textureLoad(src, p + vec2<i32>(1, 0), 0);
    let c = textureLoad(src, p + vec2<i32>(0, 1), 0);
    let d = textureLoad(src, p + vec2<i32>(1, 1), 0);
    return ((a + b) + (c + d)) * 0.25;
}
