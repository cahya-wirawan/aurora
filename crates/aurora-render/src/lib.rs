//! Executes the render graph on GPU or CPU, producing progressive tiled output.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`schedule`] translates an `aurora_graph::RenderGraph`'s node-granular
//! dirty regions into tile-granular work lists (PLAN.md M1.3).
//! [`TileCompositor`] executes the GPU half of that work: blending one
//! tile over another via the GPU's fixed-function alpha blend unit,
//! replacing the CPU per-pixel merge `spike/FINDINGS.md` finding #1 named
//! as the real compositing bottleneck. Progressive rendering (finding
//! #3's "render a lower-resolution mip while panning fast, refining when
//! motion stops") has two pieces so far: [`mip::downsample`] box-filters
//! a tile down to a [`mip::MipLevel`], and [`preview::upload_preview`]
//! lands the result in `aurora_gpu::TileResidency`'s atlas. Picking a
//! level from real interaction state, and async evaluation, are still
//! open.

mod composite;
mod mip;
mod preview;
mod schedule;
#[cfg(test)]
mod test_support;

pub use composite::TileCompositor;
pub use mip::{MipLevel, downsample};
pub use preview::{PreviewError, upload_preview};
pub use schedule::{ScheduledWork, schedule, tiles_for_rect};
