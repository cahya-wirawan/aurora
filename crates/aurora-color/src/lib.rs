//! ICC profiles, working spaces, soft proof, and HDR transforms.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`IccProfile`]/[`Transform`] are M1.5's first piece: ICC transforms via
//! ADR 0008's `lcms2` decision. Colour-space *descriptors*
//! ([`aurora_core::ColorSpace`]/[`aurora_core::PixelFormat`]) already
//! exist from M1.1 — this crate is what actually interprets an ICC
//! profile and moves pixel data between colour spaces.
//! [`linear_to_srgb`]/[`srgb_to_linear`] are the second: the actual
//! transfer-function math, HDR/negative-value-preserving per §7.3.1b.
//! An explicit working-space *policy* type has no concrete consumer yet
//! (`aurora-filters`/`aurora-render`'s colour wiring don't exist), so
//! it's deliberately not designed speculatively — see `linear`'s own doc
//! comment. Promote-on-import/dither-on-export are still open — see
//! PLAN.md M1.5.

mod error;
mod linear;
mod profile;
mod transform;

pub use error::ColorError;
pub use linear::{linear_to_srgb, srgb_to_linear};
pub use profile::IccProfile;
pub use transform::{RenderingIntent, Transform};
