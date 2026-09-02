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
//! every tool change and [`populate_properties_panel`] empties the body
//! for itself first, the same contract its two sibling `populate_*`
//! functions already had — see `aurora-app`'s own
//! `refresh_properties_panel`.
//!
//! **Its rows are still degenerate, and that is a real, open bug** —
//! disclosed here and pinned by
//! `properties_rows_are_still_degenerate_zero_height_boxes`, deliberately
//! not fixed in the round that wrote this. A row is a
//! `WidgetKind::Container` carrying `Style::default()`, which under the
//! shared `Column` body direction resolves to full body width and **zero
//! height** — the same class of bug `crate::history_panel` had on the
//! width axis before `0.77.2`, with the axis swapped by the same
//! `Row` → `Column` change that fixed History. Nothing hit-tests or
//! paints a Properties row today, so it is currently invisible rather
//! than wrong on screen; the fix is the same one History got (real
//! `WidgetKind::ListRow`s with a token-derived `min_size`), and it needs
//! the `&Scales` this function does not yet take.

use accesskit::{Node, Role};
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetError, WidgetTree};
use taffy::Style;

use crate::panel::{PanelHandle, clear_panel_body};
use crate::tool::Tool;

/// Empties `panel`'s body, replaces its accessibility with a real
/// `Role::List` labeled with `tool`'s own [`Tool::label`], then inserts
/// one `Role::ListItem` row per `(label, value)` pair in `options`, in
/// the order given. `options` is deliberately just label/value text — no
/// tool-specific knowledge lives in this crate; see this module's own
/// doc comment for why. An empty `options` slice is a legitimate,
/// honest "no real options for this tool yet" state, not an error.
///
/// **Repopulating is safe**: `panel.body`'s existing children are
/// removed first ([`clear_panel_body`]), so switching from a tool with
/// real options to one without really empties the panel instead of
/// leaving the previous tool's stale rows in it. This landed in
/// `0.77.3` purely to give the three `populate_*` functions one
/// contract; every caller in `aurora-app` already cleared first
/// (`replace_document`, `refresh_properties_panel`, and `App::new`'s own
/// freshly built workspace), so it changed no behaviour — it moved the
/// guarantee from "every caller remembers" to "the function provides."
///
/// **Unlike its two siblings, this one is labelled, and deliberately.**
/// A body label is only right when it says something the panel's own
/// `Role::Region` does not; "Properties: Brush" names the active tool,
/// where "History" on a region already labelled "History" was just a
/// duplicate announcement (see [`crate::populate_history_panel`]).
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
    clear_panel_body(tree, panel.body)?;
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

    /// Repopulating replaces the rows rather than appending a second set
    /// beside the first — the [`clear_panel_body`] call this function
    /// makes for itself, the same guarantee `populate_layers_panel`
    /// gained in `0.77.1` and `populate_history_panel` in `0.77.2`.
    #[test]
    fn populating_the_same_panel_twice_replaces_the_rows_instead_of_stacking_them() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let options = [("Radius", "24px".to_owned())];
        for _ in 0..2 {
            if let Err(err) = populate_properties_panel(&mut tree, panel, Tool::Brush, &options) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(1),
            "a second call must replace the row, not stack a second one beside it"
        );

        if let Err(err) = populate_properties_panel(&mut tree, panel, Tool::Move, &[]) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.children(panel.body),
            Some([].as_slice()),
            "and switching to a tool with no options must really empty the panel"
        );
    }

    /// **An honest before-picture of a known, disclosed bug, not a
    /// guarantee.** A Properties row is a `WidgetKind::Container`
    /// carrying `Style::default()`, which under `crate::panel`'s shared
    /// `Column` body direction resolves to full body width and **zero
    /// height** — laid out, but degenerate, unhittable, and unpaintable,
    /// exactly the shape a History row had on the *width* axis before
    /// `0.77.2`. This test asserts the broken geometry on purpose, so
    /// that the disclosure in this module's own doc comment is something
    /// CI notices changing rather than prose that can quietly go stale.
    /// The same discipline `layers_panel`'s own
    /// `tab_order_currently_stops_on_every_layer_row` uses for a
    /// different disclosed gap.
    ///
    /// **Whoever fixes the bug should delete this test**, not weaken it:
    /// its failure is the intended signal that the gap closed.
    #[test]
    fn properties_rows_are_still_degenerate_zero_height_boxes() {
        let mut ws = crate::workspace::build_workspace();
        let options = [
            ("Radius", "24px".to_owned()),
            ("Hardness", "80%".to_owned()),
        ];
        if let Err(err) =
            populate_properties_panel(&mut ws.tree, ws.properties, Tool::Brush, &options)
        {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.properties.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 2, "one row per option");
        for &row in rows {
            let Some(row_bounds) = ws.tree.bounds(row) else {
                unreachable!("just laid out");
            };
            assert_eq!(
                row_bounds.height, 0,
                "the disclosed bug: a Style::default() row in a Column body has no height \
                 at all -- {row_bounds:?}"
            );
            #[allow(clippy::cast_precision_loss)]
            let point = (
                (row_bounds.x + i64::from(row_bounds.width) / 2) as f32,
                row_bounds.y as f32,
            );
            assert_ne!(
                ws.tree.hit_test(point),
                Some(row),
                "and a zero-height row cannot be hit, the same way a zero-width History row \
                 could not be before 0.77.2"
            );
        }
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
