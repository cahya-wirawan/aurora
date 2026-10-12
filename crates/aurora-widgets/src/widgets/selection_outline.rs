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
//! merged into one flat mesh, in `text.primary` and `surface.canvas`
//! respectively, the two existing tokens that are the document's own
//! maximum-contrast pair in every built-in theme (text on the canvas
//! surface), so a dash of either shows on any backdrop. No token was added;
//! see PLAN.md for the gap this leaves (a dedicated `selection.ants.*`
//! pair is the design owner's call).
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
    /// `text.primary`.
    Light,
    /// `surface.canvas`.
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

/// A selection outline's state: the dashes to draw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionOutlineState {
    runs: Vec<OutlineRun>,
}

impl SelectionOutlineState {
    /// The dashes drawn, in no particular order.
    #[must_use]
    pub fn runs(&self) -> &[OutlineRun] {
        &self.runs
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
