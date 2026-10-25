//! Selection modes for the selection tools (0.215.0): New, Add, Subtract
//! and Intersect, the document half. Pure functions over the live
//! [`SelectionSet`] and the gesture's state (never `App`), so a test
//! drives them without a window; `lib.rs` owns the pointer plumbing.
//!
//! **Which mode.** Decided once, at the press, and kept for the whole
//! gesture: the options bar's mode button when it is not New, otherwise the
//! modifiers held at the press (Photoshop's): `Shift` adds, `Alt` subtracts,
//! `Shift`+`Alt` intersects. Releasing or pressing a key mid-drag changes
//! nothing.
//!
//! **A gesture.** In New mode a press deselects at once, as before. In any
//! other mode the selection in force at the press (`base`) stays visible,
//! and every live update shows `combine(base, shape, mode)`, where `shape`
//! is what the tool has drawn so far ([`aurora_doc::combine_selections`]);
//! a combination the mask cap refuses leaves the last shown selection in
//! place. The recorded step runs from the selection at the press to the
//! combined one, so one undo restores exactly what was there. A click or
//! zero-area drag adds, subtracts or intersects nothing, so it changes
//! nothing in the combining modes (in New mode it deselects, as before).

use aurora_doc::{Selection, SelectionMode, SelectionSet, combine_selections};
use aurora_widgets::shortcut::Modifiers;

/// The mode the modifiers held at a press ask for: none = New, `Shift` =
/// Add, `Alt` = Subtract, both = Intersect.
pub(crate) const fn mode_from_modifiers(modifiers: Modifiers) -> SelectionMode {
    match (modifiers.shift, modifiers.alt) {
        (false, false) => SelectionMode::Replace,
        (true, false) => SelectionMode::Add,
        (false, true) => SelectionMode::Subtract,
        (true, true) => SelectionMode::Intersect,
    }
}

/// The mode of a gesture: the options bar's `button` when it is not New
/// (it overrides the modifiers), otherwise the modifiers' own.
pub(crate) const fn resolve_mode(button: SelectionMode, modifiers: Modifiers) -> SelectionMode {
    match button {
        SelectionMode::Replace => mode_from_modifiers(modifiers),
        other => other,
    }
}

/// The mode of a gesture pressed with `modifiers` under the app's tool
/// `settings`: the one place the options bar's button and the modifiers
/// meet for the Marquees, the Lassos, the Magic Wand and (0.238.0) the Quick
/// Select.
pub(crate) const fn gesture_mode(
    settings: &crate::ToolSettings,
    modifiers: Modifiers,
) -> SelectionMode {
    resolve_mode(settings.selection_mode, modifiers)
}

/// The mode and starting selection of one selection gesture.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModeGesture {
    /// How the tool's shape combines with [`Self::base`].
    pub mode: SelectionMode,
    /// The selection in force at the press (`None`: nothing). Only
    /// consulted when `mode` is not New.
    pub base: Option<Selection>,
    /// A combination the mask cap refused during this gesture (0.216.0);
    /// latched. [`Self::take_refusal`] turns it into one notice.
    pub refused: std::cell::Cell<bool>,
    /// Whether the notice for [`Self::refused`] was already given.
    pub notified: std::cell::Cell<bool>,
    /// After a slow live combination, the time before which the next one
    /// is skipped (0.216.0; the Lasso's back-off, [`crate::lasso_tools`]).
    pub not_before: Option<std::time::Instant>,
    /// The newest shape a skipped update left unplaced; the release
    /// places it exactly ([`release_marquee`]).
    pub pending: Option<Selection>,
}

impl ModeGesture {
    /// A New-mode gesture: nothing to combine with.
    pub(crate) const fn replace() -> Self {
        Self {
            mode: SelectionMode::Replace,
            base: None,
            refused: std::cell::Cell::new(false),
            notified: std::cell::Cell::new(false),
            not_before: None,
            pending: None,
        }
    }

    /// [`Self::replace`], boxed for a `Drag` field (it keeps that enum's
    /// variants close in size).
    pub(crate) fn boxed_replace() -> Box<Self> {
        Box::new(Self::replace())
    }

    /// The selection that `shape` (what the tool has drawn; `None`
    /// for nothing) makes under this gesture's mode, or `Err` when the
    /// mask cap refuses the combination.
    ///
    /// # Errors
    ///
    /// [`aurora_doc::DocError::SelectionMaskTooLarge`].
    pub(crate) fn result(
        &self,
        shape: Option<&Selection>,
    ) -> Result<Option<Selection>, aurora_doc::DocError> {
        match (self.mode, shape) {
            (SelectionMode::Replace, shape) => Ok(shape.cloned()),
            // Nothing drawn yet adds, subtracts and intersects nothing.
            (_, None) => Ok(self.base.clone()),
            (mode, Some(shape)) => combine_selections(self.base.as_ref(), shape, mode),
        }
    }

    /// Puts [`Self::result`] of `shape` into `selection`. A refused
    /// combination leaves the selection as it was and returns `false`.
    pub(crate) fn place(&self, selection: &mut SelectionSet, shape: Option<&Selection>) -> bool {
        match self.result(shape) {
            Ok(Some(next)) => selection.select(next),
            Ok(None) => selection.deselect(),
            Err(err) => {
                tracing::debug!(
                    ?err,
                    "a combined selection is over the cap; the selection is unchanged"
                );
                self.refused.set(true);
                return false;
            }
        }
        true
    }
}

impl ModeGesture {
    /// One live update of a Marquee or Ellipse gesture (0.216.0): in a
    /// combining mode the combination is recomputed per pointer move, so
    /// after one slower than [`crate::lasso_tools::SLOW_REBUILD`] the next
    /// updates wait (the Lasso's back-off) and the newest shape is kept in
    /// [`Self::pending`] for the release. `now` is when the update began.
    /// Returns whether the selection was updated.
    pub(crate) fn place_throttled(
        &mut self,
        selection: &mut SelectionSet,
        shape: Selection,
        now: std::time::Instant,
    ) -> bool {
        if self.mode == SelectionMode::Replace {
            self.pending = None;
            return self.place(selection, Some(&shape));
        }
        if self.not_before.is_some_and(|t| now < t) {
            self.pending = Some(shape);
            return false;
        }
        let placed = self.place(selection, Some(&shape));
        self.pending = None;
        self.not_before = crate::lasso_tools::backoff_until(now, now.elapsed());
        placed
    }

    /// Whether a refused combination still needs its notice, once per
    /// gesture.
    pub(crate) fn take_refusal(&self) -> bool {
        if self.refused.get() && !self.notified.get() {
            self.notified.set(true);
            true
        } else {
            false
        }
    }
}

/// [`ModeGesture::take_refusal`] of the gesture a live drag carries.
pub(crate) fn drag_refusal(drag: Option<&crate::Drag>) -> bool {
    match drag {
        Some(crate::Drag::Marquee { modal, .. } | crate::Drag::Lasso { modal, .. }) => {
            modal.take_refusal()
        }
        Some(crate::Drag::QuickSelect(quick)) => quick.modal.take_refusal(),
        _ => false,
    }
}

/// A Marquee release: the newest shape a throttled update skipped is
/// placed now, so the finished selection is exact whatever the back-off
/// did mid-drag.
pub(crate) fn release_marquee(ending: &mut Option<crate::Drag>, selection: &mut SelectionSet) {
    if let Some(crate::Drag::Marquee { current, modal, .. }) = ending.as_mut()
        && let Some(shape) = modal.pending.take()
        && modal.place(selection, Some(&shape))
    {
        *current = selection.active();
    }
}

/// A selection press: in New mode deselects at once (a click deselects,
/// including a click on an existing selection); in any other mode keeps the
/// selection visible. Returns the gesture and what was selected, for the
/// step's "before" and for Escape.
pub(crate) fn press_selection(
    selection: &mut SelectionSet,
    mode: SelectionMode,
) -> (ModeGesture, Option<Selection>) {
    if mode == SelectionMode::Replace {
        let before = crate::selection_view::press_marquee(selection);
        (ModeGesture::replace(), before)
    } else {
        let before = selection.active();
        (
            ModeGesture {
                mode,
                base: before.clone(),
                ..ModeGesture::replace()
            },
            before,
        )
    }
}

/// The selection a gesture's `current` starts as: nothing in New mode (the
/// press deselected), the base otherwise (nothing changed yet, so a
/// release with no movement records no step).
pub(crate) fn initial_current(gesture: &ModeGesture) -> Option<Selection> {
    match gesture.mode {
        SelectionMode::Replace => None,
        _ => gesture.base.clone(),
    }
}

/// The History row of a combining gesture; `None` in New mode (the tool's
/// own label stands).
pub(crate) const fn mode_step_label(mode: SelectionMode) -> Option<&'static str> {
    match mode {
        SelectionMode::Replace => None,
        SelectionMode::Add => Some("Add to Selection"),
        SelectionMode::Subtract => Some("Subtract from Selection"),
        SelectionMode::Intersect => Some("Intersect Selection"),
    }
}

/// The History row of a finished gesture: the combining label in a
/// combining mode, else `tool_label` (the tool's own, or "Deselect" when
/// it selected nothing).
pub(crate) fn step_label(mode: SelectionMode, tool_label: &'static str) -> &'static str {
    mode_step_label(mode).unwrap_or(tool_label)
}

/// The palette's row for choosing a mode.
#[cfg(any(test, target_os = "macos"))]
pub(crate) const fn palette_label(mode: SelectionMode) -> &'static str {
    match mode {
        SelectionMode::Replace => "Selection Mode: New",
        SelectionMode::Add => "Selection Mode: Add",
        SelectionMode::Subtract => "Selection Mode: Subtract",
        SelectionMode::Intersect => "Selection Mode: Intersect",
    }
}

/// A Marquee press as the app runs it: begins the selection gesture in
/// `mode` on the live `Drag::Marquee` (see [`press_selection`]).
pub(crate) fn press_marquee_drag(
    drag: &mut Option<crate::Drag>,
    selection: &mut SelectionSet,
    mode: SelectionMode,
) {
    let (gesture, began_with) = press_selection(selection, mode);
    if let Some(crate::Drag::Marquee {
        before,
        current,
        modal,
        ..
    }) = drag.as_mut()
    {
        *before = began_with;
        *current = initial_current(&gesture);
        **modal = gesture;
    }
}

#[cfg(test)]
mod tests {
    use aurora_doc::SelectionMode;
    use aurora_widgets::shortcut::Modifiers;

    use super::gesture_mode;

    fn mods(shift: bool, alt: bool) -> Modifiers {
        Modifiers {
            shift,
            alt,
            control: false,
            meta: false,
        }
    }

    #[test]
    fn the_gesture_mode_is_the_button_unless_it_is_new() {
        let mut settings = crate::ToolSettings::default();
        assert_eq!(
            gesture_mode(&settings, mods(true, false)),
            SelectionMode::Add
        );
        assert_eq!(
            gesture_mode(&settings, mods(false, false)),
            SelectionMode::Replace
        );
        settings.selection_mode = SelectionMode::Subtract;
        assert_eq!(
            gesture_mode(&settings, mods(true, true)),
            SelectionMode::Subtract
        );
        assert_eq!(
            gesture_mode(&settings, mods(false, false)),
            SelectionMode::Subtract
        );
    }

    /// `App::handle_pointer_pressed` and `App::select_wand` need a window,
    /// so this checks (one line at a time) that each of the four press
    /// paths - the Lassos, the Marquees, the Magic Wand and the Quick
    /// Select - asks [`gesture_mode`] for its mode.
    #[test]
    fn the_four_press_paths_take_their_mode_from_gesture_mode() {
        let source = include_str!("lib.rs");
        let call = [
            "selection_modes::gesture_mode(",
            "&self.tool_settings, self.modifiers)",
        ]
        .concat();
        let uses = source.lines().filter(|l| l.contains(&call)).count();
        assert_eq!(uses, 4);
    }
}
