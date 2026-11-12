//! Editable layer filters, the commands (task 176): "Gaussian Blur…" adds a
//! blur to the active pixel layer, and a few palette/menu commands edit,
//! hide or remove the layer's filters. The data is `aurora_core::
//! LayerFilters`, the drawing is [`crate::layer_filter_app`].
//!
//! **What exists.** Gaussian Blur… (asks for the radius in the command
//! palette's number prompt, like Select > Modify: the typed text starts as
//! the last radius used this session, `ToolSettings::gaussian_blur_radius`,
//! not persisted), Edit Last Layer Filter… (the same prompt, starting at
//! the layer's last filter's radius), Hide or Show Layer Filters, Remove
//! Last Layer Filter and Remove All Layer Filters. Each is ONE structural
//! undo step through `History::set_filters`, labelled in the History panel.
//! A blur applied to a layer that already has one stacks (Photoshop's
//! Smart Filters stack too); the filters' total reach is capped at one tile
//! ([`aurora_core::MAX_FILTER_REACH`]).
//!
//! **What does not.** No Properties-panel editor (slider per filter, eye,
//! delete button) yet: editing is by command, and only the last filter can
//! be edited. No live preview while the radius is typed. Nothing is
//! persisted but the document's own `layer-filters` entry.

use aurora_widgets::widgets::{
    CommandEntry, command_palette_state, insert_command_prompt, set_command_palette_commands,
    set_command_palette_message, set_command_palette_query,
};

use crate::{FocusManager, LockRefusal, WidgetId, command_palette_style};

pub(crate) const COMMAND_GAUSSIAN_BLUR: &str = "layer.filter.gaussian_blur";
pub(crate) const COMMAND_EDIT_LAST: &str = "layer.filter.edit_last";
pub(crate) const COMMAND_TOGGLE: &str = "layer.filter.toggle";
pub(crate) const COMMAND_REMOVE_LAST: &str = "layer.filter.remove_last";
pub(crate) const COMMAND_REMOVE_ALL: &str = "layer.filter.remove_all";

/// The radius the prompt starts with the first time.
pub(crate) const DEFAULT_RADIUS: f32 = 5.0;

/// Which prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptKind {
    /// Gaussian Blur…: add a filter.
    Add,
    /// Edit Last Layer Filter…: change the last filter's radius.
    EditLast,
}

impl PromptKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Add => "Gaussian Blur Radius in Pixels",
            Self::EditLast => "Layer Filter Radius in Pixels",
        }
    }

    const fn row_id(self) -> &'static str {
        match self {
            Self::Add => "layer.filter.gaussian_blur.apply",
            Self::EditLast => "layer.filter.edit_last.apply",
        }
    }

    fn for_row(id: &str) -> Option<Self> {
        [Self::Add, Self::EditLast]
            .into_iter()
            .find(|kind| kind.row_id() == id)
    }

    fn row_title(self, amount: &str) -> String {
        match self {
            Self::Add => format!("Gaussian Blur, radius {amount} px"),
            Self::EditLast => format!("Change the last layer filter to radius {amount} px"),
        }
    }

    fn bare_title(self) -> &'static str {
        match self {
            Self::Add => "Gaussian Blur",
            Self::EditLast => "Edit Last Layer Filter",
        }
    }
}

/// One change to a layer's filters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FilterEdit {
    /// Append a Gaussian Blur of this radius.
    AddGaussianBlur(f32),
    /// Set the last filter's radius.
    EditLastRadius(f32),
    /// Hide every filter, or show them all when none is shown.
    ToggleVisible,
    RemoveLast,
    RemoveAll,
}

/// The command a palette/menu id names, for ids that need no number.
pub(crate) fn plain_edit_for_command(id: &str) -> Option<FilterEdit> {
    match id {
        COMMAND_TOGGLE => Some(FilterEdit::ToggleVisible),
        COMMAND_REMOVE_LAST => Some(FilterEdit::RemoveLast),
        COMMAND_REMOVE_ALL => Some(FilterEdit::RemoveAll),
        _ => None,
    }
}

/// The prompt a command id opens, if it opens one.
pub(crate) fn prompt_for_command(id: &str) -> Option<PromptKind> {
    match id {
        COMMAND_GAUSSIAN_BLUR => Some(PromptKind::Add),
        COMMAND_EDIT_LAST => Some(PromptKind::EditLast),
        _ => None,
    }
}

/// What `edit` would leave the layer's filters as, and the History label.
/// `Ok(None)` when nothing would change.
///
/// # Errors
///
/// The calm notice for why it cannot run.
pub(crate) fn plan(
    layers: &aurora_doc::LayerTree,
    layer: Option<aurora_doc::LayerId>,
    edit: FilterEdit,
) -> Result<Option<(Option<aurora_core::LayerFilters>, &'static str)>, LockRefusal> {
    let layer = layer.ok_or(LockRefusal::FilterNeedsPixelLayer)?;
    if !matches!(
        layers.kind(layer),
        Some(aurora_doc::LayerKind::Pixel { .. })
    ) {
        return Err(LockRefusal::FilterNeedsPixelLayer);
    }
    let current = layers.filters(layer).cloned().unwrap_or_default();
    let mut next = current.clone();
    let label = match edit {
        FilterEdit::AddGaussianBlur(radius) => {
            next.filters.push(aurora_core::LayerFilter::new(
                aurora_core::FilterKind::GaussianBlur { radius },
            ));
            "Gaussian Blur"
        }
        FilterEdit::EditLastRadius(radius) => {
            let last = next.filters.last_mut().ok_or(LockRefusal::FilterNothing)?;
            match &mut last.kind {
                aurora_core::FilterKind::GaussianBlur { radius: old } => *old = radius,
            }
            "Edit Layer Filter"
        }
        FilterEdit::ToggleVisible => {
            if next.filters.is_empty() {
                return Err(LockRefusal::FilterNothing);
            }
            let show = !next.any_visible();
            for filter in &mut next.filters {
                filter.visible = show;
            }
            if show {
                "Show Layer Filters"
            } else {
                "Hide Layer Filters"
            }
        }
        FilterEdit::RemoveLast => {
            next.filters.pop().ok_or(LockRefusal::FilterNothing)?;
            "Remove Layer Filter"
        }
        FilterEdit::RemoveAll => {
            if next.filters.is_empty() {
                return Err(LockRefusal::FilterNothing);
            }
            next.filters.clear();
            "Remove Layer Filters"
        }
    };
    next.validate().map_err(|_| LockRefusal::FilterTooLarge)?;
    if next == current {
        return Ok(None);
    }
    Ok(Some(((!next.filters.is_empty()).then_some(next), label)))
}

/// Applies `edit` to `layer` as one undo step. The caller has checked
/// [`plan`]; `None` when nothing changed or the tree refused (logged).
pub(crate) fn apply(
    layers: &mut aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    layer: Option<aurora_doc::LayerId>,
    edit: FilterEdit,
) -> Option<&'static str> {
    let (value, label) = plan(layers, layer, edit).ok()??;
    let layer = layer?;
    match history.set_filters(layers, layer, value) {
        Ok(_) => Some(label),
        Err(err) => {
            tracing::warn!(
                ?err,
                "the layer filter change was refused by the layer tree"
            );
            None
        }
    }
}

/// The last filter's radius on `layer`, the Edit prompt's starting value.
pub(crate) fn last_radius(
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
) -> Option<f32> {
    match layers.filters(layer)?.filters.last()?.kind {
        aurora_core::FilterKind::GaussianBlur { radius } => Some(radius),
    }
}

/// `value` as the prompt shows it.
fn amount_text(value: f32) -> String {
    crate::selection_refine::amount_text(value)
}

/// The radius typed into the prompt, or the reason it is refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_radius(text: &str) -> Result<f32, String> {
    let trimmed = text.trim();
    let digits = trimmed.strip_suffix("px").map_or(trimmed, str::trim_end);
    if digits.is_empty() {
        return Err("Enter a radius in pixels.".to_owned());
    }
    if !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("Use digits only, for example 5.".to_owned());
    }
    let Ok(value) = digits.parse::<f32>() else {
        return Err("That is not a number.".to_owned());
    };
    if !value.is_finite() || value < aurora_core::MIN_GAUSSIAN_RADIUS {
        return Err(format!(
            "Enter {} px or more.",
            amount_text(aurora_core::MIN_GAUSSIAN_RADIUS)
        ));
    }
    if value > aurora_core::MAX_GAUSSIAN_RADIUS {
        return Err(format!(
            "At most {} px.",
            amount_text(aurora_core::MAX_GAUSSIAN_RADIUS)
        ));
    }
    Ok(value)
}

fn prompt_entries(kind: PromptKind, query: &str) -> Vec<CommandEntry> {
    let amount = query.trim();
    let title = if amount.is_empty() {
        kind.bare_title().to_owned()
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
    kind: PromptKind,
    start: f32,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let query = amount_text(start);
    let root = match insert_command_prompt(
        &mut workspace.tree,
        workspace.root,
        kind.label(),
        prompt_entries(kind, &query),
        &workspace.scales,
    ) {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the layer filter prompt");
            return false;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the layer filter prompt");
    }
    if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
        tracing::warn!(?err, "failed to fill the layer filter prompt");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the layer filter prompt");
    }
    *palette = Some(root);
    true
}

fn prompt_kind(
    tree: &aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<PromptKind> {
    let state = command_palette_state(tree, root).ok()?;
    if state.is_filtering() {
        return None;
    }
    PromptKind::for_row(&state.commands().first()?.id)
}

/// After the typed text changed: the row follows it and a refusal is
/// withdrawn. Returns whether `root` is a layer filter prompt.
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
        tracing::warn!(?err, "failed to update the layer filter prompt");
    }
    if let Err(err) = set_command_palette_message(tree, root, None) {
        tracing::warn!(?err, "failed to clear the layer filter prompt's message");
    }
    true
}

/// The prompt's Enter. `None` when `root` is no layer filter prompt;
/// `Some(Ok(edit))` for a valid radius (the caller closes the prompt and
/// applies it); `Some(Err(()))` for a refused one: the prompt stays open
/// and shows the reason.
pub(crate) fn prompt_enter(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
) -> Option<Result<FilterEdit, ()>> {
    let kind = prompt_kind(tree, root)?;
    let query = command_palette_state(tree, root).ok()?.query().to_owned();
    match parse_radius(&query) {
        Ok(radius) => Some(Ok(match kind {
            PromptKind::Add => FilterEdit::AddGaussianBlur(radius),
            PromptKind::EditLast => FilterEdit::EditLastRadius(radius),
        })),
        Err(message) => {
            let row = vec![CommandEntry::new(kind.row_id(), message.clone())];
            if let Err(err) = set_command_palette_commands(tree, root, row) {
                tracing::warn!(?err, "failed to show a refused radius");
            }
            if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
                tracing::warn!(?err, "failed to describe a refused radius");
            }
            Some(Err(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> (
        aurora_doc::LayerTree,
        aurora_doc::History,
        aurora_doc::LayerId,
        aurora_doc::LayerId,
    ) {
        let mut tree = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let rect = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        let Ok(pixel) = history.add_pixel_layer(&mut tree, "p", rect, None) else {
            unreachable!("pixel layer");
        };
        let Ok(group) = history.add_group(&mut tree, "g", None) else {
            unreachable!("group");
        };
        (tree, history, pixel, group)
    }

    #[test]
    fn parse_radius_accepts_the_range_and_explains_refusals() {
        assert_eq!(parse_radius("5"), Ok(5.0));
        assert_eq!(parse_radius(" 2.5 px "), Ok(2.5));
        assert_eq!(parse_radius("0.1"), Ok(0.1));
        assert_eq!(parse_radius("85"), Ok(85.0));
        for bad in ["", "abc", "-3", "0", "0.05", "86", "1e3", "5 6", "."] {
            assert!(parse_radius(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn add_edit_toggle_remove_are_each_one_step_and_undo() {
        let (mut tree, mut history, pixel, _) = doc();
        let mut run =
            |tree: &mut aurora_doc::LayerTree, edit| apply(tree, &mut history, Some(pixel), edit);
        assert_eq!(
            run(&mut tree, FilterEdit::AddGaussianBlur(4.0)),
            Some("Gaussian Blur")
        );
        assert_eq!(tree.filters(pixel).map(|f| f.filters.len()), Some(1));
        assert_eq!(
            run(&mut tree, FilterEdit::EditLastRadius(9.0)),
            Some("Edit Layer Filter")
        );
        assert_eq!(last_radius(&tree, pixel), Some(9.0));
        // Same radius again: nothing to do.
        assert_eq!(run(&mut tree, FilterEdit::EditLastRadius(9.0)), None);
        assert_eq!(
            run(&mut tree, FilterEdit::ToggleVisible),
            Some("Hide Layer Filters")
        );
        assert!(
            !tree
                .filters(pixel)
                .is_some_and(aurora_core::LayerFilters::any_visible)
        );
        assert_eq!(
            run(&mut tree, FilterEdit::ToggleVisible),
            Some("Show Layer Filters")
        );
        assert_eq!(
            run(&mut tree, FilterEdit::AddGaussianBlur(2.0)),
            Some("Gaussian Blur")
        );
        assert_eq!(
            run(&mut tree, FilterEdit::RemoveLast),
            Some("Remove Layer Filter")
        );
        assert_eq!(last_radius(&tree, pixel), Some(9.0));
        assert_eq!(
            run(&mut tree, FilterEdit::RemoveAll),
            Some("Remove Layer Filters")
        );
        assert!(tree.filters(pixel).is_none());
        // Every change was one undo step: undoing all of them empties it.
        for _ in 0..8 {
            if history.undo(&mut tree).is_err() {
                break;
            }
        }
        assert!(tree.filters(pixel).is_none());
    }

    #[test]
    fn refusals_are_calm_notices_and_change_nothing() {
        let (mut tree, mut history, pixel, group) = doc();
        let steps = history.journal_len();
        assert_eq!(
            plan(&tree, None, FilterEdit::RemoveAll),
            Err(LockRefusal::FilterNeedsPixelLayer)
        );
        assert_eq!(
            plan(&tree, Some(group), FilterEdit::AddGaussianBlur(3.0)),
            Err(LockRefusal::FilterNeedsPixelLayer)
        );
        for edit in [
            FilterEdit::EditLastRadius(3.0),
            FilterEdit::ToggleVisible,
            FilterEdit::RemoveLast,
            FilterEdit::RemoveAll,
        ] {
            assert_eq!(
                plan(&tree, Some(pixel), edit),
                Err(LockRefusal::FilterNothing),
                "{edit:?}"
            );
        }
        // Two 85 px blurs reach farther than a tile.
        assert!(
            apply(
                &mut tree,
                &mut history,
                Some(pixel),
                FilterEdit::AddGaussianBlur(85.0)
            )
            .is_some()
        );
        assert_eq!(
            plan(&tree, Some(pixel), FilterEdit::AddGaussianBlur(85.0)),
            Err(LockRefusal::FilterTooLarge)
        );
        assert_eq!(history.journal_len(), steps + 1);
    }

    #[test]
    fn command_ids_resolve() {
        assert_eq!(
            prompt_for_command(COMMAND_GAUSSIAN_BLUR),
            Some(PromptKind::Add)
        );
        assert_eq!(
            prompt_for_command(COMMAND_EDIT_LAST),
            Some(PromptKind::EditLast)
        );
        assert_eq!(
            plain_edit_for_command(COMMAND_TOGGLE),
            Some(FilterEdit::ToggleVisible)
        );
        assert_eq!(plain_edit_for_command(COMMAND_GAUSSIAN_BLUR), None);
    }
}
