//! wgpu device management, shader library, and GPU tile residency.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it — [ADR 0001](../../../docs/adr/0001-custom-wgpu-ui.md)
//! and invariant §7.3.8 are why UI and canvas share one [`GpuContext`]
//! rather than each owning a separate device.
//!
//! Device/queue management, the shader library/pipeline cache, GPU tile
//! residency, budgeted upload scheduling, and surface configuration/resize
//! are all implemented — M1.2 is complete. The surface/resize path
//! ([`GpuSurface`]) is real but unverified against an actual window in
//! this environment; see its module doc.

mod context;
mod error;
mod pipeline;
#[cfg(test)]
mod render_test;
mod residency;
#[cfg(test)]
mod residency_test;
mod shader;
mod surface;
#[cfg(test)]
mod test_support;

pub use context::GpuContext;
pub use error::GpuError;
pub use pipeline::{Blend, PipelineCache, PipelineKey};
pub use residency::{SyncStats, TileResidency};
pub use shader::ShaderLibrary;
pub use surface::GpuSurface;
