//! A tree: a `Role::Tree` container holding real, nested
//! `Role::TreeItem` rows, each with its own depth, label, selection,
//! and — for a row that declares children — an expanded/collapsed
//! state that is kept in lockstep with the tree's actual structure.
//!
//! **Scope, stated honestly.** Five things a finished tree widget has
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
//! - **No in-row content.** A row has no content container of its own:
//!   its `padding.top` band is where its *own* line would be drawn, and
//!   anything inserted under a row becomes a child *row*, one level
//!   deeper, not something sitting beside the row's label. So the
//!   motivating Layers-panel shape — thumbnail, visibility checkbox,
//!   and name on one line — cannot be built here yet. Closing that
//!   needs a real per-row content box (the row becoming a `Row` of
//!   [content | children] rather than a `Column` of [own band |
//!   children]), which is a redesign of [`style`], [`insert_tree_item`]
//!   and `paint::paint_tree_item` together, not a local fix. What *is*
//!   fixed is the silent data loss it used to cause:
//!   [`set_tree_item_expanded`]`(id, false)` removes only the tree rows
//!   beneath `id`, never a widget of some other kind a caller put there.
//! - **One tab stop per row, not per tree.** Every enabled row declares
//!   `Action::Focus`, so `FocusManager` treats a 500-row tree as 500 tab
//!   stops, where the conventional pattern is one stop on the tree plus
//!   arrow-key navigation between rows (`accesskit`'s own
//!   active-descendant shape). Choosing between those two is an
//!   architectural decision about this crate's whole focus model, not a
//!   tree-local one, so it is named here rather than quietly settled —
//!   the same way `Scrollbar` disclosed that only the Windows adapter
//!   consumes its expanded state.
//!
//! **`WidgetTree`'s own traversals are unbounded recursion**
//! (`paint_order`/`collect_paint_order`, `build_taffy_node`,
//! `hit_test_from`, `remove_subtree`, `apply_taffy_layout`), and this
//! is the first widget in the crate that *invites* deep nesting — a
//! layers panel over a deeply grouped document is exactly the shape
//! that would find it. Measured: `compute_layout` overflows the stack
//! and **aborts the process** (`SIGABRT`, which no `Result` and no
//! `panic = "deny"` lint can catch) somewhere between depth 1100 and
//! 1200 in a debug build, and between 3000 and 4000 in release.
//!
//! Fixing that properly is still out of scope — it is a property of
//! `tree.rs`, shared by every widget, and all five sites would have to
//! be converted to explicit stacks together. What this module does
//! instead is refuse to *build* a tree deep enough to reach it:
//! [`insert_tree_item`] returns [`WidgetError::TreeTooDeep`] beyond
//! [`MAX_TREE_DEPTH`] — a margin of about 4× below the shallowest
//! measured abort (debug), an order of magnitude below the release
//! figure, and two orders above any real document's group nesting.
//!
//! # The accessibility vocabulary, and which platforms actually read it
//!
//! [`node`] sets `Role::TreeItem`, `set_label`, `set_level(depth)`, and
//! `set_selected`, plus `set_description` when the row has one
//! ([`TreeItemState::description`] — a second line announced *alongside*
//! the label, absent entirely rather than empty when there is nothing to
//! add). **`level` is 0-based here** — `accesskit`'s own
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
//! [`set_tree_item_expanded`]`(id, false)` removes the **tree rows**
//! beneath `id` from the [`WidgetTree`] outright (cascading through each
//! whole subtree, via `WidgetTree::remove`), and the caller re-inserts
//! them on expand — the same "real tree nodes, not a hidden flag" shape
//! `command_palette`'s own open/close already uses. Two consequences a
//! caller must know: a collapsed row's descendant *rows* no longer
//! exist, so their [`WidgetId`]s are dead; and expanding a row does
//! **not** repopulate it — [`set_tree_item_expanded`]`(id, true)` on a
//! row with no children is a deliberate, legal no-op that only updates
//! the accessibility node, which is exactly what a lazily-populated tree
//! needs.
//!
//! **Rows, not "everything underneath".** A child of some other kind (a
//! `Checkbox`, a `ColorSwatch`, a plain container) is left exactly where
//! it is: collapse is a statement about a group's *rows*, and silently
//! deleting a widget a caller deliberately parented to a row would be
//! data loss reachable straight through this module's own validated
//! public API. Rows nested *inside* such a container are still removed,
//! since they really are descendant rows of the collapsed group — the
//! same "look through plain containers, stop at a `Role::Tree`"
//! traversal [`insert_tree_item`] uses to derive depth.
//!
//! `has_children` is **caller-declared** at insert time, not derived
//! from `tree.children(id).is_empty()`: a collapsed group has no rows in
//! the tree and must still announce itself as a group rather than as a
//! leaf. The *expanded* half is the opposite, and deliberately so —
//! [`node`] announces `expanded` only when the row both claims to be
//! expanded and really has rows under it right now. A group whose rows
//! were destroyed by a collapse and never repopulated would otherwise
//! announce "expanded" over nothing, and offer a `Collapse` action that
//! could never change anything. [`insert_tree_item`] maintains the
//! stored flags in the one direction it can — inserting a row under a
//! row (however many plain containers deep) makes that ancestor
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

/// The deepest a row may be nested: the largest legal
/// [`TreeItemState::depth`], so at most `MAX_TREE_DEPTH + 1` rows in one
/// chain. Beyond it [`insert_tree_item`] returns
/// [`WidgetError::TreeTooDeep`] rather than building a tree that aborts
/// the process during layout — see this module's own doc comment for the
/// measured thresholds (1100–1200 debug, 3000–4000 release) and for why
/// the underlying recursion is not what this cap fixes.
///
/// `255` is chosen to be uninteresting from both directions: about 4×
/// below the shallowest measured abort (debug; an order of magnitude
/// below the release figure), and two orders above any real document's
/// group nesting (Photoshop's own PSD group nesting does not approach
/// it, and a human cannot navigate 255 levels of indent in a panel
/// anyway). That margin is a checked fact, not just a remembered
/// measurement: `a_tree_at_the_maximum_depth_survives_every_traversal`
/// builds a real chain at exactly this depth and drives every traversal
/// this module's doc comment names — `compute_layout`, `paint_order`,
/// `hit_test`, `accessibility_update` — over it.
pub const MAX_TREE_DEPTH: usize = 255;

/// One row of a tree. `Eq`, unlike most state types here — every field
/// is a `String`, `usize`, or `bool`, no floats involved.
// Four bools, and the lint is worth answering rather than waving
// through. Three of them (`selected`, `disabled`, `has_children`) are
// genuinely independent, simultaneously-settable facts about a row.
// (`shortcut::Modifiers` carries the same allow, but it is *not* the
// precedent for this one: all four of its bools are fully independent,
// which is exactly what isn't true here.) `expanded` is the honest
// exception: it means nothing unless `has_children`, and
// `Option<bool>` -- exactly `accesskit`'s own `is_expanded()` shape --
// would make that unrepresentable rather than merely documented. It is
// kept as a plain `bool` because these two are the caller-facing fields
// the widget's whole API is specified in terms of (`insert_tree_item`'s
// `has_children` argument, `set_tree_item_expanded`), and the coupling
// is enforced at both ends rather than only documented: `insert_tree_
// item` never stores `expanded: true` on a row that isn't a group, and
// `node()` never emits `expanded` for a leaf -- asserted by
// `a_leaf_row_declares_no_expanded_state_and_no_expand_collapse_action`,
// `collapsing_a_leaf_leaves_its_node_leaf_shaped`, and
// `a_fresh_leaf_stores_expanded_false_rather_than_a_meaningless_true`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeItemState {
    /// This row's own accessible name. Reaches the accessibility node
    /// and nothing else — see this module's own doc comment on why no
    /// pixels come of it yet.
    pub label: String,
    /// A second line of accessible detail, announced **alongside** the
    /// label rather than instead of it (`accesskit`'s own `description`
    /// property — a layers panel's "Multiply, 80%, hidden" next to the
    /// layer's own name). `None` when the row has nothing to add, and
    /// no property is emitted at all in that case rather than an empty
    /// string, the same shape [`super::ScrollbarState::label`] uses.
    ///
    /// **Unsanitized, exactly like [`Self::label`]** — this crate
    /// bounds neither, and a description built from document data is
    /// the caller's to bound before it gets here (`aurora-ui`'s Layers
    /// panel does that for the label via
    /// `aurora_doc::sanitize_display_name`).
    ///
    /// **Why it lives in state at all**, rather than the caller setting
    /// it on the node after [`insert_tree_item`] returns: `refresh_node`
    /// rebuilds the *whole* node from this struct on every mutation, so
    /// an externally-set description survives only until the next one —
    /// and that is sooner than "eventually". [`insert_tree_item`] itself
    /// calls `refresh_node(ancestor)` whenever a child row is inserted,
    /// so a parent row's externally-set description would be destroyed
    /// by populating its own children, before the caller had even
    /// finished building the tree.
    pub description: Option<String>,
    /// How deep this row sits, **0-based** (a top-level row is `0`).
    /// Derived by [`insert_tree_item`] from the row's real parent, not
    /// supplied by the caller — two rows that are siblings in the
    /// [`WidgetTree`] cannot disagree about their depth.
    pub depth: usize,
    /// Whether this row's children are currently shown. Meaningless
    /// (and never announced) when `has_children` is `false` — see this
    /// module's own doc comment. This is the row's *intent*; what gets
    /// announced is this **and** whether the row really has rows under
    /// it right now, so a group emptied by a collapse announces
    /// "collapsed" until something repopulates it.
    pub expanded: bool,
    pub selected: bool,
    pub disabled: bool,
    /// Whether this row is a group at all — **caller-declared**, not
    /// derived from whether it currently has child widgets, because a
    /// collapsed group has none and must still announce "collapsed"
    /// rather than "leaf".
    pub has_children: bool,
}

/// This row's accessibility node. `showing_children` is whether the row
/// really has tree rows beneath it *right now* (`child_tree_rows`), and
/// is not the same question as `state.expanded`: a group that was
/// collapsed — which destroys its rows — and then re-expanded without
/// anything repopulating it intends to be expanded and has nothing to
/// show. Announcing `expanded` there would tell a screen reader to read
/// out an open container with no contents, and would offer a `Collapse`
/// that removes nothing and cannot be undone by the matching `Expand`.
/// So the announced state is the conjunction, and the caller's stored
/// intent is kept intact in [`TreeItemState`] for when real rows arrive.
fn node(state: &TreeItemState, showing_children: bool) -> Node {
    let expanded = state.expanded && showing_children;
    let mut node = Node::new(Role::TreeItem);
    node.set_label(state.label.clone());
    // Only when there is one -- an absent description is no property at
    // all, not an empty one, the same way `label`/`ScrollbarState::label`
    // are handled elsewhere in this crate.
    if let Some(description) = &state.description {
        node.set_description(description.clone());
    }
    node.set_level(state.depth);
    node.set_selected(state.selected);
    // Only a real group carries this property at all -- a leaf setting
    // `false` here would be announced as "collapsed". See this module's
    // own doc comment.
    if state.has_children {
        node.set_expanded(expanded);
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
            if expanded {
                node.add_action(Action::Collapse);
            } else {
                node.add_action(Action::Expand);
            }
        }
    }
    node
}

/// The nearest tree rows beneath `id`: its own [`WidgetKind::TreeItem`]
/// children, plus any found by looking *through* a child of some other
/// kind (a plain container a caller wrapped rows in). Never descends
/// into a row it has already found — those are that row's own children,
/// not `id`'s — and never past a nested `Role::Tree`, which begins a
/// tree of its own.
///
/// Iterative rather than recursive on purpose: this module's own doc
/// comment records that `tree.rs`'s five existing recursive traversals
/// abort the process on a deep enough tree, and adding a sixth would be
/// making that worse while claiming to fix it.
///
/// One function for two callers, deliberately — [`node`] asks "is this
/// group really showing anything?" and [`set_tree_item_expanded`] asks
/// "what does collapsing remove?", and those two answers disagreeing is
/// exactly how a row ends up announcing a state its structure doesn't
/// have.
fn child_tree_rows(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Vec<WidgetId> {
    let mut found = Vec::new();
    let mut pending: Vec<WidgetId> = match tree.children(id) {
        Some(children) => children.to_vec(),
        None => return found,
    };
    while let Some(candidate) = pending.pop() {
        if matches!(tree.payload(candidate), Some(WidgetKind::TreeItem(_))) {
            found.push(candidate);
            continue;
        }
        if is_tree_root(tree, candidate) {
            continue;
        }
        if let Some(children) = tree.children(candidate) {
            pending.extend_from_slice(children);
        }
    }
    found
}

/// The nearest enclosing tree row at or above `id`, looking *through*
/// plain containers — a caller may legally wrap rows in one, and before
/// this walk existed a single `WidgetKind::Container` between two rows
/// silently reset the deeper one's depth to `0` and left the outer row
/// claiming to be a childless leaf while a child row was visibly
/// indented beneath it. Stops at a nested `Role::Tree`, whose rows
/// belong to that tree rather than to anything above it.
fn enclosing_tree_item(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> Option<WidgetId> {
    let mut current = id;
    loop {
        match tree.payload(current) {
            Some(WidgetKind::TreeItem(_)) => return Some(current),
            Some(_) => {}
            None => return None,
        }
        if is_tree_root(tree, current) {
            return None;
        }
        current = tree.parent(current)?;
    }
}

/// Whether `id` is a tree's own root container ([`insert_tree_view`]).
/// Asked of the `accesskit` role rather than the payload because
/// `insert_tree_view` deliberately stores a plain
/// [`WidgetKind::Container`] — the `Role::Tree` node *is* the only thing
/// that distinguishes it from any other container.
fn is_tree_root(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    tree.accessibility(id).map(Node::role) == Some(Role::Tree)
}

/// Rebuilds `id`'s accessibility node from its current state and its
/// current real structure, and marks it dirty — the one place that
/// knows [`node`] needs both.
fn refresh_node(tree: &mut WidgetTree<WidgetKind>, id: WidgetId) -> Result<(), WidgetError> {
    let showing_children = !child_tree_rows(tree, id).is_empty();
    let Some(WidgetKind::TreeItem(state)) = tree.payload(id) else {
        return Err(WidgetError::WrongWidgetKind(id));
    };
    let accessibility = node(state, showing_children);
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

/// One row's own layout: a `Column` (its children are the rows nested
/// under it, stacked), full parent width, and a height that is *at
/// least* one row but grows to contain whatever children it has.
///
/// **Indentation is `padding.left`, not an offset applied at paint
/// time.** Each row inherits its ancestors' padding through `taffy`'s
/// own absolute-position accumulation (`WidgetTree::apply_taffy_layout`
/// adds each parent's origin into its children's), so a row at depth
/// `n` lands `n` padding steps in without this module ever computing
/// `depth * indent` itself. (`n`, not `n + 1`: a row's own box starts
/// where its parent's *padding* ends, and its own padding indents its
/// children rather than itself. The extra step would only ever apply to
/// content drawn inside a row's own content box, and this module draws
/// none — see "No in-row content" in the module doc comment.) That
/// matters beyond tidiness: the indent is then in the row's real
/// `bounds`, so hit-testing and painting agree with it automatically.
///
/// **`min_size.width` is the floor under that same indent.** The width
/// is `percent(1.0)` of the parent's *content* box, so every level costs
/// one `spacing.md` step: a row at depth `n` in a `W`-wide panel
/// resolves to `W - n * spacing.md`, which reaches **zero** at about
/// `W / 16` — twelve or thirteen rows deep in a 200 px panel, a depth a
/// deeply grouped document really reaches. A zero-width row is not a
/// small row, it is a degenerate layout box: it paints nothing at all,
/// can never be hit, and still consumes a full row of vertical space —
/// a phantom the user can see the effect of and not the row. The floor
/// keeps the box well-formed. One row height is the value: no new token
/// is invented for it (a "minimum row width" would be a design
/// decision, not an engineering default — CLAUDE.md), and a square of
/// the row's own height is the smallest thing that is still a real,
/// grabbable target.
///
/// Stated honestly, because the floor is a floor and not a cure: a row
/// indented that far starts at or past its panel's own right edge
/// (`x = n * spacing.md`, and every row runs flush to the panel's right
/// edge), so it lies outside the panel whether it is 0 or 21 px wide,
/// and `WidgetTree::hit_test` will not reach the part of a row that
/// overhangs its own parent. Making a deep row genuinely usable needs
/// horizontal scrolling and a clipping container — the same missing
/// piece this module's own doc comment already names for vertical
/// overflow — or an indent model that doesn't spend width. What the
/// floor buys today is that nothing in the tree is ever a zero-size box.
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
            width: length(row),
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
/// `depth` is derived, never passed: a row is one deeper than the
/// nearest enclosing row, looking *through* any plain containers between
/// them (`enclosing_tree_item`), and a row with no enclosing row at
/// all is depth `0`. `has_children` is the caller's declaration that
/// this row is a group — see [`TreeItemState::has_children`] for why it
/// can't be derived. A new **group** starts `expanded: true` (it is not
/// hiding anything yet); a new **leaf** starts `expanded: false`, since
/// storing `true` there would be a flag that contradicts
/// `has_children: false` and means nothing either way.
///
/// When there is an enclosing row, this marks *it* `has_children = true`
/// and `expanded = true` (a child was just added, and it is really
/// visible), rebuilds its accessibility node, and marks it dirty.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist, or
/// [`WidgetError::TreeTooDeep`] if the new row would sit deeper than
/// [`MAX_TREE_DEPTH`] — see this module's own doc comment for the
/// measured process abort that cap exists to stay away from. Nothing is
/// added when either happens.
pub fn insert_tree_item(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: &str,
    has_children: bool,
) -> Result<WidgetId, WidgetError> {
    if !tree.contains(parent) {
        return Err(WidgetError::UnknownWidget(parent));
    }
    let ancestor = enclosing_tree_item(tree, parent);
    // `saturating_add`, not `+`: an overflowing add is a panic in a
    // debug build, and this crate denies panics. `MAX_TREE_DEPTH` makes
    // the overflow unreachable anyway, and "unreachable" is not the
    // standard this workspace holds itself to.
    let depth = match ancestor.and_then(|id| tree.payload(id)) {
        Some(WidgetKind::TreeItem(parent_state)) => parent_state.depth.saturating_add(1),
        _ => 0,
    };
    if depth > MAX_TREE_DEPTH {
        return Err(WidgetError::TreeTooDeep {
            parent,
            depth,
            max: MAX_TREE_DEPTH,
        });
    }
    let state = TreeItemState {
        label: label.to_owned(),
        // Not a sixth parameter: most rows have nothing to add, and
        // `set_tree_item_description` is the one way to set it, which
        // keeps the field's "rebuilt from state, never set on the node
        // directly" rule true from the only entry point there is.
        description: None,
        depth,
        // Not unconditionally `true`: see this function's own doc
        // comment, and `TreeItemState`'s.
        expanded: has_children,
        selected: false,
        disabled: false,
        has_children,
    };
    let id = tree.insert(
        parent,
        style(scales),
        // A freshly inserted row has no children of its own yet, so it
        // is showing none -- a group inserted here announces itself
        // collapsed until something is really put under it.
        node(&state, false),
        WidgetKind::TreeItem(state),
    )?;

    // The enclosing row now really does have a descendant row, and it is
    // really shown.
    if let Some(ancestor) = ancestor {
        if let Some(WidgetKind::TreeItem(ancestor_state)) = tree.payload_mut(ancestor) {
            ancestor_state.has_children = true;
            ancestor_state.expanded = true;
        }
        refresh_node(tree, ancestor)?;
    }
    Ok(id)
}

/// Expands or collapses `id` (a tree row) — **structurally**, not
/// visually: collapsing removes the tree rows beneath `id` from the tree
/// (each cascading through its own whole subtree), so their
/// [`WidgetId`]s are dead afterwards and a caller that wants them back
/// re-inserts them.
///
/// **Only rows.** A child of some other kind stays exactly where it is;
/// see this module's own doc comment for why silently deleting one would
/// be data loss reachable through this function's own validated API.
///
/// Expanding is only ever a state/accessibility change: this function
/// has nothing to re-insert (it never kept the removed rows), so
/// `set_tree_item_expanded(id, true)` on a row with no children is a
/// deliberate, legal no-op beyond updating the node — which is exactly
/// what a lazily-populated tree wants. It does **not** make the row
/// announce itself as expanded: with nothing under it, that would be an
/// open container with no contents. See this module's own doc comment.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row,
/// or [`WidgetError::WidgetDisabled`] if the row is disabled — the same
/// "a disabled widget refuses the interaction, not merely the paint"
/// rule `set_scrollbar_value` and `toggle_checkbox` already follow. A
/// caller driving state rather than replaying a gesture re-enables the
/// row first. A removal that fails part-way through returns that error
/// too, having removed whatever it already had — `WidgetTree::remove`
/// only fails for an unknown id or the root, neither of which a
/// descendant of `id` can be.
pub fn set_tree_item_expanded(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    expanded: bool,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        state.expanded = expanded;
        Ok(())
    })?;
    if !expanded {
        // Snapshotted first, deliberately: `children` borrows `tree`
        // immutably for as long as the slice lives, so removing while
        // iterating it doesn't compile -- the same `.to_vec()` shape
        // `command_palette::rebuild_rows` already uses for its own row
        // teardown. `child_tree_rows` is already an owned `Vec`, and is
        // what keeps a non-row child out of this loop.
        for row in child_tree_rows(tree, id) {
            tree.remove(row)?;
        }
        // Recomputed now that the rows are really gone: `node` reads
        // real structure, and the node built inside `with_tree_item_mut`
        // above saw the pre-removal tree. Cheap, and it means this
        // function never leaves a node describing a structure that no
        // longer exists.
        refresh_node(tree, id)?;
    }
    Ok(())
}

/// Renames `id` (a tree row). Routed through the same path every other
/// mutator here uses rather than left to [`WidgetTree::payload_mut`]:
/// a label edited directly reaches neither the accessibility node nor
/// the damage region, so a screen reader would keep announcing the old
/// name and the row would keep its old pixels. A layers panel renames
/// rows more often than it changes any other field, which is exactly why
/// the unsanctioned path was worth closing.
///
/// Unlike [`set_tree_item_selected`] and [`set_tree_item_expanded`] this
/// is allowed on a **disabled** row: a rename is an owner-driven change
/// to what the row *is*, not a user gesture the disabled state exists to
/// refuse — a locked layer still gets renamed when the document renames
/// it.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row.
pub fn set_tree_item_label(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    label: &str,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        label.clone_into(&mut state.label);
        Ok(())
    })
}

/// Sets (or, with `None`, clears) `id`'s second line of accessible
/// detail — [`TreeItemState::description`], announced alongside the
/// label rather than in place of it.
///
/// Routed through the same path as every other mutator here for the
/// same reason [`set_tree_item_label`] is, and it is the *only*
/// supported way to set this: a description written straight onto the
/// accessibility node is destroyed by the next `refresh_node`, which
/// [`insert_tree_item`] triggers on a row the moment a child is
/// inserted under it. See [`TreeItemState::description`].
///
/// Allowed on a **disabled** row, matching [`set_tree_item_label`]
/// rather than [`set_tree_item_selected`]: describing what a row *is*
/// is owner-driven, not a user gesture the disabled state exists to
/// refuse.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row.
pub fn set_tree_item_description(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    description: Option<&str>,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        state.description = description.map(ToOwned::to_owned);
        Ok(())
    })
}

/// Sets whether `id` (a tree row) is selected. Selection is per-row
/// here: nothing in this module enforces "one row at a time", which is
/// an owning widget's own policy (a layers panel allows multi-select, a
/// file picker may not).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a tree row,
/// or [`WidgetError::WidgetDisabled`] if the row is disabled — see
/// [`set_tree_item_expanded`] for the precedent that policy follows, and
/// [`set_tree_item_label`] for the one mutation it deliberately does not
/// cover. Select a row *before* disabling it (which is what an owner
/// building a disabled-but-selected row does anyway), not after.
pub fn set_tree_item_selected(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    selected: bool,
) -> Result<(), WidgetError> {
    with_tree_item_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
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
    refresh_node(tree, id)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TREE_DEPTH, TreeItemState, insert_tree_item, insert_tree_view,
        set_tree_item_description, set_tree_item_disabled, set_tree_item_expanded,
        set_tree_item_label, set_tree_item_selected,
    };
    use crate::WidgetError;
    use crate::widgets::{
        WidgetKind, insert_checkbox, insert_color_swatch, insert_container, new_tree, test_scales,
        tree_row_height,
    };
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
        // A real child, because "expanded" is announced only over real
        // rows -- see `an_empty_group_announces_collapsed_however_its_
        // own_flag_reads`.
        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Child", false) {
            unreachable!("{err:?}");
        }
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
    /// depends on. What it must *not* do is announce the row as
    /// expanded: a screen reader would read out an open container with
    /// nothing in it, and the `Collapse` action it would then offer
    /// removes nothing, so Collapse-then-Expand would loop forever
    /// changing nothing.
    #[test]
    fn re_expanding_an_emptied_group_announces_collapsed_until_rows_come_back() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Child", false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_expanded(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        assert!(
            state_of(&tree, group).expanded,
            "the caller's own intent is kept -- it is the announcement that is derived"
        );
        assert_eq!(
            tree.children(group),
            Some([].as_slice()),
            "expanding cannot bring back rows this module never kept"
        );
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("still exists");
        };
        assert_eq!(
            accessibility.is_expanded(),
            Some(false),
            "a group with nothing under it must not announce itself expanded"
        );
        assert!(accessibility.supports_action(Action::Expand));
        assert!(
            !accessibility.supports_action(Action::Collapse),
            "there is nothing left to collapse"
        );

        // ... and it flips back the moment real rows exist again.
        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Child again", false) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(true));
        assert!(accessibility.supports_action(Action::Collapse));
    }

    /// The same rule seen from the other end: a group declared at insert
    /// time, before its rows are added, is a group with nothing in it.
    #[test]
    fn an_empty_group_announces_collapsed_however_its_own_flag_reads() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            state_of(&tree, group).expanded,
            "a fresh group intends to be expanded"
        );
        let Some(accessibility) = tree.accessibility(group) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.is_expanded(), Some(false));
        assert!(accessibility.supports_action(Action::Expand));
    }

    /// A leaf's stored `expanded` is `false`, not a meaningless `true`.
    /// The announced shape was already right (`node` never emits the
    /// property for a leaf); this is about the payload a caller reading
    /// [`TreeItemState`] directly actually sees, and about `Eq` over it
    /// not distinguishing two leaves by a field neither of them has.
    #[test]
    fn a_fresh_leaf_stores_expanded_false_rather_than_a_meaningless_true() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let leaf = match insert_tree_item(&mut tree, root, &scales, "Leaf", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let state = state_of(&tree, leaf);
        assert!(!state.has_children);
        assert!(
            !state.expanded,
            "a leaf has nothing to expand, so it must not store that it is expanded"
        );
    }

    #[test]
    fn set_tree_item_disabled_clears_every_action() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Child", false) {
            unreachable!("{err:?}");
        }
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
            set_tree_item_description(&mut tree, root, Some("x")),
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
            set_tree_item_description(&mut tree, bogus, Some("x")),
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

    /// The live crash this round exists for, reproduced end to end:
    /// focus a descendant row, collapse its ancestor (which really
    /// removes the focused widget), and build the update an adapter
    /// would receive. `accesskit_consumer::State::validate_global`
    /// panics with "Focused ID #N is not in the node list" on a focus id
    /// the update doesn't carry, so before
    /// `WidgetTree::accessibility_update`'s fallback existed this test
    /// aborted rather than failed. Nothing here calls
    /// `FocusManager::validate` on purpose — the whole point is that a
    /// caller who forgets to (as `aurora-app`'s own `push_accessibility`
    /// does on this path) still cannot crash a screen reader.
    #[test]
    fn collapsing_the_focused_rows_ancestor_still_produces_a_valid_update() {
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
        let group = match insert_tree_item(&mut tree, view, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match insert_tree_item(&mut tree, group, &scales, "Child", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut focus = crate::FocusManager::new();
        if let Err(err) = focus.focus(&mut tree, child) {
            unreachable!("{err:?}");
        }
        assert_eq!(focus.focused(), Some(child));

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(child), "the focused row is really gone");

        let Some(stale) = focus.focused() else {
            unreachable!("the focus manager still points at the removed row");
        };
        let update = tree.accessibility_update(stale);
        assert_eq!(
            update.focus, root,
            "a focus into the removed subtree must fall back to a live node"
        );
        let _consumer_tree = accesskit_consumer::Tree::new(update, true);
    }

    /// A *multi-child* group, deliberately — the chain in
    /// `collapsing_removes_the_whole_subtree_from_the_widget_tree`
    /// cannot tell a full removal apart from one that stops after the
    /// first child, and the only tests that could were GPU-gated (so
    /// they self-skip on a machine with no adapter, which is this
    /// workspace's ordinary CI state).
    #[test]
    fn collapsing_removes_every_child_subtree_not_only_the_first() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let insert = |tree: &mut crate::WidgetTree<WidgetKind>, parent, label, group| {
            match insert_tree_item(tree, parent, &scales, label, group) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let group = insert(&mut tree, root, "Group", true);
        let first = insert(&mut tree, group, "First", true);
        let first_child = insert(&mut tree, first, "First child", false);
        let second = insert(&mut tree, group, "Second", true);
        let second_child = insert(&mut tree, second, "Second child", false);
        let third = insert(&mut tree, group, "Third", false);
        assert_eq!(tree.len(), 7, "root + group + three subtrees");

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        for gone in [first, first_child, second, second_child, third] {
            assert!(
                !tree.contains(gone),
                "every child subtree must go, not just the first"
            );
        }
        assert_eq!(tree.children(group), Some([].as_slice()));
        assert_eq!(
            tree.len(),
            2,
            "only the root and the collapsed row are left"
        );
    }

    /// Collapse is a statement about a group's *rows*. A widget of some
    /// other kind that a caller deliberately parented to a row is not a
    /// row, and deleting it was silent data loss reachable straight
    /// through this module's own validated public API.
    #[test]
    fn collapsing_removes_rows_and_leaves_every_other_widget_alone() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Layer group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // The Layers-panel shape: a row that also carries its own
        // visibility toggle and colour chip, inserted as ordinary
        // widgets rather than through `insert_tree_item`, so the row's
        // own `has_children` never claimed them.
        let visible = match insert_checkbox(&mut tree, group, &scales, "Visible") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let swatch = match insert_color_swatch(
            &mut tree,
            group,
            &scales,
            aurora_theme::Color { r: 1, g: 2, b: 3 },
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let row = match insert_tree_item(&mut tree, group, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(row), "the child row really is removed");
        assert!(
            tree.contains(visible),
            "a checkbox a caller put on the row must survive its collapse"
        );
        assert!(tree.contains(swatch));
        assert!(
            tree.payload(visible).is_some() && tree.payload(swatch).is_some(),
            "and survive intact, not as empty shells"
        );
        assert_eq!(tree.len(), 4, "root + group + checkbox + swatch");
    }

    /// ... but a plain container is looked *through*: rows nested inside
    /// one really are descendant rows of the collapsed group, so they
    /// go, while the container a caller built stays.
    #[test]
    fn collapsing_looks_through_a_plain_container_to_the_rows_inside_it() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let wrapper = match insert_container(&mut tree, group, Style::default()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let nested = match insert_tree_item(&mut tree, wrapper, &scales, "Nested", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        assert!(tree.contains(wrapper), "the caller's own container stays");
        assert!(
            !tree.contains(nested),
            "a row is a row however it was wrapped"
        );
    }

    /// An ordinary `WidgetKind::Container` between two rows is a legal,
    /// unremarkable thing to insert, and it used to silently reset the
    /// deeper row's depth to `0` while leaving the outer row announcing
    /// itself as a childless leaf — with the child row visibly indented
    /// one step beneath it on screen.
    #[test]
    fn depth_and_group_state_resolve_through_an_intervening_container() {
        let (mut tree, root) = new_tree(sized_root());
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match insert_tree_item(&mut tree, view, &scales, "Top", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let wrapper = match insert_container(&mut tree, top, Style::default()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let nested = match insert_tree_item(&mut tree, wrapper, &scales, "Nested", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(
            state_of(&tree, nested).depth,
            1,
            "a container between two rows is not a new tree"
        );
        let top_state = state_of(&tree, top);
        assert!(
            top_state.has_children,
            "the outer row really does have a row under it"
        );
        assert!(top_state.expanded);
        let Some(accessibility) = tree.accessibility(top) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(true));
        assert!(accessibility.supports_action(Action::Collapse));
        let Some(accessibility) = tree.accessibility(nested) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.level(),
            Some(1),
            "the announced level must match the indent the layout really produces"
        );

        // ... and the indent it announces is the indent it gets.
        tree.compute_layout(300.0, 200.0);
        let (Some(top_bounds), Some(nested_bounds)) = (tree.bounds(top), tree.bounds(nested))
        else {
            unreachable!("just laid out");
        };
        assert_eq!(nested_bounds.x - top_bounds.x, 16);
    }

    /// A nested tree is a tree of its own: its rows belong to it, not to
    /// whatever row the tree happens to sit inside.
    #[test]
    fn a_nested_tree_view_starts_its_own_depth_and_claims_no_outer_parent() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let outer = match insert_tree_item(&mut tree, root, &scales, "Outer", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner_view = match insert_tree_view(&mut tree, outer, Some("Inner")) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner_row = match insert_tree_item(&mut tree, inner_view, &scales, "Inner row", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state_of(&tree, inner_row).depth, 0);
        assert!(
            !state_of(&tree, outer).has_children,
            "a whole tree inside a row is not that row's own child row"
        );
    }

    /// The sanctioned rename. A `payload_mut` label edit updates neither
    /// the accessibility node nor the damage region, so a screen reader
    /// keeps reading the old name and the row keeps its old pixels —
    /// which is exactly what the other setters' own dirty/damage test
    /// asserts, applied to the field a layers panel changes most.
    #[test]
    fn set_tree_item_label_renames_the_node_and_widens_the_damage_region() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
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

        if let Err(err) = set_tree_item_label(&mut tree, row, "Background") {
            unreachable!("{err:?}");
        }
        assert_eq!(state_of(&tree, row).label, "Background");
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.label(), Some("Background"));
        assert_eq!(tree.is_dirty(row), Some(true));
        assert_eq!(tree.take_damage(), Some(bounds));
    }

    /// The description's own sanctioned setter, held to the same
    /// standard as the label's: it must reach the node *and* the damage
    /// region, or a screen reader and the pixels disagree.
    #[test]
    fn set_tree_item_description_reaches_the_node_and_widens_the_damage_region() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.description(),
            None,
            "a fresh row carries no description property at all"
        );
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

        if let Err(err) = set_tree_item_description(&mut tree, row, Some("Multiply, 80%")) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            state_of(&tree, row).description.as_deref(),
            Some("Multiply, 80%")
        );
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.description(), Some("Multiply, 80%"));
        assert_eq!(
            accessibility.label(),
            Some("Layer 1"),
            "a description is announced alongside the label, not instead of it"
        );
        assert_eq!(tree.is_dirty(row), Some(true));
        assert_eq!(tree.take_damage(), Some(bounds));
    }

    /// Why the description has to live in [`TreeItemState`] rather than
    /// being written onto the node after the fact. `refresh_node`
    /// rebuilds the whole node from state on *every* mutation, and
    /// `insert_tree_item` triggers one on a row the moment a child
    /// arrives under it — so an externally-set description would be
    /// destroyed by populating a group's own children, before the caller
    /// had even finished building the panel.
    #[test]
    fn a_description_survives_selection_collapsing_and_a_new_child_row() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Effects", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_description(&mut tree, group, Some("Group")) {
            unreachable!("{err:?}");
        }
        let described = |tree: &crate::WidgetTree<WidgetKind>, step: &str| {
            let Some(accessibility) = tree.accessibility(group) else {
                unreachable!("still exists after {step}");
            };
            assert_eq!(
                accessibility.description(),
                Some("Group"),
                "the description must survive {step}"
            );
        };
        described(&tree, "being set");

        if let Err(err) = set_tree_item_selected(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        described(&tree, "selection");

        if let Err(err) = set_tree_item_expanded(&mut tree, group, false) {
            unreachable!("{err:?}");
        }
        described(&tree, "collapsing");

        if let Err(err) = insert_tree_item(&mut tree, group, &scales, "Glow", false) {
            unreachable!("{err:?}");
        }
        described(&tree, "a new child row's own refresh of its parent");
    }

    #[test]
    fn clearing_a_description_removes_it_from_the_node() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Layer 1", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_description(&mut tree, row, Some("Normal, 100%")) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_description(&mut tree, row, None) {
            unreachable!("{err:?}");
        }
        assert_eq!(state_of(&tree, row).description, None);
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(
            accessibility.description(),
            None,
            "clearing must remove the property, not leave an empty string"
        );
    }

    /// A description is owner-driven, like a rename — a locked layer
    /// still changes blend mode.
    #[test]
    fn set_tree_item_description_is_allowed_on_a_disabled_row() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Locked", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_disabled(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_description(&mut tree, row, Some("Multiply, 80%")) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            state_of(&tree, row).description.as_deref(),
            Some("Multiply, 80%")
        );
    }

    /// A rename is owner-driven, not a user gesture, so it is the one
    /// mutator a disabled row still accepts.
    #[test]
    fn set_tree_item_label_is_allowed_on_a_disabled_row() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let row = match insert_tree_item(&mut tree, root, &scales, "Locked", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_disabled(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_tree_item_label(&mut tree, row, "Locked, renamed") {
            unreachable!("{err:?}");
        }
        assert_eq!(state_of(&tree, row).label, "Locked, renamed");
    }

    /// The two gesture-shaped mutators refuse a disabled row, matching
    /// `set_scrollbar_value` and `toggle_checkbox` rather than leaving
    /// this widget's own policy unstated.
    #[test]
    fn selecting_or_collapsing_a_disabled_row_is_refused() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let group = match insert_tree_item(&mut tree, root, &scales, "Group", true) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match insert_tree_item(&mut tree, group, &scales, "Child", false) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_tree_item_disabled(&mut tree, group, true) {
            unreachable!("{err:?}");
        }
        for result in [
            set_tree_item_selected(&mut tree, group, true),
            set_tree_item_expanded(&mut tree, group, false),
        ] {
            match result {
                Err(WidgetError::WidgetDisabled(id)) => assert_eq!(id, group),
                other => unreachable!("expected WidgetDisabled, got {other:?}"),
            }
        }
        assert!(
            tree.contains(child),
            "a refused collapse must not have removed anything"
        );
        assert!(!state_of(&tree, group).selected);
    }

    /// Indentation eats into a `percent(1.0)` width one `spacing.md`
    /// step per level, so in a narrow panel a deep enough row used to
    /// resolve to *zero* width — a degenerate box that paints nothing,
    /// can never be hit, and still consumes a row of vertical space.
    /// `min_size.width` is the floor under it. (What the floor does not
    /// do is bring an over-indented row back inside its panel; see
    /// `style`'s own doc comment.)
    #[test]
    fn a_deeply_nested_row_in_a_narrow_panel_never_resolves_to_zero_width() {
        let (mut tree, root) = new_tree(Style {
            size: Size {
                width: length(64.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let view = match insert_tree_view(&mut tree, root, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // 64px of panel and 16px of indent per level: without a floor,
        // the fifth row down is already 0 wide.
        let mut parent = view;
        let mut rows = Vec::new();
        for depth in 0..8 {
            parent = match insert_tree_item(&mut tree, parent, &scales, "Row", true) {
                Ok(id) => id,
                Err(err) => unreachable!("depth {depth}: {err:?}"),
            };
            rows.push(parent);
        }
        tree.compute_layout(64.0, 400.0);

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let floor = tree_row_height(&scales) as u32;
        for (depth, &row) in rows.iter().enumerate() {
            let Some(bounds) = tree.bounds(row) else {
                unreachable!("just laid out");
            };
            assert!(
                bounds.width >= floor,
                "a row at depth {depth} resolved to {} px wide, below the {floor} px floor",
                bounds.width
            );
        }
        // Without the floor these would be the arithmetic: 64, 48, 32,
        // 16, and then 0 for every level below. The fifth row down is
        // the one that used to vanish.
        let Some(&fifth) = rows.get(4) else {
            unreachable!("eight rows were just inserted");
        };
        let Some(bounds) = tree.bounds(fifth) else {
            unreachable!("just laid out");
        };
        assert_eq!(bounds.width, floor);
        // And the limitation the floor does *not* remove, pinned rather
        // than implied: this row's indent has already carried it to the
        // panel's own right edge, so it is off-panel and `hit_test`
        // cannot reach it -- that needs clipping and horizontal
        // scrolling, the gap this module's own doc comment names.
        assert_eq!(
            bounds.x, 64,
            "one spacing.md step per level, four levels in"
        );
        #[allow(clippy::cast_precision_loss)]
        let point = (
            (bounds.x + 1) as f32,
            (bounds.y + i64::from(bounds.height) / 2) as f32,
        );
        assert_eq!(
            tree.hit_test(point),
            None,
            "a row indented past its panel is outside it, floored width or not"
        );
    }

    /// `WidgetTree`'s traversals are recursive, and a deep enough tree
    /// aborts the process during layout (measured: between depth 1100
    /// and 1200 in a debug build). `MAX_TREE_DEPTH` is what keeps a
    /// caller from building one through this module's own public API —
    /// a real error, about 4x short of the debug abort (an order of
    /// magnitude short of the release one).
    #[test]
    fn insert_tree_item_refuses_to_nest_past_the_maximum_depth() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let mut parent = root;
        for depth in 0..=MAX_TREE_DEPTH {
            parent = match insert_tree_item(&mut tree, parent, &scales, "Row", false) {
                Ok(id) => id,
                Err(err) => unreachable!("depth {depth} must still be legal: {err:?}"),
            };
        }
        assert_eq!(state_of(&tree, parent).depth, MAX_TREE_DEPTH);
        let before = tree.len();

        match insert_tree_item(&mut tree, parent, &scales, "One too deep", false) {
            Err(WidgetError::TreeTooDeep {
                parent: at,
                depth,
                max,
            }) => {
                assert_eq!(at, parent);
                assert_eq!(depth, MAX_TREE_DEPTH + 1);
                assert_eq!(max, MAX_TREE_DEPTH);
            }
            other => unreachable!("expected TreeTooDeep, got {other:?}"),
        }
        assert_eq!(tree.len(), before, "a refused insert must add nothing");
        assert!(
            !state_of(&tree, parent).has_children,
            "and must not mark its would-be parent as a group either"
        );
    }

    /// `MAX_TREE_DEPTH`'s whole justification is a *measured* margin
    /// below a real abort, not merely an assumption that the cap is
    /// small enough to be safe. This drives every traversal the module
    /// doc names (`compute_layout`, `paint_order`, `hit_test`,
    /// `accessibility_update`) over a tree at exactly that depth, so the
    /// margin is a checked fact rather than a remembered number.
    #[test]
    fn a_tree_at_the_maximum_depth_survives_every_traversal() {
        let (mut tree, root) = new_tree(Style {
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            ..Default::default()
        });
        let scales = test_scales();
        let mut parent = root;
        for depth in 0..=MAX_TREE_DEPTH {
            parent = match insert_tree_item(&mut tree, parent, &scales, "Row", false) {
                Ok(id) => id,
                Err(err) => unreachable!("depth {depth} must still be legal: {err:?}"),
            };
        }
        let deepest = parent;

        // A generous height: MAX_TREE_DEPTH + 1 rows, each one
        // `tree_row_height` tall, is what a real layout of this chain
        // needs — this is the traversal `compute_layout` itself walks
        // recursively (`build_taffy_node`/`apply_taffy_layout`).
        let height = tree_row_height(&scales) * (MAX_TREE_DEPTH as f32 + 1.0);
        tree.compute_layout(2_000.0, height);

        // `paint_order` (`collect_paint_order`) is its own recursion,
        // separate from layout.
        assert_eq!(
            tree.paint_order().len(),
            MAX_TREE_DEPTH + 2,
            "the tree's own root, plus one entry per row in the chain"
        );

        // `hit_test_from` recurses root-to-leaf; a point inside the
        // shallowest row's own strip is unaffected by how far the chain
        // eventually indents, so this exercises the recursion without
        // depending on the panel being wide enough for the deepest row
        // (that width floor is `a_row_indented_past_its_panel_still_has_a_floored_size`'s
        // own, separate concern).
        let Some(top_bounds) = tree.bounds(root) else {
            unreachable!("root must be laid out")
        };
        // `hit_test` prefers the deepest node containing a point (as
        // `a_childs_row_sits_below_its_parents_own_row_not_on_top_of_it`
        // already pins), so this lands on the first row, not the root
        // itself — what matters here is only that the 256-deep descent
        // returns at all rather than overflowing the stack.
        let shallow_point = ((top_bounds.x + 1) as f32, (top_bounds.y + 1) as f32);
        assert!(
            tree.hit_test(shallow_point).is_some(),
            "hit-testing must still terminate at this depth, not just at the root"
        );

        // `accessibility_update`/`accessibility_update`'s own descent
        // (`WidgetTree::accessibility_update`'s doc comment names the
        // real crash this guards against) must produce a tree
        // `accesskit_consumer` accepts as structurally valid, focused on
        // the deepest row specifically.
        let update = tree.accessibility_update(deepest);
        let _consumer_tree = accesskit_consumer::Tree::new(update, true);
    }
}
