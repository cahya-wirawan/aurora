//! Tests for the grouped PSD save report (task 157): grouping, order,
//! dedupe, caps, counts, the ask rule and the two messages.

#![allow(clippy::indexing_slicing)] // fixtures with known group counts

use aurora_io::{PsdWriteReport, ReportCategory, ReportEntry};

use crate::psd_report::{FROM_OPEN_HEADING, SaveReport, check_message, save_message};

fn entry(category: ReportCategory, text: &str) -> ReportEntry {
    ReportEntry::new(category, text)
}

fn report(entries: Vec<ReportEntry>) -> PsdWriteReport {
    PsdWriteReport::from_entries(entries)
}

#[test]
fn groups_come_in_the_documented_order_whatever_the_arrival_order() {
    let mut entries = vec![
        entry(ReportCategory::Kept, "k"),
        entry(ReportCategory::FromOpen, "o"),
        entry(ReportCategory::Changed, "c"),
        entry(ReportCategory::NotSaved, "n"),
        entry(ReportCategory::NeedsAttention, "a"),
    ];
    let forward = SaveReport::new(&report(entries.clone()));
    entries.reverse();
    let backward = SaveReport::new(&report(entries));
    assert_eq!(forward, backward);
    let order: Vec<ReportCategory> = forward.groups.iter().map(|g| g.category).collect();
    assert_eq!(order, ReportCategory::ALL);
    // The documented order, spelled out: what needs a decision comes first.
    assert_eq!(
        order,
        [
            ReportCategory::NeedsAttention,
            ReportCategory::NotSaved,
            ReportCategory::Changed,
            ReportCategory::FromOpen,
            ReportCategory::Kept,
        ]
    );
    assert!(ReportCategory::NeedsAttention < ReportCategory::Kept);
}

#[test]
fn lines_sort_by_text_ignoring_case_then_exactly() {
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::Changed, "b second"),
        entry(ReportCategory::Changed, "B first"),
        entry(ReportCategory::Changed, "a zero"),
        entry(ReportCategory::Changed, "A zero"),
    ]));
    let texts: Vec<&str> = r.groups[0]
        .entries
        .iter()
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(texts, ["A zero", "a zero", "B first", "b second"]);
}

#[test]
fn repeated_lines_are_shown_once_and_merge_their_names() {
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::NotSaved, "x").with_affected(&["B"]),
        entry(ReportCategory::NotSaved, "x").with_affected(&["A", "B"]),
        // The same sentence in another category is another line.
        entry(ReportCategory::Changed, "x"),
    ]));
    assert_eq!(r.count(ReportCategory::NotSaved), 1);
    assert_eq!(r.count(ReportCategory::Changed), 1);
    assert_eq!(r.groups[0].entries[0].affected, ["A", "B"]);
}

#[test]
fn counts_and_the_summary_line() {
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::Changed, "1"),
        entry(ReportCategory::Changed, "2"),
        entry(ReportCategory::Changed, "3"),
        entry(ReportCategory::NotSaved, "4"),
        entry(ReportCategory::NotSaved, "5"),
    ]));
    assert_eq!(r.summary().as_deref(), Some("2 not saved, 3 changes"));
    let one = SaveReport::new(&report(vec![
        entry(ReportCategory::NeedsAttention, "a"),
        entry(ReportCategory::Changed, "c"),
        entry(ReportCategory::Kept, "k"),
        entry(ReportCategory::FromOpen, "o"),
    ]));
    assert_eq!(
        one.summary().as_deref(),
        Some("1 needs attention, 1 change, 1 from the open, 1 kept as they are")
    );
    assert_eq!(SaveReport::default().summary(), None);
    let two = SaveReport::new(&report(vec![
        entry(ReportCategory::NeedsAttention, "a"),
        entry(ReportCategory::NeedsAttention, "b"),
    ]));
    assert_eq!(two.summary().as_deref(), Some("2 need attention"));
}

#[test]
fn the_ask_rule_only_kept_lines_do_not_ask() {
    for category in ReportCategory::ALL {
        let r = SaveReport::new(&report(vec![entry(category, "line")]));
        assert_eq!(r.asks(), category != ReportCategory::Kept, "{category:?}");
    }
    let mixed = SaveReport::new(&report(vec![
        entry(ReportCategory::Kept, "k"),
        entry(ReportCategory::Changed, "c"),
    ]));
    assert!(mixed.asks());
    assert!(!SaveReport::default().asks());
}

/// Task 161: the 12-line cap and "and N more" are gone; every line of every
/// group is in the dialog body (the dialog scrolls instead).
#[test]
fn every_line_of_every_group_is_in_the_dialog_body() {
    let n = 40;
    let mut entries = Vec::new();
    for category in ReportCategory::ALL {
        for i in 0..n {
            entries.push(entry(category, &format!("{category:?} line {i:02}")));
        }
    }
    let r = SaveReport::new(&report(entries));
    let body = r.dialog_body();
    for category in ReportCategory::ALL {
        for i in 0..n {
            assert!(
                body.contains(&format!("{category:?} line {i:02}")),
                "{category:?} {i}"
            );
        }
    }
    assert!(!body.contains("more"), "{body}");
    assert!(!body.contains("Aurora's log"), "{body}");
    assert_eq!(
        body.matches('\u{2022}').count(),
        n * ReportCategory::ALL.len()
    );
    // Needs attention and Not saved still come first.
    let na = body.find("Needs your attention").unwrap_or(usize::MAX);
    let ns = body.find("Not saved:").unwrap_or(usize::MAX);
    let ch = body.find("Changed:").unwrap_or(usize::MAX);
    assert!(na < ns && ns < ch, "{body}");
    // And through the real messages.
    let rep = report(vec![entry(
        ReportCategory::Changed,
        "zzz last changed line",
    )]);
    assert!(save_message("a.psd", &rep).contains("zzz last changed line"));
    assert!(check_message("a", &rep).contains("zzz last changed line"));
}

/// The dialog lists a line's every affected layer when the sentence shows
/// only the first five, up to the 2,000-name limit.
#[test]
fn a_report_naming_two_thousand_layers_lists_them_all_in_the_dialog() {
    let names: Vec<String> = (0..2000).map(|i| format!("Layer {i:04}")).collect();
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::Changed, "Two thousand layers.").with_affected(&names),
    ]));
    let body = r.dialog_body();
    assert!(body.contains("\u{201c}Layer 0000\u{201d}"), "first");
    assert!(body.contains("\u{201c}Layer 1999\u{201d}"), "last");
    assert!(
        !body.contains("and 0 more") && !body.contains(" more"),
        "none cut"
    );
    // Past the limit the log's own rule applies.
    let more: Vec<String> = (0..2003).map(|i| format!("L{i:04}")).collect();
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::Changed, "Many.").with_affected(&more),
    ]));
    let body = r.dialog_body();
    assert!(
        body.contains("L1999") && !body.contains("L2000"),
        "cut at the limit"
    );
    assert!(
        body.ends_with("and 3 more"),
        "{}",
        &body[body.len().saturating_sub(40)..]
    );
    assert_eq!(
        r.full_text().matches("All:").count(),
        1,
        "the log is as before"
    );
}

#[test]
fn the_all_line_appears_only_when_the_sentence_does_not_list_every_name() {
    let five: Vec<String> = (0..aurora_io::REPORT_NAMES_SHOWN)
        .map(|i| format!("N{i}"))
        .collect();
    let mut six = five.clone();
    six.push("N5".to_owned());
    let at_limit = SaveReport::new(&report(vec![
        entry(ReportCategory::NotSaved, "Five.").with_affected(&five),
    ]));
    assert!(!at_limit.dialog_body().contains("All:"));
    let over = SaveReport::new(&report(vec![
        entry(ReportCategory::NotSaved, "Six.").with_affected(&six),
    ]));
    assert!(over.dialog_body().contains("All:"));
    // The log keeps listing from two names, as before.
    assert!(at_limit.full_text().contains("All:"));
}

/// The asking rule is unchanged by the full list: any non-Kept line asks.
#[test]
fn the_ask_rule_is_unchanged_with_many_lines() {
    let many_kept = SaveReport::new(&report(
        (0..100)
            .map(|i| entry(ReportCategory::Kept, &format!("k{i}")))
            .collect(),
    ));
    assert!(!many_kept.asks());
    let mut entries: Vec<ReportEntry> = (0..100)
        .map(|i| entry(ReportCategory::Kept, &format!("k{i}")))
        .collect();
    entries.push(entry(ReportCategory::FromOpen, "carried"));
    assert!(SaveReport::new(&report(entries)).asks());
}

#[test]
fn the_full_text_lists_every_affected_name_past_the_sentences_own_cap() {
    let names: Vec<String> = (0..9).map(|i| format!("Layer {i}")).collect();
    let r = SaveReport::new(&report(vec![
        entry(ReportCategory::NotSaved, "Nine layers.").with_affected(&names),
    ]));
    let full = r.full_text();
    assert!(full.contains("\u{201c}Layer 8\u{201d}"), "{full}");
    // A single name is already in the sentence: no repeat.
    let one = SaveReport::new(&report(vec![
        entry(ReportCategory::NotSaved, "One.").with_affected(&["Solo"]),
    ]));
    assert!(!one.full_text().contains("All:"));
}

#[test]
fn file_derived_text_is_sanitised() {
    let r = SaveReport::new(&report(vec![entry(
        ReportCategory::FromOpen,
        "evil\u{202e}\u{7} line",
    )]));
    assert_eq!(r.groups[0].entries[0].text, "evil line");
}

#[test]
fn the_save_message_has_counts_groups_and_the_footer() {
    let message = save_message(
        "a.psd",
        &report(vec![
            entry(
                ReportCategory::FromOpen,
                "Clipping masks are shown unclipped.",
            ),
            entry(
                ReportCategory::NotSaved,
                "Paths are not saved in a Photoshop file.",
            ),
            entry(ReportCategory::Kept, "Smart objects kept."),
        ]),
    );
    assert_eq!(
        message,
        format!(
            "Saving \u{201c}a.psd\u{201d} as a Photoshop file changes it (1 not saved, 1 from \
             the open, 1 kept as they are):\nNot saved:\n\u{2022} Paths are not saved in a \
             Photoshop file.\n{FROM_OPEN_HEADING}:\n\u{2022} Clipping masks are shown \
             unclipped.\nKept as they are:\n\u{2022} Smart objects kept.\nSave Anyway writes the \
             file with these changes; your document is not changed. Nothing has been written yet."
        )
    );
}

#[test]
fn the_check_message_says_nothing_was_written_and_never_claims_photoshop() {
    let empty = check_message("doc", &PsdWriteReport::default());
    assert!(
        empty.contains("would change nothing that Aurora can detect"),
        "{empty}"
    );
    assert!(empty.contains("No file was written"), "{empty}");
    assert!(
        empty.contains("has not been opened in Photoshop"),
        "{empty}"
    );
    let kept_only = check_message("doc", &report(vec![entry(ReportCategory::Kept, "kept")]));
    assert!(kept_only.contains("would change nothing:\n"), "{kept_only}");
    assert!(kept_only.contains("kept"), "{kept_only}");
    let lossy = check_message("doc", &report(vec![entry(ReportCategory::Changed, "c")]));
    assert!(lossy.contains("would change it (1 change):"), "{lossy}");
}

// -- task 161: the real dialog in the real workspace --

mod dialog {
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey};
    use aurora_widgets::widgets::WidgetKind;
    use aurora_widgets::{FocusManager, WidgetId, WidgetTree};

    use crate::{
        DialogPointer, OpenDialog, PointerButton, handle_dialog_key, handle_dialog_pointer,
        is_dialog_scrollbar, open_dialog, open_dialog_lines, psd_save,
    };

    use super::*;

    const WINDOW: (f32, f32) = (1000.0, 500.0);

    struct Rig {
        workspace: aurora_ui::Workspace,
        focus: FocusManager,
        dialog: Option<OpenDialog>,
    }

    fn rig(line_count: usize, scrolling: bool) -> Rig {
        let scales = crate::test_workspace_scales();
        let mut workspace = aurora_ui::build_workspace(&scales);
        let mut focus = FocusManager::default();
        let mut dialog = None;
        aurora_widgets::compute_text_layout(&mut workspace.tree, WINDOW.0, WINDOW.1, None);
        let entries: Vec<ReportEntry> = (0..line_count)
            .map(|i| entry(ReportCategory::Changed, &format!("Changed line {i:03}.")))
            .collect();
        let message = save_message("big.psd", &report(entries));
        let lines: Vec<String> = message.split('\n').map(str::to_owned).collect();
        let opened = if scrolling {
            open_dialog_lines(
                &mut workspace,
                &mut focus,
                &mut dialog,
                &scales,
                psd_save::PSD_SAVE_WITH_CHANGES_TITLE,
                &message,
                &lines,
                psd_save::psd_save_dialog_actions(),
                crate::DialogPurpose::PsdSaveWithChanges,
            )
        } else {
            open_dialog(
                &mut workspace,
                &mut focus,
                &mut dialog,
                &scales,
                psd_save::PSD_SAVE_WITH_CHANGES_TITLE,
                &message,
                psd_save::psd_save_dialog_actions(),
                crate::DialogPurpose::PsdSaveWithChanges,
            )
        };
        assert!(opened);
        aurora_widgets::compute_text_layout(&mut workspace.tree, WINDOW.0, WINDOW.1, None);
        Rig {
            workspace,
            focus,
            dialog,
        }
    }

    fn tree(rig: &Rig) -> &WidgetTree<WidgetKind> {
        &rig.workspace.tree
    }

    fn open(rig: &Rig) -> &OpenDialog {
        match rig.dialog.as_ref() {
            Some(dialog) => dialog,
            None => unreachable!("open"),
        }
    }

    fn body(rig: &Rig) -> WidgetId {
        match open(rig).body {
            Some(body) => body,
            None => unreachable!("a scrolling dialog"),
        }
    }

    fn key(key: NamedKey) -> KeyChord {
        KeyChord::new(Modifiers::none(), Key::Named(key))
    }

    #[test]
    fn a_three_hundred_line_report_scrolls_inside_the_window_with_its_buttons_reachable() {
        let rig = rig(300, true);
        let body = body(&rig);
        let t = tree(&rig);
        assert!(t.scroll_range(body).is_some_and(|r| r > 1000.0), "scrolls");
        let rows = t.children(body).map_or(0, <[WidgetId]>::len);
        assert!(rows >= 300, "every line is a row: {rows}");
        let Some(dialog) = t.bounds(open(&rig).root) else {
            unreachable!("laid out");
        };
        assert!(
            dialog.y >= 0 && dialog.y + i64::from(dialog.height) <= 500,
            "inside the window: {dialog:?}"
        );
        for (id, button) in &open(&rig).actions {
            let Some(b) = t.bounds(*button) else {
                unreachable!("laid out");
            };
            #[allow(clippy::cast_precision_loss)]
            let centre = (
                b.x as f32 + b.width as f32 / 2.0,
                b.y as f32 + b.height as f32 / 2.0,
            );
            assert_eq!(t.hit_test(centre), Some(*button), "{id} is reachable");
        }
    }

    #[test]
    fn the_plain_dialog_for_the_same_report_does_not_scroll() {
        let rig = rig(300, false);
        assert_eq!(
            open(&rig).body,
            None,
            "the old path stays for other dialogs"
        );
    }

    #[test]
    fn page_keys_scroll_and_home_and_end_reach_the_ends() {
        let mut rig = rig(300, true);
        let body = body(&rig);
        let at = |rig: &Rig| tree(rig).scroll_y(body).unwrap_or(-1.0);
        assert!(at(&rig).abs() < f32::EPSILON);
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        assert!(handle_dialog_key(workspace, focus, dialog, key(NamedKey::PageDown)).is_none());
        let page = at(&rig);
        assert!(page > 0.0, "PageDown moved the message");
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        handle_dialog_key(workspace, focus, dialog, key(NamedKey::ArrowDown));
        assert!(at(&rig) > page, "ArrowDown moved it further");
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        handle_dialog_key(workspace, focus, dialog, key(NamedKey::End));
        let range = tree(&rig).scroll_range(body).unwrap_or(0.0);
        assert!((at(&rig) - range).abs() < 1.0, "End is the last line");
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        handle_dialog_key(workspace, focus, dialog, key(NamedKey::Home));
        assert!(at(&rig).abs() < f32::EPSILON, "Home is the top");
        assert!(rig.dialog.is_some(), "scrolling never closes the dialog");
    }

    #[test]
    fn enter_and_escape_still_choose_cancel_and_enter_never_saves() {
        for named in [NamedKey::Enter, NamedKey::Escape] {
            let mut rig = rig(300, true);
            let Rig {
                workspace,
                focus,
                dialog,
            } = &mut rig;
            let result = handle_dialog_key(workspace, focus, dialog, key(named));
            let Some(result) = result else {
                unreachable!("a choice");
            };
            assert_eq!(result.action, psd_save::PSD_SAVE_CANCEL, "{named:?}");
            assert!(rig.dialog.is_none());
        }
    }

    #[test]
    fn the_dialog_wheel_target_is_the_body_only_over_a_scrolling_dialog() {
        let scrolling = rig(300, true);
        let Some(dialog) = tree(&scrolling).bounds(open(&scrolling).root) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let inside = (
            dialog.x as f32 + 40.0,
            dialog.y as f32 + dialog.height as f32 / 2.0,
        );
        assert_eq!(
            crate::dialog_wheel_target(tree(&scrolling), scrolling.dialog.as_ref(), inside),
            Some(body(&scrolling))
        );
        assert_eq!(
            crate::dialog_wheel_target(tree(&scrolling), scrolling.dialog.as_ref(), (1.0, 1.0)),
            None,
            "outside the dialog the modal still swallows the wheel"
        );
        let plain = rig(300, false);
        assert_eq!(
            crate::dialog_wheel_target(tree(&plain), plain.dialog.as_ref(), inside),
            None
        );
    }

    #[test]
    fn only_the_dialogs_scrollbar_is_let_through_a_press_and_a_button_click_still_chooses() {
        let mut rig = rig(300, true);
        let bar = match open(&rig).scrollbar {
            Some(bar) => bar,
            None => unreachable!("a bar"),
        };
        assert!(is_dialog_scrollbar(tree(&rig), rig.dialog.as_ref(), bar));
        let (_, cancel) = open(&rig).actions[1].clone();
        assert!(!is_dialog_scrollbar(
            tree(&rig),
            rig.dialog.as_ref(),
            cancel
        ));
        assert!(!is_dialog_scrollbar(tree(&rig), None, bar));
        // A linked scrollbar elsewhere in the window (a panel's) is not the
        // dialog's: the modal still swallows presses on it.
        let mut others = 0;
        let mut stack = vec![tree(&rig).root()];
        while let Some(id) = stack.pop() {
            stack.extend(tree(&rig).children(id).unwrap_or_default().iter().copied());
            if aurora_widgets::widgets::scrollbar_target(tree(&rig), id).is_some()
                && !tree(&rig).is_within(open(&rig).root, id)
            {
                others += 1;
                assert!(!is_dialog_scrollbar(tree(&rig), rig.dialog.as_ref(), id));
            }
        }
        assert!(
            others > 0,
            "the workspace has panel scrollbars to test against"
        );
        // A press on a button is the choice; one on the bar is swallowed
        // here (the App then routes it to the bar).
        let Some(b) = tree(&rig).bounds(bar) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let on_bar = (b.x as f32 + 2.0, b.y as f32 + 2.0);
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        assert!(matches!(
            handle_dialog_pointer(workspace, focus, dialog, PointerButton::Primary, on_bar),
            DialogPointer::Swallowed
        ));
        assert!(rig.dialog.is_some());
        let Some(cb) = tree(&rig).bounds(cancel) else {
            unreachable!("laid out");
        };
        #[allow(clippy::cast_precision_loss)]
        let on_cancel = (cb.x as f32 + 4.0, cb.y as f32 + 4.0);
        let Rig {
            workspace,
            focus,
            dialog,
        } = &mut rig;
        assert!(matches!(
            handle_dialog_pointer(workspace, focus, dialog, PointerButton::Primary, on_cancel),
            DialogPointer::Chosen(_)
        ));
    }
}
