//! Preserved Photoshop text layers (task 155).
//!
//! A PSD text layer is a pixel layer whose pixels are a *rendering* of text
//! Photoshop keeps as data: the layer's `TySh` tagged block (the type tool's
//! settings: the transform, the text and its style runs as an engine-data
//! stream, the warp and the bounding box) plus, at document level, the
//! `Txt2` block holding the document's text-engine resources. Aurora does
//! not lay text out or edit it: the layer opens as an ordinary raster layer,
//! and this module holds what is needed to write the text back **verbatim**
//! when the document is saved as a `.psd`, so the layer reopens in
//! Photoshop as an editable text layer.
//!
//! Two kinds of opaque data, both kept byte for byte and never interpreted
//! by the document model:
//!
//! - [`TextLayer`], per layer: the layer's `TySh` (or legacy `tySh`) block.
//!   Small (about 10 KB for a real layer), journaled like smart-object data
//!   (`LayerOp::SetTextLayer`) and carried by `.aur`'s `text-layers` entry.
//! - The document's `Txt2` block, held by the tree
//!   ([`crate::LayerTree::text_engine_data`]) behind an `Arc` and carried
//!   by the same `.aur` entry. It is written back only when at least one
//!   text layer is.
//!
//! # When the data is dropped (the same policy as smart objects)
//!
//! The block describes what the pixels *were*. Once the pixels change, or
//! the layer moves (the transform in `TySh` is in document coordinates and
//! Aurora does not update it), writing the old block back would make
//! Photoshop re-render the old text over the user's edit. So a PSD save
//! keeps a text layer only while the layer's pixels still have the
//! [`TextLayer::pixel_fingerprint`] they had when the data was attached and
//! its bounds are still [`TextLayer::bounds`]; otherwise the layer is
//! written as an ordinary pixel layer and the save report says so. Opacity,
//! blend mode, visibility, locks, masks and effects do not change the pixels
//! or the placement, so they keep the text.
//!
//! The fingerprint is [`crate::smart_object::surface_fingerprint`]: a hash
//! of the layer's tiles independent of the tile store's own bookkeeping, so
//! it survives a `.aur` save and reopen and an undo that puts the old pixels
//! back.

use aurora_core::Rect;

/// The most bytes one text layer's block may hold. A real one is about
/// 10 KB; a layer with thousands of style runs is a few hundred KB.
pub const MAX_TEXT_BLOCK_BYTES: usize = 1024 * 1024;

/// The most bytes of text blocks all of a document's text layers keep
/// together. **Well under the `.aur` reader's 64 MiB cap on the `history`
/// entry**, because the blocks travel twice, in the `text-layers` entry and
/// in the journal (`LayerOp::SetTextLayer`, and the carrier after a
/// journaled `Restore`), and smart-object blocks share that journal.
pub const MAX_TEXT_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes of the document-level `Txt2` block kept. A real one is
/// 30-130 KB.
pub const MAX_TEXT_ENGINE_BYTES: usize = 4 * 1024 * 1024;

/// The tagged-block keys a text layer keeps: Photoshop's type tool object
/// setting and the legacy (Photoshop 5) type tool info.
pub const TEXT_BLOCK_KEYS: [[u8; 4]; 2] = [*b"TySh", *b"tySh"];

/// A layer's preserved text data (task 155). See the module documentation
/// for what it is and when a PSD save drops it.
///
/// Positional `postcard`, like everything the journal and `.aur` carry: a
/// change to this type's shape needs a new `LayerOp` variant and a new
/// `.aur` entry version, never an edit of this one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TextLayer {
    /// The block's key: one of [`TEXT_BLOCK_KEYS`].
    pub key: [u8; 4],
    /// The block's data, exactly as the file held it.
    pub data: Vec<u8>,
    /// The layer's bounds when the data was attached; a save keeps the text
    /// only while the layer is still here.
    pub bounds: Rect,
    /// [`crate::smart_object::surface_fingerprint`] of the layer's pixels
    /// when the data was attached; a save keeps the text only while it still
    /// holds.
    pub pixel_fingerprint: u64,
}

/// Why a [`TextLayer`] (or the document's text-engine block) was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TextLayerError {
    #[error("the text block is not one of the kept kinds")]
    BadKey,
    #[error("the text block is empty")]
    Empty,
    #[error("a text block is too large")]
    BlockTooLarge,
    #[error("the document's text blocks are too large together")]
    TotalTooLarge,
}

impl TextLayer {
    /// Checks the bounds this crate puts on untrusted data.
    ///
    /// # Errors
    ///
    /// [`TextLayerError`] naming the first problem.
    pub fn validate(&self) -> Result<(), TextLayerError> {
        if !TEXT_BLOCK_KEYS.contains(&self.key) {
            return Err(TextLayerError::BadKey);
        }
        if self.data.is_empty() {
            return Err(TextLayerError::Empty);
        }
        if self.data.len() > MAX_TEXT_BLOCK_BYTES {
            return Err(TextLayerError::BlockTooLarge);
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // fixed-size fixtures
mod tests {
    use super::*;

    fn layer() -> TextLayer {
        TextLayer {
            key: *b"TySh",
            data: vec![0, 1, 2, 3],
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
    fn a_well_formed_text_layer_validates() {
        assert_eq!(layer().validate(), Ok(()));
        let mut legacy = layer();
        legacy.key = *b"tySh";
        assert_eq!(legacy.validate(), Ok(()));
    }

    #[test]
    fn validation_refuses_each_kind_of_bad_data() {
        let mut l = layer();
        l.key = *b"SoLd";
        assert_eq!(l.validate(), Err(TextLayerError::BadKey));
        let mut l = layer();
        l.data.clear();
        assert_eq!(l.validate(), Err(TextLayerError::Empty));
        let mut l = layer();
        l.data = vec![0; MAX_TEXT_BLOCK_BYTES + 1];
        assert_eq!(l.validate(), Err(TextLayerError::BlockTooLarge));
        // The bound itself is allowed.
        l.data = vec![0; MAX_TEXT_BLOCK_BYTES];
        assert_eq!(l.validate(), Ok(()));
    }

    #[test]
    fn a_text_layer_belongs_to_a_pixel_layer() {
        let mut tree = crate::LayerTree::new();
        let b = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let layer_id = tree.add_pixel_layer("p", b, None).ok();
        let group = tree.add_group("g", None).ok();
        let (Some(layer_id), Some(group)) = (layer_id, group) else {
            unreachable!("ids")
        };
        assert!(tree.set_text_layer(layer_id, Some(layer())).is_ok());
        assert!(tree.set_text_layer(group, Some(layer())).is_err());
        assert!(tree.set_text_layer(layer_id, None).is_ok());
        assert!(tree.text_layer(layer_id).is_none());
    }

    #[test]
    fn the_total_of_all_layers_is_bounded() {
        let mut tree = crate::LayerTree::new();
        let b = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
        };
        let per = MAX_TEXT_BLOCK_BYTES;
        let count = MAX_TEXT_TOTAL_BYTES / per;
        let mut ids = Vec::new();
        for i in 0..=count {
            let Ok(id) = tree.add_pixel_layer(format!("t{i}"), b, None) else {
                unreachable!("layer")
            };
            ids.push(id);
        }
        let big = TextLayer {
            data: vec![1; per],
            ..layer()
        };
        for id in ids.iter().take(count) {
            assert!(tree.set_text_layer(*id, Some(big.clone())).is_ok());
        }
        // One more would pass the document bound.
        assert!(matches!(
            tree.set_text_layer(ids[count], Some(big.clone())),
            Err(crate::DocError::InvalidTextLayer(
                TextLayerError::TotalTooLarge
            ))
        ));
        // Replacing a layer's own block is charged without it.
        assert!(tree.set_text_layer(ids[0], Some(big)).is_ok());
        // A small one still fits after removing a large one.
        assert!(tree.set_text_layer(ids[0], None).is_ok());
        assert!(tree.set_text_layer(ids[count], Some(layer())).is_ok());
    }

    #[test]
    fn the_engine_block_is_bounded_and_shared() {
        let mut tree = crate::LayerTree::new();
        assert!(tree.text_engine_data().is_none());
        let block = std::sync::Arc::new(vec![9u8; 100]);
        assert!(tree.set_text_engine_data(Some(block.clone())).is_ok());
        assert_eq!(tree.text_engine_data(), Some(&block));
        let cloned = tree.clone();
        assert!(std::sync::Arc::ptr_eq(
            cloned.text_engine_data().unwrap_or(&block),
            &block
        ));
        assert!(matches!(
            tree.set_text_engine_data(Some(std::sync::Arc::new(vec![
                0;
                MAX_TEXT_ENGINE_BYTES + 1
            ]))),
            Err(crate::DocError::InvalidTextLayer(
                TextLayerError::BlockTooLarge
            ))
        ));
        assert!(matches!(
            tree.set_text_engine_data(Some(std::sync::Arc::new(Vec::new()))),
            Err(crate::DocError::InvalidTextLayer(TextLayerError::Empty))
        ));
        assert_eq!(tree.text_engine_data(), Some(&block), "unchanged on error");
        assert!(tree.set_text_engine_data(None).is_ok());
        assert!(tree.text_engine_data().is_none());
    }
}
