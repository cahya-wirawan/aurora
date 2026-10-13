//! The Dodge, Burn and Sponge (0.224.0) at the app level: the wiring of the
//! three tools (shortcuts, palette, labels, locks, Quick Mask) and the drag
//! glue. The pixel behaviour through the production stamp helper is in
//! `selection_paint_tests` (it shares that file's rig); the options-bar
//! controls are in `fill_tests`.

use crate::tone_stamp::{self, ToneKind};
use crate::{AppCommand, DabTool, Drag, LockRefusal};
use aurora_ui::Tool;
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::none()
    }
}

#[test]
fn the_tools_are_wired_into_the_shortcuts_the_palette_and_the_tools_panel() {
    for primary_is_meta in [false, true] {
        let table = crate::default_shortcuts_for(primary_is_meta);
        let chord = |m, c| KeyChord::new(m, Key::Character(c));
        assert_eq!(
            table.resolve(chord(Modifiers::none(), 'o')),
            Some(&AppCommand::SelectTool(Tool::Dodge))
        );
        assert_eq!(
            table.resolve(chord(shift(), 'o')),
            Some(&AppCommand::SelectTool(Tool::Burn))
        );
        assert_eq!(
            table.resolve(chord(shift(), 'p')),
            Some(&AppCommand::SelectTool(Tool::Sponge))
        );
        // The bare P stays free, and the neighbours keep their tools.
        assert_eq!(table.resolve(chord(Modifiers::none(), 'p')), None);
        assert_eq!(
            table.resolve(chord(shift(), 'r')),
            Some(&AppCommand::SelectTool(Tool::Blur))
        );
        assert_eq!(
            table.resolve(chord(shift(), 's')),
            Some(&AppCommand::SelectTool(Tool::Sharpen))
        );
    }
    // Every tool has its own chord in the table (no collision).
    let table = crate::default_shortcuts_for(false);
    for tool in Tool::ALL {
        let m = if tool.shortcut_needs_shift() {
            shift()
        } else {
            Modifiers::none()
        };
        let c = tool.shortcut_key().to_ascii_lowercase();
        assert_eq!(
            table.resolve(KeyChord::new(m, Key::Character(c))),
            Some(&AppCommand::SelectTool(tool)),
            "{tool:?}"
        );
    }
    assert_eq!(Tool::Dodge.shortcut_text(), "O");
    assert_eq!(Tool::Burn.shortcut_text(), "Shift+O");
    assert_eq!(Tool::Sponge.shortcut_text(), "Shift+P");
    assert_eq!(crate::tool_command_id(Tool::Dodge), "tool.dodge");
    assert_eq!(crate::tool_command_id(Tool::Burn), "tool.burn");
    assert_eq!(crate::tool_command_id(Tool::Sponge), "tool.sponge");
    let commands = crate::palette_commands();
    for (id, title) in [
        ("tool.dodge", "Select Dodge Tool"),
        ("tool.burn", "Select Burn Tool"),
        ("tool.sponge", "Select Sponge Tool"),
    ] {
        assert!(
            commands.iter().any(|c| c.id == id && c.title == title),
            "the palette lists {title}"
        );
    }
    for (tool, icon) in [
        (Tool::Dodge, aurora_widgets::icons::IconKind::Dodge),
        (Tool::Burn, aurora_widgets::icons::IconKind::Burn),
        (Tool::Sponge, aurora_widgets::icons::IconKind::Sponge),
    ] {
        assert_eq!(aurora_ui::tool_icon(tool), icon);
    }
    assert_eq!(
        aurora_ui::tool_description(Tool::Burn),
        "Burn tool, shortcut Shift+O"
    );
    assert_eq!(crate::DODGE_STROKE_LABEL, "Dodge Stroke");
    assert_eq!(crate::BURN_STROKE_LABEL, "Burn Stroke");
    assert_eq!(crate::SPONGE_STROKE_LABEL, "Sponge Stroke");
    assert_eq!(tone_stamp::stroke_label(ToneKind::Dodge), "Dodge Stroke");
    assert_eq!(tone_stamp::stroke_label(ToneKind::Burn), "Burn Stroke");
    assert_eq!(tone_stamp::stroke_label(ToneKind::Sponge), "Sponge Stroke");
    assert_eq!(
        LockRefusal::DodgeNeedsPixelLayer.message(),
        "Select a pixel layer to dodge. A group, an adjustment layer or no layer can't be dodged."
    );
    assert_eq!(
        LockRefusal::BurnNeedsPixelLayer.message(),
        "Select a pixel layer to burn. A group, an adjustment layer or no layer can't be burned."
    );
    assert_eq!(
        LockRefusal::SpongeNeedsPixelLayer.message(),
        "Select a pixel layer to sponge. A group, an adjustment layer or no layer can't be sponged."
    );
}

#[test]
fn a_toning_press_begins_a_brush_drag_carrying_its_kind() {
    let view = aurora_ui::CanvasView::new();
    for (tool, kind) in [
        (Tool::Dodge, ToneKind::Dodge),
        (Tool::Burn, ToneKind::Burn),
        (Tool::Sponge, ToneKind::Sponge),
    ] {
        let drag = crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        assert_eq!(tone_stamp::drag_tone(drag.as_ref()), Some(kind));
        assert_eq!(tone_stamp::kind_of(tool), Some(kind));
        assert!(matches!(
            drag,
            Some(Drag::Brush {
                hard: false,
                clone: None,
                smudge: None,
                filter: None,
                ..
            })
        ));
        assert_eq!(crate::dab_tool_of_drag(drag.as_ref()), DabTool::Brush);
        assert!(
            crate::begin_drag(
                tool,
                crate::PointerButton::Secondary,
                (5.0, 5.0),
                &view,
                None
            )
            .is_none()
        );
    }
    for tool in [
        Tool::Brush,
        Tool::Pencil,
        Tool::CloneStamp,
        Tool::Smudge,
        Tool::Blur,
        Tool::Sharpen,
        Tool::Eraser,
    ] {
        let other = crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        assert_eq!(tone_stamp::drag_tone(other.as_ref()), None, "{tool:?}");
        assert_eq!(tone_stamp::kind_of(tool), None);
    }
    assert_eq!(tone_stamp::drag_tone(None), None);
    assert_eq!(tone_stamp::drag_tone(Some(&Drag::Eyedropper)), None);
}

#[test]
fn locks_treat_the_toning_tools_like_the_brush() {
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    for tool in [Tool::Dodge, Tool::Burn, Tool::Sponge] {
        assert_eq!(
            crate::press_refusal(tool, pixels),
            Some(LockRefusal::PixelsLocked)
        );
        assert_eq!(
            crate::press_refusal(tool, aurora_doc::LayerLock::default()),
            None
        );
    }
    for (refusal, own) in [
        (LockRefusal::DodgeNeedsPixelLayer, Tool::Dodge),
        (LockRefusal::BurnNeedsPixelLayer, Tool::Burn),
        (LockRefusal::SpongeNeedsPixelLayer, Tool::Sponge),
    ] {
        for tool in Tool::ALL {
            assert_eq!(
                crate::lock_refusal_applies(refusal, tool, aurora_doc::LayerLock::default()),
                tool == own,
                "{refusal:?} {tool:?}"
            );
        }
    }
}

#[test]
fn quick_mask_refuses_the_toning_tools_with_the_existing_notice() {
    for tool in [Tool::Dodge, Tool::Burn, Tool::Sponge] {
        assert_eq!(
            crate::quick_mask::tool_refusal(tool),
            Some(LockRefusal::QuickMaskToolUnavailable)
        );
        assert!(crate::lock_refusal_applies(
            LockRefusal::QuickMaskToolUnavailable,
            tool,
            aurora_doc::LayerLock::default()
        ));
    }
}

#[test]
fn the_toning_settings_mirror_into_the_options_bar_view() {
    let mut s = crate::ToolSettings::default();
    let view = s.fill_view();
    assert_eq!(
        (view.dodge_exposure, view.burn_exposure, view.sponge_flow),
        (50, 50, 50)
    );
    assert!(s.set_strength(Tool::Dodge, 12.0));
    assert!(s.set_strength(Tool::Burn, 88.0));
    assert!(s.set_strength(Tool::Sponge, 7.0));
    let view = s.fill_view();
    assert_eq!(
        (view.dodge_exposure, view.burn_exposure, view.sponge_flow),
        (12, 88, 7)
    );
    assert_eq!((view.strength, view.blur_strength), (50, 50));
    assert!((tone_stamp::amount_fraction(&s, ToneKind::Dodge) - 0.12).abs() < 1e-6);
    assert!((tone_stamp::amount_fraction(&s, ToneKind::Burn) - 0.88).abs() < 1e-6);
    assert!((tone_stamp::amount_fraction(&s, ToneKind::Sponge) - 0.07).abs() < 1e-6);
}

#[test]
fn the_app_wires_the_press_the_dab_and_the_stamp_to_the_toning_tools() {
    // `App` needs a window; these single-line checks pin the glue the free
    // helpers cannot: the press asks `tone_stamp::press`, the dab site
    // dispatches on the drag and stamps through the selection, and the
    // commit labels the step.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert!(production.contains("&& let Some(why) = tone_stamp::press("));
    assert!(production.contains("if tone_stamp::drag_tone(self.drag.as_ref()).is_some() {"));
    assert!(production.contains("tone_stamp::tone_dab_through_selection("));
    assert!(production.contains("tone_stamp::stroke_label(kind)"));
}
