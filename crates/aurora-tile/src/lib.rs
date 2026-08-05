//! Tile store: paging to scratch disk, LRU image cache, half-float tile storage.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it — [ADR 0005](../../../docs/adr/0005-tile-size-scratch-budget.md)
//! settles the 256×256 px tile size and scratch-disk budget model this
//! crate implements, and [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)
//! settles [`TileStore`]'s own multi-surface addressing (every tile
//! access takes a [`SurfaceId`] alongside a [`TileId`]) — one shared
//! store per document, not one per layer.
//!
//! Compositing/blending is deliberately **not** this crate's job (that's
//! `aurora-graph`/`aurora-brush`) — this crate owns storage, paging,
//! compression, and dirty-rectangle *carrying* only.

mod codec;
mod error;
mod store;
mod tile;
mod writer;

pub use error::TileError;
pub use store::{Stats, TileStore};
pub use tile::{CHANNELS, SAMPLES, Surface, SurfaceId, TEXELS, TILE, Tile, TileId};
