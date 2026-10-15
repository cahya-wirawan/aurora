//! The parameters of a Selective Color adjustment (0.245.0): Photoshop's
//! nine colour ranges, four ink sliders each, and the Relative/Absolute
//! method.
//!
//! It lives in `aurora-core`, beside [`crate::ColorBalanceParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::SelectiveColorOp`)
//!
//! **A textbook approximation, UNVALIDATED**: every fixture holds the
//! neutral layer, so nothing here was compared with Photoshop (see the
//! filter's docs for the model and every choice). The ranges are, in
//! storage order, Reds, Yellows, Greens, Cyans, Blues, Magentas, Whites,
//! Neutrals and Blacks; the sliders of each are `[cyan, magenta, yellow,
//! black]`, whole percent in `-100..=100`.
//!
//! # Validation
//!
//! Every value finite and in `-100..=100`. A value outside is an error,
//! never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(ranges, absolute)`, the ranges nine
//! rows of four `f32`s, then one `bool` (`false` Relative, `true`
//! Absolute): 145 bytes.

/// The nine colour ranges' names, in storage order.
pub const SELECTIVE_COLOR_RANGES: [&str; 9] = [
    "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals", "Blacks",
];

/// The four sliders' names.
pub const SELECTIVE_COLOR_INKS: [&str; 4] = ["Cyan", "Magenta", "Yellow", "Black"];

/// The largest slider magnitude.
pub const SELECTIVE_COLOR_MAX: f32 = 100.0;

/// Why a [`SelectiveColorParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectiveColorError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// A slider is outside `-100..=100`.
    Slider,
}

impl std::fmt::Display for SelectiveColorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Selective Color value is not a finite number",
            Self::Slider => "a Selective Color slider is outside -100..100",
        })
    }
}

impl std::error::Error for SelectiveColorError {}

/// A range's sliders: `[cyan, magenta, yellow, black]`.
pub type InkRow = [f32; 4];

/// The wire form: see the module docs.
type Wire = ([InkRow; 9], bool);

/// A Selective Color adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct SelectiveColorParams {
    /// `[range][cyan, magenta, yellow, black]`, ranges in
    /// [`SELECTIVE_COLOR_RANGES`] order.
    ranges: [InkRow; 9],
    absolute: bool,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for SelectiveColorParams {}

impl Default for SelectiveColorParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for SelectiveColorParams {
    type Error = SelectiveColorError;

    fn try_from(v: Wire) -> Result<Self, SelectiveColorError> {
        Self::new(v.0, v.1)
    }
}

impl From<SelectiveColorParams> for Wire {
    fn from(p: SelectiveColorParams) -> Self {
        (p.ranges, p.absolute)
    }
}

impl SelectiveColorParams {
    /// Every slider at zero, Relative (Photoshop's default): changes
    /// nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            ranges: [[0.0; 4]; 9],
            absolute: false,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`SelectiveColorError`] naming the first rule broken.
    pub fn new(ranges: [InkRow; 9], absolute: bool) -> Result<Self, SelectiveColorError> {
        let all = || ranges.iter().flatten();
        if all().any(|v| !v.is_finite()) {
            return Err(SelectiveColorError::NotFinite);
        }
        if all().any(|v| !(-SELECTIVE_COLOR_MAX..=SELECTIVE_COLOR_MAX).contains(v)) {
            return Err(SelectiveColorError::Slider);
        }
        Ok(Self { ranges, absolute })
    }

    /// The sliders of every range.
    #[must_use]
    pub const fn ranges(&self) -> [InkRow; 9] {
        self.ranges
    }

    /// The four sliders of range `range`, `None` past the last.
    #[must_use]
    pub fn range(&self, range: usize) -> Option<InkRow> {
        self.ranges.get(range).copied()
    }

    /// Whether the method is Absolute (`false`: Relative).
    #[must_use]
    pub const fn absolute(&self) -> bool {
        self.absolute
    }

    /// Whether the adjustment changes nothing: every slider exactly zero
    /// (the method is irrelevant then).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.ranges.iter().flatten().all(|&v| v == 0.0)
    }

    /// A copy with slider `slider` (`0` cyan, `1` magenta, `2` yellow, `3`
    /// black) of range `range` replaced.
    ///
    /// # Errors
    /// As [`Self::new`]; an index past the last is
    /// [`SelectiveColorError::Slider`].
    pub fn with_slider(
        &self,
        range: usize,
        slider: usize,
        value: f32,
    ) -> Result<Self, SelectiveColorError> {
        let mut ranges = self.ranges;
        *ranges
            .get_mut(range)
            .and_then(|r| r.get_mut(slider))
            .ok_or(SelectiveColorError::Slider)? = value;
        Self::new(ranges, self.absolute)
    }

    /// A copy with the method switched.
    #[must_use]
    pub const fn with_absolute(&self, absolute: bool) -> Self {
        Self { absolute, ..*self }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{SelectiveColorError as E, SelectiveColorParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_every_slider_at_zero_whatever_the_method() {
        assert!(P::identity().is_identity());
        assert!(P::default().is_identity());
        assert!(!P::identity().absolute());
        assert!(P::identity().with_absolute(true).is_identity());
        assert!(ok(P::new([[-0.0; 4]; 9], false)).is_identity());
        for r in 0..9 {
            for s in 0..4 {
                assert!(
                    !ok(P::identity().with_slider(r, s, 1.0)).is_identity(),
                    "{r} {s}"
                );
            }
        }
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new([[100.0, -100.0, 100.0, -100.0]; 9], true).is_ok());
        for r in 0..9 {
            for s in 0..4 {
                assert_eq!(P::identity().with_slider(r, s, 100.5), Err(E::Slider));
                assert_eq!(P::identity().with_slider(r, s, -100.5), Err(E::Slider));
                for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    assert_eq!(P::identity().with_slider(r, s, bad), Err(E::NotFinite));
                }
            }
        }
        assert_eq!(P::identity().with_slider(9, 0, 0.0), Err(E::Slider));
        assert_eq!(P::identity().with_slider(0, 4, 0.0), Err(E::Slider));
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let mut ranges = [[0.0_f32; 4]; 9];
        for (i, r) in ranges.iter_mut().enumerate() {
            for (j, v) in r.iter_mut().enumerate() {
                *v = (i as f32 * 4.0 + j as f32) - 18.0;
            }
        }
        let p = ok(P::new(ranges, true));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(
            bytes.len(),
            36 * 4 + 1,
            "thirty-six f32s and a bool, positional"
        );
        assert_eq!(bytes.get(144), Some(&1));
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        for slot in 0..36 {
            for bad in [101.0_f32, -100.5, f32::NAN, f32::NEG_INFINITY] {
                let mut v = [[0.0_f32; 4]; 9];
                v[slot / 4][slot % 4] = bad;
                let Ok(wire) = postcard::to_allocvec(&(v, false)) else {
                    unreachable!("encodes");
                };
                assert!(postcard::from_bytes::<P>(&wire).is_err(), "{bad} in {slot}");
            }
        }
        assert!(postcard::from_bytes::<P>(bytes.get(..144).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_edit_only_their_slot() {
        let mut ranges = [[0.0_f32; 4]; 9];
        ranges[4] = [1.0, 2.0, 3.0, 4.0];
        let p = ok(P::new(ranges, true));
        for r in 0..9 {
            for s in 0..4 {
                let q = ok(p.with_slider(r, s, -50.0));
                let mut want = p.ranges();
                want[r][s] = -50.0;
                assert_eq!(q.ranges(), want);
                assert!(q.absolute());
            }
        }
        assert_eq!(p.range(4), Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(p.range(9), None);
        let q = p.with_absolute(false);
        assert!(!q.absolute());
        assert_eq!(q.ranges(), p.ranges());
    }
}
