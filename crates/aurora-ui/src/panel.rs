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
use taffy::style_helpers::TaffyZero;
use taffy::{Dimension, Display, Overflow, Style};

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
    let body = widgets::insert_container(tree, root, body_style(false))?;
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
///
/// **A panel's share of the rail is its siblings' business, never its
/// own content's** — `flex_basis: 0` plus `min_size.height: 0` are what
/// make that true, and both are load-bearing (0.77.1). Real bug, with
/// real numbers: with the default `flex_basis: auto`, a panel's base
/// size is its *content* height, and flexbox's automatic minimum size
/// then refuses to shrink a flex item below that content — so a Layers
/// panel over a 43-layer document (rows are ~21 px each, and nothing
/// caps how many there are) claimed the whole 900 px rail and left
/// Properties and History at literally zero height, off the bottom of
/// the window and not even hit-testable. `flex_basis: 0` gives all
/// three panels the same base size regardless of what is inside them,
/// so the rail is always divided by `flex_grow` alone; `min_size.
/// height: 0` is what stops the automatic minimum size from putting the
/// content height back. Content taller than the resulting share
/// overflows the panel (see [`body_style`]) rather than growing it.
///
/// **`min_size.width` is pinned to `0` too, for the same reason on the
/// other axis (0.77.3).** It was left `AUTO` when the height was pinned,
/// because only the height axis had a demonstrated bug at the time. That
/// asymmetry became a latent one the moment [`body_style`] became a
/// `Column`: width is now the *cross* axis, and a row's own
/// `min_size.width` (one row height, `crate::history_panel`'s
/// `row_style`) propagates up through body → panel root → dock rail as a
/// floor on the rail's whole width. It is currently unreachable —
/// `crate::workspace::set_rail_width` clamps to `[150, 600]` and the
/// propagated floor is 21 px — but "unreachable because something else
/// happens to clamp harder" is exactly the shape the height bug had
/// before a 43-layer document made it reachable. Pinning both axes
/// closes the class rather than the one instance.
fn root_style(collapsed: bool) -> Style {
    Style {
        flex_direction: taffy::FlexDirection::Column,
        flex_grow: if collapsed { 0.0 } else { 1.0 },
        flex_basis: Dimension::ZERO,
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        ..Default::default()
    }
}

/// A panel body's own style — `Column`, and `flex_grow: 1.0` with the
/// same `flex_basis: 0` / `min_size.height: 0` pair [`root_style`] explains,
/// so the body is exactly as tall as the share its root was given and
/// never a pixel taller, whatever it holds. Without that the clamp on
/// the root alone would not be enough: the body would still size to its
/// content and spill its rows down across the panels below it, where
/// `WidgetTree::hit_test` would happily hand a click meant for
/// Properties to a Layers row.
///
/// `Overflow::Hidden` is what actually clips: `WidgetTree::hit_test`
/// already refuses to descend into a parent whose own bounds don't
/// contain the point, so a row past the bottom of the panel is
/// unreachable by pointer, and since `0.77.3`
/// `aurora_widgets::paint::paint_widget` intersects every widget's own
/// paint geometry with any ancestor declaring a clipping overflow, so it
/// is invisible as well — rather than merely covered by whatever the
/// paint order happens to draw next. This declaration is what that
/// intersection reads; it is the only clipping overflow in the
/// workspace. **What does not exist yet is any way to *reach* that
/// content** — there is no scrolling container in `aurora-widgets` (see
/// `aurora_widgets::widgets::tree_view`'s own module doc comment), so
/// rows past the bottom of a crowded Layers panel are currently not
/// reachable at all. That is a real, disclosed gap and the next piece of
/// work here; it is strictly better than the alternative it replaced,
/// which was losing the Properties and History panels entirely.
///
/// **`FlexDirection::Column` is new in `0.77.2`, and it is a bug fix,
/// not a preference.** The body previously inherited `Style::default()`'s
/// `FlexDirection::Row`; combined with `taffy`'s default
/// `align_items: Stretch` on the cross axis, that resolved every *direct*
/// child of a body to **zero width and full body height** — measured, not
/// argued: five History rows in a real 1600×900 `build_workspace` all
/// came back as `Rect { x: 1350, y: 600, width: 0, height: 300 }`,
/// stacked exactly on top of one another, and `WidgetTree::hit_test`
/// (which needs a point genuinely inside a rect) returned `None` for
/// every one of them. [`root_style`] has always declared `Column` for
/// the "content stacks downward" reason; the body was simply left out.
///
/// What this does and does not change:
///
/// - **Layers** is unaffected. `aurora_widgets::widgets::
///   insert_tree_view` gives its own container an explicit
///   `size: { width: percent(1.0), height: percent(1.0) }` on *both*
///   axes, so its resolved box is identical under `Row` or `Column`.
///   **The height being a *definite* `percent(1.0)`, not merely
///   present, is what carries that** — and it is worth spelling out,
///   because once height became the main axis the flex-item automatic
///   minimum size became able to clamp the container *upward* to its
///   content, and a Layers tree over a long document is easily 900 px of
///   content inside a 300 px body. A definite main size caps the
///   automatic minimum, so the container stays the body's height and its
///   overflow is clipped rather than pushing the panel open. Changing
///   that height to `auto()` would silently reintroduce exactly the
///   rail-starvation bug `0.77.1` fixed on [`root_style`].
///   That container is now redundant for Layers and is deliberately
///   kept: removing it would mean rewriting every Layers test's
///   tree-root traversal for no behavioural gain.
/// - **History** is what this fixes, together with its own rows'
///   real `min_size` (`crate::history_panel`).
/// - **Properties is neither fixed nor broken by this.** Its rows are
///   `Style::default()` under `Row` *and* under `Column`, so they were
///   degenerate before and are degenerate now — the axis simply moves
///   from width to height. That is a real, separate, still-open bug,
///   named here rather than left to be rediscovered.
///
/// Setting `Column` here rather than as a per-panel override is what
/// makes it survive: [`set_panel_collapsed`] resets the body to this
/// same shared `body_style` on every collapse *and* every expand, so a
/// per-panel override would be silently discarded on the first
/// collapse/expand round trip. A change to the shared default cannot be
/// discarded by a reset to that same default.
///
/// `Display::None` while collapsed is what actually hides the content
/// ([`set_panel_collapsed`]); the rest of the style is kept identical
/// across both states so expanding restores exactly the layout the body
/// had before.
fn body_style(collapsed: bool) -> Style {
    Style {
        display: if collapsed {
            Display::None
        } else {
            Display::Flex
        },
        flex_direction: taffy::FlexDirection::Column,
        flex_grow: 1.0,
        flex_basis: Dimension::ZERO,
        // Both axes, not just the height -- see `root_style`'s own doc
        // comment for why the width pin is preventive rather than a fix
        // for a reachable bug.
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// Removes every one of `body`'s current children, leaving the body
/// itself in the tree.
///
/// **No `populate_*` function needs an external call to this any more,
/// and that is the current contract** — a correction, since through
/// `0.77.2` this comment named exactly the two that had already stopped
/// needing it. [`crate::populate_layers_panel`] (`0.77.1`),
/// [`crate::populate_history_panel`] (`0.77.2`) and
/// [`crate::populate_properties_panel`] (`0.77.3`) each call this
/// themselves as their first step, so repopulating any panel replaces
/// its rows rather than stacking a second set beside the first. Calling
/// it beforehand is therefore redundant, not wrong.
///
/// What it is still for: emptying a panel with no repopulation to
/// follow — [`close_panel`]'s own second half, and any future caller
/// that wants a body genuinely empty rather than filled with something
/// else.
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

    tree.set_style(panel.body, body_style(collapsed))?;

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
/// **The body's own accessibility node is reset too (0.77.3)**, back to
/// the neutral `Role::GenericContainer` [`insert_panel`] created it
/// with. Every `populate_*` function replaces that node with a real
/// `Role::List` for the content it is about to insert, so without the
/// reset a closed-then-reopened panel would announce an empty list —
/// a role promising rows that were just freed. Emptying the children
/// while leaving the role claiming them is precisely the mismatch this
/// function exists to avoid; a plain [`set_panel_collapsed`]
/// deliberately keeps both, because it keeps the content too.
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
    clear_panel_body(tree, panel.body)?;
    tree.set_accessibility(panel.body, Node::new(Role::GenericContainer))
}

#[cfg(test)]
mod tests {
    use super::{
        clear_panel_body, close_panel, insert_panel, panel_is_collapsed, set_panel_collapsed,
    };
    use aurora_widgets::WidgetError;
    use aurora_widgets::widgets::{self, WidgetKind};
    use taffy::Style;
    use taffy::style_helpers::TaffyZero;

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

    /// The regression test for the `0.77.2` zero-width-row bug. A body
    /// left at `Style::default()`'s `FlexDirection::Row` laid its direct
    /// children out *side by side*, and with `taffy`'s default
    /// `align_items: Stretch` each one resolved to zero width and the
    /// body's full height — invisible and unhittable. Two sized
    /// containers must stack, sharing a left edge.
    #[test]
    fn a_panel_body_stacks_its_children_vertically() {
        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: taffy::style_helpers::length(200.0_f32),
                height: taffy::style_helpers::length(200.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let child_style = || Style {
            size: taffy::Size {
                width: taffy::style_helpers::percent(1.0_f32),
                height: taffy::style_helpers::length(20.0_f32),
            },
            ..Default::default()
        };
        let (Ok(first), Ok(second)) = (
            widgets::insert_container(&mut tree, panel.body, child_style()),
            widgets::insert_container(&mut tree, panel.body, child_style()),
        ) else {
            unreachable!("the body was just inserted");
        };
        tree.compute_layout(200.0, 200.0);

        let (Some(first_bounds), Some(second_bounds)) = (tree.bounds(first), tree.bounds(second))
        else {
            unreachable!("just laid out");
        };
        assert!(
            first_bounds.width > 0,
            "a body's own child must not resolve to a degenerate zero-width box: {first_bounds:?}"
        );
        assert_eq!(
            second_bounds.x, first_bounds.x,
            "sibling children must share a left edge, not sit beside each other"
        );
        assert_eq!(
            second_bounds.y,
            first_bounds.y + i64::from(first_bounds.height),
            "the second child must stack directly under the first"
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

    /// Closing frees the content, so it must free the *role* that
    /// described it too. Otherwise a closed-then-reopened History panel
    /// still announces a `Role::List` — an empty list, promising rows
    /// that were just removed.
    #[test]
    fn closing_a_panel_resets_its_bodys_accessibility_to_a_neutral_container() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut list = accesskit::Node::new(accesskit::Role::List);
        list.set_label("Something a populate_* call left behind");
        if let Err(err) = tree.set_accessibility(panel.body, list) {
            unreachable!("{err:?}");
        }
        if let Err(err) = widgets::insert_container(&mut tree, panel.body, Style::default()) {
            unreachable!("{err:?}");
        }

        if let Err(err) = close_panel(&mut tree, panel) {
            unreachable!("{err:?}");
        }

        let Some(accessibility) = tree.accessibility(panel.body) else {
            unreachable!("the body itself survives close_panel");
        };
        assert_eq!(
            accessibility.role(),
            accesskit::Role::GenericContainer,
            "an emptied body must not keep claiming to be a list"
        );
        assert_eq!(
            accessibility.label(),
            None,
            "nor keep the name the last populate_* call gave it"
        );
    }

    /// `min_size` is pinned to zero on *both* axes, not just the height
    /// `0.77.1` fixed. Under the `Column` body direction width is the
    /// cross axis, so a row's own `min_size.width` would otherwise
    /// propagate up as a floor on the whole dock rail's width — the same
    /// shape as the height bug, one axis over. Read off the styles
    /// themselves: the propagated floor is currently unreachable behind
    /// `workspace::set_rail_width`'s own `[150, 600]` clamp, so no
    /// layout assertion could catch a regression here.
    #[test]
    fn a_panels_own_styles_never_impose_a_minimum_size_on_either_axis() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        for collapsed in [false, true] {
            if let Err(err) = set_panel_collapsed(&mut tree, panel, collapsed) {
                unreachable!("{err:?}");
            }
            for (name, id) in [("root", panel.root), ("body", panel.body)] {
                let Some(style) = tree.style(id) else {
                    unreachable!("just inserted");
                };
                assert_eq!(
                    style.min_size,
                    taffy::Size {
                        width: taffy::Dimension::ZERO,
                        height: taffy::Dimension::ZERO,
                    },
                    "a panel's {name} must never impose its content's size on the rail \
                     (collapsed: {collapsed})"
                );
            }
        }
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
