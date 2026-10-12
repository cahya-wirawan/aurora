//! The Layers panel's filter (0.199.0, round L5b): semantics, the filter
//! row's structure, and its interaction with the rows.

use std::collections::HashMap;

use aurora_core::Rect;
use aurora_doc::{Adjustment, LayerId, LayerTree};
use aurora_theme::Scales;
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{WidgetId, WidgetTree};
use taffy::Style;

use crate::layer_filter::{KindToggle, LayerFilter, shown_layers};
use crate::layers_panel::{LayersView, populate_layers_panel_view};
use crate::panel::{PanelHandle, insert_panel};
use crate::{NO_MATCH_TEXT, insert_layer_controls, layer_row_neighbor};

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn scales() -> Scales {
    ok(Scales::from_toml_str(include_str!(
        "../../../design/tokens/scales.toml"
    )))
}

fn rect() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    }
}

struct Doc {
    layers: LayerTree,
    background: LayerId,
    post2: LayerId,
    logo: LayerId,
    vso: LayerId,
    #[allow(dead_code)]
    effect: LayerId,
    glow: LayerId,
    post1: LayerId,
    title: LayerId,
    post3: LayerId,
    curves: LayerId,
}

/// Background; Post 2 { Logo, Vector Smart Object, Effect { Glow } };
/// Post 1 { Title }; Post 3 (empty); Curves 1.
fn doc() -> Doc {
    let mut layers = LayerTree::new();
    let background = ok(layers.add_pixel_layer("Background", rect(), None));
    let post2 = ok(layers.add_group("Post 2", None));
    let logo = ok(layers.add_pixel_layer("Logo", rect(), Some(post2)));
    let vso = ok(layers.add_pixel_layer("Vector Smart Object", rect(), Some(post2)));
    let effect = ok(layers.add_group("Effect", Some(post2)));
    let glow = ok(layers.add_pixel_layer("Glow", rect(), Some(effect)));
    let post1 = ok(layers.add_group("Post 1", None));
    let title = ok(layers.add_pixel_layer("Title", rect(), Some(post1)));
    let post3 = ok(layers.add_group("Post 3", None));
    let curves = ok(layers.add_adjustment_layer_at(
        "Curves 1",
        Adjustment::Curves(aurora_core::CurvesParams::default()),
        None,
        0,
    ));
    Doc {
        layers,
        background,
        post2,
        logo,
        vso,
        effect,
        glow,
        post1,
        title,
        post3,
        curves,
    }
}

struct Rig {
    tree: WidgetTree<WidgetKind>,
    panel: PanelHandle,
    scales: Scales,
    view: LayersView,
    rows: HashMap<WidgetId, LayerId>,
}

impl Rig {
    fn new(layers: &LayerTree) -> Self {
        let (mut tree, root) = widgets::new_tree(Style::default());
        let scales = scales();
        let panel = ok(insert_panel(&mut tree, root, "Layers", &scales));
        let mut rig = Self {
            tree,
            panel,
            scales,
            view: LayersView::default(),
            rows: HashMap::new(),
        };
        rig.populate(layers);
        rig
    }

    fn populate(&mut self, layers: &LayerTree) {
        self.rows = ok(populate_layers_panel_view(
            &mut self.tree,
            self.panel,
            &self.scales,
            layers,
            &mut self.view,
        ));
    }

    fn filter(&mut self, layers: &LayerTree, text: &str, kinds: &[KindToggle]) {
        self.view.filter = LayerFilter {
            text: text.to_owned(),
            ..LayerFilter::default()
        };
        for &kind in kinds {
            self.view.filter.set_kind(kind, true);
        }
        self.populate(layers);
    }

    fn row(&self, layer: LayerId) -> Option<WidgetId> {
        self.rows
            .iter()
            .find(|&(_, &id)| id == layer)
            .map(|(&row, _)| row)
    }

    /// The shown layers' names in on-screen order, with their depth.
    fn shown(&self, layers: &LayerTree) -> Vec<(usize, String)> {
        fn walk(
            tree: &WidgetTree<WidgetKind>,
            rows: &HashMap<WidgetId, LayerId>,
            layers: &LayerTree,
            parent: WidgetId,
            depth: usize,
            out: &mut Vec<(usize, String)>,
        ) {
            for &child in tree.children(parent).unwrap_or_default() {
                if let Some(&id) = rows.get(&child) {
                    out.push((depth, layers.name(id).unwrap_or_default().to_owned()));
                    walk(tree, rows, layers, child, depth + 1, out);
                }
            }
        }
        let Some(&container) = self.tree.children(self.panel.body).and_then(|c| c.first()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        walk(&self.tree, &self.rows, layers, container, 0, &mut out);
        out
    }

    fn names(&self, layers: &LayerTree) -> Vec<String> {
        self.shown(layers).into_iter().map(|(_, n)| n).collect()
    }

    fn state(&self, layer: LayerId) -> Option<&widgets::TreeItemState> {
        match self.tree.payload(self.row(layer)?) {
            Some(WidgetKind::TreeItem(state)) => Some(state),
            _ => None,
        }
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|&s| s.to_owned()).collect()
}

#[test]
fn an_empty_or_blank_search_with_no_kind_shows_every_layer() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    let all = rig.shown(&d.layers);
    assert_eq!(all.len(), 10);
    for text in ["", "   ", "\t"] {
        rig.filter(&d.layers, text, &[]);
        assert_eq!(rig.shown(&d.layers), all, "{text:?}");
        assert!(!rig.view.filter.is_active());
    }
}

#[test]
fn matching_ignores_case_and_surrounding_spaces_and_handles_unicode() {
    let mut d = doc();
    let accented = ok(d.layers.add_pixel_layer("Écran Ünïcode", rect(), None));
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "  GLOW ", &[]);
    assert_eq!(rig.names(&d.layers), names(&["Post 2", "Effect", "Glow"]));
    rig.filter(&d.layers, "ÉCRAN", &[]);
    assert_eq!(rig.names(&d.layers), names(&["Écran Ünïcode"]));
    rig.filter(&d.layers, "ünÏ", &[]);
    assert_eq!(rig.names(&d.layers), names(&["Écran Ünïcode"]));
    assert!(rig.row(accented).is_some());
}

#[test]
fn ancestors_of_matches_stay_and_everything_else_is_hidden() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "glow", &[]);
    assert_eq!(
        rig.shown(&d.layers),
        vec![
            (0, "Post 2".to_owned()),
            (1, "Effect".to_owned()),
            (2, "Glow".to_owned())
        ]
    );
    for hidden in [
        d.background,
        d.logo,
        d.vso,
        d.post1,
        d.title,
        d.post3,
        d.curves,
    ] {
        assert!(rig.row(hidden).is_none());
    }
}

#[test]
fn each_kind_toggle_alone_and_combined_with_text() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "", &[KindToggle::Adjustment]);
    assert_eq!(rig.names(&d.layers), names(&["Curves 1"]));
    rig.filter(&d.layers, "", &[KindToggle::Pixel]);
    assert_eq!(
        rig.names(&d.layers),
        names(&[
            "Post 1",
            "Title",
            "Post 2",
            "Effect",
            "Glow",
            "Vector Smart Object",
            "Logo",
            "Background"
        ])
    );
    // Groups only: a matching group shows its matching descendants (the
    // nested group), not its pixel layers; one with none is a leaf.
    rig.filter(&d.layers, "", &[KindToggle::Group]);
    assert_eq!(
        rig.names(&d.layers),
        names(&["Post 3", "Post 1", "Post 2", "Effect"])
    );
    assert!(!rig.state(d.post1).is_some_and(|s| s.has_children));
    assert!(!rig.state(d.post3).is_some_and(|s| s.has_children));
    assert!(rig.state(d.post2).is_some_and(|s| s.has_children));
    // Text and kind together are an AND.
    rig.filter(&d.layers, "post 1", &[KindToggle::Pixel]);
    assert_eq!(rig.names(&d.layers), Vec::<String>::new());
    rig.filter(&d.layers, "t", &[KindToggle::Pixel, KindToggle::Adjustment]);
    assert_eq!(
        rig.names(&d.layers),
        names(&["Post 1", "Title", "Post 2", "Vector Smart Object"])
    );
}

#[test]
fn a_matching_group_without_matching_descendants_is_a_leaf() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "post 1", &[]);
    assert_eq!(rig.names(&d.layers), names(&["Post 1"]));
    let Some(state) = rig.state(d.post1) else {
        unreachable!("shown")
    };
    assert!(!state.has_children, "its Title does not match");
}

#[test]
fn a_match_inside_a_collapsed_group_is_shown_and_the_group_forced_open() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    let Some(row) = rig.row(d.post2) else {
        unreachable!("row")
    };
    ok(widgets::set_tree_item_expanded(&mut rig.tree, row, false));
    rig.populate(&d.layers);
    assert!(rig.view.remembers_collapsed(d.post2));
    assert!(rig.row(d.glow).is_none());
    rig.filter(&d.layers, "glow", &[]);
    assert!(rig.row(d.glow).is_some());
    assert!(rig.state(d.post2).is_some_and(|s| s.expanded));
}

#[test]
fn collapse_memory_comes_back_when_the_filter_clears() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    for layer in [d.post2, d.post1] {
        let Some(row) = rig.row(layer) else {
            unreachable!("row")
        };
        ok(widgets::set_tree_item_expanded(&mut rig.tree, row, false));
    }
    rig.populate(&d.layers);
    // Two filters in a row, one hiding Post 1 entirely.
    rig.filter(&d.layers, "glow", &[]);
    rig.filter(&d.layers, "effect", &[]);
    assert!(
        rig.view.remembers_collapsed(d.post1),
        "hidden, still remembered"
    );
    rig.filter(&d.layers, "", &[]);
    assert_eq!(
        rig.names(&d.layers).len(),
        10 - 5,
        "Post 2 (four rows) and Post 1 (one) collapsed again"
    );
    assert!(rig.state(d.post2).is_some_and(|s| !s.expanded));
    assert!(rig.state(d.post1).is_some_and(|s| !s.expanded));
    assert!(rig.row(d.title).is_none());
}

#[test]
fn nothing_matching_shows_a_calm_line_and_clearing_removes_it() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "zzz", &[]);
    assert!(rig.rows.is_empty());
    let line = |rig: &Rig| {
        rig.tree
            .children(rig.panel.body)
            .unwrap_or_default()
            .iter()
            .any(|&id| {
                matches!(rig.tree.payload(id), Some(WidgetKind::Label(s)) if s.text == NO_MATCH_TEXT)
            })
    };
    assert!(line(&rig));
    assert_eq!(rig.view.counts(), (0, 10));
    rig.filter(&d.layers, "", &[]);
    assert!(!line(&rig));
    assert_eq!(rig.view.counts(), (10, 10));
}

#[test]
fn arrow_navigation_walks_only_the_shown_rows() {
    let d = doc();
    let mut rig = Rig::new(&d.layers);
    rig.filter(&d.layers, "o", &[KindToggle::Pixel]);
    let order = rig.names(&d.layers);
    let Some(mut at) = rig.row(d.post2) else {
        unreachable!("row")
    };
    let mut walked = vec!["Post 2".to_owned()];
    while let Some(next) = layer_row_neighbor(&rig.tree, at, true) {
        let Some(&id) = rig.rows.get(&next) else {
            unreachable!("a shown row")
        };
        walked.push(d.layers.name(id).unwrap_or_default().to_owned());
        at = next;
    }
    assert_eq!(
        walked,
        order
            .get(order.iter().position(|n| n == "Post 2").unwrap_or(0)..)
            .unwrap_or_default()
            .to_vec()
    );
    assert!(!walked.contains(&"Title".to_owned()));
}

#[test]
fn the_filter_is_not_applied_to_the_unfiltered_entry_point() {
    // `populate_layers_panel` keeps its old contract: no filter, collapse
    // read off the rows.
    let d = doc();
    let (mut tree, root) = widgets::new_tree(Style::default());
    let scales = scales();
    let panel = ok(insert_panel(&mut tree, root, "Layers", &scales));
    let rows = ok(crate::populate_layers_panel(
        &mut tree, panel, &scales, &d.layers,
    ));
    assert_eq!(rows.len(), 10);
}

#[test]
fn shown_layers_is_none_without_a_filter_and_a_set_with_one() {
    let d = doc();
    assert!(shown_layers(&d.layers, &LayerFilter::default()).is_none());
    let filter = LayerFilter {
        text: "title".to_owned(),
        ..LayerFilter::default()
    };
    let Some(shown) = shown_layers(&d.layers, &filter) else {
        unreachable!("active")
    };
    assert_eq!(shown.len(), 2);
    assert!(shown.contains(&d.title) && shown.contains(&d.post1));
    assert!(filter.matches(&d.layers, d.title) && !filter.matches(&d.layers, d.post1));
}

#[test]
fn the_filter_row_is_the_strips_first_row_with_named_accessible_parts() {
    let (mut tree, root) = widgets::new_tree(Style::default());
    let scales = scales();
    let panel = ok(insert_panel(&mut tree, root, "Layers", &scales));
    let controls = ok(insert_layer_controls(&mut tree, panel, &scales));
    let filter = controls.filter;
    assert_eq!(
        tree.children(controls.root).and_then(|c| c.first()),
        Some(&filter.row)
    );
    // Tab order: the search glyph (decorative), the field, then the three
    // toggles, all before the blend dropdown.
    let order: Vec<WidgetId> = tree.children(filter.row).unwrap_or_default().to_vec();
    assert_eq!(
        order,
        vec![
            filter.icon,
            filter.field,
            filter.pixel,
            filter.adjustment,
            filter.group
        ]
    );
    let Some(field) = tree.accessibility(filter.field) else {
        unreachable!("node")
    };
    assert_eq!(field.role(), accesskit::Role::TextInput);
    assert_eq!(field.label(), Some("Search layers"));
    assert_eq!(
        field.placeholder(),
        Some("Search"),
        "shorter than the label"
    );
    for (toggle, want) in KindToggle::ALL.into_iter().zip([
        "Show pixel layers",
        "Show adjustment layers",
        "Show groups",
    ]) {
        let Some(node) = tree.accessibility(filter.button(toggle)) else {
            unreachable!("node")
        };
        assert_eq!(node.label(), Some(want));
        assert_eq!(node.role(), accesskit::Role::Button);
        assert_eq!(node.toggled(), Some(accesskit::Toggled::False));
        assert!(node.supports_action(accesskit::Action::Focus));
    }
    // The row is never disabled with no active layer.
    ok(crate::sync_layer_controls(
        &mut tree,
        controls,
        &LayerTree::new(),
        None,
        None,
    ));
    assert!(matches!(
        tree.payload(filter.field),
        Some(WidgetKind::TextField(s)) if !s.disabled
    ));
    assert!(matches!(
        tree.payload(filter.pixel),
        Some(WidgetKind::Button(s)) if !s.disabled
    ));
}

#[test]
fn write_read_clear_and_the_summary_description_round_trip() {
    let (mut tree, root) = widgets::new_tree(Style::default());
    let scales = scales();
    let panel = ok(insert_panel(&mut tree, root, "Layers", &scales));
    let row = ok(insert_layer_controls(&mut tree, panel, &scales)).filter;
    let want = LayerFilter {
        text: "post".to_owned(),
        pixel: true,
        adjustment: false,
        group: true,
    };
    assert!(ok(crate::write_layer_filter(&mut tree, &row, &want)));
    assert!(
        !ok(crate::write_layer_filter(&mut tree, &row, &want)),
        "idempotent"
    );
    assert_eq!(crate::read_layer_filter(&tree, &row), want);
    let Some(node) = tree.accessibility(row.pixel) else {
        unreachable!("node")
    };
    assert_eq!(node.toggled(), Some(accesskit::Toggled::True));
    assert!(ok(crate::set_layer_filter_summary(
        &mut tree, &row, 12, 277, true
    )));
    let description = |tree: &WidgetTree<WidgetKind>| {
        tree.accessibility(row.field)
            .and_then(|n| n.description().map(str::to_owned))
    };
    assert_eq!(
        description(&tree).as_deref(),
        Some("12 of 277 layers shown")
    );
    assert!(ok(crate::set_layer_filter_summary(
        &mut tree, &row, 277, 277, false
    )));
    assert_eq!(description(&tree), None);
    assert!(ok(crate::clear_layer_filter_text(&mut tree, &row)));
    assert!(!ok(crate::clear_layer_filter_text(&mut tree, &row)));
    assert_eq!(crate::read_layer_filter(&tree, &row).text, "");
}

#[test]
fn the_strip_floor_counts_the_filter_row_exactly() {
    let (mut tree, root) = widgets::new_tree(Style::default());
    let scales = scales();
    let panel = ok(insert_panel(&mut tree, root, "Layers", &scales));
    let controls = ok(insert_layer_controls(&mut tree, panel, &scales));
    let Some(style) = tree.style(controls.root) else {
        unreachable!("the strip has a style")
    };
    // Three slider-height rows (blend, Opacity, Fill), the Lock row and the
    // filter row (each as tall as its icon buttons), four gaps of
    // `spacing.xs` and `spacing.sm` padding above and below.
    let row = widgets::row_height(&scales);
    let button = widgets::icon_button_side(&scales, widgets::IconSize::Small).max(row);
    #[allow(clippy::cast_precision_loss)]
    let (gap, pad) = (scales.spacing.xs as f32, scales.spacing.sm as f32);
    let want = 3.0 * row + 2.0 * button + 4.0 * gap + 2.0 * pad;
    assert_eq!(style.min_size.height, taffy::style_helpers::length(want));
    assert!(
        (want - 167.0).abs() < f32::EPSILON,
        "{want} at the default tokens"
    );
}
