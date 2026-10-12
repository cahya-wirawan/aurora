//! The selection's marching ants in the running app (0.212.0): the pure
//! timing half ([`ants_step`]), which the event loop and the frame share so
//! they never disagree, and the pure command half ([`select_all`]).
//!
//! The ants march one pixel every [`MARCH_STEP`] (10 Hz, a slow fixed rate)
//! while a selection exists, the window has OS focus and reduced motion is
//! off; otherwise the phase is fixed at `0` and the loop schedules nothing
//! (the same gating the caret blink has). With no selection the step asks
//! for no wake-up at all, so an idle app keeps sleeping in `Wait`.

use std::time::{Duration, Instant};

use aurora_doc::Selection;

/// How long one marching step lasts.
pub(crate) const MARCH_STEP: Duration = Duration::from_millis(100);

/// The ants' clock: when marching started and the phase the last frame
/// drew.
#[derive(Debug)]
pub(crate) struct AntsState {
    pub(crate) epoch: Instant,
    pub(crate) drawn_phase: u32,
}

impl AntsState {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            epoch: now,
            drawn_phase: 0,
        }
    }
}

/// What [`ants_step`] decides for one frame or loop turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AntsStep {
    /// The phase to draw with now (`0` when not marching).
    pub(crate) phase: u32,
    /// When the phase next changes; `None` when it never will (no
    /// selection, reduced motion, a background window).
    pub(crate) next_tick: Option<Instant>,
    /// Whether the phase differs from the last drawn one while a
    /// selection is shown, so a frame is due.
    pub(crate) redraw: bool,
}

/// The ants' one step, from whether a selection exists, whether the window
/// has focus, the reduced-motion preference, `now`, the march `epoch` and
/// the phase the last frame `drawn`.
pub(crate) fn ants_step(
    selection_active: bool,
    window_focused: bool,
    reduced_motion: bool,
    now: Instant,
    epoch: Instant,
    drawn: u32,
) -> AntsStep {
    let marching = selection_active && window_focused && !reduced_motion;
    if !marching {
        return AntsStep {
            phase: 0,
            next_tick: None,
            redraw: selection_active && drawn != 0,
        };
    }
    let elapsed = now.saturating_duration_since(epoch);
    let step_ms = MARCH_STEP.as_millis().max(1);
    let steps = elapsed.as_millis() / step_ms;
    let period = u128::from(aurora_ui::MARCH_PERIOD);
    let phase = u32::try_from(steps % period).unwrap_or(0);
    let next_ms = (steps + 1) * step_ms;
    let next_tick = epoch.checked_add(Duration::from_millis(
        u64::try_from(next_ms).unwrap_or(u64::MAX),
    ));
    AntsStep {
        phase,
        next_tick,
        redraw: phase != drawn,
    }
}

/// Select All: the whole document, `(0, 0, w, h)`; `None` for an empty
/// document (nothing to select).
pub(crate) fn select_all(canvas_size: (u32, u32)) -> Option<Selection> {
    (canvas_size.0 > 0 && canvas_size.1 > 0).then(|| {
        Selection::new(aurora_core::Rect {
            x: 0,
            y: 0,
            width: canvas_size.0,
            height: canvas_size.1,
        })
    })
}

/// The three selection commands (0.212.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionCommand {
    SelectAll,
    Deselect,
    Inverse,
}

impl SelectionCommand {
    pub(crate) fn from_app(command: crate::AppCommand) -> Option<Self> {
        match command {
            crate::AppCommand::SelectAll => Some(Self::SelectAll),
            crate::AppCommand::Deselect => Some(Self::Deselect),
            crate::AppCommand::InverseSelection => Some(Self::Inverse),
            _ => None,
        }
    }
}

/// Runs `command` on `selection` for a document of `canvas_size`; returns
/// whether the selection changed. Inverse with nothing selected selects
/// nothing (Photoshop greys it out); Deselect of nothing changes nothing.
pub(crate) fn apply_selection_command(
    selection: &mut aurora_doc::SelectionSet,
    command: SelectionCommand,
    canvas_size: (u32, u32),
) -> bool {
    let before = selection.active();
    match command {
        SelectionCommand::SelectAll => {
            if let Some(all) = select_all(canvas_size) {
                selection.select(all);
            }
        }
        SelectionCommand::Deselect => selection.deselect(),
        SelectionCommand::Inverse => {
            let _ = selection.invert();
        }
    }
    selection.active() != before
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use aurora_core::Rect;
    use aurora_doc::{Selection, SelectionSet};
    use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
    use aurora_widgets::widgets::TextFieldChord;

    use super::{MARCH_STEP, SelectionCommand, ants_step, apply_selection_command, select_all};
    use crate::{
        ActivatedCommand, AppCommand, PRIMARY_IS_META, activate_command, default_shortcuts_for,
        palette_commands,
    };

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn primary(shift: bool, meta: bool) -> Modifiers {
        Modifiers {
            control: !meta,
            meta,
            shift,
            alt: false,
        }
    }

    #[test]
    fn the_ants_march_at_a_fixed_rate_and_wrap() {
        let epoch = Instant::now();
        let at = |ms: u64, drawn| {
            ants_step(
                true,
                true,
                false,
                epoch + Duration::from_millis(ms),
                epoch,
                drawn,
            )
        };
        assert_eq!(at(0, 0).phase, 0);
        assert!(!at(0, 0).redraw, "nothing new to draw at the epoch");
        assert_eq!(at(99, 0).phase, 0);
        assert_eq!(at(100, 0).phase, 1);
        assert!(at(100, 0).redraw);
        assert!(!at(100, 1).redraw);
        let period = u64::from(aurora_ui::MARCH_PERIOD);
        assert_eq!(at(100 * period, 0).phase, 0, "wraps");
        assert_eq!(at(100 * (period + 3), 0).phase, 3);
        // The next wake-up is the next step boundary, strictly later.
        assert_eq!(
            at(150, 1).next_tick,
            Some(epoch + Duration::from_millis(200))
        );
        assert_eq!(
            at(200, 2).next_tick,
            Some(epoch + Duration::from_millis(300))
        );
        assert_eq!(MARCH_STEP, Duration::from_millis(100));
    }

    #[test]
    fn reduced_motion_a_background_window_or_no_selection_never_march_or_wake() {
        let epoch = Instant::now();
        let later = epoch + Duration::from_secs(5);
        // Reduced motion: steady at phase 0, no timer, and a frame only to
        // settle a phase that was already drawn non-zero.
        let steady = ants_step(true, true, true, later, epoch, 0);
        assert_eq!(
            (steady.phase, steady.next_tick, steady.redraw),
            (0, None, false)
        );
        assert!(ants_step(true, true, true, later, epoch, 3).redraw);
        // A background window: paused, no timer.
        let background = ants_step(true, false, false, later, epoch, 0);
        assert_eq!((background.phase, background.next_tick), (0, None));
        // No selection: no timer, no frame, whatever else is true -- an
        // idle app keeps sleeping.
        let none = ants_step(false, true, false, later, epoch, 5);
        assert_eq!((none.phase, none.next_tick, none.redraw), (0, None, false));
        assert_eq!(
            crate::next_control_flow(later, none.next_tick, None),
            winit::event_loop::ControlFlow::Wait
        );
        // With a selection the loop wakes at the next step instead.
        let marching = ants_step(true, true, false, later, epoch, 0);
        assert!(matches!(
            crate::next_control_flow(later, marching.next_tick, None),
            winit::event_loop::ControlFlow::WaitUntil(_)
        ));
    }

    #[test]
    fn select_all_deselect_and_inverse_edit_the_selection() {
        let mut set = SelectionSet::new();
        let size = (640, 480);
        assert!(!apply_selection_command(
            &mut set,
            SelectionCommand::Inverse,
            size
        ));
        assert_eq!(set.active(), None, "inverse of nothing selects nothing");
        assert!(!apply_selection_command(
            &mut set,
            SelectionCommand::Deselect,
            size
        ));
        assert!(apply_selection_command(
            &mut set,
            SelectionCommand::SelectAll,
            size
        ));
        assert_eq!(set.active(), Some(Selection::new(rect(0, 0, 640, 480))));
        assert!(!apply_selection_command(
            &mut set,
            SelectionCommand::SelectAll,
            size
        ));
        assert!(apply_selection_command(
            &mut set,
            SelectionCommand::Inverse,
            size
        ));
        assert_eq!(set.active().map(|s| s.inverted), Some(true));
        assert!(apply_selection_command(
            &mut set,
            SelectionCommand::Inverse,
            size
        ));
        assert_eq!(set.active().map(|s| s.inverted), Some(false));
        assert!(apply_selection_command(
            &mut set,
            SelectionCommand::Deselect,
            size
        ));
        assert_eq!(set.active(), None);
        assert_eq!(
            select_all((0, 10)),
            None,
            "an empty document selects nothing"
        );
    }

    #[test]
    fn the_shortcuts_resolve_on_both_platform_tables_without_conflicts() {
        for meta in [false, true] {
            let table = default_shortcuts_for(meta);
            for (ch, shift, command) in [
                ('a', false, AppCommand::SelectAll),
                ('d', false, AppCommand::Deselect),
                ('i', true, AppCommand::InverseSelection),
            ] {
                let chord = KeyChord::new(primary(shift, meta), Key::Character(ch));
                assert_eq!(table.resolve(chord), Some(&command), "meta {meta} {ch}");
            }
            // The bare `d` is still Photoshop's default-colours key.
            assert_eq!(
                table.resolve(KeyChord::new(Modifiers::none(), Key::Character('d'))),
                Some(&AppCommand::ResetColours)
            );
        }
    }

    #[test]
    fn a_focused_text_field_claims_select_all_before_the_registry() {
        let chord = TextFieldChord::from_key(
            &Key::Character('a'),
            primary(false, PRIMARY_IS_META),
            PRIMARY_IS_META,
        );
        assert_eq!(chord, Some(TextFieldChord::SelectAll));
    }

    #[test]
    fn the_palette_lists_and_runs_the_three_commands() {
        let commands = palette_commands();
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        let mut focus = aurora_widgets::FocusManager::default();
        for (title, expected) in [
            ("Select All", SelectionCommand::SelectAll),
            ("Deselect", SelectionCommand::Deselect),
            ("Inverse Selection", SelectionCommand::Inverse),
        ] {
            let Some(entry) = commands.iter().find(|c| c.title == title) else {
                unreachable!("{title} is in the palette");
            };
            let activated = activate_command(
                &mut workspace,
                &mut focus,
                &entry.id,
                &mut crate::tests::FakeFileDialog::default(),
            );
            assert!(
                matches!(activated, Some(ActivatedCommand::Selection(c)) if c == expected),
                "{title}: {activated:?}"
            );
        }
    }
}
