//! The retained-mode widget tree: identity, nesting, damage tracking, and
//! a required accessibility node per widget (invariant §7.3.9). PLAN.md
//! M1.7's first deliverable.

use std::collections::HashMap;

use accesskit::{Node as AccessibilityNode, NodeId, Tree, TreeId, TreeUpdate};
use aurora_core::Rect;

use crate::error::WidgetError;

/// A widget's identity. This *is* `accesskit::NodeId`, not a wrapper
/// around it — invariant §7.3.9 says every widget carries an accessKit
/// node "as part of its definition, not a pass"; making the tree's own
/// identity and the accessibility node's identity the literal same value
/// means there is no separate id space to keep in sync or forget to.
pub type WidgetId = NodeId;

struct WidgetNode<W> {
    parent: Option<WidgetId>,
    /// Paint/tab order, first to last — unlike `aurora_doc::LayerTree`'s
    /// "newest on top, insert at index 0" convention, a widget tree has
    /// no natural "on top" for a fresh child the way a layers panel does;
    /// new children are appended at the end, the same convention
    /// `Node::push_child` (this module's `insert`) and every mainstream
    /// UI toolkit's "append child" already use.
    children: Vec<WidgetId>,
    bounds: Rect,
    accessibility: AccessibilityNode,
    dirty: bool,
    payload: W,
}

/// A retained-mode tree of widgets: exactly one root (unlike
/// [`aurora_doc::LayerTree`]'s multiple top-level layers — an
/// application has one root window, not several independent ones),
/// arbitrary nesting below it, per-widget damage tracking, and a
/// required [`accesskit::Node`] on every widget from the moment it's
/// created.
///
/// Layout (this crate's own layout engine, PLAN.md M1.7) and input/focus
/// routing are separate, later pieces layered on top of this structure —
/// this type owns identity, nesting, bounds, damage, and accessibility
/// content only.
pub struct WidgetTree<W> {
    nodes: HashMap<WidgetId, WidgetNode<W>>,
    root: WidgetId,
    next_id: u64,
    /// Accumulated screen-space damage since the last
    /// [`Self::take_damage`] — same `Option<Rect>` +
    /// [`aurora_core::Rect::union`] accumulation idiom
    /// `aurora_tile::Tile::mark_dirty`/`take_dirty` already use.
    damage: Option<Rect>,
}

impl<W> WidgetTree<W> {
    /// Creates a new tree with `payload` as its root widget, covering
    /// `bounds`, described by `accessibility`. Returns the tree and the
    /// root's id (always `NodeId(0)`, but returned rather than assumed,
    /// so callers never hardcode it).
    #[must_use]
    pub fn new(accessibility: AccessibilityNode, bounds: Rect, payload: W) -> (Self, WidgetId) {
        let root = WidgetId::from(0);
        let mut nodes = HashMap::new();
        nodes.insert(
            root,
            WidgetNode {
                parent: None,
                children: Vec::new(),
                bounds,
                accessibility,
                dirty: true,
                payload,
            },
        );
        (
            Self {
                nodes,
                root,
                next_id: 1,
                damage: Some(bounds),
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

    /// Adds a new widget as the last child of `parent`, covering `bounds`,
    /// described by `accessibility`. Marks the new widget's own bounds
    /// dirty.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
    /// Nothing is added when this happens.
    pub fn insert(
        &mut self,
        parent: WidgetId,
        bounds: Rect,
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
                bounds,
                accessibility,
                dirty: true,
                payload,
            },
        );

        let Some(parent_node) = self.nodes.get_mut(&parent) else {
            unreachable!("parent's existence was already checked above");
        };
        parent_node.children.push(id);

        self.mark_region_dirty(bounds);
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
        self.mark_region_dirty(node.bounds);
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
        // Both the vacated region and the newly occupied one need
        // repainting, not just the new position.
        self.mark_region_dirty(old_bounds);
        self.mark_region_dirty(bounds);
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
        let bounds = node.bounds;
        self.mark_region_dirty(bounds);
        Ok(())
    }

    fn mark_region_dirty(&mut self, region: Rect) {
        self.damage = Some(match self.damage {
            Some(existing) => existing.union(&region),
            None => region,
        });
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
    /// per the a11y spike) actually sends to the screen reader.
    #[must_use]
    pub fn accessibility_update(&self, focus: WidgetId) -> TreeUpdate {
        let nodes = self
            .nodes
            .iter()
            .map(|(&id, node)| (id, node.accessibility.clone()))
            .collect();
        TreeUpdate {
            nodes,
            tree: Some(Tree::new(self.root)),
            tree_id: TreeId::ROOT,
            focus,
        }
    }
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
    use super::WidgetTree;
    use crate::WidgetError;
    use accesskit::{Node, Role};
    use aurora_core::Rect;

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

    #[test]
    fn new_tree_has_exactly_the_root() {
        let (tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        assert_eq!(tree.len(), 1);
        assert!(!tree.is_empty());
        assert_eq!(tree.root(), root);
        assert!(tree.contains(root));
        assert_eq!(tree.parent(root), None);
        assert_eq!(tree.children(root), Some([].as_slice()));
        assert_eq!(tree.payload(root), Some(&"root"));
    }

    #[test]
    fn insert_adds_a_child_at_the_end_and_marks_it_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let a = match tree.insert(root, bounds(0, 0, 10, 10), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.insert(root, bounds(10, 0, 10, 10), label("b"), "b") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(root), Some([a, b].as_slice()));
        assert_eq!(tree.parent(a), Some(root));
        assert_eq!(tree.is_dirty(b), Some(true));
    }

    #[test]
    fn insert_rejects_an_unknown_parent() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let bogus = accesskit::NodeId(999);
        match tree.insert(bogus, bounds(0, 0, 1, 1), label("x"), "x") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "a failed insert must add nothing");
        let _ = root;
    }

    #[test]
    fn remove_detaches_a_leaf_and_updates_the_parent() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let a = match tree.insert(root, bounds(0, 0, 10, 10), label("a"), "a") {
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
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let group = match tree.insert(root, bounds(0, 0, 50, 50), label("group"), "group") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.insert(group, bounds(0, 0, 10, 10), label("leaf"), "leaf") {
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
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        match tree.remove(root) {
            Err(WidgetError::CannotRemoveRoot(id)) => assert_eq!(id, root),
            other => unreachable!("expected CannotRemoveRoot, got {other:?}"),
        }
        assert!(tree.contains(root));
    }

    #[test]
    fn remove_rejects_an_unknown_id() {
        let (mut tree, _root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let bogus = accesskit::NodeId(999);
        match tree.remove(bogus) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_bounds_updates_and_marks_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let a = match tree.insert(root, bounds(0, 0, 10, 10), label("a"), "a") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage(); // clear the insert's own damage first.

        if let Err(err) = tree.set_bounds(a, bounds(5, 5, 10, 10)) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.bounds(a), Some(bounds(5, 5, 10, 10)));
        assert_eq!(tree.is_dirty(a), Some(true));
    }

    #[test]
    fn set_bounds_rejects_an_unknown_id() {
        let (mut tree, _root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let bogus = accesskit::NodeId(999);
        match tree.set_bounds(bogus, bounds(0, 0, 1, 1)) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn take_damage_accumulates_via_union_and_clears() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 10, 10), "root");
        // The root's own creation already dirtied (0,0,10,10). Clear it
        // first so this test's own assertions are about its own inserts.
        tree.take_damage();

        if let Err(err) = tree.insert(root, bounds(0, 0, 5, 5), label("a"), "a") {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.insert(root, bounds(20, 20, 5, 5), label("b"), "b") {
            unreachable!("{err:?}");
        }

        let damage = tree.take_damage();
        assert_eq!(
            damage,
            Some(bounds(0, 0, 5, 5).union(&bounds(20, 20, 5, 5))),
            "damage must be the union of every dirtied region since the last take"
        );

        assert_eq!(
            tree.take_damage(),
            None,
            "damage must be cleared after being taken"
        );
    }

    #[test]
    fn take_damage_clears_every_widgets_own_dirty_flag() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 10, 10), "root");
        let a = match tree.insert(root, bounds(0, 0, 5, 5), label("a"), "a") {
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
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let a = match tree.insert(root, bounds(0, 0, 10, 10), label("a"), "a") {
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

    #[test]
    fn set_accessibility_replaces_the_node_and_marks_dirty() {
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        let a = match tree.insert(root, bounds(0, 0, 10, 10), label("a"), "a") {
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
        let (mut tree, root) = WidgetTree::new(label("root"), bounds(0, 0, 100, 100), "root");
        if let Some(payload) = tree.payload_mut(root) {
            *payload = "renamed root";
        }
        assert_eq!(tree.payload(root), Some(&"renamed root"));
    }
}
