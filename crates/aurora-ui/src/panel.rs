//! A docked panel: a titled region of the workspace — "Layers",
//! "Properties", "History" in the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). PLAN.md M1.8's docking/panels
//! bullet, first slice.
//!
//! A panel here is a labeled region with a body to put content in, a
//! real painted background (`aurora_widgets::widgets::WidgetKind::
//! Panel`, `surface.panel` — `aurora_widgets::paint`'s own
//! `paint_panel`, not just an unpainted `Container` like most of this
//! workspace's chrome still is), and real interactivity:
//! [`set_panel_collapsed`]/[`close_panel`] (this module), plus resize
//! (`aurora_ui::workspace::set_rail_width`, the rail's own width, not
//! per-panel) and cross-session persistence (`aurora-app`'s own
//! `save_workspace_layout`/`load_workspace_layout`) landed as separate,
//! later work. `panel.root` itself is never removed from the tree —
//! only its own docked *slot*, `Workspace`'s own `layers`/`properties`/
//! `history` fields, would need to become optional for that, a real,
//! separate architecture decision deliberately not made
//! ([`close_panel`]'s own doc comment). Still genuinely open: drag-to-
//! redock and floating panels — both need real interaction/drag-state
//! machinery this crate doesn't build yet.

use accesskit::{Action, Node, Role};
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::{Display, Style};

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

/// Adds a new, empty, titled panel as the last child of `parent`,
/// initially expanded (not collapsed — see [`set_panel_collapsed`]).
///
/// `Role::Region` (not `Role::GenericContainer`) — the ARIA concept of
/// a perceivable, nameable section a user would want to navigate
/// directly to, which is exactly what a docked panel is. Carries
/// `Action::Focus` so it's a real `Tab` stop
/// (`aurora_widgets::FocusManager`) — real content *within* a panel
/// (individual layer/history rows) isn't focusable yet, matching this
/// module's own "static skeleton" scope; landing on the panel itself is
/// the first real, honest keyboard-navigation target that exists.
/// `Action::Collapse` and `Node::set_expanded(true)` mark it as a real
/// disclosure region from the moment it exists, not only once
/// [`set_panel_collapsed`] is first called. `WidgetKind::Panel` (not
/// `Container`) gives the root a real painted background — see this
/// module's own doc comment.
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
    root_node.add_action(Action::Collapse);
    root_node.set_expanded(true);
    let root = tree.insert(parent, root_style(false), root_node, WidgetKind::Panel)?;
    let body = widgets::insert_container(tree, root, Style::default())?;
    Ok(PanelHandle { root, body })
}

/// A panel's own root style — `Column` (the body stacks under the
/// header once one exists), `flex_grow` the one thing
/// [`set_panel_collapsed`] actually toggles. `1.0` (share the rail's
/// height with its siblings, same as every other docked panel) while
/// expanded; `0.0` while collapsed, so it stops claiming a share at
/// all and its siblings' own `flex_grow: 1.0` absorbs the space it
/// gives up — the ordinary flexbox behaviour every sibling already
/// has, not a special case.
fn root_style(collapsed: bool) -> Style {
    Style {
        flex_direction: taffy::FlexDirection::Column,
        flex_grow: if collapsed { 0.0 } else { 1.0 },
        ..Default::default()
    }
}

/// Removes every one of `body`'s current children — the "empty it
/// first" half [`crate::populate_layers_panel`]/
/// [`crate::populate_history_panel`] both need before a caller can call
/// either again for a freshly opened document. Both modules' own doc
/// comments call themselves "one-shot, not reactive": calling either
/// twice without this in between would just append a second set of
/// rows alongside the first, not replace them.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `body` doesn't exist.
pub fn clear_panel_body(
    tree: &mut WidgetTree<WidgetKind>,
    body: WidgetId,
) -> Result<(), WidgetError> {
    if !tree.contains(body) {
        return Err(WidgetError::UnknownWidget(body));
    }
    let children: Vec<WidgetId> = tree.children(body).unwrap_or_default().to_vec();
    for child in children {
        tree.remove(child)?;
    }
    Ok(())
}

/// Whether `panel`'s own body is currently collapsed — the query half
/// of [`set_panel_collapsed`].
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't
/// exist.
pub fn panel_is_collapsed(
    tree: &WidgetTree<WidgetKind>,
    panel: PanelHandle,
) -> Result<bool, WidgetError> {
    let style = tree
        .style(panel.body)
        .ok_or(WidgetError::UnknownWidget(panel.body))?;
    Ok(style.display == Display::None)
}

/// Collapses (`collapsed: true`) or expands `panel`.
///
/// Collapsing doesn't remove the body or its content from the tree —
/// whatever a caller already populated it with (layer rows, history
/// entries) survives, ready to reappear on expand without needing to be
/// rebuilt. Two things change, both needed: the body's own layout style
/// becomes `Display::None` ("the node is hidden, and its children will
/// also be hidden," per `taffy`'s own docs), *and* `panel.root`'s own
/// `flex_grow` drops to `0.0` — the body alone
/// isn't enough, since `panel.root` (not `panel.body`) is the actual
/// flex item the rail shares height between; a hidden-but-still-
/// `flex_grow: 1.0` root would keep claiming its full share of the
/// rail's height even with nothing visible inside it (caught by this
/// function's own test, not assumed). With both set, the collapsed
/// panel's share goes to its still-expanded siblings automatically —
/// ordinary flexbox behaviour, not a special case. The region's own
/// `Node::set_expanded`/`Action::Collapse`/`Action::Expand` are updated
/// to match, the real disclosure-widget shape a screen reader already
/// expects.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root` or
/// `panel.body` doesn't exist. Both styles are set before
/// `panel.root`'s accessibility node is checked, so a missing
/// accessibility node leaves the new layout state in place rather than
/// rolling it back — the same "partial application on a genuinely
/// malformed handle" tradeoff [`clear_panel_body`] already accepts
/// mid-loop, not a new one introduced here.
pub fn set_panel_collapsed(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    collapsed: bool,
) -> Result<(), WidgetError> {
    tree.set_style(panel.root, root_style(collapsed))?;

    let body_style = if collapsed {
        Style {
            display: Display::None,
            ..Default::default()
        }
    } else {
        Style::default()
    };
    tree.set_style(panel.body, body_style)?;

    let node = tree
        .accessibility(panel.root)
        .ok_or(WidgetError::UnknownWidget(panel.root))?;
    let mut updated = node.clone();
    updated.set_expanded(!collapsed);
    if collapsed {
        updated.remove_action(Action::Collapse);
        updated.add_action(Action::Expand);
    } else {
        updated.remove_action(Action::Expand);
        updated.add_action(Action::Collapse);
    }
    tree.set_accessibility(panel.root, updated)
}

/// Closes `panel`: the same layout/accessibility change
/// [`set_panel_collapsed`]`(tree, panel, true)` already makes, plus
/// really freeing its current content ([`clear_panel_body`]) rather
/// than just hiding it. Unlike a plain collapse — which deliberately
/// keeps content resident so a quick re-expand needs no rebuild, see
/// that function's own doc comment — closing trades that cheap-toggle
/// guarantee for actually reclaiming the memory and simplifying the
/// accessibility tree down to just the region itself.
///
/// Reopening is the ordinary `set_panel_collapsed(tree, panel, false)`:
/// the body comes back empty until whatever populated it before
/// (`populate_layers_panel`/`populate_history_panel`, `aurora-ui`'s own
/// higher-level callers) runs again on the next real document-state
/// change — the same "one-shot, not reactive" contract those functions
/// already document. This module knows nothing about layers or history
/// to repopulate anything itself.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root` or
/// `panel.body` doesn't exist.
pub fn close_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
) -> Result<(), WidgetError> {
    set_panel_collapsed(tree, panel, true)?;
    clear_panel_body(tree, panel.body)
}

#[cfg(test)]
mod tests {
    use super::{
        clear_panel_body, close_panel, insert_panel, panel_is_collapsed, set_panel_collapsed,
    };
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
        assert!(accessibility.supports_action(accesskit::Action::Collapse));
        assert!(!accessibility.supports_action(accesskit::Action::Expand));
        assert_eq!(accessibility.is_expanded(), Some(true));
        assert_eq!(tree.payload(panel.root), Some(&WidgetKind::Panel));
        assert_eq!(tree.children(panel.body), Some([].as_slice()));
        assert_eq!(tree.parent(panel.body), Some(panel.root));
        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(!collapsed, "a freshly inserted panel starts expanded"),
            Err(err) => unreachable!("{err:?}"),
        }
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

    #[test]
    fn clear_panel_body_removes_every_child_but_keeps_the_body_itself() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        for _ in 0..3 {
            if let Err(err) = widgets::insert_container(&mut tree, panel.body, Style::default()) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(tree.children(panel.body).map(<[_]>::len), Some(3));

        if let Err(err) = clear_panel_body(&mut tree, panel.body) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.children(panel.body), Some([].as_slice()));
        assert!(
            tree.contains(panel.body),
            "the body itself must survive, only its children are removed"
        );
    }

    #[test]
    fn clear_panel_body_rejects_an_unknown_body() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match clear_panel_body(&mut tree, bogus) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn collapsing_a_panel_hides_its_body_and_survives_its_own_content() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = widgets::insert_container(&mut tree, panel.body, Style::default()) {
            unreachable!("{err:?}");
        }

        if let Err(err) = set_panel_collapsed(&mut tree, panel, true) {
            unreachable!("{err:?}");
        }

        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(collapsed),
            Err(err) => unreachable!("{err:?}"),
        }
        let Some(style) = tree.style(panel.body) else {
            unreachable!("body still exists");
        };
        assert_eq!(style.display, taffy::Display::None);
        assert_eq!(
            tree.children(panel.body).map(<[_]>::len),
            Some(1),
            "collapsing must not remove the body's own content"
        );

        let Some(accessibility) = tree.accessibility(panel.root) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(false));
        assert!(!accessibility.supports_action(accesskit::Action::Collapse));
        assert!(accessibility.supports_action(accesskit::Action::Expand));
    }

    #[test]
    fn expanding_a_collapsed_panel_restores_its_bodys_own_layout() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_panel_collapsed(&mut tree, panel, true) {
            unreachable!("{err:?}");
        }

        if let Err(err) = set_panel_collapsed(&mut tree, panel, false) {
            unreachable!("{err:?}");
        }

        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(!collapsed),
            Err(err) => unreachable!("{err:?}"),
        }
        let Some(accessibility) = tree.accessibility(panel.root) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_expanded(), Some(true));
        assert!(accessibility.supports_action(accesskit::Action::Collapse));
        assert!(!accessibility.supports_action(accesskit::Action::Expand));
    }

    #[test]
    fn a_collapsed_panel_gives_its_own_height_back_to_its_siblings() {
        let (mut tree, root) = widgets::new_tree(Style {
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: taffy::style_helpers::length(100.0_f32),
                height: taffy::style_helpers::length(200.0_f32),
            },
            ..Default::default()
        });
        let first = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let second = match insert_panel(&mut tree, root, "Properties") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 200.0);
        let Some(before) = tree.bounds(second.root) else {
            unreachable!("just laid out");
        };
        assert_eq!(before.height, 100, "two panels share the height equally");

        if let Err(err) = set_panel_collapsed(&mut tree, first, true) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(100.0, 200.0);
        let Some(after) = tree.bounds(second.root) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            after.height, 200,
            "the collapsed panel's own share must go to its still-expanded sibling, ordinary \
             flex_grow sharing, not a special case"
        );
    }

    #[test]
    fn panel_is_collapsed_rejects_an_unknown_body() {
        let (tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        let panel = super::PanelHandle {
            root: bogus,
            body: bogus,
        };
        match panel_is_collapsed(&tree, panel) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_panel_collapsed_rejects_an_unknown_body() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        let panel = super::PanelHandle {
            root: bogus,
            body: bogus,
        };
        match set_panel_collapsed(&mut tree, panel, true) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn closing_a_panel_collapses_it_and_really_empties_its_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        for _ in 0..3 {
            if let Err(err) = widgets::insert_container(&mut tree, panel.body, Style::default()) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(tree.children(panel.body).map(<[_]>::len), Some(3));

        if let Err(err) = close_panel(&mut tree, panel) {
            unreachable!("{err:?}");
        }

        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(
                collapsed,
                "closing must collapse, same as set_panel_collapsed"
            ),
            Err(err) => unreachable!("{err:?}"),
        }
        assert_eq!(
            tree.children(panel.body),
            Some([].as_slice()),
            "unlike a plain collapse, closing must really empty the body"
        );
        assert!(
            tree.contains(panel.body),
            "the body itself must survive -- only its children are removed, same as \
             clear_panel_body alone"
        );
    }

    #[test]
    fn reopening_a_closed_panel_restores_its_layout_with_an_empty_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = widgets::insert_container(&mut tree, panel.body, Style::default()) {
            unreachable!("{err:?}");
        }
        if let Err(err) = close_panel(&mut tree, panel) {
            unreachable!("{err:?}");
        }

        if let Err(err) = set_panel_collapsed(&mut tree, panel, false) {
            unreachable!("{err:?}");
        }

        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(!collapsed),
            Err(err) => unreachable!("{err:?}"),
        }
        assert_eq!(
            tree.children(panel.body),
            Some([].as_slice()),
            "reopening doesn't repopulate on its own -- this module knows nothing about \
             layers/history content, see close_panel's own doc comment"
        );
    }

    #[test]
    fn close_panel_rejects_an_unknown_body() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        let panel = super::PanelHandle {
            root: bogus,
            body: bogus,
        };
        match close_panel(&mut tree, panel) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
