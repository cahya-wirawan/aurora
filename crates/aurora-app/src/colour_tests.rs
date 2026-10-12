//! Tests for the foreground/background colour controls (0.208.0): the
//! shortcuts and palette commands, the swatches following the state, the
//! pointer and key flows into the picker popover, the Brush and
//! Eyedropper colours, and the Tools panel layout.
//!
//! `App` needs a window, so the flows are driven through the same free
//! functions `App` calls ([`route_widget_pointer`], [`route_widget_key`],
//! [`ColourController`]); `App`'s own thin wiring (which field it reads,
//! the Escape hook) is therefore **not** run here.

#![allow(clippy::float_cmp)] // exact constants and exact copies, not computed values

use crate::colour_state::{ColourController, ColourState, Effect, eyedropper_target};
use crate::{
    ActivatedCommand, AppCommand, ClipboardAccess, FileDialogAccess, KeyOutcome, PointerPhase,
    WidgetOwner, WidgetPointer, default_shortcuts, default_shortcuts_for, handle_key,
    palette_commands, route_widget_key, route_widget_pointer,
};
use aurora_ui::{ColourAction, ColourTarget, Tool};
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey};
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{ActionOutcome, ClickTracker, FocusManager, PointerOutcome, WidgetId};

const W: f32 = 1000.0;
const H: f32 = 800.0;

#[derive(Default)]
struct NoClipboard;

impl ClipboardAccess for NoClipboard {
    fn get_text(&mut self) -> Option<String> {
        None
    }
    fn set_text(&mut self, _text: String) {}
}

struct NoFiles;

impl FileDialogAccess for NoFiles {
    fn pick_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
    fn save_file(&mut self) -> Option<std::path::PathBuf> {
        None
    }
}

/// `App`'s colour wiring without a window.
struct Rig {
    workspace: aurora_ui::Workspace,
    focus: FocusManager,
    click: ClickTracker,
    scales: aurora_theme::Scales,
    colour: ColourController,
}

impl Rig {
    fn new() -> Self {
        let scales = crate::test_workspace_scales();
        let mut workspace = aurora_ui::build_workspace(&scales);
        workspace.tree.compute_layout(W, H);
        let mut rig = Self {
            workspace,
            focus: FocusManager::new(),
            click: ClickTracker::default(),
            scales,
            colour: ColourController::default(),
        };
        rig.colour.sync(&mut rig.workspace);
        rig
    }

    fn apply(&mut self, effect: Effect) {
        if effect.relayout {
            self.workspace.tree.compute_layout(W, H);
        }
    }

    fn centre(&self, id: WidgetId) -> (f32, f32) {
        let Some(b) = self.workspace.tree.bounds(id) else {
            unreachable!("laid out");
        };
        (
            b.x as f32 + b.width as f32 / 2.0,
            b.y as f32 + b.height as f32 / 2.0,
        )
    }

    /// `App::route_gallery`'s colour half: light dismiss on a `Down`, then
    /// the shared router, then the colour outcome.
    fn pointer(&mut self, phase: PointerPhase, at: (f32, f32)) -> WidgetPointer {
        if phase == PointerPhase::Down {
            let effect = self
                .colour
                .light_dismiss(&mut self.workspace, &mut self.focus, at);
            self.apply(effect);
        }
        let routed = route_widget_pointer(
            &mut self.workspace,
            &mut self.focus,
            &mut None,
            None,
            None,
            &mut self.click,
            &self.scales,
            false,
            phase,
            at,
            Modifiers::none(),
            &mut crate::NoTextHit,
        );
        if routed.owner == Some(WidgetOwner::ColourControls)
            && let Some(outcome) = routed.outcome.as_ref()
        {
            let effect = self.colour.apply_outcome(
                &mut self.workspace,
                &mut self.focus,
                &self.scales,
                outcome,
            );
            self.apply(effect);
        }
        routed
    }

    fn click(&mut self, id: WidgetId) -> WidgetPointer {
        let at = self.centre(id);
        let _ = self.pointer(PointerPhase::Down, at);
        self.pointer(PointerPhase::Up, at)
    }

    fn key(&mut self, key: Key) -> Option<(WidgetOwner, KeyOutcome)> {
        let routed = route_widget_key(
            &mut self.workspace,
            &mut self.focus,
            &mut None,
            None,
            None,
            &self.scales,
            false,
            false,
            Modifiers::none(),
            key,
            None,
            &mut NoClipboard,
        );
        if let Some((WidgetOwner::ColourControls, KeyOutcome::Handled(outcome))) = &routed {
            let effect = self.colour.apply_outcome(
                &mut self.workspace,
                &mut self.focus,
                &self.scales,
                outcome,
            );
            self.apply(effect);
        }
        routed
    }

    fn swatch_colour(&self, target: ColourTarget) -> [u8; 3] {
        let id = self.workspace.tools.colours.swatch(target);
        let Some(WidgetKind::ColorSwatch(state)) = self.workspace.tree.payload(id) else {
            unreachable!("a swatch");
        };
        [state.color.r, state.color.g, state.color.b]
    }

    fn label(&self, id: WidgetId) -> String {
        self.workspace
            .tree
            .accessibility(id)
            .and_then(|node| node.label().map(str::to_owned))
            .unwrap_or_default()
    }
}

fn right(r: aurora_core::Rect) -> i64 {
    r.x + i64::from(r.width)
}

fn bottom(r: aurora_core::Rect) -> i64 {
    r.y + i64::from(r.height)
}

fn shortcut_command(key: char) -> Option<AppCommand> {
    default_shortcuts()
        .resolve(KeyChord::new(Modifiers::none(), Key::Character(key)))
        .copied()
}

#[test]
fn x_and_d_are_bound_and_collide_with_no_tool_letter() {
    assert_eq!(shortcut_command('x'), Some(AppCommand::SwapColours));
    assert_eq!(shortcut_command('d'), Some(AppCommand::ResetColours));
    // Every tool's letter still selects its own tool, and none of them is
    // X or D.
    for tool in Tool::ALL {
        let key = tool.shortcut_key().to_ascii_lowercase();
        assert_ne!(key, 'x');
        assert_ne!(key, 'd');
        if tool.shortcut_needs_shift() {
            // Paint Bucket is Shift+G (0.209.0), never the bare letter.
            assert_eq!(
                shortcut_command(key),
                Some(AppCommand::SelectTool(Tool::Gradient))
            );
            continue;
        }
        assert_eq!(shortcut_command(key), Some(AppCommand::SelectTool(tool)));
    }
    // On macOS the table is the same for these bare keys.
    for primary_is_meta in [false, true] {
        let table = default_shortcuts_for(primary_is_meta);
        let chord = |c| KeyChord::new(Modifiers::none(), Key::Character(c));
        assert_eq!(table.resolve(chord('x')), Some(&AppCommand::SwapColours));
        assert_eq!(table.resolve(chord('d')), Some(&AppCommand::ResetColours));
    }
}

#[test]
fn the_keys_hand_the_colour_commands_back_to_the_app() {
    for (key, expected) in [
        ('x', ActivatedCommand::SwapColours),
        ('d', ActivatedCommand::ResetColours),
    ] {
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        let mut focus = FocusManager::default();
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = crate::UndoOrder::default();
        let picked = handle_key(
            &mut workspace,
            &mut focus,
            &mut None,
            &mut None,
            &mut tool,
            &crate::ToolSettings::default(),
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &default_shortcuts(),
            Modifiers::none(),
            Key::Character(key),
            None,
            &mut NoClipboard,
            &mut NoFiles,
        );
        assert_eq!(picked, Some(expected), "{key}");
        assert_eq!(tool, Tool::default(), "a colour key does not switch tools");
    }
}

#[test]
fn the_palette_lists_and_resolves_both_colour_commands() {
    let commands = palette_commands();
    let ids: Vec<&str> = commands.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"colour.swap"));
    assert!(ids.contains(&"colour.reset"));
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = FocusManager::default();
    assert_eq!(
        crate::activate_command(&mut workspace, &mut focus, "colour.swap", &mut NoFiles),
        Some(ActivatedCommand::SwapColours)
    );
    assert_eq!(
        crate::activate_command(&mut workspace, &mut focus, "colour.reset", &mut NoFiles),
        Some(ActivatedCommand::ResetColours)
    );
}

#[test]
fn swatches_start_black_over_white_and_name_their_colour() {
    let rig = Rig::new();
    let c = rig.workspace.tools.colours;
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [0, 0, 0]);
    assert_eq!(rig.swatch_colour(ColourTarget::Background), [255, 255, 255]);
    assert_eq!(rig.label(c.foreground), "Foreground colour, RGB 0 0 0");
    assert_eq!(
        rig.label(c.background),
        "Background colour, RGB 255 255 255"
    );
    for id in [c.foreground, c.background, c.swap, c.defaults] {
        let Some(node) = rig.workspace.tree.accessibility(id) else {
            unreachable!("built");
        };
        assert!(node.supports_action(accesskit::Action::Click), "{id:?}");
        assert!(node.description().is_some_and(|d| !d.is_empty()), "{id:?}");
    }
    assert_eq!(rig.label(c.swap), aurora_ui::SWAP_COLOURS_LABEL);
    assert!(
        rig.workspace
            .tree
            .accessibility(c.swap)
            .and_then(|n| n.description())
            .is_some_and(|d| d.contains("shortcut X"))
    );
    assert!(
        rig.workspace
            .tree
            .accessibility(c.defaults)
            .and_then(|n| n.description())
            .is_some_and(|d| d.contains("shortcut D"))
    );
}

#[test]
fn swatches_track_the_state_after_swap_reset_and_set() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    let effect = rig.colour.set(
        &mut rig.workspace,
        ColourTarget::Foreground,
        [1.0, 0.0, 0.0],
    );
    assert!(effect.changed);
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [255, 0, 0]);
    assert_eq!(rig.label(c.foreground), "Foreground colour, RGB 255 0 0");
    let effect = rig.colour.swap(&mut rig.workspace);
    assert!(effect.changed);
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [255, 255, 255]);
    assert_eq!(rig.swatch_colour(ColourTarget::Background), [255, 0, 0]);
    assert_eq!(rig.label(c.background), "Background colour, RGB 255 0 0");
    assert_eq!(
        rig.label(c.foreground),
        "Foreground colour, RGB 255 255 255"
    );
    let effect = rig.colour.reset(&mut rig.workspace);
    assert!(effect.changed);
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [0, 0, 0]);
    assert_eq!(rig.swatch_colour(ColourTarget::Background), [255, 255, 255]);
    assert_eq!(rig.label(c.foreground), "Foreground colour, RGB 0 0 0");
    // A no-op reports nothing.
    assert_eq!(rig.colour.reset(&mut rig.workspace), Effect::default());
    // Refused values touch neither the state nor the swatch.
    assert_eq!(
        rig.colour.set(
            &mut rig.workspace,
            ColourTarget::Foreground,
            [f32::NAN, 0.0, 0.0]
        ),
        Effect::default()
    );
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [0, 0, 0]);
}

#[test]
fn clicking_swap_and_defaults_runs_the_same_commands_as_the_keys() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.colour.set(
        &mut rig.workspace,
        ColourTarget::Foreground,
        [0.0, 0.0, 1.0],
    );
    let routed = rig.click(c.swap);
    assert_eq!(routed.owner, Some(WidgetOwner::ColourControls));
    assert_eq!(rig.colour.state.foreground(), [1.0, 1.0, 1.0]);
    assert_eq!(rig.colour.state.background(), [0.0, 0.0, 1.0]);
    assert_eq!(rig.swatch_colour(ColourTarget::Background), [0, 0, 255]);
    let routed = rig.click(c.defaults);
    assert_eq!(routed.owner, Some(WidgetOwner::ColourControls));
    assert_eq!(rig.colour.state, ColourState::default());
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [0, 0, 0]);
    assert!(matches!(
        routed.outcome,
        Some(PointerOutcome::Action(ActionOutcome::Activated(id))) if id == c.defaults
    ));
}

#[test]
fn a_swatch_opens_its_own_picker_and_a_second_click_closes_it() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.colour.set(
        &mut rig.workspace,
        ColourTarget::Background,
        [0.0, 1.0, 0.0],
    );
    let routed = rig.click(c.background);
    assert_eq!(routed.owner, Some(WidgetOwner::ColourControls));
    let Some(open) = rig.colour.popover else {
        unreachable!("opened");
    };
    assert_eq!(open.target, ColourTarget::Background);
    assert_eq!(
        rig.workspace.tree.layer(open.frame),
        Some(aurora_widgets::PaintLayer::Popover)
    );
    assert_eq!(
        rig.label(open.frame),
        "Background colour picker",
        "the popover names the colour it edits"
    );
    // The picker opens on the background's colour.
    let Ok(picker) = widgets::color_picker_state(&rig.workspace.tree, open.picker) else {
        unreachable!("a picker");
    };
    assert_eq!(picker.color(), aurora_ui::swatch_colour([0.0, 1.0, 0.0]));
    // Keyboard focus moved into it.
    assert!(
        rig.focus
            .focused()
            .is_some_and(|f| open.contains(&rig.workspace.tree, f))
    );
    // The same swatch again closes it and keeps focus on the swatch.
    rig.click(c.background);
    assert_eq!(rig.colour.popover, None);
    assert!(!rig.workspace.tree.contains(open.frame));
    assert_eq!(rig.focus.focused(), Some(c.background));
}

#[test]
fn the_other_swatch_moves_the_picker_over() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.click(c.foreground);
    let first = rig.colour.popover;
    assert_eq!(first.map(|o| o.target), Some(ColourTarget::Foreground));
    rig.click(c.background);
    let Some(second) = rig.colour.popover else {
        unreachable!("still open");
    };
    assert_eq!(second.target, ColourTarget::Background);
    assert!(first.is_some_and(|o| !rig.workspace.tree.contains(o.frame)));
    // Exactly one popover exists.
    assert_eq!(rig.workspace.tree.popover_roots().len(), 1);
}

#[test]
fn a_picker_edit_changes_only_the_colour_it_was_opened_for() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.click(c.background);
    let Some(open) = rig.colour.popover else {
        unreachable!("opened");
    };
    let Ok(picker) = widgets::color_picker_state(&rig.workspace.tree, open.picker) else {
        unreachable!("a picker");
    };
    let Some(hue) = picker.part_id(widgets::ColorPickerPart::Hue) else {
        unreachable!("a hue strip");
    };
    // A press on the hue strip's middle sets a hue: a saturated-or-not
    // colour, but the pure-white background becomes a different colour.
    let at = rig.centre(hue);
    let _ = rig.pointer(PointerPhase::Down, at);
    let _ = rig.pointer(PointerPhase::Up, at);
    assert_eq!(
        rig.colour.state.foreground(),
        [0.0, 0.0, 0.0],
        "the foreground is untouched by a background pick"
    );
    // White has no saturation, so a hue alone leaves it white; drag in the
    // square instead, which sets saturation and value.
    let Ok(picker) = widgets::color_picker_state(&rig.workspace.tree, open.picker) else {
        unreachable!("a picker");
    };
    let Some(area) = picker.area_id() else {
        unreachable!("an area");
    };
    let Some(b) = rig.workspace.tree.bounds(area) else {
        unreachable!("laid out");
    };
    let at = (
        b.x as f32 + b.width as f32 * 0.9,
        b.y as f32 + b.height as f32 * 0.5,
    );
    let _ = rig.pointer(PointerPhase::Down, at);
    let _ = rig.pointer(PointerPhase::Up, at);
    assert_ne!(rig.colour.state.background(), [1.0, 1.0, 1.0]);
    assert_ne!(
        aurora_ui::swatch_colour(rig.colour.state.background()),
        aurora_ui::swatch_colour([1.0; 3])
    );
    assert_eq!(rig.colour.state.foreground(), [0.0, 0.0, 0.0]);
    // The swatch follows the edit, live.
    assert_eq!(rig.swatch_colour(ColourTarget::Background), {
        let k = aurora_ui::swatch_colour(rig.colour.state.background());
        [k.r, k.g, k.b]
    });
}

#[test]
fn a_picker_key_edits_the_target_and_x_still_reaches_the_shortcuts() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.click(c.foreground);
    let Some(open) = rig.colour.popover else {
        unreachable!("opened");
    };
    // Focus is on a picker part: an arrow key edits the foreground.
    let before = rig.colour.state.foreground();
    let routed = rig.key(Key::Named(NamedKey::ArrowUp));
    assert!(matches!(
        routed,
        Some((WidgetOwner::ColourControls, KeyOutcome::Handled(_)))
    ));
    assert_ne!(rig.colour.state.foreground(), before);
    assert_eq!(rig.colour.state.background(), [1.0, 1.0, 1.0]);
    // A character key is not a widget's: `X` falls through to the table.
    assert!(rig.key(Key::Character('x')).is_none());
    assert!(rig.key(Key::Character('d')).is_none());
    // Escape's effect (the app's hook): closing returns focus to the swatch.
    let effect = rig
        .colour
        .close_popover(&mut rig.workspace, &mut rig.focus, true);
    rig.apply(effect);
    assert!(!rig.workspace.tree.contains(open.frame));
    assert_eq!(rig.focus.focused(), Some(c.foreground));
}

#[test]
fn a_press_elsewhere_closes_the_picker_but_a_press_inside_or_on_a_swatch_does_not() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    rig.click(c.foreground);
    let Some(open) = rig.colour.popover else {
        unreachable!("opened");
    };
    // Inside the picker.
    let inside = rig.centre(open.picker);
    let _ = rig.pointer(PointerPhase::Down, inside);
    let _ = rig.pointer(PointerPhase::Up, inside);
    assert!(rig.colour.popover.is_some(), "a press inside keeps it open");
    // On a swatch's Down (the Up toggles).
    let swatch = rig.centre(c.background);
    let effect = rig
        .colour
        .light_dismiss(&mut rig.workspace, &mut rig.focus, swatch);
    assert_eq!(effect, Effect::default());
    assert!(rig.colour.popover.is_some());
    // On the canvas area.
    let canvas = rig.centre(rig.workspace.canvas_area);
    assert!(
        !open.contains(
            &rig.workspace.tree,
            rig.workspace.tree.hit_test(canvas).unwrap_or(open.frame)
        ),
        "the canvas point is outside the popover"
    );
    let _ = rig.pointer(PointerPhase::Down, canvas);
    assert_eq!(rig.colour.popover, None);
    assert!(!rig.workspace.tree.contains(open.frame));
}

#[test]
fn the_brush_paints_the_foreground_colour_exactly() {
    let mut state = ColourState::default();
    assert!(state.set(ColourTarget::Foreground, [0.25, 0.5, 0.75]));
    assert!(state.set(ColourTarget::Background, [1.0, 0.0, 0.0]));
    assert_eq!(state.paint_colour(), [0.25, 0.5, 0.75]);
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(s) => s,
        Err(err) => unreachable!("{err:?}"),
    };
    let surface = aurora_tile::SurfaceId::from_raw(7);
    let settings = crate::ToolSettings::default();
    let mut drag = None;
    let _ = crate::stamp_tool_dab(
        crate::DabTool::Brush,
        &mut store,
        surface,
        (60.0, 60.0),
        &settings,
        state.paint_colour(),
        &mut drag,
    );
    // The centre texel is premultiplied (the brush is soft: alpha just
    // under one); un-premultiplied, it is the foreground, not the
    // background, to f16 precision.
    let Some([r, g, b, a]) = crate::sample_pixel(&mut store, surface, (60.0, 60.0)) else {
        unreachable!("the dab painted its centre");
    };
    assert!(a > 0.9, "alpha {a}");
    for (got, want) in [(r / a, 0.25), (g / a, 0.5), (b / a, 0.75)] {
        assert!((got - want).abs() < 0.002, "{got} vs {want}");
    }
    // Swapping changes what the next dab paints.
    assert!(state.swap());
    assert_eq!(state.paint_colour(), [1.0, 0.0, 0.0]);
}

#[test]
fn the_eyedropper_sets_the_foreground_and_alt_sets_the_background() {
    let mut rig = Rig::new();
    let alt = Modifiers {
        alt: true,
        ..Modifiers::none()
    };
    assert_eq!(
        eyedropper_target(Modifiers::none()),
        ColourTarget::Foreground
    );
    assert_eq!(eyedropper_target(alt), ColourTarget::Background);
    let effect =
        rig.colour
            .eyedropper_pick(&mut rig.workspace, Modifiers::none(), [0.5, 0.25, 0.75]);
    assert!(effect.changed);
    assert_eq!(rig.colour.state.foreground(), [0.5, 0.25, 0.75]);
    assert_eq!(rig.colour.state.background(), [1.0, 1.0, 1.0]);
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [128, 64, 191]);
    rig.colour
        .eyedropper_pick(&mut rig.workspace, alt, [0.0, 1.0, 0.0]);
    assert_eq!(rig.colour.state.background(), [0.0, 1.0, 0.0]);
    assert_eq!(rig.colour.state.foreground(), [0.5, 0.25, 0.75]);
    assert_eq!(rig.swatch_colour(ColourTarget::Background), [0, 255, 0]);
    // An eyedropper pick refreshes an open picker for that colour.
    rig.click(rig.workspace.tools.colours.foreground);
    rig.colour
        .eyedropper_pick(&mut rig.workspace, Modifiers::none(), [1.0, 0.0, 0.0]);
    let Some(open) = rig.colour.popover else {
        unreachable!("open");
    };
    let Ok(picker) = widgets::color_picker_state(&rig.workspace.tree, open.picker) else {
        unreachable!("a picker");
    };
    assert_eq!(picker.color(), aurora_ui::swatch_colour([1.0, 0.0, 0.0]));
}

#[test]
fn the_colour_group_fits_the_strip_and_its_parts_do_not_collide() {
    let rig = Rig::new();
    let t = &rig.workspace.tree;
    let c = rig.workspace.tools.colours;
    let bounds = |id| match t.bounds(id) {
        Some(b) => b,
        None => unreachable!("laid out"),
    };
    let strip = bounds(rig.workspace.tools.root);
    let group = bounds(c.group);
    assert_eq!(strip.width, 72, "the strip stays two columns wide");
    assert!(group.x >= strip.x && right(group) <= right(strip));
    assert!(bottom(group) <= bottom(strip));
    assert_eq!(group.width, 64);
    assert_eq!(group.height, 64);
    let inside = |id| {
        let b = bounds(id);
        b.x >= group.x && b.y >= group.y && right(b) <= right(group) && bottom(b) <= bottom(group)
    };
    for id in [c.background, c.foreground, c.swap, c.defaults] {
        assert!(inside(id), "{id:?} lies in the group");
    }
    let (fg, bg) = (bounds(c.foreground), bounds(c.background));
    assert_eq!((fg.x, fg.y), (group.x, group.y));
    assert_eq!(
        (bg.x - fg.x, bg.y - fg.y),
        (16, 16),
        "overlapped like Photoshop"
    );
    let (swap, defaults) = (bounds(c.swap), bounds(c.defaults));
    assert_eq!(right(swap), right(group), "swap is top-right");
    assert_eq!(swap.y, group.y);
    assert_eq!(defaults.x, group.x, "defaults is bottom-left");
    assert_eq!(bottom(defaults), bottom(group));
    let overlaps = |a: aurora_core::Rect, b: aurora_core::Rect| {
        a.x < right(b) && b.x < right(a) && a.y < bottom(b) && b.y < bottom(a)
    };
    for (a, b) in [
        (swap, fg),
        (swap, bg),
        (defaults, fg),
        (defaults, bg),
        (swap, defaults),
    ] {
        assert!(!overlaps(a, b), "{a:?} and {b:?}");
    }
    // The foreground is in front: a hit at the overlap lands on it.
    let overlap = (bg.x as f32 + 2.0, bg.y as f32 + 2.0);
    assert_eq!(t.hit_test(overlap), Some(c.foreground));
    // And a hit on the background's exposed corner lands on the background.
    let corner = ((right(bg)) as f32 - 2.0, (bottom(bg)) as f32 - 2.0);
    assert_eq!(t.hit_test(corner), Some(c.background));
}

#[test]
fn the_pointer_lands_on_the_same_swatch_at_scale_one_and_two() {
    let rig = Rig::new();
    let c = rig.workspace.tools.colours;
    let (x, y) = rig.centre(c.swap);
    for scale in [1.0_f64, 2.0] {
        let physical = (f64::from(x) * scale, f64::from(y) * scale);
        let logical = crate::logical_point(physical, scale);
        assert_eq!(
            rig.workspace.tree.hit_test(logical),
            Some(c.swap),
            "scale {scale}"
        );
    }
}

#[test]
fn every_action_a_control_runs_is_distinct() {
    let rig = Rig::new();
    let c = rig.workspace.tools.colours;
    let actions: Vec<_> = [c.foreground, c.background, c.swap, c.defaults]
        .iter()
        .map(|&id| c.action_for(id))
        .collect();
    assert_eq!(
        actions,
        vec![
            Some(ColourAction::Edit(ColourTarget::Foreground)),
            Some(ColourAction::Edit(ColourTarget::Background)),
            Some(ColourAction::Swap),
            Some(ColourAction::Reset),
        ]
    );
}

#[test]
fn an_assistive_technology_click_reaches_the_colour_controls() {
    let mut rig = Rig::new();
    let c = rig.workspace.tools.colours;
    let layer_rows = std::collections::HashMap::new();
    for id in [c.foreground, c.background, c.swap, c.defaults] {
        let request = accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: aurora_widgets::ACCESSIBILITY_TREE_ID,
            target_node: id,
            data: None,
        };
        let reaction = crate::route_accessibility_action(
            &mut rig.workspace,
            &mut rig.focus,
            None,
            &layer_rows,
            None,
            None,
            &request,
        );
        assert!(
            matches!(
                reaction,
                crate::AccessibilityReaction::ColourControl(ActionOutcome::Activated(got)) if got == id
            ),
            "{id:?}: {reaction:?}"
        );
        // What the app does with it.
        let effect = rig.colour.apply_edit(
            &mut rig.workspace,
            &mut rig.focus,
            &rig.scales,
            crate::ColourEdit::Activated(id),
        );
        rig.apply(effect);
    }
    // swatch fg (opens), bg (retargets), swap (swapped), defaults (reset).
    assert_eq!(rig.colour.state, ColourState::default());
    assert_eq!(
        rig.colour.popover.map(|open| open.target),
        Some(ColourTarget::Background)
    );
    // A picker report reaches only the picker's own colour.
    let Some(open) = rig.colour.popover else {
        unreachable!("open");
    };
    let effect = rig.colour.apply_edit(
        &mut rig.workspace,
        &mut rig.focus,
        &rig.scales,
        crate::ColourEdit::Picked(open.picker, [0.5_f32, 0.25, 0.75].map(f32::to_bits)),
    );
    assert!(effect.changed);
    assert_eq!(rig.colour.state.background(), [0.5, 0.25, 0.75]);
    assert_eq!(rig.colour.state.foreground(), [0.0; 3]);
    let _ = rig.colour.apply_edit(
        &mut rig.workspace,
        &mut rig.focus,
        &rig.scales,
        crate::ColourEdit::Picked(c.group, [0.9_f32; 3].map(f32::to_bits)),
    );
    assert_eq!(
        rig.colour.state.foreground(),
        [0.0; 3],
        "a stranger picker is ignored"
    );
}

#[test]
fn an_eyedropper_pick_clamps_out_of_gamut_values_and_refuses_only_non_finite_ones() {
    let mut rig = Rig::new();
    // f16 overshoot and a tiny negative: clamped, not silently ignored.
    let effect = rig.colour.eyedropper_pick(
        &mut rig.workspace,
        Modifiers::none(),
        [1.0004, -0.0002, 0.5],
    );
    assert!(effect.changed);
    assert_eq!(rig.colour.state.foreground(), [1.0, 0.0, 0.5]);
    assert_eq!(rig.swatch_colour(ColourTarget::Foreground), [255, 0, 128]);
    // Non-finite: refused, nothing changes.
    for bad in [[f32::NAN, 0.0, 0.0], [0.0, f32::INFINITY, 0.0]] {
        let effect = rig
            .colour
            .eyedropper_pick(&mut rig.workspace, Modifiers::none(), bad);
        assert_eq!(effect, Effect::default());
    }
    assert_eq!(rig.colour.state.foreground(), [1.0, 0.0, 0.5]);
}

#[test]
fn dab_colour_is_the_foreground_for_both_dab_tools() {
    let mut state = ColourState::default();
    assert!(state.set(ColourTarget::Foreground, [0.25, 0.5, 0.75]));
    for tool in [crate::DabTool::Brush, crate::DabTool::Eraser] {
        assert_eq!(
            crate::colour_state::dab_colour(tool, &state),
            [0.25, 0.5, 0.75]
        );
    }
}

#[test]
fn escape_closes_the_popover_only_when_nothing_modal_owns_the_key() {
    use crate::colour_state::escape_closes_popover as f;
    let esc = Key::Named(NamedKey::Escape);
    assert!(f(esc, true, false, false));
    assert!(!f(esc, false, false, false), "no popover");
    assert!(!f(esc, true, true, false), "a dialog owns Escape");
    assert!(!f(esc, true, false, true), "the palette owns Escape");
    assert!(!f(Key::Named(NamedKey::Enter), true, false, false));
    assert!(!f(Key::Character('x'), true, false, false));
}

/// A source-level guard on `App`'s dab sites (which need a window to run):
/// both read their colour through `dab_colour`, and no non-test source of
/// `lib.rs` reads the colour state's foreground or background directly.
#[test]
fn the_app_dab_sites_take_their_colour_from_dab_colour_only() {
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    assert_eq!(
        production
            .matches("colour_state::dab_colour(DabTool::Brush, &self.colour.state)")
            .count(),
        1,
        "the Brush dab site"
    );
    assert_eq!(
        production
            .matches("colour_state::dab_colour(DabTool::Eraser, &self.colour.state)")
            .count(),
        1,
        "the Eraser dab site"
    );
    for forbidden in [
        "state.background()",
        "state.foreground()",
        "state.paint_colour()",
    ] {
        assert!(
            !production.contains(forbidden),
            "App reads {forbidden} directly instead of through dab_colour"
        );
    }
    // The Escape hook passes the real popover, dialog and palette state.
    // Compared with all whitespace removed: a Windows checkout has CRLF line
    // endings, and rustfmt's layout may change.
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    assert!(
        squeeze(production).contains(&squeeze(
            "colour_state::escape_closes_popover(key, self.colour.popover.is_some(), self.dialog.is_some(), self.command_palette.is_some(),)"
        )),
        "the Escape hook"
    );
}

/// 0.209.0: the Paint Bucket fills with the foreground and the Gradient
/// runs foreground to background (Reverse swaps them), and `App` reads
/// those colours only through `fill_foreground`/`fill_background`. Compared
/// with all whitespace removed (CRLF checkouts, rustfmt layout).
#[test]
fn the_app_fill_sites_take_their_colours_from_the_fill_accessors_only() {
    use crate::colour_state::{ColourState, fill_background, fill_foreground};
    let mut state = ColourState::default();
    assert!(state.set(ColourTarget::Foreground, [0.1, 0.2, 0.3]));
    assert!(state.set(ColourTarget::Background, [0.7, 0.8, 0.9]));
    assert_eq!(fill_foreground(&state), [0.1, 0.2, 0.3]);
    assert_eq!(fill_background(&state), [0.7, 0.8, 0.9]);

    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    let squeezed = squeeze(production);
    // The bucket: the foreground alone.
    assert!(squeezed.contains(&squeeze(
        "&self.tool_settings, colour_state::fill_foreground(&self.colour.state), self.doc.selection.active(), );"
    )));
    // The gradient: foreground then background.
    assert!(squeezed.contains(&squeeze(
        "colour_state::fill_foreground(&self.colour.state), colour_state::fill_background(&self.colour.state), self.doc.selection.active(), );"
    )));
    assert_eq!(
        squeezed.matches("colour_state::fill_foreground(").count(),
        2
    );
    assert_eq!(
        squeezed.matches("colour_state::fill_background(").count(),
        1
    );
}
