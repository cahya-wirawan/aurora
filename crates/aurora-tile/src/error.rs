//! Error types for `aurora-tile`.

use crate::tile::{SurfaceId, TileId};
use std::path::PathBuf;

/// Errors from the tile store.
///
/// `#[non_exhaustive]`: more variants will be added as paging/compression
/// grow (e.g. real disk-quota handling); downstream `match`es must already
/// handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TileError {
    /// The scratch directory itself couldn't be created/accessed —
    /// distinct from a per-tile I/O failure ([`TileError::Io`]), which
    /// always names the tile involved.
    #[error("scratch directory {path:?} unavailable: {source}")]
    ScratchDirUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Scratch-disk I/O failed while paging a tile in or out. Names both
    /// `surface` and `id` — a shared, multi-surface store (ADR 0010) can
    /// have the same `TileId` resident for many different surfaces at
    /// once, so `id` alone would be ambiguous.
    #[error("scratch-disk I/O failed for surface {surface:?} tile {id:?}: {source}")]
    Io {
        surface: SurfaceId,
        id: TileId,
        #[source]
        source: std::io::Error,
    },
    /// A tile file on disk didn't parse as a valid tile (bad magic,
    /// unsupported version, truncated payload, or a decoded length that
    /// isn't exactly one whole tile (`aurora_tile::SAMPLES` samples)).
    #[error("corrupt tile file: {0}")]
    CorruptFile(String),
    /// A resident tile's own texel slice was not exactly one whole
    /// tile's worth of samples (`aurora_tile::SAMPLES`) — a malformed
    /// tile *in memory*, distinct from [`TileError::CorruptFile`],
    /// which is always about bytes read back off the scratch disk.
    ///
    /// Nothing in this crate constructs it: `Tile` allocates `SAMPLES`
    /// samples and never resizes, so every tile this store hands out is
    /// the right length. It exists for the layers above, which take a
    /// texel slice as a *parameter* and so cannot rely on that —
    /// `aurora-brush`'s dab path refuses to paint a tile whose pre-dab
    /// content it could not capture for undo, and needs a truthful
    /// error to report it with rather than a borrowed one.
    #[error(
        "surface {surface:?} tile {id:?} holds {samples} samples, not one whole tile's {expected}"
    )]
    MalformedTile {
        surface: SurfaceId,
        id: TileId,
        samples: usize,
        expected: usize,
    },
    /// Propagated from `aurora-core` (e.g. an invalid `Size`).
    #[error(transparent)]
    Core(#[from] aurora_core::CoreError),
}
