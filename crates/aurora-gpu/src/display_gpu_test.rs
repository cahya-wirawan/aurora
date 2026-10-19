//! Real-GPU tests of [`crate::DisplayTexture`] (task 142): the picture is
//! the atlas's texel for texel at and above 100 %, pages are seamless, the
//! mip chain is the premultiplied 2 x 2 box mean, a power-of-two zoom shows
//! exactly one level, and a re-uploaded tile changes only its own region
//! at every level.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_arguments,
    clippy::many_single_char_names
)]

use crate::test_support::{real_context, real_tile_store};
use crate::{CanvasPipeline, DisplayTexture, TileResidency};
use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileId, TileStore};
use half::f16;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const LEVELS: u32 = 6;

fn surface() -> SurfaceId {
    SurfaceId::from_raw(u64::MAX)
}

/// Deterministic noise: straight colour in `[0, 1]`, alpha from a few
/// levels (transparent, translucent, opaque) varying by region, so the
/// premultiply and the box mean both matter.
fn fill(store: &mut TileStore, doc_size: (u32, u32), seed: u64) {
    let tiles = crate::doc_tiles(doc_size);
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for y in 0..tiles.1 {
        for x in 0..tiles.0 {
            let Ok(tile) = store.get_mut(surface(), TileId { x, y }) else {
                unreachable!("a fresh store accepts tiles");
            };
            for (i, texel) in tile.texels_mut().chunks_exact_mut(CHANNELS).enumerate() {
                let (tx, ty) = (i as u32 % TILE, i as u32 / TILE);
                let r = next();
                let alpha = match (tx / 37 + ty / 23 + x + y) % 5 {
                    0 => 0.0,
                    1 => 0.25,
                    2 => 0.6,
                    _ => 1.0,
                };
                for (c, sample) in texel.iter_mut().take(3).enumerate() {
                    *sample = f16::from_f32(((r >> (c * 16)) & 0xFFFF) as f32 / 65535.0);
                }
                if let Some(a) = texel.get_mut(3) {
                    *a = f16::from_f32(alpha);
                }
            }
        }
    }
}

fn display_from(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    store: &mut TileStore,
    doc_size: (u32, u32),
    max_dimension: u32,
) -> DisplayTexture {
    let Ok(mut display) =
        DisplayTexture::with_max_dimension(device, doc_size, LEVELS, max_dimension)
    else {
        unreachable!("pageable");
    };
    let tiles = crate::doc_tiles(doc_size);
    for y in 0..tiles.1 {
        for x in 0..tiles.0 {
            let id = TileId { x, y };
            let texels = match store.get(surface(), id) {
                Ok(tile) => tile.texels().to_vec(),
                Err(err) => unreachable!("{err}"),
            };
            if display.upload_tile(queue, id, &texels).is_err() {
                unreachable!("a whole tile inside the document");
            }
        }
    }
    let _ = display.regenerate_mips(device, queue);
    display
}

fn target(device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("display-test-target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Reads `level` of `texture` (`width` x `height` at level 0) back as raw
/// bytes, `bytes_per_texel` each, rows unpadded.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    level: u32,
    size: (u32, u32),
    bytes_per_texel: u32,
) -> Vec<u8> {
    let (w, h) = ((size.0 >> level).max(1), (size.1 >> level).max(1));
    let row = w * bytes_per_texel;
    let padded = row.div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("display-test-readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("display-test-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: level,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("mapped once the device is idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("mapped");
    };
    let mut out = Vec::with_capacity((row * h) as usize);
    for line in data.chunks_exact(padded as usize) {
        out.extend_from_slice(line.get(..row as usize).unwrap_or_default());
    }
    drop(data);
    buffer.unmap();
    out
}

fn as_f16(bytes: &[u8]) -> Vec<f16> {
    bytes
        .chunks_exact(2)
        .map(|b| {
            f16::from_le_bytes([
                b.first().copied().unwrap_or(0),
                b.get(1).copied().unwrap_or(0),
            ])
        })
        .collect()
}

/// Draws one canvas frame into a fresh target and reads it back (RGBA8).
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewport: (u32, u32),
    record: impl FnOnce(&mut wgpu::RenderPass<'_>),
) -> Vec<u8> {
    let texture = target(device, viewport);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("display-test-draw"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("display-test-draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        record(&mut pass);
    }
    queue.submit(std::iter::once(encoder.finish()));
    read_back(device, queue, &texture, 0, viewport, 4)
}

fn render_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut CanvasPipeline,
    store: &mut TileStore,
    viewport: (u32, u32),
    origin: (f32, f32),
    zoom: f32,
    clip: (f32, f32, f32, f32),
) -> Vec<u8> {
    let mut residency = TileResidency::new(device, queue, viewport);
    residency.set_document_clip(Some(clip));
    residency.set_origin(queue, origin, viewport, zoom);
    let _ = residency.sync(queue, store, surface(), true, usize::MAX);
    let bind_group = canvas.bind_group(device, &residency);
    let pipeline = canvas.pipeline(device, FORMAT).clone();
    draw(device, queue, viewport, |pass| {
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    })
}

fn render_display(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut CanvasPipeline,
    display: &DisplayTexture,
    viewport: (u32, u32),
    origin: (f32, f32),
    zoom: f32,
    clip: (f32, f32, f32, f32),
) -> Vec<u8> {
    display.set_view(queue, origin, viewport, zoom, Some(clip), [1.0; 3]);
    let groups: Vec<wgpu::BindGroup> = (0..display.pages().len())
        .filter_map(|page| canvas.display_bind_group(device, display, page))
        .collect();
    let scissors = display.page_scissors(origin, viewport, zoom);
    let pipeline = canvas.display_pipeline(device, FORMAT).clone();
    draw(device, queue, viewport, |pass| {
        pass.set_pipeline(&pipeline);
        for &(page, x0, y0, x1, y1) in &scissors {
            let Some(group) = groups.get(page) else {
                continue;
            };
            pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    })
}

fn clip_for(doc_size: (u32, u32)) -> (f32, f32, f32, f32) {
    (0.0, 0.0, doc_size.0 as f32, doc_size.1 as f32)
}

fn differing_pixels(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(x, y)| x != y)
        .count()
}

/// At 100 % and the integer magnifications, from whole and fractional
/// origins (none putting a pixel centre exactly on a texel edge), the
/// display texture draws the same bytes as the atlas, one page or many.
#[test]
fn at_and_above_100_percent_the_display_draws_the_atlas_picture_bit_for_bit() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (1700, 1150);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0x0DD5);
    let one = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let paged = display_from(device, queue, &mut store, doc_size, 3 * TILE);
    assert_eq!(one.pages().len(), 1);
    assert!(paged.pages().len() > 4, "{} pages", paged.pages().len());
    let mut canvas = CanvasPipeline::new(device);
    let viewport = (300, 200);
    let clip = clip_for(doc_size);
    for &(origin, zoom) in &[
        ((0.0, 0.0), 1.0_f32),
        ((37.0, 11.0), 1.0),
        ((500.25, 600.75), 1.0),
        ((1500.0, 1000.0), 1.0),
        ((0.0, 0.0), 2.0),
        ((767.5, 511.5), 2.0),
        ((10.0, 20.0), 3.0),
        ((1530.0, 1100.0), 4.0),
    ] {
        let atlas = render_atlas(
            device,
            queue,
            &mut canvas,
            &mut store,
            viewport,
            origin,
            zoom,
            clip,
        );
        for (name, display) in [("one page", &one), ("paged", &paged)] {
            let shown = render_display(
                device,
                queue,
                &mut canvas,
                display,
                viewport,
                origin,
                zoom,
                clip,
            );
            assert_eq!(
                differing_pixels(&atlas, &shown),
                0,
                "{name}: {origin:?} at {zoom}x differs from the atlas"
            );
        }
    }
}

/// Pages are seamless at every zoom, the minified ones included: the paged
/// texture draws exactly what one texture does.
#[test]
fn pages_draw_exactly_what_one_texture_draws_at_every_zoom() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (1700, 1150);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0xBEEF);
    let one = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let paged = display_from(device, queue, &mut store, doc_size, 3 * TILE);
    let mut canvas = CanvasPipeline::new(device);
    let viewport = (320, 240);
    let clip = clip_for(doc_size);
    for &(origin, zoom) in &[
        ((0.0, 0.0), 0.25_f32),
        ((-100.0, -60.0), 0.125),
        ((200.0, 150.0), 0.5),
        ((256.0 - 160.0, 256.0 - 120.0), 0.73),
        ((512.0 - 100.0, 768.0 - 90.0), 0.37),
        ((1.0, 2.0), 1.0 / 32.0),
    ] {
        let a = render_display(
            device,
            queue,
            &mut canvas,
            &one,
            viewport,
            origin,
            zoom,
            clip,
        );
        let b = render_display(
            device,
            queue,
            &mut canvas,
            &paged,
            viewport,
            origin,
            zoom,
            clip,
        );
        // A page samples the same texels at the same positions, but its
        // texture coordinates are relative to another origin, so a filter
        // weight can quantise differently (the hardware's sub-texel
        // precision): within one 8-bit step, spread over the picture, not
        // concentrated at the page edges.
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        let seam = seam_pixels(&paged, origin, viewport, zoom);
        let at_seams = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .enumerate()
            .filter(|(i, (x, y))| x != y && seam.contains(&(*i as u32)))
            .count();
        println!(
            "{origin:?} x{zoom}: {} of {} pixels differ, worst {worst}, {at_seams} of {} seam \
             pixels differ",
            differing_pixels(&a, &b),
            viewport.0 * viewport.1,
            seam.len()
        );
        assert!(worst <= 1, "{origin:?} at {zoom}x: paged off by {worst}");
        assert!(
            at_seams * 20 <= seam.len().max(1),
            "{origin:?} at {zoom}x: {at_seams} of {} seam pixels differ",
            seam.len()
        );
    }
}

/// At a fractional magnification `Nearest` sampling puts some pixel
/// centres within the hardware's sub-texel precision of a texel edge, and
/// two parameterisations of the same position (the atlas's toroidal one,
/// the display's per page) can pick the two texels either side of it. Both
/// are a correct nearest texel; measured here, and bounded, rather than
/// claimed identical.
#[test]
fn at_a_fractional_magnification_only_edge_ties_differ_from_the_atlas() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (1700, 1150);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0xF00D);
    let one = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let paged = display_from(device, queue, &mut store, doc_size, 3 * TILE);
    let mut canvas = CanvasPipeline::new(device);
    let viewport = (320, 240);
    let clip = clip_for(doc_size);
    let total = (viewport.0 * viewport.1) as usize;
    for &(origin, zoom) in &[
        ((300.0, 300.0), 1.7_f32),
        ((12.0, 40.0), 1.25),
        ((700.0, 500.0), 1.41),
    ] {
        let atlas = render_atlas(
            device,
            queue,
            &mut canvas,
            &mut store,
            viewport,
            origin,
            zoom,
            clip,
        );
        let a = render_display(
            device,
            queue,
            &mut canvas,
            &one,
            viewport,
            origin,
            zoom,
            clip,
        );
        let b = render_display(
            device,
            queue,
            &mut canvas,
            &paged,
            viewport,
            origin,
            zoom,
            clip,
        );
        let (da, db) = (differing_pixels(&atlas, &a), differing_pixels(&a, &b));
        // A tie: the pixel centre within 1/128 texel of a texel edge.
        let tie = |i: usize| {
            let (x, y) = (i as u32 % viewport.0, i as u32 / viewport.0);
            let near = |o: f32, p: u32| {
                let c = f64::from(o) + (f64::from(p) + 0.5) / f64::from(zoom);
                (c - c.round()).abs() < 1.0 / 128.0
            };
            near(origin.0, x) || near(origin.1, y)
        };
        let untied = |p: &[u8], q: &[u8]| {
            p.chunks_exact(4)
                .zip(q.chunks_exact(4))
                .enumerate()
                .filter(|(i, (u, v))| u != v && !tie(*i))
                .count()
        };
        let ties = (0..total).filter(|&i| tie(i)).count();
        println!(
            "{origin:?} x{zoom}: atlas vs display {da} of {total}, one vs paged {db}, \
             {ties} pixel centres on a texel edge"
        );
        assert_eq!(
            untied(&atlas, &a),
            0,
            "{origin:?} x{zoom}: a non-tie differs from the atlas"
        );
        assert_eq!(
            untied(&a, &b),
            0,
            "{origin:?} x{zoom}: a non-tie differs between pages"
        );
    }
}

/// The pixel indices within two pixels of a page's owned edge.
fn seam_pixels(
    display: &DisplayTexture,
    origin: (f32, f32),
    viewport: (u32, u32),
    zoom: f32,
) -> std::collections::HashSet<u32> {
    let mut out = std::collections::HashSet::new();
    for (_, x0, y0, x1, y1) in display.page_scissors(origin, viewport, zoom) {
        for y in y0..y1 {
            for x in x0..x1 {
                let near = |v: u32, lo: u32, hi: u32, extent: u32| {
                    (lo > 0 && v < lo + 2) || (hi < extent && v + 2 >= hi)
                };
                if near(x, x0, x1, viewport.0) || near(y, y0, y1, viewport.1) {
                    out.insert(y * viewport.0 + x);
                }
            }
        }
    }
    out
}

/// Level `level` (`size` at level 0) as the CPU computes it from the level
/// below it, `below` (read back from the GPU): the premultiplied 2 x 2 mean
/// in f32, rounded to f16.
fn cpu_level(below: &[f16], size: (u32, u32), level: u32) -> Vec<f16> {
    let (w, h) = ((size.0 >> level).max(1), (size.1 >> level).max(1));
    let bw = (size.0 >> (level - 1)).max(1);
    let mut next = vec![f16::ZERO; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let at = |dx: u32, dy: u32| {
                    below
                        .get((((2 * y + dy) * bw + 2 * x + dx) * 4 + c) as usize)
                        .map_or(0.0, |v| v.to_f32())
                };
                let mean = ((at(0, 0) + at(1, 0)) + (at(0, 1) + at(1, 1))) * 0.25;
                if let Some(slot) = next.get_mut(((y * w + x) * 4 + c) as usize) {
                    *slot = f16::from_f32(mean);
                }
            }
        }
    }
    next
}

/// The whole chain on the CPU from `level0` alone (each level from the
/// CPU's own level below).
fn cpu_chain(level0: &[f16], size: (u32, u32), levels: u32) -> Vec<Vec<f16>> {
    let mut out = vec![level0.to_vec()];
    for level in 1..levels {
        let below = out.last().cloned().unwrap_or_default();
        out.push(cpu_level(&below, size, level));
    }
    out
}

fn ulps(a: f16, b: f16) -> u16 {
    a.to_bits().abs_diff(b.to_bits())
}

#[test]
fn the_mip_chain_is_the_premultiplied_box_mean_and_level_zero_the_premultiplied_tiles() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (700, 450);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0x1234);
    let display = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let Some(texture) = display.page_texture(0) else {
        unreachable!("one page");
    };
    let size = (3 * TILE, 2 * TILE);
    let level0 = as_f16(&read_back(device, queue, texture, 0, size, 8));
    // Level 0: each tile's premultiplied texels, transparent outside the
    // document.
    for ty in 0..2 {
        for tx in 0..3 {
            let Ok(tile) = store.get(surface(), TileId { x: tx, y: ty }) else {
                unreachable!("filled");
            };
            for (i, texel) in tile.texels().chunks_exact(CHANNELS).enumerate() {
                let (x, y) = (tx * TILE + i as u32 % TILE, ty * TILE + i as u32 / TILE);
                let base = ((y * size.0 + x) * 4) as usize;
                let shown = level0.get(base..base + 4).unwrap_or_default();
                let alpha = texel.get(3).map_or(0.0, |a| a.to_f32());
                let inside = x < doc_size.0 && y < doc_size.1;
                for c in 0..4 {
                    let want = if !inside {
                        f16::ZERO
                    } else if c == 3 {
                        texel.get(3).copied().unwrap_or(f16::ZERO)
                    } else {
                        f16::from_f32(texel.get(c).map_or(0.0, |v| v.to_f32()) * alpha)
                    };
                    assert_eq!(
                        shown.get(c).map(|v| v.to_bits()),
                        Some(want.to_bits()),
                        "level 0 texel ({x}, {y}) channel {c}"
                    );
                }
            }
        }
    }
    // Each level against the CPU's mean of the GPU's own level below: one
    // f16 step at most (the GPU's f32 -> f16 rounding). Against the CPU's
    // chain from level 0 alone the steps compound a little, level by level.
    let chain = cpu_chain(&level0, size, LEVELS);
    let mut below = level0.clone();
    for level in 1..LEVELS {
        let gpu = as_f16(&read_back(device, queue, texture, level, size, 8));
        let cpu = cpu_level(&below, size, level);
        assert_eq!(gpu.len(), cpu.len());
        let worst = gpu
            .iter()
            .zip(&cpu)
            .map(|(&a, &b)| ulps(a, b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "level {level}: worst {worst} f16 steps from the CPU mean"
        );
        let Some(whole) = chain.get(level as usize) else {
            unreachable!("built");
        };
        let drift = gpu
            .iter()
            .zip(whole)
            .map(|(&a, &b)| ulps(a, b))
            .max()
            .unwrap_or(0);
        assert!(
            u32::from(drift) <= level + 1,
            "level {level}: {drift} f16 steps from the CPU chain"
        );
        below = gpu;
    }
}

/// At `2^-L` with the view on the level's texel grid, every pixel is level
/// `L`'s texel over the checkerboard (within one 8-bit step).
#[test]
fn a_power_of_two_zoom_shows_exactly_one_level() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (1024, 768);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0x77);
    let display = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let Some(texture) = display.page_texture(0) else {
        unreachable!("one page");
    };
    let mut canvas = CanvasPipeline::new(device);
    for level in 1..4_u32 {
        let zoom = 1.0 / f32::from(1_u16 << level);
        let texels = as_f16(&read_back(device, queue, texture, level, doc_size, 8));
        let w = doc_size.0 >> level;
        let viewport = (doc_size.0 >> level, doc_size.1 >> level);
        let origin = (0.0, 0.0);
        let shown = render_display(
            device,
            queue,
            &mut canvas,
            &display,
            viewport,
            origin,
            zoom,
            clip_for(doc_size),
        );
        let mut worst = 0_i32;
        for (i, pixel) in shown.chunks_exact(4).enumerate() {
            let (x, y) = (i as u32 % viewport.0, i as u32 / viewport.0);
            let base = ((y * w + x) * 4) as usize;
            let c: Vec<f32> = (0..4)
                .map(|k| texels.get(base + k).map_or(0.0, |v| v.to_f32()))
                .collect();
            let check = if ((x / 8) + (y / 8)) % 2 == 0 {
                0.24
            } else {
                0.18
            };
            for k in 0..3 {
                let want = (c.get(k).copied().unwrap_or(0.0)
                    + check * (1.0 - c.get(3).copied().unwrap_or(0.0)))
                    * 255.0;
                let got = f32::from(pixel.get(k).copied().unwrap_or(0));
                worst = worst.max((want.round() - got).abs() as i32);
            }
        }
        assert!(
            worst <= 1,
            "level {level}: worst {worst} 8-bit steps off level {level}"
        );
    }
}

/// Re-uploading one tile rebuilds its region at every level and leaves the
/// rest of the chain untouched.
#[test]
fn a_reuploaded_tile_changes_only_its_own_region_at_every_level() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (1024, 512);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0xABC);
    let mut display = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let Some(texture) = display.page_texture(0) else {
        unreachable!("one page");
    };
    let before: Vec<Vec<f16>> = (0..LEVELS)
        .map(|level| as_f16(&read_back(device, queue, texture, level, doc_size, 8)))
        .collect();
    let changed = TileId { x: 2, y: 1 };
    let new: Vec<f16> = (0..SAMPLES)
        .map(|i| {
            f16::from_f32(if i % 4 == 3 {
                0.5
            } else {
                (i % 7) as f32 / 7.0
            })
        })
        .collect();
    let uploads = display.uploads();
    if display.upload_tile(queue, changed, &new).is_err() {
        unreachable!("inside");
    }
    assert_eq!(display.uploads(), uploads + 1);
    let passes = display.regenerate_mips(device, queue);
    assert_eq!(passes, LEVELS - 1, "one pass per level for one tile");
    assert_eq!(display.regenerate_mips(device, queue), 0, "nothing left");
    let Some(texture) = display.page_texture(0) else {
        unreachable!("one page");
    };
    let after: Vec<Vec<f16>> = (0..LEVELS)
        .map(|level| as_f16(&read_back(device, queue, texture, level, doc_size, 8)))
        .collect();
    // Each level's expectation from the GPU's own level below (level 0 is
    // compared with itself).
    let chain: Vec<Vec<f16>> = (0..LEVELS)
        .map(
            |level| match level.checked_sub(1).and_then(|l| after.get(l as usize)) {
                Some(below) => cpu_level(below, doc_size, level),
                None => after.first().cloned().unwrap_or_default(),
            },
        )
        .collect();
    for level in 0..LEVELS {
        let w = (doc_size.0 >> level).max(1);
        let side = TILE >> level;
        let (Some(old), Some(now), Some(cpu)) = (
            before.get(level as usize),
            after.get(level as usize),
            chain.get(level as usize),
        ) else {
            unreachable!("every level read");
        };
        let mut inside_changed = 0;
        for (i, ((&o, &n), &c)) in old.iter().zip(now).zip(cpu).enumerate() {
            let texel = i as u32 / 4;
            let (x, y) = (texel % w, texel / w);
            let inside = x / side == changed.x && y / side == changed.y;
            if inside {
                assert!(ulps(n, c) <= 1, "level {level} ({x}, {y}) is the new mean");
                if o != n {
                    inside_changed += 1;
                }
            } else {
                assert_eq!(
                    o.to_bits(),
                    n.to_bits(),
                    "level {level} ({x}, {y}) untouched"
                );
            }
        }
        assert!(
            inside_changed > 0,
            "level {level}: the tile's region changed"
        );
    }
}

/// Setting a view, however it zooms or pans, uploads nothing.
#[test]
fn a_view_change_uploads_nothing() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let doc_size = (700, 450);
    let (_dir, mut store) = real_tile_store(64);
    fill(&mut store, doc_size, 0x5);
    let mut display = display_from(device, queue, &mut store, doc_size, u32::MAX);
    let (uploads, passes) = (display.uploads(), display.mip_passes());
    let mut canvas = CanvasPipeline::new(device);
    for &(origin, zoom) in &[
        ((0.0, 0.0), 1.0_f32),
        ((-50.0, -20.0), 0.3),
        ((100.0, 80.0), 2.5),
    ] {
        let _ = render_display(
            device,
            queue,
            &mut canvas,
            &display,
            (300, 200),
            origin,
            zoom,
            clip_for(doc_size),
        );
    }
    assert_eq!(display.regenerate_mips(device, queue), 0);
    assert_eq!((display.uploads(), display.mip_passes()), (uploads, passes));
}

#[test]
fn an_upload_outside_the_document_or_of_the_wrong_size_is_refused() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let Ok(mut display) = DisplayTexture::new(device, (300, 300), LEVELS) else {
        unreachable!("small");
    };
    let tile = vec![f16::ZERO; SAMPLES];
    assert!(
        display
            .upload_tile(queue, TileId { x: 2, y: 0 }, &tile)
            .is_err()
    );
    assert!(
        display
            .upload_tile(
                queue,
                TileId { x: 0, y: 0 },
                tile.get(..4).unwrap_or_default()
            )
            .is_err()
    );
    assert_eq!(display.uploads(), 0);
    assert!(DisplayTexture::new(device, (0, 10), LEVELS).is_err());
    assert!(DisplayTexture::with_max_dimension(device, (2000, 10), LEVELS, 2 * TILE).is_err());
}
