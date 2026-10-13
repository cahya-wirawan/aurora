//! The Tools panel's **foreground/background colour** controls (0.208.0):
//! Photoshop's overlapped swatch pair with a swap arrow at its top-right
//! and a default-colours icon at its bottom-left, below the tool buttons
//! (reference `images/photoshop-workspace-01.jpg`, section C), and the
//! colour-picker popover a swatch opens.
//!
//! **This module owns no colour state.** The colours are `aurora-app`'s
//! (`ColourState`: document-independent, no undo, never dirty); the app
//! mirrors them into the swatches with [`sync_colour_controls`] and turns
//! a click on a swatch, the swap button or the defaults button into the
//! same commands the `X` and `D` shortcuts run.
//!
//! **Layout, from tokens.** The pair sits in a fixed-height
//! group (`ColourControls::group`) whose four children are placed
//! absolutely: the background swatch at `(spacing.md, spacing.md)`, the
//! foreground swatch in front at `(0, 0)` (both `size.colour_swatch` square, 24 px, so
//! they overlap by `size.colour_swatch - spacing.md`), the swap button flush
//! right at the top and the defaults button flush left at the bottom
//! (both small icon buttons, `spacing.md` icon plus `spacing.xxs` each
//! side). The group's height is [`colour_group_height`]: the pair
//! (`size.colour_swatch + spacing.md`) plus one small button row. At the default density that is 64 px, inside
//! the strip's 64 px of content width ([`crate::tools_panel_width`] less
//! its padding), so the panel stays 72 px wide. The swatch side is the
//! `size.colour_swatch` token (0.219.0); the overlap offset and the group
//! height are derived from `spacing.*` and `icon_button_side`.
//!
//! **Accessibility.** Every control has a node. A swatch is a
//! `Role::ColorWell` named with its colour as text ("Foreground colour,
//! RGB 0 0 0"), so the colour is never conveyed by the fill alone; its
//! description names the shortcuts. The two icon buttons are named "Swap
//! foreground and background colours" and "Default colours", described
//! with their shortcuts (`X`, `D`).
//!
//! **The picker.** [`open_colour_popover`] adds a `RaisedPanel` popover
//! (`PaintLayer::Popover`, so it paints and hit-tests above everything,
//! unclipped) to the right of the group, holding the toolkit's
//! [`ColorPicker`](aurora_widgets::widgets::insert_color_picker). It is
//! HSV only: **no hex or numeric entry, no alpha, no colour
//! management** (the picker has none) — gaps, not half-built features.
//! Closing removes the whole popover subtree, so nothing stale can hold
//! focus.
//!
//! The group and the popover are provisional and have not been seen by a
//! human, nor on the Mac.

use accesskit::{Node, Role};
use aurora_theme::{Color, Scales};
use aurora_widgets::icons::IconKind;
use aurora_widgets::widgets::{self, IconSize, WidgetKind};
use aurora_widgets::{PaintLayer, WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::{auto, length, percent, zero};
use taffy::{FlexDirection, Position, Rect as LayoutRect, Size, Style};

/// The group's accessible label.
pub const COLOUR_CONTROLS_LABEL: &str = "Foreground and background colour";

/// The swap button's accessible name.
pub const SWAP_COLOURS_LABEL: &str = "Swap foreground and background colours";

/// The defaults button's accessible name.
pub const DEFAULT_COLOURS_LABEL: &str = "Default colours";

/// The swap button's description (name of the shortcut included).
pub const SWAP_COLOURS_DESCRIPTION: &str =
    "Swaps the foreground and background colours, shortcut X";

/// The defaults button's description.
pub const DEFAULT_COLOURS_DESCRIPTION: &str =
    "Resets the foreground to black and the background to white, shortcut D";

/// The picker's square is this many [`widgets::row_height`]s on a side (an engineering layout
/// multiplier, like the gallery's, not a style value).
pub const COLOUR_PICKER_ROWS: f32 = 6.0;

/// Which of the two colours a control is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColourTarget {
    Foreground,
    Background,
}

impl ColourTarget {
    /// "Foreground" / "Background".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            ColourTarget::Foreground => "Foreground",
            ColourTarget::Background => "Background",
        }
    }

    /// The other colour.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            ColourTarget::Foreground => ColourTarget::Background,
            ColourTarget::Background => ColourTarget::Foreground,
        }
    }
}

/// The group and its four controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourControls {
    /// The `Role::Group` holding the pair and the two buttons.
    pub group: WidgetId,
    /// The background swatch (behind).
    pub background: WidgetId,
    /// The foreground swatch (in front).
    pub foreground: WidgetId,
    /// The swap button, top-right.
    pub swap: WidgetId,
    /// The default-colours button, bottom-left.
    pub defaults: WidgetId,
}

impl ColourControls {
    /// The swatch of `target`.
    #[must_use]
    pub const fn swatch(&self, target: ColourTarget) -> WidgetId {
        match target {
            ColourTarget::Foreground => self.foreground,
            ColourTarget::Background => self.background,
        }
    }

    /// The colour a swatch `id` stands for, if it is one of the two.
    #[must_use]
    pub fn target_of(&self, id: WidgetId) -> Option<ColourTarget> {
        if id == self.foreground {
            Some(ColourTarget::Foreground)
        } else if id == self.background {
            Some(ColourTarget::Background)
        } else {
            None
        }
    }
}

/// What an activation of a colour control means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourAction {
    /// A swatch: open (or close) its picker.
    Edit(ColourTarget),
    /// The swap button.
    Swap,
    /// The default-colours button.
    Reset,
}

impl ColourControls {
    /// The action activating `id` runs, if `id` is one of the controls.
    #[must_use]
    pub fn action_for(&self, id: WidgetId) -> Option<ColourAction> {
        if let Some(target) = self.target_of(id) {
            Some(ColourAction::Edit(target))
        } else if id == self.swap {
            Some(ColourAction::Swap)
        } else if id == self.defaults {
            Some(ColourAction::Reset)
        } else {
            None
        }
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(token: u32) -> f32 {
    token as f32
}

/// A swatch's side: the `size.colour_swatch` token (0.219.0, 24 px, what
/// `spacing.lg` gave before).
#[must_use]
pub fn colour_swatch_side(scales: &Scales) -> f32 {
    px(scales.size.colour_swatch)
}

/// How far the background swatch is offset from the foreground one, on
/// both axes: `spacing.md`.
#[must_use]
pub fn colour_swatch_offset(scales: &Scales) -> f32 {
    px(scales.spacing.md)
}

/// The group's height: the pair (a swatch side plus the offset), then the
/// defaults button's row below it.
#[must_use]
pub fn colour_group_height(scales: &Scales) -> f32 {
    colour_swatch_side(scales)
        + colour_swatch_offset(scales)
        + widgets::icon_button_side(scales, IconSize::Small)
}

/// The icon size of the swap and defaults buttons.
pub const COLOUR_BUTTON_ICON_SIZE: IconSize = IconSize::Small;

fn group_style(scales: &Scales) -> Style {
    Style {
        flex_shrink: 0.0,
        size: Size {
            width: auto(),
            height: length(colour_group_height(scales)),
        },
        ..Default::default()
    }
}

fn swatch_style(scales: &Scales, x: f32, y: f32) -> Style {
    let side = length(colour_swatch_side(scales));
    Style {
        position: Position::Absolute,
        size: Size {
            width: side,
            height: side,
        },
        inset: LayoutRect {
            left: length(x),
            top: length(y),
            right: auto(),
            bottom: auto(),
        },
        ..Default::default()
    }
}

/// The 8-bit colour a swatch draws for straight sRGB `rgb` (rounded,
/// clamped; a non-finite channel draws as 0).
#[must_use]
pub fn swatch_colour(rgb: [f32; 3]) -> Color {
    let to_u8 = |c: f32| {
        if c.is_finite() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            byte
        } else {
            0
        }
    };
    Color {
        r: to_u8(rgb[0]),
        g: to_u8(rgb[1]),
        b: to_u8(rgb[2]),
    }
}

/// A swatch's accessible name: "Foreground colour, RGB 0 0 0" (8-bit
/// channels, so the colour is stated in text and not by the fill alone).
#[must_use]
pub fn swatch_name(target: ColourTarget, rgb: [f32; 3]) -> String {
    let c = swatch_colour(rgb);
    format!("{} colour, RGB {} {} {}", target.label(), c.r, c.g, c.b)
}

/// A swatch's accessible description.
#[must_use]
pub fn swatch_description(target: ColourTarget) -> String {
    format!(
        "Opens the colour picker for the {} colour. X swaps, D resets to black and white",
        target.label().to_lowercase()
    )
}

/// Adds the colour group as the last child of `parent` (the Tools
/// strip), showing `foreground` over `background`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist. On
/// failure the partly built group is removed again.
pub fn insert_colour_controls(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    foreground: [f32; 3],
    background: [f32; 3],
) -> Result<ColourControls, WidgetError> {
    let mut node = Node::new(Role::Group);
    node.set_label(COLOUR_CONTROLS_LABEL);
    let group = tree.insert(parent, group_style(scales), node, WidgetKind::Container)?;
    let built = build(tree, group, scales, foreground, background);
    if built.is_err() {
        let _ = tree.remove(group);
    }
    built
}

fn build(
    tree: &mut WidgetTree<WidgetKind>,
    group: WidgetId,
    scales: &Scales,
    foreground: [f32; 3],
    background: [f32; 3],
) -> Result<ColourControls, WidgetError> {
    let offset = colour_swatch_offset(scales);
    // Background first, so the foreground paints (and hit-tests) in front.
    let bg = widgets::insert_color_swatch_styled(
        tree,
        group,
        swatch_style(scales, offset, offset),
        swatch_colour(background),
    )?;
    let fg = widgets::insert_color_swatch_styled(
        tree,
        group,
        swatch_style(scales, 0.0, 0.0),
        swatch_colour(foreground),
    )?;
    let swap = widgets::insert_icon_button(
        tree,
        group,
        scales,
        SWAP_COLOURS_LABEL,
        IconKind::SwapArrows,
        COLOUR_BUTTON_ICON_SIZE,
    )?;
    widgets::set_button_description(tree, swap, Some(SWAP_COLOURS_DESCRIPTION))?;
    place(tree, swap, Place::TopRight)?;
    let defaults = widgets::insert_icon_button(
        tree,
        group,
        scales,
        DEFAULT_COLOURS_LABEL,
        IconKind::DefaultColours,
        COLOUR_BUTTON_ICON_SIZE,
    )?;
    widgets::set_button_description(tree, defaults, Some(DEFAULT_COLOURS_DESCRIPTION))?;
    place(tree, defaults, Place::BottomLeft)?;
    let controls = ColourControls {
        group,
        background: bg,
        foreground: fg,
        swap,
        defaults,
    };
    sync_colour_controls(tree, &controls, foreground, background)?;
    Ok(controls)
}

#[derive(Clone, Copy)]
enum Place {
    TopRight,
    BottomLeft,
}

fn place(tree: &mut WidgetTree<WidgetKind>, id: WidgetId, at: Place) -> Result<(), WidgetError> {
    let mut style = tree
        .style(id)
        .cloned()
        .ok_or(WidgetError::UnknownWidget(id))?;
    style.position = Position::Absolute;
    style.inset = match at {
        Place::TopRight => LayoutRect {
            left: auto(),
            right: zero(),
            top: zero(),
            bottom: auto(),
        },
        Place::BottomLeft => LayoutRect {
            left: zero(),
            right: auto(),
            top: auto(),
            bottom: zero(),
        },
    };
    tree.set_style(id, style)
}

/// Shows `foreground` and `background` in the swatches and their
/// accessible names, touching only what differs. Returns whether anything
/// changed.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`]/[`WidgetError::WrongWidgetKind`]
/// if a swatch is missing.
pub fn sync_colour_controls(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ColourControls,
    foreground: [f32; 3],
    background: [f32; 3],
) -> Result<bool, WidgetError> {
    let mut changed = false;
    for (target, rgb) in [
        (ColourTarget::Foreground, foreground),
        (ColourTarget::Background, background),
    ] {
        let id = controls.swatch(target);
        let colour = swatch_colour(rgb);
        let shown = match tree.payload(id) {
            Some(WidgetKind::ColorSwatch(state)) => state.color,
            Some(_) => return Err(WidgetError::WrongWidgetKind(id)),
            None => return Err(WidgetError::UnknownWidget(id)),
        };
        let named = tree
            .accessibility(id)
            .is_some_and(|node| node.label().is_some());
        if shown != colour {
            widgets::set_color_swatch_color(tree, id, colour)?;
            changed = true;
        }
        // The name and description change only with the colour (and are
        // set once at first sync), so build their strings only then.
        if shown != colour || !named {
            changed |= widgets::set_color_swatch_accessible(
                tree,
                id,
                &swatch_name(target, rgb),
                Some(&swatch_description(target)),
            )?;
        }
    }
    Ok(changed)
}

/// Whether `id` is a colour control of the Tools panel or inside the
/// colour popover: any widget at or below a `Role::Group` labelled
/// [`COLOUR_CONTROLS_LABEL`]. Read off the tree because the app's
/// ownership test is handed the tree alone.
#[must_use]
pub fn is_colour_control(tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
    let mut current = Some(id);
    while let Some(candidate) = current {
        if tree.accessibility(candidate).is_some_and(|node| {
            node.role() == Role::Group && node.label() == Some(COLOUR_CONTROLS_LABEL)
        }) {
            return true;
        }
        current = tree.parent(candidate);
    }
    false
}

/// The open colour popover: a raised frame holding one colour picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourPopover {
    /// Which colour the picker edits.
    pub target: ColourTarget,
    /// The `PaintLayer::Popover` frame.
    pub frame: WidgetId,
    /// The colour picker inside it.
    pub picker: WidgetId,
}

impl ColourPopover {
    /// Whether `id` is the popover or inside it.
    #[must_use]
    pub fn contains(&self, tree: &WidgetTree<WidgetKind>, id: WidgetId) -> bool {
        tree.is_within(self.frame, id)
    }
}

/// Opens a colour picker for `target`, set to `rgb`, as a popover to the
/// right of the group. Returns it; the caller keeps it to close it.
///
/// # Errors
///
/// Returns a [`WidgetError`] if the group or the picker cannot be built;
/// the partly built popover is removed again.
pub fn open_colour_popover(
    tree: &mut WidgetTree<WidgetKind>,
    controls: &ColourControls,
    scales: &Scales,
    target: ColourTarget,
    rgb: [f32; 3],
) -> Result<ColourPopover, WidgetError> {
    let pad = length(px(scales.spacing.sm));
    let style = Style {
        position: Position::Absolute,
        flex_direction: FlexDirection::Column,
        padding: LayoutRect {
            left: pad,
            right: pad,
            top: pad,
            bottom: pad,
        },
        inset: LayoutRect {
            left: percent(1.0_f32),
            right: auto(),
            top: auto(),
            bottom: zero(),
        },
        ..Default::default()
    };
    let mut node = Node::new(Role::Group);
    node.set_label(format!("{} colour picker", target.label()));
    let frame = tree.insert(controls.group, style, node, WidgetKind::RaisedPanel)?;
    let built = (|| {
        tree.set_layer(frame, PaintLayer::Popover)?;
        let side = COLOUR_PICKER_ROWS * widgets::row_height(scales);
        let picker = widgets::insert_color_picker(
            tree,
            frame,
            scales,
            &format!("{} colour", target.label()),
            swatch_colour(rgb),
            side,
        )?;
        Ok(ColourPopover {
            target,
            frame,
            picker,
        })
    })();
    if built.is_err() {
        let _ = tree.remove(frame);
    }
    built
}

/// Removes the popover's whole subtree.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if it is already gone.
pub fn close_colour_popover(
    tree: &mut WidgetTree<WidgetKind>,
    popover: &ColourPopover,
) -> Result<(), WidgetError> {
    tree.remove(popover.frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::build_workspace;
    use aurora_widgets::icons::IconKind;

    fn scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn swatch_names_state_the_colour_in_text() {
        assert_eq!(
            swatch_name(ColourTarget::Foreground, [0.0, 0.0, 0.0]),
            "Foreground colour, RGB 0 0 0"
        );
        assert_eq!(
            swatch_name(ColourTarget::Background, [1.0, 1.0, 1.0]),
            "Background colour, RGB 255 255 255"
        );
        assert_eq!(
            swatch_name(ColourTarget::Foreground, [0.5, 0.25, 2.0]),
            "Foreground colour, RGB 128 64 255"
        );
        assert_eq!(
            swatch_name(ColourTarget::Foreground, [f32::NAN, -1.0, 0.0]),
            "Foreground colour, RGB 0 0 0"
        );
    }

    #[test]
    fn group_height_is_derived_from_tokens() {
        let s = scales();
        let expected = px(s.size.colour_swatch)
            + px(s.spacing.md)
            + widgets::icon_button_side(&s, IconSize::Small);
        assert!((colour_group_height(&s) - expected).abs() < f32::EPSILON);
        // 0.219.0: the tokens reproduce the geometry drawn before them.
        assert!((colour_swatch_side(&s) - 24.0).abs() < f32::EPSILON);
        assert!((colour_group_height(&s) - 64.0).abs() < f32::EPSILON);
        // And they follow the token, not `spacing.lg`.
        let mut bigger = s.clone();
        bigger.size.colour_swatch = 30;
        assert!((colour_swatch_side(&bigger) - 30.0).abs() < f32::EPSILON);
        assert!((colour_group_height(&bigger) - 70.0).abs() < f32::EPSILON);
    }

    #[test]
    fn target_and_action_lookups_agree() {
        let ws = build_workspace(&scales());
        let c = ws.tools.colours;
        assert_eq!(c.target_of(c.foreground), Some(ColourTarget::Foreground));
        assert_eq!(c.target_of(c.background), Some(ColourTarget::Background));
        assert_eq!(c.target_of(c.swap), None);
        assert_eq!(
            c.action_for(c.foreground),
            Some(ColourAction::Edit(ColourTarget::Foreground))
        );
        assert_eq!(c.action_for(c.swap), Some(ColourAction::Swap));
        assert_eq!(c.action_for(c.defaults), Some(ColourAction::Reset));
        assert_eq!(c.action_for(c.group), None);
        assert_eq!(ColourTarget::Foreground.other(), ColourTarget::Background);
    }
    #[test]
    fn the_group_is_the_last_child_of_the_strip_behind_a_rule() {
        let ws = build_workspace(&scales());
        let children = ws.tree.children(ws.tools.root).unwrap_or_default();
        assert_eq!(children.last(), Some(&ws.tools.colours.group));
        let Some(rule) = children.get(children.len().saturating_sub(2)) else {
            unreachable!("a rule precedes the group");
        };
        assert_eq!(
            ws.tree.accessibility(*rule).map(accesskit::Node::role),
            Some(Role::Splitter)
        );
        let Some(node) = ws.tree.accessibility(ws.tools.colours.group) else {
            unreachable!("built");
        };
        assert_eq!(node.role(), Role::Group);
        assert_eq!(node.label(), Some(COLOUR_CONTROLS_LABEL));
        let c = ws.tools.colours;
        // Background first so the foreground paints in front.
        assert_eq!(
            ws.tree.children(c.group),
            Some(&[c.background, c.foreground, c.swap, c.defaults][..])
        );
        for id in [c.group, c.background, c.foreground, c.swap, c.defaults] {
            assert!(is_colour_control(&ws.tree, id));
        }
        assert!(!is_colour_control(&ws.tree, ws.tools.root));
        assert!(!is_colour_control(&ws.tree, ws.tools.buttons[0].1));
        assert!(!is_colour_control(&ws.tree, ws.canvas_area));
    }

    #[test]
    fn the_buttons_wear_the_new_icons() {
        let ws = build_workspace(&scales());
        let c = ws.tools.colours;
        for (id, icon) in [
            (c.swap, IconKind::SwapArrows),
            (c.defaults, IconKind::DefaultColours),
        ] {
            let Some(WidgetKind::Button(state)) = ws.tree.payload(id) else {
                unreachable!("a button");
            };
            assert_eq!(state.icon, Some((icon, COLOUR_BUTTON_ICON_SIZE)));
        }
    }

    #[test]
    fn the_popover_opens_beside_the_group_floats_above_and_closes_clean() {
        let scales = scales();
        let mut ws = build_workspace(&scales);
        ws.tree.compute_layout(1000.0, 800.0);
        let before = ws.tree.len();
        let c = ws.tools.colours;
        let Ok(open) = open_colour_popover(
            &mut ws.tree,
            &c,
            &scales,
            ColourTarget::Foreground,
            [0.0; 3],
        ) else {
            unreachable!("opens");
        };
        ws.tree.compute_layout(1000.0, 800.0);
        assert_eq!(ws.tree.layer(open.frame), Some(PaintLayer::Popover));
        assert!(open.contains(&ws.tree, open.picker));
        let (Some(frame), Some(strip), Some(group)) = (
            ws.tree.bounds(open.frame),
            ws.tree.bounds(ws.tools.root),
            ws.tree.bounds(c.group),
        ) else {
            unreachable!("laid out");
        };
        assert!(
            frame.x >= group.x + i64::from(group.width),
            "right of the group"
        );
        assert!(
            frame.x + i64::from(frame.width) <= 1000,
            "inside the window"
        );
        assert_eq!(
            frame.y + i64::from(frame.height),
            group.y + i64::from(group.height),
            "anchored to the group's bottom"
        );
        let _ = strip;
        // Its picker accepts a hit.
        let Some(picker) = ws.tree.bounds(open.picker) else {
            unreachable!("laid out");
        };
        let centre = (
            picker.x as f32 + picker.width as f32 / 2.0,
            picker.y as f32 + picker.height as f32 / 2.0,
        );
        assert!(
            ws.tree
                .hit_test(centre)
                .is_some_and(|hit| open.contains(&ws.tree, hit))
        );
        assert!(close_colour_popover(&mut ws.tree, &open).is_ok());
        assert_eq!(ws.tree.len(), before, "nothing is left behind");
        assert!(close_colour_popover(&mut ws.tree, &open).is_err());
    }

    #[test]
    fn the_gallery_shows_the_new_icons_in_every_tone() {
        let scales = scales();
        let mut ws = build_workspace(&scales);
        let root = ws.root;
        let Ok(_gallery) = crate::insert_gallery_panel(&mut ws.tree, root, &scales) else {
            unreachable!("the gallery builds");
        };
        let shown = |kind: IconKind| {
            ws.tree
                .paint_order()
                .into_iter()
                .filter(|id| {
                    matches!(ws.tree.payload(*id), Some(WidgetKind::Icon(state)) if state.kind == kind)
                })
                .count()
        };
        for kind in [IconKind::SwapArrows, IconKind::DefaultColours] {
            assert!(shown(kind) >= 3, "{kind:?} in three tones: {}", shown(kind));
        }
    }

    #[test]
    fn a_failed_build_leaves_no_partial_group() {
        let mut ws = build_workspace(&scales());
        let bogus = accesskit::NodeId(u64::MAX);
        assert!(
            insert_colour_controls(&mut ws.tree, bogus, &scales(), [0.0; 3], [1.0; 3]).is_err()
        );
    }
}
