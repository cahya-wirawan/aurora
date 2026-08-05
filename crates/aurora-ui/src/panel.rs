//! A docked panel: a titled region of the workspace — "Layers",
//! "Properties", "History" in the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). PLAN.md M1.8's docking/panels
//! bullet, first slice.
//!
//! **Static only.** A panel here is a labeled region with a body to put
//! content in — there is no drag-to-redock, resize, collapse, close, or
//! floating yet, and no persisted workspace layout. Those are the
//! actual "docking" and "custom workspaces" half of that bullet,
//! deliberately left open: each needs real interaction/drag-state
//! machinery this first pass doesn't build. What exists here is the
//! structural piece everything else will attach to.

use accesskit::{Action, Node, Role};
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::Style;

/// One inserted panel's own widget ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelHandle {
    /// The panel's own root — a labeled `Role::Region`, the accessible
    /// name a screen reader announces for the whole panel.
    pub root: WidgetId,
    /// Where a caller adds this panel's real content once it exists
    /// (layer rows, property fields, history entries) — currently
    /// always empty.
    pub body: WidgetId,
}

/// Adds a new, empty, titled panel as the last child of `parent`.
///
/// `Role::Region` (not `Role::GenericContainer`) — the ARIA concept of
/// a perceivable, nameable section a user would want to navigate
/// directly to, which is exactly what a docked panel is. Carries
/// `Action::Focus` so it's a real `Tab` stop
/// (`aurora_widgets::FocusManager`) — real content *within* a panel
/// (individual layer/history rows) isn't focusable yet, matching this
/// module's own "static skeleton" scope; landing on the panel itself is
/// the first real, honest keyboard-navigation target that exists.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_panel(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    title: impl Into<String>,
) -> Result<PanelHandle, WidgetError> {
    let mut root_node = Node::new(Role::Region);
    root_node.set_label(title.into());
    root_node.add_action(Action::Focus);
    let root = tree.insert(
        parent,
        Style {
            flex_direction: taffy::FlexDirection::Column,
            flex_grow: 1.0,
            ..Default::default()
        },
        root_node,
        WidgetKind::Container,
    )?;
    let body = widgets::insert_container(tree, root, Style::default())?;
    Ok(PanelHandle { root, body })
}

#[cfg(test)]
mod tests {
    use super::insert_panel;
    use aurora_widgets::WidgetError;
    use aurora_widgets::widgets::{self, WidgetKind};
    use taffy::Style;

    #[test]
    fn insert_panel_adds_a_labeled_region_with_an_empty_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(accessibility) = tree.accessibility(panel.root) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::Region);
        assert_eq!(accessibility.label(), Some("Layers"));
        assert!(accessibility.supports_action(accesskit::Action::Focus));
        assert_eq!(tree.payload(panel.root), Some(&WidgetKind::Container));
        assert_eq!(tree.children(panel.body), Some([].as_slice()));
        assert_eq!(tree.parent(panel.body), Some(panel.root));
    }

    #[test]
    fn insert_panel_rejects_an_unknown_parent() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match insert_panel(&mut tree, bogus, "Layers") {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
