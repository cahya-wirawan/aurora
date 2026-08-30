//! Geometry, colour types, pixel formats, error types, and IDs shared by every crate.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it. `aurora-core` depends on
//! nothing else in the workspace (`scripts/layering.json`) — it is the
//! foundation every other crate builds on, so it stays free of anything
//! crate-specific (no pixel storage, no tile concept, no document model).

pub mod color;
pub mod error;
pub mod geometry;
pub mod id;

pub use color::{Channels, ColorSpace, PixelFormat, SampleFormat};
pub use error::CoreError;
pub use geometry::{MAX_DOCUMENT_EXTENT, MAX_DOCUMENT_ORIGIN, Rect, Size};
pub use id::{Id, IdGenerator};
