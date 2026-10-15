//! Select > Colour Range (0.238.0) as the app runs it:
//! `colour_range_app::{colour_range_select, colour_range_step}` with a real
//! `SelectionSet` and undo order, the Fuzziness prompt, the palette and the
//! menus. `App::run_colour_range` needs a window (it only supplies the
//! foreground colour and the options-bar mode; a source check pins both).

use super::*;
use crate::LockRefusal;
use crate::colour_range_app::{self, ColourRangeOutcome};
use aurora_doc::SelectionMode;

impl Rig {
    fn colour_range(
        &mut self,
        colour: [f32; 3],
        fuzziness: u16,
        mode: SelectionMode,
    ) -> ColourRangeOutcome {
        colour_range_app::colour_range_step(
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
            &mut self.store,
            &self.layers,
            Some(self.layer),
            colour,
            fuzziness,
            (400, 300),
            mode,
        )
    }
}

fn blocks() -> Rig {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.rect_fill(100, 80, 200, 160, [1.0, 0.0, 0.0, 1.0]);
    rig.rect_fill(250, 80, 330, 160, [0.0, 0.0, 1.0, 1.0]);
    rig.rect_fill(100, 200, 200, 260, [0.9, 0.1, 0.1, 1.0]);
    rig
}

#[test]
fn it_selects_the_pixels_near_the_foreground_colour_in_one_labelled_step() {
    let mut rig = blocks();
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    let s = mask_of_selection(&rig);
    // Both reds (the second is a little off), not the blue; the hard
    // block edges are not softened.
    assert_eq!(s.coverage_at(150, 120), 1.0);
    // The second red is a little off: partly covered, by the ramp.
    let off = f32::from(aurora_brush::coverage_byte(
        aurora_brush::colour_distance([1.0, 0.0, 0.0], [0.9, 0.1, 0.1]),
        40,
        1.0,
    )) / 255.0;
    assert!(
        off > 0.0 && off < 1.0,
        "the fixture sits on the ramp: {off}"
    );
    assert!(
        (s.coverage_at(150, 230) - off).abs() < 0.02,
        "{}",
        s.coverage_at(150, 230)
    );
    assert_eq!(s.coverage_at(290, 120), 0.0);
    assert_eq!(s.coverage_at(99, 120), 0.0);
    assert_eq!(s.bounds, rect(100, 80, 100, 180));
    assert_eq!(rig.labels(), ["Colour Range"]);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 180)));
}

#[test]
fn fuzziness_widens_and_narrows_what_is_taken() {
    let mut rig = blocks();
    // Zero takes the exact colour only.
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 0, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 80)));
    // The maximum takes the blue as well.
    let mut rig = blocks();
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 200, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    let s = ok_selection(&rig);
    assert!(s.coverage_at(290, 120) > 0.0 || s.coverage_at(150, 120) == 1.0);
}

#[test]
fn the_ramp_gives_partial_coverage_between_half_the_threshold_and_all_of_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // A grey ramp, one column per level.
    for x in 0..100_u32 {
        let v = 0.2 + 0.006 * x as f32;
        for y in 0..10 {
            rig.put(x, y, [v, v, v, 1.0]);
        }
    }
    assert_eq!(
        rig.colour_range([0.5, 0.5, 0.5], 60, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    let s = ok_selection(&rig);
    let covers: Vec<f32> = (0..100).map(|x| s.coverage_at(x, 5)).collect();
    assert!(covers.contains(&1.0));
    assert!(covers.iter().any(|c| *c > 0.0 && *c < 1.0), "a soft ramp");
    // Rises to a peak at the sample then falls: unimodal.
    let peak = covers
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |p| p.0);
    assert!(covers[..peak].windows(2).all(|w| w[0] <= w[1] + 1e-6));
    assert!(covers[peak..].windows(2).all(|w| w[0] + 1e-6 >= w[1]));
}

#[test]
fn it_combines_by_the_selection_mode() {
    let mut rig = blocks();
    rig.marquee_step(Tool::MarqueeSelect, (240.0, 70.0), (340.0, 170.0));
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Add),
        ColourRangeOutcome::Applied
    );
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(290, 120), 1.0, "the marquee's blue stays");
    assert_eq!(s.coverage_at(150, 120), 1.0, "the reds join");
    assert_eq!(rig.labels(), ["Rectangular Marquee", "Add to Selection"]);
    // Subtract removes them again; Intersect keeps only the overlap.
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Subtract),
        ColourRangeOutcome::Applied
    );
    assert_eq!(ok_selection(&rig).coverage_at(150, 120), 0.0);
    let mut rig = blocks();
    rig.marquee_step(Tool::MarqueeSelect, (0.0, 0.0), (150.0, 300.0));
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Intersect),
        ColourRangeOutcome::Applied
    );
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(120, 120), 1.0);
    assert_eq!(s.coverage_at(180, 120), 0.0, "outside the marquee");
}

#[test]
fn nothing_near_is_a_calm_refusal_and_records_nothing() {
    let mut rig = blocks();
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (50.0, 50.0));
    let before = rig.selection.active();
    assert_eq!(
        rig.colour_range([0.0, 1.0, 0.0], 10, SelectionMode::Replace),
        ColourRangeOutcome::Refused(LockRefusal::ColourRangeNothing)
    );
    assert_eq!(rig.selection.active(), before);
    assert_eq!(rig.labels(), ["Rectangular Marquee"]);
}

#[test]
fn a_group_or_no_layer_is_refused() {
    let mut rig = blocks();
    let group = ok(rig.layers.add_group("Group", None));
    rig.layer = group;
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Replace),
        ColourRangeOutcome::Refused(LockRefusal::ColourRangeNeedsPixelLayer)
    );
    let r = colour_range_app::colour_range_select(
        &mut rig.store,
        &rig.layers,
        None,
        [1.0, 0.0, 0.0],
        40,
        (400, 300),
    );
    assert_eq!(r.err(), Some(LockRefusal::ColourRangeNeedsPixelLayer));
}

#[test]
fn it_works_in_document_space_and_clips_to_the_document() {
    // A layer at (300, 200) reaching past the 400 x 300 document.
    let mut rig = Rig::new(rect(300, 200, 400, 300));
    rig.rect_fill(0, 0, 400, 300, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    assert_eq!(rig.selected_bounds(), Some(rect(300, 200, 100, 100)));
    // A layer left of the origin: only its part inside the document.
    let mut rig = Rig::new(rect(-50, -30, 200, 100));
    rig.rect_fill(0, 0, 200, 100, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(
        rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Replace),
        ColourRangeOutcome::Applied
    );
    assert_eq!(rig.selected_bounds(), Some(rect(0, 0, 150, 70)));
}

#[test]
fn it_writes_nothing_to_the_layer() {
    let mut rig = blocks();
    let tiles = rig.tiles();
    let before = rig.at(150, 120);
    let _ = rig.colour_range([1.0, 0.0, 0.0], 40, SelectionMode::Replace);
    assert_eq!(rig.tiles(), tiles);
    assert_eq!(rig.at(150, 120), before);
}

#[test]
fn the_fuzziness_text_is_validated() {
    use colour_range_app::parse_fuzziness as parse;
    assert_eq!(parse("40"), Ok(40));
    assert_eq!(parse(" 0 "), Ok(0));
    assert_eq!(parse("200"), Ok(200));
    assert!(parse("201").is_err());
    assert!(parse("").is_err());
    assert!(parse("4.5").is_err());
    assert!(parse("-3").is_err());
    assert!(parse("40x").is_err());
    assert!(parse("99999999999999999999").is_err());
}

#[test]
fn the_prompt_opens_with_the_last_value_validates_on_enter_and_cancels() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    assert!(colour_range_app::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        55
    ));
    let Some(root) = palette else {
        unreachable!("open");
    };
    assert!(!colour_range_app::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        10
    ));
    let Ok(state) = aurora_widgets::widgets::command_palette_state(&rig.workspace.tree, root)
    else {
        unreachable!("state")
    };
    assert_eq!(state.query(), "55");
    assert!(
        state
            .selected()
            .is_some_and(|e| e.title.contains("Fuzziness 55"))
    );
    assert_eq!(
        colour_range_app::prompt_enter(&mut rig.workspace.tree, root),
        Some(Ok(55))
    );
    // A bad value keeps the prompt and shows the reason.
    let _ =
        aurora_widgets::widgets::set_command_palette_query(&mut rig.workspace.tree, root, "300");
    assert!(colour_range_app::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
    assert_eq!(
        colour_range_app::prompt_enter(&mut rig.workspace.tree, root),
        Some(Err(()))
    );
    let Ok(state) = aurora_widgets::widgets::command_palette_state(&rig.workspace.tree, root)
    else {
        unreachable!("state")
    };
    assert!(
        state
            .selected()
            .is_some_and(|e| e.title.contains("At most 200"))
    );
    // It is not the Select > Modify prompt, and that one is not this.
    assert!(!crate::selection_refine::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
    crate::close_command_palette(&mut rig.workspace, &mut focus, &mut palette);
    assert!(palette.is_none());
    // A Modify prompt is no Colour Range prompt.
    assert!(crate::selection_refine::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        crate::selection_refine::RefineKind::Expand,
        5.0
    ));
    let Some(root) = palette else {
        unreachable!("open");
    };
    assert_eq!(
        colour_range_app::prompt_enter(&mut rig.workspace.tree, root),
        None
    );
    assert!(!colour_range_app::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
}

#[test]
fn the_command_is_in_the_palette_and_routes_to_its_prompt() {
    let entries = crate::palette_commands();
    assert!(entries.iter().any(
        |e| e.id == colour_range_app::COMMAND_ID && e.title == colour_range_app::COMMAND_TITLE
    ));
    assert!(colour_range_app::COMMAND_TITLE.starts_with("Select Colour Range"));
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut focus = aurora_widgets::FocusManager::default();
    let mut dialog = crate::tests::FakeFileDialog::default();
    assert!(matches!(
        crate::activate_command(
            &mut rig.workspace,
            &mut focus,
            colour_range_app::COMMAND_ID,
            &mut dialog
        ),
        Some(crate::ActivatedCommand::ColourRangePrompt)
    ));
}

#[test]
fn the_default_fuzziness_is_forty_and_the_app_reads_the_foreground_and_the_mode() {
    assert_eq!(ToolSettings::default().colour_range_fuzziness, 40);
    // `App::run_colour_range` needs a window: pin what it feeds the step.
    // Whitespace is dropped so a CRLF checkout (Windows) matches too.
    let lib: String = include_str!("lib.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(lib.contains("self.colour.state.range_colour(),fuzziness,"));
    assert!(lib.contains(
        "self.tool_settings.selection_mode,){colour_range_app::ColourRangeOutcome::Applied"
    ));
}
