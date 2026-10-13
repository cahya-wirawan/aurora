//! The Smudge (0.222.0) at the app level: the wiring of the tool (shortcut,
//! palette, label, locks, Quick Mask, the options bar), the carried paint's
//! lifetime and the drag glue. The pixel behaviour through the production
//! stamp helper is in `selection_paint_tests` (it shares that file's rig).

use crate::smudge_stamp;
use crate::{AppCommand, DabTool, Drag, LockRefusal};
use aurora_ui::Tool;

#[test]
fn the_tool_is_wired_into_the_shortcuts_the_palette_and_the_tools_panel() {
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
    let resolved = crate::default_shortcuts()
        .resolve(KeyChord::new(Modifiers::none(), Key::Character('r')))
        .copied();
    assert_eq!(resolved, Some(AppCommand::SelectTool(Tool::Smudge)));
    assert_eq!(Tool::Smudge.shortcut_text(), "R");
    assert_eq!(crate::tool_command_id(Tool::Smudge), "tool.smudge");
    let commands = crate::palette_commands();
    assert!(
        commands
            .iter()
            .any(|c| c.id == "tool.smudge" && c.title == "Select Smudge Tool"),
        "the palette lists Select Smudge Tool"
    );
    assert_eq!(
        aurora_ui::tool_icon(Tool::Smudge),
        aurora_widgets::icons::IconKind::Smudge
    );
    assert_eq!(
        aurora_ui::tool_description(Tool::Smudge),
        "Smudge tool, shortcut R"
    );
    assert_eq!(crate::SMUDGE_STROKE_LABEL, "Smudge Stroke");
    assert_eq!(
        LockRefusal::SmudgeNeedsPixelLayer.message(),
        "Select a pixel layer to smudge. A group, an adjustment layer or no layer can't be smudged."
    );
}

#[test]
fn a_smudge_press_begins_a_brush_drag_with_an_empty_carry_each_time() {
    let view = aurora_ui::CanvasView::new();
    let drag = crate::begin_drag(
        Tool::Smudge,
        crate::PointerButton::Primary,
        (5.0, 5.0),
        &view,
        None,
    );
    assert!(smudge_stamp::is_smudge(drag.as_ref()));
    assert!(
        smudge_stamp::drag_carry(drag.as_ref()).is_some_and(aurora_brush::SmudgeCarry::is_empty)
    );
    assert!(matches!(
        drag,
        Some(Drag::Brush {
            hard: false,
            clone: None,
            ..
        })
    ));
    assert_eq!(crate::dab_tool_of_drag(drag.as_ref()), DabTool::Brush);
    // A Smudge drag is not a Clone Stamp drag, and vice versa.
    assert_eq!(crate::clone_stamp::drag_offset(drag.as_ref()), None);
    // Every other tool's drag carries nothing and is not a smudge.
    for tool in [Tool::Brush, Tool::Pencil, Tool::CloneStamp, Tool::Eraser] {
        let other = crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        assert!(!smudge_stamp::is_smudge(other.as_ref()), "{tool:?}");
        assert!(
            smudge_stamp::drag_carry(other.as_ref()).is_none(),
            "{tool:?}"
        );
    }
    assert!(!smudge_stamp::is_smudge(None));
    assert!(!smudge_stamp::is_smudge(Some(&Drag::Eyedropper)));
    // Only the primary button smudges.
    assert!(
        crate::begin_drag(
            Tool::Smudge,
            crate::PointerButton::Secondary,
            (5.0, 5.0),
            &view,
            None
        )
        .is_none()
    );
}

#[test]
fn locks_treat_the_smudge_like_the_brush() {
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    assert_eq!(
        crate::press_refusal(Tool::Smudge, pixels),
        Some(LockRefusal::PixelsLocked)
    );
    assert_eq!(
        crate::press_refusal(Tool::Smudge, transparency),
        None,
        "a transparency lock is honoured by keeping alpha, not by refusing"
    );
    assert_eq!(
        crate::press_refusal(Tool::Smudge, aurora_doc::LayerLock::default()),
        None
    );
    // The no-layer notice stays on screen only for the Smudge.
    assert!(crate::lock_refusal_applies(
        LockRefusal::SmudgeNeedsPixelLayer,
        Tool::Smudge,
        aurora_doc::LayerLock::default()
    ));
    for tool in [Tool::Brush, Tool::CloneStamp, Tool::Eraser] {
        assert!(!crate::lock_refusal_applies(
            LockRefusal::SmudgeNeedsPixelLayer,
            tool,
            aurora_doc::LayerLock::default()
        ));
    }
}

#[test]
fn quick_mask_refuses_the_smudge_with_the_existing_notice() {
    assert_eq!(
        crate::quick_mask::tool_refusal(Tool::Smudge),
        Some(LockRefusal::QuickMaskToolUnavailable)
    );
    assert!(crate::lock_refusal_applies(
        LockRefusal::QuickMaskToolUnavailable,
        Tool::Smudge,
        aurora_doc::LayerLock::default()
    ));
}

#[test]
fn the_strength_setting_mirrors_into_the_options_bar_view_and_the_dab() {
    let settings = crate::ToolSettings::default();
    assert_eq!(settings.fill_view().strength, 50);
    let mut s = crate::ToolSettings::default();
    assert!(s.set_smudge_strength(25.0));
    assert_eq!(s.fill_view().strength, 25);
    // The dab reads the setting.
    assert!((smudge_stamp::strength_fraction(&s) - 0.25).abs() < f32::EPSILON);
}

#[test]
fn the_app_wires_the_press_the_dab_and_the_stamp_to_the_smudge() {
    // `App` needs a window; these single-line checks pin the glue the free
    // helpers cannot: the press asks `smudge_stamp::press`, the dab site
    // dispatches on the drag and stamps through the selection, and the
    // commit labels the step.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert!(production.contains("let Some(why) = smudge_stamp::press("));
    assert!(production.contains("if smudge_stamp::is_smudge(self.drag.as_ref()) {"));
    assert!(production.contains("smudge_stamp::smudge_dab_through_selection("));
    assert!(production.contains("} else if smudge.is_some() {"));
}
