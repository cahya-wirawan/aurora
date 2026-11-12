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
//! **Unsharp Mask… (task 177)** is the second kind. Its prompt is the same
//! one-line palette prompt with three numbers: "Amount Radius Threshold"
//! typed as `150 1.5 4` (percent, pixels, levels; spaces or commas; a `%`
//! or `px` suffix is allowed), starting as the values last used this
//! session (`ToolSettings::unsharp_settings`, not persisted; Photoshop's
//! defaults 50 / 1 / 0 the first time). One line was chosen over three
//! prompts in a row because the palette prompt has no "next" state, a
//! single line keeps Enter and Escape meaning one thing, and a screen
//! reader hears all three names in the prompt's label. Edit Last Layer
//! Filter… opens the prompt that fits the last filter's kind.
//!
//! **Reduce Noise… (task 178)** is the third. Its prompt is the same kind
//! of one-line prompt with four numbers: "Strength, Preserve Details,
//! Reduce Color Noise, Sharpen Details" typed as `6 60 45 25` (0..=10, then
//! three percentages 0..=100; spaces or commas; a `%` after a percentage
//! is allowed), starting as the values last used this session
//! (`ToolSettings::noise_settings`, not persisted; Photoshop's defaults
//! 6 / 60 / 45 / 25 the first time). Edit Last Layer Filter… opens it
//! with the filter's values when the last filter is a Reduce Noise. The
//! typed (registry) form of Edit Last Layer Filter only has a `radius`, so
//! it cannot change a Reduce Noise, which has none: it is refused with the
//! "no filters to change" notice; the prompt (palette, Filter menu) is the
//! way to edit one.
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
pub(crate) const COMMAND_UNSHARP_MASK: &str = "layer.filter.unsharp_mask";
pub(crate) const COMMAND_REDUCE_NOISE: &str = "layer.filter.reduce_noise";
pub(crate) const COMMAND_EDIT_LAST: &str = "layer.filter.edit_last";
pub(crate) const COMMAND_TOGGLE: &str = "layer.filter.toggle";
pub(crate) const COMMAND_REMOVE_LAST: &str = "layer.filter.remove_last";
pub(crate) const COMMAND_REMOVE_ALL: &str = "layer.filter.remove_all";

/// The radius the prompt starts with the first time.
pub(crate) const DEFAULT_RADIUS: f32 = 5.0;

/// The Photoshop defaults the Unsharp Mask prompt starts with.
pub(crate) const DEFAULT_UNSHARP: UnsharpSettings = UnsharpSettings {
    amount: 50.0,
    radius: 1.0,
    threshold: 0,
};

/// The three numbers of an Unsharp Mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnsharpSettings {
    /// Percent, 1..=500.
    pub(crate) amount: f32,
    /// Pixels (the blur's standard deviation).
    pub(crate) radius: f32,
    /// 8-bit levels, 0..=255.
    pub(crate) threshold: u8,
}

impl UnsharpSettings {
    fn kind(self) -> aurora_core::FilterKind {
        aurora_core::FilterKind::UnsharpMask {
            amount: self.amount,
            radius: self.radius,
            threshold: self.threshold,
        }
    }

    /// The prompt's text for these values.
    pub(crate) fn text(self) -> String {
        format!(
            "{} {} {}",
            amount_text(self.amount),
            amount_text(self.radius),
            self.threshold
        )
    }
}

/// Photoshop's Reduce Noise defaults.
pub(crate) const DEFAULT_NOISE: NoiseSettings = NoiseSettings {
    strength: 6.0,
    preserve_details: 60.0,
    color_noise: 45.0,
    sharpen_details: 25.0,
};

/// The four numbers of a Reduce Noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoiseSettings {
    /// 0..=10.
    pub(crate) strength: f32,
    /// Percent, 0..=100.
    pub(crate) preserve_details: f32,
    /// Percent, 0..=100.
    pub(crate) color_noise: f32,
    /// Percent, 0..=100.
    pub(crate) sharpen_details: f32,
}

impl NoiseSettings {
    fn kind(self) -> aurora_core::FilterKind {
        aurora_core::FilterKind::ReduceNoise {
            strength: self.strength,
            preserve_details: self.preserve_details,
            color_noise: self.color_noise,
            sharpen_details: self.sharpen_details,
        }
    }

    /// The prompt's text for these values.
    pub(crate) fn text(self) -> String {
        format!(
            "{} {} {} {}",
            amount_text(self.strength),
            amount_text(self.preserve_details),
            amount_text(self.color_noise),
            amount_text(self.sharpen_details)
        )
    }
}

/// Which prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptKind {
    /// Gaussian Blur…: add a filter.
    Add,
    /// Edit Last Layer Filter…: change the last filter's radius.
    EditLast,
    /// Unsharp Mask…: add a filter.
    AddUnsharp,
    /// Edit Last Layer Filter… when the last filter is an Unsharp Mask.
    EditLastUnsharp,
    /// Reduce Noise…: add a filter.
    AddNoise,
    /// Edit Last Layer Filter… when the last filter is a Reduce Noise.
    EditLastNoise,
}

impl PromptKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Add => "Gaussian Blur Radius in Pixels",
            Self::EditLast => "Layer Filter Radius in Pixels",
            Self::AddUnsharp | Self::EditLastUnsharp => {
                "Unsharp Mask: Amount in Percent, Radius in Pixels, Threshold in Levels"
            }
            Self::AddNoise | Self::EditLastNoise => {
                "Reduce Noise: Strength 0 to 10, Preserve Details, Reduce Color Noise and Sharpen Details in Percent"
            }
        }
    }

    const fn row_id(self) -> &'static str {
        match self {
            Self::Add => "layer.filter.gaussian_blur.apply",
            Self::EditLast => "layer.filter.edit_last.apply",
            Self::AddUnsharp => "layer.filter.unsharp_mask.apply",
            Self::EditLastUnsharp => "layer.filter.edit_last_unsharp.apply",
            Self::AddNoise => "layer.filter.reduce_noise.apply",
            Self::EditLastNoise => "layer.filter.edit_last_noise.apply",
        }
    }

    fn for_row(id: &str) -> Option<Self> {
        [
            Self::Add,
            Self::EditLast,
            Self::AddUnsharp,
            Self::EditLastUnsharp,
            Self::AddNoise,
            Self::EditLastNoise,
        ]
        .into_iter()
        .find(|kind| kind.row_id() == id)
    }

    fn row_title(self, amount: &str) -> String {
        match self {
            Self::Add => format!("Gaussian Blur, radius {amount} px"),
            Self::EditLast => format!("Change the last layer filter to radius {amount} px"),
            Self::AddUnsharp => match parse_unsharp(amount) {
                Ok(v) => format!("Unsharp Mask, {}", unsharp_summary(v)),
                Err(_) => format!("Unsharp Mask: {amount}"),
            },
            Self::EditLastUnsharp => match parse_unsharp(amount) {
                Ok(v) => format!(
                    "Change the last layer filter to Unsharp Mask, {}",
                    unsharp_summary(v)
                ),
                Err(_) => format!("Edit Unsharp Mask: {amount}"),
            },
            Self::AddNoise => match parse_noise(amount) {
                Ok(v) => format!("Reduce Noise, {}", noise_summary(v)),
                Err(_) => format!("Reduce Noise: {amount}"),
            },
            Self::EditLastNoise => match parse_noise(amount) {
                Ok(v) => format!(
                    "Change the last layer filter to Reduce Noise, {}",
                    noise_summary(v)
                ),
                Err(_) => format!("Edit Reduce Noise: {amount}"),
            },
        }
    }

    fn bare_title(self) -> &'static str {
        match self {
            Self::Add => "Gaussian Blur",
            Self::EditLast | Self::EditLastUnsharp | Self::EditLastNoise => {
                "Edit Last Layer Filter"
            }
            Self::AddUnsharp => "Unsharp Mask",
            Self::AddNoise => "Reduce Noise",
        }
    }
}

/// One change to a layer's filters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FilterEdit {
    /// Append a Gaussian Blur of this radius.
    AddGaussianBlur(f32),
    /// Set the last filter's radius (either kind).
    EditLastRadius(f32),
    /// Append an Unsharp Mask.
    AddUnsharpMask(UnsharpSettings),
    /// Replace the settings of the last filter, which must be an Unsharp
    /// Mask.
    EditLastUnsharp(UnsharpSettings),
    /// Append a Reduce Noise.
    AddReduceNoise(NoiseSettings),
    /// Replace the settings of the last filter, which must be a Reduce
    /// Noise.
    EditLastNoise(NoiseSettings),
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
        COMMAND_UNSHARP_MASK => Some(PromptKind::AddUnsharp),
        COMMAND_REDUCE_NOISE => Some(PromptKind::AddNoise),
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
                aurora_core::FilterKind::GaussianBlur { radius: old }
                | aurora_core::FilterKind::UnsharpMask { radius: old, .. } => *old = radius,
                // A Reduce Noise has no radius; the radius form cannot
                // change it.
                aurora_core::FilterKind::ReduceNoise { .. } => {
                    return Err(LockRefusal::FilterNothing);
                }
            }
            "Edit Layer Filter"
        }
        FilterEdit::AddUnsharpMask(settings) => {
            next.filters
                .push(aurora_core::LayerFilter::new(settings.kind()));
            "Unsharp Mask"
        }
        FilterEdit::EditLastUnsharp(settings) => {
            let last = next.filters.last_mut().ok_or(LockRefusal::FilterNothing)?;
            if !matches!(last.kind, aurora_core::FilterKind::UnsharpMask { .. }) {
                return Err(LockRefusal::FilterNothing);
            }
            last.kind = settings.kind();
            "Edit Unsharp Mask"
        }
        FilterEdit::AddReduceNoise(settings) => {
            next.filters
                .push(aurora_core::LayerFilter::new(settings.kind()));
            "Reduce Noise"
        }
        FilterEdit::EditLastNoise(settings) => {
            let last = next.filters.last_mut().ok_or(LockRefusal::FilterNothing)?;
            if !matches!(last.kind, aurora_core::FilterKind::ReduceNoise { .. }) {
                return Err(LockRefusal::FilterNothing);
            }
            last.kind = settings.kind();
            "Edit Reduce Noise"
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
        aurora_core::FilterKind::GaussianBlur { radius }
        | aurora_core::FilterKind::UnsharpMask { radius, .. } => Some(radius),
        aurora_core::FilterKind::ReduceNoise { .. } => None,
    }
}

/// The last filter's settings on `layer` when it is a Reduce Noise: the
/// Edit prompt's starting values.
pub(crate) fn last_noise(
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
) -> Option<NoiseSettings> {
    match layers.filters(layer)?.filters.last()?.kind {
        aurora_core::FilterKind::ReduceNoise {
            strength,
            preserve_details,
            color_noise,
            sharpen_details,
        } => Some(NoiseSettings {
            strength,
            preserve_details,
            color_noise,
            sharpen_details,
        }),
        aurora_core::FilterKind::GaussianBlur { .. }
        | aurora_core::FilterKind::UnsharpMask { .. } => None,
    }
}

/// The last filter's settings on `layer` when it is an Unsharp Mask: the
/// Edit prompt's starting values.
pub(crate) fn last_unsharp(
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
) -> Option<UnsharpSettings> {
    match layers.filters(layer)?.filters.last()?.kind {
        aurora_core::FilterKind::UnsharpMask {
            amount,
            radius,
            threshold,
        } => Some(UnsharpSettings {
            amount,
            radius,
            threshold,
        }),
        aurora_core::FilterKind::GaussianBlur { .. }
        | aurora_core::FilterKind::ReduceNoise { .. } => None,
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

/// "50 %, radius 1 px, threshold 0" for the prompt's row.
fn unsharp_summary(v: UnsharpSettings) -> String {
    format!(
        "amount {} %, radius {} px, threshold {} levels",
        amount_text(v.amount),
        amount_text(v.radius),
        v.threshold
    )
}

/// The three numbers typed into the Unsharp Mask prompt (amount, radius,
/// threshold; spaces or commas between; `%` after the amount and `px`
/// after the radius allowed), or the reason they are refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_unsharp(text: &str) -> Result<UnsharpSettings, String> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    let [amount, radius, threshold] = tokens.as_slice() else {
        return Err("Enter amount, radius and threshold, for example 100 1.5 0.".to_owned());
    };
    let number = |token: &str, suffix: &str, what: &str| -> Result<f32, String> {
        let digits = token.strip_suffix(suffix).unwrap_or(token);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(format!("The {what} must use digits only, for example 5."));
        }
        digits
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("The {what} is not a number."))
    };
    let amount = number(amount, "%", "amount")?;
    if !(aurora_core::MIN_UNSHARP_AMOUNT..=aurora_core::MAX_UNSHARP_AMOUNT).contains(&amount) {
        return Err(format!(
            "The amount is from {} to {} percent.",
            amount_text(aurora_core::MIN_UNSHARP_AMOUNT),
            amount_text(aurora_core::MAX_UNSHARP_AMOUNT)
        ));
    }
    let radius = number(radius, "px", "radius")?;
    if !(aurora_core::MIN_GAUSSIAN_RADIUS..=aurora_core::MAX_GAUSSIAN_RADIUS).contains(&radius) {
        return Err(format!(
            "The radius is from {} to {} px.",
            amount_text(aurora_core::MIN_GAUSSIAN_RADIUS),
            amount_text(aurora_core::MAX_GAUSSIAN_RADIUS)
        ));
    }
    let threshold = number(threshold, "", "threshold")?;
    if threshold.fract() != 0.0 || !(0.0..=255.0).contains(&threshold) {
        return Err("The threshold is a whole number from 0 to 255 levels.".to_owned());
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=255, whole
    let threshold = threshold as u8;
    Ok(UnsharpSettings {
        amount,
        radius,
        threshold,
    })
}

/// "strength 6, preserve details 60 %, ..." for the prompt's row.
fn noise_summary(v: NoiseSettings) -> String {
    format!(
        "strength {}, preserve details {} %, colour noise {} %, sharpen details {} %",
        amount_text(v.strength),
        amount_text(v.preserve_details),
        amount_text(v.color_noise),
        amount_text(v.sharpen_details)
    )
}

/// The four numbers typed into the Reduce Noise prompt (strength,
/// preserve details, reduce colour noise, sharpen details; spaces or
/// commas between; `%` after a percentage allowed), or the reason they
/// are refused.
///
/// # Errors
///
/// A short sentence for the prompt's row.
pub(crate) fn parse_noise(text: &str) -> Result<NoiseSettings, String> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    let [strength, preserve, colour, sharpen] = tokens.as_slice() else {
        return Err(
            "Enter strength, preserve details, colour noise and sharpen details, for example 6 60 45 25."
                .to_owned(),
        );
    };
    let number = |token: &str, suffix: &str, what: &str, max: f32| -> Result<f32, String> {
        let digits = token.strip_suffix(suffix).unwrap_or(token);
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(format!("The {what} must use digits only, for example 5."));
        }
        let value = digits
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("The {what} is not a number."))?;
        if (0.0..=max).contains(&value) {
            Ok(value)
        } else {
            Err(format!(
                "The {what} is from 0 to {}{}.",
                amount_text(max),
                if suffix == "%" { " percent" } else { "" }
            ))
        }
    };
    let max_percent = aurora_core::MAX_NOISE_PERCENT;
    Ok(NoiseSettings {
        strength: number(strength, "", "strength", aurora_core::MAX_NOISE_STRENGTH)?,
        preserve_details: number(preserve, "%", "preserve details", max_percent)?,
        color_noise: number(colour, "%", "colour noise", max_percent)?,
        sharpen_details: number(sharpen, "%", "sharpen details", max_percent)?,
    })
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
    start: &str,
) -> bool {
    if palette.is_some() {
        return false;
    }
    let query = start.to_owned();
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
    if matches!(kind, PromptKind::AddNoise | PromptKind::EditLastNoise) {
        return Some(match parse_noise(&query) {
            Ok(v) => Ok(if kind == PromptKind::AddNoise {
                FilterEdit::AddReduceNoise(v)
            } else {
                FilterEdit::EditLastNoise(v)
            }),
            Err(message) => show_refusal(tree, root, kind, message),
        });
    }
    if matches!(kind, PromptKind::AddUnsharp | PromptKind::EditLastUnsharp) {
        return Some(match parse_unsharp(&query) {
            Ok(v) => Ok(if kind == PromptKind::AddUnsharp {
                FilterEdit::AddUnsharpMask(v)
            } else {
                FilterEdit::EditLastUnsharp(v)
            }),
            Err(message) => show_refusal(tree, root, kind, message),
        });
    }
    match parse_radius(&query) {
        Ok(radius) => Some(Ok(match kind {
            PromptKind::Add => FilterEdit::AddGaussianBlur(radius),
            // The Unsharp Mask kinds returned above.
            PromptKind::EditLast
            | PromptKind::AddUnsharp
            | PromptKind::EditLastUnsharp
            | PromptKind::AddNoise
            | PromptKind::EditLastNoise => FilterEdit::EditLastRadius(radius),
        })),
        Err(message) => Some(show_refusal(tree, root, kind, message)),
    }
}

/// Shows why the typed text was refused; the prompt stays open.
fn show_refusal(
    tree: &mut aurora_widgets::WidgetTree<crate::WidgetKind>,
    root: WidgetId,
    kind: PromptKind,
    message: String,
) -> Result<FilterEdit, ()> {
    let row = vec![CommandEntry::new(kind.row_id(), message.clone())];
    if let Err(err) = set_command_palette_commands(tree, root, row) {
        tracing::warn!(?err, "failed to show a refused value");
    }
    if let Err(err) = set_command_palette_message(tree, root, Some(message)) {
        tracing::warn!(?err, "failed to describe a refused value");
    }
    Err(())
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

    fn settings(amount: f32, radius: f32, threshold: u8) -> UnsharpSettings {
        UnsharpSettings {
            amount,
            radius,
            threshold,
        }
    }

    #[test]
    fn parse_unsharp_reads_three_numbers_and_explains_refusals() {
        assert_eq!(parse_unsharp("150 1.5 4"), Ok(settings(150.0, 1.5, 4)));
        assert_eq!(parse_unsharp("  50,1,0 "), Ok(settings(50.0, 1.0, 0)));
        assert_eq!(
            parse_unsharp("200% 2.5px 255"),
            Ok(settings(200.0, 2.5, 255))
        );
        assert_eq!(parse_unsharp("1 0.1 0"), Ok(settings(1.0, 0.1, 0)));
        assert_eq!(parse_unsharp("500 85 255"), Ok(settings(500.0, 85.0, 255)));
        for bad in [
            "",
            "50",
            "50 1",
            "50 1 0 0",
            "0 1 0",
            "0.5 1 0",
            "501 1 0",
            "50 0 0",
            "50 0.05 0",
            "50 86 0",
            "50 1 256",
            "50 1 1.5",
            "50 1 -1",
            "-50 1 0",
            "a 1 0",
            "50 1e1 0",
            "5 0 0 ",
            "50 . 0",
        ] {
            assert!(parse_unsharp(bad).is_err(), "{bad:?}");
        }
        // The settings round-trip through the prompt's text.
        let v = settings(137.5, 2.25, 9);
        assert_eq!(parse_unsharp(&v.text()), Ok(v));
        assert_eq!(DEFAULT_UNSHARP.text(), "50 1 0");
    }

    #[test]
    fn unsharp_add_edit_and_undo_are_single_labelled_steps() {
        let (mut tree, mut history, pixel, group) = doc();
        let mut run =
            |tree: &mut aurora_doc::LayerTree, edit| apply(tree, &mut history, Some(pixel), edit);
        // Editing the unsharp settings of a blur is refused.
        assert_eq!(
            run(&mut tree, FilterEdit::AddGaussianBlur(3.0)),
            Some("Gaussian Blur")
        );
        assert_eq!(
            plan(
                &tree,
                Some(pixel),
                FilterEdit::EditLastUnsharp(settings(100.0, 1.0, 0))
            ),
            Err(LockRefusal::FilterNothing)
        );
        assert!(last_unsharp(&tree, pixel).is_none());
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::AddUnsharpMask(settings(150.0, 2.0, 4))
            ),
            Some("Unsharp Mask")
        );
        assert_eq!(last_unsharp(&tree, pixel), Some(settings(150.0, 2.0, 4)));
        assert_eq!(last_radius(&tree, pixel), Some(2.0));
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::EditLastUnsharp(settings(80.0, 1.0, 0))
            ),
            Some("Edit Unsharp Mask")
        );
        assert_eq!(last_unsharp(&tree, pixel), Some(settings(80.0, 1.0, 0)));
        // Same settings again: nothing to do.
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::EditLastUnsharp(settings(80.0, 1.0, 0))
            ),
            None
        );
        // The typed-form radius edit changes only the radius.
        assert_eq!(
            run(&mut tree, FilterEdit::EditLastRadius(6.0)),
            Some("Edit Layer Filter")
        );
        assert_eq!(last_unsharp(&tree, pixel), Some(settings(80.0, 6.0, 0)));
        // The blur plus the unsharp mask share the reach cap.
        assert_eq!(
            plan(
                &tree,
                Some(pixel),
                FilterEdit::AddUnsharpMask(settings(100.0, 85.0, 0))
            ),
            Err(LockRefusal::FilterTooLarge)
        );
        assert_eq!(
            plan(
                &tree,
                Some(group),
                FilterEdit::AddUnsharpMask(settings(100.0, 1.0, 0))
            ),
            Err(LockRefusal::FilterNeedsPixelLayer)
        );
        // Undo walks back one step at a time.
        let _ = history.undo(&mut tree);
        assert_eq!(last_unsharp(&tree, pixel), Some(settings(80.0, 1.0, 0)));
        let _ = history.undo(&mut tree);
        assert_eq!(last_unsharp(&tree, pixel), Some(settings(150.0, 2.0, 4)));
        let _ = history.undo(&mut tree);
        assert!(last_unsharp(&tree, pixel).is_none());
    }

    #[test]
    fn unsharp_prompt_rows_and_command_ids() {
        assert_eq!(
            prompt_for_command(COMMAND_UNSHARP_MASK),
            Some(PromptKind::AddUnsharp)
        );
        assert_eq!(plain_edit_for_command(COMMAND_UNSHARP_MASK), None);
        for kind in [
            PromptKind::Add,
            PromptKind::EditLast,
            PromptKind::AddUnsharp,
            PromptKind::EditLastUnsharp,
        ] {
            assert_eq!(PromptKind::for_row(kind.row_id()), Some(kind));
        }
        assert_eq!(
            prompt_entries(PromptKind::AddUnsharp, "")
                .first()
                .map(|e| e.title.clone()),
            Some("Unsharp Mask".to_owned())
        );
        let row = |q: &str| {
            prompt_entries(PromptKind::AddUnsharp, q)
                .first()
                .map(|e| e.title.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            row("150 2 4"),
            "Unsharp Mask, amount 150 %, radius 2 px, threshold 4 levels"
        );
        assert_eq!(row("150 2"), "Unsharp Mask: 150 2");
    }

    fn noise(strength: f32, preserve: f32, colour: f32, sharpen: f32) -> NoiseSettings {
        NoiseSettings {
            strength,
            preserve_details: preserve,
            color_noise: colour,
            sharpen_details: sharpen,
        }
    }

    #[test]
    fn parse_noise_reads_four_numbers_and_explains_refusals() {
        assert_eq!(parse_noise("6 60 45 25"), Ok(noise(6.0, 60.0, 45.0, 25.0)));
        assert_eq!(parse_noise(" 0,0,0,0 "), Ok(noise(0.0, 0.0, 0.0, 0.0)));
        assert_eq!(
            parse_noise("10 100% 100% 100%"),
            Ok(noise(10.0, 100.0, 100.0, 100.0))
        );
        assert_eq!(
            parse_noise("2.5 50 12.5 0"),
            Ok(noise(2.5, 50.0, 12.5, 0.0))
        );
        for bad in [
            "",
            "6 60 45",
            "6 60 45 25 1",
            "10.5 60 45 25",
            "-1 60 45 25",
            "6 101 45 25",
            "6 60 100.5 25",
            "6 60 45 101",
            "a 60 45 25",
            "6 6e1 45 25",
            "6 . 45 25",
            "6 60 -5 25",
            "6% 60 45 25",
        ] {
            assert!(parse_noise(bad).is_err(), "{bad:?}");
        }
        let v = noise(7.5, 33.5, 12.0, 1.0);
        assert_eq!(parse_noise(&v.text()), Ok(v));
        assert_eq!(DEFAULT_NOISE.text(), "6 60 45 25");
    }

    #[test]
    fn noise_add_edit_and_undo_are_single_labelled_steps() {
        let (mut tree, mut history, pixel, group) = doc();
        let mut run =
            |tree: &mut aurora_doc::LayerTree, edit| apply(tree, &mut history, Some(pixel), edit);
        assert_eq!(
            run(&mut tree, FilterEdit::AddGaussianBlur(3.0)),
            Some("Gaussian Blur")
        );
        // Editing a blur as a noise filter is refused.
        assert_eq!(
            plan(
                &tree,
                Some(pixel),
                FilterEdit::EditLastNoise(noise(1.0, 1.0, 1.0, 1.0))
            ),
            Err(LockRefusal::FilterNothing)
        );
        assert!(last_noise(&tree, pixel).is_none());
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::AddReduceNoise(noise(6.0, 60.0, 45.0, 25.0))
            ),
            Some("Reduce Noise")
        );
        assert_eq!(last_noise(&tree, pixel), Some(noise(6.0, 60.0, 45.0, 25.0)));
        // It has no radius, and no unsharp settings.
        assert_eq!(last_radius(&tree, pixel), None);
        assert!(last_unsharp(&tree, pixel).is_none());
        // The typed radius form cannot change it (disclosed refusal).
        assert_eq!(
            plan(&tree, Some(pixel), FilterEdit::EditLastRadius(6.0)),
            Err(LockRefusal::FilterNothing)
        );
        // And an unsharp edit is refused on it.
        assert_eq!(
            plan(
                &tree,
                Some(pixel),
                FilterEdit::EditLastUnsharp(settings(100.0, 1.0, 0))
            ),
            Err(LockRefusal::FilterNothing)
        );
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::EditLastNoise(noise(2.0, 90.0, 0.0, 0.0))
            ),
            Some("Edit Reduce Noise")
        );
        assert_eq!(last_noise(&tree, pixel), Some(noise(2.0, 90.0, 0.0, 0.0)));
        assert_eq!(
            run(
                &mut tree,
                FilterEdit::EditLastNoise(noise(2.0, 90.0, 0.0, 0.0))
            ),
            None
        );
        // Shared reach cap: the blur 3 px (9) + this noise at the maximum
        // (20) is fine; a second blur of 85 would not be.
        assert_eq!(
            plan(&tree, Some(pixel), FilterEdit::AddGaussianBlur(85.0)),
            Err(LockRefusal::FilterTooLarge)
        );
        assert_eq!(
            plan(
                &tree,
                Some(group),
                FilterEdit::AddReduceNoise(noise(1.0, 1.0, 1.0, 1.0))
            ),
            Err(LockRefusal::FilterNeedsPixelLayer)
        );
        let _ = history.undo(&mut tree);
        assert_eq!(last_noise(&tree, pixel), Some(noise(6.0, 60.0, 45.0, 25.0)));
        let _ = history.undo(&mut tree);
        assert!(last_noise(&tree, pixel).is_none());
    }

    #[test]
    fn noise_prompt_rows_and_command_ids() {
        assert_eq!(
            prompt_for_command(COMMAND_REDUCE_NOISE),
            Some(PromptKind::AddNoise)
        );
        assert_eq!(plain_edit_for_command(COMMAND_REDUCE_NOISE), None);
        for kind in [
            PromptKind::Add,
            PromptKind::EditLast,
            PromptKind::AddUnsharp,
            PromptKind::EditLastUnsharp,
            PromptKind::AddNoise,
            PromptKind::EditLastNoise,
        ] {
            assert_eq!(PromptKind::for_row(kind.row_id()), Some(kind));
        }
        let row = |k, q: &str| {
            prompt_entries(k, q)
                .first()
                .map(|e| e.title.clone())
                .unwrap_or_default()
        };
        assert_eq!(row(PromptKind::AddNoise, ""), "Reduce Noise");
        assert_eq!(
            row(PromptKind::AddNoise, "6 60 45 25"),
            "Reduce Noise, strength 6, preserve details 60 %, colour noise 45 %, sharpen details 25 %"
        );
        assert_eq!(row(PromptKind::AddNoise, "6 60"), "Reduce Noise: 6 60");
        assert!(
            row(PromptKind::EditLastNoise, "1 2 3 4")
                .starts_with("Change the last layer filter to Reduce Noise,")
        );
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
