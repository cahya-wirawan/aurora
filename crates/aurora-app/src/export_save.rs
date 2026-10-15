//! Flat-image export (PNG/JPEG/TIFF) through the safe-save path (0.247.0).
//!
//! Before 0.247.0 the export wrote a fixed-name `<name>.tmp` with
//! `std::fs::write` (so it truncated any file of the user's that had that
//! name), never `sync_all`ed the file or its directory, and followed no
//! symbolic link. It now goes through `aurora_io::write_verified_temp_with`
//! and `VerifiedTemp::persist_with`, the path `.aur` and PSD saves use: a
//! unique `.<name>.<random>.tmp` opened exclusively beside the real target
//! (a symbolic link is followed, so the link stays a link), the encoded
//! bytes written and `sync_all`ed, the temp file reread and decoded **by
//! path** and checked to be a real image of exactly the expected size,
//! then renamed over the target and (Unix) the directory synced. On any
//! failure before the rename the temp file is removed and the destination
//! is untouched; the destination is never opened for writing.
//!
//! A directory-sync failure after the rename is not a failure (the file is
//! in place); it comes back as `Ok(Some(advisory))`.
//!
//! The `_with` helper takes both syncs as closures so the tests inject
//! failures without a window or a special file system.

use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use aurora_io::IoError;

/// Writes `bytes` (an encoded image of `width` x `height`, in the format
/// `path`'s extension names) to `path` safely.
///
/// # Errors
///
/// [`IoError::Io`] when the temp file cannot be created, written, synced
/// or renamed, or when it does not read back as a decodable image of the
/// expected size (`InvalidData`). The temp file is removed in every error
/// case and `path` is untouched.
pub(crate) fn write_export_verified(
    path: &Path,
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Option<String>, IoError> {
    write_export_verified_with(
        path,
        bytes,
        width,
        height,
        |file| file.sync_all(),
        |dir| sync_directory(dir),
    )
}

/// [`write_export_verified`] with the data sync and the directory sync
/// injected.
///
/// # Errors
///
/// As [`write_export_verified`]; `sync_data` failing is an error,
/// `sync_dir` failing is not (it becomes the advisory).
pub(crate) fn write_export_verified_with(
    path: &Path,
    bytes: &[u8],
    width: u32,
    height: u32,
    sync_data: impl FnOnce(&File) -> std::io::Result<()>,
    sync_dir: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<Option<String>, IoError> {
    if path.file_name().is_none() {
        return Err(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the export path has no file name",
        )));
    }
    let ((), temp) = aurora_io::write_verified_temp_with(
        path,
        |file| {
            file.write_all(bytes)?;
            Ok(())
        },
        sync_data,
        |temp, ()| {
            // The temp file's own extension is `.tmp`, so the format is
            // taken from the path the user chose.
            let reread = std::fs::read(temp)?;
            let image = aurora_io::decode_by_extension(path, &reread)?;
            if image.width() == width && image.height() == height {
                Ok(())
            } else {
                Err(IoError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the exported file did not read back at the expected size",
                )))
            }
        },
    )?;
    temp.persist_with(sync_dir)
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Windows has no directory handle to sync; the rename is as durable as
/// the platform offers.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_directory(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "export_save_tests.rs"]
mod tests;
