//! A dedicated background thread that writes evicted tiles to the
//! scratch disk, so eviction never blocks the caller (the M1.1
//! "background writer" deliverable).
//!
//! Deliberately **not** `tokio`: this is one thread draining one queue,
//! not general async I/O -- a plain `std::thread` + `std::sync::mpsc`
//! (stdlib, no new dependency) is the right-sized tool. Revisit only if
//! `aurora-tile` grows genuinely concurrent I/O needs beyond one writer.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use crate::tile::{SurfaceId, TileId};

pub(crate) struct WriteJob {
    pub(crate) surface: SurfaceId,
    pub(crate) id: TileId,
    /// Which *submission* this is for `(surface, id)`, minted by
    /// `TileStore::make_room`. Echoed back on the [`WriteResult`] below
    /// so the store can tell a result for the bytes `pending` still
    /// holds from one for bytes a later eviction already superseded --
    /// without it, an older write's completion clears a newer entry
    /// and a later read silently returns pre-edit pixels.
    pub(crate) generation: u64,
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
}

/// Result of a background write, reported back so [`crate::store::TileStore`]
/// can track failures without blocking on them.
pub(crate) struct WriteResult {
    pub(crate) surface: SurfaceId,
    pub(crate) id: TileId,
    /// The submitted [`WriteJob::generation`], carried through
    /// untouched: this thread never interprets the value, it only hands
    /// it back so the store can match a result against what `pending`
    /// currently holds for the same key.
    pub(crate) generation: u64,
    pub(crate) outcome: std::io::Result<()>,
}

/// Writes one scratch tile file, owner-only where the platform lets
/// this crate say so. `0o600` is set by `OpenOptions` at creation, not
/// chmod-ed afterwards, so there is no window in which the file exists
/// wider than intended. Defence in depth: the containing scratch
/// directory is already `0o700` ([`crate::store`]'s `create_private_dir`),
/// which on its own denies every other user the traversal needed to
/// reach anything inside it.
///
/// The mode applies only when this call *creates* the file; a rewrite
/// of an existing path keeps that file's mode, which is fine because
/// every file here was created by this process, in a directory only it
/// can traverse. Rewriting is not an edge case -- it is how a later
/// generation supersedes an earlier one for the same key, so
/// `create_new(true)` is deliberately *not* used: it would fail every
/// such rewrite and break the FIFO invariant [`BackgroundWriter::spawn`]
/// documents.
///
/// **This is defence in depth *on top of* the directory's `0o700`, not
/// protection independent of it.** `open` follows symlinks, and `open`'s
/// mode argument is consulted only when the call actually creates a new
/// inode -- so if the containing directory's protection ever fails or is
/// bypassed, a pre-existing file (or a planted symlink) at this path is
/// truncated and overwritten at *its* mode, and the `0o600` requested
/// here buys nothing. The `0o700` scratch directory, which denies every
/// other user the traversal needed to reach or create anything inside
/// it, is what actually holds that line today. Closing the gap properly
/// needs `O_NOFOLLOW` on this open, tracked with the same requirement
/// for `create_private_dir` in `PLAN.md` -- it needs a `libc` dependency
/// and an `unsafe_code` override, so it is its own architecture
/// decision rather than part of this helper.
///
/// Windows ACLs are *not* addressed here: `OpenOptions` has no portable
/// equivalent -- the same gap `aurora_app::create_autosave_temp` and
/// `create_private_dir` already disclose. A Windows scratch tile is
/// created with the parent directory's inherited ACL.
fn write_tile_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

pub(crate) struct BackgroundWriter {
    tx: Option<Sender<WriteJob>>,
    handle: Option<JoinHandle<()>>,
    results_rx: mpsc::Receiver<WriteResult>,
}

impl BackgroundWriter {
    /// Spawns the writer thread. `tx`'s send is unbounded, so
    /// [`Self::submit`] never blocks the caller regardless of how far
    /// behind the writer thread is -- that non-blocking property is the
    /// entire point of this module.
    ///
    /// # Load-bearing invariant: one thread, one queue, strictly FIFO
    ///
    /// **Writes for the same key complete in submission order, and the
    /// store's stale-write fix depends on that for its *disk-content*
    /// correctness -- not just its in-memory bookkeeping.** This is not
    /// an implementation detail to preserve out of tidiness; it is half
    /// of a correctness argument whose other half lives in
    /// `TileStore::is_superseded`.
    ///
    /// The generation carried on every [`WriteJob`] protects the store's
    /// `pending` map: a superseded result cannot clear (or record a
    /// failure against) bytes that are not its own. It says nothing at
    /// all about which bytes end up in the *file*. That comes entirely
    /// from the loop below -- a single thread draining a single `mpsc`
    /// queue, one [`write_tile_file`] at a time, in the order jobs were
    /// sent --
    /// combined with `TileStore::make_room` minting generations in that
    /// same submission order. Two evictions of one tile therefore write
    /// the older bytes first and the newer bytes second, so the file is
    /// left holding the newest submission's content.
    ///
    /// Break that and the bug class reopens with no test able to catch
    /// it. If writes were spread across a thread pool (a plausible
    /// future optimization -- tile writes are embarrassingly parallel),
    /// jobs 1, 2 and 3 for one key could complete 1, 3, 2: `pending`
    /// would still be cleared correctly, by generation 3's own result,
    /// while the file on disk held generation 2's bytes. The next read
    /// falls through to that file and silently returns superseded
    /// pixels -- with the generation check present the whole time,
    /// giving false confidence that it covers exactly this.
    ///
    /// So: any future change that parallelizes or reorders tile writes
    /// **must** preserve per-key write ordering (e.g. by sharding jobs
    /// to workers by `(surface, id)` rather than round-robin), or the
    /// store needs a different disk-side guarantee -- writing to a
    /// generation-stamped temporary and renaming only if it is still the
    /// newest, say. `writes_for_one_path_land_in_submission_order` below
    /// is what pins the current guarantee, and it is the only test that
    /// pins it *deterministically*: none of `store.rs`'s deterministic
    /// cases can see disk-write ordering at all, because with the
    /// generation check doing its job `pending` reconciles correctly
    /// whichever order the bytes land in and only the file is wrong.
    /// (Measured: parallelizing this loop makes the test below fail
    /// outright, and `store.rs`'s randomized churn test fail too, on
    /// real stale reads -- but that one is timing-dependent by
    /// construction, so it is a second line of defence rather than a
    /// guarantee.)
    pub(crate) fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<WriteJob>();
        let (results_tx, results_rx) = mpsc::channel::<WriteResult>();
        let handle = std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let outcome = write_tile_file(&job.path, &job.bytes);
                // The store may already be gone (e.g. process shutdown
                // mid-write) -- a dropped results receiver is not this
                // thread's problem to report anywhere.
                let _ = results_tx.send(WriteResult {
                    surface: job.surface,
                    id: job.id,
                    generation: job.generation,
                    outcome,
                });
            }
        });
        Self {
            tx: Some(tx),
            handle: Some(handle),
            results_rx,
        }
    }

    /// Never blocks: this is what makes eviction non-blocking.
    pub(crate) fn submit(&self, job: WriteJob) {
        if let Some(tx) = &self.tx {
            // If the writer thread already panicked, there is nothing a
            // caller can usefully do about a dropped receiver at this
            // point -- the failure surfaces the next time `drain_results`
            // is polled and finds nothing, not as a panic here.
            let _ = tx.send(job);
        }
    }

    /// Non-blocking: returns whatever write results have completed so
    /// far, without waiting for more.
    pub(crate) fn drain_results(&self) -> Vec<WriteResult> {
        self.results_rx.try_iter().collect()
    }

    /// Drops the sender (so the writer thread's `recv()` loop ends once
    /// its queue drains) and joins the thread, so every already-submitted
    /// write has actually completed before this returns. Used before a
    /// document save.
    pub(crate) fn flush(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for BackgroundWriter {
    fn drop(&mut self) {
        self.flush();
    }
}

impl std::fmt::Debug for BackgroundWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundWriter").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundWriter, WriteJob};
    use crate::tile::{SurfaceId, TileId};
    use std::time::{Duration, Instant};

    #[test]
    fn writes_a_tile_to_disk() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let path = dir.path().join("0_0.tile");
        let mut writer = BackgroundWriter::spawn();
        writer.submit(WriteJob {
            surface: SurfaceId::from_raw(0),
            id: TileId { x: 0, y: 0 },
            generation: 0,
            path: path.clone(),
            bytes: vec![1, 2, 3, 4],
        });
        writer.flush();
        let written = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                unreachable!("flush() joined the writer thread, so the file must exist: {err}")
            }
        };
        assert_eq!(written, vec![1, 2, 3, 4]);
    }

    /// A scratch tile file must be created with no group or other
    /// permission bits, and must stay rewritable by a later generation
    /// for the same key.
    ///
    /// **The mode assertion is this test's own contribution.** The
    /// rewrite half is *also* pinned here only because it is convenient
    /// to check both properties from one run -- the primary,
    /// pre-existing guarantee for the rewrite case is
    /// `writes_for_one_path_land_in_submission_order` below, which
    /// submits three jobs to one path, is not `unix`-gated, and would
    /// already fail on every platform if `create_new(true)` were used by
    /// mistake. What this test adds is that the mode hardening did not
    /// cost that path *under the same `OpenOptions` call* the mode is
    /// requested on.
    ///
    /// # Guards the guard
    ///
    /// The mode is asserted as `& 0o077 == 0` -- "never wider than
    /// owner-only" -- rather than exactly `0o600`, for two reasons.
    /// Owner-only is the actual security property; and a sufficiently
    /// restrictive ambient umask (`0o377`, say) can strip owner bits
    /// too, which would make an exact-equality assertion a false failure
    /// on such a system even with the fix correctly present.
    ///
    /// That assertion alone would be vacuous under a common ambient
    /// umask: `0o077` is the default on hardened Linux, many container
    /// images, and several CI hardening profiles, and under it the *old*
    /// `fs::write` code (which requests `0o666`) also lands `0o600`. So
    /// this test first writes a sibling control file with plain
    /// `std::fs::write`, bypassing `write_tile_file` entirely, and
    /// requires that the ambient umask really does leave *it* wider than
    /// owner-only. If it does not, the premise assertion fails loudly
    /// rather than letting the real assertion pass while proving
    /// nothing -- the same shape as `store.rs`'s
    /// `new_leaves_the_scratch_directory_owner_only`, which pre-creates
    /// at `0o777` for exactly this reason. (Verified by mutation:
    /// reverting `write_tile_file` to `fs::write` makes this fail under
    /// both `umask 0022` and `umask 0077`.)
    #[cfg(unix)]
    #[test]
    fn a_scratch_tile_file_is_created_owner_only_and_stays_rewritable() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };

        // Guards the guard: a control file created the way the writer
        // used to create tiles. If the ambient umask already forces
        // owner-only on *this*, the assertion below cannot distinguish
        // the fix from its absence, and must say so rather than pass.
        let control = dir.path().join("umask-control");
        if let Err(err) = std::fs::write(&control, b"control") {
            unreachable!("writing a file into a fresh tempdir must succeed: {err}");
        }
        let control_mode = match std::fs::metadata(&control) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(err) => unreachable!("the control file was just written: {err}"),
        };
        assert_ne!(
            control_mode & 0o077,
            0,
            "this test's premise is an ambient umask that permits a wider-than-owner-only mode by \
             default (the control file came back {control_mode:04o}); this environment's umask \
             already restricts every new file to owner-only, so the assertion below would hold \
             with or without `write_tile_file`'s own `.mode(0o600)` and cannot distinguish the \
             fix from its absence -- rerun with a laxer umask (e.g. 0o022)"
        );

        let path = dir.path().join("0_0.tile");
        let mut writer = BackgroundWriter::spawn();
        for generation in 1..=2_u64 {
            writer.submit(WriteJob {
                surface: SurfaceId::from_raw(0),
                id: TileId { x: 0, y: 0 },
                generation,
                path: path.clone(),
                bytes: vec![generation as u8; 8],
            });
        }
        writer.flush();

        let written = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                unreachable!("flush() joined the writer thread, so the file must exist: {err}")
            }
        };
        assert_eq!(
            written,
            vec![2_u8; 8],
            "hardening the file's mode must not have cost the rewrite path: a later generation \
             for the same key still has to land its own bytes"
        );

        let mode = match std::fs::metadata(&path) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(err) => unreachable!("the file the writer just wrote must exist: {err}"),
        };
        assert_eq!(
            mode & 0o077,
            0,
            "a scratch tile holds the document's real unsaved pixel data; the background writer \
             must create it owner-only rather than at whatever the process umask happens to \
             allow, and this one came back {mode:04o} (the control file above confirms this \
             environment's umask would have permitted a wider mode)"
        );
    }

    /// Pins the FIFO invariant [`BackgroundWriter::spawn`]'s own doc
    /// comment explains the store depends on: three writes to one path,
    /// submitted in generation order, must leave the file holding the
    /// *last* one's bytes.
    ///
    /// This documents a property the current design has by construction
    /// -- one thread, one queue -- rather than fixing anything: there is
    /// no live bug here, and against this implementation the test cannot
    /// fail. That is the point. It exists so that a change to the
    /// threading model (a worker pool, round-robin dispatch, an async
    /// runtime) has something concrete and deterministic to fail
    /// against. Verified by mutation: dispatching each job to its own
    /// worker thread with descending delays makes this fail with the
    /// *first* submission's bytes on disk. `store.rs`'s randomized churn
    /// test also fails under that mutation, on genuine stale reads --
    /// but timing-dependently, and every deterministic store-level test
    /// stays green, because with the generation check doing its job
    /// `pending` reconciles correctly whichever order the bytes land in
    /// and only the file is wrong.
    ///
    /// The bytes are deliberately distinguishable per generation, so a
    /// failure names which submission won rather than just that one did.
    #[test]
    fn writes_for_one_path_land_in_submission_order() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let path = dir.path().join("0_0.tile");
        let mut writer = BackgroundWriter::spawn();
        for generation in 1..=3_u64 {
            writer.submit(WriteJob {
                surface: SurfaceId::from_raw(0),
                id: TileId { x: 0, y: 0 },
                generation,
                path: path.clone(),
                bytes: vec![generation as u8; 16],
            });
        }
        writer.flush();
        let written = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                unreachable!("flush() joined the writer thread, so the file must exist: {err}")
            }
        };
        assert_eq!(
            written,
            vec![3_u8; 16],
            "the newest submission for a key must be the one left on disk; anything else means \
             writes for one key can complete out of order, which reopens the stale-write bug on \
             the disk side where no store-level test can see it"
        );
    }

    #[test]
    fn submit_never_blocks_even_before_the_writer_drains() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("tempdir creation must succeed in a test environment: {err}"),
        };
        let writer = BackgroundWriter::spawn();
        let start = Instant::now();
        for i in 0..100 {
            writer.submit(WriteJob {
                surface: SurfaceId::from_raw(0),
                id: TileId { x: i, y: 0 },
                generation: 0,
                path: dir.path().join(format!("{i}_0.tile")),
                bytes: vec![0; 1024],
            });
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "submit must return immediately"
        );
    }
}
