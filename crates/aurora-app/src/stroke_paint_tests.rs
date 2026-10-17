// ---- task 118: gradient and pattern strokes ----
//
// Included into `fill_and_overlay_tests` after the gradient and pattern
// overlay tests (same helpers, plus `ramp_fill`, `quad` and the `P*`
// colours from them). The layer is a white 40 x 40 square at (100, 100)
// (`painted`) on tile (0, 0): an Outside stroke of size 2 shows at x in
// 140..142, an Inside one at 100..102 and 138..140.

use aurora_core::effects::{Stroke, StrokeFill, StrokePaint};

fn painted_stroke(
    position: StrokePosition,
    size: f32,
    fill: StrokeFill,
    paint: Option<StrokePaint>,
) -> Stroke {
    Stroke {
        position,
        size,
        fill,
        paint,
        ..Stroke::default()
    }
}

fn gradient_paint(gradient: GradientFill) -> StrokePaint {
    StrokePaint::Gradient {
        gradient,
        reference: Some(ReferenceBox {
            dx: 100,
            dy: 100,
            width: 40,
            height: 40,
        }),
    }
}

fn square_composite(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
) -> Vec<half::f16> {
    composite_tile_at(layers, store, 0, 0, (0, 0))
}

fn pattern_paint(id: &str, scale: f32, origin: (f64, f64)) -> StrokePaint {
    StrokePaint::Pattern {
        pattern: id.to_owned(),
        scale,
        origin,
    }
}

/// The square's box.
fn square() -> aurora_core::Rect {
    aurora_core::Rect {
        x: 100,
        y: 100,
        width: 40,
        height: 40,
    }
}

/// A white square layer carrying `strokes`, with `quad("q")` in the pool.
fn stroked_layer(
    store: &mut aurora_tile::TileStore,
    strokes: Vec<Stroke>,
) -> (aurora_doc::LayerTree, aurora_doc::LayerId) {
    let mut layers = aurora_doc::LayerTree::new();
    let id = painted(&mut layers, store, (0, 0), (100, 100, 40, 40));
    ok(layers.add_pattern(quad("q")));
    let fx = aurora_core::LayerEffects {
        strokes,
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    (layers, id)
}

/// The colour a gradient gives pixel `(x, y)` (its box is the square), as a premultiplied texel at coverage 1.
fn gradient_texel(gradient: &GradientFill, x: usize, y: usize) -> [f32; 4] {
    let frame = gradient.frame(square());
    #[allow(clippy::cast_precision_loss)]
    let (rgb, a) = gradient.at(frame.position(x as f64 + 0.5, y as f64 + 0.5));
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

#[test]
fn a_gradient_stroke_is_painted_per_texel_outside_and_inside() {
    let (_d, mut store) = real_tile_store();
    let g = ramp_fill(0.0, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let (layers, _) = stroked_layer(
        &mut store,
        vec![
            painted_stroke(
                StrokePosition::Outside,
                2.0,
                StrokeFill::Gradient,
                Some(gradient_paint(g.clone())),
            ),
            painted_stroke(
                StrokePosition::Inside,
                2.0,
                StrokeFill::Gradient,
                Some(gradient_paint(g.clone())),
            ),
        ],
    );
    let acc = square_composite(&layers, &mut store);
    // Outside, right of the layer: the ring at x = 10 and 11 takes the
    // gradient's colour at that pixel, which differs from pixel to pixel.
    for (x, y) in [(140, 120), (141, 120), (140, 103)] {
        near(texel(&acc, x, y), gradient_texel(&g, x, y));
    }
    // Inside, at the layer's left and right edges (full coverage there).
    for (x, y) in [(100, 120), (101, 120), (138, 120), (139, 120)] {
        near(texel(&acc, x, y), gradient_texel(&g, x, y));
    }
    // The ring really is a ramp, not one colour: red-ish at the left edge,
    // blue-ish at the right one, and the interior is the layer's grey.
    assert!(texel(&acc, 100, 120)[0] > texel(&acc, 100, 120)[2]);
    assert!(texel(&acc, 139, 120)[2] > texel(&acc, 139, 120)[0]);
    near(texel(&acc, 120, 120), [1.0; 4]);
    // Two pixels past the ring nothing is drawn.
    near(texel(&acc, 142, 120), [0.0; 4]);
}

#[test]
fn a_gradient_strokes_transparency_stop_scales_its_coverage() {
    let (_d, mut store) = real_tile_store();
    let mut g = ramp_fill(0.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    g.alphas = vec![aurora_core::FillStop {
        pos: 0.0,
        mid: 0.5,
        value: 0.5,
    }];
    let (layers, _) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Gradient,
            Some(gradient_paint(g)),
        )],
    );
    let acc = square_composite(&layers, &mut store);
    // Half-transparent red over nothing: premultiplied (0.5, 0, 0, 0.5).
    near(texel(&acc, 140, 120), [0.5, 0.0, 0.0, 0.5]);
}

#[test]
fn a_stroke_opacity_and_mode_apply_to_a_gradient_stroke() {
    let (_d, mut store) = real_tile_store();
    let g = ramp_fill(0.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    let mut stroke = painted_stroke(
        StrokePosition::Outside,
        2.0,
        StrokeFill::Gradient,
        Some(gradient_paint(g)),
    );
    stroke.opacity = 0.5;
    let (layers, _) = stroked_layer(&mut store, vec![stroke]);
    near(
        texel(&square_composite(&layers, &mut store), 140, 120),
        [0.5, 0.0, 0.0, 0.5],
    );
}

#[test]
fn a_pattern_stroke_samples_the_pool_like_a_pattern_overlay() {
    let (_d, mut store) = real_tile_store();
    // Origin (0, 0): pixel (x, y) is texel (x % 2, y % 2).
    let (layers, _) = stroked_layer(
        &mut store,
        vec![
            painted_stroke(
                StrokePosition::Outside,
                2.0,
                StrokeFill::Pattern,
                Some(pattern_paint("q", 1.0, (0.0, 0.0))),
            ),
            painted_stroke(
                StrokePosition::Inside,
                2.0,
                StrokeFill::Pattern,
                Some(pattern_paint("q", 1.0, (1.0, 0.0))),
            ),
        ],
    );
    let acc = square_composite(&layers, &mut store);
    // Outside ring right of the layer, rows 4 and 5.
    near(texel(&acc, 140, 120), PRED);
    near(texel(&acc, 141, 120), PGREEN);
    near(texel(&acc, 140, 121), PBLUE);
    near(texel(&acc, 141, 121), PWHITE);
    // The inside stroke is placed from its own origin (1, 0): pixel
    // (101, 120) is texel (0, 0), and (100, 120) the texel before it.
    near(texel(&acc, 101, 120), PRED);
    near(texel(&acc, 100, 120), PGREEN);
    near(texel(&acc, 101, 121), PBLUE);
}

#[test]
fn a_pattern_stroke_scale_interpolates() {
    let (_d, mut store) = real_tile_store();
    let (layers, _) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Pattern,
            Some(pattern_paint("q", 2.0, (0.0, 0.0))),
        )],
    );
    let acc = square_composite(&layers, &mut store);
    // 200 %: texel centres every 2 px, so pixel 141 is half way between
    // texels 0 and 1 (red and green) of row 0.
    near(texel(&acc, 140, 120), PRED);
    near(texel(&acc, 141, 120), [0.5, 0.5, 0.0, 1.0]);
}

#[test]
fn a_pattern_strokes_origin_follows_the_layers_bounds() {
    // The same 40 x 40 square, the layer's bounds shifted by one pixel
    // with it: the pattern shifts too (its origin is relative to the
    // bounds), so the picture is the same one pixel over.
    let mut rows = Vec::new();
    for ox in [0_i64, 1] {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let id = painted(&mut layers, &mut store, (ox, 0), (100 + ox, 100, 40, 40));
        ok(layers.add_pattern(quad("q")));
        let fx = aurora_core::LayerEffects {
            strokes: vec![painted_stroke(
                StrokePosition::Outside,
                3.0,
                StrokeFill::Pattern,
                Some(pattern_paint("q", 1.0, (0.0, 0.0))),
            )],
            ..aurora_core::LayerEffects::default()
        };
        ok(layers.set_effects(id, Some(fx)));
        let acc = composite_tile_at(&layers, &mut store, 0, 0, (0, 0));
        let row: Vec<[f32; 4]> = (136..146)
            .map(|x| texel(&acc, usize::try_from(x + ox).unwrap_or(0), 121))
            .collect();
        rows.push(row);
    }
    assert_eq!(rows.first(), rows.get(1));
    // And it is a real pattern: not one colour.
    let distinct: std::collections::BTreeSet<[u32; 4]> = rows
        .first()
        .into_iter()
        .flatten()
        .map(|t| t.map(f32::to_bits))
        .collect();
    assert!(distinct.len() >= 3, "{distinct:?}");
}

#[test]
fn a_stroke_without_its_paint_or_pattern_draws_nothing() {
    let (_d, mut store) = real_tile_store();
    let plain = {
        let (layers, _) = stroked_layer(&mut store, Vec::new());
        square_composite(&layers, &mut store)
    };
    for stroke in [
        painted_stroke(StrokePosition::Outside, 2.0, StrokeFill::Gradient, None),
        painted_stroke(StrokePosition::Outside, 2.0, StrokeFill::Pattern, None),
        // A pattern the pool lacks (a damaged document).
        painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Pattern,
            Some(pattern_paint("missing", 1.0, (0.0, 0.0))),
        ),
        // A paint of the wrong kind for the stroke's fill.
        painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Pattern,
            Some(gradient_paint(ramp_fill(
                0.0,
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
            ))),
        ),
    ] {
        let fx = aurora_core::LayerEffects {
            strokes: vec![stroke],
            ..aurora_core::LayerEffects::default()
        };
        let mut layers = aurora_doc::LayerTree::new();
        let id = painted(&mut layers, &mut store, (0, 0), (100, 100, 40, 40));
        ok(layers.add_pattern(quad("q")));
        // A mismatched paint is refused by validation; the others are kept.
        let _ = layers.set_effects(id, Some(fx));
        let acc = square_composite(&layers, &mut store);
        assert_eq!(texel(&acc, 140, 120), texel(&plain, 140, 120));
        near(texel(&acc, 140, 120), [0.0; 4]);
    }
}

#[test]
fn a_painted_stroke_refuses_the_gpu_path_and_an_unpaintable_one_does_not() {
    let (_d, mut store) = real_tile_store();
    let g = ramp_fill(0.0, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let (layers, id) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Gradient,
            Some(gradient_paint(g)),
        )],
    );
    assert!(layer_renders_effects(&layers, id));
    let (layers, id) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            2.0,
            StrokeFill::Gradient,
            None,
        )],
    );
    assert!(!layer_renders_effects(&layers, id));
}

#[test]
fn a_painted_strokes_reach_invalidates_the_tiles_it_falls_on() {
    let (_d, mut store) = real_tile_store();
    let g = ramp_fill(0.0, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let (layers, id) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            6.0,
            StrokeFill::Gradient,
            Some(gradient_paint(g)),
        )],
    );
    let dab = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };
    // Grown by the stroke's apron (7), so more than the dab itself.
    assert!(effect_reach_rects(&layers, id, &[dab]).len() > 1);
    let (layers, id) = stroked_layer(
        &mut store,
        vec![painted_stroke(
            StrokePosition::Outside,
            6.0,
            StrokeFill::Gradient,
            None,
        )],
    );
    assert_eq!(effect_reach_rects(&layers, id, &[dab]).len(), 1);
}

/// The corpus file with one gradient and one pattern stroke: both open
/// painted, the pattern is in the pool and its origin is relative to the
/// layer's bounds, and the open report no longer lists them as undrawn.
#[test]
fn stroke_effects_psd_opens_its_gradient_and_pattern_strokes_painted() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures/effects/stroke-effects.psd");
    let Ok(bytes) = std::fs::read(&path) else {
        println!("SKIPPED: corpus file not present");
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, _image) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let layers = &document.layers;
    let find = |name: &str| {
        let mut stack: Vec<_> = layers.roots().to_vec();
        while let Some(id) = stack.pop() {
            if layers.name(id) == Some(name) {
                return Some(id);
            }
            stack.extend(layers.children(id).unwrap_or(&[]).to_vec());
        }
        None
    };
    let paint_of = |name: &str| {
        let id = find(name)?;
        layers.effects(id)?.strokes.first()?.paint.clone()
    };
    let Some(StrokePaint::Gradient { reference, .. }) = paint_of("Gradient") else {
        unreachable!("the Gradient layer's stroke is painted");
    };
    let Some(reference) = reference else {
        unreachable!("a gradient stroke spans the record rectangle");
    };
    assert!(reference.width > 0 && reference.height > 0);
    let Some(StrokePaint::Pattern { pattern, origin, .. }) = paint_of("Pattern") else {
        unreachable!("the Pattern layer's stroke is painted");
    };
    assert!(layers.patterns().contains_key(&pattern));
    assert!(origin.0.is_finite() && origin.1.is_finite());
    assert!(
        !document
            .report
            .items
            .iter()
            .any(|line| line.contains("non-solid stroke")),
        "{:?}",
        document.report.items
    );
}
