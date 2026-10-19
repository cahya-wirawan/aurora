//! Task 140: the blend between two pyramid levels. Pure tests of the
//! weight and the level pair, and a real-GPU test of
//! [`super::super::ZoomedAtlases`] (the glue `App::prepare_zoomed_frame`
//! calls, which needs no window).

use std::time::Duration;

use aurora_tile::TileId;

use super::super::{
    FrameDocument, Level0, MAX_LEVEL, MIN_PHYSICAL_ZOOM, ZoomedAtlases, atlas_floor, blend_weight,
    level_scale, level_surface, plan, plan_blended, zoomed_floor,
};
use super::{box_filtered, document};

const CANVAS: (u32, u32) = (1920, 1080);
const DOC: (u32, u32) = (24_000, 16_000);

fn frame_at(zoom: f32, limit: u32) -> super::super::ZoomedFrame {
    plan_blended(
        (0.0, 0.0),
        zoom,
        CANVAS,
        DOC,
        zoomed_floor(CANVAS, limit),
        atlas_floor(CANVAS, limit),
    )
}

#[test]
fn a_power_of_two_zoom_does_not_blend() {
    for level in 1..=MAX_LEVEL {
        let zoom = 1.0 / level_scale(level);
        let frame = frame_at(zoom, u32::MAX);
        assert_eq!(frame.level, level);
        assert_eq!(frame.level_zoom, 1.0);
        assert_eq!(frame.blend, None, "zoom {zoom}");
        assert_eq!(blend_weight(level, 1.0, 0.94).to_bits(), 0);
    }
    // And the blended plan is the unblended one but for the blend.
    for zoom in [0.9_f32, 0.6, 0.3, 0.07, 0.04] {
        let mut blended = frame_at(zoom, u32::MAX);
        assert!(blended.blend.is_some(), "zoom {zoom}");
        blended.blend = None;
        let plain = plan(
            (0.0, 0.0),
            zoom,
            CANVAS,
            DOC,
            zoomed_floor(CANVAS, u32::MAX),
        );
        assert_eq!(blended, plain, "zoom {zoom}");
    }
}

#[test]
fn the_weight_is_the_fractional_level_of_detail() {
    for level in 1..MAX_LEVEL {
        for (level_zoom, weight) in [(1.0_f32, 0.0_f32), (0.75, 0.415_037_5), (0.5, 1.0)] {
            let got = blend_weight(level, level_zoom, 0.94);
            assert!((got - weight).abs() < 1e-6, "{level} {level_zoom}: {got}");
        }
        // Just above and below a power of two.
        assert!(blend_weight(level, 0.999, 0.94) < 0.002);
        assert!(blend_weight(level, 0.5001, 0.94) > 0.999);
    }
    // No deeper level than the deepest, nothing for a magnified level
    // (a capped atlas, ZV-1), nothing for a degenerate zoom.
    assert_eq!(blend_weight(MAX_LEVEL, 0.7, 0.94), 0.0);
    assert_eq!(blend_weight(MAX_LEVEL + 3, 0.7, 0.94), 0.0);
    assert_eq!(blend_weight(2, 1.2, 0.94), 0.0);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -0.5] {
        assert_eq!(blend_weight(2, bad, 0.94), 0.0, "{bad}");
        assert_eq!(blend_weight(0, bad, 0.94), 0.0, "{bad}");
    }
    // Level 0 ramps from the 1:1 atlas's floor, whatever that floor is.
    assert_eq!(blend_weight(0, 0.94, 0.94), 0.0);
    assert_eq!(blend_weight(0, 0.97, 0.94), 0.0);
    assert!(blend_weight(0, 0.93, 0.94) < 0.02);
    assert!(blend_weight(0, 0.5001, 0.94) > 0.999);
    assert!(blend_weight(0, 0.75, f32::NAN) > 0.41);
    assert_eq!(blend_weight(0, 0.6, 0.5), 0.0, "a 1:1 floor at one half");
    assert_eq!(blend_weight(0, 0.6, 0.4), 0.0, "below one half too");
}

#[test]
fn the_shown_mix_is_continuous_from_the_one_to_one_floor_to_the_deepest_level() {
    for limit in [u32::MAX, 8192] {
        let one = atlas_floor(CANVAS, limit);
        let mut zoom = one * 0.999_99;
        let mut previous: Option<(u32, f32)> = None;
        while zoom >= MIN_PHYSICAL_ZOOM {
            let frame = frame_at(zoom, limit);
            let weight = frame.blend.as_ref().map_or(0.0, |blend| {
                assert_eq!(blend.level, frame.level + 1);
                assert!(blend.level <= MAX_LEVEL);
                assert_eq!(blend.level_zoom, frame.level_zoom * 2.0);
                assert_eq!(blend.level_origin.0, frame.level_origin.0 / 2.0);
                blend.weight
            });
            if let Some((level, last)) = previous {
                if level == frame.level {
                    assert!(
                        (weight - last).abs() < 0.01,
                        "zoom {zoom}: {last} -> {weight}"
                    );
                } else {
                    assert_eq!(frame.level, level + 1, "zoom {zoom}");
                    assert!(
                        last > 0.99 && weight < 0.01,
                        "zoom {zoom}: {last} -> {weight}"
                    );
                }
            } else {
                assert!(weight < 0.01, "the hand-over from the 1:1 atlas: {weight}");
            }
            previous = Some((frame.level, weight));
            zoom *= 0.999;
        }
    }
    // The floor itself is the deepest level alone.
    let floor = frame_at(MIN_PHYSICAL_ZOOM, u32::MAX);
    assert_eq!((floor.level, floor.blend.clone()), (MAX_LEVEL, None));
    let above = frame_at(MIN_PHYSICAL_ZOOM * 1.01, u32::MAX);
    assert_eq!(above.level, MAX_LEVEL - 1);
    assert_eq!(above.blend.as_ref().map(|b| b.level), Some(MAX_LEVEL));
    // A non-finite zoom plans level 0 at 1:1, which never blends.
    assert_eq!(frame_at(f32::NAN, u32::MAX).blend, None);
    assert_eq!(frame_at(f32::INFINITY, u32::MAX).blend, None);
}

#[test]
fn a_capped_atlas_blends_only_toward_a_level_it_can_render() {
    for canvas in super::BIG_CANVASES {
        let floor = zoomed_floor(canvas, 8192);
        let mut zoom = atlas_floor(canvas, 8192) * 0.999;
        while zoom >= super::super::physical_floor(canvas, 8192) {
            let frame = plan_blended(
                (0.0, 0.0),
                zoom,
                canvas,
                DOC,
                floor,
                atlas_floor(canvas, 8192),
            );
            if let Some(blend) = &frame.blend {
                assert!(frame.level_zoom < 1.0, "{canvas:?} {zoom}");
                assert!(blend.level_zoom >= floor, "{canvas:?} {zoom}");
            }
            zoom *= 0.99;
        }
    }
}

#[test]
fn the_next_level_covers_every_tile_the_frame_shows() {
    for (origin, zoom) in [
        ((0.0, 0.0), 0.3_f32),
        ((5000.0, 3000.0), 0.11),
        ((-400.0, -90.0), 0.7),
    ] {
        let frame = plan_blended(
            origin,
            zoom,
            CANVAS,
            DOC,
            zoomed_floor(CANVAS, u32::MAX),
            atlas_floor(CANVAS, u32::MAX),
        );
        let Some(blend) = frame.blend.as_ref() else {
            unreachable!("zoom {zoom} blends");
        };
        assert_eq!(blend.level, frame.level + 1);
        for id in &frame.visible {
            let parent = TileId {
                x: id.x / 2,
                y: id.y / 2,
            };
            if super::super::tile_in_document(blend.level, parent, DOC) {
                assert!(blend.visible.contains(&parent), "{zoom}: {id:?}");
            }
        }
    }
}

/// Prepares frames at `zoom` until the shown level is complete and, when
/// the frame blends, the blend is live; the last frame.
fn settle_atlases(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlases: &mut ZoomedAtlases,
    doc: &mut super::Doc,
    cache: &mut crate::CompositeCache,
    zoom: f32,
    viewport: (u32, u32),
) -> super::super::ZoomedFrame {
    settle_at(
        device,
        queue,
        atlases,
        doc,
        cache,
        zoom,
        (0.0, 0.0),
        viewport,
    )
}

/// [`settle_atlases`] with the view's corner at document point `origin`.
#[allow(clippy::too_many_arguments)]
fn settle_at(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlases: &mut ZoomedAtlases,
    doc: &mut super::Doc,
    cache: &mut crate::CompositeCache,
    zoom: f32,
    origin: (f32, f32),
    viewport: (u32, u32),
) -> super::super::ZoomedFrame {
    for _ in 0..256 {
        let mut frame = plan_blended(
            origin,
            zoom,
            viewport,
            doc.size,
            zoomed_floor(viewport, u32::MAX),
            atlas_floor(viewport, u32::MAX),
        );
        let wanted = frame.blend.is_some();
        let advance = atlases.prepare(
            device,
            queue,
            &mut frame,
            FrameDocument {
                cache_id: cache.id(),
                canvas: doc.size,
                layers: &doc.layers,
                active_layer: Some(doc.top),
                store: &mut doc.store,
                cache,
            },
            false,
            [1.0, 1.0, 1.0],
            Level0::Sync(None, None),
            Duration::from_mins(1),
        );
        let faded = frame
            .blend
            .as_ref()
            .is_none_or(|blend| atlases.primary().blend_weight() == blend.weight);
        if advance.complete && frame.blend.is_some() == wanted && faded {
            return frame;
        }
    }
    unreachable!("the frame never settles at {zoom}");
}

#[test]
fn the_atlases_blend_once_both_levels_are_built_and_swap_without_a_stale_slot() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let viewport = (512, 256);
    let mut atlases = ZoomedAtlases::new(device, queue, viewport);
    assert!(atlases.partner().is_none());
    // A cold first frame between levels 1 and 2 shows level 1 alone.
    let mut cold = plan_blended(
        (0.0, 0.0),
        0.3,
        viewport,
        doc.size,
        zoomed_floor(viewport, u32::MAX),
        atlas_floor(viewport, u32::MAX),
    );
    let _ = atlases.prepare(
        device,
        queue,
        &mut cold,
        FrameDocument {
            cache_id: cache.id(),
            canvas: doc.size,
            layers: &doc.layers,
            active_layer: Some(doc.top),
            store: &mut doc.store,
            cache: &mut cache,
        },
        false,
        [1.0, 1.0, 1.0],
        Level0::Sync(None, None),
        Duration::ZERO,
    );
    assert_eq!(cold.blend, None, "nothing is built yet");
    assert_eq!(atlases.primary().blend_weight(), 0.0);

    let blended = settle_atlases(
        device,
        queue,
        &mut atlases,
        &mut doc,
        &mut cache,
        0.3,
        viewport,
    );
    let Some(blend) = blended.blend.as_ref() else {
        unreachable!("0.3 blends levels 1 and 2");
    };
    assert_eq!((blended.level, blend.level), (1, 2));
    assert_eq!(atlases.primary().blend_weight(), blend.weight);
    assert!(atlases.partner().is_some());

    // Out to 0.25: level 2 alone, its slots handed over from the partner.
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    for zoom in [0.25_f32, 0.3, 0.25] {
        let frame = settle_atlases(
            device,
            queue,
            &mut atlases,
            &mut doc,
            &mut cache,
            zoom,
            viewport,
        );
        if zoom == 0.25 {
            assert_eq!((frame.level, frame.blend.clone()), (2, None));
            assert_eq!(atlases.primary().blend_weight(), 0.0);
            for (x, y) in [(3, 5), (200, 100), (300, 7), (511, 255), (260, 130)] {
                let got = crate::tests::render_and_sample_pixel(
                    device,
                    queue,
                    &mut canvas,
                    atlases.primary(),
                    viewport,
                    (x, y),
                );
                let want = box_filtered(&mut doc.store, doc.size, 2, x, y);
                for (channel, (g, w)) in got.iter().zip(want).take(3).enumerate() {
                    assert!(
                        (f64::from(*g) - (w * 255.0).round()).abs() <= 1.0,
                        "pixel ({x}, {y}) channel {channel}: {got:?} vs {want:?}"
                    );
                }
            }
        } else {
            assert_eq!(frame.blend.as_ref().map(|b| b.level), Some(2));
        }
    }
    // The pyramid surfaces the two atlases read are distinct.
    assert_ne!(level_surface(1), level_surface(2));
}

/// One frame at `zoom` with the view's corner at `origin`, under `budget`.
#[allow(clippy::too_many_arguments)]
fn prepare_once(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlases: &mut ZoomedAtlases,
    doc: &mut super::Doc,
    cache: &mut crate::CompositeCache,
    (zoom, origin): (f32, (f32, f32)),
    viewport: (u32, u32),
    forget: bool,
    budget: Duration,
) -> (super::super::ZoomedFrame, super::super::Advance) {
    let mut frame = plan_blended(
        origin,
        zoom,
        viewport,
        doc.size,
        zoomed_floor(viewport, u32::MAX),
        atlas_floor(viewport, u32::MAX),
    );
    let advance = atlases.prepare(
        device,
        queue,
        &mut frame,
        FrameDocument {
            cache_id: cache.id(),
            canvas: doc.size,
            layers: &doc.layers,
            active_layer: Some(doc.top),
            store: &mut doc.store,
            cache,
        },
        forget,
        [1.0, 1.0, 1.0],
        Level0::Sync(None, None),
        budget,
    );
    (frame, advance)
}

/// Every pixel of the frame as the canvas draws it.
fn render_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &mut aurora_gpu::CanvasPipeline,
    atlases: &ZoomedAtlases,
    frame: &super::super::ZoomedFrame,
    viewport: (u32, u32),
) -> Vec<[u8; 4]> {
    let bind_group = atlases.bind_group(canvas, device, frame);
    let samples: Vec<(u32, u32)> = (0..viewport.1)
        .flat_map(|y| (0..viewport.0).map(move |x| (x, y)))
        .collect();
    crate::tests::render_bound_pixels(device, queue, canvas, &bind_group, viewport, &samples)
}

fn worst(a: &[[u8; 4]], b: &[[u8; 4]]) -> i32 {
    a.iter()
        .zip(b)
        .flat_map(|(p, q)| {
            p.iter()
                .zip(q)
                .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
        })
        .max()
        .unwrap_or(0)
}

/// A brush dab on the top layer at document point `at`, invalidated as
/// the app does.
fn dab(doc: &mut super::Doc, cache: &mut crate::CompositeCache, at: (f32, f32)) {
    let reference_origin = crate::composite_reference_origin(&doc.layers, Some(doc.top));
    let outcome = aurora_brush::stamp_dab(
        &mut doc.store,
        doc.top_surface,
        at,
        20.0,
        [0.1, 0.9, 0.2],
        None,
    );
    crate::invalidate_painted_tiles(
        cache,
        &doc.layers,
        doc.top,
        outcome.painted(),
        reference_origin,
    );
}

const VIEW: (u32, u32) = (512, 256);

#[test]
fn a_dab_keeps_the_blend_and_the_picture_until_its_tiles_rebuild() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let mut atlases = ZoomedAtlases::new(device, queue, VIEW);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let settled = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    let weight = settled.blend.as_ref().map_or(0.0, |blend| blend.weight);
    assert!(weight > 0.5, "{weight}");
    let before = render_frame(device, queue, &mut canvas, &atlases, &settled, VIEW);
    dab(&mut doc, &mut cache, (300.0, 300.0));
    // A frame whose levels are not rebuilt yet (no pyramid budget, as a
    // frame waiting on the worker): the blend stays, and so does the
    // picture, every tile showing what it last held.
    for _ in 0..3 {
        let (frame, advance) = prepare_once(
            device,
            queue,
            &mut atlases,
            &mut doc,
            &mut cache,
            (0.3, (0.0, 0.0)),
            VIEW,
            false,
            Duration::ZERO,
        );
        assert!(!advance.complete, "the dab's ancestors are not rebuilt");
        assert_eq!(frame.blend.as_ref().map(|b| b.weight), Some(weight));
        assert_eq!(atlases.primary().blend_weight(), weight);
        let during = render_frame(device, queue, &mut canvas, &atlases, &frame, VIEW);
        assert!(worst(&before, &during) <= 1, "the view changed while stale");
    }
    let after = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    assert_eq!(after.blend.as_ref().map(|b| b.weight), Some(weight));
    let rebuilt = render_frame(device, queue, &mut canvas, &atlases, &after, VIEW);
    assert!(worst(&before, &rebuilt) > 8, "the dab arrives once rebuilt");
}

#[test]
fn a_pan_into_unbuilt_tiles_keeps_blending() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((4200, 1100));
    let mut cache = crate::CompositeCache::default();
    let mut atlases = ZoomedAtlases::new(device, queue, VIEW);
    let settled = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    let weight = settled.blend.as_ref().map_or(0.0, |blend| blend.weight);
    assert!(weight > 0.5);
    for step in 1..=4_u16 {
        let origin = (f32::from(step) * 600.0, 0.0);
        let (frame, advance) = prepare_once(
            device,
            queue,
            &mut atlases,
            &mut doc,
            &mut cache,
            (0.3, origin),
            VIEW,
            false,
            Duration::ZERO,
        );
        assert!(!advance.complete, "step {step}: new tiles are unbuilt");
        assert_eq!(
            frame.blend.as_ref().map(|b| b.weight),
            Some(weight),
            "step {step}"
        );
    }
    let done = settle_at(
        device,
        queue,
        &mut atlases,
        &mut doc,
        &mut cache,
        0.3,
        (2400.0, 0.0),
        VIEW,
    );
    assert_eq!(done.blend.as_ref().map(|b| b.weight), Some(weight));
}

#[test]
fn forgetting_both_atlases_restarts_the_fade_and_redraws_the_same_picture() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let mut atlases = ZoomedAtlases::new(device, queue, VIEW);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let settled = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    let weight = settled.blend.as_ref().map_or(0.0, |blend| blend.weight);
    let before = render_frame(device, queue, &mut canvas, &atlases, &settled, VIEW);
    // ZV-2 with both atlases: the 1:1 atlas drew last.
    let (frame, _) = prepare_once(
        device,
        queue,
        &mut atlases,
        &mut doc,
        &mut cache,
        (0.3, (0.0, 0.0)),
        VIEW,
        true,
        Duration::ZERO,
    );
    // Both levels are still current, so the partner is complete again at
    // once, but its latch restarted: the first frame fades in.
    let first = frame.blend.as_ref().map_or(0.0, |blend| blend.weight);
    assert_eq!(first, weight);
    let shown = atlases.primary().blend_weight();
    assert!(shown > 0.0 && shown < weight, "{shown} of {weight}");
    let again = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    let after = render_frame(device, queue, &mut canvas, &atlases, &again, VIEW);
    assert!(worst(&before, &after) <= 1);
}

#[test]
fn a_new_composite_cache_rekeys_both_atlases() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let mut atlases = ZoomedAtlases::new(device, queue, VIEW);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let settled = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    let before = render_frame(device, queue, &mut canvas, &atlases, &settled, VIEW);
    let old = cache.id();
    assert_eq!(
        atlases.keys(),
        (Some((old, doc.size, 1)), Some((old, doc.size, 2)))
    );
    // A document switch: another cache, nothing current in it.
    let mut fresh = crate::CompositeCache::default();
    assert_ne!(fresh.id(), old);
    let (frame, _) = prepare_once(
        device,
        queue,
        &mut atlases,
        &mut doc,
        &mut fresh,
        (0.3, (0.0, 0.0)),
        VIEW,
        false,
        Duration::ZERO,
    );
    assert_eq!(frame.blend, None, "a cold partner under a new key");
    assert_eq!(atlases.keys().0, Some((fresh.id(), doc.size, 1)));
    let again = settle_atlases(device, queue, &mut atlases, &mut doc, &mut fresh, 0.3, VIEW);
    assert_eq!(
        atlases.keys(),
        (
            Some((fresh.id(), doc.size, 1)),
            Some((fresh.id(), doc.size, 2))
        )
    );
    let after = render_frame(device, queue, &mut canvas, &atlases, &again, VIEW);
    assert!(worst(&before, &after) <= 1);
}

#[test]
fn a_stale_partner_swapped_back_into_the_primary_uploads_what_changed_at_its_level() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let (device, queue) = (context.device(), context.queue());
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let mut atlases = ZoomedAtlases::new(device, queue, VIEW);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    let _ = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.3, VIEW);
    // 0.5 shows level 1 alone: the partner keeps level 2, unsynced.
    let half = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.5, VIEW);
    assert_eq!((half.level, half.blend.clone()), (1, None));
    let id = cache.id();
    assert_eq!(atlases.keys().1, Some((id, doc.size, 2)));
    // An edit at level 2 while nothing uploads it there.
    dab(&mut doc, &mut cache, (300.0, 300.0));
    let _ = settle_atlases(device, queue, &mut atlases, &mut doc, &mut cache, 0.5, VIEW);
    // 0.25: level 2 comes back into the primary by a swap.
    let quarter = settle_atlases(
        device,
        queue,
        &mut atlases,
        &mut doc,
        &mut cache,
        0.25,
        VIEW,
    );
    assert_eq!((quarter.level, quarter.blend.clone()), (2, None));
    assert_eq!(atlases.keys().0, Some((id, doc.size, 2)));
    let pixels = render_frame(device, queue, &mut canvas, &atlases, &quarter, VIEW);
    for (x, y) in [(75, 75), (70, 72), (80, 78), (3, 5), (300, 7), (511, 255)] {
        let got = pixels
            .get((y * VIEW.0 + x) as usize)
            .copied()
            .unwrap_or_default();
        let want = box_filtered(&mut doc.store, doc.size, 2, x, y);
        for (g, w) in got.iter().zip(want).take(3) {
            assert!(
                (f64::from(*g) - (w * 255.0).round()).abs() <= 1.0,
                "({x}, {y}): {got:?} vs {want:?}"
            );
        }
    }
}

#[test]
fn a_weight_below_one_step_is_not_drawn() {
    // Just below a power of two the next level would add under 1/256.
    let zoom = 0.25 * 0.999;
    let frame = frame_at(zoom, u32::MAX);
    assert!(blend_weight(frame.level, frame.level_zoom, 0.94) > 0.0);
    assert!(blend_weight(frame.level, frame.level_zoom, 0.94) < super::super::BLEND_EPSILON);
    assert_eq!(frame.blend, None);
}
