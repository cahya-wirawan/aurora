//! Document-agnostic widget toolkit: layout, input routing, damage tracking, accessibility.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! [`WidgetTree`] is M1.7's first piece: identity, nesting, damage
//! tracking, a `taffy`-backed layout engine, and a required
//! [`accesskit::Node`] on every widget from the moment it's created
//! (invariant §7.3.9 — accessibility "as part of its definition, not a
//! pass"). [`FocusManager`]/[`hit_test`] are the second: pointer
//! hit-testing and keyboard focus/`Tab` navigation, deliberately in terms
//! of a document-space point and abstract focus steps rather than
//! `winit::WindowEvent`s — translating real platform input into these is
//! `aurora-app`'s job. IME, the concrete widget set, vector-first
//! rendering, the component gallery, and headless test mode as an
//! explicit feature are all still open — see PLAN.md M1.7.
//!
//! This crate knows nothing about documents or layers (`aurora-doc` is a
//! layer above it) and must stay headlessly testable — every test in
//! this crate runs with no window, no GPU, and no platform accessibility
//! backend, which is exactly what "headless mode for automated UI tests"
//! (a later M1.7 bullet) is really asking this crate to already be, not
//! a separate mode bolted on afterward.

mod error;
mod input;
mod tree;

pub use error::WidgetError;
pub use input::{FocusManager, hit_test};
pub use tree::{WidgetId, WidgetTree};
