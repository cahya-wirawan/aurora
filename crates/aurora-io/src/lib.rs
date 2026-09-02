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
//! wiring `Image`'s own doc comment used to name as open, both
//! directions: [`import::write_into_store`]/[`import::read_from_store`]
//! write/read a document layer's own `aurora_tile::TileStore` surface
//! (ADR 0010), and [`import::decode_by_extension`]/
//! [`import::encode_by_extension`] pick `png`/`jpeg`/`tiff` by a file's
//! own extension — the dispatchers a real "Open File"/"Save/Export"
//! flow needs. [`aur`] is the real document format ([ADR
//! 0009](../../../docs/adr/0009-aur-document-format.md)): a full
//! `aurora_doc::LayerTree`/`History` plus every pixel layer's own
//! tiles, round-tripped through a `.aur` file — the answer to what
//! `png`/`jpeg`/`tiff` structurally can't do (more than one flat
//! image). PSD/PSB remains open.

pub mod aur;
mod channels;
mod error;
mod image;
pub mod import;
pub mod jpeg;
pub mod png;
pub mod tiff;

pub use aur::{
    AurDocument, SkippedTile, SkippedTileRecord, SkippedTiles, read as read_aur,
    write as write_aur, write_best_effort as write_aur_best_effort,
};
pub use error::IoError;
pub use image::Image;
pub use import::{decode_by_extension, encode_by_extension, read_from_store, write_into_store};
