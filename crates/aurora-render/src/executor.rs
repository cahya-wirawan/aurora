//! Async evaluation: the UI thread never blocks on rendering (§7.3.4).
//! A dedicated background thread runs submitted work; [`Executor::submit`]
//! never blocks (unbounded channel) and [`Executor::drain_completed`]
//! never blocks either (reports whatever has finished so far). PLAN.md
//! M1.3.
//!
//! Same shape as `aurora_tile::writer::BackgroundWriter`, which already
//! proved this exact pattern for scratch-disk writes — generalized here
//! to arbitrary render work. Deliberately **not** `tokio`: one thread
//! draining one queue doesn't need an async runtime (`tokio` is for I/O
//! and background work, *not* the render loop — CLAUDE.md).
//!
//! What this module does not do: define what "evaluating a node" means.
//! Unlike `aurora_graph::RenderGraph<N>`, which is generic over a
//! caller-supplied node payload, this executor runs plain closures — a
//! render task's real shape needs `aurora-doc`/`aurora-filters` to define
//! what a node computes, and neither exists yet. A closure is the
//! honest, unopinionated primitive until a real caller needs more
//! structure.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

type Task = Box<dyn FnOnce() + Send + 'static>;

/// Identifies one submitted task, returned by [`Executor::submit`] so a
/// caller can match a [`Executor::drain_completed`] result back to the
/// work it submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskId(u64);

/// Runs submitted closures on a dedicated background thread.
///
/// **Known limitation, accepted rather than solved here**: if a
/// submitted task panics, the background thread unwinds and its `recv`
/// loop ends — every task submitted afterward is silently dropped (the
/// same failure shape `BackgroundWriter` already has, and for the same
/// reason: this workspace denies `panic`/`unwrap`/`expect` everywhere, so
/// a task panicking would itself be a bug in caller code, not a
/// condition this executor is designed to route around).
pub struct Executor {
    tx: Option<Sender<(TaskId, Task)>>,
    handle: Option<JoinHandle<()>>,
    completed_rx: mpsc::Receiver<TaskId>,
    next_id: u64,
}

impl Executor {
    /// Spawns the background thread.
    #[must_use]
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<(TaskId, Task)>();
        let (completed_tx, completed_rx) = mpsc::channel::<TaskId>();
        let handle = std::thread::spawn(move || {
            while let Ok((id, task)) = rx.recv() {
                task();
                // The caller may already be gone (e.g. process shutdown
                // mid-task) -- a dropped completion receiver is not this
                // thread's problem to report anywhere, same reasoning
                // `BackgroundWriter`'s own send already uses.
                let _ = completed_tx.send(id);
            }
        });
        Self {
            tx: Some(tx),
            handle: Some(handle),
            completed_rx,
            next_id: 0,
        }
    }

    /// Never blocks: this is what makes rendering work non-blocking for
    /// whichever thread calls it. `task` runs on the background thread,
    /// not the caller's.
    pub fn submit(&mut self, task: impl FnOnce() + Send + 'static) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        if let Some(tx) = &self.tx {
            // If the background thread already panicked, there is
            // nothing a caller can usefully do about a dropped receiver
            // at this point -- the failure surfaces as this task never
            // appearing in `drain_completed`, not as a panic here.
            let _ = tx.send((id, Box::new(task)));
        }
        id
    }

    /// Non-blocking: returns the ids of whichever submitted tasks have
    /// finished so far, without waiting for more.
    #[must_use]
    pub fn drain_completed(&self) -> Vec<TaskId> {
        self.completed_rx.try_iter().collect()
    }

    /// Blocks until every already-submitted task has actually run. For
    /// shutdown and tests that need a deterministic "has it run yet"
    /// point — not something the render loop itself should call, or it
    /// defeats the entire point of this type.
    pub fn join(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        self.join();
    }
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Executor;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn submitted_tasks_actually_run() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut executor = Executor::spawn();
        let task_counter = Arc::clone(&counter);
        executor.submit(move || {
            task_counter.fetch_add(1, Ordering::SeqCst);
        });
        // join() blocks until the task has actually run, so this is a
        // deterministic check -- no sleep-and-hope polling.
        executor.join();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drain_completed_reports_a_finished_task_by_id() {
        let mut executor = Executor::spawn();
        let id = executor.submit(|| {});
        executor.join();
        assert_eq!(executor.drain_completed(), vec![id]);
    }

    #[test]
    fn drain_completed_is_empty_with_nothing_submitted() {
        let executor = Executor::spawn();
        assert!(executor.drain_completed().is_empty());
    }

    #[test]
    fn task_ids_are_distinct() {
        let mut executor = Executor::spawn();
        let a = executor.submit(|| {});
        let b = executor.submit(|| {});
        let c = executor.submit(|| {});
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn submit_never_blocks_even_before_the_executor_drains() {
        let mut executor = Executor::spawn();
        let start = Instant::now();
        for _ in 0..100 {
            executor.submit(|| {
                std::thread::sleep(Duration::from_millis(1));
            });
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "submit must return immediately, not wait for tasks to run"
        );
    }
}
