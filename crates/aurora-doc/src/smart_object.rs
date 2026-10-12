//! Preserved Photoshop smart objects (0.200.0, round S1).
//!
//! A PSD smart object is a layer whose pixels are a *rendering* of an
//! embedded file (a vector `.ai`, a PSB, a placed image) plus the placement
//! data Photoshop needs to render it again. Aurora does not render or edit
//! them: the layer opens as an ordinary raster layer, and this module holds
//! what is needed to write the smart object back **verbatim** when the
//! document is saved as a `.psd`, so the layer reopens in Photoshop as a
//! smart object.
//!
//! Two kinds of opaque data, both kept byte for byte:
//!
//! - [`SmartObject`], per layer: the layer's own tagged blocks (`SoLd`,
//!   `PlLd`, `SoLE`, `fxrp`) and the unique ID that names its embedded
//!   file. Small (a few KB), journaled like layer effects
//!   (`LayerOp::SetSmartObject`) and carried by `.aur`'s `smart-objects`
//!   entry.
//! - [`LinkedFile`], per document: one entry of the file's document-level
//!   linked-layer list (`lnk2`, `lnkD`, `lnk3` or `lnkE`) — the embedded
//!   file itself, framed exactly as the PSD frames it. Large (megabytes),
//!   shared by every layer that names its ID, held in the tree's own pool
//!   behind an [`Arc`] and written to `.aur` as its own stored entries.
//!
//! # When the data is dropped (the S1 policy)
//!
//! The embedded file describes what the pixels *were*. Once the pixels
//! change, or the layer moves (the placement in `SoLd` is in document
//! coordinates and Aurora does not update it), writing the old data back
//! would make Photoshop re-render the old picture over the user's edit. So
//! a PSD save keeps a smart object only while the layer's pixels still
//! have the [`SmartObject::pixel_fingerprint`] they had when the data was
//! attached and its bounds are still [`SmartObject::bounds`]; otherwise
//! the layer is written as an ordinary pixel layer and the save report says
//! so. Opacity, blend mode, visibility, locks, masks and effects do not
//! change the pixels or the placement, so they keep the smart object.
//!
//! The fingerprint is a hash of the layer's tiles, independent of the tile
//! store's own bookkeeping, so it survives a `.aur` save and reopen, an
//! undo that puts the old pixels back, and a delete that is undone.

use std::sync::Arc;

use aurora_core::Rect;
use aurora_tile::{SurfaceId, TILE, TileError, TileId, TileStore};
use half::f16;

/// The most tagged blocks one layer's smart object keeps.
pub const MAX_SMART_OBJECT_BLOCKS: usize = 8;

/// The most bytes one preserved block may hold. A real `SoLd` is a few KB;
/// one with many smart filters is tens of KB.
pub const MAX_SMART_OBJECT_BLOCK_BYTES: usize = 4 * 1024 * 1024;

/// The most bytes of tagged blocks all of a document's smart objects keep
/// together. **Well under the `.aur` reader's 64 MiB cap on the `history`
/// entry**, because the blocks travel twice: in the `smart-objects` entry
/// and in the journal (`LayerOp::SetSmartObject`, and the carrier after a
/// journaled `Restore`), so a document at this bound still saves a journal
/// the reader accepts. Repeated delete/undo cycles of smart layers add a
/// carrier each time and can still grow the journal past its cap; that is
/// a known limit, shared with layer effects.
pub const MAX_SMART_OBJECT_TOTAL_BLOCK_BYTES: usize = 16 * 1024 * 1024;

/// The longest unique ID kept, in bytes. Photoshop's are 36-character
/// UUIDs.
pub const MAX_SMART_OBJECT_ID_BYTES: usize = 256;

/// The most bytes of embedded files one document keeps in its linked-file
/// pool. The pool is held in memory (behind `Arc`s) and written to every
/// `.aur` save and autosave, so it is bounded; a smart object whose file
/// would pass the bound is opened as plain pixels and reported.
pub const MAX_LINKED_FILE_POOL_BYTES: u64 = 512 * 1024 * 1024;

/// The most linked files one document's pool holds.
pub const MAX_LINKED_FILES: usize = 4096;

/// The tagged-block keys a smart object layer keeps, in the order they are
/// written back.
pub const SMART_OBJECT_BLOCK_KEYS: [[u8; 4]; 4] = [*b"SoLd", *b"PlLd", *b"SoLE", *b"fxrp"];

/// The document-level keys of a PSD's linked-layer lists.
pub const LINKED_LIST_KEYS: [[u8; 4]; 4] = [*b"lnk2", *b"lnkD", *b"lnk3", *b"lnkE"];

/// One preserved tagged block: its key and its data, as the file held it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SmartObjectBlock {
    pub key: [u8; 4],
    pub data: Vec<u8>,
}

/// A layer's preserved smart-object data (0.200.0). See the module
/// documentation for what it is and when a PSD save drops it.
///
/// Positional `postcard`, like everything the journal and `.aur` carry: a
/// change to this type's shape needs a new `LayerOp` variant and a new
/// `.aur` entry version, never an edit of this one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SmartObject {
    /// The unique ID of the embedded file this layer renders
    /// (`SoLd`'s `Idnt`, else `PlLd`'s): the key into the tree's
    /// [`LinkedFile`] pool.
    pub unique_id: String,
    /// The layer's own tagged blocks, in the order they are written back.
    pub blocks: Vec<SmartObjectBlock>,
    /// The layer's bounds when the data was attached; a save keeps the
    /// smart object only while the layer is still here.
    pub bounds: Rect,
    /// [`surface_fingerprint`] of the layer's pixels when the data was
    /// attached; a save keeps the smart object only while it still holds.
    pub pixel_fingerprint: u64,
}

/// Why a [`SmartObject`] was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SmartObjectError {
    #[error("the smart object has no unique ID, or one that is too long")]
    BadId,
    #[error("the smart object has no blocks, or too many")]
    BadBlockCount,
    #[error("a smart object block is not one of the kept kinds, or appears twice")]
    BadBlockKey,
    #[error("a smart object block is too large")]
    BlockTooLarge,
    #[error("the document's embedded smart object files are too many or too large")]
    PoolTooLarge,
}

impl SmartObject {
    /// Checks the bounds this crate puts on untrusted data.
    ///
    /// # Errors
    ///
    /// [`SmartObjectError`] naming the first problem.
    pub fn validate(&self) -> Result<(), SmartObjectError> {
        if self.unique_id.is_empty() || self.unique_id.len() > MAX_SMART_OBJECT_ID_BYTES {
            return Err(SmartObjectError::BadId);
        }
        if self.blocks.is_empty() || self.blocks.len() > MAX_SMART_OBJECT_BLOCKS {
            return Err(SmartObjectError::BadBlockCount);
        }
        for (i, block) in self.blocks.iter().enumerate() {
            if !SMART_OBJECT_BLOCK_KEYS.contains(&block.key)
                || self.blocks.iter().take(i).any(|b| b.key == block.key)
            {
                return Err(SmartObjectError::BadBlockKey);
            }
            if block.data.len() > MAX_SMART_OBJECT_BLOCK_BYTES {
                return Err(SmartObjectError::BlockTooLarge);
            }
        }
        Ok(())
    }

    /// The bytes of all the blocks together.
    #[must_use]
    pub fn block_bytes(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| b.data.len())
            .fold(0usize, usize::saturating_add)
    }
}

/// One embedded file of the document's pool: an entry of a PSD's
/// document-level linked-layer list, exactly as the file framed it.
///
/// `entry` is the whole framed entry — the `u64` length, the body (kind,
/// version, unique ID, file name, type, data, …) and the padding to four
/// bytes — so a writer concatenates entries to rebuild the list's block
/// and nothing about the body needs to be understood.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkedFile {
    /// Which list the entry came from (one of [`LINKED_LIST_KEYS`]).
    pub list: [u8; 4],
    /// The entry's unique ID, which layers name.
    pub unique_id: String,
    /// The framed entry.
    pub entry: Arc<Vec<u8>>,
}

impl std::fmt::Debug for LinkedFile {
    /// Without the bytes: an embedded file can be megabytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkedFile")
            .field("list", &String::from_utf8_lossy(&self.list))
            .field("unique_id", &self.unique_id)
            .field("bytes", &self.entry.len())
            .finish()
    }
}

impl LinkedFile {
    /// Checks the bounds this crate puts on untrusted data.
    ///
    /// # Errors
    ///
    /// [`SmartObjectError::BadId`] or [`SmartObjectError::BadBlockKey`].
    pub fn validate(&self) -> Result<(), SmartObjectError> {
        if self.unique_id.is_empty() || self.unique_id.len() > MAX_SMART_OBJECT_ID_BYTES {
            return Err(SmartObjectError::BadId);
        }
        if !LINKED_LIST_KEYS.contains(&self.list) {
            return Err(SmartObjectError::BadBlockKey);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Pixel fingerprint
// ---------------------------------------------------------------------

const FINGERPRINT_MUL: u64 = 0x9E37_79B9_7F4A_7C15;

/// A hash of one tile's texels, bit for bit; `None` for a tile whose every
/// texel is `+0.0`, which is the same thing as a tile that does not exist
/// (the store reads an absent tile as that, and a read can materialise a
/// blank one), so it contributes nothing.
#[must_use]
pub fn texel_fingerprint(texels: &[f16]) -> Option<u64> {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut any = false;
    let mut chunks = texels.chunks_exact(4);
    for chunk in &mut chunks {
        let mut word = 0u64;
        for (i, t) in chunk.iter().enumerate() {
            word |= u64::from(t.to_bits()) << (16 * i);
        }
        any |= word != 0;
        h = (h ^ word).wrapping_mul(FINGERPRINT_MUL);
        h ^= h >> 29;
    }
    let mut tail = 0u64;
    for (i, t) in chunks.remainder().iter().enumerate() {
        tail |= u64::from(t.to_bits()) << (16 * i);
    }
    any |= tail != 0;
    h = (h ^ tail ^ (texels.len() as u64)).wrapping_mul(FINGERPRINT_MUL);
    h ^= h >> 31;
    any.then_some(h)
}

/// One tile's contribution to a layer's fingerprint: its [`texel_fingerprint`]
/// mixed with where the tile is. Contributions are summed (wrapping), so the
/// order tiles are visited in does not matter.
#[must_use]
pub fn tile_fingerprint_term(id: TileId, texel_hash: u64) -> u64 {
    let mut z = texel_hash ^ (u64::from(id.x) << 32 | u64::from(id.y));
    z = z.wrapping_add(FINGERPRINT_MUL);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The fingerprint of a layer's pixels: the sum of [`tile_fingerprint_term`]
/// over every tile of `surface` within `bounds` that holds anything but
/// `+0.0`. Reads the tiles (paging them in); a tile the store does not hold
/// costs one lookup.
///
/// # Errors
///
/// [`TileError`] if a held tile cannot be read.
pub fn surface_fingerprint(
    store: &mut TileStore,
    surface: SurfaceId,
    bounds: Rect,
) -> Result<u64, TileError> {
    let (tiles_x, tiles_y) = (bounds.width.div_ceil(TILE), bounds.height.div_ceil(TILE));
    let mut sum = 0u64;
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let tile = store.get(surface, id)?;
            if let Some(h) = texel_fingerprint(tile.texels()) {
                sum = sum.wrapping_add(tile_fingerprint_term(id, h));
            }
        }
    }
    Ok(sum)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // fixed-size fixtures
mod tests {
    use super::*;

    fn object() -> SmartObject {
        SmartObject {
            unique_id: "0001bdab-4207-8a45-9be1-e66466dde9b7".to_owned(),
            blocks: vec![
                SmartObjectBlock {
                    key: *b"SoLd",
                    data: vec![1, 2, 3],
                },
                SmartObjectBlock {
                    key: *b"PlLd",
                    data: vec![4],
                },
            ],
            bounds: Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            pixel_fingerprint: 7,
        }
    }

    #[test]
    fn a_well_formed_smart_object_validates() {
        assert_eq!(object().validate(), Ok(()));
    }

    #[test]
    fn validation_refuses_each_kind_of_bad_data() {
        let mut o = object();
        o.unique_id.clear();
        assert_eq!(o.validate(), Err(SmartObjectError::BadId));
        let mut o = object();
        o.unique_id = "x".repeat(MAX_SMART_OBJECT_ID_BYTES + 1);
        assert_eq!(o.validate(), Err(SmartObjectError::BadId));
        let mut o = object();
        o.blocks.clear();
        assert_eq!(o.validate(), Err(SmartObjectError::BadBlockCount));
        let mut o = object();
        o.blocks = (0..=MAX_SMART_OBJECT_BLOCKS)
            .map(|_| SmartObjectBlock {
                key: *b"SoLd",
                data: Vec::new(),
            })
            .collect();
        assert_eq!(o.validate(), Err(SmartObjectError::BadBlockCount));
        let mut o = object();
        o.blocks[1].key = *b"SoLd";
        assert_eq!(o.validate(), Err(SmartObjectError::BadBlockKey), "twice");
        let mut o = object();
        o.blocks[1].key = *b"lfx2";
        assert_eq!(o.validate(), Err(SmartObjectError::BadBlockKey), "kind");
        let mut o = object();
        o.blocks[0].data = vec![0; MAX_SMART_OBJECT_BLOCK_BYTES + 1];
        assert_eq!(o.validate(), Err(SmartObjectError::BlockTooLarge));
    }

    #[test]
    fn a_blank_tile_has_no_fingerprint_and_any_texel_change_moves_it() {
        let mut texels = vec![f16::ZERO; aurora_tile::SAMPLES];
        assert_eq!(texel_fingerprint(&texels), None);
        texels[5] = f16::from_f32(0.5);
        let a = texel_fingerprint(&texels);
        assert!(a.is_some());
        texels[5] = f16::from_f32(0.5 + 1.0 / 1024.0);
        let b = texel_fingerprint(&texels);
        assert_ne!(a, b, "one f16 step");
        texels[5] = f16::ZERO;
        texels[aurora_tile::SAMPLES - 1] = f16::from_f32(1.0);
        assert!(texel_fingerprint(&texels).is_some(), "the last sample");
        // A negative zero is not the blank tile.
        let mut negative = vec![f16::ZERO; aurora_tile::SAMPLES];
        negative[0] = f16::NEG_ZERO;
        assert!(texel_fingerprint(&negative).is_some());
    }

    #[test]
    fn the_same_texels_at_another_tile_position_add_a_different_term() {
        let h = 0x1234_5678_9ABC_DEF0;
        let a = tile_fingerprint_term(TileId { x: 0, y: 1 }, h);
        let b = tile_fingerprint_term(TileId { x: 1, y: 0 }, h);
        assert_ne!(a, b);
    }

    fn file(id: &str, list: [u8; 4], len: usize) -> LinkedFile {
        LinkedFile {
            list,
            unique_id: id.to_owned(),
            entry: Arc::new(vec![0u8; len]),
        }
    }

    #[test]
    fn the_pool_replaces_by_id_and_refuses_bad_files() {
        let mut tree = crate::LayerTree::new();
        assert_eq!(tree.add_linked_file(file("a", *b"lnk2", 8)).ok(), Some(()));
        assert_eq!(tree.add_linked_file(file("a", *b"lnkD", 12)).ok(), Some(()));
        assert_eq!(tree.linked_files().len(), 1);
        assert_eq!(tree.linked_files().get("a").map(|f| f.list), Some(*b"lnkD"));
        assert!(matches!(
            tree.add_linked_file(file("", *b"lnk2", 8)),
            Err(crate::DocError::InvalidSmartObject(SmartObjectError::BadId))
        ));
        assert!(matches!(
            tree.add_linked_file(file("b", *b"lnkX", 8)),
            Err(crate::DocError::InvalidSmartObject(
                SmartObjectError::BadBlockKey
            ))
        ));
        // Debug never prints the bytes.
        let shown = format!("{:?}", file("a", *b"lnk2", 1 << 20));
        assert!(shown.len() < 200, "{shown}");
    }

    #[test]
    fn a_smart_object_belongs_to_a_pixel_layer() {
        let mut tree = crate::LayerTree::new();
        let b = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let layer = tree.add_pixel_layer("p", b, None).ok();
        let group = tree.add_group("g", None).ok();
        let (Some(layer), Some(group)) = (layer, group) else {
            unreachable!("ids")
        };
        assert!(tree.set_smart_object(layer, Some(object())).is_ok());
        assert!(tree.set_smart_object(group, Some(object())).is_err());
        assert!(tree.set_smart_object(layer, None).is_ok());
        assert!(tree.smart_object(layer).is_none());
    }
}
