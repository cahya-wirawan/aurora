//! Real content for the Layers panel: a real
//! `aurora_widgets::widgets::tree_view` — a `Role::Tree` holding one
//! `Role::TreeItem` row per layer in an `aurora_doc::LayerTree`, nested
//! to match group structure. PLAN.md M1.8's "Layers, history,
//! tool-options panels" bullet, first slice — Layers only; History and
//! tool-options panels are separate, still-open work.
//!
//! **A real tree, not a hand-built list.** Until 0.77.0 this module
//! wrote its own `Role::List`/`Role::ListItem` nodes and its own row
//! style. It now uses the widget, which is what carries the things a
//! hand-built list structurally could not: a 0-based `level` per row
//! (derived from the row's real parent, never computed here), a real
//! `selected` state that reaches the row's *payload* — so
//! `aurora_widgets::paint_widget` actually draws the highlight, not just
//! announces it — and an `expanded` state with exactly one of
//! `Action::Expand`/`Action::Collapse` on every group row.
//!
//! **Nothing routes those expand/collapse requests yet.** The actions
//! are declared and the state is announced, but no code in `aurora-app`
//! turns an incoming `accesskit::ActionRequest` for `Action::Expand` or
//! `Action::Collapse` on a layer row into a
//! `set_tree_item_expanded` call — that is separate, still-open work,
//! and until it lands a screen-reader user can be told a group is
//! collapsible without being able to collapse it.
//!
//! **The `VoiceOver` navigation bug PLAN.md records for this panel is
//! neither fixed nor claimed fixed here.** No display server exists in
//! this workspace's sandbox, so this change was not tested against a
//! real screen reader either way; whether a real `Role::Tree` helps it
//! is a question for a human on real hardware.
//!
//! **`WidgetError::TreeTooDeep` is propagated but unreachable from a
//! real document.** `aurora_widgets::widgets::MAX_TREE_DEPTH` is `255`
//! and 0-based; `aurora_doc::MAX_LAYER_TREE_DEPTH` is `256` and 1-based
//! (a root layer sits at depth `1`), so the deepest layer a `LayerTree`
//! will accept becomes exactly a depth-`255` row — the cap is reachable
//! and not exceedable, with no margin in either direction. The `?` that
//! carries the error out is free (this function already returns
//! `Result`); no test constructs the case, since doing so would need
//! `aurora-doc`'s `test-support` escape hatch to build a tree the public
//! API refuses.
//!
//! **The row's label is bounded and sanitized.** A layer name on the
//! `.aur` path comes from the file (`App::open_aur_file` →
//! `aurora_io::read_aur`), and `aurora_doc::LayerTree` deliberately
//! stores it unmodified, so this panel puts every name through
//! [`aurora_doc::sanitize_display_name`] before it becomes an
//! `accesskit` label — the same bound `History::journal_descriptions`
//! already applies for the History panel next door. Display-only: the
//! document itself is untouched.
//!
//! **No pixel rendering** — same boundary every widget in this crate
//! keeps: a row gets a real, correct accessible name and description
//! (the layer's own name; its kind, blend mode, opacity, and visibility),
//! no thumbnail/swatch, no drawn pixels — `aurora-vector`/text rendering
//! don't exist yet.
//!
//! **Real, clickable size** (PLAN.md M1.9's "active-layer selection"):
//! row layout is `tree_view`'s own — one row height tall, indented one
//! `scales.spacing.md` step per level, with a width floor so a deeply
//! nested row never resolves to a degenerate zero-width box. So
//! `WidgetTree::compute_layout` gives each row a real, non-zero screen
//! rect a pointer can actually land on
//! (`aurora_widgets::WidgetTree::hit_test`), and every enabled row
//! carries `Action::Focus`/`Action::Click` for the same reason — a
//! screen reader needs a real target too, not just a mouse.
//! [`populate_layers_panel`] returns the `WidgetId -> LayerId` map a
//! caller (`aurora-app`) needs to turn a hit or an `ActionRequest` back
//! into "which layer".
//!
//! **One-shot, not reactive**: [`populate_layers_panel`] builds rows
//! once from whatever `LayerTree` state it's given. It does not diff
//! against a previous population or react to later document edits —
//! refreshing after an edit means calling it again against a freshly
//! emptied panel body, which this module doesn't provide a way to do
//! yet (real, separate work for whenever a document can actually be
//! edited live in `aurora-app`, which doesn't have one open yet either).

use std::collections::HashMap;

use aurora_doc::{LayerId, LayerKind, LayerTree, sanitize_display_name};
use aurora_theme::Scales;
use aurora_widgets::widgets::{
    WidgetKind, insert_tree_item, insert_tree_view, set_tree_item_description,
};
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};

use crate::panel::PanelHandle;

/// Inserts a real tree (`aurora_widgets::widgets::insert_tree_view`,
/// labelled "Layers") under `panel.body`, then one `Role::TreeItem` row
/// per layer in `layers` inside it — root layers first, top-to-bottom,
/// matching `LayerTree`'s own ordering convention, nested to mirror
/// group structure (a group's own layers become its row's children).
/// Returns every inserted row's own id mapped to the `LayerId` it
/// represents, so a caller can turn a real pointer hit or accessibility
/// action back into "which layer".
///
/// **The tree is its own container under `panel.body`, deliberately —
/// the rows are not `panel.body`'s own children.** `crate::panel::
/// insert_panel` builds the body with `Style::default()`, whose
/// `flex_direction` is `Row`; tree rows are `width: percent(1.0)` and as
/// `Row`-direction siblings would lay out *side by side* rather than
/// stacked. Setting the body's own style to `Column` instead would not
/// hold either: `crate::panel::set_panel_collapsed` resets it to
/// `Style::default()` on every expand, so the first collapse/expand
/// round trip would silently scramble the panel. `insert_tree_view`
/// brings its own `FlexDirection::Column` container, which that reset
/// path cannot reach.
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't
/// exist, or [`WidgetError::TreeTooDeep`] if a layer nests deeper than
/// `aurora_widgets::widgets::MAX_TREE_DEPTH` — see this module's own doc
/// comment for why no real `LayerTree` can reach that.
pub fn populate_layers_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    layers: &LayerTree,
) -> Result<HashMap<WidgetId, LayerId>, WidgetError> {
    let view = insert_tree_view(tree, panel.body, Some("Layers"))?;
    let mut rows = HashMap::new();
    for &id in layers.roots() {
        insert_layer_row(tree, view, scales, layers, id, &mut rows)?;
    }
    Ok(rows)
}

fn insert_layer_row(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    scales: &Scales,
    layers: &LayerTree,
    id: LayerId,
    rows: &mut HashMap<WidgetId, LayerId>,
) -> Result<WidgetId, WidgetError> {
    // `LayerTree` stores whatever name the file gave it -- there is no
    // length or content validation on `LayerTree::name`, deliberately,
    // so the document round-trips unmodified. That makes sanitizing the
    // *label* this panel's own job: see `sanitize_display_name`'s own
    // module doc comment, and the History panel next door, which goes
    // through the same bound via `History::journal_descriptions`.
    let name = sanitize_display_name(layers.name(id).unwrap_or("Untitled Layer"));
    // "Non-empty group", not "is a group". `LayerTree::children` returns
    // `None` for a pixel layer and `Some(&[])` for an *empty* group (its
    // own doc comment says so), and `TreeItem`'s `has_children` is a
    // declaration that there is something to expand. An empty group
    // therefore becomes a leaf row -- correctly: it has nothing under
    // it, and announcing "collapsed" over nothing is exactly the state
    // `tree_view` refuses to produce. Its description still reads
    // "Group", so a screen-reader user can still tell what it is.
    let children = layers.children(id).unwrap_or_default();
    let row = insert_tree_item(tree, parent, scales, name.as_ref(), !children.is_empty())?;
    // Through the widget's own setter, never onto the node: `tree_view`
    // rebuilds a row's whole node from its `TreeItemState` on every
    // mutation, including the one `insert_tree_item` performs on *this*
    // row the moment the loop below inserts a child under it.
    set_tree_item_description(tree, row, Some(&describe_layer(layers, id)))?;
    rows.insert(row, id);

    for &child in children {
        insert_layer_row(tree, row, scales, layers, child, rows)?;
    }
    Ok(row)
}

/// `"Group"` / `"Normal, 100%"` / `"Multiply, 80%, hidden"` — blend
/// mode and opacity for a pixel layer, just the kind for a group.
/// `LayerTree` stores opacity/blend mode on groups too, but describing
/// a group only by its kind avoids implying group-level compositing
/// already does something — no compositor honours it yet (`aurora-render`
/// still needs a real layer model to call it with, per that crate's own
/// M1.3 notes).
fn describe_layer(layers: &LayerTree, id: LayerId) -> String {
    let hidden = matches!(layers.visible(id), Some(false));
    let suffix = if hidden { ", hidden" } else { "" };
    match layers.kind(id) {
        Some(LayerKind::Group { .. }) => format!("Group{suffix}"),
        Some(LayerKind::Pixel { .. }) => {
            let blend = layers.blend_mode(id).unwrap_or_default();
            let opacity = layers.opacity(id).unwrap_or(1.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let percent = (opacity * 100.0).round() as u32;
            format!("{blend:?}, {percent}%{suffix}")
        }
        None => "Unknown layer".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::populate_layers_panel;
    use crate::panel::{PanelHandle, insert_panel};
    use aurora_core::Rect;
    use aurora_doc::LayerTree;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self, WidgetKind};
    use aurora_widgets::{WidgetId, WidgetTree};
    use taffy::Style;
    use taffy::style_helpers::length;

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        }
    }

    // The real, committed, owner-approved scales -- the same file
    // `aurora-theme`'s own tests parse, so this exercises real token
    // values, not a synthetic fixture.
    fn test_scales() -> Scales {
        const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
        match Scales::from_toml_str(SCALES_TOML) {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    /// The `Role::Tree` container `populate_layers_panel` inserts under
    /// `panel.body` — the rows' real parent, and the extra level every
    /// assertion below has to look through. See
    /// [`super::populate_layers_panel`]'s own doc comment for why the
    /// rows are not `panel.body`'s own children.
    fn tree_root(tree: &WidgetTree<WidgetKind>, panel: PanelHandle) -> WidgetId {
        let Some(children) = tree.children(panel.body) else {
            unreachable!("a populated panel body exists");
        };
        match children {
            [only] => *only,
            other => unreachable!("expected exactly one tree container, got {other:?}"),
        }
    }

    /// Every row under `root`, in insertion order.
    fn rows_of(tree: &WidgetTree<WidgetKind>, root: WidgetId) -> Vec<WidgetId> {
        match tree.children(root) {
            Some(children) => children.to_vec(),
            None => unreachable!("just populated"),
        }
    }

    #[test]
    fn populate_layers_panel_adds_one_row_per_layer_top_to_bottom() {
        let mut layers = LayerTree::new();
        let background = match layers.add_pixel_layer("Background", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let retouch = match layers.add_pixel_layer("Retouch", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(retouch, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(retouch, 0.8) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_visible(background, false) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        let view = tree_root(&tree, panel);
        let Some(view_accessibility) = tree.accessibility(view) else {
            unreachable!("just populated");
        };
        assert_eq!(view_accessibility.role(), accesskit::Role::Tree);
        assert_eq!(view_accessibility.label(), Some("Layers"));

        // `add_pixel_layer` inserts each new layer as the topmost root,
        // so `retouch` (added second) must come before `background` in
        // `roots()` -- and so in row order too.
        let row_ids = rows_of(&tree, view);
        assert_eq!(row_ids.len(), 2);
        let Some(first_row) = row_ids.first() else {
            unreachable!("just asserted len() == 2");
        };
        let Some(second_row) = row_ids.get(1) else {
            unreachable!("just asserted len() == 2");
        };

        let Some(first) = tree.accessibility(*first_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(first.role(), accesskit::Role::TreeItem);
        assert_eq!(first.label(), Some("Retouch"));
        assert_eq!(first.description(), Some("Multiply, 80%"));
        assert_eq!(first.level(), Some(0), "a root layer is a top-level row");
        assert_eq!(
            first.is_selected(),
            Some(false),
            "a freshly populated panel selects nothing"
        );
        assert_eq!(
            first.is_expanded(),
            None,
            "a pixel layer is a leaf, and a leaf must omit `expanded` entirely"
        );
        assert!(first.supports_action(accesskit::Action::Focus));
        assert!(first.supports_action(accesskit::Action::Click));

        let Some(second) = tree.accessibility(*second_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(second.role(), accesskit::Role::TreeItem);
        assert_eq!(second.label(), Some("Background"));
        assert_eq!(second.description(), Some("Normal, 100%, hidden"));
        assert_eq!(second.level(), Some(0));
        assert_eq!(second.is_selected(), Some(false));

        assert_eq!(
            rows.len(),
            2,
            "the returned map must have one entry per row"
        );
        assert_eq!(rows.get(first_row), Some(&retouch));
        assert_eq!(rows.get(second_row), Some(&background));
    }

    #[test]
    fn populate_layers_panel_gives_rows_a_real_nonzero_computed_size() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("Background", bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(1000.0, 800.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(row_bounds) = tree.bounds(row) else {
            unreachable!("just laid out");
        };
        assert!(
            row_bounds.width > 0 && row_bounds.height > 0,
            "a row must have a real, clickable size after layout: {row_bounds:?}"
        );
    }

    #[test]
    fn populate_layers_panel_nests_group_children_under_their_own_row() {
        let mut layers = LayerTree::new();
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let glow = match layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        assert_eq!(row_ids.len(), 1, "one top-level row for the group");
        let Some(&group_row) = row_ids.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(group_accessibility) = tree.accessibility(group_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(group_accessibility.label(), Some("Effects"));
        assert_eq!(group_accessibility.description(), Some("Group"));
        assert_eq!(group_accessibility.level(), Some(0));
        assert_eq!(
            group_accessibility.is_expanded(),
            Some(true),
            "a group with a real row under it announces itself expanded"
        );
        assert!(
            group_accessibility.supports_action(accesskit::Action::Collapse),
            "and offers exactly the action that could change that"
        );
        assert!(!group_accessibility.supports_action(accesskit::Action::Expand));

        let Some(group_children) = tree.children(group_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(group_children.len(), 1);
        let Some(&child_row) = group_children.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(child_accessibility) = tree.accessibility(child_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(child_accessibility.label(), Some("Glow"));
        assert_eq!(
            child_accessibility.level(),
            Some(1),
            "a layer inside a group is one level deeper -- derived from the real parent"
        );
        assert_eq!(
            child_accessibility.is_expanded(),
            None,
            "the child is a leaf, so it must carry no expanded state at all"
        );

        assert_eq!(rows.get(&group_row), Some(&group));
        assert_eq!(rows.get(&child_row), Some(&glow));
    }

    /// The sibling of the History panel's own bound. A layer name on
    /// the `.aur` path is whatever the file said, and `LayerTree` keeps
    /// it verbatim -- so the *label* is where it has to be bounded, or a
    /// 500 KB name with an embedded bidi override crosses straight into
    /// an assistive technology.
    #[test]
    fn a_hostile_layer_name_reaches_the_label_bounded_and_sanitized() {
        let hostile = format!(
            "safe{}txet{}{}{}{}",
            '\u{202E}',                                     // bidi override
            '\u{0007}',                                     // BEL
            '\u{2028}',                                     // line separator
            char::from_u32(0xE_0041).unwrap_or('\u{FFFD}'), // Tag 'A'
            "a".repeat(500_000),
        );
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer(hostile.clone(), bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TreeItem);
        let Some(label) = accessibility.label() else {
            unreachable!("every row carries a label");
        };
        assert!(
            label.chars().count() <= 129,
            "{} chars",
            label.chars().count()
        );
        for hostile_char in ['\u{202E}', '\u{0007}', '\u{2028}', '\u{E0041}'] {
            assert!(!label.contains(hostile_char), "{hostile_char:?} survived");
        }
        assert!(label.starts_with("safetxet"), "{label:?}");
        // Display-only: the document still holds every byte the file
        // gave it, so a save round-trips the user's own name unchanged.
        let Some(&id) = layers.roots().first() else {
            unreachable!("just added one layer");
        };
        assert_eq!(layers.name(id), Some(hostile.as_str()));
    }

    /// The `.unwrap_or` fallback is unchanged by the sanitizing step --
    /// only the sanitization is new.
    #[test]
    fn an_ordinary_name_is_labelled_verbatim() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_pixel_layer("Retouch — skin", bounds(), None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.role(), accesskit::Role::TreeItem);
        assert_eq!(accessibility.label(), Some("Retouch — skin"));
    }

    /// `LayerTree::children` returns `Some(&[])` for an empty group and
    /// `None` for a pixel layer, so "is a group" and "has something to
    /// expand" are different questions. `TreeItem`'s `has_children` is
    /// the second one: an empty group is a leaf row, which is the truth
    /// (there is nothing to expand into), and its description still says
    /// "Group" so the distinction is not lost to a screen reader.
    #[test]
    fn an_empty_group_is_a_leaf_row_with_no_expand_action() {
        let mut layers = LayerTree::new();
        if let Err(err) = layers.add_group("Empty", None) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one group");
        };
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        assert_eq!(accessibility.label(), Some("Empty"));
        assert_eq!(
            accessibility.description(),
            Some("Group"),
            "a screen-reader user must still be told this is a group"
        );
        assert_eq!(
            accessibility.is_expanded(),
            None,
            "an empty group has nothing to expand, so it carries no expanded state"
        );
        assert!(!accessibility.supports_action(accesskit::Action::Expand));
        assert!(!accessibility.supports_action(accesskit::Action::Collapse));
    }

    /// The regression test for the layout bug the tree container exists
    /// to avoid. `insert_panel` builds `panel.body` with
    /// `Style::default()` -- `FlexDirection::Row` -- so rows parented
    /// straight to it would lay out *side by side*. They must stack, and
    /// each level must indent further than the one above it.
    #[test]
    fn populate_layers_panel_stacks_rows_vertically_and_indents_each_level() {
        let mut layers = LayerTree::new();
        // `Background` first, so the group -- added after it -- becomes
        // the topmost root and so the *first* row.
        if let Err(err) = layers.add_pixel_layer("Background", bounds(), None) {
            unreachable!("{err:?}");
        }
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            unreachable!("{err:?}");
        }

        // A *sized* root, not `Style::default()`: the tree's own
        // `percent` sizes resolve against a definite parent size, and
        // `auto` isn't one (the same precondition `tree_view`'s own
        // layout tests record).
        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: length(300.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        tree.compute_layout(300.0, 400.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        assert_eq!(row_ids.len(), 2, "two roots: the group and Background");
        let (Some(&first), Some(&second)) = (row_ids.first(), row_ids.get(1)) else {
            unreachable!("just asserted len() == 2");
        };
        let (Some(first_bounds), Some(second_bounds)) = (tree.bounds(first), tree.bounds(second))
        else {
            unreachable!("just laid out");
        };
        assert_eq!(
            second_bounds.x, first_bounds.x,
            "sibling rows must share a left edge, not sit beside each other"
        );
        assert_eq!(
            second_bounds.y,
            first_bounds.y + i64::from(first_bounds.height),
            "the second root row must stack directly under the first, not next to it"
        );

        // ... and the group's own child is indented past it.
        let Some(&child) = tree.children(first).unwrap_or_default().first() else {
            unreachable!("the group has one layer in it");
        };
        let Some(child_bounds) = tree.bounds(child) else {
            unreachable!("just laid out");
        };
        assert!(
            child_bounds.x > first_bounds.x,
            "a nested row must be indented further than its own group: {child_bounds:?} vs \
             {first_bounds:?}"
        );
    }

    /// `tree_view::style`'s `padding.top` is what gives a group's own
    /// row somewhere to be: without it the first child would occupy the
    /// same band, and `hit_test` (which prefers the deeper node) would
    /// hand a click on the group to that child. Through this panel's own
    /// API, since that is where a real click arrives.
    #[test]
    fn a_group_rows_own_strip_hit_tests_to_the_group_not_its_first_child() {
        let mut layers = LayerTree::new();
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let glow = match layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let (mut tree, root) = widgets::new_tree(Style {
            size: taffy::Size {
                width: length(300.0_f32),
                height: length(400.0_f32),
            },
            ..Default::default()
        });
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        let rows = match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };
        tree.compute_layout(300.0, 400.0);

        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&group_row) = row_ids.first() else {
            unreachable!("one root row for the group");
        };
        let Some(&child_row) = tree.children(group_row).unwrap_or_default().first() else {
            unreachable!("the group has one layer in it");
        };
        let (Some(group_bounds), Some(child_bounds)) =
            (tree.bounds(group_row), tree.bounds(child_row))
        else {
            unreachable!("just laid out");
        };

        // The group's own strip is everything above where its child
        // starts.
        #[allow(clippy::cast_precision_loss)]
        let group_point = (
            (group_bounds.x + i64::from(group_bounds.width) / 2) as f32,
            (group_bounds.y + (child_bounds.y - group_bounds.y) / 2) as f32,
        );
        assert_eq!(
            tree.hit_test(group_point),
            Some(group_row),
            "a click on a group's own row must select the group"
        );
        assert_eq!(rows.get(&group_row), Some(&group));

        #[allow(clippy::cast_precision_loss)]
        let child_point = (
            (child_bounds.x + i64::from(child_bounds.width) / 2) as f32,
            (child_bounds.y + i64::from(child_bounds.height) / 2) as f32,
        );
        assert_eq!(tree.hit_test(child_point), Some(child_row));
        assert_eq!(rows.get(&child_row), Some(&glow));
    }

    /// Cross-crate proof of `tree_view`'s "the description lives in
    /// `TreeItemState`" rule, through this panel's own surface: a row's
    /// node is rebuilt from state on every mutation, so a description
    /// that was written onto the node instead would vanish the first
    /// time `aurora-app` selects a layer.
    #[test]
    fn a_row_keeps_its_description_after_being_selected() {
        let mut layers = LayerTree::new();
        let retouch = match layers.add_pixel_layer("Retouch", bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(retouch, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(retouch, 0.8) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = test_scales();
        if let Err(err) = populate_layers_panel(&mut tree, panel, &scales, &layers) {
            unreachable!("{err:?}");
        }
        let row_ids = rows_of(&tree, tree_root(&tree, panel));
        let Some(&row) = row_ids.first() else {
            unreachable!("just added one layer");
        };

        if let Err(err) = widgets::set_tree_item_selected(&mut tree, row, true) {
            unreachable!("{err:?}");
        }
        let Some(accessibility) = tree.accessibility(row) else {
            unreachable!("still exists");
        };
        assert_eq!(accessibility.is_selected(), Some(true));
        assert_eq!(
            accessibility.description(),
            Some("Multiply, 80%"),
            "selecting a row must not cost it its description"
        );
        assert_eq!(accessibility.label(), Some("Retouch"));
    }

    #[test]
    fn populate_layers_panel_rejects_an_unknown_panel_body() {
        let layers = LayerTree::new();
        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.remove(panel.body) {
            unreachable!("{err:?}");
        }
        let scales = test_scales();
        match populate_layers_panel(&mut tree, panel, &scales, &layers) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
