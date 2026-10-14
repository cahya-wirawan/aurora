//! The optional `paths` entry of a `.aur` container (0.233.0): the
//! document's Bezier paths and its work path (ADR 0009's 0.233.0 addendum).
//!
//! One `postcard` entry: a frozen `version` first, then each path as a name
//! and its subpaths (a closed flag and anchors as six `f32`: point,
//! incoming handle offset, outgoing handle offset), saved paths in order
//! and the work path last. Written only when the document has a path (every
//! other save is byte-identical to 0.232.x); the container deflates it.
//!
//! **Rules.** A file without the entry has no paths; an Aurora older than
//! 0.233.0 never looks the entry up and so ignores it. A **newer** version
//! refuses the open, loudly, like `selection`; anything else wrong with the
//! entry (undecodable, larger than [`MAX_PATHS_ENTRY_BYTES`], over a cap of
//! [`aurora_doc::PathSet::from_parts`], a bad name, an older unknown version)
//! drops the paths, never the file, with a one-line reason. The entry is
//! read through a byte cap first, so the decoder never sees more than the
//! cap; every element takes at least one byte, which bounds what it can ask
//! for.

use std::io::{Read, Seek};

use aurora_doc::{Anchor, MAX_PATH_ANCHORS_TOTAL, Path, PathSet, Subpath, WORK_PATH_NAME};
use zip::ZipArchive;

use crate::error::IoError;

/// The entry's name.
pub(crate) const PATHS_ENTRY: &str = "paths";

/// The entry's schema version.
pub(crate) const PATHS_VERSION: u32 = 1;

/// The largest entry read: every anchor at 24 bytes plus its flags, and room
/// for the names and counts.
pub(crate) const MAX_PATHS_ENTRY_BYTES: u64 = (MAX_PATH_ANCHORS_TOTAL as u64) * 25 + 256 * 1024;

#[derive(serde::Serialize, serde::Deserialize)]
struct VersionPrefix {
    version: u32,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PathDto {
    name: String,
    subpaths: Vec<(bool, Vec<[f32; 6]>)>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PathsHeader {
    version: u32,
    paths: Vec<PathDto>,
    work: Option<PathDto>,
}

/// What reading the entry produced.
#[derive(Debug)]
pub(crate) enum PathsRead {
    /// No entry: no paths.
    Absent,
    /// The restored paths.
    Restored(PathSet),
    /// The entry was unusable and was dropped; why, for the user.
    Dropped(String),
}

fn dto_of(path: &Path) -> PathDto {
    PathDto {
        name: path.name.clone(),
        subpaths: path
            .subpaths
            .iter()
            .map(|s| {
                (
                    s.closed,
                    s.anchors
                        .iter()
                        .map(|a| {
                            [
                                a.point.0,
                                a.point.1,
                                a.handle_in.0,
                                a.handle_in.1,
                                a.handle_out.0,
                                a.handle_out.1,
                            ]
                        })
                        .collect(),
                )
            })
            .collect(),
    }
}

fn path_of(dto: PathDto) -> Path {
    Path {
        name: dto.name,
        subpaths: dto
            .subpaths
            .into_iter()
            .map(|(closed, anchors)| Subpath {
                closed,
                anchors: anchors
                    .into_iter()
                    .map(|[x, y, ix, iy, ox, oy]| Anchor {
                        point: (x, y),
                        handle_in: (ix, iy),
                        handle_out: (ox, oy),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The bytes of the `paths` entry for `set`, or `None` when it has no path.
///
/// # Errors
///
/// [`IoError::ManifestSerialization`] if the header cannot be encoded.
pub(crate) fn encode_paths(set: &PathSet) -> Result<Option<Vec<u8>>, IoError> {
    if set.is_empty() {
        return Ok(None);
    }
    let header = PathsHeader {
        version: PATHS_VERSION,
        paths: set.saved().iter().map(dto_of).collect(),
        work: set.work().map(dto_of),
    };
    postcard::to_allocvec(&header)
        .map(Some)
        .map_err(|source| IoError::ManifestSerialization(source.to_string()))
}

/// Decodes the entry's `bytes`.
///
/// # Errors
///
/// [`IoError::ManifestDeserialization`] only for a version newer than this
/// build understands; any other problem is a [`PathsRead::Dropped`].
pub(crate) fn decode_paths(bytes: &[u8]) -> Result<PathsRead, IoError> {
    let Ok((prefix, _)) = postcard::take_from_bytes::<VersionPrefix>(bytes) else {
        return Ok(PathsRead::Dropped("its paths were damaged".to_owned()));
    };
    if prefix.version > PATHS_VERSION {
        return Err(IoError::ManifestDeserialization(format!(
            "unsupported paths version {} (this build understands version \
             {PATHS_VERSION}); the file was saved by a newer Aurora",
            prefix.version
        )));
    }
    let dropped = |why: &str| {
        Ok(PathsRead::Dropped(format!(
            "its paths were left out: {why}"
        )))
    };
    if prefix.version != PATHS_VERSION {
        return dropped("they are of an unknown kind");
    }
    let Ok(header) = postcard::from_bytes::<PathsHeader>(bytes) else {
        return dropped("they are damaged");
    };
    let paths: Vec<Path> = header.paths.into_iter().map(path_of).collect();
    let work = header.work.map(path_of);
    if work.as_ref().is_some_and(|w| w.name != WORK_PATH_NAME) {
        return dropped("the work path is damaged");
    }
    match PathSet::from_parts(paths, work) {
        Ok(set) => Ok(PathsRead::Restored(set)),
        Err(err) => dropped(&err.to_string()),
    }
}

/// Reads the `paths` entry of `zip`.
///
/// # Errors
///
/// As [`decode_paths`]; also a container failure other than a missing entry.
pub(crate) fn read_paths<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<PathsRead, IoError> {
    let file = match zip.by_name(PATHS_ENTRY) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(PathsRead::Absent),
        Err(err) => return Err(err.into()),
    };
    match crate::aur::read_capped(file, PATHS_ENTRY, MAX_PATHS_ENTRY_BYTES) {
        Ok(bytes) => decode_paths(&bytes),
        Err(IoError::EntryTooLarge { .. }) => Ok(PathsRead::Dropped(
            "its paths were left out: they are larger than allowed".to_owned(),
        )),
        Err(_) => Ok(PathsRead::Dropped(
            "its paths were left out: they could not be read".to_owned(),
        )),
    }
}

#[cfg(test)]
#[path = "aur_paths_tests.rs"]
mod tests;
