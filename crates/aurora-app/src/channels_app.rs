//! The app half of the Channels panel (0.232.0, [`aurora_ui::channels_panel`]).
//!
//! Everything here is a free function over plain state, so it is tested
//! without a window: what the eyes do to a document session (a view
//! setting only: no undo step, never dirty, the composite cache untouched),
//! which key does what on a focused row, the double-click rule, and the
//! per-turn sync that keeps the rows in line with the document's saved
//! selections and eyes. `App` supplies the pointer, focus and command glue.

use std::time::{Duration, Instant};

use aurora_ui::{ChannelRow, ChannelRowPart, CompositeChannel};

use crate::document_session::DocumentSession;
use crate::{Key, Modifiers, NamedKey, WidgetId};

/// Two presses on the same row within this long are a double click.
pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The saved selections' names, sorted — the rows the panel shows.
pub(crate) fn saved_names(set: &aurora_doc::SelectionSet) -> Vec<String> {
    set.saved_sorted()
        .into_iter()
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// An eye pressed (pointer, key or assistive technology): flips that
/// channel of the session's *view* setting. Touches nothing else: no
/// history step, no revision (so never dirty) and no composite
/// invalidation — the shader applies the gain. Returns the new setting.
pub(crate) fn toggle_eye(
    doc: &mut DocumentSession,
    channel: CompositeChannel,
) -> aurora_ui::ChannelVisibility {
    doc.channel_view = doc.channel_view.toggled(channel);
    doc.channel_view
}

/// Whether a press on `row` at `now` completes a double click begun by
/// `previous` (the row and time of the last press on a saved row).
pub(crate) fn is_double_click(
    previous: Option<(WidgetId, Instant)>,
    row: WidgetId,
    now: Instant,
) -> bool {
    previous.is_some_and(|(id, at)| {
        id == row
            && now
                .checked_duration_since(at)
                .is_some_and(|d| d <= DOUBLE_CLICK)
    })
}

/// What a key means on a focused Channels row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelKey {
    /// `Enter`.
    Enter,
    /// `Space`.
    Space,
    Previous,
    Next,
}

/// The pure decision behind `App::channels_row_key`: nothing while a modal
/// is open or with `Ctrl`/`Alt`/`Meta` held (a shortcut chord).
pub(crate) fn key_action(key: Key, modifiers: Modifiers, modal: bool) -> Option<ChannelKey> {
    if modal || modifiers.control || modifiers.alt || modifiers.meta {
        return None;
    }
    let Key::Named(named) = key else {
        return None;
    };
    match named {
        NamedKey::Enter => Some(ChannelKey::Enter),
        NamedKey::Space => Some(ChannelKey::Space),
        NamedKey::ArrowUp => Some(ChannelKey::Previous),
        NamedKey::ArrowDown => Some(ChannelKey::Next),
        _ => None,
    }
}

/// What an activation does to the document or view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowEffect {
    /// Flip this channel's eye.
    ToggleEye(CompositeChannel),
    /// Load this saved selection as the selection.
    Load(String),
    /// Select the row only.
    Select,
    /// Move focus to the neighbouring row.
    Move(bool),
}

/// The effect of `key` on `row`: `Enter` and `Space` toggle a composite
/// row's eye; `Enter` loads a saved row as the selection and `Space`
/// selects it; the arrows move.
pub(crate) fn key_effect(row: &ChannelRow, key: ChannelKey) -> RowEffect {
    match (row, key) {
        (_, ChannelKey::Previous) => RowEffect::Move(false),
        (_, ChannelKey::Next) => RowEffect::Move(true),
        (ChannelRow::Composite(channel), ChannelKey::Enter | ChannelKey::Space) => {
            RowEffect::ToggleEye(*channel)
        }
        (ChannelRow::Saved(name), ChannelKey::Enter) => RowEffect::Load(name.clone()),
        (ChannelRow::Saved(_), ChannelKey::Space) => RowEffect::Select,
    }
}

/// The effect of a pointer press (or an assistive technology's `Click`
/// when `assistive`) on `part` of `row`. `double` is a double click. An
/// eye toggles; a saved row loads on a double click or an assistive
/// `Click` (its default action) and is otherwise selected; a composite
/// row is selected.
pub(crate) fn press_effect(
    row: &ChannelRow,
    part: ChannelRowPart,
    double: bool,
    assistive: bool,
) -> RowEffect {
    match (row, part) {
        (ChannelRow::Composite(channel), ChannelRowPart::Eye) => RowEffect::ToggleEye(*channel),
        (ChannelRow::Saved(name), _) if double || assistive => RowEffect::Load(name.clone()),
        _ => RowEffect::Select,
    }
}

/// Keeps the Channels panel current for `doc`: rebuilds its rows when the
/// saved selections changed, re-icons the eyes when the view setting did.
/// Cheap when nothing changed; run every event-loop turn. Returns whether
/// anything changed (the caller lays out again).
pub(crate) fn sync_channels(
    workspace: &mut aurora_ui::Workspace,
    scales: &aurora_theme::Scales,
    doc: &DocumentSession,
) -> bool {
    let names = saved_names(&doc.selection);
    let panel = workspace.channels;
    match aurora_ui::sync_channels_panel(
        &mut workspace.tree,
        panel,
        scales,
        &mut workspace.channels_state,
        doc.channel_view,
        &names,
    ) {
        Ok(changed) => changed,
        Err(err) => {
            tracing::warn!(?err, "failed to refresh the Channels panel");
            false
        }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::float_cmp)]
mod tests {
    use super::{
        ChannelKey, RowEffect, is_double_click, key_action, key_effect, press_effect, saved_names,
        sync_channels, toggle_eye,
    };
    use crate::document_session::{DocumentContents, DocumentSession};
    use crate::{Key, Modifiers, NamedKey, selection_store};
    use aurora_core::Rect;
    use aurora_doc::Selection;
    use aurora_ui::{
        ChannelRow, ChannelRowPart, ChannelVisibility, CompositeChannel, build_workspace,
    };
    use std::time::{Duration, Instant};

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

    fn square() -> Selection {
        Selection::new(Rect {
            x: 1,
            y: 1,
            width: 8,
            height: 8,
        })
    }

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn plain() -> Modifiers {
        Modifiers::default()
    }

    /// A channel eye is a view setting: no history step, no revision
    /// (never dirty), no composite invalidation, nothing saved.
    #[test]
    fn an_eye_toggle_changes_the_view_and_nothing_of_the_document() {
        let mut doc = session();
        let revision = doc.undo_order.revision;
        let content = doc.undo_order.content_revision;
        let stamp = doc.composite_cache.stamp;
        let steps = doc.undo_order.undo.len();
        assert!(!doc.is_dirty());
        assert_eq!(doc.channel_view, ChannelVisibility::ALL_ON);
        let after = toggle_eye(&mut doc, CompositeChannel::Green);
        assert_eq!(after.gain(), [1.0, 0.0, 1.0]);
        assert_eq!(doc.channel_view, after);
        assert!(!doc.is_dirty(), "never dirty");
        assert_eq!(doc.undo_order.revision, revision);
        assert_eq!(doc.undo_order.content_revision, content);
        assert_eq!(doc.undo_order.undo.len(), steps, "no undo step");
        assert!(doc.undo_order.redo.is_empty());
        assert_eq!(doc.composite_cache.stamp, stamp, "no recomposite");
        // RGB with one hidden shows all three again.
        assert_eq!(
            toggle_eye(&mut doc, CompositeChannel::Rgb),
            ChannelVisibility::ALL_ON
        );
        // A new session shows all three again (per-document view).
        let mut other = session();
        toggle_eye(&mut other, CompositeChannel::Red);
        assert_eq!(session().channel_view, ChannelVisibility::ALL_ON);
        assert_ne!(other.channel_view, session().channel_view);
    }

    #[test]
    fn the_rows_follow_the_documents_saved_selections() {
        let mut ws = build_workspace(&scales());
        let scales = scales();
        let mut doc = session();
        assert!(
            !sync_channels(&mut ws, &scales, &doc),
            "nothing to change at first"
        );
        doc.selection.select(square());
        ok(doc.selection.save_active("Sky"));
        doc.selection.select(square());
        ok(doc.selection.save_active("Boat"));
        assert_eq!(saved_names(&doc.selection), ["Boat", "Sky"]);
        assert!(sync_channels(&mut ws, &scales, &doc));
        let kinds: Vec<ChannelRow> = ws
            .channels_state
            .rows
            .iter()
            .map(|(_, kind)| kind.clone())
            .collect();
        assert_eq!(
            kinds[4..],
            [
                ChannelRow::Saved("Boat".to_owned()),
                ChannelRow::Saved("Sky".to_owned())
            ]
        );
        // The eyes follow the document's view setting.
        toggle_eye(&mut doc, CompositeChannel::Blue);
        assert!(sync_channels(&mut ws, &scales, &doc));
        assert!(!ws.channels_state.visibility.blue);
        // A delete drops the row on the next sync; the empty state returns
        // once the last is gone.
        assert_eq!(
            selection_store::delete(&mut doc.undo_order, &mut doc.selection, "Sky"),
            selection_store::StoreOutcome::MetadataChanged
        );
        assert!(sync_channels(&mut ws, &scales, &doc));
        assert_eq!(ws.channels_state.rows.len(), 5);
        ok(doc.selection.delete_saved("Boat"));
        assert!(sync_channels(&mut ws, &scales, &doc));
        assert_eq!(ws.channels_state.rows.len(), 4);
        // Another document with the same names but other eyes: eyes only.
        let other = session();
        assert!(sync_channels(&mut ws, &scales, &other));
        assert!(ws.channels_state.visibility.all_on());
    }

    /// The load action goes through the existing Load Selection path: one
    /// undo step, and the saved selection becomes the active one.
    #[test]
    fn loading_a_saved_row_records_one_selection_step_through_the_store() {
        let mut ws = build_workspace(&scales());
        let mut doc = session();
        doc.selection.select(square());
        ok(doc.selection.save_active("Sky"));
        doc.selection.deselect();
        let row = ChannelRow::Saved("Sky".to_owned());
        let RowEffect::Load(name) = press_effect(&row, ChannelRowPart::Row, true, false) else {
            unreachable!("a double click loads");
        };
        let steps = doc.undo_order.undo.len();
        let outcome = selection_store::load(
            &mut ws,
            &mut doc.undo_order,
            &mut doc.history,
            &mut doc.pixel_history,
            &mut doc.selection,
            &name,
        );
        assert_eq!(outcome, selection_store::StoreOutcome::Loaded);
        assert_eq!(doc.undo_order.undo.len(), steps + 1, "ONE undo step");
        assert!(doc.selection.active().is_some());
    }

    #[test]
    fn presses_keys_and_clicks_resolve_to_the_right_effects() {
        let red = ChannelRow::Composite(CompositeChannel::Red);
        let sky = ChannelRow::Saved("Sky".to_owned());
        // Pointer.
        assert_eq!(
            press_effect(&red, ChannelRowPart::Eye, false, false),
            RowEffect::ToggleEye(CompositeChannel::Red)
        );
        assert_eq!(
            press_effect(&red, ChannelRowPart::Row, true, false),
            RowEffect::Select,
            "a composite row is never loaded"
        );
        assert_eq!(
            press_effect(&sky, ChannelRowPart::Row, false, false),
            RowEffect::Select,
            "a single click only selects"
        );
        assert_eq!(
            press_effect(&sky, ChannelRowPart::Row, true, false),
            RowEffect::Load("Sky".to_owned())
        );
        // Assistive technology: the default action loads.
        assert_eq!(
            press_effect(&sky, ChannelRowPart::Row, false, true),
            RowEffect::Load("Sky".to_owned())
        );
        assert_eq!(
            press_effect(&red, ChannelRowPart::Eye, false, true),
            RowEffect::ToggleEye(CompositeChannel::Red)
        );
        // Keys.
        assert_eq!(
            key_effect(&red, ChannelKey::Space),
            RowEffect::ToggleEye(CompositeChannel::Red)
        );
        assert_eq!(
            key_effect(&red, ChannelKey::Enter),
            RowEffect::ToggleEye(CompositeChannel::Red)
        );
        assert_eq!(
            key_effect(&sky, ChannelKey::Enter),
            RowEffect::Load("Sky".to_owned())
        );
        assert_eq!(key_effect(&sky, ChannelKey::Space), RowEffect::Select);
        assert_eq!(key_effect(&sky, ChannelKey::Next), RowEffect::Move(true));
        assert_eq!(
            key_effect(&red, ChannelKey::Previous),
            RowEffect::Move(false)
        );
        // Key decoding: plain keys only, never under a modal or a chord.
        let key = |named| Key::Named(named);
        assert_eq!(
            key_action(key(NamedKey::Space), plain(), false),
            Some(ChannelKey::Space)
        );
        assert_eq!(
            key_action(key(NamedKey::Enter), plain(), false),
            Some(ChannelKey::Enter)
        );
        assert_eq!(
            key_action(key(NamedKey::ArrowUp), plain(), false),
            Some(ChannelKey::Previous)
        );
        assert_eq!(
            key_action(key(NamedKey::ArrowDown), plain(), false),
            Some(ChannelKey::Next)
        );
        assert_eq!(key_action(key(NamedKey::Space), plain(), true), None);
        let ctrl = Modifiers {
            control: true,
            ..plain()
        };
        assert_eq!(key_action(key(NamedKey::Enter), ctrl, false), None);
        assert_eq!(key_action(key(NamedKey::Tab), plain(), false), None);
    }

    #[test]
    fn a_double_click_is_two_presses_on_the_same_row_in_time() {
        let ws = build_workspace(&scales());
        let rows: Vec<_> = ws.channels_state.rows.iter().map(|(id, _)| *id).collect();
        let (first, second) = (rows[0], rows[1]);
        let start = Instant::now();
        assert!(!is_double_click(None, first, start));
        assert!(is_double_click(
            Some((first, start)),
            first,
            start + Duration::from_millis(200)
        ));
        assert!(
            !is_double_click(
                Some((first, start)),
                second,
                start + Duration::from_millis(200)
            ),
            "another row"
        );
        assert!(
            !is_double_click(Some((first, start)), first, start + Duration::from_secs(2)),
            "too slow"
        );
    }

    fn click(target: aurora_widgets::WidgetId) -> accesskit::ActionRequest {
        accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: aurora_widgets::ACCESSIBILITY_TREE_ID,
            target_node: target,
            data: None,
        }
    }

    /// An assistive technology's `Click` reaches the app's reactions: an
    /// eye toggles, a saved row loads (its default action), a composite
    /// row selects, and the toolbar buttons run their actions.
    #[test]
    fn assistive_clicks_route_to_channels_requests() {
        use crate::{AccessibilityReaction, ChannelsRequest, route_accessibility_action};
        let mut ws = build_workspace(&scales());
        let mut doc = session();
        doc.selection.select(square());
        ok(doc.selection.save_active("Sky"));
        assert!(sync_channels(&mut ws, &scales(), &doc));
        ws.tree.compute_layout(1000.0, 800.0);
        let mut focus = crate::FocusManager::new();
        let none = std::collections::HashMap::new();
        let mut route = |ws: &mut aurora_ui::Workspace, target| {
            route_accessibility_action(ws, &mut focus, None, &none, None, None, &click(target))
        };
        let state = ws.channels_state.clone();
        let Some(red) = state.id_of(&ChannelRow::Composite(CompositeChannel::Red)) else {
            unreachable!("a Red row");
        };
        let eye = ws
            .tree
            .children(red)
            .and_then(|c| c.first().copied())
            .unwrap_or(red);
        assert!(matches!(
            route(&mut ws, eye),
            AccessibilityReaction::Channels(ChannelsRequest::Row { row, part })
                if row == red && part == ChannelRowPart::Eye
        ));
        let Some(sky) = state.id_of(&ChannelRow::Saved("Sky".to_owned())) else {
            unreachable!("a Sky row");
        };
        assert!(matches!(
            route(&mut ws, sky),
            AccessibilityReaction::Channels(ChannelsRequest::Row { row, part })
                if row == sky && part == ChannelRowPart::Row
        ));
        // Delete is disabled (and so unreachable) until a saved row is
        // selected.
        let mut selected = ws.channels_state.clone();
        ok(aurora_ui::select_channel_row(
            &mut ws.tree,
            &mut selected,
            &ChannelRow::Saved("Sky".to_owned()),
        ));
        ws.channels_state = selected;
        for action in aurora_ui::ChannelsAction::ALL {
            let Some(button) = state.toolbar.button_for(action) else {
                unreachable!("a button");
            };
            assert!(
                matches!(
                    route(&mut ws, button),
                    AccessibilityReaction::Channels(ChannelsRequest::Action(got)) if got == action
                ),
                "{action:?}"
            );
        }
    }

    /// The panel's commands, owner and persistence are wired like History's.
    #[test]
    fn the_channels_panel_has_commands_and_persists_in_the_layout() {
        use crate::{
            COMMAND_CLOSE_CHANNELS, COMMAND_FOCUS_CHANNELS, COMMAND_TOGGLE_CHANNELS,
            command_close_target, command_collapse_target, command_target, palette_commands,
        };
        let ws = build_workspace(&scales());
        let ids: Vec<String> = palette_commands().into_iter().map(|e| e.id).collect();
        for id in [
            COMMAND_FOCUS_CHANNELS,
            COMMAND_TOGGLE_CHANNELS,
            COMMAND_CLOSE_CHANNELS,
            "view.move_channels_up",
            "view.float_channels",
            "view.dock_channels",
        ] {
            assert!(ids.iter().any(|known| known == id), "{id} in the palette");
        }
        assert_eq!(
            command_target(&ws, COMMAND_FOCUS_CHANNELS),
            Some(ws.channels.root)
        );
        assert_eq!(
            command_collapse_target(&ws, COMMAND_TOGGLE_CHANNELS),
            Some(ws.channels)
        );
        assert_eq!(
            command_close_target(&ws, COMMAND_CLOSE_CHANNELS),
            Some(ws.channels)
        );
        // The arrangement saves the panel by key and reads it back.
        let saved = crate::saved_dock(&ws.dock_arrangement());
        assert!(
            saved
                .iter()
                .any(|slot| slot.panels.iter().any(|key| key == "channels"))
        );
        let (back, repaired) = crate::arrangement_from_saved(&saved, &[]);
        assert!(!repaired);
        assert_eq!(back, ws.dock_arrangement());
        // A layout saved before the panel existed gains it in History's group.
        let old: Vec<_> = saved
            .into_iter()
            .map(|mut slot| {
                slot.panels.retain(|key| key != "channels");
                slot
            })
            .collect();
        let (healed, repaired) = crate::arrangement_from_saved(&old, &[]);
        assert!(repaired);
        assert_eq!(healed, ws.dock_arrangement());
    }
}

/// `App::redraw` needs a window, so the one line that hands the document's
/// view setting to the canvas is pinned by a source check.
#[cfg(test)]
#[test]
fn the_frame_hands_the_documents_channel_gain_to_the_canvas() {
    let source = include_str!("lib.rs");
    assert!(source.contains("residency.set_channel_gain(self.doc.channel_view.gain());"));
}
