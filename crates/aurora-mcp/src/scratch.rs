//! The server's private scratch directory.
//!
//! Everything the server stages (a copy of a file being opened, an export
//! before it is copied into the write root) and everything the document
//! host spills (the tile store) lives in one directory, created `0700`,
//! locked for the life of the process and removed on exit. A `kill -9`
//! leaves it behind; the next server to start in the same parent sweeps
//! such leftovers: a directory is removed only after its lock file can be
//! locked, which is impossible while its owner lives, so a live server's
//! directory is never touched.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The prefix every scratch directory has.
pub const PREFIX: &str = "aurora-mcp-";
const LOCK_FILE: &str = ".lock";
/// A directory without a lock file this young is a server still starting.
const GRACE: Duration = Duration::from_secs(60);

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
        sweep_stale(&parent);
        let mut builder = tempfile::Builder::new();
        builder.prefix(PREFIX);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir_in(&parent)?;
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

/// Removes scratch directories in `parent` whose owner is gone. Returns how
/// many it removed. Never touches a directory whose lock is held.
pub fn sweep_stale(parent: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let path = entry.path();
        // A real directory only: never follow a link.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let stale = match File::options().read(true).write(true).open(path.join(LOCK_FILE)) {
            Ok(lock) => lock.try_lock().is_ok(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => meta
                .modified()
                .ok()
                .and_then(|m| SystemTime::now().duration_since(m).ok())
                .is_some_and(|age| age > GRACE),
            Err(_) => false,
        };
        if stale && std::fs::remove_dir_all(&path).is_ok() {
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
