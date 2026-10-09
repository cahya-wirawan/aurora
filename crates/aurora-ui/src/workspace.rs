//! The main workspace layout: canvas area + a side rail of docked
//! panels, matching the owner-approved workspace mockup
//! (`design/mockups/workspace.html`). PLAN.md M1.8's docking/panels
//! bullet, first slice.
//!
//! **Mostly static** — see [`crate::panel`]'s own doc comment for what's
//! deliberately not here yet (drag-to-redock, persisted layouts).
//! [`rail_width`]/[`set_rail_width`] are the one real piece of dock
//! interactivity so far — dragging the rail's own width is real
//! pointer-driven interaction `aurora-app` owns, this module only
//! exposes the pure layout half (see both functions' own doc comments).
//! The menubar and status bar the mockup also shows are still left out:
//! they belong to other, separate M1.8 bullets (native menus, general
//! chrome).
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
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::TaffyZero as _;
use taffy::{Dimension, FlexDirection, Style};

use crate::panel::{PanelHandle, insert_panel};
use crate::tool::Tool;
use crate::tools_panel::{ToolsPanel, insert_tools_panel};

/// The options bar's accessible label.
pub const OPTIONS_BAR_LABEL: &str = "Tool options";

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
const RAIL_WIDTH_DEFAULT: f32 = 250.0;

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
    /// Where the document canvas will render — `Canvas: infinite zoom,
    /// rotation, pan, ...` is a separate, still-open M1.8 bullet; this
    /// is an empty container reserving its place in the layout.
    pub canvas_area: WidgetId,
    /// The boundary between [`Self::canvas_area`] and [`Self::rail`] —
    /// a real `Role::Splitter`, [`rail_width`]/[`set_rail_width`]'s own
    /// target. Currently zero-width in the tree (no pixel rendering
    /// exists yet to draw a grab handle), same "real node, no pixels
    /// yet" gap every widget here already has.
    pub divider: WidgetId,
    /// The side rail — a fixed-width dock area holding the three
    /// panels below, stacked. Resizable via [`set_rail_width`]; still
    /// no drag-to-redock or persisted width across sessions (see this
    /// module's own doc comment).
    pub rail: WidgetId,
    pub layers: PanelHandle,
    pub properties: PanelHandle,
    pub history: PanelHandle,
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

    let (canvas_column, options_bar, canvas_area) = insert_canvas_column(&mut tree, root, scales);

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
    let properties = match insert_panel(&mut tree, rail, "Properties", scales) {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };
    let history = match insert_panel(&mut tree, rail, "History", scales) {
        Ok(panel) => panel,
        Err(err) => unreachable!("rail was just inserted into this same tree: {err:?}"),
    };

    Workspace {
        tree,
        root,
        tools,
        canvas_column,
        options_bar,
        canvas_area,
        divider,
        rail,
        layers,
        properties,
        history,
        history_current: None,
        history_rows: HashMap::new(),
    }
}

/// The canvas column (0.160.0): the options bar over the canvas area,
/// inserted into `root`. Infallible for the same reason
/// [`build_workspace`] is: `root` is a node of this brand-new tree.
fn insert_canvas_column(
    tree: &mut WidgetTree<WidgetKind>,
    root: WidgetId,
    scales: &Scales,
) -> (WidgetId, WidgetId, WidgetId) {
    // The only element that grows: once the tools panel claims its
    // content width, the rail its fixed width and the divider none, the
    // canvas column absorbs whatever space is left. `min_size: 0` lets a
    // narrow window squeeze it to nothing rather than overflow.
    let canvas_column = match widgets::insert_container(
        tree,
        root,
        Style {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
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
    (canvas_column, options_bar, canvas_area)
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
    tree.set_style(rail_id, rail_style(clamped))?;

    let node = tree
        .accessibility(divider_id)
        .ok_or(WidgetError::UnknownWidget(divider_id))?;
    let mut updated = node.clone();
    updated.set_numeric_value(f64::from(clamped));
    tree.set_accessibility(divider_id, updated)
}

#[cfg(test)]
mod tests {
    use super::{RAIL_MAX_WIDTH, RAIL_MIN_WIDTH, build_workspace, rail_width, set_rail_width};

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
            assert_eq!(
                (canvas.x, canvas.y, canvas.width),
                (column.x, i64::from(bar.height), column.width),
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
        // viewport: the rail claims its own fixed starting width (250),
        // the zero-width divider claims none, the tools panel its content
        // width (0.160.0), and the canvas column (the only growing
        // element) absorbs the rest; the options bar takes its own
        // height off the top of that column. Height (no explicit size)
        // fills via the parent's own 100% root, and each of the 3 stacked
        // panels shares the rail's height equally (flex_grow: 1.0 each).
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
        assert_eq!(canvas_bounds.height, 800 - bar);
        assert_eq!(rail_bounds.height, 800);
        assert_eq!(
            (canvas_bounds.x, canvas_bounds.y),
            (i64::from(tools), i64::from(bar))
        );

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
        for count in [5_usize, 60, 200] {
            let mut ws = build_workspace(&test_scales());
            fill_panels(&mut ws, &scales, (true, true, true), count);
            ws.tree.compute_layout(1600.0, 900.0);

            let (Some(layers), Some(properties), Some(history)) = (
                ws.tree.bounds(ws.layers.root),
                ws.tree.bounds(ws.properties.root),
                ws.tree.bounds(ws.history.root),
            ) else {
                unreachable!("just laid out");
            };

            assert_eq!(
                (properties.height, history.height),
                (layers.height, layers.height),
                "three equally-crowded panels must still share the rail equally at {count} \
                 rows each: {layers:?}, {properties:?}, {history:?}"
            );
            assert!(
                layers.height > 0,
                "and the share must be real, not zero: {layers:?}"
            );
            assert!(
                history.y + i64::from(history.height) <= 900,
                "no panel may be pushed off the bottom of the window: {history:?}"
            );

            let mut previous: Option<aurora_core::Rect> = None;
            for (name, panel) in [
                ("layers", layers),
                ("properties", properties),
                ("history", history),
            ] {
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
}
