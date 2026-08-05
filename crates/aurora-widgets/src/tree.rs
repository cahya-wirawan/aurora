//! The retained-mode widget tree: identity, nesting, damage tracking, and
//! a required accessibility node per widget (invariant §7.3.9). PLAN.md
//! M1.7's first deliverable.

use std::collections::HashMap;

use accesskit::{Node as AccessibilityNode, NodeId, Tree, TreeId, TreeUpdate};
use aurora_core::Rect;
use taffy::{AvailableSpace, Size as LayoutSize, Style as LayoutStyle, TaffyTree};

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
                payload,
            },
        );
        (
            Self {
                nodes,
                root,
                next_id: 1,
                damage: None,
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
    #[must_use]
    pub fn hit_test(&self, point: (f32, f32)) -> Option<WidgetId> {
        self.hit_test_from(self.root, point)
    }

    fn hit_test_from(&self, id: WidgetId, point: (f32, f32)) -> Option<WidgetId> {
        let node = self.nodes.get(&id)?;
        if !bounds_contain(node.bounds, point) {
            return None;
        }
        for &child in node.children.iter().rev() {
            if let Some(hit) = self.hit_test_from(child, point) {
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
        // Both the vacated region and the newly occupied one need
        // repainting, not just the new position.
        self.mark_region_dirty(old_bounds);
        self.mark_region_dirty(bounds);
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
        TreeUpdate {
            nodes,
            tree: Some(Tree::new(self.root)),
            tree_id: TreeId::ROOT,
            focus,
        }
    }
}

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
    use super::WidgetTree;
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
}
