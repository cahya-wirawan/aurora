//! The main workspace layout: canvas area + a side rail of docked
//! panels, matching the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). PLAN.md M1.8's docking/panels
//! bullet, first slice.
//!
//! **Mostly static** — see [`crate::panel`]'s own doc comment for what's
//! deliberately not here yet (floating panels). Drag-to-redock and its
//! persisted arrangement are 0.166.0's ([`crate::redock`], [`crate::dock`]).
//! [`rail_width`]/[`set_rail_width`] are the one real piece of dock
//! interactivity so far — dragging the rail's own width is real
//! pointer-driven interaction `aurora-app` owns, this module only
//! exposes the pure layout half (see both functions' own doc comments).
//! The menubar the mockup also shows is still left out: it belongs to a
//! separate M1.8 bullet (native menus). The status bar is in since
//! 0.162.0 ([`crate::status_bar`]): the canvas column's last child, under
//! the canvas area, so the canvas area ends one `row_height` above the
//! window's bottom edge.
//!
//! **Tools panel and options bar (0.160.0), the first workspace round.**
//! Following Photoshop's layout convention (not its look), the root row
//! is now: the tools panel ([`crate::tools_panel`], a vertical
//! `Role::Toolbar` on the canvas's left edge) → a canvas column (the
//! options bar, a horizontal `Role::Toolbar` across the top, over the
//! canvas area itself) → the divider → the rail. The canvas area is
//! therefore no longer at the window's origin: every caller mapping a
//! pointer into it must subtract [`WidgetTree::bounds`]'s own `x`/`y`
//! (`aurora-app`'s `pointer_in_canvas` always has).
//!
//! **No pixel rendering** — same "logical model now, painting later"
//! boundary every widget in `aurora-widgets` already keeps (blocked on
//! `aurora-vector`). This produces a real [`WidgetTree`] with real
//! layout (once [`WidgetTree::compute_layout`] runs) and a real
//! accessibility tree; nothing here draws a pixel — the divider
//! ([`Workspace::divider`]) is a real `Role::Splitter` node with a real
//! (currently zero) layout footprint, not a rendered grab handle yet.

use std::collections::HashMap;

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{FocusManager, WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::TaffyZero as _;
use taffy::{Dimension, Display, FlexDirection, Style};

use crate::dock::{DockArrangement, DockPanel, DockPlacement};
use crate::panel::{
    PanelHandle, PanelSizing, close_panel, insert_panel, panel_is_closed, panel_is_collapsed,
    set_panel_collapsed, set_panel_sizing,
};
use crate::panel_group::{
    PanelGroup, insert_panel_group, panel_group_selected, panel_group_shown,
    set_panel_group_collapsed, show_panel_group_tab, sync_panel_group,
};
use crate::panel_strip::{
    PanelStrip, insert_panel_strip, panel_strip_shown, set_panel_strip_shown,
};
use crate::status_bar::{StatusBar, StatusInfo, insert_status_bar};
use crate::tool::Tool;
use crate::tools_panel::{ToolsPanel, insert_tools_panel};

/// The options bar's accessible label.
pub const OPTIONS_BAR_LABEL: &str = "Tool options";

/// The Properties + History tab group's tab-list label (0.164.0).
pub const PANEL_GROUP_LABEL: &str = "Properties and History";

/// The tab [`build_workspace`] selects in the default Properties + History group, and
/// the one a saved layout without a tab falls back to: `0`, Properties
/// (0.164.0). Properties is where the Curves editor and a layer's own
/// settings live — what a user reaches for most — while History is
/// reference; Photoshop's own default likewise opens Properties over its
/// History-and-friends group.
pub const PANEL_GROUP_TAB_DEFAULT: usize = 0;

/// [`set_rail_width`]'s own clamp range, in logical px. Engineering
/// defaults, not design tokens: `design/tokens/scales.toml` has no
/// "dock region width" token (only type/spacing/radius/elevation/
/// motion, which govern widget chrome, not workspace-level layout
/// regions), and inventing one ad hoc is a design decision to raise,
/// not a gap to fill locally (CLAUDE.md) — the same reasoning that
/// already kept the old canvas:rail flex *ratio* out of the token
/// system. `RAIL_MIN_WIDTH` is enough to show a panel's own title and a
/// little content meaningfully; `RAIL_MAX_WIDTH` keeps the rail from
/// swallowing a modest window whole.
const RAIL_MIN_WIDTH: f32 = 150.0;
const RAIL_MAX_WIDTH: f32 = 600.0;
/// The rail's own starting width — the same share of a 1000px-wide
/// viewport the old 3:1 canvas:rail flex ratio already gave it (750/250),
/// kept for continuity rather than picked fresh.
pub const RAIL_WIDTH_DEFAULT: f32 = 250.0;

/// The main workspace: a canvas area and a side rail holding the
/// Layers/Properties/History panels the approved mockup shows.
#[derive(Debug)]
pub struct Workspace {
    pub tree: WidgetTree<WidgetKind>,
    pub root: WidgetId,
    /// The left tools panel (0.160.0) — the root's first child.
    pub tools: ToolsPanel,
    /// The column holding [`Self::options_bar`] above
    /// [`Self::canvas_area`] (0.160.0); it is what grows and shrinks with
    /// the window.
    pub canvas_column: WidgetId,
    /// The options bar (0.160.0): a horizontal `Role::Toolbar` across the
    /// top of the canvas column, holding the active tool's options
    /// (`aurora-app` puts [`crate::ToolControls`]' radius readout and
    /// slider here).
    pub options_bar: WidgetId,
    /// The document tab strip (0.173.0, [`crate::document_tabs`]): a
    /// `Role::TabList` "Documents" between [`Self::options_bar`] and
    /// [`Self::canvas_area`], one tab per open document; the canvas area
    /// is its `Role::TabPanel`. Not part of the saved layout.
    pub document_tabs: WidgetId,
    /// Where the document canvas will render — `Canvas: infinite zoom,
    /// rotation, pan, ...` is a separate, still-open M1.8 bullet; this
    /// is an empty container reserving its place in the layout.
    pub canvas_area: WidgetId,
    /// The status bar (0.162.0): the canvas column's last child, under
    /// [`Self::canvas_area`] — zoom and document info, a `Role::Status`
    /// region. It starts showing 100% and a 0 × 0 document; `aurora-app`
    /// syncs it to the live view and document ([`crate::sync_status_bar`]).
    pub status_bar: StatusBar,
    /// The boundary between [`Self::canvas_area`] and [`Self::rail`] —
    /// a real `Role::Splitter`, [`rail_width`]/[`set_rail_width`]'s own
    /// target. Currently zero-width in the tree (no pixel rendering
    /// exists yet to draw a grab handle), same "real node, no pixels
    /// yet" gap every widget here already has.
    pub divider: WidgetId,
    /// The side rail — a fixed-width dock area holding the three
    /// panels below, stacked. Resizable via [`set_rail_width`]; its
    /// panels rearranged by drag-to-redock (0.166.0, [`Self::slots`]; see this
    /// module's own doc comment).
    pub rail: WidgetId,
    pub layers: PanelHandle,
    /// The Properties panel — by default (0.164.0) the first tab of the
    /// Properties + History group in [`Self::slots`], always an ordinary
    /// [`PanelHandle`].
    pub properties: PanelHandle,
    /// The History panel — by default the second tab of that group.
    pub history: PanelHandle,
    /// The rail's dock slots, top to bottom (0.166.0, drag-to-redock;
    /// [`crate::dock`]): each a lone panel or a tab group
    /// ([`crate::panel_group`]). The default is Layers on its own, then
    /// Properties + History as tabs ([`crate::DockArrangement::default`]);
    /// [`crate::apply_dock_arrangement`] rearranges them, moving the
    /// panels' own subtrees, so [`Self::layers`]/[`Self::properties`]/
    /// [`Self::history`] stay valid across every move.
    pub slots: Vec<RailSlot>,
    /// The drag-to-redock drop indicator (0.166.0): an absolutely
    /// placed root child, hidden except during a panel drag
    /// ([`crate::PanelDrag`]).
    pub drop_indicator: WidgetId,
    /// The floating slots (0.167.0, [`crate::dock`]'s floating panels),
    /// bottom to top: each a frame — an absolutely placed child of
    /// [`Self::canvas_area`], so it is drawn over the canvas, under every
    /// root child after the canvas column (the drop indicator, dialogs,
    /// the palette) and every popover — holding a lone panel or a tab
    /// group. The canvas area's children are exactly these frames, in this
    /// order, which is also their `Tab` and accessibility order.
    pub floating: Vec<FloatFrame>,
    /// The collapsed rail's label strip (0.165.0, [`crate::panel_strip`]):
    /// the root's child right after [`Self::rail`], hidden while the rail
    /// is expanded and shown in its place while it is collapsed
    /// ([`set_rail_collapsed`]).
    pub panel_strip: PanelStrip,
    /// The History panel's current-step row (0.147.1) — the one
    /// [`crate::populate_history_panel`] last returned, `None` until a
    /// caller first populates it. `aurora-app` records it here so its
    /// scroll-follow can bring the marker into view after every
    /// refresh, the way it follows the active layer's row.
    pub history_current: Option<WidgetId>,
    /// The History panel's clickable rows (0.148.0), each mapped to how
    /// many steps are applied once it is the current one — the
    /// [`crate::HistoryRows::targets`] of the last population; empty until
    /// a caller first populates it. `aurora-app` hit-tests a press (and
    /// looks up an assistive technology's `Click`) here to jump.
    pub history_rows: HashMap<WidgetId, usize>,
}

/// A floating slot in the tree (0.167.0): its frame (a
/// `WidgetKind::RaisedPanel`, an unlabelled `Role::GenericContainer`
/// holding the content), a group's grip, the content, and its position in logical px from
/// the canvas area's top-left — the source of truth the frame's style is
/// written from ([`sync_floating_frames`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FloatFrame {
    pub frame: WidgetId,
    /// A floating *group's* grip (the frame's first child, above the tab
    /// strip): a `spacing.sm`-tall bare strip, the handle that moves the
    /// whole group — a group's tabs share the strip's whole width, so the
    /// strip itself has no bare part to grab. `None` for a lone panel,
    /// whose title row is its handle.
    pub grip: Option<WidgetId>,
    pub content: RailSlot,
    pub x: f32,
    pub y: f32,
}

/// One dock slot (0.166.0): a lone panel, or a tab group — in the rail
/// or, since 0.167.0, the content of a [`FloatFrame`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailSlot {
    Panel(PanelHandle),
    Group(PanelGroup),
}

impl RailSlot {
    /// The slot's own root in the rail: the panel's, or the group's.
    #[must_use]
    pub fn root(&self) -> WidgetId {
        match self {
            Self::Panel(panel) => panel.root,
            Self::Group(group) => group.root,
        }
    }

    /// The slot's panels, in tab order.
    #[must_use]
    pub fn panels(&self) -> Vec<PanelHandle> {
        match self {
            Self::Panel(panel) => vec![*panel],
            Self::Group(group) => group.members.clone(),
        }
    }
}

impl Workspace {
    /// The handle of a rail panel by identity.
    #[must_use]
    pub fn panel(&self, panel: DockPanel) -> PanelHandle {
        match panel {
            DockPanel::Layers => self.layers,
            DockPanel::Properties => self.properties,
            DockPanel::History => self.history,
        }
    }

    /// The identity of a rail panel's handle (`None` for any other panel,
    /// the Widget Gallery's say).
    #[must_use]
    pub fn dock_panel(&self, panel: PanelHandle) -> Option<DockPanel> {
        DockPanel::ALL
            .into_iter()
            .find(|&candidate| self.panel(candidate).root == panel.root)
    }

    /// Every slot, the rail's top to bottom and then the floating ones
    /// bottom to top (0.167.0).
    pub fn all_slots(&self) -> impl Iterator<Item = &RailSlot> {
        self.slots
            .iter()
            .chain(self.floating.iter().map(|float| &float.content))
    }

    /// Every tab group, the rail's top to bottom, then the floating ones
    /// (0.167.0) bottom to top.
    pub fn groups(&self) -> impl Iterator<Item = &PanelGroup> {
        self.all_slots().filter_map(|slot| match slot {
            RailSlot::Group(group) => Some(group),
            RailSlot::Panel(_) => None,
        })
    }

    /// The tab group `panel` is a member of, if it is grouped.
    #[must_use]
    pub fn group_of(&self, panel: PanelHandle) -> Option<&PanelGroup> {
        self.groups().find(|group| group.index_of(panel).is_some())
    }

    /// The tab group whose subtree holds `id` (its bar, a tab, a member's
    /// widget), if any.
    #[must_use]
    pub fn group_holding(&self, id: WidgetId) -> Option<&PanelGroup> {
        self.groups()
            .find(|group| self.tree.contains(group.root) && self.tree.is_within(group.root, id))
    }

    /// The rail panel whose subtree holds `id`, or whose group tab `id`
    /// is (a focused tab names its panel).
    #[must_use]
    pub fn panel_holding(&self, id: WidgetId) -> Option<PanelHandle> {
        for group in self.groups() {
            if let Ok(state) = widgets::tab_bar_state(&self.tree, group.bar)
                && let Some(index) = state.index_of(id)
            {
                return group.members.get(index).copied();
            }
        }
        DockPanel::ALL
            .into_iter()
            .map(|panel| self.panel(panel))
            .find(|panel| self.tree.contains(panel.root) && self.tree.is_within(panel.root, id))
    }

    /// The current arrangement, read from the slots, each group's
    /// selected tab and (0.167.0) the floating frames' positions.
    #[must_use]
    pub fn dock_arrangement(&self) -> DockArrangement {
        let entry = |placement: DockPlacement, slot: &RailSlot| {
            let panels = slot
                .panels()
                .into_iter()
                .map(|panel| self.dock_panel(panel))
                .collect();
            let selected = match slot {
                RailSlot::Panel(_) => 0,
                RailSlot::Group(group) => panel_group_selected(&self.tree, group).unwrap_or(0),
            };
            (placement, panels, selected)
        };
        let raw = self
            .slots
            .iter()
            .map(|slot| entry(DockPlacement::Rail, slot))
            .chain(self.floating.iter().map(|float| {
                entry(
                    DockPlacement::Floating {
                        x: float.x,
                        y: float.y,
                    },
                    &float.content,
                )
            }))
            .collect();
        DockArrangement::repaired_placed(raw).0
    }

    /// The floating frame holding `panel`, by index, if it floats.
    #[must_use]
    pub fn floating_index_of(&self, panel: PanelHandle) -> Option<usize> {
        self.floating.iter().position(|float| {
            float
                .content
                .panels()
                .iter()
                .any(|member| member.root == panel.root)
        })
    }
}

/// The rail's own layout style at `width` (logical px) — shared by
/// [`build_workspace`]'s own initial construction and
/// [`set_rail_width`], so the two can never drift apart. `Column`
/// direction (the three panels stack); no explicit `height` — the
/// rail's own height still comes from `root`'s cross-axis `Stretch`
/// default, unchanged from before this bullet.
///
/// **`min_size.width: 0` is what actually keeps a panel row's own width
/// out of the rail's width, and it is a bug fix (0.77.5).** `0.77.3`
/// tried to close that propagation by pinning `min_size` to zero on
/// `crate::panel`'s own `root_style` and `body_style` instead; that does
/// nothing, and the claim it shipped with was wrong. Measured on a real
/// [`build_workspace`] with `compute_layout(1.0, 200.0)`: with any one
/// of the three panels populated, the rail came back **21 px** wide — one
/// `aurora_widgets::widgets::row_height` — against a 1 px window, and
/// mutating a row's own `min_size.width` to `auto` made the floor vanish.
/// The pins on the panel styles changed nothing either way.
///
/// The mechanism, read off `taffy`'s own flexbox source
/// (`compute/flexbox.rs`, `determine_flex_base_size`, lines 817–820 in
/// `taffy 0.12.2` — the version this workspace actually pins, per
/// `Cargo.lock`) rather than assumed:
///
/// ```text
/// let style_min_main_size = child.min_size.or(...).main(dir);
/// child.resolved_minimum_main_size = style_min_main_size.unwrap_or({
///     ...measure the child's min-content size...
/// });
/// ```
///
/// Two consequences, and they are why the `0.77.3` pins were inert:
///
/// - It reads `min_size` **on the main axis only**. A panel root and a
///   panel body are both flex items of `Column` containers, so their
///   `min_size.width` is a *cross*-axis value there and this code never
///   looks at it. The rail is the flex item of the `Row` root, so width
///   *is* its main axis — the rail is the only box in the chain where a
///   width minimum reaches this branch at all.
/// - `unwrap_or` is a short circuit, not a clamp. An `auto` minimum
///   (`None`) falls through to a real min-content **measurement**, which
///   descends the whole subtree and takes each descendant's own
///   `min_size` as a floor on its contribution — which is how a row's
///   `min_size.width` reached the rail. A `min_size` that is *present*
///   replaces that measurement outright, so pinning it here is what stops
///   the descent. Pinning it to zero further down cannot: a minimum of
///   zero is a floor, and a floor never caps a content measurement.
///
/// This fixes the class for all three panels rather than one row style:
/// `aurora_widgets::widgets::tree_view::style` gives Layers' rows the
/// same width floor for its own, unrelated and genuinely load-bearing
/// reason (a deeply indented row would otherwise reach zero width — see
/// that function), and it propagated identically. `min_size.height` is
/// pinned alongside it for symmetry only; height is the rail's cross axis
/// and is already `Stretch`ed by `root`, so it is inert today.
fn rail_style(width: f32) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        size: taffy::Size {
            width: taffy::style_helpers::length(width),
            height: taffy::style_helpers::auto(),
        },
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        ..Default::default()
    }
}

/// Builds a fresh workspace: root (row) → canvas area + divider + rail
/// (column, three stacked panels). Infallible — every parent id used
/// here is one this function just created in its own, brand-new tree,
/// so `WidgetError::UnknownWidget` is structurally unreachable (the
/// same "can't fail against ids of its own making" shape
/// `aurora_widgets::widgets::new_tree` itself already has).
#[must_use]
pub fn build_workspace(scales: &Scales) -> Workspace {
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

    let tools = match insert_tools_panel(&mut tree, root, scales, Tool::default()) {
        Ok(tools) => tools,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };

    let (canvas_column, options_bar, document_tabs, canvas_area) =
        insert_canvas_column(&mut tree, root, scales);
    let initial = StatusInfo {
        zoom: crate::canvas_view::DEFAULT_ZOOM,
        scale_factor: 1.0,
        document_size: (0, 0),
        sample: aurora_tile::SAMPLE_FORMAT,
    };
    let status_bar = match insert_status_bar(&mut tree, canvas_column, scales, &initial) {
        Ok(bar) => bar,
        Err(err) => unreachable!("canvas_column was just inserted: {err:?}"),
    };

    // Deliberately not `Action::Focus` yet: a real `Tab` stop with no
    // working keyboard handler behind it (no arrow-key-driven resize
    // exists yet -- only pointer-driven, `aurora-app`'s own
    // `RailResize`) would be a worse accessibility experience than not
    // being reachable at all, forcing every keyboard/screen-reader user
    // through a stop that does nothing when they land on it. Add this
    // back once keyboard resize is real -- caught by this crate's own
    // `FocusNext` tests expecting the *panels* to be next in tab order,
    // not assumed.
    let mut divider_node = Node::new(Role::Splitter);
    divider_node.set_label("Resize dock rail");
    divider_node.add_action(Action::SetValue);
    divider_node.set_numeric_value(f64::from(RAIL_WIDTH_DEFAULT));
    divider_node.set_min_numeric_value(f64::from(RAIL_MIN_WIDTH));
    divider_node.set_max_numeric_value(f64::from(RAIL_MAX_WIDTH));
    let divider = match tree.insert(root, Style::default(), divider_node, WidgetKind::Container) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };

    let rail = match tree.insert(
        root,
        rail_style(RAIL_WIDTH_DEFAULT),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };

    let layers = match insert_panel(&mut tree, rail, "Layers", scales) {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };
    // 0.164.0: Properties and History share one slot as tabs; Layers
    // stays on its own (Photoshop keeps Layers separate and groups
    // History with other panels). Two non-empty titles and an in-range
    // default, so the group insert cannot fail.
    let panel_group = match insert_panel_group(
        &mut tree,
        rail,
        PANEL_GROUP_LABEL,
        &["Properties", "History"],
        PANEL_GROUP_TAB_DEFAULT,
        scales,
    ) {
        Ok(group) => group,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };
    let (properties, history) = match panel_group.members.as_slice() {
        [properties, history] => (*properties, *history),
        _ => unreachable!("the group was built with exactly two titles"),
    };
    // 0.161.0: Layers and Properties take their content's height (their
    // rows up to the `size.content_panel_max_rows` token, then they scroll); History keeps the zero-basis
    // `Fill` rule and absorbs what they leave. Equal thirds squeezed the
    // Properties panel's Curves editor to nothing (`PanelSizing`).
    size_docked_panels(&mut tree, layers, properties, scales);
    if let Err(err) = sync_panel_group(&mut tree, &panel_group) {
        unreachable!("the group was just inserted into this same tree: {err:?}");
    }
    // 0.165.0: the collapsed rail's label strip, right after the rail,
    // hidden until the rail collapses.
    let panel_strip = insert_rail_strip(&mut tree, root, scales, [layers, properties, history]);
    // 0.166.0: the drag-to-redock drop indicator, hidden until a drag.
    let drop_indicator = match widgets::insert_drop_indicator(&mut tree, root) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree: {err:?}"),
    };

    Workspace {
        tree,
        root,
        tools,
        canvas_column,
        options_bar,
        document_tabs,
        canvas_area,
        status_bar,
        divider,
        rail,
        layers,
        properties,
        history,
        slots: vec![RailSlot::Panel(layers), RailSlot::Group(panel_group)],
        drop_indicator,
        floating: Vec::new(),
        panel_strip,
        history_current: None,
        history_rows: HashMap::new(),
    }
}

/// Gives the two content-sized panels their docked sizing at build time
/// ([`docked_sizing`]); History keeps `insert_panel_group`'s `Fill`.
fn size_docked_panels(
    tree: &mut WidgetTree<WidgetKind>,
    layers: PanelHandle,
    properties: PanelHandle,
    scales: &Scales,
) {
    for (panel, id) in [
        (layers, DockPanel::Layers),
        (properties, DockPanel::Properties),
    ] {
        if let Err(err) = set_panel_sizing(tree, panel, docked_sizing(id), scales) {
            unreachable!("the panel was just inserted into this same tree: {err:?}");
        }
    }
}

/// How a panel shares the rail's height while docked (0.161.0's rule, one
/// place since 0.167.0): Layers and Properties take their content's height
/// up to `size.content_panel_max_rows` rows, History fills what they
/// leave. A floating panel is always [`PanelSizing::Content`] — a float
/// has no column to fill, so a `Fill` panel would shrink to its one-row
/// floor — and returns to this sizing when docked.
#[must_use]
pub fn docked_sizing(panel: DockPanel) -> PanelSizing {
    match panel {
        DockPanel::Layers | DockPanel::Properties => PanelSizing::Content,
        DockPanel::History => PanelSizing::Fill,
    }
}

/// A floating frame's style (0.167.0): absolutely placed at `x`, `y`
/// logical px from the canvas area's top-left, `width` wide, a column as
/// tall as its content up to `max_height` (the canvas area's height, once
/// known; past it the content shrinks and its body scrolls).
pub(crate) fn float_frame_style(x: f32, y: f32, width: f32, max_height: Option<f32>) -> Style {
    Style {
        position: taffy::Position::Absolute,
        flex_direction: FlexDirection::Column,
        inset: taffy::Rect {
            left: taffy::style_helpers::length(x),
            top: taffy::style_helpers::length(y),
            right: taffy::style_helpers::auto(),
            bottom: taffy::style_helpers::auto(),
        },
        size: taffy::Size {
            width: taffy::style_helpers::length(width),
            height: taffy::style_helpers::auto(),
        },
        max_size: taffy::Size {
            width: taffy::style_helpers::auto(),
            height: max_height
                .map_or_else(taffy::style_helpers::auto, taffy::style_helpers::length),
        },
        ..Default::default()
    }
}

/// A floating group's grip style (0.167.0, [`FloatFrame::grip`]): one
/// `spacing.sm` tall, never shrunk, full width.
pub(crate) fn float_grip_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let height = taffy::style_helpers::length(scales.spacing.sm as f32);
    Style {
        flex_shrink: 0.0,
        size: taffy::Size {
            width: taffy::style_helpers::auto(),
            height,
        },
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height,
        },
        ..Default::default()
    }
}

/// The width a floating panel takes (0.167.0): the rail's width — its
/// default and, today, its only width (floats are not resized on their
/// own) — capped at the canvas area's width once that is known.
pub(crate) fn float_width(workspace: &Workspace) -> f32 {
    let rail = rail_width(&workspace.tree, workspace.rail).unwrap_or(RAIL_WIDTH_DEFAULT);
    #[allow(clippy::cast_precision_loss)]
    match workspace.tree.bounds(workspace.canvas_area) {
        Some(canvas) if canvas.width > 0 => rail.min(canvas.width as f32),
        _ => rail,
    }
}

/// Keeps every floating panel inside the canvas area (0.167.0), from the
/// last layout: each frame takes the float width (the rail's, capped at
/// the canvas area's), at most the canvas area's height, and a position
/// clamped so the whole frame — its title
/// row first of all — lies inside the canvas area (its left edge in
/// `[0, W - width]`, its top in `[0, H - min(height, H)]`). The clamped
/// position is stored ([`FloatFrame::x`]/`y`), so a saved layout records
/// where the panel really is. Run after every layout — a window resize, a
/// rail resize or collapse, a scale-factor change and the first layout
/// after a load all reach it through the one layout path — and lay out
/// again when it returns `true`. A canvas area not laid out yet, or of
/// zero size, clamps nothing (so a load's positions survive until the
/// first real layout).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn sync_floating_frames(workspace: &mut Workspace) -> Result<bool, WidgetError> {
    let shown_changed = sync_floating_shown(workspace)?;
    let Some(canvas) = workspace.tree.bounds(workspace.canvas_area) else {
        return Ok(shown_changed);
    };
    if canvas.width == 0 || canvas.height == 0 {
        return Ok(shown_changed);
    }
    #[allow(clippy::cast_precision_loss)]
    let (canvas_w, canvas_h) = (canvas.width as f32, canvas.height as f32);
    let width = float_width(workspace);
    let mut changed = shown_changed;
    for index in 0..workspace.floating.len() {
        let Some(float) = workspace.floating.get(index).cloned() else {
            continue;
        };
        // A hidden frame (everything in it closed) keeps its position, so
        // reopening shows it where it was.
        if !float_frame_shown(&workspace.tree, float.frame) {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let height = workspace
            .tree
            .bounds(float.frame)
            .map_or(0.0, |bounds| bounds.height as f32)
            .min(canvas_h);
        let x = float.x.clamp(0.0, (canvas_w - width).max(0.0));
        let y = float.y.clamp(0.0, (canvas_h - height).max(0.0));
        let style = float_frame_style(x, y, width, Some(canvas_h));
        if workspace.tree.style(float.frame) != Some(&style) {
            workspace.tree.set_style(float.frame, style)?;
            changed = true;
        }
        if let Some(entry) = workspace.floating.get_mut(index) {
            entry.x = x;
            entry.y = y;
        }
    }
    Ok(changed)
}

/// Whether a floating frame is shown — `false` once every panel in it is
/// closed ([`sync_floating_shown`]).
pub(crate) fn float_frame_shown(tree: &WidgetTree<WidgetKind>, frame: WidgetId) -> bool {
    tree.style(frame)
        .is_some_and(|style| style.display != Display::None)
}

/// Hides every floating frame whose panels are all closed and shows the
/// others again (0.167.0 review J-1): hidden is `Display::None` and
/// AT-`hidden`, so a closed floating panel — or a fully closed floating
/// group's grip — leaves no invisible band over the canvas that would
/// block strokes, the wheel or start a drag, and no `Tab` stop. The
/// frame's position is untouched, so a reopened panel comes back where it
/// was. Run by every path that closes or reopens a panel (the close,
/// toggle, show and tab-select actions, a dock rearrangement) and, as a
/// backstop, by [`sync_floating_frames`] after every layout. Returns
/// whether any frame changed. The caller repairs focus
/// ([`crate::refocus_out_of_hidden`]).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn sync_floating_shown(workspace: &mut Workspace) -> Result<bool, WidgetError> {
    let mut changed = false;
    for float in workspace.floating.clone() {
        let mut open = false;
        for panel in float.content.panels() {
            open |= !panel_is_closed(&workspace.tree, panel)?;
        }
        let before = float_frame_shown(&workspace.tree, float.frame)
            && !workspace
                .tree
                .accessibility(float.frame)
                .is_some_and(Node::is_hidden);
        if before != open {
            set_shown(&mut workspace.tree, float.frame, open)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// The floating frame under `point` (window-logical px), topmost first
/// (0.167.0): the one holding the widget a press there would hit
/// (`WidgetTree::hit_test`), so a popover opened from a floating panel
/// counts as that panel's, and one from anywhere else counts as none.
#[must_use]
pub fn floating_index_at(workspace: &Workspace, point: (f32, f32)) -> Option<usize> {
    let hit = workspace.tree.hit_test(point)?;
    workspace.floating.iter().position(|float| {
        float_frame_shown(&workspace.tree, float.frame)
            && workspace.tree.is_within(float.frame, hit)
    })
}

/// Raises floating frame `index` above every other floating panel
/// (0.167.0, a press on it): the frame moves to the canvas area's last
/// child and the slot to the top of [`Workspace::floating`]. Nothing is
/// rebuilt — the frame and everything in it keep their ids and bounds, so
/// the press that raised it still lands where it was aimed. Returns
/// whether anything moved (`false` for the top one).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn raise_floating(workspace: &mut Workspace, index: usize) -> Result<bool, WidgetError> {
    if index.saturating_add(1) >= workspace.floating.len() {
        return Ok(false);
    }
    let float = workspace.floating.remove(index);
    let frame = float.frame;
    workspace.floating.push(float);
    workspace
        .tree
        .move_child(frame, workspace.canvas_area, usize::MAX)?;
    Ok(true)
}

/// The canvas column (0.160.0): the options bar over the canvas area,
/// inserted into `root`. Infallible for the same reason
/// [`build_workspace`] is: `root` is a node of this brand-new tree.
/// Builds [`Workspace::panel_strip`] under `root`: one button per rail
/// panel, `[layers, properties, history]`, each named by its own title.
fn insert_rail_strip(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
    [layers, properties, history]: [PanelHandle; 3],
) -> PanelStrip {
    match insert_panel_strip(
        tree,
        root,
        scales,
        [
            (layers, "Layers"),
            (properties, "Properties"),
            (history, "History"),
        ],
    ) {
        Ok(strip) => strip,
        Err(err) => unreachable!("root was just created by new_tree: {err:?}"),
    }
}

fn insert_canvas_column(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> (WidgetId, WidgetId, WidgetId, WidgetId) {
    // The only element that grows: once the tools panel claims its
    // content width, the rail its fixed width and the divider none, the
    // canvas column absorbs whatever space is left. `min_size: 0` lets a
    // narrow window squeeze it to nothing rather than overflow. A zero
    // `flex_basis` (0.162.0): the column takes exactly the space left,
    // whatever its own content's width — the status bar's padding, gap
    // and fixed-width zoom item would otherwise widen its basis and take
    // width from a crowded window's other children.
    let canvas_column = match widgets::insert_container(
        tree,
        root,
        Style {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Dimension::ZERO,
            min_size: taffy::Size {
                width: Dimension::ZERO,
                height: Dimension::ZERO,
            },
            ..Default::default()
        },
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("root was just created by new_tree above: {err:?}"),
    };
    let mut options_node = Node::new(Role::Toolbar);
    options_node.set_label(OPTIONS_BAR_LABEL);
    options_node.set_orientation(accesskit::Orientation::Horizontal);
    let options_bar = match tree.insert(
        canvas_column,
        options_bar_style(scales),
        options_node,
        WidgetKind::Container,
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("canvas_column was just inserted: {err:?}"),
    };
    // 0.173.0: the document tab strip takes one row off the canvas.
    let document_tabs =
        match crate::document_tabs::insert_document_tabs(tree, canvas_column, scales) {
            Ok(id) => id,
            Err(err) => unreachable!("canvas_column was just inserted: {err:?}"),
        };
    let canvas_area = match widgets::insert_container(
        tree,
        canvas_column,
        Style {
            flex_grow: 1.0,
            min_size: taffy::Size {
                width: Dimension::ZERO,
                height: Dimension::ZERO,
            },
            ..Default::default()
        },
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("canvas_column was just inserted: {err:?}"),
    };
    if let Err(err) = crate::document_tabs::link_document_panel(tree, document_tabs, canvas_area) {
        unreachable!("both were just inserted: {err:?}");
    }
    (canvas_column, options_bar, document_tabs, canvas_area)
}

/// The options bar's style: a row, never shrinking on its column's main
/// axis, padded by `spacing.xs` vertically and `spacing.sm`
/// horizontally, its children `spacing.sm` apart and vertically centred.
/// At least one control row tall even when empty, so the canvas never
/// jumps when switching to a tool with no options.
fn options_bar_style(scales: &Scales) -> Style {
    #[allow(clippy::cast_precision_loss)]
    let (xs, sm) = (scales.spacing.xs as f32, scales.spacing.sm as f32);
    let row = widgets::row_height(scales);
    Style {
        flex_direction: FlexDirection::Row,
        flex_shrink: 0.0,
        align_items: Some(taffy::AlignItems::CENTER),
        gap: taffy::Size {
            width: taffy::style_helpers::length(sm),
            height: taffy::style_helpers::length(sm),
        },
        padding: taffy::Rect {
            left: taffy::style_helpers::length(sm),
            right: taffy::style_helpers::length(sm),
            top: taffy::style_helpers::length(xs),
            bottom: taffy::style_helpers::length(xs),
        },
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: taffy::style_helpers::length(row + 2.0 * xs),
        },
        ..Default::default()
    }
}

/// The rail's own current width in logical px, read back from its real
/// layout style — `None` if `rail_id` doesn't exist or its own width is
/// `Auto` (structurally unreachable for `Workspace::rail` as this
/// module builds it, which always sets a real fixed length).
#[must_use]
pub fn rail_width(tree: &WidgetTree<WidgetKind>, rail_id: WidgetId) -> Option<f32> {
    let width = tree.style(rail_id)?.size.width;
    if width.is_auto() {
        None
    } else {
        Some(width.value())
    }
}

/// Sets the rail's own width to `width`, clamped to
/// `[RAIL_MIN_WIDTH, RAIL_MAX_WIDTH]`. Pure layout/accessibility state
/// — this module knows nothing about pointer events; `aurora-app` is
/// what turns a real drag on [`Workspace::divider`] into calls here
/// (the same "toolkit owns the mechanism, the app shell owns the
/// gesture" split PLAN.md M1.9's own tool/drag machinery already
/// keeps). Updates `divider_id`'s own `Node::set_numeric_value` to
/// match, so an assistive-technology client reading the splitter's own
/// value sees the real, current width, not a stale one — the same
/// "layout and accessibility change together" discipline
/// [`crate::panel::set_panel_collapsed`] already follows. A caller
/// still needs to re-run [`WidgetTree::compute_layout`] afterward for
/// the new width to actually reach [`WidgetTree::bounds`] — this
/// function only ever changes the style taffy resolves *from*.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `rail_id` or `divider_id`
/// doesn't exist.
pub fn set_rail_width(
    tree: &mut WidgetTree<WidgetKind>,
    rail_id: WidgetId,
    divider_id: WidgetId,
    width: f32,
) -> Result<(), WidgetError> {
    let clamped = width.clamp(RAIL_MIN_WIDTH, RAIL_MAX_WIDTH);
    // 0.165.0: a width change keeps the rail collapsed or expanded as it
    // was — `display` is the collapse, `size.width` the remembered width.
    let mut style = rail_style(clamped);
    style.display = tree
        .style(rail_id)
        .ok_or(WidgetError::UnknownWidget(rail_id))?
        .display;
    tree.set_style(rail_id, style)?;

    let node = tree
        .accessibility(divider_id)
        .ok_or(WidgetError::UnknownWidget(divider_id))?;
    let mut updated = node.clone();
    updated.set_numeric_value(f64::from(clamped));
    tree.set_accessibility(divider_id, updated)
}

/// Shows (`Display::Flex`, AT not `hidden`) or hides (`Display::None`, AT
/// `hidden`, so the subtree leaves the accessibility tree and the `Tab`
/// order) `id`. A no-op for what is already so.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub(crate) fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    shown: bool,
) -> Result<(), WidgetError> {
    let style = tree.style(id).ok_or(WidgetError::UnknownWidget(id))?;
    let display = if shown { Display::Flex } else { Display::None };
    if style.display != display {
        let mut updated = style.clone();
        updated.display = display;
        tree.set_style(id, updated)?;
    }
    let node = tree
        .accessibility(id)
        .ok_or(WidgetError::UnknownWidget(id))?;
    if node.is_hidden() == shown {
        let mut updated = node.clone();
        if shown {
            updated.clear_hidden();
        } else {
            updated.set_hidden();
        }
        tree.set_accessibility(id, updated)?;
    }
    Ok(())
}

/// Whether the right rail is collapsed to its label strip (0.165.0). The
/// rail's own `display` is the single source of truth; the divider and
/// the strip follow it ([`set_rail_collapsed`]).
#[must_use]
pub fn rail_collapsed(workspace: &Workspace) -> bool {
    workspace
        .tree
        .style(workspace.rail)
        .is_some_and(|style| style.display == Display::None)
}

/// Collapses the whole right rail to its label strip, or expands it back
/// (0.165.0). Collapsed, the rail and its divider are `Display::None` and
/// AT-`hidden` (no layout, no hits, no `Tab` stop, no divider drag) and
/// the strip ([`Workspace::panel_strip`]) is shown in their place; the
/// canvas column, the one growing element, takes the freed width. The
/// rail's width ([`rail_width`]) is never touched, so expanding restores
/// it exactly, and every panel keeps its own collapsed, closed, scroll
/// and tab state. Returns whether anything changed. The caller repairs
/// focus afterwards ([`refocus_workspace`]).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn set_rail_collapsed(workspace: &mut Workspace, collapsed: bool) -> Result<bool, WidgetError> {
    let changed = rail_collapsed(workspace) != collapsed
        || panel_strip_shown(&workspace.tree, &workspace.panel_strip) != collapsed;
    set_shown(&mut workspace.tree, workspace.rail, !collapsed)?;
    set_shown(&mut workspace.tree, workspace.divider, !collapsed)?;
    let strip = workspace.panel_strip;
    set_panel_strip_shown(&mut workspace.tree, &strip, collapsed)?;
    Ok(changed)
}

/// Flips [`set_rail_collapsed`] — the "Collapse or Expand Panels"
/// command. Returns whether the rail is collapsed afterwards.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn toggle_rail_collapsed(workspace: &mut Workspace) -> Result<bool, WidgetError> {
    let collapse = !rail_collapsed(workspace);
    set_rail_collapsed(workspace, collapse)?;
    Ok(collapse)
}

/// A strip button's action (0.165.0, "expand and show"): expands the rail
/// and shows `panel` there — its tab selected and its slot expanded
/// ([`show_workspace_panel`]). A *closed* panel stays closed, the same
/// rule the Curves transition keeps; the rail still expands. Returns
/// whether anything changed.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed workspace.
pub fn expand_rail_showing(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<bool, WidgetError> {
    // 0.167.0: a floating panel is not in the rail, so showing it never
    // expands a collapsed rail.
    let floating = workspace.floating_index_of(panel).is_some();
    let expanded = if floating {
        false
    } else {
        set_rail_collapsed(workspace, false)?
    };
    let shown = show_workspace_panel(workspace, panel)?;
    Ok(expanded || shown)
}

/// Moves keyboard focus off anything a rail change hid (0.165.0), then
/// runs the panel-group repair ([`crate::refocus_out_of_hidden`]) as the
/// backstop. Collapsing the rail moves focus inside it to the strip
/// button of the panel that held it (Layers, or the group's shown
/// member); expanding it moves focus on a strip button to the panel that
/// button showed — the Layers panel itself, or the group's selected tab.
/// Returns whether focus changed.
pub fn refocus_workspace(workspace: &mut Workspace, focus: &mut FocusManager) -> bool {
    let mut moved = false;
    if let Some(focused) = focus.focused()
        && workspace.tree.contains(focused)
    {
        let collapsed = rail_collapsed(workspace);
        let target = if collapsed && workspace.tree.is_within(workspace.rail, focused) {
            // The strip button of the panel that held focus: a lone
            // panel, or its group's shown member (0.166.0: any slot).
            let panel = match workspace.group_holding(focused) {
                Some(group) => panel_group_shown(&workspace.tree, group)
                    .and_then(|index| group.members.get(index).copied()),
                None => workspace.panel_holding(focused),
            };
            panel.and_then(|panel| workspace.panel_strip.button_for(panel))
        } else if !collapsed
            && workspace
                .tree
                .is_within(workspace.panel_strip.root, focused)
        {
            workspace
                .panel_strip
                .panel_for(focused)
                .and_then(|panel| panel_focus_target(workspace, panel))
        } else {
            None
        };
        if let Some(target) = target {
            moved = focus.focus(&mut workspace.tree, target).is_ok();
        }
    }
    let group = focus
        .focused()
        .and_then(|focused| workspace.group_holding(focused))
        .cloned();
    crate::panel_group::refocus_out_of_hidden_in(&mut workspace.tree, focus, group.as_ref())
        || moved
}

/// Where keyboard focus goes to land "on" `panel` (0.166.0): its group's
/// selected tab when it is grouped, else its own root.
#[must_use]
pub fn panel_focus_target(workspace: &Workspace, panel: PanelHandle) -> Option<WidgetId> {
    match workspace.group_of(panel) {
        Some(group) => widgets::tab_bar_state(&workspace.tree, group.bar)
            .ok()
            .and_then(widgets::TabBarState::selected_tab),
        None => Some(panel.root),
    }
}

/// The panel-toggle command's action on `panel` (0.164.0 for grouped
/// panels): an ungrouped panel flips collapsed/expanded, as before. A
/// panel in a tab group that is its group's shown,
/// expanded tab collapses the group to its tab row; otherwise its tab is
/// selected and the group expanded (reopening it if it was closed).
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed handle.
pub fn toggle_workspace_panel(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<(), WidgetError> {
    toggle_workspace_panel_in(workspace, panel)?;
    // 0.167.0: a reopened floating panel's frame is shown again.
    sync_floating_shown(workspace).map(|_| ())
}

fn toggle_workspace_panel_in(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<(), WidgetError> {
    if let Some(group) = workspace.group_of(panel).cloned()
        && let Some(index) = group.index_of(panel)
    {
        let showing = panel_group_shown(&workspace.tree, &group) == Some(index)
            && !panel_is_collapsed(&workspace.tree, panel)?;
        return if showing {
            set_panel_group_collapsed(&mut workspace.tree, &group, true)
        } else {
            show_panel_group_tab(&mut workspace.tree, &group, index)
        };
    }
    let collapsed = panel_is_collapsed(&workspace.tree, panel)?;
    set_panel_collapsed(&mut workspace.tree, panel, !collapsed)
}

/// The panel-close command's action (0.164.0): [`close_panel`], then —
/// for a grouped panel — [`sync_panel_group`], which moves the group to
/// an open sibling or hides it once every member is closed.
///
/// # Errors
///
/// As [`close_panel`].
pub fn close_workspace_panel(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<(), WidgetError> {
    close_panel(&mut workspace.tree, panel)?;
    if let Some(group) = workspace.group_of(panel).cloned() {
        sync_panel_group(&mut workspace.tree, &group)?;
    }
    // 0.167.0 review J-1: a floating frame with nothing open is hidden.
    sync_floating_shown(workspace).map(|_| ())
}

/// Selects `panel`'s tab when it is a grouped panel not currently shown
/// (0.164.0, the panel-focus commands): the group is expanded and the
/// panel reopened if it was closed. An ungrouped or already-shown panel
/// is left alone. Returns whether it changed anything.
///
/// # Errors
///
/// As [`show_panel_group_tab`].
pub fn select_panel_tab(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<bool, WidgetError> {
    let Some(group) = workspace.group_of(panel).cloned() else {
        return Ok(false);
    };
    let Some(index) = group.index_of(panel) else {
        return Ok(false);
    };
    if panel_group_shown(&workspace.tree, &group) == Some(index) {
        return Ok(false);
    }
    show_panel_group_tab(&mut workspace.tree, &group, index)?;
    sync_floating_shown(workspace)?;
    Ok(true)
}

/// Makes `panel` visible and expanded **unless it is closed** — the
/// Curves auto-show rule's action (0.161.0, extended in 0.164.0): a
/// collapsed panel is expanded and, for a grouped one, its tab selected.
/// A closed panel is never reopened. Returns whether it changed anything.
///
/// # Errors
///
/// As [`show_panel_group_tab`]/[`set_panel_collapsed`].
pub fn show_workspace_panel(
    workspace: &mut Workspace,
    panel: PanelHandle,
) -> Result<bool, WidgetError> {
    if panel_is_closed(&workspace.tree, panel)? {
        return Ok(false);
    }
    if let Some(group) = workspace.group_of(panel).cloned()
        && let Some(index) = group.index_of(panel)
    {
        if panel_group_shown(&workspace.tree, &group) == Some(index)
            && !panel_is_collapsed(&workspace.tree, panel)?
        {
            return Ok(false);
        }
        show_panel_group_tab(&mut workspace.tree, &group, index)?;
        return Ok(true);
    }
    if !panel_is_collapsed(&workspace.tree, panel)? {
        return Ok(false);
    }
    set_panel_collapsed(&mut workspace.tree, panel, false)?;
    Ok(true)
}

/// Shows the History tab (0.164.0 test support): History tests lay it
/// out, and it is not the default tab.
#[cfg(test)]
pub(crate) fn show_history_tab(workspace: &mut Workspace) {
    let history = workspace.history;
    if let Err(err) = select_panel_tab(workspace, history) {
        unreachable!("{err:?}");
    }
}

/// The group holding Properties (test support, 0.166.0): the default
/// Properties + History group, under any arrangement that keeps it.
#[cfg(test)]
pub(crate) fn test_group(workspace: &Workspace) -> PanelGroup {
    match workspace.group_of(workspace.properties) {
        Some(group) => group.clone(),
        None => unreachable!("Properties is grouped in this test"),
    }
}

#[cfg(test)]
mod tests {
    use super::{RAIL_MAX_WIDTH, RAIL_MIN_WIDTH, build_workspace, rail_width, set_rail_width};
    use aurora_widgets::FocusManager;

    fn bounds_of(ws: &super::Workspace, id: aurora_widgets::WidgetId) -> aurora_core::Rect {
        match ws.tree.bounds(id) {
            Some(bounds) => bounds,
            None => unreachable!("{id:?} is laid out"),
        }
    }

    fn tools_width(ws: &super::Workspace) -> u32 {
        bounds_of(ws, ws.tools.root).width
    }

    fn options_bar_height(ws: &super::Workspace) -> u32 {
        bounds_of(ws, ws.options_bar).height
    }

    fn overlaps(a: aurora_core::Rect, b: aurora_core::Rect) -> bool {
        a.width > 0
            && b.width > 0
            && a.height > 0
            && b.height > 0
            && a.x < b.x + i64::from(b.width)
            && b.x < a.x + i64::from(a.width)
            && a.y < b.y + i64::from(b.height)
            && b.y < a.y + i64::from(a.height)
    }

    /// 0.160.0, AC-1/AC-5: the tools panel's width and the options bar's
    /// height come from spacing tokens (text-blind here: a toggle
    /// button's own padding, no label width), the canvas sits exactly in
    /// the space they and the rail leave, and nothing overlaps — at the
    /// default rail, the widest and narrowest rail, and a narrow window
    /// (the 0.144.1 overlap lesson).
    #[test]
    fn the_tools_panel_and_options_bar_take_token_sized_space_and_never_overlap() {
        let scales = test_scales();
        let (xs, md) = (scales.spacing.xs, scales.spacing.md);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&scales) as u32;
        for (window, rail) in [
            ((1000.0, 800.0), 250.0),
            ((1600.0, 900.0), RAIL_MAX_WIDTH),
            ((1600.0, 900.0), RAIL_MIN_WIDTH),
            ((480.0, 480.0), 250.0),
            ((120.0, 200.0), RAIL_MIN_WIDTH),
        ] {
            let mut ws = build_workspace(&scales);
            if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, rail) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(window.0, window.1);
            let tools = bounds_of(&ws, ws.tools.root);
            let column = bounds_of(&ws, ws.canvas_column);
            let bar = bounds_of(&ws, ws.options_bar);
            let canvas = bounds_of(&ws, ws.canvas_area);
            let rail_bounds = bounds_of(&ws, ws.rail);
            let case = format!("window {window:?}, rail {rail}");

            assert_eq!(tools.width, 2 * xs + 2 * md, "{case}: {tools:?}");
            assert_eq!((tools.x, tools.y), (0, 0), "{case}");
            assert_eq!(bar.height, row + 2 * xs, "{case}: {bar:?}");
            assert_eq!(column.x, i64::from(tools.width), "{case}");
            assert_eq!((bar.x, bar.y), (column.x, 0), "{case}");
            // The bar's own padding is a floor on its width: in a window
            // too narrow for even that (the last case), it overhangs its
            // squeezed-to-nothing column — disclosed, not designed away.
            let bar_fits = column.width >= 2 * scales.spacing.sm;
            if bar_fits {
                assert_eq!(bar.width, column.width, "{case}");
                assert!(!overlaps(bar, rail_bounds), "{case}: bar/rail overlap");
            } else {
                assert!(
                    bar.width > column.width,
                    "{case}: only the bar's padding overhangs"
                );
            }
            // 0.173.0: and under the document tab strip.
            let strip = bounds_of(&ws, ws.document_tabs).height;
            assert_eq!(
                (canvas.x, canvas.y, canvas.width),
                (column.x, i64::from(bar.height + strip), column.width),
                "{case}: the canvas sits under the bar, right of the tools"
            );
            assert_eq!(
                column.x + i64::from(column.width),
                rail_bounds.x,
                "{case}: the canvas column ends where the rail begins"
            );
            for (name, a, b) in [
                ("tools/column", tools, column),
                ("tools/rail", tools, rail_bounds),
                ("column/rail", column, rail_bounds),
                ("bar/canvas", bar, canvas),
            ] {
                assert!(!overlaps(a, b), "{case}: {name} overlap: {a:?} {b:?}");
            }
            // Every tool button is laid out inside the strip, stacked.
            let mut previous: Option<aurora_core::Rect> = None;
            for (_, id) in ws.tools.buttons {
                let button = bounds_of(&ws, id);
                assert!(
                    button.x >= tools.x
                        && button.x + i64::from(button.width) <= tools.x + i64::from(tools.width),
                    "{case}: {button:?} outside {tools:?}"
                );
                if let Some(before) = previous {
                    assert!(button.y >= before.y + i64::from(before.height), "{case}");
                }
                previous = Some(button);
            }
        }
    }

    /// The tools panel has no scroll container: it is a fixed strip.
    #[test]
    fn the_tools_panel_does_not_scroll() {
        let mut ws = build_workspace(&test_scales());
        ws.tree.compute_layout(1000.0, 800.0);
        let strip = bounds_of(&ws, ws.tools.root);
        #[allow(clippy::cast_precision_loss)]
        let centre = (
            strip.x as f32 + strip.width as f32 / 2.0,
            strip.y as f32 + strip.height as f32 / 2.0,
        );
        assert!(ws.tree.hit_test(centre).is_some());
        assert_eq!(ws.tree.scroll_container_at(centre), None);
        let Some(node) = ws.tree.accessibility(ws.options_bar) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), accesskit::Role::Toolbar);
        assert_eq!(node.label(), Some(super::OPTIONS_BAR_LABEL));
    }

    /// Real bug, found on real macOS hardware: a `Role::GenericContainer`
    /// root never anchored into the native window's own accessibility
    /// hierarchy at all (`VoiceOver`'s Rotor came back completely empty,
    /// not even the window title) -- `Role::Window`, matching
    /// `spike/a11y-ime`'s own proven root, is what actually fixed it.
    #[test]
    fn build_workspace_roots_the_tree_as_a_labeled_window() {
        let ws = build_workspace(&test_scales());
        let Some(accessibility) = ws.tree.accessibility(ws.root) else {
            unreachable!("just built");
        };
        assert_eq!(accessibility.role(), accesskit::Role::Window);
        assert_eq!(accessibility.label(), Some("Aurora"));
    }

    #[test]
    fn build_workspace_has_a_canvas_area_and_three_docked_panels() {
        let mut ws = build_workspace(&test_scales());
        assert_eq!(ws.tree.parent(ws.canvas_area), Some(ws.canvas_column));
        assert_eq!(ws.tree.parent(ws.canvas_column), Some(ws.root));
        assert_eq!(ws.tree.parent(ws.options_bar), Some(ws.canvas_column));
        assert_eq!(ws.tree.parent(ws.tools.root), Some(ws.root));
        assert_eq!(ws.tree.parent(ws.rail), Some(ws.root));
        assert_eq!(
            ws.tree.children(ws.rail),
            Some([ws.layers.root, crate::workspace::test_group(&ws).root].as_slice()),
            "the rail docks Layers, then the Properties + History tab group (0.164.0)"
        );
        assert_eq!(
            ws.tree.children(crate::workspace::test_group(&ws).root),
            Some(
                [
                    crate::workspace::test_group(&ws).bar,
                    ws.properties.root,
                    ws.history.root
                ]
                .as_slice()
            ),
            "the group holds its tab row, then Properties and History in mockup order"
        );

        for (panel, title, role) in [
            (ws.layers, "Layers", accesskit::Role::Region),
            (ws.properties, "Properties", accesskit::Role::TabPanel),
            (ws.history, "History", accesskit::Role::TabPanel),
        ] {
            let Some(accessibility) = ws.tree.accessibility(panel.root) else {
                unreachable!("just inserted");
            };
            assert_eq!(accessibility.role(), role);
            assert_eq!(accessibility.label(), Some(title));
        }

        // A real, computed layout -- not just tree shape. A 1000x800
        // viewport: the rail claims its own fixed starting width (250),
        // the zero-width divider claims none, the tools panel its content
        // width (0.160.0), and the canvas column (the only growing
        // element) absorbs the rest; the options bar takes its own
        // height off the top of that column. Height (no explicit size)
        // fills via the parent's own 100% root. Since 0.161.0 the rail is
        // not split in equal thirds: Layers and Properties are
        // content-sized (empty here: a title row plus the viewport's
        // one-row floor) and History, the one `Fill` panel, takes the
        // rest.
        ws.tree.compute_layout(1000.0, 800.0);
        let Some(canvas_bounds) = ws.tree.bounds(ws.canvas_area) else {
            unreachable!("just laid out");
        };
        let Some(rail_bounds) = ws.tree.bounds(ws.rail) else {
            unreachable!("just laid out");
        };
        let tools = tools_width(&ws);
        let bar = options_bar_height(&ws);
        assert!(tools > 0 && bar > 0, "tools {tools}, bar {bar}");
        assert_eq!(canvas_bounds.width, 750 - tools);
        assert_eq!(rail_bounds.width, 250);
        // 0.162.0: the status bar takes one row off the column's bottom.
        let status = bounds_of(&ws, ws.status_bar.root).height;
        assert!(status > 0, "status bar {status}");
        // 0.173.0: the document tab strip takes one row off its top.
        let tabs = bounds_of(&ws, ws.document_tabs).height;
        assert!(tabs > 0, "document tabs {tabs}");
        assert_eq!(canvas_bounds.height, 800 - bar - tabs - status);
        assert_eq!(rail_bounds.height, 800);
        assert_eq!(
            (canvas_bounds.x, canvas_bounds.y),
            (i64::from(tools), i64::from(bar + tabs))
        );

        let Some(layers_bounds) = ws.tree.bounds(ws.layers.root) else {
            unreachable!("just laid out");
        };
        let Some(history_bounds) = ws.tree.bounds(ws.history.root) else {
            unreachable!("just laid out");
        };
        let Some(properties_bounds) = ws.tree.bounds(ws.properties.root) else {
            unreachable!("just laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&test_scales()) as u32;
        let Some(group_bounds) = ws.tree.bounds(crate::workspace::test_group(&ws).root) else {
            unreachable!("just laid out");
        };
        let Some(bar_bounds) = ws.tree.bounds(crate::workspace::test_group(&ws).bar) else {
            unreachable!("just laid out");
        };
        // 0.164.0: Layers is its title plus one row; the group under it is
        // its tab row over the shown tab, Properties — content-sized, and
        // with no title row of its own, so one row — and History, the
        // hidden tab, takes no space.
        assert_eq!(
            (layers_bounds.height, properties_bounds.height),
            (2 * row, row),
            "an empty content-sized panel is its title (or tab) plus one row"
        );
        assert_eq!(bar_bounds.height, row, "the tab row is one row");
        assert_eq!(
            group_bounds.y,
            layers_bounds.bottom(),
            "the group docks under Layers"
        );
        assert_eq!(bar_bounds.y, group_bounds.y, "its tab row first");
        assert_eq!(
            properties_bounds.y,
            bar_bounds.bottom(),
            "then the shown tab"
        );
        assert_eq!(
            group_bounds.height,
            2 * row,
            "one slot: the tab row and Properties"
        );
        assert_eq!(history_bounds.height, 0, "the hidden tab takes no space");
    }

    /// 0.164.0: with the History tab shown, History is the rail's `Fill`
    /// member again, so the group takes what Layers leaves.
    #[test]
    fn the_history_tab_fills_what_layers_leaves() {
        let mut ws = build_workspace(&test_scales());
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&test_scales()) as u32;
        super::show_history_tab(&mut ws);
        ws.tree.compute_layout(1000.0, 800.0);
        let (Some(group_bounds), Some(history_bounds), Some(properties_bounds)) = (
            ws.tree.bounds(crate::workspace::test_group(&ws).root),
            ws.tree.bounds(ws.history.root),
            ws.tree.bounds(ws.properties.root),
        ) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            group_bounds.height,
            800 - 2 * row,
            "the group fills the rest"
        );
        assert_eq!(
            history_bounds.height,
            800 - 3 * row,
            "History under its tab row"
        );
        assert_eq!(
            properties_bounds.height, 0,
            "Properties is now the hidden tab"
        );
    }

    #[test]
    fn build_workspace_gives_the_divider_a_real_splitter_node() {
        let ws = build_workspace(&test_scales());
        assert_eq!(ws.tree.parent(ws.divider), Some(ws.root));
        let Some(accessibility) = ws.tree.accessibility(ws.divider) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::Splitter);
        assert!(
            !accessibility.supports_action(accesskit::Action::Focus),
            "not a Tab stop yet -- no keyboard-driven resize exists to make landing on it \
             meaningful, see this module's own doc comment"
        );
        assert!(accessibility.supports_action(accesskit::Action::SetValue));
        assert_eq!(accessibility.numeric_value(), Some(250.0));
        assert_eq!(
            accessibility.min_numeric_value(),
            Some(RAIL_MIN_WIDTH.into())
        );
        assert_eq!(
            accessibility.max_numeric_value(),
            Some(RAIL_MAX_WIDTH.into())
        );
    }

    #[test]
    fn rail_width_reads_back_the_real_starting_width() {
        let ws = build_workspace(&test_scales());
        assert_eq!(rail_width(&ws.tree, ws.rail), Some(250.0));
    }

    #[test]
    fn set_rail_width_changes_what_rail_width_reads_back_and_the_layout_bounds() {
        let mut ws = build_workspace(&test_scales());

        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 300.0) {
            unreachable!("{err:?}");
        }

        assert_eq!(rail_width(&ws.tree, ws.rail), Some(300.0));
        ws.tree.compute_layout(1000.0, 800.0);
        let Some(rail_bounds) = ws.tree.bounds(ws.rail) else {
            unreachable!("just laid out");
        };
        let Some(canvas_bounds) = ws.tree.bounds(ws.canvas_area) else {
            unreachable!("just laid out");
        };
        assert_eq!(rail_bounds.width, 300);
        assert_eq!(
            canvas_bounds.width,
            700 - tools_width(&ws),
            "the canvas must give back exactly what the rail gained"
        );
    }

    #[test]
    fn set_rail_width_updates_the_dividers_own_accessibility_value() {
        let mut ws = build_workspace(&test_scales());
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 300.0) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = ws.tree.accessibility(ws.divider) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.numeric_value(), Some(300.0));
    }

    #[test]
    fn set_rail_width_clamps_below_the_minimum() {
        let mut ws = build_workspace(&test_scales());
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 10.0) {
            unreachable!("{err:?}");
        }
        assert_eq!(rail_width(&ws.tree, ws.rail), Some(RAIL_MIN_WIDTH));
    }

    #[test]
    fn set_rail_width_clamps_above_the_maximum() {
        let mut ws = build_workspace(&test_scales());
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 5000.0) {
            unreachable!("{err:?}");
        }
        assert_eq!(rail_width(&ws.tree, ws.rail), Some(RAIL_MAX_WIDTH));
    }

    #[test]
    fn set_rail_width_rejects_an_unknown_rail() {
        let mut ws = build_workspace(&test_scales());
        let bogus = accesskit::NodeId(999);
        match set_rail_width(&mut ws.tree, bogus, ws.divider, 300.0) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn set_rail_width_rejects_an_unknown_divider() {
        let mut ws = build_workspace(&test_scales());
        let bogus = accesskit::NodeId(999);
        match set_rail_width(&mut ws.tree, ws.rail, bogus, 300.0) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    // The real, committed, owner-approved scales -- the same file
    // `aurora-theme`'s own tests parse, so the two tests below exercise
    // real token values rather than a synthetic fixture.
    fn test_scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// Fills whichever of the three panels `which` names with `count`
    /// rows each, through the same real `populate_*` entry points
    /// `aurora-app` calls. `which` is `(layers, properties, history)`.
    ///
    /// Every panel goes through a different row style — Layers through
    /// `aurora_widgets::widgets::tree_view`'s, Properties and History
    /// through `crate::panel`'s own shared `row_style` — which is exactly
    /// why the tests below want all three and each one alone.
    fn fill_panels(
        ws: &mut super::Workspace,
        scales: &aurora_theme::Scales,
        which: (bool, bool, bool),
        count: usize,
    ) {
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let (layers_on, properties_on, history_on) = which;
        if layers_on {
            let mut layers = aurora_doc::LayerTree::new();
            for i in 0..count {
                if let Err(err) = layers.add_pixel_layer(format!("Layer {i}"), bounds, None) {
                    unreachable!("{err:?}");
                }
            }
            if let Err(err) = crate::populate_layers_panel(&mut ws.tree, ws.layers, scales, &layers)
            {
                unreachable!("{err:?}");
            }
        }
        if properties_on {
            let options: Vec<(&str, String)> =
                (0..count).map(|i| ("Radius", format!("{i}px"))).collect();
            if let Err(err) = crate::populate_properties_panel(
                &mut ws.tree,
                ws.properties,
                scales,
                crate::Tool::Brush,
                &options,
            ) {
                unreachable!("{err:?}");
            }
        }
        if history_on {
            // `count` rows in all: the origin row plus `count - 1` steps.
            let labels: Vec<String> = (1..count).map(|i| format!("Step {i}")).collect();
            let steps: Vec<_> = labels
                .iter()
                .map(|label| crate::HistoryStep {
                    label,
                    undone: false,
                })
                .collect();
            if let Err(err) =
                crate::populate_history_panel(&mut ws.tree, ws.history, scales, "Open", &steps)
            {
                unreachable!("{err:?}");
            }
        }
    }

    /// The real regression test for the width floor `0.77.3` claimed to
    /// have closed and did not — see [`super::rail_style`] for the
    /// `taffy` mechanism and for why the pins that round put on
    /// `crate::panel`'s own `root_style`/`body_style` were inert.
    ///
    /// **This asserts layout, not styles.** The `0.77.3` test that was
    /// supposed to protect this (`crate::panel`'s own
    /// `a_panels_own_styles_never_impose_a_minimum_size_on_either_axis`)
    /// only reads `min_size` off two `Style`s, which is equally true of
    /// the broken code and the fixed code; mutating the shared row
    /// style's own `min_size.width` survived the entire `aurora-ui`
    /// suite. A 1 px root is what makes the floor observable at all,
    /// since the floor is one row height (21 px) and every realistic
    /// window is far wider.
    ///
    /// All three panels are exercised together and each one alone,
    /// because all three floored the rail identically and a fix aimed at
    /// only the shared panel row style would have left Layers broken.
    #[test]
    fn a_populated_panel_never_floors_the_rails_own_width_to_a_row_height() {
        let scales = test_scales();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let one_row = aurora_widgets::widgets::row_height(&scales) as u32;
        assert_eq!(one_row, 21, "the floor under test, in logical px");

        for which in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
            (true, true, true),
        ] {
            let mut ws = build_workspace(&test_scales());
            fill_panels(&mut ws, &scales, which, 5);
            // 0.160.0: the tools panel never shrinks, so "a 1 px
            // window" is now 1 px beyond the tools panel's own width.
            #[allow(clippy::cast_precision_loss)]
            let tools = (2 * scales.spacing.xs + 2 * scales.spacing.md) as f32;
            ws.tree.compute_layout(tools + 1.0, 200.0);

            let Some(rail) = ws.tree.bounds(ws.rail) else {
                unreachable!("just laid out");
            };
            assert_eq!(
                rail.width, 1,
                "a rail must take its width from its own style and the space it is given, \
                 never from a row inside it (layers/properties/history: {which:?}): {rail:?}"
            );
        }
    }

    /// All three panels crowded at once, which nothing committed covered
    /// before `0.77.5`: each panel's own crowding test populates one
    /// panel and leaves the other two empty, so "Layers is fine and
    /// History is fine, separately" was the whole of the evidence that
    /// the rail divides correctly under a realistic combined load.
    #[test]
    fn all_three_panels_crowded_at_once_still_share_the_rail_and_stay_hittable() {
        let scales = test_scales();
        // 0.164.0: Properties and History are tabs of one slot, so each
        // count is checked with each tab shown; the hidden one is skipped.
        for (count, tab) in [
            (5_usize, 0_usize),
            (5, 1),
            (60, 0),
            (60, 1),
            (200, 0),
            (200, 1),
        ] {
            let mut ws = build_workspace(&test_scales());
            fill_panels(&mut ws, &scales, (true, true, true), count);
            let group = crate::workspace::test_group(&ws);
            if let Err(err) = crate::show_panel_group_tab(&mut ws.tree, &group, tab) {
                unreachable!("{err:?}");
            }
            let hidden = if tab == 0 { "history" } else { "properties" };
            ws.tree.compute_layout(1600.0, 900.0);

            let (Some(layers), Some(properties), Some(history)) = (
                ws.tree.bounds(ws.layers.root),
                ws.tree.bounds(ws.properties.root),
                ws.tree.bounds(ws.history.root),
            ) else {
                unreachable!("just laid out");
            };

            // 0.161.0: no longer equal thirds. Layers and Properties take
            // their content, up to `size.content_panel_max_rows` rows each;
            // History, the `Fill` panel, takes the rest.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let row = aurora_widgets::widgets::row_height(&scales) as u32;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cap = (1 + crate::panel::content_panel_max_rows(&scales) as u32) * row;
            for (name, panel, body) in [
                ("layers", layers, ws.layers.body),
                ("properties", properties, ws.properties.body),
                ("history", history, ws.history.body),
            ]
            .into_iter()
            .filter(|(name, ..)| *name != hidden)
            {
                assert!(
                    name == "history" || panel.height <= cap,
                    "{name} is capped at its title plus the row cap at {count} rows: {panel:?}"
                );
                let Some(body) = ws.tree.bounds(body) else {
                    unreachable!("just laid out");
                };
                assert!(
                    body.height >= row,
                    "{name}'s body keeps at least one row at {count} rows: {body:?}"
                );
            }
            if count >= 60 {
                for (name, body) in [
                    ("layers", ws.layers.body),
                    ("properties", ws.properties.body),
                    ("history", ws.history.body),
                ]
                .into_iter()
                .filter(|(name, _)| *name != hidden)
                {
                    assert!(
                        ws.tree.scroll_range(body).is_some_and(|range| range > 0.0),
                        "{name} scrolls at {count} rows"
                    );
                }
            }
            assert!(
                history.y + i64::from(history.height) <= 900,
                "no panel may be pushed off the bottom of the window: {history:?}"
            );

            let mut previous: Option<aurora_core::Rect> = None;
            for (name, panel) in [
                ("layers", layers),
                ("properties", properties),
                ("history", history),
            ]
            .into_iter()
            .filter(|(name, _)| *name != hidden)
            {
                if let Some(before) = previous {
                    assert!(
                        panel.y >= before.y + i64::from(before.height),
                        "{name} must start at or below the panel above it, never overlap it: \
                         {before:?}, {panel:?}"
                    );
                }
                previous = Some(panel);

                #[allow(clippy::cast_precision_loss)]
                let point = (
                    (panel.x + i64::from(panel.width) / 2) as f32,
                    (panel.y + i64::from(panel.height) / 2) as f32,
                );
                assert!(
                    ws.tree.hit_test(point).is_some(),
                    "{name} must stay hit-testable at {count} rows each: {panel:?}"
                );
            }
        }
    }

    /// The 0.161.0 regression set: the Properties panel's Curves strip in
    /// the real rail, shown the way `aurora-app` shows it.
    fn with_curves_shown(
        ws: &mut super::Workspace,
        scales: &aurora_theme::Scales,
    ) -> crate::ToolControls {
        let controls = match crate::insert_tool_controls(
            &mut ws.tree,
            ws.options_bar,
            ws.properties,
            scales,
        ) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut layers = aurora_doc::LayerTree::new();
        let curves = match layers.add_adjustment_layer_at(
            "Curves",
            aurora_doc::Adjustment::Curves(aurora_core::CurvesParams::identity()),
            None,
            0,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = crate::sync_curves_controls(
            &mut ws.tree,
            controls.curves,
            &layers,
            Some(curves),
            None,
            None,
        ) {
            unreachable!("{err:?}");
        }
        controls
    }

    fn bottom(rect: aurora_core::Rect) -> i64 {
        rect.y + i64::from(rect.height)
    }

    /// Asserts the three docked panels stack inside the rail, each one
    /// starting at or below the one above's bottom (the 0.144.1 lesson).
    fn assert_stacked_inside_the_rail(ws: &super::Workspace, what: &str) {
        let Some(rail) = ws.tree.bounds(ws.rail) else {
            unreachable!("laid out");
        };
        let mut previous = rail.y;
        // 0.164.0: the rail's slots are Layers, then the Properties +
        // History group — its tab row over whichever tab is shown.
        let shown = match crate::panel_group_shown(&ws.tree, &crate::workspace::test_group(ws)) {
            Some(0) => ("properties", ws.properties.root),
            Some(_) => ("history", ws.history.root),
            None => unreachable!("{what}: no tab is shown"),
        };
        for (name, root) in [
            ("layers", ws.layers.root),
            ("tab row", crate::workspace::test_group(ws).bar),
            shown,
        ] {
            let Some(bounds) = ws.tree.bounds(root) else {
                unreachable!("laid out");
            };
            assert!(
                bounds.y >= previous,
                "{what}: {name} overlaps the panel above: {bounds:?}, above ends at {previous}"
            );
            assert!(
                bottom(bounds) <= bottom(rail),
                "{what}: {name} overflows the rail: {bounds:?} vs {rail:?}"
            );
            previous = bottom(bounds);
        }
    }

    /// AC-1/AC-4: at the design owner's window (2548 x 1344 physical at
    /// scale 2, so 1274 x 672 logical — and 604 for the content area the
    /// screenshot measured under the macOS title bar) with the Widget
    /// Gallery open, the Curves editor gets its full square and its tabs,
    /// inside the Properties panel, above History. Before 0.161.0 the
    /// strip overflowed History by 18 px (672) and 40 px (604).
    #[test]
    fn the_curves_editor_gets_its_full_height_in_the_reported_window() {
        let scales = test_scales();
        let side = crate::gallery_panel::gallery_editor_size(&scales);
        for height in [672.0, 604.0, 480.0] {
            let mut ws = build_workspace(&scales);
            let controls = with_curves_shown(&mut ws, &scales);
            fill_panels(&mut ws, &scales, (true, false, true), 3);
            if let Err(err) = crate::insert_gallery_panel(&mut ws.tree, ws.root, &scales) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(1274.0, height);
            let what = format!("1274 x {height}");
            assert_stacked_inside_the_rail(&ws, &what);
            let (Some(properties), Some(tabs), Some(editor), Some(history)) = (
                ws.tree.bounds(ws.properties.root),
                ws.tree.bounds(controls.curves.channel),
                ws.tree.bounds(controls.curves.editor),
                ws.tree.bounds(ws.history.root),
            ) else {
                unreachable!("laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            let (w, h) = (editor.width as f32, editor.height as f32);
            assert!(
                (w - side).abs() < 1.0 && (h - side).abs() < 1.0,
                "{what}: the plot keeps its full square: {editor:?}"
            );
            assert!(tabs.height > 0 && tabs.width > 0, "{what}: {tabs:?}");
            assert!(
                tabs.y >= properties.y && bottom(editor) <= bottom(properties),
                "{what}: tabs and plot sit inside the Properties panel: {tabs:?} {editor:?} \
                 {properties:?}"
            );
            // 0.164.0: History is the hidden tab of the same slot, so the
            // plot has the rest of the rail; it must end inside it.
            assert_eq!(history.height, 0, "{what}: History is the hidden tab");
            let Some(rail) = ws.tree.bounds(ws.rail) else {
                unreachable!("laid out");
            };
            assert!(
                bottom(editor) <= bottom(rail),
                "{what}: the plot ends inside the rail: {editor:?} {rail:?}"
            );
            assert_eq!(
                ws.tree.scroll_range(controls.root),
                Some(0.0),
                "{what}: nothing to scroll while it fits"
            );
        }
    }

    /// AC-1/AC-4: a rail too short for everything shrinks the Curves strip
    /// — never to zero — and makes it scroll, so the editor stays
    /// reachable, while no panel overlaps another or leaves the rail.
    #[test]
    fn a_short_rail_scrolls_the_curves_strip_instead_of_squeezing_it_away() {
        let scales = test_scales();
        let row = aurora_widgets::widgets::row_height(&scales);
        for height in [300.0, 200.0] {
            let mut ws = build_workspace(&scales);
            let controls = with_curves_shown(&mut ws, &scales);
            fill_panels(&mut ws, &scales, (true, false, true), 20);
            ws.tree.compute_layout(1274.0, height);
            let what = format!("1274 x {height}");
            assert_stacked_inside_the_rail(&ws, &what);
            let Some(strip) = ws.tree.bounds(controls.root) else {
                unreachable!("laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            let strip_height = strip.height as f32;
            assert!(
                strip_height >= row,
                "{what}: the strip keeps at least one row: {strip:?}"
            );
            assert!(
                ws.tree
                    .scroll_range(controls.root)
                    .is_some_and(|range| range > 0.0),
                "{what}: the squeezed strip scrolls"
            );
            assert_eq!(
                ws.tree.is_scrollable(controls.root),
                Some(true),
                "{what}: the wheel reaches it (scroll_container_at)"
            );
        }
    }

    /// 0.162.0 AC-1/AC-4: the status bar is exactly one `row_height` tall
    /// (a token), spans the canvas column under the canvas area, ends at
    /// the window's bottom edge, and overlaps nothing — at ordinary,
    /// narrow and short windows. In a window too short for both bars the
    /// canvas is squeezed to nothing first; in one too narrow for the
    /// bar's own padding it overhangs its column, like the options bar.
    #[test]
    fn the_status_bar_is_one_token_row_under_the_canvas_and_overlaps_nothing() {
        let scales = test_scales();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&scales) as u32;
        for (window, rail) in [
            ((1000.0_f32, 800.0_f32), 250.0),
            ((1600.0, 900.0), RAIL_MAX_WIDTH),
            ((480.0, 480.0), 250.0),
            ((120.0, 200.0), RAIL_MIN_WIDTH),
            ((1000.0, 60.0), 250.0),
            ((1000.0, 40.0), 250.0),
        ] {
            let mut ws = build_workspace(&scales);
            if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, rail) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(window.0, window.1);
            let case = format!("window {window:?}, rail {rail}");
            let column = bounds_of(&ws, ws.canvas_column);
            let bar = bounds_of(&ws, ws.options_bar);
            let canvas = bounds_of(&ws, ws.canvas_area);
            let status = bounds_of(&ws, ws.status_bar.root);
            let rail_bounds = bounds_of(&ws, ws.rail);
            let tools = bounds_of(&ws, ws.tools.root);

            assert_eq!(status.height, row, "{case}: one token row: {status:?}");
            assert_eq!(status.x, column.x, "{case}");
            assert_eq!(
                status.y,
                canvas.y + i64::from(canvas.height),
                "{case}: directly under the canvas area"
            );
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let window_height = window.1 as u32;
            if window_height >= bar.height + row {
                assert_eq!(
                    status.y + i64::from(status.height),
                    i64::from(window_height),
                    "{case}: along the window's bottom edge"
                );
            } else {
                assert_eq!(canvas.height, 0, "{case}: the canvas gives way first");
            }
            if column.width >= 2 * scales.spacing.sm {
                assert_eq!(status.width, column.width, "{case}");
                assert!(!overlaps(status, rail_bounds), "{case}: status/rail");
            }
            assert!(!overlaps(status, canvas), "{case}: status/canvas");
            assert!(!overlaps(status, bar), "{case}: status/options bar");
            assert!(!overlaps(status, tools), "{case}: status/tools");
            for id in [ws.status_bar.zoom, ws.status_bar.document] {
                let item = bounds_of(&ws, id);
                assert!(
                    item.y >= status.y
                        && item.y + i64::from(item.height) <= status.y + i64::from(status.height),
                    "{case}: an item stays inside the bar's row: {item:?} in {status:?}"
                );
            }
        }
    }

    /// 0.162.0 AC-3: the Layers cap is the `size.content_panel_max_rows`
    /// token, not a constant — changing the token changes the cap.
    #[test]
    fn the_layers_cap_follows_the_content_panel_max_rows_token() {
        let default_scales = test_scales();
        assert_eq!(default_scales.size.content_panel_max_rows, 10);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&default_scales) as u32;
        for rows in [10_u32, 4, 15] {
            let mut scales = test_scales();
            scales.size.content_panel_max_rows = rows;
            let mut ws = build_workspace(&scales);
            fill_panels(&mut ws, &scales, (true, false, false), 200);
            ws.tree.compute_layout(1600.0, 1200.0);
            let layers = bounds_of(&ws, ws.layers.root);
            assert_eq!(
                layers.height,
                (1 + rows) * row,
                "a title row plus {rows} body rows: {layers:?}"
            );
        }
    }

    /// AC-4: Layers with many rows is capped at `size.content_panel_max_rows` and
    /// scrolls, leaving the Curves editor its full square.
    #[test]
    fn a_long_layers_list_is_capped_and_scrolls_beside_the_curves_editor() {
        let scales = test_scales();
        let side = crate::gallery_panel::gallery_editor_size(&scales);
        let mut ws = build_workspace(&scales);
        let controls = with_curves_shown(&mut ws, &scales);
        fill_panels(&mut ws, &scales, (true, false, true), 200);
        ws.tree.compute_layout(1274.0, 672.0);
        assert_stacked_inside_the_rail(&ws, "200 layers");
        let (Some(layers), Some(editor)) = (
            ws.tree.bounds(ws.layers.root),
            ws.tree.bounds(controls.curves.editor),
        ) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&scales) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let cap = (1 + crate::panel::content_panel_max_rows(&scales) as u32) * row;
        assert!(layers.height <= cap, "capped at the row cap: {layers:?}");
        assert!(
            ws.tree
                .scroll_range(ws.layers.body)
                .is_some_and(|range| range > 0.0),
            "the Layers body scrolls"
        );
        #[allow(clippy::cast_precision_loss)]
        let h = editor.height as f32;
        assert!((h - side).abs() < 1.0, "{editor:?}");
    }

    /// AC-4: the smallest window the app allows (640 x 480), narrow and
    /// short, with the gallery open: still stacked, no overlap.
    #[test]
    fn the_minimum_window_with_the_gallery_open_keeps_the_rail_stacked() {
        let scales = test_scales();
        let mut ws = build_workspace(&scales);
        let _ = with_curves_shown(&mut ws, &scales);
        fill_panels(&mut ws, &scales, (true, true, true), 30);
        if let Err(err) = crate::insert_gallery_panel(&mut ws.tree, ws.root, &scales) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(640.0, 480.0);
        assert_stacked_inside_the_rail(&ws, "640 x 480");
    }

    /// AC-4: a rail barely taller than every panel's floor (title, one
    /// body row, one row per controls strip: 63 + 63 + 42 = 168 px) holds
    /// every part of every panel at one row or more, each inside its own
    /// panel — the Curves strip and the Layers controls strip included.
    /// The floor is what M9/M10/M11 of 0.161.0's mutation matrix remove.
    #[test]
    fn a_rail_at_its_panels_floors_keeps_every_part_one_row_and_inside_its_panel() {
        let scales = test_scales();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = aurora_widgets::widgets::row_height(&scales) as u32;
        let mut ws = build_workspace(&scales);
        let curves = with_curves_shown(&mut ws, &scales);
        let layer_controls = match crate::insert_layer_controls(&mut ws.tree, ws.layers, &scales) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        fill_panels(&mut ws, &scales, (true, false, true), 20);
        ws.tree.compute_layout(1274.0, 172.0);
        assert_stacked_inside_the_rail(&ws, "1274 x 172");
        // 0.164.0: History is the other tab of Properties' slot; it is
        // checked with its own tab shown, below.
        let mut history_shown = build_workspace(&scales);
        fill_panels(&mut history_shown, &scales, (true, false, true), 20);
        super::show_history_tab(&mut history_shown);
        history_shown.tree.compute_layout(1274.0, 172.0);
        assert_stacked_inside_the_rail(&history_shown, "1274 x 172, History shown");
        let (Some(rows), Some(panel)) = (
            history_shown.tree.bounds(history_shown.history.viewport),
            history_shown.tree.bounds(history_shown.history.root),
        ) else {
            unreachable!("laid out");
        };
        assert!(rows.height >= row, "history rows keep one row: {rows:?}");
        assert!(rows.y >= panel.y && bottom(rows) <= bottom(panel));
        for (name, part, panel) in [
            ("layers rows", ws.layers.viewport, ws.layers.root),
            ("layers controls", layer_controls.root, ws.layers.root),
            (
                "properties rows",
                ws.properties.viewport,
                ws.properties.root,
            ),
            ("curves strip", curves.root, ws.properties.root),
        ] {
            let (Some(part), Some(panel)) = (ws.tree.bounds(part), ws.tree.bounds(panel)) else {
                unreachable!("laid out");
            };
            assert!(part.height >= row, "{name} keeps one row: {part:?}");
            assert!(
                part.y >= panel.y && bottom(part) <= bottom(panel),
                "{name} stays inside its panel: {part:?} vs {panel:?}"
            );
        }
    }
    /// 0.161.0 review J3: the sizing lives in the tree, so collapsing and
    /// expanding Properties — through the workspace's own handle or any
    /// copy of it — keeps it content-sized, and the Curves editor keeps
    /// its full square afterwards.
    #[test]
    fn properties_stays_content_sized_across_a_collapse_and_expand() {
        let scales = test_scales();
        let side = crate::gallery_panel::gallery_editor_size(&scales);
        let mut ws = build_workspace(&scales);
        let controls = with_curves_shown(&mut ws, &scales);
        let copy = ws.properties;
        for handle in [ws.properties, copy] {
            for collapsed in [true, false] {
                if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, handle, collapsed) {
                    unreachable!("{err:?}");
                }
            }
            assert_eq!(
                crate::panel_sizing(&ws.tree, ws.properties).ok(),
                Some(crate::PanelSizing::Content)
            );
            let Some(root) = ws.tree.style(ws.properties.root) else {
                unreachable!("inserted");
            };
            assert!(root.flex_basis.is_auto(), "content basis after expand");
            assert!(
                root.flex_grow.abs() < f32::EPSILON,
                "no growth after expand"
            );
        }
        assert_eq!(
            crate::panel_sizing(&ws.tree, ws.history).ok(),
            Some(crate::PanelSizing::Fill)
        );
        ws.tree.compute_layout(1274.0, 672.0);
        let Some(editor) = ws.tree.bounds(controls.curves.editor) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let h = editor.height as f32;
        assert!((h - side).abs() < 1.0, "{editor:?}");
        assert_stacked_inside_the_rail(&ws, "after collapse and expand");
    }

    /// 0.165.0 AC-1/AC-6: collapsed, the rail and its divider take no
    /// space, the label strip sits at the window's right edge with a width
    /// from spacing tokens alone (text-blind: each button is its
    /// `spacing.md` padding, the strip `spacing.xs` either side, the tools
    /// panel's own style), its buttons stack inside it, and the canvas
    /// column takes exactly the width the rail gave back.
    #[test]
    fn collapsing_the_rail_shows_a_token_sized_strip_and_widens_the_canvas() {
        let scales = test_scales();
        let (xs, md) = (scales.spacing.xs, scales.spacing.md);
        for (window, rail) in [
            ((1000.0, 800.0), 250.0),
            ((1600.0, 900.0), RAIL_MAX_WIDTH),
            ((1274.0, 672.0), RAIL_MIN_WIDTH),
            ((480.0, 480.0), 250.0),
        ] {
            let case = format!("window {window:?}, rail {rail}");
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rail_px = rail as u32;
            let mut ws = build_workspace(&scales);
            if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, rail) {
                unreachable!("{err:?}");
            }
            ws.tree.compute_layout(window.0, window.1);
            let expanded_canvas = bounds_of(&ws, ws.canvas_area).width;
            assert_eq!(bounds_of(&ws, ws.panel_strip.root).width, 0, "{case}");
            assert!(matches!(super::set_rail_collapsed(&mut ws, true), Ok(true)));
            assert!(super::rail_collapsed(&ws));
            ws.tree.compute_layout(window.0, window.1);
            let tools = bounds_of(&ws, ws.tools.root);
            let column = bounds_of(&ws, ws.canvas_column);
            let canvas = bounds_of(&ws, ws.canvas_area);
            let strip = bounds_of(&ws, ws.panel_strip.root);
            assert_eq!(strip.width, 2 * xs + 2 * md, "{case}: {strip:?}");
            #[allow(clippy::cast_possible_truncation)]
            let right = window.0 as i64;
            assert_eq!(strip.x + i64::from(strip.width), right, "{case}");
            assert_eq!(
                column.x + i64::from(column.width),
                strip.x,
                "{case}: the canvas column ends where the strip begins"
            );
            assert_eq!(bounds_of(&ws, ws.rail).width, 0, "{case}: rail hidden");
            assert_eq!(bounds_of(&ws, ws.divider).width, 0, "{case}");
            assert_eq!(
                canvas.width,
                expanded_canvas + rail_px - strip.width,
                "{case}: the canvas gains the rail's width less the strip's"
            );
            for (name, a, b) in [
                ("tools/strip", tools, strip),
                ("column/strip", column, strip),
                ("canvas/strip", canvas, strip),
            ] {
                assert!(!overlaps(a, b), "{case}: {name} overlap: {a:?} {b:?}");
            }
            let mut previous: Option<aurora_core::Rect> = None;
            for (_, id) in ws.panel_strip.buttons {
                let button = bounds_of(&ws, id);
                assert!(button.width > 0 && button.height > 0, "{case}");
                assert!(
                    button.x >= strip.x && button.right() <= strip.right(),
                    "{case}: {button:?} outside {strip:?}"
                );
                if let Some(before) = previous {
                    assert!(button.y >= before.bottom(), "{case}: stacked");
                }
                previous = Some(button);
            }
        }
    }

    /// 0.165.0 AC-1: with the text engine, the strip is its widest
    /// measured label plus the button's `spacing.md` padding plus the
    /// strip's `spacing.xs` either side — narrower than the narrowest rail,
    /// and every button the same height (one row plus padding).
    #[test]
    fn the_strips_width_is_its_widest_measured_label_plus_spacing_tokens() {
        let scales = test_scales();
        let (xs, md) = (scales.spacing.xs, scales.spacing.md);
        let mut ws = build_workspace(&scales);
        if let Err(err) = super::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        let Ok(mut engine) = aurora_text::TextEngine::new() else {
            unreachable!("the bundled font loads")
        };
        aurora_widgets::compute_text_layout(
            &mut ws.tree,
            1000.0,
            800.0,
            Some(aurora_widgets::TextMeasure {
                engine: &mut engine,
                scales: &scales,
                scale_factor: 1.0,
            }),
        );
        let strip = bounds_of(&ws, ws.panel_strip.root);
        let widest = ws
            .panel_strip
            .buttons
            .iter()
            .map(|(_, id)| bounds_of(&ws, *id).width)
            .max()
            .unwrap_or(0);
        assert!(widest > 2 * md, "the label is measured: {widest}");
        assert_eq!(strip.width, widest + 2 * xs, "{strip:?}");
        #[allow(clippy::cast_precision_loss)]
        let strip_width = strip.width as f32;
        assert!(strip_width < RAIL_MIN_WIDTH, "a narrow strip: {strip:?}");
        let column = bounds_of(&ws, ws.canvas_column);
        assert_eq!(column.right(), strip.x);
    }

    /// 0.165.0 AC-5: the rail's width is remembered across collapse and
    /// expand, a width change while collapsed keeps it collapsed, and the
    /// divider is hidden (no layout, AT-`hidden`) while collapsed.
    #[test]
    fn the_rail_width_survives_a_collapse_and_the_divider_hides_with_the_rail() {
        let mut ws = build_workspace(&test_scales());
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 320.0) {
            unreachable!("{err:?}");
        }
        assert!(matches!(super::toggle_rail_collapsed(&mut ws), Ok(true)));
        let hidden = |ws: &super::Workspace, id| {
            ws.tree
                .accessibility(id)
                .is_some_and(accesskit::Node::is_hidden)
        };
        assert!(hidden(&ws, ws.divider), "the divider leaves the AT tree");
        assert!(hidden(&ws, ws.rail), "so does the rail");
        assert!(!hidden(&ws, ws.panel_strip.root), "the strip joins it");
        assert_eq!(rail_width(&ws.tree, ws.rail), Some(320.0));
        if let Err(err) = set_rail_width(&mut ws.tree, ws.rail, ws.divider, 300.0) {
            unreachable!("{err:?}");
        }
        assert!(
            super::rail_collapsed(&ws),
            "a width change keeps it collapsed"
        );
        assert!(matches!(
            super::set_rail_collapsed(&mut ws, true),
            Ok(false)
        ));
        assert!(matches!(super::toggle_rail_collapsed(&mut ws), Ok(false)));
        assert!(!hidden(&ws, ws.divider) && !hidden(&ws, ws.rail));
        assert!(hidden(&ws, ws.panel_strip.root));
        ws.tree.compute_layout(1000.0, 800.0);
        assert_eq!(bounds_of(&ws, ws.rail).width, 300, "the width came back");
        assert_eq!(bounds_of(&ws, ws.panel_strip.root).width, 0);
    }

    /// 0.165.0 AC-2: a strip button expands the rail with its own panel
    /// shown — Layers expanded, or the group's tab for Properties/History
    /// selected and the group expanded — and a closed panel stays closed.
    #[test]
    fn expand_rail_showing_opens_the_buttons_own_panel_or_tab() {
        let mut ws = build_workspace(&test_scales());
        let history = ws.history;
        let group = crate::workspace::test_group(&ws);
        for (panel, tab) in [
            (ws.history, Some(1)),
            (ws.properties, Some(0)),
            (ws.layers, None),
        ] {
            if let Err(err) = crate::set_panel_group_collapsed(&mut ws.tree, &group, true)
                .and_then(|()| crate::set_panel_collapsed(&mut ws.tree, ws.layers, true))
                .and_then(|()| super::set_rail_collapsed(&mut ws, true).map(|_| ()))
            {
                unreachable!("{err:?}");
            }
            assert!(matches!(
                super::expand_rail_showing(&mut ws, panel),
                Ok(true)
            ));
            assert!(!super::rail_collapsed(&ws));
            assert!(matches!(
                crate::panel_is_collapsed(&ws.tree, panel),
                Ok(false)
            ));
            if let Some(tab) = tab {
                assert_eq!(crate::panel_group_shown(&ws.tree, &group), Some(tab));
            }
        }
        if let Err(err) = super::close_workspace_panel(&mut ws, history)
            .and_then(|()| super::set_rail_collapsed(&mut ws, true).map(|_| ()))
        {
            unreachable!("{err:?}");
        }
        assert!(matches!(
            super::expand_rail_showing(&mut ws, history),
            Ok(true)
        ));
        assert!(!super::rail_collapsed(&ws), "the rail still expands");
        assert!(matches!(
            crate::panel_is_closed(&ws.tree, ws.history),
            Ok(true)
        ));
    }

    /// 0.165.0 AC-2/AC-3: collapsing moves focus from inside the rail to
    /// the strip button of the panel that held it; expanding moves focus
    /// from a strip button to that button's panel (Layers itself, or the
    /// group's selected tab); and `Tab` never enters the hidden side.
    #[test]
    fn focus_follows_a_rail_collapse_to_the_strip_and_back() {
        let mut ws = build_workspace(&test_scales());
        let history = ws.history;
        let mut focus = FocusManager::new();
        let Some(lay) = ws.panel_strip.button_for(ws.layers) else {
            unreachable!("built");
        };
        if let Err(err) = focus.focus(&mut ws.tree, ws.layers.root) {
            unreachable!("{err:?}");
        }
        if let Err(err) = super::set_rail_collapsed(&mut ws, true) {
            unreachable!("{err:?}");
        }
        assert!(super::refocus_workspace(&mut ws, &mut focus));
        assert_eq!(focus.focused(), Some(lay));
        // `Tab` cycles the tools and the strip, never the hidden rail.
        for _ in 0..12 {
            if let Some(id) = focus.focus_next(&mut ws.tree) {
                assert!(
                    !ws.tree.is_within(ws.rail, id),
                    "{id:?} is in the hidden rail"
                );
            }
        }
        let Some(hist) = ws.panel_strip.button_for(ws.history) else {
            unreachable!("built");
        };
        if let Err(err) = focus
            .focus(&mut ws.tree, hist)
            .and_then(|()| super::expand_rail_showing(&mut ws, history).map(|_| ()))
        {
            unreachable!("{err:?}");
        }
        assert!(super::refocus_workspace(&mut ws, &mut focus));
        let selected_tab =
            aurora_widgets::widgets::tab_bar_state(&ws.tree, crate::workspace::test_group(&ws).bar)
                .ok()
                .and_then(aurora_widgets::widgets::TabBarState::selected_tab);
        assert_eq!(focus.focused(), selected_tab, "the History tab");
        // Focus in the shown History tab moves to Hist on collapse.
        if let Err(err) = focus
            .focus(&mut ws.tree, ws.history.root)
            .and_then(|()| super::set_rail_collapsed(&mut ws, true).map(|_| ()))
        {
            unreachable!("{err:?}");
        }
        assert!(super::refocus_workspace(&mut ws, &mut focus));
        assert_eq!(focus.focused(), Some(hist));
        for _ in 0..12 {
            if let Some(id) = focus.focus_next(&mut ws.tree) {
                assert!(
                    !ws.tree.is_within(ws.rail, id),
                    "{id:?} is in the hidden rail"
                );
            }
        }
        // And once expanded, `Tab` never lands on the hidden strip.
        if let Err(err) = super::set_rail_collapsed(&mut ws, false) {
            unreachable!("{err:?}");
        }
        let _ = super::refocus_workspace(&mut ws, &mut focus);
        for _ in 0..40 {
            if let Some(id) = focus.focus_next(&mut ws.tree) {
                assert!(
                    !ws.tree.is_within(ws.panel_strip.root, id),
                    "{id:?} is in the hidden strip"
                );
            }
        }
    }

    /// 0.173.0 (AC-4): the strip is a `Role::TabList` "Documents" between
    /// the options bar and the canvas, its tabs named after the documents,
    /// and the canvas area its `Role::TabPanel` labelled by the selected
    /// tab — relinked on every sync, never to a removed tab.
    #[test]
    fn the_document_tab_strip_is_a_tab_list_and_the_canvas_its_tab_panel() {
        let mut ws = super::build_workspace(&test_scales());
        assert_eq!(ws.tree.parent(ws.document_tabs), Some(ws.canvas_column));
        let children = ws
            .tree
            .children(ws.canvas_column)
            .unwrap_or_default()
            .to_vec();
        let position = |id| children.iter().position(|&child| child == id);
        assert_eq!(position(ws.options_bar), Some(0));
        assert_eq!(position(ws.document_tabs), Some(1));
        assert_eq!(position(ws.canvas_area), Some(2));
        let bar = ws.tree.accessibility(ws.document_tabs).cloned();
        assert_eq!(
            bar.as_ref().map(accesskit::Node::role),
            Some(accesskit::Role::TabList)
        );
        assert_eq!(
            bar.as_ref().and_then(accesskit::Node::label),
            Some(crate::DOCUMENT_TABS_LABEL)
        );
        let names = vec!["a.png".to_owned(), "b.psd".to_owned(), "c.aur".to_owned()];
        if let Err(err) =
            crate::sync_document_tabs(&mut ws.tree, ws.document_tabs, ws.canvas_area, names, 2)
        {
            unreachable!("{err:?}");
        }
        let tabs = match aurora_widgets::widgets::tab_bar_state(&ws.tree, ws.document_tabs) {
            Ok(state) => state.tabs().to_vec(),
            Err(err) => unreachable!("{err:?}"),
        };
        let labels: Vec<Option<String>> = tabs
            .iter()
            .map(|&tab| {
                ws.tree
                    .accessibility(tab)
                    .filter(|node| node.role() == accesskit::Role::Tab)
                    .and_then(|node| node.label().map(str::to_owned))
            })
            .collect();
        assert_eq!(
            labels,
            [
                Some("a.png".to_owned()),
                Some("b.psd".to_owned()),
                Some("c.aur".to_owned())
            ]
        );
        let panel = ws.tree.accessibility(ws.canvas_area).cloned();
        assert_eq!(
            panel.as_ref().map(accesskit::Node::role),
            Some(accesskit::Role::TabPanel)
        );
        assert_eq!(
            panel.as_ref().map(|node| node.labelled_by().to_vec()),
            Some(tabs.get(2).copied().into_iter().collect::<Vec<_>>())
        );
        // Shrinking to one relinks to the surviving tab.
        if let Err(err) = crate::sync_document_tabs(
            &mut ws.tree,
            ws.document_tabs,
            ws.canvas_area,
            vec!["a.png".to_owned()],
            0,
        ) {
            unreachable!("{err:?}");
        }
        let panel = ws.tree.accessibility(ws.canvas_area).cloned();
        assert_eq!(
            panel.map(|node| node.labelled_by().to_vec()),
            Some(tabs.first().copied().into_iter().collect::<Vec<_>>())
        );
        let Some(&first) = tabs.first() else {
            unreachable!("three tabs")
        };
        assert!(crate::is_document_tab(&ws.tree, ws.document_tabs, first));
        assert!(!crate::is_document_tab(
            &ws.tree,
            ws.document_tabs,
            ws.canvas_area
        ));
    }
}
