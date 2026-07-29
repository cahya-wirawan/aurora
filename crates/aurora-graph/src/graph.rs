//! The render graph itself: a DAG of nodes, dependency queries, and
//! dirty-region propagation.

use std::collections::{HashSet, VecDeque};
use std::fmt;

use aurora_core::{IdGenerator, Rect};

use crate::error::GraphError;
use crate::node::{Node, NodeEntry, NodeId};

/// A directed acyclic graph of render nodes — the shape invariant §7.3.2
/// requires (adjustments, filters, and smart objects are nodes here,
/// never baked pixels), reduced to just that shape plus dirty-region
/// tracking. `aurora-graph` may depend only on `aurora-core` and
/// `aurora-tile` (PRD §7.2), so it has no way to know what a node
/// actually computes — that's `aurora-render`'s job to execute and
/// `aurora-filters`'/`aurora-doc`'s job to define, both layered above
/// this crate. `RenderGraph<N>` is generic over the payload `N` for
/// exactly that reason: this crate only ever moves `N` around opaquely.
///
/// Node removal and edge rewiring (needed once `aurora-doc`'s layer tree
/// drives real edits — delete a layer, reorder, insert an adjustment
/// mid-stack) are deliberately not implemented yet. This first pass is
/// node definitions, the dependency DAG, and dirty propagation (PLAN.md
/// M1.3) — the shape those later mutations will need to build on.
pub struct RenderGraph<N> {
    ids: IdGenerator<Node>,
    nodes: Vec<NodeEntry<N>>,
}

impl<N> RenderGraph<N> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            ids: IdGenerator::new(),
            nodes: Vec::new(),
        }
    }

    /// Adds a node carrying `payload`, depending on `inputs`.
    ///
    /// Every id in `inputs` must already exist in this graph — created by
    /// an earlier call to this method, on this same graph. That
    /// requirement is what makes this a DAG *by construction* rather than
    /// by a separate cycle check afterward: a node can only ever name
    /// nodes that came before it, so a cycle back to itself or an
    /// ancestor is structurally unreachable. The same property makes
    /// insertion order always a valid topological order for free — see
    /// [`RenderGraph::iter`].
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] naming the first entry in
    /// `inputs` that doesn't exist yet. Nothing is added when this
    /// happens — either every input is valid and the node is added, or
    /// none of it happens.
    pub fn add_node(&mut self, payload: N, inputs: &[NodeId]) -> Result<NodeId, GraphError> {
        for &input in inputs {
            if !self.contains(input) {
                return Err(GraphError::UnknownNode(input));
            }
        }

        let id = self.ids.next_id();
        for &input in inputs {
            if let Some(entry) = self.entry_mut(input) {
                entry.dependents.push(id);
            }
        }
        self.nodes.push(NodeEntry::new(payload, inputs.to_vec()));
        Ok(id)
    }

    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        self.entry(id).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn payload(&self, id: NodeId) -> Option<&N> {
        self.entry(id).map(|entry| &entry.payload)
    }

    pub fn payload_mut(&mut self, id: NodeId) -> Option<&mut N> {
        self.entry_mut(id).map(|entry| &mut entry.payload)
    }

    /// `None` if `id` doesn't exist; `Some(&[])` if it exists but has no
    /// dependencies — callers that need to tell those apart can, unlike
    /// if this returned an empty slice for both.
    #[must_use]
    pub fn dependencies(&self, id: NodeId) -> Option<&[NodeId]> {
        self.entry(id).map(|entry| entry.inputs.as_slice())
    }

    /// Same `None`-vs-`Some(&[])` distinction as [`RenderGraph::dependencies`],
    /// for the reverse edges.
    #[must_use]
    pub fn dependents(&self, id: NodeId) -> Option<&[NodeId]> {
        self.entry(id).map(|entry| entry.dependents.as_slice())
    }

    /// Every node's id, in insertion order — always a valid topological
    /// order (see [`RenderGraph::add_node`]'s doc comment for why).
    pub fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        #[allow(clippy::cast_possible_truncation)]
        (0..self.nodes.len()).map(|index| NodeId::from_raw(index as u64))
    }

    /// Marks `id`'s own output dirty in `region`, and propagates that
    /// same region to every node that transitively depends on it — `id`
    /// included, so one call covers however deep its dependents go.
    ///
    /// Deliberately conservative: the propagated region is identical at
    /// every step rather than growing per node (e.g. a blur widening its
    /// footprint by its radius). This crate has no way to know what a
    /// node computes, so identity propagation is the safe default —
    /// correct (every truly-affected region is included) even if it marks
    /// more than the tightest possible region dirty in some cases. A
    /// kernel-aware growth hook is future work for once node payloads can
    /// answer "how far does this operation's influence reach," which
    /// needs `aurora-filters` to exist first.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::UnknownNode`] if `id` doesn't exist.
    pub fn mark_dirty(&mut self, id: NodeId, region: Rect) -> Result<(), GraphError> {
        if !self.contains(id) {
            return Err(GraphError::UnknownNode(id));
        }

        // BFS with a `visited` set, rather than recursing into
        // `mark_dirty` per dependent: in a diamond-shaped dependency (two
        // paths reconverging), that would union the same region into the
        // shared descendant once per incoming path rather than once per
        // call. Still *correct* either way (`Rect::union` is idempotent),
        // but `visited` keeps it linear instead of blowing up
        // combinatorially on a graph with many reconverging paths.
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        queue.push_back(id);
        visited.insert(id);

        while let Some(current) = queue.pop_front() {
            // Structurally unreachable (every id in `queue` came from
            // `id` itself or from another node's already-valid
            // `dependents` list) — skipped rather than trusted with a
            // panic-adjacent unwrap, consistent with this crate's own
            // "return errors, don't assume" stance elsewhere.
            let Some(entry) = self.entry_mut(current) else {
                continue;
            };
            entry.dirty = Some(match entry.dirty {
                Some(existing) => existing.union(&region),
                None => region,
            });
            let dependents = entry.dependents.clone();
            for dependent in dependents {
                if visited.insert(dependent) {
                    queue.push_back(dependent);
                }
            }
        }
        Ok(())
    }

    /// Takes and clears `id`'s accumulated dirty region, if any. `None`
    /// both when `id` doesn't exist and when it exists but has nothing
    /// dirty — callers that need to tell those apart should check
    /// [`RenderGraph::contains`] first.
    pub fn take_dirty(&mut self, id: NodeId) -> Option<Rect> {
        self.entry_mut(id).and_then(|entry| entry.dirty.take())
    }

    /// Peeks `id`'s accumulated dirty region without clearing it.
    #[must_use]
    pub fn peek_dirty(&self, id: NodeId) -> Option<Rect> {
        self.entry(id).and_then(|entry| entry.dirty)
    }

    fn entry(&self, id: NodeId) -> Option<&NodeEntry<N>> {
        self.nodes.get(usize::try_from(id.to_raw()).ok()?)
    }

    fn entry_mut(&mut self, id: NodeId) -> Option<&mut NodeEntry<N>> {
        self.nodes.get_mut(usize::try_from(id.to_raw()).ok()?)
    }
}

impl<N> Default for RenderGraph<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Hand-implemented, not derived: `#[derive(Debug)]` would add a
/// `N: Debug` bound even though this impl never actually prints a
/// payload — the same reasoning `aurora_core::Id`'s own hand-rolled
/// impls document.
impl<N> fmt::Debug for RenderGraph<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderGraph")
            .field("node_count", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::RenderGraph;
    use crate::GraphError;
    use aurora_core::{Id, Rect};

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn add_node_assigns_sequential_ids_and_grows_len() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        assert!(graph.is_empty());
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("no inputs, cannot fail: {err:?}"),
        };
        let b = match graph.add_node("b", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("no inputs, cannot fail: {err:?}"),
        };
        assert_ne!(a, b);
        assert_eq!(graph.len(), 2);
        assert!(!graph.is_empty());
    }

    #[test]
    fn add_node_rejects_an_input_that_does_not_exist_yet() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let bogus: super::NodeId = Id::from_raw(41);
        match graph.add_node("a", &[bogus]) {
            Err(GraphError::UnknownNode(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownNode, got {other:?}"),
        }
        assert!(graph.is_empty(), "a failed add_node must add nothing");
    }

    #[test]
    fn dependencies_and_dependents_track_both_directions() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match graph.add_node("b", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let c = match graph.add_node("c", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(graph.dependencies(b), Some([a].as_slice()));
        assert_eq!(graph.dependencies(a), Some([].as_slice()));
        assert_eq!(graph.dependents(a), Some([b, c].as_slice()));
        assert_eq!(graph.dependents(b), Some([].as_slice()));
    }

    #[test]
    fn dependencies_and_dependents_are_none_for_an_unknown_node() {
        let graph: RenderGraph<&str> = RenderGraph::new();
        let bogus: super::NodeId = Id::from_raw(0);
        assert_eq!(graph.dependencies(bogus), None);
        assert_eq!(graph.dependents(bogus), None);
        assert_eq!(graph.payload(bogus), None);
    }

    #[test]
    fn iter_yields_insertion_order_which_is_topological() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match graph.add_node("b", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let c = match graph.add_node("c", &[a, b]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(graph.iter().collect::<Vec<_>>(), vec![a, b, c]);
    }

    #[test]
    fn payload_accessors_read_and_write() {
        let mut graph: RenderGraph<i32> = RenderGraph::new();
        let a = match graph.add_node(10, &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(graph.payload(a), Some(&10));
        if let Some(value) = graph.payload_mut(a) {
            *value = 20;
        }
        assert_eq!(graph.payload(a), Some(&20));
    }

    #[test]
    fn mark_dirty_marks_the_node_itself() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let region = rect(0, 0, 10, 10);
        match graph.mark_dirty(a, region) {
            Ok(()) => {}
            Err(err) => unreachable!("{err:?}"),
        }
        assert_eq!(graph.peek_dirty(a), Some(region));
    }

    #[test]
    fn mark_dirty_propagates_through_a_diamond_to_every_dependent() {
        // a -> b -> d
        // a -> c -> d
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match graph.add_node("b", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let c = match graph.add_node("c", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let d = match graph.add_node("d", &[b, c]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let region = rect(1, 2, 3, 4);
        match graph.mark_dirty(a, region) {
            Ok(()) => {}
            Err(err) => unreachable!("{err:?}"),
        }

        assert_eq!(graph.peek_dirty(a), Some(region));
        assert_eq!(graph.peek_dirty(b), Some(region));
        assert_eq!(graph.peek_dirty(c), Some(region));
        assert_eq!(
            graph.peek_dirty(d),
            Some(region),
            "must reach d through both b and c"
        );
    }

    #[test]
    fn mark_dirty_accumulates_via_union_like_tile_does() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match graph.mark_dirty(a, rect(0, 0, 10, 10)) {
            Ok(()) => {}
            Err(err) => unreachable!("{err:?}"),
        }
        match graph.mark_dirty(a, rect(5, 5, 10, 10)) {
            Ok(()) => {}
            Err(err) => unreachable!("{err:?}"),
        }
        assert_eq!(graph.take_dirty(a), Some(rect(0, 0, 15, 15)));
        assert_eq!(graph.take_dirty(a), None, "cleared after taking");
    }

    #[test]
    fn mark_dirty_rejects_an_unknown_node() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let bogus: super::NodeId = Id::from_raw(0);
        match graph.mark_dirty(bogus, rect(0, 0, 1, 1)) {
            Err(GraphError::UnknownNode(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownNode, got {other:?}"),
        }
    }

    #[test]
    fn take_dirty_is_none_for_an_unknown_node() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let bogus: super::NodeId = Id::from_raw(0);
        assert_eq!(graph.take_dirty(bogus), None);
    }

    #[test]
    fn default_is_an_empty_graph() {
        let graph: RenderGraph<&str> = RenderGraph::default();
        assert!(graph.is_empty());
    }
}
