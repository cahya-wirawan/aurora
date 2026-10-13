//! The Clone Stamp (0.221.0) at the app level: the press decision, the
//! aligned offset's lifetime, and the tool's wiring (shortcut, palette,
//! label, locks, Quick Mask). The pixel behaviour through the production
//! stamp helper is in `selection_paint_tests` (it shares that file's rig).

use crate::clone_stamp::{self, CloneStamp, Press};
use crate::{AppCommand, DabTool, Drag, LockRefusal};
use aurora_core::Rect;
use aurora_ui::Tool;

fn two_layers() -> (aurora_doc::LayerId, aurora_doc::LayerId) {
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    let a = layers.add_pixel_layer("A", bounds, None);
    let b = layers.add_pixel_layer("B", bounds, None);
    match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        other => unreachable!("{other:?}"),
    }
}

#[test]
fn the_offset_is_the_rounded_source_minus_press() {
    assert_eq!(
        clone_stamp::offset_between((100.0, 40.0), (20.0, 10.0)),
        (80, 30)
    );
    assert_eq!(
        clone_stamp::offset_between((10.0, 10.0), (20.0, 50.0)),
        (-10, -40),
        "the sign is source minus press"
    );
    assert_eq!(
        clone_stamp::offset_between((10.4, 10.6), (0.0, 0.0)),
        (10, 11)
    );
    assert_eq!(
        clone_stamp::offset_between((f32::NAN, 1.0), (0.0, f32::INFINITY)),
        (0, 0),
        "a non-finite axis gives no offset"
    );
}

#[test]
fn a_press_without_alt_needs_a_source_and_a_pixel_layer() {
    let (a, _) = two_layers();
    let mut state = CloneStamp::default();
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (5.0, 5.0)),
        Press::Refused(LockRefusal::CloneNeedsSource)
    );
    assert_eq!(
        LockRefusal::CloneNeedsSource.message(),
        "Alt-click to set a clone source."
    );
    assert_eq!(
        clone_stamp::press(&mut state, false, None, (5.0, 5.0)),
        Press::Refused(LockRefusal::CloneNeedsPixelLayer)
    );
    // Alt on a non-pixel layer cannot set a source either.
    assert_eq!(
        clone_stamp::press(&mut state, true, None, (5.0, 5.0)),
        Press::Refused(LockRefusal::CloneNeedsPixelLayer)
    );
    assert!(state.source().is_none());
}

#[test]
fn alt_click_sets_the_source_and_starts_no_stroke() {
    let (a, _) = two_layers();
    let mut state = CloneStamp::default();
    assert_eq!(
        clone_stamp::press(&mut state, true, Some(a), (30.0, 40.0)),
        Press::SourceSet
    );
    assert_eq!(state.source(), Some((a, (30.0, 40.0))));
    // The next plain press fixes the offset: source minus press.
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (10.0, 10.0)),
        Press::Stroke((20, 30))
    );
}

#[test]
fn the_aligned_offset_persists_across_strokes_until_a_new_source() {
    let (a, _) = two_layers();
    let mut state = CloneStamp::default();
    let _ = clone_stamp::press(&mut state, true, Some(a), (30.0, 40.0));
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (10.0, 10.0)),
        Press::Stroke((20, 30))
    );
    // A second stroke somewhere else keeps the first stroke's offset.
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (50.0, 3.0)),
        Press::Stroke((20, 30))
    );
    // A new Alt+click replaces the source and forgets the offset.
    let _ = clone_stamp::press(&mut state, true, Some(a), (0.0, 0.0));
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (50.0, 3.0)),
        Press::Stroke((-50, -3))
    );
}

#[test]
fn the_source_is_cleared_by_a_layer_change_and_by_a_document_switch() {
    let (a, b) = two_layers();
    let mut state = CloneStamp::default();
    let _ = clone_stamp::press(&mut state, true, Some(a), (30.0, 40.0));
    // The same layer keeps it.
    state.observe_layer(Some(a));
    assert!(state.source().is_some());
    // Another layer, or none, clears it for good: coming back to `a`
    // does not revive it.
    state.observe_layer(Some(b));
    assert!(state.source().is_none());
    state.observe_layer(Some(a));
    assert!(state.source().is_none());
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(a), (1.0, 1.0)),
        Press::Refused(LockRefusal::CloneNeedsSource)
    );
    // None (no active layer) clears too.
    let _ = clone_stamp::press(&mut state, true, Some(a), (3.0, 4.0));
    state.observe_layer(None);
    assert!(state.source().is_none());
    // A source set on one layer is not used on another even unobserved.
    let _ = clone_stamp::press(&mut state, true, Some(a), (3.0, 4.0));
    assert_eq!(
        clone_stamp::press(&mut state, false, Some(b), (1.0, 1.0)),
        Press::Refused(LockRefusal::CloneNeedsSource)
    );
    // `clear` is what a document switch, New, Open or close runs.
    let _ = clone_stamp::press(&mut state, true, Some(a), (3.0, 4.0));
    state.clear();
    assert!(state.source().is_none());
}

#[test]
fn commit_live_gestures_is_where_a_switch_clears_the_source() {
    // One-line source check: the document-switch path calls `clear`.
    let source = include_str!("document_tabs.rs");
    assert!(source.contains("cx.doc.clone_stamp.clear()"));
    let lib = include_str!("lib.rs");
    assert!(lib.contains("self.doc.clone_stamp.observe_layer(self.doc.active_layer)"));
}

#[test]
fn a_clone_stamp_press_begins_a_brush_drag_carrying_its_offset() {
    let view = aurora_ui::CanvasView::new();
    let mut drag = crate::begin_drag(
        Tool::CloneStamp,
        crate::PointerButton::Primary,
        (5.0, 5.0),
        &view,
        None,
    );
    assert_eq!(clone_stamp::drag_offset(drag.as_ref()), Some((0, 0)));
    clone_stamp::set_drag_offset(&mut drag, (7, -9));
    assert_eq!(clone_stamp::drag_offset(drag.as_ref()), Some((7, -9)));
    assert!(matches!(drag, Some(Drag::Brush { hard: false, .. })));
    assert_eq!(crate::dab_tool_of_drag(drag.as_ref()), DabTool::Brush);
    // The Brush and Pencil drags carry none, and offsets are never added
    // to them.
    for tool in [Tool::Brush, Tool::Pencil] {
        let mut other =
            crate::begin_drag(tool, crate::PointerButton::Primary, (5.0, 5.0), &view, None);
        clone_stamp::set_drag_offset(&mut other, (1, 1));
        assert_eq!(clone_stamp::drag_offset(other.as_ref()), None, "{tool:?}");
    }
    assert_eq!(clone_stamp::drag_offset(None), None);
    assert_eq!(clone_stamp::drag_offset(Some(&Drag::Eyedropper)), None);
    // Only the primary button draws.
    assert!(
        crate::begin_drag(
            Tool::CloneStamp,
            crate::PointerButton::Secondary,
            (5.0, 5.0),
            &view,
            None
        )
        .is_none()
    );
}

#[test]
fn the_tool_is_wired_into_the_shortcuts_the_palette_and_the_options_bar() {
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
    let resolved = crate::default_shortcuts()
        .resolve(KeyChord::new(Modifiers::none(), Key::Character('s')))
        .copied();
    assert_eq!(resolved, Some(AppCommand::SelectTool(Tool::CloneStamp)));
    assert_eq!(Tool::CloneStamp.shortcut_text(), "S");
    let commands = crate::palette_commands();
    assert!(
        commands
            .iter()
            .any(|c| c.id == "tool.clone_stamp" && c.title == "Select Clone Stamp Tool"),
        "the palette lists Select Clone Stamp Tool"
    );
    // It shares the Brush's radius slider.
    let mut settings = crate::ToolSettings::default();
    assert!(settings.radius(Tool::CloneStamp).is_some());
    assert!(settings.set_radius(Tool::CloneStamp, 31.0));
    assert_eq!(settings.radius(Tool::Brush), Some(31.0));
}

#[test]
fn locks_treat_the_clone_stamp_like_the_brush() {
    let pixels = aurora_doc::LayerLock {
        pixels: true,
        ..aurora_doc::LayerLock::default()
    };
    let transparency = aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::default()
    };
    assert_eq!(
        crate::press_refusal(Tool::CloneStamp, pixels),
        Some(LockRefusal::PixelsLocked)
    );
    assert_eq!(
        crate::press_refusal(Tool::CloneStamp, transparency),
        None,
        "a transparency lock is honoured by keeping alpha, not by refusing"
    );
    assert_eq!(
        crate::press_refusal(Tool::CloneStamp, aurora_doc::LayerLock::default()),
        None
    );
    // The notices stay on screen only for the Clone Stamp.
    for why in [
        LockRefusal::CloneNeedsSource,
        LockRefusal::CloneNeedsPixelLayer,
    ] {
        assert!(crate::lock_refusal_applies(
            why,
            Tool::CloneStamp,
            aurora_doc::LayerLock::default()
        ));
        assert!(!crate::lock_refusal_applies(
            why,
            Tool::Brush,
            aurora_doc::LayerLock::default()
        ));
    }
}

#[test]
fn the_app_wires_the_press_the_dab_and_the_stamp_to_the_clone_stamp() {
    // `App` needs a window; these single-line checks pin the glue the free
    // helpers cannot: the press asks `clone_stamp::press` with Alt, the dab
    // site dispatches on the drag's offset and stamps through the selection.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert!(production.contains("self.modifiers.alt,"));
    assert!(production.contains("clone_stamp::drag_offset(self.drag.as_ref())"));
    assert!(production.contains("clone_stamp::clone_dab_through_selection("));
    assert!(production.contains("clone_stamp::set_drag_offset(&mut self.drag, offset)"));
}
