//! Task 173: the dark gutters between the docked regions, the flat menu bar
//! and the selected tabs that take their panel's colour.

use aurora_theme::{Scales, Theme};
use aurora_widgets::widgets::{StripSurface, WidgetKind};
use aurora_widgets::{WidgetId, paint_widget};

use crate::{Workspace, build_workspace, close_panel};

/// `spacing.xxs`, the gutter.
const GUTTER: i64 = 4;

fn scales() -> Scales {
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn theme(name: &str) -> Theme {
    const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
    let Ok(palette) = aurora_theme::Palette::from_toml_str(PALETTE_TOML) else {
        unreachable!("the committed palette parses");
    };
    let mut themes = aurora_theme::ThemeSet::new();
    for toml in [
        include_str!("../../../design/themes/dark.toml"),
        include_str!("../../../design/themes/light.toml"),
        include_str!("../../../design/themes/high-contrast-dark.toml"),
        include_str!("../../../design/themes/high-contrast-light.toml"),
        include_str!("../../../design/themes/color-critical.toml"),
    ] {
        if themes.register(toml).is_err() {
            unreachable!("the committed themes register");
        }
    }
    match themes.resolve(name, &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("{err:?}"),
    }
}

const THEMES: [&str; 5] = [
    "Dark",
    "Light",
    "High Contrast Dark",
    "High Contrast Light",
    "Color-Critical",
];

fn laid_out(width: f32, height: f32) -> Workspace {
    let mut ws = build_workspace(&scales());
    ws.tree.compute_layout(width, height);
    ws
}

fn bounds(ws: &Workspace, id: WidgetId) -> aurora_core::Rect {
    match ws.tree.bounds(id) {
        Some(bounds) => bounds,
        None => unreachable!("laid out"),
    }
}

fn colour(c: aurora_theme::Color) -> [f32; 4] {
    let [r, g, b] = c.to_srgb_f32();
    [r, g, b, 1.0]
}

fn paints(ws: &Workspace, id: WidgetId, theme: &Theme) -> Vec<[f32; 4]> {
    match paint_widget(&ws.tree, id, theme, &scales(), 1.0) {
        Ok(paints) => paints.into_iter().map(|(_, rgba)| rgba).collect(),
        Err(err) => unreachable!("{err:?}"),
    }
}

#[test]
fn there_is_a_gutter_between_shown_regions_and_none_at_the_window_edges() {
    let ws = laid_out(1000.0, 800.0);
    let (tools, column, rail) = (
        bounds(&ws, ws.tools.root),
        bounds(&ws, ws.canvas_column),
        bounds(&ws, ws.rail),
    );
    let group = bounds(&ws, crate::workspace::test_group(&ws).root);
    let layers = bounds(&ws, ws.layers.root);
    assert_eq!(column.x - tools.right(), GUTTER, "tools to canvas column");
    assert_eq!(rail.x - column.right(), GUTTER, "canvas column to rail");
    assert_eq!(
        group.y - layers.bottom(),
        GUTTER,
        "between the rail's slots"
    );
    assert_eq!(tools.x, 0, "no gutter at the left window edge");
    assert_eq!(rail.right(), 1000, "no gutter at the right window edge");
    assert_eq!(layers.y, rail.y, "none above the first slot");
}

#[test]
fn a_collapsed_rail_keeps_the_gutter_to_its_strip() {
    let mut ws = build_workspace(&scales());
    if let Err(err) = crate::set_rail_collapsed(&mut ws, true) {
        unreachable!("{err:?}");
    }
    ws.tree.compute_layout(1000.0, 800.0);
    let (column, strip) = (
        bounds(&ws, ws.canvas_column),
        bounds(&ws, ws.panel_strip.root),
    );
    assert_eq!(strip.x - column.right(), GUTTER);
    assert_eq!(strip.right(), 1000);
}

#[test]
fn a_closed_panel_and_a_hidden_slot_take_no_gutter() {
    let mut ws = build_workspace(&scales());
    ws.tree.compute_layout(1000.0, 800.0);
    let rail = bounds(&ws, ws.rail);
    // Close Layers: the group is the only shown slot and sits at the top.
    if let Err(err) = close_panel(&mut ws.tree, ws.layers) {
        unreachable!("{err:?}");
    }
    ws.tree.compute_layout(1000.0, 800.0);
    let group = bounds(&ws, crate::workspace::test_group(&ws).root);
    assert_eq!(group.y, rail.y, "no gutter where a closed panel was");
    assert_eq!(
        ws.tree.style(ws.layers.root).map(|s| s.display),
        Some(taffy::Display::None),
        "a closed panel's root is Display::None, so the gap skips it"
    );
    // Close the group's members too: the group is hidden and nothing is left.
    let members = [ws.properties, ws.history, ws.channels, ws.paths];
    for member in members {
        if let Err(err) = close_panel(&mut ws.tree, member) {
            unreachable!("{err:?}");
        }
    }
    let group = crate::workspace::test_group(&ws);
    if let Err(err) = crate::sync_panel_group(&mut ws.tree, &group) {
        unreachable!("{err:?}");
    }
    ws.tree.compute_layout(1000.0, 800.0);
    assert_eq!(
        ws.tree
            .style(crate::workspace::test_group(&ws).root)
            .map(|s| s.display),
        Some(taffy::Display::None)
    );
    // Reopening Layers restores it exactly, gutter below it included.
    if let Err(err) = crate::set_panel_collapsed(&mut ws.tree, ws.layers, false) {
        unreachable!("{err:?}");
    }
    ws.tree.compute_layout(1000.0, 800.0);
    assert_eq!(bounds(&ws, ws.layers.root).y, rail.y);
}

#[test]
fn the_menu_bar_is_a_flat_chrome_strip_with_its_role_and_label_unchanged() {
    let mut ws = build_workspace(&scales());
    if let Err(err) = crate::menu_bar::set_menu_bar_shown(&mut ws.tree, &ws.menu_bar, true) {
        unreachable!("{err:?}");
    }
    ws.tree.compute_layout(1000.0, 800.0);
    let Some(node) = ws.tree.accessibility(ws.menu_bar.root) else {
        unreachable!("built");
    };
    assert_eq!(node.role(), accesskit::Role::MenuBar);
    assert_eq!(node.label(), Some(crate::MENU_BAR_LABEL));
    assert!(matches!(
        ws.tree.payload(ws.menu_bar.root),
        Some(WidgetKind::Strip(StripSurface::Chrome))
    ));
    for name in THEMES {
        let theme = theme(name);
        // One flat, square fill in surface.chrome: no border, no radius.
        assert_eq!(
            paints(&ws, ws.menu_bar.root, &theme),
            vec![colour(theme.surface.chrome)],
            "{name}"
        );
    }
}

#[test]
fn a_selected_panel_group_tab_paints_the_panel_colour_and_an_unselected_one_does_not() {
    let ws = laid_out(1000.0, 800.0);
    let Ok(bar) =
        aurora_widgets::widgets::tab_bar_state(&ws.tree, crate::workspace::test_group(&ws).bar)
    else {
        unreachable!("the group has a bar");
    };
    let (Some(&selected), Some(&other)) = (
        bar.tabs().get(bar.selected()),
        bar.tabs().get(bar.selected() + 1),
    ) else {
        unreachable!("the group has at least two tabs");
    };
    let dark = theme("Dark");
    let sel = paints(&ws, selected, &dark);
    // Task 174: in Dark the panel-coloured fill alone marks the tab, so it
    // joins the body below with no accent underline.
    assert_eq!(
        sel,
        vec![colour(dark.surface.panel)],
        "fill only, no underline"
    );
    assert!(paints(&ws, other, &dark).is_empty(), "unselected: nothing");
    // The row itself stays the header colour.
    assert_eq!(
        paints(&ws, crate::workspace::test_group(&ws).bar, &dark).first(),
        Some(&colour(dark.surface.header))
    );
    // Where header equals panel the fill is invisible against the row, so
    // the accent underline stays as the only mark of the selected tab.
    for name in [
        "Light",
        "High Contrast Dark",
        "High Contrast Light",
        "Color-Critical",
    ] {
        let theme = theme(name);
        assert_eq!(theme.surface.header, theme.surface.panel, "{name}");
        assert_eq!(
            paints(&ws, selected, &theme).last(),
            Some(&colour(theme.accent.primary)),
            "{name}: underline kept"
        );
    }
}

#[test]
fn a_lone_panels_title_sits_on_a_panel_coloured_tab_in_a_header_coloured_strip() {
    let ws = laid_out(1000.0, 800.0);
    let dark = theme("Dark");
    let Some(&tab) = ws.tree.children(ws.layers.header).and_then(|c| c.first()) else {
        unreachable!("the title slot holds the tab");
    };
    assert_eq!(
        paints(&ws, ws.layers.header, &dark),
        vec![colour(dark.surface.header)]
    );
    assert_eq!(paints(&ws, tab, &dark), vec![colour(dark.surface.panel)]);
    assert!(matches!(
        ws.tree.payload(tab),
        Some(WidgetKind::Strip(StripSurface::Panel))
    ));
    // Hidden from assistive technology: the Region's label is the one title.
    assert!(
        ws.tree
            .accessibility(tab)
            .is_some_and(accesskit::Node::is_hidden)
    );
    // A grouped panel has no title tab.
    let Some(&grouped) = ws
        .tree
        .children(ws.properties.header)
        .and_then(|c| c.first())
    else {
        unreachable!("every panel has the tab");
    };
    assert_eq!(
        ws.tree.style(grouped).map(|s| s.display),
        Some(taffy::Display::None)
    );
}

#[test]
fn the_document_strip_is_header_coloured_with_a_panel_coloured_selected_tab() {
    let ws = laid_out(1000.0, 800.0);
    let Ok(bar) = aurora_widgets::widgets::tab_bar_state(&ws.tree, ws.document_tabs) else {
        unreachable!("the strip is a tab bar");
    };
    let Some(&tab) = bar.tabs().get(bar.selected()) else {
        unreachable!("one tab is selected");
    };
    let dark = theme("Dark");
    let strip = paints(&ws, ws.document_tabs, &dark);
    assert_eq!(strip.first(), Some(&colour(dark.surface.header)));
    // Task 174: in Dark the panel-coloured fill alone marks the tab.
    assert_eq!(
        paints(&ws, tab, &dark),
        vec![colour(dark.surface.panel)],
        "fill only, no underline"
    );
    // Where the strip's header equals the panel the fill is invisible, so
    // the accent underline is still the selected tab's mark.
    for name in [
        "Light",
        "High Contrast Dark",
        "High Contrast Light",
        "Color-Critical",
    ] {
        let theme = theme(name);
        assert_eq!(
            paints(&ws, tab, &theme).last(),
            Some(&colour(theme.accent.primary)),
            "{name}: underline kept"
        );
    }
}

#[test]
fn the_other_themes_keep_the_equalities_that_leave_them_unchanged() {
    for name in [
        "Light",
        "High Contrast Dark",
        "High Contrast Light",
        "Color-Critical",
    ] {
        let theme = theme(name);
        assert_eq!(
            theme.surface.chrome, theme.surface.app,
            "{name}: chrome = app"
        );
        assert_eq!(
            theme.surface.header, theme.surface.panel,
            "{name}: header = panel"
        );
    }
}
