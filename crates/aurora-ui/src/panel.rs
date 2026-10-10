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
//! ([`close_panel`]'s own doc comment). Drag-to-redock landed in 0.166.0
//! (`crate::redock`). Still genuinely open: floating panels (0.167.0).

use accesskit::{Action, Node, Orientation, Role};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind, row_height};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{TaffyZero, auto, length, percent};
use taffy::{Dimension, Display, Overflow, Size, Style};

/// One inserted panel's own widget ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelHandle {
    /// The panel's own root — a labeled `Role::Region`, the accessible
    /// name a screen reader announces for the whole panel.
    pub root: WidgetId,
    /// The panel's title slot (0.142.0): the root's **first** child, one
    /// row tall, where `aurora_widgets::text_runs` draws the root's own
    /// accessibility label — the panel's title, kept in exactly one place.
    /// An unlabelled `Role::GenericContainer`, so a screen reader still
    /// announces the title once (on the `Region`), never twice. Stays
    /// visible while the panel is collapsed ([`set_panel_collapsed`]), so
    /// a collapsed panel is still recognisable.
    pub header: WidgetId,
    /// Where a caller adds this panel's real content once it exists
    /// (layer rows, property fields, history entries) — currently
    /// always empty. The panel's scroll container (0.145.0).
    pub body: WidgetId,
    /// The non-scrolling row the body sits in (0.146.0), `[body |
    /// scrollbar]`: the root's second child, where the body used to be. It
    /// exists because a scroll offset moves every descendant of the body,
    /// so the bar cannot live inside it. Hidden with the body on collapse
    /// (it is one of the root's "other" children, [`set_panel_collapsed`]).
    pub viewport: WidgetId,
    /// The body's vertical scrollbar (0.146.0), along the viewport's right
    /// edge — linked to the body (`aurora_widgets::widgets::
    /// link_scrollbar`), so dragging it, pressing its track or an
    /// assistive technology's value actions scroll the body, and every
    /// text-aware layout (`aurora_widgets::compute_text_layout`) copies the
    /// body's offset back into it. `Display::None`, taking no width, while
    /// the body's content fits.
    pub scrollbar: WidgetId,
}

/// How an expanded panel takes height from the column it is docked in
/// (0.161.0). Collapsed, every panel is its title row whatever this says.
///
/// **Why there are two (0.161.0's bug).** Until 0.161.0 every docked
/// panel was [`Self::Fill`], so the rail was split into equal thirds by
/// `flex_grow` alone. Measured headlessly at the design owner's window
/// (1274 x 672 logical, Widget Gallery open, a Curves layer active): each
/// panel got 224 px, but the Properties panel's Curves strip needs 221 px
/// under its 21 px title, so its body was squeezed to 0 and the strip
/// overflowed into History's title by 18 px (40 px at 604 px) — while
/// Layers, with two rows, held a 100 px body that was mostly empty.
/// Content-sized panels fix the distribution: Layers and Properties take
/// what their content needs, History absorbs the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PanelSizing {
    /// Share the column by `flex_grow` from a zero basis — the
    /// pre-0.161.0 rule, kept for History: it takes whatever the
    /// content-sized panels leave, and scrolls.
    #[default]
    Fill,
    /// Take the content's own height (`flex_basis: auto`, no growth), its
    /// body rows counting up to [`content_panel_max_rows`] rows, shrinking
    /// with the other panels — in proportion to that basis — when the
    /// column is too short for everyone. A shrunk panel scrolls (its body, and the
    /// Properties panel's Curves strip, are scroll containers).
    Content,
}

/// The [`PanelSizing::Content`] cap, as a *count of rows* of the
/// `row_height` token: the `size.content_panel_max_rows` design token
/// (0.162.0, the design owner's decision of 2026-10-09 — it replaced the
/// 0.161.0 engineering constant `CONTENT_PANEL_MAX_ROWS`, value
/// unchanged at 10, and is the design owner's to tune). Past this many
/// body rows a content-sized panel stops growing and scrolls.
#[must_use]
pub fn content_panel_max_rows(scales: &Scales) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let rows = scales.size.content_panel_max_rows as f32;
    rows
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
/// The root's first child is the title slot ([`PanelHandle::header`],
/// 0.142.0), one row tall (`row_height(scales)`), above the body — the
/// same shape as a dialog's title slot (0.141.0). Until then nothing drew
/// a panel's title at all: a real macOS screenshot showed the Layers
/// panel as an anonymous strip of rows.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_panel(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    title: impl Into<String>,
    scales: &Scales,
) -> Result<PanelHandle, WidgetError> {
    let mut root_node = Node::new(Role::Region);
    root_node.set_label(title.into());
    root_node.add_action(Action::Focus);
    root_node.add_action(Action::Collapse);
    root_node.set_expanded(true);
    let root = tree.insert(
        parent,
        root_style(false, PanelSizing::Fill, 0.0),
        root_node,
        WidgetKind::Panel,
    )?;
    let header = tree.insert(
        root,
        header_style(scales),
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )?;
    let viewport =
        widgets::insert_container(tree, root, viewport_style(PanelSizing::Fill, scales))?;
    let body = widgets::insert_container(tree, viewport, body_style(false))?;
    // Every docked panel's body scrolls (0.145.0) -- see `body_style`.
    // The flag is the tree's own, not part of the style, so
    // `set_panel_collapsed`'s style resets keep it.
    tree.set_scrollable(body, true)?;
    // Its bar (0.146.0) sits beside it, never inside it: thickness, track
    // and thumb colours and radius are the widget's own token-derived
    // ones (`type_size md`, `paint_scrollbar`), the same as the gallery's.
    let scrollbar = widgets::insert_scrollbar(
        tree,
        viewport,
        scales,
        Orientation::Vertical,
        None,
        0.0,
        widgets::ScrollbarRange {
            min: 0.0,
            max: 0.0,
            page_size: 0.0,
        },
    )?;
    widgets::link_scrollbar(tree, scrollbar, body)?;
    let panel = PanelHandle {
        root,
        header,
        body,
        viewport,
        scrollbar,
    };
    refresh_panel_floor(tree, panel)?;
    Ok(panel)
}

/// A panel's height floor while expanded (0.161.0, see [`root_style`]):
/// the sum of its root's *shown* children's own declared minimum
/// heights. A hidden child (`Display::None`) or one with an `auto`
/// minimum adds nothing (review J6).
fn panel_floor(tree: &WidgetTree<WidgetKind>, panel: PanelHandle) -> f32 {
    shown_children_floor(tree, panel.root)
}

/// The sum of `id`'s *shown* children's own declared minimum heights —
/// [`panel_floor`]'s rule for any column, shared with a panel tab group's
/// own root (0.164.0, [`crate::panel_group`]).
pub(crate) fn shown_children_floor(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> f32 {
    tree.children(id)
        .unwrap_or_default()
        .iter()
        .filter_map(|&child| tree.style(child))
        .filter(|style| style.display != Display::None)
        .map(|style| {
            let min = style.min_size.height;
            if min.is_auto() { 0.0 } else { min.value() }
        })
        .sum()
}

/// Recomputes an expanded panel's height floor ([`panel_floor`]) after
/// a child was added to its root — the controls strips
/// ([`crate::insert_tool_controls`], [`crate::insert_layer_controls`])
/// call it. A collapsed panel is left alone: [`set_panel_collapsed`]
/// recomputes it on expanding.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root` or
/// `panel.body` doesn't exist.
pub(crate) fn refresh_panel_floor(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
) -> Result<(), WidgetError> {
    if panel_is_collapsed(tree, panel)? {
        return Ok(());
    }
    let sizing = panel_sizing(tree, panel)?;
    set_root_style(tree, panel, false, sizing)
}

/// How `panel` shares its column's height while expanded — read from the
/// tree, never from the handle (0.161.0 review J3): the viewport's own
/// style records it (`flex_basis: auto` for [`PanelSizing::Content`],
/// zero for [`PanelSizing::Fill`]) and nothing but [`set_panel_sizing`]
/// changes that style ([`set_panel_collapsed`] touches only its
/// `display`). So every copy of a handle, however old, agrees.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.viewport` doesn't
/// exist.
pub fn panel_sizing(
    tree: &WidgetTree<WidgetKind>,
    panel: PanelHandle,
) -> Result<PanelSizing, WidgetError> {
    let style = tree
        .style(panel.viewport)
        .ok_or(WidgetError::UnknownWidget(panel.viewport))?;
    Ok(if style.flex_basis.is_auto() {
        PanelSizing::Content
    } else {
        PanelSizing::Fill
    })
}

/// Sets how `panel` shares its column's height while expanded (0.161.0,
/// [`PanelSizing`]). It is recorded in the tree (the viewport's style,
/// [`panel_sizing`]), so a later [`set_panel_collapsed`] keeps it
/// through any copy of the handle. A collapsed panel stays collapsed;
/// the new sizing applies when it expands. A caller re-runs layout.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.root`,
/// `panel.viewport` or `panel.body` doesn't exist.
pub fn set_panel_sizing(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    sizing: PanelSizing,
    scales: &Scales,
) -> Result<(), WidgetError> {
    let collapsed = panel_is_collapsed(tree, panel)?;
    if tree.style(panel.viewport).is_none() {
        return Err(WidgetError::UnknownWidget(panel.viewport));
    }
    set_root_style(tree, panel, collapsed, sizing)?;
    let display = tree
        .style(panel.viewport)
        .map_or(Display::Flex, |style| style.display);
    tree.set_style(
        panel.viewport,
        Style {
            display,
            ..viewport_style(sizing, scales)
        },
    )?;
    Ok(())
}

/// The `[body | scrollbar]` row (0.146.0, [`PanelHandle::viewport`]):
/// it takes over exactly the flex role the body had in the root before —
/// grow into the panel's share of the rail from a zero basis, never
/// impose a minimum on either axis — and lays the two out side by side.
/// The body (`flex_grow: 1`, zero basis) takes the width the bar leaves,
/// and both stretch to the row's height.
///
/// **0.161.0: one row tall at least, and content-based for a
/// [`PanelSizing::Content`] panel.** The one-row floor is what the
/// panel root's automatic minimum (`min_size.height: auto`, see
/// [`root_style`]) adds up — title row, one body row and any controls
/// strip — so a crowded rail can shrink a panel's body to one row and
/// never to zero. It is an explicit floor, not the body's content
/// height, so a thousand rows still cannot starve the rail (0.77.1). A
/// content-sized panel's viewport takes its rows' height as its basis,
/// which is what makes the panel's own `flex_basis: auto` its content.
fn viewport_style(sizing: PanelSizing, scales: &Scales) -> Style {
    Style {
        flex_direction: taffy::FlexDirection::Row,
        flex_grow: 1.0,
        flex_basis: match sizing {
            PanelSizing::Fill => Dimension::ZERO,
            PanelSizing::Content => auto(),
        },
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: length(row_height(scales)),
        },
        // The content cap: a content-sized panel's rows count toward its
        // basis up to `size.content_panel_max_rows` rows, then it scrolls.
        max_size: taffy::Size {
            width: auto(),
            height: match sizing {
                PanelSizing::Fill => auto(),
                PanelSizing::Content => length(content_panel_max_rows(scales) * row_height(scales)),
            },
        },
        // A scroll container's min-content contribution is its own
        // `min_size`, not its rows: without this the panel root's
        // automatic minimum summed every row (measured: a 200-step
        // History floored at 4221 px and pushed off the rail).
        overflow: taffy::Point {
            x: Overflow::Hidden,
            y: Overflow::Hidden,
        },
        ..Default::default()
    }
}

/// The title slot's own layout: exactly one row tall (`row_height`, the
/// height a Layers or History row has, so the title reads as the panel's
/// first line), never shrunk by a crowded panel (`flex_shrink: 0`, the
/// same guard the controls strips carry), and full width by the root's
/// default cross-axis stretch. No padding of its own: the title text is
/// inset by `spacing.sm` where it is drawn, exactly as a tree row's label
/// is. `min_size.width: 0` keeps it from ever flooring the rail's width
/// (see [`root_style`]).
fn header_style(scales: &Scales) -> Style {
    let row = row_height(scales);
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: auto(),
            height: length(row),
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: length(row),
        },
        ..Default::default()
    }
}

/// A grouped panel's title slot (0.164.0, [`crate::panel_group`]): zero
/// height, so the group's tab — which names the panel — is its only
/// title. The slot is kept rather than removed because its `display` is
/// what [`panel_is_closed`] reads, and [`set_panel_collapsed`]/
/// [`close_panel`] go on toggling exactly that. `aurora_widgets::text`
/// draws a title only under a `Role::Region`, and a grouped panel is a
/// `Role::TabPanel`, so nothing is drawn into the zero-height slot either.
pub(crate) fn grouped_header_style() -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: auto(),
            height: Dimension::ZERO,
        },
        min_size: Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        max_size: Size {
            width: auto(),
            height: Dimension::ZERO,
        },
        ..Default::default()
    }
}

/// Sets `panel.root`'s style to [`root_style`] for `collapsed`/`sizing`
/// and its current floor, **keeping the root's own `display`** (0.164.0):
/// a panel tab group hides its unselected panels by their root's
/// `display` ([`crate::panel_group`]), and a collapse, a sizing change or
/// a floor refresh of a hidden panel must not show it again.
fn set_root_style(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    collapsed: bool,
    sizing: PanelSizing,
) -> Result<(), WidgetError> {
    let display = tree
        .style(panel.root)
        .ok_or(WidgetError::UnknownWidget(panel.root))?
        .display;
    let floor = panel_floor(tree, panel);
    tree.set_style(
        panel.root,
        Style {
            display,
            ..root_style(collapsed, sizing, floor)
        },
    )?;
    // A grouped panel's group mirrors its shown member's flex role and
    // floor, so it follows every change made here.
    if let Some(parent) = tree.parent(panel.root)
        && crate::panel_group::is_group_root(tree, parent)
    {
        crate::panel_group::refresh_group_root(tree, parent)?;
    }
    Ok(())
}

/// A panel's own root style — `Column` (the body stacks under the
/// header once one exists). **Since 0.161.0 the expanded style depends
/// on the panel's [`PanelSizing`]** (the last paragraph below): only a
/// [`PanelSizing::Fill`] panel (History) keeps the `flex_grow: 1.0`,
/// zero-basis rule the next two paragraphs describe; a
/// [`PanelSizing::Content`] panel (Layers, Properties) is content-based
/// and does not grow. Collapsed, every panel has `flex_grow: 0.0`, so it
/// stops claiming a share at all and its siblings absorb the space it
/// gives up — ordinary flexbox behaviour, not a special case.
///
/// **History (written when every panel was `Fill`): a `Fill` panel's
/// share of the rail is its siblings' business, never its own
/// content's** — `flex_basis: 0` plus a fixed height minimum are what
/// make that true, and both are load-bearing (0.77.1). A `Content`
/// panel avoids the same starvation differently: its rows count only up
/// to [`content_panel_max_rows`] (the viewport's `max_size`), and its
/// minimum is its fixed floor, never its content. Real bug, with
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
/// **`min_size.width` is pinned to `0` here too, and — unlike the height
/// — it does nothing. That is a correction (0.77.5) to what `0.77.3`
/// claimed.** That round pinned the width believing it would stop a row's
/// own `min_size.width` (one row height, this module's own [`row_style`])
/// from propagating up through body → panel root → dock rail as a floor
/// on the rail's whole width. It does not. Measured on a real
/// `crate::workspace::build_workspace` at `compute_layout(1.0, 200.0)`,
/// *with these pins in place*: the rail still came back 21 px wide
/// against a 1 px window whenever any one of the three panels held rows.
///
/// The reason is that `taffy` only consults a flex item's own `min_size`
/// on its **main** axis when deciding whether to fall back to a
/// content-based minimum (`compute/flexbox.rs`,
/// `determine_flex_base_size`), and a panel root and a panel body are
/// both items of `Column` containers — width is their *cross* axis, so
/// this pin is never read by that branch at all. The one box in the chain
/// where width really is the main axis is the rail itself, and that is
/// where the fix went: `crate::workspace`'s own `rail_style`, whose doc
/// comment carries the full mechanism and the numbers.
///
/// These two lines are kept rather than deleted because `min_size: 0` on
/// both axes is the honest statement of "a panel never imposes its
/// content's size on the rail", and the width half would become
/// load-bearing the moment a panel root were ever docked into a `Row`
/// container (drag-to-redock, this module's own doc comment). What is
/// removed is the claim that it closes anything today.
///
/// **Collapsed, the basis is `auto` instead (0.142.0)**: a collapsed
/// panel is exactly as tall as its title slot (every other child is
/// `Display::None`), rather than zero tall with the title spilling over
/// the panel below it. The content-sized basis is safe here for the
/// reason the zero basis exists in the expanded state: the only content
/// left is one fixed-height row.
///
/// **Collapsed, `flex_shrink` is `0` too (review revision, 0.142.0)**:
/// otherwise a rail shorter than every collapsed title row combined
/// shrank each collapsed root below its own (`flex_shrink: 0`) header,
/// and the title spilled over the panel below it (measured: an 11 px
/// root under a 21 px title). Now the collapsed panels keep their full
/// row and overflow the bottom of the rail instead, where the window
/// clips them. A *closed* panel ([`close_panel`]) uses this same style
/// with its title hidden as well, so its content height — and with
/// `flex_basis: auto` its height — is exactly zero.
///
/// **Expanded, 0.161.0 adds [`PanelSizing`] and a height floor.**
/// `min_size.height` is the panel's *floor* ([`panel_floor`]): the sum
/// of its children's own declared minimum heights — the title row, the
/// viewport's one-row floor ([`viewport_style`]) and each controls
/// strip's one-row floor. So a crowded rail shrinks each panel's body
/// and strips down to one row each and no further, instead of to zero
/// with the title and strip overflowing into the next panel (the
/// 0.144.1 lesson). Not `auto`: `taffy`'s automatic minimum is the
/// min-content height, which counted every visible body row and the
/// whole Curves strip — measured, it pinned Properties at 263 px and
/// pushed History off a 300 px rail.
/// The width stays pinned to `0` for the reason above. A
/// [`PanelSizing::Content`] root is content-based (`flex_basis: auto`,
/// `flex_grow: 0`), its viewport capped at [`content_panel_max_rows`]
/// rows; a [`PanelSizing::Fill`] root keeps the zero basis and
/// `flex_grow: 1`.
pub(crate) fn root_style(collapsed: bool, sizing: PanelSizing, floor: f32) -> Style {
    if collapsed {
        return Style {
            flex_direction: taffy::FlexDirection::Column,
            flex_grow: 0.0,
            flex_shrink: 0.0,
            flex_basis: auto(),
            min_size: taffy::Size {
                width: Dimension::ZERO,
                height: Dimension::ZERO,
            },
            ..Default::default()
        };
    }
    let (grow, basis) = match sizing {
        PanelSizing::Fill => (1.0, Dimension::ZERO),
        PanelSizing::Content => (0.0, auto()),
    };
    Style {
        flex_direction: taffy::FlexDirection::Column,
        flex_grow: grow,
        flex_shrink: 1.0,
        flex_basis: basis,
        min_size: taffy::Size {
            width: Dimension::ZERO,
            height: length(floor),
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
/// intersection reads. It was the only clipping overflow in the
/// workspace until 0.161.0, which made the viewport around it
/// ([`viewport_style`]) and the two controls strips
/// (`crate::tool_controls`, `crate::layer_controls`) clip too.
///
/// **That content is reachable by scrolling (0.145.0).** [`insert_panel`]
/// makes every body a `WidgetTree::set_scrollable` container, so the
/// mouse wheel (and a trackpad) over a panel moves the rows past its
/// bottom into view — `aurora-app` routes the wheel — and the body's own
/// bounds never move, so this clip is what keeps a scrolled-out row
/// invisible and unclickable. Until then rows past the bottom of a
/// crowded Layers panel were laid out but not reachable at all. As of
/// 0.146.0 the body also has a visible scrollbar beside it
/// ([`PanelHandle::scrollbar`]) — a mouse with no wheel can drag it or
/// press its track, and an assistive technology can set its value — and
/// the body still reports its own position and range to a screen reader.
///
/// **The body now sits in a `Row` (0.146.0), [`PanelHandle::viewport`]**,
/// not directly in the root. `flex_grow: 1` and the zero basis therefore
/// act on its *width* (it takes what the bar leaves) while its height
/// stretches to the viewport's; the viewport carries the vertical
/// `flex_grow`/zero-basis/zero-minimum role this style used to play in
/// the root, so nothing above the panel sees a difference.
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
/// - **Layers** was unaffected at the time: `aurora_widgets::widgets::
///   insert_tree_view` gave its own container an explicit
///   `size: { width: percent(1.0), height: percent(1.0) }` on both axes,
///   identical under `Row` or `Column`. **As of 0.145.0
///   `crate::populate_layers_panel` overrides that height to `auto()`
///   (content-sized, `flex_shrink: 0`)**, and this paragraph used to say
///   doing so "would silently reintroduce exactly the rail-starvation bug
///   `0.77.1` fixed". It does not, and that is measured rather than
///   argued: `a_crowded_layers_panel_never_starves_its_sibling_panels`
///   (1 to 400 layers in a real `build_workspace`) passes unedited with
///   the content-sized container. The container's automatic minimum can
///   now grow it to its content, but it is the *body's* item, and the
///   body (`flex_basis: 0`, `min_size: 0`, and a clipping overflow, whose
///   automatic minimum is zero) is what the root and rail see — so the
///   rows overflow the body, clipped, instead of pushing the panel open.
///   The override is required, not cosmetic: a container held at the
///   body's height moves with the scroll offset, and every row past its
///   shifted bottom edge would fail `WidgetTree::hit_test` at the
///   container even once scrolled into view. The container itself stays,
///   for the `Role::Tree` node it carries.
/// - **History** is what this fixes, together with its own rows'
///   real `min_size` ([`row_style`], which lived in
///   `crate::history_panel` when this was written).
/// - **Properties needed one more step, and got it in `0.77.4`.** The
///   `Column` change alone neither fixed nor broke it: its rows were
///   `Style::default()` under `Row` *and* under `Column`, so they stayed
///   degenerate with only the axis moving from width to height (full
///   body width, zero height). They are now real
///   `aurora_widgets::widgets::WidgetKind::ListRow`s sharing the same
///   [`row_style`] History uses, so all three panels' rows are
///   non-degenerate and hit-testable. What remains open there is what
///   remains open for History: no scrolling, no selection, no focus
///   stops (`crate::properties_panel`'s own module doc comment).
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
        // Both axes, not just the height -- but see `root_style`'s own
        // doc comment: the width half is inert on a `Column` item and
        // does *not* close the rail-width propagation `0.77.3` said it
        // did. `crate::workspace::rail_style` is where that is fixed.
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

/// One panel-body list row's own layout: full body width, one row height
/// tall, and never smaller than that on either axis. Shared by
/// [`crate::populate_history_panel`] and
/// [`crate::populate_properties_panel`] — the two panels whose rows are
/// `panel.body`'s own direct children. (Layers is not a caller: its rows
/// live inside `aurora_widgets::widgets::insert_tree_view`'s own
/// container and use `tree_view::style` instead.)
///
/// **It lives here rather than in either caller** because the two copies
/// would otherwise be byte-identical, and a copy carries its own prose:
/// the first draft of the Properties fix duplicated History's version
/// verbatim, doc comment included, which would have shipped a rationale
/// citing a test name that does not exist in the module quoting it.
///
/// **Two separate guards, load-bearing for two different reasons.**
/// The `0.77.2` commit message credited `flex_grow: 0.0` with preventing
/// sub-pixel rows in a long journal; that was wrong, and the correction
/// is measured rather than reasoned (0.77.3 review round).
///
/// - **`min_size.height: length(row)` is what makes a row exactly one
///   line tall at *any* row count.** It is a hard flexbox floor that
///   neither `flex_grow` nor `flex_shrink` can cross. `size.height:
///   auto()` is what makes it a real floor rather than a starting point:
///   an `auto` main size gives the item a flex base size of `0`, so its
///   *scaled* flex-shrink factor (`flex_shrink × flex_base_size`) is `0`
///   too and a crowded panel has nothing to shrink. Setting
///   `flex_grow: 1.0` here was applied as a mutation and measured: rows
///   still came back a correct 21 px at 200 entries and at 1000. The
///   "200 entries would be 1.5 px each, 1000 would be 0.3 px" scenario
///   the previous wording described **does not occur**, and nothing here
///   depends on `flex_grow` to prevent it.
/// - **`flex_grow: 0.0` (the default, spelled by omission) is what stops
///   a *sparse* panel from inflating its rows.** Free space is what
///   `flex_grow` divides, and a panel with a handful of rows has plenty:
///   the same mutation inflates five rows to 60 px each, which is what
///   `history_rows_are_real_list_row_widgets_with_a_hittable_size` and
///   its Properties twin actually catch — both assert the *exact* row
///   height, not merely a positive one, which is the assertion shape
///   that makes that regression visible. `aurora_widgets::widgets::
///   command_palette::row_style` uses `flex_grow: 1.0` deliberately,
///   because its handful of result rows really are meant to divide their
///   container evenly; a list row here is one line tall no matter how
///   much room it is offered. `aurora_widgets::widgets::tree_view::
///   style` records the same borrowed-idiom mistake, and additionally
///   adds a `padding.top` this does not.
///
/// Neither is redundant, and deleting `min_size.height` in particular
/// would bring the zero-height bug straight back — it is the guard that
/// holds, not the one the old wording credited.
///
/// `min_size.width` is one row height as well, the same square floor
/// and the same reasoning `tree_view::style` documents: no "minimum row
/// width" token exists, inventing one is a design decision rather than
/// an engineering default (CLAUDE.md), and a square of the row's own
/// height is the smallest thing that is still a real target.
///
/// **That width floor is visible from outside this row, and containing it
/// is `crate::workspace`'s own `rail_style`'s job, not this function's
/// (0.77.5).** `taffy` resolves a flex item with an `auto` main-axis
/// minimum by *measuring* its min-content size, and that measurement
/// descends the whole subtree taking each descendant's `min_size` as a
/// floor — so before `0.77.5` this one line made a 1 px-wide window's
/// dock rail 21 px wide. It is fixed where the measurement is actually
/// triggered rather than by deleting the floor here, because
/// `aurora_widgets::widgets::tree_view::style` gives Layers' rows the
/// same floor for a separate and genuinely load-bearing reason and
/// propagated identically; see `rail_style` for the source citation and
/// `crate::workspace`'s own
/// `a_populated_panel_never_floors_the_rails_own_width_to_a_row_height`
/// for the layout-level regression test.
pub(crate) fn row_style(scales: &Scales) -> Style {
    let row = row_height(scales);
    Style {
        size: Size {
            width: percent(1.0_f32),
            height: auto(),
        },
        min_size: Size {
            width: length(row),
            height: length(row),
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

/// Whether `panel` is **closed** ([`close_panel`]) rather than merely
/// collapsed (0.161.0 review J1): a close also hides the title row,
/// which a collapse keeps. A closed panel is collapsed too.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.header` or
/// `panel.body` doesn't exist.
pub fn panel_is_closed(
    tree: &WidgetTree<WidgetKind>,
    panel: PanelHandle,
) -> Result<bool, WidgetError> {
    let header = tree
        .style(panel.header)
        .ok_or(WidgetError::UnknownWidget(panel.header))?;
    Ok(header.display == Display::None && panel_is_collapsed(tree, panel)?)
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
    let sizing = panel_sizing(tree, panel)?;
    tree.set_style(panel.body, body_style(collapsed))?;
    // Any *other* child of the root is panel content too (the Layers
    // panel's controls strip, `crate::layer_controls`, lives there so
    // repopulating the body cannot destroy it) and must hide with the
    // body. Only `display` is touched, so each keeps its own style. The
    // title slot is the one exception (0.142.0): it is what keeps a
    // collapsed panel recognisable, so it stays shown in *both* states.
    // Setting it explicitly (rather than leaving it alone) is what makes
    // this function the reopen path for a closed panel too, whose title
    // `close_panel` hid.
    let others: Vec<WidgetId> = tree
        .children(panel.root)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|&child| child != panel.body && child != panel.header)
        .collect();
    for child in others {
        set_display(
            tree,
            child,
            if collapsed {
                Display::None
            } else {
                Display::Flex
            },
        )?;
    }
    set_display(tree, panel.header, Display::Flex)?;
    // The root last (0.161.0 review J6): its floor counts only the
    // children shown, so it is computed once their `display` is final.
    set_root_style(tree, panel, collapsed, sizing)?;

    let node = tree
        .accessibility(panel.root)
        .ok_or(WidgetError::UnknownWidget(panel.root))?;
    let mut updated = node.clone();
    updated.set_expanded(!collapsed);
    if updated.role() == Role::TabPanel {
        // A grouped panel (0.164.0 review I4): its group collapses as one,
        // and nothing routes a member's own `Collapse`/`Expand`, so it
        // advertises neither (`crate::panel_group`).
        updated.remove_action(Action::Collapse);
        updated.remove_action(Action::Expand);
    } else if collapsed {
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
/// hiding the title row as well (so a closed panel takes no height,
/// where a collapsed one keeps its title), plus
/// really freeing its current content ([`clear_panel_body`]) rather
/// than just hiding it. Unlike a plain collapse — which deliberately
/// keeps content resident so a quick re-expand needs no rebuild, see
/// that function's own doc comment — closing trades that cheap-toggle
/// guarantee for actually reclaiming the memory and simplifying the
/// accessibility tree down to just the region itself.
///
/// Reopening is the ordinary `set_panel_collapsed(tree, panel, false)`,
/// which shows the title row again: the body comes back empty until whatever populated it before
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
    // Unlike a collapse, a close hides the title row too (review
    // revision, 0.142.0): a closed panel takes no space at all. Any
    // `set_panel_collapsed` call — the panel-toggle command's reopen
    // path — shows it again.
    set_display(tree, panel.header, Display::None)?;
    clear_panel_body(tree, panel.body)?;
    tree.set_accessibility(panel.body, Node::new(Role::GenericContainer))
}

/// The accessible description a floating panel carries (0.167.0), so a
/// screen reader can tell a floating panel from a docked one.
pub const FLOATING_DESCRIPTION: &str = "Floating";

/// Marks `panel` as floating (0.167.0) or docked: its root becomes a
/// `WidgetKind::RaisedPanel` (`surface.raised`, one elevation step up) or
/// back to a `WidgetKind::Panel`, and carries [`FLOATING_DESCRIPTION`] or
/// no description. Its role, label, title slot and content are untouched.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `panel.root` doesn't exist.
pub(crate) fn set_panel_raised(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    raised: bool,
) -> Result<(), WidgetError> {
    let kind = if raised {
        WidgetKind::RaisedPanel
    } else {
        WidgetKind::Panel
    };
    let payload = tree
        .payload_mut(panel.root)
        .ok_or(WidgetError::UnknownWidget(panel.root))?;
    let kind_changed = *payload != kind;
    *payload = kind;
    let node = tree
        .accessibility(panel.root)
        .ok_or(WidgetError::UnknownWidget(panel.root))?;
    let mut updated = node.clone();
    if raised {
        updated.set_description(FLOATING_DESCRIPTION);
    } else {
        updated.clear_description();
    }
    if updated != *node {
        tree.set_accessibility(panel.root, updated)?;
    }
    if kind_changed {
        tree.mark_dirty(panel.root)?;
    }
    Ok(())
}

/// Puts `panel` into the shape a tab group needs, or back into a lone
/// docked panel's (0.166.0, drag-to-redock — [`crate::panel_group`] then
/// takes over a grouped one's role, label and visibility). Grouped: its
/// title slot is zero height ([`grouped_header_style`]). Lone: the
/// one-row title slot again ([`header_style`]), the root shown, its
/// accessibility node back to a focusable `Role::Region` with no
/// `labelled_by` and the `Collapse`/`Expand` action matching its state.
/// Either way the title slot keeps its `display` (a closed panel stays
/// closed) and the root's floor is recomputed from its shown children.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] for a malformed handle.
pub(crate) fn set_panel_grouped(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    grouped: bool,
    scales: &Scales,
) -> Result<(), WidgetError> {
    let display = tree
        .style(panel.header)
        .ok_or(WidgetError::UnknownWidget(panel.header))?
        .display;
    let header = if grouped {
        grouped_header_style()
    } else {
        header_style(scales)
    };
    tree.set_style(panel.header, Style { display, ..header })?;
    let collapsed = panel_is_collapsed(tree, panel)?;
    if !grouped {
        set_display(tree, panel.root, Display::Flex)?;
        let node = tree
            .accessibility(panel.root)
            .ok_or(WidgetError::UnknownWidget(panel.root))?;
        let mut updated = node.clone();
        updated.set_role(Role::Region);
        updated.clear_labelled_by();
        updated.clear_hidden();
        updated.add_action(Action::Focus);
        if collapsed {
            updated.remove_action(Action::Collapse);
            updated.add_action(Action::Expand);
        } else {
            updated.remove_action(Action::Expand);
            updated.add_action(Action::Collapse);
        }
        if updated != *node {
            tree.set_accessibility(panel.root, updated)?;
        }
    }
    let sizing = panel_sizing(tree, panel)?;
    set_root_style(tree, panel, collapsed, sizing)
}

/// Sets only `id`'s own `display`, keeping the rest of its style.
pub(crate) fn set_display(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    display: Display,
) -> Result<(), WidgetError> {
    let mut style = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    style.display = display;
    tree.set_style(id, style)
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

    // The real, committed, owner-approved scales -- the same file every
    // other `aurora-ui` test module parses.
    fn test_scales() -> aurora_theme::Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match aurora_theme::Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn insert_panel_adds_a_labeled_region_with_an_empty_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        assert_eq!(tree.parent(panel.body), Some(panel.viewport));
        assert_eq!(tree.parent(panel.viewport), Some(panel.root));
        assert_eq!(tree.parent(panel.scrollbar), Some(panel.viewport));
        match panel_is_collapsed(&tree, panel) {
            Ok(collapsed) => assert!(!collapsed, "a freshly inserted panel starts expanded"),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn insert_panel_rejects_an_unknown_parent() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match insert_panel(&mut tree, bogus, "Layers", &test_scales()) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn clear_panel_body_removes_every_child_but_keeps_the_body_itself() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        let first = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let second = match insert_panel(&mut tree, root, "Properties", &test_scales()) {
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
        // Everything but the collapsed panel's own one-row title slot
        // (0.142.0), which stays visible so the panel is recognisable.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let title_row = widgets::row_height(&test_scales()).round() as u32;
        assert_eq!(
            after.height,
            200 - title_row,
            "the collapsed panel's own share must go to its still-expanded sibling, ordinary \
             flex_grow sharing, not a special case -- all but its title row"
        );
        let Some(collapsed) = tree.bounds(first.root) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            collapsed.height, title_row,
            "a collapsed panel is its title row"
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
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
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
            header: bogus,
            body: bogus,
            viewport: bogus,
            scrollbar: bogus,
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
            header: bogus,
            body: bogus,
            viewport: bogus,
            scrollbar: bogus,
        };
        match set_panel_collapsed(&mut tree, panel, true) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }

    #[test]
    fn closing_a_panel_collapses_it_and_really_empties_its_body() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        let panel = match insert_panel(&mut tree, root, "Layers", &test_scales()) {
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
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
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
    /// `0.77.1` fixed, and this test reads that off the two styles
    /// directly.
    ///
    /// **It is a style test and nothing more, which is a correction to
    /// what it claimed through `0.77.4`.** It used to say it protected
    /// the dock rail's width from a row's own `min_size.width`. It never
    /// did: reading a `Style` back says nothing about what `taffy` does
    /// with it, and the propagation it named was live the whole time it
    /// was green — see [`super::root_style`] for the measurement and
    /// `crate::workspace`'s own `rail_style` for the real mechanism and
    /// fix. The layout-level assertion that actually covers that
    /// behaviour is `crate::workspace`'s own
    /// `a_populated_panel_never_floors_the_rails_own_width_to_a_row_height`.
    ///
    /// 0.161.0: an expanded root's height minimum is now its fixed
    /// two-row floor (`panel_floor`), still never its content's size.
    ///
    /// What this one is still worth: the *height* pin is genuinely
    /// load-bearing (`0.77.1`'s rail-starvation bug), it must survive a
    /// collapse/expand round trip, and pinning it is cheap to assert
    /// here directly.
    #[test]
    fn a_panels_own_styles_never_impose_a_minimum_size_on_either_axis() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "History", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        for collapsed in [false, true] {
            if let Err(err) = set_panel_collapsed(&mut tree, panel, collapsed) {
                unreachable!("{err:?}");
            }
            // 0.161.0: an expanded root's height minimum is its fixed
            // floor -- title row plus the viewport's one-row floor, from
            // the `row_height` token -- never its content's size; the
            // body's stays zero, and both widths stay zero.
            let row = widgets::row_height(&test_scales());
            let root_floor = if collapsed {
                taffy::Dimension::ZERO
            } else {
                taffy::style_helpers::length(2.0 * row)
            };
            for (name, id, floor) in [
                ("root", panel.root, root_floor),
                ("body", panel.body, taffy::Dimension::ZERO),
            ] {
                let Some(style) = tree.style(id) else {
                    unreachable!("just inserted");
                };
                assert_eq!(
                    style.min_size,
                    taffy::Size {
                        width: taffy::Dimension::ZERO,
                        height: floor,
                    },
                    "a panel's {name} must never impose its content's size on the rail \
                     (collapsed: {collapsed})"
                );
            }
        }
    }

    /// 0.142.0: every docked panel's first child is an unlabelled title
    /// slot one row tall, the body starts exactly where it ends, and the
    /// `Region` keeps its label as the one announcement of the title.
    #[test]
    fn every_docked_panel_has_an_unlabelled_one_row_title_slot_above_its_body() {
        let scales = test_scales();
        let mut ws = crate::build_workspace(&scales);
        ws.tree.compute_layout(1600.0, 900.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = widgets::row_height(&scales).round() as u32;
        // 0.164.0: a grouped panel (Properties, History) is a `TabPanel`
        // whose tab names it; its title slot is still the root's first
        // child, unlabelled, but zero height.
        for (panel, title) in [(ws.properties, "Properties"), (ws.history, "History")] {
            assert_eq!(
                ws.tree.children(panel.root).and_then(<[_]>::first),
                Some(&panel.header),
                "{title}: the title slot is the root's first child"
            );
            let Some(root_node) = ws.tree.accessibility(panel.root) else {
                unreachable!("just built");
            };
            assert_eq!(root_node.role(), accesskit::Role::TabPanel);
            assert_eq!(root_node.label(), Some(title));
            let Some(header) = ws.tree.style(panel.header) else {
                unreachable!("just built");
            };
            assert_eq!(
                header.size.height,
                <taffy::Dimension as taffy::style_helpers::TaffyZero>::ZERO,
                "{title}: no title row"
            );
        }
        let Some(properties) = ws.tree.bounds(ws.properties.root) else {
            unreachable!("just laid out");
        };
        let Some(body) = ws.tree.bounds(ws.properties.viewport) else {
            unreachable!("just laid out");
        };
        assert_eq!(
            body.y, properties.y,
            "the shown tab's body starts at its top"
        );
        {
            let (panel, title) = (ws.layers, "Layers");
            assert_eq!(
                ws.tree.children(panel.root).and_then(<[_]>::first),
                Some(&panel.header),
                "{title}: the title slot is the root's first child"
            );
            let Some(header_node) = ws.tree.accessibility(panel.header) else {
                unreachable!("just built");
            };
            assert_eq!(header_node.role(), accesskit::Role::GenericContainer);
            assert_eq!(header_node.label(), None, "{title}: the slot is unlabelled");
            let Some(root_node) = ws.tree.accessibility(panel.root) else {
                unreachable!("just built");
            };
            assert_eq!(root_node.role(), accesskit::Role::Region);
            assert_eq!(root_node.label(), Some(title));
            let (Some(root_box), Some(header), Some(body)) = (
                ws.tree.bounds(panel.root),
                ws.tree.bounds(panel.header),
                ws.tree.bounds(panel.body),
            ) else {
                unreachable!("just laid out");
            };
            assert_eq!(
                header.y, root_box.y,
                "{title}: the title is the panel's top row"
            );
            assert_eq!(header.height, row, "{title}: one row tall");
            assert_eq!(header.width, root_box.width, "{title}: full panel width");
            assert_eq!(
                body.y,
                header.bottom(),
                "{title}: the body starts under the title"
            );
        }
    }

    /// The title is really drawn (0.142.0): `aurora_widgets::text_runs`
    /// on each panel's title slot emits the panel's own name, inside the
    /// slot's box, and nothing on the root, body or the Layers controls
    /// strip.
    #[test]
    fn each_docked_panels_title_slot_draws_its_name_and_nothing_else_does() {
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
        let scales = test_scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match crate::insert_layer_controls(&mut ws.tree, ws.layers, &scales) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        ws.tree.compute_layout(1600.0, 900.0);
        let runs = |id| {
            let Some(bounds) = ws.tree.bounds(id) else {
                unreachable!("laid out");
            };
            aurora_widgets::text_runs(&ws.tree, id, bounds, bounds, None, &theme, &scales)
        };
        // 0.164.0: a grouped panel's tab draws its name; its zero-height
        // title slot draws nothing.
        let Ok(bar) = widgets::tab_bar_state(&ws.tree, crate::workspace::test_group(&ws).bar)
        else {
            unreachable!("the group has a bar");
        };
        let tabs = bar.tabs().to_vec();
        for (index, (panel, title)) in [(ws.properties, "Properties"), (ws.history, "History")]
            .into_iter()
            .enumerate()
        {
            assert!(
                runs(panel.header).is_empty(),
                "{title}: a grouped panel's slot draws nothing"
            );
            let Some(&tab) = tabs.get(index) else {
                unreachable!("one tab per member");
            };
            if index == 0 {
                // Only the shown group is laid out with a real tab box.
                let tab_runs = runs(tab);
                assert!(
                    tab_runs.iter().any(|run| run.text == title),
                    "{title}: its tab draws its name, got {tab_runs:?}"
                );
            }
        }
        {
            let (panel, title) = (ws.layers, "Layers");
            let header_runs = runs(panel.header);
            let [run] = &header_runs[..] else {
                unreachable!("{title}: one title run, got {header_runs:?}");
            };
            assert_eq!(run.text, title);
            let Some(header) = ws.tree.bounds(panel.header) else {
                unreachable!("laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            let (x, y, w, h) = (
                header.x as f32,
                header.y as f32,
                header.width as f32,
                header.height as f32,
            );
            assert!(
                run.rect.0 >= x && run.rect.0 + run.rect.2 <= x + w,
                "{title}: inside the slot horizontally: {run:?} vs {header:?}"
            );
            assert!((run.rect.1 - y).abs() < f32::EPSILON && (run.rect.3 - h).abs() < f32::EPSILON);
            assert!(
                runs(panel.root).is_empty(),
                "{title}: the root draws nothing"
            );
            assert!(
                runs(panel.body).is_empty(),
                "{title}: the body draws nothing"
            );
        }
        assert!(
            runs(controls.root).is_empty(),
            "the controls strip is not a title"
        );
    }

    /// A crowded panel never squeezes its title away: the slot keeps its
    /// row even when the panel's share is smaller than its content.
    #[test]
    fn a_tiny_panel_keeps_its_full_title_row() {
        let scales = test_scales();
        let (mut tree, root) = widgets::new_tree(Style {
            flex_direction: taffy::FlexDirection::Column,
            size: taffy::Size {
                width: taffy::style_helpers::length(100.0_f32),
                height: taffy::style_helpers::length(10.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "Layers", &scales) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(100.0, 10.0);
        let Some(header) = tree.bounds(panel.header) else {
            unreachable!("just laid out");
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = widgets::row_height(&scales).round() as u32;
        assert_eq!(header.height, row);
    }

    /// Collapsing hides the body and the controls strips (0.135.0) but
    /// never the title slot, so a collapsed panel is still recognisable;
    /// expanding restores the strips.
    #[test]
    fn collapsing_keeps_the_title_visible_and_still_hides_the_controls_strips() {
        let scales = test_scales();
        let mut ws = crate::build_workspace(&scales);
        let controls = match crate::insert_layer_controls(&mut ws.tree, ws.layers, &scales) {
            Ok(controls) => controls,
            Err(err) => unreachable!("{err:?}"),
        };
        let display = |ws: &crate::Workspace, id| match ws.tree.style(id) {
            Some(style) => style.display,
            None => unreachable!("exists"),
        };
        if let Err(err) = set_panel_collapsed(&mut ws.tree, ws.layers, true) {
            unreachable!("{err:?}");
        }
        assert_eq!(display(&ws, ws.layers.header), taffy::Display::Flex);
        assert_eq!(display(&ws, ws.layers.body), taffy::Display::None);
        assert_eq!(display(&ws, controls.root), taffy::Display::None);
        ws.tree.compute_layout(1600.0, 900.0);
        let (Some(root_box), Some(header)) = (
            ws.tree.bounds(ws.layers.root),
            ws.tree.bounds(ws.layers.header),
        ) else {
            unreachable!("just laid out");
        };
        assert!(header.height > 0, "the collapsed title still has a row");
        assert_eq!(
            root_box.height, header.height,
            "and the panel is exactly that row"
        );
        if let Err(err) = set_panel_collapsed(&mut ws.tree, ws.layers, false) {
            unreachable!("{err:?}");
        }
        assert_eq!(display(&ws, ws.layers.header), taffy::Display::Flex);
        assert_eq!(display(&ws, controls.root), taffy::Display::Flex);
    }

    /// Review revision (0.142.0): a collapsed panel is exactly its title
    /// row and is never shrunk below it, so in a rail too short for all
    /// three title rows the panels overflow the bottom of the rail
    /// (clipped by the window) rather than shrinking under their own
    /// headers. What is asserted is the stronger of the two possible
    /// contracts: no header overlaps the next panel at all, not merely
    /// "is clipped within its own root".
    #[test]
    fn collapsed_headers_never_overlap_the_next_panel_in_a_rail_shorter_than_three_titles() {
        let scales = test_scales();
        let mut ws = crate::build_workspace(&scales);
        // 0.164.0: two slots — Layers, and the Properties + History group,
        // whose collapsed "title" is its tab row.
        if let Err(err) = set_panel_collapsed(&mut ws.tree, ws.layers, true) {
            unreachable!("{err:?}");
        }
        let group = crate::workspace::test_group(&ws);
        if let Err(err) = crate::set_panel_group_collapsed(&mut ws.tree, &group, true) {
            unreachable!("{err:?}");
        }
        let row_f = widgets::row_height(&scales);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = row_f.round() as u32;
        // Shorter than three title rows (and than two), whatever else the
        // window spends height on.
        ws.tree.compute_layout(1600.0, row_f * 1.5);
        let mut boxes = Vec::new();
        for (root, header) in [(ws.layers.root, ws.layers.header), (group.root, group.bar)] {
            let (Some(root_box), Some(header)) = (ws.tree.bounds(root), ws.tree.bounds(header))
            else {
                unreachable!("just laid out");
            };
            assert_eq!(header.height, row, "the title keeps its full row");
            assert_eq!(
                root_box.height, header.height,
                "a collapsed panel is never shrunk below its title: {root_box:?} vs {header:?}"
            );
            boxes.push((root_box, header));
        }
        boxes.sort_by_key(|(root_box, _)| root_box.y);
        for pair in boxes.windows(2) {
            let [(_, header), (next_root, _)] = pair else {
                unreachable!("windows(2)");
            };
            assert!(
                header.bottom() <= next_root.y,
                "a collapsed title spills over the next panel: {header:?} vs {next_root:?}"
            );
        }
    }

    /// Review revision (0.142.0): closing is not collapsing. A closed
    /// panel hides its title as well and takes no height at all; the
    /// ordinary reopen path (`set_panel_collapsed(.., false)`, which the
    /// app's panel-toggle command calls on a collapsed-or-closed panel)
    /// shows the title again.
    #[test]
    fn a_closed_panel_hides_its_title_and_takes_no_space_until_reopened() {
        let scales = test_scales();
        let mut ws = crate::build_workspace(&scales);
        let display = |ws: &crate::Workspace, id| match ws.tree.style(id) {
            Some(style) => style.display,
            None => unreachable!("exists"),
        };
        if let Err(err) = close_panel(&mut ws.tree, ws.layers) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert_eq!(display(&ws, ws.layers.header), taffy::Display::None);
        let (Some(closed_root), Some(next_root)) = (
            ws.tree.bounds(ws.layers.root),
            ws.tree.bounds(crate::workspace::test_group(&ws).root),
        ) else {
            unreachable!("just laid out");
        };
        assert_eq!(closed_root.height, 0, "a closed panel takes no space");
        assert_eq!(
            next_root.y, closed_root.y,
            "the next panel starts where the closed one would have"
        );

        if let Err(err) = set_panel_collapsed(&mut ws.tree, ws.layers, false) {
            unreachable!("{err:?}");
        }
        ws.tree.compute_layout(1600.0, 900.0);
        assert_eq!(display(&ws, ws.layers.header), taffy::Display::Flex);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let row = widgets::row_height(&scales).round() as u32;
        let Some(header) = ws.tree.bounds(ws.layers.header) else {
            unreachable!("just laid out");
        };
        assert_eq!(header.height, row, "reopened, the title row is back");
    }

    #[test]
    fn close_panel_rejects_an_unknown_body() {
        let (mut tree, _root) = widgets::new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        let panel = super::PanelHandle {
            root: bogus,
            header: bogus,
            body: bogus,
            viewport: bogus,
            scrollbar: bogus,
        };
        match close_panel(&mut tree, panel) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
    /// 0.161.0 review J6: a hidden child of the root adds nothing to the
    /// panel's floor; shown, its declared minimum does.
    #[test]
    fn a_panels_floor_counts_only_its_shown_children() {
        let scales = test_scales();
        let row = widgets::row_height(&scales);
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties", &scales) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let floor = |tree: &aurora_widgets::WidgetTree<WidgetKind>| {
            tree.style(panel.root).map(|style| style.min_size.height)
        };
        let strip = |display| Style {
            display,
            min_size: taffy::Size {
                width: taffy::Dimension::ZERO,
                height: taffy::style_helpers::length(50.0_f32),
            },
            ..Default::default()
        };
        let extra =
            match widgets::insert_container(&mut tree, panel.root, strip(taffy::Display::None)) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
        if let Err(err) = super::refresh_panel_floor(&mut tree, panel) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            floor(&tree),
            Some(taffy::style_helpers::length(2.0 * row)),
            "hidden: not counted"
        );
        if let Err(err) = tree.set_style(extra, strip(taffy::Display::Flex)) {
            unreachable!("{err:?}");
        }
        if let Err(err) = super::refresh_panel_floor(&mut tree, panel) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            floor(&tree),
            Some(taffy::style_helpers::length(2.0 * row + 50.0)),
            "shown: counted"
        );
        // A collapse/expand round trip lands on the same floor: the root
        // is restyled after its children are shown again.
        for collapsed in [true, false] {
            if let Err(err) = set_panel_collapsed(&mut tree, panel, collapsed) {
                unreachable!("{err:?}");
            }
        }
        assert_eq!(
            floor(&tree),
            Some(taffy::style_helpers::length(2.0 * row + 50.0)),
            "after expand"
        );
    }

    /// 0.161.0 review J1: closing is distinguishable from collapsing.
    #[test]
    fn a_closed_panel_is_closed_and_a_collapsed_one_is_not() {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Properties", &test_scales()) {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let closed = |tree: &aurora_widgets::WidgetTree<WidgetKind>| match super::panel_is_closed(
            tree, panel,
        ) {
            Ok(closed) => closed,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!closed(&tree), "expanded");
        if let Err(err) = set_panel_collapsed(&mut tree, panel, true) {
            unreachable!("{err:?}");
        }
        assert!(!closed(&tree), "collapsed is not closed");
        if let Err(err) = close_panel(&mut tree, panel) {
            unreachable!("{err:?}");
        }
        assert!(closed(&tree), "closed");
        if let Err(err) = set_panel_collapsed(&mut tree, panel, false) {
            unreachable!("{err:?}");
        }
        assert!(!closed(&tree), "reopened");
    }
}
