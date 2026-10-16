//! The real command palette with its real command list (0.257.0): well
//! over a hundred commands in a limited panel must stack at one row
//! pitch, scroll under the keyboard, the wheel and the scrollbar's
//! container, and a press on a row must run that row.

use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey};
use aurora_widgets::widgets::{self, WidgetKind, command_palette_state};
use aurora_widgets::{FocusManager, WidgetId, WidgetTree};

use crate::{
    ActivatedCommand, COMMAND_UNDO, ClipboardAccess, FileDialogAccess, handle_palette_key,
    open_command_palette, palette_row_click, palette_wheel_target, scroll_panel,
};

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

struct Rig {
    workspace: aurora_ui::Workspace,
    focus: FocusManager,
    palette: Option<WidgetId>,
    size: (f32, f32),
}

impl Rig {
    fn open(size: (f32, f32)) -> Self {
        let mut rig = Self {
            workspace: aurora_ui::build_workspace(&crate::test_workspace_scales()),
            focus: FocusManager::default(),
            palette: None,
            size,
        };
        open_command_palette(&mut rig.workspace, &mut rig.focus, &mut rig.palette);
        rig.layout();
        rig
    }

    fn layout(&mut self) {
        aurora_widgets::compute_text_layout(&mut self.workspace.tree, self.size.0, self.size.1, None);
    }

    fn root(&self) -> WidgetId {
        match self.palette {
            Some(root) => root,
            None => unreachable!("the palette is open"),
        }
    }

    fn tree(&self) -> &WidgetTree<WidgetKind> {
        &self.workspace.tree
    }

    fn key(&mut self, key: Key, text: Option<&str>) -> Option<ActivatedCommand> {
        let picked = handle_palette_key(
            &mut self.workspace,
            &mut self.focus,
            &mut self.palette,
            KeyChord::new(Modifiers::default(), key),
            text,
            &mut NoClipboard,
            &mut NoFiles,
        );
        if self.palette.is_some() {
            self.layout();
        }
        picked
    }

    fn named(&mut self, key: NamedKey) {
        self.key(Key::Named(key), None);
    }

    fn state(&self) -> &widgets::CommandPaletteState {
        match command_palette_state(self.tree(), self.root()) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn selected_inside_viewport(&self) -> bool {
        let state = self.state();
        let (Some(row), Some(view)) = (
            state.selected_row().and_then(|r| self.tree().bounds(r)),
            self.tree().bounds(state.viewport()),
        ) else {
            return false;
        };
        row.y >= view.y && row.bottom() <= view.bottom()
    }
}

const SIZES: [(f32, f32); 3] = [(1274.0, 900.0), (800.0, 600.0), (640.0, 480.0)];

fn row_h() -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let h = widgets::row_height(&crate::test_workspace_scales()) as i64;
    h
}

#[test]
fn the_real_palette_lists_far_more_commands_than_fit() {
    let rig = Rig::open(SIZES[0]);
    assert!(
        rig.state().rows().len() > 100,
        "the shipped palette has well over a hundred commands: {}",
        rig.state().rows().len()
    );
}

#[test]
fn the_real_palette_rows_never_overlap_and_the_panel_fits_every_window() {
    let scales = crate::test_workspace_scales();
    for size in SIZES {
        let rig = Rig::open(size);
        let state = rig.state();
        let body_y = match rig.tree().bounds(state.body()) {
            Some(b) => b.y,
            None => unreachable!("laid out"),
        };
        for (n, &row) in state.rows().iter().enumerate() {
            let Some(b) = rig.tree().bounds(row) else {
                unreachable!("laid out")
            };
            assert_eq!(i64::from(b.height), row_h(), "row {n} at {size:?}");
            assert_eq!(b.y, body_y + row_h() * i64::try_from(n).unwrap_or(0));
        }
        let Some(root) = rig.tree().bounds(rig.root()) else {
            unreachable!("laid out")
        };
        let strip = row_h() + i64::from(scales.spacing.xs);
        assert_eq!(
            i64::from(root.height),
            strip + i64::from(scales.size.content_panel_max_rows) * row_h()
        );
        assert!(root.bottom() <= i64::from(size.1 as u32), "{root:?} in {size:?}");
    }
}

#[test]
fn arrow_down_page_down_and_end_keep_the_selection_visible_and_enter_runs_it() {
    let mut rig = Rig::open(SIZES[0]);
    for _ in 0..40 {
        rig.named(NamedKey::ArrowDown);
        assert!(rig.selected_inside_viewport());
    }
    assert_eq!(rig.state().selected_index(), Some(40));
    rig.named(NamedKey::PageDown);
    assert_eq!(rig.state().selected_index(), Some(50), "a page is ten rows");
    assert!(rig.selected_inside_viewport());
    rig.named(NamedKey::PageUp);
    assert_eq!(rig.state().selected_index(), Some(40));
    rig.named(NamedKey::End);
    assert_eq!(rig.state().selected_index(), Some(rig.state().rows().len() - 1));
    assert!(rig.selected_inside_viewport());
    rig.named(NamedKey::Home);
    assert_eq!(rig.state().selected_index(), Some(0));
    assert!(rig.selected_inside_viewport());
    // Enter runs the selected command and closes the palette.
    let id = rig.state().selected().map(|c| c.id.clone());
    assert!(id.is_some());
    rig.named(NamedKey::Enter);
    assert_eq!(rig.palette, None);
}

#[test]
fn typing_a_query_that_leaves_few_results_shrinks_the_panel() {
    let mut rig = Rig::open(SIZES[0]);
    let tall = rig.tree().bounds(rig.root()).map(|b| b.height);
    for ch in "undo".chars() {
        let text = ch.to_string();
        rig.key(Key::Character(ch), Some(&text));
    }
    let results = rig.state().rows().len();
    assert!((1..10).contains(&results), "{results}");
    let now = rig.tree().bounds(rig.root()).map(|b| b.height);
    assert!(now < tall, "{now:?} vs {tall:?}");
    assert_eq!(
        rig.tree().style(rig.state().scrollbar()).map(|s| s.display),
        Some(taffy::Display::None)
    );
    assert_eq!(rig.state().selected_index(), Some(0));
}

#[test]
fn the_wheel_over_the_palette_scrolls_the_rows_and_elsewhere_it_does_not() {
    let mut rig = Rig::open(SIZES[0]);
    let list = rig.state().body();
    let first = rig.state().rows()[0];
    let Some(b) = rig.tree().bounds(first) else {
        unreachable!("laid out")
    };
    let over = (b.x as f32 + 30.0, b.y as f32 + 5.0);
    assert_eq!(palette_wheel_target(rig.tree(), rig.palette, over), Some(list));
    assert_eq!(
        palette_wheel_target(rig.tree(), rig.palette, (2.0, 2.0)),
        None,
        "off the palette the ordinary routing runs"
    );
    assert_eq!(palette_wheel_target(rig.tree(), None, over), None);
    let scales = crate::test_workspace_scales();
    let moved = scroll_panel(
        &mut rig.workspace.tree,
        list,
        winit::event::MouseScrollDelta::LineDelta(0.0, -3.0),
        1.0,
        &scales,
    );
    assert!(moved);
    assert_eq!(
        rig.tree().scroll_y(list),
        Some(3.0 * widgets::row_height(&scales)),
        "one notch is one row"
    );
}

#[test]
fn a_press_on_a_scrolled_row_runs_that_row_and_a_press_elsewhere_does_nothing() {
    let mut rig = Rig::open(SIZES[0]);
    let list = rig.state().body();
    let Some(index) = rig
        .state()
        .results()
        .iter()
        .position(|entry| entry.id == COMMAND_UNDO)
    else {
        unreachable!("Undo is in the palette");
    };
    // Scroll so Undo is near the top of the viewport, by whole rows.
    #[allow(clippy::cast_precision_loss)]
    let offset = (index.saturating_sub(2) as f32) * widgets::row_height(&crate::test_workspace_scales());
    let _ = rig.workspace.tree.set_scroll_y(list, offset);
    rig.layout();
    let Some(b) = rig.state().rows().get(index).and_then(|&r| rig.tree().bounds(r)) else {
        unreachable!("laid out")
    };
    // Not on a row: the strip. Nothing happens and the palette stays.
    let strip = match rig.tree().bounds(rig.state().query_strip()) {
        Some(s) => (s.x as f32 + 10.0, s.y as f32 + 5.0),
        None => unreachable!("laid out"),
    };
    let outcome = palette_row_click(
        &mut rig.workspace,
        &mut rig.focus,
        &mut rig.palette,
        strip,
        &mut NoClipboard,
        &mut NoFiles,
    );
    assert_eq!(outcome, None);
    assert!(rig.palette.is_some());
    // On Undo's row: it runs exactly Undo and the palette closes.
    let at = (b.x as f32 + 30.0, b.y as f32 + b.height as f32 / 2.0);
    let outcome = palette_row_click(
        &mut rig.workspace,
        &mut rig.focus,
        &mut rig.palette,
        at,
        &mut NoClipboard,
        &mut NoFiles,
    );
    assert_eq!(outcome, Some(Some(ActivatedCommand::Undo)));
    assert_eq!(rig.palette, None);
}

#[test]
fn a_name_prompt_with_many_rows_is_laid_out_the_same_way() {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = FocusManager::default();
    let mut palette = None;
    assert!(crate::workspace_presets::open_workspace_name_prompt(
        &mut workspace,
        &mut focus,
        &mut palette,
        "",
    ));
    aurora_widgets::compute_text_layout(&mut workspace.tree, 800.0, 600.0, None);
    let Some(root) = palette else {
        unreachable!("open")
    };
    let Ok(state) = command_palette_state(&workspace.tree, root) else {
        unreachable!("a palette")
    };
    let Some(strip) = workspace.tree.bounds(state.query_strip()) else {
        unreachable!("laid out")
    };
    let scales = crate::test_workspace_scales();
    assert_eq!(
        i64::from(strip.height),
        row_h() + i64::from(scales.spacing.xs),
        "the prompt's strip is the fixed height too"
    );
    for &row in state.rows() {
        let Some(b) = workspace.tree.bounds(row) else {
            unreachable!("laid out")
        };
        assert_eq!(i64::from(b.height), row_h());
    }
}
