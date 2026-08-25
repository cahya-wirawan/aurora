//! A dedicated background thread that writes evicted tiles to the
//! scratch disk, so eviction never blocks the caller (the M1.1
//! "background writer" deliverable).
//!
//! Deliberately **not** `tokio`: this is one thread draining one queue,
//! not general async I/O -- a plain `std::thread` + `std::sync::mpsc`
//! (stdlib, no new dependency) is the right-sized tool. Revisit only if
//! `aurora-tile` grows genuinely concurrent I/O needs beyond one writer.

use std::fs;
use std::path::PathBuf;
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
    /// queue, one `fs::write` at a time, in the order jobs were sent --
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
                let outcome = fs::write(&job.path, &job.bytes);
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
