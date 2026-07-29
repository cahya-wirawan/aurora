//! Error types for `aurora-graph`.

use crate::node::NodeId;

/// Errors from building or updating a [`crate::RenderGraph`].
///
/// `#[non_exhaustive]`: more variants will be added once this crate grows
/// node removal/rewiring and tile-granular scheduling; downstream
/// `match`es must already handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GraphError {
    /// A [`NodeId`] passed to a `RenderGraph` method doesn't exist in
    /// that graph — either it's from a different `RenderGraph` instance,
    /// or nothing created it yet.
    #[error("node {0:?} does not exist in this graph")]
    UnknownNode(NodeId),
}
