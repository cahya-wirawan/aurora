//! The concrete widget set. PLAN.md M1.7's fourth deliverable.
//!
//! **Scope, stated honestly**: this is a first slice, not the full
//! 12-widget list PLAN.md names (button, checkbox, slider, number field,
//! dropdown, scrollbar, tree, tab bar, menu, tooltip, colour picker,
//! curve editor). `Button` ([`ButtonState`]), `Checkbox`
//! ([`CheckboxState`]), and `Slider` ([`SliderState`]) cover three
//! genuinely different interaction shapes — a discrete trigger, a
//! toggle, and a continuous drag — which is enough to validate the
//! pattern every other widget will follow. The rest need infrastructure
//! that doesn't exist yet (text editing for number fields/dropdowns,
//! scrolling for scrollbars/trees, popover layering for menus/tooltips,
//! `aurora-vector` path rendering for the colour picker/curve editor) and
//! are deliberately left open rather than stubbed out half-built.
//!
//! **No rendering**: every widget here produces layout (a `taffy::Style`,
//! resolved from `aurora_theme::Scales` — invariant §7.3.10, no
//! hardcoded spacing) and accessibility content (a real `accesskit::Node`
//! with the right role/actions/value), but there is no vector-first
//! rendering yet (a separate M1.7 bullet, blocked on `aurora-vector`
//! still being an empty skeleton) — nothing here draws a pixel. This
//! mirrors `WidgetTree` itself: a complete, tested logical model with
//! painting layered on afterward, not built into the model.
//!
//! **One shared payload type**: [`WidgetTree`] is generic over a single
//! payload `W` for the whole tree, so a tree containing more than one
//! widget kind needs one enum to unify them — [`WidgetKind`]. A future
//! `aurora-ui` panel is expected to use `WidgetTree<WidgetKind>`
//! directly, the same way this module already does in its own tests.

mod button;
mod checkbox;
mod slider;
mod text_field;

pub use button::{ButtonState, insert_button, set_button_disabled, set_button_pressed};
pub use checkbox::{CheckboxState, insert_checkbox, set_checkbox_disabled, toggle_checkbox};
pub use slider::{SliderState, insert_slider, set_slider_disabled, set_slider_value};
pub use text_field::{
    Composition, TextFieldState, UnderlineStyle, composition_segments, insert_text_field,
    set_text_field_disabled, text_field_state, with_text_field_mut,
};

use accesskit::{Node, Role};
#[cfg(test)]
use aurora_theme::Scales;
use taffy::Style;

use crate::error::WidgetError;
use crate::tree::{WidgetId, WidgetTree};

/// The payload every concrete widget in this module ultimately becomes,
/// once inserted into a [`WidgetTree`] — see this module's own doc
/// comment for why one shared enum is necessary.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetKind {
    /// A plain, non-interactive grouping node — what a fresh
    /// [`WidgetTree::new`]'s root is, and what any purely-layout wrapper
    /// (a row, a panel body) should use.
    Container,
    Button(ButtonState),
    Checkbox(CheckboxState),
    Slider(SliderState),
    TextField(TextFieldState),
}

/// Builds a [`WidgetTree`] whose root is a plain [`WidgetKind::Container`]
/// — the usual way to start a tree meant to hold concrete widgets, so a
/// caller doesn't have to spell out `Role::GenericContainer` themselves
/// every time.
#[must_use]
pub fn new_tree(style: Style) -> (WidgetTree<WidgetKind>, WidgetId) {
    WidgetTree::new(
        Node::new(Role::GenericContainer),
        style,
        WidgetKind::Container,
    )
}

/// Same as [`new_tree`], but for a non-root container inserted as a
/// child — a row, a panel body, anything that exists purely to lay out
/// its children.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `parent` doesn't exist.
pub fn insert_container(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    style: Style,
) -> Result<WidgetId, WidgetError> {
    tree.insert(
        parent,
        style,
        Node::new(Role::GenericContainer),
        WidgetKind::Container,
    )
}

/// `scales.spacing.<name>` as a plain `f32` pixel value — every concrete
/// widget's own layout style goes through this rather than a literal, per
/// invariant §7.3.10.
#[allow(clippy::cast_precision_loss)]
fn spacing(value: u32) -> f32 {
    value as f32
}

/// `scales.typography.size.<name>` as a plain `f32` pixel value, used by
/// widgets (`Checkbox`, `Slider`) whose intrinsic size is better grounded
/// in "about one line of text" than in the spacing scale — there is no
/// dedicated "control size" token yet (`design/tokens/scales.toml` has
/// none), and inventing one is a design decision (CLAUDE.md: "don't
/// invent tokens ad hoc"), not an engineering default to pick here.
#[allow(clippy::cast_precision_loss)]
fn type_size(value: u32) -> f32 {
    value as f32
}

/// The real, committed, owner-approved scales — shared by every widget
/// submodule's own tests (`crate::widgets::test_scales`), so each one
/// exercises its layout style against real values instead of a synthetic
/// fixture, without duplicating the `include_str!`/parse boilerplate
/// four times over.
#[cfg(test)]
pub(crate) fn test_scales() -> Scales {
    const SCALES_TOML: &str = include_str!("../../../../design/tokens/scales.toml");
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(s) => s,
        Err(err) => unreachable!("{err:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{WidgetKind, insert_container, new_tree};
    use taffy::Style;

    #[test]
    fn new_tree_has_a_container_root() {
        let (tree, root) = new_tree(Style::default());
        assert_eq!(tree.payload(root), Some(&WidgetKind::Container));
    }

    #[test]
    fn insert_container_adds_a_plain_grouping_node() {
        let (mut tree, root) = new_tree(Style::default());
        let row = match insert_container(&mut tree, root, Style::default()) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(tree.payload(row), Some(&WidgetKind::Container));
    }
}
