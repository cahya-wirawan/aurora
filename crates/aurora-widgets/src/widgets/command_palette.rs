//! A searchable list of commands, filtered as the user types — PLAN.md
//! M1.8's "command palette" deliverable. Only the generic mechanism
//! lives here: filtering, keyboard-driven selection, and the
//! accessibility shape a screen reader needs (a `Role::TextInput` query
//! field holding a `Role::ListBox` of `Role::ListBoxOption` results).
//! The actual list of commands (what a "Toggle Layers Panel" or "Undo"
//! command *does*) is `aurora-app`'s own, application-specific concern —
//! this module doesn't know it exists, the same "generic toolkit vs
//! Aurora-specific content" boundary [`super`]'s own doc comment already
//! draws (`aurora-ui`'s `layers_panel`/`history_panel` are the
//! document-aware analogue one layer up).
//!
//! **Open/close aren't operations this module exposes** — inserting a
//! command palette ([`insert_command_palette`]) *is* opening it, and
//! `WidgetTree::remove`-ing its root *is* closing it; there's no
//! separate visibility flag to keep in sync, the same "real tree nodes,
//! not a hidden flag" shape this crate already prefers.
//!
//! **Filtering** is a case-insensitive substring match against each
//! command's title, not fuzzy subsequence scoring (VS Code/Sublime-style
//! matching) — real, separate follow-on work if a plain substring match
//! turns out not to be good enough in practice.
//!
//! **No rendering**: same boundary every widget in this crate keeps —
//! layout + accessibility content only.

use accesskit::{Action, Node, Role};
use taffy::style_helpers::{auto, percent};
use taffy::{FlexDirection, Size, Style};

use crate::error::WidgetError;
use crate::shortcut::KeyChord;
use crate::tree::{WidgetId, WidgetTree};

use super::{ListRowState, WidgetKind};

/// One entry a [`CommandPaletteState`] can offer — a title plus an
/// optional shortcut label shown alongside it. Purely informational: a
/// [`crate::shortcut::ShortcutRegistry`] is what actually dispatches a
/// keystroke to a command; this is just what the palette displays next
/// to a matching result.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandEntry {
    /// Opaque to this crate — `aurora-app` defines what its own ids mean
    /// and matches on them after reading [`CommandPaletteState::selected`].
    pub id: String,
    pub title: String,
    pub shortcut: Option<KeyChord>,
}

impl CommandEntry {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            shortcut: None,
        }
    }

    #[must_use]
    pub fn with_shortcut(mut self, shortcut: KeyChord) -> Self {
        self.shortcut = Some(shortcut);
        self
    }
}

/// A command palette's own state — the payload of its root widget
/// ([`WidgetKind::CommandPalette`]). The filtered result rows themselves
/// are real, separate [`WidgetTree`] nodes (the same "real nested tree
/// nodes, not state hidden in one payload" shape `aurora-ui`'s layer
/// rows already use), tracked here only by id so
/// [`set_command_palette_query`] knows what to remove before rebuilding.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandPaletteState {
    body: WidgetId,
    commands: Vec<CommandEntry>,
    query: String,
    /// Indices into `commands` that matched the current `query`, in
    /// display order — parallel to `rows`.
    filtered: Vec<usize>,
    /// The real row widget shown for each `filtered` entry, same length
    /// and order as `filtered`.
    rows: Vec<WidgetId>,
    /// Index into `filtered`/`rows`, not into `commands` directly —
    /// `None` only when `filtered` is empty.
    selected: Option<usize>,
}

impl CommandPaletteState {
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn commands(&self) -> &[CommandEntry] {
        &self.commands
    }

    /// The results currently matching [`Self::query`], in display order.
    #[must_use]
    pub fn results(&self) -> Vec<&CommandEntry> {
        self.filtered
            .iter()
            .filter_map(|&index| self.commands.get(index))
            .collect()
    }

    /// The currently-selected result, if any (`None` when nothing
    /// matches the current query).
    #[must_use]
    pub fn selected(&self) -> Option<&CommandEntry> {
        let index = self.selected?;
        let &command_index = self.filtered.get(index)?;
        self.commands.get(command_index)
    }

    /// The real row widget backing [`Self::selected`], if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<WidgetId> {
        let index = self.selected?;
        self.rows.get(index).copied()
    }

    #[must_use]
    pub fn body(&self) -> WidgetId {
        self.body
    }
}

/// The results list's own layout: `Column`, filling the root panel
/// (`percent(1.0)` on both axes) so its own rows stack top-to-bottom
/// instead of the `Row`-direction default every `Style::default()`
/// container otherwise gets — the same "labeled panel, `Column`,
/// `flex_grow`" shape `aurora_ui::insert_panel` already establishes for
/// stacking real content vertically.
fn body_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: percent(1.0_f32),
            height: percent(1.0_f32),
        },
        ..Default::default()
    }
}

/// One result row's own layout: full body width, sharing the body's
/// available height equally with every other row (`flex_grow: 1.0`
/// each — the same "N siblings dividing their container's height
/// evenly" idiom `aurora_ui::workspace`'s own docked-panel rail
/// already uses) rather than a fixed pixel height. Deliberately not a
/// `Scales`-derived literal: an even flexbox split needs no absolute
/// size at all, so this doesn't need `insert_command_palette`/
/// `rebuild_rows` to take a `&Scales` they otherwise have no use for.
fn row_style() -> Style {
    Style {
        flex_grow: 1.0,
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        ..Default::default()
    }
}

fn root_node(query: &str) -> Node {
    let mut node = Node::new(Role::TextInput);
    node.set_label("Command Palette");
    node.set_value(query.to_owned());
    node.add_action(Action::Focus);
    node
}

fn row_node(entry: &CommandEntry, selected: bool) -> Node {
    let mut node = Node::new(Role::ListBoxOption);
    node.set_label(entry.title.clone());
    if let Some(shortcut) = &entry.shortcut {
        node.set_description(shortcut.to_string());
    }
    node.set_selected(selected);
    node
}

/// Inserts a new command palette as a child of `parent`: a
/// `Role::TextInput` root (its `value` is always the current query text)
/// holding a `Role::ListBox` body, itself holding one
/// `Role::ListBoxOption` row per command in `commands` — the initial,
/// empty query matches everything, mirroring what a real search field
/// shows before the user types anything.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_command_palette(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    commands: Vec<CommandEntry>,
) -> Result<WidgetId, WidgetError> {
    let root = tree.insert(
        parent,
        Style::default(),
        root_node(""),
        WidgetKind::Container,
    )?;

    let mut body_node = Node::new(Role::ListBox);
    body_node.set_label("Results");
    let body = tree.insert(root, body_style(), body_node, WidgetKind::Container)?;

    let Some(payload) = tree.payload_mut(root) else {
        unreachable!("root was just inserted above");
    };
    *payload = WidgetKind::CommandPalette(CommandPaletteState {
        body,
        commands,
        query: String::new(),
        filtered: Vec::new(),
        rows: Vec::new(),
        selected: None,
    });

    rebuild_rows(tree, root, String::new())?;
    Ok(root)
}

fn state(
    tree: &WidgetTree<WidgetKind>,
    root: WidgetId,
) -> Result<&CommandPaletteState, WidgetError> {
    let kind = tree.payload(root).ok_or(WidgetError::UnknownWidget(root))?;
    match kind {
        WidgetKind::CommandPalette(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(root)),
    }
}

/// A read-only view of `root`'s own [`CommandPaletteState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `root` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a command
/// palette.
pub fn command_palette_state(
    tree: &WidgetTree<WidgetKind>,
    root: WidgetId,
) -> Result<&CommandPaletteState, WidgetError> {
    state(tree, root)
}

/// Re-filters `root`'s commands against `query` (case-insensitive
/// substring match against each command's own title) and rebuilds its
/// result rows to match — removes every existing row and inserts fresh
/// ones rather than diffing, the same one-shot-rebuild simplicity
/// `aurora-ui`'s panel population already accepts. Selection resets to
/// the first result, or to nothing when no command matches.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `root` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a command
/// palette. Nothing changes when this happens.
pub fn set_command_palette_query(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    query: &str,
) -> Result<(), WidgetError> {
    state(tree, root)?;
    rebuild_rows(tree, root, query.to_owned())
}

fn rebuild_rows(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    query: String,
) -> Result<(), WidgetError> {
    let current = state(tree, root)?;
    let body = current.body;
    let commands = current.commands.clone();
    let old_rows = current.rows.clone();

    let needle = query.to_ascii_lowercase();
    let filtered: Vec<usize> = commands
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            needle.is_empty() || entry.title.to_ascii_lowercase().contains(&needle)
        })
        .map(|(index, _)| index)
        .collect();

    for row in old_rows {
        tree.remove(row)?;
    }

    let mut rows = Vec::with_capacity(filtered.len());
    for (position, &command_index) in filtered.iter().enumerate() {
        let Some(entry) = commands.get(command_index) else {
            unreachable!("filtered indices were derived from commands above");
        };
        let row = tree.insert(
            body,
            row_style(),
            row_node(entry, position == 0),
            WidgetKind::ListRow(ListRowState {
                selected: position == 0,
                disabled: false,
            }),
        )?;
        rows.push(row);
    }

    tree.set_accessibility(root, root_node(&query))?;

    let Some(WidgetKind::CommandPalette(state)) = tree.payload_mut(root) else {
        unreachable!("state(tree, root) above already confirmed this is a command palette");
    };
    state.selected = if rows.is_empty() { None } else { Some(0) };
    state.query = query;
    state.filtered = filtered;
    state.rows = rows;
    Ok(())
}

/// Moves `root`'s own selection cursor by one result, wrapping around —
/// `forward: true` for the next result (`ArrowDown`), `false` for the
/// previous one (`ArrowUp`). Returns the newly-selected row, or `None`
/// (a no-op) when no result currently matches the query.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `root` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a command
/// palette.
pub fn move_command_palette_selection(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    forward: bool,
) -> Result<Option<WidgetId>, WidgetError> {
    let current = state(tree, root)?;
    if current.rows.is_empty() {
        return Ok(None);
    }
    let len = current.rows.len();
    let next_index = match current.selected {
        Some(index) if forward => (index + 1) % len,
        Some(index) => (index + len - 1) % len,
        None if forward => 0,
        None => len - 1,
    };
    let old_row = current
        .selected
        .and_then(|index| current.rows.get(index).copied());
    let Some(&new_row) = current.rows.get(next_index) else {
        unreachable!("next_index is always < len by construction");
    };

    if let Some(row) = old_row {
        if let Some(WidgetKind::ListRow(row_state)) = tree.payload_mut(row) {
            row_state.selected = false;
        }
        if let Some(node) = tree.accessibility(row) {
            let mut updated = node.clone();
            updated.set_selected(false);
            tree.set_accessibility(row, updated)?;
        }
    }
    if let Some(WidgetKind::ListRow(row_state)) = tree.payload_mut(new_row) {
        row_state.selected = true;
    }
    if let Some(node) = tree.accessibility(new_row) {
        let mut updated = node.clone();
        updated.set_selected(true);
        tree.set_accessibility(new_row, updated)?;
    }

    let Some(WidgetKind::CommandPalette(state)) = tree.payload_mut(root) else {
        unreachable!("state(tree, root) above already confirmed this is a command palette");
    };
    state.selected = Some(next_index);
    Ok(Some(new_row))
}

#[cfg(test)]
mod tests {
    use super::{
        CommandEntry, command_palette_state, insert_command_palette,
        move_command_palette_selection, set_command_palette_query,
    };
    use crate::WidgetError;
    use crate::shortcut::KeyChord;
    use crate::widgets::{WidgetKind, new_tree};
    use taffy::Style;

    fn commands() -> Vec<CommandEntry> {
        vec![
            CommandEntry::new("view.layers", "Focus Layers Panel"),
            CommandEntry::new("view.history", "Focus History Panel"),
            CommandEntry::new("edit.undo", "Undo").with_shortcut(match KeyChord::parse("Ctrl+Z") {
                Ok(chord) => chord,
                Err(err) => unreachable!("{err:?}"),
            }),
        ]
    }

    #[test]
    fn insert_command_palette_starts_with_every_command_matching_the_empty_query() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(accessibility) = tree.accessibility(palette) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TextInput);
        assert_eq!(accessibility.value(), Some(""));

        let state = match command_palette_state(&tree, palette) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.results().len(), 3);
        assert_eq!(state.selected().map(|c| c.id.as_str()), Some("view.layers"));

        let Some(rows) = tree.children(state.body()) else {
            unreachable!("just inserted");
        };
        assert_eq!(rows.len(), 3);
        let Some(&first_row) = rows.first() else {
            unreachable!("just asserted len() == 3");
        };
        let Some(first_accessibility) = tree.accessibility(first_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(first_accessibility.role(), accesskit::Role::ListBoxOption);
        assert_eq!(first_accessibility.label(), Some("Focus Layers Panel"));
        assert_eq!(first_accessibility.is_selected(), Some(true));
    }

    #[test]
    fn insert_command_palette_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match insert_command_palette(&mut tree, bogus, commands()) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_query_narrows_results_to_a_case_insensitive_title_match() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = set_command_palette_query(&mut tree, palette, "undo") {
            unreachable!("{err:?}");
        }

        let state = match command_palette_state(&tree, palette) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.query(), "undo");
        let results = state.results();
        assert_eq!(results.len(), 1);
        assert_eq!(results.first().map(|c| c.id.as_str()), Some("edit.undo"));

        let Some(accessibility) = tree.accessibility(palette) else {
            unreachable!("just queried");
        };
        assert_eq!(accessibility.value(), Some("undo"));
    }

    #[test]
    fn set_query_matching_nothing_leaves_no_rows_and_no_selection() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_command_palette_query(&mut tree, palette, "nonexistent") {
            unreachable!("{err:?}");
        }

        let state = match command_palette_state(&tree, palette) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(state.results().is_empty());
        assert_eq!(state.selected(), None);
        assert_eq!(tree.children(state.body()), Some([].as_slice()));
    }

    #[test]
    fn narrowing_the_query_removes_stale_rows_from_the_tree() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let body = match command_palette_state(&tree, palette) {
            Ok(state) => state.body(),
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(body).map(<[_]>::len), Some(3));

        if let Err(err) = set_command_palette_query(&mut tree, palette, "focus") {
            unreachable!("{err:?}");
        }
        // "Undo" doesn't match "focus" -- its row must be gone, not just
        // unlisted in `results()`.
        assert_eq!(tree.children(body).map(<[_]>::len), Some(2));
    }

    #[test]
    fn move_selection_wraps_forward_and_backward() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let selected_id = |tree: &crate::WidgetTree<WidgetKind>| {
            command_palette_state(tree, palette)
                .ok()
                .and_then(super::CommandPaletteState::selected)
                .map(|c| c.id.clone())
        };

        assert_eq!(selected_id(&tree).as_deref(), Some("view.layers"));
        if let Err(err) = move_command_palette_selection(&mut tree, palette, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(selected_id(&tree).as_deref(), Some("view.history"));
        if let Err(err) = move_command_palette_selection(&mut tree, palette, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(selected_id(&tree).as_deref(), Some("edit.undo"));
        if let Err(err) = move_command_palette_selection(&mut tree, palette, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            selected_id(&tree).as_deref(),
            Some("view.layers"),
            "must wrap back to the first result"
        );

        if let Err(err) = move_command_palette_selection(&mut tree, palette, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            selected_id(&tree).as_deref(),
            Some("edit.undo"),
            "moving backward from the first result must wrap to the last"
        );
    }

    #[test]
    fn move_selection_updates_the_selected_accessibility_flag_on_the_real_rows() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let (first_row, second_row) = {
            let state = match command_palette_state(&tree, palette) {
                Ok(state) => state,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(rows) = tree.children(state.body()) else {
                unreachable!("just inserted");
            };
            let (Some(&a), Some(&b)) = (rows.first(), rows.get(1)) else {
                unreachable!("3 commands were inserted above");
            };
            (a, b)
        };

        if let Err(err) = move_command_palette_selection(&mut tree, palette, true) {
            unreachable!("{err:?}");
        }

        let Some(first_accessibility) = tree.accessibility(first_row) else {
            unreachable!("still exists");
        };
        assert_eq!(first_accessibility.is_selected(), Some(false));
        let Some(second_accessibility) = tree.accessibility(second_row) else {
            unreachable!("still exists");
        };
        assert_eq!(second_accessibility.is_selected(), Some(true));
    }

    #[test]
    fn move_selection_is_a_no_op_when_nothing_matches() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_command_palette_query(&mut tree, palette, "nonexistent") {
            unreachable!("{err:?}");
        }
        match move_command_palette_selection(&mut tree, palette, true) {
            Ok(None) => {}
            other => unreachable!("expected Ok(None), got {other:?}"),
        }
    }

    #[test]
    fn command_palette_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_command_palette_query(&mut tree, root, "x") {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn rows_are_real_list_row_widgets_with_only_the_first_selected() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let body = match command_palette_state(&tree, palette) {
            Ok(state) => state.body(),
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(rows) = tree.children(body) else {
            unreachable!("just inserted");
        };
        let (Some(&first), Some(&second)) = (rows.first(), rows.get(1)) else {
            unreachable!("3 commands were inserted above");
        };
        assert_eq!(
            tree.payload(first),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: true,
                disabled: false,
            }))
        );
        assert_eq!(
            tree.payload(second),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: false,
                disabled: false,
            }))
        );
    }

    #[test]
    fn move_selection_updates_the_list_row_payloads_own_selected_flag_too() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let (first_row, second_row) = {
            let state = match command_palette_state(&tree, palette) {
                Ok(state) => state,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(rows) = tree.children(state.body()) else {
                unreachable!("just inserted");
            };
            let (Some(&a), Some(&b)) = (rows.first(), rows.get(1)) else {
                unreachable!("3 commands were inserted above");
            };
            (a, b)
        };

        if let Err(err) = move_command_palette_selection(&mut tree, palette, true) {
            unreachable!("{err:?}");
        }

        assert_eq!(
            tree.payload(first_row),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: false,
                disabled: false,
            })),
            "the row moved away from must clear its own payload flag, not just its accessibility \
             one -- paint_widget reads the payload"
        );
        assert_eq!(
            tree.payload(second_row),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: true,
                disabled: false,
            }))
        );
    }

    /// Headless (no GPU) proof that the real layout `body_style`/
    /// `row_style` apply actually produces non-degenerate, vertically
    /// stacked row bounds -- written *before* asking for a gallery
    /// bless, the same "prove the layout half without a GPU first"
    /// discipline `command_palette_style_positions_the_panel_with_a_
    /// real_margin` (`tests/gallery.rs`) already established. Without
    /// this, `paint_list_row`'s own highlight would tessellate to a
    /// real mesh but at a degenerate zero-size rect -- invisible, not a
    /// compile or logic error, so only a real bounds assertion catches
    /// it.
    #[test]
    fn results_stack_vertically_and_share_the_panels_height() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = match insert_command_palette(&mut tree, root, commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_style(
            palette,
            Style {
                size: taffy::Size {
                    width: taffy::style_helpers::length(300.0_f32),
                    height: taffy::style_helpers::length(210.0_f32),
                },
                ..Default::default()
            },
        ) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(400.0, 300.0);

        let body = match command_palette_state(&tree, palette) {
            Ok(state) => state.body(),
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(rows) = tree.children(body) else {
            unreachable!("just inserted");
        };
        assert_eq!(rows.len(), 3, "all 3 commands match the empty query");

        let mut previous_bottom = None;
        for &row in rows {
            let Some(bounds) = tree.bounds(row) else {
                unreachable!("just laid out");
            };
            assert!(bounds.width > 0, "a row must span real width: {bounds:?}");
            assert!(bounds.height > 0, "a row must span real height: {bounds:?}");
            if let Some(previous_bottom) = previous_bottom {
                assert_eq!(
                    bounds.y, previous_bottom,
                    "rows must stack directly on top of one another, no gap or overlap"
                );
            }
            previous_bottom = Some(bounds.y + i64::from(bounds.height));
        }
        assert_eq!(
            previous_bottom,
            tree.bounds(body).map(|b| b.y + i64::from(b.height)),
            "the 3 rows together must exactly fill the body's own height"
        );
    }

    #[test]
    fn command_palette_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match set_command_palette_query(&mut tree, bogus, "x") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
