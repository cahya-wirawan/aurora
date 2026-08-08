//! A generic "selectable row" payload, shared by any widget kind that
//! shows a homogeneous list of rows a user picks one of —
//! `CommandPalette`'s own result rows (`command_palette::rebuild_rows`)
//! are the first real user; `Dropdown`/`Tree`/`Menu`
//! (`design/gallery/index.html`'s own remaining widget list) are
//! expected to reuse this exact [`super::WidgetKind::ListRow`] variant
//! for the same "which row is highlighted" concept once they exist,
//! rather than each inventing a parallel mechanism.
//!
//! Deliberately thin — unlike every other widget in this module, a row
//! has no `insert_*`/`set_*` API family of its own, and this module
//! builds no `accesskit::Node` either: a row's real accessibility role
//! varies by consumer (`Role::ListBoxOption` for `CommandPalette`,
//! `Role::MenuItem`/`Role::TreeItem` for future consumers), so each
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
