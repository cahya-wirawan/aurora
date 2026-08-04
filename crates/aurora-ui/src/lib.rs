//! Aurora-specific panels, docking, workspace, tools, and command palette.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`panel::insert_panel`] and
//! [`workspace::build_workspace`], a static first slice of PLAN.md
//! M1.8's "docking, panels, custom workspaces" bullet, matching the
//! structure of the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). See both modules' own doc
//! comments for exactly what's built and what's deliberately still
//! open (drag-to-redock, resize, persisted layouts, the menubar/
//! toolbar/status bar). [`layers_panel::populate_layers_panel`] and
//! [`history_panel::populate_history_panel`] are the "Layers, history,
//! tool-options panels" bullet's own first slice — real content from a
//! real `aurora_doc::LayerTree`/`History`; tool-options panels remain
//! separate, still open.

pub mod history_panel;
pub mod layers_panel;
pub mod panel;
pub mod workspace;

pub use history_panel::populate_history_panel;
pub use layers_panel::populate_layers_panel;
pub use panel::{PanelHandle, insert_panel};
pub use workspace::{Workspace, build_workspace};
