//! Tests for the grouped PSD save report (task 157): grouping, order,
//! dedupe, caps, counts, the ask rule and the two messages.

#![allow(clippy::indexing_slicing)] // fixtures with known group counts

use aurora_io::{PsdWriteReport, ReportCategory, ReportEntry};

use crate::psd_report::{
    FROM_OPEN_HEADING, MAX_ITEMS_PER_GROUP, SaveReport, check_message, save_message,
};

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

#[test]
fn a_long_group_is_capped_with_and_n_more_but_the_full_text_has_everything() {
    let n = MAX_ITEMS_PER_GROUP + 3;
    let entries: Vec<ReportEntry> = (0..n)
        .map(|i| entry(ReportCategory::Changed, &format!("line {i:02}")))
        .collect();
    let r = SaveReport::new(&report(entries));
    assert!(r.is_truncated());
    let body = r.dialog_body();
    assert!(body.contains("line 00"), "{body}");
    assert!(
        body.contains(&format!("line {:02}", MAX_ITEMS_PER_GROUP - 1)),
        "{body}"
    );
    assert!(
        !body.contains(&format!("line {MAX_ITEMS_PER_GROUP:02}")),
        "{body}"
    );
    assert!(body.contains("\u{2026} and 3 more"), "{body}");
    assert!(body.contains("full list is in Aurora's log"), "{body}");
    let full = r.full_text();
    for i in 0..n {
        assert!(full.contains(&format!("line {i:02}")), "{full}");
    }
    assert!(!full.contains("and 3 more"), "{full}");
    // At exactly the cap nothing is cut.
    let exact = SaveReport::new(&report(
        (0..MAX_ITEMS_PER_GROUP)
            .map(|i| entry(ReportCategory::Changed, &format!("l{i}")))
            .collect(),
    ));
    assert!(!exact.is_truncated());
    assert!(!exact.dialog_body().contains("more"));
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
