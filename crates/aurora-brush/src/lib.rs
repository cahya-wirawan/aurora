//! Brush engine: stroke input, stabilization, and dab scheduling.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`dab::dabs_along_path`], PLAN.md M1.9's "basic
//! brush and eraser" bullet's first slice — see that module's own doc
//! comment for exactly what's real (dab spacing along a stroke path) and
//! what's still, genuinely, blocked (writing an actual pixel, which
//! needs a per-layer pixel storage decision this workspace hasn't made
//! yet).

mod dab;

pub use dab::{DEFAULT_SPACING, dabs_along_path};
