//! Platform-independent part: the numbers of a `POINTER_PEN_INFO` turned into
//! degrees and `0..1`, and the bounded map from pointer id to them.

/// `PEN_MASK_PRESSURE`.
pub const MASK_PRESSURE: u32 = 0x1;
/// `PEN_MASK_ROTATION`.
pub const MASK_ROTATION: u32 = 0x2;
/// `PEN_MASK_TILT_X`.
pub const MASK_TILT_X: u32 = 0x4;
/// `PEN_MASK_TILT_Y`.
pub const MASK_TILT_Y: u32 = 0x8;
/// `PEN_FLAG_BARREL`.
pub const FLAG_BARREL: u32 = 0x1;
/// `PEN_FLAG_INVERTED`.
pub const FLAG_INVERTED: u32 = 0x2;
/// `PEN_FLAG_ERASER`.
pub const FLAG_ERASER: u32 = 0x4;

/// The most pointer ids remembered at once. A pen, a finger or two and a
/// stale entry or two; more means the oldest is dropped.
pub const MAX_TRACKED: usize = 8;

/// What the pen reported for one pointer event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPen {
    /// `0.0..=1.0`; neutral (0) when the device reports no pressure.
    pub pressure: f32,
    /// Degrees `-90..=90`, 0 when the device reports no X tilt.
    pub tilt_x: f32,
    /// Degrees `-90..=90`, 0 when the device reports no Y tilt.
    pub tilt_y: f32,
    /// Degrees `0..360`, 0 when the device reports no rotation.
    pub rotation: f32,
    /// A barrel button is held.
    pub barrel: bool,
    /// The pen is turned over.
    pub inverted: bool,
    /// The eraser end is in use.
    pub eraser: bool,
}

impl RawPen {
    /// Converts the fields of a `POINTER_PEN_INFO`. A field whose mask bit is
    /// clear is neutral; an out-of-range value is clamped (tilt, pressure) or
    /// wrapped (rotation).
    #[must_use]
    pub fn from_raw(
        mask: u32,
        flags: u32,
        pressure: u32,
        rotation: u32,
        tilt_x: i32,
        tilt_y: i32,
    ) -> Self {
        let pressure = if mask & MASK_PRESSURE != 0 {
            (pressure.min(1024) as f32) / 1024.0
        } else {
            0.0
        };
        let tilt = |bit: u32, v: i32| {
            if mask & bit != 0 {
                v.clamp(-90, 90) as f32
            } else {
                0.0
            }
        };
        let rotation = if mask & MASK_ROTATION != 0 {
            (rotation % 360) as f32
        } else {
            0.0
        };
        Self {
            pressure,
            tilt_x: tilt(MASK_TILT_X, tilt_x),
            tilt_y: tilt(MASK_TILT_Y, tilt_y),
            rotation,
            barrel: flags & FLAG_BARREL != 0,
            inverted: flags & FLAG_INVERTED != 0,
            eraser: flags & FLAG_ERASER != 0,
        }
    }
}

/// Pointer id to latest pen info, at most [`MAX_TRACKED`] entries, oldest
/// inserted evicted first. Updating an id keeps its age.
#[derive(Debug, Default, Clone)]
pub struct PenMap {
    entries: Vec<(u64, RawPen)>,
}

impl PenMap {
    /// Records `pen` for `id`.
    pub fn insert(&mut self, id: u64, pen: RawPen) {
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == id) {
            slot.1 = pen;
            return;
        }
        if self.entries.len() >= MAX_TRACKED {
            self.entries.remove(0);
        }
        self.entries.push((id, pen));
    }

    /// The latest info for `id`.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<RawPen> {
        self.entries.iter().find(|(k, _)| *k == id).map(|(_, p)| *p)
    }

    /// Forgets `id`.
    pub fn forget(&mut self, id: u64) {
        self.entries.retain(|(k, _)| *k != id);
    }

    /// Entries held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const ALL: u32 = MASK_PRESSURE | MASK_ROTATION | MASK_TILT_X | MASK_TILT_Y;

    #[allow(clippy::cast_sign_loss)]
    fn pen(p: f32) -> RawPen {
        RawPen::from_raw(MASK_PRESSURE, 0, (p * 1024.0) as u32, 0, 0, 0)
    }

    #[test]
    fn full_mask_converts_every_field() {
        let p = RawPen::from_raw(ALL, 0, 512, 90, -30, 45);
        assert_eq!(p.pressure, 0.5);
        assert_eq!(p.rotation, 90.0);
        assert_eq!((p.tilt_x, p.tilt_y), (-30.0, 45.0));
        assert!(!p.barrel && !p.inverted && !p.eraser);
    }

    #[test]
    fn clear_mask_bits_are_neutral() {
        let p = RawPen::from_raw(0, 0, 1024, 200, 40, -40);
        assert_eq!(
            (p.pressure, p.tilt_x, p.tilt_y, p.rotation),
            (0.0, 0.0, 0.0, 0.0)
        );
        for (bit, check) in [
            (MASK_PRESSURE, 0usize),
            (MASK_ROTATION, 1),
            (MASK_TILT_X, 2),
            (MASK_TILT_Y, 3),
        ] {
            let p = RawPen::from_raw(bit, 0, 1024, 200, 40, -40);
            let got = [p.pressure, p.rotation, p.tilt_x, p.tilt_y];
            for (i, v) in got.iter().enumerate() {
                assert_eq!(*v != 0.0, i == check, "bit {bit:#x} field {i}");
            }
        }
    }

    #[test]
    fn extremes_and_out_of_range_values() {
        let p = RawPen::from_raw(ALL, 0, 1024, 359, 90, -90);
        assert_eq!(
            (p.pressure, p.rotation, p.tilt_x, p.tilt_y),
            (1.0, 359.0, 90.0, -90.0)
        );
        let p = RawPen::from_raw(ALL, 0, u32::MAX, u32::MAX, i32::MAX, i32::MIN);
        assert_eq!(p.pressure, 1.0);
        assert!(p.rotation >= 0.0 && p.rotation < 360.0);
        assert_eq!((p.tilt_x, p.tilt_y), (90.0, -90.0));
        let p = RawPen::from_raw(ALL, 0, 0, 360, 0, 0);
        assert_eq!((p.pressure, p.rotation), (0.0, 0.0));
    }

    #[test]
    fn flags_are_independent() {
        let p = RawPen::from_raw(0, FLAG_BARREL | FLAG_ERASER, 0, 0, 0, 0);
        assert!(p.barrel && p.eraser && !p.inverted);
        let p = RawPen::from_raw(0, FLAG_INVERTED, 0, 0, 0, 0);
        assert!(p.inverted && !p.barrel && !p.eraser);
    }

    #[test]
    fn map_updates_forgets_and_evicts_oldest() {
        let mut m = PenMap::default();
        assert!(m.is_empty());
        m.insert(1, pen(0.25));
        m.insert(1, pen(0.5));
        assert_eq!(m.len(), 1);
        assert_eq!(m.get(1).map(|p| p.pressure), Some(0.5));
        m.forget(1);
        assert_eq!(m.get(1), None);
        m.forget(99);
        for id in 0..(MAX_TRACKED as u64 + 3) {
            m.insert(id, pen(0.1));
        }
        assert_eq!(m.len(), MAX_TRACKED);
        assert_eq!(m.get(0), None);
        assert_eq!(m.get(2), None);
        assert!(m.get(3).is_some());
        assert!(m.get(MAX_TRACKED as u64 + 2).is_some());
    }

    #[test]
    fn updating_keeps_age() {
        let mut m = PenMap::default();
        for id in 0..MAX_TRACKED as u64 {
            m.insert(id, pen(0.1));
        }
        m.insert(0, pen(0.9));
        m.insert(100, pen(0.1));
        // 0 was oldest by insertion, so it is the one evicted.
        assert_eq!(m.get(0), None);
        assert!(m.get(1).is_some());
    }
}
