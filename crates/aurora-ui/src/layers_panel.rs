//! Real content for the Layers panel: one accessible, real-sized,
//! clickable row per layer in an `aurora_doc::LayerTree`, nested to
//! match group structure. PLAN.md M1.8's "Layers, history, tool-options
//! panels" bullet, first slice — Layers only; History and tool-options
//! panels are separate, still-open work.
//!
//! **No pixel rendering** — same boundary every widget in this crate
//! keeps: a row gets a real, correct accessible name and description
//! (the layer's own name; its kind, blend mode, opacity, and visibility),
//! no thumbnail/swatch, no drawn pixels — `aurora-vector`/text rendering
//! don't exist yet.
//!
//! **Real, clickable size** (PLAN.md M1.9's "active-layer selection"):
//! each row's own style resolves padding from `scales.spacing`
//! (`md`/`sm`, the same tokens `aurora_widgets::widgets::button` already
//! uses — there is no dedicated "row height" token, and inventing one
//! is a design decision to raise, not fill locally, CLAUDE.md), so
//! `WidgetTree::compute_layout` gives each row a real, non-zero screen
//! rect a pointer can actually land on
//! (`aurora_widgets::WidgetTree::hit_test`), and carries `Action::Focus`/
//! `Action::Click` for the same reason — a screen reader needs a real
//! target too, not just a mouse. [`populate_layers_panel`] returns the
//! `WidgetId -> LayerId` map a caller (`aurora-app`) needs to turn a hit
//! or an `ActionRequest` back into "which layer".
//!
//! **One-shot, not reactive**: [`populate_layers_panel`] builds rows
//! once from whatever `LayerTree` state it's given. It does not diff
//! against a previous population or react to later document edits —
//! refreshing after an edit means calling it again against a freshly
//! emptied panel body, which this module doesn't provide a way to do
//! yet (real, separate work for whenever a document can actually be
//! edited live in `aurora-app`, which doesn't have one open yet either).

use std::collections::HashMap;

use accesskit::{Action, Node, Role};
use aurora_doc::{LayerId, LayerKind, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::style_helpers::length;
use taffy::{Rect as LayoutRect, Style};

use crate::panel::PanelHandle;

/// A row's own layout style — padding only (`scales.spacing.md`
/// horizontal, `.sm` vertical), matching
/// `aurora_widgets::widgets::button`'s own style exactly (no dedicated
/// "row height" token exists to reach for instead — see this module's
/// own doc comment).
fn row_style(scales: &Scales) -> Style {
    Style {
        padding: LayoutRect {
            left: length(spacing(scales.spacing.md)),
            right: length(spacing(scales.spacing.md)),
            top: length(spacing(scales.spacing.sm)),
            bottom: length(spacing(scales.spacing.sm)),
        },
        ..Default::default()
    }
}

/// `scales.spacing.<name>` as a plain `f32` pixel value — every concrete
/// widget's own layout style goes through this rather than a literal,
/// per invariant §7.3.10. A small, local duplicate of
/// `aurora_widgets::widgets`'s own private helper of the same name and
/// body — that one is `pub(crate)` to its own crate, and one line of
/// arithmetic isn't worth a new public cross-crate API for.
#[allow(clippy::cast_precision_loss)]
fn spacing(value: u32) -> f32 {
    value as f32
}

/// Replaces `panel`'s body accessibility with a real `Role::List`, then
/// inserts one `Role::ListItem` row per layer in `layers` as that
/// body's children — root layers first, top-to-bottom, matching
/// `LayerTree`'s own ordering convention, nested to mirror group
/// structure (a group's own layers become its row's children). Returns
/// every inserted row's own id mapped to the `LayerId` it represents,
/// so a caller can turn a real pointer hit or accessibility action back
/// into "which layer".
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_layers_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: &Scales,
    layers: &LayerTree,
) -> Result<HashMap<WidgetId, LayerId>, WidgetError> {
    let mut list_node = Node::new(Role::List);
    list_node.set_label("Layers");
    tree.set_accessibility(panel.body, list_node)?;

    let mut rows = HashMap::new();
    for &id in layers.roots() {
        insert_layer_row(tree, panel.body, scales, layers, id, &mut rows)?;
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
    let name = layers.name(id).unwrap_or("Untitled Layer");
    let mut node = Node::new(Role::ListItem);
    node.set_label(name);
    node.set_description(describe_layer(layers, id));
    node.add_action(Action::Focus);
    node.add_action(Action::Click);
    let row = tree.insert(parent, row_style(scales), node, WidgetKind::Container)?;
    rows.insert(row, id);

    if let Some(children) = layers.children(id) {
        for &child in children {
            insert_layer_row(tree, row, scales, layers, child, rows)?;
        }
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
    use crate::panel::insert_panel;
    use aurora_core::Rect;
    use aurora_doc::LayerTree;
    use aurora_theme::Scales;
    use aurora_widgets::widgets::{self};
    use taffy::Style;

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

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);

        // `add_pixel_layer` inserts each new layer as the topmost root,
        // so `retouch` (added second) must come before `background` in
        // `roots()` -- and so in row order too.
        let Some(row_ids) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
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
        assert_eq!(first.label(), Some("Retouch"));
        assert_eq!(first.description(), Some("Multiply, 80%"));
        assert!(first.supports_action(accesskit::Action::Focus));
        assert!(first.supports_action(accesskit::Action::Click));

        let Some(second) = tree.accessibility(*second_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(second.label(), Some("Background"));
        assert_eq!(second.description(), Some("Normal, 100%, hidden"));

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

        let Some(row_ids) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
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

        let Some(row_ids) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(row_ids.len(), 1, "one top-level row for the group");
        let Some(&group_row) = row_ids.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(group_accessibility) = tree.accessibility(group_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(group_accessibility.label(), Some("Effects"));
        assert_eq!(group_accessibility.description(), Some("Group"));

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

        assert_eq!(rows.get(&group_row), Some(&group));
        assert_eq!(rows.get(&child_row), Some(&glow));
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
