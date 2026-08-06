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
//! slice, closing the loop from a real mouse drag to real pixels.
//! [`undo::StrokeSnapshot`]/[`undo::PixelHistory`] are the fourth: a
//! completed stroke's own undo, since raw pixel edits have no
//! `aurora_doc::LayerOp` equivalent for `aurora_doc::History` to record
//! (a stroke has no compact, replayable description the way a layer
//! property change does — it *is* its own pixel diff, so undoing one
//! means capturing and restoring the tiles it touched, invariant
//! §7.3.3's own "dirtied tiles" applied to pixel data instead of a
//! layer's own scalar properties). [`stamp::touched_tiles`] is what lets
//! a caller know which tiles to snapshot *before* a dab changes them.
//! See each module's own doc comment for exactly what's real and what's
//! still open.

mod dab;
mod stamp;
mod undo;

pub use dab::{DEFAULT_SPACING, advance_segment, dab_step, dabs_along_path};
pub use stamp::{erase_dab, erase_stroke, stamp_dab, stamp_stroke, touched_tiles};
pub use undo::{PixelHistory, StrokeSnapshot};
