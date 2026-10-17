//! Differentials for [`crate::move_invalidation`] (60 FPS round 2): after
//! every narrow Move invalidation, each composite tile the cache still
//! calls current must equal a fresh composite of the moved document, bit
//! for bit, on random trees (masks, clipping, adjustments above, Pass
//! Through groups, shadows, strokes, glows, overlays) and on the corpus.

use std::collections::HashMap;

use crate::move_invalidation::{MoveInvalidation, invalidate_moved_layer, move_layer_live};
use crate::occlusion_cull_tests::{
    Rng, clear_shadow_cache, layer, mismatches, ok, opaque, random_node, rect, store_with_budget,
    translucent,
};
use crate::{
    CompositeBudget, CompositeCache, composite_reference_origin, composite_roots_into_tile,
};

const T: i64 = aurora_tile::TILE as i64;

/// The composite tiles each differential keeps: a `GRID` x `GRID` block
/// from the anchor.
const GRID: u32 = 4;

fn grid() -> impl Iterator<Item = aurora_tile::TileId> {
    (0..GRID).flat_map(|y| (0..GRID).map(move |x| aurora_tile::TileId { x, y }))
}

fn composite(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    id: aurora_tile::TileId,
    reference: (i64, i64),
    fresh: bool,
) -> Vec<half::f16> {
    if fresh {
        clear_shadow_cache();
    }
    let mut budget = CompositeBudget::for_pass(layers);
    budget.next_tile(layers);
    let origin = (
        reference.0 + i64::from(id.x) * T,
        reference.1 + i64::from(id.y) * T,
    );
    composite_roots_into_tile(layers, store, id, origin, reference, &mut budget).0
}

/// A composite cache with the pixels it says are current.
#[derive(Default)]
struct Shown {
    cache: CompositeCache,
    tiles: HashMap<aurora_tile::TileId, Vec<half::f16>>,
}

impl Shown {
    fn refresh(
        &mut self,
        layers: &aurora_doc::LayerTree,
        store: &mut aurora_tile::TileStore,
        reference: (i64, i64),
    ) {
        for id in grid() {
            if !self.cache.is_current(id) {
                let pixels = composite(layers, store, id, reference, false);
                self.tiles.insert(id, pixels);
                self.cache.mark_current(id);
            }
        }
    }

    /// Every tile still current equals a fresh composite; returns how many
    /// were checked.
    #[track_caller]
    fn check(
        &self,
        layers: &aurora_doc::LayerTree,
        store: &mut aurora_tile::TileStore,
        reference: (i64, i64),
        what: &str,
    ) -> usize {
        let mut kept = 0;
        for id in grid() {
            if !self.cache.is_current(id) {
                continue;
            }
            kept += 1;
            let fresh = composite(layers, store, id, reference, true);
            let shown = self.tiles.get(&id).map_or(&[][..], Vec::as_slice);
            let bad = mismatches(shown, &fresh);
            assert!(
                bad.is_empty(),
                "{what}: tile {id:?} stayed current but {} samples differ, first {:?}",
                bad.len(),
                bad.get(..bad.len().min(4))
            );
        }
        kept
    }
}

fn pixel_layers(layers: &aurora_doc::LayerTree) -> Vec<aurora_doc::LayerId> {
    let mut out = Vec::new();
    let mut pending: Vec<aurora_doc::LayerId> = layers.roots().to_vec();
    while let Some(id) = pending.pop() {
        if matches!(layers.kind(id), Some(aurora_doc::LayerKind::Pixel { .. })) {
            out.push(id);
        }
        pending.extend(layers.children(id).unwrap_or(&[]).iter().copied());
    }
    out
}

/// Every reaching effect kind, on top of whatever the generator gave.
fn reaching_effects(rng: &mut Rng) -> aurora_core::LayerEffects {
    let mut fx = aurora_core::LayerEffects::default();
    if rng.chance(60) {
        fx.drop_shadows.push(aurora_core::effects::DropShadow {
            distance: (5 + rng.below(40)) as f32,
            size: rng.below(12) as f32,
            use_global_light: false,
            angle: rng.pick(&[30.0, 120.0, 200.0, 300.0], 120.0),
            ..aurora_core::effects::DropShadow::default()
        });
    }
    if rng.chance(40) {
        fx.strokes.push(aurora_core::effects::Stroke {
            size: (1 + rng.below(9)) as f32,
            position: rng.pick(
                &[
                    aurora_core::effects::StrokePosition::Outside,
                    aurora_core::effects::StrokePosition::Center,
                    aurora_core::effects::StrokePosition::Inside,
                ],
                aurora_core::effects::StrokePosition::Outside,
            ),
            ..aurora_core::effects::Stroke::default()
        });
    }
    if rng.chance(40) {
        fx.outer_glows.push(aurora_core::effects::OuterGlow {
            size: (2 + rng.below(20)) as f32,
            opacity: 0.7,
            ..aurora_core::effects::OuterGlow::default()
        });
    }
    if fx.drop_shadows.is_empty() && fx.strokes.is_empty() && fx.outer_glows.is_empty() {
        fx.drop_shadows
            .push(aurora_core::effects::DropShadow::default());
    }
    fx
}

/// Runs `steps` random moves of `moved` and checks every kept tile after
/// each. Returns `(narrow steps, bumped steps, kept tiles checked,
/// tiles a narrow step invalidated)`.
#[allow(clippy::too_many_lines)]
fn run_moves(
    rng: &mut Rng,
    layers: &mut aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    moved: aurora_doc::LayerId,
    steps: usize,
    what: &str,
) -> (usize, usize, usize, usize) {
    let mut shown = Shown::default();
    let (mut narrow, mut bumped, mut kept, mut invalidated) = (0, 0, 0, 0);
    // Start inside the document, where the anchor holds still.
    if let Some(bounds) = layers.bounds(moved) {
        let start = aurora_core::Rect {
            x: bounds.x.max(0),
            y: bounds.y.max(0),
            ..bounds
        };
        let _ = ok(move_layer_live(
            layers,
            &mut shown.cache,
            Some(store),
            Some(moved),
            moved,
            start,
        ));
    }
    let mut reference = composite_reference_origin(layers, Some(moved));
    shown.refresh(layers, store, reference);
    for step in 0..steps {
        let Some(bounds) = layers.bounds(moved) else {
            unreachable!("a pixel layer");
        };
        let (x, y) = if rng.chance(15) {
            // Across the anchor sometimes: negative coordinates move it.
            (rng.below(500) as i64 - 250, rng.below(500) as i64 - 250)
        } else {
            (
                (bounds.x + rng.below(161) as i64 - 80).max(0),
                (bounds.y + rng.below(161) as i64 - 80).max(0),
            )
        };
        let to = aurora_core::Rect { x, y, ..bounds };
        let outcome = ok(move_layer_live(
            layers,
            &mut shown.cache,
            Some(store),
            Some(moved),
            moved,
            to,
        ));
        match outcome {
            MoveInvalidation::Narrow(n) => {
                narrow += 1;
                invalidated += n;
            }
            MoveInvalidation::Everything => bumped += 1,
        }
        reference = composite_reference_origin(layers, Some(moved));
        kept += shown.check(layers, store, reference, &format!("{what} step {step}"));
        shown.refresh(layers, store, reference);
    }
    (narrow, bumped, kept, invalidated)
}

#[test]
fn random_trees_narrow_move_equals_full_recomposite() {
    let mut rng = Rng(0x5EED_0125_0002_0001);
    let (mut narrow, mut bumped, mut kept, mut invalidated) = (0, 0, 0, 0);
    let (mut clipped, mut masked) = (0, 0);
    let trees = 48;
    for tree in 0..trees {
        let (_d, mut store) = store_with_budget(1024);
        let mut layers = aurora_doc::LayerTree::new();
        for _ in 0..=rng.below(4) {
            random_node(&mut rng, &mut layers, &mut store, None, 1);
        }
        let pixels = pixel_layers(&layers);
        for &id in &pixels {
            if rng.chance(30) {
                let fx = reaching_effects(&mut rng);
                ok(layers.set_effects(id, Some(fx)));
            }
            if rng.chance(25) && layers.set_clipped(id, true).is_ok() {
                clipped += 1;
            }
            // Masks of every flavour on the moved layer's neighbours and
            // itself: inverted, disabled, reduced density.
            if rng.chance(20) && layers.add_mask(id, rect(100, 80, 400, 300)).is_ok() {
                masked += 1;
                if rng.chance(50) {
                    ok(layers.set_mask_inverted(id, true));
                }
                if rng.chance(30) {
                    ok(layers.set_mask_enabled(id, false));
                }
                if rng.chance(30) {
                    ok(layers.set_mask_density(id, 0.5));
                }
            }
        }
        let Some(&moved) = pixels.get(rng.below(pixels.len() as u64) as usize) else {
            continue;
        };
        let (n, b, k, i) = run_moves(
            &mut rng,
            &mut layers,
            &mut store,
            moved,
            6,
            &format!("tree {tree}"),
        );
        narrow += n;
        bumped += b;
        kept += k;
        invalidated += i;
    }
    println!(
        "narrow move differential: {trees} trees, {narrow} narrow steps ({invalidated} tiles \
         invalidated), {bumped} anchor bumps, {kept} kept tiles checked, {clipped} clipped layers, \
         {masked} extra masks"
    );
    assert!(narrow > 100, "the narrow path ran: {narrow}");
    assert!(bumped > 10, "the anchor bump ran: {bumped}");
    assert!(kept > 1000, "narrowing kept tiles to check: {kept}");
    assert!(
        invalidated > 100,
        "narrowing invalidated tiles: {invalidated}"
    );
    assert!(clipped > 5, "clip groups exercised: {clipped}");
    assert!(masked > 5, "masks exercised: {masked}");
}

/// A clip base moved under a clipped layer with a shadow, a stroke and a
/// glow, an adjustment above and a Pass Through group around them.
#[test]
fn a_moved_clip_base_under_effects_and_adjustments_equals_full_recomposite() {
    let mut rng = Rng(0x5EED_0125_0002_0002);
    for case in 0..6 {
        let (_d, mut store) = store_with_budget(1024);
        let mut layers = aurora_doc::LayerTree::new();
        let _ = layer(
            &mut layers,
            &mut store,
            None,
            rect(0, 0, 1024, 1024),
            &opaque,
        );
        let group = ok(layers.add_group("pt", None));
        ok(layers.set_blend_mode(group, aurora_doc::BlendMode::PassThrough));
        let base = layer(
            &mut layers,
            &mut store,
            Some(group),
            rect(300, 260, 300, 200),
            &translucent,
        );
        let clip = layer(
            &mut layers,
            &mut store,
            Some(group),
            rect(200, 200, 600, 400),
            &opaque,
        );
        let _ =
            ok(layers.add_adjustment_layer_at("c", aurora_doc::Adjustment::Invert, Some(group), 0));
        ok(layers.set_clipped(clip, true));
        ok(layers.set_effects(clip, Some(reaching_effects(&mut rng))));
        ok(layers.set_effects(base, Some(reaching_effects(&mut rng))));
        if case % 2 == 1 {
            ok(layers.add_mask(base, rect(250, 250, 300, 300)));
        }
        let moved = if case < 3 { base } else { clip };
        let (narrow, _, kept, _) = run_moves(
            &mut rng,
            &mut layers,
            &mut store,
            moved,
            8,
            &format!("clip case {case}"),
        );
        assert!(
            narrow > 0 && kept > 0,
            "case {case}: narrow {narrow}, kept {kept}"
        );
    }
}

#[test]
fn a_move_across_the_anchor_bumps_and_one_inside_it_does_not() {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(
        &mut layers,
        &mut store,
        None,
        rect(10, 10, 100, 100),
        &opaque,
    );
    let mut cache = CompositeCache::default();
    for id in grid() {
        cache.mark_current(id);
    }
    let r = ok(move_layer_live(
        &mut layers,
        &mut cache,
        Some(&store),
        Some(id),
        id,
        rect(20, 10, 100, 100),
    ));
    // The stored tile is 256 px wide although `bounds` is 100: at x 10
    // and 20 (y 10) it covers composite tiles 0 and 1 on both rows (the stored extent, not
    // `bounds`, is what counts).
    assert_eq!(
        r,
        MoveInvalidation::Narrow(4),
        "the stored tile's two columns and two rows"
    );
    assert!(!cache.is_current(aurora_tile::TileId { x: 1, y: 0 }));
    assert!(cache.is_current(aurora_tile::TileId { x: 2, y: 0 }));
    let r = ok(move_layer_live(
        &mut layers,
        &mut cache,
        Some(&store),
        Some(id),
        id,
        rect(-5, 10, 100, 100),
    ));
    assert_eq!(r, MoveInvalidation::Everything, "the anchor moved");
    assert!(!cache.is_current(aurora_tile::TileId { x: 3, y: 3 }));
}

/// The cost of a 256-px-tile layer moved over the old content's edge: the
/// tile it leaves and the tile it enters, both.
#[test]
fn both_the_old_and_the_new_placement_are_invalidated() {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(&mut layers, &mut store, None, rect(0, 0, 200, 200), &opaque);
    let mut cache = CompositeCache::default();
    for id in grid() {
        cache.mark_current(id);
    }
    let r = ok(move_layer_live(
        &mut layers,
        &mut cache,
        Some(&store),
        Some(id),
        id,
        rect(600, 0, 200, 200),
    ));
    assert_eq!(
        r,
        MoveInvalidation::Narrow(3),
        "old tile 0, new tiles 2 and 3"
    );
    for x in [0, 2, 3] {
        assert!(
            !cache.is_current(aurora_tile::TileId { x, y: 0 }),
            "tile {x}"
        );
    }
    assert!(cache.is_current(aurora_tile::TileId { x: 1, y: 0 }));
}

/// A tile a background job is compositing is found though not current.
#[test]
fn an_in_flight_tile_is_invalidated() {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(&mut layers, &mut store, None, rect(0, 0, 200, 200), &opaque);
    let mut cache = CompositeCache::default();
    let tile = aurora_tile::TileId { x: 0, y: 0 };
    cache.track_in_flight([tile]);
    let stamp = cache.stamp();
    let r = invalidate_moved_layer(
        &mut cache,
        &layers,
        Some(&store),
        id,
        (0, 0),
        (40, 0),
        (0, 0),
        (0, 0),
    );
    assert_eq!(r, MoveInvalidation::Narrow(1));
    assert!(
        !cache.unchanged_since(tile, stamp),
        "the job's result is refused"
    );
}

/// A shadow far from its layer: the tile only the shadow lands on.
#[test]
fn a_shadow_offset_reaches_the_tile_it_lands_on() {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(&mut layers, &mut store, None, rect(0, 0, 100, 100), &opaque);
    let fx = aurora_core::LayerEffects {
        drop_shadows: vec![aurora_core::effects::DropShadow {
            distance: 300.0,
            use_global_light: false,
            angle: 180.0,
            size: 0.0,
            ..aurora_core::effects::DropShadow::default()
        }],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    let mut cache = CompositeCache::default();
    for id in grid() {
        cache.mark_current(id);
    }
    let r = invalidate_moved_layer(
        &mut cache,
        &layers,
        Some(&store),
        id,
        (0, 0),
        (0, 10),
        (0, 0),
        (0, 0),
    );
    assert!(matches!(r, MoveInvalidation::Narrow(_)));
    assert!(
        !cache.is_current(aurora_tile::TileId { x: 0, y: 0 }),
        "the content"
    );
    assert!(
        !cache.is_current(aurora_tile::TileId { x: 1, y: 0 }),
        "the shadow"
    );
    // The move is vertical, so the content stays in column 0: only the
    // shadow (x 300..400 at 180 degrees) reaches column 1, not column 2.
    assert!(
        cache.is_current(aurora_tile::TileId { x: 2, y: 0 }),
        "past the shadow"
    );
    assert!(
        cache.is_current(aurora_tile::TileId { x: 3, y: 3 }),
        "far away"
    );
}

/// A glow reaching across a tile edge by its apron alone: content fills
/// stored tile 0 exactly (x 0..256) and a vertical move keeps it there, so
/// only the glow's apron reaches column 1.
#[test]
fn a_glow_apron_reaches_across_a_tile_edge() {
    apron_case(aurora_core::LayerEffects {
        outer_glows: vec![aurora_core::effects::OuterGlow {
            size: 20.0,
            ..aurora_core::effects::OuterGlow::default()
        }],
        ..aurora_core::LayerEffects::default()
    });
}

/// The same for an Outside stroke.
#[test]
fn a_stroke_apron_reaches_across_a_tile_edge() {
    apron_case(aurora_core::LayerEffects {
        strokes: vec![aurora_core::effects::Stroke {
            size: 8.0,
            ..aurora_core::effects::Stroke::default()
        }],
        ..aurora_core::LayerEffects::default()
    });
}

fn apron_case(fx: aurora_core::LayerEffects) {
    let (_d, mut store) = store_with_budget(64);
    let mut layers = aurora_doc::LayerTree::new();
    let id = layer(&mut layers, &mut store, None, rect(0, 0, 256, 100), &opaque);
    ok(layers.set_effects(id, Some(fx)));
    let mut cache = CompositeCache::default();
    for id in grid() {
        cache.mark_current(id);
    }
    let r = invalidate_moved_layer(
        &mut cache,
        &layers,
        Some(&store),
        id,
        (0, 0),
        (0, 10),
        (0, 0),
        (0, 0),
    );
    assert!(matches!(r, MoveInvalidation::Narrow(_)));
    assert!(
        !cache.is_current(aurora_tile::TileId { x: 1, y: 0 }),
        "the apron"
    );
    assert!(
        cache.is_current(aurora_tile::TileId { x: 2, y: 0 }),
        "past the apron"
    );
}

fn corpus_files() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpora/psd/reference");
    let mut pending = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
                && std::fs::metadata(&path).is_ok_and(|m| m.len() < 8 << 20)
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn corpus_narrow_move_equals_full_recomposite() {
    let files = corpus_files();
    if files.is_empty() {
        assert!(
            std::env::var_os("AURORA_REQUIRE_CORPUS").is_none(),
            "AURORA_REQUIRE_CORPUS is set and the PSD corpus is missing"
        );
        println!("SKIPPED: corpus absent");
        return;
    }
    let mut rng = Rng(0x5EED_0125_0002_0003);
    let (mut opened, mut narrow, mut kept) = (0, 0, 0);
    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(document) = crate::open_psd_document(&bytes) else {
            continue;
        };
        let (_d, mut store) = store_with_budget(512);
        let incoming: Vec<_> = document
            .pixels
            .iter()
            .map(|p| (p.layer, &p.image, p.offset))
            .collect();
        let (_freed, failed) = crate::replace_document_pixels(
            &mut store,
            aurora_doc::LayerTree::new(),
            aurora_doc::History::new(),
            &document.layers,
            &incoming,
            &document.masks,
        );
        if failed > 0 {
            continue;
        }
        let mut layers = document.layers.clone();
        let pixels = pixel_layers(&layers);
        if pixels.is_empty() {
            continue;
        }
        opened += 1;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        for _ in 0..1 {
            let Some(&moved) = pixels.get(rng.below(pixels.len() as u64) as usize) else {
                continue;
            };
            let (n, _, k, _) = run_moves(&mut rng, &mut layers, &mut store, moved, 3, &name);
            narrow += n;
            kept += k;
        }
    }
    println!(
        "corpus narrow move: {opened} files, {narrow} narrow steps, {kept} kept tiles checked"
    );
    assert!(
        opened > 10 && narrow > 20 && kept > 50,
        "{opened} {narrow} {kept}"
    );
}

/// What a Move drag's pointer event costs in recompositing, the old way
/// (bump every cached tile) against the narrow one, on the same fixture:
/// a 2048 x 1280 view (8 x 5 composite tiles) of an opaque background, a
/// Multiply layer over all of it and a 300 x 300 layer dragged 7 px an
/// event. Printed, not asserted, but for a loose CI-safety bound.
#[test]
fn move_drag_recomposite_cost_is_measured() {
    const FRAMES: usize = 40;
    let (_d, mut store) = store_with_budget(512);
    let mut layers = aurora_doc::LayerTree::new();
    let _ = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 2048, 1280),
        &opaque,
    );
    let mid = layer(
        &mut layers,
        &mut store,
        None,
        rect(0, 0, 2048, 1280),
        &translucent,
    );
    ok(layers.set_blend_mode(mid, aurora_doc::BlendMode::Multiply));
    let moved = layer(
        &mut layers,
        &mut store,
        None,
        rect(400, 300, 300, 300),
        &opaque,
    );
    let view: Vec<aurora_tile::TileId> = (0..5)
        .flat_map(|y| (0..8).map(move |x| aurora_tile::TileId { x, y }))
        .collect();
    for run in 0..3 {
        for narrow in [false, true] {
            ok(layers.set_bounds(moved, rect(400, 300, 300, 300)));
            let mut cache = CompositeCache::default();
            let reference = composite_reference_origin(&layers, Some(moved));
            for &id in &view {
                let _ = composite(&layers, &mut store, id, reference, false);
                cache.mark_current(id);
            }
            let mut times = Vec::with_capacity(FRAMES);
            let mut recomposited = 0;
            for frame in 0..FRAMES {
                let t0 = std::time::Instant::now();
                let to = rect(400 + 7 * frame as i64, 300 + 3 * frame as i64, 300, 300);
                if narrow {
                    let _ = ok(move_layer_live(
                        &mut layers,
                        &mut cache,
                        Some(&store),
                        Some(moved),
                        moved,
                        to,
                    ));
                } else {
                    ok(layers.set_bounds(moved, to));
                    cache.bump();
                }
                for &id in &view {
                    if !cache.is_current(id) {
                        let _ = composite(&layers, &mut store, id, reference, false);
                        cache.mark_current(id);
                        recomposited += 1;
                    }
                }
                times.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let max = times.iter().copied().fold(0.0, f64::max);
            println!(
                "move drag ({}): run {run} n={FRAMES} mean={mean:.2}ms max={max:.2}ms \
                 recomposited={:.1} tiles/event of {}",
                if narrow { "narrow" } else { "bump" },
                recomposited as f64 / FRAMES as f64,
                view.len()
            );
            assert!(mean < 5000.0, "CI-safety bound only: {mean:.2}ms");
        }
    }
}
