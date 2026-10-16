//! The parameters of a Gradient Map adjustment (0.248.0): Photoshop's
//! adjustment that replaces each pixel by the colour of a gradient at the
//! pixel's grey level.
//!
//! It lives in `aurora-core`, beside [`crate::ChannelMixerParams`], so
//! `aurora-doc` can store it in a layer while `aurora-filters` applies it.
//!
//! # The model
//!
//! A gradient is a list of **colour stops**, each a position in `0..=1`
//! along the grey axis and an RGB colour (straight, encoded sRGB, each
//! channel `0..=1`). Between two stops the colour is the **linear
//! interpolation in encoded sRGB** (Photoshop's *Classic* colour
//! interpolation; the *Linear* and *Perceptual* methods that version-3
//! files can name are **not** modelled, see below). Before the first stop
//! and after the last the colour is that of the end stop. Two stops at the
//! same position make a hard step (the later one wins from that position
//! on). A **Reverse** flag mirrors the grey axis (`t -> 1 - t`) and a
//! **Dither** flag is kept for the file but changes nothing in Aurora's
//! floating-point pipeline (there is no 8-bit quantisation to dither).
//!
//! What the model leaves out, deliberately, so that a layer carrying it is
//! left out of an import instead of being drawn differently: the stops'
//! **midpoints** (only the default 50 % is representable), the gradient's
//! **transparency stops** (only fully opaque ones), a **smoothness** below
//! 100 %, Noise gradients, colour stops in a colour mode other than RGB and
//! the Linear and Perceptual interpolation methods. The wire form has a
//! method byte (`0` = Classic, the only value accepted) so that a method can
//! be added without a new adjustment ordinal.
//!
//! # Validation
//!
//! `2..=64` stops, every number finite, positions in `0..=1` and in
//! non-decreasing order, colours in `0..=1`. A value outside is an error,
//! never clamped, including when decoding a `.aur`.
//!
//! # Wire format
//!
//! Positional `postcard`: a tuple `(stops, reverse, dither, method)` where
//! `stops` is a sequence of `[position, red, green, blue]` `f32` arrays
//! (a length byte, then 16 bytes per stop), `reverse` and `dither` are
//! `bool`s and `method` is a `u8`. A two-stop map is 1 + 32 + 3 = 36 bytes.

/// The most stops a gradient may hold. Photoshop's editor allows more; 64
/// bounds the work and the allocation a hostile file can ask for.
pub const GRADIENT_MAP_MAX_STOPS: usize = 64;

/// The fewest stops a gradient may hold.
pub const GRADIENT_MAP_MIN_STOPS: usize = 2;

/// Why a [`GradientMapParams`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientMapError {
    /// A position or colour channel was `NaN` or infinite.
    NotFinite,
    /// Fewer than [`GRADIENT_MAP_MIN_STOPS`] stops.
    TooFewStops,
    /// More than [`GRADIENT_MAP_MAX_STOPS`] stops.
    TooManyStops,
    /// A position is outside `0..=1`.
    Position,
    /// A position is before the one preceding it.
    Order,
    /// A colour channel is outside `0..=1`.
    Colour,
    /// The interpolation method byte is not one this build knows.
    Method,
    /// A stop or channel index is past the last.
    Index,
}

impl std::fmt::Display for GradientMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFinite => "a gradient stop value is not a finite number",
            Self::TooFewStops => "a gradient needs at least two colour stops",
            Self::TooManyStops => "a gradient has more colour stops than Aurora allows",
            Self::Position => "a gradient stop position is outside 0..1",
            Self::Order => "gradient stops are not in order of position",
            Self::Colour => "a gradient stop colour channel is outside 0..1",
            Self::Method => "the gradient's colour interpolation method is not known",
            Self::Index => "a gradient stop or channel does not exist",
        })
    }
}

impl std::error::Error for GradientMapError {}

/// How colours are interpolated between stops. Only Classic exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GradientInterpolation {
    /// Linear interpolation of the encoded sRGB channels (Photoshop's
    /// *Classic*).
    #[default]
    Classic,
}

impl GradientInterpolation {
    /// The wire byte.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Classic => 0,
        }
    }

    /// The method a wire byte names.
    ///
    /// # Errors
    /// [`GradientMapError::Method`] for any byte but `0`.
    pub const fn from_byte(byte: u8) -> Result<Self, GradientMapError> {
        match byte {
            0 => Ok(Self::Classic),
            _ => Err(GradientMapError::Method),
        }
    }
}

/// One colour stop: a position along the grey axis and an RGB colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    /// Position `0..=1`.
    pub position: f32,
    /// Straight, encoded sRGB `[red, green, blue]`, each `0..=1`.
    pub colour: [f32; 3],
}

impl GradientStop {
    /// A stop (not validated until it is part of a [`GradientMapParams`]).
    #[must_use]
    pub const fn new(position: f32, colour: [f32; 3]) -> Self {
        Self { position, colour }
    }
}

/// The wire form: see the module docs.
type Wire = (Vec<[f32; 4]>, bool, bool, u8);

/// A Gradient Map adjustment's settings. See the module docs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Wire", into = "Wire")]
pub struct GradientMapParams {
    stops: Vec<GradientStop>,
    reverse: bool,
    dither: bool,
    interpolation: GradientInterpolation,
}

// Every value is finite (validated), so `==` is an equivalence.
impl Eq for GradientMapParams {}

impl Default for GradientMapParams {
    fn default() -> Self {
        Self::black_to_white()
    }
}

impl TryFrom<Wire> for GradientMapParams {
    type Error = GradientMapError;

    fn try_from(v: Wire) -> Result<Self, GradientMapError> {
        // Checked before anything is converted: a hostile count is refused
        // without building a second vector.
        if v.0.len() > GRADIENT_MAP_MAX_STOPS {
            return Err(GradientMapError::TooManyStops);
        }
        let stops =
            v.0.iter()
                .map(|&[p, r, g, b]| GradientStop::new(p, [r, g, b]))
                .collect();
        Self::new_with(stops, v.1, v.2, GradientInterpolation::from_byte(v.3)?)
    }
}

impl From<GradientMapParams> for Wire {
    fn from(p: GradientMapParams) -> Self {
        (
            p.stops
                .iter()
                .map(|s| [s.position, s.colour[0], s.colour[1], s.colour[2]])
                .collect(),
            p.reverse,
            p.dither,
            p.interpolation.byte(),
        )
    }
}

impl GradientMapParams {
    /// Black at 0 to white at 1, Reverse and Dither off: the greyscale
    /// conversion (**not** an identity: a coloured pixel becomes grey).
    #[must_use]
    pub fn black_to_white() -> Self {
        Self {
            stops: vec![
                GradientStop::new(0.0, [0.0; 3]),
                GradientStop::new(1.0, [1.0; 3]),
            ],
            reverse: false,
            dither: false,
            interpolation: GradientInterpolation::Classic,
        }
    }

    /// A validated record with the Classic interpolation.
    ///
    /// # Errors
    /// [`GradientMapError`] naming the first rule broken.
    pub fn new(
        stops: Vec<GradientStop>,
        reverse: bool,
        dither: bool,
    ) -> Result<Self, GradientMapError> {
        Self::new_with(stops, reverse, dither, GradientInterpolation::Classic)
    }

    fn new_with(
        stops: Vec<GradientStop>,
        reverse: bool,
        dither: bool,
        interpolation: GradientInterpolation,
    ) -> Result<Self, GradientMapError> {
        if stops.len() < GRADIENT_MAP_MIN_STOPS {
            return Err(GradientMapError::TooFewStops);
        }
        if stops.len() > GRADIENT_MAP_MAX_STOPS {
            return Err(GradientMapError::TooManyStops);
        }
        if stops
            .iter()
            .any(|s| !s.position.is_finite() || s.colour.iter().any(|c| !c.is_finite()))
        {
            return Err(GradientMapError::NotFinite);
        }
        if stops.iter().any(|s| !(0.0..=1.0).contains(&s.position)) {
            return Err(GradientMapError::Position);
        }
        if stops.windows(2).any(|w| match w {
            [a, b] => b.position < a.position,
            _ => false,
        }) {
            return Err(GradientMapError::Order);
        }
        if stops
            .iter()
            .any(|s| s.colour.iter().any(|c| !(0.0..=1.0).contains(c)))
        {
            return Err(GradientMapError::Colour);
        }
        Ok(Self {
            stops,
            reverse,
            dither,
            interpolation,
        })
    }

    /// The colour stops, in order of position.
    #[must_use]
    pub fn stops(&self) -> &[GradientStop] {
        &self.stops
    }

    /// Whether the grey axis is mirrored.
    #[must_use]
    pub const fn reverse(&self) -> bool {
        self.reverse
    }

    /// Whether the file's Dither box is ticked (no effect on pixels).
    #[must_use]
    pub const fn dither(&self) -> bool {
        self.dither
    }

    /// The colour interpolation method.
    #[must_use]
    pub const fn interpolation(&self) -> GradientInterpolation {
        self.interpolation
    }

    /// Whether the gradient has exactly two stops (the only shape the
    /// Properties editor can change the colours of).
    #[must_use]
    pub fn is_two_stop(&self) -> bool {
        self.stops.len() == 2
    }

    /// A copy with Reverse switched.
    #[must_use]
    pub fn with_reverse(&self, reverse: bool) -> Self {
        Self {
            reverse,
            ..self.clone()
        }
    }

    /// A copy with Dither switched.
    #[must_use]
    pub fn with_dither(&self, dither: bool) -> Self {
        Self {
            dither,
            ..self.clone()
        }
    }

    /// A copy with colour channel `channel` (`0` red, `1` green, `2` blue)
    /// of stop `stop` replaced.
    ///
    /// # Errors
    /// As [`Self::new`]; an index past the last is
    /// [`GradientMapError::Index`].
    pub fn with_stop_channel(
        &self,
        stop: usize,
        channel: usize,
        value: f32,
    ) -> Result<Self, GradientMapError> {
        let mut stops = self.stops.clone();
        *stops
            .get_mut(stop)
            .and_then(|s| s.colour.get_mut(channel))
            .ok_or(GradientMapError::Index)? = value;
        Self::new_with(stops, self.reverse, self.dither, self.interpolation)
    }

    /// A copy with stop `stop` moved to `position`, **clamped between its
    /// neighbours** (the previous stop's position, or `0` for the first;
    /// the next stop's, or `1` for the last), so the stops stay in order
    /// and a drag never reorders them. Equal positions (a hard step) are
    /// allowed: the result may sit exactly on a neighbour.
    ///
    /// # Errors
    /// [`GradientMapError::Index`] for a stop past the last,
    /// [`GradientMapError::NotFinite`] for a `NaN` or infinite position.
    pub fn with_stop_position(&self, stop: usize, position: f32) -> Result<Self, GradientMapError> {
        if !position.is_finite() {
            return Err(GradientMapError::NotFinite);
        }
        self.stops.get(stop).ok_or(GradientMapError::Index)?;
        let low = stop
            .checked_sub(1)
            .and_then(|before| self.stops.get(before))
            .map_or(0.0, |s| s.position);
        let high = stop
            .checked_add(1)
            .and_then(|after| self.stops.get(after))
            .map_or(1.0, |s| s.position);
        // `max` then `min`, never `f32::clamp`: it panics when the bounds
        // cross, and a bound is data here.
        let position = position.max(low).min(high);
        let mut stops = self.stops.clone();
        if let Some(s) = stops.get_mut(stop) {
            s.position = position;
        }
        Self::new_with(stops, self.reverse, self.dither, self.interpolation)
    }

    /// A copy with stop `stop`'s colour replaced.
    ///
    /// # Errors
    /// [`GradientMapError::Index`] for a stop past the last,
    /// [`GradientMapError::NotFinite`] / [`GradientMapError::Colour`] for a
    /// channel that is not a finite number in `0..=1` (never clamped).
    pub fn with_stop_colour(
        &self,
        stop: usize,
        colour: [f32; 3],
    ) -> Result<Self, GradientMapError> {
        let mut stops = self.stops.clone();
        stops.get_mut(stop).ok_or(GradientMapError::Index)?.colour = colour;
        Self::new_with(stops, self.reverse, self.dither, self.interpolation)
    }

    /// A copy with one more stop **between `stop` and the stop after it**,
    /// at the midpoint of their positions with the colour halfway between
    /// theirs (linear, in encoded sRGB: the model's own interpolation, so
    /// the gradient the layer draws does not change). Returns the new
    /// params and the new stop's index (`stop + 1`).
    ///
    /// # Errors
    /// [`GradientMapError::Index`] when there is no stop after `stop`,
    /// [`GradientMapError::TooManyStops`] at [`GRADIENT_MAP_MAX_STOPS`].
    pub fn insert_stop_after(&self, stop: usize) -> Result<(Self, usize), GradientMapError> {
        let after = stop.checked_add(1).ok_or(GradientMapError::Index)?;
        let (Some(a), Some(b)) = (self.stops.get(stop), self.stops.get(after)) else {
            return Err(GradientMapError::Index);
        };
        let colour = [0, 1, 2].map(|c| {
            let (x, y) = (
                a.colour.get(c).copied().unwrap_or(0.0),
                b.colour.get(c).copied().unwrap_or(0.0),
            );
            x + (y - x) * 0.5
        });
        let inserted = GradientStop::new(a.position + (b.position - a.position) * 0.5, colour);
        let mut stops = self.stops.clone();
        stops.insert(after, inserted);
        Ok((
            Self::new_with(stops, self.reverse, self.dither, self.interpolation)?,
            after,
        ))
    }

    /// A copy without stop `stop`.
    ///
    /// # Errors
    /// [`GradientMapError::Index`] for a stop past the last,
    /// [`GradientMapError::TooFewStops`] at [`GRADIENT_MAP_MIN_STOPS`].
    pub fn remove_stop(&self, stop: usize) -> Result<Self, GradientMapError> {
        self.stops.get(stop).ok_or(GradientMapError::Index)?;
        let mut stops = self.stops.clone();
        stops.remove(stop);
        Self::new_with(stops, self.reverse, self.dither, self.interpolation)
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::{
        GRADIENT_MAP_MAX_STOPS, GradientInterpolation, GradientMapError as E,
        GradientMapParams as P, GradientStop as S,
    };

    fn ok(p: Result<P, E>) -> P {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn pair(p: Result<(P, usize), E>) -> (P, usize) {
        match p {
            Ok(p) => p,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn two() -> Vec<S> {
        vec![S::new(0.0, [0.1, 0.2, 0.3]), S::new(1.0, [0.9, 0.8, 0.7])]
    }

    #[test]
    fn black_to_white_is_two_stops_with_both_flags_off() {
        let p = P::black_to_white();
        assert_eq!(p, P::default());
        assert_eq!(p.stops().len(), 2);
        assert_eq!(p.stops()[0], S::new(0.0, [0.0; 3]));
        assert_eq!(p.stops()[1], S::new(1.0, [1.0; 3]));
        assert!(!p.reverse() && !p.dither() && p.is_two_stop());
        assert_eq!(p.interpolation(), GradientInterpolation::Classic);
    }

    #[test]
    fn stop_counts_are_bounded_at_both_ends() {
        assert_eq!(P::new(vec![], false, false), Err(E::TooFewStops));
        assert_eq!(
            P::new(vec![S::new(0.0, [0.0; 3])], false, false),
            Err(E::TooFewStops)
        );
        let n = |count: usize| -> Vec<S> {
            (0..count)
                .map(|i| S::new(i as f32 / 100.0, [0.5; 3]))
                .collect()
        };
        assert!(P::new(n(2), false, false).is_ok());
        assert!(P::new(n(GRADIENT_MAP_MAX_STOPS), false, false).is_ok());
        assert_eq!(
            P::new(n(GRADIENT_MAP_MAX_STOPS + 1), false, false),
            Err(E::TooManyStops)
        );
    }

    #[test]
    fn positions_colours_and_order_are_checked_at_the_boundaries() {
        let with = |f: &dyn Fn(&mut Vec<S>)| {
            let mut s = two();
            f(&mut s);
            P::new(s, false, false)
        };
        assert!(with(&|s| s[0].position = 0.0).is_ok());
        assert_eq!(with(&|s| s[0].position = -0.001), Err(E::Position));
        assert_eq!(with(&|s| s[1].position = 1.001), Err(E::Position));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(with(&|s| s[0].position = bad), Err(E::NotFinite));
            assert_eq!(with(&|s| s[1].colour[2] = bad), Err(E::NotFinite));
        }
        assert_eq!(with(&|s| s[0].colour[0] = 1.001), Err(E::Colour));
        assert_eq!(with(&|s| s[1].colour[1] = -0.001), Err(E::Colour));
        assert!(with(&|s| s[1].colour = [1.0, 0.0, 1.0]).is_ok());
        // Equal positions (a hard step) are legal; going backwards is not.
        assert!(
            with(&|s| {
                s[0].position = 0.5;
                s[1].position = 0.5;
            })
            .is_ok()
        );
        assert_eq!(
            with(&|s| {
                s[0].position = 0.6;
                s[1].position = 0.5;
            }),
            Err(E::Order)
        );
    }

    #[test]
    fn with_stop_channel_edits_one_value_and_validates() {
        let p = ok(P::new(two(), true, true));
        let q = ok(p.with_stop_channel(1, 2, 0.25));
        assert_eq!(q.stops()[1].colour, [0.9, 0.8, 0.25]);
        assert_eq!(q.stops()[0], p.stops()[0]);
        assert!(q.reverse() && q.dither(), "flags survive an edit");
        assert_eq!(p.with_stop_channel(2, 0, 0.0), Err(E::Index));
        assert_eq!(p.with_stop_channel(0, 3, 0.0), Err(E::Index));
        assert_eq!(p.with_stop_channel(0, 0, 1.5), Err(E::Colour));
        assert_eq!(p.with_stop_channel(0, 0, f32::NAN), Err(E::NotFinite));
    }

    // ---- task 113: the stop-edit helpers ----

    fn three() -> P {
        ok(P::new(
            vec![
                S::new(0.0, [0.0, 0.0, 0.0]),
                S::new(0.25, [1.0, 0.0, 0.5]),
                S::new(1.0, [1.0, 1.0, 1.0]),
            ],
            true,
            true,
        ))
    }

    /// The gradient's colour at `t`, written independently of the crate's
    /// code: the later stop wins at a shared position, linear between.
    fn eval(stops: &[S], t: f32) -> [f32; 3] {
        let mut colour = stops[0].colour;
        for (i, s) in stops.iter().enumerate() {
            if t >= s.position {
                colour = s.colour;
                if let Some(next) = stops.get(i + 1)
                    && t < next.position
                {
                    let f = (t - s.position) / (next.position - s.position);
                    colour = [0, 1, 2].map(|c| s.colour[c] + (next.colour[c] - s.colour[c]) * f);
                }
            }
        }
        colour
    }

    #[test]
    fn with_stop_position_clamps_between_the_neighbours() {
        let p = three();
        // Inside the neighbours: taken as is.
        assert_eq!(ok(p.with_stop_position(1, 0.6)).stops()[1].position, 0.6);
        // Past the next stop or before the previous one: stops at it.
        assert_eq!(ok(p.with_stop_position(1, 5.0)).stops()[1].position, 1.0);
        assert_eq!(ok(p.with_stop_position(1, -5.0)).stops()[1].position, 0.0);
        // The ends are bounded by 0 and 1 and by their one neighbour.
        assert_eq!(ok(p.with_stop_position(0, 0.9)).stops()[0].position, 0.25);
        assert_eq!(ok(p.with_stop_position(0, -3.0)).stops()[0].position, 0.0);
        assert_eq!(ok(p.with_stop_position(2, 0.1)).stops()[2].position, 0.25);
        assert_eq!(ok(p.with_stop_position(2, 9.0)).stops()[2].position, 1.0);
        // Everything else is untouched, flags included.
        let q = ok(p.with_stop_position(1, 0.5));
        assert_eq!(q.stops()[0], p.stops()[0]);
        assert_eq!(q.stops()[2], p.stops()[2]);
        assert_eq!(q.stops()[1].colour, p.stops()[1].colour);
        assert!(q.reverse() && q.dither());
        assert_eq!(p.with_stop_position(3, 0.5), Err(E::Index));
        assert_eq!(p.with_stop_position(usize::MAX, 0.5), Err(E::Index));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(p.with_stop_position(1, bad), Err(E::NotFinite));
        }
    }

    #[test]
    fn a_stop_may_sit_exactly_on_a_neighbour_for_a_hard_step() {
        let q = ok(three().with_stop_position(1, 1.0));
        assert_eq!(q.stops()[1].position, q.stops()[2].position);
        let r = ok(three().with_stop_position(1, 0.0));
        assert_eq!(r.stops()[0].position, r.stops()[1].position);
    }

    #[test]
    fn with_stop_colour_replaces_a_colour_and_never_clamps() {
        let p = three();
        let q = ok(p.with_stop_colour(1, [0.1, 0.2, 0.3]));
        assert_eq!(q.stops()[1].colour, [0.1, 0.2, 0.3]);
        assert_eq!(q.stops()[1].position, 0.25);
        assert_eq!(q.stops()[0], p.stops()[0]);
        assert_eq!(p.with_stop_colour(3, [0.0; 3]), Err(E::Index));
        assert_eq!(p.with_stop_colour(0, [0.0, 1.2, 0.0]), Err(E::Colour));
        assert_eq!(p.with_stop_colour(0, [0.0, -0.1, 0.0]), Err(E::Colour));
        assert_eq!(
            p.with_stop_colour(0, [f32::NAN, 0.0, 0.0]),
            Err(E::NotFinite)
        );
        assert!(p.with_stop_colour(0, [1.0, 0.0, 1.0]).is_ok());
    }

    #[test]
    fn insert_stop_after_puts_a_midpoint_stop_that_leaves_the_gradient_alone() {
        let p = three();
        let (q, at) = pair(p.insert_stop_after(1));
        assert_eq!(at, 2);
        assert_eq!(q.stops().len(), 4);
        assert_eq!(q.stops()[2].position, 0.625);
        assert_eq!(q.stops()[2].colour, [1.0, 0.5, 0.75]);
        assert_eq!(q.stops()[1], p.stops()[1]);
        assert_eq!(q.stops()[3], p.stops()[2]);
        assert!(q.reverse() && q.dither());
        // The drawn gradient is unchanged by an insertion.
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let (a, b) = (eval(p.stops(), t), eval(q.stops(), t));
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-5, "t = {t}");
            }
        }
        // The first gap too, and the position of a tie.
        let (r, at0) = pair(p.insert_stop_after(0));
        assert_eq!((at0, r.stops()[1].position), (1, 0.125));
        let tie = ok(three().with_stop_position(1, 0.0));
        let (t, _) = pair(tie.insert_stop_after(0));
        assert_eq!(t.stops()[1].position, 0.0, "a hard step stays a hard step");
    }

    #[test]
    fn insert_stop_after_is_refused_past_the_end_and_at_the_cap() {
        let p = three();
        assert_eq!(p.insert_stop_after(2), Err(E::Index));
        assert_eq!(p.insert_stop_after(usize::MAX), Err(E::Index));
        let mut q = p;
        while q.stops().len() < GRADIENT_MAP_MAX_STOPS {
            q = pair(q.insert_stop_after(0)).0;
        }
        assert_eq!(q.stops().len(), GRADIENT_MAP_MAX_STOPS);
        assert_eq!(q.insert_stop_after(0), Err(E::TooManyStops));
    }

    #[test]
    fn remove_stop_never_goes_below_two() {
        let p = three();
        let q = ok(p.remove_stop(1));
        assert_eq!(q.stops(), &[p.stops()[0], p.stops()[2]]);
        assert!(q.reverse() && q.dither());
        assert_eq!(q.remove_stop(0), Err(E::TooFewStops));
        assert_eq!(q.remove_stop(1), Err(E::TooFewStops));
        assert_eq!(q.remove_stop(2), Err(E::Index));
        assert_eq!(p.remove_stop(3), Err(E::Index));
        assert_eq!(ok(p.remove_stop(0)).stops()[0], p.stops()[1]);
        assert_eq!(ok(p.remove_stop(2)).stops()[1], p.stops()[1]);
    }

    /// A plain model of the edits (`f32` pairs, no validation), driven by a
    /// small deterministic generator, against the crate's helpers.
    #[test]
    fn random_edit_sequences_match_an_independent_model() {
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut params = P::black_to_white();
        let mut model: Vec<(f32, [f32; 3])> = vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])];
        for step in 0..600 {
            let n = model.len();
            let pick = |u: f32| ((u * n as f32) as usize).min(n - 1);
            match (next() * 4.0) as u32 {
                0 => {
                    let i = pick(next());
                    let want = next() * 1.4 - 0.2;
                    let lo = if i == 0 { 0.0 } else { model[i - 1].0 };
                    let hi = if i + 1 == n { 1.0 } else { model[i + 1].0 };
                    model[i].0 = want.max(lo).min(hi);
                    params = ok(params.with_stop_position(i, want));
                }
                1 => {
                    let i = pick(next());
                    let colour = [next(), next(), next()];
                    model[i].1 = colour;
                    params = ok(params.with_stop_colour(i, colour));
                }
                2 => {
                    let i = pick(next());
                    if i + 1 < n && n < GRADIENT_MAP_MAX_STOPS {
                        let (a, b) = (model[i], model[i + 1]);
                        let mid = (
                            a.0 + (b.0 - a.0) * 0.5,
                            [0, 1, 2].map(|c| a.1[c] + (b.1[c] - a.1[c]) * 0.5),
                        );
                        model.insert(i + 1, mid);
                        let (q, at) = pair(params.insert_stop_after(i));
                        assert_eq!(at, i + 1, "step {step}");
                        params = q;
                    }
                }
                _ => {
                    let i = pick(next());
                    if n > 2 {
                        model.remove(i);
                        params = ok(params.remove_stop(i));
                    }
                }
            }
            let got: Vec<(f32, [f32; 3])> = params
                .stops()
                .iter()
                .map(|s| (s.position, s.colour))
                .collect();
            assert_eq!(got, model, "step {step}");
            // Re-validating the result by construction proves the invariants.
            assert!(P::new(params.stops().to_vec(), false, false).is_ok());
        }
    }

    #[test]
    fn flag_setters_change_only_their_flag() {
        let p = ok(P::new(two(), false, false));
        assert!(p.with_reverse(true).reverse() && !p.with_reverse(true).dither());
        assert!(p.with_dither(true).dither() && !p.with_dither(true).reverse());
        assert_eq!(p.with_reverse(true).with_reverse(false), p);
    }

    #[test]
    fn the_wire_form_round_trips_and_is_positional() {
        let p = ok(P::new(two(), true, false));
        let Ok(bytes) = postcard::to_allocvec(&p) else {
            unreachable!("encodes");
        };
        assert_eq!(
            bytes.len(),
            1 + 2 * 16 + 3,
            "count, two stops, two bools, method"
        );
        assert_eq!(bytes[0], 2);
        assert_eq!(bytes[33], 1, "reverse");
        assert_eq!(bytes[34], 0, "dither");
        assert_eq!(bytes[35], 0, "method byte");
        assert_eq!(postcard::from_bytes::<P>(&bytes), Ok(p));
    }

    #[test]
    fn the_wire_form_refuses_bad_values_and_hostile_counts() {
        let encode = |stops: Vec<[f32; 4]>, method: u8| -> Vec<u8> {
            match postcard::to_allocvec(&(stops, false, false, method)) {
                Ok(b) => b,
                Err(err) => unreachable!("{err:?}"),
            }
        };
        let good = vec![[0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 1.0]];
        assert!(postcard::from_bytes::<P>(&encode(good.clone(), 0)).is_ok());
        assert!(postcard::from_bytes::<P>(&encode(good.clone(), 1)).is_err());
        assert!(postcard::from_bytes::<P>(&encode(good.clone(), 255)).is_err());
        for bad in [
            vec![[0.0, 0.0, 0.0, 0.0]],
            vec![[0.0, 0.0, 0.0, 0.0], [1.5, 1.0, 1.0, 1.0]],
            vec![[0.0, 0.0, 0.0, 2.0], [1.0, 1.0, 1.0, 1.0]],
            vec![[f32::NAN, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 1.0]],
            vec![[0.8, 0.0, 0.0, 0.0], [0.2, 1.0, 1.0, 1.0]],
            vec![[0.0, 0.0, 0.0, 0.0]; GRADIENT_MAP_MAX_STOPS + 1],
        ] {
            assert!(postcard::from_bytes::<P>(&encode(bad, 0)).is_err());
        }
        // A count that claims far more stops than the bytes hold: an error,
        // not an allocation.
        let mut hostile = vec![0xff, 0xff, 0xff, 0xff, 0x0f];
        hostile.extend_from_slice(&[0; 40]);
        assert!(postcard::from_bytes::<P>(&hostile).is_err());
        assert!(postcard::from_bytes::<P>(&[]).is_err());
        assert!(postcard::from_bytes::<P>(&[2]).is_err());
    }

    #[test]
    fn interpolation_bytes_round_trip_and_unknown_ones_are_refused() {
        assert_eq!(GradientInterpolation::Classic.byte(), 0);
        assert_eq!(
            GradientInterpolation::from_byte(0),
            Ok(GradientInterpolation::Classic)
        );
        assert_eq!(GradientInterpolation::from_byte(1), Err(E::Method));
    }
}
