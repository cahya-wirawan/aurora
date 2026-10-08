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
//! `body_style`). That `min_size` comes from `crate::panel`'s own
//! `row_style`, shared with the Properties panel since `0.77.4` rather
//! than copied into each; see it for why both guards there are
//! load-bearing.
//!
//! **The steps, not the journal** (0.147.1). Until then the panel listed
//! `aurora_doc::History::journal_descriptions` — the structural
//! crash-recovery journal — so brush and eraser strokes (which live in
//! `aurora_brush::PixelHistory`) never appeared, and undo and redo, being
//! journalled themselves, *added* rows instead of moving a marker. The
//! caller now hands in the user's own undoable steps as
//! [`HistoryStep`]s, in the order `Ctrl+Z` walks them (`aurora-app`'s
//! `UndoOrder`): an origin row ("Open", "New Document") first, then every
//! applied step oldest first, then every undone (redoable) step in the
//! order redo would replay them.
//!
//! **The current step is marked.** The newest applied step's row — the
//! origin row when nothing is applied — is `ListRowState::selected`, so
//! `aurora_widgets::paint`'s `paint_list_row` draws its `accent.primary`
//! highlight and its text is `text.on_accent`. Every undone step's row is
//! `ListRowState::disabled`, drawn in `text.disabled`; no new token.
//!
//! **Rows draw their labels** (0.147.1). Before this round a row's label
//! reached only the accessibility node — `aurora_widgets::text_runs`
//! drew a `ListRow`'s text only under a menu, a dropdown list or the
//! command palette — so the panel was visually empty. Every History row
//! now opts in with `ListRowState::draws_label`; the read-only Properties
//! rows do not, and stay undrawn. No icon yet.
//!
//! **Accessibility of an undone step: an accesskit state description,
//! not a label suffix.** An undone row keeps its plain label ("Brush
//! Stroke") and carries `state_description` [`UNDONE_STATE`] ("Undone"),
//! so the drawn text and the accessible name stay the same string and a
//! screen reader still announces the step, plus its state. The origin
//! row and every step row report `selected` true or false; the "… N
//! earlier/later steps omitted" notice rows report no selection state at
//! all, since they are not steps. The "later" notice is dimmed and
//! "Undone" because it can only stand for redo steps: the window always
//! contains the current step, so everything past its end is undone. The rows are not marked accesskit-`disabled`:
//! redo can still reach them, so "unavailable" would be wrong. None of
//! this has been checked against a real screen reader.
//!
//! **Rows past the bottom of the panel scroll into reach** (0.145.0's
//! scrolling body, 0.146.0's scrollbar). `aurora-app` records the current
//! row in `Workspace::history_current` and scrolls it into view after
//! every refresh. At offset 0, with the rail's ~300 px History share and
//! 21 px rows, 13 rows are visible.
//!
//! **At most 1000 steps, plus up to two notice rows** — the same cap
//! (`aurora_doc::MAX_DESCRIPTIONS`, Photoshop's own History-states
//! maximum) `History::journal_descriptions` applies. Past it, the oldest
//! steps are dropped and the origin row becomes a synthetic "… N earlier
//! steps omitted" notice — unless the current step would fall off the
//! front, in which case the window starts at the current step and a
//! trailing "… N later steps omitted" notice stands for the redo steps
//! past its end. The current row is always shown. **Row index is not
//! step index**: row 0 is the origin or a notice, never a step — which
//! matters to whoever wires row clicks to "revert to this step", the
//! open next step.
//!
//! **The damage rect a full journal produces is not yet safe to scissor
//! with, and the rect is the whole tree's, not this panel's.**
//! `WidgetTree` accumulates one union rect across every dirty node, so
//! all three panels feed it — [`crate::properties_panel`]
//! cross-references this paragraph rather than restating it. Measured in
//! a real 1600×900 `build_workspace` with a
//! capped 1001-row journal (the pre-0.147.1 panel), 200 layers and ten tool options populated at
//! once: `Rect { 0, 0, 1600, 21621 }`, ~24× a 900 px-tall window. The
//! same measurement with *only* History populated gives the identical
//! number, because a full journal dominates the union outright (1001
//! rows of 21 px, from a body 600 px down the rail, against Layers'
//! 4500). Treat the figure as an order of magnitude rather than a
//! constant — it moves with row counts and with the rail's own share.
//! (Through `0.77.4` this comment quoted "roughly
//! `Rect { 0, 0, 1600, 21000 }`", which was neither measured nor
//! current.) Harmless today: nothing in `aurora-app`'s redraw path
//! consumes `WidgetTree::take_damage` yet (only tests and `input.rs`
//! do). Whoever wires it to a partial-repaint path must intersect it
//! with the real surface size first — an oversized scissor rect is a
//! `wgpu` validation error, in crates that deny `panic`/`unwrap`.
//!
//! **No `Action::Focus`/`Action::Click` on a row, deliberately — and no
//! click-to-jump yet.** Clicking a row does nothing (out of scope for
//! 0.147.1; the suggested next step). Adding the actions would make every
//! step a `Tab` stop — up to 1002
//! of them inside one panel — which is the same crate-wide focus-model
//! question `aurora_widgets::widgets::tree_view` already discloses and
//! the Layers panel already pays. Making History pay it too, for rows
//! that route nowhere, would be a worse experience, not a better one.
//!
//! **One-shot, not reactive** — a caller re-populates after every
//! recorded step, undo and redo (`aurora-app`'s `refresh_history_panel`):
//! a committed stroke, a structural edit, a layer-control commit, New and
//! Delete Layer, and an opened document. Never per dab.

use accesskit::{Node, Role};
use aurora_doc::MAX_DESCRIPTIONS;
use aurora_theme::Scales;
use aurora_widgets::widgets::{ListRowState, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};

use crate::panel::{PanelHandle, clear_panel_body, row_style};

/// The accesskit `state_description` an undone (redoable) step's row
/// carries — see this module's own doc comment for why a state rather
/// than a label suffix.
pub const UNDONE_STATE: &str = "Undone";

/// One of the user's undoable steps, as the History panel shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStep<'a> {
    /// What the row says ("Brush Stroke", `Added layer "Layer 2"`).
    pub label: &'a str,
    /// `true` for a step that has been undone and can be redone.
    pub undone: bool,
}

/// "… {n} earlier/later step(s) omitted", the notice row shape
/// `aurora_doc::History::journal_descriptions` already uses.
fn omitted(count: usize, which: &str) -> String {
    format!(
        "\u{2026} {count} {which} step{} omitted",
        if count == 1 { "" } else { "s" }
    )
}

/// Empties `panel`'s body, replaces its accessibility with a real
/// `Role::List`, then inserts one `Role::ListItem` row for `origin` and
/// one per step in `steps` (applied steps oldest first, then undone ones
/// in redo order — see this module's own doc comment), marking the
/// current one. Returns the current row's id, so the caller can scroll
/// it into view.
///
/// The current step is the last step with `undone == false`; with none,
/// the origin row is current.
///
/// **The `Role::List` is deliberately unlabelled.** `panel.root` is
/// already a `Role::Region` labelled "History" ([`crate::panel::
/// insert_panel`]), so naming the list inside it "History" too was the
/// same nested-duplicate-name shape [`crate::layers_panel`] had to fix
/// in `0.77.1` — a screen reader announcing the name twice on entry —
/// just one level shallower.
///
/// **The rows are `panel.body`'s own direct children**, unlike
/// [`crate::populate_layers_panel`], which nests its rows inside a
/// `Role::Tree` container of its own. `aurora-app`'s own tests count rows
/// there directly.
///
/// **`Role::List`/`Role::ListItem`, not `Role::ListBox`/
/// `Role::ListBoxOption`.** Rows now report `selected` (the current
/// step), but nothing chooses a row yet — click-to-jump is still open —
/// so a listbox would over-promise an interaction that does not exist.
/// Revisit when rows become clickable. **Not checked against a real
/// screen reader.**
///
/// **Repopulating is safe**: `panel.body`'s existing children are
/// removed first ([`clear_panel_body`]).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_history_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    origin: &str,
    steps: &[HistoryStep<'_>],
) -> Result<WidgetId, WidgetError> {
    clear_panel_body(tree, panel.body)?;
    tree.set_accessibility(panel.body, Node::new(Role::List))?;

    let style = row_style(scales);
    // `selected` is `None` for a notice row, which is not a step and
    // reports no selection state at all.
    let insert = |tree: &mut WidgetTree<WidgetKind>,
                  label: &str,
                  selected: Option<bool>,
                  undone: bool|
     -> Result<WidgetId, WidgetError> {
        let mut node = Node::new(Role::ListItem);
        node.set_label(label);
        if let Some(selected) = selected {
            node.set_selected(selected);
        }
        if undone {
            node.set_state_description(UNDONE_STATE);
        }
        tree.insert(
            panel.body,
            style.clone(),
            node,
            WidgetKind::ListRow(ListRowState {
                selected: selected.unwrap_or(false),
                disabled: undone,
                draws_label: true,
            }),
        )
    };

    let current = steps.iter().rposition(|step| !step.undone);
    let len = steps.len();
    // The window of steps shown: the newest `MAX_DESCRIPTIONS`, moved
    // back far enough that the current step is never cut off the front.
    let start = len
        .saturating_sub(MAX_DESCRIPTIONS)
        .min(current.unwrap_or(0));
    let end = start.saturating_add(MAX_DESCRIPTIONS).min(len);

    let first = if start == 0 {
        insert(tree, origin, Some(current.is_none()), false)?
    } else {
        insert(tree, &omitted(start, "earlier"), None, false)?
    };
    let mut current_row = first;
    for (index, step) in steps.iter().enumerate().take(end).skip(start) {
        let selected = current == Some(index);
        let row = insert(tree, step.label, Some(selected), step.undone)?;
        if selected {
            current_row = row;
        }
    }
    if end < len {
        insert(tree, &omitted(len - end, "later"), None, true)?;
    }
    Ok(current_row)
}

#[cfg(test)]
mod tests {
    use super::{HistoryStep, UNDONE_STATE, populate_history_panel};
    use crate::panel::insert_panel;
    use aurora_core::Rect;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, ListRowState, WidgetKind};
    use taffy::Style;
    use taffy::style_helpers::length;

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

    /// `count` distinct step labels, oldest first.
    fn history_with(count: usize) -> Vec<String> {
        (0..count)
            .map(|i| format!("Added layer \"Layer {i}\""))
            .collect()
    }

    /// Every label as an applied step.
    fn applied(labels: &[String]) -> Vec<HistoryStep<'_>> {
        labels
            .iter()
            .map(|label| HistoryStep {
                label,
                undone: false,
            })
            .collect()
    }

    /// The first `applied` labels applied, the rest undone.
    fn split(labels: &[String], applied: usize) -> Vec<HistoryStep<'_>> {
        labels
            .iter()
            .enumerate()
            .map(|(i, label)| HistoryStep {
                label,
                undone: i >= applied,
            })
            .collect()
    }

    fn row_state(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        row: aurora_widgets::WidgetId,
    ) -> ListRowState {
        match tree.payload(row) {
            Some(WidgetKind::ListRow(state)) => *state,
            other => unreachable!("a History row must be a ListRow, got {other:?}"),
        }
    }

    fn panel_tree() -> (
        aurora_widgets::WidgetTree<WidgetKind>,
        crate::panel::PanelHandle,
    ) {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        (tree, panel)
    }

    fn labels_of(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        panel: crate::panel::PanelHandle,
    ) -> Vec<String> {
        tree.children(panel.body)
            .unwrap_or_default()
            .iter()
            .map(|&row| {
                tree.accessibility(row)
                    .and_then(accesskit::Node::label)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn populate_history_panel_adds_an_origin_row_then_one_row_per_step_in_order() {
        let labels = history_with(3);
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        if let Err(err) =
            populate_history_panel(&mut tree, panel, &scales, "Open", &applied(&labels))
        {
            unreachable!("{err:?}");
        }

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);

        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 4, "origin + three steps");
        let mut expected = vec!["Open".to_owned()];
        expected.extend(labels.iter().cloned());
        assert_eq!(labels_of(&tree, panel), expected);
        for &row in rows {
            let Some(accessibility) = tree.accessibility(row) else {
                unreachable!("just inserted");
            };
            assert_eq!(accessibility.role(), accesskit::Role::ListItem);
        }
    }

    /// AC-2: the newest applied step is the one selected row, every
    /// undone step after it is disabled (dimmed), and the returned id is
    /// that selected row.
    #[test]
    fn the_current_step_is_selected_and_the_undone_steps_after_it_are_dimmed() {
        let labels = history_with(4);
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        let current =
            match populate_history_panel(&mut tree, panel, &scales, "Open", &split(&labels, 2)) {
                Ok(current) => current,
                Err(err) => unreachable!("{err:?}"),
            };
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 5);
        let states: Vec<_> = rows.iter().map(|&row| row_state(&tree, row)).collect();
        assert_eq!(
            states
                .iter()
                .map(|state| (state.selected, state.disabled))
                .collect::<Vec<_>>(),
            [
                (false, false),
                (false, false),
                (true, false),
                (false, true),
                (false, true)
            ],
            "origin, step 0, step 1 (current), then two undone steps"
        );
        assert_eq!(rows.get(2), Some(&current));
    }

    /// With nothing applied the origin row is current and every step is
    /// undone.
    #[test]
    fn with_every_step_undone_the_origin_row_is_current() {
        let labels = history_with(2);
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        let current =
            match populate_history_panel(&mut tree, panel, &scales, "Open", &split(&labels, 0)) {
                Ok(current) => current,
                Err(err) => unreachable!("{err:?}"),
            };
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.first(), Some(&current));
        assert!(row_state(&tree, current).selected);
        for &row in rows.iter().skip(1) {
            let state = row_state(&tree, row);
            assert!(state.disabled && !state.selected, "{state:?}");
        }
    }

    /// AC-4: the current row reports `selected`, other rows report
    /// not-selected, and an undone row keeps its plain label and carries
    /// the "Undone" state description instead.
    #[test]
    fn rows_report_selection_and_undone_state_to_accessibility() {
        let labels = vec!["Brush Stroke".to_owned(), "Eraser Stroke".to_owned()];
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        if let Err(err) = populate_history_panel(
            &mut tree,
            panel,
            &scales,
            "New Document",
            &split(&labels, 1),
        ) {
            unreachable!("{err:?}");
        }
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        let node = |i: usize| match rows.get(i).and_then(|&row| tree.accessibility(row)) {
            Some(node) => node,
            None => unreachable!("row {i} exists"),
        };
        assert_eq!(node(0).label(), Some("New Document"));
        assert_eq!(node(0).is_selected(), Some(false));
        assert_eq!(node(1).label(), Some("Brush Stroke"));
        assert_eq!(node(1).is_selected(), Some(true));
        assert_eq!(node(1).state_description(), None);
        assert_eq!(node(2).label(), Some("Eraser Stroke"), "no label suffix");
        assert_eq!(node(2).is_selected(), Some(false));
        assert_eq!(node(2).state_description(), Some(UNDONE_STATE));
        assert!(!node(2).is_disabled(), "redo can still reach it");
    }

    /// The rows draw their labels (before 0.147.1 a History row's text
    /// reached accessibility only): the current row in `text.on_accent`,
    /// an undone one in `text.disabled`, and the rest in `text.primary`.
    #[test]
    fn history_rows_draw_their_labels_with_the_state_colours() {
        const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
        const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
        let labels = history_with(2);
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        if let Err(err) =
            populate_history_panel(&mut tree, panel, &scales, "Open", &split(&labels, 1))
        {
            unreachable!("{err:?}");
        }
        let Ok(palette) = aurora_theme::Palette::from_toml_str(PALETTE_TOML) else {
            unreachable!("the committed palette parses");
        };
        let mut themes = aurora_theme::ThemeSet::new();
        if themes.register(DARK_THEME_TOML).is_err() {
            unreachable!("the committed Dark theme registers");
        }
        let theme = match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 21,
        };
        let colours: Vec<_> = rows
            .iter()
            .map(|&row| {
                let runs =
                    aurora_widgets::text_runs(&tree, row, bounds, bounds, None, &theme, &scales);
                assert_eq!(runs.len(), 1, "one drawn label per row");
                runs.first().map(|run| (run.text.clone(), run.color))
            })
            .collect();
        let rgba = |c: aurora_theme::Color| {
            let [r, g, b] = c.to_srgb_f32();
            [r, g, b, 1.0]
        };
        assert_eq!(
            colours,
            [
                Some(("Open".to_owned(), rgba(theme.text.primary))),
                Some((
                    labels.first().cloned().unwrap_or_default(),
                    rgba(theme.text.on_accent)
                )),
                Some((
                    labels.get(1).cloned().unwrap_or_default(),
                    rgba(theme.text.disabled)
                )),
            ]
        );
    }

    /// The other half of the `draws_label` opt-in (0.147.1 review): a
    /// read-only Properties row under the same kind of bare panel body
    /// does not opt in and still draws nothing, so its text is not drawn
    /// twice beside the tool-controls readout.
    #[test]
    fn a_properties_row_that_does_not_opt_in_draws_no_label() {
        const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
        const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
        let Ok(palette) = aurora_theme::Palette::from_toml_str(PALETTE_TOML) else {
            unreachable!("the committed palette parses");
        };
        let mut themes = aurora_theme::ThemeSet::new();
        if themes.register(DARK_THEME_TOML).is_err() {
            unreachable!("the committed Dark theme registers");
        }
        let theme = match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        };
        let (mut tree, panel) = panel_tree();
        let scales = test_scales();
        let options = [("Radius", "12px".to_owned())];
        if let Err(err) = crate::populate_properties_panel(
            &mut tree,
            panel,
            &scales,
            crate::Tool::Brush,
            &options,
        ) {
            unreachable!("{err:?}");
        }
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 1, "setup");
        let bounds = Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 21,
        };
        for row in rows {
            assert!(
                aurora_widgets::text_runs(&tree, row, bounds, bounds, None, &theme, &scales)
                    .is_empty(),
                "a row without draws_label draws no text"
            );
        }
    }

    /// AC-3's bound: past `MAX_DESCRIPTIONS` steps the oldest are folded
    /// into a notice row, and a current step that would fall off the
    /// front pulls the window back to it, with a trailing notice.
    #[test]
    fn a_long_history_is_capped_and_never_cuts_off_the_current_step() {
        let labels = history_with(1005);
        let scales = test_scales();

        let (mut tree, panel) = panel_tree();
        let current =
            match populate_history_panel(&mut tree, panel, &scales, "Open", &applied(&labels)) {
                Ok(current) => current,
                Err(err) => unreachable!("{err:?}"),
            };
        let shown = labels_of(&tree, panel);
        assert_eq!(shown.len(), 1001);
        assert_eq!(
            shown.first().map(String::as_str),
            Some("\u{2026} 5 earlier steps omitted")
        );
        assert_eq!(shown.last(), labels.last());
        assert_eq!(
            tree.children(panel.body).and_then(<[_]>::last),
            Some(&current)
        );

        let (mut tree, panel) = panel_tree();
        let current =
            match populate_history_panel(&mut tree, panel, &scales, "Open", &split(&labels, 2)) {
                Ok(current) => current,
                Err(err) => unreachable!("{err:?}"),
            };
        let shown = labels_of(&tree, panel);
        assert_eq!(shown.len(), 1002, "notice + 1000 steps + notice");
        assert_eq!(
            shown.first().map(String::as_str),
            Some("\u{2026} 1 earlier step omitted")
        );
        assert_eq!(
            shown.get(1),
            labels.get(1),
            "the current step leads the window"
        );
        assert_eq!(
            shown.last().map(String::as_str),
            Some("\u{2026} 4 later steps omitted")
        );
        let Some(rows) = tree.children(panel.body).map(<[_]>::to_vec) else {
            unreachable!("just populated");
        };
        for notice in [rows.first(), rows.last()] {
            let node = notice.and_then(|&row| tree.accessibility(row));
            assert_eq!(
                node.map(accesskit::Node::is_selected),
                Some(None),
                "a notice row is not a step and reports no selection state"
            );
        }
        assert_eq!(
            tree.children(panel.body).and_then(|rows| rows.get(1)),
            Some(&current)
        );
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
        let mut ws = crate::workspace::build_workspace(&test_scales());
        let scales = test_scales();
        if let Err(err) = populate_history_panel(
            &mut ws.tree,
            ws.history,
            &scales,
            "Open",
            &applied(&history),
        ) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.history.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 6, "an origin row plus one row per step");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let one_row = widgets::row_height(&scales) as u32;
        assert_eq!(one_row, 21, "13px of type plus 4px above and below");

        for &row in rows {
            assert!(
                matches!(
                    ws.tree.payload(row),
                    Some(&WidgetKind::ListRow(ListRowState {
                        disabled: false,
                        ..
                    }))
                ),
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
    ///
    /// The `before.height > 0` guard is the same one the Properties twin
    /// carries (0.77.5), added here for symmetry rather than because the
    /// class was uncovered: the equality below is trivially satisfied by
    /// zero-height rows all piled at one `y`, and since `0.77.4` both
    /// panels share one `crate::panel::row_style`, so one regression in
    /// that single function has to be visible from either panel's own
    /// tests. (`history_rows_are_real_list_row_widgets_with_a_hittable_
    /// size` above already asserts the exact height, so the class was
    /// caught — just not by this test.)
    #[test]
    fn history_rows_stack_top_to_bottom_without_overlapping() {
        let history = history_with(6);
        let mut ws = crate::workspace::build_workspace(&test_scales());
        let scales = test_scales();
        if let Err(err) = populate_history_panel(
            &mut ws.tree,
            ws.history,
            &scales,
            "Open",
            &applied(&history),
        ) {
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
            let mut ws = crate::workspace::build_workspace(&test_scales());
            let scales = test_scales();
            if let Err(err) = populate_history_panel(
                &mut ws.tree,
                ws.history,
                &scales,
                "Open",
                &applied(&history),
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

    /// Bounding the panel means rows that no longer fit are clipped:
    /// at scroll offset 0 they are not hittable. Since 0.145.0 the panel
    /// body scrolls (wheel/trackpad, scroll-into-view), so clipped no
    /// longer means unreachable — this pins only the unscrolled state.
    /// The rows that do fit really work.
    #[test]
    fn rows_past_the_bottom_of_a_bounded_history_panel_are_clipped_until_scrolled() {
        let history = history_with(200);
        let mut ws = crate::workspace::build_workspace(&test_scales());
        let scales = test_scales();
        if let Err(err) = populate_history_panel(
            &mut ws.tree,
            ws.history,
            &scales,
            "Open",
            &applied(&history),
        ) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);

        let Some(rows) = ws.tree.children(ws.history.body) else {
            unreachable!("just populated");
        };
        assert_eq!(
            rows.len(),
            201,
            "every step still gets a real row, after the origin row"
        );
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
        // being pinned is the arithmetic: a 300px panel share, less its title row, divided by
        // 21px rows, with no scrolling of any kind. Pinning the number
        // is what makes a silent change in the visible row count a test
        // failure rather than a shrug.
        // 300px of share less the 21px title slot (0.142.0) leaves a
        // 279px body: 13 whole 21px rows.
        assert_eq!(
            reachable, 13,
            "279px of History body (300px share less its 21px title row) divided by 21px rows \
             -- the rows that fit really work, and the other 188 are clipped until scrolled"
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
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) =
            populate_history_panel(&mut tree, panel, &scales, "Open", &applied(&history))
        {
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
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        for _ in 0..2 {
            if let Err(err) =
                populate_history_panel(&mut tree, panel, &scales, "Open", &applied(&history))
            {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(history.len() + 1),
            "a second call must replace the rows, not stack a second set beside them"
        );
    }

    #[test]
    fn populate_history_panel_rejects_an_unknown_panel_body() {
        let history: Vec<String> = Vec::new();
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(panel.body) {
            unreachable!("{err:?}");
        }
        let scales = test_scales();
        match populate_history_panel(&mut tree, panel, &scales, "Open", &applied(&history)) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
