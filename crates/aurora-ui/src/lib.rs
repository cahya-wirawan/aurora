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
//!
//! [`canvas_view::CanvasView`] and [`tool`] are PLAN.md M1.9's "basic
//! tools" bullet: the pan/zoom coordinate transform and the tool
//! dispatch logic (Move, Marquee Select, Zoom, Pan, Eyedropper) a real
//! pointer-driven canvas needs — see each module's own doc comment for
//! what's real today and what's still honestly open (Move and Eyedropper
//! in particular).

pub mod canvas_view;
pub mod history_panel;
pub mod layers_panel;
pub mod panel;
pub mod tool;
pub mod workspace;

pub use canvas_view::CanvasView;
pub use history_panel::populate_history_panel;
pub use layers_panel::populate_layers_panel;
pub use panel::{PanelHandle, insert_panel};
pub use tool::Tool;
pub use workspace::{Workspace, build_workspace};
