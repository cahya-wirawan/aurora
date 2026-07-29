//! Node identity and the per-node bookkeeping a [`crate::RenderGraph`] keeps.

use aurora_core::{Id, Rect};

/// Marker type for [`NodeId`] — never constructed, just named. See
/// `aurora_core::Id`'s own doc comment, which names `Id<Node>` as exactly
/// this kind of use case.
#[derive(Debug)]
pub struct Node;

/// Identifies one node within a single [`crate::RenderGraph`].
///
/// Graph-local, not global: two different `RenderGraph`s may each have
/// their own, unrelated node at `NodeId::from_raw(0)`. Nothing in this
/// crate enforces that a `NodeId` came from the particular graph it's
/// passed to — an id from a different graph, or one made up out of thin
/// air, surfaces as [`crate::GraphError::UnknownNode`] rather than
/// silently doing the wrong thing, since every `RenderGraph` method looks
/// the id up before trusting it.
pub type NodeId = Id<Node>;

/// One node's bookkeeping: the caller-supplied payload `N` (this crate
/// never inspects it — see [`crate::RenderGraph`]'s own doc comment for
/// why), its dependency/dependent edges, and its accumulated dirty
/// region.
pub(crate) struct NodeEntry<N> {
    pub(crate) payload: N,
    pub(crate) inputs: Vec<NodeId>,
    pub(crate) dependents: Vec<NodeId>,
    pub(crate) dirty: Option<Rect>,
}

impl<N> NodeEntry<N> {
    pub(crate) fn new(payload: N, inputs: Vec<NodeId>) -> Self {
        Self {
            payload,
            inputs,
            dependents: Vec::new(),
            dirty: None,
        }
    }
}
