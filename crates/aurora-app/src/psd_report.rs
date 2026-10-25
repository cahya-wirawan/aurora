//! The PSD save report as the user reads it (task 157): "will this look the
//! same in Photoshop?".
//!
//! [`SaveReport`] is a pure view of a [`aurora_io::PsdWriteReport`]'s
//! classified entries: duplicates removed, grouped by
//! [`ReportCategory`], sorted deterministically (group order, then the
//! sentence case-insensitively), counted, and — for the dialog — cut at
//! [`MAX_ITEMS_PER_GROUP`] lines with "and N more" in the Changed, From open
//! and Kept groups only: **Needs attention and Not saved lines are never
//! cut** ([`cap_for`]), so nothing that is lost or unchecked hides behind
//! "and N more". The dialog's message is one plain label that does not
//! scroll; a very long Needs attention or Not saved list therefore grows
//! the dialog (each sentence already shows at most five names, the rest are
//! in [`SaveReport::full_text`]) and may not fit a small window. Nothing
//! here changes what is written. [`SaveReport::full_text`] always has every
//! line and every affected name, for the log.
//!
//! **The ask rule.** A save asks "Save With Changes?" first when any entry's
//! category [asks](ReportCategory::asks): Needs attention, Not saved,
//! Changed and From open always do. Only [`ReportCategory::Kept`] entries,
//! which say that smart objects and text layers are written back as they
//! were read, do not: a report with nothing else saves without a question,
//! exactly as it did before they were listed (they were never in the report
//! then). Unverified-in-Photoshop effects and alpha channels are Needs
//! attention, so they still ask.
//!
//! **Kept lines are deliberately silent** although what they describe
//! (smart-object and text data written back verbatim) has not been checked
//! in Photoshop either: they are information, listed by Check PSD
//! Compatibility and in a dialog that is shown anyway, and they never make a
//! save ask. A PSD whose only report lines are the open's own kept-text or
//! alpha-channel notes still asks, because those notes are carried into the
//! save as From open lines ([`ReportCategory::FromOpen`]), which always ask.
//!
//! The wording is plain text in one dialog message: no section styling, as
//! no token exists for it.

use std::fmt::Write as _;

use aurora_io::{PsdWriteReport, ReportCategory, ReportEntry};

/// Lines a Changed, From open or Kept group shows in a dialog before "and
/// N more". Needs attention and Not saved are never cut: a line there may
/// be the one that matters (see [`cap_for`]).
pub(crate) const MAX_ITEMS_PER_GROUP: usize = 12;

/// The dialog's line cap for `category`: `None` (every line) for the two
/// categories that say something is lost or unchecked.
pub(crate) const fn cap_for(category: ReportCategory) -> Option<usize> {
    match category {
        ReportCategory::NeedsAttention | ReportCategory::NotSaved => None,
        _ => Some(MAX_ITEMS_PER_GROUP),
    }
}

/// The names listed after a line in the full text, at most.
const MAX_FULL_NAMES: usize = 2000;

/// The heading of the "carried from open" group (it was a sentence of the
/// flat report before task 157).
pub(crate) const FROM_OPEN_HEADING: &str = "Already different when this file was opened";

/// The group's heading in the dialog.
pub(crate) const fn heading(category: ReportCategory) -> &'static str {
    match category {
        ReportCategory::NeedsAttention => "Needs your attention",
        ReportCategory::NotSaved => "Not saved",
        ReportCategory::Changed => "Changed",
        ReportCategory::FromOpen => FROM_OPEN_HEADING,
        ReportCategory::Kept => "Kept as they are",
    }
}

/// One category's lines: deduplicated, sorted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) category: ReportCategory,
    pub(crate) entries: Vec<ReportEntry>,
}

/// A save report, grouped and sorted. Empty groups are not kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SaveReport {
    pub(crate) groups: Vec<Group>,
}

impl SaveReport {
    /// Groups `report`'s entries: dropping repeats (same category and
    /// sentence; their affected names are merged), then ordering groups as
    /// [`ReportCategory::ALL`] and lines by sentence, ignoring case, then
    /// exactly, so the same entries always give the same report whatever
    /// order they arrived in.
    pub(crate) fn new(report: &PsdWriteReport) -> Self {
        let mut groups = Vec::new();
        for category in ReportCategory::ALL {
            let mut entries: Vec<ReportEntry> = Vec::new();
            for entry in report.entries.iter().filter(|e| e.category == category) {
                let text = aurora_doc::sanitize_display_name(&entry.text).into_owned();
                if let Some(seen) = entries.iter_mut().find(|e| e.text == text) {
                    seen.affected.extend(entry.affected.iter().cloned());
                    seen.affected.sort();
                    seen.affected.dedup();
                } else {
                    let mut copy = entry.clone();
                    copy.text = text;
                    entries.push(copy);
                }
            }
            if entries.is_empty() {
                continue;
            }
            entries.sort_by(|a, b| {
                a.text
                    .to_lowercase()
                    .cmp(&b.text.to_lowercase())
                    .then_with(|| a.text.cmp(&b.text))
            });
            groups.push(Group { category, entries });
        }
        Self { groups }
    }

    /// How many lines the report has.
    pub(crate) fn total(&self) -> usize {
        self.groups.iter().map(|g| g.entries.len()).sum()
    }

    /// Lines in `category`.
    #[cfg(test)]
    pub(crate) fn count(&self, category: ReportCategory) -> usize {
        self.groups
            .iter()
            .find(|g| g.category == category)
            .map_or(0, |g| g.entries.len())
    }

    /// Whether the save must ask first: some line's category
    /// [asks](ReportCategory::asks).
    pub(crate) fn asks(&self) -> bool {
        self.groups.iter().any(|g| g.category.asks())
    }

    /// Whether the dialog cuts any group short.
    pub(crate) fn is_truncated(&self) -> bool {
        self.groups
            .iter()
            .any(|g| cap_for(g.category).is_some_and(|cap| g.entries.len() > cap))
    }

    /// "3 changes, 2 not saved": a count for each group that has lines
    /// (Kept ones too, last), or `None` for an empty report.
    pub(crate) fn summary(&self) -> Option<String> {
        let parts: Vec<String> = self
            .groups
            .iter()
            .map(|g| {
                let n = g.entries.len();
                match g.category {
                    ReportCategory::NeedsAttention => {
                        format!(
                            "{n} {}",
                            if n == 1 {
                                "needs attention"
                            } else {
                                "need attention"
                            }
                        )
                    }
                    ReportCategory::NotSaved => format!("{n} not saved"),
                    ReportCategory::Changed => {
                        format!("{n} {}", if n == 1 { "change" } else { "changes" })
                    }
                    ReportCategory::FromOpen => format!("{n} from the open"),
                    ReportCategory::Kept => format!("{n} kept as they are"),
                }
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// The groups as sections of bullet lines; when `capped`, a group with a
    /// cap ([`cap_for`]) is cut there with "and N more".
    fn sections(&self, capped: bool) -> String {
        let mut text = String::new();
        for group in &self.groups {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(heading(group.category));
            text.push(':');
            let shown = cap_for(group.category)
                .filter(|_| capped)
                .map_or(group.entries.len(), |cap| cap.min(group.entries.len()));
            for entry in group.entries.iter().take(shown) {
                text.push_str("\n\u{2022} ");
                text.push_str(&entry.text);
            }
            let more = group.entries.len() - shown;
            if more > 0 {
                let _ = write!(text, "\n  \u{2026} and {more} more");
            }
        }
        text
    }

    /// The dialog's body: the sections, capped.
    pub(crate) fn dialog_body(&self) -> String {
        let mut body = self.sections(true);
        if self.is_truncated() {
            body.push_str("\n(The full list is in Aurora's log.)");
        }
        body
    }

    /// Every line and every affected name, for the log and for tests.
    pub(crate) fn full_text(&self) -> String {
        let mut text = String::new();
        for group in &self.groups {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(heading(group.category));
            text.push(':');
            for entry in &group.entries {
                text.push_str("\n\u{2022} ");
                text.push_str(&entry.text);
                if entry.affected.len() > 1 {
                    let names: Vec<String> = entry
                        .affected
                        .iter()
                        .take(MAX_FULL_NAMES)
                        .map(|n| {
                            format!("\u{201c}{}\u{201d}", aurora_doc::sanitize_display_name(n))
                        })
                        .collect();
                    text.push_str("\n    All: ");
                    text.push_str(&names.join(", "));
                    if entry.affected.len() > MAX_FULL_NAMES {
                        let _ = write!(text, " and {} more", entry.affected.len() - MAX_FULL_NAMES);
                    }
                }
            }
        }
        text
    }
}

/// The "Save With Changes?" message for `file_name`.
pub(crate) fn save_message(file_name: &str, report: &PsdWriteReport) -> String {
    let grouped = SaveReport::new(report);
    let mut message = format!("Saving \u{201c}{file_name}\u{201d} as a Photoshop file changes it");
    if let Some(summary) = grouped.summary() {
        let _ = write!(message, " ({summary})");
    }
    message.push_str(":\n");
    message.push_str(&grouped.dialog_body());
    message.push_str(
        "\nSave Anyway writes the file with these changes; your document is not changed. \
         Nothing has been written yet.",
    );
    message
}

/// The "Check PSD Compatibility" message for the document called `name`.
pub(crate) fn check_message(name: &str, report: &PsdWriteReport) -> String {
    let grouped = SaveReport::new(report);
    let mut message = format!("Saving \u{201c}{name}\u{201d} as a Photoshop file");
    if grouped.asks() {
        if let Some(summary) = grouped.summary() {
            let _ = write!(message, " would change it ({summary})");
        }
        message.push_str(":\n");
    } else if grouped.total() == 0 {
        message.push_str(" would change nothing that Aurora can detect.");
    } else {
        message.push_str(" would change nothing:\n");
    }
    let body = grouped.dialog_body();
    if !body.is_empty() {
        message.push_str(&body);
    }
    message.push_str(
        "\nNo file was written. This is Aurora's own check of what it would save; it has not \
         been opened in Photoshop.",
    );
    message
}
