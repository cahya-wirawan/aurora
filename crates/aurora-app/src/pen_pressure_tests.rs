//! Pen pressure for the Brush, Pencil and Eraser (0.308.0), driven the way
//! `App` drives a stroke: `begin_drag`, then per event
//! `continue_drag_pressured` plus `pen_pressure::pressure_along` for each
//! dab, stamped through `selection_paint::dab_through_selection_pressured`.
//! `App` itself needs a window, so its thin glue (which fields it passes) is
//! mirrored here and not run.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::indexing_slicing
)]

use super::{RED, Rig};
use crate::{
    DabTool, Drag, PointerButton, begin_drag, continue_drag_pressured, pan_bounds, pen_pressure,
    selection_paint,
};
use aurora_ui::Tool;

type Sample = ((f32, f32), f32);

/// One stroke of `tool` through `samples` (canvas point, pressure), the way
/// `App` lays it: press dab, then each move event's interpolated dabs. The
/// stroke is left open in the rig's snapshot (no history step needed).
fn pressured_stroke(rig: &mut Rig, tool: Tool, samples: &[Sample], preserve_alpha: bool) {
    let Some(&(first, first_p)) = samples.first() else {
        unreachable!("a stroke has samples");
    };
    let mut drag = begin_drag(
        tool,
        PointerButton::Primary,
        first,
        &rig.view,
        crate::active_pixel_layer(&rig.layers, Some(rig.layer)),
    );
    let dab_tool = if tool == Tool::Eraser {
        DabTool::Eraser
    } else {
        crate::dab_tool_of_drag(drag.as_ref())
    };
    let colour = if dab_tool == DabTool::Eraser {
        [0.0; 3]
    } else {
        RED
    };
    let press = match drag.as_ref() {
        Some(Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. }) => *last_doc,
        _ => unreachable!("a paint tool starts a stroke"),
    };
    let stamp = |rig: &mut Rig, drag: &mut Option<Drag>, at: (f32, f32), p: f32| {
        let _ = selection_paint::dab_through_selection_pressured(
            dab_tool,
            preserve_alpha,
            &mut rig.store,
            rig.surface,
            rig.bounds,
            at,
            rig.selection.active(),
            &rig.settings,
            colour,
            drag,
            p,
        );
    };
    let mut previous = pen_pressure::clean(first_p);
    stamp(rig, &mut drag, press, previous);
    for &(point, raw) in samples.iter().skip(1) {
        let now = pen_pressure::clean(raw);
        let bounds = pan_bounds(&rig.layers, Some(rig.layer), Some((1000.0, 800.0)));
        let from = match drag.as_ref() {
            Some(Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. }) => *last_doc,
            _ => unreachable!("a paint stroke"),
        };
        let dabs = match drag.as_mut() {
            Some(d) => continue_drag_pressured(
                d,
                point,
                &mut rig.view,
                &mut rig.selection,
                &rig.settings,
                bounds,
                (previous, now),
            ),
            None => Vec::new(),
        };
        let to = match drag.as_ref() {
            Some(Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. }) => *last_doc,
            _ => unreachable!("a paint stroke"),
        };
        for dab in dabs {
            let p = pen_pressure::pressure_along(from, to, previous, now, dab);
            stamp(rig, &mut drag, dab, p);
        }
        previous = now;
    }
}

/// The pre-0.308.0 stroke: the engine entry points with the settings' own
/// radius and amount and `dab_step` spacing, no pressure anywhere.
fn engine_stroke(rig: &mut Rig, tool: Tool, points: &[(f32, f32)]) {
    let Some(&first) = points.first() else {
        unreachable!("a stroke has points");
    };
    let mut drag = begin_drag(
        tool,
        PointerButton::Primary,
        first,
        &rig.view,
        crate::active_pixel_layer(&rig.layers, Some(rig.layer)),
    );
    let dab_tool = if tool == Tool::Eraser {
        DabTool::Eraser
    } else {
        crate::dab_tool_of_drag(drag.as_ref())
    };
    let radius = rig.settings.dab_radius(dab_tool);
    let amount = rig.settings.paint_amount(dab_tool);
    let step = aurora_brush::dab_step(radius, aurora_brush::DEFAULT_SPACING);
    let dab = |rig: &mut Rig, drag: &mut Option<Drag>, at: (f32, f32)| {
        let local = crate::layer_local_point(rig.bounds, at);
        let _ = if dab_tool == DabTool::Eraser {
            aurora_brush::erase_dab_amount(
                &mut rig.store,
                rig.surface,
                local,
                radius,
                crate::eraser_stroke_mut(drag),
                None,
                amount,
            )
        } else {
            aurora_brush::stamp_dab_amount(
                &mut rig.store,
                rig.surface,
                local,
                radius,
                RED,
                crate::brush_stroke_mut(drag),
                None,
                false,
                dab_tool == DabTool::Pencil,
                amount,
            )
        };
    };
    dab(rig, &mut drag, first);
    let (mut last, mut carry) = (first, 0.0_f32);
    for &p in points.iter().skip(1) {
        let (dabs, new_carry) = aurora_brush::advance_segment(last, p, carry, step);
        for d in dabs {
            dab(rig, &mut drag, d);
        }
        last = p;
        carry = new_carry;
    }
}

fn region(rig: &mut Rig) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for y in 0..120 {
        for x in 0..260 {
            out.push(rig.at(x, y));
        }
    }
    out
}

fn path() -> Vec<(f32, f32)> {
    vec![
        (20.0, 60.0),
        (47.0, 63.0),
        (90.0, 55.0),
        (130.0, 70.0),
        (200.0, 60.0),
    ]
}

fn rig() -> Rig {
    Rig::new(super::rect(0, 0, 400, 300))
}

fn with_samples(points: &[(f32, f32)], pressure: f32) -> Vec<Sample> {
    points.iter().map(|&p| (p, pressure)).collect()
}

/// Rows with any alpha in layer column `x`.
fn thickness(rig: &mut Rig, x: u32) -> u32 {
    (0..300).filter(|&y| rig.at(x, y)[3] > 0.0).count() as u32
}

#[test]
fn full_pressure_equals_the_pre_pressure_strokes_for_every_tool() {
    for tool in [Tool::Brush, Tool::Pencil, Tool::Eraser] {
        for (size, opacity_on) in [(true, true), (true, false), (false, true)] {
            let mut old = rig();
            let mut new = rig();
            for r in [&mut old, &mut new] {
                r.settings.pressure_size = size;
                r.settings.pressure_opacity = opacity_on;
                r.settings.brush_radius = 12.0;
                r.settings.eraser_radius = 12.0;
                // Something under the Eraser, and a non-trivial Opacity/Flow.
                r.rect_fill(0, 0, 260, 120, [0.2, 0.4, 0.6, 1.0]);
                r.settings.brush_opacity = 70;
                r.settings.brush_flow = 60;
                r.settings.pencil_opacity = 80;
                r.settings.eraser_opacity = 90;
                r.settings.eraser_flow = 50;
            }
            engine_stroke(&mut old, tool, &path());
            pressured_stroke(&mut new, tool, &with_samples(&path(), 1.0), false);
            assert_eq!(
                region(&mut old),
                region(&mut new),
                "{tool:?} size {size} opacity {opacity_on}"
            );
        }
    }
}

#[test]
fn with_both_options_off_any_pressure_changes_nothing() {
    for tool in [Tool::Brush, Tool::Pencil, Tool::Eraser] {
        let mut old = rig();
        let mut new = rig();
        for r in [&mut old, &mut new] {
            r.settings.pressure_size = false;
            r.settings.pressure_opacity = false;
            r.settings.brush_radius = 10.0;
            r.settings.eraser_radius = 10.0;
            r.rect_fill(0, 0, 260, 120, [0.2, 0.4, 0.6, 1.0]);
        }
        engine_stroke(&mut old, tool, &path());
        let tapered: Vec<Sample> = path()
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, 0.9 - 0.2 * i as f32))
            .collect();
        pressured_stroke(&mut new, tool, &tapered, false);
        assert_eq!(region(&mut old), region(&mut new), "{tool:?}");
    }
}

#[test]
fn a_stroke_at_falling_pressure_is_narrower_at_its_tail() {
    for tool in [Tool::Brush, Tool::Pencil] {
        let mut r = rig();
        r.settings.brush_radius = 20.0;
        let line = [
            (20.0, 100.0),
            (60.0, 100.0),
            (100.0, 100.0),
            (140.0, 100.0),
            (180.0, 100.0),
        ];
        let samples: Vec<Sample> = line
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, 1.0 - 0.2 * i as f32))
            .collect();
        pressured_stroke(&mut r, tool, &samples, false);
        let head = thickness(&mut r, 30);
        let mid = thickness(&mut r, 100);
        let tail = thickness(&mut r, 175);
        assert!(head > mid && mid > tail, "{tool:?}: {head} {mid} {tail}");
        assert!(tail >= 2, "{tool:?}: the tail keeps the 1 px floor: {tail}");
    }
}

#[test]
fn the_eraser_at_falling_pressure_clears_a_narrower_tail() {
    let mut r = rig();
    r.settings.eraser_radius = 20.0;
    r.rect_fill(0, 0, 260, 200, [0.0, 0.0, 1.0, 1.0]);
    let line = [
        (20.0, 100.0),
        (60.0, 100.0),
        (100.0, 100.0),
        (140.0, 100.0),
        (180.0, 100.0),
    ];
    let samples: Vec<Sample> = line
        .iter()
        .enumerate()
        .map(|(i, &p)| (p, 1.0 - 0.2 * i as f32))
        .collect();
    pressured_stroke(&mut r, Tool::Eraser, &samples, false);
    let cleared = |r: &mut Rig, x| (0..200).filter(|&y| r.at(x, y)[3] < 0.5).count();
    let (head, tail) = (cleared(&mut r, 30), cleared(&mut r, 175));
    assert!(head > tail && tail >= 1, "{head} {tail}");
}

#[test]
fn constant_pressure_sets_a_constant_radius() {
    let mut r = rig();
    r.settings.brush_radius = 20.0;
    let line = [(30.0, 100.0), (150.0, 100.0)];
    pressured_stroke(&mut r, Tool::Brush, &with_samples(&line, 0.5), false);
    // Radius 10: the centre column spans about 20 rows.
    let t = thickness(&mut r, 90);
    assert!((17..=22).contains(&t), "{t}");
}

#[test]
fn pressure_for_opacity_caps_the_stroke_at_pressure_times_opacity() {
    let mut r = rig();
    r.settings.pressure_size = false;
    r.settings.pressure_opacity = true;
    r.settings.brush_radius = 8.0;
    r.settings.brush_opacity = 80;
    let line = [(30.0, 100.0), (150.0, 100.0)];
    pressured_stroke(&mut r, Tool::Brush, &with_samples(&line, 0.5), false);
    let a = r.at(90, 100)[3];
    assert!((a - 0.4).abs() < 0.01, "0.5 pressure on 80 % opacity: {a}");
    // Full pressure reaches the user's Opacity and never passes it.
    let mut full = rig();
    full.settings.pressure_size = false;
    full.settings.pressure_opacity = true;
    full.settings.brush_radius = 8.0;
    full.settings.brush_opacity = 80;
    pressured_stroke(&mut full, Tool::Brush, &with_samples(&line, 1.0), false);
    let a = full.at(90, 100)[3];
    assert!((a - 0.8).abs() < 0.01 && a <= 0.8 + 1e-3, "{a}");
}

#[test]
fn a_light_tail_never_lightens_a_heavy_start_over_the_same_texels() {
    for tool in [Tool::Brush, Tool::Pencil] {
        let mut r = rig();
        r.settings.pressure_size = false;
        r.settings.pressure_opacity = true;
        r.settings.brush_radius = 10.0;
        r.settings.brush_opacity = 100;
        // Out at full pressure, then back over the same ground at 0.1.
        let samples = [
            ((40.0, 100.0), 1.0),
            ((100.0, 100.0), 1.0),
            ((40.0, 100.0), 0.1),
        ];
        pressured_stroke(&mut r, tool, &samples, false);
        let a = r.at(70, 100)[3];
        assert!(a > 0.95, "{tool:?}: the heavy pass stays: {a}");
    }
}

#[test]
fn the_eraser_with_pressure_for_opacity_only_erases_further() {
    let mut r = rig();
    r.settings.pressure_size = false;
    r.settings.pressure_opacity = true;
    r.settings.eraser_radius = 10.0;
    r.rect_fill(0, 0, 260, 200, [0.0, 0.0, 1.0, 1.0]);
    let samples = [
        ((40.0, 100.0), 1.0),
        ((100.0, 100.0), 1.0),
        ((40.0, 100.0), 0.2),
    ];
    pressured_stroke(&mut r, Tool::Eraser, &samples, false);
    assert!(r.at(70, 100)[3] < 0.05, "{}", r.at(70, 100)[3]);
    let mut half = rig();
    half.settings.pressure_size = false;
    half.settings.pressure_opacity = true;
    half.settings.eraser_radius = 10.0;
    half.rect_fill(0, 0, 260, 200, [0.0, 0.0, 1.0, 1.0]);
    pressured_stroke(
        &mut half,
        Tool::Eraser,
        &with_samples(&[(40.0, 100.0), (100.0, 100.0)], 0.5),
        false,
    );
    let left = half.at(70, 100)[3];
    assert!(
        (left - 0.5).abs() < 0.02,
        "half pressure leaves half: {left}"
    );
}

#[test]
fn a_transparency_locked_brush_keeps_alpha_under_pressure() {
    let mut r = rig();
    r.settings.pressure_opacity = true;
    r.settings.brush_radius = 10.0;
    r.rect_fill(0, 0, 260, 200, [0.0, 0.0, 1.0, 0.5]);
    pressured_stroke(
        &mut r,
        Tool::Brush,
        &with_samples(&[(40.0, 100.0), (100.0, 100.0)], 0.4),
        true,
    );
    assert_eq!(r.at(70, 100)[3], 0.5);
}

#[test]
fn the_other_dab_tools_do_not_take_pressure() {
    // `App::paint_dab` hands the pressure only to the stamp helper below the
    // early returns; each of these tools' dab methods has no pressure
    // parameter, and the Quick Mask dab is called without it.
    let source = include_str!("lib.rs");
    for line in [
        "fn clone_dab(&mut self, doc_point: (f32, f32), offset: (i64, i64)) {",
        "fn smudge_dab(&mut self, doc_point: (f32, f32)) {",
        "fn filter_dab(&mut self, doc_point: (f32, f32)) {",
        "fn tone_dab(&mut self, doc_point: (f32, f32)) {",
        "fn heal_dab(&mut self, doc_point: (f32, f32)) {",
    ] {
        assert!(source.contains(line), "{line}");
    }
    assert!(!source.contains("quick_mask::paint_dab(\n                mode,\n                dab_tool,\n                doc_point,\n                &self.tool_settings,\n                &self.colour.state,\n                pressure"));
    // And the engine-level dispatch the Quick Mask etc. skip: a Brush stamp
    // through the covered wrapper (used by non-pressure callers) ignores the
    // settings' pressure options entirely.
    let mut a = rig();
    let mut b = rig();
    a.settings.pressure_size = true;
    b.settings.pressure_size = false;
    for r in [&mut a, &mut b] {
        r.settings.brush_radius = 9.0;
        let mut drag = begin_drag(
            Tool::Brush,
            PointerButton::Primary,
            (50.0, 50.0),
            &r.view,
            crate::active_pixel_layer(&r.layers, Some(r.layer)),
        );
        let _ = selection_paint::dab_through_selection(
            DabTool::Brush,
            false,
            &mut r.store,
            r.surface,
            r.bounds,
            (50.0, 50.0),
            None,
            &r.settings,
            RED,
            &mut drag,
        );
    }
    assert_eq!(region(&mut a), region(&mut b));
}
