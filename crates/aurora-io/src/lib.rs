//! Format import and export, including full layered PSD/PSB read and
//! write (PRD FR-001) — see `docs/adr/0004-psd-full-write.md` for that
//! decision.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`png`] is this crate's first real format (PLAN.md M1.9): the
//! simplest one, and the one invariant §7.3.1b's own "8-bit only at
//! import/export" wording directly names. [`Image`] is this crate's
//! own, self-contained pixel representation for it — see that type's
//! own doc comment for why it's deliberately not wired into
//! `aurora_doc::LayerTree`/`aurora_tile::TileStore` yet. PSD/PSB, JPEG,
//! TIFF, and the real document-pixel-storage wiring all remain open.

mod error;
mod image;
pub mod png;

pub use error::IoError;
pub use image::Image;
