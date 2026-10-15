//! The Quick Select tool (0.238.0) as the app drives it: `begin_drag`,
//! `quick_select::{begin, advance/continue_drag, stamp, refresh, release}`,
//! `commit_drag_into_history` and the undo order, with a real
//! `SelectionSet`. `App` needs a window, so its wrappers (which `self`
//! fields they pass) are not run here; a source check pins the one
//! line that routes a stamp.

use super::*;
use crate::quick_select;
use aurora_doc::SelectionMode;

const BLUE_PX: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const RED_PX: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

impl Rig {
    /// A Quick Select press as the app runs it (`begin_drag`, the mode's
    /// press rule, the first stamp, an exact first build).
    fn qs_press(&mut self, at: (f32, f32), mode: SelectionMode) -> Option<Drag> {
        let mut drag = begin_drag(
            Tool::QuickSelect,
            PointerButton::Primary,
            at,
            &self.view,
            None,
        );
        assert!(
            quick_select::begin(
                &mut drag,
                &self.layers,
                Some(self.layer),
                &mut self.selection,
                mode
            )
            .is_ok()
        );
        quick_select::stamp(&mut drag, &mut self.store, at, &self.settings);
        quick_select::refresh(&mut drag, &mut self.selection, (400, 300), true);
        drag
    }

    /// A pointer move: the stamps `continue_drag` hands back, then the
    /// batch's rebuild (not forced).
    fn qs_move(&mut self, drag: &mut Option<Drag>, to: (f32, f32)) -> usize {
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        let dabs = match drag.as_mut() {
            Some(d) => continue_drag(
                d,
                to,
                &mut self.view,
                &mut self.selection,
                &self.settings,
                bounds,
            ),
            None => Vec::new(),
        };
        for &dab in &dabs {
            quick_select::stamp(drag, &mut self.store, dab, &self.settings);
        }
        quick_select::refresh(drag, &mut self.selection, (400, 300), false);
        dabs.len()
    }

    /// The release: an exact rebuild, then the step. Whether one was recorded.
    fn qs_release(&mut self, mut drag: Option<Drag>) -> bool {
        quick_select::release(&mut drag, &mut self.selection, (400, 300));
        self.end_drag(drag)
    }

    /// A whole stroke along `points`.
    fn qs_stroke(&mut self, points: &[(f32, f32)], mode: SelectionMode) -> bool {
        let Some((&first, rest)) = points.split_first() else {
            unreachable!("a stroke has points");
        };
        let mut drag = self.qs_press(first, mode);
        for &p in rest {
            self.qs_move(&mut drag, p);
        }
        self.qs_release(drag)
    }
}

/// A red block (100..300 x 80..200) beside a blue one (300..380 x 80..200)
/// on a 400 x 300 transparent layer.
fn two_blocks() -> Rig {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.rect_fill(100, 80, 300, 200, RED_PX);
    rig.rect_fill(300, 80, 380, 200, BLUE_PX);
    rig.settings.set_radius(Tool::Brush, 10.0);
    rig
}

#[test]
fn a_stroke_grows_a_soft_mask_over_the_colour_it_paints_in_one_labelled_step() {
    let mut rig = two_blocks();
    assert!(rig.qs_stroke(&[(150.0, 140.0), (250.0, 140.0)], SelectionMode::Replace));
    let s = mask_of_selection(&rig);
    assert_eq!(rig.labels(), ["Quick Select"]);
    // The stroke's stamps grow 30 texels past the footprint: inside the red
    // block, never past its edges.
    assert_eq!(s.coverage_at(200, 140), 1.0);
    assert_eq!(s.coverage_at(130, 140), 1.0);
    assert_eq!(s.coverage_at(95, 140), 0.0, "left of the block");
    assert_eq!(s.coverage_at(150, 85), 0.0, "past the window's reach");
    assert_eq!(s.coverage_at(50, 140), 0.0);
    // The mask is softened one texel at its edge, as the Magic Wand's.
    let partial = (0..400).any(|x| {
        let c = s.coverage_at(x, 140);
        c > 0.0 && c < 1.0
    });
    assert!(partial, "a softened edge");
    // Undo and redo restore exactly.
    let after = rig.selection.active();
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), after);
}

#[test]
fn growth_stops_at_a_colour_edge_and_never_takes_the_other_block() {
    let mut rig = two_blocks();
    rig.settings.set_radius(Tool::Brush, 12.0);
    // A stroke right up to the red/blue edge.
    assert!(rig.qs_stroke(&[(240.0, 140.0), (290.0, 140.0)], SelectionMode::Replace));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(299, 140), 1.0, "up to the edge");
    for x in 303..380 {
        assert_eq!(s.coverage_at(x, 140), 0.0, "blue texel {x}");
    }
}

#[test]
fn the_shared_tolerance_decides_what_is_close_enough() {
    let mut rig = two_blocks();
    // A slightly different red on the right of the block.
    rig.rect_fill(200, 80, 300, 200, [0.9, 0.0, 0.0, 1.0]);
    rig.settings.set_tolerance(0.0);
    assert!(rig.qs_stroke(&[(150.0, 140.0), (160.0, 140.0)], SelectionMode::Replace));
    assert_eq!(ok_selection(&rig).coverage_at(220, 140), 0.0);
    let mut rig = two_blocks();
    rig.rect_fill(200, 80, 300, 200, [0.9, 0.0, 0.0, 1.0]);
    rig.settings.set_tolerance(40.0);
    assert!(rig.qs_stroke(&[(185.0, 140.0), (195.0, 140.0)], SelectionMode::Replace));
    assert_eq!(ok_selection(&rig).coverage_at(215, 140), 1.0);
}

#[test]
fn a_click_selects_the_footprints_surroundings_in_one_step() {
    let mut rig = two_blocks();
    assert!(rig.qs_stroke(&[(200.0, 140.0)], SelectionMode::Replace));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(200, 140), 1.0);
    assert_eq!(s.coverage_at(222, 140), 1.0, "within the reach");
    assert_eq!(s.coverage_at(260, 140), 0.0, "beyond it");
    assert_eq!(rig.labels(), ["Quick Select"]);
}

#[test]
fn it_works_in_document_space_for_a_layer_with_an_origin() {
    let mut rig = Rig::new(rect(50, 40, 200, 150));
    rig.rect_fill(10, 10, 150, 100, RED_PX);
    rig.settings.set_radius(Tool::Brush, 8.0);
    assert!(rig.qs_stroke(&[(100.0, 90.0), (130.0, 90.0)], SelectionMode::Replace));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(110, 90), 1.0);
    assert_eq!(s.coverage_at(55, 90), 0.0, "the layer's own left edge area");
    assert!(
        s.bounds.x >= 59 && s.bounds.right() <= 201,
        "{:?}",
        s.bounds
    );
}

#[test]
fn the_selection_is_clipped_to_the_document() {
    // A layer reaching past the 400 x 300 canvas.
    let mut rig = Rig::new(rect(300, 200, 400, 300));
    rig.rect_fill(0, 0, 400, 300, RED_PX);
    rig.settings.set_radius(Tool::Brush, 20.0);
    assert!(rig.qs_stroke(&[(380.0, 280.0)], SelectionMode::Replace));
    let s = ok_selection(&rig);
    // Clipped like the Magic Wand's: the region ends at the document, only
    // the one-texel softening rim reaches past it.
    assert_eq!(s.coverage_at(399, 299), 1.0);
    assert!(
        s.bounds.right() <= 401 && s.bounds.bottom() <= 301,
        "{:?}",
        s.bounds
    );
    assert_eq!(s.coverage_at(402, 290), 0.0);
}

#[test]
fn add_unions_with_the_selection_and_one_step_undoes_both() {
    let mut rig = two_blocks();
    assert!(rig.qs_stroke(&[(150.0, 140.0)], SelectionMode::Replace));
    let first = ok_selection(&rig);
    assert!(rig.qs_stroke(&[(250.0, 140.0)], SelectionMode::Add));
    let both = ok_selection(&rig);
    assert_eq!(both.coverage_at(150, 140), 1.0, "the first still there");
    assert_eq!(both.coverage_at(250, 140), 1.0);
    assert!(both.bounds.width > first.bounds.width);
    assert_eq!(rig.labels(), ["Quick Select", "Add to Selection"]);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), Some(first));
}

#[test]
fn subtract_removes_the_stroked_region_from_the_selection() {
    let mut rig = two_blocks();
    // Everything red, by the Wand.
    assert_eq!(rig.wand((150.0, 140.0)), Ok(true));
    let all_red = ok_selection(&rig);
    assert_eq!(all_red.coverage_at(150, 140), 1.0);
    assert!(rig.qs_stroke(&[(200.0, 140.0)], SelectionMode::Subtract));
    let rest = ok_selection(&rig);
    assert_eq!(rest.coverage_at(200, 140), 0.0, "the stroked spot is out");
    assert_eq!(rest.coverage_at(110, 190), 1.0, "far from it stays in");
    assert_eq!(rig.labels(), ["Magic Wand", "Subtract from Selection"]);
}

#[test]
fn intersect_keeps_only_the_overlap() {
    let mut rig = two_blocks();
    rig.marquee_step(Tool::MarqueeSelect, (0.0, 0.0), (200.0, 300.0));
    assert!(rig.qs_stroke(&[(150.0, 140.0), (250.0, 140.0)], SelectionMode::Intersect));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(150, 140), 1.0);
    assert_eq!(s.coverage_at(250, 140), 0.0, "outside the marquee");
}

#[test]
fn new_mode_replaces_a_previous_selection() {
    let mut rig = two_blocks();
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    assert!(rig.qs_stroke(&[(200.0, 140.0)], SelectionMode::Replace));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(30, 30), 0.0);
    assert_eq!(s.coverage_at(200, 140), 1.0);
}

#[test]
fn a_press_deselects_at_once_in_new_mode_and_escape_restores() {
    let mut rig = two_blocks();
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    let before = rig.selection.active();
    let mut drag = rig.qs_press((200.0, 140.0), SelectionMode::Replace);
    assert_ne!(rig.selection.active(), before, "the press replaced it");
    rig.qs_move(&mut drag, (230.0, 140.0));
    // Escape: the selection returns to what it was and nothing is recorded.
    assert!(lasso_tools::cancel_selection_gesture(
        &mut drag,
        &mut rig.selection
    ));
    assert_eq!(rig.selection.active(), before);
    assert!(drag.is_none());
    assert_eq!(rig.labels(), ["Rectangular Marquee"]);
}

#[test]
fn a_combining_press_keeps_the_selection_visible_and_records_nothing_for_a_click_off_the_layer() {
    let mut rig = two_blocks();
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    let before = rig.selection.active();
    // A click outside the layer grows nothing: no footprint at all.
    let recorded = rig.qs_stroke(&[(-30.0, -30.0)], SelectionMode::Add);
    assert!(!recorded);
    assert_eq!(rig.selection.active(), before);
    assert_eq!(rig.labels(), ["Rectangular Marquee"]);
}

#[test]
fn a_new_mode_click_off_the_layer_deselects_as_the_marquee_does() {
    let mut rig = two_blocks();
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    assert!(rig.qs_stroke(&[(-30.0, -30.0)], SelectionMode::Replace));
    assert_eq!(rig.selection.active(), None);
    assert_eq!(rig.labels(), ["Rectangular Marquee", "Deselect"]);
}

#[test]
fn a_press_with_no_pixel_layer_is_refused_and_starts_no_drag() {
    let mut rig = two_blocks();
    let group = ok(rig.layers.add_group("Group", None));
    assert_eq!(
        quick_select::press_refusal(&rig.layers, Some(group)),
        Some(crate::LockRefusal::QuickSelectNeedsPixelLayer)
    );
    assert_eq!(
        quick_select::press_refusal(&rig.layers, None),
        Some(crate::LockRefusal::QuickSelectNeedsPixelLayer)
    );
    assert_eq!(
        quick_select::press_refusal(&rig.layers, Some(rig.layer)),
        None
    );
    let mut drag = begin_drag(
        Tool::QuickSelect,
        PointerButton::Primary,
        (1.0, 1.0),
        &rig.view,
        None,
    );
    assert!(
        quick_select::begin(
            &mut drag,
            &rig.layers,
            Some(group),
            &mut rig.selection,
            SelectionMode::Replace
        )
        .is_err()
    );
    assert!(drag.is_none());
}

#[test]
fn it_never_writes_a_pixel_or_makes_a_tile() {
    let mut rig = two_blocks();
    let tiles = rig.tiles();
    let before = rig.at(150, 140);
    assert!(rig.qs_stroke(&[(150.0, 140.0), (250.0, 140.0)], SelectionMode::Replace));
    assert_eq!(rig.tiles(), tiles);
    assert_eq!(rig.at(150, 140), before);
    // Only a selection step was recorded: no pixel history.
    assert_eq!(rig.labels(), ["Quick Select"]);
}

#[test]
fn a_backed_off_rebuild_leaves_the_selection_and_the_release_places_it_exactly() {
    let run = |defer: bool| {
        let mut rig = two_blocks();
        let mut drag = rig.qs_press((150.0, 140.0), SelectionMode::Replace);
        let shown = rig.selection.active();
        if defer {
            let far = std::time::Instant::now() + std::time::Duration::from_hours(1);
            if let Some(Drag::QuickSelect(q)) = drag.as_mut() {
                q.defer_rebuilds_until(far);
            }
        }
        rig.qs_move(&mut drag, (250.0, 140.0));
        let during = rig.selection.active();
        if defer {
            assert_eq!(during, shown, "the skipped rebuild changed nothing");
        } else {
            assert_ne!(during, shown);
        }
        assert!(rig.qs_release(drag));
        (rig.selection.active(), rig.labels())
    };
    let (exact, labels) = run(true);
    let (plain, plain_labels) = run(false);
    assert_eq!(
        exact, plain,
        "the release is exact whatever the back-off did"
    );
    assert_eq!(labels, plain_labels);
}

#[test]
fn a_stroke_ended_by_a_tool_switch_is_recorded() {
    let mut rig = two_blocks();
    let mut drag = rig.qs_press((150.0, 140.0), SelectionMode::Replace);
    rig.qs_move(&mut drag, (200.0, 140.0));
    let ended = lasso_tools::take_on_tool_switch(&mut drag, Tool::QuickSelect, Tool::Brush);
    assert!(ended.is_some());
    assert!(rig.end_drag(ended));
    assert_eq!(rig.labels(), ["Quick Select"]);
    // The same tool does not end it.
    let mut drag = rig.qs_press((150.0, 140.0), SelectionMode::Add);
    assert!(
        lasso_tools::take_on_tool_switch(&mut drag, Tool::QuickSelect, Tool::QuickSelect).is_none()
    );
}

#[test]
fn the_stroke_walks_the_brush_spacing_at_the_brush_radius() {
    let mut rig = two_blocks();
    rig.settings.set_radius(Tool::Brush, 24.0);
    let mut drag = rig.qs_press((110.0, 140.0), SelectionMode::Replace);
    // Quarter-radius spacing: 6 px, so 60 px is ten stamps.
    assert_eq!(rig.qs_move(&mut drag, (170.0, 140.0)), 10);
}

#[test]
fn the_one_line_that_routes_a_stamp_reads_the_brush_radius_and_tolerance() {
    // `App::quick_select_stamp` needs a window: check its production helper
    // is the one that reads both settings.
    let source = include_str!("quick_select.rs");
    assert!(source.contains("radius: settings.dab_radius(crate::DabTool::Brush)"));
    assert!(source.contains("tolerance: settings.tolerance"));
    let lib = include_str!("lib.rs");
    assert!(lib.contains("quick_select::stamp(\n            &mut self.drag,"));
}

#[test]
fn the_tool_has_its_chord_command_and_refusals() {
    assert_eq!(
        crate::tool_command_id(Tool::QuickSelect),
        "tool.quick_select"
    );
    assert!(crate::quick_mask::tool_refusal(Tool::QuickSelect).is_some());
    // Its radius is the Brush's.
    let mut settings = ToolSettings::default();
    assert_eq!(settings.radius(Tool::QuickSelect), Some(24.0));
    assert!(settings.set_radius(Tool::QuickSelect, 40.0));
    assert_eq!(settings.radius(Tool::Brush), Some(40.0));
    // Shift+W selects it; bare W is still the Magic Wand.
    let registry = crate::default_shortcuts_for(false);
    let chord = |s: &str| {
        aurora_widgets::shortcut::KeyChord::parse(s)
            .unwrap_or_else(|err| unreachable!("a chord {s}: {err:?}"))
    };
    assert_eq!(
        registry.resolve(chord("Shift+W")),
        Some(&crate::AppCommand::SelectTool(Tool::QuickSelect))
    );
    assert_eq!(
        registry.resolve(chord("w")),
        Some(&crate::AppCommand::SelectTool(Tool::MagicWand))
    );
    // The palette lists it.
    assert!(
        crate::palette_commands()
            .iter()
            .any(|e| e.id == "tool.quick_select" && e.title == "Select Quick Select Tool")
    );
}
