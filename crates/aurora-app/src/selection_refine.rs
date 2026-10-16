//! Select > Modify (0.216.0): Feather, Expand, Contract, Smooth and Border
//! Selection, the app half. The maths is `aurora_doc`'s pure refinements
//! (`aurora_doc::{feather, expand, contract, smooth, border}`); this module
//! owns the number prompt and the one undo step.
//!
//! **The prompt** is the command palette's own prompt widget (the one
//! "Save Workspace As…" uses): a single row that says what Enter does
//! ("Expand Selection by 5 px"), the typed text starting as the last value
//! used for that command (kept in [`RefineAmounts`] for the session: not
//! document state, not persisted). Enter applies a valid number; an invalid
//! one keeps the prompt open and shows the reason in the row and in the
//! prompt's accessible description, which typing withdraws; Escape cancels.
//! Whole pixels are asked for by every command but Feather, which takes a
//! decimal radius of at least 1 px (a box blur cannot show less).
//!
//! **The step.** [`refine_step`] runs the refinement on the selection in
//! force, synchronously on the UI thread (the size guard in
//! `aurora_doc::MAX_REFINE_WORK` keeps that under about a second), and
//! records ONE selection step labelled "Feather Selection" (and so on)
//! through the same path every selection change takes: dirty-neutral, a
//! History row, undo and redo restore the exact previous selection. With no
//! selection, or a result the caps refuse, it says so in the status bar and
//! changes nothing.

use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message, set_command_palette_query,
};

use crate::{FocusManager, LockRefusal, WidgetId, command_palette_style};

/// A Select > Modify command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefineKind {
    Feather,
    Expand,
    Contract,
    Smooth,
    Border,
}

impl RefineKind {
    /// Every command, in the menu's order.
    pub(crate) const ALL: [Self; 5] = [
        Self::Feather,
        Self::Expand,
        Self::Contract,
        Self::Smooth,
        Self::Border,
    ];

    /// The palette and menu command id.
    pub(crate) const fn command_id(self) -> &'static str {
        match self {
            Self::Feather => "select.feather",
            Self::Expand => "select.expand",
            Self::Contract => "select.contract",
            Self::Smooth => "select.smooth",
            Self::Border => "select.border",
        }
    }

    /// The palette and menu title ("…": it asks for a number).
    pub(crate) const fn menu_label(self) -> &'static str {
        match self {
            Self::Feather => "Feather Selection\u{2026}",
            Self::Expand => "Expand Selection\u{2026}",
            Self::Contract => "Contract Selection\u{2026}",
            Self::Smooth => "Smooth Selection\u{2026}",
            Self::Border => "Border Selection\u{2026}",
        }
    }

    /// The History row of the step.
    pub(crate) const fn step_label(self) -> &'static str {
        match self {
            Self::Feather => "Feather Selection",
            Self::Expand => "Expand Selection",
            Self::Contract => "Contract Selection",
            Self::Smooth => "Smooth Selection",
            Self::Border => "Border Selection",
        }
    }

    /// The prompt's accessible name.
    pub(crate) const fn prompt_label(self) -> &'static str {
        match self {
            Self::Feather => "Feather Radius in Pixels",
            Self::Expand => "Expand By in Pixels",
            Self::Contract => "Contract By in Pixels",
            Self::Smooth => "Smooth Radius in Pixels",
            Self::Border => "Border Width in Pixels",
        }
    }

    /// The id of the prompt's one row.
    pub(crate) const fn row_id(self) -> &'static str {
        match self {
            Self::Feather => "select.feather.apply",
            Self::Expand => "select.expand.apply",
            Self::Contract => "select.contract.apply",
            Self::Smooth => "select.smooth.apply",
            Self::Border => "select.border.apply",
        }
    }

    /// The kind whose prompt row is `id`.
    pub(crate) fn for_row(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.row_id() == id)
    }

    /// The kind with command id `id`.
    pub(crate) fn for_command(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.command_id() == id)
    }

    /// The smallest and largest value the prompt accepts.
    pub(crate) const fn range(self) -> (f32, f32) {
        match self {
            Self::Feather => (1.0, aurora_doc::MAX_FEATHER_PX),
            Self::Expand | Self::Contract | Self::Border => (1.0, aurora_doc::MAX_REFINE_PX as f32),
            Self::Smooth => (1.0, aurora_doc::MAX_SMOOTH_PX as f32),
        }
    }

    /// Whether a fractional value is accepted (only Feather's radius).
    pub(crate) const fn allows_fraction(self) -> bool {
        matches!(self, Self::Feather)
    }

    /// What Enter does, for the prompt's row: "Expand Selection by 5 px".
    fn row_title(self, amount: &str) -> String {
        match self {
            Self::Border => format!("Border Selection, {amount} px wide"),
            Self::Feather => format!("Feather Selection, radius {amount} px"),
            Self::Smooth => format!("Smooth Selection, radius {amount} px"),
            Self::Expand => format!("Expand Selection by {amount} px"),
            Self::Contract => format!("Contract Selection by {amount} px"),
        }
    }
}

/// The last value used for each command this session (defaults: 5 px).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RefineAmounts {
    values: [f32; 5],
}

impl Default for RefineAmounts {
    fn default() -> Self {
        Self { values: [5.0; 5] }
    }
}

impl RefineAmounts {
    fn slot(kind: RefineKind) -> usize {
        RefineKind::ALL
            .iter()
            .position(|k| *k == kind)
            .unwrap_or_default()
    }

    /// The value the prompt for `kind` starts with.
    pub(crate) fn get(&self, kind: RefineKind) -> f32 {
        self.values.get(Self::slot(kind)).copied().unwrap_or(5.0)
    }

    /// Remembers `value` for `kind`.
    pub(crate) fn set(&mut self, kind: RefineKind, value: f32) {
        if let Some(slot) = self.values.get_mut(Self::slot(kind)) {
            *slot = value;
        }
    }
}

/// `value` as the prompt shows it: whole numbers without a decimal point.
pub(crate) fn amount_text(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// The number typed into `kind`'s prompt, or the reason it is refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_amount(kind: RefineKind, text: &str) -> Result<f32, String> {
    let trimmed = text.trim();
    let digits = trimmed.strip_suffix("px").map_or(trimmed, str::trim_end);
    if digits.is_empty() {
        return Err("Enter a number of pixels.".to_owned());
    }
    if !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("Use digits only, for example 5.".to_owned());
    }
    if digits.contains('.') && !kind.allows_fraction() {
        return Err("Use a whole number of pixels.".to_owned());
    }
    let Ok(value) = digits.parse::<f32>() else {
        return Err("That is not a number.".to_owned());
    };
    let (min, max) = kind.range();
    if !value.is_finite() || value < min {
        return Err(format!("Enter {} px or more.", amount_text(min)));
    }
    if value > max {
        return Err(format!("At most {} px.", amount_text(max)));
    }
    Ok(value)
}

/// The prompt's one row for the typed `query`: what Enter would do.
fn prompt_entries(kind: RefineKind, query: &str) -> Vec<CommandEntry> {
    let amount = query.trim();
    let title = if amount.is_empty() {
        kind.menu_label().trim_end_matches('\u{2026}').to_owned()
    } else {
        kind.row_title(amount)
    };
    vec![CommandEntry::new(kind.row_id(), title)]
}

/// Opens `kind`'s prompt (a no-op returning `false` if a palette or prompt
/// is open), the typed text starting as `start`.
pub(crate) fn open_prompt(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    kind: RefineKind,
    start: f32,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let query = amount_text(start);
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        kind.prompt_label(),
        prompt_entries(kind, &query),
        &workspace.scales,
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the selection prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the selection prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
        tracing::warn!(?err, "failed to fill the selection prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the selection prompt");
    }
    *palette = Some(root);
    true
}

/// The kind of the Select > Modify prompt `root` is, if it is one.
fn prompt_kind(
    tree: &aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<RefineKind> {
    let state = command_palette_state(tree, root).ok()?;
    if state.is_filtering() {
        return None;
    }
    let first = state.commands().first()?;
    RefineKind::for_row(&first.id)
}

/// After the typed text changed: the row follows it and a refusal is
/// withdrawn. Returns whether `root` is a Select > Modify prompt (so the
/// caller leaves other palettes alone).
pub(crate) fn follow_prompt_query(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> bool {
    let Some(kind) = prompt_kind(tree, root) else {
        return false;
    };
    let Ok(state) = command_palette_state(tree, root) else {
        return true;
    };
    let query = state.query().to_owned();
    if let Err(err) = set_command_palette_commands(tree, root, prompt_entries(kind, &query)) {
        tracing::warn!(?err, "failed to update the selection prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the selection prompt's message");
    }
    true
}

/// The prompt's Enter. `None` when `root` is no Select > Modify prompt.
/// Otherwise `Some(Ok((kind, amount)))` for a valid number (the caller
/// closes the prompt and applies it) or `Some(Err(()))` for a refused one:
/// the prompt stays open, its row shows the reason and its accessible
/// description carries it.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<(RefineKind, f32), ()>> {
    let kind = prompt_kind(tree, root)?;
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    match parse_amount(kind, &query) {
        Ok(amount) => Some(Ok((kind, amount))),
        Err(message) => {
            let row = vec![CommandEntry::new(kind.row_id(), message.clone())];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused selection amount");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused selection amount");
            }
            Some(Err(()))
        }
    }
}

/// What one Select > Modify command did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefineOutcome {
    /// The selection changed: one step was recorded.
    Applied,
    /// The result equals the selection (nothing to record).
    Unchanged,
    /// A calm status-bar notice, nothing changed.
    Refused(LockRefusal),
}

/// The refinement of `kind` by `amount` on `selection`.
fn refine(
    selection: &aurora_doc::Selection,
    kind: RefineKind,
    amount: f32,
) -> Result<Option<aurora_doc::Selection>, aurora_doc::DocError> {
    // Whole pixels, validated 1..=500 by the prompt.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = amount.round().max(0.0) as u32;
    match kind {
        RefineKind::Feather => aurora_doc::feather(selection, amount),
        RefineKind::Expand => aurora_doc::expand(selection, px),
        RefineKind::Contract => aurora_doc::contract(selection, px),
        RefineKind::Smooth => aurora_doc::smooth(selection, px),
        RefineKind::Border => aurora_doc::border(selection, px),
    }
}

/// Runs `kind` by `amount` on the active selection and records the one
/// step. The caller ends any live selection gesture first.
#[allow(clippy::too_many_arguments)]
pub(crate) fn refine_step(
    workspace: &mut aurora_ui::Workspace,
    undo_order: &mut crate::UndoOrder,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    selection: &mut aurora_doc::SelectionSet,
    kind: RefineKind,
    amount: f32,
) -> RefineOutcome {
    let Some(before) = selection.active() else {
        return RefineOutcome::Refused(LockRefusal::RefineNeedsSelection);
    };
    let after = match refine(&before, kind, amount) {
        Ok(after) => after,
        Err(err) => {
            tracing::debug!(?err, ?kind, "a selection refinement was refused");
            return RefineOutcome::Refused(LockRefusal::RefineTooLarge);
        }
    };
    match after {
        Some(next) => selection.select(next),
        None => selection.deselect(),
    }
    if crate::record_selection_step(
        workspace,
        undo_order,
        history,
        pixel_history,
        kind.step_label(),
        Some(before),
        selection.active(),
    ) {
        RefineOutcome::Applied
    } else {
        RefineOutcome::Unchanged
    }
}
