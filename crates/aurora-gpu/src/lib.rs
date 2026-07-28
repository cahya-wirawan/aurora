//! wgpu device management, shader library, and GPU tile residency.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it — [ADR 0001](../../../docs/adr/0001-custom-wgpu-ui.md)
//! and invariant §7.3.8 are why UI and canvas share one [`GpuContext`]
//! rather than each owning a separate device.
//!
//! Device/queue management and the shader library/pipeline cache are
//! implemented; surface configuration, resize, GPU tile residency, and
//! upload scheduling are the rest of PLAN.md M1.2, not yet started.

mod context;
mod error;
mod pipeline;
#[cfg(test)]
mod render_test;
mod shader;
#[cfg(test)]
mod test_support;

pub use context::GpuContext;
pub use error::GpuError;
pub use pipeline::{Blend, PipelineCache, PipelineKey};
pub use shader::ShaderLibrary;
