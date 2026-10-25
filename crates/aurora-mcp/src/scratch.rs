//! The server's private scratch directory.
//!
//! Everything the server stages (a copy of a file being opened, an export
//! before it is copied into the write root) and everything the document
//! host spills (the tile store) lives in one directory, created `0700`,
//! locked for the life of the process and removed on exit. A `kill -9`
//! leaves it behind; the next server to start in the same parent sweeps
//! such leftovers, but only directories that carry this server's marker
//! file and whose lock can be taken ([`sweep_stale`] lists every condition);
//! a foreign directory that merely starts with the same letters is never
//! touched.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// The prefix every scratch directory has.
pub const PREFIX: &str = "aurora-mcp-";
/// The random characters after [`PREFIX`] in a directory's name.
pub const RANDOM_LEN: usize = 8;
/// Written into every directory this server creates; the sweep deletes only
/// directories that have it.
pub const MARKER_FILE: &str = ".aurora-mcp-scratch";
/// The lock file held for the life of the owning process.
pub const LOCK_FILE: &str = ".lock";

/// A locked, private scratch directory.
#[derive(Debug)]
pub struct Scratch {
    dir: tempfile::TempDir,
    _lock: File,
}

impl Scratch {
    /// Sweeps leftovers of dead servers from `parent` (the system temporary
    /// directory when `None`), then creates a fresh directory there.
    ///
    /// # Errors
    /// The directory or its lock file could not be created.
    pub fn create(parent: Option<&Path>) -> io::Result<Self> {
        let parent: PathBuf = parent.map_or_else(std::env::temp_dir, Path::to_path_buf);
        let _ = sweep_stale(&parent);
        let mut builder = tempfile::Builder::new();
        builder.prefix(PREFIX).rand_bytes(RANDOM_LEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir_in(&parent)?;
        std::fs::write(
            dir.path().join(MARKER_FILE),
            b"created by aurora-cli mcp; safe to delete when no server is running\n",
        )?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.path().join(LOCK_FILE))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(format!("couldn't lock the scratch directory: {e}")))?;
        Ok(Self { dir, _lock: lock })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Whether `name` has the exact shape the server gives its directories: the
/// prefix and [`RANDOM_LEN`] ASCII letters or digits, nothing else.
fn has_server_shape(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|rest| {
        rest.len() == RANDOM_LEN && rest.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// Removes scratch directories in `parent` whose owner is gone. Returns how
/// many it removed.
///
/// A directory is removed only if **all** of these hold, so that nothing a
/// person or another program made is ever deleted:
/// * its name is exactly [`PREFIX`] plus [`RANDOM_LEN`] random characters;
/// * it is a real directory (a symbolic link is never followed);
/// * it contains the [`MARKER_FILE`] this server writes at creation (a real
///   file, not a link);
/// * it contains the [`LOCK_FILE`] and that lock can be taken, which is
///   impossible while its owner lives.
///
/// A directory with a marker but no lock file is left alone: it may belong to
/// a live server whose lock file a temporary-file cleaner removed, and a
/// stale one costs only disk space. There is no age-based fallback.
#[must_use]
pub fn sweep_stale(parent: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !has_server_shape(&name.to_string_lossy()) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let is_file =
            |name: &str| std::fs::symlink_metadata(path.join(name)).is_ok_and(|m| m.is_file());
        if !is_file(MARKER_FILE) || !is_file(LOCK_FILE) {
            continue;
        }
        let takeable = File::options()
            .read(true)
            .write(true)
            .open(path.join(LOCK_FILE))
            .is_ok_and(|lock| lock.try_lock().is_ok());
        if takeable && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Free bytes (available to this user) on the file system holding `path`,
/// where the platform tells: `None` otherwise.
#[must_use]
pub fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path).ok()?;
        Some(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
