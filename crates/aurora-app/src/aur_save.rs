//! Save As `.aur` through the safe-save path (0.246.0, spec gap G-15).
//!
//! Before 0.246.0 the explicit `.aur` save wrote a fixed-name
//! `<name>.tmp` with `File::create` (not exclusive, so it truncated any
//! file of that name), never `sync_all`ed it or the directory, and
//! followed no symbolic-link policy. It now goes through
//! `aurora_io::replace_verified`, the path the PSD save already uses: a
//! unique `.<name>.<random>.tmp` opened exclusively beside the real
//! target (a symbolic link is followed, so the link stays a link), the
//! container written and `sync_all`ed, the temp file reopened and
//! verified **by path**, then renamed over the target and (Unix) the
//! directory synced. On any failure before the rename the temp file is
//! removed and the destination is untouched; the destination is never
//! opened for writing.
//!
//! The helpers are free functions so the tests inject a failing writer
//! or verifier without a window.

use std::fs::File;
use std::path::Path;

use aurora_io::IoError;

/// Everything a `.aur` save reads from the live document.
pub(crate) struct AurSaveSource<'a> {
    pub(crate) layers: &'a aurora_doc::LayerTree,
    pub(crate) history: &'a aurora_doc::History,
    pub(crate) canvas_size: (u32, u32),
    pub(crate) skipped_tiles: &'a aurora_io::SkippedTiles,
    pub(crate) import_losses: Option<&'a [String]>,
    pub(crate) selection: &'a aurora_doc::SelectionSet,
}

/// Writes `path` safely: `write` fills a fresh temp file, `verify` (given
/// the temp file's path) must accept it, and only then is it renamed over
/// `path`.
///
/// # Errors
///
/// Whatever `write` returns; an [`IoError::Io`] of kind `InvalidData`
/// when `verify` refuses the file; an [`IoError::Io`] when the temp file
/// cannot be created, synced or renamed. The temp file is removed in
/// every error case before the rename and the destination is untouched.
///
/// `Ok(Some(_))` (0.247.0) is a **successful** save whose directory sync
/// failed after the rename: the file is in place, durability unconfirmed.
pub(crate) fn replace_aur_verified(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<(), IoError>,
    verify: impl FnOnce(&Path) -> bool,
) -> Result<Option<String>, IoError> {
    aurora_io::replace_verified_advised(path, write, |temp, ()| {
        if verify(temp) {
            Ok(())
        } else {
            Err(IoError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the written .aur file did not read back",
            )))
        }
    })
    .map(|((), advisory)| advisory)
}

/// [`save_aur_document`] with the verifier injected.
///
/// # Errors
///
/// As [`replace_aur_verified`].
pub(crate) fn save_aur_document_with(
    path: &Path,
    source: &AurSaveSource<'_>,
    store: &mut aurora_tile::TileStore,
    verify: impl FnOnce(&Path) -> bool,
) -> Result<Option<String>, IoError> {
    replace_aur_verified(
        path,
        |file| {
            aurora_io::write_aur_with_import_losses(
                file,
                source.layers,
                source.history,
                source.canvas_size,
                None,
                // What this document already knows it is missing, so an
                // explicit Save of a file opened with tiles gone writes
                // that fact out again instead of erasing it.
                source.skipped_tiles,
                source.import_losses,
                // The selection in force (while Quick Mask is on, the
                // one it started from).
                Some(source.selection),
                store,
            )
        },
        verify,
    )
}

/// The explicit Save As `.aur`: [`save_aur_document_with`] verified by
/// reopening the container into a throwaway store (`crate::verify_aur`).
///
/// # Errors
///
/// As [`replace_aur_verified`].
pub(crate) fn save_aur_document(
    path: &Path,
    source: &AurSaveSource<'_>,
    store: &mut aurora_tile::TileStore,
) -> Result<Option<String>, IoError> {
    save_aur_document_with(path, source, store, crate::verify_aur)
}

#[cfg(test)]
#[path = "aur_save_tests.rs"]
mod tests;
