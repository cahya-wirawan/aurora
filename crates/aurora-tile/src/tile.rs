//! Tile identity and resident tile storage.

use aurora_core::{Id, Rect};
use half::f16;

/// Marker type for [`SurfaceId`] — never constructed, just named. See
/// `aurora_core::Id`'s own doc comment and tests, which already name
/// this exact "one crate's own opaque identifier" use case.
#[derive(Debug)]
pub struct Surface;

/// Identifies one independently addressed sequence of tiles within a
/// [`crate::TileStore`] — [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)'s
/// "one shared store, addressed by surface" decision. `aurora-tile`
/// itself has no idea what a surface *represents* (a document's pixel
/// layer, a brush's own scratch layer, or anything else a future caller
/// invents) — it is exactly as opaque to this crate as `TileId` itself
/// is meaningful only as a grid position. A real `LayerId` in
/// `aurora-doc` becomes a `SurfaceId` via `SurfaceId::from_raw(layer_id.to_raw())`,
/// not a second, independently generated identifier — `aurora-tile`
/// can't depend on `aurora-doc`'s own `LayerId` type (layering runs the
/// other way), so this crate only ever sees the raw value, never
/// allocates one itself.
pub type SurfaceId = Id<Surface>;

/// Tile dimension in pixels (ADR 0005: 256×256, matching what
/// `spike/vertical-slice` actually measured — the only real data point
/// that exists for this tradeoff).
pub const TILE: u32 = 256;

/// Channels per texel: RGBA (matches `spike/vertical-slice`'s proven
/// layout and ADR 0003's half-float storage decision).
pub const CHANNELS: usize = 4;

/// Texels per tile (`TILE * TILE`).
pub const TEXELS: usize = (TILE * TILE) as usize;

/// `f16` samples per tile (`TEXELS * CHANNELS`) — the length every
/// `Tile::texels()` slice has.
pub const SAMPLES: usize = TEXELS * CHANNELS;

/// Identifies one tile by its grid position in document space (not a
/// pixel coordinate — tile `(1, 0)` covers pixels `256..512` in x).
///
/// `Serialize`/`Deserialize`: a `.aur` file (ADR 0009) names each tile's
/// own ZIP entry by its `(SurfaceId, TileId)` pair — this is the half of
/// that pair `aurora-tile` itself owns.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
}

/// One resident tile: its pixel data plus the dirty rectangle
/// accumulated since it was last taken (e.g. for GPU upload).
///
/// `dirty` is in tile-local coordinates (`0..TILE` on each axis), unioned
/// via `aurora_core::Rect::union` on every `mark_dirty` call — this is
/// the M1.1 "per-tile dirty rectangles" deliverable: a merge or upload
/// touches only the accumulated dirty region, not the whole tile
/// (`spike/FINDINGS.md` finding #1 named whole-tile merges as 65% of a
/// painting frame's cost).
#[derive(Debug)]
pub struct Tile {
    texels: Vec<f16>,
    dirty: Option<Rect>,
}

impl Tile {
    /// A new, fully-transparent tile (all channels zero).
    #[must_use]
    pub fn blank() -> Self {
        Self {
            texels: vec![f16::from_f32(0.0); SAMPLES],
            dirty: None,
        }
    }

    /// Reconstructs a tile from decoded samples (paging in from disk).
    /// Starts clean (`dirty: None`) — a tile just read back from the
    /// scratch disk matches what's stored there by definition.
    #[must_use]
    pub(crate) fn from_texels(texels: Vec<f16>) -> Self {
        Self {
            texels,
            dirty: None,
        }
    }

    #[must_use]
    pub fn texels(&self) -> &[f16] {
        &self.texels
    }

    /// Whole-slice access for callers that composite/paint into this
    /// tile. `aurora-tile` doesn't own compositing (that's
    /// `aurora-graph`/`aurora-brush`'s job) — a caller that mutates
    /// through this is responsible for calling [`Tile::mark_dirty`]
    /// itself with the region it actually touched.
    pub fn texels_mut(&mut self) -> &mut [f16] {
        &mut self.texels
    }

    /// Accumulates `rect` (tile-local coordinates) into the dirty region.
    pub fn mark_dirty(&mut self, rect: Rect) {
        self.dirty = Some(match self.dirty {
            Some(existing) => existing.union(&rect),
            None => rect,
        });
    }

    /// Takes and clears the accumulated dirty rectangle, e.g. right
    /// before a GPU upload — the next call returns `None` until
    /// [`Tile::mark_dirty`] is called again.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.dirty.take()
    }
}

#[cfg(test)]
mod tests {
    use super::{CHANNELS, SAMPLES, TEXELS, TILE, Tile, TileId};
    use aurora_core::Rect;

    #[test]
    fn tile_id_round_trips_through_real_postcard_bytes() {
        let id = TileId { x: 3, y: 7 };
        let bytes = match postcard::to_allocvec(&id) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let restored: TileId = match postcard::from_bytes(&bytes) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(restored, id);
    }

    #[test]
    fn tile_dimensions_match_adr_0005() {
        assert_eq!(TILE, 256);
        assert_eq!(TEXELS, 65_536);
        assert_eq!(SAMPLES, 262_144); // 65,536 texels * 4 channels
        assert_eq!(CHANNELS, 4);
    }

    #[test]
    fn blank_tile_is_zeroed_and_clean() {
        let mut tile = Tile::blank();
        assert_eq!(tile.texels().len(), SAMPLES);
        assert!(tile.texels().iter().all(|s| s.to_f32() == 0.0));
        assert!(tile.take_dirty().is_none());
    }

    #[test]
    fn mark_dirty_accumulates_via_union() {
        let mut tile = Tile::blank();
        tile.mark_dirty(Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        });
        tile.mark_dirty(Rect {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        });
        let dirty = tile.take_dirty();
        assert_eq!(
            dirty,
            Some(Rect {
                x: 0,
                y: 0,
                width: 15,
                height: 15
            })
        );
        // Cleared after taking.
        assert!(tile.take_dirty().is_none());
    }
}
