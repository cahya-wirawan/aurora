//! wgpu device management, shader library, and GPU tile residency.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it — [ADR 0001](../../../docs/adr/0001-custom-wgpu-ui.md)
//! and invariant §7.3.8 are why UI and canvas share one [`GpuContext`]
//! rather than each owning a separate device.
//!
//! Device/queue management is implemented; surface configuration, resize,
//! the shader library, GPU tile residency, and upload scheduling are the
//! rest of PLAN.md M1.2, not yet started.

mod context;
mod error;

pub use context::GpuContext;
pub use error::GpuError;
