//! A tree: a `Role::Tree` container holding real, nested
//! `Role::TreeItem` rows, each with its own depth, label, selection,
//! and — for a row that declares children — an expanded/collapsed
//! state that is kept in lockstep with the tree's actual structure.
//!
//! **Scope, stated honestly.** Three things a finished tree widget has
//! and this one does not:
//!
//! - **No scrolling container.** A tree taller than its parent
//!   overflows it; nothing here clips, and nothing observes a
//!   [`super::ScrollbarState`] to move content. That is the same gap
//!   `widgets`' own module doc comment records for `Scrollbar` (which
//!   is a position *model*, not scrolling), and it is unchanged by this
//!   module — a real scrolling container is separate, later work.
//! - **No disclosure triangle.** This crate draws no glyphs at all
//!   (`paint`'s own module doc comment — solid fills only), so a
//!   collapsed row is announced as collapsed but has no ▸ of its own;
//!   the caller-visible state is there, the pixel is not.
//! - **No text.** A row's `label` reaches the accessibility node and
//!   nothing else, the same "no real text shaping in this crate" gap
//!   `TextField` already has.
//!
//! **`WidgetTree`'s own traversals are unbounded recursion**
//! (`paint_order`/`collect_paint_order`, `build_taffy_node`,
//! `hit_test_from`, `remove_subtree`, `apply_taffy_layout`), and this
//! is the first widget in the crate that *invites* deep nesting — a
//! layers panel over a deeply grouped document is exactly the shape
//! that would find it. Pre-existing and not fixed here (it is a
//! property of `tree.rs`, not of this module, and every one of those
//! five sites would need converting together), but named rather than
//! left for someone to discover with a stack overflow.
//!
//! # The accessibility vocabulary, and which platforms actually read it
//!
//! [`node`] sets `Role::TreeItem`, `set_label`, `set_level(depth)`, and
//! `set_selected`. **`level` is 0-based here** — `accesskit`'s own
//! `usize` property, not ARIA's 1-based `aria-level`, so a top-level
//! row is level `0`; converting to ARIA's convention is a platform
//! adapter's job, not this crate's.
//!
//! `set_expanded` is set **only on a row that declares children**. A
//! leaf must not set it at all, and that is a real distinction rather
//! than tidiness: `accesskit_consumer::Node::supports_expand_collapse`
//! returns `true` for *every* `Role::TreeItem` regardless of this
//! property (checked against the pinned `accesskit_consumer` 0.38
//! source, `node.rs`), so the Windows UIA adapter advertises the
//! `ExpandCollapse` pattern on a leaf too; a leaf that set
//! `expanded = false` would be announced as "collapsed" and would then
//! fail an `Expand()` call. Omitting it maps to
//! `ExpandCollapseState_LeafNode` instead, which is the truth.
//!
//! For the same reason an enabled row declares **exactly one** of
//! `Action::Collapse` (when expanded) or `Action::Expand` (when
//! collapsed), never both and never either on a leaf:
//! `accesskit_windows`' own `set_expanded` returns `invalid_operation`
//! when the requested state already matches the current one (pinned
//! `accesskit_windows` 0.34, `node.rs`), so declaring the action that
//! could not change anything is declaring an action that cannot work.
//! `Action::Focus` and `Action::Click` are declared on every enabled
//! row; a disabled row declares none of them and sets `set_disabled`,
//! the same shape every other widget in this module already uses.
//!
//! **Which adapters actually consume this, stated rather than implied
//! away.** `expanded` and `level` are read by the **Windows** adapter
//! only — `accesskit_windows` maps them to the UIA `ExpandCollapse`
//! pattern and `UIA_LevelPropertyId`; neither
//! `accesskit_atspi_common` nor `accesskit_macos` reads either property
//! at all (checked by grepping the pinned sources for `is_expanded` and
//! `level()`: no hits outside the Windows crate). On Linux and macOS
//! the hierarchy is carried entirely by the **structural** parent/child
//! nesting every adapter reads — which is real here, because
//! [`insert_tree_item`] inserts a real child widget and
//! [`set_tree_item_expanded`] really removes one. That is the concrete
//! reason collapse is not a presentation-only flag in this module.
//!
//! # Collapse removes children; it does not hide them
//!
//! [`set_tree_item_expanded`]`(id, false)` removes `id`'s child widgets
//! from the [`WidgetTree`] outright (cascading through each whole
//! subtree, via `WidgetTree::remove`), and the caller re-inserts them
//! on expand — the same "real tree nodes, not a hidden flag" shape
//! `command_palette`'s own open/close already uses. Two consequences a
//! caller must know: a collapsed row's descendants no longer exist, so
//! their [`WidgetId`]s are dead; and expanding a row does **not**
//! repopulate it — [`set_tree_item_expanded`]`(id, true)` on a row with
//! no children is a deliberate, legal no-op that only updates the
//! accessibility node, which is exactly what a lazily-populated tree
//! needs.
//!
//! `has_children` is therefore **caller-declared** at insert time, not
//! derived from `tree.children(id).is_empty()`: a collapsed group has
//! no children in the tree and must still announce itself as collapsed
//! rather than as a leaf. [`insert_tree_item`] does maintain it in the
//! one direction it can — inserting a row under a row makes that parent
//! `has_children = true` and `expanded = true`, since the child is now
//! really there and really visible.
//!
//! **No crate-wide "hidden" flag was added**, deliberately. The
//! alternative design — a `WidgetNode::hidden` bool that collapse sets
//! — would have to be honoured by five separate traversals in
//! `tree.rs` (`paint_order`, `build_taffy_node`, `hit_test_from`,
//! `focus_order`, `accessibility_update`), and the last of those is
//! where it is genuinely dangerous: omitting nodes from a `TreeUpdate`
//! without also fixing up every parent's `children` list is what
//! produced a real crash on real macOS hardware once already
//! (`WidgetTree::accessibility_update`'s own doc comment records it).
//! This round's scope stays inside this file.

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{FlexDirection, Size, Style};

use super::{WidgetKind, spacing, tree_row_height};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// One row of a tree. `Eq`, unlike most state types here — every field
/// is a `String`, `usize`, or `bool`, no floats involved.
// Four bools, and the lint is worth answering rather than waving
// through. Three of them (`selected`, `disabled`, `has_children`) are
// genuinely independent, simultaneously-settable facts about a row, the
// same shape `shortcut::Modifiers` already carries this allow for.
// `expanded` is the honest exception: it means nothing unless
// `has_children`, and `Option<bool>` -- exactly `accesskit`'s own
// `is_expanded()` shape -- would make that unrepresentable rather than
// merely documented. It is kept as a plain `bool` because these two are
// the caller-facing fields the widget's whole API is specified in terms
// of (`insert_tree_item`'s `has_children` argument,
// `set_tree_item_expanded`), and the coupling is enforced in the one
// place that consumes it: `node()` never emits `expanded` for a leaf,
// asserted by `a_leaf_row_declares_no_expanded_state_and_no_expand_
// collapse_action` and `collapsing_a_leaf_leaves_its_node_leaf_shaped`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeItemState {
    /// This row's own accessible name. Reaches the accessibility node
    /// and nothing else — see this module's own doc comment on why no
    /// pixels come of it yet.
    pub label: String,
    /// How deep this row sits, **0-based** (a top-level row is `0`).
    /// Derived by [`insert_tree_item`] from the row's real parent, not
    /// supplied by the caller — two rows that are siblings in the
    /// [`WidgetTree`] cannot disagree about their depth.
    pub depth: usize,
    /// Whether this row's children are currently shown. Meaningless
    /// (and never announced) when `has_children` is `false` — see this
    /// module's own doc comment.
    pub expanded: bool,
    pub selected: bool,
    pub disabled: bool,
    /// Whether this row is a group at all — **caller-declared**, not
    /// derived from whether it currently has child widgets, because a
    /// collapsed group has none and must still announce "collapsed"
    /// rather than "leaf".
    pub has_children: bool,
}

fn node(state: &TreeItemState) -> Node {
    let mut node = Node::new(Role::TreeItem);
    node.set_label(state.label.clone());
    node.set_level(state.depth);
    node.set_selected(state.selected);
    // Only a real group carries this property at all -- a leaf setting
    // `false` here would be announced as "collapsed". See this module's
    // own doc comment.
    if state.has_children {
        node.set_expanded(state.expanded);
    }
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
        // Exactly one of the two, and only for a real group: the
        // Windows adapter refuses the one that wouldn't change
        // anything.
        if state.has_children {
            if state.expanded {
                node.add_action(Action::Collapse);
            } else {
                node.add_action(Action::Expand);
            }
        }
    }
    node
}

/// One row's own layout: a `Column` (its children are the rows nested
/// under it, stacked), full parent width, and a height that is *at
/// least* one row but grows to contain whatever children it has.
///
/// **Indentation is `padding.left`, not an offset applied at paint
/// time.** Each row inherits its ancestors' padding through `taffy`'s
/// own absolute-position accumulation (`WidgetTree::apply_taffy_layout`
/// adds each parent's origin into its children's), so a row at depth
/// `n` lands `n + 1` padding steps in without this module ever
/// computing `depth * indent` itself. That matters beyond tidiness:
/// the indent is then in the row's real `bounds`, so hit-testing and
/// painting agree with it automatically.
///
/// **`padding.top` is one row height, and is load-bearing.** A row's
/// children are laid out inside its own box; with no top padding the
/// first child would occupy exactly the same band as the parent's own
/// label row — `WidgetTree::hit_test` prefers the deeper node, so a
/// click on a parent's own row would select its first child, and that
/// child's selection highlight would paint where the parent's row is.
/// Reserving one row height above the children is what gives the
/// parent's own row somewhere to be.
///
/// **`flex_grow` stays at its `0.0` default, deliberately.**
/// `command_palette::row_style` uses `flex_grow: 1.0` because its rows
/// are *meant* to divide their container's height evenly; a tree row is
/// one line tall regardless of how many siblings share the view, so
/// borrowing that would make every row's height depend on the row
/// count. (`scrollbar::style`'s own doc comment records the same class
/// of mistake, found the hard way.)
fn style(scales: &Scales) -> Style {
    let row = tree_row_height(scales);
    Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        // A childless row gets its height from `padding.top` alone; the
        // minimum states the same "one line tall" invariant
        // independently, which is what `paint::paint_tree_item` clamps
        // a selected parent's own highlight to.
        min_size: Size {
            width: auto(),
            height: length(row),
        },
        // Spelled out rather than `..Default::default()`-ed: a
        // `taffy::Rect<LengthPercentage>` has no `Default` (unlike the
        // `Dimension` rects elsewhere in this crate), so the two unused
        // sides need a real `zero()` of their own.
        padding: taffy::Rect {
            left: length(spacing(scales.spacing.md)),
            right: zero(),
            top: length(row),
            bottom: zero(),
        },
        ..Default::default()
    }
}

/// Adds a tree's own root container — a `Role::Tree` holding
/// [`insert_tree_item`]'s rows, filling `parent` and stacking them
/// top-to-bottom. `label` names the tree itself ("Layers", say) when it
/// has a name of its own.
///
/// The container's payload is a plain [`WidgetKind::Container`]: it has
/// no state, and [`crate::paint_widget`] deliberately paints nothing
/// for it — a tree's own background belongs to whatever panel it sits
/// in, exactly as `insert_command_palette`'s `Role::ListBox` body
/// already works.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Nothing is added when this happens.
pub fn insert_tree_view(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    label: Option<&str>,
) -> Result<WidgetId, WidgetError> {
    let mut node = Node::new(Role::Tree);
    if let Some(label) = label {
        node.set_label(label.to_owned());
    }
    let style = Style {
        flex_direction: FlexDirection::Column,
        size: Size {
            width: percent(1.0_f32),
            height: percent(1.0_f32),
        },
        ..Default::default()
    };
    tree.insert(parent, style, node, WidgetKind::Container)
}

/// Adds a new, enabled, unselected row as the last child of `parent` —
/// which may be a tree's own root container ([`insert_tree_view`]) for
/// a top-level row, or another row for a nested one.
///
/// `depth` is derived, never passed: a row under a
/// [`WidgetKind::TreeItem`] is one deeper than it, and a row under
/// anything else is depth `0`. `has_children` is the caller's
/// declaration that this row is a group — see [`TreeItemState::
/// has_children`] for why it can't be derived. A new group starts
/// `expanded: true`, matching the fact that it has no children hidden
/// away yet.
///
/// When `parent` is itself a row, this marks *it* `has_children = true`
/// and `expanded = true` (a child was just added, and it is really
/// visible), rebuilds its accessibility node, and marks it dirty.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_tree_item(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    has_children: bool,
) -> Result<WidgetId, WidgetError> {
    // `saturating_add`, not `+`: an overflowing add is a panic in a
    // debug build, and this crate denies panics. A tree 2^64 deep is
    // unreachable in practice, but "unreachable in practice" is not the
    // standard this workspace holds itself to.
    let depth = match tree.payload(parent) {
        Some(WidgetKind::TreeItem(parent_state)) => parent_state.depth.saturating_add(1),
        _ => 0,
    };
    let state = TreeItemState {
        label: label.to_owned(),
        depth,
        expanded: true,
        selected: false,
        disabled: false,
        has_children,
    };
    let id = tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::TreeItem(state),
    )?;

    // The parent now really does have a child, and it is really shown.
    if let Some(WidgetKind::TreeItem(parent_state)) = tree.payload_mut(parent) {
        parent_state.has_children = true;
        parent_state.expanded = true;
        let accessibility = match tree.payload(parent) {
            Some(WidgetKind::TreeItem(parent_state)) => node(parent_state),
            _ => unreachable!("parent was just confirmed to be a TreeItem above"),
        };
        tree.set_accessibility(parent, accessibility)?;
        tree.mark_dirty(parent)?;
    }
    Ok(id)
}

/// Expands or collapses `id` (a tree row) — **structurally**, not
/// visually: collapsing removes every child widget beneath `id` from
/// the tree (each cascading through its own whole subtree), so their
/// [`WidgetId`]s are dead afterwards and a caller that wants them back
/// re-inserts them.
///
/// Expanding is only ever a state/accessibility change: this function
/// has nothing to re-insert (it never kept the removed rows), so
/// `set_tree_item_expanded(id, true)` on a row with no children is a
/// deliberate, legal no-op beyond updating the node — which is exactly
/// what a lazily-populated tree wants. See this module's own doc
/// comment.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row.
/// A removal that fails part-way through returns that error too, having
/// removed whatever it already had — `WidgetTree::remove` only fails
/// for an unknown id or the root, neither of which a child of `id` can
/// be.
pub fn set_tree_item_expanded(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    expanded: bool,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        state.expanded = expanded;
        Ok(())
    })?;
    if !expanded {
        // Snapshotted first, deliberately: `children` borrows `tree`
        // immutably for as long as the slice lives, so removing while
        // iterating it doesn't compile -- the same `.to_vec()` shape
        // `command_palette::rebuild_rows` already uses for its own row
        // teardown.
        let children = match tree.children(id) {
            Some(children) => children.to_vec(),
            None => Vec::new(),
        };
        for child in children {
            tree.remove(child)?;
        }
    }
    Ok(())
}

/// Sets whether `id` (a tree row) is selected. Selection is per-row
/// here: nothing in this module enforces "one row at a time", which is
/// an owning widget's own policy (a layers panel allows multi-select, a
/// file picker may not).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row.
pub fn set_tree_item_selected(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    selected: bool,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        state.selected = selected;
        Ok(())
    })
}

/// Sets whether `id` (a tree row) is disabled. A disabled row keeps its
/// own children and its expanded state — it is the row that can't be
/// interacted with, not the subtree.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row.
pub fn set_tree_item_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_tree_item_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut TreeItemState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::TreeItem(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::TreeItem(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a TreeItem above");
    };
    let accessibility = node(state);
    // Two calls, not one, and deliberately so -- the same gap
    // `with_scrollbar_mut` records: `set_accessibility` sets only the
    // per-widget `dirty` flag, while `mark_dirty` is what unions this
    // widget's own bounds into the tree-wide damage region
    // `take_damage` hands a renderer. A row whose selection changed has
    // *new pixels*, so it needs both.
    tree.set_accessibility(id, accessibility)?;
    tree.mark_dirty(id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        TreeItemState, insert_tree_item, insert_tree_view, set_tree_item_disabled,
        set_tree_item_expanded, set_tree_item_selected,
    };
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales, tree_row_height};
    use accesskit::{Action, Role};
    use aurora_core::Rect;
    use taffy::style_helpers::{length, percent};
    use taffy::{Size, Style};

    /// A root sized like a real panel — a percentage resolves against a
    /// definite parent size, and `Style::default()`'s `auto` isn't one
    /// (the same precondition `scrollbar::tests::sized_row` records).
    fn sized_root() -> Style {
        Style {
            size: Size {
                width: length(300.0_f32),
                height: length(200.0_f32),
            },
            ..Default::default()
        }
    }

    fn state_of(tree: &crate::WidgetTree<WidgetKind>, id: accesskit::NodeId) -> TreeItemState {
        match tree.payload(id) {
            Some(WidgetKind::TreeItem(state)) => state.clone(),
            other => unreachable!("expected TreeItem, got {other:?}"),
        }
    }

    #[test]
    fn insert_tree_view_builds_a_role_tree_container() {
        let (mut tree, root) = new_tree(Style::default());
        let view = match insert_tree_view(&mut tree, root, Some("Layers")) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.payload(view), Some(&WidgetKind::Container));
        let Some(accessibility) = tree.accessibility(view) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), Role::Tree);
        assert_eq!(accessibility.label(), Some("Layers"));
    }

    #[test]
    fn an_unnamed_tree_view_carries_no_label_at_all() {
        let (mut tree, root) = new_tree(Style::default());
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(view) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.label(),
            None,
            "an unnamed tree must carry no label, not an empty one"
        );
    }

    #[test]
    fn insert_tree_view_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match insert_tree_view(&mut tree, bogus, None) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn insert_tree_item_rejects_an_unknown_parent() {
        let (mut tree, _root) = new_tree(Style::default());
        let scales = test_scales();
        let bogus = accesskit::NodeId(999);
        match insert_tree_item(&mut tree, bogus, &scales, "x", false) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "a failed insert must add nothing");
    }

    /// The whole point of deriving depth from the real parent: three
    /// generations, nobody passing a number.
    #[test]
    fn depth_is_derived_from_the_real_parent_zero_based() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, parent, label, group| {
            match insert_tree_item(tree, parent, &scales, label, group) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let top = insert(&mut tree, view, "Top", true);
        let middle = insert(&mut tree, top, "Middle", true);
        let leaf = insert(&mut tree, middle, "Leaf", false);

        assert_eq!(state_of(&tree, top).depth, 0);
        assert_eq!(state_of(&tree, middle).depth, 1);
        assert_eq!(state_of(&tree, leaf).depth, 2);
        let Some(accessibility) = tree.accessibility(leaf) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.level(),
            Some(2),
            "accesskit's own level is 0-based, unlike ARIA's aria-level"
        );
    }

    #[test]
    fn a_group_row_announces_expanded_and_offers_only_collapse() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), Role::TreeItem);
        assert_eq!(accessibility.label(), Some("Group"));
        assert_eq!(accessibility.is_expanded(), Some(true));
        assert_eq!(accessibility.is_selected(), Some(false));
        assert!(accessibility.supports_action(Action::Focus));
        assert!(accessibility.supports_action(Action::Click));
        assert!(accessibility.supports_action(Action::Collapse));
        assert!(
            !accessibility.supports_action(Action::Expand),
            "declaring both would declare one the Windows adapter refuses with \
             invalid_operation"
        );
    }

    #[test]
    fn a_collapsed_group_offers_only_expand() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.is_expanded(), Some(false));
        assert!(accessibility.supports_action(Action::Expand));
        assert!(!accessibility.supports_action(Action::Collapse));
    }

    /// The one property a leaf must *not* have. `accesskit_consumer`
    /// advertises the `ExpandCollapse` pattern for every `Role::TreeItem`
    /// regardless, so a leaf setting `expanded = false` would be
    /// announced as "collapsed" and then fail an `Expand()` call.
    #[test]
    fn a_leaf_row_declares_no_expanded_state_and_no_expand_collapse_action() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let leaf = match insert_tree_item(&mut tree, root, &scales, "Leaf", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(leaf) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.is_expanded(),
            None,
            "a leaf must omit `expanded` entirely, not set it to false"
        );
        assert!(!accessibility.supports_action(Action::Expand));
        assert!(!accessibility.supports_action(Action::Collapse));
        // ... and it stays omitted after an unrelated mutation, since
        // `node()` is rebuilt from state on every change.
        if let Err(err) = set_tree_item_selected(&mut tree, leaf, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(leaf) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), None);
        assert_eq!(accessibility.is_selected(), Some(true));
    }

    #[test]
    fn inserting_under_a_leaf_makes_it_a_group_in_both_payload_and_node() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Was a leaf", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!state_of(&tree, row).has_children);

        if let Err(err) = insert_tree_item(&mut tree, row, &scales, "Child", false) {
            unreachable!("{err:?}");
        }
        let state = state_of(&tree, row);
        assert!(state.has_children, "a row with a real child is a group");
        assert!(state.expanded, "the new child is really visible");
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(
            accessibility.is_expanded(),
            Some(true),
            "the parent's node must be rebuilt, not left stale"
        );
        assert!(accessibility.supports_action(Action::Collapse));
    }

    /// A parent that was collapsed and then given a child must not stay
    /// collapsed while showing it — the structure and the announced
    /// state have to agree.
    #[test]
    fn inserting_under_a_collapsed_group_reopens_it() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Child", false) {
            unreachable!("{err:?}");
        }
        assert!(state_of(&tree, group).expanded);
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(true));
    }

    /// Collapse is structural: the children are gone from the tree, not
    /// merely flagged. This is what carries the hierarchy on the two
    /// platforms whose adapters never read `expanded` at all.
    #[test]
    fn collapsing_removes_the_whole_subtree_from_the_widget_tree() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, parent, label, group| {
            match insert_tree_item(tree, parent, &scales, label, group) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let group = insert(&mut tree, root, "Group", true);
        let child = insert(&mut tree, group, "Child", true);
        let grandchild = insert(&mut tree, child, "Grandchild", false);
        assert_eq!(tree.len(), 4);

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(group), "the collapsed row itself stays");
        assert!(!tree.contains(child));
        assert!(
            !tree.contains(grandchild),
            "removal must cascade through the whole subtree, not one level"
        );
        assert_eq!(tree.children(group), Some([].as_slice()));
        assert_eq!(tree.len(), 2);
        assert!(
            state_of(&tree, group).has_children,
            "a collapsed group is still a group -- otherwise it would announce as a leaf"
        );
    }

    /// Expanding never repopulates — this module never kept the removed
    /// rows. A legal no-op, and the behaviour a lazily-populated tree
    /// depends on.
    #[test]
    fn expanding_a_childless_group_updates_only_its_own_state() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_expanded(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        assert!(state_of(&tree, group).expanded);
        assert_eq!(
            tree.children(group),
            Some([].as_slice()),
            "expanding cannot bring back rows this module never kept"
        );
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(true));
    }

    #[test]
    fn set_tree_item_disabled_clears_every_action() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_disabled(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("still exists");
        };
        assert!(accessibility.is_disabled());
        assert!(!accessibility.supports_action(Action::Focus));
        assert!(!accessibility.supports_action(Action::Click));
        assert!(!accessibility.supports_action(Action::Collapse));
        assert_eq!(
            accessibility.is_expanded(),
            Some(true),
            "a disabled group still reports what it is, it just can't be acted on"
        );
    }

    /// Both halves of "dirty", not just the boolean — `set_accessibility`
    /// alone sets the per-widget flag but never widens the tree-wide
    /// damage region a renderer reads through `take_damage`, so a
    /// selection change would update a screen reader and never repaint.
    #[test]
    fn mutating_a_row_widens_the_trees_own_damage_region() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Row", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let bounds = Rect {
            x: 4,
            y: 8,
            width: 200,
            height: 21,
        };
        if let Err(err) = tree.set_bounds(row, bounds) {
            unreachable!("{err:?}");
        }
        tree.take_damage();

        if let Err(err) = set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(row), Some(true));
        assert_eq!(
            tree.take_damage(),
            Some(bounds),
            "a selection change must widen the damage region to the row's own bounds, not \
             only set its per-widget dirty flag"
        );
    }

    #[test]
    fn tree_item_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        for result in [
            set_tree_item_selected(&mut tree, root, true),
            set_tree_item_disabled(&mut tree, root, true),
            set_tree_item_expanded(&mut tree, root, false),
        ] {
            match result {
                Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
                other => unreachable!("expected WrongWidgetKind, got {other:?}"),
            }
        }
    }

    #[test]
    fn tree_item_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        for result in [
            set_tree_item_selected(&mut tree, bogus, true),
            set_tree_item_disabled(&mut tree, bogus, true),
            set_tree_item_expanded(&mut tree, bogus, false),
        ] {
            match result {
                Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
                other => unreachable!("expected UnknownWidget, got {other:?}"),
            }
        }
    }

    /// Collapsing a row that is *not* a declared group is still legal —
    /// nothing in this module refuses it, and it must not corrupt the
    /// node (a leaf never announces `expanded` at all, whatever the
    /// flag says).
    #[test]
    fn collapsing_a_leaf_leaves_its_node_leaf_shaped() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let leaf = match insert_tree_item(&mut tree, root, &scales, "Leaf", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_expanded(&mut tree, leaf, false) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(leaf) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), None);
        assert!(!accessibility.supports_action(Action::Expand));
    }

    /// The real resolved-layout proof, through `compute_layout` rather
    /// than read off `style()`: one row is one line tall, and each
    /// generation is indented one `spacing.md` step further in than its
    /// parent — accumulated by `taffy` from `padding.left`, never
    /// computed as `depth * indent` anywhere in this crate.
    #[test]
    fn rows_are_one_line_tall_and_each_level_indents_one_step_further() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, parent, label, group| {
            match insert_tree_item(tree, parent, &scales, label, group) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let top = insert(&mut tree, view, "Top", true);
        let child = insert(&mut tree, top, "Child", false);
        tree.compute_layout(300.0, 200.0);

        let (Some(view_bounds), Some(top_bounds), Some(child_bounds)) =
            (tree.bounds(view), tree.bounds(top), tree.bounds(child))
        else {
            unreachable!("just laid out");
        };
        assert_eq!(view_bounds.width, 300, "the tree fills its parent");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row_height = tree_row_height(&scales) as u32;
        assert_eq!(row_height, 21, "13px of type plus 4px above and below");
        assert_eq!(
            child_bounds.height, row_height,
            "a childless row is exactly one line tall"
        );
        assert_eq!(
            top_bounds.height,
            row_height * 2,
            "a group's own box is its own row plus its one child"
        );
        assert_eq!(
            child_bounds.x - top_bounds.x,
            16,
            "one spacing.md step further in than its parent, from padding.left alone"
        );
        assert_eq!(
            child_bounds.x + i64::from(child_bounds.width),
            top_bounds.x + i64::from(top_bounds.width),
            "an indented row still runs flush to its parent's right edge"
        );
    }

    /// `padding.top` is what gives a parent's own row somewhere to be.
    /// Without it the first child would occupy exactly the parent's own
    /// band, and `hit_test` (which prefers the deeper node) would hand a
    /// click on the parent to that child instead.
    #[test]
    fn a_childs_row_sits_below_its_parents_own_row_not_on_top_of_it() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match insert_tree_item(&mut tree, view, &scales, "Top", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match insert_tree_item(&mut tree, top, &scales, "Child", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 200.0);

        let (Some(top_bounds), Some(child_bounds)) = (tree.bounds(top), tree.bounds(child)) else {
            unreachable!("just laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row_height = i64::from(tree_row_height(&scales) as u32);
        assert_eq!(
            child_bounds.y,
            top_bounds.y + row_height,
            "the child starts exactly one row below its parent's own row"
        );
        // The parent's own strip really is the parent's: a point in it
        // must not hit-test into the child.
        #[allow(clippy::cast_precision_loss)]
        let parent_strip = (
            (top_bounds.x + i64::from(top_bounds.width) / 2) as f32,
            (top_bounds.y + row_height / 2) as f32,
        );
        assert_eq!(tree.hit_test(parent_strip), Some(top));
        #[allow(clippy::cast_precision_loss)]
        let child_strip = (
            (child_bounds.x + i64::from(child_bounds.width) / 2) as f32,
            (child_bounds.y + row_height / 2) as f32,
        );
        assert_eq!(tree.hit_test(child_strip), Some(child));
    }

    /// Rows stack, they don't divide the available height between them
    /// — the `flex_grow: 1.0` mistake `style()`'s own doc comment names.
    /// Two siblings in a 200px-tall tree must be 21px each, not 100px
    /// each.
    #[test]
    fn siblings_stay_one_line_tall_rather_than_dividing_the_trees_height() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, label| match insert_tree_item(
            tree, view, &scales, label, false,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let first = insert(&mut tree, "First");
        let second = insert(&mut tree, "Second");
        tree.compute_layout(300.0, 200.0);

        let (Some(first_bounds), Some(second_bounds)) = (tree.bounds(first), tree.bounds(second))
        else {
            unreachable!("just laid out");
        };
        assert_eq!(first_bounds.height, 21);
        assert_eq!(second_bounds.height, 21);
        assert_eq!(
            second_bounds.y,
            first_bounds.y + 21,
            "siblings stack directly, no gap or overlap"
        );
    }

    /// The accessibility tree a real adapter would receive has to be
    /// structurally valid — the exact check that caught a real crash on
    /// real macOS hardware once (`WidgetTree::accessibility_update`'s
    /// own doc comment). Run after a collapse specifically, since that
    /// is this module's one operation that removes nodes.
    #[test]
    fn a_collapsed_tree_still_produces_an_update_accesskit_consumer_accepts() {
        let (mut tree, root) = new_tree(Style {
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, Some("Layers")) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, parent, label, group| {
            match insert_tree_item(tree, parent, &scales, label, group) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let group = insert(&mut tree, view, "Group", true);
        insert(&mut tree, group, "Child", false);
        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }

        let update = tree.accessibility_update(group);
        let _consumer_tree = accesskit_consumer::Tree::new(update, true);
    }
}
