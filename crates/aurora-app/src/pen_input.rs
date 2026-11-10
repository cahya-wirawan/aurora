//! Pen and touch input (task 169): the pure parts.
//!
//! winit 0.30 delivers a pen on Windows (and fingers on Windows, iOS,
//! Android) as `WindowEvent::Touch`, with an optional [`Force`]. This
//! module maps those fields to [`PenSample`]s and decides which source owns
//! the current gesture. It touches no window, so it is tested headlessly;
//! `App::handle_touch` is the thin glue that feeds the result to the same
//! press/move/release handlers the mouse uses.
//!
//! What a `Force` honestly gives: pressure always; for `Calibrated` with an
//! `altitude_angle`, the tilt *magnitude* (90 degrees minus the altitude)
//! but no azimuth, so it is reported on `tilt_x` alone. No rotation.

use aurora_core::{PenSample, PointerSource};
use winit::event::{Force, TouchPhase};

/// What one touch event means for the stroke path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TouchAction {
    /// Start a stroke at the sample.
    Press(PenSample),
    /// Move the active stroke.
    Move(PenSample),
    /// End the stroke at the sample.
    Release(PenSample),
    /// The system took the touch away: interrupt the gesture.
    Cancel,
    /// Not ours: another source or another finger owns the gesture.
    Ignore,
}

/// Normalised pressure `0..=1` of a force; `None` (no pressure axis) is 1.
pub(crate) fn pressure_of(force: Option<Force>) -> f32 {
    let value = match force {
        Some(Force::Normalized(f)) => f,
        Some(Force::Calibrated {
            force,
            max_possible_force,
            ..
        }) if max_possible_force.is_finite() && max_possible_force > 0.0 => {
            force / max_possible_force
        }
        None | Some(Force::Calibrated { .. }) => return 1.0,
    };
    if value.is_finite() {
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = value.clamp(0.0, 1.0) as f32;
        narrowed
    } else {
        1.0
    }
}

/// The sample for a touch at `location` (physical pixels).
pub(crate) fn touch_sample(location: (f64, f64), force: Option<Force>) -> PenSample {
    let tilt_x = match force {
        Some(Force::Calibrated {
            altitude_angle: Some(altitude),
            ..
        }) if altitude.is_finite() => {
            #[allow(clippy::cast_possible_truncation)]
            let degrees = (90.0 - altitude.to_degrees()) as f32;
            degrees
        }
        _ => 0.0,
    };
    // A pen is indistinguishable from a finger here, so the source is Touch.
    PenSample::new(
        location,
        pressure_of(force),
        (tilt_x, 0.0),
        0.0,
        PointerSource::Touch,
    )
}

/// Which source owns the gesture: one mouse button at most, or one touch.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PointerGate {
    mouse_down: bool,
    touch: Option<u64>,
}

impl PointerGate {
    /// Whether a touch is the active source.
    #[cfg(test)]
    pub(crate) fn touch_active(&self) -> bool {
        self.touch.is_some()
    }

    /// A mouse press: refused while a touch is active.
    pub(crate) fn mouse_pressed(&mut self) -> bool {
        if self.touch.is_some() {
            return false;
        }
        self.mouse_down = true;
        true
    }

    /// A mouse move or release: refused while a touch is active.
    pub(crate) fn mouse_allowed(&self) -> bool {
        self.touch.is_none()
    }

    /// A mouse release that was allowed.
    pub(crate) fn mouse_released(&mut self) {
        self.mouse_down = false;
    }

    /// Classifies one touch event.
    pub(crate) fn touch(
        &mut self,
        phase: TouchPhase,
        id: u64,
        location: (f64, f64),
        force: Option<Force>,
    ) -> TouchAction {
        let sample = touch_sample(location, force);
        match (phase, self.touch) {
            (TouchPhase::Started, None) if !self.mouse_down => {
                self.touch = Some(id);
                TouchAction::Press(sample)
            }
            (TouchPhase::Moved, Some(active)) if active == id => TouchAction::Move(sample),
            (TouchPhase::Ended, Some(active)) if active == id => {
                self.touch = None;
                TouchAction::Release(sample)
            }
            (TouchPhase::Cancelled, Some(active)) if active == id => {
                self.touch = None;
                TouchAction::Cancel
            }
            _ => TouchAction::Ignore,
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps)]
    fn cal(force: f64, max: f64, alt: Option<f64>) -> Option<Force> {
        Some(Force::Calibrated {
            force,
            max_possible_force: max,
            altitude_angle: alt,
        })
    }

    #[test]
    fn pressure_for_every_force_variant() {
        assert_eq!(pressure_of(None), 1.0);
        assert_eq!(pressure_of(Some(Force::Normalized(0.25))), 0.25);
        assert_eq!(pressure_of(Some(Force::Normalized(3.0))), 1.0);
        assert_eq!(pressure_of(Some(Force::Normalized(-1.0))), 0.0);
        assert_eq!(pressure_of(Some(Force::Normalized(f64::NAN))), 1.0);
        assert_eq!(pressure_of(cal(2.0, 8.0, None)), 0.25);
        assert_eq!(pressure_of(cal(2.0, 0.0, None)), 1.0);
        assert_eq!(pressure_of(cal(2.0, f64::INFINITY, None)), 1.0);
        assert_eq!(pressure_of(cal(f64::NAN, 4.0, None)), 1.0);
    }

    #[test]
    fn tilt_comes_from_altitude_only() {
        let upright = touch_sample((1.0, 2.0), cal(1.0, 1.0, Some(std::f64::consts::FRAC_PI_2)));
        assert!(upright.tilt_x.abs() < 1e-4);
        let flat = touch_sample((1.0, 2.0), cal(1.0, 1.0, Some(0.0)));
        assert!((flat.tilt_x - 90.0).abs() < 1e-4);
        assert_eq!(flat.tilt_y, 0.0);
        assert_eq!(flat.rotation, 0.0);
        assert_eq!(flat.source, PointerSource::Touch);
        assert_eq!(touch_sample((0.0, 0.0), None).tilt_x, 0.0);
        let nan = touch_sample((0.0, 0.0), cal(1.0, 1.0, Some(f64::NAN)));
        assert_eq!(nan.tilt_x, 0.0);
    }

    #[test]
    fn phases_map_to_press_move_release() {
        let mut gate = PointerGate::default();
        let at = (10.0, 20.0);
        let f = Some(Force::Normalized(0.5));
        assert!(matches!(
            gate.touch(TouchPhase::Started, 7, at, f),
            TouchAction::Press(s) if s.pressure == 0.5 && (s.x, s.y) == at
        ));
        assert!(gate.touch_active());
        assert!(matches!(
            gate.touch(TouchPhase::Moved, 7, at, f),
            TouchAction::Move(_)
        ));
        assert!(matches!(
            gate.touch(TouchPhase::Ended, 7, at, f),
            TouchAction::Release(_)
        ));
        assert!(!gate.touch_active());
    }

    #[test]
    fn cancelled_cancels_and_frees_the_gate() {
        let mut gate = PointerGate::default();
        gate.touch(TouchPhase::Started, 1, (0.0, 0.0), None);
        assert_eq!(
            gate.touch(TouchPhase::Cancelled, 1, (0.0, 0.0), None),
            TouchAction::Cancel
        );
        assert!(!gate.touch_active());
        // A cancel with nothing active is ignored.
        assert_eq!(
            gate.touch(TouchPhase::Cancelled, 1, (0.0, 0.0), None),
            TouchAction::Ignore
        );
    }

    #[test]
    fn only_one_touch_is_active() {
        let mut gate = PointerGate::default();
        gate.touch(TouchPhase::Started, 1, (0.0, 0.0), None);
        for phase in [
            TouchPhase::Started,
            TouchPhase::Moved,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            assert_eq!(gate.touch(phase, 2, (5.0, 5.0), None), TouchAction::Ignore);
        }
        assert!(gate.touch_active());
        // Strays without a start are ignored too.
        let mut idle = PointerGate::default();
        assert_eq!(
            idle.touch(TouchPhase::Moved, 1, (0.0, 0.0), None),
            TouchAction::Ignore
        );
    }

    #[test]
    fn touch_and_mouse_exclude_each_other() {
        let mut gate = PointerGate::default();
        assert!(gate.mouse_pressed());
        assert_eq!(
            gate.touch(TouchPhase::Started, 1, (0.0, 0.0), None),
            TouchAction::Ignore
        );
        gate.mouse_released();
        assert!(matches!(
            gate.touch(TouchPhase::Started, 1, (0.0, 0.0), None),
            TouchAction::Press(_)
        ));
        assert!(!gate.mouse_pressed());
        assert!(!gate.mouse_allowed());
        gate.touch(TouchPhase::Ended, 1, (0.0, 0.0), None);
        assert!(gate.mouse_allowed());
        assert!(gate.mouse_pressed());
    }
}
