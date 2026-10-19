//! Pure tests of the display texture's bookkeeping (task 142): the budget,
//! the page plan, the scissor partition, the level of detail, the uniform
//! and the mip rectangles. The GPU differentials are in
//! `display_gpu_test.rs`.

// Exact values are the point: the level of detail and the uniform are
// exactly representable.
#![allow(clippy::float_cmp)]

use super::{
    DISCRETE_TEXEL_CAP, PageLayout, axis_segments, display_budget, display_bytes, doc_tiles,
    lod_for_zoom, mask_outside, mip_rects, page_scissors, page_uniform, plan_pages, texture_bytes,
};
use aurora_tile::{TILE, TileId};

const GIB: u64 = 1 << 30;

#[test]
fn a_discrete_gpu_gets_the_texel_cap_and_a_shared_one_a_ram_share() {
    let cap = DISCRETE_TEXEL_CAP * 8 * 4 / 3;
    let discrete = display_budget(wgpu::DeviceType::DiscreteGpu, Some(64 * GIB), false);
    assert_eq!(discrete.max_bytes, cap);
    // RAM does not bound a discrete card.
    let tiny = display_budget(wgpu::DeviceType::DiscreteGpu, Some(GIB), false);
    assert_eq!(tiny.max_bytes, cap);
    for kind in [
        wgpu::DeviceType::IntegratedGpu,
        wgpu::DeviceType::Cpu,
        wgpu::DeviceType::VirtualGpu,
        wgpu::DeviceType::Other,
    ] {
        assert_eq!(
            display_budget(kind, Some(8 * GIB), false).max_bytes,
            GIB,
            "{kind:?}: one eighth of 8 GiB"
        );
        assert_eq!(
            display_budget(kind, Some(64 * GIB), false).max_bytes,
            cap,
            "{kind:?}: capped like a discrete card"
        );
        assert_eq!(display_budget(kind, None, false).max_bytes, 512 << 20);
    }
    assert_eq!(
        display_budget(wgpu::DeviceType::DiscreteGpu, None, true).max_bytes,
        cap / 2,
        "a small machine halves it"
    );
}

#[test]
fn sixty_megapixels_fit_a_discrete_budget_and_need_two_pages_at_8192() {
    let size = (9504, 6336);
    assert_eq!(doc_tiles(size), (38, 25));
    let pages = plan_pages(doc_tiles(size), 8192 / TILE).unwrap_or_default();
    assert_eq!(pages.len(), 2);
    let Some(bytes) = display_bytes(size, 8192, 6) else {
        unreachable!("pageable");
    };
    // 40 x 25 tiles held (two gutter columns), 6 levels: about 700 MB.
    assert_eq!(
        bytes,
        texture_bytes(31 * TILE, 25 * TILE, 6) + texture_bytes(9 * TILE, 25 * TILE, 6)
    );
    assert!(bytes < display_budget(wgpu::DeviceType::DiscreteGpu, None, false).max_bytes);
    // One page on a 16,384 device.
    assert_eq!(
        plan_pages(doc_tiles(size), 16384 / TILE).map(|p| p.len()),
        Some(1)
    );
}

#[test]
fn texture_bytes_counts_every_level() {
    assert_eq!(texture_bytes(256, 256, 1), 256 * 256 * 8);
    assert_eq!(texture_bytes(256, 256, 2), (256 * 256 + 128 * 128) * 8);
    let mut sum = 0;
    let (mut w, mut h) = (512_u64, 256_u64);
    for _ in 0..10 {
        sum += w.max(1) * h.max(1) * 8;
        w /= 2;
        h /= 2;
    }
    assert_eq!(texture_bytes(512, 256, 10), sum);
}

#[test]
fn an_axis_within_the_limit_is_one_segment_with_no_gutter() {
    assert_eq!(axis_segments(5, 32), Some(vec![(0, 5, 0, 5)]));
    assert_eq!(axis_segments(32, 32), Some(vec![(0, 32, 0, 32)]));
    assert_eq!(axis_segments(0, 32), None);
    assert_eq!(axis_segments(5, 0), None);
}

#[test]
fn a_split_axis_has_a_one_tile_gutter_on_each_inner_side() {
    let Some(segments) = axis_segments(10, 4) else {
        unreachable!("splittable");
    };
    // Stride 2: owned [0,2) [2,4) [4,6) [6,8) [8,10).
    assert_eq!(
        segments,
        vec![
            (0, 3, 0, 2),
            (1, 4, 2, 4),
            (3, 4, 4, 6),
            (5, 4, 6, 8),
            (7, 3, 8, 10)
        ]
    );
    for &(first, len, owned_first, owned_end) in &segments {
        assert!(len <= 4, "within the limit");
        if owned_first > 0 {
            assert_eq!(first, owned_first - 1, "a gutter tile before");
        } else {
            assert_eq!(first, 0);
        }
        if owned_end < 10 {
            assert_eq!(first + len, owned_end + 1, "a gutter tile after");
        } else {
            assert_eq!(first + len, 10);
        }
    }
    // The owned ranges partition the axis.
    let mut next = 0;
    for &(_, _, owned_first, owned_end) in &segments {
        assert_eq!(owned_first, next);
        next = owned_end;
    }
    assert_eq!(next, 10);
    // A limit with no room for two gutters cannot split.
    assert_eq!(axis_segments(10, 2), None);
}

#[test]
fn pages_partition_the_document_and_every_tile_is_held_where_it_is_owned() {
    for (tiles, limit) in [((38, 25), 32), ((7, 9), 3), ((59, 40), 32), ((4, 4), 8)] {
        let Some(pages) = plan_pages(tiles, limit) else {
            unreachable!("pageable");
        };
        for y in 0..tiles.1 {
            for x in 0..tiles.0 {
                let id = TileId { x, y };
                let owners: Vec<&PageLayout> = pages
                    .iter()
                    .filter(|p| {
                        x >= p.owned_first.0
                            && x < p.owned_end.0
                            && y >= p.owned_first.1
                            && y < p.owned_end.1
                    })
                    .collect();
                assert_eq!(owners.len(), 1, "{tiles:?} {id:?} has one owner");
                assert!(owners.iter().all(|p| p.holds(id)));
            }
        }
        for page in &pages {
            assert!(page.tiles.0 <= limit && page.tiles.1 <= limit);
        }
    }
}

/// Every canvas pixel is drawn by exactly one page, and the page drawing
/// it owns the document position its centre samples.
#[test]
fn page_scissors_partition_the_canvas_by_the_sampled_position() {
    let tiles = (7, 5);
    let Some(pages) = plan_pages(tiles, 3) else {
        unreachable!("pageable");
    };
    let viewport = (300, 200);
    for &(origin, zoom) in &[
        ((0.0, 0.0), 1.0_f32),
        ((10.5, 3.25), 1.0),
        ((-40.0, -20.0), 0.25),
        ((100.0, 50.0), 0.5),
        ((300.0, 260.0), 2.0),
        ((511.75, 255.5), 3.0),
        ((0.0, 0.0), 0.11),
    ] {
        let scissors = page_scissors(&pages, origin, viewport, zoom, tiles);
        let mut count = vec![0_u32; (viewport.0 * viewport.1) as usize];
        for &(page, x0, y0, x1, y1) in &scissors {
            let Some(layout) = pages.get(page) else {
                unreachable!("index from the list");
            };
            for y in y0..y1 {
                for x in x0..x1 {
                    if let Some(slot) = count.get_mut((y * viewport.0 + x) as usize) {
                        *slot += 1;
                    }
                    let doc = (
                        f64::from(origin.0) + (f64::from(x) + 0.5) / f64::from(zoom),
                        f64::from(origin.1) + (f64::from(y) + 0.5) / f64::from(zoom),
                    );
                    let tile = (doc.0 / f64::from(TILE), doc.1 / f64::from(TILE));
                    let inside = |t: f64, first: u32, end: u32, total: u32| {
                        (first == 0 || t >= f64::from(first))
                            && (end >= total || t < f64::from(end))
                    };
                    assert!(
                        inside(tile.0, layout.owned_first.0, layout.owned_end.0, tiles.0)
                            && inside(tile.1, layout.owned_first.1, layout.owned_end.1, tiles.1),
                        "pixel ({x}, {y}) at {doc:?} is drawn by page {page} {layout:?}"
                    );
                }
            }
        }
        assert!(
            count.iter().all(|&c| c == 1),
            "{origin:?} x{zoom}: every pixel drawn once"
        );
    }
}

#[test]
fn one_page_draws_the_whole_canvas() {
    let tiles = (3, 2);
    let pages = plan_pages(tiles, 32).unwrap_or_default();
    assert_eq!(
        page_scissors(&pages, (-1000.0, 5.0), (640, 480), 0.7, tiles),
        vec![(0, 0, 0, 640, 480)]
    );
}

#[test]
fn the_level_of_detail_is_minus_log2_of_the_zoom_clamped_to_the_chain() {
    assert_eq!(lod_for_zoom(1.0, 6), 0.0);
    assert_eq!(lod_for_zoom(3.0, 6), 0.0, "magnified: level 0");
    assert_eq!(lod_for_zoom(0.5, 6), 1.0);
    assert_eq!(lod_for_zoom(0.25, 6), 2.0);
    assert_eq!(lod_for_zoom(1.0 / 32.0, 6), 5.0);
    assert_eq!(lod_for_zoom(1.0 / 64.0, 6), 5.0, "clamped to the chain");
    let between = lod_for_zoom(0.75, 6);
    assert!(between > 0.41 && between < 0.42, "{between}");
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(lod_for_zoom(bad, 6), 0.0, "{bad} reads as 1.0");
    }
    assert_eq!(lod_for_zoom(0.25, 1), 0.0, "one level");
}

#[test]
fn the_uniform_maps_the_viewport_onto_the_page_and_clips_like_the_atlas() {
    let values = page_uniform(
        (300.0, 100.0),
        (400, 200),
        2.0,
        (256.0, 0.0),
        (512, 256),
        Some((0.0, 0.0, 700.0, 450.0)),
        [1.0, 0.0, f32::NAN],
        0.0,
    );
    let expected = [
        (300.0 - 256.0) / 512.0,
        100.0 / 256.0,
        200.0 / 512.0,
        100.0 / 256.0,
        // The atlas's `clip_values_c`: (x - scroll) * zoom / viewport.
        (0.0 - 300.0) * 2.0 / 400.0,
        (0.0 - 100.0) * 2.0 / 200.0,
        (700.0 - 300.0) * 2.0 / 400.0,
        (450.0 - 100.0) * 2.0 / 200.0,
        1.0,
        0.0,
        1.0,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
    ];
    assert_eq!(values, expected);
    let none = page_uniform(
        (0.0, 0.0),
        (1, 1),
        1.0,
        (0.0, 0.0),
        (256, 256),
        None,
        [1.0; 3],
        3.5,
    );
    assert_eq!(none.get(4..8), Some(&[-1.0e9, -1.0e9, 1.0e9, 1.0e9][..]));
    assert_eq!(none.get(13), Some(&3.5), "the level of detail");
    let bad = page_uniform(
        (0.0, 0.0),
        (1, 1),
        1.0,
        (0.0, 0.0),
        (256, 256),
        Some((f32::NAN, 0.0, 1.0, 1.0)),
        [1.0; 3],
        0.0,
    );
    assert_eq!(
        bad.get(4..8),
        Some(&[0.0; 4][..]),
        "a bad clip clips everything"
    );
}

#[test]
fn mip_rects_merge_runs_and_scale_with_the_level() {
    let tiles = [(0, 0), (1, 0), (2, 0), (5, 0), (1, 1), (1, 0)];
    assert_eq!(
        mip_rects(&tiles, 1),
        vec![(0, 0, 384, 128), (640, 0, 128, 128), (128, 128, 128, 128)]
    );
    assert_eq!(mip_rects(&[(3, 2)], 8), vec![(3, 2, 1, 1)]);
    assert_eq!(
        mip_rects(&[(3, 2)], 9),
        vec![(3, 2, 1, 1)],
        "clamped at one texel"
    );
    assert!(mip_rects(&[], 1).is_empty());
}

#[test]
fn texels_outside_the_document_are_zeroed() {
    let row = TILE as usize * 8;
    let mut bytes = vec![0xAB_u8; row * TILE as usize];
    mask_outside(&mut bytes, (3, 2));
    for (y, line) in bytes.chunks_exact(row).enumerate() {
        for (x, texel) in line.chunks_exact(8).enumerate() {
            let kept = x < 3 && y < 2;
            assert_eq!(texel.iter().all(|&b| b == 0xAB), kept, "texel ({x}, {y})");
            assert_eq!(texel.iter().all(|&b| b == 0), !kept, "texel ({x}, {y})");
        }
    }
}
