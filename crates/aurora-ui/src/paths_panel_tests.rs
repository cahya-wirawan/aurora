#![allow(clippy::float_cmp, clippy::indexing_slicing)]

use super::{
    MAKE_SELECTION_EMPTY, MAKE_SELECTION_UNAVAILABLE, NO_PATHS_TEXT, PATH_DELETE_UNAVAILABLE,
    PATHS_TOOLBAR_LABEL, PathsAction, PathsFacts, is_paths_toolbar_button, populate_paths_panel,
    select_path_row, set_paths_toolbar_hover, sync_paths_panel, sync_paths_toolbar,
};
use crate::workspace::{Workspace, build_workspace, select_panel_tab};
use aurora_widgets::WidgetId;
use aurora_widgets::widgets::WidgetKind;

fn scales() -> aurora_theme::Scales {
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
    match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn populate(ws: &mut Workspace, list: &[&str]) {
    let scales = scales();
    if let Err(err) = populate_paths_panel(
        &mut ws.tree,
        ws.paths,
        &scales,
        &mut ws.paths_state,
        &names(list),
    ) {
        unreachable!("{err:?}");
    }
}

fn label_of(ws: &Workspace, id: WidgetId) -> String {
    ws.tree
        .accessibility(id)
        .and_then(accesskit::Node::label)
        .unwrap_or_default()
        .to_owned()
}

fn description_of(ws: &Workspace, id: WidgetId) -> String {
    ws.tree
        .accessibility(id)
        .and_then(accesskit::Node::description)
        .unwrap_or_default()
        .to_owned()
}

fn disabled(ws: &Workspace, action: PathsAction) -> bool {
    let Some(button) = ws.paths_state.toolbar.button_for(action) else {
        unreachable!("a button for {action:?}");
    };
    matches!(ws.tree.payload(button), Some(WidgetKind::Button(s)) if s.disabled)
}

fn body_labels(ws: &Workspace) -> Vec<WidgetId> {
    ws.tree
        .children(ws.paths.body)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&id| matches!(ws.tree.payload(id), Some(WidgetKind::Label(_))))
        .collect()
}

#[test]
fn paths_is_the_fourth_tab_and_starts_with_the_empty_state() {
    let mut ws = build_workspace(&scales());
    let Some(group) = ws.group_of(ws.paths).cloned() else {
        unreachable!("Paths is grouped by default");
    };
    assert_eq!(group.index_of(ws.paths), Some(3));
    assert_eq!(group.index_of(ws.channels), Some(2));
    assert_eq!(label_of(&ws, ws.paths.root), "Paths");
    assert_eq!(ws.tree.bounds(ws.paths.root).map(|b| b.height), Some(0));
    let paths = ws.paths;
    assert_eq!(select_panel_tab(&mut ws, paths).ok(), Some(true));
    ws.tree.compute_layout(1000.0, 800.0);
    assert!(ws.paths_state.rows.is_empty());
    let labels = body_labels(&ws);
    assert_eq!(labels.len(), 1);
    let texts: Vec<String> = labels
        .iter()
        .filter_map(|&id| match ws.tree.payload(id) {
            Some(WidgetKind::Label(state)) => Some(state.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [NO_PATHS_TEXT]);
    assert!(
        labels.iter().all(|&label| ws
            .tree
            .accessibility(label)
            .is_some_and(|node| !node.supports_action(accesskit::Action::Focus)
                && !node.supports_action(accesskit::Action::Click))),
        "the empty state is not interactive"
    );
}

#[test]
fn a_row_per_name_in_order_and_each_is_a_focusable_laid_out_node() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, &["Outline", "Work Path", "Eye"]);
    let paths = ws.paths;
    assert_eq!(select_panel_tab(&mut ws, paths).ok(), Some(true));
    ws.tree.compute_layout(1000.0, 800.0);
    let rows = ws.paths_state.rows.clone();
    let labels: Vec<String> = rows.iter().map(|(id, _)| label_of(&ws, *id)).collect();
    assert_eq!(labels, ["Outline", "Work Path", "Eye"]);
    assert!(
        body_labels(&ws).is_empty(),
        "no empty-state label with rows"
    );
    for (id, name) in &rows {
        let Some(rect) = ws.tree.bounds(*id) else {
            unreachable!("laid out");
        };
        assert!(rect.width > 0 && rect.height > 0, "{name}: {rect:?}");
        let Some(node) = ws.tree.accessibility(*id) else {
            unreachable!("every row has a node");
        };
        assert!(node.supports_action(accesskit::Action::Focus));
        assert!(description_of(&ws, *id).starts_with("Path."));
    }
    assert_eq!(ws.paths_state.names, ["Outline", "Work Path", "Eye"]);
}

#[test]
fn selecting_a_row_highlights_it_and_enables_the_toolbar_with_reasons() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, &["A", "B"]);
    let toolbar = ws.paths_state.toolbar;
    let (Some(make), Some(delete), Some(new)) = (
        toolbar.button_for(PathsAction::MakeSelection),
        toolbar.button_for(PathsAction::DeletePath),
        toolbar.button_for(PathsAction::NewPath),
    ) else {
        unreachable!("three buttons");
    };
    assert!(disabled(&ws, PathsAction::MakeSelection));
    assert!(disabled(&ws, PathsAction::DeletePath));
    assert!(!disabled(&ws, PathsAction::NewPath), "New is always there");
    assert_eq!(description_of(&ws, make), MAKE_SELECTION_UNAVAILABLE);
    assert_eq!(description_of(&ws, delete), PATH_DELETE_UNAVAILABLE);
    assert_eq!(description_of(&ws, new), PathsAction::NewPath.description());

    ws.paths_state.facts = PathsFacts::default();
    let mut state = ws.paths_state.clone();
    assert!(select_path_row(&mut ws.tree, &mut state, Some("B")).is_ok());
    ws.paths_state = state;
    assert_eq!(ws.paths_state.selected.as_deref(), Some("B"));
    assert!(!disabled(&ws, PathsAction::MakeSelection));
    assert!(!disabled(&ws, PathsAction::DeletePath));
    assert_eq!(
        description_of(&ws, make),
        PathsAction::MakeSelection.description()
    );
    let selected_flags: Vec<bool> = ws
        .paths_state
        .rows
        .iter()
        .map(|(id, _)| {
            ws.tree
                .accessibility(*id)
                .and_then(accesskit::Node::is_selected)
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(selected_flags, [false, true]);

    // An empty selected path cannot make a selection; Delete still works.
    assert_eq!(
        sync_paths_toolbar(
            &mut ws.tree,
            &toolbar,
            true,
            PathsFacts {
                selected_empty: true,
                ..PathsFacts::default()
            }
        )
        .ok(),
        Some(true)
    );
    assert!(disabled(&ws, PathsAction::MakeSelection));
    assert!(!disabled(&ws, PathsAction::DeletePath));
    assert_eq!(description_of(&ws, make), MAKE_SELECTION_EMPTY);
    // A row that is not shown selects nothing and disables both again.
    let mut state = ws.paths_state.clone();
    assert!(select_path_row(&mut ws.tree, &mut state, Some("Gone")).is_ok());
    ws.paths_state = state;
    assert_eq!(ws.paths_state.selected, None);
    assert!(disabled(&ws, PathsAction::MakeSelection));
    assert!(disabled(&ws, PathsAction::DeletePath));
}

#[test]
fn sync_rebuilds_on_a_name_change_and_only_rehighlights_on_a_selection_change() {
    let mut ws = build_workspace(&scales());
    let scales = scales();
    let run = |ws: &mut Workspace, list: &[&str], sel: Option<&str>, empty: bool| {
        let mut state = ws.paths_state.clone();
        let out = sync_paths_panel(
            &mut ws.tree,
            ws.paths,
            &scales,
            &mut state,
            &names(list),
            sel,
            PathsFacts {
                selected_empty: empty,
                ..PathsFacts::default()
            },
        );
        ws.paths_state = state;
        out.ok()
    };
    assert_eq!(run(&mut ws, &[], None, false), Some(false), "nothing new");
    assert_eq!(run(&mut ws, &["A", "B"], Some("A"), false), Some(true));
    let ids: Vec<WidgetId> = ws.paths_state.rows.iter().map(|(id, _)| *id).collect();
    assert_eq!(ws.paths_state.selected.as_deref(), Some("A"));
    assert_eq!(run(&mut ws, &["A", "B"], Some("A"), false), Some(false));
    // A selection change keeps every row id.
    assert_eq!(run(&mut ws, &["A", "B"], Some("B"), false), Some(true));
    let after: Vec<WidgetId> = ws.paths_state.rows.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, after);
    assert_eq!(ws.paths_state.selected.as_deref(), Some("B"));
    // The emptiness flag alone reaches the toolbar.
    assert_eq!(run(&mut ws, &["A", "B"], Some("B"), true), Some(true));
    assert!(disabled(&ws, PathsAction::MakeSelection));
    assert_eq!(run(&mut ws, &["A", "B"], Some("B"), false), Some(true));
    assert!(!disabled(&ws, PathsAction::MakeSelection));
    // A rename rebuilds.
    assert_eq!(run(&mut ws, &["A", "C"], Some("C"), false), Some(true));
    assert_eq!(ws.paths_state.names, ["A", "C"]);
    // Back to nothing: the empty state returns and the selection is gone.
    assert_eq!(run(&mut ws, &[], None, false), Some(true));
    assert_eq!(body_labels(&ws).len(), 1);
    assert!(disabled(&ws, PathsAction::DeletePath));
}

#[test]
fn a_hit_resolves_to_its_row_and_arrows_walk_the_rows() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, &["A", "B", "C"]);
    let state = ws.paths_state.clone();
    let Some(b) = state.id_of("B") else {
        unreachable!("a B row");
    };
    assert_eq!(state.row_at(&ws.tree, b), Some("B"));
    for &child in ws.tree.children(b).unwrap_or_default() {
        assert_eq!(state.row_at(&ws.tree, child), Some("B"), "a part of a row");
    }
    assert_eq!(state.row_at(&ws.tree, ws.paths.root), None);
    assert_eq!(state.name_of(b), Some("B"));
    let ids: Vec<WidgetId> = state.rows.iter().map(|(id, _)| *id).collect();
    assert_eq!(state.neighbor(ids[0], false), None);
    assert_eq!(state.neighbor(ids[0], true), Some(ids[1]));
    assert_eq!(state.neighbor(ids[2], true), None);
    assert_eq!(state.neighbor(ids[2], false), Some(ids[1]));
}

#[test]
fn the_toolbar_has_three_named_buttons_that_hover_and_are_not_other_toolbars() {
    let mut ws = build_workspace(&scales());
    let toolbar = ws.paths_state.toolbar;
    assert_eq!(ws.tree.parent(toolbar.root), Some(ws.paths.root));
    let Some(node) = ws.tree.accessibility(toolbar.root) else {
        unreachable!("a node");
    };
    assert_eq!(node.role(), accesskit::Role::Toolbar);
    assert_eq!(node.label(), Some(PATHS_TOOLBAR_LABEL));
    for (action, expected) in [
        (PathsAction::MakeSelection, "Make selection from path"),
        (PathsAction::FillPath, "Fill path"),
        (PathsAction::StrokePath, "Stroke path"),
        (PathsAction::MakeWorkPath, "Make work path from selection"),
        (PathsAction::NewPath, "New path"),
        (PathsAction::DeletePath, "Delete path"),
    ] {
        let Some(id) = toolbar.button_for(action) else {
            unreachable!("a button for {action:?}");
        };
        assert_eq!(label_of(&ws, id), expected);
        assert!(is_paths_toolbar_button(&ws.tree, id));
        assert_eq!(toolbar.action_for(id), Some(action));
        assert!(!crate::is_channels_toolbar_button(&ws.tree, id));
        assert!(!crate::is_layer_toolbar_button(&ws.tree, id));
    }
    let Some(new) = toolbar.button_for(PathsAction::NewPath) else {
        unreachable!("a button");
    };
    assert_eq!(
        set_paths_toolbar_hover(&mut ws.tree, &toolbar, Some(new)).ok(),
        Some(true)
    );
    assert_eq!(
        set_paths_toolbar_hover(&mut ws.tree, &toolbar, None).ok(),
        Some(true)
    );
}

#[test]
fn the_toolbar_sits_under_the_rows_inside_the_panel_when_laid_out() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, &["A", "B", "C"]);
    let paths = ws.paths;
    assert_eq!(select_panel_tab(&mut ws, paths).ok(), Some(true));
    ws.tree.compute_layout(1000.0, 800.0);
    let (Some(body), Some(bar), Some(panel)) = (
        ws.tree.bounds(ws.paths.body),
        ws.tree.bounds(ws.paths_state.toolbar.root),
        ws.tree.bounds(ws.paths.root),
    ) else {
        unreachable!("laid out");
    };
    assert!(
        bar.y >= body.y + i64::from(body.height) - 1,
        "under the list"
    );
    assert!(
        bar.y + i64::from(bar.height) <= panel.y + i64::from(panel.height),
        "inside the panel"
    );
    assert!(bar.height > 0);
}

#[test]
fn closing_and_reopening_the_panel_rebuilds_its_emptied_body() {
    let mut ws = build_workspace(&scales());
    let scales = scales();
    let paths = ws.paths;
    assert!(crate::close_workspace_panel(&mut ws, paths).is_ok());
    assert!(
        ws.tree
            .children(ws.paths.body)
            .unwrap_or_default()
            .is_empty(),
        "a close empties the body"
    );
    assert!(crate::toggle_workspace_panel(&mut ws, paths).is_ok());
    let mut state = ws.paths_state.clone();
    let out = sync_paths_panel(
        &mut ws.tree,
        ws.paths,
        &scales,
        &mut state,
        &[],
        None,
        PathsFacts::default(),
    );
    ws.paths_state = state;
    assert_eq!(out.ok(), Some(true), "the empty state came back");
    assert_eq!(body_labels(&ws).len(), 1);
    // And with rows.
    populate(&mut ws, &["A"]);
    assert!(crate::close_workspace_panel(&mut ws, paths).is_ok());
    assert!(crate::toggle_workspace_panel(&mut ws, paths).is_ok());
    let mut state = ws.paths_state.clone();
    let out = sync_paths_panel(
        &mut ws.tree,
        ws.paths,
        &scales,
        &mut state,
        &names(&["A"]),
        None,
        PathsFacts::default(),
    );
    ws.paths_state = state;
    assert_eq!(out.ok(), Some(true));
    assert!(
        ws.paths_state
            .rows
            .iter()
            .all(|(id, _)| ws.tree.contains(*id))
    );
    assert_eq!(ws.paths_state.rows.len(), 1);
}

#[test]
fn fill_stroke_and_make_path_follow_their_facts() {
    use super::{
        FILL_TOO_SMALL, FILL_UNAVAILABLE, MAKE_PATH_UNAVAILABLE, STROKE_TOO_SMALL,
        STROKE_UNAVAILABLE,
    };
    let mut ws = build_workspace(&scales());
    let toolbar = ws.paths_state.toolbar;
    let describe = |ws: &Workspace, a: PathsAction| {
        toolbar
            .button_for(a)
            .map(|id| description_of(ws, id))
            .unwrap_or_default()
    };
    // Nothing selected: all three off, each with its own reason.
    assert!(disabled(&ws, PathsAction::FillPath));
    assert!(disabled(&ws, PathsAction::StrokePath));
    assert!(disabled(&ws, PathsAction::MakeWorkPath));
    assert_eq!(describe(&ws, PathsAction::FillPath), FILL_UNAVAILABLE);
    assert_eq!(describe(&ws, PathsAction::StrokePath), STROKE_UNAVAILABLE);
    assert_eq!(
        describe(&ws, PathsAction::MakeWorkPath),
        MAKE_PATH_UNAVAILABLE
    );
    let sync = |ws: &mut Workspace, selected: bool, facts: PathsFacts| {
        assert!(sync_paths_toolbar(&mut ws.tree, &toolbar, selected, facts).is_ok());
    };
    // A selected path with two points can be stroked, not filled.
    sync(
        &mut ws,
        true,
        PathsFacts {
            can_stroke: true,
            ..PathsFacts::default()
        },
    );
    assert!(disabled(&ws, PathsAction::FillPath));
    assert!(!disabled(&ws, PathsAction::StrokePath));
    assert_eq!(describe(&ws, PathsAction::FillPath), FILL_TOO_SMALL);
    // Three points can do both; the reason text follows.
    sync(
        &mut ws,
        true,
        PathsFacts {
            can_fill: true,
            can_stroke: true,
            ..PathsFacts::default()
        },
    );
    assert!(!disabled(&ws, PathsAction::FillPath));
    assert_eq!(
        describe(&ws, PathsAction::FillPath),
        PathsAction::FillPath.description()
    );
    // Capable facts without a selected path still disable both.
    sync(
        &mut ws,
        false,
        PathsFacts {
            can_fill: true,
            can_stroke: true,
            ..PathsFacts::default()
        },
    );
    assert!(disabled(&ws, PathsAction::FillPath));
    assert!(disabled(&ws, PathsAction::StrokePath));
    assert_eq!(describe(&ws, PathsAction::StrokePath), STROKE_UNAVAILABLE);
    // Make Work Path needs only a selection, not a path.
    sync(
        &mut ws,
        false,
        PathsFacts {
            has_selection: true,
            ..PathsFacts::default()
        },
    );
    assert!(!disabled(&ws, PathsAction::MakeWorkPath));
    assert!(!disabled(&ws, PathsAction::NewPath));
    sync(
        &mut ws,
        true,
        PathsFacts {
            can_fill: true,
            ..PathsFacts::default()
        },
    );
    assert_eq!(describe(&ws, PathsAction::StrokePath), STROKE_TOO_SMALL);
}

/// The icon kind drawn inside a toolbar button (its `ButtonState::icon`).
fn drawn_icon(ws: &Workspace, button: WidgetId) -> Option<aurora_widgets::icons::IconKind> {
    match ws.tree.payload(button) {
        Some(WidgetKind::Button(state)) => state.icon.map(|(kind, _)| kind),
        _ => None,
    }
}

/// 0.257.0: Fill, Stroke and Make Work Path have glyphs of their own, and
/// the button really draws the glyph its action names.
#[test]
fn the_path_buttons_draw_their_own_glyphs_not_a_tools_reused_one() {
    use aurora_widgets::icons::IconKind;
    let mut ws = build_workspace(&scales());
    populate(&mut ws, &["Work Path"]);
    let expected = [
        (PathsAction::MakeSelection, IconKind::Marquee),
        (PathsAction::FillPath, IconKind::FillPath),
        (PathsAction::StrokePath, IconKind::StrokePath),
        (PathsAction::MakeWorkPath, IconKind::MakePath),
        (PathsAction::NewPath, IconKind::NewLayer),
        (PathsAction::DeletePath, IconKind::Delete),
    ];
    for (action, kind) in expected {
        assert_eq!(action.icon(), kind, "{action:?}");
        let Some(button) = ws.paths_state.toolbar.button_for(action) else {
            unreachable!("{action:?} has a button");
        };
        assert_eq!(drawn_icon(&ws, button), Some(kind), "{action:?} draws it");
    }
    for action in [
        PathsAction::FillPath,
        PathsAction::StrokePath,
        PathsAction::MakeWorkPath,
    ] {
        assert!(
            ![IconKind::PaintBucket, IconKind::Brush, IconKind::Pen].contains(&action.icon()),
            "{action:?} no longer reuses a tool's glyph"
        );
    }
}
