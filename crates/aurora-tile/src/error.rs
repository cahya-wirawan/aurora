//! Error types for `aurora-tile`.

use crate::tile::TileId;
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
    /// Scratch-disk I/O failed while paging a tile in or out.
    #[error("scratch-disk I/O failed for tile {id:?}: {source}")]
    Io {
        id: TileId,
        #[source]
        source: std::io::Error,
    },
    /// A tile file on disk didn't parse as a valid tile (bad magic,
    /// unsupported version, truncated payload, or a decompressed length
    /// that doesn't match a whole number of texels).
    #[error("corrupt tile file: {0}")]
    CorruptFile(String),
    /// Propagated from `aurora-core` (e.g. an invalid `Size`).
    #[error(transparent)]
    Core(#[from] aurora_core::CoreError),
}
