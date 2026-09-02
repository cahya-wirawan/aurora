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
//! **Real rows with a real, hittable size** (0.77.4). Until then a row
//! was a `WidgetKind::Container` carrying `Style::default()`, which under
//! the shared `Column` body direction resolved to full body width and
//! **zero height** — laid out, but degenerate, unhittable and
//! unpaintable, the same class of bug [`crate::history_panel`] had on the
//! *width* axis before `0.77.2`. Rows are now real
//! [`aurora_widgets::widgets::WidgetKind::ListRow`]s carrying
//! `crate::panel`'s own `row_style`, the same shared style a History row
//! beside them uses (it moved there from `history_panel` in this same
//! round, rather than being copied a second time).
//!
//! **Zero pixel difference, deliberately.** `aurora_widgets::paint`'s own
//! `paint_list_row` returns `Ok(vec![])` for an *unselected* row, and
//! nothing in the workspace selects a Properties row — no `set_*_selected`
//! call site, no click routing. So this draws exactly what the old code
//! drew (nothing) and changes only the geometry: a real, non-degenerate,
//! hit-testable rect. Still not drawn at all: the row's own label text
//! (it reaches the accessibility node and nothing else — there is no
//! glyph rendering here yet) and any editable control.
//!
//! **Rows past the bottom of the panel would be clipped and unreachable**
//! — the same structural gap [`crate::history_panel`] and
//! [`crate::layers_panel`] both disclose, since `crate::panel`'s own
//! `body_style` gives each panel a content-independent share of the rail
//! and `aurora-widgets` has no scrolling container. Here it is a
//! class-guard disclosure rather than a live limit: `aurora-app`'s own
//! `tool_options` returns at most one row today (a Brush or Eraser
//! radius) and the panel's rail share fits roughly fourteen, so no real
//! user path reaches the clip.
//!
//! **No `Action::Focus`/`Action::Click` on a row, deliberately** — the
//! same reasoning [`crate::history_panel`] records: routing rows to a
//! real interaction is separate work, and making them `Tab` stops that
//! go nowhere is the crate-wide focus-model question
//! `aurora_widgets::widgets::tree_view` already discloses.

use accesskit::{Node, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{ListRowState, WidgetKind};
use aurora_widgets::{WidgetError, WidgetTree};

use crate::panel::{PanelHandle, clear_panel_body, row_style};
use crate::tool::Tool;

/// Empties `panel`'s body, replaces its accessibility with a real
/// `Role::List` labeled with `tool`'s own [`Tool::label`], then inserts
/// one `Role::ListItem` row per `(label, value)` pair in `options`, in
/// the order given. `options` is deliberately just label/value text — no
/// tool-specific knowledge lives in this crate; see this module's own
/// doc comment for why. An empty `options` slice is a legitimate,
/// honest "no real options for this tool yet" state, not an error.
///
/// `scales` is what gives each row its real, token-derived height
/// (`crate::panel`'s own `row_style`, shared with
/// [`crate::populate_history_panel`]); before `0.77.4` the rows carried
/// `Style::default()` and resolved to zero height — see this module's own
/// doc comment.
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
    scales: &Scales,
    tool: Tool,
    options: &[(&str, String)],
) -> Result<(), WidgetError> {
    clear_panel_body(tree, panel.body)?;
    let mut list_node = Node::new(Role::List);
    list_node.set_label(format!("Properties: {}", tool.label()));
    tree.set_accessibility(panel.body, list_node)?;

    let style = row_style(scales);
    for (label, value) in options {
        let mut node = Node::new(Role::ListItem);
        node.set_label(format!("{label}: {value}"));
        tree.insert(
            panel.body,
            style.clone(),
            node,
            WidgetKind::ListRow(ListRowState::default()),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::populate_properties_panel;
    use crate::panel::insert_panel;
    use crate::tool::Tool;
    use aurora_core::Rect;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, ListRowState, WidgetKind};
    use taffy::Style;

    // The real, committed, owner-approved scales -- the same file
    // `aurora-theme`'s own tests parse, so this exercises real token
    // values, not a synthetic fixture.
    fn test_scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// `count` synthetic label/value pairs. Deliberately far more than
    /// `aurora-app`'s own `tool_options` ever produces (one row, a Brush
    /// or Eraser radius) -- the crowding test needs content the real
    /// caller cannot currently supply.
    fn options_with(count: usize) -> Vec<(&'static str, String)> {
        (0..count).map(|i| ("Radius", format!("{i}px"))).collect()
    }

    #[test]
    fn populate_properties_panel_adds_one_row_per_option_in_order() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let options = [("Radius", "24px".to_owned())];
        let scales = test_scales();
        if let Err(err) =
            populate_properties_panel(&mut tree, panel, &scales, Tool::Brush, &options)
        {
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
        let scales = test_scales();
        if let Err(err) = populate_properties_panel(&mut tree, panel, &scales, Tool::Move, &[]) {
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
        let scales = test_scales();
        for _ in 0..2 {
            if let Err(err) =
                populate_properties_panel(&mut tree, panel, &scales, Tool::Brush, &options)
            {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(1),
            "a second call must replace the row, not stack a second one beside it"
        );

        if let Err(err) = populate_properties_panel(&mut tree, panel, &scales, Tool::Move, &[]) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.children(panel.body),
            Some([].as_slice()),
            "and switching to a tool with no options must really empty the panel"
        );
    }

    /// The regression test for the `0.77.4` bug. Before the fix, a
    /// Properties row was a `WidgetKind::Container` carrying
    /// `Style::default()`, which under `crate::panel`'s shared `Column`
    /// body direction resolved to full body width and **zero height** --
    /// laid out, but degenerate, unhittable and unpaintable. Built
    /// through the real `build_workspace` rather than a bare
    /// `insert_panel`, because the degeneracy only appears once the body
    /// has a real resolved size to lay a row out against.
    #[test]
    fn properties_rows_are_real_list_row_widgets_with_a_hittable_size() {
        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        let options = [
            ("Radius", "24px".to_owned()),
            ("Hardness", "80%".to_owned()),
        ];
        if let Err(err) =
            populate_properties_panel(&mut ws.tree, ws.properties, &scales, Tool::Brush, &options)
        {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.properties.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 2, "one row per option");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let one_row = widgets::row_height(&scales) as u32;
        assert_eq!(one_row, 21, "13px of type plus 4px above and below");

        for &row in rows {
            assert_eq!(
                ws.tree.payload(row),
                Some(&WidgetKind::ListRow(ListRowState::default())),
                "a row must be a real ListRow, not an unpainted Container"
            );
            let Some(row_bounds) = ws.tree.bounds(row) else {
                unreachable!("just laid out");
            };
            assert!(
                row_bounds.width > 0,
                "a row must not be a degenerate zero-width box either: {row_bounds:?}"
            );
            // The exact height, not `> 0`. A sparse panel is exactly
            // where `flex_grow: 1.0` would inflate two rows to ~150px
            // each and still pass a positive-height check -- see
            // `crate::panel`'s own `row_style` for why that guard is
            // load-bearing.
            assert_eq!(
                row_bounds.height, one_row,
                "a row is exactly one line tall, not a share of the body: {row_bounds:?}"
            );
            #[allow(clippy::cast_precision_loss)]
            let point = (
                (row_bounds.x + i64::from(row_bounds.width) / 2) as f32,
                (row_bounds.y + i64::from(row_bounds.height) / 2) as f32,
            );
            assert_eq!(
                ws.tree.hit_test(point),
                Some(row),
                "and a pointer must actually land on it: {row_bounds:?}"
            );
        }
    }

    /// The other half of the same fix: rows must stack, not overlap.
    ///
    /// The strict `y` *increase* is what makes this non-vacuous, and it
    /// was added after the assertion was mutation-tested: the equality
    /// alone (`y == previous.y + previous.height`) is trivially true of
    /// the very bug being fixed, since zero-height rows all pile up at
    /// one `y` and satisfy `y == previous.y + 0`. Reverting the fix now
    /// fails this test rather than passing it by accident.
    #[test]
    fn properties_rows_stack_top_to_bottom_without_overlapping() {
        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        let options = options_with(4);
        if let Err(err) =
            populate_properties_panel(&mut ws.tree, ws.properties, &scales, Tool::Brush, &options)
        {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.properties.body) else {
            unreachable!("just populated");
        };
        assert!(rows.len() >= 3, "at least three rows, got {}", rows.len());
        let mut previous: Option<Rect> = None;
        for &row in rows {
            let Some(row_bounds) = ws.tree.bounds(row) else {
                unreachable!("just laid out");
            };
            if let Some(before) = previous {
                assert_eq!(
                    row_bounds.x, before.x,
                    "sibling rows must share a left edge, not sit beside each other"
                );
                assert!(
                    before.height > 0,
                    "a degenerate zero-height row would satisfy the stacking check below \
                     vacuously: {before:?}"
                );
                assert_eq!(
                    row_bounds.y,
                    before.y + i64::from(before.height),
                    "each row must start exactly where the one above it ended"
                );
                assert!(
                    row_bounds.y > before.y,
                    "and must really be below it, not piled on the same point: \
                     {before:?}, {row_bounds:?}"
                );
            }
            previous = Some(row_bounds);
        }
    }

    /// The Properties twin of `history_panel`'s own crowding test: rows
    /// with a real intrinsic height are exactly the content that would
    /// otherwise push a panel past its share of the rail and starve its
    /// siblings, the way a 43-layer Layers panel did before `0.77.1`.
    ///
    /// **There is deliberately no Properties twin of History's
    /// `rows_past_the_bottom_..._are_clipped_and_not_yet_reachable`.**
    /// That test pins an exact reachable-row count, which is worth doing
    /// for a journal that really reaches 1001 entries; `aurora-app`'s own
    /// `tool_options` returns at most one row, so the same assertion here
    /// would pin arithmetic no real user path can exercise. The clipping
    /// itself is disclosed in this module's own doc comment instead.
    #[test]
    fn a_crowded_properties_panel_never_starves_its_sibling_panels() {
        for count in [1_usize, 40, 200, 400] {
            let mut ws = crate::workspace::build_workspace();
            let scales = test_scales();
            let options = options_with(count);
            if let Err(err) = populate_properties_panel(
                &mut ws.tree,
                ws.properties,
                &scales,
                Tool::Brush,
                &options,
            ) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(1600.0, 900.0);

            let (Some(layers_bounds), Some(properties_bounds), Some(history_bounds)) = (
                ws.tree.bounds(ws.layers.root),
                ws.tree.bounds(ws.properties.root),
                ws.tree.bounds(ws.history.root),
            ) else {
                unreachable!("just laid out");
            };

            assert!(
                layers_bounds.height > 0 && history_bounds.height > 0,
                "{count} property rows must not collapse the sibling panels: \
                 {layers_bounds:?}, {history_bounds:?}"
            );
            assert_eq!(
                properties_bounds.height, layers_bounds.height,
                "the three panels must keep sharing the rail equally at {count} rows"
            );
            assert!(
                history_bounds.y + i64::from(history_bounds.height) <= 900,
                "no panel may be pushed off the bottom of the window: {history_bounds:?}"
            );

            for (name, panel_bounds) in [("layers", layers_bounds), ("history", history_bounds)] {
                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (panel_bounds.x + i64::from(panel_bounds.width) / 2) as f32,
                    (panel_bounds.y + i64::from(panel_bounds.height) / 2) as f32,
                );
                assert!(
                    ws.tree.hit_test(point).is_some(),
                    "{name} must stay hit-testable at {count} rows"
                );
            }
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
        let scales = test_scales();
        match populate_properties_panel(&mut tree, panel, &scales, Tool::Brush, &[]) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
