//! A colour swatch: a small filled square displaying one arbitrary RGB
//! colour (e.g. the document's current foreground/background colour, or
//! one entry in a colour picker) and acting as a button to pick or open
//! one — `design/gallery/index.html`'s own "Color swatch" section names
//! exactly this shape (`accent.primary`-filled by default there, but
//! that's a static mockup's own placeholder; a real swatch's colour is
//! caller-supplied data, not a style token — the same reason
//! [`crate::widgets::SliderState`]'s own `value` isn't one either).
//!
//! Unlike every other widget in this module, the colour a swatch shows
//! is *not* resolved from `aurora_theme::Scales`/`Theme` — invariant
//! §7.3.10 governs a widget's own chrome (background, spacing, radius),
//! not the arbitrary content it displays, the same distinction that
//! already lets `TextField::content` hold arbitrary user text without
//! being a "hardcoded style value."

use accesskit::{Action, Node, Role};
use aurora_theme::{Color, Scales};
use taffy::style_helpers::length;
use taffy::{Size, Style};

use super::{WidgetKind, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorSwatchState {
    pub color: Color,
    pub disabled: bool,
}

fn node(state: ColorSwatchState) -> Node {
    let mut node = Node::new(Role::ColorWell);
    node.set_color_value(accesskit::Color {
        red: state.color.r,
        green: state.color.g,
        blue: state.color.b,
        alpha: 255,
    });
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::Click);
    }
    node
}

fn style(scales: &Scales) -> Style {
    // Same "no dedicated control-size token exists yet" grounding
    // `Checkbox`'s own `style` already uses, for the same reason: a
    // literal (even the design mockup's own hardcoded 32px) isn't a
    // resolved token, and inventing a new one here isn't this crate's
    // decision to make (invariant §7.3.10, `CLAUDE.md`: "don't invent
    // tokens ad hoc").
    let side = length(type_size(scales.typography.size.md));
    Style {
        size: Size {
            width: side,
            height: side,
        },
        ..Default::default()
    }
}

/// Adds a new, enabled colour swatch showing `color` as the last child
/// of `parent`.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_color_swatch(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    color: Color,
) -> Result<WidgetId, WidgetError> {
    let state = ColorSwatchState {
        color,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales),
        node(state),
        WidgetKind::ColorSwatch(state),
    )
}

/// Sets the colour `id` (a colour swatch) displays.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a colour
/// swatch.
pub fn set_color_swatch_color(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    color: Color,
) -> Result<(), WidgetError> {
    with_color_swatch_mut(tree, id, |state| {
        state.color = color;
        Ok(())
    })
}

/// Sets whether `id` (a colour swatch) is disabled.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a colour
/// swatch.
pub fn set_color_swatch_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_color_swatch_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_color_swatch_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut ColorSwatchState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::ColorSwatch(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::ColorSwatch(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a ColorSwatch above");
    };
    tree.set_accessibility(id, node(*state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{insert_color_swatch, set_color_swatch_color, set_color_swatch_disabled};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::Action;
    use aurora_theme::Color;
    use taffy::Style;

    const RED: Color = Color {
        r: 200,
        g: 40,
        b: 40,
    };
    const BLUE: Color = Color {
        r: 40,
        g: 40,
        b: 200,
    };

    #[test]
    fn insert_color_swatch_creates_a_fresh_enabled_swatch_with_the_given_color() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_color_swatch(&mut tree, root, &scales, RED) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::ColorSwatch(state)) => {
                assert_eq!(state.color, RED);
                assert!(!state.disabled);
            }
            other => unreachable!("expected ColorSwatch, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(
            accessibility.color_value(),
            Some(accesskit::Color {
                red: RED.r,
                green: RED.g,
                blue: RED.b,
                alpha: 255,
            })
        );
        assert!(accessibility.supports_action(Action::Click));
    }

    #[test]
    fn set_color_swatch_color_updates_state_and_the_accessibility_node() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_color_swatch(&mut tree, root, &scales, RED) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();

        if let Err(err) = set_color_swatch_color(&mut tree, id, BLUE) {
            unreachable!("{err:?}");
        }
        match tree.payload(id) {
            Some(WidgetKind::ColorSwatch(state)) => assert_eq!(state.color, BLUE),
            other => unreachable!("expected ColorSwatch, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.color_value().map(|c| c.blue), Some(BLUE.b));
        assert_eq!(tree.is_dirty(id), Some(true));
    }

    #[test]
    fn set_color_swatch_disabled_clears_the_accesskit_actions() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_color_swatch(&mut tree, root, &scales, RED) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_color_swatch_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match tree.payload(id) {
            Some(WidgetKind::ColorSwatch(state)) => assert!(state.disabled),
            other => unreachable!("expected ColorSwatch, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert!(accessibility.is_disabled());
        assert!(!accessibility.supports_action(Action::Focus));
        assert!(!accessibility.supports_action(Action::Click));
    }

    #[test]
    fn color_swatch_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_color_swatch_color(&mut tree, root, RED) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }

    #[test]
    fn color_swatch_mutators_reject_an_unknown_widget() {
        let (mut tree, _root) = new_tree(Style::default());
        let bogus = accesskit::NodeId(999);
        match set_color_swatch_color(&mut tree, bogus, RED) {
            Err(WidgetError::UnknownWidget(id)) => assert_eq!(id, bogus),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
