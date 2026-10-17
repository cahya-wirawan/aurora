//! The optional `colour` entry of a `.aur` container (colour management,
//! round 1; ADR 0009's addendum): the document's **source profile** (the
//! RGB profile its file came in, whose pixels were converted into the sRGB
//! working space on import) and its **output profile** (chosen by the user
//! for export and, later, soft proofing). Neither describes the stored
//! pixels, which are always working-space sRGB; the manifest's own colour
//! field keeps that meaning.
//!
//! One `postcard` entry: a frozen `version`, then each profile's raw bytes
//! if set. Written only when the document has one (every other save is
//! byte-identical to before); the container deflates it. A file without the
//! entry has none; an older Aurora never looks it up. A **newer** version
//! refuses the open, like `selection`; anything else wrong with the entry
//! (undecodable, oversized, a profile that no longer passes
//! [`aurora_color::inspect_rgb_source`]) drops the colour metadata, never
//! the file, with a one-line reason. The entry is read through a byte cap
//! first ([`MAX_COLOUR_ENTRY_BYTES`]).

use std::io::{Read, Seek};

use aurora_color::{MAX_PROFILE_BYTES, inspect_rgb_source};
use zip::ZipArchive;

use crate::colour_import::SourceProfile;
use crate::error::IoError;

pub(crate) const COLOUR_ENTRY: &str = "colour";
pub(crate) const COLOUR_VERSION: u32 = 1;
/// Two profiles at the cap, plus the version and lengths.
pub(crate) const MAX_COLOUR_ENTRY_BYTES: u64 = 2 * MAX_PROFILE_BYTES as u64 + 64;

/// A document's colour metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentColour {
    /// The profile the document's file came in (pixels already converted).
    pub source: Option<SourceProfile>,
    /// The profile export (and later soft proofing) targets; `None` means
    /// sRGB.
    pub output: Option<SourceProfile>,
}

impl DocumentColour {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source.is_none() && self.output.is_none()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Wire {
    version: u32,
    source: Option<Vec<u8>>,
    output: Option<Vec<u8>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VersionPrefix {
    version: u32,
}

/// What reading the entry produced.
#[derive(Debug)]
pub(crate) enum ColourRead {
    Absent,
    Restored(DocumentColour),
    /// Unusable and dropped; why, for the user.
    Dropped(String),
}

/// The entry's bytes for `colour`, or `None` when it is empty.
///
/// # Errors
///
/// [`IoError::ManifestSerialization`] if it cannot be encoded.
pub(crate) fn encode_colour(colour: Option<&DocumentColour>) -> Result<Option<Vec<u8>>, IoError> {
    let Some(colour) = colour.filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let wire = Wire {
        version: COLOUR_VERSION,
        source: colour.source.as_ref().map(|p| p.bytes.clone()),
        output: colour.output.as_ref().map(|p| p.bytes.clone()),
    };
    postcard::to_allocvec(&wire)
        .map(Some)
        .map_err(|source| IoError::ManifestSerialization(source.to_string()))
}

fn judged(bytes: Vec<u8>) -> Option<SourceProfile> {
    let source = inspect_rgb_source(&bytes).ok()?;
    Some(SourceProfile {
        description: source.description,
        bytes,
    })
}

/// Decodes the entry's `bytes`.
///
/// # Errors
///
/// [`IoError::ManifestDeserialization`] only for a version newer than this
/// build understands.
pub(crate) fn decode_colour(bytes: &[u8]) -> Result<ColourRead, IoError> {
    let Ok((prefix, _)) = postcard::take_from_bytes::<VersionPrefix>(bytes) else {
        return Ok(ColourRead::Dropped(
            "its colour profiles were damaged and were left out".to_owned(),
        ));
    };
    if prefix.version > COLOUR_VERSION {
        return Err(IoError::ManifestDeserialization(format!(
            "unsupported colour version {} (this build understands version \
             {COLOUR_VERSION}); the file was saved by a newer Aurora",
            prefix.version
        )));
    }
    let Ok(wire) = postcard::from_bytes::<Wire>(bytes) else {
        return Ok(ColourRead::Dropped(
            "its colour profiles were damaged and were left out".to_owned(),
        ));
    };
    if wire.version != COLOUR_VERSION {
        return Ok(ColourRead::Dropped(
            "its colour profiles are of an unknown kind and were left out".to_owned(),
        ));
    }
    let source = wire.source.map(judged);
    let output = wire.output.map(judged);
    if matches!(source, Some(None)) || matches!(output, Some(None)) {
        return Ok(ColourRead::Dropped(
            "a colour profile it carried is not usable and was left out".to_owned(),
        ));
    }
    Ok(ColourRead::Restored(DocumentColour {
        source: source.flatten(),
        output: output.flatten(),
    }))
}

/// Reads the `colour` entry of `zip`.
///
/// # Errors
///
/// As [`decode_colour`]; also a container failure other than a missing
/// entry.
pub(crate) fn read_colour<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<ColourRead, IoError> {
    let file = match zip.by_name(COLOUR_ENTRY) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(ColourRead::Absent),
        Err(err) => return Err(err.into()),
    };
    match crate::aur::read_capped(file, COLOUR_ENTRY, MAX_COLOUR_ENTRY_BYTES) {
        Ok(bytes) => decode_colour(&bytes),
        Err(IoError::EntryTooLarge { .. }) => Ok(ColourRead::Dropped(
            "its colour profiles are larger than allowed and were left out".to_owned(),
        )),
        Err(_) => Ok(ColourRead::Dropped(
            "its colour profiles could not be read and were left out".to_owned(),
        )),
    }
}

#[cfg(test)]
#[path = "aur_colour_tests.rs"]
mod tests;
