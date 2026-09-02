//! Real content for the History panel: one accessible row per journal
//! entry in an `aurora_doc::History`, in chronological order. PLAN.md
//! M1.8's "Layers, history, tool-options panels" bullet — History's own
//! slice; tool-options panels remain separate, still-open work.
//!
//! **Real rows with a real, hittable size** (0.77.2). Until then a row
//! was a `WidgetKind::Container` carrying `Style::default()`, inserted
//! straight into a `Row`-direction panel body — which, with `taffy`'s
//! default `align_items: Stretch`, resolved every row to **zero width
//! and the body's full height**. Measured in a real 1600×900
//! `crate::workspace::build_workspace`, five rows all came back as
//! `Rect { x: 1350, y: 600, width: 0, height: 300 }`, stacked exactly
//! on top of one another, and `WidgetTree::hit_test` returned `None`
//! for every one: a row was not a small target, it was a degenerate
//! layout box that could never be hit and could never paint. Rows are
//! now real [`aurora_widgets::widgets::WidgetKind::ListRow`]s with a
//! `min_size` of one [`aurora_widgets::widgets::row_height`] square —
//! the same token-derived number a Layers row beside them uses — and
//! the panel body itself stacks its children ([`crate::panel`]'s own
//! `body_style`).
//!
//! **Zero pixel difference, deliberately.** `aurora_widgets::paint`'s
//! own `paint_list_row` returns `Ok(vec![])` for an *unselected* row,
//! and nothing in the workspace selects a History row yet — no
//! `set_*_selected` call site, no click routing, no "current step"
//! marker of the kind the mockup's `.history-row.current` styling
//! shows. So this change draws exactly what the old one drew (nothing)
//! and changes only the geometry: a real, non-degenerate, hit-testable
//! rect. The highlight becomes real the moment something sets
//! `selected`, which is separate, still-open work.
//!
//! Still not drawn at all: the row's own text (a row's label reaches
//! the accessibility node and nothing else — `aurora-vector`-backed
//! glyph rendering doesn't exist yet) and any icon.
//!
//! **Rows past the bottom of the panel are laid out but unreachable.**
//! The same disclosed gap [`crate::layers_panel`] records, for the same
//! reason: `crate::panel`'s own `body_style` gives the panel a
//! content-independent share of the rail, and there is no scrolling
//! container anywhere in `aurora-widgets` yet. With the rail's ~300 px
//! History share and 21 px rows, exactly **14** of an up-to-**1001**-row
//! journal are reachable; the rest are refused by `hit_test`, which will
//! not descend into a parent whose own bounds exclude the point. A real
//! scrolling or virtualized list is what closes this, and it is still
//! open.
//!
//! **1001, not 1000** — `History::journal_descriptions` caps the *real*
//! journal steps it returns at `MAX_DESCRIPTIONS` (1000, matching
//! Photoshop's own History-states maximum) and prepends a synthetic
//! "N earlier steps omitted" notice at index 0 whenever anything was
//! dropped, so its own `journal_descriptions_caps_entry_count_with_an_
//! omission_notice` test asserts `len() == 1001`. That matters to
//! whoever eventually wires row clicks up to a real "revert to this
//! step" action: **row index is not journal index**, and on a truncated
//! journal row 0 is not a step at all.
//!
//! **The damage rect a full journal produces is not yet safe to scissor
//! with.** `WidgetTree` accumulates one union rect, so 1000 stacked rows
//! union to roughly `Rect { 0, 0, 1600, 21000 }` — ~24× a 900 px-tall
//! window. Harmless today: nothing in `aurora-app`'s redraw path
//! consumes `WidgetTree::take_damage` yet (only tests and `input.rs`
//! do). Whoever wires it to a partial-repaint path must intersect it
//! with the real surface size first — an oversized scissor rect is a
//! `wgpu` validation error, in crates that deny `panic`/`unwrap`.
//!
//! **No `Action::Focus`/`Action::Click` on a row, deliberately.**
//! Adding them would make every journal entry a `Tab` stop — up to 1001
//! of them inside one panel — which is the same crate-wide focus-model
//! question `aurora_widgets::widgets::tree_view` already discloses and
//! the Layers panel already pays. Making History pay it too, for rows
//! that route nowhere, would be a worse experience, not a better one.
//!
//! **One-shot, not reactive** — see [`crate::layers_panel`]'s own doc
//! comment for why (the same reasoning applies here: nothing can edit
//! a live document in `aurora-app` yet either).

use accesskit::{Node, Role};
use aurora_doc::History;
use aurora_theme::Scales;
use aurora_widgets::widgets::{ListRowState, WidgetKind, row_height};
use aurora_widgets::{WidgetError, WidgetTree};
use taffy::style_helpers::{auto, length, percent};
use taffy::{Size, Style};

use crate::panel::{PanelHandle, clear_panel_body};

/// One history row's own layout: full body width, one row height tall,
/// and never smaller than that on either axis.
///
/// **Two separate guards, load-bearing for two different reasons.**
/// The `0.77.2` commit message credited `flex_grow: 0.0` with preventing
/// sub-pixel rows in a long journal; that was wrong, and the correction
/// is measured rather than reasoned (0.77.3 review round).
///
/// - **`min_size.height: length(row)` is what makes a row exactly one
///   line tall at *any* entry count.** It is a hard flexbox floor that
///   neither `flex_grow` nor `flex_shrink` can cross. `size.height:
///   auto()` is what makes it a real floor rather than a starting point:
///   an `auto` main size gives the item a flex base size of `0`, so its
///   *scaled* flex-shrink factor (`flex_shrink × flex_base_size`) is `0`
///   too and a crowded panel has nothing to shrink. Setting
///   `flex_grow: 1.0` here was applied as a mutation and measured: rows
///   still came back a correct 21 px at 200 entries and at 1000. The
///   "200 entries would be 1.5 px each, 1000 would be 0.3 px" scenario
///   the previous wording described **does not occur**, and nothing here
///   depends on `flex_grow` to prevent it.
/// - **`flex_grow: 0.0` (the default, spelled by omission) is what stops
///   a *sparse* panel from inflating its rows.** Free space is what
///   `flex_grow` divides, and a panel with a handful of entries has
///   plenty: the same mutation inflates five rows to 60 px each, which
///   is what `history_rows_are_real_list_row_widgets_with_a_hittable_size`
///   actually catches. `aurora_widgets::widgets::command_palette::
///   row_style` uses `flex_grow: 1.0` deliberately, because its handful
///   of result rows really are meant to divide their container evenly; a
///   history row is one line tall no matter how much room it is offered.
///   `tree_view::style` records the same borrowed-idiom mistake.
///
/// Neither is redundant, and deleting `min_size.height` in particular
/// would bring the zero-height bug straight back — it is the guard that
/// holds, not the one the old wording credited.
///
/// `min_size.width` is one row height as well, the same square floor
/// and the same reasoning `tree_view::style` documents: no "minimum row
/// width" token exists, inventing one is a design decision rather than
/// an engineering default (CLAUDE.md), and a square of the row's own
/// height is the smallest thing that is still a real target.
fn row_style(scales: &Scales) -> Style {
    let row = row_height(scales);
    Style {
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        min_size: Size {
            width: length(row),
            height: length(row),
        },
        ..Default::default()
    }
}

/// Empties `panel`'s body, replaces its accessibility with a real
/// `Role::List`, then inserts one `Role::ListItem` row per journal entry
/// in `history`, in chronological order (oldest first, matching
/// `History::journal_descriptions`'s own order).
///
/// **The `Role::List` is deliberately unlabelled.** `panel.root` is
/// already a `Role::Region` labelled "History" ([`crate::panel::
/// insert_panel`]), so naming the list inside it "History" too was the
/// same nested-duplicate-name shape [`crate::layers_panel`] had to fix
/// in `0.77.1` — a screen reader announcing the name twice on entry —
/// just one level shallower. Relabelling the body rather than nesting a
/// second container did not avoid it; dropping the label does. A body
/// label would only be right if it said something the region does not,
/// the way [`crate::populate_properties_panel`]'s "Properties: Brush"
/// names the active tool.
///
/// **The rows are `panel.body`'s own direct children**, unlike
/// [`crate::populate_layers_panel`], which nests its rows inside a
/// `Role::Tree` container of its own. Nothing here needs the extra
/// level: since `0.77.2` the shared `body_style` stacks its children
/// itself, and `Role::List` on the body plus `Role::ListItem` on the
/// rows is already a well-formed list. Adding a container would also
/// change what `children(panel.body)` means to `aurora-app`'s own
/// tests, which count rows there directly.
///
/// **`Role::List`/`Role::ListItem`, not `Role::ListBox`/
/// `Role::ListBoxOption`.** The listbox pair is what
/// `aurora_widgets::widgets::command_palette` uses, and correctly — it
/// really is a single-selection chooser. History is not, yet: nothing
/// selects a row, so a `ListBoxOption` would over-promise a selection
/// interaction that does not exist, and `ListBoxOption` outside a
/// `ListBox` parent is a malformed accessibility tree besides. **None
/// of this has been checked against a real screen reader** — there is
/// no display server in this workspace's sandbox — so it is a
/// specification-level choice, not a verified one.
///
/// **Repopulating is safe**: `panel.body`'s existing children are
/// removed first ([`clear_panel_body`]), so calling this twice replaces
/// the rows rather than appending a second set beside the first with
/// the old, now-meaningless `WidgetId`s still live.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_history_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    history: &History,
) -> Result<(), WidgetError> {
    clear_panel_body(tree, panel.body)?;
    tree.set_accessibility(panel.body, Node::new(Role::List))?;

    let style = row_style(scales);
    for description in history.journal_descriptions() {
        let mut node = Node::new(Role::ListItem);
        node.set_label(description);
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
    use super::populate_history_panel;
    use crate::panel::insert_panel;
    use aurora_core::Rect;
    use aurora_doc::{History, LayerTree};
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, ListRowState, WidgetKind};
    use taffy::Style;
    use taffy::style_helpers::length;

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }
    }

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

    /// `count` real journal entries, each one a distinct `add_pixel_layer`.
    fn history_with(count: usize) -> History {
        let mut layer_tree = LayerTree::new();
        let mut history = History::new();
        for i in 0..count {
            if let Err(err) =
                history.add_pixel_layer(&mut layer_tree, format!("Layer {i}"), bounds(), None)
            {
                unreachable!("{err:?}");
            }
        }
        history
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
        let scales = test_scales();
        if let Err(err) = populate_history_panel(&mut tree, panel, &scales, &history) {
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

    /// The regression test for the `0.77.2` bug. Before the fix, every
    /// row of this exact tree laid out as
    /// `Rect { x: 1350, y: 600, width: 0, height: 300 }` -- zero width,
    /// the body's whole height, all five stacked on the same point --
    /// and `hit_test` returned `None` for all of them. Built through the
    /// real `build_workspace` rather than a bare `insert_panel`, because
    /// the degenerate width only appears once the body has a real
    /// resolved size to stretch against.
    #[test]
    fn history_rows_are_real_list_row_widgets_with_a_hittable_size() {
        let history = history_with(5);
        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        if let Err(err) = populate_history_panel(&mut ws.tree, ws.history, &scales, &history) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.history.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 5, "one row per journal entry");
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
                "the bug: a Row-direction body stretched every row to zero width -- {row_bounds:?}"
            );
            assert_eq!(
                row_bounds.height, one_row,
                "a row is exactly one line tall, not the body's whole height: {row_bounds:?}"
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
    /// Before `0.77.2` every row shared one identical rect, so a
    /// per-row `width > 0` check alone would not have caught it.
    #[test]
    fn history_rows_stack_top_to_bottom_without_overlapping() {
        let history = history_with(6);
        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        if let Err(err) = populate_history_panel(&mut ws.tree, ws.history, &scales, &history) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.history.body) else {
            unreachable!("just populated");
        };
        assert!(rows.len() >= 5, "at least five entries, got {}", rows.len());
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
                assert_eq!(
                    row_bounds.y,
                    before.y + i64::from(before.height),
                    "each row must start exactly where the one above it ended"
                );
            }
            previous = Some(row_bounds);
        }
    }

    /// The History twin of `layers_panel`'s own crowding test: a long
    /// journal must not claim the whole rail and starve the panels above
    /// it. `crate::panel`'s own `root_style`/`body_style` are what make
    /// that true, and rows with a real intrinsic height are exactly the
    /// content that would otherwise push against it.
    #[test]
    fn a_crowded_history_panel_never_starves_its_sibling_panels() {
        for count in [1_usize, 40, 200, 400] {
            let history = history_with(count);
            let mut ws = crate::workspace::build_workspace();
            let scales = test_scales();
            if let Err(err) = populate_history_panel(&mut ws.tree, ws.history, &scales, &history) {
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
                layers_bounds.height > 0 && properties_bounds.height > 0,
                "{count} history entries must not collapse the sibling panels: \
                 {layers_bounds:?}, {properties_bounds:?}"
            );
            assert_eq!(
                history_bounds.height, layers_bounds.height,
                "the three panels must keep sharing the rail equally at {count} entries"
            );
            assert!(
                history_bounds.y + i64::from(history_bounds.height) <= 900,
                "no panel may be pushed off the bottom of the window: {history_bounds:?}"
            );

            for (name, panel_bounds) in
                [("layers", layers_bounds), ("properties", properties_bounds)]
            {
                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (panel_bounds.x + i64::from(panel_bounds.width) / 2) as f32,
                    (panel_bounds.y + i64::from(panel_bounds.height) / 2) as f32,
                );
                assert!(
                    ws.tree.hit_test(point).is_some(),
                    "{name} must stay hit-testable at {count} entries"
                );
            }
        }
    }

    /// The honest limit of the fix, pinned rather than left implied.
    /// Bounding the panel means rows that no longer fit are clipped, and
    /// with no scrolling container anywhere in `aurora-widgets` yet,
    /// clipped means **unreachable**. The rows that do fit really work,
    /// which is what makes this an improvement rather than a finished
    /// panel.
    #[test]
    fn rows_past_the_bottom_of_a_bounded_history_panel_are_clipped_and_not_yet_reachable() {
        let history = history_with(200);
        let mut ws = crate::workspace::build_workspace();
        let scales = test_scales();
        if let Err(err) = populate_history_panel(&mut ws.tree, ws.history, &scales, &history) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.history.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 200, "every journal entry still gets a real row");
        let reachable = rows
            .iter()
            .filter(|&&row| {
                let Some(row_bounds) = ws.tree.bounds(row) else {
                    unreachable!("just laid out");
                };
                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (row_bounds.x + i64::from(row_bounds.width) / 2) as f32,
                    (row_bounds.y + i64::from(row_bounds.height) / 2) as f32,
                );
                ws.tree.hit_test(point) == Some(row)
            })
            .count();
        // The exact count, not just `> 0 && < len`. The loose form read
        // as "a scrolling container would close this gap," which it
        // would not make fail -- a scrolled-out row is exactly as
        // unreachable to `hit_test` as a clipped-out one. What is really
        // being pinned is the arithmetic: a 300px panel share divided by
        // 21px rows, with no scrolling of any kind. Pinning the number
        // is what makes a silent change in the visible row count a test
        // failure rather than a shrug.
        assert_eq!(
            reachable, 14,
            "300px of History share divided by 21px rows -- the rows that fit really work, \
             and the other 186 are clipped and unreachable"
        );
    }

    /// The `Role::List` body must carry no accessible name of its own:
    /// `panel.root` is already a `Role::Region` labelled "History", and
    /// a nested node repeating that name is the same double-announcement
    /// `layers_panel` fixed in `0.77.1` by leaving its tree container
    /// unlabelled. Mirrors that module's own
    /// `tree.accessibility(tree_root).label() == None` assertion.
    #[test]
    fn the_history_list_body_carries_no_name_of_its_own() {
        let history = history_with(3);
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_history_panel(&mut tree, panel, &scales, &history) {
            unreachable!("{err:?}");
        }

        let (Some(region), Some(list)) = (
            tree.accessibility(panel.root),
            tree.accessibility(panel.body),
        ) else {
            unreachable!("just populated");
        };
        assert_eq!(
            region.label(),
            Some("History"),
            "the panel's own region is what names it"
        );
        assert_eq!(list.role(), accesskit::Role::List);
        assert_eq!(
            list.label(),
            None,
            "a nested node repeating the region's name makes a screen reader announce \
             'History' twice on entry"
        );
    }

    /// Repopulating replaces the rows rather than appending a second set
    /// beside the first — the `clear_panel_body` call this function
    /// makes for itself, the same guarantee `populate_layers_panel`
    /// gained in `0.77.1`.
    #[test]
    fn populating_the_same_panel_twice_replaces_the_rows_instead_of_stacking_them() {
        let history = history_with(3);
        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: length(300.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        for _ in 0..2 {
            if let Err(err) = populate_history_panel(&mut tree, panel, &scales, &history) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(history.journal_descriptions().len()),
            "a second call must replace the rows, not stack a second set beside them"
        );
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
        let scales = test_scales();
        match populate_history_panel(&mut tree, panel, &scales, &history) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
