//! Paths, boolean operations, stroking, and tessellation.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! **First real slice, 2026-08-06** (PLAN.md M1.7's "Vector-first
//! rendering" bullet, itself a prerequisite the M1.7 "Component gallery
//! and golden-image tests" bullet was blocked on — nothing could render
//! a widget to real pixels to diff against a golden image without this).
//! [`PathBuilder`]/[`Path`] build a resolution-independent path from
//! straight lines and quadratic/cubic Bézier segments; [`fill`]/
//! [`stroke`] tessellate one into a [`Mesh`] (flat triangle vertices +
//! indices) via `lyon` — PRD §8's own pre-decided "vector rasterization:
//! `lyon` (tessellation) + custom GPU path renderer." [`rounded_rect`]
//! is the first real shape built on top: the single most common UI
//! primitive (every button/panel/field this project's own design
//! language uses one), via the same cubic-Bézier corner approximation
//! every mainstream 2D API uses.
//!
//! **Scope, stated honestly.** This crate stays GPU-agnostic on
//! purpose — it depends on `aurora-core` only
//! (`scripts/layering.json`), not `aurora-gpu`, so [`Mesh`] is plain
//! CPU-side data (`Vec<Point>`/`Vec<u32>`); the "custom GPU path
//! renderer" half of PRD §8's own decision — actually uploading a
//! `Mesh` and drawing it via `wgpu` — belongs in a crate that depends
//! on both this one and `aurora-gpu` (`aurora-widgets`, per
//! `scripts/layering.json`), and doesn't exist yet. Neither does a real
//! widget-to-`Mesh` painting path (every widget still produces layout +
//! accessibility content only, per `aurora-widgets`' own doc comment),
//! or the golden-image gallery tests this slice exists to eventually
//! unblock. Boolean operations (union/intersection/difference — the
//! other half of this crate's own name) are a substantially harder,
//! separate computational-geometry problem (`lyon` itself doesn't
//! include them) and are not attempted here at all yet. Tessellation
//! tolerance is a fixed default ([`DEFAULT_TOLERANCE`]), not yet scaled
//! by DPI/zoom — a real, separate follow-on once a real caller needs
//! that tradeoff made for real.

mod error;
mod mesh;
mod path;
mod point;
mod shapes;

pub use error::VectorError;
pub use mesh::{DEFAULT_TOLERANCE, Mesh, fill, stroke};
pub use path::{Path, PathBuilder};
pub use point::Point;
pub use shapes::rounded_rect;
