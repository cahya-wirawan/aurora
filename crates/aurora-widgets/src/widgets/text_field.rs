//! A single-line text field: selection, caret motion (character and
//! word, grapheme-cluster aware), clipboard-shaped operations,
//! per-field undo/redo, and IME composition state.
//!
//! **No pixel rendering, no real clipboard** — this is the logical text
//! buffer only, the same "no rendering yet" scope every widget in this
//! module has (see `widgets`' own doc comment). "Clipboard" here means
//! [`TextFieldState::copy`]/[`TextFieldState::cut`]/[`TextFieldState::paste`]
//! as pure text-buffer operations returning/taking a plain `String` —
//! actually reading/writing the OS clipboard is platform-specific and
//! belongs in `aurora-app`, the same seam [`crate::hit_test`]/
//! [`crate::FocusManager`] already draw around real input.
//!
//! **IME composition** ([`TextFieldState::set_composition`]/
//! [`TextFieldState::commit_composition`]) mirrors `winit::event::Ime`
//! (`Preedit(text, cursor_range)` / `Commit(text)`, the same shape
//! `spike/a11y-ime/src/field.rs`'s `TextField` already proved out by
//! hand): uncommitted composition text is kept separate from `content`
//! until committed, never touches undo/redo on its own (only a commit
//! is a real edit), and starting a fresh composition replaces any active
//! selection first (typing over a selection — composition is what
//! typing looks like once an IME is involved). [`composition_segments`]
//! turns a [`Composition`] into byte-range segments with the underline
//! style each should render with ([`UnderlineStyle::Plain`] for
//! unconverted text, [`UnderlineStyle::Target`] for the clause the
//! candidate window is acting on — the distinction Windows TSF, macOS,
//! and `IBus` each draw under different names). Actually drawing either
//! style is an `aurora-text`/`aurora-vector` concern (both still
//! skeletons); this is the styled-segment *data* a future renderer would
//! consume — "rendering" in the same data-not-pixels sense every widget
//! in this module already uses.
//!
//! `accesskit::TextSelection` isn't exposed yet — `spike/a11y-ime`
//! already named that as unverified/open on its own (`FINDINGS.md`:
//! "`TextSelection` exposed in the tree"), so this doesn't newly defer
//! it, it inherits an already-known gap. Composition is announced via
//! `set_description`, the exact mechanism `spike/a11y-ime/src/tree.rs`
//! already proved reaches `VoiceOver` (finding 2's "the spike sets a
//! description" — this module is that finding's own follow-up design
//! work).

use std::ops::Range;

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length};
use taffy::{Rect as LayoutRect, Size, Style};
use unicode_segmentation::UnicodeSegmentation;

use super::{WidgetKind, spacing, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    content: String,
    cursor: usize,
    selection_anchor: Option<usize>,
}

/// In-progress, uncommitted IME composition text — not part of
/// [`TextFieldState::content`] until [`TextFieldState::commit_composition`]
/// is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    /// Byte range within `text` (not `content`) marking the IME's
    /// current *target* clause — the segment the candidate window is
    /// acting on. Windows TSF's `ATTR_TARGET_CONVERTED`/
    /// `ATTR_TARGET_NOTCONVERTED`, macOS's thicker-marked range, and
    /// `IBus`'s "selected" preedit attribute all name the same concept
    /// under different terms. `None` when the platform reported no
    /// target (`winit::event::Ime::Preedit`'s cursor-position pair is
    /// `None`) — the whole composition then uses the plain style. A
    /// plain `(usize, usize)` rather than `Range<usize>`, so
    /// `Composition` (and `TextFieldState`, which embeds it) can still
    /// derive `Eq` — `Range` deliberately does not implement it.
    pub target_range: Option<(usize, usize)>,
}

/// The two underline styles every mainstream IME convention
/// distinguishes. See this module's own doc comment for what "style"
/// means here — data for a future renderer, not a drawn pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
    /// Uncommitted, unconverted (or already-converted-but-not-targeted)
    /// composition text.
    Plain,
    /// The clause currently targeted by the IME's candidate window.
    Target,
}

/// Splits `composition.text` into byte-range segments paired with the
/// underline style each should render with.
#[must_use]
pub fn composition_segments(composition: &Composition) -> Vec<(Range<usize>, UnderlineStyle)> {
    let len = composition.text.len();
    let target = composition.target_range.filter(|(start, end)| start < end);
    let Some((start, end)) = target else {
        // No target, or a degenerate empty one (conveys no distinct
        // clause) -- either way, one plain segment for the whole text.
        return vec![(0..len, UnderlineStyle::Plain)];
    };
    let mut segments = Vec::with_capacity(3);
    if start > 0 {
        segments.push((0..start, UnderlineStyle::Plain));
    }
    segments.push((start..end, UnderlineStyle::Target));
    if end < len {
        segments.push((end..len, UnderlineStyle::Plain));
    }
    segments
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFieldState {
    pub label: String,
    pub content: String,
    /// Byte offset into `content`, always on a grapheme-cluster boundary.
    pub cursor: usize,
    /// Byte offset into `content` (always on a grapheme-cluster
    /// boundary), or `None` if there's no selection — just a caret at
    /// `cursor`.
    pub selection_anchor: Option<usize>,
    pub disabled: bool,
    /// In-progress IME composition, anchored at `cursor`. `None` when
    /// there's no composition underway.
    pub composition: Option<Composition>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
}

impl TextFieldState {
    fn new(label: String, content: String) -> Self {
        let cursor = content.len();
        Self {
            label,
            content,
            cursor,
            selection_anchor: None,
            disabled: false,
            composition: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// The selected byte range (`min(cursor, anchor)..max(cursor,
    /// anchor)`), or `None` if there's no selection.
    #[must_use]
    pub fn selection_range(&self) -> Option<Range<usize>> {
        self.selection_anchor.map(|anchor| {
            if anchor < self.cursor {
                anchor..self.cursor
            } else {
                self.cursor..anchor
            }
        })
    }

    /// The currently selected text, or `""` if there's no selection.
    #[must_use]
    pub fn selected_text(&self) -> &str {
        match self.selection_range() {
            Some(range) => &self.content[range],
            None => "",
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            cursor: self.cursor,
            selection_anchor: self.selection_anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.content = snapshot.content;
        self.cursor = snapshot.cursor;
        self.selection_anchor = snapshot.selection_anchor;
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        self.redo_stack.clear();
    }

    /// Undoes the most recent content-changing edit (not a caret-only
    /// move — moving the cursor isn't itself undoable, matching every
    /// mainstream editor). Returns whether there was anything to undo.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.restore(previous);
        self.redo_stack.push(current);
        true
    }

    /// Redoes the most recently undone edit. Returns whether there was
    /// anything to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.restore(next);
        self.undo_stack.push(current);
        true
    }

    /// Replaces the current selection (if any) with `text`, or inserts it
    /// at the caret. A no-op (no undo entry recorded) if `text` is empty
    /// and there's no selection to remove.
    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() && self.selection_range().is_none() {
            return;
        }
        self.push_undo();
        if let Some(range) = self.selection_range() {
            self.content.replace_range(range.clone(), text);
            self.cursor = range.start + text.len();
        } else {
            self.content.insert_str(self.cursor, text);
            self.cursor += text.len();
        }
        self.selection_anchor = None;
    }

    /// Deletes the selection, or the one grapheme cluster before the
    /// caret. A no-op at the very start with nothing selected.
    pub fn backspace(&mut self) {
        if let Some(range) = self.selection_range() {
            self.push_undo();
            self.content.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.selection_anchor = None;
        } else if self.cursor > 0 {
            let start = prev_boundary(&self.content, self.cursor);
            self.push_undo();
            self.content.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    /// Deletes the selection, or the one grapheme cluster after the
    /// caret. A no-op at the very end with nothing selected.
    pub fn delete_forward(&mut self) {
        if let Some(range) = self.selection_range() {
            self.push_undo();
            self.content.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.selection_anchor = None;
        } else if self.cursor < self.content.len() {
            let end = next_boundary(&self.content, self.cursor);
            self.push_undo();
            self.content.replace_range(self.cursor..end, "");
        }
    }

    fn move_to(&mut self, position: usize, extend_selection: bool) {
        if extend_selection {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor = position;
    }

    /// Moves the caret one grapheme cluster left (extending the
    /// selection if `extend_selection`, the same "hold Shift" shape
    /// every mainstream editor uses).
    pub fn move_left(&mut self, extend_selection: bool) {
        let target = prev_boundary(&self.content, self.cursor);
        self.move_to(target, extend_selection);
    }

    /// Same as [`Self::move_left`], but right.
    pub fn move_right(&mut self, extend_selection: bool) {
        let target = next_boundary(&self.content, self.cursor);
        self.move_to(target, extend_selection);
    }

    /// Moves the caret to the start of the current or previous word
    /// (skipping any whitespace/punctuation in between), Unicode-word-
    /// aware rather than a whitespace-splitting heuristic.
    pub fn move_word_left(&mut self, extend_selection: bool) {
        let target = prev_word_boundary(&self.content, self.cursor);
        self.move_to(target, extend_selection);
    }

    /// Moves the caret to the start of the next word.
    pub fn move_word_right(&mut self, extend_selection: bool) {
        let target = next_word_boundary(&self.content, self.cursor);
        self.move_to(target, extend_selection);
    }

    pub fn move_to_start(&mut self, extend_selection: bool) {
        self.move_to(0, extend_selection);
    }

    pub fn move_to_end(&mut self, extend_selection: bool) {
        let end = self.content.len();
        self.move_to(end, extend_selection);
    }

    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor = self.content.len();
    }

    /// The selected text, as a caller (e.g. `aurora-app`, writing to the
    /// real OS clipboard) would copy it. Doesn't mutate anything.
    #[must_use]
    pub fn copy(&self) -> String {
        self.selected_text().to_owned()
    }

    /// Same as [`Self::copy`], but also removes the selection.
    pub fn cut(&mut self) -> String {
        let text = self.copy();
        if !text.is_empty() {
            self.insert_str("");
        }
        text
    }

    /// Inserts `text` at the caret (or over the current selection) — the
    /// text-buffer half of a paste; a caller reads the real text from
    /// the OS clipboard and passes it in here.
    pub fn paste(&mut self, text: &str) {
        self.insert_str(text);
    }

    /// Sets (or replaces) the in-progress IME composition, anchored at
    /// the current cursor position — the handler for
    /// `winit::event::Ime::Preedit(text, cursor_range)`. An empty `text`
    /// clears the composition (winit's own documented synthetic-clear
    /// shape, `Ime::Preedit("", None)`). Starting a fresh composition
    /// (there was none before this call) first removes any active
    /// selection, the same "typing replaces the selection" rule any
    /// other content-changing input follows — composition is what
    /// typing looks like once an IME is involved. Not itself an undo
    /// step (composition is transient, like caret motion), except for
    /// that one selection-removal, which is a real content change.
    pub fn set_composition(
        &mut self,
        text: impl Into<String>,
        target_range: Option<(usize, usize)>,
    ) {
        let text = text.into();
        if text.is_empty() {
            self.composition = None;
            return;
        }
        if self.composition.is_none()
            && let Some(range) = self.selection_range()
        {
            self.push_undo();
            self.content.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.selection_anchor = None;
        }
        self.composition = Some(Composition { text, target_range });
    }

    /// Clears any in-progress composition and inserts `text` at the
    /// cursor (or over the active selection) — the handler for
    /// `winit::event::Ime::Commit`. Unlike [`Self::set_composition`],
    /// this is a real, undoable edit (it goes through
    /// [`Self::insert_str`]), matching every other content-changing
    /// method in this type.
    pub fn commit_composition(&mut self, text: &str) {
        self.composition = None;
        self.insert_str(text);
    }
}

/// The start byte of the grapheme cluster immediately before `from`, or
/// `0` if `from` is already at (or before) the first one.
fn prev_boundary(content: &str, from: usize) -> usize {
    content
        .grapheme_indices(true)
        .rev()
        .find(|&(i, _)| i < from)
        .map_or(0, |(i, _)| i)
}

/// The start byte of the grapheme cluster immediately after `from`, or
/// `content.len()` if `from` is already at (or after) the last one.
fn next_boundary(content: &str, from: usize) -> usize {
    content
        .grapheme_indices(true)
        .find(|&(i, _)| i > from)
        .map_or(content.len(), |(i, _)| i)
}

/// The start byte of the Unicode word immediately before `from` (see
/// [`UnicodeSegmentation::unicode_word_indices`] — skips whitespace and
/// punctuation), or `0`.
fn prev_word_boundary(content: &str, from: usize) -> usize {
    content
        .unicode_word_indices()
        .rev()
        .find(|&(i, _)| i < from)
        .map_or(0, |(i, _)| i)
}

/// Same as [`prev_word_boundary`], but forward.
fn next_word_boundary(content: &str, from: usize) -> usize {
    content
        .unicode_word_indices()
        .find(|&(i, _)| i > from)
        .map_or(content.len(), |(i, _)| i)
}

fn node(state: &TextFieldState) -> Node {
    let mut node = Node::new(Role::TextInput);
    node.set_label(state.label.clone());
    node.set_value(state.content.clone());
    if let Some(composition) = &state.composition {
        // Composition in progress: announce it, or a screen-reader user
        // hears nothing while typing CJK -- the exact mechanism
        // `spike/a11y-ime/src/tree.rs` already proved reaches VoiceOver.
        node.set_description(format!("composing: {}", composition.text));
    }
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::SetValue);
    }
    node
}

fn style(scales: &Scales) -> Style {
    Style {
        padding: LayoutRect {
            left: length(spacing(scales.spacing.sm)),
            right: length(spacing(scales.spacing.sm)),
            top: length(spacing(scales.spacing.xs)),
            bottom: length(spacing(scales.spacing.xs)),
        },
        flex_grow: 1.0,
        size: Size {
            width: auto(),
            height: length(type_size(scales.typography.size.md)),
        },
        ..Default::default()
    }
}

/// Adds a new, enabled text field as the last child of `parent`, with
/// `content` as its starting text (caret placed at the end).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_text_field(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
    content: impl Into<String>,
) -> Result<WidgetId, WidgetError> {
    let state = TextFieldState::new(label.into(), content.into());
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::TextField(state),
    )
}

/// A read-only view of `id`'s own [`TextFieldState`] — for inspecting
/// content/selection without needing a mutable borrow of `tree`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a text field.
pub fn text_field_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&TextFieldState, WidgetError> {
    match tree.payload(id) {
        Some(WidgetKind::TextField(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}

/// Runs `f` against `id`'s own [`TextFieldState`], then rebuilds and
/// applies its accessibility node from the result — the one place every
/// text field mutation in this module goes through, so "update state,
/// then re-derive the node" can't drift out of sync, the same pattern
/// `button`/`checkbox`/`slider` each use for their own, much smaller set
/// of mutators. A disabled text field rejects every call through here;
/// use [`set_text_field_disabled`] to re-enable one, which deliberately
/// doesn't go through this same guard.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a text field,
/// or [`WidgetError::WidgetDisabled`] if it's disabled.
pub fn with_text_field_mut<R>(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut TextFieldState) -> R,
) -> Result<R, WidgetError> {
    let result = {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::TextField(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        f(state)
    };
    let Some(WidgetKind::TextField(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a TextField above");
    };
    tree.set_accessibility(id, node(state))?;
    Ok(result)
}

/// Sets whether `id` (a text field) is disabled. Unlike
/// [`with_text_field_mut`], this always succeeds on a real text field —
/// re-enabling one is exactly the operation a disabled-rejecting guard
/// would otherwise make unreachable.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a text field.
pub fn set_text_field_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::TextField(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        state.disabled = disabled;
    }
    let Some(WidgetKind::TextField(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a TextField above");
    };
    tree.set_accessibility(id, node(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Composition, TextFieldState, UnderlineStyle, composition_segments, insert_text_field,
        set_text_field_disabled, text_field_state, with_text_field_mut,
    };
    use crate::WidgetError;
    use crate::widgets::{new_tree, test_scales};
    use accesskit::Action;
    use taffy::Style;

    fn field(content: &str) -> TextFieldState {
        TextFieldState::new("label".to_owned(), content.to_owned())
    }

    // -- construction / wiring --

    #[test]
    fn insert_text_field_starts_with_the_caret_at_the_end() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "hello") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let state = match text_field_state(&tree, id) {
            Ok(s) => s,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.content, "hello");
        assert_eq!(state.cursor, 5);
        assert_eq!(state.selection_anchor, None);

        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.value(), Some("hello"));
        assert!(accessibility.supports_action(Action::Focus));
    }

    #[test]
    fn text_field_state_rejects_a_wrong_widget_kind() {
        let (tree, root) = new_tree(Style::default());
        match text_field_state(&tree, root) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn with_text_field_mut_marks_the_widget_dirty_and_updates_value() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "ab") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();

        if let Err(err) = with_text_field_mut(&mut tree, id, |f| f.insert_str("c")) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.value(), Some("abc"));
    }

    #[test]
    fn with_text_field_mut_rejects_a_disabled_field() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "ab") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_text_field_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match with_text_field_mut(&mut tree, id, |f| f.insert_str("c")) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn set_text_field_disabled_can_re_enable() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "ab") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_text_field_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_text_field_disabled(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = with_text_field_mut(&mut tree, id, |f| f.insert_str("c")) {
            unreachable!("{err:?}");
        }
        match text_field_state(&tree, id) {
            Ok(state) => assert_eq!(state.content, "abc"),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // -- caret / selection (pure TextFieldState logic) --

    #[test]
    fn insert_str_inserts_at_the_caret() {
        let mut f = field("ac");
        f.cursor = 1;
        f.insert_str("b");
        assert_eq!(f.content, "abc");
        assert_eq!(f.cursor, 2);
    }

    #[test]
    fn insert_str_replaces_a_selection() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5; // "hello" selected
        f.insert_str("hi");
        assert_eq!(f.content, "hi world");
        assert_eq!(f.cursor, 2);
        assert_eq!(f.selection_anchor, None);
    }

    #[test]
    fn backspace_deletes_the_previous_grapheme() {
        let mut f = field("abc");
        f.backspace();
        assert_eq!(f.content, "ab");
        assert_eq!(f.cursor, 2);
    }

    #[test]
    fn backspace_at_the_start_is_a_no_op() {
        let mut f = field("abc");
        f.cursor = 0;
        f.backspace();
        assert_eq!(f.content, "abc");
        assert_eq!(f.cursor, 0);
    }

    #[test]
    fn backspace_deletes_a_selection_instead_of_one_char() {
        let mut f = field("abcdef");
        f.selection_anchor = Some(1);
        f.cursor = 4; // "bcd" selected
        f.backspace();
        assert_eq!(f.content, "aef");
        assert_eq!(f.cursor, 1);
        assert_eq!(f.selection_anchor, None);
    }

    #[test]
    fn delete_forward_deletes_the_next_grapheme() {
        let mut f = field("abc");
        f.cursor = 0;
        f.delete_forward();
        assert_eq!(f.content, "bc");
        assert_eq!(f.cursor, 0);
    }

    #[test]
    fn delete_forward_at_the_end_is_a_no_op() {
        let mut f = field("abc");
        f.delete_forward();
        assert_eq!(f.content, "abc");
    }

    #[test]
    fn backspace_and_delete_are_grapheme_cluster_aware_not_byte_or_char_aware() {
        // "e" + combining acute accent (U+0301) is one grapheme cluster,
        // two chars, three bytes -- a naive byte- or char-based
        // backspace would leave a dangling combining mark or panic on a
        // non-char-boundary split.
        let mut f = field("e\u{0301}x");
        assert_eq!(f.content.chars().count(), 3, "sanity check: 3 chars");
        f.cursor = f.content.len();
        f.backspace(); // must remove "x" only
        assert_eq!(f.content, "e\u{0301}");
        f.backspace(); // must remove the whole "e + accent" cluster at once
        assert_eq!(f.content, "");
    }

    #[test]
    fn move_left_and_right_move_by_one_grapheme_cluster() {
        let mut f = field("e\u{0301}x");
        f.cursor = 0;
        f.move_right(false);
        assert_eq!(f.cursor, "e\u{0301}".len(), "must skip the whole cluster");
        f.move_right(false);
        assert_eq!(f.cursor, f.content.len());
        f.move_left(false);
        assert_eq!(f.cursor, "e\u{0301}".len());
    }

    #[test]
    fn move_left_and_right_extend_or_collapse_the_selection() {
        let mut f = field("abcdef");
        f.cursor = 2;
        f.move_right(true);
        assert_eq!(f.selection_anchor, Some(2));
        assert_eq!(f.cursor, 3);
        f.move_right(true);
        assert_eq!(f.selection_anchor, Some(2), "anchor must not move again");
        assert_eq!(f.cursor, 4);

        f.move_left(false);
        assert_eq!(
            f.selection_anchor, None,
            "a plain (non-extending) move must collapse the selection"
        );
    }

    #[test]
    fn move_word_right_skips_to_the_start_of_the_next_word() {
        let mut f = field("hello, world!");
        f.cursor = 0;
        f.move_word_right(false);
        assert_eq!(f.cursor, 7, "must land at 'world', skipping ', '");
    }

    #[test]
    fn move_word_left_skips_to_the_start_of_the_current_or_previous_word() {
        let mut f = field("hello world");
        f.cursor = 8; // inside "world"
        f.move_word_left(false);
        assert_eq!(f.cursor, 6, "must land at the start of the current word");
        f.move_word_left(false);
        assert_eq!(f.cursor, 0, "must land at the start of the previous word");
    }

    #[test]
    fn move_to_start_and_end_jump_to_the_content_boundaries() {
        let mut f = field("hello");
        f.cursor = 2;
        f.move_to_start(false);
        assert_eq!(f.cursor, 0);
        f.move_to_end(false);
        assert_eq!(f.cursor, f.content.len());
    }

    #[test]
    fn select_all_selects_the_whole_content() {
        let mut f = field("hello");
        f.select_all();
        assert_eq!(f.selected_text(), "hello");
    }

    // -- clipboard --

    #[test]
    fn copy_does_not_mutate_the_field() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5;
        assert_eq!(f.copy(), "hello");
        assert_eq!(f.content, "hello world", "copy must not change the content");
    }

    #[test]
    fn cut_removes_the_selection_and_returns_it() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5;
        assert_eq!(f.cut(), "hello");
        assert_eq!(f.content, " world");
    }

    #[test]
    fn cut_with_no_selection_returns_empty_and_changes_nothing() {
        let mut f = field("hello");
        assert_eq!(f.cut(), "");
        assert_eq!(f.content, "hello");
    }

    #[test]
    fn paste_inserts_at_the_caret() {
        let mut f = field("ac");
        f.cursor = 1;
        f.paste("b");
        assert_eq!(f.content, "abc");
    }

    // -- undo/redo --

    #[test]
    fn undo_reverts_the_most_recent_edit() {
        let mut f = field("");
        f.insert_str("a");
        f.insert_str("b");
        assert_eq!(f.content, "ab");
        assert!(f.undo());
        assert_eq!(f.content, "a");
        assert!(f.undo());
        assert_eq!(f.content, "");
        assert!(!f.undo(), "nothing left to undo");
    }

    #[test]
    fn redo_reapplies_an_undone_edit() {
        let mut f = field("");
        f.insert_str("a");
        f.insert_str("b");
        f.undo();
        assert!(f.redo());
        assert_eq!(f.content, "ab");
        assert!(!f.redo(), "nothing left to redo");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut f = field("");
        f.insert_str("a");
        f.undo();
        assert!(f.redo_stack_has_entries_for_test());
        f.insert_str("x");
        assert!(!f.redo_stack_has_entries_for_test());
    }

    #[test]
    fn undo_restores_cursor_and_selection_not_just_content() {
        let mut f = field("hello");
        f.cursor = 0;
        f.selection_anchor = Some(0);
        f.cursor = 5; // whole word selected
        f.insert_str("hi"); // replaces selection
        assert_eq!(f.content, "hi");
        f.undo();
        assert_eq!(f.content, "hello");
        assert_eq!(f.selection_anchor, Some(0));
        assert_eq!(f.cursor, 5);
    }

    #[test]
    fn moving_the_caret_alone_is_not_undoable() {
        let mut f = field("hello");
        f.insert_str("!");
        f.move_left(false);
        f.move_right(false);
        assert!(f.undo(), "the insert must still be the only undo step");
        assert_eq!(f.content, "hello");
        assert!(
            !f.undo(),
            "moving the caret must not have added its own step"
        );
    }

    impl TextFieldState {
        /// Test-only peek at the redo stack, to confirm it was actually
        /// cleared rather than just asserting on behaviour that would
        /// also pass if `redo` were silently broken in some other way.
        fn redo_stack_has_entries_for_test(&self) -> bool {
            !self.redo_stack.is_empty()
        }
    }

    // -- IME composition --

    #[test]
    fn set_composition_stores_text_without_touching_content() {
        let mut f = field("hello");
        f.set_composition("ni", Some((0, 2)));
        assert_eq!(f.content, "hello", "composition must not touch content");
        match &f.composition {
            Some(c) => {
                assert_eq!(c.text, "ni");
                assert_eq!(c.target_range, Some((0, 2)));
            }
            None => unreachable!("composition must be set"),
        }
    }

    #[test]
    fn set_composition_with_empty_text_clears_it() {
        let mut f = field("hello");
        f.set_composition("ni", Some((0, 2)));
        f.set_composition("", None);
        assert!(f.composition.is_none());
    }

    #[test]
    fn set_composition_is_not_undoable_on_its_own() {
        let mut f = field("hello");
        f.set_composition("ni", None);
        f.set_composition("nih", None);
        f.set_composition("", None);
        assert!(!f.undo(), "composition updates must not push undo entries");
        assert_eq!(f.content, "hello");
    }

    #[test]
    fn starting_a_composition_replaces_an_active_selection() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5; // "hello" selected
        f.set_composition("ni", Some((0, 2)));
        assert_eq!(
            f.content, " world",
            "starting composition must remove the selected text, like typing would"
        );
        assert_eq!(f.selection_anchor, None);
        assert_eq!(f.cursor, 0);
    }

    #[test]
    fn removing_a_selection_to_start_composition_is_undoable() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5;
        f.set_composition("ni", Some((0, 2)));
        assert!(f.undo(), "the selection removal is a real content change");
        assert_eq!(f.content, "hello world");
    }

    #[test]
    fn continuing_a_composition_does_not_re_check_for_a_selection() {
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5;
        f.set_composition("n", None); // starts composition, removes selection
        f.selection_anchor = Some(0); // simulate stale selection state
        f.set_composition("ni", None); // continuing, must not remove again
        assert_eq!(f.content, " world");
    }

    #[test]
    fn commit_composition_clears_composition_and_inserts_the_final_text() {
        let mut f = field("");
        f.set_composition("ni", Some((0, 2)));
        f.commit_composition("你");
        assert!(f.composition.is_none());
        assert_eq!(f.content, "你");
        assert_eq!(f.cursor, "你".len());
    }

    #[test]
    fn commit_composition_is_undoable() {
        let mut f = field("");
        f.set_composition("ni", Some((0, 2)));
        f.commit_composition("你");
        assert!(f.undo());
        assert_eq!(f.content, "");
    }

    #[test]
    fn commit_composition_replaces_a_selection_even_without_a_prior_preedit() {
        // winit documents Commit as always preceded by an empty Preedit,
        // but a defensive check costs nothing: commit alone must still
        // replace a selection via the same path insert_str already
        // provides.
        let mut f = field("hello world");
        f.selection_anchor = Some(0);
        f.cursor = 5;
        f.commit_composition("hi");
        assert_eq!(f.content, "hi world");
    }

    #[test]
    fn composition_segments_with_no_target_is_one_plain_segment() {
        let composition = Composition {
            text: "nihao".to_owned(),
            target_range: None,
        };
        assert_eq!(
            composition_segments(&composition),
            vec![(0..5, UnderlineStyle::Plain)]
        );
    }

    #[test]
    fn composition_segments_splits_around_the_target_clause() {
        let composition = Composition {
            text: "nihao".to_owned(),
            target_range: Some((2, 5)), // "hao" targeted
        };
        assert_eq!(
            composition_segments(&composition),
            vec![
                (0..2, UnderlineStyle::Plain),
                (2..5, UnderlineStyle::Target),
            ]
        );
    }

    #[test]
    fn composition_segments_target_covering_the_whole_text_is_one_segment() {
        let composition = Composition {
            text: "hao".to_owned(),
            target_range: Some((0, 3)),
        };
        assert_eq!(
            composition_segments(&composition),
            vec![(0..3, UnderlineStyle::Target)]
        );
    }

    #[test]
    fn composition_segments_empty_target_range_is_skipped() {
        // A zero-width target (cursor hidden, per winit's own doc
        // comment: "When it's None, the cursor should be hidden" --
        // this covers the degenerate non-None-but-empty case) must not
        // emit a spurious empty Target segment.
        let composition = Composition {
            text: "nihao".to_owned(),
            target_range: Some((3, 3)),
        };
        assert_eq!(
            composition_segments(&composition),
            vec![(0..5, UnderlineStyle::Plain)]
        );
    }

    #[test]
    fn node_announces_composition_via_description() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = with_text_field_mut(&mut tree, id, |f| f.set_composition("ni", None)) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.description(), Some("composing: ni"));
    }

    #[test]
    fn node_has_no_description_once_composition_clears() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_text_field(&mut tree, root, &scales, "Name", "") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = with_text_field_mut(&mut tree, id, |f| f.set_composition("ni", None)) {
            unreachable!("{err:?}");
        }
        if let Err(err) = with_text_field_mut(&mut tree, id, |f| f.commit_composition("你")) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.description(), None);
    }
}
