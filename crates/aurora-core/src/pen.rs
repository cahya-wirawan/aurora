//! The pointer-input contract (task 169): one sample of a mouse, touch or
//! pen, with the extra axes a pen can report.
//!
//! # Units
//!
//! * `position`: **physical window pixels** as the windowing system reports
//!   them (not yet divided by the scale factor); `f64`, finite.
//! * `pressure`: `0.0..=1.0`. A device with no pressure axis reports `1.0`
//!   (a mouse press is a full press), so a consumer that ignores pressure
//!   behaves exactly as it did before pen input existed.
//! * `tilt_x`, `tilt_y`: degrees, `-90.0..=90.0`, `0.0` = the pen upright.
//!   Positive `tilt_x` leans toward the right, positive `tilt_y` toward the
//!   user (the W3C Pointer Events convention).
//! * `rotation`: degrees, `0.0..360.0`, the barrel twist; `0.0` if unknown.
//!
//! [`PenSample::new`] sanitises: a non-finite value becomes the neutral one
//! (position `0`, pressure `1`, tilt `0`, rotation `0`), an out-of-range
//! value is clamped, rotation wraps. It never panics.

/// Where a [`PenSample`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerSource {
    /// A mouse or trackpad.
    Mouse,
    /// A finger or a pen reported as touch (no way to tell them apart).
    Touch,
    /// A device known to be a pen or stylus.
    Pen,
}

/// One pointer sample. See the module documentation for the units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenSample {
    /// Physical window pixels.
    pub x: f64,
    /// Physical window pixels.
    pub y: f64,
    /// `0.0..=1.0`.
    pub pressure: f32,
    /// Degrees, `-90.0..=90.0`.
    pub tilt_x: f32,
    /// Degrees, `-90.0..=90.0`.
    pub tilt_y: f32,
    /// Degrees, `0.0..360.0`.
    pub rotation: f32,
    /// The device class.
    pub source: PointerSource,
}

impl PenSample {
    /// A sanitised sample (see the module documentation).
    #[must_use]
    pub fn new(
        position: (f64, f64),
        pressure: f32,
        tilt: (f32, f32),
        rotation: f32,
        source: PointerSource,
    ) -> Self {
        let finite64 = |v: f64| if v.is_finite() { v } else { 0.0 };
        let clamp = |v: f32, lo: f32, hi: f32, neutral: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                neutral
            }
        };
        let rotation = if rotation.is_finite() {
            let wrapped = rotation.rem_euclid(360.0);
            // `rem_euclid` of a tiny negative can round up to exactly 360.
            if wrapped >= 360.0 { 0.0 } else { wrapped }
        } else {
            0.0
        };
        Self {
            x: finite64(position.0),
            y: finite64(position.1),
            pressure: clamp(pressure, 0.0, 1.0, 1.0),
            tilt_x: clamp(tilt.0, -90.0, 90.0, 0.0),
            tilt_y: clamp(tilt.1, -90.0, 90.0, 0.0),
            rotation,
            source,
        }
    }

    /// A mouse sample: full pressure, upright, no rotation.
    #[must_use]
    pub fn mouse(position: (f64, f64)) -> Self {
        Self::new(position, 1.0, (0.0, 0.0), 0.0, PointerSource::Mouse)
    }
}

impl Default for PenSample {
    fn default() -> Self {
        Self::mouse((0.0, 0.0))
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn mouse_sample_is_full_pressure_upright() {
        let s = PenSample::mouse((3.0, 4.0));
        assert_eq!((s.x, s.y), (3.0, 4.0));
        assert_eq!(s.pressure, 1.0);
        assert_eq!((s.tilt_x, s.tilt_y, s.rotation), (0.0, 0.0, 0.0));
        assert_eq!(s.source, PointerSource::Mouse);
    }

    #[test]
    fn non_finite_values_become_neutral() {
        let s = PenSample::new(
            (f64::NAN, f64::INFINITY),
            f32::NAN,
            (f32::INFINITY, f32::NEG_INFINITY),
            f32::NAN,
            PointerSource::Pen,
        );
        assert_eq!((s.x, s.y), (0.0, 0.0));
        assert_eq!(s.pressure, 1.0);
        assert_eq!((s.tilt_x, s.tilt_y, s.rotation), (0.0, 0.0, 0.0));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let s = PenSample::new((1.0, 2.0), 7.0, (-200.0, 200.0), 725.0, PointerSource::Pen);
        assert_eq!(s.pressure, 1.0);
        assert_eq!((s.tilt_x, s.tilt_y), (-90.0, 90.0));
        assert_eq!(s.rotation, 5.0);
        let s = PenSample::new((0.0, 0.0), -1.0, (0.0, 0.0), -90.0, PointerSource::Pen);
        assert_eq!(s.pressure, 0.0);
        assert_eq!(s.rotation, 270.0);
        let s = PenSample::new((0.0, 0.0), 0.5, (0.0, 0.0), -1e-9, PointerSource::Pen);
        assert!(s.rotation >= 0.0 && s.rotation < 360.0);
    }
}
