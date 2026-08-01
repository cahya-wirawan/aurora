//! The layer tree itself: identity, nesting, and ordering. PLAN.md M1.4's
//! first deliverable.

use std::collections::HashMap;

use aurora_core::{IdGenerator, Rect};

use crate::error::DocError;
use crate::layer::{Layer, LayerEntry, LayerId, LayerKind};

/// A forest of layers: pixel layers and groups, nested to any depth.
///
/// **Ordering convention, used throughout this crate**: sibling lists
/// (both [`LayerTree::roots`] and a group's own children) are top-to-bottom
/// as a layers panel displays them — index 0 is the *topmost* layer,
/// painted last (on top) in the final composite. This is the opposite of
/// how PSD stores layers on disk (bottom layer first); `aurora-io` will
/// need to reverse one or the other when it exists. A freshly added layer
/// is inserted at index 0 (on top), matching every mainstream editor's
/// "new layer appears above the current one" behaviour.
///
/// Deliberately just two layer kinds (`Pixel`, `Group`) — see
/// [`LayerKind`]'s own doc comment for why the other nine FR-003 names
/// aren't here yet. Deliberately no opacity, blend mode, visibility, or
/// locking either — those are PLAN.md M1.4's next bullet, not this one.
pub struct LayerTree {
    ids: IdGenerator<Layer>,
    layers: HashMap<LayerId, LayerEntry>,
    /// Root-level layers. See this type's own doc comment for the
    /// ordering convention.
    roots: Vec<LayerId>,
}

impl LayerTree {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ids: IdGenerator::new(),
            layers: HashMap::new(),
            roots: Vec::new(),
        }
    }

    /// Adds a pixel layer named `name`, positioned at `bounds` in
    /// document space, as the new topmost child of `parent` (or a new
    /// topmost root, if `parent` is `None`).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `parent` is `Some` and
    /// doesn't exist, or [`DocError::NotAGroup`] if `parent` names a
    /// pixel layer. Nothing is added when this happens.
    pub fn add_pixel_layer(
        &mut self,
        name: impl Into<String>,
        bounds: Rect,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        self.insert(name.into(), parent, LayerKind::Pixel { bounds })
    }

    /// Adds an empty group named `name`, as the new topmost child of
    /// `parent` (or a new topmost root, if `parent` is `None`).
    ///
    /// # Errors
    ///
    /// Same as [`Self::add_pixel_layer`].
    pub fn add_group(
        &mut self,
        name: impl Into<String>,
        parent: Option<LayerId>,
    ) -> Result<LayerId, DocError> {
        self.insert(
            name.into(),
            parent,
            LayerKind::Group {
                children: Vec::new(),
            },
        )
    }

    fn insert(
        &mut self,
        name: String,
        parent: Option<LayerId>,
        kind: LayerKind,
    ) -> Result<LayerId, DocError> {
        // Validate `parent` before touching `self.layers` at all, so a
        // failed call adds nothing -- same "all or nothing" discipline
        // `aurora_graph::RenderGraph::add_node` uses for its own inputs.
        if let Some(parent_id) = parent {
            match self.layers.get(&parent_id) {
                None => return Err(DocError::UnknownLayer(parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(parent_id));
                }
                Some(_) => {}
            }
        }

        let id = self.ids.next_id();
        self.layers.insert(id, LayerEntry::new(name, parent, kind));

        let siblings = match self.sibling_list_mut(parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!(
                    "parent's existence and group-ness were already validated above: {err:?}"
                )
            }
        };
        siblings.insert(0, id);
        Ok(id)
    }

    /// Removes `id` from the tree. If `id` is a group, every descendant
    /// is removed too — a plain delete removes a group's contents along
    /// with it, matching every mainstream editor's actual behaviour
    /// (there's no implicit "flatten children up a level" on delete).
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn remove(&mut self, id: LayerId) -> Result<(), DocError> {
        let entry = self.layers.remove(&id).ok_or(DocError::UnknownLayer(id))?;

        let siblings = match self.sibling_list_mut(entry.parent) {
            Ok(list) => list,
            Err(err) => unreachable!("id's own recorded parent must be valid: {err:?}"),
        };
        siblings.retain(|&sibling| sibling != id);

        self.remove_subtree_contents(entry.kind);
        Ok(())
    }

    /// Removes every descendant named by `kind` (a moved-out, already-
    /// detached `LayerKind`), without touching any sibling list — by the
    /// time a grandchild is reached, its immediate parent's own entry (and
    /// thus the list a naive detach would look for) is already gone.
    fn remove_subtree_contents(&mut self, kind: LayerKind) {
        if let LayerKind::Group { children } = kind {
            for child in children {
                let Some(child_entry) = self.layers.remove(&child) else {
                    unreachable!(
                        "a group's recorded children must exist in the tree by construction"
                    );
                };
                self.remove_subtree_contents(child_entry.kind);
            }
        }
    }

    /// Moves `id` (and, if it's a group, its whole subtree) to be a child
    /// of `new_parent` at sibling position `index`, clamped to the valid
    /// range — an out-of-range `index` lands at the end rather than
    /// erroring, the same forgiving behaviour a UI drag-and-drop drop
    /// target needs.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` or `new_parent` (when
    /// `Some`) doesn't exist, [`DocError::NotAGroup`] if `new_parent`
    /// names a pixel layer, or [`DocError::CycleDetected`] if
    /// `new_parent` is `id` itself or one of `id`'s own descendants.
    /// Nothing is changed when this happens.
    pub fn reparent(
        &mut self,
        id: LayerId,
        new_parent: Option<LayerId>,
        index: usize,
    ) -> Result<(), DocError> {
        let old_parent = match self.layers.get(&id) {
            Some(entry) => entry.parent,
            None => return Err(DocError::UnknownLayer(id)),
        };

        if let Some(new_parent_id) = new_parent {
            if new_parent_id == id || self.is_descendant(new_parent_id, id) {
                return Err(DocError::CycleDetected {
                    id,
                    new_parent: new_parent_id,
                });
            }
            match self.layers.get(&new_parent_id) {
                None => return Err(DocError::UnknownLayer(new_parent_id)),
                Some(entry) if !entry.kind.is_group() => {
                    return Err(DocError::NotAGroup(new_parent_id));
                }
                Some(_) => {}
            }
        }

        // Everything validated -- detach from the old position...
        let old_siblings = match self.sibling_list_mut(old_parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!("id's current parent, just read above, must be valid: {err:?}")
            }
        };
        old_siblings.retain(|&sibling| sibling != id);

        // ...then attach at the new one.
        let new_siblings = match self.sibling_list_mut(new_parent) {
            Ok(list) => list,
            Err(err) => {
                unreachable!(
                    "new_parent's existence and group-ness were already validated: {err:?}"
                )
            }
        };
        let clamped = index.min(new_siblings.len());
        new_siblings.insert(clamped, id);

        let Some(entry) = self.layers.get_mut(&id) else {
            unreachable!("id's existence was already confirmed above");
        };
        entry.parent = new_parent;
        Ok(())
    }

    /// Whether `descendant` is nested anywhere inside `ancestor`'s
    /// subtree — [`Self::reparent`]'s cycle guard. Walks upward from
    /// `descendant` through its own chain of parents (bounded by tree
    /// depth) rather than downward through `ancestor`'s whole subtree
    /// (which could be large), since the answer only needs one path, not
    /// an exhaustive search.
    fn is_descendant(&self, descendant: LayerId, ancestor: LayerId) -> bool {
        let Some(entry) = self.layers.get(&descendant) else {
            return false;
        };
        match entry.parent {
            Some(parent) if parent == ancestor => true,
            Some(parent) => self.is_descendant(parent, ancestor),
            None => false,
        }
    }

    /// The sibling list `parent` names: [`Self::roots`] if `None`, or a
    /// group's own children if `Some`. The single place every
    /// insert/remove/reparent path goes through to find "the list `id`
    /// lives in."
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] or [`DocError::NotAGroup`] —
    /// see [`Self::add_pixel_layer`]'s doc comment.
    fn sibling_list_mut(&mut self, parent: Option<LayerId>) -> Result<&mut Vec<LayerId>, DocError> {
        match parent {
            None => Ok(&mut self.roots),
            Some(parent_id) => {
                let entry = self
                    .layers
                    .get_mut(&parent_id)
                    .ok_or(DocError::UnknownLayer(parent_id))?;
                match &mut entry.kind {
                    LayerKind::Group { children } => Ok(children),
                    LayerKind::Pixel { .. } => Err(DocError::NotAGroup(parent_id)),
                }
            }
        }
    }

    #[must_use]
    pub fn contains(&self, id: LayerId) -> bool {
        self.layers.contains_key(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// `None` both when `id` doesn't exist and when it's a root layer —
    /// callers that need to tell those apart should check
    /// [`Self::contains`] first, matching `aurora_graph::RenderGraph`'s
    /// own `payload`/`dependencies` convention.
    #[must_use]
    pub fn parent(&self, id: LayerId) -> Option<LayerId> {
        self.layers.get(&id).and_then(|entry| entry.parent)
    }

    #[must_use]
    pub fn kind(&self, id: LayerId) -> Option<&LayerKind> {
        self.layers.get(&id).map(|entry| &entry.kind)
    }

    #[must_use]
    pub fn name(&self, id: LayerId) -> Option<&str> {
        self.layers.get(&id).map(|entry| entry.name.as_str())
    }

    /// # Errors
    ///
    /// Returns [`DocError::UnknownLayer`] if `id` doesn't exist.
    pub fn set_name(&mut self, id: LayerId, name: impl Into<String>) -> Result<(), DocError> {
        let entry = self.layers.get_mut(&id).ok_or(DocError::UnknownLayer(id))?;
        entry.name = name.into();
        Ok(())
    }

    /// Root-level layers, top-to-bottom (see this type's own doc comment
    /// for the ordering convention).
    #[must_use]
    pub fn roots(&self) -> &[LayerId] {
        &self.roots
    }

    /// `None` if `id` doesn't exist, or if it exists but is a pixel layer
    /// (which structurally has no children). `Some(&[])` is a real,
    /// expected result for an empty group — callers that need to
    /// distinguish "doesn't exist" from "is a pixel layer" should check
    /// [`Self::contains`]/[`Self::kind`] directly.
    #[must_use]
    pub fn children(&self, id: LayerId) -> Option<&[LayerId]> {
        match self.kind(id)? {
            LayerKind::Group { children } => Some(children.as_slice()),
            LayerKind::Pixel { .. } => None,
        }
    }
}

impl Default for LayerTree {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LayerTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerTree")
            .field("layer_count", &self.layers.len())
            .field("root_count", &self.roots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::LayerTree;
    use crate::DocError;
    use crate::layer::{Layer, LayerKind};
    use aurora_core::{Id, Rect};

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn new_tree_is_empty() {
        let tree = LayerTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
        assert!(tree.roots().is_empty());
    }

    #[test]
    fn add_pixel_layer_and_group_at_root_newest_on_top() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_group("b", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // b was added after a, so it must be on top (index 0).
        assert_eq!(tree.roots(), [b, a]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.name(a), Some("a"));
        assert_eq!(tree.name(b), Some("b"));
    }

    #[test]
    fn add_pixel_layer_records_its_bounds_via_kind() {
        let mut tree = LayerTree::new();
        let rect = bounds();
        let id = match tree.add_pixel_layer("a", rect, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.kind(id), Some(&LayerKind::Pixel { bounds: rect }));
    }

    #[test]
    fn add_nested_layer_inside_a_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("child", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(group), Some([child].as_slice()));
        assert_eq!(tree.parent(child), Some(group));
        assert_eq!(tree.roots(), [group], "child must not also be a root");
    }

    #[test]
    fn add_rejects_an_unknown_parent() {
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(41);
        match tree.add_pixel_layer("a", bounds(), Some(bogus)) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
        assert!(tree.is_empty(), "a failed add must add nothing");
    }

    #[test]
    fn add_rejects_a_non_group_parent() {
        let mut tree = LayerTree::new();
        let pixel = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.add_pixel_layer("b", bounds(), Some(pixel)) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, pixel),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
        assert_eq!(tree.len(), 1, "a failed add must add nothing");
    }

    #[test]
    fn kind_and_children_are_none_for_an_unknown_id() {
        let tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        assert_eq!(tree.kind(bogus), None);
        assert_eq!(tree.children(bogus), None);
        assert_eq!(tree.parent(bogus), None);
        assert_eq!(tree.name(bogus), None);
        assert!(!tree.contains(bogus));
    }

    #[test]
    fn children_is_none_for_a_pixel_layer() {
        let mut tree = LayerTree::new();
        let pixel = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            tree.children(pixel),
            None,
            "a pixel layer has no children slot to return, unlike an empty group"
        );
    }

    #[test]
    fn children_is_some_empty_for_a_fresh_group() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.children(group), Some([].as_slice()));
    }

    #[test]
    fn set_name_updates_and_rejects_unknown_id() {
        let mut tree = LayerTree::new();
        let id = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_name(id, "renamed") {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.name(id), Some("renamed"));

        let bogus: super::LayerId = Id::from_raw(999);
        match tree.set_name(bogus, "x") {
            Err(DocError::UnknownLayer(got)) => assert_eq!(got, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn remove_detaches_a_root_layer() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(a) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(a));
        assert_eq!(tree.roots(), [b]);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn remove_updates_the_parent_group_children_list() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(child) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.children(group), Some([].as_slice()));
    }

    #[test]
    fn remove_cascades_into_every_descendant() {
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_group("inner", Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let leaf = match tree.add_pixel_layer("leaf", bounds(), Some(inner)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.remove(outer) {
            unreachable!("{err:?}");
        }
        assert!(!tree.contains(outer));
        assert!(!tree.contains(inner), "nested group must also be removed");
        assert!(
            !tree.contains(leaf),
            "leaf two levels down must also be removed"
        );
        assert!(tree.is_empty());
    }

    #[test]
    fn remove_rejects_an_unknown_id() {
        let mut tree = LayerTree::new();
        let bogus: super::LayerId = Id::from_raw(0);
        match tree.remove(bogus) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn reparent_moves_a_layer_between_groups() {
        let mut tree = LayerTree::new();
        let a = match tree.add_group("a", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_group("b", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(a)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        if let Err(err) = tree.reparent(child, Some(b), 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.children(a), Some([].as_slice()));
        assert_eq!(tree.children(b), Some([child].as_slice()));
        assert_eq!(tree.parent(child), Some(b));
    }

    #[test]
    fn reparent_can_move_a_layer_to_root() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match tree.add_pixel_layer("c", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.reparent(child, None, 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.parent(child), None);
        assert_eq!(tree.roots(), [child, group]);
    }

    #[test]
    fn reparent_reorders_within_the_same_parent() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let c = match tree.add_pixel_layer("c", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Insertion order puts newest on top: roots = [c, b, a].
        assert_eq!(tree.roots(), [c, b, a]);

        // Move a (currently bottom) to the very top.
        if let Err(err) = tree.reparent(a, None, 0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.roots(), [a, c, b]);
    }

    #[test]
    fn reparent_clamps_an_out_of_range_index_to_the_end() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // roots = [b, a]; move b far past the end.
        if let Err(err) = tree.reparent(b, None, 999) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.roots(), [a, b]);
    }

    #[test]
    fn reparent_rejects_a_cycle_under_self() {
        let mut tree = LayerTree::new();
        let group = match tree.add_group("g", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(group, Some(group), 0) {
            Err(DocError::CycleDetected { id, new_parent }) => {
                assert_eq!(id, group);
                assert_eq!(new_parent, group);
            }
            other => unreachable!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn reparent_rejects_a_cycle_under_a_descendant() {
        let mut tree = LayerTree::new();
        let outer = match tree.add_group("outer", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner = match tree.add_group("inner", Some(outer)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(outer, Some(inner), 0) {
            Err(DocError::CycleDetected { id, new_parent }) => {
                assert_eq!(id, outer);
                assert_eq!(new_parent, inner);
            }
            other => unreachable!("expected CycleDetected, got {other:?}"),
        }
        // Must be unchanged.
        assert_eq!(tree.parent(inner), Some(outer));
        assert_eq!(tree.parent(outer), None);
    }

    #[test]
    fn reparent_rejects_a_non_group_target() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match tree.add_pixel_layer("b", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.reparent(a, Some(b), 0) {
            Err(DocError::NotAGroup(id)) => assert_eq!(id, b),
            other => unreachable!("expected NotAGroup, got {other:?}"),
        }
    }

    #[test]
    fn reparent_rejects_unknown_ids() {
        let mut tree = LayerTree::new();
        let a = match tree.add_pixel_layer("a", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let bogus: super::LayerId = Id::from_raw(999);

        match tree.reparent(bogus, None, 0) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
        match tree.reparent(a, Some(bogus), 0) {
            Err(DocError::UnknownLayer(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownLayer, got {other:?}"),
        }
    }

    #[test]
    fn default_is_an_empty_tree() {
        let tree = LayerTree::default();
        assert!(tree.is_empty());
    }

    #[test]
    fn layer_kind_is_group_distinguishes_the_two_kinds() {
        assert!(!LayerKind::Pixel { bounds: bounds() }.is_group());
        assert!(
            LayerKind::Group {
                children: Vec::new()
            }
            .is_group()
        );
    }

    /// Not a functional test -- `Layer` is a zero-variant marker type only
    /// ever named via `PhantomData` (see `aurora_core::Id`'s own doc
    /// comment). This just confirms the type is actually exported and
    /// usable as `Id<Layer>` from outside the crate, the way `LayerId`
    /// itself is defined.
    #[test]
    fn layer_marker_type_is_exported() {
        let _id: Id<Layer> = Id::from_raw(0);
    }
}
