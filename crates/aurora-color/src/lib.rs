//! ICC profiles, working spaces, soft proof, and HDR transforms.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`IccProfile`]/[`Transform`] are M1.5's first piece: ICC transforms via
//! ADR 0008's `lcms2` decision. Colour-space *descriptors*
//! ([`aurora_core::ColorSpace`]/[`aurora_core::PixelFormat`]) already
//! exist from M1.1 — this crate is what actually interprets an ICC
//! profile and moves pixel data between colour spaces. Working spaces,
//! linear-light conversion, and promote-on-import/dither-on-export are
//! still open — see PLAN.md M1.5.

mod error;
mod profile;
mod transform;

pub use error::ColorError;
pub use profile::IccProfile;
pub use transform::{RenderingIntent, Transform};
