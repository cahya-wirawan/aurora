//! Shaping, layout, OpenType handling, and text on path.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! # What is real (0.132.0)
//!
//! Single-line UI text, CPU-side and headless:
//!
//! - [`font`] — the one bundled UI face, Inter 4.1 Regular (SIL OFL 1.1,
//!   `fonts/OFL.txt`). It is a *provisional stand-in*: the design token
//!   `type.family` is still `"TBD"` and choosing the real family is the
//!   design owner's decision, not this crate's.
//! - [`TextEngine`] — shapes a string with `harfrust` (the `HarfBuzz` port
//!   `cosmic-text` itself uses) at *physical* pixel size, measures it,
//!   answers caret positions at grapheme boundaries, and rasterizes glyphs
//!   to 8-bit coverage masks with `swash`. Shaped lines are cached per
//!   frame ([`TextEngine::begin_frame`]).
//! - [`ShelfPacker`] — the rectangle packer a glyph atlas uses. The atlas
//!   texture itself lives in `aurora-widgets`, next to the path renderer,
//!   because this crate may not depend on `aurora-gpu` (`scripts/
//!   layering.json`).
//!
//! # What is not
//!
//! No font fallback (a code point Inter lacks — CJK, emoji — shapes to the
//! `.notdef` box), no colour glyphs, no bold/italic face (a style's
//! `weight` is carried and cached on but only Regular is bundled), no
//! right-to-left or bidirectional layout (every run is shaped
//! left-to-right), no wrapping, no ellipsis, no text on path.
//!
//! # Why not `cosmic-text` directly
//!
//! `cosmic-text` requires `fontdb`, and every `fontdb` release pulls in
//! `ttf-parser` (RUSTSEC-2026-0192, unmaintained). Its shaper (`harfrust`)
//! and rasterizer (`swash`) are both `skrifa`-based and carry no such
//! advisory, and one bundled font needs no font database — so this crate
//! uses those two directly.

mod engine;
pub mod font;
mod packer;

pub use engine::{
    GlyphKey, GlyphMask, PlacedGlyph, ShapedLine, SubpixelBin, TextEngine, TextError, TextMetrics,
    TextStyle, snap_glyph_origin,
};
pub use packer::ShelfPacker;
