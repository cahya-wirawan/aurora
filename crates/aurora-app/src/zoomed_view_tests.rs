//! The zoomed-out view: level choice, frame geometry, pyramid validity
//! tied to the composite cache, and the pyramid against an independent
//! `f64` box filter of the 100 % composite (the export scaled down).

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use std::time::Duration;

use aurora_tile::{CHANNELS, SAMPLES, TILE, TileId, TileStore};

use super::{
    Advance, Level0, MAX_LEVEL, MIN_PHYSICAL_ZOOM, ViewZoom, advance, apply_view_zoom, centred_pan,
    fit_zoom, level_for_zoom, level_scale, level_surface, plan, tile_in_document,
};

const T: usize = TILE as usize;

fn ok<V, E: std::fmt::Debug>(result: Result<V, E>) -> V {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

// ---------------------------------------------------------------- levels

#[test]
fn the_level_shows_every_zoom_between_one_half_and_one_of_its_texels() {
    let mut zoom = 0.999_f32;
    while zoom > MIN_PHYSICAL_ZOOM {
        let level = level_for_zoom(zoom);
        let shown = zoom * level_scale(level);
        assert!(
            shown > 0.5 && shown <= 1.0,
            "zoom {zoom}: level {level} at {shown}"
        );
        zoom *= 0.97;
    }
}

#[test]
fn the_level_boundaries_are_the_powers_of_two() {
    assert_eq!(level_for_zoom(1.0), 0);
    assert_eq!(level_for_zoom(0.51), 0);
    assert_eq!(level_for_zoom(0.5), 1);
    assert_eq!(level_for_zoom(0.26), 1);
    assert_eq!(level_for_zoom(0.25), 2);
    assert_eq!(level_for_zoom(0.125), 3);
    assert_eq!(level_for_zoom(1.0 / 16.0), 4);
    assert_eq!(level_for_zoom(MIN_PHYSICAL_ZOOM), MAX_LEVEL);
    assert_eq!(level_for_zoom(0.001), MAX_LEVEL);
    assert_eq!(level_for_zoom(f32::NAN), 0);
    assert_eq!(level_for_zoom(-1.0), 0);
}

#[test]
fn the_floor_reaches_the_deepest_level_and_no_further() {
    assert_eq!(MIN_PHYSICAL_ZOOM * level_scale(MAX_LEVEL), 1.0);
}

#[test]
fn each_level_has_its_own_reserved_surface() {
    assert_eq!(level_surface(0), crate::composite_surface_id());
    let mut seen = std::collections::HashSet::new();
    for level in 0..=MAX_LEVEL {
        let raw = level_surface(level).to_raw();
        assert!(seen.insert(raw), "level {level}");
        assert!(raw > u64::MAX - aurora_doc::RESERVED_TOP_SURFACES);
    }
}

#[test]
fn tiles_past_the_document_are_not_in_it() {
    let canvas = (1000, 600);
    assert!(tile_in_document(0, TileId { x: 3, y: 2 }, canvas));
    assert!(!tile_in_document(0, TileId { x: 4, y: 0 }, canvas));
    assert!(!tile_in_document(0, TileId { x: 0, y: 3 }, canvas));
    assert!(tile_in_document(1, TileId { x: 1, y: 1 }, canvas));
    assert!(!tile_in_document(1, TileId { x: 2, y: 0 }, canvas));
    assert!(tile_in_document(2, TileId { x: 0, y: 0 }, canvas));
    assert!(!tile_in_document(2, TileId { x: 1, y: 0 }, canvas));
    // A document exactly four tiles wide ends where tile 4 starts.
    assert!(!tile_in_document(0, TileId { x: 4, y: 0 }, (1024, 600)));
    assert!(tile_in_document(0, TileId { x: 3, y: 0 }, (1024, 600)));
}

// -------------------------------------------------------------- geometry

#[test]
fn fitting_a_60_mp_photo_shows_all_of_it_at_level_two() {
    let canvas_px = (1600, 900);
    let doc = (9504, 6336);
    let zoom = 900.0 / 6336.0;
    let frame = plan(
        (-(1600.0 / zoom - 9504.0) / 2.0, 0.0),
        zoom,
        canvas_px,
        doc,
        super::zoomed_floor(canvas_px, u32::MAX),
    );
    assert_eq!(frame.level, 2);
    assert!((frame.level_zoom - zoom * 4.0).abs() < 1e-6);
    // Ten by seven level-2 tiles cover 9504 x 6336 / 4.
    assert_eq!(frame.visible.len(), 10 * 7);
    assert_eq!(frame.doc_clip, (0.0, 0.0, 2376.0, 1584.0));
    // Centred: the document starts right of the canvas area's corner.
    let left = ((1600.0 - 9504.0 * zoom) / 2.0).round() as u32;
    assert_eq!(frame.shift_px, (left, 0));
    assert_eq!(frame.viewport_px, (1600 - left, 900));
    assert_eq!(frame.level_origin, (0.0, 0.0));
}

#[test]
fn a_panned_zoomed_out_view_starts_at_the_level_texel_under_its_corner() {
    let frame = plan(
        (2048.0, 1024.0),
        0.25,
        (1000, 800),
        (20_000, 12_000),
        super::zoomed_floor((1000, 800), u32::MAX),
    );
    assert_eq!(frame.level, 2);
    assert_eq!(frame.level_origin, (512.0, 256.0));
    assert_eq!(frame.shift_px, (0, 0));
    // Document x 2048..6048 is level-2 x 512..1512: tiles 2..=5.
    let xs: std::collections::BTreeSet<u32> = frame.visible.iter().map(|t| t.x).collect();
    assert_eq!(xs.into_iter().collect::<Vec<_>>(), vec![2, 3, 4, 5]);
    // y 1024..4224 is level-2 y 256..1056: tiles 1..=4.
    let ys: std::collections::BTreeSet<u32> = frame.visible.iter().map(|t| t.y).collect();
    assert_eq!(ys.into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
}

#[test]
fn a_view_wholly_past_the_document_shows_no_tiles() {
    let frame = plan(
        (50_000.0, 0.0),
        0.25,
        (1000, 800),
        (20_000, 12_000),
        super::zoomed_floor((1000, 800), u32::MAX),
    );
    assert!(frame.visible.is_empty());
}

#[test]
fn fit_and_centre_put_the_whole_document_in_the_middle_of_the_canvas() {
    let canvas = (800.0, 450.0);
    let doc = (9504, 6336);
    let zoom = ok(fit_zoom(canvas, doc).ok_or(()));
    assert!((zoom - 450.0 / 6336.0).abs() < 1e-7);
    for scale in [1.0_f32, 2.0] {
        let pan = centred_pan((0.0, 0.0), zoom, canvas, doc, scale);
        let width = 9504.0 * zoom;
        assert!((pan.0 - (800.0 - width) / 2.0).abs() <= 0.5 / scale);
        // On whole physical pixels.
        assert_eq!((pan.0 * scale).fract(), 0.0);
        assert_eq!(pan.1, 0.0);
    }
    // A wider-than-canvas axis keeps its pan.
    assert_eq!(centred_pan((-37.0, 0.0), 1.0, canvas, doc, 1.0).0, -37.0);
    assert_eq!(fit_zoom(canvas, (0, 10)), None);
    assert_eq!(fit_zoom((0.0, 10.0), doc), None);
}

#[test]
fn fit_on_screen_reaches_the_new_floor_and_100_percent_is_physical() {
    let mut view = aurora_ui::CanvasView::new();
    view.set_min_zoom(crate::canvas_min_zoom((1600, 900), 2.0));
    apply_view_zoom(&mut view, ViewZoom::Fit, (800.0, 450.0), (9504, 6336), 2.0);
    let physical = view.zoom() * 2.0;
    assert!(physical < 0.2, "fitted at {physical}");
    // Every document corner is inside the canvas area.
    for corner in [(0.0, 0.0), (9504.0, 6336.0)] {
        let (x, y) = view.to_screen(corner);
        assert!((-0.5..=800.5).contains(&x) && (-0.5..=450.5).contains(&y));
    }
    apply_view_zoom(
        &mut view,
        ViewZoom::ActualPixels,
        (800.0, 450.0),
        (9504, 6336),
        2.0,
    );
    assert_eq!(view.zoom(), 0.5);
}

#[test]
fn the_canvas_floor_is_the_zoomed_out_views_floor_in_logical_zoom() {
    for (canvas, scale) in [((1920, 1080), 1.0), ((1600, 900), 2.0), ((1366, 768), 1.5)] {
        let floor = crate::canvas_min_zoom(canvas, scale);
        let physical = crate::effective_residency_zoom(floor, scale);
        assert!(physical >= MIN_PHYSICAL_ZOOM);
        assert!(physical < MIN_PHYSICAL_ZOOM * 1.0001);
    }
}

#[test]
fn the_crop_shield_and_the_selection_outline_follow_a_zoomed_out_centred_view() {
    // Overlays are drawn from the CanvasView in physical pixels: at zoom
    // 0.1 with the document centred (pan > 0) they sit where the
    // document's pixels are drawn.
    let mut view = aurora_ui::CanvasView::new();
    view.set_min_zoom(0.01);
    view.zoom_at((0.0, 0.0), 0.1);
    view.pan_by((150.0, 40.0));
    let rect = aurora_core::Rect {
        x: 1000,
        y: 500,
        width: 2000,
        height: 1000,
    };
    let scale = 2.0_f32;
    let canvas = (1000.0, 600.0);
    let shield = aurora_ui::crop_shield_rects(&rect, &view, canvas, scale);
    let (x0, y0) = view.to_screen((1000.0, 500.0));
    let (x1, y1) = view.to_screen((3000.0, 1500.0));
    let covered = |px: f32, py: f32| {
        let (px, py) = ((px * scale) as i32, (py * scale) as i32);
        shield.iter().any(|piece| {
            px >= piece.x
                && px < piece.x + piece.width as i32
                && py >= piece.y
                && py < piece.y + piece.height as i32
        })
    };
    assert!(!covered(f32::midpoint(x0, x1), f32::midpoint(y0, y1)));
    assert!(!covered(x0 + 1.0, y0 + 1.0));
    assert!(covered(x0 - 2.0, y0 + 1.0));
    assert!(covered(x1 + 2.0, y1 - 1.0));
}

// --------------------------------------------------------------- pyramid

/// A tiny deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }
}

struct Doc {
    _dir: tempfile::TempDir,
    store: TileStore,
    layers: aurora_doc::LayerTree,
    top: aurora_doc::LayerId,
    top_surface: aurora_tile::SurfaceId,
    size: (u32, u32),
}

/// A two-layer document (an opaque base, a translucent Multiply layer on
/// top reaching 300 px past the canvas on the right) of `size`.
fn document(size: (u32, u32)) -> Doc {
    let dir = ok(tempfile::tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(4096) else {
        unreachable!("non-zero");
    };
    let mut store = ok(TileStore::new(dir.path().to_path_buf(), budget));
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    let base = ok(layers.add_pixel_layer("base", bounds, None));
    let wide = aurora_core::Rect {
        width: size.0 + 300,
        ..bounds
    };
    let top = ok(layers.add_pixel_layer("top", wide, None));
    ok(layers.set_blend_mode(top, aurora_doc::BlendMode::Multiply));
    let mut rng = Lcg(0x5eed);
    for (layer, translucent, extent) in [(base, false, size), (top, true, (size.0 + 300, size.1))] {
        let surface = layers.surface_id(layer).unwrap_or_else(|| unreachable!());
        for ty in 0..extent.1.div_ceil(TILE) {
            for tx in 0..extent.0.div_ceil(TILE) {
                let tile = ok(store.get_mut(surface, TileId { x: tx, y: ty }));
                for texel in tile.texels_mut().chunks_exact_mut(CHANNELS) {
                    let a = if translucent { rng.next() } else { 1.0 };
                    let values = [rng.next(), rng.next(), rng.next(), a];
                    for (slot, v) in texel.iter_mut().zip(values) {
                        *slot = half::f16::from_f32(v);
                    }
                }
            }
        }
    }
    let top_surface = layers.surface_id(top).unwrap_or_else(|| unreachable!());
    Doc {
        _dir: dir,
        store,
        layers,
        top,
        top_surface,
        size,
    }
}

/// Advances until the visible tiles of `level` are complete.
fn settle(doc: &mut Doc, cache: &mut crate::CompositeCache, level: u32) -> usize {
    let span = TILE << level;
    let visible: Vec<TileId> = (0..doc.size.1.div_ceil(span))
        .flat_map(|y| (0..doc.size.0.div_ceil(span)).map(move |x| TileId { x, y }))
        .collect();
    let mut rounds = 0;
    loop {
        let step: Advance = advance(
            level,
            &visible,
            doc.size,
            &doc.layers,
            Some(doc.top),
            &mut doc.store,
            cache,
            Level0::Sync(None, None),
            Duration::from_mins(1),
        );
        rounds += 1;
        if step.complete {
            return rounds;
        }
        assert!(rounds < 64, "never completes");
    }
}

/// Texel `(x, y)` of `surface` (straight).
fn texel_of(store: &mut TileStore, surface: aurora_tile::SurfaceId, x: u32, y: u32) -> [f64; 4] {
    let tile = ok(store.get(
        surface,
        TileId {
            x: x / TILE,
            y: y / TILE,
        },
    ));
    let start = (((y % TILE) as usize) * T + (x % TILE) as usize) * CHANNELS;
    let mut out = [0.0; 4];
    if let Some(samples) = tile.texels().get(start..start + CHANNELS) {
        for (o, s) in out.iter_mut().zip(samples) {
            *o = f64::from(s.to_f32());
        }
    }
    out
}

/// The export scaled down: the `2^level` box filter of the 100 %
/// composite, premultiplied, the outside of the document transparent.
fn box_filtered(store: &mut TileStore, size: (u32, u32), level: u32, x: u32, y: u32) -> [f64; 4] {
    let n = 1_u32 << level;
    let mut sum = [0.0_f64; 4];
    for dy in 0..n {
        for dx in 0..n {
            let (px, py) = (x * n + dx, y * n + dy);
            if px >= size.0 || py >= size.1 {
                continue;
            }
            let [r, g, b, a] = texel_of(store, crate::composite_surface_id(), px, py);
            sum[0] += r * a;
            sum[1] += g * a;
            sum[2] += b * a;
            sum[3] += a;
        }
    }
    if sum[3] > 0.0 {
        [
            sum[0] / sum[3],
            sum[1] / sum[3],
            sum[2] / sum[3],
            sum[3] / f64::from(n * n),
        ]
    } else {
        [0.0; 4]
    }
}

/// Every texel of `level` within `tolerance` of the scaled export.
fn assert_level_matches_export(doc: &mut Doc, level: u32, tolerance: f64) {
    let n = 1_u32 << level;
    let (w, h) = (doc.size.0.div_ceil(n), doc.size.1.div_ceil(n));
    for y in 0..h {
        for x in 0..w {
            let got = texel_of(&mut doc.store, level_surface(level), x, y);
            let want = box_filtered(&mut doc.store, doc.size, level, x, y);
            for (g, r) in got.iter().zip(want) {
                assert!(
                    (g - r).abs() <= tolerance,
                    "level {level} ({x}, {y}): {got:?} vs {want:?}"
                );
            }
        }
    }
}

/// Four `f16` steps at 1.0.
const TOLERANCE: f64 = 4.0 / 2048.0;

#[test]
fn every_pyramid_level_is_the_export_scaled_down_by_its_box_filter() {
    // 1100 x 700: not a multiple of the tile or of 2^level on either axis,
    // with a layer reaching past the canvas that must not bleed in.
    let mut doc = document((1100, 700));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 3);
    for level in 1..=3 {
        assert_level_matches_export(&mut doc, level, TOLERANCE);
    }
}

#[test]
fn a_cold_pyramid_is_built_progressively_under_its_budget() {
    let mut doc = document((1100, 700));
    let mut cache = crate::CompositeCache::default();
    let visible = vec![TileId { x: 0, y: 0 }];
    // A zero budget composites level 0 but builds nothing above it.
    let step = advance(
        2,
        &visible,
        doc.size,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        Level0::Sync(None, None),
        Duration::ZERO,
    );
    assert_eq!(step.built, 0);
    assert!(step.composited > 0);
    assert!(!step.complete);
    assert!(step.more);
    let _ = settle(&mut doc, &mut cache, 2);
    // Level 2 has two tiles (1100 / 1024); level 1 (550 x 350) six.
    // that are in the document (1100 x 700: level 1 is 550 x 350, so
    // (0, 0), (1, 0), (0, 1), (1, 1)).
    assert_eq!(cache.pyramid.current_len(), 2 + 6);
}

#[test]
fn a_dab_invalidates_exactly_the_pyramid_tiles_above_it_and_they_rebuild() {
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 3);
    let before = cache.pyramid.current_len();
    let reference_origin = crate::composite_reference_origin(&doc.layers, Some(doc.top));
    let outcome = aurora_brush::stamp_dab(
        &mut doc.store,
        doc.top_surface,
        (300.0, 300.0),
        20.0,
        [0.1, 0.9, 0.2],
        None,
    );
    crate::invalidate_painted_tiles(
        &mut cache,
        &doc.layers,
        doc.top,
        outcome.painted(),
        reference_origin,
    );
    // Level-0 tile (1, 1) holds the dab: its ancestor (0, 0) at levels 1,
    // 2 and 3 is dropped, nothing else.
    for level in 1..=3 {
        assert!(
            !cache.pyramid.is_current(level, TileId { x: 0, y: 0 }),
            "{level}"
        );
    }
    assert!(cache.pyramid.is_current(1, TileId { x: 1, y: 0 }));
    assert!(cache.pyramid.is_current(2, TileId { x: 1, y: 0 }));
    assert_eq!(cache.pyramid.current_len(), before - 3);
    let _ = settle(&mut doc, &mut cache, 3);
    assert_eq!(cache.pyramid.current_len(), before);
    for level in 1..=3 {
        assert_level_matches_export(&mut doc, level, TOLERANCE);
    }
}

#[test]
fn a_live_move_leaves_no_stale_pyramid_tile() {
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 3);
    let before = cache.pyramid.current_len();
    let bounds = doc.layers.bounds(doc.top).unwrap_or_else(|| unreachable!());
    let moved = aurora_core::Rect {
        x: bounds.x + 40,
        y: bounds.y + 20,
        ..bounds
    };
    let _ = ok(crate::move_invalidation::move_layer_live(
        &mut doc.layers,
        &mut cache,
        Some(&doc.store),
        Some(doc.top),
        doc.top,
        moved,
    ));
    assert!(cache.pyramid.current_len() < before);
    let _ = settle(&mut doc, &mut cache, 3);
    for level in 1..=3 {
        assert_level_matches_export(&mut doc, level, TOLERANCE);
    }
}

#[test]
fn an_undo_rectangle_drops_the_ancestors_of_the_tiles_it_reaches() {
    // A pixel undo invalidates the rectangles it restored
    // (`invalidate_doc_rects`); here the layer's pixels in one tile change
    // by hand, as a restore would change them.
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 3);
    let tile = ok(doc.store.get_mut(doc.top_surface, TileId { x: 2, y: 1 }));
    for texel in tile.texels_mut().chunks_exact_mut(CHANNELS) {
        for (slot, v) in texel.iter_mut().zip([0.0_f32, 0.5, 1.0, 1.0]) {
            *slot = half::f16::from_f32(v);
        }
    }
    cache.invalidate_doc_rects(
        &[aurora_core::Rect {
            x: 512,
            y: 256,
            width: 256,
            height: 256,
        }],
        (0, 0),
    );
    assert!(!cache.pyramid.is_current(1, TileId { x: 1, y: 0 }));
    assert!(!cache.pyramid.is_current(2, TileId { x: 0, y: 0 }));
    assert!(cache.pyramid.is_current(1, TileId { x: 0, y: 0 }));
    let _ = settle(&mut doc, &mut cache, 3);
    for level in 1..=3 {
        assert_level_matches_export(&mut doc, level, TOLERANCE);
    }
}

#[test]
fn a_structural_change_drops_the_whole_pyramid() {
    // Visibility, opacity, blend mode, a structural undo: all bump the
    // composite cache, and the pyramid with it.
    let mut doc = document((1100, 700));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 2);
    ok(doc.layers.set_opacity(doc.top, 0.25));
    cache.bump();
    assert_eq!(cache.pyramid.current_len(), 0);
    let _ = settle(&mut doc, &mut cache, 2);
    assert_level_matches_export(&mut doc, 2, TOLERANCE);
}

#[test]
fn a_provisional_install_drops_the_ancestors_too() {
    let mut doc = document((600, 600));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 1);
    assert!(cache.pyramid.is_current(1, TileId { x: 0, y: 0 }));
    cache.mark_provisional(TileId { x: 1, y: 1 }, 0);
    assert!(!cache.pyramid.is_current(1, TileId { x: 0, y: 0 }));
}

#[test]
fn a_canvas_size_change_drops_the_pyramid() {
    let mut doc = document((1100, 700));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 2);
    assert!(cache.pyramid.current_len() > 0);
    doc.size = (900, 700);
    let _ = advance(
        2,
        &[],
        doc.size,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        Level0::Sync(None, None),
        Duration::ZERO,
    );
    assert_eq!(cache.pyramid.current_len(), 0);
    let _ = settle(&mut doc, &mut cache, 2);
    assert_level_matches_export(&mut doc, 2, TOLERANCE);
}

#[test]
fn a_pyramid_tile_is_written_whole_and_marked_dirty_for_the_atlas() {
    let mut doc = document((600, 600));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 1);
    let id = TileId { x: 0, y: 0 };
    assert!(doc.store.is_dirty(level_surface(1), id));
    assert_eq!(
        ok(doc.store.get(level_surface(1), id)).texels().len(),
        SAMPLES
    );
}

// ------------------------------------------------------------------- GPU

/// Renders the zoomed-out atlas at an exact power-of-two zoom (each
/// level texel on one physical pixel) and compares sampled pixels with
/// the box-filtered export quantised to 8 bits.
#[test]
fn the_zoomed_out_atlas_draws_the_scaled_export_at_a_power_of_two_zoom() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let device = context.device();
    let queue = context.queue();
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 2);
    let viewport = (512, 256);
    let frame = plan(
        (0.0, 0.0),
        0.25,
        viewport,
        doc.size,
        super::zoomed_floor(viewport, u32::MAX),
    );
    assert_eq!(frame.level, 2);
    assert_eq!(frame.level_zoom, 1.0);
    let mut atlas =
        aurora_gpu::TileResidency::with_coverage(device, queue, viewport, super::COVERAGE);
    atlas.set_document_clip(Some(frame.doc_clip));
    atlas.set_origin(
        queue,
        frame.level_origin,
        frame.viewport_px,
        frame.level_zoom,
    );
    let stats = atlas.sync_only(
        queue,
        &mut doc.store,
        level_surface(2),
        usize::MAX,
        &frame.visible,
    );
    assert_eq!(stats.errors, 0);
    let mut canvas = aurora_gpu::CanvasPipeline::new(device);
    for (x, y) in [
        (3, 5),
        (200, 100),
        (300, 7),
        (511, 255),
        (100, 250),
        (260, 130),
    ] {
        let got = crate::tests::render_and_sample_pixel(
            device,
            queue,
            &mut canvas,
            &atlas,
            viewport,
            (x, y),
        );
        let want = box_filtered(&mut doc.store, doc.size, 2, x, y);
        // The document is opaque: no checkerboard shows through.
        for (channel, (g, w)) in got.iter().zip(want).take(3).enumerate() {
            let expected = (w * 255.0).round();
            assert!(
                (f64::from(*g) - expected).abs() <= 1.0,
                "pixel ({x}, {y}) channel {channel}: {got:?} vs {want:?}"
            );
        }
    }
}

// ---------------------------------------------------- review (ZV-1..ZV-4)

/// Canvas areas up to 8K and past it, at wgpu's default 8192 limit.
const BIG_CANVASES: [(u32, u32); 7] = [
    (1920, 1080),
    (3968, 2160),
    (4000, 2250),
    (5120, 2880),
    (6016, 3384),
    (7680, 4320),
    (8000, 4500),
];

#[test]
fn a_capped_atlas_keeps_the_render_and_pointer_zoom_equal_at_every_zoom() {
    for limit in [8192_u32, 16_384, u32::MAX] {
        for canvas in BIG_CANVASES {
            let floor = super::physical_floor(canvas, limit);
            let mut zoom = floor;
            while zoom < 2.0 {
                let rendered = super::rendered_canvas_zoom_within(canvas, zoom, limit);
                assert!(
                    (rendered - zoom).abs() <= zoom * 1e-6,
                    "canvas {canvas:?} limit {limit}: zoom {zoom} rendered at {rendered}"
                );
                zoom *= 1.013;
            }
        }
    }
}

#[test]
fn the_level_rises_past_a_capped_atlas_floor() {
    // 5120 px wide on an 8192 limit: the double-coverage grid is capped to
    // 32 tiles, so its floor is 5120 / 7936.
    let canvas = (5120, 2880);
    let floor = super::zoomed_floor(canvas, 8192);
    assert!((floor - 5120.0 / 7936.0).abs() < 1e-6, "{floor}");
    let frame = plan((0.0, 0.0), 0.55, canvas, (20_000, 12_000), floor);
    assert_eq!(frame.level, 1);
    assert!((frame.level_zoom - 1.1).abs() < 1e-6);
    // An uncapped atlas shows the same zoom at level 0.
    assert_eq!(super::level_for_zoom_within(0.55, 0.5), 0);
    // A floor above 1 (a canvas wider than the largest texture) still finds
    // a level the atlas draws without clamping.
    for zoom in [0.06_f32, 0.3, 0.52, 0.9] {
        let level = super::level_for_zoom_within(zoom, 1.13);
        assert!(zoom * level_scale(level) >= 1.13, "{zoom}");
    }
    // The canvas floor rises with it when even level 5 would clamp.
    let huge = (8000, 4500);
    let physical = super::physical_floor(huge, 8192);
    assert!(physical * level_scale(MAX_LEVEL) >= super::zoomed_floor(huge, 8192));
    assert_eq!(super::physical_floor((1920, 1080), 8192), MIN_PHYSICAL_ZOOM);
    let logical = crate::canvas_min_zoom_within(huge, 1.0, 8192);
    assert!(logical >= physical);
}

#[test]
fn the_zoomed_out_frame_marks_itself_drawn_as_soon_as_it_uploads() {
    // ZV-2, a source pin (the method needs a window): `zoomed_drawn` is set
    // right after the upload inside `prepare_zoomed_frame`, so a frame whose
    // surface texture never arrives still makes the next 1:1 frame forget
    // the canvas atlas's slots.
    let source = include_str!("lib.rs");
    let start = source
        .find("fn prepare_zoomed_frame(")
        .unwrap_or_else(|| unreachable!("the method exists"));
    let body = source.get(start..).unwrap_or_default();
    let end = body.find("\n    fn redraw(").unwrap_or(body.len());
    let body = body.get(..end).unwrap_or_default();
    let upload = body.find(".prepare(").unwrap_or(usize::MAX);
    let marked = body.find("self.zoomed_drawn = true;").unwrap_or(0);
    assert!(upload < usize::MAX && marked > upload, "{upload} {marked}");
}

#[test]
fn a_budgeted_recomposite_always_progresses_and_stops_at_its_budget() {
    let mut doc = document((2100, 1100));
    let mut cache = crate::CompositeCache::default();
    let tiles: Vec<TileId> = (0..5)
        .flat_map(|y| (0..9).map(move |x| TileId { x, y }))
        .collect();
    let (done, more) = super::recomposite_within_budget(
        &tiles,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        None,
        None,
        Duration::ZERO,
    );
    assert_eq!(done, super::LEVEL0_CHUNK);
    assert!(more);
    let (done, more) = super::recomposite_within_budget(
        &tiles,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        None,
        None,
        Duration::from_mins(1),
    );
    assert_eq!(done, tiles.len());
    assert!(!more);
}

#[test]
fn a_pyramid_write_that_fails_is_not_counted_and_asks_for_no_redraw() {
    let mut doc = document((1100, 700));
    let mut cache = crate::CompositeCache::default();
    let _ = settle(&mut doc, &mut cache, 0);
    let visible: Vec<TileId> = (0..1)
        .flat_map(|y| (0..2).map(move |x| TileId { x, y }))
        .collect();
    super::FAIL_PYRAMID_WRITES.with(|fail| fail.set(true));
    let step = advance(
        2,
        &visible,
        doc.size,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        Level0::Sync(None, None),
        Duration::from_mins(1),
    );
    super::FAIL_PYRAMID_WRITES.with(|fail| fail.set(false));
    assert_eq!(step.built, 0);
    assert!(!step.complete);
    assert!(
        !step.more,
        "a persistent store error must not keep redrawing"
    );
    assert_eq!(cache.pyramid.current_len(), 0);
    let _ = settle(&mut doc, &mut cache, 2);
    assert_level_matches_export(&mut doc, 2, TOLERANCE);
}

#[test]
fn a_refused_worker_dispatch_is_composited_under_the_frame_budget() {
    // ZV-3: 16 x 16 tiles stale, the worker refusing: one frame composites
    // a bounded part of them and asks for another, not the whole document.
    let mut doc = document((4096, 4096));
    let mut cache = crate::CompositeCache::default();
    let mut worker = crate::background_composite::CompositeWorker::new(std::sync::Arc::new(|| {}));
    let visible: Vec<TileId> = (0..16)
        .flat_map(|y| (0..16).map(move |x| TileId { x, y }))
        .collect();
    crate::background_composite::FAIL_DISPATCH.with(|fail| fail.set(true));
    let step = advance(
        0,
        &visible,
        doc.size,
        &doc.layers,
        Some(doc.top),
        &mut doc.store,
        &mut cache,
        Level0::Worker(&mut worker),
        Duration::from_mins(1),
    );
    crate::background_composite::FAIL_DISPATCH.with(|fail| fail.set(false));
    worker.shutdown();
    let done = visible.iter().filter(|&&id| cache.is_current(id)).count();
    assert!(done >= super::LEVEL0_CHUNK, "{done}");
    assert!(done < visible.len(), "the whole document in one frame");
    assert!(step.more);
    assert!(!step.complete);
}
