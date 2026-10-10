//! A drop indicator (0.166.0): the marker a drag-and-drop gesture shows
//! where a drop would land. Generic — it knows nothing about panels,
//! documents or what is being dragged; `aurora-ui`'s dock decides where
//! it goes and what it means.
//!
//! **Shape.** One widget, inserted once, hidden until a drag needs it:
//! `Display::None` and AT-`hidden` (it is a purely visual echo of a
//! pointer gesture; the gesture's result is what an assistive technology
//! reads, through the tree it changes). [`show_drop_indicator`] places it
//! absolutely at a rectangle relative to its parent's own origin and
//! picks one of two looks ([`DropIndicatorKind`]); [`hide_drop_indicator`]
//! takes it away. It is a `Container` in the accessibility sense and
//! declares no actions, so it is never a `Tab` stop or a click target an
//! assistive technology is offered.
//!
//! **Paint** (`paint::paint_drop_indicator`): `accent.primary` — the
//! "selection highlight" token, gated at 3:1 against `surface.panel` by
//! `design/check_contrast.py`, the same colour the selected tab's
//! underline uses. An [`DropIndicatorKind::Insertion`] fills its whole
//! box (the caller sizes it as a line); a [`DropIndicatorKind::Target`]
//! outlines its box with a `size.indicator_width` stroke (the token the
//! design owner added for this in 0.166.0, shared with the selected tab's
//! underline; a caller sizes an insertion line with it too). Hidden, it
//! paints nothing at all.

use accesskit::{Node, Role};
use aurora_core::Rect;
use taffy::style_helpers::{auto, length};
use taffy::{Display, Position, Style};

use super::WidgetKind;
use crate::{WidgetError, WidgetId, WidgetTree};

/// Which look a shown drop indicator has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropIndicatorKind {
    /// An insertion point between two items: a filled line.
    Insertion,
    /// A target the dragged item would join: an outline around it.
    Target,
}

/// A drop indicator's state: `None` while hidden.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DropIndicatorState {
    shown: Option<DropIndicatorKind>,
}

impl DropIndicatorState {
    /// The shown look, `None` while hidden.
    #[must_use]
    pub fn shown(&self) -> Option<DropIndicatorKind> {
        self.shown
    }
}

fn hidden_style() -> Style {
    Style {
        display: Display::None,
        position: Position::Absolute,
        ..Default::default()
    }
}

/// Adds a hidden drop indicator as the last child of `parent`.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_drop_indicator(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
) -> Result<WidgetId, WidgetError> {
    let mut node = Node::new(Role::GenericContainer);
    node.set_hidden();
    tree.insert(
        parent,
        hidden_style(),
        node,
        WidgetKind::DropIndicator(DropIndicatorState::default()),
    )
}

/// The indicator's state.
///
/// # Errors
///
/// [`WidgetError::UnknownWidget`] / [`WidgetError::WrongWidgetKind`].
pub fn drop_indicator_state(
    tree: &WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<DropIndicatorState, WidgetError> {
    match tree.payload(id) {
        Some(WidgetKind::DropIndicator(state)) => Ok(*state),
        Some(_) => Err(WidgetError::WrongWidgetKind(id)),
        None => Err(WidgetError::UnknownWidget(id)),
    }
}

/// Shows the indicator at `rect` (logical px, relative to its parent's
/// own origin) with look `kind`. The caller re-runs layout.
///
/// # Errors
///
/// As [`drop_indicator_state`].
pub fn show_drop_indicator(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    rect: Rect,
    kind: DropIndicatorKind,
) -> Result<(), WidgetError> {
    drop_indicator_state(tree, id)?;
    #[allow(clippy::cast_precision_loss)]
    let (x, y, width, height) = (
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );
    let style = Style {
        display: Display::Flex,
        position: Position::Absolute,
        inset: taffy::Rect {
            left: length(x),
            top: length(y),
            right: auto(),
            bottom: auto(),
        },
        size: taffy::Size {
            width: length(width),
            height: length(height),
        },
        ..Default::default()
    };
    if tree.style(id) != Some(&style) {
        tree.set_style(id, style)?;
    }
    set_shown(tree, id, Some(kind))
}

/// Hides the indicator (a no-op when already hidden). The caller re-runs
/// layout.
///
/// # Errors
///
/// As [`drop_indicator_state`].
pub fn hide_drop_indicator(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
) -> Result<(), WidgetError> {
    if drop_indicator_state(tree, id)?.shown.is_none() {
        return Ok(());
    }
    tree.set_style(id, hidden_style())?;
    set_shown(tree, id, None)
}

fn set_shown(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    shown: Option<DropIndicatorKind>,
) -> Result<(), WidgetError> {
    if let Some(WidgetKind::DropIndicator(state)) = tree.payload_mut(id) {
        if state.shown == shown {
            return Ok(());
        }
        state.shown = shown;
    }
    tree.mark_dirty(id)
}

#[cfg(test)]
mod tests {
    use super::{
        DropIndicatorKind, drop_indicator_state, hide_drop_indicator, insert_drop_indicator,
        show_drop_indicator,
    };
    use crate::widgets::new_tree;
    use aurora_core::Rect;
    use taffy::Style;

    #[test]
    fn a_drop_indicator_is_hidden_until_shown_and_lays_out_where_it_is_put() {
        let (mut tree, root) = new_tree(Style::default());
        let id = match insert_drop_indicator(&mut tree, root) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            drop_indicator_state(&tree, id).ok().map(|s| s.shown()),
            Some(None)
        );
        assert!(
            tree.accessibility(id)
                .is_some_and(accesskit::Node::is_hidden)
        );
        tree.compute_layout(400.0, 300.0);
        assert_eq!(tree.bounds(id).map(|b| (b.width, b.height)), Some((0, 0)));
        let rect = Rect {
            x: 30,
            y: 40,
            width: 100,
            height: 2,
        };
        if let Err(err) = show_drop_indicator(&mut tree, id, rect, DropIndicatorKind::Insertion) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(400.0, 300.0);
        assert_eq!(tree.bounds(id), Some(rect));
        assert_eq!(
            drop_indicator_state(&tree, id).ok().and_then(|s| s.shown()),
            Some(DropIndicatorKind::Insertion)
        );
        if let Err(err) = hide_drop_indicator(&mut tree, id) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(400.0, 300.0);
        assert_eq!(
            drop_indicator_state(&tree, id).ok().map(|s| s.shown()),
            Some(None)
        );
        assert_eq!(tree.bounds(id).map(|b| (b.width, b.height)), Some((0, 0)));
        assert!(drop_indicator_state(&tree, root).is_err());
    }
}
