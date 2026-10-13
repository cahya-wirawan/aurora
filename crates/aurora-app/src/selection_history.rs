//! The memory budget of selection undo steps (0.218.0).
//!
//! A selection undo step holds the selection before and after it. A raster
//! mask is shared through an `Arc` (one 64 Mpx mask is 64 MiB), so a long
//! session of mask edits (Lasso, Magic Wand, Feather) used to grow history
//! without bound. [`trim`] fixes that **without ever dropping a
//! step**:
//!
//! * [`distinct_mask_bytes`] sums the bytes of the distinct masks the
//!   stacks reference (the same `Arc` is counted once, by pointer, and a
//!   packed mask is counted at its packed length once);
//! * when that is over the budget, [`trim`] packs the
//!   OLDEST steps' masks first (lossless `lz4`, `aurora_doc::PackedSelectionMask`),
//!   stopping as soon as the total is within budget; a mask referenced by
//!   several steps is packed once and shared;
//! * undo and redo unpack on demand; the unpacked selection equals the
//!   original bit for bit (tested);
//! * if everything is packed and the total is still over the budget, the
//!   steps are **kept** and [`SelectionHistoryStats::over_budget`] says so:
//!   there is no spill-to-disk for selections, and a step is never lost
//!   silently.
//!
//! **Budget:** 256 MiB by default, an engineering default (a quarter of the
//! 1 GiB scale of the pixel history's smallest budget, four full 64 Mpx
//! masks); `AURORA_SELECTION_HISTORY_MB` overrides it (whole MiB, 1 to
//! 16384; anything else is ignored). It is per document.

use std::collections::HashMap;
use std::sync::Arc;

use aurora_core::Rect;
use aurora_doc::{PackedSelectionMask, Selection, SelectionShape};

/// The default budget in bytes.
pub(crate) const DEFAULT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// The environment variable overriding the budget, in MiB.
pub(crate) const BUDGET_ENV: &str = "AURORA_SELECTION_HISTORY_MB";

/// The budget for an override text: whole MiB from 1 to 16384, else the
/// default.
pub(crate) fn resolve_budget_bytes(value: Option<&str>) -> u64 {
    value
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|mb| (1..=16_384).contains(mb))
        .map_or(DEFAULT_BUDGET_BYTES, |mb| mb * 1024 * 1024)
}

/// The budget for this process.
pub(crate) fn system_budget_bytes() -> u64 {
    resolve_budget_bytes(std::env::var(BUDGET_ENV).ok().as_deref())
}

/// A mask selection, packed.
#[derive(Debug, Clone)]
pub(crate) struct PackedSelection {
    inverted: bool,
    bounds: Rect,
    mask: Arc<PackedSelectionMask>,
}

impl PackedSelection {
    /// Identity of the shared packed mask (for sharing tests).
    #[cfg(test)]
    pub(crate) fn mask_ptr(&self) -> *const PackedSelectionMask {
        Arc::as_ptr(&self.mask)
    }
}

impl PartialEq for PackedSelection {
    fn eq(&self, other: &Self) -> bool {
        self.inverted == other.inverted && self.bounds == other.bounds && self.mask == other.mask
    }
}

/// One side of a step: the selection, or its packed form.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StepSelection {
    Live(Option<Selection>),
    Packed(PackedSelection),
}

impl StepSelection {
    pub(crate) fn live(selection: Option<Selection>) -> Self {
        Self::Live(selection)
    }

    /// The selection, unpacking if needed; `Err` only if packed bytes
    /// fail to decode (never for bytes this module packed).
    pub(crate) fn resolve(&self) -> Result<Option<Selection>, ()> {
        match self {
            Self::Live(selection) => Ok(selection.clone()),
            Self::Packed(packed) => {
                let mask = packed.mask.unpack().map_err(|_| ())?;
                Ok(Some(Selection {
                    bounds: packed.bounds,
                    inverted: packed.inverted,
                    shape: SelectionShape::Mask(Arc::new(mask)),
                }))
            }
        }
    }
}

/// One selection change.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectionStep {
    pub(crate) before: StepSelection,
    pub(crate) after: StepSelection,
}

impl SelectionStep {
    pub(crate) fn new(before: Option<Selection>, after: Option<Selection>) -> Self {
        Self {
            before: StepSelection::live(before),
            after: StepSelection::live(after),
        }
    }
}

/// What the budget accounting sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionHistoryStats {
    /// Steps on both stacks (never reduced by the budget).
    pub(crate) steps: usize,
    /// Bytes of distinct unpacked masks.
    pub(crate) live_bytes: u64,
    /// Bytes of distinct packed masks.
    pub(crate) packed_bytes: u64,
    /// Whether the total is over the budget with nothing left to pack.
    pub(crate) over_budget: bool,
}

impl SelectionHistoryStats {
    /// All bytes held.
    pub(crate) const fn total_bytes(&self) -> u64 {
        self.live_bytes + self.packed_bytes
    }
}

/// Bytes of the distinct masks `undo` and `redo` reference.
pub(crate) fn distinct_mask_bytes(
    undo: &[SelectionStep],
    redo: &[SelectionStep],
) -> SelectionHistoryStats {
    let mut live: HashMap<*const aurora_doc::SelectionMask, u64> = HashMap::new();
    let mut packed: HashMap<*const PackedSelectionMask, u64> = HashMap::new();
    for step in undo.iter().chain(redo) {
        for side in [&step.before, &step.after] {
            match side {
                StepSelection::Live(Some(selection)) => {
                    if let SelectionShape::Mask(mask) = &selection.shape {
                        live.insert(Arc::as_ptr(mask), mask.bounds().area_px());
                    }
                }
                StepSelection::Live(None) => {}
                StepSelection::Packed(p) => {
                    packed.insert(
                        Arc::as_ptr(&p.mask),
                        u64::try_from(p.mask.packed_len()).unwrap_or(u64::MAX),
                    );
                }
            }
        }
    }
    SelectionHistoryStats {
        steps: undo.len() + redo.len(),
        live_bytes: live.values().sum(),
        packed_bytes: packed.values().sum(),
        over_budget: false,
    }
}

/// The step at `index` counting undo bottom to top, then redo bottom to top
/// (oldest first).
fn step_at<'a>(
    undo: &'a mut [SelectionStep],
    redo: &'a mut [SelectionStep],
    index: usize,
) -> Option<&'a mut SelectionStep> {
    if index < undo.len() {
        undo.get_mut(index)
    } else {
        redo.get_mut(index - undo.len())
    }
}

/// Packs the oldest steps' masks until the total is within `budget`;
/// returns the resulting stats. Steps are never removed. `over_budget` is
/// set when every mask is packed and the total still exceeds `budget`.
pub(crate) fn trim(
    undo: &mut [SelectionStep],
    redo: &mut [SelectionStep],
    budget: u64,
) -> SelectionHistoryStats {
    let mut stats = distinct_mask_bytes(undo, redo);
    if stats.total_bytes() <= budget {
        return stats;
    }
    // One packed copy per distinct source mask.
    let mut packed_of: HashMap<*const aurora_doc::SelectionMask, Arc<PackedSelectionMask>> =
        HashMap::new();
    for index in 0..stats.steps {
        let Some(step) = step_at(undo, redo, index) else {
            break;
        };
        let mut changed = false;
        for side in [&mut step.before, &mut step.after] {
            let StepSelection::Live(Some(selection)) = &*side else {
                continue;
            };
            let SelectionShape::Mask(mask) = &selection.shape else {
                continue;
            };
            let packed = packed_of
                .entry(Arc::as_ptr(mask))
                .or_insert_with(|| Arc::new(mask.pack()))
                .clone();
            let replacement = StepSelection::Packed(PackedSelection {
                inverted: selection.inverted,
                bounds: selection.bounds,
                mask: packed,
            });
            *side = replacement;
            changed = true;
        }
        if changed {
            stats = distinct_mask_bytes(undo, redo);
            if stats.total_bytes() <= budget {
                return stats;
            }
        }
    }
    stats.over_budget = stats.total_bytes() > budget;
    stats
}
