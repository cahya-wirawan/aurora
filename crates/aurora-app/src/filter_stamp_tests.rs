//! The Blur and Sharpen (0.223.0) at the app level: the wiring of the two
//! tools (shortcuts, palette, labels, locks, Quick Mask, the options bar)
//! and the drag glue. The pixel behaviour through the production stamp
//! helper is in `selection_paint_tests` (it shares that file's rig).

use crate::filter_stamp;
use crate::{AppCommand, DabTool, Drag, LockRefusal};
use aurora_brush::FilterKind;
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
            table.resolve(chord(shift(), 'r')),
            Some(&AppCommand::SelectTool(Tool::Blur))
        );
        assert_eq!(
            table.resolve(chord(shift(), 's')),
            Some(&AppCommand::SelectTool(Tool::Sharpen))
        );
        // The bare letters keep their tools.
        assert_eq!(
            table.resolve(chord(Modifiers::none(), 'r')),
            Some(&AppCommand::SelectTool(Tool::Smudge))
        );
        assert_eq!(
            table.resolve(chord(Modifiers::none(), 's')),
            Some(&AppCommand::SelectTool(Tool::CloneStamp))
        );
    }
    assert_eq!(Tool::Blur.shortcut_text(), "Shift+R");
    assert_eq!(Tool::Sharpen.shortcut_text(), "Shift+S");
    assert_eq!(crate::tool_command_id(Tool::Blur), "tool.blur");
    assert_eq!(crate::tool_command_id(Tool::Sharpen), "tool.sharpen");
    let commands = crate::palette_commands();
    for (id, title) in [
        ("tool.blur", "Select Blur Tool"),
        ("tool.sharpen", "Select Sharpen Tool"),
    ] {
        assert!(
            commands.iter().any(|c| c.id == id && c.title == title),
            "the palette lists {title}"
        );
    }
    assert_eq!(
        aurora_ui::tool_icon(Tool::Blur),
        aurora_widgets::icons::IconKind::Blur
    );
    assert_eq!(
        aurora_ui::tool_icon(Tool::Sharpen),
        aurora_widgets::icons::IconKind::Sharpen
    );
    assert_eq!(
        aurora_ui::tool_description(Tool::Blur),
        "Blur tool, shortcut Shift+R"
    );
    assert_eq!(crate::BLUR_STROKE_LABEL, "Blur Stroke");
    assert_eq!(crate::SHARPEN_STROKE_LABEL, "Sharpen Stroke");
    assert_eq!(filter_stamp::stroke_label(FilterKind::Blur), "Blur Stroke");
    assert_eq!(
        filter_stamp::stroke_label(FilterKind::Sharpen),
        "Sharpen Stroke"
    );
    assert_eq!(
        LockRefusal::BlurNeedsPixelLayer.message(),
        "Select a pixel layer to blur. A group, an adjustment layer or no layer can't be blurred."
    );
    assert_eq!(
        LockRefusal::SharpenNeedsPixelLayer.message(),
        "Select a pixel layer to sharpen. A group, an adjustment layer or no layer can't be sharpened."
    );
}

#[test]
fn a_filter_press_begins_a_brush_drag_carrying_its_kind() {
    let view = aurora_ui::CanvasView::new();
    for (tool, kind) in [
        (Tool::Blur, FilterKind::Blur),
        (Tool::Sharpen, FilterKind::Sharpen),
    ] {
        let drag = crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        assert_eq!(filter_stamp::drag_filter(drag.as_ref()), Some(kind));
        assert_eq!(filter_stamp::kind_of(tool), Some(kind));
        assert!(matches!(
            drag,
            Some(Drag::Brush {
                hard: false,
                clone: None,
                smudge: None,
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
        Tool::Eraser,
    ] {
        let other = crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        assert_eq!(filter_stamp::drag_filter(other.as_ref()), None, "{tool:?}");
        assert_eq!(filter_stamp::kind_of(tool), None);
    }
    assert_eq!(filter_stamp::drag_filter(None), None);
    assert_eq!(filter_stamp::drag_filter(Some(&Drag::Eyedropper)), None);
}

#[test]
fn locks_treat_the_filters_like_the_brush() {
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    for tool in [Tool::Blur, Tool::Sharpen] {
        assert_eq!(
            crate::press_refusal(tool, pixels),
            Some(LockRefusal::PixelsLocked)
        );
        assert_eq!(crate::press_refusal(tool, transparency), None);
        assert_eq!(
            crate::press_refusal(tool, aurora_doc::LayerLock::default()),
            None
        );
    }
    // Each no-layer notice stays on screen only for its own tool.
    for (refusal, own) in [
        (LockRefusal::BlurNeedsPixelLayer, Tool::Blur),
        (LockRefusal::SharpenNeedsPixelLayer, Tool::Sharpen),
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
fn quick_mask_refuses_the_filters_with_the_existing_notice() {
    for tool in [Tool::Blur, Tool::Sharpen] {
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
fn the_strength_settings_mirror_into_the_options_bar_view() {
    let mut s = crate::ToolSettings::default();
    let view = s.fill_view();
    assert_eq!(
        (view.strength, view.blur_strength, view.sharpen_strength),
        (50, 50, 50)
    );
    assert!(s.set_strength(Tool::Blur, 12.0));
    assert!(s.set_strength(Tool::Sharpen, 88.0));
    let view = s.fill_view();
    assert_eq!(
        (view.strength, view.blur_strength, view.sharpen_strength),
        (50, 12, 88)
    );
    assert!((filter_stamp::strength_fraction(&s, FilterKind::Blur) - 0.12).abs() < 1e-6);
    assert!((filter_stamp::strength_fraction(&s, FilterKind::Sharpen) - 0.88).abs() < 1e-6);
}

#[test]
fn the_app_wires_the_press_the_dab_and_the_stamp_to_the_filters() {
    // `App` needs a window; these single-line checks pin the glue the free
    // helpers cannot: the press asks `filter_stamp::press`, the dab site
    // dispatches on the drag and stamps through the selection, and the
    // commit labels the step.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert!(production.contains("&& let Some(why) = filter_stamp::press("));
    assert!(production.contains("if filter_stamp::drag_filter(self.drag.as_ref()).is_some() {"));
    assert!(production.contains("filter_stamp::filter_dab_through_selection("));
    assert!(production.contains("filter_stamp::stroke_label(kind)"));
}
