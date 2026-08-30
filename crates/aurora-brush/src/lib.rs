//! Brush engine: stroke input, stabilization, and dab scheduling.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`dab::dabs_along_path`]/[`dab::advance_segment`],
//! PLAN.md M1.9's "basic brush and eraser" bullet's first slice —
//! turning a stroke path (or, for a live pointer drag, one segment at a
//! time) into dab positions. [`stamp::stamp_dab`] is the second slice:
//! actually stamping those dabs into real pixels, now that
//! [ADR 0010](../../../docs/adr/0010-layer-pixel-storage.md)
//! gives pixel storage a real, addressable shape
//! (`aurora_tile::TileStore`, addressed by `SurfaceId`).
//! [`stamp::erase_dab`] is the eraser half of
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
//! layer's own scalar properties). [`stamp::touched_tiles`] is the pure
//! bounding-box geometry of which tiles a dab is aimed at; since 0.55.0
//! it is no longer how that undo snapshot is captured, because a dab
//! that fails to page a tile in — or that acquires one and then changes
//! nothing in it (0.56.0) — must not leave an undo entry claiming it.
//! [`stamp::stamp_dab`]/[`stamp::erase_dab`] capture each tile
//! themselves, in the instant before they first write to it
//! ([`undo::StrokeSnapshot::record_content`]), and report what they
//! actually managed to do through [`stamp::DabOutcome`].
//!
//! There is deliberately no whole-stroke `stamp_stroke`/`erase_stroke`
//! entry point any more (removed in 0.56.0). Both existed, both had
//! only test callers, and both kept a `Result<usize, TileError>` shape
//! meaning "the first broken tile abandons the rest of the stroke" —
//! the coarse-grained form of exactly the bug 0.55.0 fixed one level
//! down. A caller wanting a whole stroke composes
//! [`dab::dabs_along_path`] with [`stamp::stamp_dab`] and decides for
//! itself what a partial failure means, which is what `aurora-app`
//! already did.
//!
//! See each module's own doc comment for exactly what's real and what's
//! still open.

mod dab;
mod stamp;
#[cfg(test)]
mod test_support;
mod undo;

pub use dab::{DEFAULT_SPACING, advance_segment, dab_step, dabs_along_path};
pub use stamp::{DabOutcome, erase_dab, stamp_dab, touched_tiles};
pub use undo::{PixelHistory, StrokeSnapshot};
