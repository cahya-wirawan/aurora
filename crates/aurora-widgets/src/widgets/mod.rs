//! The concrete widget set. PLAN.md M1.7's fourth deliverable.
//!
//! **Scope, stated honestly**: not yet the full 12-widget list PLAN.md
//! names (button, checkbox, slider, field, dropdown, scrollbar, tree,
//! tab bar, menu, tooltip, colour swatch, curve editor —
//! `design/gallery/index.html`'s own list). `Button` ([`ButtonState`]),
//! `Checkbox` ([`CheckboxState`]), `Slider` ([`SliderState`]) covered
//! three genuinely different interaction shapes first — a discrete
//! trigger, a toggle, and a continuous drag — enough to validate the
//! pattern every other widget follows; `TextField`
//! ([`TextFieldState`]), `CommandPalette` ([`CommandPaletteState`]),
//! `ColorSwatch` ([`ColorSwatchState`]), `Scrollbar`
//! ([`ScrollbarState`]), and now `Tree` ([`TreeItemState`]) followed —
//! **6 of the 12 named widgets**: button, checkbox, slider, scrollbar,
//! colour swatch, and tree. (Recounted, not carried forward: this
//! sentence said "4 of the 12" through `0.75.1`, a count inherited from
//! PLAN.md's own stricter transcription of the same list — which reads
//! "colour picker" where `design/gallery/index.html` has "Color
//! swatch", so [`ColorSwatchState`] was going uncounted against a list
//! it does match. `TextField` is deliberately *still* not counted: the
//! list's own entry is a **number** field, and [`TextFieldState`] has
//! no numeric value, range, or step semantics at all. `CommandPalette`
//! and [`WidgetKind::Panel`] are not on the list in any form.)
//! `Scrollbar` is a deliberately narrow landing: a bounded position
//! *model* with an accessibility node, a layout style, and a paint, but
//! nothing in this crate scrolls any content yet. It has a real
//! component-gallery entry with a contrast check passing in every
//! built-in theme; only its golden-image diff tests are unblessed
//! (`#[ignore]`d pending a human bless on real GPU hardware, the same
//! discipline every other widget's goldens already follow — see
//! `scrollbar.rs`'s own module doc comment for the full account).
//! `Tree` ([`insert_tree_view`]/[`insert_tree_item`]) is the same
//! shape: real hierarchy (a `Role::Tree` container holding real,
//! nested `Role::TreeItem` rows), real expand/collapse that actually
//! adds and removes child widgets, per-level indentation from `taffy`'s
//! own padding accumulation, and a real gallery entry — but still **no
//! scrolling container** (a tree taller than its parent overflows), **no
//! disclosure-triangle glyph** (this crate draws no glyphs at all), and
//! **no in-row content** (a row's own band holds nothing but the row, so
//! the Layers-panel "thumbnail + checkbox + name on one line" shape
//! isn't buildable yet — `tree_view.rs`'s own doc comment has the full
//! list of what it does and doesn't promise). The rest still need
//! infrastructure that doesn't exist yet
//! (real text shaping for dropdowns, popover layering for
//! menus/tooltips, `aurora-vector` path rendering for the curve editor)
//! and are deliberately left open rather than stubbed out half-built.
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
mod color_swatch;
mod command_palette;
mod dialog;
mod list_row;
mod scrollbar;
mod slider;
mod text_field;
mod tree_view;

pub use button::{ButtonState, insert_button, set_button_disabled, set_button_pressed};
pub use checkbox::{CheckboxState, insert_checkbox, set_checkbox_disabled, toggle_checkbox};
pub use color_swatch::{
    ColorSwatchState, insert_color_swatch, set_color_swatch_color, set_color_swatch_disabled,
};
pub use command_palette::{
    CommandEntry, CommandPaletteState, command_palette_state, insert_command_palette,
    move_command_palette_selection, set_command_palette_query,
};
pub use dialog::{DialogAction, DialogHandle, insert_dialog};
pub use list_row::ListRowState;
pub use scrollbar::{
    ScrollbarRange, ScrollbarState, insert_scrollbar, set_scrollbar_disabled, set_scrollbar_value,
};
pub use slider::{SliderState, insert_slider, set_slider_disabled, set_slider_value};
pub use text_field::{
    Composition, TextFieldState, UnderlineStyle, composition_segments, insert_text_field,
    set_text_field_disabled, text_field_state, with_text_field_mut,
};
pub use tree_view::{
    MAX_TREE_DEPTH, TreeItemState, insert_tree_item, insert_tree_view, set_tree_item_description,
    set_tree_item_disabled, set_tree_item_expanded, set_tree_item_label, set_tree_item_selected,
};

use accesskit::{Node, Role};
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
    /// A bounded position along one axis — a position *model* only, not
    /// a scrolling container: nothing in this crate scrolls any content
    /// yet. See `scrollbar.rs`'s own module doc comment.
    Scrollbar(ScrollbarState),
    TextField(TextFieldState),
    CommandPalette(CommandPaletteState),
    ColorSwatch(ColorSwatchState),
    /// A selectable row within some owning widget's own list —
    /// `CommandPalette`'s own result rows today, see
    /// [`ListRowState`]'s own module doc comment for why this is a
    /// deliberately shared, generic variant rather than one per
    /// consumer.
    ListRow(ListRowState),
    /// One row of a tree — a label, its own depth, and whether it is
    /// selected, expanded, and declares children. Deliberately *not*
    /// [`ListRowState`] (which stays exactly as it is): see
    /// `tree_view.rs`'s and `list_row.rs`'s own module doc comments for
    /// why a tree row went its own way rather than widening the shared
    /// variant.
    TreeItem(TreeItemState),
    /// A titled, dockable region's own root — `aurora-ui`'s own
    /// `insert_panel` (Layers/Properties/History today) is the first
    /// real consumer, but nothing about this variant is document- or
    /// layer-aware (no state at all, in fact — see this variant's own
    /// paint for why none is needed), the same "generic primitive,
    /// Aurora-specific *content* stays one layer up" split
    /// [`ListRowState`] already draws. Not one of `design/gallery/
    /// index.html`'s own 12 named widgets — it's workspace chrome, not
    /// a component in that gallery's own sense.
    Panel,
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

/// One tree row's own height: a line of default UI text plus the
/// smallest real spacing step above and below it. Both halves come from
/// the token scales (invariant §7.3.10 — never a literal), through the
/// same two helpers every other widget's own intrinsic size already
/// goes through, so the `cast_precision_loss` allow lives in one place
/// rather than at every call site.
///
/// Shared rather than private to `tree_view` because
/// [`crate::paint_widget`] needs the same number: a tree row's own
/// layout box grows to contain its children, so its *highlight* has to
/// be clamped back to one row's height or a selected parent would paint
/// over every descendant beneath it (`paint::paint_tree_item`).
pub(crate) fn tree_row_height(scales: &Scales) -> f32 {
    type_size(scales.typography.size.md) + spacing(scales.spacing.xxs) * 2.0
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
