// ---- task 110: gradient overlays ----
//
// Included into `fill_and_overlay_tests` (uses `solid`, `composite`,
// `texel`, `near`, `ok`, `real_tile_store`, `painted`, `bounds` (a 10 x 10
// layer), `set_overlays`).

use aurora_core::effects::{EffectBlendMode, GradientOverlay, ReferenceBox};
use aurora_core::{FillStop, GradientFill, GradientStyle};

fn ramp_fill(angle: f64, from: [f32; 3], to: [f32; 3]) -> GradientFill {
    GradientFill {
        colours: vec![
            FillStop {
                pos: 0.0,
                mid: 0.5,
                value: from,
            },
            FillStop {
                pos: 1.0,
                mid: 0.5,
                value: to,
            },
        ],
        alphas: Vec::new(),
        style: GradientStyle::Linear,
        angle,
        scale: 1.0,
        reverse: false,
        align: true,
        offset: (0.0, 0.0),
    }
}

fn grad_overlay(gradient: GradientFill) -> GradientOverlay {
    GradientOverlay {
        gate: aurora_core::effects::EffectGate::default(),
        blend_mode: EffectBlendMode::Normal,
        opacity: 1.0,
        gradient,
        dither: false,
        reference: None,
    }
}

fn set_gradients(
    layers: &mut aurora_doc::LayerTree,
    id: aurora_doc::LayerId,
    gradients: Vec<GradientOverlay>,
) {
    let fx = aurora_core::LayerEffects {
        gradient_overlays: gradients,
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
}

const GBLACK: [f32; 3] = [0.0; 3];
const GWHITE: [f32; 3] = [1.0; 3];

fn grey_at(acc: &[half::f16], x: usize, y: usize) -> f32 {
    texel(acc, x, y)[0]
}

#[test]
fn a_horizontal_gradient_spans_the_layer_bounds_at_texel_centres() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    set_gradients(&mut layers, id, vec![grad_overlay(ramp_fill(0.0, GBLACK, GWHITE))]);
    let acc = composite(&layers, &mut store);
    // t = 0.5 + (x + 0.5 - 5) / 10 for a 10-wide layer.
    for x in 0..10 {
        let want = (x as f32 + 0.5) / 10.0;
        near(texel(&acc, x, 3), [want, want, want, 1.0]);
    }
    // Constant down a column.
    near(texel(&acc, 4, 0), texel(&acc, 4, 9));
}

#[test]
fn angle_ninety_runs_bottom_to_top() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    set_gradients(&mut layers, id, vec![grad_overlay(ramp_fill(90.0, GBLACK, GWHITE))]);
    let acc = composite(&layers, &mut store);
    assert!(grey_at(&acc, 2, 0) > 0.9, "top is the last stop");
    assert!(grey_at(&acc, 2, 9) < 0.1, "bottom is the first stop");
    near(texel(&acc, 0, 4), texel(&acc, 9, 4));
}

#[test]
fn reverse_swaps_the_ends() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    let mut g = ramp_fill(0.0, GBLACK, GWHITE);
    g.reverse = true;
    set_gradients(&mut layers, id, vec![grad_overlay(g)]);
    let acc = composite(&layers, &mut store);
    near(texel(&acc, 0, 0), [0.95, 0.95, 0.95, 1.0]);
    near(texel(&acc, 9, 0), [0.05, 0.05, 0.05, 1.0]);
}

#[test]
fn overlay_opacity_mixes_with_the_content() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    let mut o = grad_overlay(ramp_fill(0.0, GWHITE, GWHITE));
    o.opacity = 0.25;
    set_gradients(&mut layers, id, vec![o]);
    near(
        texel(&composite(&layers, &mut store), 5, 5),
        [1.0, 0.25, 0.25, 1.0],
    );
}

#[test]
fn overlay_blend_mode_is_used() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [0.5, 1.0, 0.5, 1.0]);
    let mut o = grad_overlay(ramp_fill(0.0, [0.5; 3], [0.5; 3]));
    o.blend_mode = EffectBlendMode::Multiply;
    set_gradients(&mut layers, id, vec![o]);
    near(
        texel(&composite(&layers, &mut store), 5, 5),
        [0.25, 0.5, 0.25, 1.0],
    );
}

#[test]
fn transparency_stops_make_the_gradient_translucent() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    let mut g = ramp_fill(0.0, GWHITE, GWHITE);
    g.alphas = vec![
        FillStop {
            pos: 0.0,
            mid: 0.5,
            value: 0.0,
        },
        FillStop {
            pos: 1.0,
            mid: 0.5,
            value: 1.0,
        },
    ];
    set_gradients(&mut layers, id, vec![grad_overlay(g)]);
    let acc = composite(&layers, &mut store);
    // At x = 0 the stop is ~5 % white over red; at x = 9 ~95 %.
    near(texel(&acc, 0, 0), [1.0, 0.05, 0.05, 1.0]);
    near(texel(&acc, 9, 0), [1.0, 0.95, 0.95, 1.0]);
}

#[test]
fn the_reference_box_places_the_gradient_and_the_shape_clips_it() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    // A 4 x 4 square at (2, 2) on a layer whose bounds are 4096 square.
    let id = painted(&mut layers, &mut store, (0, 0), (2, 2, 4, 4));
    let mut o = grad_overlay(ramp_fill(0.0, GBLACK, GWHITE));
    o.reference = Some(ReferenceBox {
        dx: 2,
        dy: 2,
        width: 4,
        height: 4,
    });
    set_gradients(&mut layers, id, vec![o.clone()]);
    let acc = composite(&layers, &mut store);
    near(texel(&acc, 2, 3), [0.125, 0.125, 0.125, 1.0]);
    near(texel(&acc, 5, 3), [0.875, 0.875, 0.875, 1.0]);
    near(texel(&acc, 1, 3), [0.0; 4]);
    near(texel(&acc, 6, 3), [0.0; 4]);
    // Without the box the gradient spans the 4096-px bounds: nearly flat
    // mid-grey over the square.
    o.reference = None;
    set_gradients(&mut layers, id, vec![o]);
    let acc = composite(&layers, &mut store);
    assert!(grey_at(&acc, 3, 3) < 0.01, "t is near 0 far left of the 4096 box's centre");
}

#[test]
fn fill_does_not_fade_the_gradient_and_opacity_does() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [1.0, 0.0, 0.0, 1.0]);
    set_gradients(&mut layers, id, vec![grad_overlay(ramp_fill(0.0, GWHITE, GWHITE))]);
    ok(layers.set_fill_opacity(id, 0.0));
    near(texel(&composite(&layers, &mut store), 5, 5), [1.0; 4]);
    ok(layers.set_opacity(id, 0.5));
    near(texel(&composite(&layers, &mut store), 5, 5), [0.5; 4]);
}

#[test]
fn a_gradient_overlay_sits_above_a_colour_overlay() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [0.0, 0.0, 0.0, 1.0]);
    let fx = aurora_core::LayerEffects {
        color_overlays: vec![overlay(EffectBlendMode::Normal, [0.0, 1.0, 0.0], 1.0)],
        gradient_overlays: vec![{
            let mut o = grad_overlay(ramp_fill(0.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
            o.opacity = 0.5;
            o
        }],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    near(
        texel(&composite(&layers, &mut store), 5, 5),
        [0.5, 0.5, 0.0, 1.0],
    );
}

#[test]
fn a_shown_gradient_overlay_refuses_the_gpu_path_and_gates_hide_it() {
    let (_d, mut store) = real_tile_store();
    let mut layers = aurora_doc::LayerTree::new();
    let id = solid(&mut layers, &mut store, None, [0.0, 1.0, 0.0, 1.0]);
    assert!(document_qualifies_for_gpu_compositing(&layers));
    set_gradients(&mut layers, id, vec![grad_overlay(ramp_fill(0.0, GBLACK, GWHITE))]);
    assert!(layer_renders_effects(&layers, id));
    assert!(!document_qualifies_for_gpu_compositing(&layers));
    let mut off = grad_overlay(ramp_fill(0.0, GBLACK, GWHITE));
    off.gate.enabled = false;
    set_gradients(&mut layers, id, vec![off]);
    assert!(!layer_renders_effects(&layers, id));
    assert!(document_qualifies_for_gpu_compositing(&layers));
    // Hidden, it draws nothing even if the effect path is taken (a second
    // shown overlay on the layer forces it).
    let mut shown = grad_overlay(ramp_fill(0.0, [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]));
    shown.opacity = 1.0;
    let mut hidden = grad_overlay(ramp_fill(0.0, GBLACK, GBLACK));
    hidden.gate.present = false;
    set_gradients(&mut layers, id, vec![shown, hidden]);
    near(
        texel(&composite(&layers, &mut store), 5, 5),
        [1.0, 0.0, 0.0, 1.0],
    );
    let fx = aurora_core::LayerEffects {
        enabled: false,
        gradient_overlays: vec![grad_overlay(ramp_fill(0.0, GBLACK, GWHITE))],
        ..aurora_core::LayerEffects::default()
    };
    ok(layers.set_effects(id, Some(fx)));
    assert!(!layer_renders_effects(&layers, id));
}

/// `(file, mean bound after, max bound after)`. Mean errors against
/// Photoshop's merged images (levels) BEFORE task 110 -> AFTER:
/// `fill_adjustments.psd` 54.38 -> 34.18, `effects/shape-fx2.psd`
/// 41.39 -> 1.29, `layer_comps.psd` 9.63 -> 9.51,
/// `effects/stroke-composite.psd` 13.98 -> 6.30, `layer_effects.psd`
/// 7.77 -> 6.75, `masks.psd` 0.164 -> 0.154. Bounds sit just above the
/// measurement. `stroke-composite`'s max is not bounded: a clip group whose
/// base carries effects adds each clipped layer's change to the
/// accumulator (an approximation, `fold_clip_group`) and leaves one
/// out-of-range texel there.
const GRADIENT_OVERLAY_FIDELITY: [(&str, f64); 6] = [
    ("fill_adjustments.psd", 34.5),
    ("effects/shape-fx2.psd", 1.4),
    ("layer_comps.psd", 9.6),
    ("effects/stroke-composite.psd", 6.5),
    ("layer_effects.psd", 6.9),
    ("masks.psd", 0.16),
];

#[test]
fn gradient_overlays_match_photoshops_merged_images_better_than_before() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    for (file, mean_bound) in GRADIENT_OVERLAY_FIDELITY {
        let path = root.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("SKIPPED: corpus file not present at {}", path.display());
            continue;
        };
        let Ok(merged) = aurora_io::decode_psd_merged_image(&bytes) else {
            unreachable!("{file}: no merged image");
        };
        let (_scratch, mut store) = real_tile_store();
        let (document, after) = open_and_composite_psd(
            &bytes,
            &mut store,
            (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
        );
        let drawn = crate::layer_ids_in_order(&document.layers)
            .into_iter()
            .filter_map(|id| document.layers.effects(id))
            .map(aurora_core::LayerEffects::shown_gradient_overlays)
            .sum::<usize>();
        assert!(drawn > 0, "{file}: no gradient overlay was read");
        let Some((mean, _max)) = audit_diff(&after, &merged) else {
            unreachable!("{file}: sizes differ");
        };
        let mean = mean.iter().sum::<f64>() / 3.0;
        println!("GRADIENT-OVERLAY {file}: mean {mean:.3}");
        assert!(mean <= mean_bound, "{file}: mean {mean} over {mean_bound}");
    }
}

#[test]
fn an_imported_overlay_spans_the_record_rectangle_not_the_canvas_sized_layer() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpora/psd/reference/psd-tools-fixtures");
    let Ok(bytes) = std::fs::read(root.join("fill_adjustments.psd")) else {
        println!("SKIPPED: corpus file not present");
        return;
    };
    let (_scratch, mut store) = real_tile_store();
    let (document, _) = open_and_composite_psd(
        &bytes,
        &mut store,
        (aurora_doc::LayerTree::new(), aurora_doc::History::new()),
    );
    let mut seen = 0;
    for id in crate::layer_ids_in_order(&document.layers) {
        let Some(fx) = document.layers.effects(id) else {
            continue;
        };
        for o in fx.gradient_overlays.iter().filter(|o| fx.shows(o.gate)) {
            let (Some(r), Some(b)) = (o.reference, document.layers.bounds(id)) else {
                unreachable!("an imported overlay carries its record rectangle");
            };
            let at = r.within(b);
            // Rectangle 1's record is (-1, -1)-(129, 129).
            assert_eq!((at.width, at.height), (130, 130));
            assert!(at.x == -1 || at.x == 127, "{at:?}");
            seen += 1;
        }
    }
    assert_eq!(seen, 2, "Rectangle 1 and Rectangle 3");
}
