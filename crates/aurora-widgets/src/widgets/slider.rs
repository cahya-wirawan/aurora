//! A slider: a continuous drag, the third of the three interaction
//! shapes this first widget slice covers.

use accesskit::{Action, Node, Role};
use aurora_theme::Scales;
use taffy::style_helpers::length;
use taffy::{Size, Style};

use super::{WidgetKind, type_size};
use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

#[derive(Debug, Clone, PartialEq)]
pub struct SliderState {
    pub label: String,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub disabled: bool,
}

fn node(state: &SliderState) -> Node {
    let mut node = Node::new(Role::Slider);
    node.set_label(state.label.clone());
    node.set_numeric_value(state.value);
    node.set_min_numeric_value(state.min);
    node.set_max_numeric_value(state.max);
    if state.disabled {
        node.set_disabled();
    } else {
        node.add_action(Action::Focus);
        node.add_action(Action::SetValue);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    }
    node
}

fn style(scales: &Scales) -> Style {
    // Fills the available space along the main axis (a slider is sized
    // by its container, not by its own content) with a track height
    // grounded in the type scale -- see `type_size`'s own doc comment on
    // why, same reasoning `checkbox::style` already uses.
    Style {
        flex_grow: 1.0,
        size: Size {
            width: taffy::style_helpers::auto(),
            height: length(type_size(scales.typography.size.md)),
        },
        ..Default::default()
    }
}

/// Adds a new slider as the last child of `parent`, with `value` (already
/// clamped to `min..=max`) as its starting position.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
/// Assumes `min <= max` (the caller's responsibility — there is no
/// sensible way to validate a slider's own range against anything
/// external).
pub fn insert_slider(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    label: impl Into<String>,
    value: f64,
    min: f64,
    max: f64,
) -> Result<WidgetId, WidgetError> {
    let state = SliderState {
        label: label.into(),
        value: value.clamp(min, max),
        min,
        max,
        disabled: false,
    };
    tree.insert(
        parent,
        style(scales),
        node(&state),
        WidgetKind::Slider(state),
    )
}

/// Sets `id` (a slider) to `value`, clamped to its own `min..=max`.
/// Returns the clamped value actually stored.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist,
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a slider, or
/// [`WidgetError::WidgetDisabled`] if it's disabled.
pub fn set_slider_value(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    value: f64,
) -> Result<f64, WidgetError> {
    let mut result = 0.0;
    with_slider_mut(tree, id, |state| {
        if state.disabled {
            return Err(WidgetError::WidgetDisabled(id));
        }
        state.value = value.clamp(state.min, state.max);
        result = state.value;
        Ok(())
    })?;
    Ok(result)
}

/// Sets whether `id` (a slider) is disabled.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `id` doesn't exist, or
/// [`WidgetError::WrongWidgetKind`] if it exists but isn't a slider.
pub fn set_slider_disabled(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    disabled: bool,
) -> Result<(), WidgetError> {
    with_slider_mut(tree, id, |state| {
        state.disabled = disabled;
        Ok(())
    })
}

fn with_slider_mut(
    tree: &mut WidgetTree<WidgetKind>,
    id: WidgetId,
    f: impl FnOnce(&mut SliderState) -> Result<(), WidgetError>,
) -> Result<(), WidgetError> {
    {
        let kind = tree.payload_mut(id).ok_or(WidgetError::UnknownWidget(id))?;
        let WidgetKind::Slider(state) = kind else {
            return Err(WidgetError::WrongWidgetKind(id));
        };
        f(state)?;
    }
    let Some(WidgetKind::Slider(state)) = tree.payload(id) else {
        unreachable!("id was just confirmed to be a Slider above");
    };
    tree.set_accessibility(id, node(state))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{insert_slider, set_slider_disabled, set_slider_value};
    use crate::WidgetError;
    use crate::widgets::{WidgetKind, new_tree, test_scales};
    use accesskit::Action;
    use taffy::Style;

    #[test]
    // Exact-literal round-trip, no arithmetic -- same reasoning
    // `tree::tests` already documents for its own float_cmp allows.
    #[allow(clippy::float_cmp)]
    fn insert_slider_clamps_an_out_of_range_starting_value() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_slider(&mut tree, root, &scales, "vol", 150.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match tree.payload(id) {
            Some(WidgetKind::Slider(state)) => assert_eq!(state.value, 100.0),
            other => unreachable!("expected Slider, got {other:?}"),
        }
        let Some(accessibility) = tree.accessibility(id) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.numeric_value(), Some(100.0));
        assert_eq!(accessibility.min_numeric_value(), Some(0.0));
        assert_eq!(accessibility.max_numeric_value(), Some(100.0));
        assert!(accessibility.supports_action(Action::SetValue));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn set_slider_value_clamps_to_the_sliders_own_range() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_slider(&mut tree, root, &scales, "vol", 50.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        match set_slider_value(&mut tree, id, 75.0) {
            Ok(v) => assert_eq!(v, 75.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_slider_value(&mut tree, id, -10.0) {
            Ok(v) => assert_eq!(v, 0.0),
            Err(err) => unreachable!("{err:?}"),
        }
        match set_slider_value(&mut tree, id, 1000.0) {
            Ok(v) => assert_eq!(v, 100.0),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn set_slider_value_marks_the_widget_dirty() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_slider(&mut tree, root, &scales, "vol", 0.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.take_damage();
        if let Err(err) = set_slider_value(&mut tree, id, 50.0) {
            unreachable!("{err:?}");
        }
        assert_eq!(tree.is_dirty(id), Some(true));
    }

    #[test]
    fn set_slider_value_rejects_a_disabled_slider() {
        let (mut tree, root) = new_tree(Style::default());
        let scales = test_scales();
        let id = match insert_slider(&mut tree, root, &scales, "vol", 0.0, 0.0, 100.0) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = set_slider_disabled(&mut tree, id, true) {
            unreachable!("{err:?}");
        }
        match set_slider_value(&mut tree, id, 50.0) {
            Err(WidgetError::WidgetDisabled(got)) => assert_eq!(got, id),
            other => unreachable!("expected WidgetDisabled, got {other:?}"),
        }
    }

    #[test]
    fn slider_mutators_reject_a_wrong_widget_kind() {
        let (mut tree, root) = new_tree(Style::default());
        match set_slider_value(&mut tree, root, 1.0) {
            Err(WidgetError::WrongWidgetKind(id)) => assert_eq!(id, root),
            other => unreachable!("expected WrongWidgetKind, got {other:?}"),
        }
    }
}
