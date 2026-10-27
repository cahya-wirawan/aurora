//! What a document's own open changed or left out, one structured line each
//! (task 163).
//!
//! A PSD's open report used to be free text: the lines were carried into
//! every later "Save With Changes?" report as one undifferentiated "From
//! open" group. Each line now carries an [`ImportLossKind`] set where the
//! line is produced (`psd::note_kind` for the reader's counted notes, and
//! beside the text for the colour-mode lines), so the save report can list
//! what is **Changed** (the file's content is kept in another form) apart
//! from what is **Not saved** (something the file had is absent from the
//! document, and so from a saved file). A line nobody could place with
//! confidence stays [`ImportLossKind::FromOpen`], which is also what every
//! line of an older `.aur` reads as.
//!
//! The text of a line is unchanged by the kind; the kind never changes what
//! a save writes, and every kind makes a save ask first (as the single
//! "From open" group always did).

use crate::psd::ReportCategory;

/// How an open-time line is grouped in a later Photoshop save report.
///
/// The discriminants are the `.aur` `import-losses` entry's wire values
/// (version 2) and are frozen: new kinds are appended, and a value this
/// build does not know reads as [`Self::FromOpen`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ImportLossKind {
    /// Already different when the file was opened and not placed more
    /// precisely: display approximations, unverified drawing, damaged-file
    /// repairs and anything uncertain. Also every line of a version-1
    /// `.aur` entry.
    #[default]
    FromOpen = 0,
    /// The content is kept, in another form: converted to pixels or to
    /// another colour space, drawn from its settings, limited to a range, or
    /// held in a model that saves back differently.
    Changed = 1,
    /// Something the file had is not in the document, so not in a saved
    /// file: a layer, mask, effect or setting left out, ignored or not kept,
    /// or the original of something that was turned into pixels.
    NotSaved = 2,
}

impl ImportLossKind {
    /// The kind for a wire value; an unknown value is [`Self::FromOpen`].
    #[must_use]
    pub const fn from_wire(value: u8) -> Self {
        match value {
            1 => Self::Changed,
            2 => Self::NotSaved,
            _ => Self::FromOpen,
        }
    }

    /// The wire value ([`Self::from_wire`]'s inverse).
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        self as u8
    }

    /// The save-report group this kind's lines are listed in.
    #[must_use]
    pub const fn category(self) -> ReportCategory {
        match self {
            Self::FromOpen => ReportCategory::FromOpen,
            Self::Changed => ReportCategory::Changed,
            Self::NotSaved => ReportCategory::NotSaved,
        }
    }
}

/// One line of what an open changed or left out, with its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportLoss {
    pub kind: ImportLossKind,
    pub text: String,
}

impl ImportLoss {
    #[must_use]
    pub fn new(kind: ImportLossKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    /// A line whose kind nobody settled ([`ImportLossKind::FromOpen`]).
    #[must_use]
    pub fn from_open(text: impl Into<String>) -> Self {
        Self::new(ImportLossKind::FromOpen, text)
    }
}
