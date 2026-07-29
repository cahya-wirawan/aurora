//! Tile-granular scheduling: converts a [`RenderGraph`]'s node-granular
//! dirty regions into per-node lists of the tiles that actually need
//! re-evaluation (PLAN.md M1.3).

use aurora_core::Rect;
use aurora_graph::{NodeId, RenderGraph};
use aurora_tile::{TILE, TileId};

/// One node's outstanding work: the tiles overlapping its dirty region.
///
/// Never empty by construction — [`schedule`] only emits an entry for a
/// node that actually has tiles to redo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledWork {
    pub node: NodeId,
    pub tiles: Vec<TileId>,
}

/// Builds the tile-granular work list for every dirty node in `graph`, in
/// topological order (`RenderGraph::iter`'s own insertion-order guarantee
/// — a node's inputs are always scheduled before it).
///
/// Deliberately non-destructive (`peek_dirty`, not `take_dirty`):
/// `RenderGraph` tracks one dirty [`Rect`] per node, not one per tile, so
/// there is no way to record "6 of this node's 8 dirty tiles are done"
/// without losing the other 2. Clearing eagerly here — before an executor
/// (not yet implemented; GPU compositing, progressive rendering, and
/// async evaluation are still open, see PLAN.md M1.3) has actually
/// committed output for every listed tile — would mean a
/// budget-interrupted or failed tile silently never gets retried, the
/// same failure shape `aurora_gpu::TileResidency`'s upload budgeting was
/// careful to avoid. Whoever executes a [`ScheduledWork`] and commits
/// every one of its tiles is responsible for calling
/// `graph.take_dirty(work.node)` itself; this function only computes what
/// the work *is*.
#[must_use]
pub fn schedule<N>(graph: &RenderGraph<N>) -> Vec<ScheduledWork> {
    graph
        .iter()
        .filter_map(|node| {
            let region = graph.peek_dirty(node)?;
            let tiles = tiles_for_rect(region);
            (!tiles.is_empty()).then_some(ScheduledWork { node, tiles })
        })
        .collect()
}

/// Every [`TileId`] the document-space `rect` overlaps.
///
/// `rect` is clipped to non-negative coordinates first: a layer's `Rect`
/// can extend off-canvas (`aurora_core::Rect`'s own doc comment — moved
/// past an edge, mid-transform), but [`TileId`]'s `x`/`y` are `u32`,
/// document-tile space only. A rect entirely off-canvas yields no tiles
/// rather than panicking or wrapping.
#[must_use]
pub fn tiles_for_rect(rect: Rect) -> Vec<TileId> {
    if rect.is_empty() {
        return Vec::new();
    }
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = rect.right();
    let bottom = rect.bottom();
    if right <= left || bottom <= top {
        return Vec::new();
    }

    #[allow(clippy::cast_sign_loss)]
    let x0 = left as u32 / TILE;
    #[allow(clippy::cast_sign_loss)]
    let x1 = (right - 1) as u32 / TILE;
    #[allow(clippy::cast_sign_loss)]
    let y0 = top as u32 / TILE;
    #[allow(clippy::cast_sign_loss)]
    let y1 = (bottom - 1) as u32 / TILE;

    let mut tiles = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1)) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            tiles.push(TileId { x, y });
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::{ScheduledWork, schedule, tiles_for_rect};
    use aurora_core::Rect;
    use aurora_graph::RenderGraph;
    use aurora_tile::TileId;

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn tiles_for_rect_single_tile() {
        let tiles = tiles_for_rect(rect(10, 10, 5, 5));
        assert_eq!(tiles, vec![TileId { x: 0, y: 0 }]);
    }

    #[test]
    fn tiles_for_rect_spans_a_tile_boundary() {
        // TILE = 256; x spans [200, 312), crossing the 256 boundary.
        let tiles = tiles_for_rect(rect(200, 0, 112, 10));
        assert_eq!(tiles, vec![TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 }]);
    }

    #[test]
    fn tiles_for_rect_clips_negative_coordinates() {
        // x spans [-100, 256) -- clipped to [0, 256), one tile.
        let tiles = tiles_for_rect(rect(-100, 0, 356, 10));
        assert_eq!(tiles, vec![TileId { x: 0, y: 0 }]);
    }

    #[test]
    fn tiles_for_rect_entirely_off_canvas_is_empty() {
        let tiles = tiles_for_rect(rect(-100, -100, 50, 50));
        assert!(tiles.is_empty());
    }

    #[test]
    fn tiles_for_rect_empty_rect_is_empty() {
        assert!(tiles_for_rect(rect(0, 0, 0, 0)).is_empty());
    }

    #[test]
    fn schedule_only_includes_dirty_nodes_in_topological_order() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match graph.add_node("b", &[a]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = graph.add_node("d", &[]) {
            unreachable!("{err:?}");
        }

        if let Err(err) = graph.mark_dirty(a, rect(0, 0, 10, 10)) {
            unreachable!("{err:?}");
        }

        let work = schedule(&graph);
        assert_eq!(
            work.len(),
            2,
            "a and its dependent b, not the independent d"
        );
        match (work.first(), work.get(1)) {
            (Some(first), Some(second)) => {
                assert_eq!(first.node, a);
                assert_eq!(second.node, b);
            }
            other => unreachable!("length just asserted to be 2: {other:?}"),
        }
    }

    #[test]
    fn schedule_computes_tiles_from_the_node_dirty_region() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = graph.mark_dirty(a, rect(200, 0, 112, 10)) {
            unreachable!("{err:?}");
        }

        let work = schedule(&graph);
        assert_eq!(
            work,
            vec![ScheduledWork {
                node: a,
                tiles: vec![TileId { x: 0, y: 0 }, TileId { x: 1, y: 0 }],
            }]
        );
    }

    #[test]
    fn schedule_is_non_destructive() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        let a = match graph.add_node("a", &[]) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = graph.mark_dirty(a, rect(0, 0, 10, 10)) {
            unreachable!("{err:?}");
        }

        assert_eq!(schedule(&graph).len(), 1);
        assert_eq!(schedule(&graph).len(), 1, "peek_dirty must not clear state");
        assert!(graph.peek_dirty(a).is_some());
    }

    #[test]
    fn schedule_is_empty_for_a_graph_with_nothing_dirty() {
        let mut graph: RenderGraph<&str> = RenderGraph::new();
        if let Err(err) = graph.add_node("a", &[]) {
            unreachable!("{err:?}");
        }
        assert!(schedule(&graph).is_empty());
    }
}
