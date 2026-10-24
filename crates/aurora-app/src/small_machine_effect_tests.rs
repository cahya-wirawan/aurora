// ---- 0.192.0: small-machine bounds ----
//
// Included into `fill_and_overlay_tests` (it uses that module's helpers:
// `painted`, `composite_tile_at`, `shadow`, `real_tile_store`,
// `clear_shadow_cache`, `computes`).

use crate::small_machine::{self, force_small_machine};

/// Two layers, each with a drop shadow, an outside stroke and an outer
/// glow, whose coverage crosses tile boundaries; composited over a 3 x 3
/// tile grid (nine tiles, three coverage kinds each).
fn bounds_fixture() -> (
    tempfile::TempDir,
    aurora_tile::TileStore,
    aurora_doc::LayerTree,
) {
    let (dir, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    for rect in [(240, 100, 12, 12), (500, 500, 30, 30)] {
        let id = painted(&mut layers, &mut store, (0, 0), rect);
        let fx = aurora_core::LayerEffects {
            drop_shadows: vec![shadow(135.0, 9.0, 8.0, 0.0)],
            strokes: vec![stroke(StrokePosition::Outside, 5.0, [1.0, 0.0, 0.0])],
            outer_glows: vec![glow(14.0, 0.0, [0.2, 0.8, 1.0])],
            ..aurora_core::LayerEffects::default()
        };
        ok(layers.set_effects(id, Some(fx)));
    }
    (dir, store, layers)
}

/// Both passes over the nine tiles with the thread's cache replaced by one
/// holding `cap_bytes`; the tiles, the coverages computed and the most
/// entries the cache ever held.
fn bounded_composite(cap_bytes: usize) -> (Vec<Vec<half::f16>>, usize, usize) {
    let (_d, mut store, layers) = bounds_fixture();
    SHADOW_CACHE.with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            *c = ShadowCache::with_cap_bytes(cap_bytes);
        }
    });
    let start = computes();
    let mut tiles = Vec::new();
    let mut most = 0;
    for _pass in 0..2 {
        tiles.clear();
        for ty in 0..3 {
            for tx in 0..3 {
                tiles.push(composite_tile_at(&layers, &mut store, tx, ty, (0, 0)));
                let held = SHADOW_CACHE.with(|c| c.try_borrow().map_or(0, |c| c.entries.len()));
                most = most.max(held);
            }
        }
    }
    (tiles, computes() - start, most)
}

#[test]
fn a_default_cache_takes_the_machines_cap_and_a_tiny_cap_holds_one_tile() {
    {
        let _large = force_small_machine(false);
        assert_eq!(ShadowCache::default().cap_entries(), 1024);
    }
    {
        let _small = force_small_machine(true);
        let small = ShadowCache::default().cap_entries();
        assert_eq!(small, 256);
        assert_eq!(
            small,
            small_machine::SHADOW_CACHE_BYTES_SMALL / SHADOW_CACHE_ENTRY_BYTES
        );
        clear_shadow_cache();
        assert_eq!(
            SHADOW_CACHE.with(|c| c.try_borrow().map_or(0, |c| c.cap_entries())),
            256
        );
    }
    // Under one tile's plane, or none: still one entry, never zero.
    for bytes in [0, 1, SHADOW_CACHE_ENTRY_BYTES - 1, SHADOW_CACHE_ENTRY_BYTES] {
        assert_eq!(ShadowCache::with_cap_bytes(bytes).cap_entries(), 1, "{bytes}");
    }
    assert_eq!(
        ShadowCache::with_cap_bytes(2 * SHADOW_CACHE_ENTRY_BYTES + 5).cap_entries(),
        2
    );
}

#[test]
fn a_cache_smaller_than_one_tile_keeps_one_entry_and_evicts_the_rest() {
    let key = |i: i64| ShadowCacheKey {
        kind: 0,
        store: 1,
        surface: aurora_tile::SurfaceId::from_raw(1),
        layer_origin: (0, 0),
        doc_origin: (i * 256, 0),
        geometry: (0, 0, 0, 0),
        mask: None,
        shape: 0,
        versions: Vec::new(),
    };
    let mut cache = ShadowCache::with_cap_bytes(0);
    for pass in 0..3 {
        cache.begin_pass();
        for i in 0..5 {
            cache.insert(key(i), std::sync::Arc::new(vec![0.0]));
            assert!(cache.entries.len() <= 1, "pass {pass}");
            assert_eq!(cache.order.len(), cache.entries.len());
        }
    }
    // The first entry of a pass is kept, the rest of that pass are not (RC-2),
    // and the next pass evicts it for its own first.
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.get(&key(0)).is_some());
    assert!(cache.get(&key(4)).is_none());
}

/// A smaller cache, however small, only recomputes: the pixels are the
/// large machine's, bit for bit, shadows, strokes and glows together.
#[test]
fn a_small_machines_caps_composite_the_large_machines_pixels() {
    let (large, large_computes, large_most) = {
        let _large = force_small_machine(false);
        bounded_composite(small_machine::SHADOW_CACHE_BYTES_LARGE)
    };
    // Nine tiles, three kinds (shadow, stroke, glow), second pass all hits.
    assert!(large_computes > 0);
    assert!(large_most > 0);
    let mut recomputed_more = false;
    for (name, bytes) in [
        ("small machine", small_machine::SHADOW_CACHE_BYTES_SMALL),
        ("three tiles", 3 * SHADOW_CACHE_ENTRY_BYTES),
        ("one tile", SHADOW_CACHE_ENTRY_BYTES),
        ("no bytes", 0),
    ] {
        let _small = force_small_machine(true);
        let (tiles, computes, most) = bounded_composite(bytes);
        assert_eq!(tiles, large, "{name}: pixels differ from the large cap");
        assert!(most <= (bytes / SHADOW_CACHE_ENTRY_BYTES).max(1), "{name}: held {most}");
        assert!(computes >= large_computes, "{name}");
        recomputed_more |= computes > large_computes;
    }
    // The tiny caps really evicted: eviction worked, and cost recomputation.
    assert!(recomputed_more, "no cap smaller than the document recomputed");
}

/// The default caches, as the app makes them on each kind of machine.
#[test]
fn the_forced_small_default_cache_composites_the_forced_large_pixels() {
    let run = |small: bool| {
        let _guard = force_small_machine(small);
        let (_d, mut store, layers) = bounds_fixture();
        clear_shadow_cache();
        let mut out = Vec::new();
        for ty in 0..3 {
            for tx in 0..3 {
                out.push(composite_tile_at(&layers, &mut store, tx, ty, (0, 0)));
            }
        }
        out
    };
    assert_eq!(run(true), run(false));
}

// Every seam and cache test of the effects rounds, again on a forced small
// machine (the default caches then hold 256 tiles).
macro_rules! on_a_small_machine {
    ($($name:ident => $test:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let _small = force_small_machine(true);
                clear_shadow_cache();
                assert_eq!(
                    SHADOW_CACHE.with(|c| c.try_borrow().map_or(0, |c| c.cap_entries())),
                    256
                );
                $test();
            }
        )*
    };
}

on_a_small_machine! {
    shadow_seams_hold_on_a_small_machine => shadow_is_seamless_across_tile_boundaries,
    shadow_cache_hits_hold_on_a_small_machine => shadow_cache_hits_on_visibility_and_opacity_edits,
    shadow_cache_misses_hold_on_a_small_machine => shadow_cache_misses_on_a_dab_within_reach_only,
    shadow_cache_keys_hold_on_a_small_machine => shadow_cache_misses_on_set_effects_move_mask_and_undo,
    stroke_seams_hold_on_a_small_machine => stroke_is_seamless_across_tile_boundaries,
    stroke_cache_holds_on_a_small_machine => stroke_cache_hits_on_style_edits_and_misses_on_a_dab,
    glow_seams_hold_on_a_small_machine => glow_is_seamless_across_tile_boundaries_even_at_exactly_one_apron,
    glow_cache_holds_on_a_small_machine => glow_coverage_is_cached_apart_from_a_shadows_and_hit_by_colour_edits,
}
