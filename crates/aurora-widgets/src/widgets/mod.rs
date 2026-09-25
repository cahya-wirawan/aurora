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
//! ([`ScrollbarState`]), `Tree` ([`TreeItemState`]), `Dropdown`
//! ([`DropdownState`], `0.120.0`), `TabBar` ([`TabBarState`],
//! `0.121.0`), `Tooltip` ([`Tooltip`], `0.122.0`), `Menu`
//! ([`MenuState`], `0.123.0`), the colour picker
//! ([`ColorPickerState`], `0.125.0`) and now the curve editor
//! ([`CurveEditorState`], `0.126.0`) followed — **11 of the 12 named
//! widgets**: button, checkbox, slider, dropdown, scrollbar, colour
//! swatch/picker, tree, tab bar, tooltip, menu, and curve editor; the
//! number field remains. The picker
//! takes the swatch's slot rather than adding an eleventh (PLAN.md's
//! own list reads "colour picker" where the gallery reads "Color
//! swatch"); [`ColorSwatchState`] stays a primitive, reused by the
//! picker as its preview. (Recounted,
//! not carried forward: this sentence said "4 of the 12" through `0.75.1`, a count inherited from
//! PLAN.md's own stricter transcription of the same list — which reads
//! "colour picker" where `design/gallery/index.html` has "Color
//! swatch", so [`ColorSwatchState`] was going uncounted against a list
//! it does match. `TextField` is deliberately *still* not counted: the
//! list's own entry is a **number** field, and [`TextFieldState`] has
//! no numeric value, range, or step semantics at all.
//! `CommandPalette`, [`WidgetKind::Panel`] and [`WidgetKind::Dialog`]
//! are not on the list in any form, so none of them moves this count.)
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
//! list of what it does and doesn't promise). `Dropdown`
//! ([`insert_dropdown`]/[`handle_dropdown_key`]) is a select-only combo
//! box: a real `Role::ComboBox` whose open list is a real
//! `Role::ListBox` child holding [`WidgetKind::ListRow`] options, removed on
//! close, driven by an exhaustively tested key table — but painted in
//! ordinary tree order with **no popover layering** (a later sibling
//! paints over the open list), with options **unreachable by
//! hit-testing** (they lie outside the control's bounds), and with no
//! text or `▾` glyph (`dropdown.rs`'s own doc comment has the full
//! list). `TabBar` ([`insert_tab_bar`]/[`handle_tab_bar_key`]) is a
//! real `Role::TabList` holding one real `Role::Tab` per tab, with
//! automatic activation, wrapping arrow keys, `Home`/`End`, and roving
//! focus (only the selected tab is focusable) — the bar only, no tab
//! panels, no label glyphs, and no keyboard-focus ring (`tab_bar.rs`'s
//! own doc comment has the full list). `Tooltip` ([`Tooltip`]) is a
//! caller-owned show/hide controller, not a payload: a pure,
//! exhaustively tested hover/focus/delay/`Escape` state machine that
//! inserts a real `Role::Tooltip` child under its owner while shown and
//! removes it when hidden — the caller supplies every `Instant` (no clock
//! in this crate), it never writes the owner's own node, and like the
//! dropdown's list it paints in ordinary tree order with **no popover
//! layering**, no viewport flip, and no text measurement
//! (`tooltip.rs`'s own doc comment has the full list). `Menu`
//! ([`open_menu`]/[`handle_menu_key`]) is a keyboard-driven popup menu: a
//! real `Role::Menu` holding focus, its highlighted `Role::MenuItem` its
//! `active_descendant`, with separators, disabled items skipped, wrapping
//! arrows, `Home`/`End`, and its whole subtree removed on activate or
//! cancel — again with **no popover layering**, no pointer support, no
//! submenus and no text (`menu.rs`'s own doc comment has the full list).
//! The colour picker ([`insert_color_picker`]/[`handle_color_picker_key`])
//! is an HSV saturation/value square, a hue strip and a read-only
//! preview swatch — the first widget to paint a vertex-coloured
//! gradient (`0.124.0`'s primitive), exposed to accessibility as two
//! channel sliders in a group plus a hue slider (`accesskit` has no
//! two-dimensional role), with no text entry, no alpha and no focus ring
//! (`color_picker.rs`'s own doc comment has the full list).
//! The curve editor ([`insert_curve_editor`]/[`handle_curve_editor_key`])
//! edits an `aurora_core::ToneCurve` (a monotone cubic through 2 to 16
//! control points, the model living in `aurora-core` so the future
//! Curves adjustment shares it): one root that paints a well, a quarter
//! grid, the identity diagonal, the curve and a marker per point, and one
//! `Role::Slider` per point with roving focus (only the selected point is
//! a tab stop). Points are selected, moved, added and removed from the
//! keyboard and the pointer (geometric hit-testing, no pointer capture —
//! a drag is the caller calling a `*_from_point` function per move), with
//! no histogram, no channel selector and no text (`curve_editor.rs`'s own
//! doc comment has the full list). The rest — a number field — needs no
//! popover layering but still needs number semantics that don't exist
//! yet, and is deliberately left open rather than stubbed out
//! half-built.
//!
//! **Every module here is a model, not a painter** — and that is a
//! division of labour, not a missing feature. A widget module produces
//! layout (a `taffy::Style`, resolved from `aurora_theme::Scales` —
//! invariant §7.3.10, no hardcoded spacing) and accessibility content
//! (a real `accesskit::Node` with the right role/actions/value); the
//! pixels are [`crate::paint_widget`]'s job, one layer over, which
//! tessellates real geometry through `aurora-vector` for **twenty**
//! of the [`WidgetKind`] variants below (every one except `Container`,
//! [`WidgetKind::ColorPicker`]'s own root, whose children paint, and
//! [`WidgetKind::CurveEditorPoint`], whose root paints;
//! [`WidgetKind::CurveEditor`] as of `0.126.0`;
//! [`WidgetKind::ColorPickerPart`] as of `0.125.0`, the first to paint a
//! gradient through [`crate::paint_widget_ops`],
//! [`WidgetKind::Dialog`] included as of `0.79.0`,
//! [`WidgetKind::Dropdown`] and [`WidgetKind::DropdownList`] as of
//! `0.120.0`, [`WidgetKind::TabBar`] and [`WidgetKind::Tab`] as of
//! `0.121.0`, [`WidgetKind::Tooltip`] as of `0.122.0`,
//! [`WidgetKind::Menu`] and [`WidgetKind::MenuSeparator`] as of
//! `0.123.0`). This mirrors
//! `WidgetTree` itself: a complete, tested logical model with painting
//! layered on afterward, not built into the model.
//!
//! (That paragraph said "there is no vector-first rendering yet …
//! nothing here draws a pixel" through `0.79.0`. It was written when
//! `aurora-vector` really was an empty skeleton and went stale without
//! being noticed. What is *actually* still missing is **glyphs**: no
//! module here and nothing in `paint.rs` draws text, so every label,
//! every dialog title and message, and every tree row's own name reach
//! the accessibility tree and nothing else.)
//!
//! **One shared payload type**: [`WidgetTree`] is generic over a single
//! payload `W` for the whole tree, so a tree containing more than one
//! widget kind needs one enum to unify them — [`WidgetKind`]. A future
//! `aurora-ui` panel is expected to use `WidgetTree<WidgetKind>`
//! directly, the same way this module already does in its own tests.

mod button;
mod checkbox;
mod color_picker;
mod color_swatch;
mod command_palette;
mod curve_editor;
mod dialog;
mod dropdown;
mod list_row;
mod menu;
mod scrollbar;
mod slider;
mod tab_bar;
mod text_field;
mod tooltip;
mod tree_view;

pub use button::{ButtonState, insert_button, set_button_disabled, set_button_pressed};
pub use checkbox::{CheckboxState, insert_checkbox, set_checkbox_disabled, toggle_checkbox};
pub use color_picker::{
    ColorPickerKey, ColorPickerOutcome, ColorPickerPart, ColorPickerPartRole, ColorPickerPartState,
    ColorPickerState, Hsv, color_picker_part_at, color_picker_part_of, color_picker_state,
    handle_color_picker_key, insert_color_picker, set_color_picker_color,
    set_color_picker_disabled, set_color_picker_hsv, set_hue_from_point,
    set_saturation_value_from_point,
};
pub use color_swatch::{
    ColorSwatchState, insert_color_swatch, set_color_swatch_color, set_color_swatch_disabled,
};
pub use command_palette::{
    CommandEntry, CommandPaletteState, command_palette_state, insert_command_palette,
    move_command_palette_selection, set_command_palette_query,
};
pub use curve_editor::{
    CurveEditorKey, CurveEditorOutcome, CurveEditorPointState, CurveEditorState,
    add_curve_point_from_point, curve_editor_of, curve_editor_point_at, curve_editor_state,
    handle_curve_editor_key, insert_curve_editor, move_selected_point_from_point,
    select_curve_point, set_curve_editor_disabled, set_curve_editor_points,
};
pub(crate) use curve_editor::{MARKER_RING_WIDTH, plot_rect};
pub use dialog::{DialogAction, DialogHandle, insert_dialog};
pub use dropdown::{
    DropdownKey, DropdownOutcome, DropdownState, dropdown_state, handle_dropdown_key,
    insert_dropdown, set_dropdown_disabled, set_dropdown_open, set_dropdown_selected,
    toggle_dropdown,
};
pub use list_row::ListRowState;
pub use menu::{
    MenuItem, MenuItemKind, MenuKey, MenuOutcome, MenuState, close_menu, handle_menu_key,
    menu_state, open_menu,
};
pub use scrollbar::{
    ScrollbarRange, ScrollbarState, insert_scrollbar, set_scrollbar_disabled, set_scrollbar_value,
};
pub use slider::{SliderState, insert_slider, set_slider_disabled, set_slider_value};
pub use tab_bar::{
    TabBarKey, TabBarOutcome, TabBarState, TabState, handle_tab_bar_key, insert_tab_bar,
    select_tab, set_tab_bar_disabled, tab_bar_state,
};
pub use text_field::{
    Composition, TextFieldState, UnderlineStyle, composition_segments, insert_text_field,
    set_text_field_disabled, text_field_state, with_text_field_mut,
};
pub use tooltip::{Tooltip, TooltipPhase};
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
    /// `CommandPalette`'s own result rows, `Dropdown`'s option rows
    /// (0.120.0) and `Menu`'s action items (0.123.0) today, see
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
    /// A modal dialog's own root — `widgets::dialog`'s `insert_dialog` is
    /// the only constructor. No state, for the same reason [`Panel`] has
    /// none: its paint is a pure function of its own bounds and the
    /// theme. Its fill is `surface.overlay` ("Elevation 2: modals,
    /// dialogs", `design/tokens/vocabulary.md`), deliberately *not* the
    /// `surface.raised` [`CommandPalette`] resolves, plus an
    /// unconditional `border.default` outline — see
    /// `paint::paint_dialog`. Not one of `design/gallery/index.html`'s
    /// own 12 named widgets either, the same status [`Panel`] and
    /// [`CommandPalette`] already have.
    ///
    /// **The unit-like shape is a real, disclosed trade-off.** The
    /// moment a dialog needs a visual *variant* — a destructive
    /// "danger" alert painting `state.error` where a neutral one paints
    /// `surface.overlay` is the obvious first one — this has to become
    /// `Dialog(DialogState)`, which breaks every exhaustive `match` on
    /// [`WidgetKind`] in and above this crate. This enum carries no
    /// `#[non_exhaustive]` to soften that, deliberately: adding one is
    /// an API decision affecting every existing match site, not a
    /// dialog-sized change, and the blast radius today is small (every
    /// consumer is a path dependency inside this workspace — nothing
    /// here is published). Recorded so the cost is visible when someone
    /// does need the variant, not so it reads as already handled.
    ///
    /// [`Panel`]: WidgetKind::Panel
    /// [`CommandPalette`]: WidgetKind::CommandPalette
    Dialog,
    /// A select-only dropdown's own control — `Role::ComboBox`, painted
    /// as a `surface.sunken` well with a border that turns
    /// `border.focus` while open (`paint::paint_dropdown`). See
    /// `dropdown.rs`'s own module doc comment for the transition table,
    /// the accessibility vocabulary and which adapters read it, and what
    /// it deliberately does not do (no popover layering, options
    /// unreachable by hit-testing).
    Dropdown(DropdownState),
    /// An open dropdown's own list — the `Role::ListBox` child
    /// `dropdown.rs` inserts on open and removes on close, holding one
    /// [`WidgetKind::ListRow`] per option. No state, for the same reason
    /// [`WidgetKind::Panel`] has none: its paint (`surface.raised`, the
    /// "Elevation 1: dropdowns, popovers" token, plus an unconditional
    /// `border.default` outline — `paint::paint_dropdown_list`) is a pure
    /// function of its bounds and the theme.
    DropdownList,
    /// A tab bar's own row — `Role::TabList`, painted as a 1 px
    /// `border.default` rule along its bottom edge
    /// (`paint::paint_tab_bar`). See `tab_bar.rs`'s own module doc
    /// comment for the key table, the roving-focus accessibility shape,
    /// and what it deliberately does not do (no panels, no focus ring).
    TabBar(TabBarState),
    /// One tab of a [`WidgetKind::TabBar`] — `Role::Tab`, created by
    /// `tab_bar.rs` at insert. The selected tab paints an
    /// `accent.primary` underline (`paint::paint_tab`); an inactive one
    /// paints nothing outside High Contrast.
    Tab(TabState),
    /// A shown tooltip — the `Role::Tooltip` child `tooltip.rs`'s
    /// [`Tooltip`] controller inserts under its owner while shown and
    /// removes when hidden. No state, for the same reason
    /// [`WidgetKind::DropdownList`] has none: its text lives only in its
    /// accessibility label, and its paint (`surface.overlay` plus an
    /// unconditional `border.default` outline — `paint::paint_tooltip`)
    /// is a pure function of its bounds and the theme. See `tooltip.rs`'s
    /// own module doc comment for the transition table and what it
    /// deliberately does not do (no z-layering, no text measurement).
    Tooltip,
    /// An open popup menu's own root — `Role::Menu`, inserted by
    /// `menu.rs`'s [`open_menu`] and removed when an item is activated or
    /// the menu is cancelled (its existence *is* "open"). Holds one child
    /// per item: a [`WidgetKind::ListRow`] with a `Role::MenuItem` node
    /// per action, a [`WidgetKind::MenuSeparator`] per separator. Painted
    /// as `surface.raised` ("Elevation 1: ... context menus") with an
    /// unconditional `border.default` outline (`paint::paint_menu`). See
    /// `menu.rs`'s own module doc comment for the key table, the
    /// `active_descendant` focus model, and what it deliberately does not
    /// do (no popover layer, no pointer, no submenus).
    Menu(MenuState),
    /// A separator between groups of a [`WidgetKind::Menu`]'s items —
    /// `Role::Splitter`, created only by `menu.rs`, never on its own. No
    /// state: its paint (one centred `border.default` band at most 1 px
    /// tall, `paint::paint_menu_separator`) is a pure function of its
    /// bounds and the theme.
    MenuSeparator,
    /// A colour picker's own root — `Role::Group`, inserted by
    /// `color_picker.rs`'s [`insert_color_picker`]. Paints nothing
    /// itself; its children do. See `color_picker.rs`'s own module doc
    /// comment for the structure, the key table, the two-slider
    /// accessibility shape and what it deliberately does not do.
    ColorPicker(ColorPickerState),
    /// One of a [`WidgetKind::ColorPicker`]'s own parts, created only by
    /// `color_picker.rs`: the saturation/value square (paints a
    /// `PaintOp::Gradient` plus its marker), its two channel sliders
    /// (paint nothing), or the hue strip (a gradient plus its marker).
    /// The gradient colours are *content*, not tokens; the markers are
    /// tokens.
    ColorPickerPart(ColorPickerPartState),
    /// A curve editor's own root — `Role::Group`, inserted by
    /// `curve_editor.rs`'s [`insert_curve_editor`]. Paints **everything**
    /// the editor shows (well, grid, identity diagonal, curve, point
    /// markers — `paint::paint_curve_editor`); its point children paint
    /// nothing. See `curve_editor.rs`'s own module doc comment for the
    /// key table, the roving-focus accessibility shape and what it
    /// deliberately does not do.
    CurveEditor(CurveEditorState),
    /// One control point of a [`WidgetKind::CurveEditor`] — a
    /// `Role::Slider` over the editor's whole box, created only by
    /// `curve_editor.rs`. Paints nothing.
    CurveEditorPoint(CurveEditorPointState),
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

/// One list-or-tree row's own height: a line of default UI text plus
/// the smallest real spacing step above and below it. Both halves come
/// from the token scales (invariant §7.3.10 — never a literal), through
/// the same two helpers every other widget's own intrinsic size already
/// goes through, so the `cast_precision_loss` allow lives in one place
/// rather than at every call site.
///
/// Three consumers, which is why this is shared rather than private to
/// `tree_view` (and why it is named for a *row* rather than a tree row
/// — it was `tree_row_height` through `0.77.1`, when `tree_view` was
/// the only widget that laid rows out):
///
/// 1. [`insert_tree_item`]'s own row layout — one row height tall, and
///    the floor under a deeply indented row's width.
/// 2. [`crate::paint_widget`] needs the same number: a tree row's own
///    layout box grows to contain its children, so its *highlight* has
///    to be clamped back to one row's height or a selected parent would
///    paint over every descendant beneath it (`paint::paint_tree_item`).
/// 3. `aurora_ui::panel`'s own shared `row_style`, which both the
///    History and the Properties panel build their rows from — plain
///    [`WidgetKind::ListRow`]s under a panel body rather than a tree,
///    taking their `min_size` from here so a row in either panel is
///    exactly as tall as a Layers row beside it and never a degenerate
///    box. (It lived in `aurora_ui::history_panel` when this list was
///    written, and served one panel; it moved and gained its second
///    caller in `0.77.4`.)
#[must_use]
pub fn row_height(scales: &Scales) -> f32 {
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
