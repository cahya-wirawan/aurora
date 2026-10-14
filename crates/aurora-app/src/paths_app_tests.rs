//! Tests of the Paths glue (0.233.0): Make Selection from Path as one
//! selection step, the path commands as undo steps, the Pen's keys, the
//! panel's rows, the commands and the layout, the Quick Mask refusals.

use aurora_doc::{Anchor, PathSet, Subpath, WORK_PATH_NAME};
use aurora_ui::{Tool, build_workspace};

use super::{
    COMMAND_DELETE, COMMAND_MAKE_SELECTION, COMMAND_NEW, COMMAND_RENAME, COMMAND_SAVE_WORK,
    EMPTY_PATH_NOTICE, MAKE_SELECTION_LABEL, NO_PATH_NOTICE, PathsCommand, PenKey, command_for,
    edit_paths, make_selection, pen_key_action, record_step, selection_of_path, sync_paths,
};
use crate::document_session::{DocumentContents, DocumentSession};
use crate::{Key, Modifiers, NamedKey, UndoDirection, UndoKind};

fn scales() -> aurora_theme::Scales {
    match crate::load_scales() {
        Ok(scales) => scales,
        Err(err) => unreachable!("{err}"),
    }
}

fn session() -> DocumentSession {
    DocumentSession::new(DocumentContents {
        layers: aurora_doc::LayerTree::new(),
        history: aurora_doc::History::new(),
        canvas_size: (64, 64),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: None,
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: None,
    })
}

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn triangle() -> Vec<Subpath> {
    vec![Subpath {
        anchors: vec![
            Anchor::corner(4.0, 4.0),
            Anchor::corner(40.0, 6.0),
            Anchor::corner(20.0, 40.0),
        ],
        closed: true,
    }]
}

fn doc_with_path(name: &str) -> DocumentSession {
    let mut doc = session();
    ok(doc.selection.paths_mut().add(name));
    ok(doc.selection.paths_mut().set_subpaths(name, triangle()));
    doc
}

#[test]
fn make_selection_replaces_the_selection_as_one_undoable_step() {
    let mut ws = build_workspace(&scales());
    let mut doc = doc_with_path("Outline");
    let before = doc.selection.active();
    assert!(before.is_none());
    ok(make_selection(&mut ws, &mut doc, Some("Outline")).map_err(|e| e.to_owned()));
    let made = doc.selection.active();
    assert!(
        made.as_ref()
            .is_some_and(|s| matches!(s.shape, aurora_doc::SelectionShape::Mask(_)))
    );
    assert_eq!(doc.undo_order.undo, vec![UndoKind::Selection]);
    assert_eq!(doc.undo_order.undo_labels, vec![MAKE_SELECTION_LABEL]);
    assert!(!doc.is_dirty(), "a selection step never dirties");
    // Undo puts back "no selection"; redo the mask.
    assert!(doc.undo_order.apply_selection_step(UndoDirection::Undo));
    assert_eq!(doc.undo_order.take_pending_selection(), Some(None));
    assert!(doc.undo_order.apply_selection_step(UndoDirection::Redo));
    assert_eq!(doc.undo_order.take_pending_selection(), Some(made));
    // Doing it again over an existing selection is again one step whose
    // undo restores that selection.
    let existing = doc.selection.active();
    doc.selection
        .select(aurora_doc::Selection::new(aurora_core::Rect {
            x: 1,
            y: 1,
            width: 5,
            height: 5,
        }));
    let rect = doc.selection.active();
    ok(make_selection(&mut ws, &mut doc, Some("Outline")).map_err(|e| e.to_owned()));
    let _ = existing;
    assert!(doc.undo_order.apply_selection_step(UndoDirection::Undo));
    assert_eq!(doc.undo_order.take_pending_selection(), Some(rect));
}

#[test]
fn make_selection_refuses_calmly_and_records_nothing() {
    let mut ws = build_workspace(&scales());
    let mut doc = session();
    ok(doc.selection.paths_mut().add("Empty"));
    assert_eq!(
        make_selection(&mut ws, &mut doc, Some("Empty")),
        Err(EMPTY_PATH_NOTICE)
    );
    assert_eq!(
        make_selection(&mut ws, &mut doc, Some("Missing")),
        Err(NO_PATH_NOTICE)
    );
    assert_eq!(make_selection(&mut ws, &mut doc, None), Err(NO_PATH_NOTICE));
    assert!(doc.undo_order.undo.is_empty());
    assert!(doc.selection.active().is_none());
    // A path wholly outside the document selects nothing either.
    ok(doc.selection.paths_mut().set_subpaths(
        "Empty",
        vec![Subpath {
            anchors: vec![
                Anchor::corner(500.0, 500.0),
                Anchor::corner(540.0, 500.0),
                Anchor::corner(520.0, 540.0),
            ],
            closed: true,
        }],
    ));
    assert_eq!(
        selection_of_path(doc.selection.paths(), Some("Empty"), doc.canvas_size)
            .map_err(str::to_owned)
            .err()
            .as_deref(),
        Some(EMPTY_PATH_NOTICE)
    );
}

#[test]
fn the_pens_keys_act_only_on_the_pen_with_something_to_act_on() {
    let none = Modifiers::default();
    let enter = Key::Named(NamedKey::Enter);
    let escape = Key::Named(NamedKey::Escape);
    let back = Key::Named(NamedKey::Backspace);
    let delete = Key::Named(NamedKey::Delete);
    let action = |tool, modal, modifiers, drawing, selected, key| {
        pen_key_action(tool, modal, modifiers, drawing, selected, key)
    };
    assert_eq!(
        action(Tool::Pen, false, none, true, false, enter),
        PenKey::End
    );
    assert_eq!(
        action(Tool::Pen, false, none, true, false, escape),
        PenKey::Cancel
    );
    assert_eq!(
        action(Tool::Pen, false, none, true, false, back),
        PenKey::Backspace
    );
    assert_eq!(
        action(Tool::Pen, false, none, false, true, delete),
        PenKey::Delete
    );
    // Nothing to act on: the keys go on to the shortcuts.
    assert_eq!(
        action(Tool::Pen, false, none, false, false, enter),
        PenKey::Ignore
    );
    assert_eq!(
        action(Tool::Pen, false, none, false, true, escape),
        PenKey::Ignore
    );
    assert_eq!(
        action(Tool::Pen, false, none, true, false, delete),
        PenKey::Ignore
    );
    // Another tool, a modal, or a held modifier (Primary+Enter is Make
    // Selection) never gives the Pen the key.
    assert_eq!(
        action(Tool::Brush, false, none, true, true, enter),
        PenKey::Ignore
    );
    assert_eq!(
        action(Tool::Pen, true, none, true, true, enter),
        PenKey::Ignore
    );
    let ctrl = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert_eq!(
        action(Tool::Pen, false, ctrl, true, true, enter),
        PenKey::Ignore
    );
}

#[test]
fn path_commands_are_undo_steps_that_change_only_the_path_set() {
    let mut ws = build_workspace(&scales());
    let mut doc = session();
    // New Path: unique names.
    let name = doc.selection.paths().next_free_name();
    let step = ok(edit_paths(doc.selection.paths_mut(), "New Path", |set| {
        set.add(&name).map(|_| ())
    }));
    let Some(step) = step else {
        unreachable!("a step");
    };
    record_step(&mut ws, &mut doc, step);
    assert_eq!(
        doc.selection.paths().names().collect::<Vec<_>>(),
        vec!["Path 1"]
    );
    assert_eq!(doc.undo_order.undo, vec![UndoKind::Path]);
    assert_eq!(doc.undo_order.undo_labels, vec!["New Path"]);
    assert!(!doc.is_dirty());
    assert_eq!(doc.selection.paths().next_free_name(), "Path 2");
    // A rename onto a taken name is refused and changes nothing.
    ok(doc.selection.paths_mut().add("Other"));
    let before = doc.selection.paths().clone();
    let refused = edit_paths(doc.selection.paths_mut(), "Rename Path", |set| {
        set.rename("Path 1", "Other").map(|_| ())
    });
    assert!(refused.is_err());
    assert_eq!(doc.selection.paths(), &before);
    // Saving the work path is a rename into a normal path.
    ok(doc.selection.paths_mut().ensure_work());
    let step = ok(edit_paths(
        doc.selection.paths_mut(),
        "Save Work Path",
        |set| set.rename(WORK_PATH_NAME, "Saved").map(|_| ()),
    ));
    assert_eq!(step.as_ref().map(|s| s.label), Some("Save Work Path"));
    assert!(doc.selection.paths().work().is_none());
    // An edit that changes nothing gives no step.
    let none = ok(edit_paths(
        doc.selection.paths_mut(),
        "Rename Path",
        |set| set.rename("Saved", "Saved").map(|_| ()),
    ));
    assert!(none.is_none());
    // Delete.
    let step = ok(edit_paths(
        doc.selection.paths_mut(),
        "Delete Path",
        |set| set.delete("Other"),
    ));
    assert!(step.is_some());
    assert!(doc.selection.paths().get("Other").is_none());
}

#[test]
fn the_paths_panel_follows_the_document() {
    let mut ws = build_workspace(&scales());
    let mut doc = session();
    let s = scales();
    assert!(!sync_paths(&mut ws, &s, &doc, None), "nothing to change");
    assert!(ws.paths_state.names.is_empty());
    ok(doc.selection.paths_mut().add("One"));
    ok(doc.selection.paths_mut().ensure_work());
    assert!(sync_paths(&mut ws, &s, &doc, None));
    assert_eq!(
        ws.paths_state.names,
        vec!["One".to_owned(), WORK_PATH_NAME.to_owned()]
    );
    assert_eq!(ws.paths_state.selected, None);
    // Selecting a path highlights it; an empty one disables Make Selection.
    sync_paths(&mut ws, &s, &doc, Some("One"));
    assert_eq!(ws.paths_state.selected.as_deref(), Some("One"));
    assert!(ws.paths_state.selected_empty);
    ok(doc.selection.paths_mut().set_subpaths("One", triangle()));
    sync_paths(&mut ws, &s, &doc, Some("One"));
    assert!(!ws.paths_state.selected_empty);
    // A name that no longer exists selects nothing.
    sync_paths(&mut ws, &s, &doc, Some("Gone"));
    assert_eq!(ws.paths_state.selected, None);
    // Deleting empties the panel back to its empty state.
    ok(doc.selection.paths_mut().delete("One"));
    ok(doc.selection.paths_mut().delete(WORK_PATH_NAME));
    assert!(sync_paths(&mut ws, &s, &doc, None));
    assert!(ws.paths_state.names.is_empty());
}

#[test]
fn the_commands_are_in_the_palette_the_menus_and_the_shortcuts() {
    for (id, command) in [
        (COMMAND_MAKE_SELECTION, PathsCommand::MakeSelection),
        (COMMAND_NEW, PathsCommand::New),
        (COMMAND_DELETE, PathsCommand::Delete),
        (COMMAND_RENAME, PathsCommand::Rename),
        (COMMAND_SAVE_WORK, PathsCommand::SaveWork),
    ] {
        assert_eq!(command_for(id), Some(command));
    }
    assert_eq!(command_for("paths.nope"), None);
    let ids: Vec<String> = crate::palette_commands()
        .into_iter()
        .map(|e| e.id)
        .collect();
    for id in [
        COMMAND_MAKE_SELECTION,
        COMMAND_NEW,
        COMMAND_DELETE,
        COMMAND_RENAME,
        COMMAND_SAVE_WORK,
        "tool.pen",
        crate::COMMAND_FOCUS_PATHS,
        crate::COMMAND_TOGGLE_PATHS,
        crate::COMMAND_CLOSE_PATHS,
        "view.move_paths_up",
        "view.float_paths",
        "view.dock_paths",
    ] {
        assert!(ids.iter().any(|known| known == id), "{id} in the palette");
    }
    let titles: Vec<String> = crate::palette_commands()
        .into_iter()
        .map(|e| e.title)
        .collect();
    assert!(titles.iter().any(|t| t == "Select Pen Tool"));
    // Shortcuts: bare P and Primary+Enter.
    let table = crate::default_shortcuts_for(false);
    let chord = |text: &str| ok(aurora_widgets::shortcut::KeyChord::parse(text));
    assert_eq!(
        table.resolve(chord("P")),
        Some(&crate::AppCommand::SelectTool(Tool::Pen))
    );
    assert_eq!(
        table.resolve(chord("Ctrl+Enter")),
        Some(&crate::AppCommand::MakeSelectionFromPath)
    );
    // The activated command resolves.
    let mut ws = build_workspace(&scales());
    let mut focus = crate::FocusManager::default();
    assert_eq!(
        crate::activate_command(
            &mut ws,
            &mut focus,
            COMMAND_MAKE_SELECTION,
            &mut crate::tests::FakeFileDialog::default()
        ),
        Some(crate::ActivatedCommand::Paths(PathsCommand::MakeSelection))
    );
}

#[test]
fn the_panel_commands_target_the_panel_and_the_layout_heals() {
    let ws = build_workspace(&scales());
    assert_eq!(
        crate::command_target(&ws, crate::COMMAND_FOCUS_PATHS),
        Some(ws.paths.root)
    );
    assert_eq!(
        crate::command_collapse_target(&ws, crate::COMMAND_TOGGLE_PATHS),
        Some(ws.paths)
    );
    assert_eq!(
        crate::command_close_target(&ws, crate::COMMAND_CLOSE_PATHS),
        Some(ws.paths)
    );
    let saved = crate::saved_dock(&ws.dock_arrangement());
    assert!(
        saved
            .iter()
            .any(|slot| slot.panels.iter().any(|key| key == "paths"))
    );
    let (back, repaired) = crate::arrangement_from_saved(&saved, &[]);
    assert!(!repaired);
    assert_eq!(back, ws.dock_arrangement());
    // A layout saved before Paths existed (no "paths" key) gets it back.
    let old: Vec<_> = saved
        .into_iter()
        .map(|mut slot| {
            slot.panels.retain(|key| key != "paths");
            slot
        })
        .collect();
    let (healed, repaired) = crate::arrangement_from_saved(&old, &[]);
    assert!(repaired);
    assert_eq!(healed, ws.dock_arrangement());
}

#[test]
fn quick_mask_refuses_the_pen_and_make_selection() {
    assert_eq!(
        crate::quick_mask::tool_refusal(Tool::Pen),
        Some(crate::LockRefusal::QuickMaskToolUnavailable)
    );
    let mut context = crate::menu_model::MenuContext::default();
    assert!(crate::menu_model::enabled(COMMAND_MAKE_SELECTION, &context));
    context.quick_mask = true;
    assert!(!crate::menu_model::enabled(
        COMMAND_MAKE_SELECTION,
        &context
    ));
}

#[test]
fn a_notice_shows_its_own_sentence_and_a_psd_save_names_paths() {
    assert_eq!(
        crate::LockRefusal::Path(EMPTY_PATH_NOTICE).message(),
        EMPTY_PATH_NOTICE
    );
    let mut set = aurora_doc::SelectionSet::new();
    assert!(crate::psd_save::paths_loss_line(&set).is_none());
    let mut paths = PathSet::new();
    ok(paths.add("A"));
    set.set_paths(paths);
    assert!(crate::psd_save::paths_loss_line(&set).is_some_and(|line| line.contains("Paths")));
}
