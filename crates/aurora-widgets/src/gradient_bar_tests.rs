//! Task 113: the gradient bar's pointer routing and paint, headless.

use aurora_theme::{Palette, Theme, ThemeSet};
use taffy::style_helpers::length;
use taffy::{Display, FlexDirection, Overflow, Point as TPoint, Size, Style};

use crate::action::{ActionOutcome, ActionRejection};
use crate::input::FocusManager;
use crate::paint::{PaintOp, paint_widget_ops};
use crate::pointer::{ClickTracker, PointerEvent, PointerOutcome, PointerPhase, handle_pointer};
use crate::tree::{WidgetId, WidgetTree};
use crate::widgets::{
    self, GradientBarStop as Stop, WidgetKind, gradient_bar_colour_at, new_tree, test_scales,
};

const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");

fn theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(p) => p,
        Err(err) => unreachable!("{err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("{err:?}");
    }
    match themes.resolve("Dark", &palette) {
        Ok(t) => t,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn stops() -> Vec<Stop> {
    vec![
        Stop::new(0.0, [0.0, 0.0, 0.0]),
        Stop::new(0.5, [1.0, 0.0, 0.0]),
        Stop::new(1.0, [1.0, 1.0, 1.0]),
    ]
}

struct Rig {
    tree: WidgetTree<WidgetKind>,
    bar: WidgetId,
    focus: FocusManager,
    click: ClickTracker,
}

fn rig_with(stops: &[Stop], clip: bool) -> Rig {
    let (mut tree, root) = new_tree(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        size: Size {
            width: length(200.0_f32),
            height: length(100.0_f32),
        },
        ..Default::default()
    });
    let parent = if clip {
        // A clipping ancestor 80 px wide at the origin cuts the bar.
        ok(widgets::insert_container(
            &mut tree,
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                overflow: TPoint {
                    x: Overflow::Hidden,
                    y: Overflow::Hidden,
                },
                size: Size {
                    width: length(80.0_f32),
                    height: length(100.0_f32),
                },
                ..Default::default()
            },
        ))
    } else {
        root
    };
    let bar = ok(widgets::insert_gradient_bar(
        &mut tree,
        parent,
        &test_scales(),
        "Gradient preview",
        stops,
    ));
    if clip {
        // The bar keeps its full width inside the narrow clipping parent.
        let mut style = tree.style(bar).cloned().unwrap_or_default();
        style.size.width = length(200.0_f32);
        ok(tree.set_style(bar, style));
    }
    tree.compute_layout(200.0, 100.0);
    Rig {
        tree,
        bar,
        focus: FocusManager::new(),
        click: ClickTracker::default(),
    }
}

fn rig() -> Rig {
    rig_with(&stops(), false)
}

impl Rig {
    fn event(
        &mut self,
        phase: PointerPhase,
        position: (f32, f32),
    ) -> Result<PointerOutcome, ActionRejection> {
        handle_pointer(
            &mut self.tree,
            &mut self.focus,
            &mut self.click,
            PointerEvent { phase, position },
        )
    }

    fn x(&self, position: f32) -> f32 {
        let s = ok(widgets::gradient_bar_state(&self.tree, self.bar)).clone();
        let b = self.tree.bounds(self.bar);
        match b.and_then(|b| crate::widgets::gradient_bar_geometry(b, &s)) {
            Some(g) => g.x_of(position),
            None => unreachable!("laid out"),
        }
    }

    fn state(&self) -> widgets::GradientBarState {
        ok(widgets::gradient_bar_state(&self.tree, self.bar)).clone()
    }
}

// ---- pointer ----

#[test]
fn a_press_on_a_marker_selects_and_grabs_it_without_taking_focus() {
    let mut r = rig();
    let x = r.x(0.5);
    let out = ok(r.event(PointerPhase::Down, (x, 5.0)));
    assert_eq!(out, PointerOutcome::Changed(r.bar));
    assert_eq!(r.state().selected(), 1);
    assert_eq!(r.click.captured(), Some(r.bar));
    assert_eq!(r.focus.focused(), None, "the bar is not a tab stop");
}

#[test]
fn a_press_between_markers_does_nothing_and_grabs_nothing() {
    let mut r = rig();
    let x = (r.x(0.0) + r.x(0.5)) / 2.0;
    assert_eq!(
        ok(r.event(PointerPhase::Down, (x, 5.0))),
        PointerOutcome::Ignored
    );
    assert_eq!(r.click.captured(), None);
    assert_eq!(r.state().selected(), 0);
}

#[test]
fn a_drag_reports_each_move_and_the_up_releases() {
    let mut r = rig();
    let x = r.x(0.5);
    ok(r.event(PointerPhase::Down, (x, 5.0)));
    let to = r.x(0.3);
    let moved = ok(r.event(PointerPhase::Move, (to, 20.0)));
    assert_eq!(
        moved,
        PointerOutcome::Action(ActionOutcome::GradientBarChanged { bar: r.bar })
    );
    assert!((r.state().stops()[1].position - 0.3).abs() < 0.001);
    // The same place again changes nothing.
    assert_eq!(
        ok(r.event(PointerPhase::Move, (to, 20.0))),
        PointerOutcome::Ignored
    );
    assert_eq!(
        ok(r.event(PointerPhase::Up, (to, 20.0))),
        PointerOutcome::Released(r.bar)
    );
    assert_eq!(r.click.captured(), None);
    // After the release a move edits nothing.
    assert_eq!(
        ok(r.event(PointerPhase::Move, (r.x(0.4), 20.0))),
        PointerOutcome::Ignored
    );
    assert!((r.state().stops()[1].position - 0.3).abs() < 0.001);
}

#[test]
fn a_drag_never_passes_a_neighbour() {
    let mut r = rig();
    let x = r.x(0.5);
    ok(r.event(PointerPhase::Down, (x, 5.0)));
    ok(r.event(PointerPhase::Move, (9000.0, 5.0)));
    assert_eq!(r.state().stops()[1].position, 1.0);
    ok(r.event(PointerPhase::Move, (-9000.0, 5.0)));
    assert_eq!(r.state().stops()[1].position, 0.0);
    assert_eq!(r.state().stops().len(), 3);
}

#[test]
fn a_drag_ends_when_the_selection_or_the_stop_count_changes_under_it() {
    let mut r = rig();
    let x = r.x(0.5);
    ok(r.event(PointerPhase::Down, (x, 5.0)));
    ok(widgets::select_gradient_stop(&mut r.tree, r.bar, 2));
    assert_eq!(
        ok(r.event(PointerPhase::Move, (r.x(0.7), 5.0))),
        PointerOutcome::Cancelled(r.bar)
    );
    assert_eq!(r.click.captured(), None);
    assert_eq!(r.state().stops()[2].position, 1.0, "nothing moved");

    let mut r = rig();
    let x = r.x(0.5);
    ok(r.event(PointerPhase::Down, (x, 5.0)));
    let two = stops()[..2].to_vec();
    ok(widgets::set_gradient_bar_stops(&mut r.tree, r.bar, &two));
    assert_eq!(
        ok(r.event(PointerPhase::Move, (r.x(0.7), 5.0))),
        PointerOutcome::Cancelled(r.bar)
    );
}

#[test]
fn a_disabled_bar_refuses_a_press() {
    let mut r = rig();
    ok(widgets::set_gradient_bar_disabled(&mut r.tree, r.bar, true));
    let x = r.x(0.5);
    assert!(matches!(
        r.event(PointerPhase::Down, (x, 5.0)),
        Err(ActionRejection::Disabled(_))
    ));
    assert_eq!(r.click.captured(), None);
}

// ---- paint ----

fn ops(r: &Rig, theme: &Theme) -> Vec<PaintOp> {
    ok(paint_widget_ops(&r.tree, r.bar, theme, &test_scales(), 1.0))
}

fn gradient(ops: &[PaintOp]) -> Option<&aurora_vector::ColorMesh> {
    ops.iter().find_map(|op| match op {
        PaintOp::Gradient(m) => Some(m),
        _ => None,
    })
}

fn solids(ops: &[PaintOp]) -> Vec<&crate::paint::Paint> {
    ops.iter()
        .filter_map(|op| match op {
            PaintOp::Solid(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn bbox(mesh: &aurora_vector::Mesh) -> (f32, f32, f32, f32) {
    let xs = mesh.vertices.iter().map(|p| p.x);
    let ys = mesh.vertices.iter().map(|p| p.y);
    (
        xs.clone().fold(f32::MAX, f32::min),
        ys.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
        ys.fold(f32::MIN, f32::max),
    )
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
}

#[test]
fn the_bar_paints_its_gradient_across_the_track_in_the_stops_own_colours() {
    let r = rig();
    let all = ops(&r, &theme());
    assert!(
        matches!(all.first(), Some(PaintOp::Gradient(_))),
        "gradient first"
    );
    let Some(mesh) = gradient(&all) else {
        unreachable!("a gradient")
    };
    let scales = test_scales();
    let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.position.x).collect();
    let (lo, hi) = (
        xs.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
    );
    let half = scales.spacing.sm as f32 / 2.0;
    assert_eq!((lo, hi), (half, 200.0 - half));
    // Every vertex colour is the model's colour at its own x.
    let track = hi - lo;
    for v in &mesh.vertices {
        let t = (v.position.x - lo) / track;
        let want = gradient_bar_colour_at(&stops(), t).unwrap_or([9.0; 3]);
        assert!(
            close([v.color[0], v.color[1], v.color[2]], want),
            "x {} t {t}: {:?} vs {want:?}",
            v.position.x,
            v.color
        );
        assert_eq!(v.color[3], 1.0);
    }
    let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.position.y).collect();
    assert_eq!(ys.iter().copied().fold(f32::MAX, f32::min), 0.0);
    assert_eq!(
        ys.iter().copied().fold(f32::MIN, f32::max),
        scales.spacing.lg as f32
    );
}

#[test]
fn a_hard_step_paints_a_jump_not_a_blend() {
    let hard = [
        Stop::new(0.0, [0.0; 3]),
        Stop::new(0.5, [0.2; 3]),
        Stop::new(0.5, [0.8; 3]),
        Stop::new(1.0, [1.0; 3]),
    ];
    let r = rig_with(&hard, false);
    let all = ops(&r, &theme());
    let Some(mesh) = gradient(&all) else {
        unreachable!("a gradient")
    };
    let mid = r.x(0.5);
    let at_mid: Vec<[f32; 4]> = mesh
        .vertices
        .iter()
        .filter(|v| (v.position.x - mid).abs() < 1e-4)
        .map(|v| v.color)
        .collect();
    assert!(at_mid.iter().any(|c| close([c[0], c[1], c[2]], [0.2; 3])));
    assert!(at_mid.iter().any(|c| close([c[0], c[1], c[2]], [0.8; 3])));
    assert!(
        mesh.vertices.iter().all(|v| v.position.x <= mid + 1e-4 || {
            let c = v.color;
            c[0] >= 0.8 - 1e-5
        }),
        "nothing right of the step is darker than the later stop"
    );
}

#[test]
fn a_clipped_bar_paints_only_the_visible_part_with_the_gradients_own_colours() {
    let r = rig_with(&stops(), true);
    let all = ops(&r, &theme());
    let Some(mesh) = gradient(&all) else {
        unreachable!("a gradient")
    };
    let max_x = mesh
        .vertices
        .iter()
        .map(|v| v.position.x)
        .fold(f32::MIN, f32::max);
    assert_eq!(max_x, 80.0, "cut at the clip");
    let half = test_scales().spacing.sm as f32 / 2.0;
    let track = 200.0 - 2.0 * half;
    for v in &mesh.vertices {
        let t = (v.position.x - half) / track;
        let want = gradient_bar_colour_at(&stops(), t).unwrap_or([9.0; 3]);
        assert!(
            close([v.color[0], v.color[1], v.color[2]], want),
            "x {}",
            v.position.x
        );
    }
}

#[test]
fn the_selected_marker_alone_has_the_accent_outline_and_is_drawn_last() {
    let mut r = rig();
    let t = theme();
    let accent = t.accent.primary.to_srgb_f32();
    let find = |ops: &[PaintOp]| -> Vec<usize> {
        solids(ops)
            .iter()
            .enumerate()
            .filter(|(_, (_, c))| close([c[0], c[1], c[2]], accent))
            .map(|(i, _)| i)
            .collect()
    };
    let all = ops(&r, &t);
    let rings = find(&all);
    assert_eq!(rings.len(), 1, "one accent shape");
    let s = solids(&all);
    assert_eq!(rings.first().copied(), s.len().checked_sub(1), "last");
    let centre = |p: &crate::paint::Paint| {
        let (x0, _, x1, _) = bbox(&p.0);
        (x0 + x1) / 2.0
    };
    assert!((centre(s[s.len() - 1]) - r.x(0.0)).abs() < 1.0, "on stop 0");
    ok(widgets::select_gradient_stop(&mut r.tree, r.bar, 2));
    let all = ops(&r, &t);
    let s = solids(&all);
    let rings = find(&all);
    assert_eq!(rings.len(), 1);
    assert_eq!(rings.first().copied(), s.len().checked_sub(1), "still last");
    assert!(
        (centre(s[s.len() - 1]) - r.x(1.0)).abs() < 1.0,
        "now on stop 2"
    );
}

#[test]
fn every_marker_is_filled_with_its_stops_colour_below_the_bar() {
    let r = rig();
    let all = ops(&r, &theme());
    let scales = test_scales();
    for stop in stops() {
        let found = solids(&all).into_iter().any(|(mesh, c)| {
            let (x0, y0, x1, _) = bbox(mesh);
            close([c[0], c[1], c[2]], stop.colour)
                && c[3] == 1.0
                && ((x0 + x1) / 2.0 - r.x(stop.position)).abs() < 1.0
                && y0 >= scales.spacing.lg as f32 - 0.01
        });
        assert!(found, "no filled marker for {stop:?}");
    }
}

#[test]
fn a_disabled_bar_dims_the_gradient_and_every_solid() {
    let mut r = rig();
    ok(widgets::set_gradient_bar_disabled(&mut r.tree, r.bar, true));
    let t = theme();
    let all = ops(&r, &t);
    let Some(mesh) = gradient(&all) else {
        unreachable!("a gradient")
    };
    assert!(
        mesh.vertices
            .iter()
            .all(|v| v.color[3] == t.state.disabled_opacity)
    );
    assert!(!solids(&all).is_empty());
    assert!(
        solids(&all)
            .iter()
            .all(|(_, c)| c[3] <= t.state.disabled_opacity)
    );
}

#[test]
fn a_marker_cut_by_a_clip_is_not_drawn_but_the_visible_ones_are() {
    let r = rig_with(&stops(), true);
    let all = ops(&r, &theme());
    let drawn = solids(&all)
        .into_iter()
        .filter(|(mesh, c)| c[3] == 1.0 && bbox(mesh).3 > test_scales().spacing.lg as f32)
        .count();
    // Only the marker at 0 (x 0..12) fits whole inside the 80 px clip; the
    // one at 0.5 (x ~100) is outside it and the one at 1 too.
    assert_eq!(drawn, 2, "the marker's fill and its selected ring");
}
