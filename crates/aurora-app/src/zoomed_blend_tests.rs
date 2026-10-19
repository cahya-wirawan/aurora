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
    for _ in 0..64 {
        let mut frame = plan_blended(
            (0.0, 0.0),
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
        if advance.complete && frame.blend.is_some() == wanted {
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
