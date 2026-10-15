//! The parameters of a Channel Mixer adjustment (0.245.0): Photoshop's RGB
//! mixer, three output channels, each a mix of the three source channels
//! plus a constant, and the Monochrome checkbox.
//!
//! It lives in `aurora-core`, beside [`crate::ColorBalanceParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The math (applied by `aurora_filters::ChannelMixerOp`)
//!
//! On **straight, encoded** sRGB, input clamped to `[0, 1]`. Each output row
//! is `[red, green, blue, constant]`: the three source percentages
//! (`-200..=200`) and the constant (`-100..=100`, a percent of full scale).
//! Output channel `i` is
//!
//! ```text
//! out_i = (row_i.red * R + row_i.green * G + row_i.blue * B) / 100 + row_i.constant / 100
//! ```
//!
//! clamped to `[0, 1]`. The identity is the diagonal at 100 with zero
//! constants. With **Monochrome** on, one grey is made from the *first*
//! row and written to all three channels; the other two rows are kept (so
//! the state survives toggling) but unused. **Which row Photoshop stores
//! the Gray mix in is unverified** (no fixture has Monochrome on): the first
//! row is a choice, and so is working in encoded sRGB (Photoshop mixes in
//! the document's own encoding, which for the sRGB documents this opens is
//! the same thing). **Not compared with Photoshop**: every fixture holds
//! the neutral mixer.
//!
//! # Validation
//!
//! Every value finite, sources in `-200..=200`, constants in `-100..=100`.
//! A value outside is an error, never clamped, including when decoding a
//! `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(red_row, green_row, blue_row,
//! monochrome)`, each row four `f32`s `[red, green, blue, constant]`, then
//! one `bool`: 49 bytes.

/// The output rows' names, in storage order.
pub const CHANNEL_MIXER_OUTPUTS: [&str; 3] = ["Red", "Green", "Blue"];

/// The largest source percentage.
pub const CHANNEL_MIXER_SOURCE_MAX: f32 = 200.0;

/// The largest constant.
pub const CHANNEL_MIXER_CONSTANT_MAX: f32 = 100.0;

/// Why a [`ChannelMixerParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMixerError {
    /// A value was `NaN` or infinite.
    NotFinite,
    /// A source percentage is outside `-200..=200`.
    Source,
    /// A constant is outside `-100..=100`.
    Constant,
    /// An output or slot index is past the last.
    Index,
}

impl std::fmt::Display for ChannelMixerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a Channel Mixer value is not a finite number",
            Self::Source => "a Channel Mixer source is outside -200..200",
            Self::Constant => "a Channel Mixer constant is outside -100..100",
            Self::Index => "a Channel Mixer output or slot does not exist",
        })
    }
}

impl std::error::Error for ChannelMixerError {}

/// A row: `[red, green, blue, constant]`.
pub type MixerRow = [f32; 4];

/// The wire form: see the module docs.
type Wire = (MixerRow, MixerRow, MixerRow, bool);

/// A Channel Mixer adjustment's settings. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct ChannelMixerParams {
    /// `[output][red, green, blue, constant]`, outputs in
    /// [`CHANNEL_MIXER_OUTPUTS`] order.
    rows: [MixerRow; 3],
    monochrome: bool,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for ChannelMixerParams {}

impl Default for ChannelMixerParams {
    fn default() -> Self {
        Self::identity()
    }
}

impl TryFrom<Wire> for ChannelMixerParams {
    type Error = ChannelMixerError;

    fn try_from(v: Wire) -> Result<Self, ChannelMixerError> {
        Self::new([v.0, v.1, v.2], v.3)
    }
}

impl From<ChannelMixerParams> for Wire {
    fn from(p: ChannelMixerParams) -> Self {
        let [red, green, blue] = p.rows;
        (red, green, blue, p.monochrome)
    }
}

impl ChannelMixerParams {
    /// Each output is its own source at 100 %, no constants, Monochrome
    /// off: changes nothing.
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            rows: [
                [100.0, 0.0, 0.0, 0.0],
                [0.0, 100.0, 0.0, 0.0],
                [0.0, 0.0, 100.0, 0.0],
            ],
            monochrome: false,
        }
    }

    /// A validated record.
    ///
    /// # Errors
    /// [`ChannelMixerError`] naming the first rule broken.
    pub fn new(rows: [MixerRow; 3], monochrome: bool) -> Result<Self, ChannelMixerError> {
        if rows.iter().flatten().any(|v| !v.is_finite()) {
            return Err(ChannelMixerError::NotFinite);
        }
        for row in &rows {
            let [r, g, b, k] = *row;
            let source = -CHANNEL_MIXER_SOURCE_MAX..=CHANNEL_MIXER_SOURCE_MAX;
            if ![r, g, b].iter().all(|v| source.contains(v)) {
                return Err(ChannelMixerError::Source);
            }
            if !(-CHANNEL_MIXER_CONSTANT_MAX..=CHANNEL_MIXER_CONSTANT_MAX).contains(&k) {
                return Err(ChannelMixerError::Constant);
            }
        }
        Ok(Self { rows, monochrome })
    }

    /// The three rows.
    #[must_use]
    pub const fn rows(&self) -> [MixerRow; 3] {
        self.rows
    }

    /// Row `output` (`0` red, `1` green, `2` blue), `None` past the last.
    #[must_use]
    pub fn row(&self, output: usize) -> Option<MixerRow> {
        self.rows.get(output).copied()
    }

    /// Whether Monochrome is on.
    #[must_use]
    pub const fn monochrome(&self) -> bool {
        self.monochrome
    }

    /// Whether the adjustment changes nothing: Monochrome off and the
    /// identity rows (a negative zero counts as zero).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        !self.monochrome && self.rows == Self::identity().rows
    }

    /// A copy with slot `slot` (`0` red, `1` green, `2` blue, `3`
    /// constant) of row `output` replaced.
    ///
    /// # Errors
    /// As [`Self::new`]; an index past the last is
    /// [`ChannelMixerError::Index`].
    pub fn with_value(
        &self,
        output: usize,
        slot: usize,
        value: f32,
    ) -> Result<Self, ChannelMixerError> {
        let mut rows = self.rows;
        *rows
            .get_mut(output)
            .and_then(|r| r.get_mut(slot))
            .ok_or(ChannelMixerError::Index)? = value;
        Self::new(rows, self.monochrome)
    }

    /// A copy with Monochrome switched.
    #[must_use]
    pub const fn with_monochrome(&self, monochrome: bool) -> Self {
        Self {
            monochrome,
            ..*self
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::{ChannelMixerError as E, ChannelMixerParams as P};

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn identity_is_the_diagonal_at_100_with_monochrome_off() {
        assert!(P::identity().is_identity());
        assert!(P::default().is_identity());
        assert!(!P::identity().with_monochrome(true).is_identity());
        for o in 0..3 {
            for s in 0..4 {
                let v = if s == o { 99.0 } else { 1.0 };
                assert!(
                    !ok(P::identity().with_value(o, s, v)).is_identity(),
                    "{o} {s}"
                );
            }
        }
        // A negative zero is a zero.
        assert!(ok(P::identity().with_value(0, 1, -0.0)).is_identity());
    }

    #[test]
    fn validation_names_the_rule_broken_at_the_boundaries() {
        assert!(P::new([[200.0, -200.0, 200.0, 100.0]; 3], true).is_ok());
        assert!(P::new([[-200.0, 200.0, -200.0, -100.0]; 3], false).is_ok());
        for o in 0..3 {
            for s in 0..3 {
                assert_eq!(P::identity().with_value(o, s, 200.5), Err(E::Source));
                assert_eq!(P::identity().with_value(o, s, -200.5), Err(E::Source));
            }
            assert_eq!(P::identity().with_value(o, 3, 100.5), Err(E::Constant));
            assert_eq!(P::identity().with_value(o, 3, -100.5), Err(E::Constant));
            for s in 0..4 {
                for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    assert_eq!(P::identity().with_value(o, s, bad), Err(E::NotFinite));
                }
            }
        }
        // 150 is a legal source but an illegal constant.
        assert_eq!(P::identity().with_value(0, 3, 150.0), Err(E::Constant));
        assert!(P::identity().with_value(0, 0, 150.0).is_ok());
        assert_eq!(P::identity().with_value(3, 0, 0.0), Err(E::Index));
        assert_eq!(P::identity().with_value(0, 4, 0.0), Err(E::Index));
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_bad_values() {
        let p = ok(P::new(
            [
                [1.0, -2.0, 3.0, -4.0],
                [5.0, -6.0, 200.0, 7.0],
                [-200.0, 8.0, -9.0, 100.0],
            ],
            true,
        ));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(
            bytes.len(),
            12 * 4 + 1,
            "twelve f32s and a bool, positional"
        );
        assert_eq!(bytes.get(48), Some(&1));
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
        for slot in 0..12 {
            for bad in [201.0_f32, -200.5, f32::NAN, f32::INFINITY] {
                let mut v = [0.0_f32; 12];
                if let Some(x) = v.get_mut(slot) {
                    *x = bad;
                }
                let tuple = (
                    [v[0], v[1], v[2], v[3]],
                    [v[4], v[5], v[6], v[7]],
                    [v[8], v[9], v[10], v[11]],
                    false,
                );
                let Ok(wire) = postcard::to_allocvec(&tuple) else {
                    unreachable!("encodes");
                };
                // A constant slot (3, 7, 11) tolerates 100 but not 150; 201
                // is bad everywhere.
                assert!(postcard::from_bytes::<P>(&wire).is_err(), "{bad} in {slot}");
            }
        }
        // 150 in a constant slot is refused although 150 is a legal source.
        let tuple = (
            [0.0_f32, 0.0, 0.0, 150.0],
            [0.0_f32; 4],
            [0.0_f32; 4],
            false,
        );
        let Ok(wire) = postcard::to_allocvec(&tuple) else {
            unreachable!("encodes");
        };
        assert!(postcard::from_bytes::<P>(&wire).is_err());
        assert!(postcard::from_bytes::<P>(bytes.get(..48).unwrap_or(&[])).is_err());
    }

    #[test]
    fn setters_edit_only_their_slot() {
        let p = ok(P::new(
            [
                [1.0, 2.0, 3.0, 4.0],
                [5.0, 6.0, 7.0, 8.0],
                [9.0, 10.0, 11.0, 12.0],
            ],
            true,
        ));
        for o in 0..3 {
            for s in 0..4 {
                let q = ok(p.with_value(o, s, -50.0));
                let mut want = p.rows();
                want[o][s] = -50.0;
                assert_eq!(q.rows(), want);
                assert!(q.monochrome());
            }
        }
        assert_eq!(p.row(1), Some([5.0, 6.0, 7.0, 8.0]));
        assert_eq!(p.row(3), None);
        let q = p.with_monochrome(false);
        assert!(!q.monochrome());
        assert_eq!(q.rows(), p.rows());
    }
}
