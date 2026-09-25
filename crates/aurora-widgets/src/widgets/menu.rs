//! A popup menu: a vertical list of action items, optionally broken up
//! by separators, driven from the keyboard, that closes itself when an
//! item is activated or the menu is cancelled.
//!
//! **Shape: a pure state machine plus a structural reconcile**, the
//! split `dropdown.rs` and `tab_bar.rs` already use. [`MenuState`] owns
//! the items and the highlighted index; every key is a transition on
//! that state alone ([`MenuState`]'s own `transition`, unit-tested with no
//! tree at all). The tree is then brought in line with it: the menu's
//! own [`WidgetKind::Menu`] node holds one child per item — a
//! [`WidgetKind::ListRow`] with a `Role::MenuItem` node for an action,
//! a [`WidgetKind::MenuSeparator`] with a `Role::Splitter` node for a
//! separator — in the caller's own order.
//!
//! **"Open" means "the node exists".** [`open_menu`] inserts the whole
//! subtree; activating an item, cancelling, or [`close_menu`] removes
//! it. There is no `open` flag to disagree with the tree, and no closed
//! menu to reopen: a caller opens a fresh one each time.
//!
//! # Keys
//!
//! [`MenuKey`] is this module's own vocabulary, bridged from
//! `shortcut::NamedKey` by [`MenuKey::from_named_key`] — the same
//! disclosed reason `dropdown.rs` gives for its own. "Enabled" below
//! means an [`MenuItemKind::Action`] item whose `enabled` is `true`;
//! separators and disabled items are **always skipped**.
//!
//! | key | highlight `Some(i)` | highlight `None` (every action disabled) |
//! |---|---|---|
//! | `Down` | next enabled after `i`, **wrapping** → `Moved(j)`; `j == i` → `Ignored` | `Ignored` |
//! | `Up` | previous enabled before `i`, wrapping; same | `Ignored` |
//! | `Home` / `End` | first / last enabled → `Moved`; already there → `Ignored` | `Ignored` |
//! | `Enter` / `Space` | `Activated(i)`, subtree removed (see below) | `Ignored`, stays open |
//! | `Escape` | `Cancelled`, subtree removed | `Cancelled`, subtree removed |
//!
//! Every index a [`MenuOutcome`] reports is an index into the caller's
//! own `items`, separators included, so a caller maps it straight back
//! to the command it built the item from.
//!
//! **Every key reconciles first.** Before a key is acted on, the tree is
//! brought in line with the state's current highlight (the precedent
//! `tab_bar.rs` sets), so a removed item or a stale [`MenuState`] written
//! back through `WidgetTree::payload_mut` is repaired even by a key that
//! is then `Ignored`. If that repair changed anything, `Enter`/`Space`
//! returns `Ignored` rather than activating an item that was not the one
//! painted or announced; and only an enabled action is ever `Activated`.
//! The repair is partial in one case: on an intact menu the O(n) pass
//! compares payloads, and fully checks (node included) only rows whose
//! payload disagrees plus the highlighted row, so a written-back state
//! differing only in a *non-highlighted* row's label leaves that row's
//! accessible label stale — never what gets activated.
//! On an intact menu an `Ignored` key changes nothing and costs no
//! damage.
//!
//! The initial highlight is the first enabled item, or `None` when
//! every action is disabled (a menu with no action at all is rejected at
//! open).
//!
//! **A disclosed divergence from the WAI-ARIA APG *menu* pattern**: the
//! APG keeps disabled menu items focusable ("disabled menu items are
//! focusable but cannot be activated"), so a screen-reader user hears
//! that they exist. This skips them, the `Dropdown`-like behaviour most
//! desktop toolkits ship. Which one Aurora wants is a design-owner
//! question (PRD FR-027 *Ownership*), raised here rather than decided.
//!
//! # Focus and the accessibility vocabulary
//!
//! The menu node itself declares `Action::Focus` and is what holds
//! keyboard focus; the highlighted item is its `active_descendant`
//! (cleared when nothing is highlighted) — the same model `Dropdown`'s
//! control uses. `accesskit_consumer` 0.38 resolves focus as
//! `focused.active_descendant().unwrap_or(focused)` for any role
//! (`tree.rs:544-548`), so a screen reader follows the highlight with
//! no item ever holding real focus; items declare no `Action::Focus`
//! and never enter the Tab order.
//!
//! - **Menu**: `Role::Menu`, the caller's label. No `expanded` (a menu
//!   that exists is open) and no `controls` (nothing here names a
//!   dangling id). `accesskit_macos` maps it to `NSAccessibilityMenuRole`,
//!   `accesskit_windows` to UIA `Menu`, `accesskit_atspi_common` to
//!   AT-SPI `Menu`.
//! - **Item**: `Role::MenuItem`, its label, and `position_in_set` /
//!   `size_of_set` counting **action items only** (a separator is not
//!   an item a screen reader counts) — declared explicitly because the
//!   consumer computes neither. An enabled item declares `Action::Click`
//!   and nothing else; a disabled one is `set_disabled` with no actions.
//!   **Never `selected`**: ARIA menus do not use `aria-selected`, and
//!   `accesskit_windows` only exposes UIA `SelectionItem` for list, tab
//!   and tree roles.
//! - **Separator**: `Role::Splitter`, no label, no actions — AT-SPI
//!   `Separator` and UIA `Separator`. **A disclosed mismatch**: macOS
//!   maps `Splitter` to `NSAccessibilitySplitterRole`, not a menu
//!   separator; `accesskit` 0.24 has no dedicated separator role.
//!
//! No adapter in the pinned `accesskit` set raises a menu-opened event
//! (no `MenuOpened`/`MenuModeStart`/`MenuPopupStart` anywhere), so a
//! screen reader learns a menu opened only from the focus change the
//! caller makes.
//!
//! **The caller owns focus**: after [`open_menu`], call
//! `FocusManager::focus(tree, menu)`; after an `Activated`/`Cancelled`
//! outcome or [`close_menu`], restore focus to whatever opened it.
//! `FocusManager` can go on holding a removed id until then (the same
//! hazard `tab_bar.rs` records). `Tab` is not handled — the APG closes a
//! menu on `Tab`; a caller that wants that calls [`close_menu`].
//!
//! # Layout
//!
//! The menu is `Position::Absolute` at the caller's `at`, relative to
//! `parent`'s padding box (taffy's absolute-inset reference box), so it
//! never moves `parent`'s other children. Its width is the caller's
//! `width` — **caller-supplied**, because this crate measures no text
//! and deriving a minimum width would mean inventing a token. Its height
//! is its children's: one `row_height` per action, one `spacing.xxs`
//! per separator (a provisional choice — see the design-owner questions
//! below).
//!
//! **The menu owns all its children** — do not insert foreign children
//! under it. A caller-inserted `ListRow`/`MenuSeparator` is taken for a
//! corrupted item and removed by the next key's rebuild, and any other
//! foreign child is left in place but laid out in the column. And
//! **`parent` must be a stable container**: a menu parented inside
//! another widget's reconciled subtree (a `Dropdown`'s option list, say)
//! is destroyed the next time that widget reconciles its own children.
//!
//! # Paint
//!
//! `paint::paint_menu` draws a `radius.sm` `surface.raised` rounded rect
//! ("Elevation 1: dropdowns, popovers, context menus",
//! `design/tokens/vocabulary.md`) with an unconditional `border.default`
//! stroke and the High Contrast outline — the shared shape
//! `paint::bordered_surface` draws. Items paint through the existing
//! `ListRow` arm (an `accent.primary` highlight, dimmed by
//! `state.disabled_opacity` on a disabled row). A separator paints one
//! `border.default` band at most 1 px tall, vertically centred and the
//! separator's full width; `border.default` is decorative and not gated
//! by `design/check_contrast.py`. A highlighted row is full width and
//! so covers the inner half of the menu's border beside it — the same
//! as `Dropdown`'s list.
//!
//! # What this deliberately does not do
//!
//! - **A popover, clamped to the window and nothing smarter**
//!   (0.127.0). The menu is a [`PaintLayer::Popover`](crate::PaintLayer)
//!   root: it paints after every base-layer widget, no clipping
//!   ancestor of `parent` clips it, and both `WidgetTree::hit_test` and
//!   `crate::hit_test` reach every item wherever it lies — including
//!   the part overflowing `parent`'s own bounds — clamped only to the
//!   tree root's bounds (the window). It is never flipped or moved to
//!   stay on screen.
//! - **No pointer routing.** Hit-testing reaches the items, but nothing
//!   routes a pointer press or hover to this module. There is no hover
//!   highlight, so a pointer-opened menu on macOS (where no item is
//!   highlighted initially) is not modelled either: the keyboard is the
//!   only driver.
//! - **No submenus**, no check/radio items, no glyphs (labels, shortcut
//!   hints, arrows), no viewport flip near a window edge (only the
//!   window clamp above), and
//!   no routing of `accesskit::ActionRequest`s (a screen reader's
//!   `Click` on an item reaches no code here yet).
//!
//! **Design-owner questions** (PRD FR-027 *Ownership*): there is no
//! menu mockup in `design/gallery/index.html`, so every token here is
//! provisional; submenus; disabled-item focusability (above); the
//! separator's token and height; and how a menu's width should be
//! chosen once text measurement exists.

use std::collections::HashSet;

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length, percent};
use taffy::{FlexDirection, Position, Rect as LayoutRect, Size, Style};

use super::list_row::ListRowState;
use super::{WidgetKind, row_height, spacing};
use crate::error::WidgetError;
use crate::shortcut::NamedKey;
use crate::tree::{PaintLayer, WidgetId, WidgetTree};

/// What one entry of a menu is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuItemKind {
    /// A command the user can activate.
    Action,
    /// A visual divider between groups of actions. Never highlighted,
    /// never activated, and not counted in any item's `i of n`.
    Separator,
}

/// One entry the caller hands [`open_menu`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// The item's accessible label. Empty is accepted (the same as a
    /// `Dropdown` option), and ignored for a separator.
    pub label: String,
    /// Whether an action can be highlighted and activated. Ignored for a
    /// separator, which never can.
    pub enabled: bool,
    pub kind: MenuItemKind,
}

impl MenuItem {
    /// An enabled action labelled `label`.
    #[must_use]
    pub fn action(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            enabled: true,
            kind: MenuItemKind::Action,
        }
    }

    /// A separator.
    #[must_use]
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            enabled: false,
            kind: MenuItemKind::Separator,
        }
    }

    /// Whether this is an action that can be highlighted and activated.
    fn is_enabled_action(&self) -> bool {
        self.kind == MenuItemKind::Action && self.enabled
    }
}

/// The keys a menu responds to — see this module's own doc comment for
/// the transition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuKey {
    Up,
    Down,
    Home,
    End,
    Enter,
    Space,
    Escape,
}

impl MenuKey {
    /// The menu key `key` stands for, if any — `None` for every
    /// `NamedKey` a menu does not handle (`Tab` included, see this
    /// module's doc comment).
    #[must_use]
    pub fn from_named_key(key: NamedKey) -> Option<Self> {
        match key {
            NamedKey::ArrowUp => Some(Self::Up),
            NamedKey::ArrowDown => Some(Self::Down),
            NamedKey::Home => Some(Self::Home),
            NamedKey::End => Some(Self::End),
            NamedKey::Enter => Some(Self::Enter),
            NamedKey::Space => Some(Self::Space),
            NamedKey::Escape => Some(Self::Escape),
            _ => None,
        }
    }
}

/// What a key did. Every index is into the caller's own `items`,
/// separators included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOutcome {
    /// Nothing changed.
    Ignored,
    /// The highlight moved to this item; the menu stays open.
    Moved(usize),
    /// This item was activated; the menu's subtree has been removed.
    Activated(usize),
    /// The menu was dismissed without activating anything; its subtree
    /// has been removed.
    Cancelled,
}

/// A menu's own state — the payload of its [`WidgetKind::Menu`] node.
///
/// Every field is private behind a getter, because it is kept in
/// lockstep with real tree structure. `PartialEq` but not `Eq`: it
/// carries two resolved `f32` heights.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    label: String,
    items: Vec<MenuItem>,
    highlighted: Option<usize>,
    item_ids: Vec<WidgetId>,
    row_height: f32,
    separator_height: f32,
}

impl MenuState {
    fn new(label: String, items: Vec<MenuItem>, row_height: f32, separator_height: f32) -> Self {
        let highlighted = items.iter().position(MenuItem::is_enabled_action);
        Self {
            label,
            items,
            highlighted,
            item_ids: Vec::new(),
            row_height,
            separator_height,
        }
    }

    /// The menu's accessible label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The items, exactly as the caller supplied them.
    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// The highlighted item's index into [`Self::items`], or `None` when
    /// every action is disabled.
    #[must_use]
    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// The tree id of each item's own child node, parallel to
    /// [`Self::items`].
    #[must_use]
    pub fn item_ids(&self) -> &[WidgetId] {
        &self.item_ids
    }

    /// The highlighted item's own tree id, if any.
    fn highlighted_id(&self) -> Option<WidgetId> {
        self.highlighted
            .and_then(|index| self.item_ids.get(index).copied())
    }

    fn is_enabled(&self, index: usize) -> bool {
        self.items
            .get(index)
            .is_some_and(MenuItem::is_enabled_action)
    }

    /// The first enabled item after `from`, stepping forwards (`forward`)
    /// or backwards and wrapping at either end, skipping `from` itself —
    /// so it returns `from` when that is the only enabled item. At most
    /// `len` steps, each an overflow-free `+ 1`/`- 1` with an explicit
    /// wrap, so no `len` however large can make it overflow or land on
    /// the wrong index. A `from` at or past `len` (never produced by this
    /// module) starts from the matching end.
    fn next_enabled(&self, from: usize, forward: bool) -> Option<usize> {
        let len = self.items.len();
        let last = len.checked_sub(1)?;
        let mut index = from;
        for _ in 0..len {
            index = if forward {
                if index >= last { 0 } else { index + 1 }
            } else if index == 0 || index > last {
                last
            } else {
                index - 1
            };
            if self.is_enabled(index) {
                return Some(index);
            }
        }
        None
    }

    /// The pure transition table (see this module's doc comment). Does
    /// not mutate: the caller commits a `Moved` only once the tree has
    /// been brought in line with it.
    fn transition(&self, key: MenuKey) -> MenuOutcome {
        if key == MenuKey::Escape {
            return MenuOutcome::Cancelled;
        }
        let Some(current) = self.highlighted else {
            return MenuOutcome::Ignored;
        };
        let target = match key {
            // Only an enabled action is ever activated, even if a
            // highlight written back through `WidgetTree::payload_mut`
            // names a separator, a disabled item or no item at all.
            MenuKey::Enter | MenuKey::Space if self.is_enabled(current) => {
                return MenuOutcome::Activated(current);
            }
            MenuKey::Enter | MenuKey::Space => return MenuOutcome::Ignored,
            MenuKey::Escape => return MenuOutcome::Cancelled,
            MenuKey::Down => self.next_enabled(current, true),
            MenuKey::Up => self.next_enabled(current, false),
            MenuKey::Home => self.items.iter().position(MenuItem::is_enabled_action),
            MenuKey::End => self.items.iter().rposition(MenuItem::is_enabled_action),
        };
        match target {
            Some(index) if index != current => MenuOutcome::Moved(index),
            _ => MenuOutcome::Ignored,
        }
    }
}

fn menu_node(state: &MenuState) -> Node {
    let mut node = Node::new(Role::Menu);
    node.set_label(state.label.clone());
    node.add_action(Action::Focus);
    if let Some(item) = state.highlighted_id() {
        node.set_active_descendant(item);
    }
    node
}

/// One action item's node. `position`/`size` are 1-based among action
/// items only; `accesskit_consumer` 0.38 computes neither.
fn item_node(label: &str, position: usize, size: usize, enabled: bool) -> Node {
    let mut node = Node::new(Role::MenuItem);
    node.set_label(label.to_owned());
    node.set_position_in_set(position);
    node.set_size_of_set(size);
    if enabled {
        node.add_action(Action::Click);
    } else {
        node.set_disabled();
    }
    node
}

fn separator_node() -> Node {
    Node::new(Role::Splitter)
}

/// The payload an item's child should have, given whether it is
/// highlighted. Cheap — no allocation — so a whole-menu check can afford
/// it for every item.
fn expected_payload(item: &MenuItem, highlighted: bool) -> WidgetKind {
    match item.kind {
        MenuItemKind::Action => WidgetKind::ListRow(ListRowState {
            selected: highlighted,
            disabled: !item.enabled,
        }),
        MenuItemKind::Separator => WidgetKind::MenuSeparator,
    }
}

/// The node and payload item `index` of `items` should have, given
/// `highlighted`. `position` is the item's 1-based position among
/// action items and `size` their count.
fn expected_child(
    item: &MenuItem,
    position: usize,
    size: usize,
    highlighted: bool,
) -> (Node, WidgetKind) {
    let node = match item.kind {
        MenuItemKind::Action => item_node(&item.label, position, size, item.enabled),
        MenuItemKind::Separator => separator_node(),
    };
    (node, expected_payload(item, highlighted))
}

/// Every item's expected node and payload, in order.
fn expected_children(items: &[MenuItem], highlighted: Option<usize>) -> Vec<(Node, WidgetKind)> {
    let size = items
        .iter()
        .filter(|item| item.kind == MenuItemKind::Action)
        .count();
    let mut position = 0_usize;
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            if item.kind == MenuItemKind::Action {
                position = position.saturating_add(1);
            }
            expected_child(item, position, size, highlighted == Some(index))
        })
        .collect()
}

/// The menu's own layout: out of flow at `at` inside `parent`'s padding
/// box, `width` wide, as tall as its children stacked.
fn menu_style(at: (f32, f32), width: f32) -> Style {
    Style {
        position: Position::Absolute,
        flex_direction: FlexDirection::Column,
        inset: LayoutRect {
            left: length(at.0),
            right: auto(),
            top: length(at.1),
            bottom: auto(),
        },
        size: Size {
            width: length(width),
            height: auto(),
        },
        ..Default::default()
    }
}

/// One child: exactly `height` tall, never shrunk, full width — the
/// same shape as `dropdown::row_style`.
fn child_style(height: f32) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: percent(1.0_f32),
            height: length(height),
        },
        ..Default::default()
    }
}

/// Opens a menu as the last child of `parent`, offering `items` in
/// order, at `at` (logical px from `parent`'s padding-box origin) and
/// `width` wide. The first enabled action is highlighted. The caller
/// focuses the returned id itself (see this module's doc comment).
///
/// # Errors
///
/// Nothing is added when any of these happens:
///
/// - [`WidgetError::IndexOutOfRange`] (`index: 0`, `len: 0`) if `items`
///   holds no [`MenuItemKind::Action`] at all — empty or separators
///   only — the same error `insert_tab_bar` returns for an empty list.
/// - [`WidgetError::InvalidRange`] if `width` is not finite and above
///   zero (`min: 0.0, max: width`), or either coordinate of `at` is not
///   finite (`min: at.0, max: at.1`). **Reused rather than a new
///   variant**, and a loose fit: the reported `min`/`max` carry no range
///   meaning (for `at` they are simply its two coordinates), and for
///   `width == 0.0` the range satisfies `min <= max`, so the variant's
///   own message reads oddly.
/// - [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
///
/// **Finite but absurd values are accepted unchecked**: an `at` of
/// `±1e30`, a `width` of `f32::MAX` or a subnormal `1e-40` all open a
/// menu. Layout does not panic on them — the menu's bounds saturate —
/// but it lands far off-screen or vanishingly thin; choosing a sane
/// position and width is the caller's job, since this crate measures no
/// text and clamps to no viewport (see "What this deliberately does not
/// do").
pub fn open_menu(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    at: (f32, f32),
    width: f32,
    items: Vec<MenuItem>,
) -> Result<WidgetId, WidgetError> {
    if !items.iter().any(|item| item.kind == MenuItemKind::Action) {
        return Err(WidgetError::IndexOutOfRange { index: 0, len: 0 });
    }
    if !(width.is_finite() && width > 0.0) {
        return Err(WidgetError::InvalidRange {
            min: 0.0,
            max: f64::from(width),
        });
    }
    if !(at.0.is_finite() && at.1.is_finite()) {
        return Err(WidgetError::InvalidRange {
            min: f64::from(at.0),
            max: f64::from(at.1),
        });
    }
    let state = MenuState::new(
        label.to_owned(),
        items,
        row_height(scales),
        spacing(scales.spacing.xxs),
    );
    let menu = tree.insert(
        parent,
        menu_style(at, width),
        menu_node(&state),
        WidgetKind::Menu(state),
    )?;
    // A menu floats above every base-layer widget and escapes `parent`'s
    // clipping ancestors (this module's own doc comment); a failure here
    // takes the same cleanup path as a failed child build.
    let built = tree
        .set_layer(menu, PaintLayer::Popover)
        .and_then(|()| build_children(tree, menu))
        .and_then(|ids| {
            let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu) else {
                return Err(WidgetError::WrongWidgetKind(menu));
            };
            state.item_ids = ids;
            let node = menu_node(state);
            tree.set_accessibility(menu, node)
        });
    if let Err(err) = built {
        // Best effort, and unreachable today (every id here was just
        // inserted): nothing is left behind on failure.
        let _ = tree.remove(menu);
        return Err(err);
    }
    Ok(menu)
}

fn state(tree: &WidgetTree<WidgetKind>, menu: WidgetId) -> Result<&MenuState, WidgetError> {
    match tree.payload(menu).ok_or(WidgetError::UnknownWidget(menu))? {
        WidgetKind::Menu(state) => Ok(state),
        _ => Err(WidgetError::WrongWidgetKind(menu)),
    }
}

/// A read-only view of `menu`'s own [`MenuState`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `menu` doesn't exist (a
/// closed menu included), or [`WidgetError::WrongWidgetKind`] if it
/// isn't a menu.
pub fn menu_state(
    tree: &WidgetTree<WidgetKind>,
    menu: WidgetId,
) -> Result<&MenuState, WidgetError> {
    state(tree, menu)
}

/// Feeds one key to `menu` — see this module's doc comment for the
/// table. On `Activated` or `Cancelled` the menu's whole subtree has
/// been removed by the time this returns.
///
/// **Reconciles first, on every call** (the precedent `tab_bar.rs`
/// sets): before the key is acted on, the menu's children and its own
/// node are brought in line with the state's *current* highlight, so an
/// item removed from under it or a stale [`MenuState`] written back
/// through `WidgetTree::payload_mut` is repaired even by a key that is
/// then `Ignored`, and `active_descendant` never names a dead id. If
/// that repair changed anything, an `Enter`/`Space` on the same call
/// returns `Ignored` instead of activating: the item the state named was
/// not the one painted or announced, and the user now sees the true
/// highlight and can press again. An intact menu costs no damage.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a live menu; nothing changes.
///
/// **Commit on success**: a `Moved` highlight is written to the state
/// only after the two rows involved have been rewritten, so a failure
/// there (which needs a tree id that does not exist, and is unreachable
/// today) leaves the state where it was; any row it had already
/// rewritten disagrees with that state and is repaired by the next key.
pub fn handle_menu_key(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    key: MenuKey,
) -> Result<MenuOutcome, WidgetError> {
    let previous = state(tree, menu)?.highlighted;
    let repaired = sync(tree, menu, previous)?;
    let outcome = state(tree, menu)?.transition(key);
    match outcome {
        MenuOutcome::Ignored => {}
        MenuOutcome::Moved(index) => {
            // `sync` has just verified every child, so only the two rows
            // whose highlight changes can disagree.
            if let Some(previous) = previous {
                sync_row(tree, menu, previous, Some(index))?;
            }
            sync_row(tree, menu, index, Some(index))?;
            let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu) else {
                return Err(WidgetError::WrongWidgetKind(menu));
            };
            state.highlighted = Some(index);
            sync_menu_node(tree, menu)?;
        }
        MenuOutcome::Activated(_) if repaired => return Ok(MenuOutcome::Ignored),
        MenuOutcome::Activated(_) | MenuOutcome::Cancelled => tree.remove(menu)?,
    }
    Ok(outcome)
}

/// Closes `menu` without activating anything — removes its whole
/// subtree, exactly as `Escape` does.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// for an id that isn't a live menu; nothing changes.
pub fn close_menu(tree: &mut WidgetTree<WidgetKind>, menu: WidgetId) -> Result<(), WidgetError> {
    state(tree, menu)?;
    tree.remove(menu)
}

/// Whether `id` is one of `menu`'s own item nodes: a live
/// [`WidgetKind::ListRow`] or [`WidgetKind::MenuSeparator`] **child of
/// `menu`**. Checked before this module rewrites or removes anything, so
/// a stale id in [`MenuState::item_ids`] (a snapshot written back
/// through `WidgetTree::payload_mut`, or an id since reused) can never
/// make it touch a widget it does not own.
fn is_own_item(tree: &WidgetTree<WidgetKind>, menu: WidgetId, id: WidgetId) -> bool {
    tree.parent(id) == Some(menu)
        && matches!(
            tree.payload(id),
            Some(WidgetKind::ListRow(_) | WidgetKind::MenuSeparator)
        )
}

/// Brings `menu`'s children and its own node in line with its state at
/// `highlighted`, records the item ids, and returns whether anything
/// disagreed. **Trusts nothing it did not just check**: if the menu's
/// item children are not exactly the tracked ids, one per item, they
/// are all removed and rebuilt; otherwise every child's payload is
/// compared (cheap — no node is built), and each one that disagrees,
/// plus the highlighted row regardless, is then verified **whole**,
/// node included, and rewritten and dirtied only if it differs.
fn sync(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    highlighted: Option<usize>,
) -> Result<bool, WidgetError> {
    let current = state(tree, menu)?;
    // One pass over the menu's children: the `ListRow`/`MenuSeparator`
    // ones (a child's parent is `menu` by construction) must be exactly
    // the tracked ids, in order, one per item; each one's payload is
    // compared on the way.
    let mut intact = current.item_ids.len() == current.items.len();
    let mut seen = 0_usize;
    let mut stale = Vec::new();
    for &child in tree.children(menu).unwrap_or_default() {
        if !intact {
            break;
        }
        let payload = tree.payload(child);
        if !matches!(
            payload,
            Some(WidgetKind::ListRow(_) | WidgetKind::MenuSeparator)
        ) {
            continue;
        }
        match (current.item_ids.get(seen), current.items.get(seen)) {
            (Some(&id), Some(item)) if id == child => {
                if payload != Some(&expected_payload(item, highlighted == Some(seen))) {
                    stale.push(seen);
                }
                seen = seen.saturating_add(1);
            }
            _ => intact = false,
        }
    }
    intact &= seen == current.item_ids.len();
    let mut changed = false;
    if intact {
        for index in stale.into_iter().chain(highlighted) {
            changed |= sync_row(tree, menu, index, highlighted)?;
        }
    } else {
        let ids = rebuild_children(tree, menu, highlighted)?;
        let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu) else {
            return Err(WidgetError::WrongWidgetKind(menu));
        };
        state.item_ids = ids;
        changed = true;
    }
    Ok(sync_menu_node(tree, menu)? || changed)
}

/// Verifies item `index`'s child **whole** — payload and node — against
/// the state at `highlighted`, rewriting and dirtying it only if it
/// differs; returns whether it did. Only called once [`sync`] has
/// established that every tracked id is one of the menu's own items.
fn sync_row(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    index: usize,
    highlighted: Option<usize>,
) -> Result<bool, WidgetError> {
    let current = state(tree, menu)?;
    let (Some(&id), Some(item)) = (current.item_ids.get(index), current.items.get(index)) else {
        return Ok(false);
    };
    let is_action = |item: &&MenuItem| item.kind == MenuItemKind::Action;
    let size = current.items.iter().filter(is_action).count();
    let position = current
        .items
        .iter()
        .take(index.saturating_add(1))
        .filter(is_action)
        .count();
    let (node, payload) = expected_child(item, position, size, highlighted == Some(index));
    if tree.payload(id) == Some(&payload) && tree.accessibility(id) == Some(&node) {
        return Ok(false);
    }
    if let Some(slot) = tree.payload_mut(id) {
        *slot = payload;
    }
    tree.set_accessibility(id, node)?;
    tree.mark_dirty(id)?;
    Ok(true)
}

/// Rewrites and dirties the menu's own node if it differs from what its
/// state says (its `active_descendant` above all); returns whether it did.
fn sync_menu_node(tree: &mut WidgetTree<WidgetKind>, menu: WidgetId) -> Result<bool, WidgetError> {
    let expected = menu_node(state(tree, menu)?);
    if tree.accessibility(menu) == Some(&expected) {
        return Ok(false);
    }
    tree.set_accessibility(menu, expected)?;
    tree.mark_dirty(menu)?;
    Ok(true)
}

/// Removes every item child of `menu` — every `ListRow`/`MenuSeparator`
/// child, **and** every child whose id is tracked in `item_ids` whatever
/// its payload now is (an item overwritten to another kind would
/// otherwise be orphaned and still sized) — then builds one fresh child
/// per item at `highlighted`. `WidgetTree::remove` marks each removed
/// child's old bounds dirty, so the vacated area repaints.
fn rebuild_children(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    highlighted: Option<usize>,
) -> Result<Vec<WidgetId>, WidgetError> {
    let tracked: HashSet<WidgetId> = state(tree, menu)?.item_ids.iter().copied().collect();
    let doomed: Vec<WidgetId> = tree
        .children(menu)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&child| is_own_item(tree, menu, child) || tracked.contains(&child))
        .collect();
    for child in doomed {
        tree.remove(child)?;
    }
    insert_children(tree, menu, highlighted)
}

/// [`open_menu`]'s first build, at the state's own initial highlight.
fn build_children(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
) -> Result<Vec<WidgetId>, WidgetError> {
    let highlighted = state(tree, menu)?.highlighted;
    insert_children(tree, menu, highlighted)
}

fn insert_children(
    tree: &mut WidgetTree<WidgetKind>,
    menu: WidgetId,
    highlighted: Option<usize>,
) -> Result<Vec<WidgetId>, WidgetError> {
    let current = state(tree, menu)?;
    let row = child_style(current.row_height);
    let separator = child_style(current.separator_height);
    let kinds: Vec<MenuItemKind> = current.items.iter().map(|item| item.kind).collect();
    let expected = expected_children(&current.items, highlighted);
    let mut ids = Vec::with_capacity(expected.len());
    for (kind, (node, payload)) in kinds.into_iter().zip(expected) {
        let style = match kind {
            MenuItemKind::Action => row.clone(),
            MenuItemKind::Separator => separator.clone(),
        };
        ids.push(tree.insert(menu, style, node, payload)?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::{
        MenuItem, MenuItemKind, MenuKey, MenuOutcome, MenuState, close_menu, handle_menu_key,
        menu_state, open_menu,
    };
    use crate::WidgetError;
    use crate::shortcut::NamedKey;
    use crate::tree::{PaintLayer, WidgetId, WidgetTree};
    use crate::widgets::{
        ListRowState, WidgetKind, insert_button, insert_dropdown, new_tree, row_height, test_scales,
    };
    use accesskit::{Action, Role};
    use taffy::style_helpers::length;
    use taffy::{FlexDirection, Size, Style};

    fn disabled(label: &str) -> MenuItem {
        MenuItem {
            enabled: false,
            ..MenuItem::action(label)
        }
    }

    /// `Cut, Copy, ---, (Paste disabled), Delete` — indices 0..=4, with
    /// enabled actions at 0, 1 and 4.
    fn items() -> Vec<MenuItem> {
        vec![
            MenuItem::action("Cut"),
            MenuItem::action("Copy"),
            MenuItem::separator(),
            disabled("Paste"),
            MenuItem::action("Delete"),
        ]
    }

    fn pure(items: Vec<MenuItem>) -> MenuState {
        MenuState::new("Edit".to_owned(), items, 21.0, 4.0)
    }

    fn at(items: Vec<MenuItem>, highlighted: Option<usize>) -> MenuState {
        let mut state = pure(items);
        state.highlighted = highlighted;
        state
    }

    fn ok<T>(result: Result<T, WidgetError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    // ---- The pure transition table ------------------------------------

    #[test]
    fn the_initial_highlight_is_the_first_enabled_action() {
        assert_eq!(pure(items()).highlighted(), Some(0));
        let leading = vec![
            MenuItem::separator(),
            disabled("Undo"),
            MenuItem::action("Redo"),
        ];
        assert_eq!(pure(leading).highlighted(), Some(2));
        assert_eq!(pure(vec![disabled("A"), disabled("B")]).highlighted(), None);
    }

    #[test]
    fn down_skips_separators_and_disabled_items_and_wraps() {
        let state = at(items(), Some(0));
        assert_eq!(state.transition(MenuKey::Down), MenuOutcome::Moved(1));
        let state = at(items(), Some(1));
        assert_eq!(
            state.transition(MenuKey::Down),
            MenuOutcome::Moved(4),
            "skips the separator at 2 and the disabled Paste at 3"
        );
        let state = at(items(), Some(4));
        assert_eq!(
            state.transition(MenuKey::Down),
            MenuOutcome::Moved(0),
            "wraps"
        );
    }

    #[test]
    fn up_skips_separators_and_disabled_items_and_wraps() {
        let state = at(items(), Some(4));
        assert_eq!(state.transition(MenuKey::Up), MenuOutcome::Moved(1));
        let state = at(items(), Some(1));
        assert_eq!(state.transition(MenuKey::Up), MenuOutcome::Moved(0));
        let state = at(items(), Some(0));
        assert_eq!(
            state.transition(MenuKey::Up),
            MenuOutcome::Moved(4),
            "wraps"
        );
    }

    #[test]
    fn home_and_end_pick_the_first_and_last_enabled_items() {
        let bracketed = vec![
            disabled("A"),
            MenuItem::action("B"),
            MenuItem::separator(),
            MenuItem::action("C"),
            disabled("D"),
        ];
        let state = at(bracketed.clone(), Some(3));
        assert_eq!(state.transition(MenuKey::Home), MenuOutcome::Moved(1));
        assert_eq!(state.transition(MenuKey::End), MenuOutcome::Ignored);
        let state = at(bracketed, Some(1));
        assert_eq!(state.transition(MenuKey::End), MenuOutcome::Moved(3));
        assert_eq!(state.transition(MenuKey::Home), MenuOutcome::Ignored);
    }

    #[test]
    fn a_single_enabled_item_ignores_every_movement_key() {
        let state = pure(vec![
            disabled("A"),
            MenuItem::separator(),
            MenuItem::action("B"),
        ]);
        assert_eq!(state.highlighted(), Some(2));
        for key in [MenuKey::Up, MenuKey::Down, MenuKey::Home, MenuKey::End] {
            assert_eq!(state.transition(key), MenuOutcome::Ignored, "{key:?}");
        }
    }

    #[test]
    fn enter_and_space_activate_the_highlight_and_escape_cancels() {
        let state = at(items(), Some(4));
        assert_eq!(state.transition(MenuKey::Enter), MenuOutcome::Activated(4));
        assert_eq!(state.transition(MenuKey::Space), MenuOutcome::Activated(4));
        assert_eq!(state.transition(MenuKey::Escape), MenuOutcome::Cancelled);
    }

    #[test]
    fn with_every_action_disabled_only_escape_does_anything() {
        let state = pure(vec![disabled("A"), MenuItem::separator(), disabled("B")]);
        assert_eq!(state.highlighted(), None);
        for key in [
            MenuKey::Up,
            MenuKey::Down,
            MenuKey::Home,
            MenuKey::End,
            MenuKey::Enter,
            MenuKey::Space,
        ] {
            assert_eq!(state.transition(key), MenuOutcome::Ignored, "{key:?}");
        }
        assert_eq!(state.transition(MenuKey::Escape), MenuOutcome::Cancelled);
    }

    #[test]
    fn named_keys_map_onto_the_seven_menu_keys_and_nothing_else() {
        let cases = [
            (NamedKey::ArrowUp, Some(MenuKey::Up)),
            (NamedKey::ArrowDown, Some(MenuKey::Down)),
            (NamedKey::Home, Some(MenuKey::Home)),
            (NamedKey::End, Some(MenuKey::End)),
            (NamedKey::Enter, Some(MenuKey::Enter)),
            (NamedKey::Space, Some(MenuKey::Space)),
            (NamedKey::Escape, Some(MenuKey::Escape)),
            (NamedKey::Tab, None),
            (NamedKey::ArrowLeft, None),
            (NamedKey::ArrowRight, None),
        ];
        for (named, expected) in cases {
            assert_eq!(MenuKey::from_named_key(named), expected, "{named:?}");
        }
    }

    // ---- Tree wrappers --------------------------------------------------

    fn sized_root() -> Style {
        Style {
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(300.0_f32),
                height: length(300.0_f32),
            },
            ..Default::default()
        }
    }

    fn opened(items: Vec<MenuItem>) -> (WidgetTree<WidgetKind>, WidgetId) {
        let (mut tree, root) = new_tree(sized_root());
        let menu = ok(open_menu(
            &mut tree,
            root,
            &test_scales(),
            "Edit",
            (37.0, 11.0),
            120.0,
            items,
        ));
        (tree, menu)
    }

    fn press(tree: &mut WidgetTree<WidgetKind>, menu: WidgetId, key: MenuKey) -> MenuOutcome {
        ok(handle_menu_key(tree, menu, key))
    }

    fn snapshot(tree: &WidgetTree<WidgetKind>, menu: WidgetId) -> MenuState {
        ok(menu_state(tree, menu)).clone()
    }

    fn count(tree: &WidgetTree<WidgetKind>) -> usize {
        fn walk(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> usize {
            1 + tree
                .children(id)
                .unwrap_or_default()
                .iter()
                .map(|&c| walk(tree, c))
                .sum::<usize>()
        }
        walk(tree, tree.root())
    }

    #[test]
    fn open_menu_builds_a_menu_with_one_child_per_item_in_order() {
        let (tree, menu) = opened(items());
        let state = snapshot(&tree, menu);
        assert_eq!(tree.children(menu), Some(state.item_ids()));
        assert_eq!(state.item_ids().len(), 5);
        let roles: Vec<Option<Role>> = state
            .item_ids()
            .iter()
            .map(|&id| tree.accessibility(id).map(accesskit::Node::role))
            .collect();
        assert_eq!(
            roles,
            vec![
                Some(Role::MenuItem),
                Some(Role::MenuItem),
                Some(Role::Splitter),
                Some(Role::MenuItem),
                Some(Role::MenuItem),
            ]
        );
        let payloads: Vec<Option<&WidgetKind>> = state
            .item_ids()
            .iter()
            .map(|&id| tree.payload(id))
            .collect();
        let row = |selected, disabled| WidgetKind::ListRow(ListRowState { selected, disabled });
        assert_eq!(
            payloads,
            vec![
                Some(&row(true, false)),
                Some(&row(false, false)),
                Some(&WidgetKind::MenuSeparator),
                Some(&row(false, true)),
                Some(&row(false, false)),
            ]
        );
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.role(), Role::Menu);
        assert_eq!(node.label(), Some("Edit"));
        let mut expected = accesskit::Node::new(Role::Menu);
        expected.set_label("Edit");
        expected.add_action(Action::Focus);
        if let Some(&first) = state.item_ids().first() {
            expected.set_active_descendant(first);
        }
        assert_eq!(
            node, &expected,
            "focusable, labelled, highlight as active_descendant"
        );
    }

    #[test]
    fn open_menu_rejects_bad_input_and_adds_nothing() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let before = count(&tree);
        let try_open = |tree: &mut WidgetTree<WidgetKind>, parent, at, width, items| {
            open_menu(tree, parent, &scales, "Edit", at, width, items)
        };
        for bad in [vec![], vec![MenuItem::separator(), MenuItem::separator()]] {
            match try_open(&mut tree, root, (0.0, 0.0), 100.0, bad) {
                Err(WidgetError::IndexOutOfRange { index: 0, len: 0 }) => {}
                other => unreachable!("expected IndexOutOfRange, got {other:?}"),
            }
        }
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            match try_open(&mut tree, root, (0.0, 0.0), width, items()) {
                Err(WidgetError::InvalidRange { .. }) => {}
                other => unreachable!("expected InvalidRange for {width}, got {other:?}"),
            }
        }
        for bad_at in [(f32::NAN, 0.0), (0.0, f32::INFINITY)] {
            match try_open(&mut tree, root, bad_at, 100.0, items()) {
                Err(WidgetError::InvalidRange { .. }) => {}
                other => unreachable!("expected InvalidRange for {bad_at:?}, got {other:?}"),
            }
        }
        let bogus = accesskit::NodeId(999);
        match try_open(&mut tree, bogus, (0.0, 0.0), 100.0, items()) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(count(&tree), before, "nothing was added");
    }

    #[test]
    fn empty_labels_and_an_all_disabled_menu_are_accepted() {
        let (tree, menu) = opened(vec![disabled(""), disabled("B")]);
        let state = snapshot(&tree, menu);
        assert_eq!(state.highlighted(), None);
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.active_descendant(), None, "nothing highlighted");
    }

    #[test]
    fn moving_rewrites_exactly_the_two_items_and_the_active_descendant() {
        let (mut tree, menu) = opened(items());
        let ids = snapshot(&tree, menu).item_ids().to_vec();
        let Some(&[cut, copy, _, _, delete]) = Some(ids.as_slice()) else {
            unreachable!("five items");
        };
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(1));
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(4));
        let state = snapshot(&tree, menu);
        assert_eq!(state.highlighted(), Some(4));
        assert_eq!(
            state.item_ids(),
            ids.as_slice(),
            "an intact menu keeps its children"
        );
        let selected = |id| matches!(tree.payload(id), Some(WidgetKind::ListRow(r)) if r.selected);
        assert!(!selected(cut) && !selected(copy) && selected(delete));
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.active_descendant(), Some(delete));
        for &id in &ids {
            let Some(item) = tree.accessibility(id) else {
                unreachable!("live");
            };
            assert!(
                !item.is_selected().unwrap_or(false),
                "a menu item is never `selected`"
            );
        }
    }

    /// A menu is a popover root, so the part of it overflowing a small,
    /// clipping `parent` is still reached by both hit-testers.
    #[test]
    fn a_menu_overflowing_its_parent_is_a_reachable_popover() {
        let (mut tree, root) = new_tree(sized_root());
        let panel = ok(tree.insert(
            root,
            Style {
                size: Size {
                    width: length(150.0_f32),
                    height: length(30.0_f32),
                },
                overflow: taffy::Point {
                    x: taffy::Overflow::Hidden,
                    y: taffy::Overflow::Hidden,
                },
                ..Default::default()
            },
            accesskit::Node::new(Role::Group),
            WidgetKind::Container,
        ));
        let menu = ok(open_menu(
            &mut tree,
            panel,
            &test_scales(),
            "Edit",
            (10.0, 5.0),
            120.0,
            items(),
        ));
        tree.compute_layout(300.0, 300.0);
        assert_eq!(tree.layer(menu), Some(PaintLayer::Popover));
        let state = snapshot(&tree, menu);
        let Some(&last) = state.item_ids().last() else {
            unreachable!("five items");
        };
        assert_eq!(tree.popover_root_of(last), Some(menu));
        let (Some(panel_bounds), Some(item)) = (tree.bounds(panel), tree.bounds(last)) else {
            unreachable!("laid out");
        };
        assert!(
            item.y >= panel_bounds.bottom(),
            "the last item lies wholly below the panel: {item:?} vs {panel_bounds:?}"
        );
        #[allow(clippy::cast_precision_loss)]
        let (x, y) = (item.x as f32 + 3.0, item.y as f32 + 3.0);
        assert_eq!(tree.hit_test((x, y)), Some(last));
        assert_eq!(
            crate::hit_test(&tree, f64::from(x), f64::from(y)),
            Some(last)
        );
        assert_eq!(tree.paint_order().last(), Some(&last));
    }

    #[test]
    fn an_ignored_key_changes_nothing_and_costs_no_damage() {
        let (mut tree, menu) = opened(vec![MenuItem::action("Only")]);
        tree.compute_layout(300.0, 300.0);
        let _ = tree.take_damage();
        let before = snapshot(&tree, menu);
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Ignored);
        assert_eq!(snapshot(&tree, menu), before);
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn activating_and_cancelling_remove_the_whole_subtree() {
        for (key, expected) in [
            (MenuKey::Enter, MenuOutcome::Activated(0)),
            (MenuKey::Space, MenuOutcome::Activated(0)),
            (MenuKey::Escape, MenuOutcome::Cancelled),
        ] {
            let (mut tree, menu) = opened(items());
            let ids = snapshot(&tree, menu).item_ids().to_vec();
            assert_eq!(press(&mut tree, menu, key), expected);
            assert!(!tree.contains(menu), "{key:?}: the menu is gone");
            assert!(
                ids.iter().all(|&id| !tree.contains(id)),
                "{key:?}: every item is gone"
            );
            assert_eq!(count(&tree), 1, "{key:?}: only the root is left");
        }
        let (mut tree, menu) = opened(vec![disabled("A")]);
        assert_eq!(press(&mut tree, menu, MenuKey::Enter), MenuOutcome::Ignored);
        assert!(
            tree.contains(menu),
            "Enter with nothing highlighted keeps the menu open"
        );
        ok(close_menu(&mut tree, menu));
        assert!(!tree.contains(menu));
    }

    #[test]
    fn menu_functions_reject_a_non_menu_or_removed_id_and_change_nothing() {
        let (mut tree, menu) = opened(items());
        let root = tree.root();
        let scales = test_scales();
        let button = ok(insert_button(&mut tree, root, &scales, "OK"));
        let dropdown = ok(insert_dropdown(
            &mut tree,
            root,
            &scales,
            "Blend",
            vec!["Normal".to_owned()],
            Some(0),
        ));
        let before = count(&tree);
        for id in [button, dropdown, root] {
            match handle_menu_key(&mut tree, id, MenuKey::Escape) {
                Err(WidgetError::WrongWidgetKind(got)) => assert_eq!(got, id),
                other => unreachable!("expected WrongWidgetKind, got {other:?}"),
            }
            match close_menu(&mut tree, id) {
                Err(WidgetError::WrongWidgetKind(got)) => assert_eq!(got, id),
                other => unreachable!("expected WrongWidgetKind, got {other:?}"),
            }
        }
        assert_eq!(count(&tree), before);
        ok(close_menu(&mut tree, menu));
        for result in [
            handle_menu_key(&mut tree, menu, MenuKey::Down).map(|_| ()),
            close_menu(&mut tree, menu),
            menu_state(&tree, menu).map(|_| ()),
        ] {
            match result {
                Err(WidgetError::UnknownWidget(got)) => assert_eq!(got, menu),
                other => unreachable!("expected UnknownWidget, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_externally_removed_item_is_rebuilt_on_the_next_move() {
        let (mut tree, menu) = opened(items());
        let old = snapshot(&tree, menu).item_ids().to_vec();
        let Some(&copy) = old.get(1) else {
            unreachable!("five items");
        };
        ok(tree.remove(copy));
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(1));
        let state = snapshot(&tree, menu);
        assert_eq!(state.item_ids().len(), 5);
        assert_eq!(tree.children(menu), Some(state.item_ids()));
        let Some(&new_copy) = state.item_ids().get(1) else {
            unreachable!("five items");
        };
        assert!(matches!(tree.payload(new_copy), Some(WidgetKind::ListRow(r)) if r.selected));
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.active_descendant(), Some(new_copy), "never a dead id");
    }

    /// A stale `item_ids` naming a widget the menu does not own (here a
    /// button elsewhere) must never be rewritten or removed.
    #[test]
    fn a_foreign_id_in_item_ids_is_never_touched() {
        let (mut tree, menu) = opened(items());
        let root = tree.root();
        let button = ok(insert_button(&mut tree, root, &test_scales(), "OK"));
        let button_node = tree.accessibility(button).cloned();
        let button_payload = tree.payload(button).cloned();
        if let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu)
            && let Some(slot) = state.item_ids.get_mut(1)
        {
            *slot = button;
        }
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(1));
        assert!(tree.contains(button), "the foreign widget survives");
        assert_eq!(tree.parent(button), Some(root));
        assert_eq!(tree.accessibility(button).cloned(), button_node);
        assert_eq!(tree.payload(button).cloned(), button_payload);
        let state = snapshot(&tree, menu);
        assert!(!state.item_ids().contains(&button));
        assert_eq!(tree.children(menu), Some(state.item_ids()));
    }

    /// RT-1 (review): a stale [`MenuState`] written back after a move
    /// names Cut while the tree still paints and announces Copy. `Enter`
    /// must repair the tree first and then *not* activate — pre-fix it
    /// returned `Activated(0)` for an item neither painted nor the
    /// `active_descendant`.
    #[test]
    fn a_stale_state_written_back_is_repaired_before_enter_can_activate() {
        let (mut tree, menu) = opened(items());
        let stale = snapshot(&tree, menu);
        let Some(&[cut, copy, ..]) = Some(stale.item_ids()) else {
            unreachable!("five items");
        };
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(1));
        if let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu) {
            *state = stale;
        }
        assert_eq!(
            press(&mut tree, menu, MenuKey::Enter),
            MenuOutcome::Ignored,
            "the tree disagreed with the state: repair, don't activate"
        );
        assert!(tree.contains(menu), "still open");
        let selected = |tree: &WidgetTree<WidgetKind>, id| matches!(tree.payload(id), Some(WidgetKind::ListRow(r)) if r.selected);
        assert!(
            selected(&tree, cut) && !selected(&tree, copy),
            "one highlight"
        );
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.active_descendant(), Some(cut));
        assert_eq!(
            press(&mut tree, menu, MenuKey::Enter),
            MenuOutcome::Activated(0),
            "once the user sees Cut, Enter activates it"
        );
    }

    /// RT-2 (review): an `Ignored` key must still repair a removed
    /// highlighted item — pre-fix `active_descendant` went on naming the
    /// dead id.
    #[test]
    fn an_ignored_key_still_repairs_a_dangling_active_descendant() {
        let (mut tree, menu) = opened(items());
        let Some(&cut) = snapshot(&tree, menu).item_ids().first() else {
            unreachable!("five items");
        };
        ok(tree.remove(cut));
        assert_eq!(press(&mut tree, menu, MenuKey::Home), MenuOutcome::Ignored);
        let state = snapshot(&tree, menu);
        assert_eq!(tree.children(menu), Some(state.item_ids()));
        let Some(&new_cut) = state.item_ids().first() else {
            unreachable!("five items");
        };
        assert_ne!(new_cut, cut);
        assert!(matches!(tree.payload(new_cut), Some(WidgetKind::ListRow(r)) if r.selected));
        let Some(node) = tree.accessibility(menu) else {
            unreachable!("live");
        };
        assert_eq!(node.active_descendant(), Some(new_cut), "never a dead id");
    }

    #[test]
    fn enter_never_activates_a_highlight_that_is_not_an_enabled_action() {
        for highlighted in [Some(2), Some(3), Some(99)] {
            let state = at(items(), highlighted);
            for key in [MenuKey::Enter, MenuKey::Space] {
                assert_eq!(
                    state.transition(key),
                    MenuOutcome::Ignored,
                    "{key:?} at {highlighted:?}"
                );
            }
        }
        let (mut tree, menu) = opened(items());
        if let Some(WidgetKind::Menu(state)) = tree.payload_mut(menu) {
            state.highlighted = Some(3);
        }
        assert_eq!(press(&mut tree, menu, MenuKey::Enter), MenuOutcome::Ignored);
        assert_eq!(
            press(&mut tree, menu, MenuKey::Enter),
            MenuOutcome::Ignored,
            "the disabled Paste is never activated, even once the tree agrees"
        );
        assert!(tree.contains(menu));
    }

    #[test]
    fn stepping_from_past_the_end_starts_from_the_matching_end() {
        let state = at(items(), Some(99));
        assert_eq!(state.transition(MenuKey::Down), MenuOutcome::Moved(0));
        assert_eq!(state.transition(MenuKey::Up), MenuOutcome::Moved(4));
    }

    /// An item whose payload was overwritten to another kind is still the
    /// menu's own child: the rebuild removes it rather than orphaning a
    /// still-sized stranger among the fresh items.
    #[test]
    fn an_item_overwritten_to_another_kind_is_removed_by_the_rebuild() {
        let (mut tree, menu) = opened(items());
        let Some(&copy) = snapshot(&tree, menu).item_ids().get(1) else {
            unreachable!("five items");
        };
        if let Some(slot) = tree.payload_mut(copy) {
            *slot = WidgetKind::Container;
        }
        assert_eq!(press(&mut tree, menu, MenuKey::Down), MenuOutcome::Moved(1));
        assert!(!tree.contains(copy), "the overwritten item is gone");
        let state = snapshot(&tree, menu);
        assert_eq!(tree.children(menu), Some(state.item_ids()));
        assert_eq!(state.item_ids().len(), 5);
    }

    /// Finite but absurd `at`/`width` are accepted unchecked, as the
    /// `open_menu` doc discloses; layout must not panic on them.
    #[test]
    fn finite_but_absurd_geometry_is_accepted_and_lays_out_without_panicking() {
        for (at, width) in [
            ((1e30, -1e30), 120.0),
            ((0.0, 0.0), f32::MAX),
            ((0.0, 0.0), 1e-40),
            ((f32::MAX, f32::MIN), f32::MAX),
        ] {
            let (mut tree, root) = new_tree(sized_root());
            let menu = ok(open_menu(
                &mut tree,
                root,
                &test_scales(),
                "Edit",
                at,
                width,
                items(),
            ));
            tree.compute_layout(300.0, 300.0);
            assert!(tree.bounds(menu).is_some(), "{at:?} / {width}: laid out");
        }
    }

    // ---- Accessibility ---------------------------------------------------

    fn find_all(tree: &accesskit_consumer::Tree, role: Role) -> Vec<accesskit_consumer::Node<'_>> {
        let mut found = Vec::new();
        let mut pending = vec![tree.state().root()];
        while let Some(node) = pending.pop() {
            if node.role() == role {
                found.push(node);
            }
            pending.extend(node.children());
        }
        found
    }

    #[test]
    fn the_consumer_sees_roles_positions_actions_and_follows_the_highlight() {
        let (mut tree, menu) = opened(items());
        let mut focus = crate::FocusManager::new();
        ok(focus.focus(&mut tree, menu));
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(menu), true);

        let mut items: Vec<_> = find_all(&consumer, Role::MenuItem)
            .into_iter()
            .map(|node| {
                (
                    node.position_in_set(),
                    node.size_of_set(),
                    node.label(),
                    node.is_disabled(),
                    node.supports_action(Action::Click, &|_| {
                        accesskit_consumer::FilterResult::Include
                    }),
                    node.supports_action(Action::Focus, &|_| {
                        accesskit_consumer::FilterResult::Include
                    }),
                    node.is_selected(),
                )
            })
            .collect();
        items.sort_by_key(|item| item.0);
        let s = Some(4);
        assert_eq!(
            items,
            vec![
                (Some(1), s, Some("Cut".to_owned()), false, true, false, None),
                (
                    Some(2),
                    s,
                    Some("Copy".to_owned()),
                    false,
                    true,
                    false,
                    None
                ),
                (
                    Some(3),
                    s,
                    Some("Paste".to_owned()),
                    true,
                    false,
                    false,
                    None
                ),
                (
                    Some(4),
                    s,
                    Some("Delete".to_owned()),
                    false,
                    true,
                    false,
                    None
                ),
            ],
            "positions count actions only; disabled has no actions; nothing is selected"
        );
        assert_eq!(find_all(&consumer, Role::Splitter).len(), 1);
        assert_eq!(find_all(&consumer, Role::Menu).len(), 1);

        let focused = |tree: &WidgetTree<WidgetKind>| {
            let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(menu), true);
            consumer
                .state()
                .focus()
                .map(|node| (node.role(), node.label()))
        };
        assert_eq!(
            focused(&tree),
            Some((Role::MenuItem, Some("Cut".to_owned())))
        );
        press(&mut tree, menu, MenuKey::Up);
        assert_eq!(
            focused(&tree),
            Some((Role::MenuItem, Some("Delete".to_owned()))),
            "focus follows the highlight through active_descendant"
        );
    }

    #[test]
    fn with_nothing_highlighted_the_consumer_focuses_the_menu_itself() {
        let (mut tree, menu) = opened(vec![disabled("A")]);
        let mut focus = crate::FocusManager::new();
        ok(focus.focus(&mut tree, menu));
        let consumer = accesskit_consumer::Tree::new(tree.accessibility_update(menu), true);
        assert_eq!(
            consumer.state().focus().map(|node| node.role()),
            Some(Role::Menu)
        );
    }

    #[test]
    fn the_menu_is_focusable_and_its_items_never_enter_the_tab_order() {
        let (mut tree, menu) = opened(items());
        let root = tree.root();
        let button = ok(insert_button(&mut tree, root, &test_scales(), "OK"));
        let mut focus = crate::FocusManager::new();
        let mut visited = Vec::new();
        for _ in 0..4 {
            if let Some(next) = focus.focus_next(&mut tree) {
                visited.push(next);
            }
        }
        assert_eq!(visited, vec![menu, button, menu, button]);
    }

    // ---- Layout ----------------------------------------------------------

    #[test]
    fn the_menu_sits_at_its_offset_at_its_width_and_moves_no_sibling() {
        let scales = test_scales();
        let (mut tree, root) = new_tree(sized_root());
        let before = ok(insert_button(&mut tree, root, &scales, "Before"));
        let menu = ok(open_menu(
            &mut tree,
            root,
            &scales,
            "Edit",
            (37.0, 11.0),
            120.0,
            items(),
        ));
        let after = ok(insert_button(&mut tree, root, &scales, "After"));
        tree.compute_layout(300.0, 300.0);
        let (Some(parent), Some(bounds), Some(before_box), Some(after_box)) = (
            tree.bounds(root),
            tree.bounds(menu),
            tree.bounds(before),
            tree.bounds(after),
        ) else {
            unreachable!("laid out");
        };
        assert_eq!((bounds.x, bounds.y), (parent.x + 37, parent.y + 11));
        assert_eq!(bounds.width, 120);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (row, sep) = (
            row_height(&scales) as u32,
            super::spacing(scales.spacing.xxs) as u32,
        );
        assert_eq!(bounds.height, row * 4 + sep, "four rows and one separator");
        assert_eq!(
            after_box.y,
            before_box.bottom(),
            "the menu is out of flow: the next sibling sits right under the previous one"
        );

        let state = snapshot(&tree, menu);
        let mut y = bounds.y;
        for (item, &id) in state.items().iter().zip(state.item_ids()) {
            let Some(child) = tree.bounds(id) else {
                unreachable!("laid out");
            };
            let height = if item.kind == MenuItemKind::Separator {
                sep
            } else {
                row
            };
            assert_eq!(
                (child.x, child.y, child.width, child.height),
                (bounds.x, y, 120, height)
            );
            y += i64::from(height);
        }
    }
}
