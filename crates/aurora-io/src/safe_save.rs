//! Replacing a user's file safely (0.176.0): write to a fresh temp file
//! beside it, make it durable, verify it by reopening it, and only then
//! swap it in — so a failed write, a short write or a file that does not
//! read back never touches the original (CLAUDE.md: "never overwrite a
//! user's file in place").

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::IoError;

/// Writes `target` safely ([`write_verified_temp`], then
/// [`VerifiedTemp::persist`]):
///
/// 1. creates a new, uniquely named temp file (`.<name>.<random>.tmp`,
///    opened with `O_EXCL`, so it can never be an existing file — the
///    user's own `<name>.tmp` included) in `target`'s own directory, so
///    the final rename stays on one file system;
/// 2. runs `write` on it, then `sync_all`s it;
/// 3. runs `verify` on its **path** (verify opens the file itself), with
///    what `write` returned;
/// 4. renames it over `target`, then (Unix) fsyncs the directory so the
///    rename itself is durable.
///
/// On any error before the rename the temp file is removed and `target`
/// is untouched.
///
/// Permissions: a new file gets the usual `0o666` less the umask. On Unix
/// an existing target's permission bits are copied exactly onto the temp
/// file (through its handle, so the umask does not apply). Its owner,
/// group, ACLs and extended attributes are **not** kept, and a hard link
/// to the target is broken: the target becomes a new file. On Windows
/// nothing is copied — in particular not a read-only bit, so the temp
/// file stays deletable; replacing a read-only target then fails at the
/// rename, and the temp file is removed.
///
/// # Errors
///
/// Whatever `write` or `verify` returns; [`IoError::Io`] when the temp
/// file cannot be created, synced or renamed. A directory-sync failure
/// after the rename is **not** an error (0.247.0): the new file is in
/// place, so the save succeeded; use [`replace_verified_advised`] to see
/// the advisory it leaves.
pub fn replace_verified<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, IoError>,
    verify: impl FnOnce(&Path, &T) -> Result<(), IoError>,
) -> Result<T, IoError> {
    replace_verified_advised(target, write, verify).map(|(value, _advisory)| value)
}

/// [`replace_verified`] that also returns the directory-sync advisory
/// ([`VerifiedTemp::persist`]) instead of dropping it: `Some` means the
/// new file **is** in place but the system would not confirm that its
/// directory entry reached the disk.
///
/// # Errors
///
/// As [`replace_verified`], except that a directory-sync failure after
/// the rename is no longer an error (0.247.0).
pub fn replace_verified_advised<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, IoError>,
    verify: impl FnOnce(&Path, &T) -> Result<(), IoError>,
) -> Result<(T, Option<String>), IoError> {
    let (value, temp) = write_verified_temp(target, write, verify)?;
    let advisory = temp.persist()?;
    Ok((value, advisory))
}

/// The calm sentence a save that could not confirm its directory sync
/// carries (0.247.0): the file was written, verified and renamed into
/// place, and only the final "the rename reached the disk" confirmation
/// failed (some FUSE and network mounts refuse a directory `fsync`).
pub const DIR_SYNC_ADVISORY: &str = "The file was saved, but the system did not confirm that it reached the disk. Keep another copy until you are sure.";

/// A written, synced and verified temp file beside its target, not yet
/// swapped in ([`write_verified_temp`], 0.177.0): [`Self::persist`]
/// renames it over the target; dropping it deletes it and leaves the
/// target untouched. What a "Save With Changes?" dialog holds while the
/// user decides.
#[derive(Debug)]
pub struct VerifiedTemp {
    temp: tempfile::NamedTempFile,
    target: PathBuf,
    dir: PathBuf,
}

impl VerifiedTemp {
    /// The file it will replace — the real file when the requested path
    /// was a symbolic link ([`resolve_save_target`]).
    #[must_use]
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The temp file's own path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.temp.path()
    }

    /// Renames the temp file over the target, then (Unix) fsyncs the
    /// directory. Returns `Ok(None)` when the directory sync confirmed,
    /// and `Ok(Some(`[`DIR_SYNC_ADVISORY`]`))` when it failed: the rename
    /// had already happened, so the file **is** saved and the failure is
    /// only logged and handed back as an advisory (0.247.0; before that
    /// it was an error, and a mount that cannot sync a directory made
    /// every save look failed while the new file was already in place).
    ///
    /// # Errors
    ///
    /// [`IoError::Io`] when the rename fails (the temp file is then
    /// removed and the target untouched).
    pub fn persist(self) -> Result<Option<String>, IoError> {
        self.persist_with(sync_dir)
    }

    /// [`Self::persist`] with the directory sync injected (tests).
    ///
    /// # Errors
    ///
    /// As [`Self::persist`].
    pub fn persist_with(
        self,
        sync: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<Option<String>, IoError> {
        let Self { temp, target, dir } = self;
        temp.persist(&target)
            .map_err(|err| IoError::Io(err.error))?;
        match sync(&dir) {
            Ok(()) => Ok(None),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    dir = %dir.display(),
                    "the file was saved but its directory could not be synced"
                );
                Ok(Some(DIR_SYNC_ADVISORY.to_owned()))
            }
        }
    }
}

/// Where a save to `requested` really writes (0.177.0, N-2): a symbolic
/// link is followed to the file it names, so the link stays a link and
/// its target is what gets replaced (the temp file then sits beside the
/// real file, on its file system). Anything else is returned as is.
///
/// # Errors
///
/// [`IoError::Io`] for a link that names nothing (a dangling link is
/// refused rather than replaced by a regular file).
pub fn resolve_save_target(requested: &Path) -> Result<PathBuf, IoError> {
    match std::fs::symlink_metadata(requested) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(std::fs::canonicalize(requested)?),
        _ => Ok(requested.to_path_buf()),
    }
}

/// [`replace_verified`]'s first half (0.177.0): resolves `target`
/// ([`resolve_save_target`]), writes, syncs and verifies a temp file
/// beside it, and returns it unpersisted with `write`'s value. On any
/// error the temp file is removed; the target is never touched.
///
/// # Errors
///
/// As [`replace_verified`], short of the rename.
pub fn write_verified_temp<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, IoError>,
    verify: impl FnOnce(&Path, &T) -> Result<(), IoError>,
) -> Result<(T, VerifiedTemp), IoError> {
    write_verified_temp_with(target, write, |file| file.sync_all(), verify)
}

/// [`write_verified_temp`] with the data sync injected (tests): `sync`
/// runs after `write` and before `verify`, and its failure is fatal (the
/// temp file is removed, the target untouched).
///
/// # Errors
///
/// As [`write_verified_temp`].
pub fn write_verified_temp_with<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, IoError>,
    sync: impl FnOnce(&File) -> std::io::Result<()>,
    verify: impl FnOnce(&Path, &T) -> Result<(), IoError>,
) -> Result<(T, VerifiedTemp), IoError> {
    let target = resolve_save_target(target)?;
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut builder = tempfile::Builder::new();
    let prefix = format!(".{name}.");
    builder.prefix(&prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    // Dropping `temp` on any early return below deletes the file.
    let mut temp = builder.tempfile_in(&dir)?;
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(&target) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    let value = write(temp.as_file_mut())?;
    sync(temp.as_file())?;
    // `verify` gets the path, not this handle, and opens the file itself:
    // what it checks is what is on disk after the sync.
    verify(temp.path(), &value)?;
    Ok((value, VerifiedTemp { temp, target, dir }))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Windows has no directory handle to sync; `MoveFileEx` (what
/// `persist` uses) is as durable as the platform offers.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "safe_save_tests.rs"]
mod tests;
