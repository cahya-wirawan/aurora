//! A selection outline (0.212.0): the "marching ants" a selection is drawn
//! with over the canvas. Generic — it knows nothing about documents,
//! selections or views; `aurora-ui` computes the dash runs
//! (`aurora_ui::selection_outline`) and hands them in.
//!
//! **Shape.** One widget per workspace, inserted once as a child of the
//! canvas area and never resized: zero-sized and absolutely placed at its
//! parent's top-left corner, so it can never be the target of a hit test
//! and never moves layout. Its payload is a list of [`OutlineRun`]s — axis
//! aligned rectangles, in *physical* pixels relative to that corner, each
//! in one of two tones. An empty list paints nothing.
//!
//! **Paint** (`paint::paint_selection_outline`): every run of one tone is
//! merged into one flat mesh, in `selection.ants_light` and
//! `selection.ants_dark` respectively (0.219.0, the design owner's tokens:
//! pure white and pure black in every built-in theme, gated at 3:1 against
//! each other), so a dash of either shows on any backdrop. Before 0.219.0
//! the pair was `text.primary` and `surface.canvas`.
//!
//! **Tint (0.217.0, Quick Mask).** The same widget also carries a list of
//! [`TintRect`]s: axis-aligned rectangles in the same physical-pixel space,
//! each with an intensity `level` of `1..=`[`TINT_LEVELS`]. They paint
//! beneath the ants as flat meshes in the `quick_mask.overlay` token's
//! colour (0.219.0: `semantic.red.500`; it was `state.error` before) at an
//! opacity of `level / TINT_LEVELS` of that token's alpha (0.5, Photoshop's
//! Quick Mask red at 50 %; one mesh per level present). An empty list
//! paints nothing.
//!
//! **Accessibility**: AT-`hidden`, never focusable, no actions. The
//! selection itself is announced through the status bar's text.

use accesskit::{Node, Role};
use taffy::style_helpers::TaffyZero as _;
use taffy::{Dimension, Display, LengthPercentageAuto, Position, Size, Style};

use super::WidgetKind;
use crate::{WidgetError, WidgetId, WidgetTree};

/// Which of the two ant colours a run is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineTone {
    /// `selection.ants_light`.
    Light,
    /// `selection.ants_dark`.
    Dark,
}

/// One dash: an axis-aligned rectangle in physical pixels, relative to
/// the outline widget's origin (the canvas area's top-left corner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineRun {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub tone: OutlineTone,
}

/// The number of tint intensity levels.
pub const TINT_LEVELS: u8 = 16;

/// One tinted rectangle in physical pixels relative to the widget's
/// origin; `level` is `1..=`[`TINT_LEVELS`] (a larger one is clamped when
/// painted, `0` paints nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TintRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub level: u8,
}

/// A selection outline's state: the dashes to draw, and the tint beneath.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionOutlineState {
    runs: Vec<OutlineRun>,
    tint: Vec<TintRect>,
}

impl SelectionOutlineState {
    /// The dashes drawn, in no particular order.
    #[must_use]
    pub fn runs(&self) -> &[OutlineRun] {
        &self.runs
    }

    /// The tinted rectangles (Quick Mask), in no particular order.
    #[must_use]
    pub fn tint(&self) -> &[TintRect] {
        &self.tint
    }
}

/// Adds an empty selection outline as the first child of `parent`, so
/// everything else that parent holds (floating panels) paints over it.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_selection_outline(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
) -> Result<WidgetId, WidgetError> {
    let mut node = Node::new(Role::GenericContainer);
    node.set_hidden();
    let style = Style {
        display: Display::Flex,
        position: Position::Absolute,
        inset: taffy::Rect {
            left: LengthPercentageAuto::ZERO,
            top: LengthPercentageAuto::ZERO,
            right: taffy::style_helpers::auto(),
            bottom: taffy::style_helpers::auto(),
        },
        size: Size {
            width: Dimension::ZERO,
            height: Dimension::ZERO,
        },
        ..Default::default()
    };
    let id = tree.insert(
        parent,
        style,
        node,
        WidgetKind::SelectionOutline(SelectionOutlineState::default()),
    )?;
    tree.move_child(id, parent, 0)?;
    Ok(id)
}

/// The outline's state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn selection_outline_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<&SelectionOutlineState, WidgetError> {
    match tree.payload(id) {
        Some(WidgetKind::SelectionOutline(state)) => Ok(state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}

/// Replaces the outline's dashes. Returns whether anything changed (the
/// caller asks for a frame only then). No relayout is needed.
///
/// # Errors
///
/// As [`selection_outline_state`].
pub fn set_selection_outline(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    runs: Vec<OutlineRun>,
) -> Result<bool, WidgetError> {
    if selection_outline_state(tree, id)?.runs == runs {
        return Ok(false);
    }
    match tree.payload_mut(id) {
        Some(WidgetKind::SelectionOutline(state)) => {
            state.runs = runs;
            Ok(true)
        }
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}

/// Replaces the tint rectangles. Returns whether anything changed. No
/// relayout is needed.
///
/// # Errors
///
/// As [`selection_outline_state`].
pub fn set_selection_tint(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    rects: Vec<TintRect>,
) -> Result<bool, WidgetError> {
    if selection_outline_state(tree, id)?.tint == rects {
        return Ok(false);
    }
    match tree.payload_mut(id) {
        Some(WidgetKind::SelectionOutline(state)) => {
            state.tint = rects;
            Ok(true)
        }
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}
