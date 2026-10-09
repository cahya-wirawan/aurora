//! Aurora-specific panels, docking, workspace, tools, and command palette.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it.
//!
//! First real code: [`panel::insert_panel`] and
//! [`workspace::build_workspace`], a first slice of PLAN.md M1.8's
//! "docking, panels, custom workspaces" bullet, matching the structure
//! of the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). [`panel::set_panel_collapsed`],
//! [`panel::close_panel`], and [`workspace::set_rail_width`] are the
//! real interactivity landed so far. See each module's own doc comment
//! for exactly what's built and what's deliberately still open
//! (drag-to-redock, floating, persisted layouts, the menubar/toolbar/
//! status bar). [`layers_panel::populate_layers_panel`],
//! [`history_panel::populate_history_panel`], and
//! [`properties_panel::populate_properties_panel`] are the "Layers,
//! history, tool-options panels" bullet's three slices — real content
//! from a real `aurora_doc::LayerTree`/`History`, and (for Properties) a
//! generic label/value row list `aurora-app` populates from whichever
//! real per-tool parameters it actually has (see
//! [`properties_panel`]'s own doc comment for why this crate carries no
//! tool-specific knowledge itself).
//!
//! [`layer_controls`] (0.135.0) adds the Layers panel's opacity slider,
//! blend-mode dropdown and visibility checkbox for the active layer —
//! the first panel widgets that edit a live document (`aurora-app` owns
//! the undo semantics).
//!
//! [`canvas_view::CanvasView`] and [`tool`] are PLAN.md M1.9's "basic
//! tools" bullet: the pan/zoom coordinate transform and the tool
//! dispatch logic (Move, Marquee Select, Zoom, Pan, Eyedropper) a real
//! pointer-driven canvas needs — see each module's own doc comment for
//! what's real today and what's still honestly open (Move and Eyedropper
//! in particular).

pub mod canvas_view;
pub mod curves_controls;
pub mod gallery_panel;
pub mod history_panel;
pub mod layer_controls;
pub mod layers_panel;
pub mod panel;
pub mod panel_group;
pub mod properties_panel;
pub mod status_bar;
pub mod tool;
pub mod tool_controls;
pub mod tools_panel;
pub mod workspace;

pub use canvas_view::CanvasView;
pub use curves_controls::{
    CurvesChannel, CurvesControls, channel_curve, curves_controls_contains, curves_controls_shown,
    curves_params, curves_selected_channel, insert_curves_controls, select_curves_channel,
    sync_curves_controls, with_channel_curve,
};
pub use gallery_panel::{
    GALLERY_CURVE_CAPTION, GalleryPanel, apply_gallery_outcome, gallery_close_popovers,
    gallery_contains, gallery_content_height, gallery_hover, gallery_light_dismiss,
    gallery_next_deadline, gallery_tick, insert_gallery_panel, remove_gallery_panel,
};
pub use history_panel::{
    HistoryRows, HistoryStep, UNDONE_STATE, populate_history_panel, populate_history_panel_rows,
};
pub use layer_controls::{
    LayerControls, blend_mode_index, blend_mode_label, blend_mode_options, insert_layer_controls,
    layer_controls_contains, sync_layer_controls,
};
pub use layers_panel::{layer_row_description, populate_layers_panel};
pub use panel::{
    PanelHandle, PanelSizing, clear_panel_body, close_panel, insert_panel, panel_is_closed,
    panel_is_collapsed, panel_sizing, set_panel_collapsed, set_panel_sizing,
};
pub use panel_group::{
    PanelGroup, follow_panel_group_tab, insert_panel_group, is_panel_group_tab,
    panel_group_contains, panel_group_is_collapsed, panel_group_selected, panel_group_shown,
    refocus_out_of_hidden, set_panel_group_collapsed, show_panel_group_tab, sync_panel_group,
};
pub use properties_panel::populate_properties_panel;
pub use status_bar::{
    STATUS_BAR_LABEL, StatusBar, StatusInfo, document_text, insert_status_bar, physical_zoom,
    sample_format_text, status_bar_text, sync_status_bar, zoom_text,
};
pub use tool::Tool;
pub use tool_controls::{
    TOOL_RADIUS_MAX, TOOL_RADIUS_MIN, ToolControls, insert_tool_controls, radius_readout,
    radius_slider_shown, sync_tool_controls, tool_controls_contains,
};
pub use tools_panel::{
    TOOLS_PANEL_LABEL, ToolsPanel, insert_tools_panel, selected_tool, sync_tools_panel,
    tools_panel_contains,
};
pub use workspace::{
    OPTIONS_BAR_LABEL, PANEL_GROUP_LABEL, PANEL_GROUP_TAB_DEFAULT, Workspace, build_workspace,
    close_workspace_panel, rail_width, select_panel_tab, set_rail_width, show_workspace_panel,
    toggle_workspace_panel,
};
