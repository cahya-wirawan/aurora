//! Shared "get a real device or skip" pattern for this crate's real-GPU
//! tests — same rationale as `aurora_gpu::test_support` (real hardware
//! verification, not a mock, matching this project's general practice),
//! duplicated locally since that helper is private to `aurora-gpu`.

#![cfg(test)]

use aurora_gpu::{GpuContext, GpuError};
use std::sync::{Mutex, MutexGuard};

/// Serializes this crate's real-GPU tests. `aurora-gpu` found a real
/// cross-test deadlock under `cargo test`'s default multi-threaded runner
/// when multiple tests each create their own `wgpu::Instance`/`Device`
/// concurrently (`aurora_gpu::test_support`'s own doc comment has the
/// full story). This crate compiles to a separate test binary, so
/// `aurora-gpu`'s lock doesn't cover it — same protection, applied
/// independently rather than assumed inherited.
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
/// runner); any other failure is a real bug and panics — matching
/// `aurora_gpu::test_support::real_context`'s own reasoning exactly.
pub(crate) fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match GpuContext::new() {
        Ok(context) => Some(GpuTestContext {
            _guard: guard,
            context,
        }),
        Err(GpuError::NoSuitableAdapter) => {
            eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
            None
        }
        Err(err) => {
            #[allow(clippy::panic)]
            {
                panic!("device request failed with a real adapter present: {err}");
            }
        }
    }
}
