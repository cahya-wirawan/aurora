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
        CommandEntry, command_palette_state, insert_command_palette, insert_command_prompt,
        move_command_palette_selection, set_command_palette_commands, set_command_palette_message,
        set_command_palette_query,
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

        let Some(rows) = tree.children(state.body()).and_then(|c| c.get(1..)) else {
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
        // Only the query strip is left: it stays visible with no results.
        assert_eq!(
            tree.children(state.body()),
            Some([state.query_strip()].as_slice())
        );
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
        // The query strip plus one row per result.
        assert_eq!(tree.children(body).map(<[_]>::len), Some(4));

        if let Err(err) = set_command_palette_query(&mut tree, palette, "focus") {
            unreachable!("{err:?}");
        }
        // "Undo" doesn't match "focus" -- its row must be gone, not just
        // unlisted in `results()`.
        assert_eq!(tree.children(body).map(<[_]>::len), Some(3));
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
            let Some(rows) = tree.children(state.body()).and_then(|c| c.get(1..)) else {
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
        let Some(rows) = tree.children(body).and_then(|c| c.get(1..)) else {
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
                draws_label: false,
            }))
        );
        assert_eq!(
            tree.payload(second),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: false,
                disabled: false,
                draws_label: false,
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
            let Some(rows) = tree.children(state.body()).and_then(|c| c.get(1..)) else {
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
                draws_label: false,
            })),
            "the row moved away from must clear its own payload flag, not just its accessibility \
             one -- paint_widget reads the payload"
        );
        assert_eq!(
            tree.payload(second_row),
            Some(&WidgetKind::ListRow(super::ListRowState {
                selected: true,
                disabled: false,
                draws_label: false,
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
        assert_eq!(
            rows.len(),
            4,
            "the query strip plus all 3 commands matching the empty query"
        );

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

    /// 0.168.0: a prompt is the palette with its own name and no
    /// filtering; its commands and message can be replaced, and the
    /// message is the root's accessible description.
    #[test]
    fn a_prompt_never_filters_and_carries_its_label_and_message() {
        let (mut tree, root) = new_tree(Style::default());
        let prompt = match insert_command_prompt(&mut tree, root, "Workspace Name", commands()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_command_palette_query(&mut tree, prompt, "zzz no match") {
            unreachable!("{err:?}");
        }
        let state = match command_palette_state(&tree, prompt) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
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

        if let Err(err) = set_command_palette_message(&mut tree, prompt, Some("Bad".to_owned())) {
            unreachable!("{err:?}");
        }
        let replaced = vec![CommandEntry::new("only", "Only Row")];
        if let Err(err) = set_command_palette_commands(&mut tree, prompt, replaced) {
            unreachable!("{err:?}");
        }
        let state = match command_palette_state(&tree, prompt) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.message(), Some("Bad"));
        assert_eq!(state.query(), "zzz no match", "the query survives");
        assert_eq!(state.selected().map(|c| c.id.as_str()), Some("only"));
        assert_eq!(state.rows().len(), 1);
        let node = tree.accessibility(prompt).cloned();
        assert_eq!(node.as_ref().and_then(|n| n.description()), Some("Bad"));
        if let Err(err) = set_command_palette_message(&mut tree, prompt, None) {
            unreachable!("{err:?}");
        }
        let node = tree.accessibility(prompt).cloned();
        assert_eq!(node.as_ref().and_then(|n| n.description()), None);

        // The ordinary palette still filters, under its own name.
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
        assert!(state.is_filtering());
        assert_eq!(state.label(), "Command Palette");
        assert_eq!(state.results().len(), 1);
    }
}
