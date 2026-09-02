//! A generic "selectable row" payload, shared by any widget kind that
//! shows a homogeneous list of rows a user picks one of —
//! `CommandPalette`'s own result rows (`command_palette::rebuild_rows`)
//! are the first real user; `Dropdown`/`Menu`
//! (`design/gallery/index.html`'s own remaining widget list) are
//! expected to reuse this exact [`super::WidgetKind::ListRow`] variant
//! for the same "which row is highlighted" concept once they exist,
//! rather than each inventing a parallel mechanism.
//!
//! **`Tree` was on that list and is no longer** — it has its own
//! [`super::WidgetKind::TreeItem`]/[`super::TreeItemState`] instead
//! (`tree_view.rs`, landed `0.76.0`). Not a change of heart about
//! sharing: a tree row genuinely needs four fields this one does not
//! (`label`, `depth`, `expanded`, `has_children`), and adding them here
//! would have put them on every `CommandPalette` result row — which has
//! no depth and cannot expand — while costing this struct its `Copy`
//! and `Eq` derives (a `String` label is neither), and with them the
//! by-value `paint_list_row(*state, ...)` call and the
//! `assert_eq!(payload, ListRow(...))` comparisons that already depend
//! on them. Reuse where the concept is genuinely the same; a separate
//! variant where it isn't.
//!
//! Deliberately thin — unlike every other widget in this module, a row
//! has no `insert_*`/`set_*` API family of its own, and this module
//! builds no `accesskit::Node` either: a row's real accessibility role
//! varies by consumer (`Role::ListBoxOption` for `CommandPalette`,
//! `Role::MenuItem` for a future menu), so each
//! owning widget builds its own row node and inserts
//! `WidgetKind::ListRow(state)` directly. The same "owning widget
//! controls the node and layout, this module only carries paint-
//! relevant state" split [`super::color_swatch`]'s own doc comment
//! already draws between chrome and content.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListRowState {
    /// Whether this row is the one its owning widget currently has
    /// selected — the one property every consumer needs, since it's
    /// what a highlight actually paints.
    pub selected: bool,
    pub disabled: bool,
}
