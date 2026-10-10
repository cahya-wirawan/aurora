//! Drag-to-redock (0.166.0, workspace round 5) and floating panels
//! (0.167.0): applying a [`DockArrangement`] to the workspace's tree, the
//! move operations the pointer drag and the keyboard commands share, and
//! the pointer drag's own state machine ([`PanelDrag`]). Photoshop's dock
//! convention, not its look.
//!
//! **Applying an arrangement moves panels, never rebuilds them.** Each
//! panel's whole subtree — its content, ids, scroll offset, sizing and
//! collapsed or closed state — is reparented with
//! `WidgetTree::move_child`; only the tab groups' own roots and tab bars,
//! and (0.167.0) the floating frames, are rebuilt. So every handle
//! `aurora-app` holds into a panel (layer rows, the Layers controls, the
//! Curves editor, History rows) stays valid across any move, and nothing
//! needs repopulating.
//!
//! **A drag** starts on a docked lone panel's title row or on a docked
//! group's tab ([`panel_drag_source`]), and (0.167.0) on a floating
//! panel: its title row or its group's bare tab strip drags the whole
//! floating slot, a floating group's tab drags that one panel out. Below
//! [`PANEL_DRAG_THRESHOLD`] of travel it is still a click — whatever the
//! press already did (a tab switch) stands and the release changes
//! nothing. Past it, every move picks a drop target and shows the drop
//! indicator there: a line in a rail gap, an outline round a slot's title
//! row or tab strip to join it as a tab (a floating slot's too), or — over
//! the canvas area — an outline of where the panel will float. A drop on
//! a target moves the panel ([`move_workspace_panel`],
//! [`move_workspace_slot`]); a drop anywhere else, `Escape`, or a
//! collapsed rail under a docked panel's drag cancels, leaving the
//! arrangement exactly as it was.
//!
//! **The collapsed rail (0.165.0's label strip) disables docked
//! dragging.** No drag can start on a docked panel while the rail is
//! collapsed, and a docked panel's drag in progress when the rail
//! collapses finds no target and cancels. Floating panels stay usable:
//! they can still be moved and torn apart over the canvas area (the rail
//! is hidden, so nothing can be re-docked by pointer until it expands).
//! The keyboard commands still rearrange a collapsed rail's slots.
//!
//! **Not document state.** Nothing here touches `aurora_doc::History`:
//! a layout move is not undoable, the same as a rail resize or a panel
//! collapse.

use aurora_core::Rect;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, DropIndicatorKind, WidgetKind, row_height};
use aurora_widgets::{FocusManager, WidgetError};

use crate::dock::{DockArrangement, DockPanel, DockPlacement, DropTarget, PanelMove, SlotRef};
use crate::panel::{
    PanelHandle, set_panel_collapsed, set_panel_grouped, set_panel_raised, set_panel_sizing,
};
use crate::panel_group::{build_panel_group, set_panel_group_collapsed, show_panel_group_tab};
use crate::workspace::{
    FloatFrame, RailSlot, Workspace, docked_sizing, float_frame_style, float_grip_style,
    float_width, panel_focus_target, rail_collapsed, set_rail_collapsed, set_shown,
};
use crate::{panel_is_collapsed, panel_sizing};

/// How far, in logical px, a press on a panel's title or tab must travel
/// before it becomes a panel drag rather than a click. An interaction
/// tolerance, not a style value (the same footing as `aurora-app`'s
/// `RAIL_DIVIDER_HIT_TOLERANCE`): invariant §7.3.10 governs what a widget
/// draws, and no design token names a drag distance. 4 px matches the
/// common desktop default (Windows' `SM_CXDRAG`). **Logical px**: every
/// point handed to [`PanelDrag`] is window-logical (`aurora-app` divides
/// each physical cursor position by the window's scale factor first), so
/// the threshold is 4 pt on a Retina display, 8 physical px. A floating
/// panel's title drag uses the same threshold (0.167.0).
pub const PANEL_DRAG_THRESHOLD: f32 = 4.0;

/// `arrangement` as untrusted placed slots, for
/// [`DockArrangement::repaired_placed`].
fn raw_placed(
    arrangement: &DockArrangement,
) -> Vec<(DockPlacement, Vec<Option<DockPanel>>, usize)> {
    let slot = |slot: &crate::dock::DockSlot| {
        (
            slot.panels.iter().copied().map(Some).collect::<Vec<_>>(),
            slot.selected,
        )
    };
    arrangement
        .slots()
        .iter()
        .map(|entry| {
            let (panels, selected) = slot(entry);
            (DockPlacement::Rail, panels, selected)
        })
        .chain(arrangement.floating().iter().map(|float| {
            let (panels, selected) = slot(&float.slot);
            (
                DockPlacement::Floating {
                    x: float.x,
                    y: float.y,
                },
                panels,
                selected,
            )
        }))
        .collect()
}

/// Puts a dissolved group's members back into the rail as lone panels and
/// removes the group's root.
fn dissolve_group(
    workspace: &mut Workspace,
    group: &crate::panel_group::PanelGroup,
    scales: &Scales,
) -> Result<(), WidgetError> {
    let rail = workspace.rail;
    for member in &group.members {
        workspace.tree.move_child(member.root, rail, usize::MAX)?;
        set_panel_grouped(&mut workspace.tree, *member, false, scales)?;
    }
    workspace.tree.remove(group.root)
}

/// Builds slot `members` (with tab `selected`) as child `index` of
/// `parent`: a lone panel moved there, or a group built round its members
/// and collapsed or expanded as its selected member was.
fn build_slot(
    workspace: &mut Workspace,
    parent: aurora_widgets::WidgetId,
    index: usize,
    slot: &crate::dock::DockSlot,
    scales: &Scales,
) -> Result<RailSlot, WidgetError> {
    let members: Vec<(PanelHandle, &str)> = slot
        .panels
        .iter()
        .map(|&panel| (workspace.panel(panel), panel.title()))
        .collect();
    if let [(panel, _)] = members.as_slice() {
        workspace.tree.move_child(panel.root, parent, index)?;
        return Ok(RailSlot::Panel(*panel));
    }
    let collapsed = match slot.selected_panel() {
        Some(panel) => panel_is_collapsed(&workspace.tree, workspace.panel(panel))?,
        None => false,
    };
    let group = build_panel_group(
        &mut workspace.tree,
        parent,
        index,
        &members,
        slot.selected,
        scales,
    )?;
    set_panel_group_collapsed(&mut workspace.tree, &group, collapsed)?;
    Ok(RailSlot::Group(group))
}

/// Rearranges the dock to `arrangement` (repaired first, so every panel
/// appears exactly once — [`DockArrangement::repaired_placed`]). A no-op
/// (`Ok(false)`) when it already is. Every group is dissolved back into
/// lone panels and every floating frame removed (its panel moved back to
/// the rail first), then each rail slot is rebuilt in order — a lone
/// panel's root moved into place, a group built round its members
/// (`crate::panel_group`'s `build_panel_group`) and collapsed or expanded
/// as its selected member was, so a group always collapses as one — and
/// (0.167.0) each floating slot, bottom to top, in a new frame appended to
/// the canvas area ([`crate::workspace::FloatFrame`]). A floating panel is
/// a `RaisedPanel`, [`crate::panel::FLOATING_DESCRIPTION`]-described and
/// content-sized; a docked one goes back to its docked kind and sizing
/// ([`crate::docked_sizing`]). The label strip's buttons follow the docked
/// panel order, a floating panel's hidden (the strip expands the rail, and
/// a floating panel is not in it). The caller repairs focus
/// ([`crate::refocus_workspace`], or use
/// [`apply_dock_arrangement_keeping_focus`]) and re-runs layout.
///
/// **All-or-nothing in practice (review I3).** Every id the rebuild
/// touches — the rail, the strip, the canvas area, each panel's root,
/// title slot, body and viewport, each current group's root and bar, each
/// floating frame — is checked to exist *before* anything moves, and the
/// arrangement is repaired first (so every slot is non-empty with an
/// in-range tab, which is all `build_panel_group` can refuse). After that
/// check nothing in the rebuild can fail: every step addresses an id just
/// checked or just created. So an `Err` means the precheck refused and
/// the tree is untouched. The tree is not transactional, so this rests on
/// that argument, not on a rollback.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace, before
/// anything changes.
pub fn apply_dock_arrangement(
    workspace: &mut Workspace,
    arrangement: &DockArrangement,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let (arrangement, _) = DockArrangement::repaired_placed(raw_placed(arrangement));
    if workspace.dock_arrangement() == arrangement {
        return Ok(false);
    }
    let rail = workspace.rail;
    let mut needed = vec![rail, workspace.panel_strip.root, workspace.canvas_area];
    for panel in DockPanel::ALL {
        let handle = workspace.panel(panel);
        needed.extend([handle.root, handle.header, handle.body, handle.viewport]);
    }
    needed.extend(workspace.groups().flat_map(|group| [group.root, group.bar]));
    needed.extend(workspace.floating.iter().map(|float| float.frame));
    if let Some(&missing) = needed.iter().find(|&&id| !workspace.tree.contains(id)) {
        return Err(WidgetError::UnknownWidget(missing));
    }
    for slot in std::mem::take(&mut workspace.slots) {
        if let RailSlot::Group(group) = slot {
            dissolve_group(workspace, &group, scales)?;
        }
    }
    for float in std::mem::take(&mut workspace.floating) {
        match &float.content {
            RailSlot::Group(group) => dissolve_group(workspace, group, scales)?,
            RailSlot::Panel(panel) => workspace.tree.move_child(panel.root, rail, usize::MAX)?,
        }
        workspace.tree.remove(float.frame)?;
    }
    for (index, slot) in arrangement.slots().iter().enumerate() {
        let built = build_slot(workspace, rail, index, slot, scales)?;
        workspace.slots.push(built);
    }
    let width = float_width(workspace);
    #[allow(clippy::cast_precision_loss)]
    let max_height = workspace
        .tree
        .bounds(workspace.canvas_area)
        .filter(|canvas| canvas.height > 0)
        .map(|canvas| canvas.height as f32);
    for float in arrangement.floating() {
        let frame = workspace.tree.insert(
            workspace.canvas_area,
            float_frame_style(float.x, float.y, width, max_height),
            accesskit::Node::new(accesskit::Role::GenericContainer),
            WidgetKind::RaisedPanel,
        )?;
        let grip = if float.slot.panels.len() > 1 {
            let mut node = accesskit::Node::new(accesskit::Role::GenericContainer);
            // A purely visual handle: the group's tab list names it.
            node.set_hidden();
            Some(workspace.tree.insert(
                frame,
                float_grip_style(scales),
                node,
                WidgetKind::Container,
            )?)
        } else {
            None
        };
        let content = build_slot(workspace, frame, usize::MAX, &float.slot, scales)?;
        workspace.floating.push(FloatFrame {
            frame,
            grip,
            content,
            x: float.x,
            y: float.y,
        });
    }
    for panel in DockPanel::ALL {
        let handle = workspace.panel(panel);
        let floating = arrangement.is_floating(panel);
        set_panel_raised(&mut workspace.tree, handle, floating)?;
        let sizing = if floating {
            crate::PanelSizing::Content
        } else {
            docked_sizing(panel)
        };
        if panel_sizing(&workspace.tree, handle)? != sizing {
            set_panel_sizing(&mut workspace.tree, handle, sizing, scales)?;
        }
    }
    sync_strip(workspace, &arrangement)?;
    // Review J-1: a frame whose panels are all closed is born hidden.
    crate::workspace::sync_floating_shown(workspace)?;
    Ok(true)
}

/// [`apply_dock_arrangement`] with focus kept (review I2): a focused
/// widget that still exists and is shown keeps focus (panel subtrees move
/// whole); one a rebuilt tab bar took away — a focused tab — moves to its
/// panel's new place ([`panel_focus_target`]); then the hidden-widget
/// repair runs ([`crate::refocus_workspace`]). The shared path of
/// [`move_workspace_panel`] and `aurora-app`'s Reset Panel Layout.
///
/// # Errors
///
/// As [`apply_dock_arrangement`].
pub fn apply_dock_arrangement_keeping_focus(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    arrangement: &DockArrangement,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let held = focus
        .focused()
        .and_then(|focused| workspace.panel_holding(focused));
    let changed = apply_dock_arrangement(workspace, arrangement, scales)?;
    if focus.validate(&workspace.tree)
        && let Some(held) = held
        && let Some(target) = panel_focus_target(workspace, held)
    {
        let _ = focus.focus(&mut workspace.tree, target);
    }
    crate::refocus_workspace(workspace, focus);
    Ok(changed)
}

/// Orders the label strip's buttons as the docked panels stand in the
/// rail, and shows only theirs (0.167.0: a floating panel has no strip
/// button — the strip's action is "expand the rail showing this panel",
/// and a floating panel is not in the rail; it stays visible over the
/// canvas instead).
fn sync_strip(workspace: &mut Workspace, arrangement: &DockArrangement) -> Result<(), WidgetError> {
    let order = arrangement
        .slots()
        .iter()
        .flat_map(|slot| slot.panels.iter().copied())
        .chain(
            arrangement
                .floating()
                .iter()
                .flat_map(|float| float.slot.panels.iter().copied()),
        );
    for (index, panel) in order.enumerate() {
        if let Some(button) = workspace.panel_strip.button_for(workspace.panel(panel)) {
            workspace
                .tree
                .move_child(button, workspace.panel_strip.root, index)?;
            set_shown(&mut workspace.tree, button, !arrangement.is_floating(panel))?;
        }
    }
    Ok(())
}

/// Shows `panel` where it now is, as a tab click would: its tab selected
/// and its slot expanded, reopening it if it was closed (the contract a
/// tab click on a closed member's tab already has).
fn show_moved_panel(workspace: &mut Workspace, panel: PanelHandle) -> Result<(), WidgetError> {
    if let Some(group) = workspace.group_of(panel).cloned()
        && let Some(index) = group.index_of(panel)
    {
        return show_panel_group_tab(&mut workspace.tree, &group, index);
    }
    set_panel_collapsed(&mut workspace.tree, panel, false)
}

/// Moves `panel` to `target` — the drop half of a [`PanelDrag`] and the
/// keyboard commands' shared path. Returns `Ok(false)`, changing
/// nothing, when the move is a no-op ([`DockArrangement::moved`]).
/// Otherwise the arrangement is applied ([`apply_dock_arrangement`]),
/// the moved panel shown and its tab selected where it landed
/// (`show_moved_panel`), and focus repaired: a focused widget that still
/// exists and is shown keeps focus (panel subtrees move whole); one a
/// rebuilt tab bar took away moves to its panel's new place
/// ([`panel_focus_target`]); then the hidden-widget repair runs
/// ([`crate::refocus_workspace`]).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn move_workspace_panel(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    panel: DockPanel,
    target: DropTarget,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let Some(next) = workspace.dock_arrangement().moved(panel, target) else {
        return Ok(false);
    };
    apply_dock_arrangement_keeping_focus(workspace, focus, &next, scales)?;
    let handle = workspace.panel(panel);
    show_moved_panel(workspace, handle)?;
    // Showing the moved panel can hide the tab that held focus.
    crate::refocus_workspace(workspace, focus);
    Ok(true)
}

/// Moves the **whole slot** holding `member` to `target` (0.167.0: a
/// floating panel's or floating group's title drag —
/// [`DockArrangement::moved_slot`]): a move over the canvas area, a
/// re-dock into a rail gap, or a join of another slot. Each panel keeps
/// its collapsed and closed state; focus is kept as
/// [`apply_dock_arrangement_keeping_focus`] keeps it. `Ok(false)` for a
/// no-op.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn move_workspace_slot(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    member: DockPanel,
    target: DropTarget,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let Some(next) = workspace.dock_arrangement().moved_slot(member, target) else {
        return Ok(false);
    };
    apply_dock_arrangement_keeping_focus(workspace, focus, &next, scales)?;
    Ok(true)
}

/// A keyboard move of `panel` ([`PanelMove`], the command path):
/// [`move_workspace_panel`] to [`DockArrangement::move_target`], then
/// focus on the moved panel ([`panel_focus_target`]) so a keyboard or
/// screen-reader user lands where it went — unless the rail is collapsed,
/// when focus is left to the strip. Returns `Ok(false)` at an end (a lone
/// top panel moved up, say) and for a floating panel, changing nothing.
///
/// # Errors
///
/// As [`move_workspace_panel`].
pub fn move_workspace_panel_by(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    panel: DockPanel,
    direction: PanelMove,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    let Some(target) = workspace.dock_arrangement().move_target(panel, direction) else {
        return Ok(false);
    };
    if !move_workspace_panel(workspace, focus, panel, target, scales)? {
        return Ok(false);
    }
    if !rail_collapsed(workspace)
        && let Some(target) = panel_focus_target(workspace, workspace.panel(panel))
    {
        let _ = focus.focus(&mut workspace.tree, target);
    }
    Ok(true)
}

/// "Float PANEL Panel" (0.167.0, the keyboard path to floating): floats
/// `panel` — out of its group, if grouped — on top of every other floating
/// panel, at a cascade position (one `row_height` further right and down
/// per panel already floating, from the canvas area's top-left; the
/// layout clamp keeps it inside), and lands focus on it. `Ok(false)` for a
/// panel already floating.
///
/// # Errors
///
/// As [`move_workspace_panel`].
pub fn float_workspace_panel(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    panel: DockPanel,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    if workspace.dock_arrangement().is_floating(panel) {
        return Ok(false);
    }
    #[allow(clippy::cast_precision_loss)]
    let step = row_height(scales) * (workspace.floating.len() + 1) as f32;
    if !move_workspace_panel(
        workspace,
        focus,
        panel,
        DropTarget::Float { x: step, y: step },
        scales,
    )? {
        return Ok(false);
    }
    if let Some(target) = panel_focus_target(workspace, workspace.panel(panel)) {
        let _ = focus.focus(&mut workspace.tree, target);
    }
    Ok(true)
}

/// "Dock PANEL Panel" (0.167.0): docks a floating `panel` — out of its
/// floating group, if grouped — as the rail's last slot, expanding a
/// collapsed rail so it can be seen, and lands focus on it. `Ok(false)`
/// for a panel already docked.
///
/// # Errors
///
/// As [`move_workspace_panel`].
pub fn dock_workspace_panel(
    workspace: &mut Workspace,
    focus: &mut FocusManager,
    panel: DockPanel,
    scales: &Scales,
) -> Result<bool, WidgetError> {
    if !workspace.dock_arrangement().is_floating(panel) {
        return Ok(false);
    }
    set_rail_collapsed(workspace, false)?;
    let gap = workspace.slots.len();
    if !move_workspace_panel(workspace, focus, panel, DropTarget::Gap(gap), scales)? {
        return Ok(false);
    }
    if let Some(target) = panel_focus_target(workspace, workspace.panel(panel)) {
        let _ = focus.focus(&mut workspace.tree, target);
    }
    Ok(true)
}

fn contains(rect: Rect, (x, y): (f32, f32)) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let (left, top, width, height) = (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    width > 0.0 && height > 0.0 && x >= left && x < left + width && y >= top && y < top + height
}

/// A slot's title zone: a lone panel's title row, a group's tab strip.
fn title_zone(workspace: &Workspace, slot: &RailSlot) -> Option<Rect> {
    match slot {
        RailSlot::Panel(panel) => workspace.tree.bounds(panel.header),
        RailSlot::Group(group) => workspace.tree.bounds(group.bar),
    }
}

/// What a press would drag, and from where.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DragSource {
    panel: DockPanel,
    /// The whole slot moves (a floating title or bare tab strip), not the
    /// one panel.
    whole_slot: bool,
    /// The panel or slot is docked in the rail.
    docked: bool,
    /// The top-left the dragged thing's float position is measured from
    /// (its frame, title row or tab), window-logical px.
    anchor: (f32, f32),
    /// The dragged thing's current height, logical px.
    height: f32,
}

#[allow(clippy::cast_precision_loss)]
fn origin(rect: Rect) -> (f32, f32) {
    (rect.x as f32, rect.y as f32)
}

#[allow(clippy::cast_precision_loss)]
fn height_of(workspace: &Workspace, id: aurora_widgets::WidgetId) -> f32 {
    workspace
        .tree
        .bounds(id)
        .map_or(0.0, |bounds| bounds.height as f32)
}

/// A group's tab under `point`, as that tab's panel and the tab's bounds.
fn tab_at(
    workspace: &Workspace,
    group: &crate::panel_group::PanelGroup,
    point: (f32, f32),
) -> Option<(DockPanel, PanelHandle, Rect)> {
    let state = widgets::tab_bar_state(&workspace.tree, group.bar).ok()?;
    state.tabs().iter().enumerate().find_map(|(index, &tab)| {
        let zone = workspace
            .tree
            .bounds(tab)
            .filter(|&zone| contains(zone, point))?;
        let member = *group.members.get(index)?;
        Some((workspace.dock_panel(member)?, member, zone))
    })
}

fn drag_source(workspace: &Workspace, point: (f32, f32)) -> Option<DragSource> {
    // Floating panels first, topmost first: they are drawn over the canvas
    // and nothing else is under them.
    for float in workspace.floating.iter().rev() {
        if !crate::workspace::float_frame_shown(&workspace.tree, float.frame) {
            continue;
        }
        let Some(frame) = workspace.tree.bounds(float.frame) else {
            continue;
        };
        if !contains(frame, point) {
            continue;
        }
        // The float's exact position, not its rounded bounds, so a move
        // shifts it by exactly the pointer's travel.
        #[allow(clippy::cast_precision_loss)]
        let anchor = workspace.tree.bounds(workspace.canvas_area).map_or_else(
            || origin(frame),
            |canvas| (canvas.x as f32 + float.x, canvas.y as f32 + float.y),
        );
        let whole = |panel: Option<DockPanel>| {
            panel.map(|panel| DragSource {
                panel,
                whole_slot: true,
                docked: false,
                anchor,
                height: height_of(workspace, float.frame),
            })
        };
        return match &float.content {
            RailSlot::Panel(panel) => {
                if workspace
                    .tree
                    .bounds(panel.header)
                    .is_some_and(|zone| contains(zone, point))
                {
                    whole(workspace.dock_panel(*panel))
                } else {
                    None
                }
            }
            RailSlot::Group(group) => {
                if let Some((panel, member, zone)) = tab_at(workspace, group, point) {
                    Some(DragSource {
                        panel,
                        whole_slot: false,
                        docked: false,
                        anchor: origin(zone),
                        height: height_of(workspace, member.root),
                    })
                } else if [Some(group.bar), float.grip]
                    .into_iter()
                    .flatten()
                    .any(|zone| {
                        workspace
                            .tree
                            .bounds(zone)
                            .is_some_and(|zone| contains(zone, point))
                    })
                {
                    let selected = crate::panel_group_selected(&workspace.tree, group)
                        .ok()
                        .and_then(|index| group.members.get(index).copied())
                        .and_then(|member| workspace.dock_panel(member));
                    whole(selected)
                } else {
                    None
                }
            }
        };
    }
    if rail_collapsed(workspace) {
        return None;
    }
    for slot in &workspace.slots {
        match slot {
            RailSlot::Panel(panel) => {
                if let Some(zone) = workspace
                    .tree
                    .bounds(panel.header)
                    .filter(|&zone| contains(zone, point))
                {
                    return workspace.dock_panel(*panel).map(|id| DragSource {
                        panel: id,
                        whole_slot: false,
                        docked: true,
                        anchor: origin(zone),
                        height: height_of(workspace, panel.root),
                    });
                }
            }
            RailSlot::Group(group) => {
                if let Some((panel, member, zone)) = tab_at(workspace, group, point) {
                    return Some(DragSource {
                        panel,
                        whole_slot: false,
                        docked: true,
                        anchor: origin(zone),
                        height: height_of(workspace, member.root),
                    });
                }
            }
        }
    }
    None
}

/// The panel a primary press at `point` would drag: a docked lone panel's
/// title row or one of a docked group's tabs (that tab's panel), or
/// (0.167.0) a floating panel's title row, a floating group's tab, or its
/// grip ([`crate::FloatFrame::grip`]) or any bare part of its tab strip
/// (the selected panel; the whole floating slot moves).
/// `None` anywhere else, and over the docked panels while the rail is
/// collapsed.
#[must_use]
pub fn panel_drag_source(workspace: &Workspace, point: (f32, f32)) -> Option<DockPanel> {
    drag_source(workspace, point).map(|source| source.panel)
}

/// The rail target under `point`, unvalidated: see [`drop_target_at`].
fn rail_target_at(workspace: &Workspace, point: (f32, f32)) -> Option<DropTarget> {
    if rail_collapsed(workspace) {
        return None;
    }
    let rail = workspace.tree.bounds(workspace.rail)?;
    if !contains(rail, point) {
        return None;
    }
    let mut target = DropTarget::Gap(workspace.slots.len());
    for (index, slot) in workspace.slots.iter().enumerate() {
        let Some(bounds) = workspace.tree.bounds(slot.root()) else {
            continue;
        };
        if bounds.width == 0 || bounds.height == 0 {
            continue;
        }
        if let Some(zone) = title_zone(workspace, slot).filter(|&zone| contains(zone, point)) {
            // The zone's top quarter is still the gap above, so a gap
            // stays reachable above a slot whose body is too short (or
            // collapsed) to have an upper half of its own.
            #[allow(clippy::cast_precision_loss)]
            let edge = zone.y as f32 + zone.height as f32 / 4.0;
            target = if point.1 < edge {
                DropTarget::Gap(index)
            } else {
                DropTarget::Join(index)
            };
            break;
        }
        if contains(bounds, point) {
            #[allow(clippy::cast_precision_loss)]
            let middle = bounds.y as f32 + bounds.height as f32 / 2.0;
            target = DropTarget::Gap(if point.1 < middle { index } else { index + 1 });
            break;
        }
    }
    Some(target)
}

/// Where `panel` would land if dropped at `point` **in the rail**, from
/// the last layout: `None` outside the rail, while the rail is collapsed,
/// and where the drop would change nothing ([`DockArrangement::moved`]).
/// On a slot's title zone (a lone panel's title row, a group's tab strip)
/// it joins that slot — except in the zone's top quarter, which is the gap
/// above it; elsewhere in a slot's upper half it goes into the gap above
/// it, in its lower half the gap below; below every slot, the last gap.
/// Zero-height slots (a closed panel, a group whose members are all
/// closed) are skipped, so they are never a join target and their gaps
/// are reached through their neighbours. A [`PanelDrag`] adds the floating
/// targets (0.167.0) on top of this.
#[must_use]
pub fn drop_target_at(
    workspace: &Workspace,
    panel: DockPanel,
    point: (f32, f32),
) -> Option<DropTarget> {
    let target = rail_target_at(workspace, point)?;
    workspace
        .dock_arrangement()
        .moved(panel, target)
        .map(|_| target)
}

/// The float position, logical px from the canvas area's top-left, a
/// floating panel `height` tall dragged by `anchor`'s offset `grab` would
/// take with the pointer at `point`: clamped as
/// [`crate::sync_floating_frames`] clamps it. `None` before the canvas
/// area is laid out.
fn float_position(
    workspace: &Workspace,
    point: (f32, f32),
    grab: (f32, f32),
    height: f32,
) -> Option<(f32, f32)> {
    let canvas = workspace.tree.bounds(workspace.canvas_area)?;
    #[allow(clippy::cast_precision_loss)]
    let (cx, cy, cw, ch) = (
        canvas.x as f32,
        canvas.y as f32,
        canvas.width as f32,
        canvas.height as f32,
    );
    let width = float_width(workspace);
    let x = (point.0 - grab.0 - cx).clamp(0.0, (cw - width).max(0.0));
    let y = (point.1 - grab.1 - cy).clamp(0.0, (ch - height.min(ch)).max(0.0));
    Some((x, y))
}

/// Where the drop indicator goes for `target`, in window-logical px (the
/// indicator is a root child, so that is also relative to its parent):
/// a gap is a `size.indicator_width`-thick line (the 0.166.0 token)
/// across the rail at the boundary between the two slots, kept inside the
/// rail; a join outlines the slot's title zone (a floating slot's too);
/// a float (0.167.0) outlines where the floating panel will appear — the
/// float width and, here, one title row tall ([`PanelDrag`] draws it as
/// tall as the dragged panel).
#[must_use]
pub fn drop_indicator_rect(
    workspace: &Workspace,
    target: DropTarget,
    scales: &Scales,
) -> Option<(Rect, DropIndicatorKind)> {
    float_indicator(workspace, target, scales, row_height(scales))
}

fn float_indicator(
    workspace: &Workspace,
    target: DropTarget,
    scales: &Scales,
    height: f32,
) -> Option<(Rect, DropIndicatorKind)> {
    match target {
        DropTarget::Join(index) => {
            let zone = title_zone(workspace, workspace.slots.get(index)?)?;
            Some((zone, DropIndicatorKind::Target))
        }
        DropTarget::JoinFloating(index) => {
            let zone = title_zone(workspace, &workspace.floating.get(index)?.content)?;
            Some((zone, DropIndicatorKind::Target))
        }
        DropTarget::Float { x, y } => {
            let canvas = workspace.tree.bounds(workspace.canvas_area)?;
            #[allow(clippy::cast_possible_truncation)]
            let (dx, dy) = (x.round() as i64, y.round() as i64);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (width, height) = (
                float_width(workspace).max(0.0).round() as u32,
                height
                    .max(row_height(scales))
                    .min(f32::from(u16::MAX))
                    .round() as u32,
            );
            Some((
                Rect {
                    x: canvas.x + dx,
                    y: canvas.y + dy,
                    width,
                    height: height.min(canvas.height),
                },
                DropIndicatorKind::Target,
            ))
        }
        DropTarget::Gap(index) => {
            let rail = workspace.tree.bounds(workspace.rail)?;
            let boundary = match workspace.slots.get(index) {
                Some(slot) => workspace.tree.bounds(slot.root())?.y,
                None => workspace
                    .slots
                    .iter()
                    .filter_map(|slot| workspace.tree.bounds(slot.root()))
                    .map(|bounds| bounds.y + i64::from(bounds.height))
                    .max()
                    .unwrap_or(rail.y),
            };
            let thickness = scales.size.indicator_width.max(1);
            let top = rail.y;
            let bottom = rail.y + i64::from(rail.height) - i64::from(thickness);
            let y = (boundary - i64::from(thickness / 2)).clamp(top, bottom.max(top));
            Some((
                Rect {
                    x: rail.x,
                    y,
                    width: rail.width,
                    height: thickness,
                },
                DropIndicatorKind::Insertion,
            ))
        }
    }
}

/// A primary press on a panel's title or tab, and the drag it may become
/// (0.166.0; floating panels 0.167.0). `aurora-app` holds one from the
/// press to the release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelDrag {
    panel: DockPanel,
    whole_slot: bool,
    docked: bool,
    origin: (f32, f32),
    /// The press's offset from the dragged thing's top-left, so a float
    /// lands with the grabbed point still under the pointer.
    grab: (f32, f32),
    height: f32,
    active: bool,
    target: Option<DropTarget>,
}

impl PanelDrag {
    /// A press at `point`: `Some` when it lands on a drag source
    /// ([`panel_drag_source`]). Changes nothing — the press's own effect
    /// (a tab switch) is the caller's ordinary routing.
    #[must_use]
    pub fn press(workspace: &Workspace, point: (f32, f32)) -> Option<Self> {
        drag_source(workspace, point).map(|source| Self {
            panel: source.panel,
            whole_slot: source.whole_slot,
            docked: source.docked,
            origin: point,
            grab: (point.0 - source.anchor.0, point.1 - source.anchor.1),
            height: source.height,
            active: false,
            target: None,
        })
    }

    /// The pressed panel (for a whole-slot drag, its selected panel).
    #[must_use]
    pub fn panel(&self) -> DockPanel {
        self.panel
    }

    /// Whether the drag moves the panel's whole slot (a floating panel's
    /// title or a floating group's bare tab strip, 0.167.0) rather than
    /// the one panel.
    #[must_use]
    pub fn moves_slot(&self) -> bool {
        self.whole_slot
    }

    /// Whether the press has travelled past [`PANEL_DRAG_THRESHOLD`] and
    /// is a drag.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The current drop target, `None` before the threshold and over
    /// anything that is not one.
    #[must_use]
    pub fn target(&self) -> Option<DropTarget> {
        self.target
    }

    /// The target at `point`, validated against the arrangement: a
    /// floating slot's title zone joins it; the rail is the rail's targets
    /// ([`drop_target_at`]); anywhere else over the canvas area — over a
    /// floating panel's body included — floats there (the dragged slot's
    /// own frame is canvas too). A docked panel's drag finds nothing while
    /// the rail is collapsed.
    fn target_at(&self, workspace: &Workspace, point: (f32, f32)) -> Option<DropTarget> {
        if self.docked && rail_collapsed(workspace) {
            return None;
        }
        let arrangement = workspace.dock_arrangement();
        let own = match arrangement.locate(self.panel) {
            Some((SlotRef::Floating(index), _)) => Some(index),
            _ => None,
        };
        let float = |workspace: &Workspace| {
            float_position(workspace, point, self.grab, self.height)
                .map(|(x, y)| DropTarget::Float { x, y })
        };
        let mut target = None;
        let mut decided = false;
        for (index, frame) in workspace.floating.iter().enumerate().rev() {
            if !crate::workspace::float_frame_shown(&workspace.tree, frame.frame)
                || !workspace
                    .tree
                    .bounds(frame.frame)
                    .is_some_and(|bounds| contains(bounds, point))
            {
                continue;
            }
            decided = true;
            let mine = self.whole_slot && own == Some(index);
            let on_title = title_zone(workspace, &frame.content)
                .into_iter()
                .chain(frame.grip.and_then(|grip| workspace.tree.bounds(grip)))
                .any(|zone| contains(zone, point));
            target = if !mine && on_title {
                Some(DropTarget::JoinFloating(index))
            } else {
                float(workspace)
            };
            break;
        }
        if !decided {
            target = rail_target_at(workspace, point).or_else(|| {
                workspace
                    .tree
                    .bounds(workspace.canvas_area)
                    .filter(|&canvas| contains(canvas, point))
                    .and_then(|_| float(workspace))
            });
        }
        let target = target?;
        let valid = if self.whole_slot {
            arrangement.moved_slot(self.panel, target)
        } else {
            arrangement.moved(self.panel, target)
        };
        valid.map(|_| target)
    }

    /// The pointer moved to `point`: crosses the threshold once travel
    /// exceeds it, then retargets and shows, moves or hides the drop
    /// indicator. Returns whether the tree changed (the caller re-runs
    /// layout).
    ///
    /// # Errors
    ///
    /// [`WidgetError::UnknownWidget`] for a malformed workspace.
    pub fn update(
        &mut self,
        workspace: &mut Workspace,
        point: (f32, f32),
        scales: &Scales,
    ) -> Result<bool, WidgetError> {
        if !self.active {
            let (dx, dy) = (point.0 - self.origin.0, point.1 - self.origin.1);
            if dx * dx + dy * dy <= PANEL_DRAG_THRESHOLD * PANEL_DRAG_THRESHOLD {
                return Ok(false);
            }
            self.active = true;
        }
        self.target = self.target_at(workspace, point);
        let placed = self
            .target
            .and_then(|target| float_indicator(workspace, target, scales, self.height));
        let indicator = workspace.drop_indicator;
        let before = workspace.tree.style(indicator).cloned();
        let shown_before = widgets::drop_indicator_state(&workspace.tree, indicator)?.shown();
        match placed {
            Some((rect, kind)) => {
                widgets::show_drop_indicator(&mut workspace.tree, indicator, rect, kind)?;
            }
            None => widgets::hide_drop_indicator(&mut workspace.tree, indicator)?,
        }
        let shown_after = widgets::drop_indicator_state(&workspace.tree, indicator)?.shown();
        Ok(before.as_ref() != workspace.tree.style(indicator) || shown_before != shown_after)
    }

    /// The release: hides the indicator, then — for an active drag over a
    /// target — moves the panel ([`move_workspace_panel`]) or its whole
    /// slot ([`move_workspace_slot`]) there. A click below the threshold,
    /// or a drop on no target, changes nothing else. Returns whether the
    /// arrangement changed.
    ///
    /// # Errors
    ///
    /// As [`move_workspace_panel`].
    pub fn finish(
        self,
        workspace: &mut Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
    ) -> Result<bool, WidgetError> {
        widgets::hide_drop_indicator(&mut workspace.tree, workspace.drop_indicator)?;
        match self.target {
            Some(target) if self.active && !(self.docked && rail_collapsed(workspace)) => {
                if self.whole_slot {
                    move_workspace_slot(workspace, focus, self.panel, target, scales)
                } else {
                    move_workspace_panel(workspace, focus, self.panel, target, scales)
                }
            }
            _ => Ok(false),
        }
    }

    /// `Escape`, the pointer leaving the window, a lost window focus or a
    /// second button: hides the indicator and changes nothing else.
    ///
    /// # Errors
    ///
    /// [`WidgetError::UnknownWidget`] for a malformed workspace.
    pub fn cancel(self, workspace: &mut Workspace) -> Result<(), WidgetError> {
        widgets::hide_drop_indicator(&mut workspace.tree, workspace.drop_indicator)
    }
}

#[cfg(test)]
mod tests {
    use aurora_core::Rect;
    use aurora_widgets::widgets::{self, DropIndicatorKind};
    use aurora_widgets::{FocusManager, PaintOp, WidgetId};

    use super::{PANEL_DRAG_THRESHOLD, PanelDrag, move_workspace_panel_by};
    use crate::dock::{DockPanel, DropTarget, PanelMove};
    use crate::workspace::{RailSlot, Workspace, build_workspace};

    const WINDOW: (f32, f32) = (1200.0, 800.0);

    fn scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn workspace() -> (Workspace, aurora_theme::Scales) {
        let scales = scales();
        let mut ws = build_workspace(&scales);
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        (ws, scales)
    }

    fn bounds(ws: &Workspace, id: WidgetId) -> Rect {
        match ws.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("{id:?} is laid out"),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn centre(rect: Rect) -> (f32, f32) {
        (
            rect.x as f32 + rect.width as f32 / 2.0,
            rect.y as f32 + rect.height as f32 / 2.0,
        )
    }

    #[allow(clippy::cast_precision_loss)]
    fn near_bottom(rect: Rect) -> (f32, f32) {
        (
            rect.x as f32 + rect.width as f32 / 2.0,
            rect.y as f32 + rect.height as f32 - 2.0,
        )
    }

    fn shape(ws: &Workspace) -> Vec<(Vec<DockPanel>, usize)> {
        ws.dock_arrangement()
            .slots()
            .iter()
            .map(|slot| (slot.panels.clone(), slot.selected))
            .collect()
    }

    fn tab_of(ws: &Workspace, panel: DockPanel) -> WidgetId {
        let handle = ws.panel(panel);
        let Some(group) = ws.group_of(handle) else {
            unreachable!("{panel:?} is grouped");
        };
        let Some(index) = group.index_of(handle) else {
            unreachable!("member");
        };
        match widgets::tab_bar_state(&ws.tree, group.bar)
            .ok()
            .and_then(|state| state.tabs().get(index).copied())
        {
            Some(tab) => tab,
            None => unreachable!("one tab per member"),
        }
    }

    /// Presses at `from`, moves through `to`, and returns the drag
    /// (re-laying out after every changing move, as the app does).
    fn drag(ws: &mut Workspace, from: (f32, f32), to: &[(f32, f32)]) -> PanelDrag {
        let Some(mut drag) = PanelDrag::press(ws, from) else {
            unreachable!("{from:?} is a drag source");
        };
        for &point in to {
            match drag.update(ws, point, &scales()) {
                Ok(true) => ws.tree.compute_layout(WINDOW.0, WINDOW.1),
                Ok(false) => {}
                Err(err) => unreachable!("{err:?}"),
            }
        }
        drag
    }

    fn finish(ws: &mut Workspace, focus: &mut FocusManager, drag: PanelDrag) -> bool {
        let scales = scales();
        let changed = match drag.finish(ws, focus, &scales) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        };
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        changed
    }

    /// Everything a cancel must leave untouched: the accessibility tree
    /// (structure, roles, labels, states), every widget's bounds and the
    /// arrangement.
    fn snapshot(ws: &Workspace) -> String {
        let update = ws.tree.accessibility_update(ws.root);
        let mut nodes: Vec<String> = update
            .nodes
            .iter()
            .map(|(id, node)| format!("{id:?}={node:?}@{:?}", ws.tree.bounds(*id)))
            .collect();
        nodes.sort();
        format!("{:?}|{}", shape(ws), nodes.join("\n"))
    }

    fn rail_children(ws: &Workspace) -> Vec<WidgetId> {
        ws.tree
            .children(ws.rail)
            .map(<[_]>::to_vec)
            .unwrap_or_default()
    }

    fn at_rail_children(ws: &Workspace) -> Vec<WidgetId> {
        let update = ws.tree.accessibility_update(ws.root);
        update
            .nodes
            .iter()
            .find(|(id, _)| *id == ws.rail)
            .map(|(_, node)| node.children().to_vec())
            .unwrap_or_default()
    }

    /// AC-1: a press on a title travelling no further than the threshold
    /// is a click — nothing moves and nothing is drawn — while a drag past
    /// it to the rail's bottom reorders Layers below the group, the tree's
    /// rail children and the accessibility tree following.
    #[test]
    fn a_drag_past_the_threshold_reorders_and_a_click_below_it_changes_nothing() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        let before = snapshot(&ws);
        let title = centre(bounds(&ws, ws.layers.header));
        let short = (title.0 + PANEL_DRAG_THRESHOLD, title.1);
        let click = drag(&mut ws, title, &[short]);
        assert!(
            !click.is_active(),
            "{PANEL_DRAG_THRESHOLD} px is still a click"
        );
        assert_eq!(click.target(), None);
        assert!(!finish(&mut ws, &mut focus, click));
        assert_eq!(snapshot(&ws), before, "a click changes nothing");

        let rail = near_bottom(bounds(&ws, ws.rail));
        let moved = drag(&mut ws, title, &[short, (title.0, title.1 + 50.0), rail]);
        assert!(moved.is_active());
        assert_eq!(moved.target(), Some(DropTarget::Gap(2)));
        assert!(finish(&mut ws, &mut focus, moved));
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::Properties, DockPanel::History], 0),
                (vec![DockPanel::Layers], 0)
            ]
        );
        let roots: Vec<WidgetId> = ws.slots.iter().map(RailSlot::root).collect();
        assert_eq!(rail_children(&ws), roots);
        assert_eq!(roots.last(), Some(&ws.layers.root));
        assert_eq!(at_rail_children(&ws), roots, "the AccessKit order follows");
        assert!(
            bounds(&ws, ws.layers.root).y > bounds(&ws, ws.properties.root).y,
            "Layers is laid out below the group"
        );
    }

    /// AC-1, the tab half: a click on History's tab below the threshold is
    /// today's tab switch (done by the press's ordinary routing) and moves
    /// nothing.
    #[test]
    fn a_tab_click_below_the_threshold_leaves_the_arrangement_alone() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        let tab = centre(bounds(&ws, tab_of(&ws, DockPanel::History)));
        let Some(press) = PanelDrag::press(&ws, tab) else {
            unreachable!("a tab is a drag source");
        };
        assert_eq!(press.panel(), DockPanel::History);
        let before = shape(&ws);
        assert!(!finish(&mut ws, &mut focus, press));
        assert_eq!(shape(&ws), before);
    }

    /// AC-2: Layers dropped on the group's tab strip joins it as its last
    /// tab, selected and shown; History dragged out of the group into the
    /// top gap becomes its own slot; the last member dragged out of a
    /// two-tab group dissolves it, leaving no empty group anywhere.
    #[test]
    fn a_tab_strip_drop_joins_a_drag_out_splits_and_empty_groups_go() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        let layers_row = ws.layers.body;
        let title = centre(bounds(&ws, ws.layers.header));
        let bar = centre(bounds(&ws, crate::workspace::test_group(&ws).bar));
        let joined = drag(&mut ws, title, &[(title.0, title.1 + 10.0), bar]);
        assert_eq!(joined.target(), Some(DropTarget::Join(1)));
        assert!(finish(&mut ws, &mut focus, joined));
        assert_eq!(
            shape(&ws),
            vec![(
                vec![DockPanel::Properties, DockPanel::History, DockPanel::Layers],
                2
            )]
        );
        let group = crate::workspace::test_group(&ws);
        assert_eq!(crate::panel_group_shown(&ws.tree, &group), Some(2));
        assert!(ws.tree.contains(layers_row), "the panel moved whole");
        assert_eq!(
            ws.tree
                .accessibility(ws.layers.root)
                .map(accesskit::Node::role),
            Some(accesskit::Role::TabPanel)
        );

        // History's tab out to the top gap: its own slot above the group.
        let bar_box = bounds(&ws, group.bar);
        let history_tab = centre(bounds(&ws, tab_of(&ws, DockPanel::History)));
        // The tab strip's top quarter is the gap above the group.
        #[allow(clippy::cast_precision_loss)]
        let top_half = (history_tab.0, bar_box.y as f32 + 1.0);
        let split = drag(&mut ws, history_tab, &[top_half]);
        assert_eq!(split.target(), Some(DropTarget::Gap(0)));
        assert!(finish(&mut ws, &mut focus, split));
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::History], 0),
                (vec![DockPanel::Properties, DockPanel::Layers], 1)
            ]
        );
        assert_eq!(
            ws.tree
                .accessibility(ws.history.root)
                .map(accesskit::Node::role),
            Some(accesskit::Role::Region),
            "a lone panel is a region again"
        );

        // Layers' tab out to the bottom gap: the two-tab group loses its
        // last but one member and is removed.
        let old_group = crate::workspace::test_group(&ws);
        let layers_tab = centre(bounds(&ws, tab_of(&ws, DockPanel::Layers)));
        let bottom = near_bottom(bounds(&ws, ws.rail));
        let out = drag(
            &mut ws,
            layers_tab,
            &[(layers_tab.0, layers_tab.1 + 30.0), bottom],
        );
        assert_eq!(out.target(), Some(DropTarget::Gap(2)));
        assert!(finish(&mut ws, &mut focus, out));
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::History], 0),
                (vec![DockPanel::Properties], 0),
                (vec![DockPanel::Layers], 0)
            ]
        );
        assert_eq!(ws.groups().count(), 0, "no group is left");
        assert!(
            !ws.tree.contains(old_group.root),
            "the emptied group is removed"
        );
        assert!(!ws.tree.contains(old_group.bar));
        assert_eq!(
            rail_children(&ws),
            vec![ws.history.root, ws.properties.root, ws.layers.root]
        );
    }

    /// AC-3: `Escape` (a cancel) mid-drag, a drop over the status bar and
    /// one over the tools panel each leave the whole tree byte-identical.
    /// (Adapted in 0.167.0: the canvas area is now a float target —
    /// `a_drop_over_the_canvas_floats_and_escape_cancels`.)
    #[test]
    fn a_cancel_or_a_drop_outside_any_target_changes_nothing() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        let before = snapshot(&ws);
        let title = centre(bounds(&ws, ws.layers.header));
        let history = near_bottom(bounds(&ws, ws.rail));

        let escaped = drag(&mut ws, title, &[(title.0, title.1 + 20.0), history]);
        assert!(
            escaped.target().is_some(),
            "over a real target when cancelled"
        );
        if let Err(err) = escaped.cancel(&mut ws) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        assert_eq!(snapshot(&ws), before, "Escape");

        for outside in [
            centre(bounds(&ws, ws.status_bar.root)),
            centre(bounds(&ws, ws.tools.root)),
        ] {
            let dropped = drag(&mut ws, title, &[history, outside]);
            assert!(dropped.is_active());
            assert_eq!(dropped.target(), None, "{outside:?} is no target");
            assert!(!finish(&mut ws, &mut focus, dropped));
            assert_eq!(snapshot(&ws), before, "dropped at {outside:?}");
        }
    }

    fn indicator_ops(ws: &Workspace) -> Vec<PaintOp> {
        const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
        const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
        let Ok(palette) = aurora_theme::Palette::from_toml_str(PALETTE_TOML) else {
            unreachable!("the committed palette parses");
        };
        let mut themes = aurora_theme::ThemeSet::new();
        if themes.register(DARK_THEME_TOML).is_err() {
            unreachable!("the committed Dark theme registers");
        }
        let theme = match themes.resolve("Dark", &palette) {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        };
        match aurora_widgets::paint_widget_ops(&ws.tree, ws.drop_indicator, &theme, &scales(), 1.0)
        {
            Ok(ops) => ops
                .into_iter()
                .inspect(|op| {
                    if let PaintOp::Solid((_, colour)) = op {
                        let [r, g, b] = theme.accent.primary.to_srgb_f32();
                        #[allow(clippy::float_cmp)]
                        let exact = *colour == [r, g, b, 1.0];
                        assert!(exact, "accent.primary, from the theme: {colour:?}");
                    }
                })
                .collect(),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// AC-4: the drop indicator paints nothing outside a drag, an
    /// `accent.primary` line in a gap (focus-ring thick, as wide as the
    /// rail), an outline round a tab strip it would join, and nothing
    /// again once the drag ends.
    #[test]
    fn the_drop_indicator_is_drawn_from_tokens_and_only_during_a_drag() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        assert!(indicator_ops(&ws).is_empty(), "nothing before a drag");
        let title = centre(bounds(&ws, ws.layers.header));
        let below_threshold = drag(&mut ws, title, &[(title.0 + 1.0, title.1)]);
        assert!(indicator_ops(&ws).is_empty(), "nothing below the threshold");
        if let Err(err) = below_threshold.cancel(&mut ws) {
            unreachable!("{err:?}");
        }

        let history = near_bottom(bounds(&ws, ws.rail));
        let gap = drag(&mut ws, title, &[history]);
        let line = bounds(&ws, ws.drop_indicator);
        let rail = bounds(&ws, ws.rail);
        assert_eq!(
            widgets::drop_indicator_state(&ws.tree, ws.drop_indicator)
                .ok()
                .and_then(|state| state.shown()),
            Some(DropIndicatorKind::Insertion)
        );
        let thickness = scales().size.indicator_width;
        assert_eq!(
            (line.x, line.width, line.height),
            (rail.x, rail.width, thickness)
        );
        assert!(!indicator_ops(&ws).is_empty(), "a line during the drag");
        let bar = bounds(&ws, crate::workspace::test_group(&ws).bar);
        let mut join = gap;
        if let Err(err) = join.update(&mut ws, centre(bar), &scales()) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        assert_eq!(
            bounds(&ws, ws.drop_indicator),
            bar,
            "outlines the tab strip"
        );
        assert_eq!(
            widgets::drop_indicator_state(&ws.tree, ws.drop_indicator)
                .ok()
                .and_then(|state| state.shown()),
            Some(DropIndicatorKind::Target)
        );
        assert!(!indicator_ops(&ws).is_empty());
        // Over the status bar: no target, no indicator (0.167.0: the
        // canvas area floats instead).
        let status = centre(bounds(&ws, ws.status_bar.root));
        if let Err(err) = join.update(&mut ws, status, &scales()) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        assert!(indicator_ops(&ws).is_empty(), "nothing over a non-target");
        assert!(!finish(&mut ws, &mut focus, join));
        assert!(indicator_ops(&ws).is_empty(), "nothing after the drag");
        assert!(
            ws.tree
                .accessibility(ws.drop_indicator)
                .is_some_and(accesskit::Node::is_hidden),
            "never announced"
        );
    }

    /// AC-5: the keyboard moves rearrange the rail, land focus on the
    /// moved panel (its tab when grouped) and keep the AccessKit rail
    /// order equal to the slots after every move; at an end they change
    /// nothing.
    #[test]
    fn keyboard_moves_keep_focus_and_accessibility_order_consistent() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        let run = |ws: &mut Workspace, focus: &mut FocusManager, panel, direction| {
            let moved = match move_workspace_panel_by(ws, focus, panel, direction, &scales) {
                Ok(moved) => moved,
                Err(err) => unreachable!("{err:?}"),
            };
            ws.tree.compute_layout(WINDOW.0, WINDOW.1);
            let roots: Vec<WidgetId> = ws.slots.iter().map(RailSlot::root).collect();
            assert_eq!(rail_children(ws), roots);
            assert_eq!(at_rail_children(ws), roots);
            moved
        };
        assert!(
            !run(&mut ws, &mut focus, DockPanel::Layers, PanelMove::Up),
            "top already"
        );
        assert_eq!(focus.focused(), None, "a refused move moves no focus");

        assert!(run(
            &mut ws,
            &mut focus,
            DockPanel::Properties,
            PanelMove::Up
        ));
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::Layers], 0),
                (vec![DockPanel::Properties], 0),
                (vec![DockPanel::History], 0)
            ]
        );
        assert_eq!(focus.focused(), Some(ws.properties.root));

        assert!(run(
            &mut ws,
            &mut focus,
            DockPanel::Layers,
            PanelMove::NextGroup
        ));
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::Properties, DockPanel::Layers], 1),
                (vec![DockPanel::History], 0)
            ]
        );
        let layers_tab = tab_of(&ws, DockPanel::Layers);
        assert_eq!(
            focus.focused(),
            Some(layers_tab),
            "on the moved panel's tab"
        );
        assert!(
            ws.tree
                .accessibility(layers_tab)
                .is_some_and(|node| node.is_selected() == Some(true)),
            "its tab is the active one"
        );

        // Focus on Layers' tab when that tab bar is rebuilt by a move of
        // another panel: it follows Layers to its new tab.
        assert!(run(
            &mut ws,
            &mut focus,
            DockPanel::History,
            PanelMove::NextGroup
        ));
        let layers_tab = tab_of(&ws, DockPanel::Layers);
        let _ = focus.focus(&mut ws.tree, layers_tab);
        assert!(run(
            &mut ws,
            &mut focus,
            DockPanel::Properties,
            PanelMove::Down
        ));
        assert_eq!(focus.focused(), Some(ws.properties.root));
        assert!(run(
            &mut ws,
            &mut focus,
            DockPanel::History,
            PanelMove::Down
        ));
        let Some(focused) = focus.focused() else {
            unreachable!("focus is kept");
        };
        assert!(ws.tree.contains(focused));
        for panel in DockPanel::ALL {
            assert_eq!(
                ws.dock_arrangement()
                    .slots()
                    .iter()
                    .flat_map(|slot| slot.panels.iter())
                    .filter(|&&p| p == panel)
                    .count(),
                1
            );
        }
    }

    /// AC-7: with the rail collapsed to its label strip no press starts a
    /// drag (the strip's buttons included), and a drag in progress when
    /// the rail collapses finds no target and its release changes
    /// nothing.
    #[test]
    fn a_collapsed_rail_disables_panel_dragging() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        let title = centre(bounds(&ws, ws.layers.header));
        let history = near_bottom(bounds(&ws, ws.rail));
        let mut live = drag(&mut ws, title, &[(title.0, title.1 + 20.0)]);
        if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(WINDOW.0, WINDOW.1);
        let before = snapshot(&ws);
        assert_eq!(PanelDrag::press(&ws, title), None, "no title is pressable");
        for (_, button) in ws.panel_strip.buttons {
            assert_eq!(PanelDrag::press(&ws, centre(bounds(&ws, button))), None);
        }
        if let Err(err) = live.update(&mut ws, history, &scales()) {
            unreachable!("{err:?}");
        }
        assert_eq!(live.target(), None);
        assert!(!finish(&mut ws, &mut focus, live));
        assert_eq!(snapshot(&ws), before);
    }

    /// Moves keep each panel's identity: the strip's buttons follow the
    /// new panel order, and every panel stays reachable.
    #[test]
    fn the_label_strip_follows_the_panel_order() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        if let Err(err) = move_workspace_panel_by(
            &mut ws,
            &mut focus,
            DockPanel::Layers,
            PanelMove::Down,
            &scales,
        ) {
            unreachable!("{err:?}");
        }
        let order: Vec<WidgetId> = [DockPanel::Properties, DockPanel::History, DockPanel::Layers]
            .into_iter()
            .filter_map(|panel| ws.panel_strip.button_for(ws.panel(panel)))
            .collect();
        assert_eq!(
            ws.tree.children(ws.panel_strip.root).map(<[_]>::to_vec),
            Some(order)
        );
    }

    // -- 0.167.0: floating panels --

    /// Lays out as the app does: layout, then the floating clamp, again
    /// while it changes something.
    fn lay(ws: &mut Workspace, window: (f32, f32)) {
        ws.tree.compute_layout(window.0, window.1);
        for _ in 0..3 {
            match crate::sync_floating_frames(ws) {
                Ok(true) => ws.tree.compute_layout(window.0, window.1),
                Ok(false) => break,
                Err(err) => unreachable!("{err:?}"),
            }
        }
    }

    /// [`drag`] then [`finish`], laying out with the floating clamp.
    fn drop_at(
        ws: &mut Workspace,
        focus: &mut FocusManager,
        from: (f32, f32),
        to: &[(f32, f32)],
    ) -> bool {
        let Some(mut live) = PanelDrag::press(ws, from) else {
            unreachable!("{from:?} is a drag source");
        };
        for &point in to {
            if let Err(err) = live.update(ws, point, &scales()) {
                unreachable!("{err:?}");
            }
            lay(ws, WINDOW);
        }
        let changed = match live.finish(ws, focus, &scales()) {
            Ok(changed) => changed,
            Err(err) => unreachable!("{err:?}"),
        };
        lay(ws, WINDOW);
        changed
    }

    #[allow(clippy::cast_precision_loss)]
    fn offset(rect: Rect, dx: f32, dy: f32) -> (f32, f32) {
        (rect.x as f32 + dx, rect.y as f32 + dy)
    }

    fn floating_shape(ws: &Workspace) -> Vec<(Vec<DockPanel>, usize)> {
        ws.dock_arrangement()
            .floating()
            .iter()
            .map(|float| (float.slot.panels.clone(), float.slot.selected))
            .collect()
    }

    /// Every panel exactly once across the rail and the floating frames,
    /// read off the tree: its root's parent is the rail, a frame, or a
    /// group root inside one of those; the canvas area holds exactly the
    /// frames; no group or frame is empty.
    fn every_panel_once(ws: &Workspace) {
        let frames: Vec<WidgetId> = ws.floating.iter().map(|float| float.frame).collect();
        assert_eq!(
            ws.tree
                .children(ws.canvas_area)
                .map(<[_]>::to_vec)
                .unwrap_or_default(),
            frames
        );
        for panel in DockPanel::ALL {
            let root = ws.panel(panel).root;
            let homes = ws
                .all_slots()
                .filter(|slot| slot.panels().iter().any(|h| h.root == root))
                .count();
            assert_eq!(homes, 1, "{panel:?} appears exactly once");
            let parent = ws.tree.parent(root);
            let home = parent.and_then(|p| {
                if p == ws.rail || frames.contains(&p) {
                    Some(p)
                } else {
                    ws.tree.parent(p)
                }
            });
            assert!(
                home == Some(ws.rail) || home.is_some_and(|h| frames.contains(&h)),
                "{panel:?} sits in the rail or a frame"
            );
            let floating = ws.dock_arrangement().is_floating(panel);
            let kind = if floating {
                aurora_widgets::widgets::WidgetKind::RaisedPanel
            } else {
                aurora_widgets::widgets::WidgetKind::Panel
            };
            assert_eq!(ws.tree.payload(root), Some(&kind), "{panel:?}");
        }
        for float in &ws.floating {
            let expected = if float.grip.is_some() { 2 } else { 1 };
            assert_eq!(
                ws.tree.children(float.frame).map(<[_]>::len),
                Some(expected)
            );
            assert_eq!(float.grip.is_some(), float.content.panels().len() > 1);
        }
    }

    /// AC-1: dragging the Layers title over the canvas shows an outline of
    /// where it will float — the float width (the rail's), as tall as the
    /// panel, the grabbed point kept under the pointer — Escape then
    /// leaves everything byte-identical, and a real drop floats it there:
    /// its root in a `RaisedPanel` frame under the canvas area, described
    /// "Floating", its strip button gone.
    #[test]
    fn a_drop_over_the_canvas_floats_and_escape_cancels() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        let before = snapshot(&ws);
        let header = bounds(&ws, ws.layers.header);
        let title = offset(header, 10.0, 5.0);
        let canvas = bounds(&ws, ws.canvas_area);
        let over = offset(canvas, 300.0, 200.0);
        let mut live = drag(&mut ws, title, &[(title.0 - 30.0, title.1), over]);
        let expected = DropTarget::Float { x: 290.0, y: 195.0 };
        assert_eq!(live.target(), Some(expected), "grab offset kept");
        let outline = bounds(&ws, ws.drop_indicator);
        assert_eq!(
            widgets::drop_indicator_state(&ws.tree, ws.drop_indicator)
                .ok()
                .and_then(|state| state.shown()),
            Some(DropIndicatorKind::Target)
        );
        let rail = bounds(&ws, ws.rail);
        assert_eq!(
            (outline.x, outline.y, outline.width),
            (canvas.x + 290, canvas.y + 195, rail.width),
            "outlined where it will float"
        );
        assert_eq!(outline.height, bounds(&ws, ws.layers.root).height);
        assert!(!indicator_ops(&ws).is_empty());
        if let Err(err) = live.update(&mut ws, over, &scales()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = live.cancel(&mut ws) {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert_eq!(snapshot(&ws), before, "Escape changes nothing");

        assert!(drop_at(
            &mut ws,
            &mut focus,
            title,
            &[(title.0 - 30.0, title.1), over]
        ));
        assert_eq!(floating_shape(&ws), vec![(vec![DockPanel::Layers], 0)]);
        assert_eq!(
            shape(&ws),
            vec![(vec![DockPanel::Properties, DockPanel::History], 0)]
        );
        let Some(float) = ws.floating.first().cloned() else {
            unreachable!("one float");
        };
        assert_eq!((float.x, float.y), (290.0, 195.0));
        assert_eq!(ws.tree.parent(float.frame), Some(ws.canvas_area));
        assert_eq!(ws.tree.parent(ws.layers.root), Some(float.frame));
        let frame = bounds(&ws, float.frame);
        assert_eq!(
            (frame.x, frame.y, frame.width),
            (canvas.x + 290, canvas.y + 195, rail.width)
        );
        assert!(
            ws.tree
                .accessibility(ws.layers.root)
                .is_some_and(|node| node.role() == accesskit::Role::Region
                    && node.description() == Some(crate::FLOATING_DESCRIPTION)
                    && node.label() == Some("Layers")
                    && !node.is_modal()),
            "a non-modal region named by its title"
        );
        let Some(button) = ws.panel_strip.button_for(ws.layers) else {
            unreachable!("Layers has a strip button");
        };
        assert!(
            ws.tree
                .accessibility(button)
                .is_some_and(accesskit::Node::is_hidden),
            "no strip button while floating"
        );
        assert!(indicator_ops(&ws).is_empty(), "nothing after the drop");
        every_panel_once(&ws);
    }

    /// AC-2: a floating title drag moves it (clamped to the canvas area);
    /// dropping its title into a rail gap re-docks it, the same drag.
    #[test]
    fn a_floating_title_drag_moves_clamped_and_redocks() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        let canvas = bounds(&ws, ws.canvas_area);
        let title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        assert!(drop_at(
            &mut ws,
            &mut focus,
            title,
            &[offset(canvas, 110.0, 105.0)]
        ));
        let header = bounds(&ws, ws.layers.header);
        let grip = offset(header, 10.0, 5.0);
        assert!(PanelDrag::press(&ws, grip).is_some_and(|d| d.moves_slot()));
        // A move of (+40, +30).
        assert!(drop_at(
            &mut ws,
            &mut focus,
            grip,
            &[(grip.0 + 40.0, grip.1 + 30.0)]
        ));
        let float = ws.floating.first().cloned();
        assert_eq!(float.as_ref().map(|f| (f.x, f.y)), Some((140.0, 130.0)));
        // Far past the bottom-right corner: clamped wholly inside.
        let grip = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let corner = offset(
            canvas,
            canvas.width as f32 - 1.0,
            canvas.height as f32 - 1.0,
        );
        assert!(drop_at(&mut ws, &mut focus, grip, &[corner]));
        let frame = bounds(&ws, ws.floating.first().map_or(ws.root, |f| f.frame));
        assert!(frame.x >= canvas.x && frame.y >= canvas.y);
        assert_eq!(
            frame.x + i64::from(frame.width),
            canvas.x + i64::from(canvas.width)
        );
        assert_eq!(
            frame.y + i64::from(frame.height),
            canvas.y + i64::from(canvas.height)
        );
        // Its title onto the rail's bottom: docked again.
        let grip = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let bottom = near_bottom(bounds(&ws, ws.rail));
        assert!(drop_at(&mut ws, &mut focus, grip, &[bottom]));
        assert!(ws.floating.is_empty());
        assert_eq!(
            shape(&ws),
            vec![
                (vec![DockPanel::Properties, DockPanel::History], 0),
                (vec![DockPanel::Layers], 0)
            ]
        );
        assert_eq!(
            crate::panel_sizing(&ws.tree, ws.layers).ok(),
            Some(crate::docked_sizing(DockPanel::Layers))
        );
        every_panel_once(&ws);
    }

    /// AC-2: joining and splitting between floating and docked groups —
    /// a docked tab torn off floats alone (its group dissolves), a docked
    /// panel dropped on the float's title joins it, a docked lone panel on
    /// the floating group's tab strip joins too, a floating group's bare
    /// strip moves the whole group, its tab dragged into the rail docks
    /// that panel, and its last tab torn off over the canvas floats on its
    /// own: every panel exactly once and no empty group, every step.
    #[test]
    fn floating_and_docked_groups_join_and_split() {
        let (mut ws, _) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        let canvas = bounds(&ws, ws.canvas_area);
        let history = centre(bounds(&ws, tab_of(&ws, DockPanel::History)));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            history,
            &[offset(canvas, 100.0, 100.0)]
        ));
        assert_eq!(floating_shape(&ws), vec![(vec![DockPanel::History], 0)]);
        assert_eq!(ws.groups().count(), 0, "the docked group dissolved");
        every_panel_once(&ws);

        // Properties' title onto the floating History's title: a floating group.
        let props = offset(bounds(&ws, ws.properties.header), 10.0, 5.0);
        let onto = centre(bounds(&ws, ws.history.header));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            props,
            &[(props.0, props.1 + 20.0), onto]
        ));
        assert_eq!(
            floating_shape(&ws),
            vec![(vec![DockPanel::History, DockPanel::Properties], 1)]
        );
        every_panel_once(&ws);

        // Layers onto the floating group's tab strip.
        let layers = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let Some(bar) = ws.groups().next().map(|group| group.bar) else {
            unreachable!("a floating group");
        };
        let strip = centre(bounds(&ws, bar));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            layers,
            &[(layers.0, layers.1 + 20.0), strip]
        ));
        assert_eq!(
            floating_shape(&ws),
            vec![(
                vec![DockPanel::History, DockPanel::Properties, DockPanel::Layers],
                2
            )]
        );
        assert!(shape(&ws).is_empty(), "the rail is empty");
        every_panel_once(&ws);

        // The group's grip above its tabs moves the whole group.
        let Some(grip) = ws.floating.first().and_then(|float| float.grip) else {
            unreachable!("a floating group has a grip");
        };
        let bare = centre(bounds(&ws, grip));
        let Some(grab) = PanelDrag::press(&ws, bare) else {
            unreachable!("the grip is a drag source");
        };
        assert!(grab.moves_slot());
        let before = ws.floating.first().map(|f| (f.x, f.y));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            bare,
            &[(bare.0 + 50.0, bare.1 + 60.0)]
        ));
        let after = ws.floating.first().map(|f| (f.x, f.y));
        assert_eq!(
            before.zip(after).map(|(b, a)| (a.0 - b.0, a.1 - b.1)),
            Some((50.0, 60.0)),
            "moved as one"
        );
        assert_eq!(floating_shape(&ws).first().map(|s| s.0.len()), Some(3));

        // A floating tab into the (empty) rail docks that panel.
        let props_tab = centre(bounds(&ws, tab_of(&ws, DockPanel::Properties)));
        let rail = centre(bounds(&ws, ws.rail));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            props_tab,
            &[(props_tab.0, props_tab.1 + 20.0), rail]
        ));
        assert_eq!(shape(&ws), vec![(vec![DockPanel::Properties], 0)]);
        assert_eq!(
            floating_shape(&ws),
            vec![(vec![DockPanel::History, DockPanel::Layers], 1)]
        );
        // The last-but-one tab torn off over the canvas: two lone floats,
        // the emptied floating group gone.
        let old_group = ws.groups().next().map(|group| group.root);
        let history_tab = centre(bounds(&ws, tab_of(&ws, DockPanel::History)));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            history_tab,
            &[
                (history_tab.0, history_tab.1 + 40.0),
                offset(canvas, 20.0, 400.0)
            ]
        ));
        assert_eq!(
            floating_shape(&ws),
            vec![(vec![DockPanel::Layers], 0), (vec![DockPanel::History], 0)]
        );
        assert_eq!(ws.groups().count(), 0);
        assert!(old_group.is_some_and(|root| !ws.tree.contains(root)));
        every_panel_once(&ws);
    }

    /// AC-3: the topmost floating panel wins a point two of them share,
    /// and raising the lower one puts it on top — the canvas area's child
    /// order, the arrangement's stacking order and the hit test all follow.
    #[test]
    fn raising_a_floating_panel_puts_it_on_top() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        for panel in [DockPanel::Layers, DockPanel::History] {
            match super::float_workspace_panel(&mut ws, &mut focus, panel, &scales) {
                Ok(true) => {}
                other => unreachable!("{other:?}"),
            }
            lay(&mut ws, WINDOW);
        }
        let [lower, upper] = [0, 1].map(|i| ws.floating.get(i).map_or(ws.root, |f| f.frame));
        let shared = offset(bounds(&ws, upper), 5.0, 5.0);
        assert!(
            super::contains(bounds(&ws, lower), shared),
            "they overlap there"
        );
        assert_eq!(crate::floating_index_at(&ws, shared), Some(1));
        // A press on the lower one's uncovered corner raises it; the
        // widget under that press is the same before and after.
        let corner = offset(bounds(&ws, lower), 2.0, 2.0);
        assert_eq!(crate::floating_index_at(&ws, corner), Some(0));
        let held = ws.tree.hit_test(corner);
        match crate::raise_floating(&mut ws, 0) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        assert_eq!(
            ws.tree.hit_test(corner),
            held,
            "raising moved nothing under the press"
        );
        lay(&mut ws, WINDOW);
        assert_eq!(
            ws.tree.children(ws.canvas_area).map(<[_]>::to_vec),
            Some(vec![upper, lower])
        );
        assert_eq!(crate::floating_index_at(&ws, shared), Some(1));
        assert!(
            ws.tree
                .hit_test(shared)
                .is_some_and(|hit| ws.tree.is_within(lower, hit))
        );
        assert_eq!(
            floating_shape(&ws),
            vec![(vec![DockPanel::History], 0), (vec![DockPanel::Layers], 0)]
        );
        assert_eq!(
            crate::raise_floating(&mut ws, 1).ok(),
            Some(false),
            "already on top"
        );
        assert_eq!(
            crate::raise_floating(&mut ws, usize::MAX).ok(),
            Some(false),
            "review J-3: no overflow"
        );
        // Review (b): the AccessKit tree after a raise lists the frames
        // under the canvas area in stacking order, and every panel root is
        // some node's child exactly once.
        let update = ws.tree.accessibility_update(ws.root);
        let children_of = |id: WidgetId| {
            update
                .nodes
                .iter()
                .find(|(node, _)| *node == id)
                .map(|(_, node)| node.children().to_vec())
        };
        assert_eq!(children_of(ws.canvas_area), Some(vec![upper, lower]));
        for panel in DockPanel::ALL {
            let root = ws.panel(panel).root;
            let parents = update
                .nodes
                .iter()
                .filter(|(_, node)| node.children().contains(&root))
                .count();
            assert_eq!(parents, 1, "{panel:?} appears exactly once");
        }
    }

    /// AC-4 at the tree level: a float clamps back inside when the window
    /// shrinks, when the rail widens (the float follows the rail's width),
    /// and stays put and usable when the rail collapses (the canvas area
    /// only grows). Its title row is inside the canvas area every time.
    #[test]
    fn floating_panels_clamp_on_resize_rail_width_and_collapse() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        match super::float_workspace_panel(&mut ws, &mut focus, DockPanel::Layers, &scales) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        lay(&mut ws, WINDOW);
        let canvas = bounds(&ws, ws.canvas_area);
        let title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let far = offset(
            canvas,
            canvas.width as f32 - 2.0,
            canvas.height as f32 - 2.0,
        );
        assert!(drop_at(&mut ws, &mut focus, title, &[far]));
        let inside = |ws: &Workspace| {
            let canvas = bounds(ws, ws.canvas_area);
            let frame = bounds(ws, ws.floating.first().map_or(ws.root, |f| f.frame));
            let header = bounds(ws, ws.layers.header);
            frame.x >= canvas.x
                && frame.y >= canvas.y
                && frame.x + i64::from(frame.width) <= canvas.x + i64::from(canvas.width)
                && frame.y + i64::from(frame.height) <= canvas.y + i64::from(canvas.height)
                && header.y >= canvas.y
                && header.y + i64::from(header.height) <= canvas.y + i64::from(canvas.height)
        };
        assert!(inside(&ws));
        lay(&mut ws, (700.0, 450.0));
        assert!(inside(&ws), "window shrink");
        if let Err(err) = crate::set_rail_width(&mut ws.tree, ws.rail, ws.divider, 400.0) {
            unreachable!("{err:?}");
        }
        lay(&mut ws, (700.0, 450.0));
        assert!(inside(&ws), "rail widened");
        let canvas_w = bounds(&ws, ws.canvas_area).width;
        assert_eq!(
            bounds(&ws, ws.floating.first().map_or(ws.root, |f| f.frame)).width,
            400.min(canvas_w),
            "the rail's width, capped at the canvas area's"
        );
        if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        lay(&mut ws, (700.0, 450.0));
        assert!(inside(&ws), "rail collapsed");
        assert!(bounds(&ws, ws.layers.root).height > 0, "still shown");
        lay(&mut ws, (40.0, 30.0));
        let frame = bounds(&ws, ws.floating.first().map_or(ws.root, |f| f.frame));
        let canvas = bounds(&ws, ws.canvas_area);
        assert!(
            frame.x >= canvas.x && frame.y >= canvas.y,
            "a tiny window pins it at the origin"
        );
    }

    /// AC-5: a floating panel is a `Tab` stop between the options bar and
    /// the rail (its frame is a canvas-area child), the AccessKit tree
    /// lists its frame under the canvas area, and Float/Dock land focus on
    /// the panel; Dock expands a collapsed rail first.
    #[test]
    fn floating_panels_are_tab_stops_and_float_and_dock_move_focus() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        let _ = focus.focus(&mut ws.tree, ws.properties.root);
        match super::float_workspace_panel(&mut ws, &mut focus, DockPanel::History, &scales) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        lay(&mut ws, WINDOW);
        assert_eq!(
            focus.focused(),
            Some(ws.history.root),
            "focus followed the float"
        );
        assert_eq!(
            crate::panel_sizing(&ws.tree, ws.history).ok(),
            Some(crate::PanelSizing::Content),
            "a floating panel is content-sized (History's docked Fill would shrink to one row)"
        );
        let mut order = Vec::new();
        let mut probe = FocusManager::new();
        for _ in 0..200 {
            match probe.focus_next(&mut ws.tree) {
                Some(id) if !order.contains(&id) => order.push(id),
                _ => break,
            }
        }
        let at = |id| order.iter().position(|&o| o == id);
        assert!(at(ws.history.root).is_some(), "reachable by Tab");
        assert!(at(ws.history.root) < at(ws.layers.root), "before the rail");
        let update = ws.tree.accessibility_update(ws.root);
        let canvas_children = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ws.canvas_area)
            .map(|(_, node)| node.children().to_vec());
        assert_eq!(
            canvas_children,
            Some(ws.floating.iter().map(|f| f.frame).collect::<Vec<_>>())
        );
        assert_eq!(
            super::float_workspace_panel(&mut ws, &mut focus, DockPanel::History, &scales).ok(),
            Some(false),
            "already floating"
        );
        if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        match super::dock_workspace_panel(&mut ws, &mut focus, DockPanel::History, &scales) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        lay(&mut ws, WINDOW);
        assert!(!crate::rail_collapsed(&ws), "Dock expands the rail");
        assert_eq!(
            crate::panel_sizing(&ws.tree, ws.history).ok(),
            Some(crate::docked_sizing(DockPanel::History)),
            "back to its docked sizing"
        );
        assert_eq!(focus.focused(), Some(ws.history.root));
        assert_eq!(
            shape(&ws).last(),
            Some(&(vec![DockPanel::History], 0)),
            "docked at the rail's end"
        );
        assert_eq!(
            super::dock_workspace_panel(&mut ws, &mut focus, DockPanel::History, &scales).ok(),
            Some(false),
            "already docked"
        );
        every_panel_once(&ws);
    }

    /// AC-7: with the rail collapsed a floating panel stays laid out, its
    /// title still drags it (and docked titles still do not), it has no
    /// strip button (hidden, AT-hidden, not a `Tab` stop) while the docked
    /// panels keep theirs, and a drag of it over the strip finds nothing.
    #[test]
    fn a_collapsed_rail_keeps_floating_panels_usable_without_a_strip_button() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        match super::float_workspace_panel(&mut ws, &mut focus, DockPanel::Layers, &scales) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert!(bounds(&ws, ws.layers.root).height > 0);
        let Some(button) = ws.panel_strip.button_for(ws.layers) else {
            unreachable!("a strip button");
        };
        assert_eq!(
            bounds(&ws, button).height,
            0,
            "no button for a floating panel"
        );
        assert!(
            ws.tree
                .accessibility(button)
                .is_some_and(accesskit::Node::is_hidden)
        );
        for docked in [ws.properties, ws.history] {
            let Some(other) = ws.panel_strip.button_for(docked) else {
                unreachable!("a strip button");
            };
            assert!(bounds(&ws, other).height > 0, "docked panels keep theirs");
        }
        let mut probe = FocusManager::new();
        let mut seen = Vec::new();
        for _ in 0..200 {
            match probe.focus_next(&mut ws.tree) {
                Some(id) if !seen.contains(&id) => seen.push(id),
                _ => break,
            }
        }
        assert!(!seen.contains(&button));
        assert!(
            seen.contains(&ws.layers.root),
            "the floating panel is a Tab stop"
        );
        let title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let before = ws.floating.first().map(|f| (f.x, f.y));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            title,
            &[(title.0 + 30.0, title.1 + 20.0)]
        ));
        let after = ws.floating.first().map(|f| (f.x, f.y));
        assert_ne!(before, after, "it still moves");
        let strip = centre(bounds(&ws, ws.panel_strip.root));
        let title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let Some(mut live) = PanelDrag::press(&ws, title) else {
            unreachable!("a floating title is a source");
        };
        if let Err(err) = live.update(&mut ws, strip, &scales) {
            unreachable!("{err:?}");
        }
        assert_eq!(live.target(), None, "the strip is no target");
        let _ = live.cancel(&mut ws);
    }

    /// Review J-1: closing a floating lone panel hides its frame (no
    /// invisible band over the canvas: no floating hit, no drag source),
    /// focus leaves it, and reopening shows it again at the same place.
    /// A floating group's frame stays while one tab is open and hides
    /// (grip included) once both are closed; reopening restores it.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn closing_a_floating_panel_hides_its_frame_and_reopening_restores_it() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        let canvas = bounds(&ws, ws.canvas_area);
        let title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        assert!(drop_at(
            &mut ws,
            &mut focus,
            title,
            &[offset(canvas, 300.0, 200.0)]
        ));
        let Some(float) = ws.floating.first().cloned() else {
            unreachable!("one float");
        };
        let rect = bounds(&ws, float.frame);
        let inside = centre(rect);
        let float_title = offset(bounds(&ws, ws.layers.header), 10.0, 5.0);
        let _ = focus.focus(&mut ws.tree, ws.layers.root);
        if let Err(err) = {
            let handle = ws.layers;
            crate::close_workspace_panel(&mut ws, handle)
        } {
            unreachable!("{err:?}");
        }
        crate::refocus_workspace(&mut ws, &mut focus);
        lay(&mut ws, WINDOW);
        assert!(!crate::workspace::float_frame_shown(&ws.tree, float.frame));
        assert!(
            ws.tree
                .accessibility(float.frame)
                .is_some_and(accesskit::Node::is_hidden)
        );
        assert_eq!(
            crate::floating_index_at(&ws, inside),
            None,
            "no band left over the canvas"
        );
        assert_eq!(
            PanelDrag::press(&ws, float_title),
            None,
            "no drag from a hidden frame"
        );
        assert_ne!(
            focus.focused(),
            Some(ws.layers.root),
            "focus left the hidden panel"
        );
        if let Err(err) = {
            let handle = ws.layers;
            crate::toggle_workspace_panel(&mut ws, handle)
        } {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert!(crate::workspace::float_frame_shown(&ws.tree, float.frame));
        let back = bounds(&ws, float.frame);
        assert_eq!((back.x, back.y), (rect.x, rect.y), "reopened where it was");
        assert_eq!(
            ws.floating.first().map(|f| (f.x, f.y)),
            Some((float.x, float.y))
        );

        // A floating group: History floats, Properties joins it.
        let history = centre(bounds(&ws, tab_of(&ws, DockPanel::History)));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            history,
            &[offset(canvas, 40.0, 40.0)]
        ));
        let props = offset(bounds(&ws, ws.properties.header), 10.0, 5.0);
        let onto = centre(bounds(&ws, ws.history.header));
        assert!(drop_at(
            &mut ws,
            &mut focus,
            props,
            &[(props.0, props.1 + 20.0), onto]
        ));
        let Some(group) = ws.floating.iter().find(|f| f.grip.is_some()).cloned() else {
            unreachable!("a floating group");
        };
        let group_rect = bounds(&ws, group.frame);
        let grip = centre(bounds(&ws, group.grip.unwrap_or(ws.root)));
        if let Err(err) = {
            let handle = ws.properties;
            crate::close_workspace_panel(&mut ws, handle)
        } {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert!(
            crate::workspace::float_frame_shown(&ws.tree, group.frame),
            "one tab still open"
        );
        if let Err(err) = {
            let handle = ws.history;
            crate::close_workspace_panel(&mut ws, handle)
        } {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert!(
            !crate::workspace::float_frame_shown(&ws.tree, group.frame),
            "nothing open"
        );
        assert_eq!(
            PanelDrag::press(&ws, grip),
            None,
            "the grip is no handle when hidden"
        );
        assert_eq!(crate::floating_index_at(&ws, grip), None);
        if let Err(err) = {
            let handle = ws.history;
            crate::toggle_workspace_panel(&mut ws, handle)
        } {
            unreachable!("{err:?}");
        }
        lay(&mut ws, WINDOW);
        assert!(crate::workspace::float_frame_shown(&ws.tree, group.frame));
        let back = bounds(&ws, group.frame);
        assert_eq!((back.x, back.y), (group_rect.x, group_rect.y));
        let _ = scales;
        every_panel_once(&ws);
    }

    /// Review (a): showing a floating panel from the strip's action never
    /// expands a collapsed rail (the panel is not in it).
    #[test]
    fn showing_a_floating_panel_leaves_a_collapsed_rail_collapsed() {
        let (mut ws, scales) = workspace();
        let mut focus = FocusManager::new();
        lay(&mut ws, WINDOW);
        match super::float_workspace_panel(&mut ws, &mut focus, DockPanel::Layers, &scales) {
            Ok(true) => {}
            other => unreachable!("{other:?}"),
        }
        if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.layers, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            {
                let handle = ws.layers;
                crate::expand_rail_showing(&mut ws, handle)
            }
            .ok(),
            Some(true)
        );
        assert!(crate::rail_collapsed(&ws), "the rail stays collapsed");
        assert_eq!(
            crate::panel_is_collapsed(&ws.tree, ws.layers).ok(),
            Some(false)
        );
        // A docked panel still expands it.
        assert_eq!(
            {
                let handle = ws.history;
                crate::expand_rail_showing(&mut ws, handle)
            }
            .ok(),
            Some(true)
        );
        assert!(!crate::rail_collapsed(&ws));
    }
}
