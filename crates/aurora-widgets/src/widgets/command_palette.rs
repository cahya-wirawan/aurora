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
//! **Layout (0.257.0 rewrite; before it, every row shared the body's
//! height and, with a long list, drew on top of its neighbours).** The
//! tree is
//!
//! ```text
//! root (CommandPalette payload; the caller's style: size, position)
//! └─ frame            Column, full width, as tall as its content
//!    ├─ query strip   fixed height, never shrinks, draws the query
//!    └─ viewport      Row, grows/shrinks inside the frame, at most
//!       │             `size.content_panel_max_rows` rows tall, clips
//!       ├─ body       the ListBox: a scroll container holding the rows
//!       │  └─ rows    one fixed `row_height` each, never shrink
//!       └─ scrollbar  a linked scrollbar, hidden while the rows fit
//! ```
//!
//! so a palette with few results is as tall as its strip plus its rows,
//! one with many stops at the cap and scrolls, and no row is ever
//! squeezed below its own height. The caller sizes only the root (the
//! app gives it a width, a top inset and an `auto` height). The row,
//! strip and cap come from the `Scales` passed to
//! [`insert_command_palette`] (`row_height`, `spacing.xs`,
//! `size.content_panel_max_rows`) — the same tokens the dock panels'
//! rows and their scrolling cap use.
//!
//! **Keyboard**: [`move_command_palette_selection`] (arrows, wrapping),
//! [`step_command_palette_selection`] (page, first, last: no wrap) and
//! [`select_command_palette_at`] (a pointer press) all keep the selected
//! row scrolled into view. A new query or command list resets the scroll
//! to the top (the selection resets to the first result).
//!
//! **Rendering** lives in `crate::paint`/`crate::text`, like every
//! widget's: the root paints its panel, the query strip
//! ([`CommandPaletteState::query_strip`]) draws the typed query with a
//! caret at its end, and each result row draws its command's title
//! ([`CommandPaletteState::row_title`]); rows scrolled out of the
//! viewport are clipped by the viewport like any scrolled panel row. The
//! strip's caret shows while focus is anywhere within the palette (the
//! palette itself or one of its rows).

use accesskit::{Action, Node, Orientation, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{TaffyZero, auto, length, percent};
use taffy::{Dimension, FlexDirection, Overflow, Size, Style};

use crate::error::WidgetError;
use crate::shortcut::KeyChord;
use crate::tree::{WidgetId, WidgetTree};

use super::{ListRowState, ScrollbarRange, WidgetKind, insert_scrollbar, link_scrollbar};

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

/// How [`step_command_palette_selection`] moves the selection. None of
/// them wraps (an arrow key's wrapping is
/// [`move_command_palette_selection`]'s own behaviour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteStep {
    /// One result down, stopping at the last.
    Next,
    /// One result up, stopping at the first.
    Previous,
    /// A page down: as many results as the viewport shows (at least
    /// one), stopping at the last.
    PageDown,
    /// A page up, stopping at the first.
    PageUp,
    /// The first result.
    First,
    /// The last result.
    Last,
}

/// A command palette's own state — the payload of its root widget
/// ([`WidgetKind::CommandPalette`]). The filtered result rows themselves
/// are real, separate [`WidgetTree`] nodes (the same "real nested tree
/// nodes, not state hidden in one payload" shape `aurora-ui`'s layer
/// rows already use), tracked here only by id so
/// [`set_command_palette_query`] knows what to remove before rebuilding.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandPaletteState {
    /// The results' scroll container (the `Role::ListBox`): the rows'
    /// parent.
    body: WidgetId,
    /// The viewport row holding [`Self::body`] and [`Self::scrollbar`].
    viewport: WidgetId,
    /// The linked scrollbar beside the rows.
    scrollbar: WidgetId,
    /// The first child of the frame, drawing the query (0.133.0).
    query_strip: WidgetId,
    /// Every result row's height (`row_height`), kept for paging.
    row_height: f32,
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
    /// The root's accessible name — "Command Palette", or a prompt's own
    /// label ([`insert_command_prompt`], 0.168.0).
    label: String,
    /// `false` for a prompt: every command is shown whatever the query
    /// (the query is free text the caller reads, not a search).
    filtering: bool,
    /// A message the root's accessible description carries — a prompt's
    /// validation message, say ([`set_command_palette_message`]).
    message: Option<String>,
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

    /// The root's accessible name ("Command Palette", or a prompt's label).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the query filters the commands — `false` for a prompt
    /// ([`insert_command_prompt`]).
    #[must_use]
    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    /// The message the root's accessible description carries, if any
    /// ([`set_command_palette_message`]).
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
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

    /// The index of [`Self::selected`] among [`Self::results`].
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// The results list: the `Role::ListBox` scroll container whose
    /// children are exactly the result rows (the query strip is not in
    /// it, since 0.257.0).
    #[must_use]
    pub fn body(&self) -> WidgetId {
        self.body
    }

    /// The viewport row clipping [`Self::body`] (and holding the
    /// scrollbar): its height is what limits the palette's height.
    #[must_use]
    pub fn viewport(&self) -> WidgetId {
        self.viewport
    }

    /// The scrollbar linked to [`Self::body`]; hidden while the rows fit.
    #[must_use]
    pub fn scrollbar(&self) -> WidgetId {
        self.scrollbar
    }

    /// The query strip: a fixed-height row above the results (0.257.0;
    /// before it shared the rows' height), drawing the query text with a
    /// caret at its end. It stays when no command matches.
    #[must_use]
    pub fn query_strip(&self) -> WidgetId {
        self.query_strip
    }

    /// The result rows, in display order (the children of
    /// [`Self::body`]).
    #[must_use]
    pub fn rows(&self) -> &[WidgetId] {
        &self.rows
    }

    /// Every result row's height in logical px (`row_height`).
    #[must_use]
    pub fn row_height(&self) -> f32 {
        self.row_height
    }

    /// The title of the command result row `row` shows, or `None` if
    /// `row` is not one of this palette's current rows.
    #[must_use]
    pub fn row_title(&self, row: WidgetId) -> Option<&str> {
        let position = self.rows.iter().position(|id| *id == row)?;
        let &command = self.filtered.get(position)?;
        self.commands.get(command).map(|entry| entry.title.as_str())
    }
}

/// The palette enclosing `id` (the root itself, the strip, the body, a
/// row ...), with its root id: the walk up from `id` is at most six
/// levels (a row is four below the root). For `crate::text`, which finds
/// what to draw from a widget.
pub(crate) fn enclosing_palette(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Option<(WidgetId, &CommandPaletteState)> {
    let mut current = Some(id);
    for _ in 0..6 {
        let candidate = current?;
        if let Some(WidgetKind::CommandPalette(state)) = tree.payload(candidate) {
            return Some((candidate, state));
        }
        current = tree.parent(candidate);
    }
    None
}

/// The frame under the root: a `Column` as wide as the root and as tall
/// as its content — or, when the caller gave the root a definite height,
/// as tall as that (the root's cross-axis stretch).
fn frame_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        ..Default::default()
    }
}

/// The query strip's height: one row plus one `spacing.xs` of air, never
/// shrunk by a crowded palette.
fn strip_height(scales: &Scales) -> f32 {
    super::row_height(scales) + super::spacing(scales.spacing.xs)
}

fn strip_style(scales: &Scales) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: length(strip_height(scales)),
        },
        ..Default::default()
    }
}

/// How many rows the viewport shows before it scrolls:
/// `size.content_panel_max_rows`, the token the dock panels' scrolling
/// cap already is, at least one.
fn max_visible_rows(scales: &Scales) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let rows = scales.size.content_panel_max_rows as f32;
    rows.max(1.0)
}

/// The viewport: it takes the rows' height up to the cap, then clips and
/// the rows scroll. It may shrink below its basis (a root with a smaller
/// definite height) and never needs more than the cap. Like a dock
/// panel's viewport, a clipping scroll container's own minimum is zero,
/// so the rows' total height never floors the palette.
fn viewport_style(scales: &Scales) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        flex_grow: 1.0,
        flex_shrink: 1.0,
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        max_size: Size {
            width: auto(),
            height: length(max_visible_rows(scales) * super::row_height(scales)),
        },
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// The results list's own layout: a `Column` scroll container that
/// fills the viewport beside the scrollbar, its rows stacking
/// top-to-bottom at their own height.
fn body_style() -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        flex_grow: 1.0,
        flex_basis: Dimension::ZERO,
        min_size: Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// One result row's own layout: full body width, exactly `row_height`
/// tall, and never shrunk — the 0.133.0 layout shared the body's height
/// evenly (`flex_grow`) and, past a few dozen commands, drew every row
/// on top of its neighbours.
fn row_style(row_height: f32) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: length(row_height),
        },
        ..Default::default()
    }
}

/// The command palette's own accessible name.
const PALETTE_LABEL: &str = "Command Palette";

fn root_node(label: &str, query: &str, message: Option<&str>) -> Node {
    let mut node = Node::new(Role::TextInput);
    node.set_label(label.to_owned());
    node.set_value(query.to_owned());
    if let Some(message) = message {
        node.set_description(message.to_owned());
    }
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
/// holding a fixed-height query strip and a scrolling `Role::ListBox`
/// with one `Role::ListBoxOption` row per command in `commands` — the
/// initial, empty query matches everything, mirroring what a real search
/// field shows before the user types anything. `scales` gives the row
/// height, the strip height and the number of rows shown before it
/// scrolls (see the module docs); the caller sizes and positions the
/// root with [`WidgetTree::set_style`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_command_palette(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    commands: Vec<CommandEntry>,
    scales: &Scales,
) -> Result<WidgetId, WidgetError> {
    insert_palette(
        tree,
        parent,
        PALETTE_LABEL.to_owned(),
        true,
        commands,
        scales,
    )
}

/// Inserts a *prompt* (0.168.0): the same widget as
/// [`insert_command_palette`] — a `Role::TextInput` root named `label`
/// whose value is the typed text, holding a `Role::ListBox` of rows — but
/// the query never filters `commands`: it is free text the caller reads
/// back ([`CommandPaletteState::query`]) when a row is activated, a name
/// to save something under, say. Typing, selection and closing work
/// exactly as in the palette; what a row means is the caller's.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_command_prompt(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    label: impl Into<String>,
    commands: Vec<CommandEntry>,
    scales: &Scales,
) -> Result<WidgetId, WidgetError> {
    insert_palette(tree, parent, label.into(), false, commands, scales)
}

fn insert_palette(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    label: String,
    filtering: bool,
    commands: Vec<CommandEntry>,
    scales: &Scales,
) -> Result<WidgetId, WidgetError> {
    let root = tree.insert(
        parent,
        Style::default(),
        root_node(&label, "", None),
        WidgetKind::Container,
    )?;

    let built = build_parts(tree, root, scales);
    let (body, viewport, scrollbar, query_strip) = match built {
        Ok(parts) => parts,
        Err(err) => {
            // Nothing is left behind when a part cannot be built.
            let _ = tree.remove(root);
            return Err(err);
        }
    };

    let Some(payload) = tree.payload_mut(root) else {
        unreachable!("root was just inserted above");
    };
    *payload = WidgetKind::CommandPalette(CommandPaletteState {
        body,
        viewport,
        scrollbar,
        query_strip,
        row_height: super::row_height(scales),
        commands,
        query: String::new(),
        filtered: Vec::new(),
        rows: Vec::new(),
        selected: None,
        label,
        filtering,
        message: None,
    });

    rebuild_rows(tree, root, String::new())?;
    Ok(root)
}

/// The palette's fixed skeleton under `root`: returns
/// `(body, viewport, scrollbar, query_strip)`.
fn build_parts(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> Result<(WidgetId, WidgetId, WidgetId, WidgetId), WidgetError> {
    let frame = tree.insert(
        root,
        frame_style(),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    // Presentational: the root's own `Role::TextInput` value already is
    // the query, so a labelled strip would announce it twice.
    let query_strip = tree.insert(
        frame,
        strip_style(scales),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    let viewport = tree.insert(
        frame,
        viewport_style(scales),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    let mut body_node = Node::new(Role::ListBox);
    body_node.set_label("Results");
    let body = tree.insert(viewport, body_style(), body_node, WidgetKind::Container)?;
    tree.set_scrollable(body, true)?;
    let scrollbar = insert_scrollbar(
        tree,
        viewport,
        scales,
        Orientation::Vertical,
        None,
        0.0,
        ScrollbarRange {
            min: 0.0,
            max: 0.0,
            page_size: 0.0,
        },
    )?;
    link_scrollbar(tree, scrollbar, body)?;
    Ok((body, viewport, scrollbar, query_strip))
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
/// the first result, or to nothing when no command matches, and the rows
/// scroll back to the top.
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

/// Replaces `root`'s commands with `commands` and rebuilds its rows
/// against the current query (0.168.0: a caller whose list depends on
/// state the palette cannot see, or a prompt whose rows describe what the
/// typed text would do). Selection resets to the first result.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `root` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a command
/// palette. Nothing changes when this happens.
pub fn set_command_palette_commands(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    commands: Vec<CommandEntry>,
) -> Result<(), WidgetError> {
    let query = state(tree, root)?.query.clone();
    let Some(WidgetKind::CommandPalette(current)) = tree.payload_mut(root) else {
        return Err(WidgetError::WrongWidgetKind(root));
    };
    current.commands = commands;
    rebuild_rows(tree, root, query)
}

/// Sets (or, with `None`, clears) the message `root`'s accessible
/// description carries (0.168.0) — a prompt's validation message, read
/// out with the field it describes.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `root` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a command
/// palette. Nothing changes when this happens.
pub fn set_command_palette_message(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    message: Option<String>,
) -> Result<(), WidgetError> {
    let current = state(tree, root)?;
    let node = root_node(&current.label, &current.query, message.as_deref());
    tree.set_accessibility(root, node)?;
    let Some(WidgetKind::CommandPalette(current)) = tree.payload_mut(root) else {
        return Err(WidgetError::WrongWidgetKind(root));
    };
    current.message = message;
    Ok(())
}

fn rebuild_rows(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    query: String,
) -> Result<(), WidgetError> {
    let current = state(tree, root)?;
    let body = current.body;
    let row_height = current.row_height;
    let query_strip = current.query_strip;
    let commands = current.commands.clone();
    let old_rows = current.rows.clone();
    let filtering = current.filtering;
    let label = current.label.clone();
    let message = current.message.clone();

    let needle = query.to_ascii_lowercase();
    let filtered: Vec<usize> = commands
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            !filtering || needle.is_empty() || entry.title.to_ascii_lowercase().contains(&needle)
        })
        .map(|(index, _)| index)
        .collect();

    for row in old_rows {
        tree.remove(row)?;
    }
    // New rows start at the top: the offset is reset while the list is
    // empty, so no stale row is shifted.
    tree.set_scroll_y(body, 0.0)?;

    let mut rows = Vec::with_capacity(filtered.len());
    for (position, &command_index) in filtered.iter().enumerate() {
        let Some(entry) = commands.get(command_index) else {
            unreachable!("filtered indices were derived from commands above");
        };
        let row = tree.insert(
            body,
            row_style(row_height),
            row_node(entry, position == 0),
            WidgetKind::ListRow(ListRowState {
                selected: position == 0,
                disabled: false,
                draws_label: false,
            }),
        )?;
        rows.push(row);
    }

    tree.set_accessibility(root, root_node(&label, &query, message.as_deref()))?;
    // The strip draws the query: repaint it.
    tree.mark_dirty(query_strip)?;

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
/// previous one (`ArrowUp`) — and scrolls it into view. Returns the
/// newly-selected row, or `None` (a no-op) when no result currently
/// matches the query.
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
    select_index(tree, root, next_index)
}

/// Moves the selection without wrapping — one result, a page, or to
/// either end ([`PaletteStep`]) — and scrolls it into view. A page is as
/// many results as the viewport's last layout showed, at least one.
/// Returns the newly-selected row, or `None` (a no-op) when no result
/// matches the query.
///
/// # Errors
///
/// As [`move_command_palette_selection`].
pub fn step_command_palette_selection(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    step: PaletteStep,
) -> Result<Option<WidgetId>, WidgetError> {
    let current = state(tree, root)?;
    let len = current.rows.len();
    if len == 0 {
        return Ok(None);
    }
    let last = len - 1;
    let from = current.selected.unwrap_or(0).min(last);
    let page = {
        let shown = tree.bounds(current.body).map_or(0, |b| b.height);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        #[allow(clippy::cast_precision_loss)]
        let rows = if current.row_height > 0.0 {
            (shown as f32 / current.row_height).floor() as usize
        } else {
            1
        };
        rows.max(1)
    };
    let target = match step {
        PaletteStep::Next => (from + 1).min(last),
        PaletteStep::Previous => from.saturating_sub(1),
        PaletteStep::PageDown => from.saturating_add(page).min(last),
        PaletteStep::PageUp => from.saturating_sub(page),
        PaletteStep::First => 0,
        PaletteStep::Last => last,
    };
    select_index(tree, root, target)
}

/// Selects the result row under `point` (window-logical px, the same
/// space as [`WidgetTree::bounds`]) — the pointer-press half of the
/// palette. Hit-testing sees the rows where scrolling put them, and a
/// row the viewport clips is not hit. Returns the newly-selected row, or
/// `None` (nothing changes) when `point` is not on a result row.
///
/// # Errors
///
/// As [`move_command_palette_selection`].
pub fn select_command_palette_at(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    point: (f32, f32),
) -> Result<Option<WidgetId>, WidgetError> {
    let current = state(tree, root)?;
    let Some(hit) = tree.hit_test(point) else {
        return Ok(None);
    };
    let Some(index) = current.rows.iter().position(|&row| row == hit) else {
        return Ok(None);
    };
    select_index(tree, root, index)
}

/// Makes row `index` the selected one: both highlight flags, the
/// accessibility `selected` flags and the scroll position.
fn select_index(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    index: usize,
) -> Result<Option<WidgetId>, WidgetError> {
    let current = state(tree, root)?;
    let Some(&new_row) = current.rows.get(index) else {
        return Ok(None);
    };
    let old_row = current
        .selected
        .and_then(|selected| current.rows.get(selected).copied());

    if old_row != Some(new_row) {
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
    }

    let Some(WidgetKind::CommandPalette(state)) = tree.payload_mut(root) else {
        unreachable!("state(tree, root) above already confirmed this is a command palette");
    };
    state.selected = Some(index);
    tree.scroll_into_view(new_row);
    Ok(Some(new_row))
}

#[cfg(test)]
mod tests {
    use super::{
        CommandEntry, PaletteStep, command_palette_state, insert_command_palette,
        insert_command_prompt, move_command_palette_selection, select_command_palette_at,
        set_command_palette_commands, set_command_palette_message, set_command_palette_query,
        step_command_palette_selection,
    };
    use crate::WidgetError;
    use crate::shortcut::KeyChord;
    use aurora_core::Rect;
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{WidgetKind, new_tree, row_height, test_scales};
    use taffy::Style;
    use taffy::style_helpers::{auto, length};

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

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn palette_in(
        tree: &mut WidgetTree<WidgetKind>,
        root: WidgetId,
        commands: Vec<CommandEntry>,
    ) -> WidgetId {
        ok(insert_command_palette(tree, root, commands, &test_scales()))
    }

    /// `aurora-app`'s `command_palette_style`: an absolutely placed,
    /// 480 px wide, auto-height root under a 96 px top inset.
    fn app_style() -> Style {
        Style {
            position: taffy::Position::Absolute,
            size: taffy::Size {
                width: length(480.0_f32),
                height: auto(),
            },
            inset: taffy::Rect {
                top: length(96.0_f32),
                left: length(0.0_f32),
                right: length(0.0_f32),
                bottom: auto(),
            },
            margin: taffy::Rect {
                left: auto(),
                right: auto(),
                top: length(0.0_f32),
                bottom: length(0.0_f32),
            },
            ..Default::default()
        }
    }

    /// A tree whose root is the window, as in the app.
    fn window_tree(width: f32, height: f32) -> (WidgetTree<WidgetKind>, WidgetId) {
        new_tree(Style {
            size: taffy::Size {
                width: length(width),
                height: length(height),
            },
            ..Default::default()
        })
    }

    fn many(count: usize) -> Vec<CommandEntry> {
        (0..count)
            .map(|n| CommandEntry::new(format!("cmd.{n}"), format!("Command {n}")))
            .collect()
    }

    /// A palette of `count` commands as the app opens it, laid out
    /// text-aware-style (`compute_text_layout`, which syncs the linked
    /// scrollbar) in a `width` x `height` logical window.
    fn opened(count: usize, width: f32, height: f32) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = window_tree(width, height);
        let palette = palette_in(&mut tree, root, many(count));
        ok(tree.set_style(palette, app_style()));
        crate::compute_text_layout(&mut tree, width, height, None);
        (tree, palette)
    }

    fn relayout(tree: &mut WidgetTree<WidgetKind>, width: f32, height: f32) {
        crate::compute_text_layout(tree, width, height, None);
    }

    fn bounds(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Rect {
        match tree.bounds(id) {
            Some(rect) => rect,
            None => unreachable!("laid out"),
        }
    }

    fn row_h() -> i64 {
        #[allow(clippy::cast_possible_truncation)]
        let h = row_height(&test_scales()) as i64;
        h
    }

    fn strip_h() -> i64 {
        row_h() + i64::from(test_scales().spacing.xs)
    }

    fn visible_cap() -> i64 {
        i64::from(test_scales().size.content_panel_max_rows)
    }

    fn selected_index(tree: &WidgetTree<WidgetKind>, palette: WidgetId) -> Option<usize> {
        ok(command_palette_state(tree, palette)).selected_index()
    }

    fn selected_id(tree: &WidgetTree<WidgetKind>, palette: WidgetId) -> Option<String> {
        ok(command_palette_state(tree, palette))
            .selected()
            .map(|c| c.id.clone())
    }

    fn rows_of(tree: &WidgetTree<WidgetKind>, palette: WidgetId) -> Vec<WidgetId> {
        ok(command_palette_state(tree, palette)).rows().to_vec()
    }

    /// The selected row lies wholly inside the viewport.
    fn selected_is_inside_viewport(tree: &WidgetTree<WidgetKind>, palette: WidgetId) -> bool {
        let state = ok(command_palette_state(tree, palette));
        let Some(row) = state.selected_row() else {
            return false;
        };
        let (r, v) = (bounds(tree, row), bounds(tree, state.viewport()));
        r.y >= v.y && r.bottom() <= v.bottom()
    }

    #[test]
    fn insert_command_palette_starts_with_every_command_matching_the_empty_query() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());

        let Some(accessibility) = tree.accessibility(palette) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TextInput);
        assert_eq!(accessibility.value(), Some(""));

        let state = ok(command_palette_state(&tree, palette));
        assert_eq!(state.results().len(), 3);
        assert_eq!(state.selected().map(|c| c.id.as_str()), Some("view.layers"));

        let Some(rows) = tree.children(state.body()) else {
            unreachable!("just inserted");
        };
        assert_eq!(rows.len(), 3, "the body holds exactly the result rows");
        let Some(&first_row) = rows.first() else {
            unreachable!("just asserted len() == 3");
        };
        let Some(first_accessibility) = tree.accessibility(first_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(first_accessibility.role(), accesskit::Role::ListBoxOption);
        assert_eq!(first_accessibility.label(), Some("Focus Layers Panel"));
        assert_eq!(first_accessibility.is_selected(), Some(true));
        let Some(body_accessibility) = tree.accessibility(state.body()) else {
            unreachable!("just inserted");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::ListBox);
    }

    #[test]
    fn insert_command_palette_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match insert_command_palette(&mut tree, bogus, commands(), &test_scales()) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "nothing is left behind");
    }

    #[test]
    fn set_query_narrows_results_to_a_case_insensitive_title_match() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        ok(set_command_palette_query(&mut tree, palette, "undo"));

        let state = ok(command_palette_state(&tree, palette));
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
        let palette = palette_in(&mut tree, root, commands());
        ok(set_command_palette_query(&mut tree, palette, "nonexistent"));

        let state = ok(command_palette_state(&tree, palette));
        assert!(state.results().is_empty());
        assert_eq!(state.selected(), None);
        assert_eq!(tree.children(state.body()).map(<[_]>::len), Some(0));
        assert!(tree.contains(state.query_strip()), "the strip stays");
    }

    #[test]
    fn narrowing_the_query_removes_stale_rows_from_the_tree() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        let body = ok(command_palette_state(&tree, palette)).body();
        assert_eq!(tree.children(body).map(<[_]>::len), Some(3));
        ok(set_command_palette_query(&mut tree, palette, "focus"));
        assert_eq!(tree.children(body).map(<[_]>::len), Some(2));
    }

    #[test]
    fn move_selection_wraps_forward_and_backward() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        assert_eq!(selected_id(&tree, palette).as_deref(), Some("view.layers"));
        ok(move_command_palette_selection(&mut tree, palette, true));
        assert_eq!(selected_id(&tree, palette).as_deref(), Some("view.history"));
        ok(move_command_palette_selection(&mut tree, palette, true));
        assert_eq!(selected_id(&tree, palette).as_deref(), Some("edit.undo"));
        ok(move_command_palette_selection(&mut tree, palette, true));
        assert_eq!(
            selected_id(&tree, palette).as_deref(),
            Some("view.layers"),
            "must wrap back to the first result"
        );
        ok(move_command_palette_selection(&mut tree, palette, false));
        assert_eq!(
            selected_id(&tree, palette).as_deref(),
            Some("edit.undo"),
            "moving backward from the first result must wrap to the last"
        );
    }

    #[test]
    fn move_selection_updates_the_flags_on_the_real_rows() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        let rows = rows_of(&tree, palette);
        let (Some(&first_row), Some(&second_row)) = (rows.first(), rows.get(1)) else {
            unreachable!("3 commands were inserted above");
        };
        ok(move_command_palette_selection(&mut tree, palette, true));

        let flag = |row| tree.accessibility(row).and_then(accesskit::Node::is_selected);
        assert_eq!(flag(first_row), Some(false));
        assert_eq!(flag(second_row), Some(true));
        let payload = |selected| {
            WidgetKind::ListRow(super::ListRowState {
                selected,
                disabled: false,
                draws_label: false,
            })
        };
        assert_eq!(
            tree.payload(first_row),
            Some(&payload(false)),
            "paint_widget reads the payload, so it must clear too"
        );
        assert_eq!(tree.payload(second_row), Some(&payload(true)));
    }

    #[test]
    fn move_selection_is_a_no_op_when_nothing_matches() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        ok(set_command_palette_query(&mut tree, palette, "nonexistent"));
        assert_eq!(
            ok(move_command_palette_selection(&mut tree, palette, true)),
            None
        );
        assert_eq!(
            ok(step_command_palette_selection(
                &mut tree,
                palette,
                PaletteStep::PageDown
            )),
            None
        );
    }

    #[test]
    fn command_palette_mutators_reject_a_wrong_widget_kind_and_an_unknown_widget() {
        let (mut tree, root) = new_tree(Style::default());
        match set_command_palette_query(&mut tree, root, "x") {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
        let bogus = accesskit::NodeId(999);
        match set_command_palette_query(&mut tree, bogus, "x") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        match select_command_palette_at(&mut tree, bogus, (0.0, 0.0)) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn rows_are_real_list_row_widgets_with_only_the_first_selected() {
        let (mut tree, root) = new_tree(Style::default());
        let palette = palette_in(&mut tree, root, commands());
        let rows = rows_of(&tree, palette);
        let (Some(&first), Some(&second)) = (rows.first(), rows.get(1)) else {
            unreachable!("3 commands were inserted above");
        };
        let payload = |selected| {
            WidgetKind::ListRow(super::ListRowState {
                selected,
                disabled: false,
                draws_label: false,
            })
        };
        assert_eq!(tree.payload(first), Some(&payload(true)));
        assert_eq!(tree.payload(second), Some(&payload(false)));
    }

    /// 0.168.0: a prompt is the palette with its own name and no
    /// filtering; its commands and message can be replaced, and the
    /// message is the root's accessible description.
    #[test]
    fn a_prompt_never_filters_and_carries_its_label_and_message() {
        let (mut tree, root) = new_tree(Style::default());
        let prompt = ok(insert_command_prompt(
            &mut tree,
            root,
            "Workspace Name",
            commands(),
            &test_scales(),
        ));
        ok(set_command_palette_query(&mut tree, prompt, "zzz no match"));
        let state = ok(command_palette_state(&tree, prompt));
        assert!(!state.is_filtering());
        assert_eq!(state.label(), "Workspace Name");
        assert_eq!(state.results().len(), 3, "a prompt shows every row");
        let node = tree.accessibility(prompt).cloned();
        assert_eq!(
            node.as_ref().and_then(|n| n.label()),
            Some("Workspace Name")
        );
        assert_eq!(node.as_ref().and_then(|n| n.value()), Some("zzz no match"));
        assert_eq!(node.as_ref().and_then(|n| n.description()), None);

        ok(set_command_palette_message(
            &mut tree,
            prompt,
            Some("Bad".to_owned()),
        ));
        ok(set_command_palette_commands(
            &mut tree,
            prompt,
            vec![CommandEntry::new("only", "Only Row")],
        ));
        let state = ok(command_palette_state(&tree, prompt));
        assert_eq!(state.message(), Some("Bad"));
        assert_eq!(state.query(), "zzz no match", "the query survives");
        assert_eq!(state.selected().map(|c| c.id.as_str()), Some("only"));
        assert_eq!(state.rows().len(), 1);
        let node = tree.accessibility(prompt).cloned();
        assert_eq!(node.as_ref().and_then(|n| n.description()), Some("Bad"));
        ok(set_command_palette_message(&mut tree, prompt, None));
        let node = tree.accessibility(prompt).cloned();
        assert_eq!(node.as_ref().and_then(|n| n.description()), None);

        let palette = palette_in(&mut tree, root, commands());
        ok(set_command_palette_query(&mut tree, palette, "undo"));
        let state = ok(command_palette_state(&tree, palette));
        assert!(state.is_filtering());
        assert_eq!(state.label(), "Command Palette");
        assert_eq!(state.results().len(), 1);
    }

    // ---- 0.257.0: layout and scrolling with many commands ----

    /// Every row is exactly `row_height` tall and each starts exactly one
    /// row below the previous one: no overlap, at three window sizes
    /// (1274x900, 800x600 and 1274x900 physical at scale factor 2, which
    /// is 637x450 logical: layout is logical).
    #[test]
    fn two_hundred_commands_never_overlap_at_any_window_size() {
        for (w, h) in [(1274.0, 900.0), (800.0, 600.0), (637.0, 450.0)] {
            let (tree, palette) = opened(200, w, h);
            let state = ok(command_palette_state(&tree, palette));
            let rows = state.rows();
            assert_eq!(rows.len(), 200);
            let body = bounds(&tree, state.body());
            let mut expected_y = body.y;
            for &row in rows {
                let r = bounds(&tree, row);
                assert_eq!(i64::from(r.height), row_h(), "row height at {w}x{h}");
                assert_eq!(r.y, expected_y, "rows stack at exactly one row pitch");
                assert_eq!(r.x, body.x);
                expected_y += row_h();
            }
            let strip = bounds(&tree, state.query_strip());
            assert_eq!(i64::from(strip.height), strip_h(), "the strip is fixed");
            assert!(
                strip.bottom() <= body.y,
                "the strip sits above the rows: {strip:?} {body:?}"
            );
        }
    }

    #[test]
    fn the_panel_stops_at_the_row_cap_and_fits_the_window() {
        for (w, h) in [(1274.0, 900.0), (800.0, 600.0), (637.0, 450.0)] {
            let (tree, palette) = opened(200, w, h);
            let state = ok(command_palette_state(&tree, palette));
            let root = bounds(&tree, palette);
            let viewport = bounds(&tree, state.viewport());
            assert_eq!(
                i64::from(viewport.height),
                visible_cap() * row_h(),
                "the viewport shows the token's number of rows"
            );
            assert_eq!(
                i64::from(root.height),
                strip_h() + visible_cap() * row_h(),
                "the panel is the strip plus the capped viewport"
            );
            assert!(
                root.bottom() <= i64::from(h as u32),
                "the panel fits the window at {w}x{h}: {root:?}"
            );
        }
    }

    #[test]
    fn the_first_rows_are_visible_at_open_and_the_rest_are_clipped() {
        let (tree, palette) = opened(200, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        assert_eq!(selected_index(&tree, palette), Some(0));
        assert_eq!(tree.scroll_y(state.body()), Some(0.0));
        assert!(selected_is_inside_viewport(&tree, palette));
        let viewport = bounds(&tree, state.viewport());
        let rows = state.rows();
        #[allow(clippy::cast_sign_loss)]
        let cap = visible_cap() as usize;
        let last_shown = bounds(&tree, rows[cap - 1]);
        assert!(last_shown.bottom() <= viewport.bottom());
        let first_hidden = bounds(&tree, rows[cap]);
        assert!(
            first_hidden.bottom() > viewport.bottom(),
            "the row after the cap lies below the viewport (clipped)"
        );
        assert_eq!(
            tree.visible_rect(rows[cap], first_hidden),
            None,
            "the viewport clips that row away entirely"
        );
        assert_eq!(
            tree.visible_rect(rows[0], bounds(&tree, rows[0])),
            Some(bounds(&tree, rows[0])),
            "while the first row is wholly visible"
        );
        assert!(tree.scroll_range(state.body()).unwrap_or(0.0) > 0.0);
    }

    #[test]
    fn the_scrollbar_shows_for_overflow() {
        let (tree, palette) = opened(200, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        assert_ne!(
            tree.style(state.scrollbar()).map(|s| s.display),
            Some(taffy::Display::None),
            "200 rows overflow: the bar is shown"
        );
        assert_eq!(
            crate::widgets::scrollbar_target(&tree, state.scrollbar()),
            Some(state.body())
        );
    }

    #[test]
    fn arrow_down_keeps_the_selected_row_inside_the_viewport() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        for step in 1..=60_usize {
            ok(move_command_palette_selection(&mut tree, palette, true));
            relayout(&mut tree, 1274.0, 900.0);
            assert_eq!(selected_index(&tree, palette), Some(step));
            assert!(
                selected_is_inside_viewport(&tree, palette),
                "after {step} presses the selected row is visible"
            );
        }
        let body = ok(command_palette_state(&tree, palette)).body();
        assert!(tree.scroll_y(body).unwrap_or(0.0) > 0.0, "it scrolled");
    }

    #[test]
    fn arrow_up_from_the_top_wraps_to_the_last_row_and_scrolls_to_it() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        ok(move_command_palette_selection(&mut tree, palette, false));
        relayout(&mut tree, 1274.0, 900.0);
        assert_eq!(selected_index(&tree, palette), Some(199));
        assert!(selected_is_inside_viewport(&tree, palette));
        ok(move_command_palette_selection(&mut tree, palette, true));
        relayout(&mut tree, 1274.0, 900.0);
        assert_eq!(selected_index(&tree, palette), Some(0));
        assert!(selected_is_inside_viewport(&tree, palette));
        let body = ok(command_palette_state(&tree, palette)).body();
        assert_eq!(tree.scroll_y(body), Some(0.0), "wrapping to the top");
    }

    #[test]
    fn a_selection_reached_by_arrows_scrolls_just_far_enough() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let body = ok(command_palette_state(&tree, palette)).body();
        let cap = visible_cap();
        for _ in 0..(cap - 1) {
            ok(move_command_palette_selection(&mut tree, palette, true));
        }
        assert_eq!(
            tree.scroll_y(body),
            Some(0.0),
            "the last visible row needs no scrolling"
        );
        ok(move_command_palette_selection(&mut tree, palette, true));
        #[allow(clippy::cast_precision_loss)]
        let one_row = row_h() as f32;
        assert_eq!(
            tree.scroll_y(body),
            Some(one_row),
            "one row past the viewport scrolls by exactly one row"
        );
    }

    #[test]
    fn a_short_list_has_no_scrollbar_and_the_panel_shrinks_to_fit() {
        let mut list = many(200);
        list.push(CommandEntry::new("z.a", "Zebra A"));
        list.push(CommandEntry::new("z.b", "Zebra B"));
        list.push(CommandEntry::new("z.c", "Zebra C"));
        let (mut tree, root) = window_tree(1274.0, 900.0);
        let palette = palette_in(&mut tree, root, list);
        ok(tree.set_style(palette, app_style()));
        relayout(&mut tree, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        let tall = bounds(&tree, palette).height;
        assert_ne!(
            tree.style(state.scrollbar()).map(|s| s.display),
            Some(taffy::Display::None)
        );

        ok(set_command_palette_query(&mut tree, palette, "zebra"));
        relayout(&mut tree, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        assert_eq!(state.rows().len(), 3);
        assert_eq!(
            i64::from(bounds(&tree, palette).height),
            strip_h() + 3 * row_h(),
            "strip plus three rows, nothing more"
        );
        assert!(bounds(&tree, palette).height < tall);
        assert_eq!(
            tree.style(state.scrollbar()).map(|s| s.display),
            Some(taffy::Display::None),
            "nothing to scroll: the bar hides itself"
        );
        assert_eq!(tree.scroll_range(state.body()), Some(0.0));
    }

    #[test]
    fn a_no_match_palette_is_just_the_strip() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        ok(set_command_palette_query(&mut tree, palette, "nonexistent"));
        relayout(&mut tree, 1274.0, 900.0);
        assert_eq!(i64::from(bounds(&tree, palette).height), strip_h());
    }

    #[test]
    fn scrolling_moves_the_rows_by_the_offset() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        let body = state.body();
        let rows = state.rows().to_vec();
        let before = bounds(&tree, rows[5]).y;
        assert_eq!(ok(tree.scroll_by(body, 3.0 * 21.0)), true);
        assert_eq!(bounds(&tree, rows[5]).y, before - 63);
        relayout(&mut tree, 1274.0, 900.0);
        assert_eq!(
            bounds(&tree, rows[5]).y,
            before - 63,
            "a relayout keeps the offset"
        );
        let over_rows = {
            let r = bounds(&tree, rows[6]);
            #[allow(clippy::cast_precision_loss)]
            (r.x as f32 + 10.0, r.y as f32 + 5.0)
        };
        assert_eq!(
            tree.scroll_container_at(over_rows),
            Some(body),
            "the wheel over the rows finds the list"
        );
    }

    #[test]
    fn a_click_after_scrolling_picks_the_row_under_the_pointer() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let state = ok(command_palette_state(&tree, palette));
        let body = state.body();
        let rows = state.rows().to_vec();
        ok(tree.scroll_by(body, 5.0 * 21.0));
        let centre = |tree: &WidgetTree<WidgetKind>, row| {
            let r = bounds(tree, row);
            #[allow(clippy::cast_precision_loss)]
            (r.x as f32 + 20.0, r.y as f32 + r.height as f32 / 2.0)
        };
        let target = centre(&tree, rows[9]);
        let picked = ok(select_command_palette_at(&mut tree, palette, target));
        assert_eq!(picked, Some(rows[9]));
        assert_eq!(selected_id(&tree, palette).as_deref(), Some("cmd.9"));
        // The old (unscrolled) place of row 0 is above the viewport: not a row.
        let above = centre(&tree, rows[0]);
        assert_eq!(ok(select_command_palette_at(&mut tree, palette, above)), None);
        assert_eq!(selected_id(&tree, palette).as_deref(), Some("cmd.9"));
        // The query strip is not a row either.
        let strip = bounds(&tree, ok(command_palette_state(&tree, palette)).query_strip());
        #[allow(clippy::cast_precision_loss)]
        let on_strip = (strip.x as f32 + 5.0, strip.y as f32 + 5.0);
        assert_eq!(ok(select_command_palette_at(&mut tree, palette, on_strip)), None);
    }

    #[test]
    fn page_down_and_page_up_move_by_the_visible_rows_without_wrapping() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let step = |tree: &mut WidgetTree<WidgetKind>, s| {
            ok(step_command_palette_selection(tree, palette, s));
            relayout(tree, 1274.0, 900.0);
        };
        step(&mut tree, PaletteStep::PageDown);
        assert_eq!(selected_index(&tree, palette), Some(10), "one page = 10 rows");
        assert!(selected_is_inside_viewport(&tree, palette));
        step(&mut tree, PaletteStep::PageDown);
        assert_eq!(selected_index(&tree, palette), Some(20));
        assert!(selected_is_inside_viewport(&tree, palette));
        step(&mut tree, PaletteStep::PageUp);
        assert_eq!(selected_index(&tree, palette), Some(10));
        step(&mut tree, PaletteStep::PageUp);
        step(&mut tree, PaletteStep::PageUp);
        assert_eq!(selected_index(&tree, palette), Some(0), "stops at the top");
        step(&mut tree, PaletteStep::Last);
        assert_eq!(selected_index(&tree, palette), Some(199));
        assert!(selected_is_inside_viewport(&tree, palette));
        step(&mut tree, PaletteStep::PageDown);
        assert_eq!(selected_index(&tree, palette), Some(199), "stops at the end");
        step(&mut tree, PaletteStep::First);
        assert_eq!(selected_index(&tree, palette), Some(0));
        assert!(selected_is_inside_viewport(&tree, palette));
        step(&mut tree, PaletteStep::Next);
        step(&mut tree, PaletteStep::Next);
        step(&mut tree, PaletteStep::Previous);
        assert_eq!(selected_index(&tree, palette), Some(1));
        step(&mut tree, PaletteStep::Previous);
        step(&mut tree, PaletteStep::Previous);
        assert_eq!(selected_index(&tree, palette), Some(0), "no wrap upward");
    }

    #[test]
    fn a_new_query_scrolls_back_to_the_top() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let body = ok(command_palette_state(&tree, palette)).body();
        ok(tree.scroll_by(body, 400.0));
        assert!(tree.scroll_y(body).unwrap_or(0.0) > 0.0);
        ok(set_command_palette_query(&mut tree, palette, "command 1"));
        assert_eq!(tree.scroll_y(body), Some(0.0));
        relayout(&mut tree, 1274.0, 900.0);
        assert!(selected_is_inside_viewport(&tree, palette));
        ok(tree.scroll_by(body, 100.0));
        ok(set_command_palette_commands(&mut tree, palette, many(150)));
        assert_eq!(tree.scroll_y(body), Some(0.0), "new commands too");
    }

    #[test]
    fn a_prompt_with_many_rows_scrolls_the_same_way() {
        let (mut tree, root) = window_tree(800.0, 600.0);
        let prompt = ok(insert_command_prompt(
            &mut tree,
            root,
            "Workspace Name",
            many(120),
            &test_scales(),
        ));
        ok(tree.set_style(prompt, app_style()));
        relayout(&mut tree, 800.0, 600.0);
        let state = ok(command_palette_state(&tree, prompt));
        let viewport = bounds(&tree, state.viewport());
        assert_eq!(i64::from(viewport.height), visible_cap() * row_h());
        let mut y = bounds(&tree, state.body()).y;
        for &row in state.rows() {
            let r = bounds(&tree, row);
            assert_eq!((r.y, i64::from(r.height)), (y, row_h()));
            y += row_h();
        }
        for _ in 0..30 {
            ok(move_command_palette_selection(&mut tree, prompt, true));
        }
        relayout(&mut tree, 800.0, 600.0);
        assert!(selected_is_inside_viewport(&tree, prompt));
    }

    /// A caller that gives the root a definite height (the toolkit gallery
    /// and older callers did) still gets non-overlapping, clipped rows.
    #[test]
    fn a_definite_height_root_clips_instead_of_squeezing_rows() {
        let (mut tree, root) = window_tree(400.0, 300.0);
        let palette = palette_in(&mut tree, root, many(200));
        ok(tree.set_style(
            palette,
            Style {
                size: taffy::Size {
                    width: length(300.0_f32),
                    height: length(150.0_f32),
                },
                ..Default::default()
            },
        ));
        relayout(&mut tree, 400.0, 300.0);
        let state = ok(command_palette_state(&tree, palette));
        let root_b = bounds(&tree, palette);
        let viewport = bounds(&tree, state.viewport());
        assert!(viewport.bottom() <= root_b.bottom(), "{viewport:?} in {root_b:?}");
        assert_eq!(
            i64::from(bounds(&tree, state.query_strip()).height),
            strip_h(),
            "a crowded palette never shrinks its strip"
        );
        for pair in state.rows().windows(2) {
            let (a, b) = (bounds(&tree, pair[0]), bounds(&tree, pair[1]));
            assert_eq!(i64::from(a.height), row_h());
            assert_eq!(b.y - a.y, row_h());
        }
    }

    #[test]
    fn three_commands_are_as_tall_as_the_strip_plus_three_rows() {
        let (tree, palette) = opened(3, 800.0, 600.0);
        let state = ok(command_palette_state(&tree, palette));
        assert_eq!(
            i64::from(bounds(&tree, palette).height),
            strip_h() + 3 * row_h()
        );
        assert_eq!(
            tree.style(state.scrollbar()).map(|s| s.display),
            Some(taffy::Display::None)
        );
    }

    #[test]
    fn rows_outside_the_viewport_stay_in_the_accessibility_tree_and_scroll_is_reported() {
        let (mut tree, palette) = opened(200, 1274.0, 900.0);
        let (body, last_row, strip_id) = {
            let state = ok(command_palette_state(&tree, palette));
            (state.body(), state.rows()[199], state.query_strip())
        };
        ok(tree.scroll_by(body, 63.0));
        let update = tree.accessibility_update(palette);
        let node_of = |id: WidgetId| update.nodes.iter().find(|(n, _)| *n == id).map(|(_, n)| n);
        let Some(last) = node_of(last_row) else {
            unreachable!("every row is in the accessibility tree");
        };
        assert_eq!(last.role(), accesskit::Role::ListBoxOption);
        let Some(list) = node_of(body) else {
            unreachable!("the list is in the tree");
        };
        assert_eq!(list.role(), accesskit::Role::ListBox);
        assert_eq!(list.scroll_y(), Some(63.0));
        assert!(list.scroll_y_max().unwrap_or(0.0) > 0.0);
        let selected = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == accesskit::Role::ListBoxOption && n.is_selected() == Some(true))
            .count();
        assert_eq!(selected, 1, "exactly one selected option");
        let Some(strip) = node_of(strip_id) else {
            unreachable!("the strip is in the tree");
        };
        assert_eq!(strip.role(), accesskit::Role::GenericContainer);
        assert_eq!(strip.label(), None, "the strip stays presentational");
    }
}
