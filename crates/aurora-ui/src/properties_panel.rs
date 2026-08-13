//! Real content for the Properties panel: one accessible row per
//! label/value pair the caller supplies for whichever
//! [`crate::tool::Tool`] is active. PLAN.md M1.8's "Layers, history,
//! tool-options panels" bullet — the tool-options slice, closing the
//! gap Layers and History left open (see this crate's own `lib.rs` doc
//! comment and each of those two modules' own doc comments for that
//! history).
//!
//! **Generic mechanism, caller owns the data** — the same split
//! [`crate::tool`]'s own doc comment draws between this crate (tool
//! identity, pure geometry) and `aurora-app` (the live document, the
//! live parameters). This module knows nothing about `BRUSH_RADIUS`,
//! `ERASER_RADIUS`, or any other tool-specific constant — those live in
//! `aurora-app`, the one place real per-tool parameters exist today
//! (`aurora-ui` has no dependency on `aurora-app`, and couldn't reach
//! them even if it wanted to — `scripts/layering.json`). A caller with
//! no real options for the active tool passes an empty slice, which
//! [`populate_properties_panel`] renders as an honest empty list — not
//! a placeholder row, not an invented default.
//!
//! **No pixel rendering** — same boundary [`crate::history_panel`] and
//! [`crate::layers_panel`] both keep: a row's accessible name is the
//! label/value text the caller passed in, no icon, no editable field, no
//! drawn pixels. There is no size-picker or other editable-options UI
//! here — that's real, separate work for whenever `aurora-vector`/text
//! rendering exist to build one with.
//!
//! **One-shot, not reactive** — see [`crate::layers_panel`]'s own doc
//! comment for why (the same reasoning applies here: nothing can edit
//! a live document in `aurora-app` yet either). A caller re-populates on
//! every tool change by clearing the body first
//! (`crate::clear_panel_body`) — see `aurora-app`'s own
//! `refresh_properties_panel`.

use accesskit::{Node, Role};
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetError, WidgetTree};
use taffy::Style;

use crate::panel::PanelHandle;
use crate::tool::Tool;

/// Replaces `panel`'s body accessibility with a real `Role::List`
/// labeled with `tool`'s own [`Tool::label`], then inserts one
/// `Role::ListItem` row per `(label, value)` pair in `options`, in the
/// order given. `options` is deliberately just label/value text — no
/// tool-specific knowledge lives in this crate; see this module's own
/// doc comment for why. An empty `options` slice is a legitimate,
/// honest "no real options for this tool yet" state, not an error.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_properties_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    tool: Tool,
    options: &[(&str, String)],
) -> Result<(), WidgetError> {
    let mut list_node = Node::new(Role::List);
    list_node.set_label(format!("Properties: {}", tool.label()));
    tree.set_accessibility(panel.body, list_node)?;

    for (label, value) in options {
        let mut node = Node::new(Role::ListItem);
        node.set_label(format!("{label}: {value}"));
        tree.insert(panel.body, Style::default(), node, WidgetKind::Container)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::populate_properties_panel;
    use crate::panel::insert_panel;
    use crate::tool::Tool;
    use aurora_widgets::widgets;
    use taffy::Style;

    #[test]
    fn populate_properties_panel_adds_one_row_per_option_in_order() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let options = [("Radius", "24px".to_owned())];
        if let Err(err) = populate_properties_panel(&mut tree, panel, Tool::Brush, &options) {
            unreachable!("{err:?}");
        }

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);
        assert_eq!(body_accessibility.label(), Some("Properties: Brush"));

        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), options.len());
        assert_eq!(rows.len(), 1);

        let Some(&row) = rows.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::ListItem);
        assert_eq!(accessibility.label(), Some("Radius: 24px"));
    }

    #[test]
    fn populate_properties_panel_with_no_options_leaves_an_empty_but_labeled_list() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = populate_properties_panel(&mut tree, panel, Tool::Move, &[]) {
            unreachable!("{err:?}");
        }

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);
        assert_eq!(body_accessibility.label(), Some("Properties: Move"));

        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(
            rows.len(),
            0,
            "a tool with no real options yet must render zero rows, not a placeholder"
        );
    }

    #[test]
    fn populate_properties_panel_rejects_an_unknown_panel_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(panel.body) {
            unreachable!("{err:?}");
        }
        match populate_properties_panel(&mut tree, panel, Tool::Brush, &[]) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
