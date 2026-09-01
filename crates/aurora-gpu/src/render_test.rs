//! End-to-end proof that [`crate::CanvasPipeline`] and
//! [`crate::TileResidency`] together actually draw correct pixels — not
//! just "it compiled," a real rendered-output check, matching this
//! project's general practice of measuring rather than assuming
//! (`spike/FINDINGS.md`, `aurora-tile`'s bit-exact round-trip tests).
//! Exercises the exact same real types `aurora-app`'s own canvas
//! rendering does, not a hand-rolled duplicate of them.

#![cfg(test)]

use crate::test_support::real_context;
use crate::{CanvasPipeline, TileResidency};
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;
use std::num::NonZeroUsize;

/// A 256×256 viewport -- one tile exactly, so a solid tile fills the
/// whole rendered output and the expected pixel colour is unconditional
/// (no partial-tile/checkerboard-edge cases to account for).
const VIEWPORT: (u32, u32) = (256, 256);

/// A 300×300 viewport — **the viewport every minification test below
/// has to use**, and the reason is worth stating once here.
///
/// The atlas is sized `viewport.div_ceil(TILE) + 1` tiles, so
/// `TileResidency::min_zoom_for_viewport` (the floor below which the
/// canvas renders at the floor rather than shrinking further) is
/// `viewport / (viewport rounded up to whole tiles)`. For [`VIEWPORT`],
/// a whole number of tiles, that is exactly **1.0** — minification is
/// unreachable there, and a test that asks for zoom 0.5 on it is
/// silently testing zoom 1.0 instead. 300 px rounds up to two tiles, so
/// the floor is `300 / 512 = 0.5859`, and everything from there up to
/// 1.0 genuinely minifies (up to 1.7 atlas texels per screen pixel,
/// LOD 0.77 — comfortably past the LOD-0.5 boundary where the sampler
/// used to cross into an unwritten mip level).
const MINIFYING_VIEWPORT: (u32, u32) = (300, 300);

/// [`MINIFYING_VIEWPORT`]'s own zoom floor, `300 / 512`, spelled out so
/// the tests below can say which side of it a zoom sits on.
const MINIFYING_FLOOR: f32 = 300.0 / 512.0;

/// Eight evenly spaced sample points across [`MINIFYING_VIEWPORT`], each
/// the centre of its own eighth of the frame and all far from any tile
/// boundary at the scales these tests render at.
fn eight_columns_across() -> Vec<(u32, u32)> {
    (0..8).map(|i| (18 + i * 37, 64)).collect()
}

fn tile_store() -> (tempfile::TempDir, TileStore) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
    };
    let Some(budget) = NonZeroUsize::new(4) else {
        unreachable!("4 is non-zero");
    };
    let store = match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("scratch dir just created by tempfile must be usable: {err}"),
    };
    (dir, store)
}

/// Fills tile `id` on `surface` with a solid, straight `rgba` colour —
/// shared by [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`]'s
/// two differently-coloured tiles.
fn paint(store: &mut TileStore, surface: SurfaceId, id: TileId, rgba: [f32; 4]) {
    let tile = match store.get_mut(surface, id) {
        Ok(tile) => tile,
        Err(err) => unreachable!("a fresh store must accept this: {err}"),
    };
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let Some(&channel) = rgba.get(i % 4) else {
            unreachable!("i % 4 is always in range 0..4");
        };
        *sample = f16::from_f32(channel);
    }
}

#[test]
// One linear setup-render-readback flow for a single real GPU test --
// splitting it into helper functions would just relocate the same lines
// without reducing the actual complexity of what a real render pass
// needs (store, residency, pipeline, bind group, pass, readback).
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_renders_a_real_residencys_tile_correctly() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    // Solid, opaque green -- with alpha = 1, the checkerboard-behind-
    // transparency logic must contribute nothing, so the expected
    // output is exactly this colour, unconditionally.
    {
        let tile = match store.get_mut(surface, TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("a fresh store must accept this: {err}"),
        };
        for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
            let channel = match i % 4 {
                1 | 3 => 1.0,
                _ => 0.0,
            };
            *sample = f16::from_f32(channel);
        }
    }

    let mut residency = TileResidency::new(device, queue, VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 4,
        "a 256x256 viewport needs a 2x2 slot grid"
    );
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let bind_group = canvas.bind_group(device, &residency);
    let pipeline = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    let bytes_per_row = VIEWPORT.0 * 4;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(VIEWPORT.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(VIEWPORT.1),
            },
        },
        wgpu::Extent3d {
            width: VIEWPORT.0,
            height: VIEWPORT.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });

    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };

    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    // Checks the center pixel, not the corner: the atlas is padded to a
    // 2x2-tile (512x512) texture but the render target/viewport is only
    // 256x256, so a center sample is safely away from any UV-wrapping
    // edge case at the sampled region's own boundary.
    let center_row = (VIEWPORT.1 / 2) as usize;
    let center_col = (VIEWPORT.0 / 2) as usize;
    let offset = center_row * (bytes_per_row as usize) + center_col * 4;
    let Some(pixel) = data.get(offset..offset + 4) else {
        unreachable!("offset is well within the readback buffer's own bounds");
    };
    match pixel {
        [r, g, b, a] => {
            assert_eq!(*r, 0, "red channel");
            assert_eq!(*g, 255, "green channel");
            assert_eq!(*b, 0, "blue channel");
            assert_eq!(
                *a, 255,
                "alpha channel (checkerboard must not show through opaque content)"
            );
        }
        _ => unreachable!("sliced exactly 4 bytes"),
    }
    drop(data);
    readback_buffer.unmap();
}

/// Reads back one pixel from a fresh render of `residency` through
/// `canvas` into a `target_size` render target — the readback half of
/// `canvas_pipeline_renders_a_real_residencys_tile_correctly` above,
/// factored out since [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`]
/// below needs it twice (once per zoom level) and duplicating the full
/// encoder/pass/readback sequence a second time would be real
/// repetition, unlike that test's own single linear pass.
fn render_and_sample_pixel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut CanvasPipeline,
    residency: &TileResidency,
    target_size: (u32, u32),
    sample: (u32, u32),
) -> [u8; 4] {
    let sampled =
        render_and_sample_pixels(device, queue, canvas, residency, target_size, &[sample]);
    match sampled.first() {
        Some(&pixel) => pixel,
        None => unreachable!("exactly one sample point was requested"),
    }
}

/// The many-points form of [`render_and_sample_pixel`]: renders **one**
/// frame and reads several pixels out of it.
///
/// This exists because a single centre-pixel check is structurally blind
/// to a whole class of bug — anything where the canvas shows real,
/// plausible content at the centre while the content *across* the
/// viewport is wrong (duplicated, mis-ordered, or clamped). Reading many
/// points from the same frame is also the only way to compare them
/// meaningfully: separate renders would each be a separate frame.
fn render_and_sample_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut CanvasPipeline,
    residency: &TileResidency,
    target_size: (u32, u32),
    samples: &[(u32, u32)],
) -> Vec<[u8; 4]> {
    let bind_group = canvas.bind_group(device, residency);
    let pipeline = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: target_size.0,
            height: target_size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    // Padded up to `COPY_BYTES_PER_ROW_ALIGNMENT` (256): wgpu rejects a
    // texture-to-buffer copy whose row stride is not a multiple of it,
    // and a viewport width that is not a multiple of 64 pixels
    // (64 * 4 bytes) has one. Every offset below is computed from this
    // padded stride, so the padding is invisible to callers -- it is
    // what lets a test pick a viewport for its *geometry* (e.g. 300 px,
    // the only way to reach a minifying zoom now that the atlas clamps
    // zoom to whole-tile coverage) rather than for its row alignment.
    let bytes_per_row = (target_size.0 * 4).div_ceil(256) * 256;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bytes_per_row) * u64::from(target_size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(target_size.1),
            },
        },
        wgpu::Extent3d {
            width: target_size.0,
            height: target_size.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let mut results = Vec::with_capacity(samples.len());
    for &(sx, sy) in samples {
        let offset = (sy as usize) * (bytes_per_row as usize) + (sx as usize) * 4;
        let Some(pixel) = data.get(offset..offset + 4) else {
            unreachable!("every sample point is well within the readback buffer's own bounds");
        };
        results.push(match pixel {
            &[r, g, b, a] => [r, g, b, a],
            _ => unreachable!("sliced exactly 4 bytes"),
        });
    }
    drop(data);
    readback_buffer.unmap();
    results
}

/// The test that catches "the compositing paths were straightened but
/// `fs_canvas` was left on the premultiplied formula" — the single most
/// likely way 0.52.0's three-layer premultiplied-alpha fix could have
/// gone wrong.
///
/// Until 0.52.0 two bugs cancelled out on screen: `aurora-app`'s
/// top-level compositing entry points left the composite surface in
/// *premultiplied* alpha, and this shader's own final line was the
/// premultiplied "over" formula (`c.rgb + bg * (1 - c.a)`), so the live
/// canvas looked approximately right while every export and every
/// eyedropper read carried the wrong colour. Straightening the
/// compositing paths without also fixing the shader would have made
/// translucent content render *too bright* — here, clipping to a fully
/// saturated 255 — so this test pins the shader half specifically.
///
/// Fixture: a solid 50%-alpha white tile, `(1.0, 1.0, 1.0, 0.5)` in
/// straight alpha, which is exactly what `composite_roots_into_tile` now
/// produces for one opaque-white layer at 50% opacity. Expected at the
/// sampled centre pixel: the straight-alpha "over" of white onto the
/// checkerboard's own lighter square (`check = 0.24`, since
/// `floor(128.5 / 8) = 16` on both axes and `(16 + 16) % 2 == 0`):
/// `1.0 * 0.5 + 0.24 * 0.5 = 0.62`, i.e. `round(0.62 * 255) = 158` in
/// the `Rgba8Unorm` render target. The pre-fix formula would give
/// `1.0 + 0.24 * 0.5 = 1.12`, clamped to `255` — asserted against
/// explicitly below.
///
/// A tolerance of 1 (out of 255), not exact equality: unlike the
/// fully-opaque fixtures the tests around this one use, `0.24` is not an
/// exact binary fraction, so the last unit of the 8-bit quantization is
/// genuinely at the mercy of the driver's own rounding. The two
/// candidate values here differ by ~97 units, so a tolerance of 1
/// discriminates them with enormous margin.
#[test]
fn canvas_pipeline_blends_a_translucent_tile_against_the_checkerboard() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [1.0, 1.0, 1.0, 0.5],
    );

    let mut residency = TileResidency::new(device, queue, VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let pixel = render_and_sample_pixel(
        device,
        queue,
        &mut canvas,
        &residency,
        VIEWPORT,
        (VIEWPORT.0 / 2, VIEWPORT.1 / 2),
    );

    let [r, g, b, a] = pixel;
    // 0.5 * 1.0 + 0.5 * 0.24 = 0.62 -> 158/255.
    let expected = 158_i32;
    for (channel, label) in [(r, "red"), (g, "green"), (b, "blue")] {
        let delta = i32::from(channel) - expected;
        assert!(
            delta.abs() <= 1,
            "{label} channel: expected ~{expected} (straight-alpha white over the 0.24 \
             checkerboard square), got {channel}"
        );
        assert!(
            channel < 250,
            "{label} channel clipped to {channel}: fs_canvas is still using the \
             premultiplied \"over\" formula, which now double-counts alpha and renders \
             translucent content far too bright"
        );
    }
    assert_eq!(a, 255, "the canvas shader always writes an opaque result");
}

#[test]
/// The real, visible proof of PLAN.md M1.9's "make zoom visible" work:
/// `TileResidency::set_origin`'s new `zoom` parameter must actually
/// change what the canvas shader draws, not just accept the argument.
///
/// Two adjacent, differently-coloured tiles (green at `(0, 0)`, red at
/// `(1, 0)`), a residency/render-target wide enough (512 px, exactly
/// two tiles) to show both side by side at 100% zoom. At 100% zoom, a
/// sample point near the right edge lands in the red tile; at 200%
/// zoom, `uv_scale` halves (this test's real point), so the same origin
/// now shows only the *left* half of that same atlas region stretched
/// across the whole viewport — the same screen-space sample point that
/// used to land in the red tile now lands back in the green one.
fn canvas_pipeline_reflects_zoom_by_magnifying_the_atlas() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [0.0, 1.0, 0.0, 1.0],
    );
    paint(
        &mut store,
        surface,
        TileId { x: 1, y: 0 },
        [1.0, 0.0, 0.0, 1.0],
    );

    // A 512x256 viewport spans exactly the two painted tiles -- grid
    // becomes (3, 2) (the usual "+1" margin column/row), atlas 768x512.
    let viewport = (512, 256);
    let mut residency = TileResidency::new(device, queue, viewport);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 6, "a 3x2 slot grid");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);

    // Near the right edge of a 512-wide target: at 100% zoom this is
    // deep inside the red tile (uv_scale spans the full two-tile width,
    // so screen x=480/512 maps into atlas x ~ 720, past the 512 boundary
    // between tile 0 and tile 1).
    let sample = (480, 128);
    residency.set_origin(queue, (0.0, 0.0), viewport, 1.0);
    let at_100_percent =
        render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        at_100_percent,
        [255, 0, 0, 255],
        "at 100% zoom the same sample point must show the red tile"
    );

    // At 200% zoom, uv_scale halves -- the same origin now shows only
    // the atlas's own first 256 (of 768) texels stretched across the
    // full 512px target, so screen x=480 maps into atlas x ~ 360, still
    // inside tile 0 (green), not tile 1.
    residency.set_origin(queue, (0.0, 0.0), viewport, 2.0);
    let at_200_percent =
        render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        at_200_percent,
        [0, 255, 0, 255],
        "at 200% zoom the same screen point must now show the green tile -- \
         zoom must actually magnify, not just accept the argument"
    );
}

/// The money test for the sub-tile fractional-scroll fix (real symptoms
/// Cahya reported interactively: painted content landing offset from the
/// cursor after zoom/pan, and panning under one tile not visibly moving
/// anything). Same two-adjacent-tiles setup as
/// [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`] above, but
/// exercising `set_origin`'s own fractional axis instead of `zoom`: a
/// `doc_origin` of exactly half a tile (128 of 256px) must shift the
/// rendered tile boundary by exactly that many screen pixels, not snap to
/// the nearest whole-tile boundary (the pre-fix behaviour, equivalent to
/// `doc_origin (0, 0)` regardless of the fractional part).
#[test]
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_reflects_a_sub_tile_fractional_pan() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [0.0, 1.0, 0.0, 1.0],
    );
    paint(
        &mut store,
        surface,
        TileId { x: 1, y: 0 },
        [1.0, 0.0, 0.0, 1.0],
    );

    let viewport = (512, 256);
    let mut residency = TileResidency::new(device, queue, viewport);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 6, "a 3x2 slot grid");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);

    // At `doc_origin (0, 0)`, screen x=200 samples atlas x=200 -- inside
    // tile 0 (green, atlas [0, 256)).
    let sample = (200, 128);
    residency.set_origin(queue, (0.0, 0.0), viewport, 1.0);
    let unshifted =
        render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        unshifted,
        [0, 255, 0, 255],
        "baseline: screen x=200 with no pan must show the green tile"
    );

    // `doc_origin` shifted by exactly half a tile (128px, deliberately
    // *not* a whole-tile multiple): the same screen x=200 now samples
    // atlas x = 128 + 200 = 328 -- past the 256 tile boundary, inside
    // tile 1 (red). A floor-to-tile implementation (the pre-fix bug)
    // would discard the 128px fractional remainder entirely and render
    // as if `doc_origin` were still `(0, 0)`, leaving this sample green.
    let half_tile = TILE as f32 / 2.0;
    residency.set_origin(queue, (half_tile, 0.0), viewport, 1.0);
    let shifted = render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        shifted,
        [255, 0, 0, 255],
        "a half-tile doc_origin must shift the rendered content by exactly \
         that many pixels, landing this sample in the red tile -- not \
         silently snapped back to the nearest whole tile boundary"
    );
}

/// The regression backstop: a `doc_origin` that's an exact multiple of
/// `TILE` (so `sub_tile` works out to `(0.0, 0.0)`) must render bit-
/// identically to the old, `TileId`-typed `set_origin` API -- proving the
/// refactor didn't perturb the already-correct whole-tile case, only
/// added real behaviour for the fractional one.
/// [`canvas_pipeline_reflects_zoom_by_magnifying_the_atlas`] above already
/// covers `doc_origin (0, 0)` (trivially a multiple of `TILE`); this
/// covers a *non-zero* whole-tile shift, the case
/// `residency.rs`'s own `toroidal_addressing_uploads_only_the_newly_exposed_column`
/// test exercises for upload bookkeeping but nothing GPU-renders.
#[test]
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_with_a_whole_tile_doc_origin_matches_the_pre_refactor_tileid_behaviour() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [0.0, 1.0, 0.0, 1.0],
    );
    paint(
        &mut store,
        surface,
        TileId { x: 1, y: 0 },
        [1.0, 0.0, 0.0, 1.0],
    );

    let viewport = (512, 256);
    let mut residency = TileResidency::new(device, queue, viewport);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 6, "a 3x2 slot grid");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);

    // `doc_origin (TILE, 0)` -- one whole tile of pan, zero sub-tile
    // remainder -- must behave exactly like the old `TileId { x: 1, y: 0
    // }` `set_origin` call would have: tile 1's own content (red) now
    // starts at the canvas's own top-left corner.
    residency.set_origin(queue, (TILE as f32, 0.0), viewport, 1.0);
    let sample = (10, 128);
    let pixel = render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        pixel,
        [255, 0, 0, 255],
        "a whole-tile doc_origin must show tile 1's own colour starting at \
         the canvas's own top-left corner, matching the pre-refactor \
         TileId-typed API exactly"
    );
}

/// Mirrors the actual reported bug shape: a pan smaller than one full
/// `TILE` (50 of 256px) must produce a *visible* rendering change, not
/// nothing -- the "painting doesn't move while panning" symptom
/// specifically (as opposed to
/// [`canvas_pipeline_reflects_a_sub_tile_fractional_pan`] above, which
/// proves the shift is *exactly* right; this proves a small, realistic
/// drag increment isn't silently swallowed the way the pre-fix
/// floor-to-tile implementation swallowed anything under one `TILE`).
#[test]
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_a_small_sub_tile_pan_visibly_moves_the_content() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    paint(
        &mut store,
        surface,
        TileId { x: 0, y: 0 },
        [0.0, 1.0, 0.0, 1.0],
    );
    paint(
        &mut store,
        surface,
        TileId { x: 1, y: 0 },
        [1.0, 0.0, 0.0, 1.0],
    );

    let viewport = (512, 256);
    let mut residency = TileResidency::new(device, queue, viewport);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 6, "a 3x2 slot grid");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);

    // Sample just inside the green tile, close enough to the tile
    // boundary (atlas x=256) that a 50px pan crosses it: at doc_origin
    // (0, 0), screen x=230 samples atlas x=230 (green); a 50px pan moves
    // that same screen point to atlas x=280 (red).
    let sample = (230, 128);
    residency.set_origin(queue, (0.0, 0.0), viewport, 1.0);
    let before = render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        before,
        [0, 255, 0, 255],
        "baseline must show the green tile"
    );

    let pan_step_px = 50.0;
    residency.set_origin(queue, (pan_step_px, 0.0), viewport, 1.0);
    let after = render_and_sample_pixel(device, queue, &mut canvas, &residency, viewport, sample);
    assert_eq!(
        after,
        [255, 0, 0, 255],
        "a 50px pan (well under one 256px TILE) must actually move the \
         rendered content -- the pre-fix bug reported interactively as \
         \"panning doesn't move anything until a large drag, then jumps\""
    );
    assert_ne!(
        before, after,
        "a sub-tile pan must visibly change the rendered pixel, not leave \
         it snapped to the pre-pan tile boundary"
    );
}

/// The money test for "the canvas goes pure checkerboard when you zoom
/// out" — reported as "zoom 0.5 renders pure checkerboard", but the real
/// boundary is not 0.5 at all.
///
/// `write_uniform` sets `uv_scale = viewport_px / zoom / tex_size`, so
/// the atlas covers `1/zoom` texels per screen pixel and the hardware's
/// own computed LOD is `log2(1/zoom) = -log2(zoom)`. The sampler rounds
/// that to the nearest whole mip level, so it crossed from level 0 to
/// level 1 at LOD 0.5 — `zoom = 2^-0.5 ≈ 0.7071`. *Every* zoom below
/// ~71% was affected, not 0.5 specifically. The atlas carries
/// `MIP_LEVELS` = 4 levels but only level 0 is ever written live
/// (`sync` writes `mip_level: 0`; `upload_mip` has no call site in
/// `aurora-app`), and wgpu lazily zero-initializes what was never
/// written — so the sampler read `(0, 0, 0, 0)`, `fs_canvas`'s
/// straight-alpha "over" collapsed to pure background, and the user saw
/// checkerboard where their document should be.
///
/// Runs on [`MINIFYING_VIEWPORT`], not [`VIEWPORT`]: since 0.57.3 the
/// atlas clamps zoom to `min_zoom_for_viewport`, which for a
/// whole-number-of-tiles viewport is exactly 1.0 — on `VIEWPORT` this
/// sweep would silently render every entry at zoom 1.0 and prove
/// nothing about minification. On a 300 px viewport the floor is
/// 0.5859, so the entries below it render at 0.5859 (1.7 texels per
/// pixel, LOD 0.77) and the entries above it render at themselves;
/// either way every one of them is genuinely minified and still past
/// the LOD-0.5 boundary this test exists to guard.
///
/// All nine tiles of the 3×3 slot grid are painted, not just `(0, 0)`:
/// at these zooms the window spans more than one tile, and
/// `min_filter: Linear` can pull from any neighbouring slot, so a
/// single painted tile would make the result depend on slot-boundary
/// filtering rather than on the bug under test.
#[test]
fn canvas_pipeline_shows_tile_content_at_minifying_zoom_levels() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    let green = [0.0, 1.0, 0.0, 1.0];
    for y in 0..3 {
        for x in 0..3 {
            paint(&mut store, surface, TileId { x, y }, green);
        }
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 9,
        "a 300x300 viewport needs a 3x3 slot grid"
    );
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let centre = (MINIFYING_VIEWPORT.0 / 2, MINIFYING_VIEWPORT.1 / 2);

    // 1.0/0.99/0.75 sit above the LOD-0.5 rounding boundary and always
    // worked; everything from 0.7071 down sits on or below it and
    // rendered pure checkerboard. `2^-0.5` and `0.70` bracket the
    // boundary itself as tightly as f32 usefully allows -- the exact
    // crossing point is the one value a coarse sweep would step over.
    // `0.01` is `aurora_ui::CanvasView::MIN_ZOOM` (spelled as a literal:
    // `aurora-ui` sits *above* this crate in the layering, so this crate
    // cannot depend on it -- PRD 7.2), the extreme end of the range a
    // user can actually reach.
    for zoom in [
        1.0_f32,
        0.99,
        0.75,
        2.0_f32.powf(-0.5),
        0.70,
        0.6,
        0.5,
        0.25,
        0.01,
    ] {
        residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, zoom);
        let pixel = render_and_sample_pixel(
            device,
            queue,
            &mut canvas,
            &residency,
            MINIFYING_VIEWPORT,
            centre,
        );
        assert_eq!(
            pixel,
            [0, 255, 0, 255],
            "at zoom {zoom} (rendered at {}) the canvas must show the painted \
             opaque green tile. A value near [61, 61, 61, 255] is the \
             checkerboard's lighter square at this pixel, i.e. *no content at \
             all* rather than a wrong colour -- that is the pre-fix symptom of \
             the sampler selecting an unwritten, zero-initialized mip level",
            TileResidency::effective_zoom(MINIFYING_VIEWPORT, zoom)
        );
    }
}

/// The mechanism-discriminating half of the pair above: positive proof
/// that at zoom 0.5 the sampler was reading mip level *1*, rather than
/// some other source of transparency (a missed upload, a bad slot
/// address, a wrong `uv_offset`).
///
/// Level 0 is painted green by `sync`; level 1 is then filled magenta by
/// hand via `upload_mip`. Before the fix this sampled magenta — the
/// sampler really was selecting level 1. After the fix it samples green,
/// because the view bound for sampling exposes level 0 and nothing else
/// (`mip_level_count: Some(1)`), so level selection has nowhere to go.
///
/// On [`MINIFYING_VIEWPORT`] for the reason that constant documents: a
/// zoom of 0.5 on [`VIEWPORT`] is clamped to 1.0, which magnifies, and
/// a magnifying access never selects a lower level at all — the test
/// would pass with the bug fully present.
///
/// This deliberately pins *today's policy* — the atlas is sampled at mip
/// 0 only, because mips 1-3 are never populated in the live app. If
/// progressive/LOD rendering is ever wired in (PLAN.md M1.3), this test
/// is expected to fail, and that failure is the signal to widen the
/// view's `mip_level_count` and choose a `mipmap_filter` deliberately
/// rather than by accident.
#[test]
fn canvas_pipeline_samples_mip_level_zero_not_a_lower_level() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    let mut ids = Vec::new();
    for y in 0..3 {
        for x in 0..3 {
            ids.push(TileId { x, y });
        }
    }
    for id in &ids {
        paint(&mut store, surface, *id, [0.0, 1.0, 0.0, 1.0]);
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 9);
    assert_eq!(stats.errors, 0);

    // Mip level 1 is a half-size tile: (TILE/2)^2 texels, CHANNELS each.
    let half = (TILE as usize) / 2;
    let mut magenta = Vec::with_capacity(half * half * CHANNELS);
    for i in 0..(half * half * CHANNELS) {
        let channel = match i % 4 {
            1 => 0.0,
            _ => 1.0,
        };
        magenta.push(f16::from_f32(channel));
    }
    for id in &ids {
        if let Err(err) = residency.upload_mip(queue, *id, 1, &magenta) {
            unreachable!("a level-1 upload of the exact expected size must succeed: {err}");
        }
    }

    let mut canvas = CanvasPipeline::new(device);
    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.5);
    let pixel = render_and_sample_pixel(
        device,
        queue,
        &mut canvas,
        &residency,
        MINIFYING_VIEWPORT,
        (MINIFYING_VIEWPORT.0 / 2, MINIFYING_VIEWPORT.1 / 2),
    );
    assert_eq!(
        pixel,
        [0, 255, 0, 255],
        "at zoom 0.5 the sampler must read mip level 0 (green). \
         [255, 0, 255, 255] means it selected level 1 (the magenta this test \
         wrote there), which is the exact mechanism behind the pure-checkerboard \
         bug -- live, level 1 is never written and reads as transparent black"
    );
}

/// Paints tile `id` a colour that *encodes its own document position*,
/// so a rendered pixel can be traced back to the tile it came from:
/// red = `x / 8`, green = `y / 8`, blue = 1, alpha = 1.
///
/// Blue is pinned at full for every tile so it doubles as an
/// unambiguous "this pixel is real content" marker: `fs_canvas`'s
/// empty-canvas checkerboard is a low, neutral grey on every channel, so
/// a blue channel near 255 cannot be checkerboard no matter how the two
/// checkerboard shades happen to quantize.
///
/// Every existing canvas test in this file paints one uniform colour
/// across every tile, which makes them all structurally incapable of
/// seeing *which* tile a pixel came from — the exact blind spot the
/// duplication test below exists to close.
fn paint_position_encoded(store: &mut TileStore, surface: SurfaceId, id: TileId) {
    #[allow(clippy::cast_precision_loss)]
    let rgba = [(id.x as f32) / 8.0, (id.y as f32) / 8.0, 1.0, 1.0];
    paint(store, surface, id, rgba);
}

/// Recovers the tile x index a sampled pixel's red channel encodes, the
/// inverse of [`paint_position_encoded`]. Distinct tiles land 32 8-bit
/// levels apart, far more than any rounding or filtering slop.
fn decode_tile_x(pixel: [u8; 4]) -> u32 {
    let Some(&red) = pixel.first() else {
        unreachable!("a sampled pixel always has four channels");
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = (f32::from(red) / 255.0 * 8.0).round() as u32;
    index
}

/// The canvas must never show the *same* document region twice across
/// one frame — the "zoomed out and the document is tiled/duplicated"
/// bug, which is a different defect from the mip-level checkerboard the
/// tests above cover, and one the checkerboard was previously hiding.
///
/// Mechanism: `TileResidency`'s atlas grid is sized from the viewport
/// alone (`viewport.div_ceil(TILE) + 1`), never from zoom, while
/// `write_uniform`'s `uv_scale = viewport_px / zoom / tex_size` grows
/// without bound as zoom falls. Once `uv_scale` exceeds the atlas's own
/// coverage the sampler's `AddressMode::Repeat` wraps and re-samples the
/// *same* resident slots, so the canvas showed a convincing, seamless,
/// completely wrong duplicate of a sub-region of the document. `sync`
/// reported `uploaded = 0, errors = 0` throughout: nothing anywhere
/// signalled that content was missing.
///
/// The threshold is `zoom < viewport_px / atlas_px`, which for a real
/// 1920 px canvas is `zoom < 0.833` — *higher* than the 0.7071 mip
/// boundary, so this was already reachable in the 0.71-0.83 band before
/// the mip fix, and would have become visible across the whole
/// zoomed-out range after it.
///
/// What this asserts is deliberately weaker than "every sample shows the
/// tile whose document position belongs there": the atlas genuinely does
/// not hold the whole document at a minifying zoom, and making it hold
/// it means sizing the atlas from zoom or wiring progressive/LOD
/// rendering — separate, larger, already-tracked M1.3 work. What it does
/// assert is the property that distinguishes an honest limitation from a
/// lie: the tile shown must be **non-decreasing** left to right, and
/// must actually advance (a frame showing one tile everywhere satisfies
/// "non-decreasing" vacuously). Real content may run out; it must never
/// wrap around and start over.
#[test]
fn canvas_pipeline_does_not_duplicate_document_content_when_zoomed_out() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    // The document has six tiles across; the 300x300 viewport's atlas
    // holds three of them, and the frame at the zoom floor shows two.
    for y in 0..3 {
        for x in 0..6 {
            paint_position_encoded(&mut store, surface, TileId { x, y });
        }
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 9, "a 3x3 slot grid, fully uploaded");
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.25);

    let samples = eight_columns_across();
    let pixels = render_and_sample_pixels(
        device,
        queue,
        &mut canvas,
        &residency,
        MINIFYING_VIEWPORT,
        &samples,
    );
    let seen: Vec<u32> = pixels.iter().map(|&p| decode_tile_x(p)).collect();

    assert!(
        seen.is_sorted(),
        "tile indices across the viewport must never go backwards; \
         got {seen:?} at zoom 0.25. A repeating pattern (e.g. \
         [0, 0, 1, 1, 0, 0, 1, 1]) is the atlas UV wrapping around and \
         showing the same document region a second time -- a seamless, \
         convincing duplicate of content that is not there"
    );
    assert!(
        seen.contains(&0),
        "the document's own top-left tile must still be visible at the \
         left edge; got {seen:?}"
    );
    assert!(
        seen.contains(&1),
        "the frame must actually span more than one tile, or \
         `is_sorted` above is satisfied by a frame that shows a single \
         tile everywhere and proves nothing; got {seen:?}"
    );
}

/// Positive proof that `min_filter: FilterMode::Linear` is genuinely in
/// effect when the canvas is minified — i.e. that pinning the sampler to
/// a single mip level did not silently turn filtering off.
///
/// This matters because the two obvious ways to pin a sampler to level 0
/// are *not* equivalent. `lod_max_clamp: 0.0` clamps the computed LOD to
/// zero, and both the Vulkan and OpenGL specs make the
/// magnification/minification decision on the LOD **after** that clamp —
/// so a clamped LOD can never be positive, every access classifies as
/// magnification, and `mag_filter` (`Nearest`) is used for every sample,
/// `min_filter` never. Restricting the texture *view* to one mip level
/// instead (`mip_level_count: Some(1)`) leaves the LOD alone: the access
/// still classifies as minification, `min_filter` still applies, and
/// there is simply no other level for level selection to reach.
///
/// Every other canvas test in this file paints uniform colours, under
/// which `Nearest` and `Linear` are indistinguishable by construction —
/// which is exactly why this needed its own fixture.
///
/// Fixture: a hard black/white edge down the middle of the atlas, viewed
/// at zoom 0.6 (1.667 atlas texels per screen pixel, so the LOD is
/// `log2(1.667) ≈ 0.74`, unambiguously minification). `Nearest` can only
/// ever return the two source values; `Linear` must produce at least one
/// intermediate pixel somewhere along the edge.
///
/// On [`MINIFYING_VIEWPORT`], whose zoom floor is 0.5859: zoom 0.6 is
/// just above it, so it is rendered as asked. On [`VIEWPORT`] the floor
/// is 1.0, the same request would be rendered at 1.0, and *nothing*
/// would be minified — this test would then be asserting a property of
/// `mag_filter` while claiming to test `min_filter`.
#[test]
fn canvas_pipeline_min_filter_linear_still_applies_when_minified() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    // Column 0 black, columns 1-2 white -- a vertical edge, so the
    // vertical axis contributes nothing and only horizontal filtering
    // shows up.
    for y in 0..3 {
        paint(
            &mut store,
            surface,
            TileId { x: 0, y },
            [0.0, 0.0, 0.0, 1.0],
        );
        for x in 1..3 {
            paint(&mut store, surface, TileId { x, y }, [1.0, 1.0, 1.0, 1.0]);
        }
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 9);
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    // Zoom 0.6 has to sit above this viewport's own zoom floor
    // (`MINIFYING_FLOOR`, 0.5859), or it is not the zoom being rendered
    // and the LOD reasoning above does not describe this frame.
    assert!(
        (TileResidency::effective_zoom(MINIFYING_VIEWPORT, 0.6) - 0.6).abs() < 1e-6,
        "this viewport must render zoom 0.6 as asked; its floor is \
         {MINIFYING_FLOOR}"
    );
    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.6);

    // The atlas's own black/white boundary (document texel x = 256)
    // lands at screen x = 256 * 0.6 ~= 153.6; scan generously either
    // side of it rather than betting on one exact pixel.
    let samples: Vec<(u32, u32)> = (130..180).map(|x| (x, 64)).collect();
    let pixels = render_and_sample_pixels(
        device,
        queue,
        &mut canvas,
        &residency,
        MINIFYING_VIEWPORT,
        &samples,
    );
    let reds: Vec<u8> = pixels
        .iter()
        .map(|&p| match p.first() {
            Some(&red) => red,
            None => unreachable!("a sampled pixel always has four channels"),
        })
        .collect();

    assert!(
        reds.iter().any(|&r| (10..=245).contains(&r)),
        "sampling across a hard black/white edge under minification must \
         produce at least one blended pixel; got only the two source \
         values {reds:?}, which is what `mag_filter: Nearest` returns -- \
         i.e. `min_filter: Linear` is not actually being used"
    );
}

/// **The negative control for the alpha convention the atlas is in.**
///
/// A `textureSample` with `min_filter: Linear` averages four texels per
/// channel, in fixed-function hardware, *before* `fs_canvas` runs. In
/// the **straight**-alpha domain that average weights a fully
/// transparent texel's RGB exactly as heavily as an opaque neighbour's,
/// so whatever colour happens to be sitting behind `alpha = 0` bleeds
/// into the visible result at a hard alpha edge — a dark halo if it is
/// transparent black, a bright one if it is transparent white. No
/// formula in the shader can undo that; the information is gone by the
/// time the shader sees the tap. Premultiplying at *upload*
/// (`TileResidency::sync`/`upload_mip`) is what makes the same hardware
/// average the correct alpha-weighted one.
///
/// **The fixture is self-calibrating**, which is why it is written as a
/// pair rather than as one absolute expected value. Two documents,
/// identical except for the RGB stored *behind* the transparent side:
///
/// - A: opaque white `(1, 1, 1, 1)` next to transparent **black**
///   `(0, 0, 0, 0)`.
/// - B: opaque white `(1, 1, 1, 1)` next to transparent **white**
///   `(1, 1, 1, 0)`.
///
/// Both are legal straight-alpha content and both are *visually
/// identical documents* — alpha 0 means "not there", so what colour is
/// stored underneath must not be observable. Premultiplying maps both
/// transparent texels to `(0, 0, 0, 0)`, so the two frames come out the
/// same. In the straight domain they do not: at the midpoint of the edge
/// (`a = 0.5`) A renders `0.5 * 0.5 + bg * 0.5` and B renders
/// `1.0 * 0.5 + bg * 0.5` — a gap of 0.25, about **64 of 255 units**,
/// far above any rounding noise.
///
/// Asserting frame-equality rather than one expected number means the
/// test needs to know neither the exact per-pixel alpha along the ramp
/// nor which checkerboard square each sample lands on. The second
/// assertion (at least one genuinely blended pixel) is what stops a
/// degenerate frame — two identical *blank* renders — from satisfying
/// the first one vacuously.
///
/// **The second half of the test covers the second half of the fix.**
/// The A/B check above is a control for the *upload* step: it fails if
/// `premultiply_rgba` stops running, and it passes either way for the
/// shader line, since a premultiplied atlas makes both fixtures
/// identical before `fs_canvas` ever sees them. So the test also renders
/// a **uniform 50%-alpha white** document at the same minified zoom,
/// where alpha is `0.5` by construction rather than by wherever the
/// sampler's ramp happened to land, and pins the premultiplied-domain
/// value: `0.5 + bg * 0.5` (150 or 158 of 255, for the checkerboard's
/// two squares) against the straight-domain `0.25 + bg * 0.5` (87 or
/// 94). Between those bands there is nothing but ~56 units of daylight.
///
/// **Both controls were measured, not argued** — by offscreen pixel
/// readback on a real discrete GPU: `NVIDIA GeForce RTX 3090 (Vulkan,
/// DiscreteGpu)`, the adapter `real_context()` selected and printed.
/// `AURORA_REQUIRE_GPU=1`, which hard-fails on a `DeviceType::Cpu`
/// adapter, passes on this machine — so these numbers are **not** from a
/// software rasterizer, unlike some older measurements recorded
/// elsewhere in this crate:
///
/// - With `fs_canvas` temporarily reverted to the straight-domain
///   `c.rgb * c.a + bg * (1.0 - c.a)` and `premultiply_rgba` left in
///   place, the uniform-alpha assertion fails, reading **94** where it
///   requires ~150–158. The single blended pixel on the hard edge above
///   moves from **186** to **129** in the same run — the 57-unit gap
///   this test's own scan records.
/// - With `fs_canvas` correct and `premultiply_rgba` made a no-op, the
///   A/B assertion fails at sample 24: the transparent-**black** frame
///   reads **46** (pure checkerboard) where the transparent-**white**
///   frame reads **255** (fully blown out) — a **209/255** gap, which is
///   the halo itself, in both directions at once.
///
/// **What that does and does not establish.** It is real-hardware
/// *pixel* verification — one GPU, one vendor, one backend (NVIDIA,
/// Vulkan); Metal and DX12 are unverified, and one adapter's filtering
/// is indicative rather than settled. It is **not** interactive
/// verification: this machine has no display server, so nothing here
/// went through a window, a swapchain, or a human's eyes, which is
/// exactly the gap CLAUDE.md's "lesson from the last round" is about.
/// And note what was never observed at all: **the original halo was
/// never reproduced as a user-visible artifact** — not in software, not
/// on this GPU, not by anyone. The fix is justified by the arithmetic of
/// texture filtering and confirmed by readback. "The halo is fixed" is a
/// stronger claim than anything here supports, and should not be made.
#[test]
// One test, two controls, deliberately: the A/B pair and the
// known-alpha check cover the two halves of one fix (upload, then
// shader) and are only meaningful read together -- splitting them would
// relocate lines without reducing what a reader has to hold in mind.
#[allow(clippy::too_many_lines)]
fn canvas_pipeline_does_not_bleed_transparent_black_across_a_hard_alpha_edge() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();

    // The same scan the `min_filter` test uses, for the same reason: the
    // atlas's own edge (document texel x = 256) lands at screen
    // x = 256 * 0.6 ~= 153.6, and betting on one exact pixel would make
    // this test about arithmetic nobody cares about.
    let samples: Vec<(u32, u32)> = (130..180).map(|x| (x, 64)).collect();

    // `behind` is the RGB stored *underneath* the transparent side. It
    // must not be observable.
    let render = |behind: f32| -> Vec<[u8; 4]> {
        let (_dir, mut store) = tile_store();
        let surface = SurfaceId::from_raw(0);
        for y in 0..3 {
            paint(
                &mut store,
                surface,
                TileId { x: 0, y },
                [1.0, 1.0, 1.0, 1.0],
            );
            for x in 1..3 {
                paint(
                    &mut store,
                    surface,
                    TileId { x, y },
                    [behind, behind, behind, 0.0],
                );
            }
        }

        let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
        let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
        assert_eq!(stats.uploaded, 9);
        assert_eq!(stats.errors, 0);
        residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.6);

        let mut canvas = CanvasPipeline::new(device);
        render_and_sample_pixels(
            device,
            queue,
            &mut canvas,
            &residency,
            MINIFYING_VIEWPORT,
            &samples,
        )
    };

    let behind_black = render(0.0);
    let behind_white = render(1.0);

    let reds = |pixels: &[[u8; 4]]| -> Vec<u8> {
        pixels
            .iter()
            .map(|p| match p.first() {
                Some(&red) => red,
                None => unreachable!("a sampled pixel always has four channels"),
            })
            .collect()
    };
    let black_reds = reds(&behind_black);
    let white_reds = reds(&behind_white);

    // The edge really is being filtered, or the equality below would be
    // satisfied by two identical unblended frames and prove nothing.
    assert!(
        black_reds.iter().any(|&r| (10..=245).contains(&r)),
        "the scan must actually cross a filtered alpha edge; got only \
         plateau values {black_reds:?}"
    );

    let mut worst = 0_i32;
    let mut worst_at = 0_usize;
    for (i, (&b, &w)) in black_reds.iter().zip(white_reds.iter()).enumerate() {
        let delta = (i32::from(b) - i32::from(w)).abs();
        if delta > worst {
            worst = delta;
            worst_at = i;
        }
    }
    assert!(
        worst <= 2,
        "the RGB stored behind a fully transparent texel leaked into the \
         visible result: at sample {worst_at} the frame with transparent \
         *black* behind the edge read {:?} while the frame with \
         transparent *white* behind it read {:?} -- a gap of {worst}/255. \
         Those two documents are visually identical (alpha 0 means \
         \"not there\"), so any gap at all means the atlas is being \
         filtered in the straight-alpha domain. Measured at 209/255 with \
         `premultiply_rgba` made a no-op (the halo itself), and 0 with \
         the premultiply-at-upload step this test guards.\n\
         behind-black: {black_reds:?}\nbehind-white: {white_reds:?}",
        black_reds.get(worst_at),
        white_reds.get(worst_at),
    );

    // -- The shader half: alpha known by construction, not by ramp --
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);
    for y in 0..3 {
        for x in 0..3 {
            paint(&mut store, surface, TileId { x, y }, [1.0, 1.0, 1.0, 0.5]);
        }
    }
    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.errors, 0);
    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.6);
    let mut canvas = CanvasPipeline::new(device);
    let pixel = render_and_sample_pixel(
        device,
        queue,
        &mut canvas,
        &residency,
        MINIFYING_VIEWPORT,
        (MINIFYING_VIEWPORT.0 / 2, MINIFYING_VIEWPORT.1 / 2),
    );
    let Some(&red) = pixel.first() else {
        unreachable!("a sampled pixel always has four channels");
    };
    // Uniform content, so filtering cannot change the sampled value and
    // alpha really is 0.5 wherever this lands. The band spans both
    // checkerboard squares (`0.5 + 0.18 * 0.5 = 0.59` -> 150 and
    // `0.5 + 0.24 * 0.5 = 0.62` -> 158) with a few units of slack, and
    // excludes the straight-domain pair (87 and 94) by ~56 units.
    assert!(
        (145..=165).contains(&red),
        "a uniform 50%-alpha white document rendered {red}, not the \
         premultiplied-domain 150..158 (`0.5 + bg * 0.5`). Measured at \
         94 with `fs_canvas` reverted to the straight-domain \
         `c.rgb * c.a + bg * (1.0 - c.a)` while the atlas is \
         premultiplied -- i.e. alpha counted twice, translucent content \
         rendering far too dark."
    );
}

/// A degenerate `zoom` must not corrupt the canvas.
///
/// `write_uniform` divides by `zoom`. `set_origin`'s doc comment used to
/// argue no guard was needed because `aurora_ui::CanvasView` clamps zoom
/// to `[MIN_ZOOM, MAX_ZOOM]` — but the value `aurora-app` actually
/// passes is `effective_residency_zoom(canvas_zoom, scale_factor)`, a
/// product formed in a *different* crate, and that helper only guards
/// `scale_factor`. Zero, negative, infinite, denormal, and NaN zooms all
/// reach here, and each poisons `uv_scale`.
///
/// The fallback is `1.0`, matching `effective_residency_zoom`'s own
/// existing pattern for a bad `scale_factor`, so each degenerate value
/// must render *exactly* the frame zoom 1.0 renders.
///
/// Position-encoded tiles and eight sample points, not one uniform
/// colour at the centre: with a single flat green fixture every texel in
/// the atlas is identical, so any sample at all — including one taken at
/// a wildly wrong UV — returns the expected pixel and the test proves
/// nothing.
///
/// On [`MINIFYING_VIEWPORT`] so that "renders the same frame as zoom
/// 1.0" is a real claim: on [`VIEWPORT`] every zoom in this test — good
/// or degenerate — is clamped to 1.0 and *every* frame matches, so the
/// equality assertions would hold with the guard removed entirely. The
/// `assert_ne!` below pins that distinction directly.
#[test]
fn canvas_pipeline_survives_a_degenerate_zoom() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    for y in 0..3 {
        for x in 0..3 {
            paint_position_encoded(&mut store, surface, TileId { x, y });
        }
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 9);
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let samples = eight_columns_across();
    let render = |canvas: &mut CanvasPipeline, residency: &TileResidency| {
        render_and_sample_pixels(
            device,
            queue,
            canvas,
            residency,
            MINIFYING_VIEWPORT,
            &samples,
        )
    };

    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 1.0);
    let baseline = render(&mut canvas, &residency);

    for zoom in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, zoom);
        let pixels = render(&mut canvas, &residency);
        assert_eq!(
            pixels, baseline,
            "zoom {zoom} must fall back to 1.0 and render the identical \
             frame. Anything else means the degenerate value reached \
             `uv_scale` and the canvas is showing something that \
             corresponds to no part of the document, with no error \
             reported anywhere"
        );
    }

    // A denormal is *not* one of these cases, and deliberately does not
    // take the fallback: it is finite and positive, so it is a real (if
    // absurd) zoom, and the zoom floor absorbs it exactly as it absorbs
    // `MIN_ZOOM` -- both render at `min_zoom_for_viewport`. Substituting
    // 1.0 here would be the wrong answer, not a safer one, so this pins
    // the behaviour that it renders exactly as the smallest zoom a user
    // can actually reach does.
    residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, 0.01);
    let at_min_zoom = render(&mut canvas, &residency);
    residency.set_origin(
        queue,
        (0.0, 0.0),
        MINIFYING_VIEWPORT,
        f32::MIN_POSITIVE / 2.0,
    );
    let at_denormal = render(&mut canvas, &residency);
    assert_eq!(
        at_denormal, at_min_zoom,
        "a denormal zoom must saturate to this viewport's own zoom floor \
         exactly as MIN_ZOOM does, not poison `uv_scale`"
    );
    assert_ne!(
        at_min_zoom, baseline,
        "the floor frame and the zoom-1.0 frame have to actually differ, or \
         every equality above is satisfied by a viewport on which no zoom \
         changes anything -- which is exactly what this test looked like on \
         a 256 px viewport, whose floor is 1.0"
    );
}

/// `resize` rebuilds the atlas through `Self::new`, so it inherits both
/// the single-mip-level view and the atlas-coverage clamp — today, and
/// only incidentally. This pins that: a future `resize` that stops
/// delegating to `new` (or grows its own texture/view construction)
/// would otherwise silently reintroduce the pure-checkerboard bug on
/// every window resize, with every other test in this file still green
/// because none of them resize before rendering.
#[test]
fn canvas_pipeline_after_a_resize_still_shows_content_when_zoomed_out() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    for y in 0..3 {
        for x in 0..4 {
            paint_position_encoded(&mut store, surface, TileId { x, y });
        }
    }

    let mut residency = TileResidency::new(device, queue, (128, 128));
    residency.resize(device, queue, MINIFYING_VIEWPORT);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(
        stats.uploaded, 9,
        "the post-resize 3x3 grid, freshly filled"
    );
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let samples = eight_columns_across();
    for zoom in [0.6_f32, 0.5, 0.25, 0.01] {
        residency.set_origin(queue, (0.0, 0.0), MINIFYING_VIEWPORT, zoom);
        let pixels = render_and_sample_pixels(
            device,
            queue,
            &mut canvas,
            &residency,
            MINIFYING_VIEWPORT,
            &samples,
        );
        let seen: Vec<u32> = pixels.iter().map(|&p| decode_tile_x(p)).collect();
        assert!(
            pixels
                .iter()
                .all(|&p| matches!(p, [_, _, blue, _] if blue >= 200)),
            "a resized atlas must still show real content when minified; \
             at zoom {zoom} the full-blue content marker was missing \
             ({pixels:?}), i.e. the canvas fell back to the empty-canvas \
             checkerboard -- the pure-checkerboard bug returning"
        );
        assert!(
            seen.is_sorted(),
            "a resized atlas must inherit the zoom floor too; at zoom \
             {zoom} the tile indices across the viewport went backwards \
             ({seen:?}), i.e. the UV wrapped and duplicated document \
             content"
        );
        assert!(
            seen.contains(&1),
            "the frame must actually span more than one tile at zoom \
             {zoom}, or `is_sorted` is vacuous; got {seen:?}"
        );
    }
}

/// The atlas's UV wrap is **load-bearing**, and must survive any fix
/// aimed at the duplication bug above.
///
/// `TileResidency` is a toroidal sliding window: slot `tx % grid.0`
/// holds tile `tx`, and `write_uniform` wraps the scroll offset into
/// `[0, 1)`. So whenever the visible window straddles the atlas's own
/// right or bottom edge — which happens for most pan positions, at every
/// zoom, because a 1920 px viewport's atlas is only 2304 px wide and
/// `uv_scale` is already 0.833 at 100% — the right-hand part of the
/// screen legitimately samples `uv > 1.0` and *must* wrap around to slot
/// 0 to find the tile that belongs there.
///
/// That makes `AddressMode::ClampToEdge` the wrong tool for the
/// duplication bug, even though it superficially describes the desired
/// "stop at the edge" behaviour: it cannot distinguish this legitimate
/// wrap from an out-of-coverage one, and would smear the atlas's edge
/// column across the right half of the canvas during ordinary panning at
/// 100% zoom. Here, origin `1.5 * TILE` puts the window at
/// `uv ∈ [0.75, 1.25]`, so the right half of the frame is served
/// entirely by the wrap; `ClampToEdge` renders tile 1 twice.
#[test]
fn canvas_pipeline_wraps_to_the_toroidal_slot_when_the_window_straddles_the_atlas_edge() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    for y in 0..3 {
        for x in 0..3 {
            paint_position_encoded(&mut store, surface, TileId { x, y });
        }
    }

    let mut residency = TileResidency::new(device, queue, VIEWPORT);
    // Half a tile past tile 1: the window covers document x 384..640,
    // i.e. the right half of tile 1 followed by the left half of tile 2,
    // and tile 2 lives in slot 0 (2 % 2), reachable only through the wrap.
    let origin = 1.5 * f64::from(TILE) as f32;
    residency.set_origin(queue, (origin, 0.0), VIEWPORT, 1.0);
    let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
    assert_eq!(stats.uploaded, 4);
    assert_eq!(stats.errors, 0);

    let mut canvas = CanvasPipeline::new(device);
    let samples: Vec<(u32, u32)> = (0..8).map(|i| (16 + i * 32, 64)).collect();
    let pixels =
        render_and_sample_pixels(device, queue, &mut canvas, &residency, VIEWPORT, &samples);
    let seen: Vec<u32> = pixels.iter().map(|&p| decode_tile_x(p)).collect();

    assert_eq!(
        seen,
        vec![1, 1, 1, 1, 2, 2, 2, 2],
        "the left half of the frame must show tile 1 and the right half \
         tile 2, which is only reachable by wrapping past the atlas's own \
         right edge into slot 0. `[1, 1, 1, 1, 1, 1, 1, 1]` means the wrap \
         was replaced with edge clamping and half the canvas is now a \
         duplicate of the wrong tile"
    );
}

/// **RT12-02, end to end through a real render pass**: panning at a
/// constant zoom must not change the scale the canvas is drawn at.
///
/// The regression this pins was real and measured. `uv_scale` used to be
/// clamped per axis to `tex_size - sub_tile`, and `sub_tile` — the
/// fractional part of the pan position within the top-left tile — sweeps
/// `[0, TILE)` continuously as the user pans. So the clamp, and with it
/// the rendered scale, ramped smoothly across one tile of panning and
/// then snapped back: measured 0.500 → 0.889 (a 1.78× change) at a
/// *constant* user zoom of 0.5. The document visibly breathed while the
/// user dragged it.
///
/// Mechanism of the test: tile 0 is black and every tile right of it is
/// white, so the document has exactly one vertical edge, at document
/// x = `TILE`. Wherever that edge lands on screen tells us the scale
/// directly — `screen_x = (TILE - doc_origin_x) * rendered_zoom` — and
/// the frame is re-rendered at nine pan positions spanning one whole
/// tile. If the scale is pan-independent, every one of those nine edge
/// positions falls on the same straight line; if it ramps, they do not.
///
/// Both a zoom below this viewport's floor (0.5, rendered at the floor)
/// and one above it (0.75, rendered as asked) are checked, so the test
/// also confirms the *value* being held constant is the right one and
/// not merely constant.
#[test]
fn canvas_pipeline_keeps_one_scale_while_panning_a_whole_tile() {
    let Some(context) = real_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let (_dir, mut store) = tile_store();
    let surface = SurfaceId::from_raw(0);

    for y in 0..3 {
        paint(
            &mut store,
            surface,
            TileId { x: 0, y },
            [0.0, 0.0, 0.0, 1.0],
        );
        for x in 1..5 {
            paint(&mut store, surface, TileId { x, y }, [1.0, 1.0, 1.0, 1.0]);
        }
    }

    let mut residency = TileResidency::new(device, queue, MINIFYING_VIEWPORT);
    let mut canvas = CanvasPipeline::new(device);
    let row: Vec<(u32, u32)> = (0..MINIFYING_VIEWPORT.0).map(|x| (x, 150)).collect();

    for zoom in [0.5_f32, 0.75] {
        let rendered = TileResidency::effective_zoom(MINIFYING_VIEWPORT, zoom);
        for step in 0..=8 {
            let doc_origin_x = (step as f32) * (TILE as f32) / 8.0;
            residency.set_origin(queue, (doc_origin_x, 0.0), MINIFYING_VIEWPORT, zoom);
            let _ = residency.sync(queue, &mut store, surface, false, usize::MAX);
            let pixels = render_and_sample_pixels(
                device,
                queue,
                &mut canvas,
                &residency,
                MINIFYING_VIEWPORT,
                &row,
            );
            // The first column that is unambiguously the white half of
            // the document: `min_filter: Linear` blends across roughly
            // one pixel at the edge, so this lands within a pixel of the
            // true boundary rather than exactly on it.
            let Some(edge) = pixels
                .iter()
                .position(|&p| matches!(p, [red, _, _, _] if red > 200))
            else {
                unreachable!("the white half of the document is always in frame");
            };
            let expected = ((TILE as f32) - doc_origin_x) * rendered;
            #[allow(clippy::cast_precision_loss)]
            let seen = edge as f32;
            assert!(
                (seen - expected).abs() <= 2.0,
                "at a constant zoom of {zoom} (rendered at {rendered}), panning \
                 to document x {doc_origin_x} put the document's own black/white \
                 edge at screen x {seen} where {expected} is where that scale \
                 puts it. An edge that drifts as the pan advances -- and snaps \
                 back once per tile -- means the rendered scale is a function of \
                 the pan position, which is the regression this test exists for"
            );
        }
    }
}
