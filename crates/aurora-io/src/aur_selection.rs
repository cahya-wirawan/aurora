//! The optional `selection` entry of a `.aur` container (0.218.0): the
//! document's active selection and its named saved selections (ADR 0009's
//! 0.218.0 addendum).
//!
//! **Layout.** One entry: a `postcard` header ([`SelectionHeader`]: a frozen
//! `version` first, the active selection's [`SelectionMeta`], and
//! `(name, meta)` for each saved selection, sorted by name so equal
//! documents give equal bytes) followed by the raster coverage of every
//! mask selection, raw, in header order (active first). The header says how
//! many bytes each blob takes, so every size is validated against the caps
//! **before** a single coverage byte is copied; the container deflates the
//! entry, which is what makes a mostly-empty 64 Mpx mask small.
//!
//! **Rules.** Written only when the document has an active or a saved
//! selection (every other save is byte-identical to 0.217.x). A file
//! without the entry opens with no selection, which is what every older
//! file means; an Aurora older than 0.218.0 never looks the entry up and
//! so ignores it (tested: the reader opens a container with an unknown
//! extra entry). A **newer** version refuses the open, loudly, like
//! `layer-effects`; anything else wrong with the entry (undecodable,
//! oversized, over a cap, a bad name, a size that does not add up, an
//! older version) drops the selection, never the file, and the caller is
//! handed a one-line reason to show.

use std::io::{Read, Seek};

use aurora_core::Rect;
use aurora_doc::{
    MAX_SAVED_SELECTION_BYTES, MAX_SAVED_SELECTIONS, MAX_SELECTION_MASK_PIXELS, Selection,
    SelectionMask, SelectionSet, SelectionShape,
};
use zip::ZipArchive;

use crate::error::IoError;

/// The entry's name.
pub(crate) const SELECTION_ENTRY: &str = "selection";

/// The entry's schema version.
pub(crate) const SELECTION_VERSION: u32 = 1;

/// Room for the header: 64 names of up to 64 characters (4 bytes each)
/// plus the fixed fields.
const HEADER_ALLOWANCE: u64 = 64 * 1024;

/// The largest entry read: the active mask, every saved one, the header.
pub(crate) const MAX_SELECTION_ENTRY_BYTES: u64 =
    MAX_SELECTION_MASK_PIXELS + MAX_SAVED_SELECTION_BYTES + HEADER_ALLOWANCE;

/// The farthest a selection's origin may lie from the document origin.
const MAX_ORIGIN: u64 = 1 << 32;

const KIND_RECT: u8 = 0;
const KIND_ELLIPSE: u8 = 1;
const KIND_MASK: u8 = 2;

#[derive(serde::Serialize, serde::Deserialize)]
struct VersionPrefix {
    version: u32,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct SelectionMeta {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    inverted: bool,
    kind: u8,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SelectionHeader {
    version: u32,
    active: Option<SelectionMeta>,
    saved: Vec<(String, SelectionMeta)>,
}

/// What reading the entry produced.
#[derive(Debug)]
pub(crate) enum SelectionRead {
    /// No entry: no selection.
    Absent,
    /// The restored selections.
    Restored(SelectionSet),
    /// The entry was unusable and was dropped; why, for the user.
    Dropped(String),
}

fn meta_of(selection: &Selection) -> SelectionMeta {
    let (kind, bounds) = match &selection.shape {
        SelectionShape::Rect => (KIND_RECT, selection.bounds),
        SelectionShape::Ellipse => (KIND_ELLIPSE, selection.bounds),
        SelectionShape::Mask(mask) => (KIND_MASK, mask.bounds()),
    };
    SelectionMeta {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
        inverted: selection.inverted,
        kind,
    }
}

/// The bytes of the `selection` entry for `set`, or `None` when it holds
/// nothing to write.
///
/// # Errors
///
/// [`IoError::ManifestSerialization`] if the header cannot be encoded.
pub(crate) fn encode_selection(set: &SelectionSet) -> Result<Option<Vec<u8>>, IoError> {
    if set.is_empty() {
        return Ok(None);
    }
    let active = set.active();
    let saved = set.saved_sorted();
    let header = SelectionHeader {
        version: SELECTION_VERSION,
        active: active.as_ref().map(meta_of),
        saved: saved
            .iter()
            .map(|(name, selection)| ((*name).to_owned(), meta_of(selection)))
            .collect(),
    };
    let mut bytes = postcard::to_allocvec(&header)
        .map_err(|source| IoError::ManifestSerialization(source.to_string()))?;
    let blobs = active
        .iter()
        .chain(saved.iter().map(|(_, selection)| *selection));
    for selection in blobs {
        if let SelectionShape::Mask(mask) = &selection.shape {
            bytes.extend_from_slice(mask.data());
        }
    }
    Ok(Some(bytes))
}

/// The selection `meta` describes, taking its coverage (if a mask) from
/// the front of `blobs`.
fn selection_of(meta: &SelectionMeta, blobs: &mut &[u8]) -> Result<Selection, String> {
    let bounds = Rect {
        x: meta.x,
        y: meta.y,
        width: meta.width,
        height: meta.height,
    };
    let shape = match meta.kind {
        KIND_RECT => SelectionShape::Rect,
        KIND_ELLIPSE => SelectionShape::Ellipse,
        KIND_MASK => {
            let len = usize::try_from(bounds.area_px()).map_err(|_| "a mask is too large")?;
            if blobs.len() < len {
                return Err("a mask is cut short".to_owned());
            }
            let (data, rest) = blobs.split_at(len);
            *blobs = rest;
            let mask = SelectionMask::new(bounds, data.to_vec()).map_err(|err| err.to_string())?;
            SelectionShape::Mask(std::sync::Arc::new(mask))
        }
        _ => return Err("it holds a kind of selection this build does not know".to_owned()),
    };
    Ok(Selection {
        bounds,
        inverted: meta.inverted,
        shape,
    })
}

/// Whether `meta`'s origin and size are sane, and its mask size if it is
/// one (every check that precedes any allocation).
fn checked_mask_bytes(meta: &SelectionMeta) -> Result<u64, String> {
    // `unsigned_abs`, not `abs`: `i64::MIN.abs()` overflows (a panic in a
    // build with overflow checks, a wrap past this very check without).
    if meta.x.unsigned_abs() > MAX_ORIGIN || meta.y.unsigned_abs() > MAX_ORIGIN {
        return Err("a selection lies absurdly far from the document".to_owned());
    }
    match meta.kind {
        KIND_RECT | KIND_ELLIPSE => Ok(0),
        KIND_MASK => {
            let pixels = u64::from(meta.width) * u64::from(meta.height);
            if pixels == 0 || pixels > MAX_SELECTION_MASK_PIXELS {
                Err("a mask is empty or larger than 64 megapixels".to_owned())
            } else {
                Ok(pixels)
            }
        }
        _ => Err("it holds a kind of selection this build does not know".to_owned()),
    }
}

/// Decodes the entry's `bytes`.
///
/// # Errors
///
/// [`IoError::ManifestDeserialization`] only for a version newer than
/// this build understands; any other problem is a `SelectionRead::Dropped`.
pub(crate) fn decode_selection(bytes: &[u8]) -> Result<SelectionRead, IoError> {
    let Ok((prefix, _)) = postcard::take_from_bytes::<VersionPrefix>(bytes) else {
        return Ok(SelectionRead::Dropped(
            "its selection was damaged".to_owned(),
        ));
    };
    if prefix.version > SELECTION_VERSION {
        return Err(IoError::ManifestDeserialization(format!(
            "unsupported selection version {} (this build understands version \
             {SELECTION_VERSION}); the file was saved by a newer Aurora",
            prefix.version
        )));
    }
    let dropped = |why: &str| {
        Ok(SelectionRead::Dropped(format!(
            "its selection was left out: {why}"
        )))
    };
    if prefix.version != SELECTION_VERSION {
        return dropped("it is of an unknown kind");
    }
    let Ok((header, mut blobs)) = postcard::take_from_bytes::<SelectionHeader>(bytes) else {
        return dropped("it is damaged");
    };
    if header.saved.len() > MAX_SAVED_SELECTIONS {
        return dropped("it holds too many saved selections");
    }
    // Every size is known and checked before any coverage is copied.
    let mut total = 0_u64;
    let mut saved_total = 0_u64;
    let metas = header
        .active
        .iter()
        .map(|meta| (false, meta))
        .chain(header.saved.iter().map(|(_, meta)| (true, meta)));
    for (is_saved, meta) in metas {
        match checked_mask_bytes(meta) {
            Ok(bytes) => {
                total += bytes;
                if is_saved {
                    saved_total += bytes;
                }
            }
            Err(why) => return dropped(&why),
        }
    }
    if saved_total > MAX_SAVED_SELECTION_BYTES {
        return dropped("its saved selections are too large");
    }
    if u64::try_from(blobs.len()).ok() != Some(total) {
        return dropped("its size does not add up");
    }
    let mut build = || -> Result<SelectionSet, String> {
        let active = match &header.active {
            Some(meta) => Some(selection_of(meta, &mut blobs)?),
            None => None,
        };
        let mut saved = Vec::with_capacity(header.saved.len());
        for (name, meta) in &header.saved {
            saved.push((name.clone(), selection_of(meta, &mut blobs)?));
        }
        SelectionSet::from_parts(active, saved).map_err(|err| err.to_string())
    };
    match build() {
        Ok(set) => Ok(SelectionRead::Restored(set)),
        Err(why) => dropped(&why),
    }
}

/// Reads the `selection` entry of `zip`.
///
/// # Errors
///
/// As [`decode_selection`]; also a container failure other than a missing
/// entry.
pub(crate) fn read_selection<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
) -> Result<SelectionRead, IoError> {
    let file = match zip.by_name(SELECTION_ENTRY) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(SelectionRead::Absent),
        Err(err) => return Err(err.into()),
    };
    match crate::aur::read_capped(file, SELECTION_ENTRY, MAX_SELECTION_ENTRY_BYTES) {
        Ok(bytes) => decode_selection(&bytes),
        Err(IoError::EntryTooLarge { .. }) => Ok(SelectionRead::Dropped(
            "its selection was left out: it is larger than allowed".to_owned(),
        )),
        Err(_) => Ok(SelectionRead::Dropped(
            "its selection was left out: it could not be read".to_owned(),
        )),
    }
}

#[cfg(test)]
#[path = "aur_selection_tests.rs"]
mod tests;
