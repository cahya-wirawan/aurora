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
/// file cannot be created, synced or renamed, or the directory synced.
/// A directory-sync failure is reported after the rename has happened:
/// the new file is in place but its durability is not confirmed.
pub fn replace_verified<T>(
    target: &Path,
    write: impl FnOnce(&mut File) -> Result<T, IoError>,
    verify: impl FnOnce(&Path, &T) -> Result<(), IoError>,
) -> Result<T, IoError> {
    let (value, temp) = write_verified_temp(target, write, verify)?;
    temp.persist()?;
    Ok(value)
}

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
    /// directory.
    ///
    /// # Errors
    ///
    /// [`IoError::Io`] when the rename fails (the temp file is then
    /// removed and the target untouched) or the directory sync does
    /// (after the rename: the new file is in place, its durability not
    /// confirmed).
    pub fn persist(self) -> Result<(), IoError> {
        let Self { temp, target, dir } = self;
        temp.persist(&target)
            .map_err(|err| IoError::Io(err.error))?;
        sync_dir(&dir)
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
    temp.as_file().sync_all()?;
    // `verify` gets the path, not this handle, and opens the file itself:
    // what it checks is what is on disk after the sync.
    verify(temp.path(), &value)?;
    Ok((value, VerifiedTemp { temp, target, dir }))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), IoError> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

/// Windows has no directory handle to sync; `MoveFileEx` (what
/// `persist` uses) is as durable as the platform offers.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_dir(_dir: &Path) -> Result<(), IoError> {
    Ok(())
}
