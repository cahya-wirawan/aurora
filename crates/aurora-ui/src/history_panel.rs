//! Real content for the History panel: one accessible row per journal
//! entry in an `aurora_doc::History`, in chronological order. PLAN.md
//! M1.8's "Layers, history, tool-options panels" bullet — History's own
//! slice; tool-options panels remain separate, still-open work.
//!
//! **No pixel rendering** — same boundary every widget in this crate
//! keeps: a row's accessible name is `History::journal_descriptions`'s
//! own human-readable text; there's no icon, no highlighting for the
//! "current" step the way the mockup's `.history-row.current` styling
//! shows, no drawn pixels.
//!
//! **One-shot, not reactive** — see [`crate::layers_panel`]'s own doc
//! comment for why (the same reasoning applies here: nothing can edit
//! a live document in `aurora-app` yet either).

use accesskit::{Node, Role};
use aurora_doc::History;
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetError, WidgetTree};
use taffy::Style;

use crate::panel::PanelHandle;

/// Replaces `panel`'s body accessibility with a real `Role::List`, then
/// inserts one `Role::ListItem` row per journal entry in `history`, in
/// chronological order (oldest first, matching
/// `History::journal_descriptions`'s own order).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_history_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    history: &History,
) -> Result<(), WidgetError> {
    let mut list_node = Node::new(Role::List);
    list_node.set_label("History");
    tree.set_accessibility(panel.body, list_node)?;

    for description in history.journal_descriptions() {
        let mut node = Node::new(Role::ListItem);
        node.set_label(description);
        tree.insert(panel.body, Style::default(), node, WidgetKind::Container)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::populate_history_panel;
    use crate::panel::insert_panel;
    use aurora_core::Rect;
    use aurora_doc::{History, LayerTree};
    use aurora_widgets::widgets;
    use taffy::Style;

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }
    }

    #[test]
    fn populate_history_panel_adds_one_row_per_journal_entry_in_order() {
        let mut layer_tree = LayerTree::new();
        let mut history = History::new();
        if let Err(err) = history.add_pixel_layer(&mut layer_tree, "Background", bounds(), None) {
            unreachable!("{err:?}");
        }
        let id = match history.add_pixel_layer(&mut layer_tree, "Retouch", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = history.set_opacity(&mut layer_tree, id, 0.8) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = populate_history_panel(&mut tree, panel, &history) {
            unreachable!("{err:?}");
        }

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);

        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        let expected = history.journal_descriptions();
        assert_eq!(rows.len(), expected.len());
        assert_eq!(rows.len(), 3, "add + add + set_opacity");

        for (&row, description) in rows.iter().zip(expected.iter()) {
            let Some(accessibility) = tree.accessibility(row) else {
                unreachable!("just inserted");
            };
            assert_eq!(accessibility.role(), accesskit::Role::ListItem);
            assert_eq!(accessibility.label(), Some(description.as_str()));
        }
    }

    #[test]
    fn populate_history_panel_rejects_an_unknown_panel_body() {
        let history = History::new();
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(panel.body) {
            unreachable!("{err:?}");
        }
        match populate_history_panel(&mut tree, panel, &history) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
