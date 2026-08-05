//! Brush engine: stroke input, stabilization, and dab scheduling.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`dab::dabs_along_path`], PLAN.md M1.9's "basic
//! brush and eraser" bullet's first slice — turning a stroke path into
//! dab positions. [`stamp::stamp_dab`]/[`stamp::stamp_stroke`] are the
//! second slice: actually stamping those dabs into real pixels, now
//! that [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md) gives
//! pixel storage a real, addressable shape (`aurora_tile::TileStore`,
//! addressed by `SurfaceId`). See each module's own doc comment for
//! exactly what's real and what's still open (eraser, undo-as-you-drag,
//! and wiring a live Brush/Eraser tool into `aurora-app`'s pointer input
//! are all separate, still-open follow-on work).

mod dab;
mod stamp;

pub use dab::{DEFAULT_SPACING, dabs_along_path};
pub use stamp::{stamp_dab, stamp_stroke};
