//! The retained-mode widget tree: identity, nesting, damage tracking, and
//! a required accessibility node per widget (invariant §7.3.9). PLAN.md
//! M1.7's first deliverable.

use std::collections::{BTreeSet, HashMap};

use accesskit::{Node as AccessibilityNode, NodeId, Tree, TreeId, TreeUpdate};
use aurora_core::Rect;
use taffy::{AvailableSpace, Overflow, Size as LayoutSize, Style as LayoutStyle, TaffyTree};

use crate::error::WidgetError;

/// A widget's identity. This *is* `accesskit::NodeId`, not a wrapper
/// around it — invariant §7.3.9 says every widget carries an accessKit
/// node "as part of its definition, not a pass"; making the tree's own
/// identity and the accessibility node's identity the literal same value
/// means there is no separate id space to keep in sync or forget to.
pub type WidgetId = NodeId;

/// A widget with no computed layout yet — [`WidgetTree::new`]/
/// [`WidgetTree::insert`]'s initial `bounds` before the first
/// [`WidgetTree::compute_layout`] call.
const UNLAID_OUT: Rect = Rect {
    x: 0,
    y: 0,
    width: 0,
    height: 0,
};

/// Which paint layer a widget's subtree belongs to — the popover
/// (overlay) layer 0.127.0 added, so a dropdown's open list, a menu or a
/// tooltip paints and hit-tests *above* every ordinary widget instead of
/// in its own structural position among its siblings.
///
/// Set per widget with [`WidgetTree::set_layer`]; the flag is *not*
/// copied to descendants. A widget's effective layer is
/// [`PaintLayer::Popover`] if *any* ancestor-or-self's own flag is
/// `Popover`, else [`PaintLayer::Base`]: a `Base` flag on a widget
/// inside a popover has no effect. A non-root widget whose **own** flag
/// is `Popover` is a *popover root* ([`WidgetTree::popover_root_of`]),
/// and its parent is the popover's *owner*. The accessibility tree and
/// `Tab` order are unaffected — both stay structural, so a popover is
/// still announced and focused as its owner's child.
///
/// **A popover follows its owner out of sight** (0.127.0 review): when
/// the owner is wholly clipped away by its own clipping ancestors (a
/// control scrolled or collapsed out of a panel body that hides its
/// overflow), its whole popover subtree neither paints nor hit-tests —
/// exactly what the pre-0.127.0 structural clip did — rather than
/// floating with no visible owner. A partly visible owner keeps its
/// popover whole. **Damage gap, disclosed:** no damage is raised when a
/// popover hides or reappears *solely* because its owner's clipping
/// ancestors changed (a panel collapsing while the owner's own bounds
/// stay put) — only the ancestor's old and new bounds are dirtied, so a
/// part of the popover outside them repaints only if the popover's own
/// bounds also change. Harmless while `aurora-app` repaints every frame
/// in full (it never consumes `take_damage`).
///
/// **Contract: a popover root must contain its descendants.** Hit-testing
/// a popover descends only through widgets whose own bounds contain the
/// point, starting at the popover root, while painting clips a
/// popover's descendants only by the popover root's own overflow (and
/// the window). So a descendant overflowing a popover root whose
/// overflow is `Visible` would paint on top of the base layer while
/// clicks on the overhang fall through to the widget beneath it. The
/// three shipped popovers (a dropdown's open list, a menu, a tooltip)
/// all lay their children out inside their own bounds, pinned by a
/// test; a new popover must too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaintLayer {
    /// Painted and hit-tested in the widget's own structural position.
    #[default]
    Base,
    /// This widget roots a popover: its subtree is painted after every
    /// [`PaintLayer::Base`] widget (and after every popover root created
    /// before it), is not clipped by any ancestor outside the popover —
    /// only by the tree root's own bounds (the window) — and is
    /// hit-tested before everything painted beneath it.
    Popover,
}

struct WidgetNode<W> {
    parent: Option<WidgetId>,
    /// Paint/tab order, first to last — unlike `aurora_doc::LayerTree`'s
    /// "newest on top, insert at index 0" convention, a widget tree has
    /// no natural "on top" for a fresh child the way a layers panel does;
    /// new children are appended at the end, the same convention
    /// `Node::push_child` (this module's `insert`) and every mainstream
    /// UI toolkit's "append child" already use.
    children: Vec<WidgetId>,
    /// This widget's layout *input* — flex properties, sizing, spacing.
    /// [`WidgetTree::compute_layout`] is the only thing that reads it;
    /// [`Self::bounds`] is the (derived, cached) output.
    style: LayoutStyle,
    /// This widget's last-computed screen-space bounds — [`UNLAID_OUT`]
    /// until [`WidgetTree::compute_layout`] has run at least once.
    /// `set_bounds` remains a public escape hatch for a widget that
    /// manages its own placement outside the flex layout system (e.g. an
    /// absolutely-positioned overlay), but the normal path is `style` in,
    /// `compute_layout` out.
    bounds: Rect,
    accessibility: AccessibilityNode,
    dirty: bool,
    /// How far past [`Self::bounds`], in whole pixels on every side, this
    /// widget's own pixels can reach — `0` for almost every widget, and
    /// [`crate::paint::FOCUS_RING_MAX_OUTSET`] for the one widget
    /// [`crate::FocusManager`] currently holds focus on, whose keyboard
    /// focus ring paints *outside* its bounds (a CSS `outline-offset`).
    /// Every damage this widget reports ([`WidgetTree::mark_dirty`],
    /// [`WidgetTree::set_bounds`], removal) is grown by it, so a focused
    /// slider whose thumb moves, or a focused widget that is re-laid out,
    /// repaints its ring's overhang rather than leaving stale ring pixels
    /// behind. Set only via [`WidgetTree::set_damage_outset`].
    damage_outset: u32,
    /// This widget's *own* paint-layer flag — see [`PaintLayer`] for how
    /// the effective layer is derived from it.
    layer: PaintLayer,
    payload: W,
}

/// A retained-mode tree of widgets: exactly one root (unlike
/// `aurora_doc::LayerTree`'s multiple top-level layers — an
/// application has one root window, not several independent ones),
/// arbitrary nesting below it, per-widget damage tracking, and a
/// required [`accesskit::Node`] on every widget from the moment it's
/// created.
///
/// Input/focus routing is a separate, later piece layered on top of this
/// structure — this type owns identity, nesting, layout (style in,
/// bounds out — see [`Self::compute_layout`]), damage, and accessibility
/// content.
pub struct WidgetTree<W> {
    nodes: HashMap<WidgetId, WidgetNode<W>>,
    root: WidgetId,
    next_id: u64,
    /// Accumulated screen-space damage since the last
    /// [`Self::take_damage`] — same `Option<Rect>` +
    /// [`aurora_core::Rect::union`] accumulation idiom
    /// `aurora_tile::Tile::mark_dirty`/`take_dirty` already use.
    damage: Option<Rect>,
    /// Every popover root (a non-root widget whose own flag is
    /// [`PaintLayer::Popover`]), keyed by its raw id so iteration is
    /// already [`Self::popover_roots`]' stacking order. Maintained by
    /// [`Self::set_layer`] and [`Self::remove`] (the only two places a
    /// flag or a node can change), so neither [`Self::paint_order`] nor a
    /// per-hover [`Self::hit_test`] has to scan every node for it.
    popovers: BTreeSet<u64>,
}

/// `rect` grown by `outset` whole pixels on every side — the damage a
/// widget with a nonzero `WidgetNode::damage_outset` reports. An empty
/// rect (a widget not laid out yet) stays exactly as it is: it paints
/// nothing, so it has no overhang to repaint either, and growing it would
/// only drag the damage region toward its origin.
fn outset_rect(rect: Rect, outset: u32) -> Rect {
    if outset == 0 || rect.width == 0 || rect.height == 0 {
        return rect;
    }
    Rect {
        x: rect.x - i64::from(outset),
        y: rect.y - i64::from(outset),
        width: rect.width.saturating_add(outset.saturating_mul(2)),
        height: rect.height.saturating_add(outset.saturating_mul(2)),
    }
}

impl<W> WidgetTree<W> {
    /// Creates a new tree with `payload` as its root widget, laid out per
    /// `style`, described by `accessibility`. Returns the tree and the
    /// root's id (always `NodeId(0)`, but returned rather than assumed,
    /// so callers never hardcode it). The root's bounds are
    /// `UNLAID_OUT` until [`Self::compute_layout`] runs.
    #[must_use]
    pub fn new(
        accessibility: AccessibilityNode,
        style: LayoutStyle,
        payload: W,
    ) -> (Self, WidgetId) {
        let root = WidgetId::from(0);
        let mut nodes = HashMap::new();
        nodes.insert(
            root,
            WidgetNode {
                parent: None,
                children: Vec::new(),
                style,
                bounds: UNLAID_OUT,
                accessibility,
                dirty: true,
                damage_outset: 0,
                layer: PaintLayer::Base,
                payload,
            },
        );
        (
            Self {
                nodes,
                root,
                next_id: 1,
                damage: None,
                popovers: BTreeSet::new(),
            },
            root,
        )
    }

    #[must_use]
    pub fn root(&self) -> WidgetId {
        self.root
    }

    #[must_use]
    pub fn contains(&self, id: WidgetId) -> bool {
        self.nodes.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every widget in this tree, in paint order: root first, then each
    /// child subtree (in [`Self::children`]'s own first-inserted-to-last
    /// order) before the next sibling's own — the same traversal
    /// [`Self::hit_test`] already walks (in reverse, for "topmost wins",
    /// apart from its popover pass — see below),
    /// exposed as a real method rather than requiring every caller that
    /// needs "every widget, correctly ordered" (a real per-frame paint
    /// pass, most notably) to reimplement tree descent themselves.
    ///
    /// **Popover roots are deferred** (0.127.0): the structural walk does
    /// not descend into a child whose own [`PaintLayer`] is
    /// [`PaintLayer::Popover`]; instead each popover root's subtree is
    /// appended afterwards, in [`Self::popover_roots`]' stacking order
    /// (creation order, bottom to top), by the same rule — so a popover
    /// nested inside another popover is painted in its own turn, after
    /// its container. Every widget still appears exactly once, and a tree
    /// with no popover roots gets exactly the pre-0.127.0 order.
    #[must_use]
    pub fn paint_order(&self) -> Vec<WidgetId> {
        let mut order = Vec::with_capacity(self.nodes.len());
        self.collect_paint_order(self.root, &mut order);
        for popover in self.popover_stack() {
            self.collect_paint_order(popover, &mut order);
        }
        order
    }

    fn collect_paint_order(&self, id: WidgetId, order: &mut Vec<WidgetId>) {
        order.push(id);
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        for &child in &node.children {
            if self.layer(child) == Some(PaintLayer::Popover) {
                continue;
            }
            self.collect_paint_order(child, order);
        }
    }

    /// `id`'s *own* [`PaintLayer`] flag, as last set by
    /// [`Self::set_layer`] (not the effective layer it inherits — see
    /// [`Self::popover_root_of`] for that). `None` if `id` doesn't exist.
    #[must_use]
    pub fn layer(&self, id: WidgetId) -> Option<PaintLayer> {
        self.nodes.get(&id).map(|node| node.layer)
    }

    /// Sets `id`'s own [`PaintLayer`] flag. Setting the flag it already
    /// has is a no-op (no damage); a real change dirties the union of the
    /// (non-empty) bounds of every widget in `id`'s subtree, since the whole subtree
    /// now paints at a different depth (and, for a popover, unclipped by
    /// its former clipping ancestors).
    ///
    /// Stacking between popover roots is by creation order, **not** by
    /// when this was called — see [`Self::popover_roots`]. A popover root
    /// must lay its descendants out inside its own bounds — see
    /// [`PaintLayer`]'s contract.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
    /// [`WidgetError::CannotLayerRoot`] if `id` is this tree's root (the
    /// root is the window: there is nothing beneath it to float above).
    /// Nothing is changed when this happens.
    pub fn set_layer(&mut self, id: WidgetId, layer: PaintLayer) -> Result<(), WidgetError> {
        if id == self.root {
            return Err(WidgetError::CannotLayerRoot(id));
        }
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        if node.layer == layer {
            return Ok(());
        }
        node.layer = layer;
        node.dirty = true;
        match layer {
            PaintLayer::Popover => self.popovers.insert(u64::from(id)),
            PaintLayer::Base => self.popovers.remove(&u64::from(id)),
        };
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            let Some(node) = self.nodes.get(&current) else {
                continue;
            };
            let (bounds, children) = (node.bounds, node.children.clone());
            // A widget with no area yet (not laid out) occupies no pixels;
            // unioning its zero rect would only drag the damage region
            // toward its origin.
            if bounds.width > 0 && bounds.height > 0 {
                self.mark_region_dirty(bounds);
            }
            stack.extend(children);
        }
        Ok(())
    }

    /// The nearest ancestor-or-self of `id` whose own flag is
    /// [`PaintLayer::Popover`] — the popover `id` paints and hit-tests
    /// as part of — or `None` if `id` is in the base layer (or doesn't
    /// exist). The tree root is never a popover root.
    #[must_use]
    pub fn popover_root_of(&self, id: WidgetId) -> Option<WidgetId> {
        let mut current = Some(id);
        while let Some(candidate) = current {
            if candidate == self.root {
                return None;
            }
            let node = self.nodes.get(&candidate)?;
            if node.layer == PaintLayer::Popover {
                return Some(candidate);
            }
            current = node.parent;
        }
        None
    }

    /// Every popover root in this tree (every non-root widget whose own
    /// flag is [`PaintLayer::Popover`]), in stacking order, bottom to
    /// top: ascending [`WidgetId`], i.e. creation order. So a popover
    /// nested inside another always stacks above its container, and the
    /// most recently *created* popover wins an overlap — which is why the
    /// dropdown rebuilds its list on every open and the tooltip
    /// re-inserts its node on every show. A widget created long ago and
    /// flagged later stacks by its creation, not by the flagging.
    #[must_use]
    pub fn popover_roots(&self) -> Vec<WidgetId> {
        self.popover_stack().collect()
    }

    /// [`Self::popover_roots`] without the allocation: iterates the
    /// maintained set directly, bottom to top.
    fn popover_stack(&self) -> impl DoubleEndedIterator<Item = WidgetId> + '_ {
        self.popovers.iter().map(|&id| WidgetId::from(id))
    }

    /// Whether `popover`'s *owner* (its parent) is wholly clipped away,
    /// by the same rule [`Self::visible_rect`] clips a widget's paint —
    /// in which case the popover neither paints nor hit-tests (see
    /// [`PaintLayer`]). Recursive through nested popovers: an owner that
    /// itself sits inside a hidden popover is clipped away too. `false`
    /// for a widget with no parent or unknown bounds. A zero-area owner
    /// under any clipping ancestor counts as clipped away (its popover
    /// is hidden); one with no clipping ancestor at all is left alone —
    /// irrelevant for the shipped widgets, whose owners are laid-out
    /// controls, but a future zero-size anchor inside a clipped panel
    /// would lose its popover.
    pub(crate) fn popover_owner_hidden(&self, popover: WidgetId) -> bool {
        let Some(owner) = self.parent(popover) else {
            return false;
        };
        let Some(owner_bounds) = self.bounds(owner) else {
            return false;
        };
        self.visible_rect(owner, owner_bounds).is_none()
    }

    /// `bounds` (normally `id`'s own) intersected with every clipping
    /// ancestor of `id` — the one clip rule both paint
    /// (`paint::clip_to_clipping_ancestors`, which documents it in full)
    /// and, through [`Self::popover_owner_hidden`], [`Self::hit_test`]
    /// use, so the two cannot disagree about whether a popover's owner
    /// is visible. `None` when nothing is left, or when `id` sits inside
    /// a popover whose owner is wholly clipped away.
    ///
    /// A widget inside a popover is clipped only by clipping ancestors
    /// up to and including its popover root, then clamped to the tree
    /// root's own bounds (the window). `bounds` is returned untouched
    /// when nothing clips it at all.
    pub(crate) fn visible_rect(&self, id: WidgetId, bounds: Rect) -> Option<Rect> {
        let popover = self.popover_root_of(id);
        if popover.is_some_and(|popover| self.popover_owner_hidden(popover)) {
            return None;
        }
        let mut left = bounds.x;
        let mut top = bounds.y;
        let mut right = bounds.x.saturating_add(i64::from(bounds.width));
        let mut bottom = bounds.y.saturating_add(i64::from(bounds.height));
        let mut clipped = false;
        // A popover root is not clipped by anything above it: its walk
        // starts with no ancestor at all.
        let mut current = if self.layer(id) == Some(PaintLayer::Popover) {
            None
        } else {
            self.parent(id)
        };
        while let Some(ancestor) = current {
            if let (Some(style), Some(clip)) = (self.style(ancestor), self.bounds(ancestor)) {
                if style.overflow.x != Overflow::Visible {
                    clipped = true;
                    left = left.max(clip.x);
                    right = right.min(clip.x.saturating_add(i64::from(clip.width)));
                }
                if style.overflow.y != Overflow::Visible {
                    clipped = true;
                    top = top.max(clip.y);
                    bottom = bottom.min(clip.y.saturating_add(i64::from(clip.height)));
                }
            }
            // A popover root's own `Overflow::Hidden` still clips its
            // descendants (processed just above); nothing above it does.
            if self.layer(ancestor) == Some(PaintLayer::Popover) {
                break;
            }
            current = self.parent(ancestor);
        }
        // Every popover is clamped to the tree root's own bounds (the
        // window) instead — the same gate `Self::hit_test` applies.
        if popover.is_some()
            && let Some(window) = self.bounds(self.root)
        {
            clipped = true;
            left = left.max(window.x);
            top = top.max(window.y);
            right = right.min(window.x.saturating_add(i64::from(window.width)));
            bottom = bottom.min(window.y.saturating_add(i64::from(window.height)));
        }
        // Returned untouched, not merely unchanged, when nothing clips at
        // all: a widget whose bounds are still the default zero rect must
        // keep painting the degenerate shape it always did, rather than
        // being turned into nothing by an empty intersection with itself.
        if !clipped {
            return Some(bounds);
        }
        if right <= left || bottom <= top {
            return None;
        }
        Some(Rect {
            x: left,
            y: top,
            width: u32::try_from(right - left).ok()?,
            height: u32::try_from(bottom - top).ok()?,
        })
    }

    /// Adds a new widget as the last child of `parent`, laid out per
    /// `style`, described by `accessibility`. Its bounds are
    /// `UNLAID_OUT` until [`Self::compute_layout`] runs — inserting a
    /// widget dirties `parent`'s subtree (its layout may now change) but
    /// not a specific screen region, since the new widget doesn't have
    /// screen bounds yet.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
    /// Nothing is added when this happens.
    pub fn insert(
        &mut self,
        parent: WidgetId,
        style: LayoutStyle,
        accessibility: AccessibilityNode,
        payload: W,
    ) -> Result<WidgetId, WidgetError> {
        if !self.nodes.contains_key(&parent) {
            return Err(WidgetError::UnknownWidget(parent));
        }

        let id = WidgetId::from(self.next_id);
        self.next_id += 1;
        self.nodes.insert(
            id,
            WidgetNode {
                parent: Some(parent),
                children: Vec::new(),
                style,
                bounds: UNLAID_OUT,
                accessibility,
                dirty: true,
                damage_outset: 0,
                layer: PaintLayer::Base,
                payload,
            },
        );

        let Some(parent_node) = self.nodes.get_mut(&parent) else {
            unreachable!("parent's existence was already checked above");
        };
        parent_node.children.push(id);

        Ok(id)
    }

    /// Removes `id` and, recursively, every descendant. Marks every
    /// removed widget's last-known bounds dirty (so the region they used
    /// to occupy gets repainted).
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::CannotRemoveRoot`] if `id` is this tree's
    /// root, or [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    /// Nothing is changed when this happens.
    pub fn remove(&mut self, id: WidgetId) -> Result<(), WidgetError> {
        if id == self.root {
            return Err(WidgetError::CannotRemoveRoot(id));
        }
        let node = self.nodes.get(&id).ok_or(WidgetError::UnknownWidget(id))?;
        let Some(parent) = node.parent else {
            unreachable!("only the root (rejected above) can have no parent");
        };

        let Some(parent_node) = self.nodes.get_mut(&parent) else {
            unreachable!("a widget's recorded parent must exist in the tree by construction");
        };
        parent_node.children.retain(|&child| child != id);

        self.remove_subtree(id);
        Ok(())
    }

    fn remove_subtree(&mut self, id: WidgetId) {
        let Some(node) = self.nodes.remove(&id) else {
            unreachable!("a parent's recorded children must exist in the tree by construction");
        };
        self.mark_region_dirty(outset_rect(node.bounds, node.damage_outset));
        self.popovers.remove(&u64::from(id));
        for child in node.children {
            self.remove_subtree(child);
        }
    }

    #[must_use]
    pub fn parent(&self, id: WidgetId) -> Option<WidgetId> {
        self.nodes.get(&id).and_then(|node| node.parent)
    }

    /// `None` both when `id` doesn't exist and when it exists but has no
    /// children — callers that need to tell those apart should check
    /// [`Self::contains`] first, matching `aurora_doc::LayerTree`'s own
    /// `parent`/`children` convention.
    #[must_use]
    pub fn children(&self, id: WidgetId) -> Option<&[WidgetId]> {
        self.nodes.get(&id).map(|node| node.children.as_slice())
    }

    #[must_use]
    pub fn bounds(&self, id: WidgetId) -> Option<Rect> {
        self.nodes.get(&id).map(|node| node.bounds)
    }

    /// The topmost widget whose current bounds contain `point`
    /// (screen-space, same units [`Self::bounds`] reports), or `None` if
    /// none does — the "what did the user actually click" query real
    /// pointer input needs, this crate's first (this type has offered
    /// `bounds` since M1.7, but nothing has needed the reverse direction
    /// until now).
    ///
    /// Descends into `children` (paint order [`Self::children`] already
    /// documents: first inserted to last) before considering a node's
    /// own bounds a hit, and checks them in *reverse* order — the
    /// last-painted child is topmost, the one a real click should prefer
    /// when widgets overlap. A parent whose own bounds don't contain
    /// `point` is not descended into at all, on the assumption
    /// (already true of every widget this crate builds via flex layout)
    /// that a child never paints outside its parent's own bounds.
    ///
    /// **Popovers are the one exception to that assumption** (0.127.0):
    /// a point outside the tree root's own bounds (the window) hits
    /// nothing, exactly as a popover is clamped to the window when
    /// painted; otherwise every popover root is tried first, topmost
    /// ([`Self::popover_roots`]' last) first, gated only by its *own*
    /// bounds — never its owner's or any other ancestor's — and only
    /// then the base layer. A popover whose owner is wholly clipped away
    /// is skipped entirely, exactly as it is not painted (see
    /// [`PaintLayer`]). The structural descent skips children that
    /// root a popover, so each widget is reached through exactly one
    /// route, the same one [`Self::paint_order`] paints it by.
    /// [`crate::hit_test`] delegates to the same traversal, so the two
    /// hit-testers cannot disagree about layering.
    #[must_use]
    pub fn hit_test(&self, point: (f32, f32)) -> Option<WidgetId> {
        self.hit_test_by(|bounds| bounds_contain(bounds, point))
    }

    /// The single hit-test traversal behind both [`Self::hit_test`] and
    /// [`crate::hit_test`]: each passes its own containment predicate
    /// (`f32` vs `f64` point), and everything about layering lives here.
    pub(crate) fn hit_test_by(&self, contains: impl Fn(Rect) -> bool) -> Option<WidgetId> {
        let root = self.nodes.get(&self.root)?;
        if !contains(root.bounds) {
            return None;
        }
        for popover in self.popover_stack().rev() {
            if self.popover_owner_hidden(popover) {
                continue;
            }
            if let Some(hit) = self.hit_test_from(popover, &contains) {
                return Some(hit);
            }
        }
        self.hit_test_from(self.root, &contains)
    }

    fn hit_test_from(&self, id: WidgetId, contains: &impl Fn(Rect) -> bool) -> Option<WidgetId> {
        let node = self.nodes.get(&id)?;
        if !contains(node.bounds) {
            return None;
        }
        for &child in node.children.iter().rev() {
            if self.layer(child) == Some(PaintLayer::Popover) {
                continue;
            }
            if let Some(hit) = self.hit_test_from(child, contains) {
                return Some(hit);
            }
        }
        Some(id)
    }

    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    pub fn set_bounds(&mut self, id: WidgetId, bounds: Rect) -> Result<(), WidgetError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        let old_bounds = node.bounds;
        node.bounds = bounds;
        node.dirty = true;
        let outset = node.damage_outset;
        // Both the vacated region and the newly occupied one need
        // repainting, not just the new position.
        self.mark_region_dirty(outset_rect(old_bounds, outset));
        self.mark_region_dirty(outset_rect(bounds, outset));
        Ok(())
    }

    #[must_use]
    pub fn style(&self, id: WidgetId) -> Option<&LayoutStyle> {
        self.nodes.get(&id).map(|node| &node.style)
    }

    /// Replaces `id`'s layout style — takes effect on the next
    /// [`Self::compute_layout`] call, not immediately (unlike
    /// [`Self::set_bounds`], a style change alone doesn't know what the
    /// new bounds would be without re-running layout).
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    pub fn set_style(&mut self, id: WidgetId, style: LayoutStyle) -> Result<(), WidgetError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        node.style = style;
        Ok(())
    }

    #[must_use]
    pub fn payload(&self, id: WidgetId) -> Option<&W> {
        self.nodes.get(&id).map(|node| &node.payload)
    }

    #[must_use]
    pub fn payload_mut(&mut self, id: WidgetId) -> Option<&mut W> {
        self.nodes.get_mut(&id).map(|node| &mut node.payload)
    }

    /// This widget's own accessibility node, as last set by
    /// [`Self::insert`]/[`Self::set_accessibility`] — not the tree-wide
    /// [`accesskit::TreeUpdate`], which [`Self::accessibility_update`]
    /// builds from every widget's own node together.
    #[must_use]
    pub fn accessibility(&self, id: WidgetId) -> Option<&AccessibilityNode> {
        self.nodes.get(&id).map(|node| &node.accessibility)
    }

    /// Replaces `id`'s accessibility node (e.g. a text field updating its
    /// `value` after a keystroke). Marks `id` dirty — accessibility
    /// content changing is itself damage a screen reader needs to be told
    /// about, the same way a bounds change is damage a renderer needs to
    /// repaint.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    pub fn set_accessibility(
        &mut self,
        id: WidgetId,
        accessibility: AccessibilityNode,
    ) -> Result<(), WidgetError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        node.accessibility = accessibility;
        node.dirty = true;
        Ok(())
    }

    /// Whether `id` has been marked dirty since the last time it was
    /// cleared (there is no per-widget "take", only the tree-wide
    /// [`Self::take_damage`] — a renderer cares about the accumulated
    /// screen region, not which individual widgets contributed to it).
    #[must_use]
    pub fn is_dirty(&self, id: WidgetId) -> Option<bool> {
        self.nodes.get(&id).map(|node| node.dirty)
    }

    /// Marks `id` dirty without changing its bounds or accessibility
    /// content (e.g. a widget that needs repainting for a reason this
    /// tree doesn't model itself, like a hover-state colour change).
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    pub fn mark_dirty(&mut self, id: WidgetId) -> Result<(), WidgetError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        node.dirty = true;
        let region = outset_rect(node.bounds, node.damage_outset);
        self.mark_region_dirty(region);
        Ok(())
    }

    /// Sets how far past its bounds `id`'s damage reaches — see
    /// `WidgetNode::damage_outset`. Crate-private: the only caller is
    /// [`crate::FocusManager`], which grows the focused widget's damage by
    /// its focus ring's overhang and shrinks it back to `0` on blur.
    /// Changing it marks nothing dirty by itself; the caller dirties the
    /// widget afterwards (with the *new* outset) or before (with the old).
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist.
    pub(crate) fn set_damage_outset(
        &mut self,
        id: WidgetId,
        outset: u32,
    ) -> Result<(), WidgetError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(WidgetError::UnknownWidget(id))?;
        node.damage_outset = outset;
        Ok(())
    }

    /// Resets every node's damage outset to `0`, dirtying each one that
    /// had a nonzero outset (with that outset, so the old ring's overhang
    /// is repainted). [`crate::FocusManager`] calls it on every focus
    /// change before growing the new widget's outset, so an outset left
    /// behind by a manager that was dropped or replaced without blurring
    /// — which no later manager knows about — never outlives the next
    /// focus change. O(widgets), on a focus change only.
    pub(crate) fn clear_damage_outsets(&mut self) {
        let mut regions = Vec::new();
        for node in self.nodes.values_mut() {
            if node.damage_outset != 0 {
                node.dirty = true;
                regions.push(outset_rect(node.bounds, node.damage_outset));
                node.damage_outset = 0;
            }
        }
        for region in regions {
            self.mark_region_dirty(region);
        }
    }

    /// `id`'s current damage outset — see `WidgetNode::damage_outset`.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn damage_outset(&self, id: WidgetId) -> Option<u32> {
        self.nodes.get(&id).map(|node| node.damage_outset)
    }

    fn mark_region_dirty(&mut self, region: Rect) {
        self.damage = Some(match self.damage {
            Some(existing) => existing.union(&region),
            None => region,
        });
    }

    /// Recomputes every widget's bounds from its own `style`, treating
    /// `width`/`height` as the root's available space (typically the
    /// window's current client size). Rebuilds a fresh internal `taffy`
    /// tree on every call rather than keeping one permanently in sync
    /// with this tree's own structure — this tree stays the single
    /// source of truth for identity/nesting, and re-deriving layout from
    /// it fresh is the same "recomputed on demand from a source of
    /// truth" shape `aurora_doc::History::replay` already uses for its
    /// own journal. Each widget's bounds are set via [`Self::set_bounds`]
    /// internally, so the usual dirty-marking (both vacated and newly
    /// occupied regions) applies here too, not a separate code path.
    pub fn compute_layout(&mut self, width: f32, height: f32) {
        let mut taffy = TaffyTree::<()>::new();
        let mut taffy_ids = HashMap::new();
        self.build_taffy_node(self.root, &mut taffy, &mut taffy_ids);

        let Some(&taffy_root) = taffy_ids.get(&self.root) else {
            unreachable!("build_taffy_node always inserts the node it was called with");
        };
        let available = LayoutSize {
            width: AvailableSpace::Definite(width),
            height: AvailableSpace::Definite(height),
        };
        if taffy.compute_layout(taffy_root, available).is_err() {
            unreachable!(
                "TaffyError only occurs for a node id from a different tree, \
                 which this method never constructs"
            );
        }

        self.apply_taffy_layout(self.root, &taffy, &taffy_ids, 0.0, 0.0);
    }

    /// Builds `id`'s subtree in `taffy`, children first (`taffy::TaffyTree`
    /// needs a node's children to already exist before the node itself can
    /// reference them), recording each widget's corresponding
    /// `taffy::NodeId` in `taffy_ids`.
    fn build_taffy_node(
        &self,
        id: WidgetId,
        taffy: &mut TaffyTree<()>,
        taffy_ids: &mut HashMap<WidgetId, taffy::NodeId>,
    ) {
        let Some(node) = self.nodes.get(&id) else {
            unreachable!("build_taffy_node is only ever called with ids known to exist");
        };
        let mut taffy_children = Vec::with_capacity(node.children.len());
        for &child in &node.children {
            self.build_taffy_node(child, taffy, taffy_ids);
            let Some(&taffy_child) = taffy_ids.get(&child) else {
                unreachable!("just inserted by the recursive call above");
            };
            taffy_children.push(taffy_child);
        }

        let result = if taffy_children.is_empty() {
            taffy.new_leaf(node.style.clone())
        } else {
            taffy.new_with_children(node.style.clone(), &taffy_children)
        };
        let Ok(taffy_id) = result else {
            unreachable!(
                "a style value and freshly-created children in this same taffy \
                 tree are always valid"
            );
        };
        taffy_ids.insert(id, taffy_id);
    }

    /// Walks `id`'s subtree top-down, converting `taffy`'s parent-relative
    /// `Layout::location` into this tree's absolute screen-space bounds
    /// (`parent_x`/`parent_y` is the already-accumulated absolute origin
    /// of `id`'s own parent), and writes each widget's new bounds back via
    /// [`Self::set_bounds`].
    fn apply_taffy_layout(
        &mut self,
        id: WidgetId,
        taffy: &TaffyTree<()>,
        taffy_ids: &HashMap<WidgetId, taffy::NodeId>,
        parent_x: f32,
        parent_y: f32,
    ) {
        let Some(&taffy_id) = taffy_ids.get(&id) else {
            unreachable!("every widget id has a corresponding taffy node from build_taffy_node");
        };
        let Ok(layout) = taffy.layout(taffy_id) else {
            unreachable!("layout was just computed for exactly this taffy tree");
        };
        let abs_x = parent_x + layout.location.x;
        let abs_y = parent_y + layout.location.y;
        // `.max(0.0)` before the cast makes the sign-loss clippy warns
        // about unreachable in practice, but not provable statically.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bounds = Rect {
            x: abs_x as i64,
            y: abs_y as i64,
            width: layout.size.width.max(0.0) as u32,
            height: layout.size.height.max(0.0) as u32,
        };

        let children = match self.nodes.get(&id) {
            Some(node) => node.children.clone(),
            None => unreachable!("id is known to exist: it was just looked up via taffy_ids"),
        };

        if let Err(err) = self.set_bounds(id, bounds) {
            unreachable!("id is known to exist: {err:?}");
        }

        for child in children {
            self.apply_taffy_layout(child, taffy, taffy_ids, abs_x, abs_y);
        }
    }

    /// Takes and clears the accumulated screen-space damage region, and
    /// clears every widget's own per-widget dirty flag — e.g. right
    /// before a repaint, so a widget touched again afterward is tracked
    /// as freshly dirty rather than silently merged into the frame that
    /// already painted it.
    pub fn take_damage(&mut self) -> Option<Rect> {
        for node in self.nodes.values_mut() {
            node.dirty = false;
        }
        self.damage.take()
    }

    /// Builds a full [`accesskit::TreeUpdate`] from every widget's own
    /// accessibility node — what a platform adapter (`accesskit_winit`,
    /// per the a11y spike) actually sends to the screen reader. Each
    /// node's `children` is set here, from this tree's own real
    /// structure — a widget's stored [`AccessibilityNode`] never carries
    /// it itself (nothing else in this module ever sets it), so without
    /// this every node but the root would come out with no declared
    /// children, and `accesskit_consumer` rejects that as a
    /// disconnected tree (confirmed via a real crash on real macOS
    /// hardware: "N nodes which are neither in the current tree nor a
    /// child of another node from the update").
    ///
    /// **A `focus` that no longer exists falls back to this tree's own
    /// root**, and that guard is load-bearing rather than defensive
    /// tidiness. `accesskit_consumer::State::validate_global` (pinned
    /// 0.38, `tree.rs`) *panics* with "Focused ID #N is not in the node
    /// list" on a focus id the update doesn't carry — the same
    /// disconnected-tree validation class as the crash above, reproduced
    /// live. Any caller that removes widgets can orphan the focus
    /// between the removal and the next
    /// [`crate::FocusManager::validate`], and `widgets::tree_view`'s own
    /// collapse is exactly such a caller; putting the check here defends
    /// every removal site at once rather than each one separately.
    /// Callers that track focus should still call `FocusManager::
    /// validate` — this only guarantees the update is *valid*, not that
    /// the caller's own idea of focus was repaired.
    #[must_use]
    pub fn accessibility_update(&self, focus: WidgetId) -> TreeUpdate {
        let nodes = self
            .nodes
            .iter()
            .map(|(&id, node)| {
                let mut accessibility = node.accessibility.clone();
                accessibility.set_children(node.children.clone());
                (id, accessibility)
            })
            .collect();
        let focus = if self.nodes.contains_key(&focus) {
            focus
        } else {
            self.root
        };
        TreeUpdate {
            nodes,
            tree: Some(Tree::new(self.root)),
            tree_id: ACCESSIBILITY_TREE_ID,
            focus,
        }
    }
}

/// The one `accesskit::TreeId` every [`WidgetTree::accessibility_update`]
/// is published under — and therefore the only `target_tree` an incoming
/// `accesskit::ActionRequest` can legitimately name
/// (`crate::action::handle_action` rejects any other). One constant, so
/// the two cannot drift apart.
pub const ACCESSIBILITY_TREE_ID: TreeId = TreeId::ROOT;

/// Half-open containment — `point` is inside `rect` if `rect.x <=
/// point.x < rect.right()` (and the same for `y`) — matching
/// `aurora_core::Rect::intersects`'s own convention (two rects only
/// touching at a shared edge don't overlap).
#[allow(clippy::cast_precision_loss)]
fn bounds_contain(rect: Rect, point: (f32, f32)) -> bool {
    let (x, y) = point;
    let (left, top, right, bottom) = (
        rect.x as f32,
        rect.y as f32,
        rect.right() as f32,
        rect.bottom() as f32,
    );
    x >= left && y >= top && x < right && y < bottom
}

impl<W> std::fmt::Debug for WidgetTree<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidgetTree")
            .field("len", &self.nodes.len())
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{PaintLayer, WidgetId, WidgetTree};
    use crate::WidgetError;
    use accesskit::{Node, Role};
    use aurora_core::Rect;
    use taffy::style_helpers::{length, percent};
    use taffy::{FlexDirection, Size, Style};

    fn bounds(x: i64, y: i64, w: u32, h: u32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn label(text: &str) -> Node {
        let mut node = Node::new(Role::Label);
        node.set_label(text);
        node
    }

    /// A style with an explicit, fixed pixel size — the common case for
    /// these tests, which mostly care about layout math, not exercising
    /// every style property.
    fn sized(width: f32, height: f32) -> Style {
        Style {
            size: Size {
                width: length(width),
                height: length(height),
            },
            ..Default::default()
        }
    }

    #[test]
    fn new_tree_has_exactly_the_root() {
        let (tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        assert_eq!(tree.len(), 1);
        assert!(!tree.is_empty());
        assert_eq!(tree.root(), root);
        assert!(tree.contains(root));
        assert_eq!(tree.parent(root), None);
        assert_eq!(tree.children(root), Some([].as_slice()));
        assert_eq!(tree.payload(root), Some(&"root"));
        assert_eq!(
            tree.bounds(root),
            Some(bounds(0, 0, 0, 0)),
            "unlaid-out bounds until compute_layout runs"
        );
    }

    #[test]
    fn insert_adds_a_child_at_the_end_and_marks_it_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, Style::default(), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(root), Some([a, b].as_slice()));
        assert_eq!(tree.parent(a), Some(root));
        assert_eq!(tree.is_dirty(b), Some(true));
    }

    #[test]
    fn paint_order_visits_root_then_each_child_subtree_before_the_next_sibling() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let group = match tree.insert(root, Style::default(), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.insert(group, Style::default(), label("leaf"), "leaf") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let last = match tree.insert(root, Style::default(), label("last"), "last") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.paint_order(), vec![root, group, leaf, last]);
    }

    #[test]
    fn insert_rejects_an_unknown_parent() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let bogus = accesskit::NodeId(999);
        match tree.insert(bogus, Style::default(), label("x"), "x") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "a failed insert must add nothing");
        let _ = root;
    }

    #[test]
    fn remove_detaches_a_leaf_and_updates_the_parent() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(a));
        assert_eq!(tree.children(root), Some([].as_slice()));
    }

    #[test]
    fn remove_cascades_into_every_descendant() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let group = match tree.insert(root, Style::default(), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.insert(group, Style::default(), label("leaf"), "leaf") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(group) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(group));
        assert!(!tree.contains(leaf));
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn remove_rejects_the_root() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        match tree.remove(root) {
            Err(WidgetError::CannotRemoveRoot(id)) => assert_eq!(id, root),
            other => unreachable!("expected CannotRemoveRoot, got {other:?}"),
        }
        assert!(tree.contains(root));
    }

    #[test]
    fn remove_rejects_an_unknown_id() {
        let (mut tree, _root) = WidgetTree::new(label("root"), Style::default(), "root");
        let bogus = accesskit::NodeId(999);
        match tree.remove(bogus) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_bounds_updates_and_marks_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();

        if let Err(err) = tree.set_bounds(a, bounds(5, 5, 10, 10)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.bounds(a), Some(bounds(5, 5, 10, 10)));
        assert_eq!(tree.is_dirty(a), Some(true));
    }

    #[test]
    fn set_bounds_rejects_an_unknown_id() {
        let (mut tree, _root) = WidgetTree::new(label("root"), Style::default(), "root");
        let bogus = accesskit::NodeId(999);
        match tree.set_bounds(bogus, bounds(0, 0, 1, 1)) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_bounds_dirties_both_the_old_and_new_region() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 5, 5)) {
            unreachable!("{err:?}");
        }
        tree.take_damage();

        if let Err(err) = tree.set_bounds(root, bounds(20, 20, 5, 5)) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.take_damage(),
            Some(bounds(0, 0, 5, 5).union(&bounds(20, 20, 5, 5))),
            "both the vacated and the newly occupied region must be dirtied"
        );
    }

    #[test]
    fn a_damage_outset_grows_every_damage_the_widget_reports() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let at = |x| Rect {
            x,
            y: 10,
            width: 20,
            height: 10,
        };
        let grown = |rect: Rect| Rect {
            x: rect.x - 3,
            y: rect.y - 3,
            width: rect.width + 6,
            height: rect.height + 6,
        };
        if let Err(err) = tree.set_bounds(a, at(10)) {
            unreachable!("{err:?}");
        }
        tree.take_damage();
        if let Err(err) = tree.set_damage_outset(a, 3) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), None, "setting it dirties nothing");

        if let Err(err) = tree.mark_dirty(a) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(grown(at(10))));

        if let Err(err) = tree.set_bounds(a, at(50)) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.take_damage(),
            Some(grown(at(10)).union(&grown(at(50))))
        );

        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(grown(at(50))));
        assert!(matches!(
            tree.set_damage_outset(a, 1),
            Err(WidgetError::UnknownWidget(id)) if id == a
        ));
    }

    #[test]
    fn a_damage_outset_leaves_a_widget_with_no_area_alone() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();
        if let Err(err) = tree.set_damage_outset(a, 4) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.mark_dirty(a) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.take_damage(),
            tree.bounds(a),
            "an unlaid widget paints nothing"
        );
    }

    #[test]
    fn take_damage_clears_every_widgets_own_dirty_flag() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.is_dirty(a), Some(true));
        tree.take_damage();
        assert_eq!(tree.is_dirty(a), Some(false));
        assert_eq!(tree.is_dirty(root), Some(false));
    }

    #[test]
    fn accessibility_update_includes_every_widget_and_the_given_focus() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let update = tree.accessibility_update(a);
        assert_eq!(update.nodes.len(), 2);
        assert!(update.nodes.iter().any(|(id, _)| *id == root));
        assert!(update.nodes.iter().any(|(id, _)| *id == a));
        assert_eq!(update.focus, a);
        match update.tree {
            Some(t) => assert_eq!(t.root, root),
            None => unreachable!("expected Some(Tree)"),
        }
    }

    /// A real, structural regression test for a real bug: the first
    /// version of `accessibility_update` never set each node's
    /// `children`, so every node but the root came out looking
    /// disconnected — `accesskit_consumer` (the library
    /// `accesskit_winit`'s adapter uses internally) rejects that,
    /// confirmed by an actual crash on real macOS hardware running
    /// `aurora-app`: "`TreeUpdate` includes N nodes which are neither in
    /// the current tree nor a child of another node from the update."
    /// This is exactly the validation neither this file's own prior
    /// tests nor `aurora-widgets/tests/headless.rs` ever exercised —
    /// both checked node *count*/individual field values, never real
    /// parent-child connectivity. `accesskit_consumer::Tree::new` panics
    /// on a disconnected tree, so a plain call here (no `unwrap`/
    /// `expect` needed) is the whole test: if this regresses, the test
    /// fails with that same panic.
    #[test]
    fn accessibility_update_produces_a_tree_accesskit_consumer_accepts() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let group = match tree.insert(root, Style::default(), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.insert(group, Style::default(), label("leaf"), "leaf") {
            unreachable!("{err:?}");
        }

        let update = tree.accessibility_update(root);
        let _consumer_tree = accesskit_consumer::Tree::new(update, true);
    }

    /// The second half of the same crash class, and a real one: a
    /// removal orphans whatever focus pointed into the removed subtree,
    /// and `accesskit_consumer::State::validate_global` panics with
    /// "Focused ID #N is not in the node list" when that stale id
    /// reaches it. Reproduced live before this guard existed, by
    /// collapsing a tree row whose descendant held focus. The fallback
    /// has to be a node the update really carries — the root always is.
    #[test]
    fn a_focus_on_a_removed_widget_falls_back_to_the_root() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let group = match tree.insert(root, Style::default(), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.insert(group, Style::default(), label("leaf"), "leaf") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(group) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(leaf), "the removal cascaded, as it must");

        let update = tree.accessibility_update(leaf);
        assert_eq!(
            update.focus, root,
            "a stale focus must fall back to a node the update really carries"
        );
        // ... and the guard is what keeps this from aborting the test
        // process: `accesskit_consumer` panics on a focus it can't find.
        let _consumer_tree = accesskit_consumer::Tree::new(update, true);
    }

    /// The fallback is a *fallback*, not a rewrite: a focus that is
    /// still in the tree is passed through untouched.
    #[test]
    fn a_live_focus_is_never_rewritten_by_the_fallback() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.accessibility_update(a).focus, a);
    }

    #[test]
    fn set_accessibility_replaces_the_node_and_marks_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();

        if let Err(err) = tree.set_accessibility(a, label("renamed")) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.accessibility(a).and_then(|n| n.label()),
            Some("renamed")
        );
        assert_eq!(tree.is_dirty(a), Some(true));
    }

    #[test]
    fn payload_mut_allows_updating_the_widgets_own_data() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Some(payload) = tree.payload_mut(root) {
            *payload = "renamed root";
        }
        assert_eq!(tree.payload(root), Some(&"renamed root"));
    }

    #[test]
    fn style_can_be_read_back_and_replaced() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        assert_eq!(tree.style(root), Some(&Style::default()));

        if let Err(err) = tree.set_style(root, sized(50.0, 50.0)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.style(root), Some(&sized(50.0, 50.0)));
    }

    #[test]
    // `taffy` does not implicitly stretch an `Auto`-sized, childless root to
    // fill the available space -- confirmed by running this test with the
    // opposite assertion first and seeing (0, 0, 0, 0) come back, not
    // (0, 0, 300, 150). `Auto` sizes to content, and a childless root has
    // no content; there is no built-in "root fills the viewport" the way
    // CSS's `html, body { width: 100% }` convention provides. A caller
    // that wants the root to fill its window must ask for that explicitly
    // (see the `percent`-sized test right below), the same way a real web
    // page does.
    fn compute_layout_auto_root_with_no_children_stays_content_sized() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        tree.compute_layout(300.0, 150.0);
        assert_eq!(tree.bounds(root), Some(bounds(0, 0, 0, 0)));
    }

    #[test]
    fn compute_layout_a_percent_sized_root_fills_the_available_space() {
        let root_style = Style {
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            ..Default::default()
        };
        let (mut tree, root) = WidgetTree::new(label("root"), root_style, "root");
        tree.compute_layout(300.0, 150.0);
        assert_eq!(tree.bounds(root), Some(bounds(0, 0, 300, 150)));
    }

    #[test]
    fn compute_layout_lays_out_a_row_of_fixed_size_children_left_to_right() {
        let root_style = Style {
            flex_direction: FlexDirection::Row,
            ..Default::default()
        };
        let (mut tree, root) = WidgetTree::new(label("root"), root_style, "root");
        let a = match tree.insert(root, sized(40.0, 20.0), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, sized(30.0, 20.0), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        tree.compute_layout(300.0, 150.0);

        assert_eq!(tree.bounds(a), Some(bounds(0, 0, 40, 20)));
        assert_eq!(
            tree.bounds(b),
            Some(bounds(40, 0, 30, 20)),
            "b must start exactly where a ends"
        );
    }

    #[test]
    fn compute_layout_accumulates_absolute_position_through_nested_groups() {
        let root_style = Style {
            flex_direction: FlexDirection::Row,
            padding: taffy::Rect {
                left: length(10.0_f32),
                top: length(5.0_f32),
                right: length(0.0_f32),
                bottom: length(0.0_f32),
            },
            ..Default::default()
        };
        let (mut tree, root) = WidgetTree::new(label("root"), root_style, "root");
        let group = match tree.insert(root, sized(100.0, 100.0), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.insert(group, sized(20.0, 20.0), label("leaf"), "leaf") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        tree.compute_layout(300.0, 150.0);

        assert_eq!(
            tree.bounds(group),
            Some(bounds(10, 5, 100, 100)),
            "the group must be offset by the root's own padding"
        );
        assert_eq!(
            tree.bounds(leaf),
            Some(bounds(10, 5, 20, 20)),
            "the leaf's absolute position must include its ancestors' offsets too"
        );
    }

    #[test]
    fn compute_layout_marks_changed_widgets_dirty() {
        let root_style = Style {
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            ..Default::default()
        };
        let (mut tree, _root) = WidgetTree::new(label("root"), root_style, "root");
        tree.compute_layout(100.0, 100.0);
        tree.take_damage();

        tree.compute_layout(200.0, 200.0);
        assert_eq!(
            tree.take_damage(),
            Some(bounds(0, 0, 100, 100).union(&bounds(0, 0, 200, 200))),
            "resizing the root must dirty both the old and new region"
        );
    }

    #[test]
    fn hit_test_finds_the_deepest_widget_containing_the_point() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 100, 100)) {
            unreachable!("{err:?}");
        }
        let child = match tree.insert(root, Style::default(), label("child"), "child") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(child, bounds(10, 10, 20, 20)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.hit_test((15.0, 15.0)), Some(child));
    }

    #[test]
    fn hit_test_falls_back_to_the_parent_when_no_child_matches() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 100, 100)) {
            unreachable!("{err:?}");
        }
        let child = match tree.insert(root, Style::default(), label("child"), "child") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(child, bounds(10, 10, 20, 20)) {
            unreachable!("{err:?}");
        }
        // Inside root, outside child.
        assert_eq!(tree.hit_test((50.0, 50.0)), Some(root));
    }

    #[test]
    fn hit_test_returns_none_outside_every_widget() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 100, 100)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.hit_test((200.0, 200.0)), None);
    }

    #[test]
    fn hit_test_prefers_the_last_painted_of_two_overlapping_children() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 100, 100)) {
            unreachable!("{err:?}");
        }
        let a = match tree.insert(root, Style::default(), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(a, bounds(0, 0, 50, 50)) {
            unreachable!("{err:?}");
        }
        // Inserted after `a`, so later in paint order -- must win the
        // same overlapping region.
        let b = match tree.insert(root, Style::default(), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(b, bounds(0, 0, 50, 50)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.hit_test((25.0, 25.0)), Some(b));
    }

    #[test]
    fn hit_test_is_half_open_a_point_on_the_right_or_bottom_edge_is_outside() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        if let Err(err) = tree.set_bounds(root, bounds(0, 0, 10, 10)) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            tree.hit_test((10.0, 5.0)),
            None,
            "exactly on the right edge"
        );
        assert_eq!(
            tree.hit_test((5.0, 10.0)),
            None,
            "exactly on the bottom edge"
        );
        assert_eq!(tree.hit_test((9.999, 9.999)), Some(root), "just inside");
    }

    // -- popover layer (0.127.0) --

    fn ins(tree: &mut WidgetTree<&'static str>, parent: WidgetId, name: &'static str) -> WidgetId {
        match tree.insert(parent, Style::default(), label(name), name) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn place(tree: &mut WidgetTree<&'static str>, id: WidgetId, rect: Rect) {
        if let Err(err) = tree.set_bounds(id, rect) {
            unreachable!("{err:?}");
        }
    }

    fn pop(tree: &mut WidgetTree<&'static str>, id: WidgetId) {
        if let Err(err) = tree.set_layer(id, PaintLayer::Popover) {
            unreachable!("{err:?}");
        }
    }

    #[test]
    fn paint_order_defers_a_popover_subtree_after_the_whole_base_layer() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = ins(&mut tree, root, "a");
        let p = ins(&mut tree, a, "p");
        let p1 = ins(&mut tree, p, "p1");
        let b = ins(&mut tree, root, "b");
        assert_eq!(
            tree.paint_order(),
            vec![root, a, p, p1, b],
            "no popovers yet"
        );
        pop(&mut tree, p);
        assert_eq!(tree.paint_order(), vec![root, a, b, p, p1]);
        assert_eq!(tree.popover_roots(), vec![p]);
        assert_eq!(tree.popover_root_of(p1), Some(p));
        assert_eq!(tree.popover_root_of(p), Some(p));
        assert_eq!(tree.popover_root_of(a), None);
        assert_eq!(tree.popover_root_of(root), None);
        assert_eq!(
            tree.layer(p1),
            Some(PaintLayer::Base),
            "the flag is not copied"
        );
    }

    fn nested_scene() -> (WidgetTree<&'static str>, [WidgetId; 6]) {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = ins(&mut tree, root, "a");
        let p1 = ins(&mut tree, a, "p1");
        let p2 = ins(&mut tree, p1, "p2");
        let q = ins(&mut tree, p2, "q");
        let p3 = ins(&mut tree, root, "p3");
        for id in [p1, p2, p3] {
            pop(&mut tree, id);
        }
        place(&mut tree, root, bounds(0, 0, 100, 100));
        place(&mut tree, a, bounds(0, 0, 20, 20));
        place(&mut tree, p1, bounds(10, 30, 40, 40));
        place(&mut tree, p2, bounds(15, 35, 10, 10));
        place(&mut tree, q, bounds(16, 36, 5, 5));
        place(&mut tree, p3, bounds(40, 60, 40, 30));
        (tree, [root, a, p1, p2, q, p3])
    }

    #[test]
    fn nested_popovers_paint_each_in_their_own_turn_exactly_once() {
        let (tree, [root, a, p1, p2, q, p3]) = nested_scene();
        assert_eq!(tree.popover_roots(), vec![p1, p2, p3]);
        let order = tree.paint_order();
        assert_eq!(order, vec![root, a, p1, p2, q, p3]);
        assert_eq!(order.len(), tree.len(), "every widget exactly once");
        assert_eq!(tree.popover_root_of(q), Some(p2));
    }

    #[test]
    fn the_later_popover_wins_an_overlap() {
        let (tree, [_, _, p1, _, _, p3]) = nested_scene();
        // (45, 65) lies in both p1 (10..50 x 30..70) and p3 (40..80 x 60..90).
        assert_eq!(tree.hit_test((45.0, 65.0)), Some(p3));
        assert_eq!(tree.hit_test((12.0, 32.0)), Some(p1));
    }

    #[test]
    fn a_popover_is_hit_outside_its_owners_bounds() {
        let (tree, [root, a, p1, p2, q, _]) = nested_scene();
        // q (16..21 x 36..41) lies wholly outside a (0..20 x 0..20).
        assert_eq!(tree.hit_test((17.0, 37.0)), Some(q));
        assert_eq!(tree.hit_test((23.0, 43.0)), Some(p2));
        assert_eq!(tree.hit_test((5.0, 5.0)), Some(a));
        assert_eq!(tree.hit_test((90.0, 10.0)), Some(root));
        let _ = p1;
    }

    #[test]
    fn a_popover_past_the_root_is_not_hit_there() {
        let (mut tree, [_, _, _, _, _, p3]) = nested_scene();
        place(&mut tree, p3, bounds(80, 80, 50, 50));
        assert_eq!(tree.hit_test((90.0, 90.0)), Some(p3), "inside the window");
        assert_eq!(tree.hit_test((110.0, 110.0)), None, "outside the window");
        assert_eq!(
            tree.hit_test((100.0, 90.0)),
            None,
            "the root's edge is half-open"
        );
    }

    /// The hit-test half of "a popover follows its owner out of sight",
    /// through nested popovers: an owner scrolled wholly out of a
    /// clipping body hides its popover *and* a popover nested inside it,
    /// so the base widget beneath is hit instead.
    #[test]
    fn a_popover_whose_owner_is_clipped_away_is_not_hit() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let clipping = Style {
            overflow: taffy::Point {
                x: taffy::Overflow::Hidden,
                y: taffy::Overflow::Hidden,
            },
            ..Style::default()
        };
        let body = match tree.insert(root, clipping, label("body"), "body") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let under = ins(&mut tree, root, "under");
        let owner = ins(&mut tree, body, "owner");
        let list = ins(&mut tree, owner, "list");
        let sub = ins(&mut tree, list, "sub");
        place(&mut tree, root, bounds(0, 0, 100, 100));
        place(&mut tree, body, bounds(0, 0, 100, 20));
        place(&mut tree, under, bounds(0, 50, 100, 50));
        place(&mut tree, owner, bounds(0, 30, 50, 10));
        place(&mut tree, list, bounds(0, 50, 50, 20));
        place(&mut tree, sub, bounds(60, 50, 30, 20));
        pop(&mut tree, list);
        pop(&mut tree, sub);
        assert_eq!(tree.hit_test((5.0, 55.0)), Some(under), "owner hidden");
        assert_eq!(tree.hit_test((65.0, 55.0)), Some(under), "nested too");
        place(&mut tree, owner, bounds(0, 15, 50, 10));
        assert_eq!(tree.hit_test((5.0, 55.0)), Some(list), "owner partly shown");
        assert_eq!(tree.hit_test((65.0, 55.0)), Some(sub));
    }

    /// The maintained popover set always equals a fresh scan of every
    /// live, non-root node flagged `Popover`, in ascending id order,
    /// across a long pseudo-random run of inserts, removals and flag
    /// changes.
    #[test]
    fn the_popover_set_tracks_every_flag_and_removal() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let mut live = vec![root];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move |n: usize| -> usize {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
        };
        for _ in 0..4000 {
            let Some(&target) = live.get(next(live.len())) else {
                unreachable!("the root is always live");
            };
            match next(4) {
                0 | 1 => live.push(ins(&mut tree, target, "n")),
                2 if target != root => {
                    if let Err(err) = tree.remove(target) {
                        unreachable!("{err:?}");
                    }
                    live.retain(|&id| tree.contains(id));
                }
                _ if target != root => {
                    let layer = if next(2) == 0 {
                        PaintLayer::Popover
                    } else {
                        PaintLayer::Base
                    };
                    if let Err(err) = tree.set_layer(target, layer) {
                        unreachable!("{err:?}");
                    }
                }
                _ => {}
            }
            let mut scanned: Vec<WidgetId> = tree
                .nodes
                .iter()
                .filter(|&(&id, node)| id != root && node.layer == PaintLayer::Popover)
                .map(|(&id, _)| id)
                .collect();
            scanned.sort_by_key(|&id| u64::from(id));
            assert_eq!(tree.popover_roots(), scanned);
        }
    }

    #[test]
    fn set_layer_dirties_the_whole_subtree_once_and_is_idempotent() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let p = ins(&mut tree, root, "p");
        let child = ins(&mut tree, p, "child");
        place(&mut tree, root, bounds(0, 0, 100, 100));
        place(&mut tree, p, bounds(10, 10, 10, 10));
        place(&mut tree, child, bounds(30, 40, 5, 5));
        let _ = tree.take_damage();
        pop(&mut tree, p);
        assert_eq!(tree.is_dirty(p), Some(true));
        assert_eq!(tree.take_damage(), Some(bounds(10, 10, 25, 35)));
        pop(&mut tree, p);
        assert_eq!(tree.take_damage(), None, "same layer again: no damage");
        if let Err(err) = tree.set_layer(p, PaintLayer::Base) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(bounds(10, 10, 25, 35)));
        assert!(tree.popover_roots().is_empty());
    }

    #[test]
    fn set_layer_rejects_the_root_and_an_unknown_id_and_changes_nothing() {
        let (mut tree, root) = WidgetTree::new(label("root"), Style::default(), "root");
        let a = ins(&mut tree, root, "a");
        place(&mut tree, root, bounds(0, 0, 10, 10));
        let _ = tree.take_damage();
        assert!(matches!(
            tree.set_layer(root, PaintLayer::Popover),
            Err(WidgetError::CannotLayerRoot(id)) if id == root
        ));
        assert_eq!(tree.layer(root), Some(PaintLayer::Base));
        assert!(tree.popover_roots().is_empty());
        assert_eq!(tree.take_damage(), None);
        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        let _ = tree.take_damage();
        assert!(matches!(
            tree.set_layer(a, PaintLayer::Popover),
            Err(WidgetError::UnknownWidget(id)) if id == a
        ));
        assert_eq!(tree.layer(a), None);
        assert_eq!(tree.take_damage(), None);
    }

    #[test]
    fn removing_a_popover_dirties_its_bounds_and_drops_it_from_the_stack() {
        let (mut tree, [_, _, p1, p2, q, p3]) = nested_scene();
        let _ = tree.take_damage();
        if let Err(err) = tree.remove(p1) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.take_damage(), Some(bounds(10, 30, 40, 40)));
        assert_eq!(tree.popover_roots(), vec![p3]);
        assert!(!tree.contains(p2) && !tree.contains(q));
    }
}
