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
//! PLAN.md M1.7. [`shortcut`] (M1.8) rounds out keyboard input: a small,
//! platform-agnostic [`KeyChord`] vocabulary plus a chord ->
//! generic-command [`ShortcutRegistry`], and
//! [`widgets::insert_command_palette`] is the searchable-command-list
//! widget a shortcut typically opens; [`widgets::insert_dialog`] is a
//! generic modal alert/confirmation (the mechanism a crash-recovery
//! prompt is built from) — both keep the same "abstract steps, not
//! `winit` types" seam as `FocusManager`/`hit_test`.
//!
//! This crate knows nothing about documents or layers (`aurora-doc` is a
//! layer above it). Its *core* — layout, input routing, damage
//! tracking, accessibility ([`WidgetTree`]/[`FocusManager`]/
//! [`widgets`]) — must stay headlessly testable, and does: every test
//! for those pieces still runs with no window, no GPU, and no platform
//! accessibility backend, `tests/headless.rs` is still the permanent,
//! end-to-end proof (a small multi-widget form, laid out, focus/pointer-
//! routed, every widget mutated including an IME composition, a full
//! `accesskit::TreeUpdate` inspected — all through this crate's public
//! API, nothing platform-specific anywhere in that call graph).
//!
//! [`render`] (added 2026-08-06) is the one real exception, by design:
//! the GPU path renderer PRD §8 names ("vector rasterization: `lyon`
//! (tessellation) + custom GPU path renderer"), the still-open
//! "vector-first rendering" M1.7 bullet's own next real step. This
//! crate is where it has to live — the one crate depending on both
//! `aurora-vector` and `aurora-gpu` (`scripts/layering.json`) —
//! and `wgpu` is now a real, direct part of this crate's own dependency
//! graph because of it, not just an allowance nothing used yet. Its own
//! tests skip (not fail) with no real GPU adapter present, the same
//! "headless CI still runs everything, real hardware exercises the
//! rest" shape every other `wgpu`-touching crate in this workspace
//! already uses — not a new exception, the same rule applied here for
//! the first time.
//!
//! [`paint_widget`] (added 2026-08-06) is the piece `render`'s own doc
//! comment named as still open: the first real widget-to-`Mesh` path,
//! covering `Button`'s own solid rounded-rect background, coloured from
//! a real, resolved `aurora_theme::Theme` (invariant §7.3.10) rather
//! than a literal. Every other widget still paints nothing — see
//! [`paint_widget`]'s own doc comment for exactly which and why. The
//! golden-image component gallery this unblocks is still separate,
//! still-open follow-on work.

mod error;
mod input;
mod paint;
pub mod render;
#[cfg(test)]
mod render_test;
pub mod shortcut;
mod test_support;
mod tree;
pub mod widgets;

pub use error::WidgetError;
pub use input::{FocusManager, hit_test};
pub use paint::{Paint, paint_widget};
pub use render::{GpuMesh, PathPipeline};
pub use shortcut::{KeyChord, Modifiers as ShortcutModifiers, ShortcutRegistry};
pub use tree::{WidgetId, WidgetTree};
