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
//! as the real compositing bottleneck. [`composite_tile_cpu`] is its
//! CPU-side sibling for *multi*-layer compositing: straight-alpha
//! "source over" generalized by a real per-layer [`BlendMode`] — `Normal`
//! plus the 8-mode "simple separable" family (`Darken`/`Multiply`/
//! `Lighten`/`Screen`/`Difference`/`Exclusion`/`Subtract`/`Divide`); the
//! ~18 remaining `aurora_doc::BlendMode` variants (dodge/burn,
//! overlay/light family, non-separable Hue/Saturation/Color/Luminosity,
//! Dissolve, DarkerColor/LighterColor) are separate, still-open follow-on
//! work — run entirely on the CPU because the orchestration crate
//! (`aurora-app`) needs to walk a real `aurora_doc::LayerTree` — a sibling
//! crate this one can't depend on — per visible tile, every frame a layer
//! changes, translating that real document blend mode into this crate's
//! own narrower [`BlendMode`] at the boundary; a real GPU-accelerated
//! version is separate, still-open follow-on work (PLAN.md M1.9).
//!
//! Progressive rendering (finding
//! #3's "render a lower-resolution mip while panning fast, refining when
//! motion stops") has two pieces: [`mip::downsample`] box-filters a tile
//! down to a [`mip::MipLevel`], and [`preview::upload_preview`] lands the
//! result in `aurora_gpu::TileResidency`'s atlas — picking a level from
//! real interaction state is still open. [`Executor`] is async
//! evaluation's first piece (§7.3.4: the UI thread never blocks on
//! rendering): a background thread that runs submitted work without
//! blocking the caller.

mod composite;
mod executor;
#[cfg(test)]
mod latency;
mod mip;
mod preview;
mod schedule;
#[cfg(test)]
mod test_support;

pub use composite::{BlendMode, TileCompositor, composite_tile_cpu};
pub use executor::{Executor, TaskId};
pub use mip::{MipLevel, downsample};
pub use preview::{PreviewError, upload_preview};
pub use schedule::{ScheduledWork, schedule, tiles_for_rect};
