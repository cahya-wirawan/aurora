//! ICC profiles, working spaces, soft proof, and HDR transforms.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`IccProfile`]/[`Transform`] are M1.5's first piece: ICC transforms via
//! ADR 0008's `lcms2` decision. [`IccProfile::to_bytes`] (added
//! 2026-08-06) is the inverse of [`IccProfile::from_bytes`] — real
//! profile *bytes* out, not just a value usable in memory — what
//! `.aur`'s own manifest (ADR 0009) needs to embed a real profile
//! instead of a bare colour-space tag. Colour-space *descriptors*
//! ([`aurora_core::ColorSpace`]/[`aurora_core::PixelFormat`]) already
//! exist from M1.1 — this crate is what actually interprets an ICC
//! profile and moves pixel data between colour spaces.
//! [`linear_to_srgb`]/[`srgb_to_linear`] are the second: the actual
//! transfer-function math, HDR/negative-value-preserving per §7.3.1b.
//! An explicit working-space *policy* type has no concrete consumer yet
//! (`aurora-filters`/`aurora-render`'s colour wiring don't exist), so
//! it's deliberately not designed speculatively — see `linear`'s own doc
//! comment. [`promote_u8`]/[`dither_quantize`] are the third: the 8-bit
//! import/export boundary invariant §7.3.1b names — this crate provides
//! the conversion math; `aurora-io` (still a skeleton) will call it once
//! a real image format reader/writer exists.
//!
//! This crate is M1.5's home; PLAN.md tracks what's done and what's
//! still open there.

mod cmyk;
mod dither;
mod error;
mod linear;
mod profile;
mod source;
mod transform;

pub use cmyk::{CmykSource, CmykTransform, inspect_cmyk_source, naive_cmyk_to_rgb};
pub use dither::{
    DITHER_SIZE, dither_quantize, dither_quantize_with, dither_thresholds, promote_u8, promote_u16,
    quantize_u8, quantize_u16,
};
pub use error::ColorError;
pub use linear::{linear_to_srgb, srgb_to_linear};
pub use profile::{IccProfile, icc_bytes_match_ignoring_stamps};
pub use source::{
    MAX_PROFILE_BYTES, ProfileHeader, ProfileProblem, ProfileSpace, RgbSource,
    SRGB_EQUIVALENCE_TOLERANCE, inspect_rgb_source, parse_header, profile_description,
};
pub use transform::{RenderingIntent, Transform};
