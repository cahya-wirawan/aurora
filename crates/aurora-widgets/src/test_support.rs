//! Shared "get a real device or skip" pattern for this crate's real-GPU
//! tests (`render.rs`) — same rationale as `aurora_gpu::test_support`
//! (real hardware verification, not a mock, matching this project's
//! general practice).
//!
//! The skip-vs-fail *decision* is not duplicated here: it lives once in
//! `aurora_gpu::test_support::real_context_or_skip`, reached through
//! `aurora-gpu`'s `test-support` feature (enabled from this crate's
//! `[dev-dependencies]`). What stays local is only the crate-local
//! [`GPU_TEST_LOCK`] and the wrapper that bundles its guard with the
//! context — this crate compiles to its own test binary, so a lock in
//! another crate would not cover it. `tests/gallery.rs` is a third,
//! separate binary and carries its own pair for the same reason.

#![cfg(test)]

use aurora_gpu::GpuContext;
use std::sync::{Mutex, MutexGuard};

/// Serializes this crate's real-GPU tests. `aurora-gpu` found a real
/// cross-test deadlock under `cargo test`'s default multi-threaded
/// runner when multiple tests each create their own
/// `wgpu::Instance`/`Device` concurrently (`aurora_gpu::test_support`'s
/// own doc comment has the full story). This crate compiles to a
/// separate test binary, so `aurora-gpu`'s own lock doesn't cover it —
/// same protection, applied independently rather than assumed
/// inherited.
static GPU_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct GpuTestContext {
    _guard: MutexGuard<'static, ()>,
    context: GpuContext,
}

impl std::ops::Deref for GpuTestContext {
    type Target = GpuContext;
    fn deref(&self) -> &GpuContext {
        &self.context
    }
}

/// `None` is an inconclusive skip (no GPU adapter on this machine/CI
/// runner, and `AURORA_REQUIRE_GPU` unset); any other failure — and a
/// missing adapter with that variable set — is a real bug and panics.
/// The decision itself lives in
/// `aurora_gpu::test_support::real_context_or_skip`, which this crate
/// reaches via its `test-support` dev-dependency feature; only the lock
/// and the guard-bundling wrapper are local, since this crate compiles
/// to its own test binary.
pub(crate) fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    aurora_gpu::test_support::real_context_or_skip().map(|context| GpuTestContext {
        _guard: guard,
        context,
    })
}
