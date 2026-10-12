//! Spill files for the pixel history (0.210.0).
//!
//! `aurora-brush`'s `PixelHistory` keeps every stroke's pre-edit tiles,
//! compressed ([`crate::codec`]), so the undo stack is unbounded in step
//! count (the PRD's "unlimited history"). Its *memory* is bounded by a
//! budget ([`crate::system_history_budget_bytes`]); the oldest
//! compressed snapshots that do not fit are written here, one file per
//! snapshot, and read back on undo. Nothing is ever dropped.
//!
//! This module is only the file layer, built on the hardening the tile
//! store's scratch directory already has:
//!
//! - A [`SpillArea`] is a fresh directory (owner-only on Unix) inside the
//!   store's own scratch directory, which is itself private
//!   (`TileStore`'s `create_private_dir`), made through the store's
//!   [`StagingRoot`]. It is removed whole when the area drops, so a
//!   closed document leaves nothing behind; a crash leaves it inside the
//!   scratch directory that the startup sweep removes.
//! - A [`SpillFile`] is created with `O_CREAT | O_EXCL` (a pre-planted
//!   symlink or file is an error, never followed) and mode `0600`, and
//!   opened for reading with `O_NOFOLLOW`. It deletes itself when
//!   dropped, which is how a truncated redo stack or a discarded
//!   document frees its spilled steps.
//! - Failures are returned, never swallowed: the history keeps the data
//!   in memory when a spill fails.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::TileError;
use crate::staging::StagingRoot;

/// A private directory that holds spill files, removed when dropped.
#[derive(Debug)]
pub struct SpillArea {
    dir: PathBuf,
    next: AtomicU64,
}

impl StagingRoot {
    /// A fresh, empty spill directory inside the store's scratch
    /// directory (owner-only on Unix), for the pixel history. One per
    /// document session.
    ///
    /// # Errors
    ///
    /// [`TileError::ScratchDirUnavailable`] if it cannot be created (it
    /// never reuses an existing directory).
    pub fn spill_area(&self) -> Result<SpillArea, TileError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = self
            .scratch_dir
            .join(format!("history-{}-{sequence:x}", self.owner));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .map_err(|source| TileError::ScratchDirUnavailable {
                path: dir.clone(),
                source,
            })?;
        Ok(SpillArea {
            dir,
            next: AtomicU64::new(0),
        })
    }
}

impl SpillArea {
    /// The directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Writes `parts`, concatenated, to a new file in this area.
    ///
    /// # Errors
    ///
    /// [`TileError::Spill`] if the file cannot be created or written; a
    /// partly written file is removed.
    pub fn write(&self, parts: &[&[u8]]) -> Result<SpillFile, TileError> {
        let name = self.next.fetch_add(1, Ordering::Relaxed);
        let path = self.dir.join(format!("{name:x}.hist"));
        let mut len: u64 = 0;
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&path)?;
            for part in parts {
                file.write_all(part)?;
                len = len.saturating_add(part.len() as u64);
            }
            file.flush()
        })();
        match result {
            Ok(()) => Ok(SpillFile { path, len }),
            Err(source) => {
                let _ = std::fs::remove_file(&path);
                Err(TileError::Spill { path, source })
            }
        }
    }
}

impl Drop for SpillArea {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                path = %self.dir.display(),
                %err,
                "could not remove a pixel-history spill directory"
            ),
        }
    }
}

/// One spilled file; deletes itself when dropped.
#[derive(Debug)]
pub struct SpillFile {
    path: PathBuf,
    len: u64,
}

impl SpillFile {
    /// The file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The bytes written.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// `true` if nothing was written.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reads `len` bytes at `offset`.
    ///
    /// # Errors
    ///
    /// [`TileError::Spill`] if the range is outside the file as written,
    /// or the file cannot be opened or read (it was removed, say).
    pub fn read_range(&self, offset: u64, len: usize) -> Result<Vec<u8>, TileError> {
        let fail = |source: std::io::Error| TileError::Spill {
            path: self.path.clone(),
            source,
        };
        offset
            .checked_add(len as u64)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| {
                fail(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "range outside the spilled file",
                ))
            })?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&self.path).map_err(fail)?;
        file.seek(SeekFrom::Start(offset)).map_err(fail)?;
        let mut bytes = vec![0_u8; len];
        file.read_exact(&mut bytes).map_err(fail)?;
        Ok(bytes)
    }
}

impl Drop for SpillFile {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                path = %self.path.display(),
                %err,
                "could not remove a pixel-history spill file"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{TileError, TileStore};

    fn store() -> (tempfile::TempDir, TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(4) else {
            unreachable!("non-zero");
        };
        match TileStore::new(dir.path().join("scratch"), budget) {
            Ok(store) => (dir, store),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn entries(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| entries.flatten().count())
    }

    #[test]
    fn write_read_range_and_cleanup() {
        let (_dir, store) = store();
        let area = match store.staging_root().spill_area() {
            Ok(area) => area,
            Err(err) => unreachable!("{err:?}"),
        };
        let file = match area.write(&[b"hello ", b"world"]) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(file.len(), 11);
        assert!(!file.is_empty());
        assert_eq!(file.read_range(6, 5).ok().as_deref(), Some(&b"world"[..]));
        assert_eq!(
            file.read_range(0, 11).ok().as_deref(),
            Some(&b"hello world"[..])
        );
        assert!(
            matches!(file.read_range(6, 6), Err(TileError::Spill { .. })),
            "a range past the end is refused"
        );
        assert!(
            matches!(file.read_range(u64::MAX, 2), Err(TileError::Spill { .. })),
            "an overflowing range is refused"
        );
        assert_eq!(entries(area.path()), 1);
        let path = file.path().to_path_buf();
        drop(file);
        assert!(!path.exists(), "dropping a spill file deletes it");
        assert_eq!(entries(area.path()), 0);
    }

    #[test]
    fn dropping_the_area_removes_the_directory_and_its_files() {
        let (_dir, store) = store();
        let area = match store.staging_root().spill_area() {
            Ok(area) => area,
            Err(err) => unreachable!("{err:?}"),
        };
        let dir = area.path().to_path_buf();
        let file = area.write(&[b"x"]);
        assert!(file.is_ok());
        drop(area);
        assert!(!dir.exists());
        // The file handle outliving the area is harmless.
        drop(file);
    }

    #[test]
    fn a_missing_directory_is_a_returned_error_not_a_panic() {
        let (_dir, store) = store();
        let area = match store.staging_root().spill_area() {
            Ok(area) => area,
            Err(err) => unreachable!("{err:?}"),
        };
        let _ = std::fs::remove_dir_all(area.path());
        assert!(matches!(area.write(&[b"x"]), Err(TileError::Spill { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_is_owner_only_and_a_planted_symlink_is_not_followed() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, store) = store();
        let area = match store.staging_root().spill_area() {
            Ok(area) => area,
            Err(err) => unreachable!("{err:?}"),
        };
        let mode = std::fs::metadata(area.path()).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o700));
        // Plant a symlink where the next file would go.
        let target = dir.path().join("victim");
        assert!(std::fs::write(&target, b"keep").is_ok());
        let planted = area.path().join("0.hist");
        assert!(std::os::unix::fs::symlink(&target, &planted).is_ok());
        assert!(
            matches!(area.write(&[b"overwrite"]), Err(TileError::Spill { .. })),
            "create_new refuses an existing name, symlink or not"
        );
        assert_eq!(std::fs::read(&target).ok().as_deref(), Some(&b"keep"[..]));
    }
}
