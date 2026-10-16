#![allow(clippy::float_cmp, clippy::indexing_slicing)]

use super::{
    CHANNELS_TOOLBAR_LABEL, ChannelRow, ChannelRowPart, ChannelVisibility, ChannelsAction,
    CompositeChannel, NO_SAVED_SELECTIONS_TEXT, eye_is_on, is_channels_toolbar_button,
    populate_channels_panel, select_channel_row, set_channels_toolbar_hover, sync_channel_eyes,
    sync_channels_panel,
};
use crate::workspace::{Workspace, build_workspace, select_panel_tab};
use aurora_widgets::WidgetId;
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{IconAccess, WidgetKind, icon_state};

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

fn eye_of(ws: &Workspace, channel: CompositeChannel) -> WidgetId {
    let Some(row) = ws.channels_state.id_of(&ChannelRow::Composite(channel)) else {
        unreachable!("{channel:?} has a row");
    };
    for &child in ws.tree.children(row).unwrap_or_default() {
        if matches!(ws.tree.payload(child), Some(WidgetKind::Icon(_))) {
            return child;
        }
    }
    unreachable!("{channel:?} has an eye");
}

fn populate(ws: &mut Workspace, visibility: ChannelVisibility, list: &[&str]) {
    let scales = scales();
    if let Err(err) = populate_channels_panel(
        &mut ws.tree,
        ws.channels,
        &scales,
        &mut ws.channels_state,
        visibility,
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

fn body_labels(ws: &Workspace) -> Vec<WidgetId> {
    ws.tree
        .children(ws.channels.body)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&id| matches!(ws.tree.payload(id), Some(WidgetKind::Label(_))))
        .collect()
}

#[test]
fn the_visibility_model_toggles_channels_and_the_rgb_row_as_a_group() {
    let all = ChannelVisibility::default();
    assert_eq!(all, ChannelVisibility::ALL_ON);
    assert!(all.all_on());
    assert_eq!(all.gain(), [1.0, 1.0, 1.0]);
    let no_green = all.toggled(CompositeChannel::Green);
    assert_eq!(no_green.gain(), [1.0, 0.0, 1.0]);
    assert!(!no_green.is_on(CompositeChannel::Green));
    assert!(
        !no_green.is_on(CompositeChannel::Rgb),
        "RGB reads on only when all three are"
    );
    assert!(no_green.is_on(CompositeChannel::Red));
    // RGB with one hidden shows all three; with all shown hides all three.
    assert_eq!(no_green.toggled(CompositeChannel::Rgb), all);
    let none = all.toggled(CompositeChannel::Rgb);
    assert_eq!(none.gain(), [0.0, 0.0, 0.0]);
    assert!(!none.is_on(CompositeChannel::Rgb));
    assert_eq!(none.toggled(CompositeChannel::Rgb), all);
    // Only Red on: red is the only channel displayed.
    let only_red = none.toggled(CompositeChannel::Red);
    assert_eq!(only_red.gain(), [1.0, 0.0, 0.0]);
    // Each colour channel flips on its own.
    assert_eq!(only_red.toggled(CompositeChannel::Red), none);
    assert_eq!(none.toggled(CompositeChannel::Blue).gain(), [0.0, 0.0, 1.0]);
}

#[test]
fn channels_is_the_third_tab_with_four_composite_rows_and_the_empty_state() {
    let mut ws = build_workspace(&scales());
    let Some(group) = ws.group_of(ws.channels).cloned() else {
        unreachable!("Channels is grouped by default");
    };
    assert_eq!(group.index_of(ws.channels), Some(2));
    assert_eq!(group.index_of(ws.history), Some(1));
    assert_eq!(label_of(&ws, ws.channels.root), "Channels");
    // Hidden until its tab is chosen, like History.
    assert_eq!(ws.tree.bounds(ws.channels.root).map(|b| b.height), Some(0));
    let channels = ws.channels;
    assert_eq!(select_panel_tab(&mut ws, channels).ok(), Some(true));
    ws.tree.compute_layout(1000.0, 800.0);
    let rows = ws.channels_state.rows.clone();
    assert_eq!(rows.len(), 4, "no saved selection: just the composite rows");
    let labels: Vec<String> = rows.iter().map(|(id, _)| label_of(&ws, *id)).collect();
    assert_eq!(labels, ["RGB", "Red", "Green", "Blue"]);
    for (id, _) in &rows {
        let Some(rect) = ws.tree.bounds(*id) else {
            unreachable!("laid out");
        };
        assert!(
            rect.width > 0 && rect.height > 0,
            "a hittable row: {rect:?}"
        );
        let Some(node) = ws.tree.accessibility(*id) else {
            unreachable!("every row has a node");
        };
        assert!(node.supports_action(accesskit::Action::Focus));
    }
    // The empty state: one non-interactive label, no extra row.
    let labels = body_labels(&ws);
    assert_eq!(labels.len(), 1);
    let texts: Vec<String> = labels
        .iter()
        .filter_map(|&id| match ws.tree.payload(id) {
            Some(WidgetKind::Label(state)) => Some(state.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, [NO_SAVED_SELECTIONS_TEXT]);
    assert!(
        labels.iter().all(|&label| ws
            .tree
            .accessibility(label)
            .is_some_and(|node| !node.supports_action(accesskit::Action::Focus)
                && !node.supports_action(accesskit::Action::Click))),
        "not interactive"
    );
}

#[test]
fn the_eyes_are_named_toggle_buttons_that_are_not_tab_stops() {
    let ws = build_workspace(&scales());
    for (channel, name) in [
        (CompositeChannel::Rgb, "Hide RGB channel"),
        (CompositeChannel::Red, "Hide Red channel"),
        (CompositeChannel::Green, "Hide Green channel"),
        (CompositeChannel::Blue, "Hide Blue channel"),
    ] {
        let eye = eye_of(&ws, channel);
        let Ok(state) = icon_state(&ws.tree, eye) else {
            unreachable!("an icon");
        };
        assert_eq!(state.kind, IconKind::Eye);
        assert_eq!(
            state.access,
            IconAccess::Button {
                label: name.to_owned(),
                toggled: Some(true)
            }
        );
        let Some(node) = ws.tree.accessibility(eye) else {
            unreachable!("a node");
        };
        assert_eq!(node.role(), accesskit::Role::Button);
        assert!(node.supports_action(accesskit::Action::Click));
        assert!(!node.supports_action(accesskit::Action::Focus));
    }
}

#[test]
fn an_eye_change_re_icons_in_place_and_keeps_every_row_id() {
    let mut ws = build_workspace(&scales());
    let ids: Vec<WidgetId> = ws.channels_state.rows.iter().map(|(id, _)| *id).collect();
    let eyes_before: Vec<WidgetId> = CompositeChannel::ALL
        .iter()
        .map(|&c| eye_of(&ws, c))
        .collect();
    let hidden = ChannelVisibility::default().toggled(CompositeChannel::Red);
    let scales = scales();
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            hidden,
            &[],
        )
        .ok(),
        Some(true)
    );
    let after: Vec<WidgetId> = ws.channels_state.rows.iter().map(|(id, _)| *id).collect();
    assert_eq!(after, ids, "a toggle never rebuilds a row");
    let eyes_after: Vec<WidgetId> = CompositeChannel::ALL
        .iter()
        .map(|&c| eye_of(&ws, c))
        .collect();
    assert_eq!(eyes_after, eyes_before);
    let state = |ws: &Workspace, channel| {
        let eye = eye_of(ws, channel);
        (
            eye_is_on(&ws.tree, eye),
            icon_state(&ws.tree, eye).map(|s| s.kind).ok(),
            label_of(ws, eye),
        )
    };
    assert_eq!(
        state(&ws, CompositeChannel::Red),
        (
            Some(false),
            Some(IconKind::EyeOff),
            "Show Red channel".to_owned()
        )
    );
    assert_eq!(state(&ws, CompositeChannel::Green).0, Some(true));
    assert_eq!(state(&ws, CompositeChannel::Blue).0, Some(true));
    assert_eq!(
        state(&ws, CompositeChannel::Rgb).0,
        Some(false),
        "RGB is on only when all three are"
    );
    // Nothing changed: nothing reported.
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            hidden,
            &[],
        )
        .ok(),
        Some(false)
    );
    // Back to all on.
    assert_eq!(
        sync_channel_eyes(
            &mut ws.tree,
            &mut ws.channels_state,
            ChannelVisibility::ALL_ON
        )
        .ok(),
        Some(true)
    );
    assert_eq!(state(&ws, CompositeChannel::Rgb).0, Some(true));
}

#[test]
fn saved_selections_get_rows_below_the_composite_ones_and_follow_the_names() {
    let mut ws = build_workspace(&scales());
    let scales = scales();
    let visibility = ChannelVisibility::default();
    let two = names(&["Sky", "Sun"]);
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            visibility,
            &two,
        )
        .ok(),
        Some(true),
        "new names rebuild the rows"
    );
    let kinds: Vec<ChannelRow> = ws
        .channels_state
        .rows
        .iter()
        .map(|(_, kind)| kind.clone())
        .collect();
    assert_eq!(
        kinds[4..],
        [
            ChannelRow::Saved("Sky".to_owned()),
            ChannelRow::Saved("Sun".to_owned())
        ]
    );
    let labels: Vec<String> = ws
        .channels_state
        .rows
        .iter()
        .map(|(id, _)| label_of(&ws, *id))
        .collect();
    assert_eq!(labels, ["RGB", "Red", "Green", "Blue", "Sky", "Sun"]);
    assert!(body_labels(&ws).is_empty(), "no empty-state label");
    // A saved row has no eye and describes how to load it.
    let Some(sky) = ws
        .channels_state
        .id_of(&ChannelRow::Saved("Sky".to_owned()))
    else {
        unreachable!("a row for Sky");
    };
    assert!(ws.tree.children(sky).unwrap_or_default().is_empty());
    assert!(
        ws.tree
            .accessibility(sky)
            .and_then(accesskit::Node::description)
            .is_some_and(|text| text.contains("Enter"))
    );
    // The same names change nothing.
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            visibility,
            &two,
        )
        .ok(),
        Some(false)
    );
    // Deleting one rebuilds.
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            visibility,
            &names(&["Sun"]),
        )
        .ok(),
        Some(true)
    );
    assert_eq!(ws.channels_state.rows.len(), 5);
}

#[test]
fn selecting_a_saved_row_enables_delete_and_a_vanished_selection_falls_back_to_rgb() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, ChannelVisibility::default(), &["Sky", "Sun"]);
    let delete_disabled = |ws: &Workspace| {
        let Some(button) = ws.channels_state.toolbar.button_for(ChannelsAction::Delete) else {
            unreachable!("a Delete button");
        };
        matches!(ws.tree.payload(button), Some(WidgetKind::Button(s)) if s.disabled)
    };
    assert_eq!(
        ws.channels_state.selected,
        ChannelRow::Composite(CompositeChannel::Rgb)
    );
    assert!(delete_disabled(&ws), "disabled with no saved row selected");
    let sun = ChannelRow::Saved("Sun".to_owned());
    let mut state = ws.channels_state.clone();
    if let Err(err) = select_channel_row(&mut ws.tree, &mut state, &sun) {
        unreachable!("{err:?}");
    }
    ws.channels_state = state;
    assert_eq!(ws.channels_state.selected_saved(), Some("Sun"));
    assert!(!delete_disabled(&ws), "enabled for a saved row");
    let Some(sun_id) = ws.channels_state.id_of(&sun) else {
        unreachable!("a row for Sun");
    };
    assert!(matches!(
        ws.tree.payload(sun_id),
        Some(WidgetKind::TreeItem(state)) if state.selected
    ));
    // The selection survives a rebuild that keeps the name ...
    populate(&mut ws, ChannelVisibility::default(), &["Sun", "Zed"]);
    assert_eq!(ws.channels_state.selected_saved(), Some("Sun"));
    assert!(!delete_disabled(&ws));
    // ... and falls back to RGB, with Delete disabled, when it is gone.
    populate(&mut ws, ChannelVisibility::default(), &["Zed"]);
    assert_eq!(
        ws.channels_state.selected,
        ChannelRow::Composite(CompositeChannel::Rgb)
    );
    assert!(delete_disabled(&ws), "nothing to delete");
    // Selecting a row that is not shown selects RGB.
    let mut state = ws.channels_state.clone();
    if let Err(err) = select_channel_row(
        &mut ws.tree,
        &mut state,
        &ChannelRow::Saved("Nope".to_owned()),
    ) {
        unreachable!("{err:?}");
    }
    assert_eq!(state.selected, ChannelRow::Composite(CompositeChannel::Rgb));
}

#[test]
fn a_hit_resolves_to_the_row_or_its_eye_and_arrows_walk_the_rows() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, ChannelVisibility::default(), &["Sky"]);
    let state = ws.channels_state.clone();
    let Some(red) = state.id_of(&ChannelRow::Composite(CompositeChannel::Red)) else {
        unreachable!("a Red row");
    };
    let eye = eye_of(&ws, CompositeChannel::Red);
    assert_eq!(
        state.row_part(&ws.tree, eye),
        Some((red, ChannelRowPart::Eye))
    );
    assert_eq!(
        state.row_part(&ws.tree, red),
        Some((red, ChannelRowPart::Row))
    );
    assert_eq!(state.row_part(&ws.tree, ws.channels.root), None);
    let ids: Vec<WidgetId> = state.rows.iter().map(|(id, _)| *id).collect();
    assert_eq!(state.neighbor(ids[0], false), None, "nothing above RGB");
    assert_eq!(state.neighbor(ids[0], true), Some(ids[1]));
    assert_eq!(
        state.neighbor(ids[3], true),
        Some(ids[4]),
        "into the saved list"
    );
    assert_eq!(state.neighbor(ids[4], false), Some(ids[3]));
    assert_eq!(state.neighbor(ids[4], true), None, "nothing below the last");
}

#[test]
fn the_toolbar_has_a_new_and_a_delete_button_that_hover_like_the_layers_ones() {
    let mut ws = build_workspace(&scales());
    let toolbar = ws.channels_state.toolbar;
    assert_eq!(ws.tree.parent(toolbar.root), Some(ws.channels.root));
    let Some(node) = ws.tree.accessibility(toolbar.root) else {
        unreachable!("a node");
    };
    assert_eq!(node.role(), accesskit::Role::Toolbar);
    assert_eq!(node.label(), Some(CHANNELS_TOOLBAR_LABEL));
    for (action, expected) in [
        (
            ChannelsAction::NewFromSelection,
            "New channel from selection",
        ),
        (ChannelsAction::Delete, "Delete channel"),
    ] {
        let Some(id) = toolbar.button_for(action) else {
            unreachable!("a button for {action:?}");
        };
        assert_eq!(label_of(&ws, id), expected);
        assert!(is_channels_toolbar_button(&ws.tree, id));
        assert_eq!(toolbar.action_for(id), Some(action));
    }
    let Some(new) = toolbar.button_for(ChannelsAction::NewFromSelection) else {
        unreachable!("a button");
    };
    assert!(
        !matches!(ws.tree.payload(new), Some(WidgetKind::Button(s)) if s.disabled),
        "New is always available"
    );
    assert!(!crate::is_layer_toolbar_button(&ws.tree, new));
    assert_eq!(
        set_channels_toolbar_hover(&mut ws.tree, &toolbar, Some(new)).ok(),
        Some(true)
    );
    assert_eq!(
        set_channels_toolbar_hover(&mut ws.tree, &toolbar, None).ok(),
        Some(true)
    );
}

#[test]
fn the_toolbar_sits_under_the_rows_inside_the_panel_when_laid_out() {
    let mut ws = build_workspace(&scales());
    populate(&mut ws, ChannelVisibility::default(), &["A", "B", "C"]);
    let channels = ws.channels;
    assert_eq!(select_panel_tab(&mut ws, channels).ok(), Some(true));
    ws.tree.compute_layout(1000.0, 800.0);
    let (Some(body), Some(bar), Some(panel)) = (
        ws.tree.bounds(ws.channels.body),
        ws.tree.bounds(ws.channels_state.toolbar.root),
        ws.tree.bounds(ws.channels.root),
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
fn closing_and_reopening_the_panel_rebuilds_its_emptied_rows() {
    let mut ws = build_workspace(&scales());
    let scales = scales();
    let channels = ws.channels;
    assert!(crate::close_workspace_panel(&mut ws, channels).is_ok());
    assert!(
        ws.tree
            .children(ws.channels.body)
            .unwrap_or_default()
            .is_empty(),
        "a close empties the body"
    );
    assert!(crate::toggle_workspace_panel(&mut ws, channels).is_ok());
    // Same names, same eyes: only the dead rows make the sync rebuild.
    assert_eq!(
        sync_channels_panel(
            &mut ws.tree,
            ws.channels,
            &scales,
            &mut ws.channels_state,
            ChannelVisibility::default(),
            &[],
        )
        .ok(),
        Some(true)
    );
    assert!(
        ws.channels_state
            .rows
            .iter()
            .all(|(id, _)| ws.tree.contains(*id))
    );
    assert_eq!(ws.channels_state.rows.len(), 4);
}

/// 0.257.0: the New channel button draws the channels glyph (it reused the
/// New Layer icon before), Delete keeps its bin.
#[test]
fn the_new_channel_button_draws_the_channels_glyph() {
    assert_eq!(ChannelsAction::NewFromSelection.icon(), IconKind::Channels);
    assert_eq!(ChannelsAction::Delete.icon(), IconKind::Delete);
    let ws = built_workspace_for_icon_test();
    for (action, kind) in [
        (ChannelsAction::NewFromSelection, IconKind::Channels),
        (ChannelsAction::Delete, IconKind::Delete),
    ] {
        let Some(button) = ws.channels_state.toolbar.button_for(action) else {
            unreachable!("{action:?} has a button");
        };
        let drawn = ws
            .tree
            .children(button)
            .unwrap_or_default()
            .iter()
            .find_map(|&child| match ws.tree.payload(child) {
                Some(aurora_widgets::widgets::WidgetKind::Icon(icon)) => Some(icon.kind),
                _ => None,
            });
        assert_eq!(drawn, Some(kind), "{action:?}");
    }
}

fn built_workspace_for_icon_test() -> crate::workspace::Workspace {
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
    match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => crate::workspace::build_workspace(&scales),
        Err(err) => unreachable!("{err:?}"),
    }
}
