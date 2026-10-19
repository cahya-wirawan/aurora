//! Task 140: the canvas's blend between two atlases (the zoomed-out
//! view's two adjacent pyramid levels), drawn on a real GPU and checked
//! against a CPU reference.
//!
//! What is pinned:
//!
//! - a zero weight is bit-identical to the unblended canvas, whatever the
//!   partner holds (the 1:1 path and every power-of-two zoom draw that
//!   way);
//! - a non-zero weight mixes the partner in premultiplied space before the
//!   checkerboard, within one 8-bit step of an `f64` reference computed
//!   from the same `f16` inputs (the atlas stores premultiplied `f16`, the
//!   shader mixes in `f32` and the target quantises to 8 bits: one step
//!   covers the `f16` rounding of the premultiplied texel, at most 2^-11
//!   relative, and the target's rounding; Metal's quantiser flips are one
//!   step too);
//! - across a level boundary the blended picture is continuous: just above
//!   it (level `L` minified to just under half, mixed toward level `L+1`
//!   with weight `1 - log2(1 + eps)`) it differs from just below it (level
//!   `L+1` alone) by the residual weight only, where the unblended picture
//!   pops by the full aliasing of level `L`.

use super::{CanvasPipeline, TileResidency, paint, render_and_sample_blended};
use crate::test_support::real_context;
use aurora_tile::{SurfaceId, TILE, TileId, TileStore};
use half::f16;
use std::num::NonZeroUsize;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn store(budget: usize) -> (tempfile::TempDir, TileStore) {
    let Ok(dir) = tempfile::tempdir() else {
        unreachable!("a test can make a temp dir");
    };
    let Some(budget) = NonZeroUsize::new(budget) else {
        unreachable!("the budget is non-zero");
    };
    let Ok(store) = TileStore::new(dir.path().to_path_buf(), budget) else {
        unreachable!("a fresh scratch dir is usable");
    };
    (dir, store)
}

fn every_pixel(size: (u32, u32)) -> Vec<(u32, u32)> {
    (0..size.1)
        .flat_map(|y| (0..size.0).map(move |x| (x, y)))
        .collect()
}

/// Fills tile `id` with 1-px vertical stripes: even document columns
/// opaque white, odd ones opaque black (a pattern whose 2 x 2 mean is a
/// flat 0.5 grey and which aliases as hard as anything can when minified).
fn paint_stripes(store: &mut TileStore, surface: SurfaceId, id: TileId) {
    let Ok(tile) = store.get_mut(surface, id) else {
        unreachable!("a fresh store accepts the tile");
    };
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let column = (i / 4) as u32 % TILE;
        let value = if i % 4 == 3 || column % 2 == 0 {
            1.0
        } else {
            0.0
        };
        *sample = f16::from_f32(value);
    }
}

/// Fills tile `id` with a pattern that varies per texel (so a stray partner
/// sample anywhere would show).
fn paint_ramp(store: &mut TileStore, surface: SurfaceId, id: TileId) {
    let Ok(tile) = store.get_mut(surface, id) else {
        unreachable!("a fresh store accepts the tile");
    };
    for (i, sample) in tile.texels_mut().iter_mut().enumerate() {
        let texel = (i / 4) as u32;
        let (x, y) = (texel % TILE, texel / TILE);
        let value = match i % 4 {
            0 => (x as f32 + 0.5) / TILE as f32,
            1 => (y as f32 + 0.5) / TILE as f32,
            2 => ((x ^ y) % 7) as f32 / 6.0,
            _ => 0.25 + 0.75 * ((x + 3 * y) % 5) as f32 / 4.0,
        };
        *sample = f16::from_f32(value);
    }
}

fn atlas_over(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    store: &mut TileStore,
    surface: SurfaceId,
    viewport: (u32, u32),
    zoom: f32,
    weight: f32,
) -> TileResidency {
    let mut atlas = TileResidency::with_coverage(device, queue, viewport, 2);
    atlas.set_blend_weight(weight);
    atlas.set_origin(queue, (0.0, 0.0), viewport, zoom);
    let stats = atlas.sync(queue, store, surface, false, usize::MAX);
    assert_eq!(stats.errors, 0);
    atlas
}

#[test]
fn the_blend_weight_is_made_safe() {
    // Pure: `set_blend_weight` needs an atlas, so this is checked through a
    // real one when there is a GPU, and is the reason a NaN weight can
    // never reach the shader's `> 0.0` test as anything but zero.
    let Some(context) = real_context() else {
        return;
    };
    let mut atlas = TileResidency::new(context.device(), context.queue(), (256, 256));
    assert_eq!(atlas.blend_weight().to_bits(), 0.0_f32.to_bits());
    for (weight, expected) in [
        (f32::NAN, 0.0),
        (-0.5, 0.0),
        (-0.0, 0.0),
        (0.0, 0.0),
        (0.25, 0.25),
        (1.0, 1.0),
        (7.0, 1.0),
        (f32::INFINITY, 1.0),
        (f32::NEG_INFINITY, 0.0),
    ] {
        atlas.set_blend_weight(weight);
        assert_eq!(
            atlas.blend_weight().to_bits(),
            f32::to_bits(expected),
            "{weight}"
        );
    }
}

#[test]
fn a_zero_blend_weight_is_bit_identical_to_the_unblended_canvas() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let (_dir, mut store) = store(64);
    let primary_surface = SurfaceId::from_raw(0);
    let partner_surface = SurfaceId::from_raw(1);
    let viewport = (300, 300);
    for y in 0..4 {
        for x in 0..4 {
            paint_ramp(&mut store, primary_surface, TileId { x, y });
            paint(
                &mut store,
                partner_surface,
                TileId { x, y },
                [1.0, 0.0, 1.0, 1.0],
            );
        }
    }
    let mut canvas = CanvasPipeline::new(device);
    let pixels = every_pixel(viewport);
    for zoom in [1.0, 0.7, 0.55] {
        let primary = atlas_over(
            device,
            queue,
            &mut store,
            primary_surface,
            viewport,
            zoom,
            0.0,
        );
        let partner = atlas_over(
            device,
            queue,
            &mut store,
            partner_surface,
            viewport,
            zoom * 2.0,
            0.0,
        );
        let alone = render_and_sample_blended(
            device,
            queue,
            &mut canvas,
            (&primary, None),
            viewport,
            &pixels,
            FORMAT,
        );
        let blended = render_and_sample_blended(
            device,
            queue,
            &mut canvas,
            (&primary, Some(&partner)),
            viewport,
            &pixels,
            FORMAT,
        );
        assert!(
            alone == blended,
            "zoom {zoom}: a zero weight changed the picture"
        );
        // And the partner is really bound: a positive weight shows it.
        let mut primary = primary;
        primary.set_blend_weight(1.0);
        primary.set_origin(queue, (0.0, 0.0), viewport, zoom);
        let all_partner = render_and_sample_blended(
            device,
            queue,
            &mut canvas,
            (&primary, Some(&partner)),
            viewport,
            &[(150, 150)],
            FORMAT,
        );
        assert_eq!(all_partner, vec![[255, 0, 255, 255]], "zoom {zoom}");
    }
}

/// The reference for one channel: the atlas's premultiplied `f16` texels
/// mixed in `f64`, put over the checkerboard square `check`, as 8 bits.
fn reference(primary: [f32; 4], partner: [f32; 4], weight: f64, check: f64) -> [i32; 3] {
    let premultiplied = |rgba: [f32; 4]| {
        let a = f16::from_f32(rgba[3]).to_f32();
        let channel = |c: f32| f64::from(f16::from_f32(f16::from_f32(c).to_f32() * a).to_f32());
        [
            channel(rgba[0]),
            channel(rgba[1]),
            channel(rgba[2]),
            f64::from(a),
        ]
    };
    let (p, q) = (premultiplied(primary), premultiplied(partner));
    let mix = |i: usize| p[i] * (1.0 - weight) + q[i] * weight;
    let alpha = mix(3);
    let out = |i: usize| ((mix(i) + check * (1.0 - alpha)) * 255.0).round() as i32;
    [out(0), out(1), out(2)]
}

#[test]
fn the_blend_mixes_the_partner_in_premultiplied_space_before_the_checkerboard() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let (_dir, mut store) = store(8);
    let viewport = (256, 256);
    let primary_colour = [0.8, 0.2, 0.1, 1.0];
    let partner_colour = [0.1, 0.6, 0.9, 0.5];
    paint(
        &mut store,
        SurfaceId::from_raw(0),
        TileId { x: 0, y: 0 },
        primary_colour,
    );
    paint(
        &mut store,
        SurfaceId::from_raw(1),
        TileId { x: 0, y: 0 },
        partner_colour,
    );
    let mut canvas = CanvasPipeline::new(device);
    let mut primary = TileResidency::new(device, queue, viewport);
    let _ = primary.sync(queue, &mut store, SurfaceId::from_raw(0), false, usize::MAX);
    let mut partner = TileResidency::new(device, queue, viewport);
    let _ = partner.sync(queue, &mut store, SurfaceId::from_raw(1), false, usize::MAX);
    partner.set_origin(queue, (0.0, 0.0), viewport, 1.0);
    // (4, 4) is on a light (0.24) checkerboard square, (12, 4) a dark one.
    let points = [(4, 4), (12, 4)];
    for weight in [0.125_f32, 0.25, 0.5, 0.75, 0.985, 1.0] {
        primary.set_blend_weight(weight);
        primary.set_origin(queue, (0.0, 0.0), viewport, 1.0);
        let got = render_and_sample_blended(
            device,
            queue,
            &mut canvas,
            (&primary, Some(&partner)),
            viewport,
            &points,
            FORMAT,
        );
        for (pixel, check) in got.iter().zip([0.24_f32, 0.18]) {
            let expected = reference(
                primary_colour,
                partner_colour,
                f64::from(weight),
                f64::from(check),
            );
            for (channel, want) in pixel.iter().zip(expected) {
                assert!(
                    (i32::from(*channel) - want).abs() <= 1,
                    "weight {weight}: got {pixel:?}, expected {expected:?}"
                );
            }
            assert_eq!(pixel.get(3), Some(&255));
        }
    }
}

#[test]
fn the_blended_picture_is_continuous_across_a_level_boundary() {
    let Some(context) = real_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let (_dir, mut store) = store(64);
    let fine = SurfaceId::from_raw(0);
    let coarse = SurfaceId::from_raw(1);
    let viewport = (300, 300);
    assert!(TileResidency::min_zoom_for_viewport_with_coverage(viewport, 2) < 0.5);
    for y in 0..4 {
        for x in 0..4 {
            paint_stripes(&mut store, fine, TileId { x, y });
            paint(&mut store, coarse, TileId { x, y }, [0.5, 0.5, 0.5, 1.0]);
        }
    }
    let mut canvas = CanvasPipeline::new(device);
    let pixels = every_pixel(viewport);
    let eps = 0.01_f32;
    // Just above the boundary: the fine level at 0.5 * (1 + eps), mixed
    // toward the coarse one (the 2 x 2 mean, at twice that zoom) by the
    // weight `zoomed_view::blend_weight` gives, `-log2(level zoom)`.
    let level_zoom = 0.5 * (1.0 + eps);
    let weight = -level_zoom.log2();
    assert!(weight > 0.98 && weight < 1.0, "{weight}");
    let mut above = atlas_over(
        device, queue, &mut store, fine, viewport, level_zoom, weight,
    );
    let partner = atlas_over(
        device,
        queue,
        &mut store,
        coarse,
        viewport,
        level_zoom * 2.0,
        0.0,
    );
    // Just below it: the coarse level alone, at 1 - eps.
    let below = atlas_over(device, queue, &mut store, coarse, viewport, 1.0 - eps, 0.0);
    let render = |canvas: &mut CanvasPipeline, pair: (&TileResidency, Option<&TileResidency>)| {
        render_and_sample_blended(device, queue, canvas, pair, viewport, &pixels, FORMAT)
    };
    let below_px = render(&mut canvas, (&below, None));
    let blended_px = render(&mut canvas, (&above, Some(&partner)));
    above.set_blend_weight(0.0);
    above.set_origin(queue, (0.0, 0.0), viewport, level_zoom);
    let popped_px = render(&mut canvas, (&above, Some(&partner)));
    let worst = |a: &[[u8; 4]], b: &[[u8; 4]]| {
        a.iter()
            .zip(b)
            .flat_map(|(p, q)| {
                p.iter()
                    .zip(q)
                    .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
            })
            .max()
            .unwrap_or(0)
    };
    let blended = worst(&blended_px, &below_px);
    let popped = worst(&popped_px, &below_px);
    // The residual weight `1 - w` (about 0.0144) of a +-0.5 aliasing is at
    // most about 1.8 levels, plus one step of rounding on each side.
    assert!(blended <= 3, "blended picture jumps by {blended} levels");
    // The control: without the blend the same boundary pops by tens of
    // levels, so the test can see a pop.
    assert!(popped >= 60, "the unblended control only jumps by {popped}");
}
