//! The main workspace layout: canvas area + a side rail of docked
//! panels, matching the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). PLAN.md M1.8's docking/panels
//! bullet, first slice.
//!
//! **Static only** — see [`crate::panel`]'s own doc comment for what's
//! deliberately not here yet (drag-to-redock, resize, persisted
//! layouts). The menubar, toolbar, and status bar the mockup also shows
//! are left out of this pass too: they belong to other, separate M1.8
//! bullets (native menus, tools, general chrome), not the docking/panel
//! structure this one is about.
//!
//! **No pixel rendering** — same "logical model now, painting later"
//! boundary every widget in `aurora-widgets` already keeps (blocked on
//! `aurora-vector`). This produces a real [`WidgetTree`] with real
//! layout (once [`WidgetTree::compute_layout`] runs) and a real
//! accessibility tree; nothing here draws a pixel.

use accesskit::{Node, Role};
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetId, WidgetTree};
use taffy::{FlexDirection, Style};

use crate::panel::{PanelHandle, insert_panel};

/// The main workspace: a canvas area and a side rail holding the
/// Layers/Properties/History panels the approved mockup shows.
#[derive(Debug)]
pub struct Workspace {
    pub tree: WidgetTree<WidgetKind>,
    pub root: WidgetId,
    /// Where the document canvas will render — `Canvas: infinite zoom,
    /// rotation, pan, ...` is a separate, still-open M1.8 bullet; this
    /// is an empty container reserving its place in the layout.
    pub canvas_area: WidgetId,
    /// The side rail — a fixed dock area holding the three panels
    /// below, stacked. No resize/redock yet (see this module's own doc
    /// comment).
    pub rail: WidgetId,
    pub layers: PanelHandle,
    pub properties: PanelHandle,
    pub history: PanelHandle,
}

/// Builds a fresh workspace: root (row) → canvas area + rail (column,
/// three stacked panels). Infallible — every parent id used here is one
/// this function just created in its own, brand-new tree, so
/// `WidgetError::UnknownWidget` is structurally unreachable (the same
/// "can't fail against ids of its own making" shape
/// `aurora_widgets::widgets::new_tree` itself already has).
#[must_use]
pub fn build_workspace() -> Workspace {
    // Canvas area gets the lion's share of the width; the rail gets a
    // fixed share of what's left. Flex *ratios*, not an absolute pixel
    // width -- `design/tokens/scales.toml` has no "dock region width"
    // token (only type/spacing/radius/elevation/motion, which govern
    // widget chrome, not workspace-level layout regions), and inventing
    // one ad hoc is a design decision to raise, not a gap to fill
    // locally (CLAUDE.md). A real, resizable dock width is exactly the
    // "docking" half of this bullet that's still open.
    const CANVAS_FLEX: f32 = 3.0;
    const RAIL_FLEX: f32 = 1.0;

    let (mut tree, root) = widgets::new_tree(Style {
        flex_direction: FlexDirection::Row,
        size: taffy::Size {
            width: taffy::style_helpers::percent(1.0_f32),
            height: taffy::style_helpers::percent(1.0_f32),
        },
        ..Default::default()
    });

    // `new_tree`'s own default root role is `Role::GenericContainer` --
    // right for a nested/internal container, but this tree's root *is*
    // the whole application window's content, and a `GenericContainer`
    // there means the tree never anchors into the native window's own
    // accessibility hierarchy at all: confirmed on real macOS hardware
    // -- VoiceOver's Rotor ("Window Spots") came back completely empty,
    // not even showing the window's own title, where the same check
    // against `spike/a11y-ime`'s `Role::Window` root correctly listed
    // both the title and a labeled field. `Role::Window` here matches
    // that proven configuration.
    let mut window_node = Node::new(Role::Window);
    window_node.set_label("Aurora");
    if let Err(err) = tree.set_accessibility(root, window_node) {
        unreachable!("root was just created by new_tree above: {err:?}");
    }

    let canvas_area = match widgets::insert_container(
        &mut tree,
        root,
        Style {
            flex_grow: CANVAS_FLEX,
            ..Default::default()
        },
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };

    let rail = match widgets::insert_container(
        &mut tree,
        root,
        Style {
            flex_direction: FlexDirection::Column,
            flex_grow: RAIL_FLEX,
            ..Default::default()
        },
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };

    let layers = match insert_panel(&mut tree, rail, "Layers") {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };
    let properties = match insert_panel(&mut tree, rail, "Properties") {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };
    let history = match insert_panel(&mut tree, rail, "History") {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };

    Workspace {
        tree,
        root,
        canvas_area,
        rail,
        layers,
        properties,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::build_workspace;

    /// Real bug, found on real macOS hardware: a `Role::GenericContainer`
    /// root never anchored into the native window's own accessibility
    /// hierarchy at all (`VoiceOver`'s Rotor came back completely empty,
    /// not even the window title) -- `Role::Window`, matching
    /// `spike/a11y-ime`'s own proven root, is what actually fixed it.
    #[test]
    fn build_workspace_roots_the_tree_as_a_labeled_window() {
        let ws = build_workspace();
        let Some(accessibility) = ws.tree.accessibility(ws.root) else {
            unreachable!("just built");
        };
        assert_eq!(accessibility.role(), accesskit::Role::Window);
        assert_eq!(accessibility.label(), Some("Aurora"));
    }

    #[test]
    fn build_workspace_has_a_canvas_area_and_three_docked_panels() {
        let mut ws = build_workspace();
        assert_eq!(ws.tree.parent(ws.canvas_area), Some(ws.root));
        assert_eq!(ws.tree.parent(ws.rail), Some(ws.root));
        assert_eq!(
            ws.tree.children(ws.rail),
            Some([ws.layers.root, ws.properties.root, ws.history.root].as_slice()),
            "panels must be docked in the rail in mockup order: Layers, Properties, History"
        );

        for (panel, title) in [
            (ws.layers, "Layers"),
            (ws.properties, "Properties"),
            (ws.history, "History"),
        ] {
            let Some(accessibility) = ws.tree.accessibility(panel.root) else {
                unreachable!("just inserted");
            };
            assert_eq!(accessibility.role(), accesskit::Role::Region);
            assert_eq!(accessibility.label(), Some(title));
        }

        // A real, computed layout -- not just tree shape. A 1000x800
        // viewport with a 3:1 canvas:rail flex ratio splits width
        // 750/250; height (no explicit size) fills via the parent's own
        // 100% root, and each of the 3 stacked panels shares the rail's
        // height equally (flex_grow: 1.0 each).
        ws.tree.compute_layout(1000.0, 800.0);
        let Some(canvas_bounds) = ws.tree.bounds(ws.canvas_area) else {
            unreachable!("just laid out");
        };
        let Some(rail_bounds) = ws.tree.bounds(ws.rail) else {
            unreachable!("just laid out");
        };
        assert_eq!(canvas_bounds.width, 750);
        assert_eq!(rail_bounds.width, 250);
        assert_eq!(canvas_bounds.height, 800);
        assert_eq!(rail_bounds.height, 800);

        let Some(layers_bounds) = ws.tree.bounds(ws.layers.root) else {
            unreachable!("just laid out");
        };
        let Some(history_bounds) = ws.tree.bounds(ws.history.root) else {
            unreachable!("just laid out");
        };
        assert!(
            layers_bounds.height > 0 && layers_bounds.height == history_bounds.height,
            "the three panels must share the rail's height equally: {layers_bounds:?} vs {history_bounds:?}"
        );
    }
}
