//! Panel tab groups (0.164.0, workspace round 3): two or more docked
//! panels sharing one dock slot as tabs, following Photoshop's layout
//! convention (not its look) — a tab row over the selected panel's body.
//!
//! **Shape.** A group is one column in the rail: its root (an unlabelled
//! `Role::GenericContainer`) holds an `aurora_widgets` tab bar
//! (`Role::TabList`, one `Role::Tab` per member) as its first child, then
//! each member panel's own root, exactly as [`insert_panel`] builds it.
//! Every member stays an ordinary [`PanelHandle`] — collapse, close,
//! Content/Fill sizing, the scrollable body and its linked scrollbar all
//! keep working through `crate::panel`'s functions — with three changes
//! made while it is grouped:
//!
//! - its root is a `Role::TabPanel`, labelled by its tab
//!   (`labelled_by`), and keeps its own label as well;
//! - its title slot is zero height ([`crate::panel`]'s
//!   `grouped_header_style`): the tab names it;
//! - only the selected member is shown. An unselected one is
//!   `Display::None` (taking no layout and no hits), `hidden` in the
//!   accessibility tree (so an assistive technology skips its whole
//!   subtree) and declares no `Action::Focus` (so `Tab` never lands on
//!   it). A hidden body keeps its scroll offset (`WidgetTree`'s scroll
//!   clamp leaves a zero-height container's offset alone), its content,
//!   its sizing and its collapsed state, so switching back restores them.
//!
//! **The group's root mirrors the shown member's flex role** — its
//! `flex_grow`/`flex_shrink`/`flex_basis` — with a floor of the bar's row
//! plus that member's own floor, so the group takes exactly one slot in
//! the rail and a crowded rail cannot overlap the next slot (the 0.144.1
//! lesson). `refresh_group_root` is that rule, run by every `crate::
//! panel` style change on a grouped panel as well as by
//! [`sync_panel_group`].
//!
//! **Collapsing a group** collapses every open member, so the group
//! shrinks to its tab row ([`set_panel_group_collapsed`]). **Closing a
//! member** ([`crate::close_panel`] then [`sync_panel_group`]) moves the
//! selection to an open sibling; with every member closed the whole
//! group, tab row included, is hidden and its bar disabled. A closed
//! member's tab stays in the row: choosing it reopens that panel (empty
//! until its next repopulation — the same contract the panel-toggle
//! command already has for a closed panel).

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind, row_height};
use aurora_widgets::{FocusManager, WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero as _, length};
use taffy::{Dimension, Display, FlexDirection, Size, Style};

use crate::panel::{
    PanelHandle, PanelSizing, grouped_header_style, insert_panel, panel_is_closed,
    panel_is_collapsed, root_style, set_display, set_panel_collapsed, shown_children_floor,
};

/// One inserted panel tab group's own widget ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelGroup {
    /// The group's column in the rail (an unlabelled
    /// `Role::GenericContainer`).
    pub root: WidgetId,
    /// The tab bar, the root's first child: `Role::TabList`, one tab per
    /// member, in member order.
    pub bar: WidgetId,
    /// The member panels, in tab order.
    pub members: Vec<PanelHandle>,
}

impl PanelGroup {
    /// `panel`'s tab index in this group, if it is a member.
    #[must_use]
    pub fn index_of(&self, panel: PanelHandle) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.root == panel.root)
    }
}

/// Adds a new panel tab group as the last child of `parent`: a tab bar
/// labelled `label` with one tab per `titles` entry, and one empty,
/// expanded [`PanelSizing::Fill`] panel per title, tab `selected` shown.
///
/// # Errors
///
/// [`WidgetError::IndexOutOfRange`] if `titles` is empty or `selected`
/// names no title (nothing is inserted), or [`WidgetError::UnknownWidget`]
/// if `parent` doesn't exist.
pub fn insert_panel_group(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    label: &str,
    titles: &[&str],
    selected: usize,
    scales: &Scales,
) -> Result<PanelGroup, WidgetError> {
    if selected >= titles.len() {
        return Err(WidgetError::IndexOutOfRange {
            index: selected,
            len: titles.len(),
        });
    }
    let root = tree.insert(
        parent,
        root_style(false, PanelSizing::Fill, 0.0),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    let bar = widgets::insert_tab_bar(
        tree,
        root,
        scales,
        label,
        titles.iter().map(|title| (*title).to_owned()).collect(),
        selected,
    )?;
    // The tab row never shrinks and counts one row toward the group's
    // floor (`shown_children_floor` reads declared minimums only).
    let mut bar_style = tree
        .style(bar)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(bar))?;
    bar_style.flex_shrink = 0.0;
    bar_style.min_size.height = length(row_height(scales));
    tree.set_style(bar, bar_style)?;
    let mut members = Vec::with_capacity(titles.len());
    for title in titles {
        let panel = insert_panel(tree, root, *title, scales)?;
        tree.set_style(panel.header, grouped_header_style())?;
        members.push(panel);
    }
    let group = PanelGroup { root, bar, members };
    sync_panel_group(tree, &group)?;
    Ok(group)
}

/// A tab group's tab-list label from its members' titles (0.166.0):
/// "Properties and History", "Layers, Properties and History" — the
/// default group's label is exactly [`crate::PANEL_GROUP_LABEL`].
#[must_use]
pub fn panel_group_label(titles: &[&str]) -> String {
    match titles {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Builds a tab group at child `index` of `parent` (0.166.0,
/// drag-to-redock) from panels that already exist: a new group root and
/// tab bar (one tab per `members` entry, labelled by its title), each
/// member's root moved in — its subtree, ids, content, scroll offset and
/// collapsed or closed state intact — and given the grouped title slot,
/// tab `selected` shown ([`sync_panel_group`]).
///
/// # Errors
///
/// [`WidgetError::IndexOutOfRange`] if `members` is empty or `selected`
/// names no member (nothing is built), else
/// [`WidgetError::UnknownWidget`] for a malformed handle.
pub(crate) fn build_panel_group(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    index: usize,
    members: &[(PanelHandle, &str)],
    selected: usize,
    scales: &Scales,
) -> Result<PanelGroup, WidgetError> {
    if selected >= members.len() {
        return Err(WidgetError::IndexOutOfRange {
            index: selected,
            len: members.len(),
        });
    }
    let titles: Vec<&str> = members.iter().map(|(_, title)| *title).collect();
    let root = tree.insert(
        parent,
        root_style(false, PanelSizing::Fill, 0.0),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    tree.move_child(root, parent, index)?;
    let bar = widgets::insert_tab_bar(
        tree,
        root,
        scales,
        &panel_group_label(&titles),
        titles.iter().map(|title| (*title).to_owned()).collect(),
        selected,
    )?;
    let mut bar_style = tree
        .style(bar)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(bar))?;
    bar_style.flex_shrink = 0.0;
    bar_style.min_size.height = length(row_height(scales));
    tree.set_style(bar, bar_style)?;
    for &(member, _) in members {
        tree.move_child(member.root, root, usize::MAX)?;
        crate::panel::set_panel_grouped(tree, member, true, scales)?;
    }
    let group = PanelGroup {
        root,
        bar,
        members: members.iter().map(|(member, _)| *member).collect(),
    };
    sync_panel_group(tree, &group)?;
    Ok(group)
}

/// The tab the group's bar has selected.
///
/// # Errors
///
/// As [`widgets::tab_bar_state`].
pub fn panel_group_selected(
    tree: &WidgetTree<WidgetKind>,
    group: &PanelGroup,
) -> Result<usize, WidgetError> {
    Ok(widgets::tab_bar_state(tree, group.bar)?.selected())
}

/// The member currently shown (its root not `Display::None`), if any —
/// `None` once every member is closed.
#[must_use]
pub fn panel_group_shown(tree: &WidgetTree<WidgetKind>, group: &PanelGroup) -> Option<usize> {
    group.members.iter().position(|member| {
        tree.style(member.root)
            .is_some_and(|style| style.display != Display::None)
    })
}

/// Whether `id` is the group's tab bar or one of its tabs.
#[must_use]
pub fn panel_group_contains(
    tree: &WidgetTree<WidgetKind>,
    group: &PanelGroup,
    id: WidgetId,
) -> bool {
    tree.contains(group.bar) && tree.is_within(group.bar, id)
}

/// Whether the group is collapsed to its tab row: its shown member is
/// collapsed, or nothing is shown at all.
///
/// # Errors
///
/// As [`panel_is_collapsed`].
pub fn panel_group_is_collapsed(
    tree: &WidgetTree<WidgetKind>,
    group: &PanelGroup,
) -> Result<bool, WidgetError> {
    match panel_group_shown(tree, group).and_then(|index| group.members.get(index)) {
        Some(&member) => panel_is_collapsed(tree, member),
        None => Ok(true),
    }
}

/// Brings the group's tree in line with its bar's selection — a
/// structural sync, never a reopen: shows the selected member, hides the
/// others (layout, accessibility and focusability, see this module's doc
/// comment) and refreshes the group's own root (`refresh_group_root`).
/// A selected member that is closed hands the selection to the first open
/// sibling; with every member closed the group and its bar are hidden and
/// the bar disabled. Idempotent.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a member or bar id that doesn't
/// exist.
pub fn sync_panel_group(
    tree: &mut WidgetTree<WidgetKind>,
    group: &PanelGroup,
) -> Result<(), WidgetError> {
    let mut open = Vec::with_capacity(group.members.len());
    for &member in &group.members {
        open.push(!panel_is_closed(tree, member)?);
    }
    let any_open = open.contains(&true);
    if any_open {
        widgets::set_tab_bar_disabled(tree, group.bar, false)?;
    }
    let mut selected = panel_group_selected(tree, group)?;
    if !open.get(selected).copied().unwrap_or(false)
        && let Some(first) = open.iter().position(|&is_open| is_open)
    {
        // A closed panel's tab is never left selected while a sibling is
        // open: closing the shown tab must not leave an empty group.
        widgets::select_tab(tree, group.bar, first)?;
        selected = first;
    }
    let tabs = widgets::tab_bar_state(tree, group.bar)?.tabs().to_vec();
    for (index, &member) in group.members.iter().enumerate() {
        let shown = any_open && index == selected;
        set_display(
            tree,
            member.root,
            if shown { Display::Flex } else { Display::None },
        )?;
        let node = tree
            .accessibility(member.root)
            .ok_or(WidgetError::UnknownWidget(member.root))?;
        let mut updated = node.clone();
        updated.set_role(Role::TabPanel);
        // Only ever a live tab id: `accesskit_consumer` unwraps every
        // `labelled_by` id it resolves, so a dangling one would panic.
        match tabs.get(index) {
            Some(&tab) if tree.contains(tab) => updated.set_labelled_by(vec![tab]),
            _ => updated.clear_labelled_by(),
        }
        // Review I4: the group collapses as one (`set_panel_group_collapsed`)
        // and no path routes a member's own `Collapse`/`Expand`, so a
        // grouped member does not advertise them (see also
        // `crate::panel::set_panel_collapsed`).
        updated.remove_action(Action::Collapse);
        updated.remove_action(Action::Expand);
        if shown {
            updated.clear_hidden();
            updated.add_action(Action::Focus);
        } else {
            updated.set_hidden();
            updated.remove_action(Action::Focus);
        }
        if updated != *node {
            tree.set_accessibility(member.root, updated)?;
        }
    }
    let node = tree
        .accessibility(group.root)
        .ok_or(WidgetError::UnknownWidget(group.root))?;
    if node.is_hidden() == any_open {
        let mut updated = node.clone();
        if any_open {
            updated.clear_hidden();
        } else {
            updated.set_hidden();
        }
        tree.set_accessibility(group.root, updated)?;
    }
    if !any_open {
        widgets::set_tab_bar_disabled(tree, group.bar, true)?;
    }
    refresh_group_root(tree, group.root)
}

/// Shows tab `index` as a user gesture would — a click, an assistive
/// technology's `Click`, an arrow key, a panel command: selects it,
/// expands the group (every open member, plus this one, reopening it if it
/// was closed) and syncs ([`sync_panel_group`]).
///
/// # Errors
///
/// [`WidgetError::IndexOutOfRange`] if `index` names no member, else as
/// [`sync_panel_group`].
pub fn show_panel_group_tab(
    tree: &mut WidgetTree<WidgetKind>,
    group: &PanelGroup,
    index: usize,
) -> Result<(), WidgetError> {
    let Some(&target) = group.members.get(index) else {
        return Err(WidgetError::IndexOutOfRange {
            index,
            len: group.members.len(),
        });
    };
    widgets::set_tab_bar_disabled(tree, group.bar, false)?;
    widgets::select_tab(tree, group.bar, index)?;
    for &member in &group.members {
        if member == target || !panel_is_closed(tree, member)? {
            set_panel_collapsed(tree, member, false)?;
        }
    }
    sync_panel_group(tree, group)
}

/// After input reached the group's bar (a pointer press, a key, an
/// assistive technology's action): when the bar now selects a different
/// tab from the one shown, shows it ([`show_panel_group_tab`]). With
/// `activated` — a pointer press or an assistive technology's `Click`, not
/// a key or a hover — the already-selected tab of a collapsed group
/// expands it too (review I5). Returns whether it changed anything (the
/// caller lays out again).
///
/// # Errors
///
/// As [`show_panel_group_tab`].
pub fn follow_panel_group_tab(
    tree: &mut WidgetTree<WidgetKind>,
    group: &PanelGroup,
    activated: bool,
) -> Result<bool, WidgetError> {
    let selected = panel_group_selected(tree, group)?;
    if panel_group_shown(tree, group) == Some(selected)
        && !(activated && panel_group_is_collapsed(tree, group)?)
    {
        return Ok(false);
    }
    show_panel_group_tab(tree, group, selected)?;
    Ok(true)
}

/// Collapses (`true`) the group to its tab row, or expands it: every open
/// member is collapsed or expanded together, so a later tab switch shows
/// the same state. Closed members are left closed.
///
/// # Errors
///
/// As [`sync_panel_group`].
pub fn set_panel_group_collapsed(
    tree: &mut WidgetTree<WidgetKind>,
    group: &PanelGroup,
    collapsed: bool,
) -> Result<(), WidgetError> {
    for &member in &group.members {
        if !panel_is_closed(tree, member)? {
            set_panel_collapsed(tree, member, collapsed)?;
        }
    }
    sync_panel_group(tree, group)
}

/// Whether `id`, or any of its ancestors, is hidden — `Display::None` in
/// layout or `hidden` in the accessibility tree.
fn hidden_in_tree(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let mut current = Some(id);
    while let Some(id) = current {
        let gone = tree
            .style(id)
            .is_some_and(|style| style.display == Display::None)
            || tree.accessibility(id).is_some_and(Node::is_hidden);
        if gone {
            return true;
        }
        current = tree.parent(id);
    }
    false
}

fn focusable_shown(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    tree.accessibility(id)
        .is_some_and(|node| node.supports_action(Action::Focus))
        && !hidden_in_tree(tree, id)
}

/// Moves keyboard focus off a widget that is no longer shown (0.164.0
/// review I1) — the one repair every path that can hide a panel runs: a
/// tab switch, a group collapse or close, the panel commands, the Curves
/// rule, and a plain collapse of an ungrouped panel. `FocusManager::
/// validate` only drops an id that no longer exists; a hidden widget still
/// exists, and keys would still reach it.
///
/// When the focused widget, or an ancestor, is `Display::None` or
/// AT-`hidden`: focus moves to `group`'s selected tab when the widget was
/// inside the group (its tab names the panel just hidden), else to its
/// nearest ancestor that is shown and focusable (a collapsed panel's own
/// root); with neither — every member of the group closed, its bar
/// disabled — focus is cleared. A focus that is not hidden is left alone.
/// Returns whether focus changed.
pub fn refocus_out_of_hidden(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    group: &PanelGroup,
) -> bool {
    refocus_out_of_hidden_in(tree, focus, Some(group))
}

/// [`refocus_out_of_hidden`] for a workspace that may hold no group at
/// all (0.166.0: every panel can be docked on its own) — `group` is the
/// group holding the focused widget, if any.
pub(crate) fn refocus_out_of_hidden_in(
    tree: &mut WidgetTree<WidgetKind>,
    focus: &mut FocusManager,
    group: Option<&PanelGroup>,
) -> bool {
    if focus.validate(tree) {
        return true;
    }
    let Some(focused) = focus.focused() else {
        return false;
    };
    if !hidden_in_tree(tree, focused) {
        return false;
    }
    let tab = group
        .filter(|group| tree.is_within(group.root, focused))
        .and_then(|group| widgets::tab_bar_state(tree, group.bar).ok()?.selected_tab())
        .filter(|&tab| focusable_shown(tree, tab));
    let target = tab.or_else(|| {
        let mut current = tree.parent(focused);
        while let Some(id) = current {
            if focusable_shown(tree, id) {
                return Some(id);
            }
            current = tree.parent(id);
        }
        None
    });
    match target {
        Some(target) if focus.focus(tree, target).is_ok() => {}
        _ => focus.blur(tree),
    }
    true
}

/// Whether `id` is a panel tab group's tab bar or one of its tabs — the
/// tree-only form of [`panel_group_contains`], for a caller (`aurora-app`'s
/// widget-owner lookup) that has no [`PanelGroup`] at hand.
#[must_use]
pub fn is_panel_group_tab(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let bar = match tree.payload(id) {
        Some(WidgetKind::TabBar(_)) => Some(id),
        Some(WidgetKind::Tab(_)) => tree.parent(id),
        _ => None,
    };
    bar.filter(|&bar| matches!(tree.payload(bar), Some(WidgetKind::TabBar(_))))
        .and_then(|bar| tree.parent(bar))
        .is_some_and(|root| is_group_root(tree, root))
}

/// Whether `id` is a panel tab group's root: a plain container whose
/// first child is a tab bar and whose other children are all panels.
pub(crate) fn is_group_root(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let Some(children) = tree.children(id) else {
        return false;
    };
    let Some((&first, rest)) = children.split_first() else {
        return false;
    };
    matches!(tree.payload(id), Some(WidgetKind::Container))
        && matches!(tree.payload(first), Some(WidgetKind::TabBar(_)))
        && !rest.is_empty()
        && rest
            .iter()
            .all(|&child| matches!(tree.payload(child), Some(WidgetKind::Panel)))
}

/// Re-derives a group root's own style from its shown member (see this
/// module's doc comment): the member's flex role, and a floor of the
/// group's shown children's declared minimums (the tab row plus the
/// member's own floor). `Display::None` when no member is shown.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `root` doesn't exist.
pub(crate) fn refresh_group_root(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
) -> Result<(), WidgetError> {
    let children = tree
        .children(root)
        .ok_or(WidgetError::UnknownWidget(root))?
        .to_vec();
    let shown = children.iter().skip(1).copied().find(|&child| {
        tree.style(child)
            .is_some_and(|style| style.display != Display::None)
    });
    let style = match shown.and_then(|member| tree.style(member)) {
        Some(member) => Style {
            flex_direction: FlexDirection::Column,
            flex_grow: member.flex_grow,
            flex_shrink: member.flex_shrink,
            flex_basis: member.flex_basis,
            min_size: Size {
                width: Dimension::ZERO,
                height: length(shown_children_floor(tree, root)),
            },
            ..Default::default()
        },
        None => Style {
            display: Display::None,
            ..root_style(true, PanelSizing::Fill, 0.0)
        },
    };
    if tree.style(root) == Some(&style) {
        return Ok(());
    }
    tree.set_style(root, style)
}

#[cfg(test)]
mod tests {
    use aurora_widgets::FocusManager;
    use aurora_widgets::widgets::{self, WidgetKind};
    use taffy::Display;

    use super::{
        PanelGroup, insert_panel_group, panel_group_shown, set_panel_group_collapsed,
        show_panel_group_tab, sync_panel_group,
    };
    use crate::panel::{PanelSizing, set_panel_collapsed, set_panel_sizing};

    fn test_scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn group() -> (
        aurora_widgets::WidgetTree<WidgetKind>,
        PanelGroup,
        aurora_theme::Scales,
    ) {
        let scales = test_scales();
        let (mut tree, root) = widgets::new_tree(taffy::Style::default());
        match insert_panel_group(&mut tree, root, "Group", &["One", "Two"], 0, &scales) {
            Ok(group) => (tree, group, scales),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn display(
        tree: &aurora_widgets::WidgetTree<WidgetKind>,
        id: aurora_widgets::WidgetId,
    ) -> Display {
        match tree.style(id) {
            Some(style) => style.display,
            None => unreachable!("exists"),
        }
    }

    #[test]
    fn an_empty_or_out_of_range_group_is_refused_and_inserts_nothing() {
        let scales = test_scales();
        let (mut tree, root) = widgets::new_tree(taffy::Style::default());
        for (titles, selected) in [(&[][..], 0), (&["One"][..], 1)] {
            assert!(insert_panel_group(&mut tree, root, "G", titles, selected, &scales).is_err());
        }
        assert_eq!(tree.children(root).map(<[_]>::len), Some(0));
    }

    #[test]
    fn a_style_change_on_a_hidden_member_never_shows_it() {
        let (mut tree, group, scales) = group();
        let [one, two] = group.members[..] else {
            unreachable!("two members");
        };
        assert_eq!(display(&tree, two.root), Display::None);
        if let Err(err) = set_panel_sizing(&mut tree, two, PanelSizing::Content, &scales) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_panel_collapsed(&mut tree, two, true) {
            unreachable!("{err:?}");
        }
        if let Err(err) = set_panel_collapsed(&mut tree, two, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            display(&tree, two.root),
            Display::None,
            "still the hidden tab"
        );
        assert_eq!(display(&tree, one.root), Display::Flex);
        assert_eq!(panel_group_shown(&tree, &group), Some(0));
    }

    #[test]
    fn sync_is_idempotent_and_only_the_shown_tab_panel_is_a_tab_stop() {
        let (mut tree, group, _) = group();
        let before = tree.accessibility_update(group.root);
        if let Err(err) = sync_panel_group(&mut tree, &group) {
            unreachable!("{err:?}");
        }
        let after = tree.accessibility_update(group.root);
        assert_eq!(before.nodes, after.nodes, "a second sync changes nothing");
        let mut focus = FocusManager::new();
        let mut stops = Vec::new();
        while let Some(id) = focus.focus_next(&mut tree) {
            if stops.contains(&id) {
                break;
            }
            stops.push(id);
        }
        let [shown, hidden] = group.members.as_slice() else {
            unreachable!("two members");
        };
        assert!(stops.contains(&shown.root));
        assert!(
            !stops.contains(&hidden.root),
            "a hidden tab panel is never a Tab stop: {stops:?}"
        );
    }

    #[test]
    fn collapsing_the_group_collapses_every_member_and_a_tab_switch_keeps_it_together() {
        let (mut tree, group, _) = group();
        if let Err(err) = set_panel_group_collapsed(&mut tree, &group, true) {
            unreachable!("{err:?}");
        }
        for member in &group.members {
            assert_eq!(display(&tree, member.body), Display::None);
        }
        tree.compute_layout(300.0, 600.0);
        let (Some(root), Some(bar)) = (tree.bounds(group.root), tree.bounds(group.bar)) else {
            unreachable!("laid out");
        };
        assert_eq!(root.height, bar.height, "collapsed to the tab row");
        if let Err(err) = show_panel_group_tab(&mut tree, &group, 1) {
            unreachable!("{err:?}");
        }
        for member in &group.members {
            assert_eq!(
                display(&tree, member.body),
                Display::Flex,
                "expanded together"
            );
        }
        assert_eq!(panel_group_shown(&tree, &group), Some(1));
    }
}
