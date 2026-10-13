//! The Quick Mask buffer (0.217.0): the selection as an editable 8-bit
//! coverage grid over the whole document rectangle, and the way back to a
//! [`Selection`].
//!
//! A [`QuickMask`] holds one byte per document texel (`0` = unselected,
//! `255` = fully selected, row-major, origin `(0, 0)`), is mutable in place
//! (a painting tool edits [`QuickMask::edit`]'s slice) and counts its edits
//! in [`QuickMask::version`], which a cache keyed on "has it changed" reads.
//! It is **bounded** like a selection mask: at most
//! [`MAX_SELECTION_MASK_PIXELS`] texels, so a larger document cannot enter
//! the mode ([`DocError::SelectionMaskTooLarge`]).
//!
//! **In.** [`QuickMask::from_selection`] reads the selection's effective
//! coverage (inversion applied) over the document rectangle; `None` is
//! "everything selected" (all `255`), Photoshop's. A selection extending
//! past the document is clipped to it: the part outside is lost if the
//! buffer is later written back.
//!
//! **Out.** [`QuickMask::outcome`]: all `255` is [`QuickMaskOutcome::Everything`]
//! (no selection), all `0` is [`QuickMaskOutcome::Nothing`] (an empty
//! selection is never stored), anything else is the tight crop of its
//! non-zero texels as a plain rectangle (all `255` inside) or a raster
//! mask.

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection::{Selection, SelectionShape};
use crate::selection_combine::fill_effective_row;
use crate::selection_mask::{MAX_SELECTION_MASK_PIXELS, SelectionMask};

/// What a Quick Mask buffer means as a selection.
#[derive(Debug, Clone, PartialEq)]
pub enum QuickMaskOutcome {
    /// Every texel is fully selected: no selection.
    Everything,
    /// No texel is selected: no selection (never stored empty).
    Nothing,
    /// A selection of the non-zero texels.
    Region(Selection),
}

/// An editable coverage grid over the document rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickMask {
    size: (u32, u32),
    data: Vec<u8>,
    version: u64,
}

impl QuickMask {
    /// The grid of `selection` (`None` = everything selected) over a
    /// document of `size` pixels.
    ///
    /// # Errors
    ///
    /// [`DocError::SelectionMaskTooLarge`] over the 64 Mpx cap,
    /// [`DocError::SelectionMaskInvalid`] for an empty document.
    pub fn from_selection(
        selection: Option<&Selection>,
        size: (u32, u32),
    ) -> Result<Self, DocError> {
        let pixels = u64::from(size.0) * u64::from(size.1);
        if pixels > MAX_SELECTION_MASK_PIXELS {
            return Err(DocError::SelectionMaskTooLarge(pixels));
        }
        if pixels == 0 {
            return Err(DocError::SelectionMaskInvalid);
        }
        let width = usize::try_from(size.0).map_err(|_| DocError::SelectionMaskInvalid)?;
        let mut data = vec![u8::MAX; usize::try_from(pixels).unwrap_or(0)];
        if let Some(selection) = selection {
            for (y, row) in data.chunks_exact_mut(width).enumerate() {
                let y = i64::try_from(y).unwrap_or(0);
                fill_effective_row(selection, y, 0, row);
            }
        }
        Ok(Self {
            size,
            data,
            version: 0,
        })
    }

    /// The document size the grid covers.
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        self.size
    }

    /// How many edits have changed the grid; `0` for an untouched one.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Whether any edit changed the grid since it was built.
    #[must_use]
    pub const fn is_edited(&self) -> bool {
        self.version > 0
    }

    /// The coverage byte of document texel `(x, y)`; `0` outside the grid.
    #[must_use]
    pub fn byte_at(&self, x: i64, y: i64) -> u8 {
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return 0;
        };
        let (Ok(w), Ok(h)) = (usize::try_from(self.size.0), usize::try_from(self.size.1)) else {
            return 0;
        };
        if x >= w || y >= h {
            return 0;
        }
        self.data.get(y * w + x).copied().unwrap_or(0)
    }

    /// Row `y` of the grid, `None` outside it.
    #[must_use]
    pub fn row(&self, y: i64) -> Option<&[u8]> {
        let y = usize::try_from(y).ok()?;
        let w = usize::try_from(self.size.0).ok()?;
        self.data.chunks_exact(w).nth(y)
    }

    /// Edits the grid through `f(data, width, height)`, which returns the
    /// rectangle it changed (`None` for no change). A change bumps
    /// [`Self::version`]; the rectangle is handed back.
    pub fn edit(&mut self, f: impl FnOnce(&mut [u8], u32, u32) -> Option<Rect>) -> Option<Rect> {
        let changed = f(&mut self.data, self.size.0, self.size.1);
        if changed.is_some() {
            self.version += 1;
        }
        changed
    }

    /// What the grid means as a selection (see the module docs).
    #[must_use]
    pub fn outcome(&self) -> QuickMaskOutcome {
        let Ok(width) = usize::try_from(self.size.0) else {
            return QuickMaskOutcome::Nothing;
        };
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            (usize::MAX, usize::MAX, 0_usize, 0_usize);
        let mut any = false;
        let mut all_full = true;
        for (y, row) in self.data.chunks_exact(width).enumerate() {
            if all_full && row.iter().any(|&v| v != u8::MAX) {
                all_full = false;
            }
            let first = row.iter().position(|&v| v != 0);
            let last = row.iter().rposition(|&v| v != 0);
            if let (Some(first), Some(last)) = (first, last) {
                any = true;
                min_x = min_x.min(first);
                max_x = max_x.max(last);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
        if !any {
            return QuickMaskOutcome::Nothing;
        }
        if all_full {
            return QuickMaskOutcome::Everything;
        }
        let tight = Rect {
            x: i64::try_from(min_x).unwrap_or(0),
            y: i64::try_from(min_y).unwrap_or(0),
            width: u32::try_from(max_x - min_x + 1).unwrap_or(u32::MAX),
            height: u32::try_from(max_y - min_y + 1).unwrap_or(u32::MAX),
        };
        let mut cropped = Vec::with_capacity(usize::try_from(tight.area_px()).unwrap_or(0));
        for row in self
            .data
            .chunks_exact(width)
            .skip(min_y)
            .take(max_y - min_y + 1)
        {
            if let Some(part) = row.get(min_x..=max_x) {
                cropped.extend_from_slice(part);
            }
        }
        if cropped.iter().all(|&v| v == u8::MAX) {
            return QuickMaskOutcome::Region(Selection::new(tight));
        }
        match SelectionMask::new(tight, cropped) {
            Ok(mask) => QuickMaskOutcome::Region(Selection {
                bounds: tight,
                inverted: false,
                shape: SelectionShape::Mask(std::sync::Arc::new(mask)),
            }),
            Err(_) => QuickMaskOutcome::Nothing,
        }
    }
}
