//! Real content for the Layers panel: one accessible row per layer in
//! an `aurora_doc::LayerTree`, nested to match group structure.
//! PLAN.md M1.8's "Layers, history, tool-options panels" bullet, first
//! slice — Layers only; History and tool-options panels are separate,
//! still-open work.
//!
//! **No pixel rendering** — same boundary every widget in this crate
//! keeps: a row gets a real, correct accessible name and description
//! (the layer's own name; its kind, blend mode, opacity, and visibility),
//! no thumbnail/swatch, no drawn pixels — `aurora-vector`/text rendering
//! don't exist yet.
//!
//! **One-shot, not reactive**: [`populate_layers_panel`] builds rows
//! once from whatever `LayerTree` state it's given. It does not diff
//! against a previous population or react to later document edits —
//! refreshing after an edit means calling it again against a freshly
//! emptied panel body, which this module doesn't provide a way to do
//! yet (real, separate work for whenever a document can actually be
//! edited live in `aurora-app`, which doesn't have one open yet either).

use accesskit::{Node, Role};
use aurora_doc::{LayerId, LayerKind, LayerTree};
use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{WidgetError, WidgetId, WidgetTree};
use taffy::Style;

use crate::panel::PanelHandle;

/// Replaces `panel`'s body accessibility with a real `Role::List`, then
/// inserts one `Role::ListItem` row per layer in `layers` as that
/// body's children — root layers first, top-to-bottom, matching
/// `LayerTree`'s own ordering convention, nested to mirror group
/// structure (a group's own layers become its row's children).
///
/// # Errors
///
/// Returns [`WidgetError::UnknownWidget`] if `panel.body` doesn't exist.
pub fn populate_layers_panel(
    tree: &mut WidgetTree<WidgetKind>,
    panel: PanelHandle,
    layers: &LayerTree,
) -> Result<(), WidgetError> {
    let mut list_node = Node::new(Role::List);
    list_node.set_label("Layers");
    tree.set_accessibility(panel.body, list_node)?;

    for &id in layers.roots() {
        insert_layer_row(tree, panel.body, layers, id)?;
    }
    Ok(())
}

fn insert_layer_row(
    tree: &mut WidgetTree<WidgetKind>,
    parent: WidgetId,
    layers: &LayerTree,
    id: LayerId,
) -> Result<WidgetId, WidgetError> {
    let name = layers.name(id).unwrap_or("Untitled Layer");
    let mut node = Node::new(Role::ListItem);
    node.set_label(name);
    node.set_description(describe_layer(layers, id));
    let row = tree.insert(parent, Style::default(), node, WidgetKind::Container)?;

    if let Some(children) = layers.children(id) {
        for &child in children {
            insert_layer_row(tree, row, layers, child)?;
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
        if let Err(err) = populate_layers_panel(&mut tree, panel, &layers) {
            unreachable!("{err:?}");
        }

        let Some(body_accessibility) = tree.accessibility(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(body_accessibility.role(), accesskit::Role::List);

        // `add_pixel_layer` inserts each new layer as the topmost root,
        // so `retouch` (added second) must come before `background` in
        // `roots()` -- and so in row order too.
        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 2);
        let Some(first_row) = rows.first() else {
            unreachable!("just asserted len() == 2");
        };
        let Some(second_row) = rows.get(1) else {
            unreachable!("just asserted len() == 2");
        };

        let Some(first) = tree.accessibility(*first_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(first.label(), Some("Retouch"));
        assert_eq!(first.description(), Some("Multiply, 80%"));

        let Some(second) = tree.accessibility(*second_row) else {
            unreachable!("just inserted");
        };
        assert_eq!(second.label(), Some("Background"));
        assert_eq!(second.description(), Some("Normal, 100%, hidden"));
    }

    #[test]
    fn populate_layers_panel_nests_group_children_under_their_own_row() {
        let mut layers = LayerTree::new();
        let group = match layers.add_group("Effects", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_pixel_layer("Glow", bounds(), Some(group)) {
            unreachable!("{err:?}");
        }

        let (mut tree, root) = widgets::new_tree(Style::default());
        let panel = match insert_panel(&mut tree, root, "Layers") {
            Ok(panel) => panel,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = populate_layers_panel(&mut tree, panel, &layers) {
            unreachable!("{err:?}");
        }

        let Some(rows) = tree.children(panel.body) else {
            unreachable!("just populated");
        };
        assert_eq!(rows.len(), 1, "one top-level row for the group");
        let Some(&group_row) = rows.first() else {
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
        match populate_layers_panel(&mut tree, panel, &layers) {
            Err(aurora_widgets::WidgetError::UnknownWidget(id)) => assert_eq!(id, panel.body),
            other => unreachable!("expected UnknownWidget, got {other:?}"),
        }
    }
}
