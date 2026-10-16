//! Tests for the document strip's overflow (0.260.0): the pure window
//! arithmetic, and the strip laid out at several widths and counts.
#![allow(clippy::float_cmp, clippy::indexing_slicing)]

use super::{
    DOCUMENT_OVERFLOW_LABEL, document_tab_capacity, document_tab_close_at, document_tab_max_width,
    document_tab_min_width, document_tab_window, is_document_overflow_part,
    open_document_overflow_menu, sync_document_tab_window, sync_document_tabs,
};
use crate::workspace::{Workspace, build_workspace};
use aurora_theme::Scales;
use aurora_widgets::WidgetId;
use aurora_widgets::widgets::{self, MenuItem, WidgetKind};

fn scales() -> Scales {
    const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn names(count: usize) -> Vec<String> {
    (0..count).map(|n| format!("Document {n}.psd")).collect()
}

/// Lays `ws` out at `width` x 800 logical px, then lets the strip choose
/// its window and lays out again until it settles (the app's loop).
fn lay_out(ws: &mut Workspace, width: f32) -> usize {
    let scales = scales();
    ws.tree.compute_layout(width, 800.0);
    let mut passes = 1;
    for _ in 0..4 {
        match sync_document_tab_window(ws, &scales) {
            Ok(true) => {
                ws.tree.compute_layout(width, 800.0);
                passes += 1;
            }
            Ok(false) => break,
            Err(err) => unreachable!("{err:?}"),
        }
    }
    passes
}

fn show(ws: &mut Workspace, count: usize, selected: usize) {
    if let Err(err) = sync_document_tabs(
        &mut ws.tree,
        ws.document_tabs,
        ws.canvas_area,
        names(count),
        selected,
    ) {
        unreachable!("{err:?}");
    }
}

fn tabs(ws: &Workspace) -> Vec<WidgetId> {
    match widgets::tab_bar_state(&ws.tree, ws.document_tabs) {
        Ok(state) => state.tabs().to_vec(),
        Err(err) => unreachable!("{err:?}"),
    }
}

fn rect(ws: &Workspace, id: WidgetId) -> (i64, i64, i64, i64) {
    let Some(b) = ws.tree.bounds(id) else {
        unreachable!("laid out")
    };
    (b.x, b.y, i64::from(b.width), i64::from(b.height))
}

fn shown_indices(ws: &Workspace) -> Vec<usize> {
    let Ok(state) = widgets::tab_bar_state(&ws.tree, ws.document_tabs) else {
        unreachable!("a tab bar")
    };
    (0..state.labels().len())
        .filter(|&index| state.is_tab_shown(index))
        .collect()
}

fn overflow_shown(ws: &Workspace) -> bool {
    ws.tree
        .style(ws.document_overflow)
        .is_some_and(|style| style.display != taffy::Display::None)
}

#[test]
fn the_tab_floor_and_ceiling_come_from_the_spacing_scale() {
    let scales = scales();
    let (min, max) = (
        document_tab_min_width(&scales),
        document_tab_max_width(&scales),
    );
    assert_eq!(min, 112.0);
    assert_eq!(max, 192.0);
    #[allow(clippy::cast_precision_loss)]
    let (pad, md) = (scales.spacing.sm as f32, scales.spacing.md as f32);
    let close = widgets::tab_close_width(&scales);
    // The floor leaves room for both paddings, the close button and more
    // than a bare ellipsis of label.
    assert!(min > 2.0 * pad + close + 3.0 * md);
    assert!(max > min);
}

#[test]
fn capacity_counts_whole_tabs_with_their_gaps_and_is_at_least_one() {
    assert_eq!(document_tab_capacity(952.0, 112.0, 8.0), 8);
    assert_eq!(document_tab_capacity(120.0, 112.0, 8.0), 1);
    assert_eq!(document_tab_capacity(231.0, 112.0, 8.0), 1);
    assert_eq!(document_tab_capacity(232.0, 112.0, 8.0), 2);
    assert_eq!(document_tab_capacity(351.0, 112.0, 8.0), 2);
    assert_eq!(document_tab_capacity(352.0, 112.0, 8.0), 3);
    assert_eq!(document_tab_capacity(0.0, 112.0, 8.0), 1);
    assert_eq!(document_tab_capacity(-50.0, 112.0, 8.0), 1);
    assert_eq!(document_tab_capacity(f32::NAN, 112.0, 8.0), 1);
    assert_eq!(document_tab_capacity(500.0, 0.0, 8.0), 1);
    assert_eq!(document_tab_capacity(500.0, f32::INFINITY, 8.0), 1);
}

#[test]
fn the_window_always_holds_the_selected_tab_and_moves_by_the_least() {
    for count in 0..=14_usize {
        for capacity in 0..=16_usize {
            for selected in 0..count.max(1) {
                let starts: Vec<Option<(usize, usize)>> = std::iter::once(None)
                    .chain(
                        (0..count.max(1))
                            .flat_map(|s| (s + 1..=count.max(1)).map(move |e| Some((s, e)))),
                    )
                    .collect();
                for previous in starts {
                    let (start, end) = document_tab_window(count, selected, capacity, previous);
                    let cap = capacity.max(1);
                    if cap >= count {
                        assert_eq!((start, end), (0, count));
                        continue;
                    }
                    assert_eq!(
                        end - start,
                        cap,
                        "{count} {capacity} {selected} {previous:?}"
                    );
                    assert!(end <= count);
                    assert!(
                        (start..end).contains(&selected),
                        "{count} {capacity} {selected} {previous:?} -> {start}..{end}"
                    );
                    // A previous start that still works is kept.
                    if let Some((old, _)) = previous
                        && old + cap <= count
                        && (old..old + cap).contains(&selected)
                    {
                        assert_eq!(start, old, "kept: {count} {capacity} {selected}");
                    }
                    // Otherwise the selected tab sits at an edge of the window.
                    else if let Some((old, _)) = previous {
                        let clamped = old.min(count - cap);
                        if !(clamped..clamped + cap).contains(&selected) {
                            assert!(selected == start || selected + 1 == end);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn stepping_through_every_document_moves_the_window_one_tab_at_a_time() {
    let (count, cap) = (30, 7);
    let mut window = None;
    let mut last_start = 0;
    for selected in 0..count {
        let (start, end) = document_tab_window(count, selected, cap, window);
        assert!(start >= last_start && start - last_start <= 1, "{selected}");
        last_start = start;
        window = Some((start, end));
    }
    assert_eq!(window, Some((count - cap, count)));
    // And back (Ctrl+Shift+Tab wrapping from the first to the last is a
    // jump, but the window still contains it).
    for selected in (0..count).rev() {
        let (start, end) = document_tab_window(count, selected, cap, window);
        assert!((start..end).contains(&selected));
        window = Some((start, end));
    }
    assert_eq!(window, Some((0, cap)));
}

#[test]
fn every_count_and_width_keeps_visible_tabs_readable_inside_the_strip_with_the_active_one() {
    let scales = scales();
    let min = document_tab_min_width(&scales);
    let max = document_tab_max_width(&scales);
    for width in [1274.0_f32, 800.0, 640.0, 637.0] {
        for count in [1_usize, 2, 3, 12, 30, 100] {
            let mut ws = build_workspace(&scales);
            show(&mut ws, count, 0);
            lay_out(&mut ws, width);
            for selected in 0..count {
                show(&mut ws, count, selected);
                lay_out(&mut ws, width);
                let case = format!("w={width} n={count} sel={selected}");
                let (bx, _, bw, _) = rect(&ws, ws.document_tabs);
                assert!(bx + bw <= width as i64 + 1, "{case}: strip past window");
                let shown = shown_indices(&ws);
                assert!(shown.contains(&selected), "{case}: active hidden");
                let all_tabs = tabs(&ws);
                for (index, &tab) in all_tabs.iter().enumerate() {
                    let (x, _, w, _) = rect(&ws, tab);
                    let node_hidden = ws
                        .tree
                        .accessibility(tab)
                        .is_some_and(accesskit::Node::is_hidden);
                    if shown.contains(&index) {
                        assert!(!node_hidden, "{case}: shown tab {index} is AT-hidden");
                        assert!(w as f32 >= min - 1.0, "{case}: tab {index} is {w} wide");
                        assert!(w as f32 <= max + 1.0, "{case}: tab {index} is {w} wide");
                        assert!(x >= bx, "{case}: tab {index} starts left of the strip");
                        assert!(
                            x + w <= bx + bw + 1,
                            "{case}: tab {index} ends past the strip"
                        );
                    } else {
                        assert!(node_hidden, "{case}: hidden tab {index} is announced");
                        assert_eq!(w, 0, "{case}: hidden tab {index} has width");
                    }
                }
                let fits = count as f32 * min + (count - 1) as f32 * 8.0 <= bw as f32;
                assert_eq!(overflow_shown(&ws), !fits, "{case}");
                assert_eq!(shown.len() == count, fits, "{case}");
                if !fits {
                    let (ox, _, ow, _) = rect(&ws, ws.document_overflow);
                    assert!(ow > 0, "{case}");
                    assert!(ox >= bx && ox + ow <= bx + bw + 1, "{case}: button outside");
                    // The button sits after the last shown tab.
                    if let Some(&last) = shown.last().and_then(|i| all_tabs.get(*i)) {
                        let (lx, _, lw, _) = rect(&ws, last);
                        assert!(ox >= lx + lw, "{case}: button overlaps a tab");
                    }
                    let Some(node) = ws.tree.accessibility(ws.document_overflow) else {
                        unreachable!("built");
                    };
                    assert_eq!(node.label(), Some(DOCUMENT_OVERFLOW_LABEL));
                    assert!(!node.is_hidden(), "{case}");
                    let expected = format!("{} more", count - shown.len());
                    assert_eq!(node.description(), Some(expected.as_str()), "{case}");
                } else {
                    let hidden = ws
                        .tree
                        .accessibility(ws.document_overflow)
                        .is_some_and(accesskit::Node::is_hidden);
                    assert!(hidden, "{case}: the button is announced while hidden");
                }
            }
        }
    }
}

#[test]
fn a_switch_sequence_a_close_and_a_new_document_keep_the_active_tab_visible() {
    let scales = scales();
    for width in [1274.0_f32, 640.0] {
        let mut ws = build_workspace(&scales);
        let mut count = 30;
        show(&mut ws, count, 0);
        lay_out(&mut ws, width);
        let mut active = 0;
        // Ctrl+Tab through all, wrapping.
        for _ in 0..=count {
            active = (active + 1) % count;
            show(&mut ws, count, active);
            lay_out(&mut ws, width);
            assert!(shown_indices(&ws).contains(&active), "w={width} a={active}");
        }
        // Ctrl+Shift+Tab from 0 wraps to the last.
        show(&mut ws, count, 0);
        lay_out(&mut ws, width);
        show(&mut ws, count, count - 1);
        lay_out(&mut ws, width);
        assert!(shown_indices(&ws).contains(&(count - 1)));
        // Close the active (last) one: the new active is the one before.
        count -= 1;
        active = count - 1;
        show(&mut ws, count, active);
        lay_out(&mut ws, width);
        assert!(shown_indices(&ws).contains(&active));
        // Close one in the middle while the window is at the end.
        count -= 1;
        show(&mut ws, count, active.min(count - 1));
        lay_out(&mut ws, width);
        assert!(shown_indices(&ws).contains(&active.min(count - 1)));
        // A new document is appended and active.
        count += 1;
        active = count - 1;
        show(&mut ws, count, active);
        lay_out(&mut ws, width);
        assert!(shown_indices(&ws).contains(&active));
        // Shrinking to fewer than fit hides the button and shows all.
        for keep in [2_usize, 1] {
            show(&mut ws, keep, 0);
            lay_out(&mut ws, width);
            let (_, _, strip, _) = rect(&ws, ws.document_tabs);
            #[allow(clippy::cast_precision_loss)]
            let fits = keep as f32 * 112.0 + (keep - 1) as f32 * 8.0 <= strip as f32;
            assert_eq!(shown_indices(&ws).len() == keep, fits);
            assert_eq!(overflow_shown(&ws), !fits);
            assert!(shown_indices(&ws).contains(&0));
        }
    }
}

#[test]
fn picking_a_visible_tab_does_not_move_the_window() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 12);
    lay_out(&mut ws, 800.0);
    let before = shown_indices(&ws);
    let Some(&second) = before.get(1) else {
        unreachable!("several tabs are shown")
    };
    show(&mut ws, 30, second);
    lay_out(&mut ws, 800.0);
    assert_eq!(shown_indices(&ws), before);
}

#[test]
fn a_resize_re_evaluates_the_window() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 20);
    lay_out(&mut ws, 640.0);
    let narrow = shown_indices(&ws);
    lay_out(&mut ws, 1274.0);
    let wide = shown_indices(&ws);
    assert!(wide.len() > narrow.len());
    assert!(wide.contains(&20));
    lay_out(&mut ws, 640.0);
    assert!(shown_indices(&ws).contains(&20));
    // Wide enough for all of them: no window, no button.
    show(&mut ws, 3, 2);
    lay_out(&mut ws, 1274.0);
    assert_eq!(shown_indices(&ws), vec![0, 1, 2]);
    assert!(!overflow_shown(&ws));
}

#[test]
fn a_strip_that_was_never_laid_out_or_has_no_width_is_left_alone() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 5);
    // No layout: width 0.
    assert!(matches!(
        sync_document_tab_window(&mut ws, &scales),
        Ok(false)
    ));
    assert_eq!(shown_indices(&ws).len(), 30);
    assert!(!overflow_shown(&ws));
    // A window of no width at all must not panic either.
    ws.tree.compute_layout(0.0, 0.0);
    let _ = sync_document_tab_window(&mut ws, &scales);
    assert!(shown_indices(&ws).contains(&5));
    for count in [0_usize, 1, 2, 100] {
        let mut ws = build_workspace(&scales);
        if count > 0 {
            show(&mut ws, count, count - 1);
        }
        for width in [0.0, 1.0, 50.0] {
            ws.tree.compute_layout(width, 100.0);
            let _ = sync_document_tab_window(&mut ws, &scales);
        }
    }
}

#[test]
fn layout_settles_in_two_passes_and_the_window_is_stable_afterwards() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 100, 50);
    let passes = lay_out(&mut ws, 800.0);
    assert!(passes <= 3, "{passes}");
    let settled = shown_indices(&ws);
    assert!(matches!(
        sync_document_tab_window(&mut ws, &scales),
        Ok(false)
    ));
    lay_out(&mut ws, 800.0);
    assert_eq!(shown_indices(&ws), settled);
}

#[test]
fn the_overflow_menu_lists_every_document_inside_the_strip() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 3);
    lay_out(&mut ws, 800.0);
    assert!(overflow_shown(&ws));
    let items: Vec<MenuItem> = names(30).iter().map(|n| MenuItem::action(n)).collect();
    let menu = match open_document_overflow_menu(&mut ws, &scales, 600.0, items) {
        Ok(menu) => menu,
        Err(err) => unreachable!("{err:?}"),
    };
    ws.tree.compute_layout(800.0, 800.0);
    let Ok(state) = widgets::menu_state(&ws.tree, menu) else {
        unreachable!("a menu")
    };
    assert_eq!(state.items().len(), 30);
    assert_eq!(ws.tree.parent(menu), Some(ws.document_tabs));
    let (bx, _, bw, _) = rect(&ws, ws.document_tabs);
    let (mx, _, mw, _) = rect(&ws, menu);
    assert!(mw <= bw, "the menu is no wider than the strip");
    assert!(mx >= bx && mx + mw <= bx + bw + 1, "the menu stays inside");
    // Parts: the button and the popup (and its rows), not tabs or their
    // close buttons.
    assert!(is_document_overflow_part(&ws.tree, ws.document_overflow));
    assert!(is_document_overflow_part(&ws.tree, menu));
    for &row in state.item_ids() {
        assert!(is_document_overflow_part(&ws.tree, row));
    }
    for &tab in &tabs(&ws) {
        assert!(!is_document_overflow_part(&ws.tree, tab));
    }
    for index in 0..3 {
        if let Some(close) = widgets::tab_close_button(&ws.tree, ws.document_tabs, index) {
            assert!(!is_document_overflow_part(&ws.tree, close));
        }
    }
    assert!(!is_document_overflow_part(&ws.tree, ws.canvas_area));
    // The tab labels survive: the tabs are not touched by the popup.
    assert_eq!(tabs(&ws).len(), 30);
}

#[test]
fn the_close_button_of_a_visible_tab_names_the_document_not_the_window_slot() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 20);
    lay_out(&mut ws, 800.0);
    let shown = shown_indices(&ws);
    for &index in &shown {
        let Some(close) = widgets::tab_close_button(&ws.tree, ws.document_tabs, index) else {
            unreachable!("closable")
        };
        let (x, y, w, h) = rect(&ws, close);
        assert!(w > 0 && h > 0, "tab {index}");
        #[allow(clippy::cast_precision_loss)]
        let point = ((x + w / 2) as f32, (y + h / 2) as f32);
        assert_eq!(
            document_tab_close_at(&ws.tree, ws.document_tabs, point),
            Some(index)
        );
    }
    // A hidden tab cannot be hit at all.
    for index in (0..30).filter(|i| !shown.contains(i)) {
        if let Some(close) = widgets::tab_close_button(&ws.tree, ws.document_tabs, index) {
            let (_, _, w, h) = rect(&ws, close);
            assert!(w == 0 || h == 0, "hidden tab {index} keeps a hit area");
        }
    }
}

#[test]
fn accessible_names_of_the_shown_tabs_and_the_button_are_unique() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 30, 9);
    lay_out(&mut ws, 800.0);
    let mut seen = std::collections::HashSet::new();
    let strip = ws.document_tabs;
    let Some(children) = ws.tree.children(strip) else {
        unreachable!("strip")
    };
    for &child in children {
        let Some(node) = ws.tree.accessibility(child) else {
            continue;
        };
        if node.is_hidden() {
            continue;
        }
        let name = node.label().unwrap_or_default().to_owned();
        assert!(seen.insert(name.clone()), "duplicate AT name {name}");
        if matches!(ws.tree.payload(child), Some(WidgetKind::Tab(_))) {
            assert_eq!(node.role(), accesskit::Role::Tab);
        }
    }
    assert!(seen.contains(DOCUMENT_OVERFLOW_LABEL));
}

#[test]
fn the_overflow_button_stays_behind_every_tab_after_tabs_are_added() {
    let scales = scales();
    let mut ws = build_workspace(&scales);
    show(&mut ws, 4, 0);
    lay_out(&mut ws, 640.0);
    show(&mut ws, 40, 0);
    lay_out(&mut ws, 640.0);
    let Some(children) = ws.tree.children(ws.document_tabs) else {
        unreachable!("strip")
    };
    let button = children.iter().position(|&c| c == ws.document_overflow);
    let last_tab = children
        .iter()
        .rposition(|&c| matches!(ws.tree.payload(c), Some(WidgetKind::Tab(_))));
    assert!(button > last_tab, "{button:?} {last_tab:?}");
}
