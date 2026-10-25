//! wgpu device management, shader library, and GPU tile residency.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it — [ADR 0001](../../../docs/adr/0001-custom-wgpu-ui.md)
//! and invariant §7.3.8 are why UI and canvas share one [`GpuContext`]
//! rather than each owning a separate device.
//!
//! Device/queue management, the shader library/pipeline cache, GPU tile
//! residency, budgeted upload scheduling, and surface configuration/resize
//! are all implemented and verified against real GPU hardware — the
//! surface/resize path ([`GpuSurface`]) against a real window, on a live
//! macOS desktop session (`examples/surface_smoke.rs`); see its module doc.
//! Every module has now run against real hardware on Vulkan (Linux) and
//! Metal (macOS); only DX12 (Windows) remains unvalidated — see `PLAN.md`
//! M1.2.
//!
//! [`CanvasPipeline`] draws a [`TileResidency`]'s own atlas as a real,
//! on-screen fullscreen triangle (`spike/vertical-slice`'s own proven
//! `vs_canvas`/`fs_canvas` shader) — the bind-group-layout/pipeline
//! logic this crate's own tests had already been exercising privately
//! (`render_test.rs`) is now real, public production API, for
//! `aurora-app`'s own canvas rendering (PLAN.md M1.8) to use.

mod canvas;
pub mod capture;
mod context;
mod display;
#[cfg(test)]
mod display_gpu_test;
mod error;
mod pipeline;
#[cfg(test)]
mod render_test;
mod residency;
#[cfg(test)]
mod residency_test;
mod shader;
mod surface;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use canvas::{CHECKERBOARD_SHADES, CanvasPipeline};
pub use context::GpuContext;
pub use display::{
    DISCRETE_TEXEL_CAP, DisplayBudget, DisplayTexture, LEVEL0_LOD_CLAMP, MAX_DISPLAY_MIP_LEVELS,
    MIN_TRILINEAR_LOD, PageLayout, SHARED_MEMORY_DIVISOR, display_budget, display_bytes,
    display_lod, doc_tiles, lod_for_zoom, mip_levels_for, mip_rects, page_scissors, page_uniform,
    plan_pages, texture_bytes,
};
pub use error::GpuError;
pub use pipeline::{Blend, PipelineCache, PipelineKey};
pub use residency::{SyncStats, TileResidency};
pub use shader::ShaderLibrary;
pub use surface::{GpuSurface, choose_surface_format};
