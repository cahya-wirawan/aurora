//! Tiles written to the scratch disk *before* the store knows them
//! (0.154.0).
//!
//! An opened document's tiles are built and encoded on a background
//! decode thread, which has no access to the [`TileStore`] (it is owned
//! by, and written only from, the UI thread). Until 0.154.0 the encoded
//! tiles travelled to the UI thread in memory and were handed to
//! [`TileStore::insert_encoded`], whose writer then put them on the
//! scratch disk — so the whole encoded document sat in memory at least
//! until the install. Now the decode thread writes each tile itself, into
//! a **staging area** the store's [`StagingRoot`] creates inside the
//! store's own scratch directory, and hands over only a [`StagedTile`]
//! (a path and a length); the install adopts each one with
//! [`TileStore::insert_staged`], which records the file as the tile's
//! paged-out copy — no I/O and no copy on the UI thread.
//!
//! Ownership, so nothing leaks and nothing is lost:
//!
//! - A [`StagedTile`] that is never adopted deletes its own file when it
//!   is dropped (a superseded or failed open, a layer whose surface is
//!   gone, a decode that panicked and unwound).
//! - A staging directory none of whose tiles was ever adopted is removed
//!   whole when its last handle drops. Once any tile is adopted the
//!   directory stays: the store now names files inside it. Each adopted
//!   file is deleted by the store when the tile is paged in (it is
//!   resident from then on, and a later eviction writes the store's own
//!   file) or forgotten; the directory itself, empty by then, is left for
//!   the scratch directory's own end-of-session sweep.
//! - Only the store whose [`StagingRoot`] made the area can adopt its
//!   tiles ([`TileError::ForeignStagedTile`]).
//!
//! [`TileStore`]: crate::TileStore
//! [`TileStore::insert_encoded`]: crate::TileStore::insert_encoded
//! [`TileStore::insert_staged`]: crate::TileStore::insert_staged

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::error::TileError;
use crate::store::EncodedTile;

/// One staging directory, shared by every tile staged in it.
#[derive(Debug)]
pub(crate) struct StagingDir {
    pub(crate) path: PathBuf,
    /// The owning store's instance token.
    pub(crate) owner: String,
    /// Set by the first [`crate::TileStore::insert_staged`]; until then the
    /// directory is removed whole when its last handle drops.
    pub(crate) adopted: AtomicBool,
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if self.adopted.load(Ordering::Acquire) {
            return;
        }
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                path = %self.path.display(),
                %err,
                "could not remove an unadopted tile staging directory"
            ),
        }
    }
}

/// Where a store's staging areas are made — cheap to clone and `Send`, so
/// a background thread can create its own area without the store
/// ([`crate::TileStore::staging_root`]).
#[derive(Clone, Debug)]
pub struct StagingRoot {
    pub(crate) scratch_dir: PathBuf,
    pub(crate) owner: String,
}

impl StagingRoot {
    /// A fresh, empty staging directory inside the store's scratch
    /// directory (owner-only on Unix). One per open.
    ///
    /// # Errors
    ///
    /// [`TileError::ScratchDirUnavailable`] if the directory cannot be
    /// created (it never reuses an existing one).
    pub fn area(&self) -> Result<StagingArea, TileError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = self
            .scratch_dir
            .join(format!("staging-{}-{sequence:x}", self.owner));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|source| TileError::ScratchDirUnavailable {
                path: path.clone(),
                source,
            })?;
        Ok(StagingArea {
            dir: Arc::new(StagingDir {
                path,
                owner: self.owner.clone(),
                adopted: AtomicBool::new(false),
            }),
            next: 0,
        })
    }
}

/// One open's staging directory: [`Self::stage`] writes a tile into it.
#[derive(Debug)]
pub struct StagingArea {
    dir: Arc<StagingDir>,
    next: u64,
}

impl StagingArea {
    /// The staging directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.dir.path
    }

    /// Writes `tile`'s encoded bytes — exactly what the store's writer
    /// would write for it — to a new file in this area.
    ///
    /// # Errors
    ///
    /// [`TileError::Staging`] if the file cannot be written; a partly
    /// written file is removed.
    pub fn stage(&mut self, tile: &EncodedTile) -> Result<StagedTile, TileError> {
        let path = self.dir.path.join(format!("{:x}.tile", self.next));
        self.next = self.next.wrapping_add(1);
        if let Err(source) = write_new(&path, tile.bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(TileError::Staging { path, source });
        }
        Ok(StagedTile {
            dir: Arc::clone(&self.dir),
            path: Some(path),
            len: tile.len(),
        })
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// One tile written to a [`StagingArea`], waiting to be adopted by
/// [`crate::TileStore::insert_staged`]. Deletes its file when dropped
/// unadopted.
#[derive(Debug)]
pub struct StagedTile {
    pub(crate) dir: Arc<StagingDir>,
    /// `None` once adopted.
    pub(crate) path: Option<PathBuf>,
    pub(crate) len: usize,
}

impl StagedTile {
    /// The encoded length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Always `false`: an encoded tile carries at least its header.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The staged file, while it is not adopted.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl Drop for StagedTile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => tracing::warn!(
                    path = %path.display(),
                    %err,
                    "could not remove an unadopted staged tile"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use half::f16;

    use crate::{EncodedTile, SurfaceId, Tile, TileId, TileSnapshot, TileStore};

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

    fn noisy(seed: u16) -> Tile {
        let mut tile = Tile::blank();
        for (i, texel) in tile.texels_mut().iter_mut().enumerate() {
            *texel = f16::from_bits(((i as u16).wrapping_mul(31) ^ seed) & 0x3bff);
        }
        tile
    }

    fn ok<T>(result: Result<T, crate::TileError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn files_under(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() { files_under(&path) } else { 1 }
                })
                .sum()
        })
    }

    /// An adopted staged tile reads back exactly as the same tile inserted
    /// in memory does, is snapshotted from its file without being paged
    /// in, is reported dirty once read, and its file is deleted on
    /// page-in.
    #[test]
    fn an_adopted_staged_tile_is_the_inserted_tile_and_is_cleaned_up_on_page_in() {
        let (_dir, mut store) = store();
        let (_other_dir, mut reference) = self::store();
        let surface = SurfaceId::from_raw(3);
        let id = TileId { x: 1, y: 2 };
        let tile = noisy(7);
        let encoded = EncodedTile::of(&tile);
        let mut area = ok(store.staging_root().area());
        let staged = ok(area.stage(&encoded));
        let Some(path) = staged.path().map(std::path::Path::to_path_buf) else {
            unreachable!("not adopted yet");
        };
        assert!(path.starts_with(area.path()));
        ok(store.insert_staged(surface, id, staged));
        reference.insert_encoded(surface, id, encoded.clone());
        drop(area);
        assert!(path.exists(), "an adopted area keeps its files");
        match ok(store.snapshot_tile(surface, id)) {
            Some(TileSnapshot::Encoded(bytes)) => assert_eq!(bytes.as_slice(), encoded.bytes()),
            other => unreachable!("{other:?}"),
        }
        let texels = ok(store.get(surface, id)).texels().to_vec();
        assert_eq!(texels, ok(reference.get(surface, id)).texels());
        assert_eq!(texels, tile.texels());
        assert!(
            store.take_dirty(surface, id).is_some(),
            "uploaded at the next sync"
        );
        assert!(!path.exists(), "deleted once paged in");
    }

    /// Nothing leaks: an unadopted area is removed whole, an unadopted
    /// tile deletes its file, a forgotten adopted tile's file goes with
    /// it, and another store's tile is refused (and deleted).
    #[test]
    fn staged_files_never_outlive_their_owner() {
        let (dir, mut store) = store();
        let scratch = dir.path().join("scratch");
        let tile = EncodedTile::of(&noisy(1));
        let mut area = ok(store.staging_root().area());
        let area_path = area.path().to_path_buf();
        let kept = ok(area.stage(&tile));
        let dropped = ok(area.stage(&tile));
        assert_eq!(files_under(&area_path), 2);
        drop(dropped);
        assert_eq!(files_under(&area_path), 1);
        drop(area);
        assert!(area_path.is_dir(), "a staged tile still holds the area");
        drop(kept);
        assert!(!area_path.exists(), "an unadopted area is removed whole");

        let mut area = ok(store.staging_root().area());
        let surface = SurfaceId::from_raw(1);
        let id = TileId { x: 0, y: 0 };
        ok(store.insert_staged(surface, id, ok(area.stage(&tile))));
        drop(area);
        assert_eq!(files_under(&scratch), 1);
        assert!(store.forget_tile(surface, id));
        assert_eq!(files_under(&scratch), 0, "forgotten with its file");

        let (_other_dir, other) = self::store();
        let mut foreign = ok(other.staging_root().area());
        let staged = ok(foreign.stage(&tile));
        let Some(path) = staged.path().map(std::path::Path::to_path_buf) else {
            unreachable!("not adopted");
        };
        assert!(matches!(
            store.insert_staged(surface, id, staged),
            Err(crate::TileError::ForeignStagedTile)
        ));
        assert!(!path.exists(), "a refused tile deletes its file");
        assert!(!store.contains_tile(surface, id));
    }

    /// 0.154.0 review I2: adopting a key whose earlier write may still be
    /// in flight keeps `forget_tile`'s tombstone. Then the tile is paged in
    /// and evicted again (a new write to the store's own path) before or
    /// after the old write's result drains: the tile must read back as
    /// the adopted content every time — the old result must never delete
    /// the newer file. Repeated, since whether the old write is still in
    /// flight is a race.
    #[test]
    fn an_adopted_key_with_an_older_write_in_flight_survives_a_re_eviction() {
        let (_dir, mut store) = store();
        let surface = SurfaceId::from_raw(5);
        for round in 0..64_u16 {
            let id = TileId {
                x: u32::from(round),
                y: 0,
            };
            store.insert_encoded(surface, id, EncodedTile::of(&noisy(round)));
            let adopted = noisy(round.wrapping_add(1000));
            let mut area = ok(store.staging_root().area());
            ok(store.insert_staged(surface, id, ok(area.stage(&EncodedTile::of(&adopted)))));
            drop(area);
            assert_eq!(ok(store.get(surface, id)).texels(), adopted.texels());
            // Evict it (budget 4): its next write goes to the store's own
            // `tile_path`, the file the old write's result names.
            for other in 0..6 {
                let _ = ok(store.get(
                    SurfaceId::from_raw(6),
                    TileId {
                        x: other,
                        y: u32::from(round),
                    },
                ));
            }
            if round % 2 == 0 {
                ok(store.flush());
            }
            assert_eq!(
                ok(store.get(surface, id)).texels(),
                adopted.texels(),
                "round {round}"
            );
            for other in 0..6 {
                let _ = ok(store.get(
                    SurfaceId::from_raw(6),
                    TileId {
                        x: other,
                        y: u32::from(round),
                    },
                ));
            }
        }
        ok(store.flush());
        for round in 0..64_u16 {
            let id = TileId {
                x: u32::from(round),
                y: 0,
            };
            assert_eq!(
                ok(store.get(surface, id)).texels(),
                noisy(round.wrapping_add(1000)).texels(),
                "after every write drained, round {round}"
            );
        }
    }

    /// A key already held is replaced whole by the adopted tile.
    #[test]
    fn adopting_over_a_held_tile_replaces_it() {
        let (_dir, mut store) = store();
        let surface = SurfaceId::from_raw(2);
        let id = TileId { x: 0, y: 0 };
        store.insert_encoded(surface, id, EncodedTile::of(&noisy(1)));
        let fresh = noisy(9);
        let mut area = ok(store.staging_root().area());
        ok(store.insert_staged(surface, id, ok(area.stage(&EncodedTile::of(&fresh)))));
        assert_eq!(ok(store.get(surface, id)).texels(), fresh.texels());
    }
}
