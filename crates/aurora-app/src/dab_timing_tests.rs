//! The redraw handler's synchronous dab recomposite, timed (60 FPS round
//! 7): two or three stale composite tiles of a CPU-path document (an
//! opaque backdrop under a translucent `Exclusion` layer), recomposited
//! through [`crate::recomposite_tiles`] over a 4 x 4-tile view, as a brush
//! dab leaves them. Prints the per-run mean; run with `--ignored
//! --nocapture` in the release profile.

use crate::occlusion_cull_tests::{layer, ok, opaque, rect, store_with_budget, translucent};

#[test]
#[ignore = "timing only; run with --ignored --nocapture"]
fn time_a_dab_recomposite() {
    // The shipping decision: the pool is on (tests default to off).
    crate::frame_composite::set_parallel_enabled_for_tests(true); // R7-ONLY
    let (_d, mut store) = store_with_budget(256);
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 1024, 1024),
        &opaque,
    );
    let top = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 1024, 1024),
        &translucent,
    );
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Exclusion));
    let tiles: Vec<_> = (0..4)
        .flat_map(|y| (0..4).map(move |x| aurora_tile::TileId { x, y }))
        .collect();
    let mut cache = crate::CompositeCache::default();
    crate::recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
    for stale in [2_usize, 3] {
        let runs = 200_u32;
        let mut total = std::time::Duration::ZERO;
        for run in 0..runs {
            let first = (run as usize * 5) % tiles.len();
            for k in 0..stale {
                if let Some(&id) = tiles.get((first + k) % tiles.len()) {
                    cache.note_invalidated(id);
                }
            }
            let started = std::time::Instant::now();
            crate::recomposite_tiles(&tiles, &layers, None, &mut store, &mut cache, None, None);
            total += started.elapsed();
        }
        println!(
            "dab timing: {stale} stale tiles: mean {:.3} ms",
            total.as_secs_f64() * 1000.0 / f64::from(runs)
        );
    }
    crate::frame_composite::set_parallel_enabled_for_tests(false); // R7-ONLY
}
