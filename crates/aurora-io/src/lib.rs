//! Format import and export, including full layered PSD/PSB read and
//! write (PRD FR-001) — see `docs/adr/0004-psd-full-write.md` for that
//! decision.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`png`] is this crate's first real format (PLAN.md M1.9): the
//! simplest one, and the one invariant §7.3.1b's own "8-bit only at
//! import/export" wording directly names. [`jpeg`] is the second, via
//! `zune-jpeg`/`jpeg-encoder` (PRD §8.2's own pure-Rust image-codec
//! pair). [`tiff`] is the third, via the `tiff` crate (`image-tiff`
//! upstream, the same image-rs family `png` belongs to) — one crate
//! covers both decode and encode, unlike JPEG's two-crate split.
//! [`Image`] is this crate's own, self-contained pixel representation
//! for all three. `channels` (private) holds the
//! grayscale/grayscale-with-alpha/RGB → RGBA expansion helpers `png`
//! and `tiff` both need. [`import`] is the real document-pixel-storage
//! wiring `Image`'s own doc comment used to name as open:
//! [`import::write_into_store`] writes a decoded `Image` into a
//! document layer's own `aurora_tile::TileStore` surface (ADR 0010),
//! and [`import::decode_by_extension`] picks `png`/`jpeg`/`tiff::decode`
//! by a file's own extension — the dispatcher a real "Open File" flow
//! needs. PSD/PSB remains open.

mod channels;
mod error;
mod image;
pub mod import;
pub mod jpeg;
pub mod png;
pub mod tiff;

pub use error::IoError;
pub use image::Image;
pub use import::{decode_by_extension, write_into_store};
