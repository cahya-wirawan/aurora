//! The PSD save report as the user reads it (task 157): "will this look the
//! same in Photoshop?".
//!
//! [`SaveReport`] is a pure view of a [`aurora_io::PsdWriteReport`]'s
//! classified entries: duplicates removed, grouped by
//! [`ReportCategory`], sorted deterministically (group order, then the
//! sentence case-insensitively) and counted. **The dialog shows every line
//! of every group** (task 161; before it, Changed, From open and Kept were
//! cut at 12 lines with "and N more"): the message is drawn as rows in a
//! scroll container capped to the window (`aurora_widgets::widgets::
//! insert_dialog_scrolling`), so a long list scrolls instead of growing the
//! dialog. Needs attention and Not saved come first. A line that names more
//! layers than its sentence lists is followed by an "All:" line with every
//! name (at most [`MAX_FULL_NAMES`], then "and N more"). Nothing here
//! changes what is written. [`SaveReport::full_text`] is the log's text and
//! is unchanged.
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
//! The wording is plain text in one dialog message. The section headings are
//! not bold: the only bundled face is Inter Regular and the text engine has
//! no weight selection (task 161, decision B skipped).

use std::fmt::Write as _;

use aurora_io::{PsdWriteReport, ReportCategory, ReportEntry};

/// The names listed after a line in the full text, at most.
pub(crate) const MAX_FULL_NAMES: usize = 2000;

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

    /// The dialog's body: every group as a heading and bullet lines, every
    /// line included. A line that names more layers than its sentence lists
    /// ([`aurora_io::REPORT_NAMES_SHOWN`]) is followed by an "All:" line.
    pub(crate) fn dialog_body(&self) -> String {
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
                if entry.affected.len() > aurora_io::REPORT_NAMES_SHOWN {
                    push_all_names(&mut text, &entry.affected);
                }
            }
        }
        text
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
                    push_all_names(&mut text, &entry.affected);
                }
            }
        }
        text
    }
}

/// Appends the "All:" line naming every affected layer (at most
/// [`MAX_FULL_NAMES`], then "and N more").
fn push_all_names(text: &mut String, affected: &[String]) {
    let names: Vec<String> = affected
        .iter()
        .take(MAX_FULL_NAMES)
        .map(|n| format!("\u{201c}{}\u{201d}", aurora_doc::sanitize_display_name(n)))
        .collect();
    text.push_str("\n    All: ");
    text.push_str(&names.join(", "));
    if affected.len() > MAX_FULL_NAMES {
        let _ = write!(text, " and {} more", affected.len() - MAX_FULL_NAMES);
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
