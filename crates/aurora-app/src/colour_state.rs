//! The foreground/background colours (0.208.0): Photoshop's pair, the
//! Brush's paint colour and the Eyedropper's destination.
//!
//! **Not document state.** Changing, swapping or resetting a colour
//! records no undo step, adds no History row and never makes a document
//! dirty; the pair is not saved in a `.aur` file and **is not persisted
//! across sessions** (every launch starts black over white), the same
//! status [`crate::ToolSettings`] has. Colours are straight, encoded
//! sRGB `[r, g, b]` in `[0, 1]` (the space the picker, the swatches and
//! the Brush all work in: no colour management exists yet); there is no
//! alpha, because the Brush has none.

use aurora_theme::Scales;
use aurora_ui::{ColourAction, ColourPopover, ColourTarget};
use aurora_widgets::widgets::{self, ColorPickerState};
use aurora_widgets::{ActionOutcome, FocusManager, PointerOutcome};

/// The initial foreground: black — what `App::current_colour` started as
/// before 0.208.0, so a fresh app paints the same pixels.
pub(crate) const DEFAULT_FOREGROUND: [f32; 3] = [0.0, 0.0, 0.0];

/// The initial background: white.
pub(crate) const DEFAULT_BACKGROUND: [f32; 3] = [1.0, 1.0, 1.0];

/// Whether `rgb` is a colour the state accepts: every channel finite and
/// in `[0, 1]`.
pub(crate) fn is_valid_colour(rgb: [f32; 3]) -> bool {
    rgb.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c))
}

/// The foreground and background colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColourState {
    foreground: [f32; 3],
    background: [f32; 3],
}

impl Default for ColourState {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
        }
    }
}

impl ColourState {
    /// The foreground: what the Brush paints with.
    pub(crate) const fn foreground(&self) -> [f32; 3] {
        self.foreground
    }

    /// The colour the Brush paints with: the foreground, always.
    pub(crate) const fn paint_colour(&self) -> [f32; 3] {
        self.foreground
    }

    /// The background.
    pub(crate) const fn background(&self) -> [f32; 3] {
        self.background
    }

    /// `target`'s colour.
    pub(crate) const fn get(&self, target: ColourTarget) -> [f32; 3] {
        match target {
            ColourTarget::Foreground => self.foreground,
            ColourTarget::Background => self.background,
        }
    }

    /// Sets `target`'s colour. Refuses (returns `false`, changing nothing)
    /// a colour with a non-finite or out-of-`[0, 1]` channel; otherwise
    /// returns whether the colour changed (bit-for-bit).
    pub(crate) fn set(&mut self, target: ColourTarget, rgb: [f32; 3]) -> bool {
        if !is_valid_colour(rgb) {
            return false;
        }
        let slot = match target {
            ColourTarget::Foreground => &mut self.foreground,
            ColourTarget::Background => &mut self.background,
        };
        if slot.map(f32::to_bits) == rgb.map(f32::to_bits) {
            return false;
        }
        *slot = rgb;
        true
    }

    /// Exchanges the two colours. Returns whether anything changed (not
    /// when they are equal).
    pub(crate) fn swap(&mut self) -> bool {
        let changed = self.foreground.map(f32::to_bits) != self.background.map(f32::to_bits);
        std::mem::swap(&mut self.foreground, &mut self.background);
        changed
    }

    /// Black over white. Returns whether anything changed.
    pub(crate) fn reset(&mut self) -> bool {
        let defaults = Self::default();
        let changed = *self != defaults;
        *self = defaults;
        changed
    }
}

/// Which colour an Eyedropper pick sets: the foreground, or the
/// background while `Alt` (`Option` on macOS) is held — Photoshop's
/// convention.
pub(crate) fn eyedropper_target(modifiers: aurora_widgets::shortcut::Modifiers) -> ColourTarget {
    if modifiers.alt {
        ColourTarget::Background
    } else {
        ColourTarget::Foreground
    }
}

/// The colour a `tool` dab is stamped with: the foreground for the Brush
/// (the one place a dab site reads a colour, so none can pick the
/// background); the Eraser takes the same value and ignores it.
pub(crate) fn dab_colour(_tool: crate::DabTool, state: &ColourState) -> [f32; 3] {
    state.paint_colour()
}

/// Whether `key` closes the colour popover: `Escape` while it is open and
/// no dialog or the command palette (which own the key) is.
pub(crate) fn escape_closes_popover(
    key: aurora_widgets::shortcut::Key,
    popover_open: bool,
    dialog_open: bool,
    palette_open: bool,
) -> bool {
    key == aurora_widgets::shortcut::Key::Named(aurora_widgets::shortcut::NamedKey::Escape)
        && popover_open
        && !dialog_open
        && !palette_open
}

/// What a [`ColourController`] call changed in the tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Effect {
    /// Something the screen reader or the frame shows changed.
    pub changed: bool,
    /// The tree's structure changed (a popover opened or closed): lay out
    /// again.
    pub relayout: bool,
}

impl Effect {
    const NONE: Effect = Effect {
        changed: false,
        relayout: false,
    };
    const CHANGED: Effect = Effect {
        changed: true,
        relayout: false,
    };
    const RELAYOUT: Effect = Effect {
        changed: true,
        relayout: true,
    };
}

/// The colour state, the Tools panel's swatches and the picker popover
/// together (0.208.0): everything the colour controls do that does not
/// need a window, so it is tested headlessly.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ColourController {
    pub state: ColourState,
    pub popover: Option<ColourPopover>,
}

impl ColourController {
    /// Mirrors the state into the swatches and an open picker.
    pub(crate) fn sync(&mut self, workspace: &mut aurora_ui::Workspace) -> Effect {
        let controls = workspace.tools.colours;
        if let Some(open) = self.popover
            && !workspace.tree.contains(open.frame)
        {
            self.popover = None;
        }
        let mut changed = match aurora_ui::sync_colour_controls(
            &mut workspace.tree,
            &controls,
            self.state.foreground(),
            self.state.background(),
        ) {
            Ok(changed) => changed,
            Err(err) => {
                tracing::warn!(?err, "failed to sync the colour swatches");
                false
            }
        };
        if let Some(open) = self.popover {
            // The picker follows a swap/reset/eyedropper pick; a value it
            // just reported itself rounds to the same 8-bit colour, so
            // its own drag is never rewritten under the pointer.
            let wanted = aurora_ui::swatch_colour(self.state.get(open.target));
            let shown = widgets::color_picker_state(&workspace.tree, open.picker)
                .map(ColorPickerState::color);
            if shown.is_ok_and(|shown| shown != wanted) {
                match widgets::set_color_picker_color(&mut workspace.tree, open.picker, wanted) {
                    Ok(_) => changed = true,
                    Err(err) => tracing::warn!(?err, "failed to sync the colour picker"),
                }
            }
        }
        if changed {
            Effect::CHANGED
        } else {
            Effect::NONE
        }
    }

    pub(crate) fn set(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        target: ColourTarget,
        rgb: [f32; 3],
    ) -> Effect {
        if self.state.set(target, rgb) {
            self.sync(workspace)
        } else {
            Effect::NONE
        }
    }

    /// An Eyedropper pick of `rgb` with `modifiers` held.
    pub(crate) fn eyedropper_pick(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        modifiers: aurora_widgets::shortcut::Modifiers,
        rgb: [f32; 3],
    ) -> Effect {
        // The composite can hold out-of-range values (f16 overshoot, a
        // tiny negative): clamp finite channels into 0..1 so a pick is
        // never silently refused; only a non-finite channel is.
        if !rgb.iter().all(|c| c.is_finite()) {
            return Effect::NONE;
        }
        let rgb = rgb.map(|c| c.clamp(0.0, 1.0));
        self.set(workspace, eyedropper_target(modifiers), rgb)
    }

    pub(crate) fn swap(&mut self, workspace: &mut aurora_ui::Workspace) -> Effect {
        if self.state.swap() {
            self.sync(workspace)
        } else {
            Effect::NONE
        }
    }

    pub(crate) fn reset(&mut self, workspace: &mut aurora_ui::Workspace) -> Effect {
        if self.state.reset() {
            self.sync(workspace)
        } else {
            Effect::NONE
        }
    }

    /// What a widget outcome on a colour control means: a swatch opens or
    /// closes its picker, the buttons swap or reset, a picker change
    /// edits the colour it was opened for.
    pub(crate) fn apply_outcome(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
        outcome: &PointerOutcome,
    ) -> Effect {
        let PointerOutcome::Action(action) = outcome else {
            return Effect::NONE;
        };
        let controls = workspace.tools.colours;
        match action {
            ActionOutcome::Activated(id) => match controls.action_for(*id) {
                Some(ColourAction::Edit(target)) => {
                    self.toggle_popover(workspace, focus, scales, target)
                }
                Some(ColourAction::Swap) => self.swap(workspace),
                Some(ColourAction::Reset) => self.reset(workspace),
                None => Effect::NONE,
            },
            ActionOutcome::ColorChanged { picker, hsv, .. } => {
                self.picked(workspace, *picker, hsv.to_srgb_f32())
            }
            _ => Effect::NONE,
        }
    }

    /// The open picker `picker` reported `rgb`: edit the colour it was
    /// opened for. A report from any other picker is ignored.
    pub(crate) fn picked(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        picker: aurora_widgets::WidgetId,
        rgb: [f32; 3],
    ) -> Effect {
        match self.popover {
            Some(open) if open.picker == picker => self.set(workspace, open.target, rgb),
            _ => Effect::NONE,
        }
    }

    /// An assistive technology's action on a colour control, as the app
    /// receives it ([`crate::ColourEdit`]).
    pub(crate) fn apply_edit(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
        edit: crate::ColourEdit,
    ) -> Effect {
        match edit {
            crate::ColourEdit::Activated(id) => self.apply_outcome(
                workspace,
                focus,
                scales,
                &PointerOutcome::Action(ActionOutcome::Activated(id)),
            ),
            crate::ColourEdit::Picked(picker, bits) => {
                self.picked(workspace, picker, bits.map(f32::from_bits))
            }
        }
    }

    /// A swatch was activated: open its picker, close it if it is the one
    /// open, or move the picker over to the other colour.
    pub(crate) fn toggle_popover(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        focus: &mut FocusManager,
        scales: &Scales,
        target: ColourTarget,
    ) -> Effect {
        let reopen = self.popover.map(|open| open.target) != Some(target);
        let closed = self.close_popover(workspace, focus, false);
        let controls = workspace.tools.colours;
        if !reopen {
            // Closing by the same swatch: focus returns to it.
            let _ = focus.focus(&mut workspace.tree, controls.swatch(target));
            return closed;
        }
        match aurora_ui::open_colour_popover(
            &mut workspace.tree,
            &controls,
            scales,
            target,
            self.state.get(target),
        ) {
            Ok(open) => {
                self.popover = Some(open);
                if let Ok(picker) = widgets::color_picker_state(&workspace.tree, open.picker)
                    && let Some(part) = picker.focus_target()
                {
                    let _ = focus.focus(&mut workspace.tree, part);
                }
                Effect::RELAYOUT
            }
            Err(err) => {
                tracing::warn!(?err, "failed to open the colour picker");
                closed
            }
        }
    }

    /// Closes the popover (a no-op if none is open); with `restore_focus`
    /// the focus goes back to the swatch it edited.
    pub(crate) fn close_popover(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        focus: &mut FocusManager,
        restore_focus: bool,
    ) -> Effect {
        let Some(open) = self.popover.take() else {
            return Effect::NONE;
        };
        if let Err(err) = aurora_ui::close_colour_popover(&mut workspace.tree, &open) {
            tracing::warn!(?err, "failed to close the colour picker");
        }
        focus.validate(&workspace.tree);
        if restore_focus {
            let swatch = workspace.tools.colours.swatch(open.target);
            let _ = focus.focus(&mut workspace.tree, swatch);
        }
        Effect::RELAYOUT
    }

    /// A press outside the popover (and not on a swatch, which toggles it
    /// itself) closes it; the press then goes on as usual.
    pub(crate) fn light_dismiss(
        &mut self,
        workspace: &mut aurora_ui::Workspace,
        focus: &mut FocusManager,
        position: (f32, f32),
    ) -> Effect {
        let Some(open) = self.popover else {
            return Effect::NONE;
        };
        let controls = workspace.tools.colours;
        let inside = workspace.tree.hit_test(position).is_some_and(|hit| {
            open.contains(&workspace.tree, hit) || controls.target_of(hit).is_some()
        });
        if inside {
            Effect::NONE
        } else {
            self.close_popover(workspace, focus, false)
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact constants and exact copies, not computed values
mod tests {
    use super::*;

    #[test]
    fn a_fresh_state_is_black_over_white() {
        let state = ColourState::default();
        assert_eq!(state.foreground(), [0.0, 0.0, 0.0]);
        assert_eq!(state.background(), [1.0, 1.0, 1.0]);
        assert_eq!(state.get(ColourTarget::Foreground), [0.0; 3]);
        assert_eq!(state.get(ColourTarget::Background), [1.0; 3]);
    }

    #[test]
    fn swap_exchanges_and_is_an_involution() {
        let mut state = ColourState::default();
        assert!(state.set(ColourTarget::Foreground, [0.25, 0.5, 0.75]));
        assert!(state.swap());
        assert_eq!(state.foreground(), [1.0, 1.0, 1.0]);
        assert_eq!(state.background(), [0.25, 0.5, 0.75]);
        assert!(state.swap());
        assert_eq!(state.foreground(), [0.25, 0.5, 0.75]);
        assert_eq!(state.background(), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn swapping_equal_colours_changes_nothing() {
        let mut state = ColourState::default();
        assert!(state.set(ColourTarget::Foreground, [1.0; 3]));
        assert!(!state.swap());
    }

    #[test]
    fn reset_restores_black_over_white_and_reports_change() {
        let mut state = ColourState::default();
        assert!(!state.reset(), "already default");
        assert!(state.set(ColourTarget::Foreground, [0.1, 0.2, 0.3]));
        assert!(state.set(ColourTarget::Background, [0.9, 0.8, 0.7]));
        assert!(state.reset());
        assert_eq!(state, ColourState::default());
    }

    #[test]
    fn set_edits_only_its_target() {
        let mut state = ColourState::default();
        assert!(state.set(ColourTarget::Background, [0.5, 0.0, 1.0]));
        assert_eq!(state.foreground(), DEFAULT_FOREGROUND);
        assert_eq!(state.background(), [0.5, 0.0, 1.0]);
        assert!(!state.set(ColourTarget::Background, [0.5, 0.0, 1.0]));
        assert!(state.set(ColourTarget::Foreground, [0.0, 1.0, 0.0]));
        assert_eq!(state.background(), [0.5, 0.0, 1.0]);
    }

    #[test]
    fn invalid_colours_are_refused_and_change_nothing() {
        let mut state = ColourState::default();
        for bad in [
            [f32::NAN, 0.0, 0.0],
            [0.0, f32::INFINITY, 0.0],
            [0.0, 0.0, f32::NEG_INFINITY],
            [-0.001, 0.0, 0.0],
            [0.0, 1.001, 0.0],
        ] {
            assert!(!is_valid_colour(bad), "{bad:?}");
            assert!(!state.set(ColourTarget::Foreground, bad), "{bad:?}");
            assert!(!state.set(ColourTarget::Background, bad), "{bad:?}");
        }
        assert_eq!(state, ColourState::default());
        assert!(is_valid_colour([0.0, 0.5, 1.0]));
    }
}
