//! The per-colour-range part of a Hue/Saturation adjustment (0.204.0):
//! Photoshop's six ranges (reds, yellows, greens, cyans, blues, magentas),
//! each with its own hue, saturation and lightness and a four-number
//! fuzzy range (ramp up between edges 1 and 2, full between 2 and 3, ramp
//! down between 3 and 4, in degrees, wrapping around 360), exactly the
//! `hue2` block's range records.
//!
//! A [`HueSaturationRangesParams`] is the master/Colorize
//! [`HueSaturationParams`] plus the six [`HueRange`]s. It is stored as the
//! **appended** `Adjustment::HueSaturationRanges` variant (ordinal 3), so
//! the bytes of the 0.203.0 variant never change; a layer whose ranges are
//! all at their defaults is stored as that older variant
//! ([`HueSaturationRangesParams::is_plain`]).
//!
//! # Validation
//!
//! Every value finite; each edge in `0..=360`; the three forward gaps
//! `(e2-e1)`, `(e3-e2)`, `(e4-e3)`, each taken modulo 360, sum to at most
//! 360 (the ramps must not overlap themselves); hue in `-180..=180`;
//! saturation and lightness in `-100..=100`. A value outside is an error,
//! never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: `(HueSaturationParams, [HueRange; 6])`, a
//! `HueRange` being seven `f32`s `(edge1..edge4, hue, saturation,
//! lightness)`.

use crate::HueSaturationParams;

/// The number of colour ranges.
pub const HUE_RANGE_COUNT: usize = 6;

/// The range names, in the `hue2` block's order.
pub const HUE_RANGE_NAMES: [&str; HUE_RANGE_COUNT] =
    ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];

/// Photoshop's standard edges for the six ranges, in the same order.
pub const DEFAULT_HUE_RANGE_EDGES: [[f32; 4]; HUE_RANGE_COUNT] = [
    [315.0, 345.0, 15.0, 45.0],
    [15.0, 45.0, 75.0, 105.0],
    [75.0, 105.0, 135.0, 165.0],
    [135.0, 165.0, 195.0, 225.0],
    [195.0, 225.0, 255.0, 285.0],
    [255.0, 285.0, 315.0, 345.0],
];

/// Why a [`HueRange`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HueRangeError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// An edge is outside `0..=360`.
    Edge,
    /// The edges' forward gaps sum to more than 360 degrees.
    EdgeOrder,
    /// The hue is outside `-180..=180`.
    Hue,
    /// The saturation or lightness is outside `-100..=100`.
    Amount,
}

impl std::fmt::Display for HueRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Hue/Saturation range value is not a finite number",
            Self::Edge => "a Hue/Saturation range edge is outside 0..360",
            Self::EdgeOrder => "a Hue/Saturation range's edges wrap around more than once",
            Self::Hue => "a Hue/Saturation range hue is outside -180..180",
            Self::Amount => "a Hue/Saturation range saturation or lightness is outside -100..100",
        })
    }
}

impl std::error::Error for HueRangeError {}

/// The forward angle from `a` to `b` in `[0, 360)`.
#[must_use]
pub fn forward_gap(a: f32, b: f32) -> f32 {
    (b - a).rem_euclid(360.0)
}

type RangeWire = (f32, f32, f32, f32, f32, f32, f32);

/// One colour range: its four edges and its three adjustments.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RangeWire", into = "RangeWire")]
pub struct HueRange {
    edges: [f32; 4],
    hue: f32,
    saturation: f32,
    lightness: f32,
}

impl Eq for HueRange {}

impl TryFrom<RangeWire> for HueRange {
    type Error = HueRangeError;

    fn try_from(v: RangeWire) -> Result<Self, HueRangeError> {
        Self::new([v.0, v.1, v.2, v.3], v.4, v.5, v.6)
    }
}

impl From<HueRange> for RangeWire {
    fn from(r: HueRange) -> Self {
        (
            r.edges[0],
            r.edges[1],
            r.edges[2],
            r.edges[3],
            r.hue,
            r.saturation,
            r.lightness,
        )
    }
}

impl HueRange {
    /// Range `index` (`0..6`) at Photoshop's standard edges, adjusting
    /// nothing. An index past the last gives the last.
    #[must_use]
    pub fn standard(index: usize) -> Self {
        let edges = DEFAULT_HUE_RANGE_EDGES
            .get(index)
            .copied()
            .unwrap_or(DEFAULT_HUE_RANGE_EDGES[HUE_RANGE_COUNT - 1]);
        Self {
            edges,
            hue: 0.0,
            saturation: 0.0,
            lightness: 0.0,
        }
    }

    /// A validated range. See the module docs.
    ///
    /// # Errors
    /// [`HueRangeError`] naming the first rule broken.
    pub fn new(
        edges: [f32; 4],
        hue: f32,
        saturation: f32,
        lightness: f32,
    ) -> Result<Self, HueRangeError> {
        if edges.iter().any(|v| !v.is_finite())
            || !hue.is_finite()
            || !saturation.is_finite()
            || !lightness.is_finite()
        {
            return Err(HueRangeError::NotFinite);
        }
        if edges.iter().any(|e| !(0.0..=360.0).contains(e)) {
            return Err(HueRangeError::Edge);
        }
        let [e1, e2, e3, e4] = edges;
        if forward_gap(e1, e2) + forward_gap(e2, e3) + forward_gap(e3, e4) > 360.0 {
            return Err(HueRangeError::EdgeOrder);
        }
        if !(-180.0..=180.0).contains(&hue) {
            return Err(HueRangeError::Hue);
        }
        if !(-100.0..=100.0).contains(&saturation) || !(-100.0..=100.0).contains(&lightness) {
            return Err(HueRangeError::Amount);
        }
        Ok(Self {
            edges,
            hue,
            saturation,
            lightness,
        })
    }

    /// The four edges in degrees.
    #[must_use]
    pub const fn edges(&self) -> [f32; 4] {
        self.edges
    }

    /// The hue shift in degrees, `-180..=180`.
    #[must_use]
    pub const fn hue(&self) -> f32 {
        self.hue
    }

    /// The saturation, `-100..=100`.
    #[must_use]
    pub const fn saturation(&self) -> f32 {
        self.saturation
    }

    /// The lightness, `-100..=100`.
    #[must_use]
    pub const fn lightness(&self) -> f32 {
        self.lightness
    }

    /// Whether the range adjusts nothing (the edges then do not matter).
    #[must_use]
    pub fn adjusts_nothing(&self) -> bool {
        self.hue == 0.0 && self.saturation == 0.0 && self.lightness == 0.0
    }

    /// A copy with the hue replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_hue(&self, hue: f32) -> Result<Self, HueRangeError> {
        Self::new(self.edges, hue, self.saturation, self.lightness)
    }

    /// A copy with the saturation replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_saturation(&self, saturation: f32) -> Result<Self, HueRangeError> {
        Self::new(self.edges, self.hue, saturation, self.lightness)
    }

    /// A copy with the lightness replaced.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_lightness(&self, lightness: f32) -> Result<Self, HueRangeError> {
        Self::new(self.edges, self.hue, self.saturation, lightness)
    }
}

type RangesWire = (HueSaturationParams, [HueRange; HUE_RANGE_COUNT]);

/// The master/Colorize settings plus the six colour ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "RangesWire", into = "RangesWire")]
pub struct HueSaturationRangesParams {
    base: HueSaturationParams,
    ranges: [HueRange; HUE_RANGE_COUNT],
}

impl From<RangesWire> for HueSaturationRangesParams {
    fn from(v: RangesWire) -> Self {
        Self {
            base: v.0,
            ranges: v.1,
        }
    }
}

impl From<HueSaturationRangesParams> for RangesWire {
    fn from(p: HueSaturationRangesParams) -> Self {
        (p.base, p.ranges)
    }
}

impl From<HueSaturationParams> for HueSaturationRangesParams {
    fn from(base: HueSaturationParams) -> Self {
        Self::plain(base)
    }
}

impl HueSaturationRangesParams {
    /// `base` with all six ranges at their standard edges, adjusting nothing.
    #[must_use]
    pub fn plain(base: HueSaturationParams) -> Self {
        Self {
            base,
            ranges: std::array::from_fn(HueRange::standard),
        }
    }

    /// `base` with the given ranges (each already validated).
    #[must_use]
    pub const fn new(base: HueSaturationParams, ranges: [HueRange; HUE_RANGE_COUNT]) -> Self {
        Self { base, ranges }
    }

    /// The master/Colorize settings.
    #[must_use]
    pub const fn base(&self) -> &HueSaturationParams {
        &self.base
    }

    /// The six ranges.
    #[must_use]
    pub const fn ranges(&self) -> &[HueRange; HUE_RANGE_COUNT] {
        &self.ranges
    }

    /// Range `index` (`0..6`), or `None`.
    #[must_use]
    pub fn range(&self, index: usize) -> Option<&HueRange> {
        self.ranges.get(index)
    }

    /// Whether the ranges are exactly the standard ones adjusting nothing,
    /// so the settings are stored as the older `HueSaturation` variant.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.ranges
            .iter()
            .enumerate()
            .all(|(i, r)| *r == HueRange::standard(i))
    }

    /// Whether no range adjusts anything (edges ignored): the rendering is
    /// then exactly the master/Colorize adjustment's.
    #[must_use]
    pub fn ranges_adjust_nothing(&self) -> bool {
        self.ranges.iter().all(HueRange::adjusts_nothing)
    }

    /// Whether the adjustment changes nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.base.is_identity() && (self.base.colorize() || self.ranges_adjust_nothing())
    }

    /// A copy with the master/Colorize settings replaced.
    #[must_use]
    pub const fn with_base(&self, base: HueSaturationParams) -> Self {
        Self {
            base,
            ranges: self.ranges,
        }
    }

    /// A copy with range `index` replaced; `None` for an index past the last.
    #[must_use]
    pub fn with_range(&self, index: usize, range: HueRange) -> Option<Self> {
        let mut ranges = self.ranges;
        *ranges.get_mut(index)? = range;
        Some(Self {
            base: self.base,
            ranges,
        })
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        DEFAULT_HUE_RANGE_EDGES, HueRange as R, HueRangeError as E, HueSaturationRangesParams as P,
        forward_gap,
    };
    use crate::HueSaturationParams;

    fn ok<T>(r: Result<T, E>) -> T {
        match r {
            Ok(v) => v,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn the_standard_edges_validate_and_wrap() {
        for (i, edges) in DEFAULT_HUE_RANGE_EDGES.iter().enumerate() {
            assert_eq!(R::standard(i).edges(), *edges);
            assert!(R::new(*edges, 0.0, 0.0, 0.0).is_ok(), "{i}");
        }
        assert_eq!(forward_gap(345.0, 15.0), 30.0);
        assert_eq!(forward_gap(15.0, 345.0), 330.0);
        assert_eq!(forward_gap(360.0, 0.0), 0.0);
    }

    #[test]
    fn validation_names_the_first_rule_broken() {
        let e = [315.0, 345.0, 15.0, 45.0];
        // Photoshop corpus ranges, wrapping and wide.
        assert!(R::new([303.0, 4.0, 82.0, 175.0], -180.0, -100.0, 100.0).is_ok());
        assert!(R::new([199.0, 359.0, 29.0, 170.0], 180.0, 100.0, -100.0).is_ok());
        // A full circle is allowed, a little more is not.
        assert!(R::new([0.0, 0.0, 180.0, 360.0], 0.0, 0.0, 0.0).is_ok());
        assert_eq!(
            R::new([0.0, 120.0, 240.0, 1.0], 0.0, 0.0, 0.0),
            Err(E::EdgeOrder)
        );
        assert_eq!(
            R::new([0.0, 100.0, 200.0, 361.0], 0.0, 0.0, 0.0),
            Err(E::Edge)
        );
        assert_eq!(R::new([-1.0, 0.0, 0.0, 0.0], 0.0, 0.0, 0.0), Err(E::Edge));
        assert_eq!(R::new(e, 180.5, 0.0, 0.0), Err(E::Hue));
        assert_eq!(R::new(e, -181.0, 0.0, 0.0), Err(E::Hue));
        assert_eq!(R::new(e, 0.0, 100.5, 0.0), Err(E::Amount));
        assert_eq!(R::new(e, 0.0, 0.0, -100.5), Err(E::Amount));
        for bad in [f32::NAN, f32::INFINITY] {
            assert_eq!(
                R::new([bad, 0.0, 0.0, 0.0], 0.0, 0.0, 0.0),
                Err(E::NotFinite)
            );
            assert_eq!(R::new(e, bad, 0.0, 0.0), Err(E::NotFinite));
            assert_eq!(R::new(e, 0.0, bad, 0.0), Err(E::NotFinite));
            assert_eq!(R::new(e, 0.0, 0.0, bad), Err(E::NotFinite));
        }
    }

    #[test]
    fn plain_neutral_and_identity_are_told_apart() {
        let plain = P::plain(HueSaturationParams::identity());
        assert!(plain.is_plain() && plain.is_identity() && plain.ranges_adjust_nothing());
        let moved = ok(R::standard(2).with_saturation(10.0));
        let some = ok_some(plain.with_range(2, moved));
        assert!(!some.is_plain() && !some.is_identity() && !some.ranges_adjust_nothing());
        // Edited edges alone keep it non-plain but adjusting nothing.
        let edges = ok(R::new([80.0, 100.0, 140.0, 160.0], 0.0, 0.0, 0.0));
        let e = ok_some(plain.with_range(2, edges));
        assert!(!e.is_plain() && e.ranges_adjust_nothing() && e.is_identity());
        // Colorize ignores the ranges.
        let col = HueSaturationParams::identity().with_colorize(true);
        assert!(!P::plain(col).is_identity());
        assert!(plain.with_range(6, moved).is_none() && plain.range(6).is_none());
    }

    fn ok_some<T>(v: Option<T>) -> T {
        match v {
            Some(v) => v,
            None => unreachable!("some"),
        }
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let moved = ok(R::new([303.0, 4.0, 82.0, 175.0], -38.0, 93.0, -27.0));
        let base = ok_base();
        let p = ok_some(P::plain(base).with_range(1, moved));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        // 25-byte base + six ranges of seven f32s.
        assert_eq!(bytes.len(), 25 + 6 * 28);
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        // Corrupt one range hue to 200: the decode must refuse it.
        let mut bad = bytes.clone();
        let at = 25 + 28 + 16;
        bad.splice(at..at + 4, 200.0_f32.to_le_bytes());
        assert!(postcard::from_bytes::<P>(&bad).is_err());
        let mut bad = bytes.clone();
        bad.splice(25..29, f32::NAN.to_le_bytes());
        assert!(postcard::from_bytes::<P>(&bad).is_err());
        assert!(postcard::from_bytes::<P>(bytes.get(..bytes.len() - 1).unwrap_or(&[])).is_err());
    }

    fn ok_base() -> HueSaturationParams {
        match HueSaturationParams::new(-27.0, 12.0, -64.0, false, 0.0, 25.0, 0.0) {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }
}
