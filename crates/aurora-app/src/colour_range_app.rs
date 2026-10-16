//! Select > Colour Range (0.238.0), the app half. The maths is
//! `aurora_brush::colour_range` (CIE76 distance, a soft ramp); this module
//! owns the Fuzziness prompt and the one undo step.
//!
//! **What it selects.** The pixels of the **active pixel layer** (like the
//! Magic Wand: the composite is never sampled, a group, an adjustment layer
//! or no layer is refused) inside the document whose colour is within the
//! Fuzziness of the **foreground colour** (the Eyedropper sets it). The mask
//! is soft: a pixel's coverage falls from full to none as its distance goes
//! from half the threshold to the whole of it, scaled by its alpha. It
//! combines with the selection in force by the options bar's selection mode
//! (New replaces; Add, Subtract and Intersect combine; the modifiers play no
//! part in a command) and is recorded as ONE selection step, "Colour Range"
//! (a combining mode's own label in a combining mode).
//!
//! **The prompt** is the command palette's prompt widget, like Select >
//! Modify's (`selection_refine`): one row saying what Enter does ("Colour
//! Range, Fuzziness 40"), the typed text starting as the last Fuzziness used
//! (`ToolSettings::colour_range_fuzziness`: session state, not persisted,
//! default 40), a whole number `0..=200`; an invalid one keeps the prompt
//! open and shows the reason.
//!
//! **Cost.** The scan runs synchronously on the UI thread over the part of
//! the layer inside the document (at most 64 Mpx, else refused with the calm
//! "too large" notice), reading only the tiles that exist.

use aurora_core::Rect;
use aurora_doc::{LayerKind, LayerTree, Selection, SelectionMode};
use aurora_tile::TileStore;
use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message, set_command_palette_query,
};

use crate::{FocusManager, LockRefusal, WidgetId, command_palette_style};

/// The palette and menu command id.
pub(crate) const COMMAND_ID: &str = "select.colour_range";

/// The palette and menu title ("…": it asks for a number).
pub(crate) const COMMAND_TITLE: &str = "Select Colour Range\u{2026}";

/// The History row of the step (New mode).
pub(crate) const STEP_LABEL: &str = "Colour Range";

/// The prompt's accessible name.
const PROMPT_LABEL: &str = "Colour Range Fuzziness";

/// The id of the prompt's one row.
const ROW_ID: &str = "select.colour_range.apply";

/// The Fuzziness the prompt starts with the first time.
pub(crate) const DEFAULT_FUZZINESS: u16 = 40;

/// The number typed into the prompt, or the reason it is refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_fuzziness(text: &str) -> Result<u16, String> {
    let digits = text.trim();
    if digits.is_empty() {
        return Err("Enter a Fuzziness from 0 to 200.".to_owned());
    }
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err("Use a whole number, for example 40.".to_owned());
    }
    let Ok(value) = digits.parse::<u32>() else {
        return Err("That is not a number.".to_owned());
    };
    if value > u32::from(aurora_brush::MAX_FUZZINESS) {
        return Err(format!("At most {}.", aurora_brush::MAX_FUZZINESS));
    }
    u16::try_from(value).map_err(|_| "That is not a number.".to_owned())
}

/// The prompt's one row for the typed `query`: what Enter would do.
fn prompt_entries(query: &str) -> Vec<CommandEntry> {
    let amount = query.trim();
    let title = if amount.is_empty() {
        "Colour Range".to_owned()
    } else {
        format!("Colour Range, Fuzziness {amount}")
    };
    vec![CommandEntry::new(ROW_ID, title)]
}

/// Opens the prompt (a no-op returning `false` if a palette or prompt is
/// open), the typed text starting as `start`.
pub(crate) fn open_prompt(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    start: u16,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let query = start.to_string();
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        PROMPT_LABEL,
        prompt_entries(&query),
        &workspace.scales,
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the Colour Range prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the Colour Range prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
        tracing::warn!(?err, "failed to fill the Colour Range prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the Colour Range prompt");
    }
    *palette = Some(root);
    true
}

/// Whether `root` is the Colour Range prompt.
fn is_prompt(tree: &aurora_widgets::WidgetTree<crate::WidgetKind>, root: WidgetId) -> bool {
    let Ok(state) = command_palette_state(tree, root) else {
        return false;
    };
    !state.is_filtering() && state.commands().first().is_some_and(|c| c.id == ROW_ID)
}

/// After the typed text changed: the row follows it and a refusal is
/// withdrawn. Returns whether `root` is the Colour Range prompt.
pub(crate) fn follow_prompt_query(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> bool {
    if !is_prompt(tree, root) {
        return false;
    }
    let Ok(state) = command_palette_state(tree, root) else {
        return true;
    };
    let query = state.query().to_owned();
    if let Err(err) = set_command_palette_commands(tree, root, prompt_entries(&query)) {
        tracing::warn!(?err, "failed to update the Colour Range prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the Colour Range prompt's message");
    }
    true
}

/// The prompt's Enter. `None` when `root` is not the Colour Range prompt;
/// `Some(Ok(fuzziness))` for a valid number (the caller closes the prompt
/// and applies it); `Some(Err(()))` for a refused one: the prompt stays
/// open and shows the reason.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<u16, ()>> {
    if !is_prompt(tree, root) {
        return None;
    }
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    match parse_fuzziness(&query) {
        Ok(fuzziness) => Some(Ok(fuzziness)),
        Err(message) => {
            let row = vec![CommandEntry::new(ROW_ID, message.clone())];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused Fuzziness");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused Fuzziness");
            }
            Some(Err(()))
        }
    }
}

/// What the command did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColourRangeOutcome {
    /// The selection changed: one step was recorded.
    Applied,
    /// The result equals the selection (nothing to record).
    Unchanged,
    /// A calm status-bar notice, nothing changed.
    Refused(LockRefusal),
}

/// The Colour Range selection of `layer` (never empty), or why not.
///
/// # Errors
///
/// A refusal for the status bar.
pub(crate) fn colour_range_select(
    store: &mut TileStore,
    layers: &LayerTree,
    layer: Option<aurora_doc::LayerId>,
    colour: [f32; 3],
    fuzziness: u16,
    document_size: (u32, u32),
) -> Result<Selection, LockRefusal> {
    let needs_layer = LockRefusal::ColourRangeNeedsPixelLayer;
    let id = layer.ok_or(needs_layer)?;
    let Some(LayerKind::Pixel { bounds }) = layers.kind(id).cloned() else {
        return Err(needs_layer);
    };
    let surface = layers.surface_id(id).ok_or(needs_layer)?;
    // The document, in layer-local texels.
    let window = Rect {
        x: -bounds.x,
        y: -bounds.y,
        width: document_size.0,
        height: document_size.1,
    };
    let spec = aurora_brush::ColourRangeSpec {
        colour,
        fuzziness,
        window,
        max_tiles: aurora_brush::MAX_FILL_TILES,
    };
    let mask =
        match aurora_brush::colour_range(store, surface, (bounds.width, bounds.height), &spec) {
            Ok(Some(mask)) => mask,
            Ok(None) => return Err(LockRefusal::ColourRangeNothing),
            Err(aurora_brush::FillError::TooLarge { .. }) => {
                return Err(LockRefusal::SelectionTooLarge);
            }
            Err(aurora_brush::FillError::Tile(err)) => {
                tracing::warn!(?err, "Colour Range could not read a tile");
                return Err(LockRefusal::ColourRangeNothing);
            }
        };
    let doc_bounds = Rect {
        x: mask.bounds.x.saturating_add(bounds.x),
        y: mask.bounds.y.saturating_add(bounds.y),
        width: mask.bounds.width,
        height: mask.bounds.height,
    };
    match aurora_doc::SelectionMask::new(doc_bounds, mask.data) {
        Ok(mask) => Ok(Selection::from_mask(std::sync::Arc::new(mask))),
        Err(aurora_doc::DocError::SelectionMaskTooLarge(_)) => Err(LockRefusal::SelectionTooLarge),
        Err(_) => Err(LockRefusal::ColourRangeNothing),
    }
}

/// Runs Colour Range on `layer` for `colour` and `fuzziness` and records the
/// one step. The caller ends any live selection gesture first.
#[allow(clippy::too_many_arguments)]
pub(crate) fn colour_range_step(
    workspace: &mut aurora_ui::Workspace,
    undo_order: &mut crate::UndoOrder,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    selection: &mut aurora_doc::SelectionSet,
    store: &mut TileStore,
    layers: &LayerTree,
    layer: Option<aurora_doc::LayerId>,
    colour: [f32; 3],
    fuzziness: u16,
    document_size: (u32, u32),
    mode: SelectionMode,
) -> ColourRangeOutcome {
    let picked = match colour_range_select(store, layers, layer, colour, fuzziness, document_size) {
        Ok(picked) => picked,
        Err(why) => return ColourRangeOutcome::Refused(why),
    };
    let before = selection.active();
    let Ok(combined) = aurora_doc::combine_selections(before.as_ref(), &picked, mode) else {
        return ColourRangeOutcome::Refused(LockRefusal::SelectionTooLarge);
    };
    match combined {
        Some(next) => selection.select(next),
        None => selection.deselect(),
    }
    let label = crate::selection_modes::step_label(mode, STEP_LABEL);
    if crate::record_selection_step(
        workspace,
        undo_order,
        history,
        pixel_history,
        label,
        before,
        selection.active(),
    ) {
        ColourRangeOutcome::Applied
    } else {
        ColourRangeOutcome::Unchanged
    }
}
