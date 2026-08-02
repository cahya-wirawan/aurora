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
//! `aurora-app`'s job. [`widgets`] is the third: a first slice of the
//! concrete widget set (`Button`/`Checkbox`/`Slider`/`TextField`, the
//! last with full IME composition support) — see that module's own doc
//! comment for exactly what's covered and what's deliberately not.
//! Vector-first rendering and the component gallery are still open — see
//! PLAN.md M1.7.
//!
//! This crate knows nothing about documents or layers (`aurora-doc` is a
//! layer above it) and must stay headlessly testable — every test in
//! this crate runs with no window, no GPU, and no platform accessibility
//! backend, and this crate's own `Cargo.toml` has no `wgpu`/`winit`
//! anywhere in its dependency graph (`aurora-gpu`/`aurora-vector`/
//! `aurora-text` are allowed by `scripts/layering.json` for when
//! vector-first rendering starts, but aren't depended on until then) —
//! a fact `cargo tree -p aurora-widgets -i wgpu` can check directly, not
//! just an inference from what the code happens not to call.
//! `tests/headless.rs` is the permanent, end-to-end proof: it builds a
//! small multi-widget form, lays it out, routes focus and pointer input,
//! mutates every widget (including an IME composition), and inspects a
//! full `accesskit::TreeUpdate` — all through this crate's public API,
//! with nothing platform-specific anywhere in the call graph. This is
//! what "headless mode for automated UI tests" (M1.7's own bullet) was
//! asking this crate to already be, made explicit and checked rather
//! than left as a hopeful property.

mod error;
mod input;
mod tree;
pub mod widgets;

pub use error::WidgetError;
pub use input::{FocusManager, hit_test};
pub use tree::{WidgetId, WidgetTree};
