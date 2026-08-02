//! Document-agnostic widget toolkit: layout, input routing, damage tracking, accessibility.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`WidgetTree`] is M1.7's first piece: identity, nesting, damage
//! tracking, and a required [`accesskit::Node`] on every widget from the
//! moment it's created (invariant §7.3.9 — accessibility "as part of its
//! definition, not a pass"). Layout, input/focus routing, IME, the
//! concrete widget set, vector-first rendering, the component gallery,
//! and headless test mode are all still open — see PLAN.md M1.7.
//!
//! This crate knows nothing about documents or layers (`aurora-doc` is a
//! layer above it) and must stay headlessly testable — every test in
//! this crate runs with no window, no GPU, and no platform accessibility
//! backend, which is exactly what "headless mode for automated UI tests"
//! (a later M1.7 bullet) is really asking this crate to already be, not
//! a separate mode bolted on afterward.

mod error;
mod tree;

pub use error::WidgetError;
pub use tree::{WidgetId, WidgetTree};
