//! A dropdown (a select-only combo box): a control showing the current
//! choice out of a fixed list of options, which opens a list of those
//! options beneath itself, lets the user move a highlight through them
//! with the arrow keys, and commits or cancels.
//!
//! **Shape: a pure state machine plus a structural reconcile**, the
//! same split `command_palette.rs` and `tree_view.rs` already use.
//! [`DropdownState`] owns the options, the committed selection, and —
//! only while open — the highlighted option; every key and every
//! mutator is a transition on that state alone (unit-tested with no
//! tree at all). After each transition the tree is reconciled to match:
//! while open, a [`WidgetKind::DropdownList`] (`Role::ListBox`) is a
//! real **child** of the control, holding one real
//! [`WidgetKind::ListRow`] (`Role::ListBoxOption`) per option; while
//! closed, that list does not exist — closing is `WidgetTree::remove`,
//! so every option row's [`WidgetId`] is dead afterwards. The same "real
//! tree nodes, not a hidden flag" rule `tree_view.rs`'s own doc comment
//! argues for at length.
//!
//! # Keys
//!
//! [`DropdownKey`] is this module's own four-key vocabulary, and the
//! choice is a disclosed one: `shortcut::NamedKey` (`shortcut.rs`)
//! already names all four keys, so a reusable enum *does* exist. It was
//! not used as the transition input because it is a 24-variant,
//! `#[non_exhaustive]` enum whose other 20 keys would all be "ignored",
//! which would make the transition table below unreadable and its test
//! non-exhaustive in spirit. [`DropdownKey::from_named_key`] bridges the
//! two, so `aurora-app`'s existing winit → `NamedKey` translation
//! composes with this widget without a second translation layer.
//!
//! The table, following the WAI-ARIA APG *select-only combobox* for the
//! keys it covers:
//!
//! | state | key | result |
//! |---|---|---|
//! | disabled | any | `Err(WidgetDisabled)`, nothing changes |
//! | closed, no options | any | `Ignored` |
//! | closed | `Down`/`Up`/`Enter` | opens, highlight = selection (or the first option), `Opened` |
//! | closed | `Escape` | `Ignored` |
//! | open | `Down`/`Up` | highlight moves one step, **clamped, no wrap** — `HighlightMoved(i)`; at either end `Ignored` |
//! | open | `Enter` | commits the highlight as the selection, closes, `Committed { index, changed }` |
//! | open | `Escape` | closes, selection unchanged, `Cancelled` |
//!
//! Not covered, and not claimed: `Space`, `Home`/`End`, `PageUp`/
//! `PageDown`, `Alt+Down`, and type-ahead — the APG lists all of them.
//! Nothing here handles a pointer click on an option either (see "Hit
//! testing" below); [`toggle_dropdown`] is the whole pointer story, for
//! a click on the control itself. An assistive technology's
//! `Expand`/`Collapse`/`Click` request reaches [`set_dropdown_open`] and
//! [`toggle_dropdown`] through `crate::action::handle_action` (0.128.0;
//! see "Accessibility actions" below).
//!
//! # The accessibility vocabulary, checked against the pinned sources
//!
//! Every claim below was checked against `accesskit` 0.24.1 and its
//! adapters as pinned in `Cargo.lock` (`accesskit_consumer` 0.38.0,
//! `accesskit_windows` 0.34.0, `accesskit_macos` 0.26.3,
//! `accesskit_atspi_common` 0.19.1), not assumed.
//!
//! - **The control is `Role::ComboBox`**, not `EditableComboBox`: this
//!   is select-only, there is no text to edit. Its `label` is the
//!   dropdown's own name, its `value` is the selected option's text —
//!   **absent, not an empty string**, when nothing is selected, and
//!   equally absent when the selected option's own text is `""` (an
//!   empty option is accepted at insert, since a caller may genuinely
//!   offer one, but a present-and-empty `value` is exactly what this
//!   convention exists to avoid) — and it always carries `expanded`
//!   (open or not).
//! - **`expanded` is read by the Windows adapter only.**
//!   `accesskit_consumer::Node::supports_expand_collapse` is `true` for
//!   every `Role::ComboBox` (`accesskit_consumer` `src/node.rs:644-650`),
//!   so it is always set here, unlike a leaf tree row;
//!   `accesskit_windows` maps it to the UIA `ExpandCollapse` pattern
//!   (`src/node.rs:713-724`). Neither `accesskit_macos` nor
//!   `accesskit_atspi_common` reads `expanded` at all (a
//!   case-insensitive grep for "expand" over both crates' `src/` has no
//!   hits). On those two platforms "open" is carried only by the list
//!   **structurally existing** — which it really does here.
//! - **Exactly one of `Action::Expand`/`Action::Collapse`** on an
//!   enabled control — a choice made to *match* the Windows adapter,
//!   not one it requires. `accesskit_windows`' own `set_expanded`
//!   (`src/node.rs:955-966`) decides whether a UIA expand/collapse is
//!   valid from the node's `expanded` value and `is_disabled()` alone,
//!   returning `invalid_operation` when the requested state already
//!   matches; it never consults the declared actions. Declaring only
//!   the one action that adapter would actually forward keeps the
//!   declared actions honest about what can succeed — the same choice
//!   `tree_view.rs` makes. `Focus` and `Click` are always declared on an
//!   enabled control; a disabled one declares no actions at all and sets
//!   `set_disabled`.
//! - **`value` is read by Windows and macOS, not AT-SPI.** Windows
//!   exposes the UIA `Value` pattern whenever the node has a value
//!   (`accesskit_windows` `src/node.rs:591-596`); macOS maps
//!   `Role::ComboBox` to `NSAccessibilityPopUpButtonRole`
//!   (`accesskit_macos` `src/node.rs:102`) and returns the string value
//!   from `accessibilityValue` (`src/node.rs:323-338`). AT-SPI's
//!   accessible name is the `label` unless the label comes from the
//!   value (`accesskit_atspi_common` `src/node.rs:38-44`), and its
//!   `Value` interface is numeric only (`src/node.rs:534-535`), so an
//!   Orca user hears the *label*, and the current choice only once the
//!   list is open and an option is selected. A real, disclosed gap in
//!   the adapter mapping, not something this file can fix.
//! - **Focus stays on the control; the highlighted option is its
//!   `active_descendant`** while open (setter: `accesskit` 0.24.1
//!   `src/lib.rs:1902`). The consumer resolves the platform-visible
//!   focus *through* it — `TreeState::focus` returns the focused node's
//!   active descendant (`accesskit_consumer` `src/tree.rs:544-548`) —
//!   and all three adapters read focus through that one function, so
//!   moving the highlight moves a screen reader's focus on every
//!   platform without the options ever being focusable themselves. The
//!   lookup is an `and_then`, not an `unwrap` (`src/node.rs:949-953`), so
//!   a stale id could not crash a consumer; this module clears it on
//!   close regardless.
//! - **`controls` is deliberately not set**, although `set_controls`
//!   exists (`accesskit` `src/lib.rs:1882`): `accesskit_consumer`'s own
//!   `Node::controls` *unwraps* every id it is handed
//!   (`src/node.rs:940-946`), so a `controls` pointing at a list that
//!   has since been removed would be a panic in a consumer — the same
//!   crash class `WidgetTree::accessibility_update`'s own doc comment
//!   records. The list is already the control's structural child, which
//!   every adapter reads, so the relation would add nothing.
//! - **The list is `Role::ListBox`**, labelled with the dropdown's own
//!   label. **Each option is `Role::ListBoxOption`**, labelled with its
//!   text, with `selected` true on exactly the **highlighted** one, not
//!   the committed choice — deliberately: the WAI-ARIA APG *select-only
//!   combobox* example puts `aria-selected="true"` on the option its
//!   `aria-activedescendant` references, i.e. the highlight, and the
//!   committed choice is carried by the control's own `value` — read by Windows'
//!   `SelectionItem` pattern (`accesskit_windows` `src/node.rs:651-677`),
//!   macOS's selected-children list (`accesskit_macos`
//!   `src/node.rs:443`), and AT-SPI's `Selection` interface
//!   (`accesskit_atspi_common` `src/node.rs:1184`). **Options declare no
//!   actions at all** — in particular no `Action::Focus`, so they never
//!   enter `FocusManager`'s tab order — the same shape
//!   `command_palette.rs`'s result rows already have.
//!
//! # What this does not do, stated rather than implied away
//!
//! - **The list is a popover, clamped to the window and nothing
//!   smarter** (0.127.0). The list is inserted as a
//!   [`PaintLayer::Popover`](crate::PaintLayer) root, so it paints after
//!   every base-layer widget (a later sibling of the dropdown no longer
//!   paints over it), both `WidgetTree::hit_test` and `crate::hit_test`
//!   reach its options (pinned by
//!   `hit_testing_an_open_option_reaches_the_option`), and a clipping
//!   `Overflow` ancestor no longer clips it — a dropdown near the bottom
//!   of a panel body opens past the body's edge. What it still does
//!   *not* do is **flip or reposition**: the list always hangs below the
//!   control, and the part that would fall outside the tree root's own
//!   bounds (the window) is simply clamped away, unreachable and
//!   unpainted.
//! - **No glyphs**: neither the selected option's text, nor the option
//!   rows' text, nor the `▾` indicator `design/gallery/index.html`
//!   shows is drawn — this crate draws no glyphs at all, and which
//!   token an indicator would resolve is a design-owner question.
//! - **Options are fixed at insert time.** There is no
//!   `set_dropdown_options`; nothing needs one yet.
//! - **No dismiss on an outside click or on focus loss.** An open list
//!   closes only through this module's own mutators (`Enter`, `Escape`,
//!   [`toggle_dropdown`], [`set_dropdown_open`], disabling); nothing
//!   here observes a click elsewhere or focus moving away, so a caller
//!   that wants either must call [`set_dropdown_open`] itself.
//! - **Accessibility actions: the control only.** The control's
//!   `Focus`/`Click`/`Expand`/`Collapse` are routed (0.128.0) by
//!   `crate::action::handle_action` to [`toggle_dropdown`] and
//!   [`set_dropdown_open`] — `Collapse` closes without committing, like
//!   `Escape`. **Choosing an option is not possible through an assistive
//!   technology's actions**: option rows declare no `Click`, so the
//!   dispatcher refuses one; a screen-reader user picks an option with
//!   the arrow keys and `Enter`, and whether a real screen reader lets
//!   them do that has not been checked by a human.
//! - **The highlighted option covers part of the list's border.** A
//!   highlighted row is the list's full width (`paint_list_row` fills
//!   the row's whole box), so its `accent.primary` fill lies over the
//!   inner half of the list's own centred 1 px `border.default` stroke
//!   along the left and right edges — and along the top or bottom edge
//!   too when the first or last option is highlighted.
//! - **An externally removed list is repaired lazily.** If a caller
//!   removes the list (or one of its rows) with `WidgetTree::remove`
//!   directly, the control's node keeps its `expanded: true` and its
//!   now-dangling `active_descendant` until the next *state-changing*
//!   transition on this dropdown (an `Ignored` key or a no-op setter
//!   returns before reconciling), which rebuilds the list and the node together (see
//!   `sync_structure`). Nothing observes the removal itself.

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{AlignItems, FlexDirection, Position, Rect as LayoutRect, Size, Style};

use super::{ListRowState, WidgetKind, row_height, spacing};
use crate::error::WidgetError;
use crate::shortcut::NamedKey;
use crate::tree::{PaintLayer, WidgetId, WidgetTree};

/// The four keys a dropdown responds to — see this module's own doc
/// comment for the transition table and for why this is not
/// `shortcut::NamedKey` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropdownKey {
    Up,
    Down,
    Enter,
    Escape,
}

impl DropdownKey {
    /// The dropdown key `key` stands for, if any — `None` for every
    /// `NamedKey` a dropdown does not handle, so a caller can forward
    /// every key it translates and drop the rest.
    #[must_use]
    pub fn from_named_key(key: NamedKey) -> Option<Self> {
        match key {
            NamedKey::ArrowUp => Some(Self::Up),
            NamedKey::ArrowDown => Some(Self::Down),
            NamedKey::Enter => Some(Self::Enter),
            NamedKey::Escape => Some(Self::Escape),
            _ => None,
        }
    }
}

/// What a transition did — every mutator that can open, move, commit, or
/// close reports one, so a caller can react (apply a committed choice,
/// restore focus after a cancel) without diffing state itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropdownOutcome {
    /// The list opened.
    Opened,
    /// The highlight moved to this option index; the list stays open.
    HighlightMoved(usize),
    /// The highlighted option became the selection and the list closed.
    /// `changed` is `false` when it was already the selection.
    Committed { index: usize, changed: bool },
    /// The list closed without changing the selection.
    Cancelled,
    /// Nothing changed at all.
    Ignored,
}

/// A dropdown's own state — the payload of its control
/// ([`WidgetKind::Dropdown`]).
///
/// Only `label` is a public field; everything else is private behind
/// getters, because it is kept in lockstep with real tree structure (the
/// list and its rows) or with a rule a mutator enforces. **`disabled` is
/// private too** — unlike most other widget states here — because a
/// direct write would skip the "disabling closes the list first" rule
/// [`set_dropdown_disabled`] enforces and leave an open, unclosable list
/// (every key and `set_dropdown_open` refuse a disabled dropdown). Read
/// it with [`Self::is_disabled`].
///
/// `Clone`, so a caller can snapshot it; writing a stale snapshot back
/// through [`WidgetTree::payload_mut`] is tolerated — the next
/// state-changing transition's structural reconcile discards any list the snapshot
/// does not own (`sync_structure`) — but is not a supported way to
/// change a dropdown.
///
/// `PartialEq` but not `Eq`: it carries the option rows' resolved `f32`
/// height.
#[derive(Debug, Clone, PartialEq)]
pub struct DropdownState {
    /// The dropdown's own accessible name (the control's `label`, and
    /// the open list's too).
    pub label: String,
    disabled: bool,
    options: Vec<String>,
    /// The committed choice, an index into `options`.
    selected: Option<usize>,
    /// `Some` exactly while the list is open — "open" is not a separate
    /// flag, so an open list with no highlight is unrepresentable.
    highlighted: Option<usize>,
    /// The open list's own widget, `Some` exactly while open (after the
    /// structural reconcile has run).
    list: Option<WidgetId>,
    /// One row per option, in option order, while open; empty while
    /// closed.
    rows: Vec<WidgetId>,
    /// `row_height(scales)` at insert time — see `row_style`.
    row_height: f32,
}

impl DropdownState {
    fn new(label: String, options: Vec<String>, selected: Option<usize>, row_height: f32) -> Self {
        Self {
            label,
            disabled: false,
            options,
            selected,
            highlighted: None,
            list: None,
            rows: Vec::new(),
            row_height,
        }
    }

    #[must_use]
    pub fn options(&self) -> &[String] {
        &self.options
    }

    /// Whether the dropdown refuses user input — see this struct's own
    /// doc comment for why this is a getter, not a public field.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// The committed selection's index, if any.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The committed selection's text, if any.
    #[must_use]
    pub fn selected_option(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.options.get(index))
            .map(String::as_str)
    }

    /// The highlighted option's index — `Some` exactly while open.
    #[must_use]
    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.highlighted.is_some()
    }

    /// The open list's own widget (`Role::ListBox`), `None` while closed.
    #[must_use]
    pub fn list(&self) -> Option<WidgetId> {
        self.list
    }

    /// The option rows, in option order — empty while closed.
    #[must_use]
    pub fn rows(&self) -> &[WidgetId] {
        &self.rows
    }

    /// The row backing [`Self::highlighted`], while open.
    #[must_use]
    pub fn highlighted_row(&self) -> Option<WidgetId> {
        self.highlighted
            .and_then(|index| self.rows.get(index).copied())
    }

    /// Opens with the highlight seeded on the selection (or the first
    /// option) — shared by every "open" entry point so they cannot seed
    /// differently.
    fn open(&mut self) -> DropdownOutcome {
        if self.options.is_empty() {
            return DropdownOutcome::Ignored;
        }
        let seed = self
            .selected
            .filter(|&index| index < self.options.len())
            .unwrap_or(0);
        self.highlighted = Some(seed);
        DropdownOutcome::Opened
    }

    /// The pure key transition — this module's doc-comment table,
    /// exactly. Touches no tree.
    fn apply_key(
        &mut self,
        id: WidgetId,
        key: DropdownKey,
    ) -> Result<DropdownOutcome, WidgetError> {
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        let Some(highlighted) = self.highlighted else {
            return Ok(match key {
                DropdownKey::Up | DropdownKey::Down | DropdownKey::Enter => self.open(),
                DropdownKey::Escape => DropdownOutcome::Ignored,
            });
        };
        let last = self.options.len().saturating_sub(1);
        Ok(match key {
            DropdownKey::Down if highlighted < last => {
                self.highlighted = Some(highlighted + 1);
                DropdownOutcome::HighlightMoved(highlighted + 1)
            }
            DropdownKey::Up if highlighted > 0 && highlighted <= last => {
                self.highlighted = Some(highlighted - 1);
                DropdownOutcome::HighlightMoved(highlighted - 1)
            }
            DropdownKey::Up | DropdownKey::Down => DropdownOutcome::Ignored,
            DropdownKey::Enter => {
                self.highlighted = None;
                if highlighted >= self.options.len() {
                    // Unreachable through this module (options are fixed
                    // and the highlight is only ever moved within them),
                    // but a commit of an index that names nothing would
                    // store an invalid selection, so it closes instead.
                    return Ok(DropdownOutcome::Cancelled);
                }
                let changed = self.selected != Some(highlighted);
                self.selected = Some(highlighted);
                DropdownOutcome::Committed {
                    index: highlighted,
                    changed,
                }
            }
            DropdownKey::Escape => {
                self.highlighted = None;
                DropdownOutcome::Cancelled
            }
        })
    }

    /// Opens (seeded like `Down`) or closes (like `Escape`, never
    /// committing). `Ignored` when already in the requested state.
    fn apply_open(&mut self, id: WidgetId, open: bool) -> Result<DropdownOutcome, WidgetError> {
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        Ok(match (open, self.is_open()) {
            (true, false) => self.open(),
            (false, true) => {
                self.highlighted = None;
                DropdownOutcome::Cancelled
            }
            _ => DropdownOutcome::Ignored,
        })
    }

    /// Disabling an open dropdown closes it first (without committing):
    /// a disabled control declares no actions, so a list left open would
    /// offer a screen-reader user options nothing could act on. Returns
    /// whether anything changed — `false` for a request that matches the
    /// current state, so a no-op costs no rebuild and no damage.
    fn apply_disabled(&mut self, disabled: bool) -> bool {
        let before = (self.disabled, self.highlighted);
        if disabled {
            self.highlighted = None;
        }
        self.disabled = disabled;
        before != (self.disabled, self.highlighted)
    }

    /// Sets the committed selection. While open, a `Some` selection also
    /// re-seeds the highlight onto it, so the list shows the owner's new
    /// value and a later `Enter` commits *that* rather than silently
    /// overriding it with a stale highlight; a `None` selection leaves
    /// the highlight where it was (there is nothing to seed it on).
    /// Returns whether anything changed.
    fn apply_selected(&mut self, selected: Option<usize>) -> Result<bool, WidgetError> {
        checked_selection(selected, self.options.len())?;
        let before = (self.selected, self.highlighted);
        self.selected = selected;
        if let (Some(index), Some(_)) = (selected, self.highlighted) {
            self.highlighted = Some(index);
        }
        Ok(before != (self.selected, self.highlighted))
    }
}

fn checked_selection(selected: Option<usize>, len: usize) -> Result<(), WidgetError> {
    match selected {
        Some(index) if index >= len => Err(WidgetError::IndexOutOfRange { index, len }),
        _ => Ok(()),
    }
}

/// The control's own node. `active_descendant` is the highlighted row,
/// which only exists after the structural reconcile — so this is always
/// built *after* it.
fn control_node(state: &DropdownState) -> Node {
    let mut node = Node::new(Role::ComboBox);
    node.set_label(state.label.clone());
    // Absent, never present-and-empty -- including for an option whose
    // own text is "" (see this module's own doc comment).
    if let Some(value) = state.selected_option().filter(|value| !value.is_empty()) {
        node.set_value(value.to_owned());
    }
    let open = state.is_open();
    node.set_expanded(open);
    if let Some(row) = state.highlighted_row() {
        node.set_active_descendant(row);
    }
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        // Exactly one -- see this module's own doc comment.
        if open {
            node.add_action(Action::Collapse);
        } else {
            node.add_action(Action::Expand);
        }
    }
    node
}

fn list_node(label: &str) -> Node {
    let mut node = Node::new(Role::ListBox);
    node.set_label(label.to_owned());
    node
}

/// One option row's node. `position_in_set`/`size_of_set` are set
/// explicitly (1-based position, total option count):
/// `accesskit_consumer` 0.38 does not compute either from the tree
/// (`src/node.rs:626-634` only reads what the node declares), so a
/// screen reader's "3 of 5" is only announced if the node carries it.
fn option_node(text: &str, index: usize, len: usize, highlighted: bool) -> Node {
    let mut node = Node::new(Role::ListBoxOption);
    node.set_label(text.to_owned());
    node.set_selected(highlighted);
    node.set_position_in_set(index.saturating_add(1));
    node.set_size_of_set(len);
    node
}

/// The control's own layout: `design/gallery/index.html`'s
/// `padding: 4px var(--spacing-xs)` taken to the nearest real tokens
/// (`spacing.xxs` = 4 vertically, `spacing.xs` = 8 horizontally), at
/// least one row tall (`row_height` — the height of the rows it opens,
/// so control and options read as one scale), and the full width of its
/// parent. No width literal: the mockup's `120px` is a mockup value, and
/// a caller sizes a dropdown by sizing its container.
///
/// `align_self: FlexStart` keeps a `Row` parent's default `Stretch` from
/// inflating the control to the row's whole height — the class of bug
/// `scrollbar::style`'s own doc comment records. `flex_grow` stays `0.0`
/// for the same reason.
fn control_style(scales: &Scales) -> Style {
    Style {
        align_self: Some(AlignItems::FLEX_START),
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        min_size: Size {
            width: auto(),
            height: length(row_height(scales)),
        },
        padding: LayoutRect {
            left: length(spacing(scales.spacing.xs)),
            right: length(spacing(scales.spacing.xs)),
            top: length(spacing(scales.spacing.xxs)),
            bottom: length(spacing(scales.spacing.xxs)),
        },
        ..Default::default()
    }
}

/// The open list: taken out of flow (`Position::Absolute`) so opening it
/// never pushes the dropdown's siblings around, anchored to the
/// control's own bottom-left corner (`top: 100%`, `left: 0`) and as wide
/// as the control, stacking its rows. Its height is its rows'.
fn list_style() -> Style {
    Style {
        position: Position::Absolute,
        flex_direction: FlexDirection::Column,
        inset: LayoutRect {
            left: zero(),
            right: auto(),
            top: percent(1.0_f32),
            bottom: auto(),
        },
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        ..Default::default()
    }
}

/// One option row: exactly one row tall (`row_height(scales)`, resolved
/// once at insert time and carried in [`DropdownState`] — the list is
/// built lazily on open, by mutators that take no `&Scales`) and never shrunk — unlike
/// `command_palette::row_style`, whose rows deliberately divide a sized
/// panel's height, a dropdown list has no height of its own to divide.
fn row_style(height: f32) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: length(height),
        },
        ..Default::default()
    }
}

/// Adds a new, enabled, closed dropdown as the last child of `parent`,
/// offering `options` with `selected` as its initial choice.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist, or
/// [`WidgetError::IndexOutOfRange`] if `selected` names no option.
/// Nothing is added when either happens.
pub fn insert_dropdown(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    options: Vec<String>,
    selected: Option<usize>,
) -> Result<WidgetId, WidgetError> {
    checked_selection(selected, options.len())?;
    let state = DropdownState::new(label.to_owned(), options, selected, row_height(scales));
    tree.insert(
        parent,
        control_style(scales),
        control_node(&state),
        WidgetKind::Dropdown(state),
    )
}

fn state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Result<&DropdownState, WidgetError> {
    match tree.payload(id).ok_or(WidgetError::UnknownWidget(id))? {
        WidgetKind::Dropdown(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// A read-only view of `id`'s own [`DropdownState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a dropdown.
pub fn dropdown_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&DropdownState, WidgetError> {
    state(tree, id)
}

/// Feeds one key to `id` — see this module's own doc comment for the
/// full transition table.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a dropdown, or [`WidgetError::WidgetDisabled`]
/// if it is disabled. Nothing changes when any of these happens.
///
/// The one other error path — the structural reconcile after a
/// successful transition failing, which needs a tree id that does not
/// exist and is unreachable today — is **not** atomic: it returns the
/// error with the dropdown left closed and no list behind it, not in the
/// state it started in (`sync_structure` has the details). The same
/// holds for every mutator below.
pub fn handle_dropdown_key(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    key: DropdownKey,
) -> Result<DropdownOutcome, WidgetError> {
    with_dropdown_mut(tree, id, |state| outcome_of(state.apply_key(id, key)))
}

/// Opens or closes `id` — opening seeds the highlight exactly as `Down`
/// does, closing never commits (it is `Escape`, not `Enter`). A
/// dropdown with no options never opens (`Ignored`).
///
/// # Errors
///
/// As [`handle_dropdown_key`].
pub fn set_dropdown_open(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    open: bool,
) -> Result<DropdownOutcome, WidgetError> {
    with_dropdown_mut(tree, id, |state| outcome_of(state.apply_open(id, open)))
}

/// Opens `id` if closed, closes it (without committing) if open — what a
/// pointer click on the control, or its `Action::Click`, does.
///
/// # Errors
///
/// As [`handle_dropdown_key`].
pub fn toggle_dropdown(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<DropdownOutcome, WidgetError> {
    with_dropdown_mut(tree, id, |state| {
        let open = !state.is_open();
        outcome_of(state.apply_open(id, open))
    })
}

/// Enables or disables `id`. Disabling an open dropdown closes it first,
/// removing its list, without committing the highlight. A request that
/// matches the current state changes nothing — no rebuild, no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a dropdown. Nothing
/// changes when either happens.
pub fn set_dropdown_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_dropdown_mut(tree, id, |state| Ok(((), state.apply_disabled(disabled))))
}

/// Sets `id`'s committed selection directly — an owner-driven change
/// (the document's own value changed, an undo), not a user gesture, so
/// unlike [`handle_dropdown_key`] it is allowed on a **disabled**
/// dropdown, the same distinction `set_tree_item_label` draws.
///
/// An open list stays open, and a `Some` selection **re-seeds the
/// highlight onto it** — so the list shows the new value, and a later
/// `Enter` commits it rather than silently overriding the owner with
/// whatever the highlight was on before. A `None` selection leaves the
/// highlight where it was. Setting the selection it already has changes
/// nothing — no rebuild, no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a dropdown, or [`WidgetError::IndexOutOfRange`]
/// if `selected` names no option. Nothing changes when any happens.
pub fn set_dropdown_selected(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    selected: Option<usize>,
) -> Result<(), WidgetError> {
    with_dropdown_mut(tree, id, |state| Ok(((), state.apply_selected(selected)?)))
}

/// Pairs a transition's outcome with whether anything changed, which is
/// every outcome but `Ignored`.
fn outcome_of(
    outcome: Result<DropdownOutcome, WidgetError>,
) -> Result<(DropdownOutcome, bool), WidgetError> {
    outcome.map(|outcome| (outcome, outcome != DropdownOutcome::Ignored))
}

/// The one path every mutator here goes through: run the pure
/// transition on the payload, and — unless it reports that nothing
/// changed (an `Ignored` key or a no-op setter, which then costs no
/// damage at all) — reconcile the list's real structure with the new
/// state, rebuild the control's node (after the reconcile, because
/// `active_descendant` names a row the reconcile may have just created),
/// and mark it dirty.
///
/// Both `set_accessibility` *and* `mark_dirty`, deliberately:
/// `set_accessibility` only sets the per-widget flag, while `mark_dirty`
/// unions the control's bounds into the damage region a renderer reads
/// — the gap `with_scrollbar_mut`'s own comment records. An open/close
/// changes the control's border colour, so it has new pixels.
///
/// The control's node is rebuilt even when the reconcile fails, so it
/// always describes the state the reconcile left behind (closed, on a
/// failure — see `sync_structure`) rather than the one before it.
fn with_dropdown_mut<T>(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut DropdownState) -> Result<(T, bool), WidgetError>,
) -> Result<T, WidgetError> {
    let (result, changed, previous) = {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Dropdown(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        let previous = state.highlighted;
        let (result, changed) = f(state)?;
        (result, changed, previous)
    };
    if !changed {
        return Ok(result);
    }
    let synced = sync_structure(tree, id, previous);
    let node = control_node(state(tree, id)?);
    tree.set_accessibility(id, node)?;
    tree.mark_dirty(id)?;
    synced?;
    Ok(result)
}

/// Makes the tree match `id`'s state: open ⇒ exactly one list child with
/// one row per option and the highlight on exactly one row; closed ⇒ no
/// list child at all. `previous` is the highlight before the transition
/// that led here, used only as a hint for the cheap path below.
///
/// **Trusts nothing it did not just check**, because a caller can
/// reach behind this module — `WidgetTree::remove` on the list or on one
/// row, or a stale [`DropdownState`] snapshot written back through
/// `WidgetTree::payload_mut`:
///
/// - The tracked list is kept only if it is still a live
///   [`WidgetKind::DropdownList`] **child of `id`** (a snapshot's list
///   id may since have been removed, or never have been this control's),
///   and — while open — only if its children are *exactly* the tracked
///   rows, one per option. Anything else is removed and rebuilt, so a
///   row removed on its own can never leave a dead id in `rows` or in
///   the control's `active_descendant`.
/// - **Every other `DropdownList` child of `id` is removed**, so a
///   snapshot written back over an open dropdown cannot leave a second,
///   orphaned list painting ghost options and exposing them to
///   accessibility.
///
/// Moving the highlight within a kept list touches at most two rows —
/// the one `previous` names, verified to be shown as highlighted (the
/// other rows are not re-checked), and the new one — and clones no option text but theirs.
/// If `previous` does not check out (a snapshot written back, say), it
/// falls back to walking every row's payload, still without cloning.
///
/// # Failure
///
/// On any error the dropdown is left **closed**, with any partly built
/// list removed and `list`/`rows` cleared, so a failure can never leave
/// an untracked list behind — but the transition that led here is *not*
/// rolled back beyond that: a failed open or move ends closed, not where
/// it started. In practice every error here is unreachable today:
/// `WidgetTree::insert`/`remove`/`set_accessibility`/`mark_dirty` fail
/// only for an id that does not exist (or for removing the root), and
/// every id this function hands them was just inserted or just checked
/// live.
fn sync_structure(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    previous: Option<usize>,
) -> Result<(), WidgetError> {
    let result = reconcile(tree, id, previous);
    if result.is_err() {
        close_after_failure(tree, id);
    }
    result
}

/// `sync_structure`'s body; see there.
fn reconcile(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    previous: Option<usize>,
) -> Result<(), WidgetError> {
    let current = state(tree, id)?;
    let highlighted = current.highlighted;
    let owned = current.list.filter(|&list| {
        tree.parent(list) == Some(id)
            && matches!(tree.payload(list), Some(WidgetKind::DropdownList))
    });
    let keep = highlighted.and(owned).filter(|&list| {
        current.rows.len() == current.options.len()
            && tree.children(list) == Some(current.rows.as_slice())
    });

    remove_lists_except(tree, id, keep)?;

    let Some(highlighted) = highlighted else {
        return write_structure(tree, id, None, Vec::new());
    };
    if keep.is_some() {
        return move_highlight(tree, id, previous, highlighted);
    }
    build_list(tree, id, highlighted)
}

/// Removes every [`WidgetKind::DropdownList`] child of `id` except
/// `keep`. `WidgetTree::remove` cascades through each one's rows,
/// marking their old bounds dirty so the vacated area repaints.
fn remove_lists_except(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    keep: Option<WidgetId>,
) -> Result<(), WidgetError> {
    let strays: Vec<WidgetId> = tree
        .children(id)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&child| {
            Some(child) != keep && matches!(tree.payload(child), Some(WidgetKind::DropdownList))
        })
        .collect();
    for stray in strays {
        tree.remove(stray)?;
    }
    Ok(())
}

/// Moves the shown highlight within a list already verified intact —
/// see `sync_structure` for the two-row cheap path and its fallback.
fn move_highlight(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    previous: Option<usize>,
    highlighted: usize,
) -> Result<(), WidgetError> {
    let current = state(tree, id)?;
    let shown = |index: usize| {
        current
            .rows
            .get(index)
            .and_then(|&row| match tree.payload(row) {
                Some(WidgetKind::ListRow(row_state)) => Some((row, row_state.selected)),
                _ => None,
            })
    };
    let changes: Vec<(usize, WidgetId, bool)> = match previous.map(|index| (index, shown(index))) {
        Some((prev, Some((prev_row, true)))) => {
            let mut changes = Vec::with_capacity(2);
            if prev != highlighted {
                changes.push((prev, prev_row, false));
            }
            if let Some((row, false)) = shown(highlighted) {
                changes.push((highlighted, row, true));
            }
            changes
        }
        _ => current
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, &row)| match tree.payload(row) {
                Some(WidgetKind::ListRow(row_state))
                    if row_state.selected != (index == highlighted) =>
                {
                    Some((index, row, index == highlighted))
                }
                _ => None,
            })
            .collect(),
    };
    for (index, row, on) in changes {
        let options = &state(tree, id)?.options;
        let len = options.len();
        let text = options.get(index).cloned().unwrap_or_default();
        if let Some(WidgetKind::ListRow(row_state)) = tree.payload_mut(row) {
            row_state.selected = on;
        }
        tree.set_accessibility(row, option_node(&text, index, len, on))?;
        tree.mark_dirty(row)?;
    }
    Ok(())
}

/// Builds a fresh list under `id` with one row per option, the
/// `highlighted` one selected, and records it. On a failure partway the
/// caller (`sync_structure`) removes the partly built list, which is
/// already a `DropdownList` child of `id` and so found without being
/// recorded.
fn build_list(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    highlighted: usize,
) -> Result<(), WidgetError> {
    let current = state(tree, id)?;
    let label = current.label.clone();
    let options = current.options.clone();
    let row_style = row_style(current.row_height);
    let list = tree.insert(
        id,
        list_style(),
        list_node(&label),
        WidgetKind::DropdownList,
    )?;
    // The open list floats above every base-layer widget and escapes its
    // control's clipping ancestors — see this module's own doc comment.
    // On an error the half-built list is removed by `sync_structure`,
    // exactly as a failed row insert already is.
    tree.set_layer(list, PaintLayer::Popover)?;
    let len = options.len();
    let mut rows = Vec::with_capacity(len);
    for (index, text) in options.iter().enumerate() {
        let on = index == highlighted;
        rows.push(tree.insert(
            list,
            row_style.clone(),
            option_node(text, index, len, on),
            WidgetKind::ListRow(ListRowState {
                selected: on,
                disabled: false,
            }),
        )?);
    }
    write_structure(tree, id, Some(list), rows)
}

/// Records the reconciled `list`/`rows` in `id`'s own state.
fn write_structure(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    list: Option<WidgetId>,
    rows: Vec<WidgetId>,
) -> Result<(), WidgetError> {
    let Some(WidgetKind::Dropdown(state)) = tree.payload_mut(id) else {
        return Err(WidgetError::WrongWidgetKind(id));
    };
    state.list = list;
    state.rows = rows;
    Ok(())
}

/// `sync_structure`'s failure path: remove every list child of `id`
/// (including a partly built one) and leave the state closed, so no
/// untracked list survives. Best effort by construction — it runs while
/// an error is already being returned, so a second failure here (which,
/// like the first, needs an id that does not exist) is not reported.
fn close_after_failure(tree: &mut WidgetTree<WidgetKind>, id: WidgetId) {
    // Ignored: see this function's doc comment.
    let _ = remove_lists_except(tree, id, None);
    if let Some(WidgetKind::Dropdown(state)) = tree.payload_mut(id) {
        state.highlighted = None;
        state.list = None;
        state.rows.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DropdownKey, DropdownOutcome, DropdownState, close_after_failure, dropdown_state,
        handle_dropdown_key, insert_dropdown, set_dropdown_disabled, set_dropdown_open,
        set_dropdown_selected, toggle_dropdown,
    };
    use crate::WidgetError;
    use crate::shortcut::NamedKey;
    use crate::tree::{PaintLayer, WidgetId, WidgetTree};
    use crate::widgets::{
        ListRowState, WidgetKind, insert_button, new_tree, row_height, test_scales,
    };
    use accesskit::{Action, Role};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    const ID: WidgetId = accesskit::NodeId(7);

    fn options() -> Vec<String> {
        ["Normal", "Multiply", "Screen"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn pure(selected: Option<usize>) -> DropdownState {
        DropdownState::new("Blend mode".to_owned(), options(), selected, 21.0)
    }

    fn opened_at(highlight: usize) -> DropdownState {
        let mut state = pure(Some(highlight));
        assert_eq!(state.open(), DropdownOutcome::Opened);
        assert_eq!(state.highlighted(), Some(highlight));
        state
    }

    fn key(state: &mut DropdownState, key: DropdownKey) -> DropdownOutcome {
        match state.apply_key(ID, key) {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    const KEYS: [DropdownKey; 4] = [
        DropdownKey::Up,
        DropdownKey::Down,
        DropdownKey::Enter,
        DropdownKey::Escape,
    ];

    // ---- The pure transition table, every row of it -------------------

    /// Built only through real transitions: disabling an open dropdown
    /// closes it (`apply_disabled`), so "open and disabled" is not a
    /// reachable state and is not constructed here.
    #[test]
    fn a_disabled_dropdown_refuses_every_key_and_changes_nothing() {
        for was_open in [false, true] {
            for k in KEYS {
                let mut state = if was_open {
                    opened_at(1)
                } else {
                    pure(Some(1))
                };
                assert!(state.apply_disabled(true));
                assert!(state.is_disabled());
                assert!(!state.is_open(), "disabling closed it");
                let before = state.clone();
                match state.apply_key(ID, k) {
                    Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, ID),
                    other => unreachable!("{k:?}: expected WidgetDisabled, got {other:?}"),
                }
                assert_eq!(state, before, "{k:?} must not change a disabled dropdown");
            }
        }
    }

    #[test]
    fn a_closed_dropdown_with_no_options_ignores_every_key() {
        for k in KEYS {
            let mut state = DropdownState::new("Empty".to_owned(), Vec::new(), None, 21.0);
            assert_eq!(key(&mut state, k), DropdownOutcome::Ignored, "{k:?}");
            assert!(!state.is_open());
        }
    }

    #[test]
    fn down_up_and_enter_open_a_closed_dropdown_on_its_selection() {
        for k in [DropdownKey::Down, DropdownKey::Up, DropdownKey::Enter] {
            let mut state = pure(Some(2));
            assert_eq!(key(&mut state, k), DropdownOutcome::Opened, "{k:?}");
            assert_eq!(state.highlighted(), Some(2), "{k:?} seeds on the selection");
            assert_eq!(state.selected(), Some(2), "{k:?} opening commits nothing");
        }
    }

    #[test]
    fn opening_with_nothing_selected_highlights_the_first_option() {
        for k in [DropdownKey::Down, DropdownKey::Up, DropdownKey::Enter] {
            let mut state = pure(None);
            assert_eq!(key(&mut state, k), DropdownOutcome::Opened);
            assert_eq!(state.highlighted(), Some(0), "{k:?}");
            assert_eq!(state.selected(), None);
        }
    }

    #[test]
    fn escape_on_a_closed_dropdown_is_ignored() {
        let mut state = pure(Some(1));
        let before = state.clone();
        assert_eq!(
            key(&mut state, DropdownKey::Escape),
            DropdownOutcome::Ignored
        );
        assert_eq!(state, before);
    }

    #[test]
    fn down_and_up_move_the_highlight_one_step_while_open() {
        let mut state = opened_at(1);
        assert_eq!(
            key(&mut state, DropdownKey::Down),
            DropdownOutcome::HighlightMoved(2)
        );
        assert_eq!(
            key(&mut state, DropdownKey::Up),
            DropdownOutcome::HighlightMoved(1)
        );
        assert_eq!(
            key(&mut state, DropdownKey::Up),
            DropdownOutcome::HighlightMoved(0)
        );
        assert_eq!(
            state.selected(),
            Some(1),
            "moving the highlight commits nothing"
        );
        assert!(state.is_open());
    }

    /// APG select-only: the highlight stops at either end, it does not
    /// wrap (unlike `CommandPalette`'s own selection, which does).
    #[test]
    fn the_highlight_is_clamped_at_both_ends_and_never_wraps() {
        let mut state = opened_at(2);
        let before = state.clone();
        assert_eq!(key(&mut state, DropdownKey::Down), DropdownOutcome::Ignored);
        assert_eq!(state, before, "Down on the last option changes nothing");

        let mut state = opened_at(0);
        let before = state.clone();
        assert_eq!(key(&mut state, DropdownKey::Up), DropdownOutcome::Ignored);
        assert_eq!(state, before, "Up on the first option changes nothing");
    }

    #[test]
    fn enter_commits_the_highlight_and_closes() {
        let mut state = opened_at(0);
        key(&mut state, DropdownKey::Down);
        assert_eq!(
            key(&mut state, DropdownKey::Enter),
            DropdownOutcome::Committed {
                index: 1,
                changed: true
            }
        );
        assert_eq!(state.selected(), Some(1));
        assert!(!state.is_open());
        assert_eq!(state.highlighted(), None);
    }

    #[test]
    fn committing_the_existing_selection_reports_no_change() {
        let mut state = opened_at(2);
        assert_eq!(
            key(&mut state, DropdownKey::Enter),
            DropdownOutcome::Committed {
                index: 2,
                changed: false
            }
        );
        assert_eq!(state.selected(), Some(2));
    }

    #[test]
    fn escape_closes_without_committing() {
        let mut state = opened_at(0);
        key(&mut state, DropdownKey::Down);
        assert_eq!(
            key(&mut state, DropdownKey::Escape),
            DropdownOutcome::Cancelled
        );
        assert_eq!(
            state.selected(),
            Some(0),
            "the moved highlight was not committed"
        );
        assert!(!state.is_open());
    }

    #[test]
    fn open_and_close_requests_are_ignored_when_already_in_that_state() {
        let mut state = pure(Some(1));
        assert_eq!(
            state.apply_open(ID, false).ok(),
            Some(DropdownOutcome::Ignored)
        );
        assert_eq!(
            state.apply_open(ID, true).ok(),
            Some(DropdownOutcome::Opened)
        );
        assert_eq!(
            state.highlighted(),
            Some(1),
            "set_open(true) seeds like Down"
        );
        assert_eq!(
            state.apply_open(ID, true).ok(),
            Some(DropdownOutcome::Ignored)
        );
        assert_eq!(
            state.apply_open(ID, false).ok(),
            Some(DropdownOutcome::Cancelled)
        );
        assert_eq!(state.selected(), Some(1));
    }

    #[test]
    fn disabling_an_open_dropdown_closes_it_without_committing() {
        let mut state = opened_at(0);
        key(&mut state, DropdownKey::Down);
        assert!(state.apply_disabled(true));
        assert!(state.is_disabled());
        assert!(!state.is_open());
        assert_eq!(state.selected(), Some(0));
        assert!(
            !state.apply_disabled(true),
            "disabling a disabled dropdown changes nothing"
        );
        assert!(state.apply_disabled(false));
        assert!(!state.is_disabled());
        assert!(!state.is_open(), "re-enabling does not reopen");
    }

    #[test]
    fn apply_selected_rejects_an_index_that_names_no_option() {
        let mut state = pure(Some(1));
        match state.apply_selected(Some(3)) {
            Err(WidgetError::IndexOutOfRange { index: 3, len: 3 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        assert_eq!(state.selected(), Some(1));
        assert_eq!(state.apply_selected(Some(1)).ok(), Some(false), "a no-op");
        assert_eq!(state.apply_selected(None).ok(), Some(true));
        assert_eq!(state.selected(), None);
    }

    /// An owner-driven selection while open re-seeds the highlight onto
    /// it, so a later `Enter` commits the owner's value rather than a
    /// stale highlight; `None` leaves the highlight alone.
    #[test]
    fn apply_selected_while_open_reseeds_the_highlight() {
        let mut state = opened_at(0);
        assert_eq!(state.apply_selected(Some(2)).ok(), Some(true));
        assert_eq!(state.highlighted(), Some(2));
        assert_eq!(
            key(&mut state, DropdownKey::Enter),
            DropdownOutcome::Committed {
                index: 2,
                changed: false
            },
            "Enter commits what the owner set, not the old highlight"
        );

        let mut state = opened_at(1);
        assert_eq!(state.apply_selected(None).ok(), Some(true));
        assert_eq!(state.highlighted(), Some(1), "None has nothing to seed on");
        assert_eq!(state.selected(), None);

        let mut state = pure(Some(0));
        assert_eq!(state.apply_selected(Some(2)).ok(), Some(true));
        assert!(!state.is_open(), "a closed dropdown stays closed");
    }

    #[test]
    fn named_keys_map_onto_the_four_dropdown_keys_and_nothing_else() {
        assert_eq!(
            DropdownKey::from_named_key(NamedKey::ArrowUp),
            Some(DropdownKey::Up)
        );
        assert_eq!(
            DropdownKey::from_named_key(NamedKey::ArrowDown),
            Some(DropdownKey::Down)
        );
        assert_eq!(
            DropdownKey::from_named_key(NamedKey::Enter),
            Some(DropdownKey::Enter)
        );
        assert_eq!(
            DropdownKey::from_named_key(NamedKey::Escape),
            Some(DropdownKey::Escape)
        );
        assert_eq!(DropdownKey::from_named_key(NamedKey::Space), None);
        assert_eq!(DropdownKey::from_named_key(NamedKey::Tab), None);
    }

    // ---- Tree wrappers -------------------------------------------------

    fn sized_root() -> Style {
        Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(200.0_f32),
                height: length(200.0_f32),
            },
            ..Default::default()
        }
    }

    fn inserted(selected: Option<usize>) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let id = match insert_dropdown(
            &mut tree,
            root,
            &test_scales(),
            "Blend mode",
            options(),
            selected,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (tree, id)
    }

    fn press(tree: &mut WidgetTree<WidgetKind>, id: WidgetId, k: DropdownKey) -> DropdownOutcome {
        match handle_dropdown_key(tree, id, k) {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn snapshot(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> DropdownState {
        match dropdown_state(tree, id) {
            Ok(state) => state.clone(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn insert_dropdown_builds_a_closed_combo_box_announcing_its_selection() {
        let (tree, id) = inserted(Some(1));
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::ComboBox);
        assert_eq!(node.label(), Some("Blend mode"));
        assert_eq!(node.value(), Some("Multiply"));
        assert_eq!(node.is_expanded(), Some(false));
        assert_eq!(node.active_descendant(), None);
        assert!(node.supports_action(Action::Focus));
        assert!(node.supports_action(Action::Click));
        assert!(node.supports_action(Action::Expand));
        assert!(
            !node.supports_action(Action::Collapse),
            "a closed dropdown must not offer the Collapse the Windows adapter refuses"
        );
        assert_eq!(
            tree.children(id),
            Some([].as_slice()),
            "closed: no list at all"
        );
    }

    #[test]
    fn a_dropdown_with_no_selection_has_no_value_rather_than_an_empty_one() {
        let (tree, id) = inserted(None);
        let Some(node) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.value(), None);
    }

    #[test]
    fn insert_dropdown_rejects_an_out_of_range_selection_and_adds_nothing() {
        let (mut tree, root) = new_tree(sized_root());
        let before = tree.len();
        match insert_dropdown(&mut tree, root, &test_scales(), "X", options(), Some(3)) {
            Err(WidgetError::IndexOutOfRange { index: 3, len: 3 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        assert_eq!(tree.len(), before);
    }

    #[test]
    fn insert_dropdown_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(sized_root());
        let bogus = accesskit::NodeId(999);
        match insert_dropdown(&mut tree, bogus, &test_scales(), "X", options(), None) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn opening_builds_a_real_listbox_child_with_one_option_per_entry() {
        let (mut tree, id) = inserted(Some(1));
        assert_eq!(
            press(&mut tree, id, DropdownKey::Down),
            DropdownOutcome::Opened
        );

        let state = snapshot(&tree, id);
        let Some(list) = state.list() else {
            unreachable!("open means a list exists");
        };
        assert_eq!(
            tree.children(id),
            Some([list].as_slice()),
            "the list is the combo's child"
        );
        assert_eq!(tree.payload(list), Some(&WidgetKind::DropdownList));
        let Some(list_node) = tree.accessibility(list) else {
            unreachable!("just inserted");
        };
        assert_eq!(list_node.role(), Role::ListBox);
        assert_eq!(list_node.label(), Some("Blend mode"));

        let Some(rows) = tree.children(list) else {
            unreachable!("just inserted");
        };
        assert_eq!(rows, state.rows());
        assert_eq!(rows.len(), 3);
        let mut selected = 0;
        for (index, (&row, text)) in rows.iter().zip(options()).enumerate() {
            let Some(node) = tree.accessibility(row) else {
                unreachable!("just inserted");
            };
            assert_eq!(node.role(), Role::ListBoxOption);
            assert_eq!(node.label(), Some(text.as_str()));
            for action in [
                Action::Focus,
                Action::Click,
                Action::Expand,
                Action::Collapse,
            ] {
                assert!(
                    !node.supports_action(action),
                    "an option declares no actions -- in particular no Focus: {action:?}"
                );
            }
            let on = index == 1;
            assert_eq!(node.is_selected(), Some(on));
            assert_eq!(
                tree.payload(row),
                Some(&WidgetKind::ListRow(ListRowState {
                    selected: on,
                    disabled: false
                }))
            );
            selected += usize::from(on);
        }
        assert_eq!(selected, 1, "exactly one option is selected");

        let Some(node) = tree.accessibility(id) else {
            unreachable!("still exists");
        };
        assert_eq!(node.is_expanded(), Some(true));
        assert!(node.supports_action(Action::Collapse));
        assert!(!node.supports_action(Action::Expand));
        assert_eq!(node.active_descendant(), rows.get(1).copied());
    }

    #[test]
    fn moving_the_highlight_updates_both_payload_and_node_and_the_active_descendant() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        assert_eq!(
            press(&mut tree, id, DropdownKey::Down),
            DropdownOutcome::HighlightMoved(1)
        );
        let rows = snapshot(&tree, id).rows().to_vec();
        for (index, &row) in rows.iter().enumerate() {
            let on = index == 1;
            assert_eq!(
                tree.accessibility(row)
                    .and_then(accesskit::Node::is_selected),
                Some(on)
            );
            assert_eq!(
                tree.payload(row),
                Some(&WidgetKind::ListRow(ListRowState {
                    selected: on,
                    disabled: false
                })),
                "paint_widget reads the payload, so it must move too"
            );
        }
        assert_eq!(
            tree.accessibility(id)
                .and_then(accesskit::Node::active_descendant),
            rows.get(1).copied()
        );
        assert_eq!(
            press(&mut tree, id, DropdownKey::Up),
            DropdownOutcome::HighlightMoved(0)
        );
        assert_eq!(
            tree.accessibility(id)
                .and_then(accesskit::Node::active_descendant),
            rows.first().copied()
        );
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            Some("Normal"),
            "moving the highlight does not change the announced value"
        );
    }

    #[test]
    fn enter_removes_the_rows_and_updates_the_value() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        press(&mut tree, id, DropdownKey::Down);
        let state = snapshot(&tree, id);
        let (Some(list), rows) = (state.list(), state.rows().to_vec()) else {
            unreachable!("open");
        };
        assert_eq!(
            press(&mut tree, id, DropdownKey::Enter),
            DropdownOutcome::Committed {
                index: 1,
                changed: true
            }
        );
        assert!(!tree.contains(list), "closing removes the list outright");
        for row in rows {
            assert!(!tree.contains(row), "... and every option row with it");
        }
        let state = snapshot(&tree, id);
        assert_eq!(state.list(), None);
        assert!(state.rows().is_empty());
        let Some(node) = tree.accessibility(id) else {
            unreachable!("still exists");
        };
        assert_eq!(node.value(), Some("Multiply"));
        assert_eq!(node.is_expanded(), Some(false));
        assert_eq!(node.active_descendant(), None);
        assert!(node.supports_action(Action::Expand));
    }

    #[test]
    fn escape_removes_the_rows_and_keeps_the_value() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        press(&mut tree, id, DropdownKey::Down);
        let Some(list) = snapshot(&tree, id).list() else {
            unreachable!("open");
        };
        assert_eq!(
            press(&mut tree, id, DropdownKey::Escape),
            DropdownOutcome::Cancelled
        );
        assert!(!tree.contains(list));
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            Some("Normal")
        );
    }

    #[test]
    fn toggle_and_set_open_open_and_close_the_real_list() {
        let (mut tree, id) = inserted(None);
        assert_eq!(
            toggle_dropdown(&mut tree, id).ok(),
            Some(DropdownOutcome::Opened)
        );
        assert!(snapshot(&tree, id).list().is_some());
        assert_eq!(
            toggle_dropdown(&mut tree, id).ok(),
            Some(DropdownOutcome::Cancelled)
        );
        assert!(snapshot(&tree, id).list().is_none());
        assert_eq!(
            set_dropdown_open(&mut tree, id, true).ok(),
            Some(DropdownOutcome::Opened)
        );
        assert_eq!(
            set_dropdown_open(&mut tree, id, true).ok(),
            Some(DropdownOutcome::Ignored)
        );
        assert_eq!(
            set_dropdown_open(&mut tree, id, false).ok(),
            Some(DropdownOutcome::Cancelled)
        );
        assert_eq!(tree.children(id), Some([].as_slice()));
    }

    #[test]
    fn a_disabled_dropdown_declares_no_actions_and_refuses_keys() {
        let (mut tree, id) = inserted(Some(1));
        if let Err(err) = set_dropdown_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        let Some(node) = tree.accessibility(id) else {
            unreachable!("still exists");
        };
        assert!(node.is_disabled());
        for action in [
            Action::Focus,
            Action::Click,
            Action::Expand,
            Action::Collapse,
        ] {
            assert!(!node.supports_action(action), "{action:?}");
        }
        assert_eq!(
            node.value(),
            Some("Multiply"),
            "a disabled dropdown still reports its value"
        );
        let before = snapshot(&tree, id);
        for k in KEYS {
            match handle_dropdown_key(&mut tree, id, k) {
                Err(WidgetError::WidgetDisabled(err_id)) => assert_eq!(err_id, id),
                other => unreachable!("{k:?}: expected WidgetDisabled, got {other:?}"),
            }
        }
        match toggle_dropdown(&mut tree, id) {
            Err(WidgetError::WidgetDisabled(_)) => {}
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
        assert_eq!(snapshot(&tree, id), before);
        assert_eq!(tree.children(id), Some([].as_slice()));
    }

    #[test]
    fn disabling_an_open_dropdown_removes_its_list() {
        let (mut tree, id) = inserted(Some(1));
        press(&mut tree, id, DropdownKey::Down);
        let Some(list) = snapshot(&tree, id).list() else {
            unreachable!("open");
        };
        if let Err(err) = set_dropdown_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(list));
        let state = snapshot(&tree, id);
        assert!(!state.is_open());
        assert_eq!(state.selected(), Some(1));
        assert_eq!(
            tree.accessibility(id)
                .and_then(accesskit::Node::is_expanded),
            Some(false)
        );
    }

    #[test]
    fn set_dropdown_selected_updates_the_value_and_is_allowed_while_disabled() {
        let (mut tree, id) = inserted(Some(0));
        if let Err(err) = set_dropdown_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_dropdown_selected(&mut tree, id, Some(2)) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            Some("Screen")
        );
        if let Err(err) = set_dropdown_selected(&mut tree, id, None) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            None
        );
        match set_dropdown_selected(&mut tree, id, Some(9)) {
            Err(WidgetError::IndexOutOfRange { index: 9, len: 3 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        assert_eq!(
            snapshot(&tree, id).selected(),
            None,
            "a rejected index changes nothing"
        );
    }

    #[test]
    fn dropdown_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(sized_root());
        let results = [
            handle_dropdown_key(&mut tree, root, DropdownKey::Down).map(|_| ()),
            set_dropdown_open(&mut tree, root, true).map(|_| ()),
            toggle_dropdown(&mut tree, root).map(|_| ()),
            set_dropdown_disabled(&mut tree, root, true),
            set_dropdown_selected(&mut tree, root, None),
            dropdown_state(&tree, root).map(|_| ()),
        ];
        for result in results {
            match result {
                Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
                other => unreachable!("expected WrongWidgetKind, got {other:?}"),
            }
        }
    }

    #[test]
    fn dropdown_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(sized_root());
        let bogus = accesskit::NodeId(999);
        let results = [
            handle_dropdown_key(&mut tree, bogus, DropdownKey::Down).map(|_| ()),
            set_dropdown_open(&mut tree, bogus, true).map(|_| ()),
            toggle_dropdown(&mut tree, bogus).map(|_| ()),
            set_dropdown_disabled(&mut tree, bogus, true),
            set_dropdown_selected(&mut tree, bogus, None),
            dropdown_state(&tree, bogus).map(|_| ()),
        ];
        for result in results {
            match result {
                Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
                other => unreachable!("expected UnknownWidget, got {other:?}"),
            }
        }
    }

    /// Both halves of "dirty": an open changes the control's border
    /// colour, so it must reach the damage region a renderer reads, not
    /// just the per-widget flag. And an `Ignored` key must cost nothing.
    #[test]
    fn opening_widens_the_damage_region_and_an_ignored_key_does_not() {
        let (mut tree, id) = inserted(Some(0));
        tree.compute_layout(200.0, 200.0);
        tree.take_damage();

        assert_eq!(
            press(&mut tree, id, DropdownKey::Escape),
            DropdownOutcome::Ignored
        );
        assert_eq!(tree.take_damage(), None, "an ignored key changes no pixels");

        press(&mut tree, id, DropdownKey::Down);
        assert_eq!(tree.is_dirty(id), Some(true));
        assert_eq!(
            tree.take_damage(),
            tree.bounds(id),
            "the control's own bounds"
        );

        tree.compute_layout(200.0, 200.0);
        tree.take_damage();
        let Some(list) = snapshot(&tree, id).list() else {
            unreachable!("open");
        };
        let list_bounds = tree.bounds(list);
        press(&mut tree, id, DropdownKey::Escape);
        let Some(damage) = tree.take_damage() else {
            unreachable!("closing must produce damage");
        };
        let Some(list_bounds) = list_bounds else {
            unreachable!("laid out");
        };
        assert!(
            damage.x <= list_bounds.x
                && damage.bottom() >= list_bounds.bottom()
                && damage.right() >= list_bounds.right(),
            "the vacated list area must be repainted: {damage:?} vs {list_bounds:?}"
        );
    }

    /// The disconnected-tree/stale-focus crash class
    /// `WidgetTree::accessibility_update`'s own doc comment records:
    /// every shape this widget produces — closed, open (with an
    /// `active_descendant`), and after a close removed the list — must be
    /// an update `accesskit_consumer` accepts.
    #[test]
    fn accesskit_consumer_accepts_the_update_closed_open_and_after_closing() {
        let (mut tree, id) = inserted(Some(1));
        let _closed = accesskit_consumer::Tree::new(tree.accessibility_update(id), true);

        press(&mut tree, id, DropdownKey::Down);
        let open = accesskit_consumer::Tree::new(tree.accessibility_update(id), true);
        let highlighted = snapshot(&tree, id).highlighted_row();
        let focus = open.state().focus();
        assert_eq!(
            focus.map(|node| (node.role(), node.label())),
            Some((Role::ListBoxOption, Some("Multiply".to_owned()))),
            "the consumer resolves focus through active_descendant to the highlighted option"
        );

        // `accesskit_consumer` 0.38 computes neither `position_in_set`
        // nor `size_of_set`; every option declares both itself, and they
        // survive a highlight move (which rewrites two option nodes).
        press(&mut tree, id, DropdownKey::Down);
        let moved = accesskit_consumer::Tree::new(tree.accessibility_update(id), true);
        let mut positions: Vec<(Option<usize>, Option<usize>)> = Vec::new();
        let mut pending = vec![moved.state().root()];
        while let Some(node) = pending.pop() {
            if node.role() == Role::ListBoxOption {
                positions.push((node.position_in_set(), node.size_of_set()));
            }
            pending.extend(node.children());
        }
        let len = snapshot(&tree, id).options.len();
        let mut expected: Vec<(Option<usize>, Option<usize>)> =
            (1..=len).map(|p| (Some(p), Some(len))).collect();
        positions.sort_unstable();
        expected.sort_unstable();
        assert_eq!(positions, expected, "every option says `i of n`");

        let Some(row) = highlighted else {
            unreachable!("open");
        };
        press(&mut tree, id, DropdownKey::Escape);
        assert!(!tree.contains(row));
        let _after = accesskit_consumer::Tree::new(tree.accessibility_update(id), true);
        // A focus left on a removed option (a caller that focused it
        // directly) falls back to the root rather than panicking.
        let _stale = accesskit_consumer::Tree::new(tree.accessibility_update(row), true);
    }

    #[test]
    fn options_never_enter_the_tab_order() {
        let (mut tree, id) = inserted(Some(0));
        let root = tree.root();
        let button = match insert_button(&mut tree, root, &test_scales(), "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        press(&mut tree, id, DropdownKey::Down);
        let mut focus = crate::FocusManager::new();
        let mut visited = Vec::new();
        for _ in 0..4 {
            if let Some(next) = focus.focus_next(&mut tree) {
                visited.push(next);
            }
        }
        assert_eq!(visited, vec![id, button, id, button]);
    }

    fn laid_out_open() -> (WidgetTree<WidgetKind>, WidgetId, DropdownState) {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        tree.compute_layout(200.0, 200.0);
        let state = snapshot(&tree, id);
        (tree, id, state)
    }

    #[test]
    fn the_open_list_hangs_below_the_control_at_its_width_with_rows_stacked() {
        let (tree, id, state) = laid_out_open();
        let row = row_height(&test_scales());
        let (Some(control), Some(list)) =
            (tree.bounds(id), state.list().and_then(|l| tree.bounds(l)))
        else {
            unreachable!("laid out");
        };
        assert_eq!(control.width, 200, "the control fills its parent's width");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row_px = row as u32;
        assert_eq!(control.height, row_px, "one row tall");
        assert_eq!(
            list.y,
            control.bottom(),
            "the list starts at the control's bottom edge"
        );
        assert_eq!(list.x, control.x);
        assert_eq!(list.width, control.width);
        assert_eq!(list.height, row_px * 3, "exactly its three rows tall");
        let mut expected_y = list.y;
        for &r in state.rows() {
            let Some(bounds) = tree.bounds(r) else {
                unreachable!("laid out");
            };
            assert_eq!(bounds.y, expected_y);
            assert_eq!(bounds.height, row_px);
            assert_eq!(bounds.x, list.x);
            assert_eq!(bounds.width, list.width);
            expected_y += i64::from(row_px);
        }
    }

    /// Opening does not push siblings around: the list is out of flow.
    #[test]
    fn opening_does_not_move_a_following_sibling() {
        let (mut tree, id) = inserted(Some(0));
        let root = tree.root();
        let button = match insert_button(&mut tree, root, &test_scales(), "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(200.0, 200.0);
        let closed = tree.bounds(button);
        press(&mut tree, id, DropdownKey::Down);
        tree.compute_layout(200.0, 200.0);
        assert_eq!(tree.bounds(button), closed);
    }

    /// The open list is a popover root: both hit-testers reach an
    /// option even though it lies entirely below the control's own
    /// bounds (it replaces 0.121.0's
    /// `hit_testing_an_open_option_does_not_reach_the_option`, which
    /// pinned the opposite as a disclosed limitation), and `focus_at`
    /// over it climbs row -> list -> control and focuses the control.
    #[test]
    fn hit_testing_an_open_option_reaches_the_option() {
        let (mut tree, id, state) = laid_out_open();
        let Some(list) = state.list() else {
            unreachable!("open");
        };
        assert_eq!(tree.layer(list), Some(PaintLayer::Popover));
        assert_eq!(tree.popover_root_of(list), Some(list));
        let Some(&row) = state.rows().get(1) else {
            unreachable!("three rows");
        };
        assert_eq!(tree.popover_root_of(row), Some(list));
        let Some(bounds) = tree.bounds(row) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let (x, y) = (bounds.x as f32 + 5.0, bounds.y as f32 + 5.0);
        assert_eq!(tree.hit_test((x, y)), Some(row));
        assert_eq!(
            crate::hit_test(&tree, f64::from(x), f64::from(y)),
            Some(row)
        );
        let mut focus = crate::FocusManager::new();
        assert_eq!(
            focus.focus_at(&mut tree, f64::from(x), f64::from(y)),
            Some(id),
            "an option declares no Focus action, so the click bubbles to the control"
        );
    }

    /// The list subtree is the last thing painted, after a later sibling
    /// button that the open list overlaps.
    #[test]
    fn the_open_list_paints_after_a_later_sibling() {
        let (mut tree, id) = inserted(Some(0));
        let root = tree.root();
        let button = match insert_button(&mut tree, root, &test_scales(), "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        press(&mut tree, id, DropdownKey::Down);
        tree.compute_layout(200.0, 200.0);
        let state = snapshot(&tree, id);
        let Some(list) = state.list() else {
            unreachable!("open");
        };
        let order = tree.paint_order();
        let mut expected_tail = vec![list];
        expected_tail.extend_from_slice(state.rows());
        assert!(
            order.ends_with(&expected_tail),
            "{order:?} must end with {expected_tail:?}"
        );
        let position = |w: WidgetId| order.iter().position(|&x| x == w);
        assert!(position(button) < position(list));
        assert_eq!(order.len(), tree.len(), "every widget exactly once");
        // The button sits where the list hangs, so the list must win.
        let Some(button_bounds) = tree.bounds(button) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let point = (button_bounds.x as f32 + 2.0, button_bounds.y as f32 + 2.0);
        let hit = tree.hit_test(point);
        assert!(
            hit.is_some_and(|h| tree.popover_root_of(h) == Some(list)),
            "the button under the open list is not what a click reaches: {hit:?}"
        );
    }

    /// A list removed behind this module's back is rebuilt on the next
    /// transition rather than trusted.
    #[test]
    fn a_list_removed_externally_is_rebuilt_rather_than_trusted() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        let Some(list) = snapshot(&tree, id).list() else {
            unreachable!("open");
        };
        if let Err(err) = tree.remove(list) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            press(&mut tree, id, DropdownKey::Down),
            DropdownOutcome::HighlightMoved(1)
        );
        let state = snapshot(&tree, id);
        let Some(new_list) = state.list() else {
            unreachable!("still open");
        };
        assert!(tree.contains(new_list));
        assert_eq!(state.rows().len(), 3);
        assert!(state.rows().iter().all(|&r| tree.contains(r)));
        assert_eq!(
            tree.accessibility(id)
                .and_then(accesskit::Node::active_descendant),
            state.rows().get(1).copied()
        );
    }

    fn list_children(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Vec<WidgetId> {
        tree.children(id)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&c| tree.payload(c) == Some(&WidgetKind::DropdownList))
            .collect()
    }

    /// Every structural invariant an open dropdown must hold: exactly one
    /// list child, whose children are exactly `rows` (all live), one per
    /// option, with exactly the highlighted one selected in both payload
    /// and node, and the control's `active_descendant` a live row.
    fn assert_open_structure_is_sound(tree: &WidgetTree<WidgetKind>, id: WidgetId) {
        let state = snapshot(tree, id);
        let Some(highlighted) = state.highlighted() else {
            unreachable!("expected an open dropdown");
        };
        let Some(list) = state.list() else {
            unreachable!("open means a list exists");
        };
        assert_eq!(
            list_children(tree, id),
            vec![list],
            "exactly one list child"
        );
        assert_eq!(tree.children(list), Some(state.rows()));
        assert_eq!(state.rows().len(), state.options().len());
        for (index, &row) in state.rows().iter().enumerate() {
            assert!(tree.contains(row));
            let on = index == highlighted;
            assert_eq!(
                tree.payload(row),
                Some(&WidgetKind::ListRow(ListRowState {
                    selected: on,
                    disabled: false
                })),
                "row {index}"
            );
            assert_eq!(
                tree.accessibility(row)
                    .and_then(accesskit::Node::is_selected),
                Some(on),
                "row {index}"
            );
        }
        let Some(active) = tree
            .accessibility(id)
            .and_then(accesskit::Node::active_descendant)
        else {
            unreachable!("open means an active descendant");
        };
        assert!(
            tree.contains(active),
            "active_descendant must be a live node"
        );
        assert_eq!(Some(active), state.highlighted_row());
    }

    /// R1: a single *row* removed behind this module's back (not the
    /// whole list) must not be trusted either — before the fix the stale
    /// id stayed in `rows`, `active_descendant` dangled, and `Enter`
    /// could commit an index whose row was gone.
    #[test]
    fn a_row_removed_externally_is_rebuilt_rather_than_trusted() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        let Some(&row1) = snapshot(&tree, id).rows().get(1) else {
            unreachable!("three rows");
        };
        if let Err(err) = tree.remove(row1) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            press(&mut tree, id, DropdownKey::Down),
            DropdownOutcome::HighlightMoved(1)
        );
        assert_open_structure_is_sound(&tree, id);
        assert!(!snapshot(&tree, id).rows().contains(&row1));
        let _consumer = accesskit_consumer::Tree::new(tree.accessibility_update(id), true);
    }

    /// R2: a stale snapshot written back through `payload_mut` must not
    /// leave a second, orphaned list under the control.
    #[test]
    fn a_stale_snapshot_written_back_never_leaves_a_second_list() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        let stale = snapshot(&tree, id);
        press(&mut tree, id, DropdownKey::Escape);
        press(&mut tree, id, DropdownKey::Down);
        assert_eq!(list_children(&tree, id).len(), 1);
        let Some(kind) = tree.payload_mut(id) else {
            unreachable!("exists");
        };
        *kind = WidgetKind::Dropdown(stale);

        press(&mut tree, id, DropdownKey::Down);
        assert_eq!(
            list_children(&tree, id).len(),
            1,
            "exactly one DropdownList child after the write-back"
        );
        assert_open_structure_is_sound(&tree, id);

        press(&mut tree, id, DropdownKey::Escape);
        assert!(list_children(&tree, id).is_empty(), "closing leaves none");
        assert_eq!(tree.children(id), Some([].as_slice()));
    }

    /// R2's other half: a tracked list that is not this control's own
    /// child (a snapshot taken from another dropdown) is neither kept
    /// nor removed — it is not ours — and this control builds its own.
    #[test]
    fn a_tracked_list_owned_by_another_dropdown_is_left_alone() {
        let (mut tree, a) = inserted(Some(0));
        let root = tree.root();
        let b = match insert_dropdown(&mut tree, root, &test_scales(), "B", options(), Some(2)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        press(&mut tree, a, DropdownKey::Down);
        let a_state = snapshot(&tree, a);
        let Some(a_list) = a_state.list() else {
            unreachable!("open");
        };
        let Some(kind) = tree.payload_mut(b) else {
            unreachable!("exists");
        };
        *kind = WidgetKind::Dropdown(a_state);

        press(&mut tree, b, DropdownKey::Down);
        assert!(tree.contains(a_list), "a's own list is untouched");
        assert_eq!(list_children(&tree, a), vec![a_list]);
        assert_ne!(snapshot(&tree, b).list(), Some(a_list));
        assert_open_structure_is_sound(&tree, b);
    }

    /// The two-row cheap path must not be fooled by a stale hint: a
    /// snapshot written back over the *same* live list names a previous
    /// highlight that is no longer the shown one, and the fallback walk
    /// must still leave exactly one row selected.
    #[test]
    fn a_stale_highlight_hint_falls_back_to_a_full_walk() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        let stale = snapshot(&tree, id);
        press(&mut tree, id, DropdownKey::Down);
        press(&mut tree, id, DropdownKey::Down);
        assert_eq!(snapshot(&tree, id).highlighted(), Some(2));
        let Some(kind) = tree.payload_mut(id) else {
            unreachable!("exists");
        };
        *kind = WidgetKind::Dropdown(stale);

        assert_eq!(
            press(&mut tree, id, DropdownKey::Down),
            DropdownOutcome::HighlightMoved(1)
        );
        assert_open_structure_is_sound(&tree, id);
    }

    /// R9: moving the highlight touches exactly the two rows involved.
    #[test]
    fn moving_the_highlight_dirties_only_the_two_rows_involved() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        tree.compute_layout(200.0, 200.0);
        tree.take_damage();
        press(&mut tree, id, DropdownKey::Down);
        let rows = snapshot(&tree, id).rows().to_vec();
        let dirty: Vec<Option<bool>> = rows.iter().map(|&r| tree.is_dirty(r)).collect();
        assert_eq!(dirty, vec![Some(true), Some(true), Some(false)]);
        assert_open_structure_is_sound(&tree, id);
    }

    /// R4: a setter asked for the state it already has costs nothing.
    #[test]
    fn no_op_setters_produce_no_damage() {
        let (mut tree, id) = inserted(Some(1));
        tree.compute_layout(200.0, 200.0);
        tree.take_damage();
        if let Err(err) = set_dropdown_disabled(&mut tree, id, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_dropdown_selected(&mut tree, id, Some(1)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), None);
        assert_eq!(tree.is_dirty(id), Some(false));

        if let Err(err) = set_dropdown_selected(&mut tree, id, Some(2)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), tree.bounds(id), "a real change damages");
        if let Err(err) = set_dropdown_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        assert!(tree.take_damage().is_some());
        if let Err(err) = set_dropdown_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), None, "disabling twice is a no-op");
    }

    /// R7: an owner-set selection while open moves the real highlight
    /// row and `active_descendant` with it.
    #[test]
    fn set_dropdown_selected_while_open_moves_the_real_highlight() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        press(&mut tree, id, DropdownKey::Down);
        if let Err(err) = set_dropdown_selected(&mut tree, id, Some(2)) {
            unreachable!("{err:?}");
        }
        let state = snapshot(&tree, id);
        assert_eq!(state.highlighted(), Some(2));
        assert_open_structure_is_sound(&tree, id);
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            Some("Screen")
        );
        assert_eq!(
            press(&mut tree, id, DropdownKey::Enter),
            DropdownOutcome::Committed {
                index: 2,
                changed: false
            }
        );
    }

    /// R8: an empty option's text is never announced as a present,
    /// empty `value`.
    #[test]
    fn an_empty_selected_option_has_no_value() {
        let (mut tree, root) = new_tree(sized_root());
        let id = match insert_dropdown(
            &mut tree,
            root,
            &test_scales(),
            "Preset",
            vec![String::new(), "Custom".to_owned()],
            Some(0),
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(snapshot(&tree, id).selected_option(), Some(""));
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            None
        );
        if let Err(err) = set_dropdown_selected(&mut tree, id, Some(1)) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.accessibility(id).and_then(accesskit::Node::value),
            Some("Custom")
        );
    }

    /// R6: the failure path leaves no list behind — tracked or partly
    /// built and untracked — and the dropdown closed. Driven directly,
    /// since every error `sync_structure` can see is unreachable through
    /// the public API today (see its own doc comment).
    #[test]
    fn a_failed_reconcile_leaves_the_dropdown_closed_with_no_list() {
        let (mut tree, id) = inserted(Some(0));
        press(&mut tree, id, DropdownKey::Down);
        // A partly built, untracked list, as an insert failing partway
        // through `build_list` would leave.
        let partial = match tree.insert(
            id,
            Style::default(),
            accesskit::Node::new(Role::ListBox),
            WidgetKind::DropdownList,
        ) {
            Ok(list) => list,
            Err(err) => unreachable!("{err:?}"),
        };
        close_after_failure(&mut tree, id);
        assert!(!tree.contains(partial));
        assert!(list_children(&tree, id).is_empty());
        let state = snapshot(&tree, id);
        assert!(!state.is_open());
        assert_eq!(state.list(), None);
        assert!(state.rows().is_empty());
        assert_eq!(state.selected(), Some(0), "nothing was committed");
    }
}
