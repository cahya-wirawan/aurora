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
//! (drag-to-redock landed in 0.166.0 and floating panels in 0.167.0,
//! [`redock`] and [`dock`] — and the menubar/toolbar/
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
pub mod colour_controls;
pub mod curves_controls;
pub mod dock;
pub mod document_tabs;
pub mod fill_controls;
pub mod gallery_panel;
pub mod history_panel;
pub mod hue_saturation_controls;
pub mod layer_controls;
pub mod layer_filter;
pub mod layer_toolbar;
pub mod layers_panel;
pub mod levels_controls;
pub mod panel;
pub mod panel_group;
pub mod panel_strip;
pub mod properties_panel;
pub mod redock;
pub mod selection_outline;
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
pub use dock::{
    DockArrangement, DockPanel, DockPlacement, DockSlot, DropTarget, FloatSlot, PanelMove, SlotRef,
};
pub use document_tabs::{
    DOCUMENT_TABS_LABEL, INITIAL_DOCUMENT_TAB, document_tab_close_at, document_tab_close_index,
    document_tab_selected, insert_document_tabs, is_document_strip_tab, is_document_tab,
    link_document_panel, set_document_tab_descriptions, set_document_tab_names, sync_document_tabs,
};
pub use gallery_panel::{
    GALLERY_CURVE_CAPTION, GalleryPanel, apply_gallery_outcome, gallery_close_popovers,
    gallery_contains, gallery_content_height, gallery_hover, gallery_light_dismiss,
    gallery_next_deadline, gallery_tick, insert_gallery_panel, remove_gallery_panel,
};
pub use history_panel::{
    HistoryRows, HistoryStep, UNDONE_STATE, populate_history_panel, populate_history_panel_rows,
};
pub use hue_saturation_controls::{
    HueSaturationControls, HueSaturationSlider, hue_saturation_colorize_checked,
    hue_saturation_controls_contains, hue_saturation_controls_shown, hue_saturation_params,
    hue_saturation_selected_range, insert_hue_saturation_controls, sync_hue_saturation_controls,
};
pub use layer_controls::{
    LayerControls, LockToggle, blend_mode_at, blend_mode_index, blend_mode_index_for,
    blend_mode_label, blend_mode_options, blend_mode_options_for, insert_layer_controls,
    layer_controls_contains, sync_layer_controls, sync_layer_readouts,
};
pub use layer_filter::{
    KindToggle, LayerFilter, LayerFilterRow, NO_MATCH_TEXT, SEARCH_LABEL, SEARCH_PLACEHOLDER,
    clear_layer_filter_text, is_layer_filter_field, read_layer_filter, set_layer_filter_summary,
    shown_layers, write_layer_filter,
};
pub use layer_toolbar::{
    DELETE_UNAVAILABLE, LAYER_TOOLBAR_ICON_SIZE, LAYER_TOOLBAR_LABEL, LayerAction, LayerToolbar,
    insert_layer_toolbar, is_layer_toolbar_button, layer_toolbar_contains, layer_toolbar_height,
    set_layer_toolbar_hover, sync_layer_toolbar,
};
pub use levels_controls::{
    LevelsControls, LevelsSlider, channel_levels, insert_levels_controls, levels_controls_contains,
    levels_controls_shown, levels_params, levels_selected_channel, select_levels_channel,
    sync_levels_controls, with_channel_levels,
};
#[cfg(test)]
mod layer_filter_tests;
#[cfg(test)]
mod layers_panel_render;
pub use colour_controls::{
    COLOUR_BUTTON_ICON_SIZE, COLOUR_CONTROLS_LABEL, COLOUR_PICKER_ROWS, ColourAction,
    ColourControls, ColourPopover, ColourTarget, DEFAULT_COLOURS_DESCRIPTION,
    DEFAULT_COLOURS_LABEL, SWAP_COLOURS_DESCRIPTION, SWAP_COLOURS_LABEL, close_colour_popover,
    colour_group_height, colour_swatch_offset, colour_swatch_side, insert_colour_controls,
    is_colour_control, open_colour_popover, swatch_colour, swatch_description, swatch_name,
    sync_colour_controls,
};
pub use fill_controls::{
    FillControls, FillView, OPACITY_MAX, OPACITY_MIN, TOLERANCE_MAX, TOLERANCE_MIN,
    insert_fill_controls, is_fill_tool, opacity_readout_text, shown_fill_group, sync_fill_controls,
    tolerance_readout_text,
};
pub use layers_panel::{
    LayerRowPart, LayersView, layer_row_description, layer_row_height, layer_row_neighbor,
    layer_row_part, layer_row_thumbnail, layer_thumbnail_side, populate_layers_panel,
    populate_layers_panel_view, reset_layers_panel, set_layer_row_selected,
    set_layer_row_thumbnail, sync_layer_row, visible_layer_rows,
};
pub use panel::{
    FLOATING_DESCRIPTION, PanelHandle, PanelSizing, clear_panel_body, close_panel, insert_panel,
    panel_is_closed, panel_is_collapsed, panel_sizing, set_panel_collapsed, set_panel_content_cap,
    set_panel_sizing,
};
pub use panel_group::{
    PanelGroup, follow_panel_group_tab, insert_panel_group, is_panel_group_tab,
    panel_group_contains, panel_group_is_collapsed, panel_group_label, panel_group_selected,
    panel_group_shown, refocus_out_of_hidden, set_panel_group_collapsed, show_panel_group_tab,
    sync_panel_group,
};
pub use panel_strip::{
    PANEL_SHORT_LABELS, PANEL_STRIP_LABEL, PanelStrip, is_panel_strip_button, panel_short_label,
    panel_strip_shown,
};
pub use properties_panel::populate_properties_panel;
pub use redock::{
    PANEL_DRAG_THRESHOLD, PanelDrag, apply_dock_arrangement, apply_dock_arrangement_keeping_focus,
    dock_workspace_panel, drop_indicator_rect, drop_target_at, float_workspace_panel,
    move_workspace_panel, move_workspace_panel_by, move_workspace_slot, panel_drag_source,
};
pub use selection_outline::{
    DASH_LENGTH, MARCH_PERIOD, selection_outline_runs, sync_selection_outline,
};
pub use status_bar::{
    STATUS_BAR_LABEL, StatusBar, StatusInfo, document_label_text, document_text, insert_status_bar,
    physical_zoom, sample_format_text, selection_text, status_bar_text, sync_status_bar, zoom_text,
};
pub use tool::Tool;
pub use tool_controls::{
    TOOL_RADIUS_MAX, TOOL_RADIUS_MIN, ToolControls, insert_tool_controls, radius_readout,
    radius_slider_shown, sync_tool_controls, tool_controls_contains,
};
pub use tools_panel::{
    DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, TOOL_COLUMNS, TOOL_GROUPS, TOOL_ICON_SIZE,
    TOOLS_PANEL_LABEL, ToolsPanel, insert_tools_panel, is_tools_panel_button, selected_tool,
    set_tools_panel_hover, sync_tools_panel, tool_description, tool_icon, tools_panel_contains,
    tools_panel_width,
};
pub use workspace::{
    FloatFrame, OPTIONS_BAR_LABEL, PANEL_GROUP_LABEL, PANEL_GROUP_TAB_DEFAULT, RAIL_WIDTH_DEFAULT,
    RailSlot, Workspace, build_workspace, close_workspace_panel, docked_sizing,
    expand_rail_showing, floating_index_at, panel_focus_target, rail_collapsed, rail_width,
    raise_floating, refocus_workspace, select_panel_tab, set_rail_collapsed, set_rail_width,
    show_workspace_panel, sync_floating_frames, sync_floating_shown, toggle_rail_collapsed,
    toggle_workspace_panel,
};
