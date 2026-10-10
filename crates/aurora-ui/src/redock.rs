//! Drag-to-redock (0.166.0, workspace round 5): applying a
//! [`DockArrangement`] to the workspace's tree, the move operations the
//! pointer drag and the keyboard commands share, and the pointer drag's
//! own state machine ([`PanelDrag`]). Photoshop's dock convention, not
//! its look.
//!
//! **Applying an arrangement moves panels, never rebuilds them.** Each
//! panel's whole subtree — its content, ids, scroll offset, sizing and
//! collapsed or closed state — is reparented with
//! `WidgetTree::move_child`; only the tab groups' own roots and tab bars
//! are rebuilt. So every handle `aurora-app` holds into a panel (layer
//! rows, the Layers controls, the Curves editor, History rows) stays
//! valid across any move, and nothing needs repopulating.
//!
//! **A drag** starts on a lone panel's title row or on a group's tab
//! ([`panel_drag_source`]). Below [`PANEL_DRAG_THRESHOLD`] of travel it is
//! still a click — whatever the press already did (a tab switch) stands
//! and the release changes nothing. Past it, every move picks a drop
//! target ([`drop_target_at`]) and shows the drop indicator there: a
//! line in the gap between two slots, or an outline round a slot's title
//! row or tab strip to join it as a tab. A drop on a target moves the
//! panel ([`move_workspace_panel`]); a drop anywhere else, `Escape`, or a
//! collapsed rail cancels, leaving the arrangement exactly as it was.
//!
//! **The collapsed rail (0.165.0's label strip) disables dragging.** No
//! drag can start while the rail is collapsed (no title row or tab is
//! shown; the strip's buttons are not drag sources), and a drag in
//! progress when the rail collapses finds no target and cancels. The
//! keyboard commands still rearrange a collapsed rail's slots (the
//! arrangement is data; the strip's button order follows it).
//!
//! **Not document state.** Nothing here touches `aurora_doc::History`:
//! a layout move is not undoable, the same as a rail resize or a panel
//! collapse.

use aurora_core::Rect;
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, DropIndicatorKind};
use aurora_widgets::{FocusManager, WidgetError};

use crate::dock::{DockArrangement, DockPanel, DropTarget, PanelMove};
use crate::panel::{PanelHandle, set_panel_collapsed, set_panel_grouped};
use crate::panel_group::{build_panel_group, set_panel_group_collapsed, show_panel_group_tab};
use crate::panel_is_collapsed;
use crate::workspace::{RailSlot, Workspace, panel_focus_target, rail_collapsed};

/// How far, in logical px, a press on a panel's title or tab must travel
/// before it becomes a panel drag rather than a click. An interaction
/// tolerance, not a style value (the same footing as `aurora-app`'s
/// `RAIL_DIVIDER_HIT_TOLERANCE`): invariant §7.3.10 governs what a widget
/// draws, and no design token names a drag distance. 4 px matches the
/// common desktop default (Windows' `SM_CXDRAG`). **Logical px**: every
/// point handed to [`PanelDrag`] is window-logical (`aurora-app` divides
/// each physical cursor position by the window's scale factor first), so
/// the threshold is 4 pt on a Retina display, 8 physical px.
pub const PANEL_DRAG_THRESHOLD: f32 = 4.0;

/// Rearranges the rail to `arrangement` (repaired first, so every panel
/// appears exactly once — [`DockArrangement::repaired`]). A no-op
/// (`Ok(false)`) when it already is. Every group is dissolved back into
/// lone panels, then each slot is rebuilt in order: a lone panel's root
/// moved into place, a group built round its members
/// (`crate::panel_group`'s `build_panel_group`) and collapsed or expanded
/// as its selected member was, so a group always collapses as one. The
/// label strip's buttons follow the new panel order. The caller repairs
/// focus ([`crate::refocus_workspace`], or use
/// [`apply_dock_arrangement_keeping_focus`]) and re-runs layout.
///
/// **All-or-nothing in practice (review I3).** Every id the rebuild
/// touches — the rail, the strip, each panel's root, title slot, body and
/// viewport, each current group's root — is checked to exist *before*
/// anything moves, and the arrangement is repaired first (so every slot
/// is non-empty with an in-range tab, which is all `build_panel_group`
/// can refuse). After that check nothing in the rebuild can fail: every
/// step addresses an id just checked or just created. So an `Err` means
/// the precheck refused and the tree is untouched. The tree is not
/// transactional, so this rests on that argument, not on a rollback.
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
    let raw = arrangement
        .slots()
        .iter()
        .map(|slot| {
            (
                slot.panels.iter().copied().map(Some).collect(),
                slot.selected,
            )
        })
        .collect();
    let (arrangement, _) = DockArrangement::repaired(raw);
    if workspace.dock_arrangement() == arrangement {
        return Ok(false);
    }
    let rail = workspace.rail;
    let mut needed = vec![rail, workspace.panel_strip.root];
    for panel in DockPanel::ALL {
        let handle = workspace.panel(panel);
        needed.extend([handle.root, handle.header, handle.body, handle.viewport]);
    }
    needed.extend(workspace.groups().flat_map(|group| [group.root, group.bar]));
    if let Some(&missing) = needed.iter().find(|&&id| !workspace.tree.contains(id)) {
        return Err(WidgetError::UnknownWidget(missing));
    }
    for slot in std::mem::take(&mut workspace.slots) {
        let RailSlot::Group(group) = slot else {
            continue;
        };
        for member in &group.members {
            workspace.tree.move_child(member.root, rail, usize::MAX)?;
            set_panel_grouped(&mut workspace.tree, *member, false, scales)?;
        }
        workspace.tree.remove(group.root)?;
    }
    for (index, slot) in arrangement.slots().iter().enumerate() {
        let members: Vec<(PanelHandle, &str)> = slot
            .panels
            .iter()
            .map(|&panel| (workspace.panel(panel), panel.title()))
            .collect();
        if let [(panel, _)] = members.as_slice() {
            workspace.tree.move_child(panel.root, rail, index)?;
            workspace.slots.push(RailSlot::Panel(*panel));
            continue;
        }
        let collapsed = match slot.selected_panel() {
            Some(panel) => panel_is_collapsed(&workspace.tree, workspace.panel(panel))?,
            None => false,
        };
        let group = build_panel_group(
            &mut workspace.tree,
            rail,
            index,
            &members,
            slot.selected,
            scales,
        )?;
        set_panel_group_collapsed(&mut workspace.tree, &group, collapsed)?;
        workspace.slots.push(RailSlot::Group(group));
    }
    sync_strip_order(workspace, &arrangement)?;
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

/// Orders the label strip's buttons as the panels now stand in the rail.
fn sync_strip_order(
    workspace: &mut Workspace,
    arrangement: &DockArrangement,
) -> Result<(), WidgetError> {
    let order = arrangement
        .slots()
        .iter()
        .flat_map(|slot| slot.panels.iter().copied());
    for (index, panel) in order.enumerate() {
        if let Some(button) = workspace.panel_strip.button_for(workspace.panel(panel)) {
            workspace
                .tree
                .move_child(button, workspace.panel_strip.root, index)?;
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

/// A keyboard move of `panel` ([`PanelMove`], the command path):
/// [`move_workspace_panel`] to [`DockArrangement::move_target`], then
/// focus on the moved panel ([`panel_focus_target`]) so a keyboard or
/// screen-reader user lands where it went — unless the rail is collapsed,
/// when focus is left to the strip. Returns `Ok(false)` at an end (a lone
/// top panel moved up, say), changing nothing.
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

/// The panel a primary press at `point` would drag: a lone panel's title
/// row, or one of a group's tabs (that tab's panel). `None` anywhere
/// else, and always while the rail is collapsed.
#[must_use]
pub fn panel_drag_source(workspace: &Workspace, point: (f32, f32)) -> Option<DockPanel> {
    if rail_collapsed(workspace) {
        return None;
    }
    for slot in &workspace.slots {
        match slot {
            RailSlot::Panel(panel) => {
                if workspace
                    .tree
                    .bounds(panel.header)
                    .is_some_and(|zone| contains(zone, point))
                {
                    return workspace.dock_panel(*panel);
                }
            }
            RailSlot::Group(group) => {
                let Ok(state) = widgets::tab_bar_state(&workspace.tree, group.bar) else {
                    continue;
                };
                for (index, &tab) in state.tabs().iter().enumerate() {
                    if workspace
                        .tree
                        .bounds(tab)
                        .is_some_and(|zone| contains(zone, point))
                    {
                        return group
                            .members
                            .get(index)
                            .and_then(|&member| workspace.dock_panel(member));
                    }
                }
            }
        }
    }
    None
}

/// Where `panel` would land if dropped at `point`, from the last layout:
/// `None` outside the rail (the canvas, the status bar, the tools panel
/// and the strip are never targets), while the rail is collapsed, and
/// where the drop would change nothing ([`DockArrangement::moved`]). On
/// a slot's title zone (a lone panel's title row, a group's tab strip) it
/// joins that slot — except in the zone's top quarter, which is the gap
/// above it; elsewhere in a slot's upper half it goes into the gap above
/// it, in its lower half the gap below; below every slot, the last gap.
/// Zero-height slots (a closed panel, a group whose members are all
/// closed) are skipped, so they are never a join target and their gaps
/// are reached through their neighbours.
#[must_use]
pub fn drop_target_at(
    workspace: &Workspace,
    panel: DockPanel,
    point: (f32, f32),
) -> Option<DropTarget> {
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
    workspace
        .dock_arrangement()
        .moved(panel, target)
        .map(|_| target)
}

/// Where the drop indicator goes for `target`, in window-logical px (the
/// indicator is a root child, so that is also relative to its parent):
/// a gap is a `size.indicator_width`-thick line (the 0.166.0 token) across the
/// rail at the boundary between the two slots, kept inside the rail; a
/// join outlines the slot's title zone.
#[must_use]
pub fn drop_indicator_rect(
    workspace: &Workspace,
    target: DropTarget,
    scales: &Scales,
) -> Option<(Rect, DropIndicatorKind)> {
    let rail = workspace.tree.bounds(workspace.rail)?;
    match target {
        DropTarget::Join(index) => {
            let zone = title_zone(workspace, workspace.slots.get(index)?)?;
            Some((zone, DropIndicatorKind::Target))
        }
        DropTarget::Gap(index) => {
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
/// (0.166.0). `aurora-app` holds one from the press to the release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelDrag {
    panel: DockPanel,
    origin: (f32, f32),
    active: bool,
    target: Option<DropTarget>,
}

impl PanelDrag {
    /// A press at `point`: `Some` when it lands on a drag source
    /// ([`panel_drag_source`]). Changes nothing — the press's own effect
    /// (a tab switch) is the caller's ordinary routing.
    #[must_use]
    pub fn press(workspace: &Workspace, point: (f32, f32)) -> Option<Self> {
        panel_drag_source(workspace, point).map(|panel| Self {
            panel,
            origin: point,
            active: false,
            target: None,
        })
    }

    /// The pressed panel.
    #[must_use]
    pub fn panel(&self) -> DockPanel {
        self.panel
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
        self.target = drop_target_at(workspace, self.panel, point);
        let placed = self
            .target
            .and_then(|target| drop_indicator_rect(workspace, target, scales));
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
    /// target — moves the panel there ([`move_workspace_panel`]). A click
    /// below the threshold, or a drop on no target, changes nothing else.
    /// Returns whether the arrangement changed.
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
            Some(target) if self.active && !rail_collapsed(workspace) => {
                move_workspace_panel(workspace, focus, self.panel, target, scales)
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

    /// AC-3: `Escape` (a cancel) mid-drag, a drop over the canvas, and a
    /// drop over the status bar each leave the whole tree byte-identical.
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
            centre(bounds(&ws, ws.canvas_area)),
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
        // Over the canvas: no target, no indicator.
        let canvas = centre(bounds(&ws, ws.canvas_area));
        if let Err(err) = join.update(&mut ws, canvas, &scales()) {
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
}
