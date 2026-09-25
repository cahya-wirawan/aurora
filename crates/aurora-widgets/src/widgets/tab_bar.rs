//! A tab bar: a horizontal row of tabs, exactly one of which is
//! selected, moved with the arrow keys and `Home`/`End`.
//!
//! **Scope: the bar only.** This is a `Role::TabList` holding one real
//! `Role::Tab` child per tab. There are **no tab panels** — nothing here
//! owns, shows or hides content per tab; a caller reads
//! [`TabBarState::selected`] (or reacts to [`TabBarOutcome::Selected`])
//! and swaps its own content. `design/gallery/index.html`'s own
//! "Tab bar" entry is the same scope.
//!
//! **Shape: a pure state machine plus a structural reconcile**, the
//! same split `dropdown.rs` uses. [`TabBarState`] owns the labels and
//! the selected index; every key and every mutator is a transition on
//! that state alone (unit-tested with no tree at all). After every
//! successful call — a no-op included — the tree is reconciled to match: the
//! bar's [`WidgetKind::Tab`] children are exactly the tracked
//! [`TabBarState::tabs`], one per label, each carrying its own
//! `selected`/`disabled` in both its payload and its node. Unlike a
//! dropdown's list, the tabs are created **once, at insert**, and are
//! never removed by this module except to repair a structure a caller
//! damaged from outside (see "Lazy repair" below).
//!
//! # Keys
//!
//! [`TabBarKey`] is this module's own four-key vocabulary, for the same
//! disclosed reason `dropdown.rs` gives for its own: the transition table
//! below stays exhaustive and readable, and [`TabBarKey::from_named_key`]
//! bridges to `shortcut::NamedKey`, so `aurora-app`'s existing winit →
//! `NamedKey` translation composes with it. `NamedKey::Home` and
//! `NamedKey::End` were added for this widget (`0.121.0`), together with
//! their `KeyChord` parse/display entries and `aurora-app`'s winit
//! translation — the only three tables that enumerate `NamedKey`.
//!
//! **Keys are addressed to the bar's id**, not to a tab's. Focus sits on
//! the selected *tab* (see "Roving focus" below), so a caller routing a
//! key from the focused widget resolves the bar first with
//! `WidgetTree::parent(focused_tab)`.
//!
//! The table follows the WAI-ARIA APG *tabs* pattern with **automatic
//! activation** — moving selects, there is no separate "activate" key:
//!
//! | state | input | result |
//! |---|---|---|
//! | disabled | any key, [`select_tab`] | `Err(WidgetDisabled)`, nothing changes |
//! | one tab | any key | `Ignored` |
//! | `i` | `Right` | `i + 1`, or `0` from the last tab, `Selected` — **wraps** |
//! | `i` | `Left` | `i - 1`, or `n - 1` from the first tab, `Selected` — **wraps** |
//! | `i != 0` | `Home` | `Selected(0)`; at `0` it is `Ignored` |
//! | `i != n - 1` | `End` | `Selected(n - 1)`; at `n - 1` it is `Ignored` |
//! | any | [`select_tab`] out of range | `Err(IndexOutOfRange)` — checked **before** disabled |
//! | `i` | [`select_tab`]`(i)` | `Ignored`, no damage |
//! | any | [`set_tab_bar_disabled`] to the current value | no-op, no damage |
//!
//! Wrapping is the APG's own recommendation for tabs ("If focus is on
//! the last tab, moves focus to the first tab"), and deliberately the
//! opposite of `dropdown.rs`'s clamped list.
//!
//! # The accessibility vocabulary, checked against the pinned sources
//!
//! Checked against `accesskit` 0.24.1 and `accesskit_consumer` 0.38.0,
//! `accesskit_windows` 0.34.0, `accesskit_macos` 0.26.3,
//! `accesskit_atspi_common` 0.19.1, as pinned in `Cargo.lock`.
//!
//! - **The bar is `Role::TabList`** — mapped to UIA's `Tab` control type
//!   (`accesskit_windows` `src/node.rs:199`), `NSAccessibilityTabGroupRole`
//!   (`accesskit_macos` `src/node.rs:166`) and AT-SPI `PageTabList`
//!   (`accesskit_atspi_common` `src/node.rs:255`). Labelled with the
//!   bar's own name, **no actions** (it is never focused itself), and
//!   `set_disabled` when disabled.
//! - **Orientation is not set.** `accesskit_consumer`'s
//!   `Node::orientation` already reports `Horizontal` for a `TabList`
//!   with none (`src/node.rs:570-579`, the `TabList` arm at `574-575`),
//!   which a test pins.
//! - **Each tab is `Role::Tab`**, labelled with its text, and carries
//!   `selected` **on every tab**, `true` on exactly one and `false` on
//!   the rest — not absent on the rest: `Node::is_selectable` is "has the
//!   attribute at all, and is not disabled" (`src/node.rs:603-606`), and
//!   the Windows `SelectionItem` pattern for `Role::Tab` likewise needs
//!   the attribute present (`accesskit_windows` `src/node.rs:659-662`).
//!   A present `selected` also makes a tab non-*invocable*
//!   (`is_invocable`, `src/node.rs:653-667`), which is right: activating
//!   a tab selects it, it does not "invoke" anything.
//! - **Roving focus: only the selected tab declares `Action::Focus`.**
//!   Every enabled tab declares `Action::Click`; the selected one also
//!   declares `Focus`. So [`crate::FocusManager`]'s `Tab` order visits
//!   exactly one tab per bar — the APG's "when focus moves into the tab
//!   list, place focus on the active tab" — and an inactive tab is
//!   `NotFocusable`. A `Click` on the already-selected tab is honestly a
//!   no-op ([`select_tab`] returns `Ignored`). A **disabled** bar sets
//!   `set_disabled` on every tab and declares no actions on any of them.
//! - **`controls` is deliberately not set**, for `dropdown.rs`'s reason:
//!   `accesskit_consumer`'s `Node::controls` unwraps every id it is
//!   handed (`src/node.rs:940-946`), and there is no panel to point at
//!   anyway. No `active_descendant` either — focus is real, not virtual.
//!
//! # What this does not do, stated rather than implied away
//!
//! - **The caller must re-resolve focus after every transition, not only
//!   a selection change.** This module never touches a
//!   [`crate::FocusManager`] (no widget module does), and three things
//!   here take `Focus` away from the tab a caller may have focused:
//!   1. a [`TabBarOutcome::Selected`] moves the one tab that declares
//!      `Focus`;
//!   2. [`set_tab_bar_disabled`]`(true)` removes `Focus` from **every**
//!      tab, so nothing in the bar is focusable until it is re-enabled —
//!      and re-enabling gives it back only to the selected tab;
//!   3. a lazy repair (below) **replaces every tab id**, so a
//!      `FocusManager` can be left holding an id that no longer exists.
//!
//!   So after any successful call — including
//!   `set_tab_bar_disabled(false)` following a `true`, and an `Ignored`
//!   key, which may have repaired — a caller that had a tab of this bar
//!   focused must re-read [`TabBarState::selected_tab`] and call
//!   `FocusManager::focus(tree, it)` (or move focus elsewhere while the
//!   bar is disabled), or its focus is left on a tab that is dead or no
//!   longer focusable.
//! - **A pointer click on an inactive tab needs caller routing.** An
//!   inactive tab declares no `Focus`, so `FocusManager::focus_at` does
//!   not stop on it (and the bar above it is not focusable either). A
//!   caller that wants click-to-select hit-tests, maps the hit tab with
//!   [`TabBarState::index_of`], and calls [`select_tab`].
//! - **Accessibility `Click` and `Focus` are routed** (0.128.0) by
//!   `crate::action::handle_action`: a `Click` on a tab calls
//!   [`select_tab`] and, if focus sat on the previously selected tab,
//!   moves it to the new one (roving focus). Nothing else is declared,
//!   so nothing else is accepted.
//! - **No keyboard-focus ring is painted** — a crate-wide gap (nothing in
//!   `paint.rs` reads `FocusManager`), so the mockup's "focused" gallery
//!   state is not rendered here and the gallery has no cell for it. There
//!   is no `focused` field on purpose.
//! - **No glyphs**: tab labels reach the accessibility tree only. Tabs
//!   therefore share the bar's width **equally** (`flex_grow: 1`,
//!   `flex_basis: 0`) — a stand-in until text can be measured, not a
//!   design decision.
//! - **No per-tab disabled**, **no right-to-left** handling (`Left` always
//!   means "previous"), **no owner-driven setter** (there is no
//!   `set_tab_bar_selected` that works on a disabled bar — [`select_tab`]
//!   is a user gesture and refuses one), and **labels are fixed at
//!   insert**. None is needed yet.
//! - **Labels are not validated** — the caller's responsibility, the same
//!   rule `dropdown.rs` applies to its options. An empty label is
//!   accepted and exposed as an empty accessible name (a tab a screen
//!   reader can only announce as "tab 2 of 3"), and duplicate labels are
//!   accepted as distinct tabs. A test pins both, so a future decision to
//!   reject them is a visible behaviour change rather than a silent one.
//!
//! # Size
//!
//! **Tab bars are small, and this module is written for small.** Every
//! successful call walks every tab once (O(n): one expected `Node` built
//! and compared per tab), an `Ignored` key included — nothing at tab-bar
//! sizes, and measured at ~41 ms per key at 100,000 tabs (the `test`
//! profile, one run, not a benchmark). A lazy repair is **O(n²)**:
//! `WidgetTree::remove` rescans the parent's child list on every removal
//! and the tree has no bulk-remove, so rebuilding 100,000 tabs after one
//! external removal was measured at ~6.9 s. The practical ceiling is
//! "fits in a window's width" — tens of tabs; a bar with thousands of
//! entries wants a list or an overflow menu, not this widget.
//!
//! # Lazy repair
//!
//! If a caller removes a tab with `WidgetTree::remove`, writes a stale
//! [`TabBarState`] snapshot back through `WidgetTree::payload_mut`, or
//! overwrites a tab's or the bar's payload or node, the damage is
//! repaired on the next **successful** call into this module — every
//! one, an `Ignored` key and a no-op setter included (a call that returns
//! an error changes and repairs nothing). This deliberately differs from
//! `dropdown.rs`, whose no-ops return before reconciling: a tab bar's
//! tabs are persistent, so a caller reading
//! [`TabBarState::selected_tab`] after an `Ignored` key must not be
//! handed a dead id. On an intact bar the check costs no damage.
//!
//! Whenever the bar's `Tab` children are not exactly the tracked
//! [`TabBarState::tabs`], one per label, every `Tab` child is removed and
//! a fresh set is built from the labels, so no dead id survives in `tabs`
//! and no orphaned duplicate survives under the bar — **and every tab id
//! changes** (see the focus bullet above). Otherwise each tab's payload
//! and whole node, and the bar's whole node, are compared with what the
//! state says, and only what disagrees is rewritten.
//!
//! Children of any other kind a caller put under the bar are left alone,
//! but **not in place relative to the tabs**: `WidgetTree` can only
//! append a child, so a rebuild puts the fresh tabs **after** every such
//! child, whatever order they had before. Nothing in this crate puts a
//! non-`Tab` child under a bar.

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{AlignItems, FlexDirection, Rect as LayoutRect, Size, Style};

use super::{WidgetKind, row_height, spacing};
use crate::error::WidgetError;
use crate::shortcut::NamedKey;
use crate::tree::{WidgetId, WidgetTree};

/// The four keys a tab bar responds to — see this module's own doc
/// comment for the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabBarKey {
    Left,
    Right,
    Home,
    End,
}

impl TabBarKey {
    /// The tab-bar key `key` stands for, if any — `None` for every
    /// `NamedKey` a tab bar does not handle.
    #[must_use]
    pub fn from_named_key(key: NamedKey) -> Option<Self> {
        match key {
            NamedKey::ArrowLeft => Some(Self::Left),
            NamedKey::ArrowRight => Some(Self::Right),
            NamedKey::Home => Some(Self::Home),
            NamedKey::End => Some(Self::End),
            _ => None,
        }
    }
}

/// What a transition did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabBarOutcome {
    /// The selection moved to this tab index. The caller should move
    /// focus to [`TabBarState::selected_tab`] — see this module's own
    /// doc comment.
    Selected(usize),
    /// Nothing changed at all.
    Ignored,
}

/// The tab layout values resolved from the token scales at insert time
/// and carried in [`TabBarState`], because a lazy repair rebuilds tabs
/// from mutators that take no `&Scales`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TabMetrics {
    /// `spacing.xs`, above and below.
    pad_v: f32,
    /// `spacing.sm`, left and right.
    pad_h: f32,
    /// `row_height(scales)`.
    min_height: f32,
}

/// A tab bar's own state — the payload of its bar
/// ([`WidgetKind::TabBar`]).
///
/// Only `label` is public; everything else is kept in lockstep with the
/// real `Tab` children, so it sits behind getters. `Clone`, so a caller
/// can snapshot it; writing a stale snapshot back is tolerated (the next
/// successful call into this module repairs the structure — see "Lazy repair"
/// in this module's own doc comment) but is not a supported way to
/// change a tab bar.
///
/// `PartialEq` but not `Eq`: it carries resolved `f32` layout metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct TabBarState {
    /// The bar's own accessible name.
    pub label: String,
    disabled: bool,
    labels: Vec<String>,
    /// Always `< labels.len()` — a tab bar has exactly one selected tab.
    selected: usize,
    /// One `Tab` child per label, in order.
    tabs: Vec<WidgetId>,
    metrics: TabMetrics,
}

impl TabBarState {
    fn new(label: String, labels: Vec<String>, selected: usize, metrics: TabMetrics) -> Self {
        Self {
            label,
            disabled: false,
            labels,
            selected,
            tabs: Vec::new(),
            metrics,
        }
    }

    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Every tab's label, in order.
    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// The selected tab's index.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The `Tab` widgets, in label order.
    #[must_use]
    pub fn tabs(&self) -> &[WidgetId] {
        &self.tabs
    }

    /// The selected tab's widget — the one tab that declares
    /// `Action::Focus` (on an enabled bar), and where a caller should move
    /// focus after a [`TabBarOutcome::Selected`].
    ///
    /// This is a plain read of the tracked ids, never re-validated: right
    /// after any **successful** call into this module (an `Ignored` key or
    /// a no-op setter included — every one reconciles) it names a live
    /// `Tab` child of the bar. Between a caller damaging the bar from
    /// outside and the next successful call it can name a **dead** id or
    /// a tab whose node no longer declares `Focus`; a call that returns
    /// an error (a disabled bar's key, say) does not repair. `None` is
    /// unreachable for a bar built by [`insert_tab_bar`].
    #[must_use]
    pub fn selected_tab(&self) -> Option<WidgetId> {
        self.tabs.get(self.selected).copied()
    }

    /// The index of `tab` among this bar's tabs, if it is one of them —
    /// what a caller routing a pointer click needs before [`select_tab`].
    #[must_use]
    pub fn index_of(&self, tab: WidgetId) -> Option<usize> {
        self.tabs.iter().position(|&id| id == tab)
    }

    /// The pure key transition — this module's doc-comment table,
    /// exactly. Touches no tree.
    fn apply_key(&mut self, id: WidgetId, key: TabBarKey) -> Result<TabBarOutcome, WidgetError> {
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        let n = self.labels.len();
        if n == 0 {
            // Unreachable: `insert_tab_bar` refuses an empty list and
            // labels are fixed.
            return Ok(TabBarOutcome::Ignored);
        }
        // `selected < n` is this type's own invariant (see the field);
        // the wrap below is written so it cannot overflow either way.
        let current = self.selected;
        let target = match key {
            TabBarKey::Right => {
                if current + 1 >= n {
                    0
                } else {
                    current + 1
                }
            }
            TabBarKey::Left => {
                if current == 0 {
                    n - 1
                } else {
                    current - 1
                }
            }
            TabBarKey::Home => 0,
            TabBarKey::End => n - 1,
        };
        Ok(self.move_to(target))
    }

    /// Selects `index` as a user gesture. The range is checked before
    /// the disabled state, so a caller's out-of-range index is reported
    /// as the bug it is whatever state the bar is in.
    fn apply_select(&mut self, id: WidgetId, index: usize) -> Result<TabBarOutcome, WidgetError> {
        let len = self.labels.len();
        if index >= len {
            return Err(WidgetError::IndexOutOfRange { index, len });
        }
        if self.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        Ok(self.move_to(index))
    }

    fn move_to(&mut self, target: usize) -> TabBarOutcome {
        if target == self.selected {
            return TabBarOutcome::Ignored;
        }
        self.selected = target;
        TabBarOutcome::Selected(target)
    }

    /// Returns whether anything changed.
    fn apply_disabled(&mut self, disabled: bool) -> bool {
        let changed = self.disabled != disabled;
        self.disabled = disabled;
        changed
    }
}

/// One tab's own state — the payload of a [`WidgetKind::Tab`]. Written
/// only by this module; read it through the getters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabState {
    /// The tab's own accessible name.
    pub label: String,
    selected: bool,
    disabled: bool,
}

impl TabState {
    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Whether the owning bar is disabled (there is no per-tab disabled).
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

fn bar_node(state: &TabBarState) -> Node {
    let mut node = Node::new(Role::TabList);
    node.set_label(state.label.clone());
    if state.disabled {
        node.set_disabled();
    }
    node
}

/// Tab `index` of `len`. `position_in_set` (1-based) and `size_of_set`
/// are set explicitly: `accesskit_consumer` 0.38 computes neither from
/// the tree (`src/node.rs:626-634` only reads what the node declares), so
/// a screen reader's "tab 2 of 4" is only announced if the node says so.
fn tab_node(label: &str, index: usize, len: usize, selected: bool, disabled: bool) -> Node {
    let mut node = Node::new(Role::Tab);
    node.set_label(label.to_owned());
    node.set_position_in_set(index.saturating_add(1));
    node.set_size_of_set(len);
    // Present on every tab, true or false -- see this module's doc.
    node.set_selected(selected);
    if disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Click);
        if selected {
            // Roving focus: only the selected tab is focusable.
            node.add_action(Action::Focus);
        }
    }
    node
}

/// The bar: a `Row` of tabs `spacing.xs` apart, the full width of its
/// parent, as tall as its tabs. `align_self: FlexStart` keeps a `Row`
/// parent's default `Stretch` from inflating it vertically — the class
/// of bug `scrollbar::style`'s own doc comment records.
fn bar_style(scales: &Scales) -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        align_self: Some(AlignItems::FLEX_START),
        gap: Size {
            width: length(spacing(scales.spacing.xs)),
            height: zero(),
        },
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        ..Default::default()
    }
}

/// One tab: an equal share of the bar (`flex_grow: 1`, `flex_basis: 0`
/// — see "No glyphs" in this module's doc comment), at least one row
/// tall, padded `spacing.xs` vertically and `spacing.sm` horizontally.
fn tab_style(metrics: TabMetrics) -> Style {
    Style {
        flex_grow: 1.0,
        flex_basis: zero(),
        min_size: Size {
            width: auto(),
            height: length(metrics.min_height),
        },
        padding: LayoutRect {
            left: length(metrics.pad_h),
            right: length(metrics.pad_h),
            top: length(metrics.pad_v),
            bottom: length(metrics.pad_v),
        },
        ..Default::default()
    }
}

/// Adds a new, enabled tab bar as the last child of `parent`, with one
/// tab per entry of `tabs` and `selected` selected.
///
/// # Errors
///
/// Returns [`WidgetError::IndexOutOfRange`] if `tabs` is empty or
/// `selected` names no tab (a tab bar always has exactly one selected
/// tab), or [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when any of these happens.
pub fn insert_tab_bar(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    tabs: Vec<String>,
    selected: usize,
) -> Result<WidgetId, WidgetError> {
    if selected >= tabs.len() {
        return Err(WidgetError::IndexOutOfRange {
            index: selected,
            len: tabs.len(),
        });
    }
    let metrics = TabMetrics {
        pad_v: spacing(scales.spacing.xs),
        pad_h: spacing(scales.spacing.sm),
        min_height: row_height(scales),
    };
    let state = TabBarState::new(label.to_owned(), tabs, selected, metrics);
    let bar = tree.insert(
        parent,
        bar_style(scales),
        bar_node(&state),
        WidgetKind::TabBar(state),
    )?;
    if let Err(err) = rebuild_tabs(tree, bar) {
        // Unreachable (`bar` was just inserted), but never leave a
        // half-built bar behind. Best effort: the error being returned
        // is the real one.
        let _ = tree.remove(bar);
        return Err(err);
    }
    Ok(bar)
}

fn state(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Result<&TabBarState, WidgetError> {
    match tree.payload(id).ok_or(WidgetError::UnknownWidget(id))? {
        WidgetKind::TabBar(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(id)),
    }
}

/// A read-only view of `bar`'s own [`TabBarState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `bar` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a tab bar.
pub fn tab_bar_state(
    tree: &WidgetTree<WidgetKind>,
    bar: WidgetId,
) -> Result<&TabBarState, WidgetError> {
    state(tree, bar)
}

/// Feeds one key to `bar` (the bar's id, not a tab's — see this module's
/// own doc comment).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a tab bar, or [`WidgetError::WidgetDisabled`] if
/// it is disabled. Nothing changes when any of these happens.
pub fn handle_tab_bar_key(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    key: TabBarKey,
) -> Result<TabBarOutcome, WidgetError> {
    with_tab_bar_mut(tree, bar, |state| state.apply_key(bar, key))
}

/// Selects tab `index` — a user gesture (a routed pointer click or
/// accessibility `Click`), so it refuses a disabled bar. Selecting the
/// tab that is already selected is `Ignored` and costs no damage.
///
/// # Errors
///
/// As [`handle_tab_bar_key`], plus [`WidgetError::IndexOutOfRange`] if
/// `index` names no tab — checked before the disabled state.
pub fn select_tab(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    index: usize,
) -> Result<TabBarOutcome, WidgetError> {
    with_tab_bar_mut(tree, bar, |state| state.apply_select(bar, index))
}

/// Enables or disables `bar` and every tab in it. A request that matches
/// the current state changes nothing — no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `bar` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it isn't a tab bar.
pub fn set_tab_bar_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_tab_bar_mut(tree, bar, |state| {
        state.apply_disabled(disabled);
        Ok(())
    })
}

/// The one path every mutator here goes through: run the pure
/// transition on the payload and then — **whether or not it changed
/// anything** — reconcile the tabs with the state and bring the bar's own
/// node in line. A transition that returns an error changes nothing and
/// reconciles nothing.
///
/// Reconciling even after an `Ignored` key or a no-op setter is what
/// makes [`TabBarState::selected_tab`] live after every successful call,
/// damaged bar or not. It costs no damage on an intact bar: only a
/// widget whose node or payload disagrees with the state is rewritten and
/// dirtied, so a no-op still produces no damage at all. It is one O(n)
/// walk over the tabs per call (see "Size" in this module's doc comment).
///
/// Both `set_accessibility` *and* `mark_dirty` for every widget whose
/// node or pixels changed: `set_accessibility` only sets the per-widget
/// flag, `mark_dirty` is what reaches the damage region (the gap
/// `with_scrollbar_mut`'s own comment records). A selection change
/// touches exactly the two tabs involved; the bar itself only when its
/// node differs from [`bar_node`] of the current state — its `disabled`
/// state (and so its rule's alpha), its `label`, or a node overwritten
/// from outside.
fn with_tab_bar_mut<T>(
    tree: &mut WidgetTree<WidgetKind>,
    bar: WidgetId,
    f: impl FnOnce(&mut TabBarState) -> Result<T, WidgetError>,
) -> Result<T, WidgetError> {
    let result = {
        let kind = tree
            .payload_mut(bar)
            .ok_or(WidgetError::UnknownWidget(bar))?;
        let WidgetKind::TabBar(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(bar));
        };
        f(state)?
    };
    reconcile(tree, bar)?;
    let expected = bar_node(state(tree, bar)?);
    if tree.accessibility(bar) != Some(&expected) {
        tree.set_accessibility(bar, expected)?;
        tree.mark_dirty(bar)?;
    }
    Ok(result)
}

/// The `Tab` children of `bar`, in order.
fn tab_children(tree: &WidgetTree<WidgetKind>, bar: WidgetId) -> Vec<WidgetId> {
    tree.children(bar)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&child| matches!(tree.payload(child), Some(WidgetKind::Tab(_))))
        .collect()
}

/// Makes the tree match `bar`'s state. **Trusts nothing it did not just
/// check**: if the bar's `Tab` children are not exactly the tracked
/// tabs, one per label, they are all rebuilt (`rebuild_tabs`); otherwise
/// each tab's payload is compared field by field, and its **whole**
/// accessibility node (`accesskit::Node: PartialEq`) with the
/// [`tab_node`] the state says it should have — role, label, position,
/// `selected`, `disabled` and every action — and only a tab that
/// disagrees anywhere is rewritten and dirtied. So a selection change
/// touches exactly two tabs, a disabled toggle every tab, and a node
/// overwritten from outside with the right `selected` but the wrong
/// actions (a selected tab without `Focus`, an inactive one with it) is
/// still caught. The walk builds one expected `Node` per tab: O(n), and
/// no damage when everything agrees.
///
/// # Failure
///
/// Every error here needs an id that does not exist and is unreachable
/// today. A failure partway through a rebuild leaves untracked `Tab`
/// children behind, which the next reconcile finds and replaces (they
/// are not the tracked `tabs`), so it cannot leave a permanent duplicate.
fn reconcile(tree: &mut WidgetTree<WidgetKind>, bar: WidgetId) -> Result<(), WidgetError> {
    let current = state(tree, bar)?;
    let intact =
        current.tabs.len() == current.labels.len() && tab_children(tree, bar) == current.tabs;
    if !intact {
        return rebuild_tabs(tree, bar);
    }
    let current = state(tree, bar)?;
    let len = current.labels.len();
    let stale: Vec<(WidgetId, usize, Node)> = current
        .tabs
        .iter()
        .enumerate()
        .filter_map(|(index, &tab)| {
            let selected = index == current.selected;
            let label = current.labels.get(index).map_or("", String::as_str);
            let payload_ok = matches!(
                tree.payload(tab),
                Some(WidgetKind::Tab(t))
                    if t.selected == selected && t.disabled == current.disabled && t.label == label
            );
            let expected = tab_node(label, index, len, selected, current.disabled);
            let node_ok = tree.accessibility(tab) == Some(&expected);
            (!(payload_ok && node_ok)).then_some((tab, index, expected))
        })
        .collect();
    for (tab, index, node) in stale {
        let current = state(tree, bar)?;
        let selected = index == current.selected;
        let disabled = current.disabled;
        let label = current.labels.get(index).cloned().unwrap_or_default();
        if let Some(WidgetKind::Tab(t)) = tree.payload_mut(tab) {
            t.selected = selected;
            t.disabled = disabled;
            t.label = label;
        }
        tree.set_accessibility(tab, node)?;
        tree.mark_dirty(tab)?;
    }
    Ok(())
}

/// Removes every `Tab` child of `bar` and builds one fresh tab per
/// label, then records them. `WidgetTree::remove` marks each removed
/// tab's old bounds dirty, so the vacated area repaints. The fresh tabs
/// are appended — after any non-`Tab` child of the bar — and each
/// removal rescans the bar's child list, so this is O(n²); see "Size" in
/// this module's doc comment.
fn rebuild_tabs(tree: &mut WidgetTree<WidgetKind>, bar: WidgetId) -> Result<(), WidgetError> {
    for tab in tab_children(tree, bar) {
        tree.remove(tab)?;
    }
    let current = state(tree, bar)?;
    let labels = current.labels.clone();
    let selected = current.selected;
    let disabled = current.disabled;
    let style = tab_style(current.metrics);
    let len = labels.len();
    let mut tabs = Vec::with_capacity(len);
    for (index, label) in labels.into_iter().enumerate() {
        let on = index == selected;
        tabs.push(tree.insert(
            bar,
            style.clone(),
            tab_node(&label, index, len, on, disabled),
            WidgetKind::Tab(TabState {
                label,
                selected: on,
                disabled,
            }),
        )?);
    }
    let Some(WidgetKind::TabBar(state)) = tree.payload_mut(bar) else {
        return Err(WidgetError::WrongWidgetKind(bar));
    };
    state.tabs = tabs;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        TabBarKey, TabBarOutcome, TabBarState, TabMetrics, handle_tab_bar_key, insert_tab_bar,
        select_tab, set_tab_bar_disabled, tab_bar_state,
    };
    use crate::WidgetError;
    use crate::shortcut::NamedKey;
    use crate::tree::{WidgetId, WidgetTree};
    use crate::widgets::{WidgetKind, insert_button, new_tree, row_height, spacing, test_scales};
    use accesskit::{Action, Orientation, Role};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    const ID: WidgetId = accesskit::NodeId(7);
    const KEYS: [TabBarKey; 4] = [
        TabBarKey::Left,
        TabBarKey::Right,
        TabBarKey::Home,
        TabBarKey::End,
    ];

    fn labels(n: usize) -> Vec<String> {
        ["Layers", "Channels", "Paths", "History"]
            .into_iter()
            .take(n)
            .map(String::from)
            .collect()
    }

    fn pure(n: usize, selected: usize) -> TabBarState {
        let metrics = TabMetrics {
            pad_v: 8.0,
            pad_h: 12.0,
            min_height: 21.0,
        };
        TabBarState::new("Panels".to_owned(), labels(n), selected, metrics)
    }

    fn key(state: &mut TabBarState, k: TabBarKey) -> TabBarOutcome {
        match state.apply_key(ID, k) {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // ---- The pure transition table, every row of it -------------------

    #[test]
    fn right_and_left_move_one_step_and_wrap_at_both_ends() {
        // (from, key, expected) over a three-tab bar -- every cell.
        let table = [
            (0, TabBarKey::Right, TabBarOutcome::Selected(1)),
            (1, TabBarKey::Right, TabBarOutcome::Selected(2)),
            (2, TabBarKey::Right, TabBarOutcome::Selected(0)),
            (0, TabBarKey::Left, TabBarOutcome::Selected(2)),
            (1, TabBarKey::Left, TabBarOutcome::Selected(0)),
            (2, TabBarKey::Left, TabBarOutcome::Selected(1)),
            (0, TabBarKey::Home, TabBarOutcome::Ignored),
            (1, TabBarKey::Home, TabBarOutcome::Selected(0)),
            (2, TabBarKey::Home, TabBarOutcome::Selected(0)),
            (0, TabBarKey::End, TabBarOutcome::Selected(2)),
            (1, TabBarKey::End, TabBarOutcome::Selected(2)),
            (2, TabBarKey::End, TabBarOutcome::Ignored),
        ];
        for (from, k, expected) in table {
            let mut state = pure(3, from);
            assert_eq!(key(&mut state, k), expected, "{from} {k:?}");
            let now = match expected {
                TabBarOutcome::Selected(index) => index,
                TabBarOutcome::Ignored => from,
            };
            assert_eq!(state.selected(), now, "{from} {k:?}");
        }
    }

    #[test]
    fn a_single_tab_ignores_every_key() {
        for k in KEYS {
            let mut state = pure(1, 0);
            assert_eq!(key(&mut state, k), TabBarOutcome::Ignored, "{k:?}");
            assert_eq!(state.selected(), 0);
        }
    }

    #[test]
    fn a_disabled_bar_refuses_every_key_and_select_and_changes_nothing() {
        for k in KEYS {
            let mut state = pure(3, 1);
            assert!(state.apply_disabled(true));
            let before = state.clone();
            match state.apply_key(ID, k) {
                Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, ID),
                other => unreachable!("expected WidgetDisabled, got {other:?}"),
            }
            assert_eq!(state, before);
        }
        let mut state = pure(3, 1);
        state.apply_disabled(true);
        match state.apply_select(ID, 2) {
            Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, ID),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
        assert_eq!(state.selected(), 1);
    }

    /// The order is a decision, pinned: a caller's out-of-range index is
    /// reported as `IndexOutOfRange` even on a disabled bar.
    #[test]
    fn select_out_of_range_is_reported_before_disabled() {
        for disabled in [false, true] {
            let mut state = pure(3, 0);
            state.apply_disabled(disabled);
            match state.apply_select(ID, 3) {
                Err(WidgetError::IndexOutOfRange { index: 3, len: 3 }) => {}
                other => unreachable!("expected IndexOutOfRange, got {other:?}"),
            }
            assert_eq!(state.selected(), 0);
        }
    }

    #[test]
    fn selecting_the_current_tab_is_ignored_and_another_is_selected() {
        let mut state = pure(3, 1);
        assert!(matches!(
            state.apply_select(ID, 1),
            Ok(TabBarOutcome::Ignored)
        ));
        assert!(matches!(
            state.apply_select(ID, 2),
            Ok(TabBarOutcome::Selected(2))
        ));
        assert_eq!(state.selected(), 2);
    }

    #[test]
    fn apply_disabled_reports_whether_anything_changed() {
        let mut state = pure(3, 0);
        assert!(!state.apply_disabled(false));
        assert!(state.apply_disabled(true));
        assert!(!state.apply_disabled(true));
        assert!(state.apply_disabled(false));
    }

    #[test]
    fn named_keys_map_onto_the_four_tab_bar_keys_and_nothing_else() {
        let all = [
            NamedKey::Enter,
            NamedKey::Escape,
            NamedKey::Tab,
            NamedKey::Backspace,
            NamedKey::Delete,
            NamedKey::Space,
            NamedKey::ArrowUp,
            NamedKey::ArrowDown,
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Home,
            NamedKey::End,
            NamedKey::F1,
            NamedKey::F2,
            NamedKey::F3,
            NamedKey::F4,
            NamedKey::F5,
            NamedKey::F6,
            NamedKey::F7,
            NamedKey::F8,
            NamedKey::F9,
            NamedKey::F10,
            NamedKey::F11,
            NamedKey::F12,
        ];
        // Every variant, checked by an exhaustive match: adding a
        // `NamedKey` fails to compile here until it is added above.
        for k in all {
            match k {
                NamedKey::Enter
                | NamedKey::Escape
                | NamedKey::Tab
                | NamedKey::Backspace
                | NamedKey::Delete
                | NamedKey::Space
                | NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::Home
                | NamedKey::End
                | NamedKey::F1
                | NamedKey::F2
                | NamedKey::F3
                | NamedKey::F4
                | NamedKey::F5
                | NamedKey::F6
                | NamedKey::F7
                | NamedKey::F8
                | NamedKey::F9
                | NamedKey::F10
                | NamedKey::F11
                | NamedKey::F12 => {}
            }
        }
        assert_eq!(all.len(), 24);
        let mapped: Vec<(NamedKey, TabBarKey)> = all
            .into_iter()
            .filter_map(|k| TabBarKey::from_named_key(k).map(|t| (k, t)))
            .collect();
        assert_eq!(
            mapped,
            vec![
                (NamedKey::ArrowLeft, TabBarKey::Left),
                (NamedKey::ArrowRight, TabBarKey::Right),
                (NamedKey::Home, TabBarKey::Home),
                (NamedKey::End, TabBarKey::End),
            ]
        );
    }

    // ---- Tree wrappers -------------------------------------------------

    fn sized_root() -> Style {
        Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(256.0_f32),
                height: length(120.0_f32),
            },
            ..Default::default()
        }
    }

    fn inserted(selected: usize) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let bar = match insert_tab_bar(
            &mut tree,
            root,
            &test_scales(),
            "Panels",
            labels(3),
            selected,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (tree, bar)
    }

    fn press(tree: &mut WidgetTree<WidgetKind>, bar: WidgetId, k: TabBarKey) -> TabBarOutcome {
        match handle_tab_bar_key(tree, bar, k) {
            Ok(outcome) => outcome,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn snapshot(tree: &WidgetTree<WidgetKind>, bar: WidgetId) -> TabBarState {
        match tab_bar_state(tree, bar) {
            Ok(state) => state.clone(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// Every structural and per-tab claim, checked from scratch: the
    /// bar's `Tab` children are exactly the tracked tabs, one per label,
    /// each with the right payload *and* node, and exactly one selected.
    fn assert_structure_is_sound(tree: &WidgetTree<WidgetKind>, bar: WidgetId) {
        let state = snapshot(tree, bar);
        let tab_children: Vec<WidgetId> = tree
            .children(bar)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&c| matches!(tree.payload(c), Some(WidgetKind::Tab(_))))
            .collect();
        assert_eq!(tab_children, state.tabs());
        assert_eq!(state.tabs().len(), state.labels().len());
        assert_eq!(
            state.selected_tab(),
            state.tabs().get(state.selected()).copied()
        );
        let mut selected_count = 0;
        for (index, &tab) in state.tabs().iter().enumerate() {
            let on = index == state.selected();
            let Some(WidgetKind::Tab(payload)) = tree.payload(tab) else {
                unreachable!("a tab");
            };
            let Some(node) = tree.accessibility(tab) else {
                unreachable!("live");
            };
            assert_eq!(payload.is_selected(), on);
            assert_eq!(payload.is_disabled(), state.is_disabled());
            assert_eq!(
                Some(payload.label.as_str()),
                state.labels().get(index).map(String::as_str)
            );
            assert_eq!(node.role(), Role::Tab);
            assert_eq!(node.label(), state.labels().get(index).map(String::as_str));
            assert_eq!(node.is_selected(), Some(on), "present on every tab");
            assert_eq!(node.position_in_set(), Some(index + 1));
            assert_eq!(node.size_of_set(), Some(state.labels().len()));
            assert_eq!(node.is_disabled(), state.is_disabled());
            let enabled = !state.is_disabled();
            assert_eq!(node.supports_action(Action::Click), enabled);
            assert_eq!(node.supports_action(Action::Focus), enabled && on);
            if on {
                selected_count += 1;
            }
        }
        assert_eq!(selected_count, 1);
    }

    #[test]
    fn insert_tab_bar_builds_a_tab_list_with_one_real_tab_per_label() {
        let (tree, bar) = inserted(1);
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("just inserted");
        };
        assert_eq!(node.role(), Role::TabList);
        assert_eq!(node.label(), Some("Panels"));
        assert!(!node.is_disabled());
        assert!(!node.supports_action(Action::Focus));
        assert!(!node.supports_action(Action::Click));
        assert_eq!(node.orientation(), None, "left to the consumer's default");
        let state = snapshot(&tree, bar);
        assert_eq!(state.tabs().len(), 3);
        assert_eq!(state.selected(), 1);
        assert_structure_is_sound(&tree, bar);
    }

    #[test]
    fn insert_tab_bar_rejects_an_empty_list_a_bad_index_and_an_unknown_parent() {
        let (mut tree, root) = new_tree(sized_root());
        let before = tree.len();
        match insert_tab_bar(&mut tree, root, &test_scales(), "X", Vec::new(), 0) {
            Err(WidgetError::IndexOutOfRange { index: 0, len: 0 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        match insert_tab_bar(&mut tree, root, &test_scales(), "X", labels(3), 3) {
            Err(WidgetError::IndexOutOfRange { index: 3, len: 3 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        let bogus = accesskit::NodeId(999);
        match insert_tab_bar(&mut tree, bogus, &test_scales(), "X", labels(3), 0) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), before, "nothing added");
    }

    #[test]
    fn a_key_updates_both_tabs_payload_and_node() {
        let (mut tree, bar) = inserted(0);
        let tabs = snapshot(&tree, bar).tabs().to_vec();
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Right),
            TabBarOutcome::Selected(1)
        );
        assert_eq!(snapshot(&tree, bar).tabs(), tabs, "tabs are not rebuilt");
        assert_structure_is_sound(&tree, bar);
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Left),
            TabBarOutcome::Selected(0)
        );
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Left),
            TabBarOutcome::Selected(2),
            "wraps"
        );
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Home),
            TabBarOutcome::Selected(0)
        );
        assert_eq!(
            press(&mut tree, bar, TabBarKey::End),
            TabBarOutcome::Selected(2)
        );
        assert_structure_is_sound(&tree, bar);
        assert_eq!(snapshot(&tree, bar).selected_tab(), tabs.get(2).copied());
    }

    #[test]
    fn select_tab_selects_and_refuses_an_out_of_range_index() {
        let (mut tree, bar) = inserted(0);
        assert!(matches!(
            select_tab(&mut tree, bar, 2),
            Ok(TabBarOutcome::Selected(2))
        ));
        assert_structure_is_sound(&tree, bar);
        match select_tab(&mut tree, bar, 9) {
            Err(WidgetError::IndexOutOfRange { index: 9, len: 3 }) => {}
            other => unreachable!("expected IndexOutOfRange, got {other:?}"),
        }
        assert_eq!(snapshot(&tree, bar).selected(), 2);
    }

    #[test]
    fn a_disabled_bar_declares_no_actions_on_any_tab_and_refuses_input() {
        let (mut tree, bar) = inserted(1);
        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, true) {
            unreachable!("{err:?}");
        }
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("live");
        };
        assert!(node.is_disabled());
        assert_structure_is_sound(&tree, bar);
        for &tab in snapshot(&tree, bar).tabs() {
            let Some(node) = tree.accessibility(tab) else {
                unreachable!("live");
            };
            assert!(!node.supports_action(Action::Focus));
            assert!(!node.supports_action(Action::Click));
        }
        for k in [TabBarKey::Right, TabBarKey::Home] {
            match handle_tab_bar_key(&mut tree, bar, k) {
                Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, bar),
                other => unreachable!("expected WidgetDisabled, got {other:?}"),
            }
        }
        match select_tab(&mut tree, bar, 0) {
            Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, bar),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
        assert_eq!(snapshot(&tree, bar).selected(), 1);

        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, false) {
            unreachable!("{err:?}");
        }
        assert_structure_is_sound(&tree, bar);
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("live");
        };
        assert!(!node.is_disabled());
    }

    #[test]
    fn tab_bar_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, bar) = inserted(0);
        let Some(&tab) = snapshot(&tree, bar).tabs().first() else {
            unreachable!("three tabs");
        };
        for id in [tree.root(), tab] {
            let results = [
                handle_tab_bar_key(&mut tree, id, TabBarKey::Right).map(|_| ()),
                select_tab(&mut tree, id, 0).map(|_| ()),
                set_tab_bar_disabled(&mut tree, id, true),
                tab_bar_state(&tree, id).map(|_| ()),
            ];
            for result in results {
                match result {
                    Err(WidgetError::WrongWidgetKind(got)) => assert_eq!(got, id),
                    other => unreachable!("expected WrongWidgetKind, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn tab_bar_mutators_reject_an_unknown_widget() {
        let (mut tree, _bar) = inserted(0);
        let bogus = accesskit::NodeId(999);
        let results = [
            handle_tab_bar_key(&mut tree, bogus, TabBarKey::Right).map(|_| ()),
            select_tab(&mut tree, bogus, 0).map(|_| ()),
            set_tab_bar_disabled(&mut tree, bogus, true),
            tab_bar_state(&tree, bogus).map(|_| ()),
        ];
        for result in results {
            match result {
                Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
                other => unreachable!("expected UnknownWidget, got {other:?}"),
            }
        }
    }

    #[test]
    fn no_op_transitions_produce_no_damage() {
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        tree.take_damage();
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Home),
            TabBarOutcome::Ignored
        );
        assert!(matches!(
            select_tab(&mut tree, bar, 0),
            Ok(TabBarOutcome::Ignored)
        ));
        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), None);
        assert_eq!(tree.is_dirty(bar), Some(false));
        for &tab in snapshot(&tree, bar).tabs() {
            assert_eq!(tree.is_dirty(tab), Some(false));
        }
    }

    #[test]
    fn a_selection_change_dirties_exactly_the_two_tabs_involved() {
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        tree.take_damage();
        press(&mut tree, bar, TabBarKey::Right);
        let tabs = snapshot(&tree, bar).tabs().to_vec();
        let dirty: Vec<Option<bool>> = tabs.iter().map(|&t| tree.is_dirty(t)).collect();
        assert_eq!(dirty, vec![Some(true), Some(true), Some(false)]);
        assert_eq!(
            tree.is_dirty(bar),
            Some(false),
            "the bar's pixels did not change"
        );
        let (Some(a), Some(b)) = (
            tabs.first().and_then(|&t| tree.bounds(t)),
            tabs.get(1).and_then(|&t| tree.bounds(t)),
        ) else {
            unreachable!("laid out");
        };
        assert_eq!(tree.take_damage(), Some(a.union(&b)));
    }

    #[test]
    fn disabling_dirties_the_bar_and_every_tab_and_twice_is_a_no_op() {
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        tree.take_damage();
        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(bar), Some(true));
        for &tab in snapshot(&tree, bar).tabs() {
            assert_eq!(tree.is_dirty(tab), Some(true));
        }
        assert_eq!(tree.take_damage(), tree.bounds(bar));
        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), None, "disabling twice is a no-op");
    }

    #[test]
    fn a_tab_removed_externally_is_rebuilt_rather_than_trusted() {
        let (mut tree, bar) = inserted(0);
        let old = snapshot(&tree, bar).tabs().to_vec();
        let Some(&middle) = old.get(1) else {
            unreachable!("three tabs");
        };
        if let Err(err) = tree.remove(middle) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Right),
            TabBarOutcome::Selected(1)
        );
        let state = snapshot(&tree, bar);
        assert!(!state.tabs().contains(&middle), "no dead id survives");
        for &tab in state.tabs() {
            assert!(tree.contains(tab));
        }
        assert_structure_is_sound(&tree, bar);
    }

    #[test]
    fn a_stale_snapshot_written_back_never_leaves_duplicate_tabs() {
        let (mut tree, bar) = inserted(0);
        let stale = snapshot(&tree, bar);
        let Some(&first) = stale.tabs().first() else {
            unreachable!("three tabs");
        };
        if let Err(err) = tree.remove(first) {
            unreachable!("{err:?}");
        }
        press(&mut tree, bar, TabBarKey::Right); // repairs: fresh ids
        assert_ne!(snapshot(&tree, bar).tabs(), stale.tabs());
        let Some(kind) = tree.payload_mut(bar) else {
            unreachable!("exists");
        };
        *kind = WidgetKind::TabBar(stale);

        press(&mut tree, bar, TabBarKey::End);
        let tab_count = tree
            .children(bar)
            .unwrap_or_default()
            .iter()
            .filter(|&&c| matches!(tree.payload(c), Some(WidgetKind::Tab(_))))
            .count();
        assert_eq!(
            tab_count, 3,
            "exactly one tab per label after the write-back"
        );
        assert_structure_is_sound(&tree, bar);
        assert_eq!(snapshot(&tree, bar).selected(), 2);
    }

    #[test]
    fn a_tab_whose_payload_was_overwritten_is_repaired_on_the_next_change() {
        let (mut tree, bar) = inserted(0);
        let Some(&last) = snapshot(&tree, bar).tabs().get(2) else {
            unreachable!("three tabs");
        };
        if let Some(WidgetKind::Tab(tab)) = tree.payload_mut(last) {
            tab.selected = true;
        }
        press(&mut tree, bar, TabBarKey::Right);
        assert_structure_is_sound(&tree, bar);
    }

    /// The node is checked independently of the payload: a tab whose
    /// accessibility node was overwritten from outside (payload intact)
    /// is rewritten on the next change too.
    #[test]
    fn a_tab_whose_node_was_overwritten_is_repaired_on_the_next_change() {
        let (mut tree, bar) = inserted(0);
        let Some(&last) = snapshot(&tree, bar).tabs().get(2) else {
            unreachable!("three tabs");
        };
        if let Err(err) = tree.set_accessibility(last, accesskit::Node::new(Role::Tab)) {
            unreachable!("{err:?}");
        }
        press(&mut tree, bar, TabBarKey::Right);
        assert_structure_is_sound(&tree, bar);
    }

    /// Overwrites `tab`'s node from outside with `node`.
    fn tamper(tree: &mut WidgetTree<WidgetKind>, tab: WidgetId, node: accesskit::Node) {
        if let Err(err) = tree.set_accessibility(tab, node) {
            unreachable!("{err:?}");
        }
    }

    /// The whole node is compared, not just `selected`/`disabled`: an
    /// inactive tab overwritten with the right `selected(false)` and
    /// `disabled` but an extra `Focus` (so it would enter the tab order)
    /// or the wrong label is repaired on the next change, even though
    /// that change does not otherwise touch it.
    #[test]
    fn an_inactive_tab_node_with_the_right_flags_but_wrong_actions_or_label_is_repaired() {
        for wrong_label in [false, true] {
            let (mut tree, bar) = inserted(0);
            let Some(&last) = snapshot(&tree, bar).tabs().get(2) else {
                unreachable!("three tabs");
            };
            let mut node = accesskit::Node::new(Role::Tab);
            node.set_label(if wrong_label { "Wrong" } else { "Paths" });
            node.set_position_in_set(3);
            node.set_size_of_set(3);
            node.set_selected(false);
            node.add_action(Action::Click);
            if !wrong_label {
                node.add_action(Action::Focus);
            }
            tamper(&mut tree, last, node);
            press(&mut tree, bar, TabBarKey::Right); // 0 -> 1, never tab 2
            assert_structure_is_sound(&tree, bar);
        }
    }

    /// The selected tab overwritten with `selected(true)` but no `Focus`
    /// (so nothing in the bar is focusable) is repaired by the very next
    /// call — here an `Ignored` key, which still reconciles.
    #[test]
    fn a_selected_tab_node_without_focus_is_repaired_even_by_an_ignored_key() {
        let (mut tree, bar) = inserted(0);
        let Some(first) = snapshot(&tree, bar).selected_tab() else {
            unreachable!("selected");
        };
        let mut node = accesskit::Node::new(Role::Tab);
        node.set_label("Layers");
        node.set_position_in_set(1);
        node.set_size_of_set(3);
        node.set_selected(true);
        node.add_action(Action::Click);
        tamper(&mut tree, first, node);
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Home),
            TabBarOutcome::Ignored
        );
        assert_structure_is_sound(&tree, bar);
        let mut focus = crate::FocusManager::new();
        if let Err(err) = focus.focus(&mut tree, first) {
            unreachable!("focusable again: {err:?}");
        }
    }

    /// The bar's own node is compared as a whole too: a tampered role,
    /// and a `pub label` edited through the payload, both reach the node
    /// on the next call — and dirty the bar.
    #[test]
    fn the_bar_node_is_repaired_and_a_label_edit_reaches_it() {
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        tree.take_damage();
        let mut wrong = accesskit::Node::new(Role::Group);
        wrong.set_label("Panels");
        tamper(&mut tree, bar, wrong);
        press(&mut tree, bar, TabBarKey::Right);
        assert_eq!(
            tree.accessibility(bar).map(accesskit::Node::role),
            Some(Role::TabList)
        );
        assert_eq!(tree.is_dirty(bar), Some(true));

        if let Some(WidgetKind::TabBar(state)) = tree.payload_mut(bar) {
            state.label = "Docks".to_owned();
        }
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Right),
            TabBarOutcome::Selected(2)
        );
        let Some(node) = tree.accessibility(bar) else {
            unreachable!("live");
        };
        assert_eq!(node.role(), Role::TabList);
        assert_eq!(node.label(), Some("Docks"));
    }

    /// After the selected tab is removed from outside, an `Ignored` key
    /// (`Home` at index 0) still repairs: `selected_tab()` names a live,
    /// focusable tab rather than the dead id.
    #[test]
    fn an_ignored_key_after_external_damage_still_leaves_a_live_selected_tab() {
        let (mut tree, bar) = inserted(0);
        let Some(dead) = snapshot(&tree, bar).selected_tab() else {
            unreachable!("selected");
        };
        if let Err(err) = tree.remove(dead) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            press(&mut tree, bar, TabBarKey::Home),
            TabBarOutcome::Ignored
        );
        let Some(live) = snapshot(&tree, bar).selected_tab() else {
            unreachable!("selected");
        };
        assert_ne!(live, dead, "rebuilt, not trusted");
        assert!(tree.contains(live));
        assert_structure_is_sound(&tree, bar);
        let mut focus = crate::FocusManager::new();
        if let Err(err) = focus.focus(&mut tree, live) {
            unreachable!("the repaired selected tab is focusable: {err:?}");
        }
    }

    /// Labels are the caller's responsibility (this module's doc): an
    /// empty label is accepted and exposed as an empty name, duplicates
    /// as distinct tabs. Pinned so rejecting them later is deliberate.
    #[test]
    fn empty_and_duplicate_labels_are_accepted_as_the_callers_responsibility() {
        let (mut tree, root) = new_tree(sized_root());
        let tabs = vec![String::new(), "Same".to_owned(), "Same".to_owned()];
        let bar = match insert_tab_bar(&mut tree, root, &test_scales(), "", tabs, 0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_structure_is_sound(&tree, bar);
        let state = snapshot(&tree, bar);
        let names: Vec<Option<&str>> = state
            .tabs()
            .iter()
            .map(|&t| tree.accessibility(t).and_then(|n| n.label()))
            .collect();
        assert_eq!(names, vec![Some(""), Some("Same"), Some("Same")]);
        assert_eq!(tree.accessibility(bar).and_then(|n| n.label()), Some(""));
    }

    /// The disconnected-tree/stale-focus crash class
    /// `WidgetTree::accessibility_update`'s own doc comment records,
    /// plus the consumer-side reading of every claim this module makes:
    /// initial, after a move, and disabled.
    #[test]
    fn accesskit_consumer_reads_the_tab_list_as_documented() {
        let (mut tree, bar) = inserted(0);
        let check = |tree: &WidgetTree<WidgetKind>, selected: usize, disabled: bool| {
            let focus = snapshot(tree, bar).selected_tab().unwrap_or(bar);
            let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(focus), true);
            let root = consumer.state().root();
            let Some(list) = root.children().find(|n| n.role() == Role::TabList) else {
                unreachable!("the bar is the root's child");
            };
            assert_eq!(list.orientation(), Some(Orientation::Horizontal));
            assert_eq!(list.is_disabled(), disabled);
            let filter = accesskit_consumer::common_filter;
            let tabs: Vec<_> = list.children().collect();
            assert_eq!(tabs.len(), 3);
            let mut selected_count = 0;
            for (index, tab) in tabs.iter().enumerate() {
                let on = index == selected;
                assert_eq!(tab.role(), Role::Tab);
                assert_eq!(tab.position_in_set(), Some(index + 1), "1-based");
                assert_eq!(tab.size_of_set(), Some(3));
                assert_eq!(tab.is_selected(), Some(on));
                assert_eq!(tab.is_selectable(), !disabled);
                assert_eq!(tab.supports_action(Action::Focus, &filter), !disabled && on);
                assert_eq!(tab.is_clickable(&filter), !disabled);
                assert!(!tab.is_invocable(&filter), "selection is not invocation");
                assert_eq!(tab.controls().count(), 0);
                if tab.is_selected() == Some(true) {
                    selected_count += 1;
                }
            }
            assert_eq!(selected_count, 1);
        };
        check(&tree, 0, false);
        press(&mut tree, bar, TabBarKey::Right);
        check(&tree, 1, false);
        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, true) {
            unreachable!("{err:?}");
        }
        check(&tree, 1, true);
    }

    #[test]
    fn the_tab_order_visits_only_the_selected_tab() {
        let (mut tree, bar) = inserted(0);
        let root = tree.root();
        let button = match insert_button(&mut tree, root, &test_scales(), "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let tabs = snapshot(&tree, bar).tabs().to_vec();
        let (Some(&first), Some(&second)) = (tabs.first(), tabs.get(1)) else {
            unreachable!("three tabs");
        };
        let mut focus = crate::FocusManager::new();
        let cycle = |focus: &mut crate::FocusManager, tree: &mut WidgetTree<WidgetKind>| {
            (0..4)
                .filter_map(|_| focus.focus_next(tree))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            cycle(&mut focus, &mut tree),
            vec![first, button, first, button]
        );

        press(&mut tree, bar, TabBarKey::Right);
        match focus.focus(&mut tree, first) {
            Err(WidgetError::NotFocusable(id)) => assert_eq!(id, first),
            other => unreachable!("the old tab is no longer focusable: {other:?}"),
        }
        let mut focus = crate::FocusManager::new();
        assert_eq!(
            cycle(&mut focus, &mut tree),
            vec![second, button, second, button]
        );

        if let Err(err) = set_tab_bar_disabled(&mut tree, bar, true) {
            unreachable!("{err:?}");
        }
        let mut focus = crate::FocusManager::new();
        assert_eq!(
            cycle(&mut focus, &mut tree),
            vec![button, button, button, button]
        );
    }

    /// A pointer press on an inactive tab focuses nothing: the tab has no
    /// `Focus` and neither does the bar above it — the caller-routing gap
    /// this module's own doc comment discloses.
    #[test]
    fn a_pointer_press_on_an_inactive_tab_does_not_focus_it() {
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        let Some(inactive) = snapshot(&tree, bar)
            .tabs()
            .get(2)
            .and_then(|&t| tree.bounds(t))
        else {
            unreachable!("laid out");
        };
        let mut focus = crate::FocusManager::new();
        #[allow(clippy::cast_precision_loss)]
        let hit = focus.focus_at(
            &mut tree,
            (inactive.x + i64::from(inactive.width / 2)) as f64,
            (inactive.y + i64::from(inactive.height / 2)) as f64,
        );
        assert_eq!(hit, None);
        assert_eq!(focus.focused(), None);
    }

    #[test]
    fn tabs_share_the_bar_equally_with_a_token_gap_and_at_least_one_row_tall() {
        let scales = test_scales();
        let (mut tree, bar) = inserted(0);
        tree.compute_layout(256.0, 120.0);
        let Some(bar_bounds) = tree.bounds(bar) else {
            unreachable!("laid out");
        };
        assert_eq!(bar_bounds.width, 256);
        let rects: Vec<_> = snapshot(&tree, bar)
            .tabs()
            .iter()
            .filter_map(|&t| tree.bounds(t))
            .collect();
        assert_eq!(rects.len(), 3);
        let (Some(first), Some(last)) = (rects.first(), rects.last()) else {
            unreachable!("three tabs");
        };
        // The tabs fill the bar edge to edge: without `flex_grow` they
        // would collapse to their padding at the left.
        assert_eq!(first.x, bar_bounds.x, "first tab starts at the bar's left");
        assert_eq!(
            last.right(),
            bar_bounds.right(),
            "last tab ends at its right"
        );
        let gap = spacing(scales.spacing.xs);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let gap_px = gap as i64;
        for pair in rects.windows(2) {
            let [a, b] = pair else {
                unreachable!("windows(2)");
            };
            assert_eq!(a.width, b.width, "equal share");
            assert_eq!(b.x - a.right(), gap_px, "spacing.xs apart");
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let min_height = row_height(&scales) as u32;
        for rect in &rects {
            assert!(rect.height >= min_height, "{rect:?}");
            assert_eq!(rect.y, bar_bounds.y);
            assert_eq!(rect.bottom(), bar_bounds.bottom());
        }
    }

    #[test]
    fn a_tab_bar_in_a_row_parent_is_not_stretched_vertically() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(Style {
            flex_direction: FlexDirection::Row,
            size: Size {
                width: length(240.0_f32),
                height: length(200.0_f32),
            },
            ..Default::default()
        });
        let bar = match insert_tab_bar(&mut tree, root, &scales, "Panels", labels(3), 0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(240.0, 200.0);
        let Some(bounds) = tree.bounds(bar) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = row_height(&scales) as u32;
        assert_eq!(bounds.height, row, "one row tall, not the parent's 200");
    }
}
