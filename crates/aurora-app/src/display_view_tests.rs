//! Pure tests of the display-texture view's decisions (task 142). The GPU
//! tests (uploads, invalidation, the pictures) are in
//! `tests/display_frame_tests.rs`.

use super::{DisplayMode, eligible, mode_from, visible_tiles};
use aurora_tile::TileId;

const GIB: u64 = 1 << 30;

fn budget(max_bytes: u64) -> aurora_gpu::DisplayBudget {
    aurora_gpu::DisplayBudget { max_bytes }
}

#[test]
fn the_environment_turns_it_off_forces_it_or_leaves_it_to_the_budget() {
    for off in ["0", "off", "OFF", " false ", "no"] {
        assert_eq!(mode_from(Some(off)), DisplayMode::Off, "{off:?}");
    }
    for on in ["1", "on", "force", "TRUE", "yes"] {
        assert_eq!(mode_from(Some(on)), DisplayMode::Force, "{on:?}");
    }
    for auto in [None, Some(""), Some("auto"), Some("2")] {
        assert_eq!(mode_from(auto), DisplayMode::Auto, "{auto:?}");
    }
}

#[test]
fn a_document_is_eligible_only_within_the_budget_unless_forced() {
    let sixty = (9504, 6336);
    let Some(bytes) = aurora_gpu::display_bytes(sixty, 8192, super::DISPLAY_MIP_LEVELS) else {
        unreachable!("pageable");
    };
    assert!(bytes > 600 << 20 && bytes < 800 << 20, "{bytes}");
    assert!(eligible(sixty, 8192, budget(bytes), DisplayMode::Auto));
    assert!(!eligible(sixty, 8192, budget(bytes - 1), DisplayMode::Auto));
    assert!(eligible(sixty, 8192, budget(0), DisplayMode::Force));
    assert!(!eligible(sixty, 8192, budget(64 * GIB), DisplayMode::Off));
    // Not pageable: never, forced or not.
    assert!(!eligible(
        (0, 100),
        8192,
        budget(64 * GIB),
        DisplayMode::Force
    ));
    assert!(!eligible(
        (5000, 100),
        512,
        budget(64 * GIB),
        DisplayMode::Force
    ));
    // The discrete default holds 150 MP of level 0 with its chain; whole
    // tiles, page gutters and six levels make a 96 MP document about the
    // largest that fits on an 8,192 device, not a 300k-ceiling document.
    let discrete = aurora_gpu::display_budget(wgpu::DeviceType::DiscreteGpu, None, false);
    assert!(eligible((12000, 8000), 8192, discrete, DisplayMode::Auto));
    assert!(!eligible((20000, 12000), 8192, discrete, DisplayMode::Auto));
}

fn ids(v: &[TileId]) -> Vec<(u32, u32)> {
    v.iter().map(|id| (id.x, id.y)).collect()
}

#[test]
fn at_100_percent_the_visible_tiles_are_those_under_the_canvas() {
    // 512 x 256 canvas at (100, 0): document x 100..612, y 0..256.
    let tiles = visible_tiles((100.0, 0.0), 1.0, (512, 256), (2000, 2000));
    assert_eq!(
        ids(&tiles),
        vec![(0, 0), (1, 0), (2, 0)],
        "no filter reach at level 0"
    );
    // Magnified: fewer.
    let tiles = visible_tiles((0.0, 0.0), 2.0, (512, 512), (2000, 2000));
    assert_eq!(ids(&tiles), vec![(0, 0)]);
}

#[test]
fn zoomed_out_the_visible_tiles_reach_one_texel_of_the_sampled_level_and_stay_in_the_document() {
    // At 0.5 (level 1, trilinear up to level 2: 4 px of reach) a view
    // ending exactly on a tile edge reaches into the next tile.
    let tiles = visible_tiles((0.0, 0.0), 0.5, (256, 128), (2000, 2000));
    assert_eq!(
        ids(&tiles),
        vec![(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1)]
    );
    // A zoomed-out view left of and above the document (centred) starts
    // at tile 0 and ends at the document.
    let tiles = visible_tiles((-500.0, -300.0), 0.1, (1920, 1080), (1000, 600));
    assert_eq!(
        ids(&tiles),
        vec![
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2)
        ]
    );
    // Nothing of the document under the canvas: nothing.
    assert!(visible_tiles((5000.0, 0.0), 1.0, (100, 100), (1000, 600)).is_empty());
    // A degenerate zoom reads as 1.0.
    assert_eq!(
        visible_tiles((0.0, 0.0), f32::NAN, (256, 256), (1000, 600)),
        visible_tiles((0.0, 0.0), 1.0, (256, 256), (1000, 600))
    );
}

/// The documents either side of the discrete budget, one tile column
/// apart: the one below is drawn through the display texture, the one
/// above keeps the atlas and the pyramid.
#[test]
fn documents_just_below_and_just_above_the_budget_take_their_own_paths() {
    let discrete = aurora_gpu::display_budget(wgpu::DeviceType::DiscreteGpu, None, false);
    let height = 8000;
    let tile = aurora_tile::TILE;
    let mut width = tile;
    while eligible((width + tile, height), 8192, discrete, DisplayMode::Auto) {
        width += tile;
    }
    let below = (width, height);
    let above = (width + 1, height);
    assert!(eligible(below, 8192, discrete, DisplayMode::Auto));
    assert!(!eligible(above, 8192, discrete, DisplayMode::Auto));
    let bytes = |size| aurora_gpu::display_bytes(size, 8192, super::DISPLAY_MIP_LEVELS);
    assert!(bytes(below).is_some_and(|b| b <= discrete.max_bytes));
    assert!(bytes(above).is_some_and(|b| b > discrete.max_bytes));
    // Forced, the one above is drawn too; off, neither.
    assert!(eligible(above, 8192, discrete, DisplayMode::Force));
    assert!(!eligible(below, 8192, discrete, DisplayMode::Off));
}
