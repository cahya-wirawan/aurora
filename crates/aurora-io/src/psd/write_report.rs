//! The PSD save report's structure (task 157).
//!
//! A save's report used to be a flat list of sentences. Each sentence is now
//! a [`ReportEntry`] with a [`ReportCategory`] and, where the writer knows
//! them, the names of the layers (or saved selections) it is about, so the
//! app can group, count and cap them. The sentences themselves are
//! unchanged, and so is what is written: this module only describes it.

/// How many layer names a report sentence lists before "and N more"; a
/// dialog shows every name of a longer list on its own line (task 161).
pub const REPORT_NAMES_SHOWN: usize = 5;

/// What kind of thing a [`ReportEntry`] says about the saved file.
///
/// Declared in the order a reader should meet them: what needs a decision
/// first, what is kept as it is last. The derived order is that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReportCategory {
    /// Saved, but Aurora could not check it in Photoshop or Photoshop may
    /// draw it differently: unverified effect or channel layouts, a colour
    /// profile that is not kept.
    NeedsAttention,
    /// Part of the document is not in the file (or not editable there).
    NotSaved,
    /// Saved, but rounded, normalised, shortened or converted.
    Changed,
    /// Already different when the document was opened (carried from the
    /// open's own report); the app adds these, the writer never does.
    FromOpen,
    /// Written back as it was read (smart objects, text layers): information
    /// only. It never makes a save ask a question.
    Kept,
}

impl ReportCategory {
    /// Every category, in display order.
    pub const ALL: [Self; 5] = [
        Self::NeedsAttention,
        Self::NotSaved,
        Self::Changed,
        Self::FromOpen,
        Self::Kept,
    ];

    /// Whether an entry of this category makes a save ask first. Only
    /// [`Self::Kept`] does not: it records that something survives intact.
    #[must_use]
    pub const fn asks(self) -> bool {
        !matches!(self, Self::Kept)
    }
}

/// One line of the report: the sentence, its category and, where the writer
/// knows them, every name it concerns (layers, or saved selections for the
/// alpha-channel lines), sorted and without duplicates. The sentence may
/// show only the first few of those names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportEntry {
    pub category: ReportCategory,
    pub text: String,
    pub affected: Vec<String>,
}

impl ReportEntry {
    #[must_use]
    pub fn new(category: ReportCategory, text: impl Into<String>) -> Self {
        Self {
            category,
            text: text.into(),
            affected: Vec::new(),
        }
    }

    /// The same entry naming `names`, sorted, without duplicates.
    #[must_use]
    pub fn with_affected<S: AsRef<str>>(mut self, names: &[S]) -> Self {
        let mut affected: Vec<String> = names.iter().map(|n| n.as_ref().to_owned()).collect();
        affected.sort();
        affected.dedup();
        self.affected = affected;
        self
    }
}

/// What a PSD save changes, one human-readable line per kind of change,
/// in a fixed order (categories A then B of the PSD write plan). Empty
/// for a lossless save.
///
/// `items` holds the sentences that make the save *lossy* (every entry whose
/// category [asks](ReportCategory::asks)), exactly as before task 157;
/// `entries` holds those plus the [`ReportCategory::Kept`] ones, classified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PsdWriteReport {
    pub items: Vec<String>,
    pub entries: Vec<ReportEntry>,
}

impl PsdWriteReport {
    /// A report from classified entries; `items` follows from them.
    #[must_use]
    pub fn from_entries(entries: Vec<ReportEntry>) -> Self {
        let items = entries
            .iter()
            .filter(|entry| entry.category.asks())
            .map(|entry| entry.text.clone())
            .collect();
        Self { items, entries }
    }

    /// Appends entries (the app's own: lines carried from the open, colour
    /// and path notes), keeping `items` in step.
    pub fn extend_entries(&mut self, more: impl IntoIterator<Item = ReportEntry>) {
        for entry in more {
            if entry.category.asks() {
                self.items.push(entry.text.clone());
            }
            self.entries.push(entry);
        }
    }

    /// Whether the save changes nothing (Kept lines do not count): the save
    /// then needs no confirmation.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
