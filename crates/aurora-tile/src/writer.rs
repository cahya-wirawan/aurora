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
    pub(crate) path: PathBuf,
    pub(crate) bytes: Vec<u8>,
}

/// Result of a background write, reported back so [`crate::store::TileStore`]
/// can track failures without blocking on them.
pub(crate) struct WriteResult {
    pub(crate) surface: SurfaceId,
    pub(crate) id: TileId,
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
