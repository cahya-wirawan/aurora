//! Brush engine: stroke input, stabilization, and dab scheduling.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`dab::dabs_along_path`]/[`dab::advance_segment`],
//! PLAN.md M1.9's "basic brush and eraser" bullet's first slice —
//! turning a stroke path (or, for a live pointer drag, one segment at a
//! time) into dab positions. [`stamp::stamp_dab`]/[`stamp::stamp_stroke`]
//! are the second slice: actually stamping those dabs into real pixels,
//! now that [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)
//! gives pixel storage a real, addressable shape
//! (`aurora_tile::TileStore`, addressed by `SurfaceId`).
//! [`stamp::erase_dab`]/[`stamp::erase_stroke`] are the eraser half of
//! that same bullet — the same dab geometry, subtractive (multiplying
//! existing alpha toward zero) instead of blended. `aurora-app`'s own
//! `Tool::Brush`/`Tool::Eraser` wiring (a live document, a live tile
//! store, and a real pointer-drag-to-paint/erase dispatch) is the third
//! slice, closing the loop from a real mouse drag to real pixels. See
//! each module's own doc comment for exactly what's real and what's
//! still open (undo-as-you-drag remains separate, still-open follow-on
//! work).

mod dab;
mod stamp;

pub use dab::{DEFAULT_SPACING, advance_segment, dab_step, dabs_along_path};
pub use stamp::{erase_dab, erase_stroke, stamp_dab, stamp_stroke};
