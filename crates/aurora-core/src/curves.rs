//! The parameters of a Curves adjustment (0.155.0): one composite RGB
//! [`ToneCurve`] plus an optional curve per colour channel, which is
//! Photoshop's own Curves model (its `curv` block stores a master curve
//! and per-channel curves, any of which may be absent).
//!
//! It lives in `aurora-core`, beside [`ToneCurve`], for the same layering
//! reason the curve does (`scripts/layering.json`): `aurora-doc` stores
//! it in a layer, `aurora-filters` builds the lookup tables that apply
//! it, and neither of those crates may depend on the other.
//!
//! **Order of application** is per-channel first, then the composite
//! curve: `out_c = composite(channel_c(in_c))`. That is the order
//! psd-tools' Photoshop-checked compositor uses (`_apply_luts`:
//! "individual adjustments get applied independently of each other, then
//! the master lut is applied"); `aurora_filters::CurvesLut` implements
//! exactly this order.
//!
//! **Wire format** (`.aur` manifests and the history journal, both
//! positional `postcard`): the four fields in declaration order, each
//! curve as its point list (see [`ToneCurve`]'s `Serialize`). A curve
//! that fails [`ToneCurve::new`]'s validation on decode is a decode
//! error, never a silently repaired curve.

use crate::tone_curve::ToneCurve;

/// A Curves adjustment's parameters. See the module docs for the model
/// and the order the curves apply in.
///
/// `None` for a channel curve means "no curve on this channel", which
/// applies exactly like an identity curve.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CurvesParams {
    /// The composite (RGB) curve, applied to every colour channel after
    /// that channel's own curve.
    pub composite: ToneCurve,
    /// The red channel's own curve, if any.
    pub red: Option<ToneCurve>,
    /// The green channel's own curve, if any.
    pub green: Option<ToneCurve>,
    /// The blue channel's own curve, if any.
    pub blue: Option<ToneCurve>,
}

// Every coordinate of every `ToneCurve` is finite (its own validated
// invariant) and its tangents are a deterministic function of its
// points, so `PartialEq` here is a true equivalence relation: no NaN can
// make a value unequal to itself.
impl Eq for CurvesParams {}

impl CurvesParams {
    /// Every curve the identity: the adjustment changes nothing.
    #[must_use]
    pub fn identity() -> Self {
        Self::default()
    }

    /// The channel curve for colour channel `index` (`0` red, `1` green,
    /// `2` blue), or `None` for no curve or any other index.
    #[must_use]
    pub fn channel(&self, index: usize) -> Option<&ToneCurve> {
        match index {
            0 => self.red.as_ref(),
            1 => self.green.as_ref(),
            2 => self.blue.as_ref(),
            _ => None,
        }
    }

    /// Whether every curve present is the two-point identity, so the
    /// adjustment is an exact passthrough.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        let identity = |curve: &ToneCurve| curve.is_identity();
        identity(&self.composite)
            && [&self.red, &self.green, &self.blue]
                .iter()
                .all(|curve| curve.as_ref().is_none_or(identity))
    }
}

#[cfg(test)]
mod tests {
    use super::CurvesParams;
    use crate::tone_curve::{CurvePoint, ToneCurve};

    #[test]
    fn identity_params_report_identity_and_a_bent_channel_does_not() {
        assert!(CurvesParams::identity().is_identity());
        let mut params = CurvesParams::identity();
        params.green = Some(ToneCurve::identity());
        assert!(
            params.is_identity(),
            "an explicit identity channel is still identity"
        );
        params.green = match ToneCurve::new(&[CurvePoint::new(0.0, 0.2), CurvePoint::new(1.0, 1.0)])
        {
            Ok(curve) => Some(curve),
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(!params.is_identity());
        assert!(params.channel(1).is_some());
        assert!(params.channel(0).is_none());
        assert!(params.channel(3).is_none());
    }

    #[test]
    fn postcard_round_trips_and_refuses_an_invalid_or_oversized_curve() {
        let params = CurvesParams {
            composite: match ToneCurve::new(&[
                CurvePoint::new(0.0, 0.1),
                CurvePoint::new(0.5, 0.7),
                CurvePoint::new(1.0, 0.9),
            ]) {
                Ok(curve) => curve,
                Err(err) => unreachable!("{err:?}"),
            },
            red: None,
            green: Some(ToneCurve::identity()),
            blue: None,
        };
        let bytes = match postcard::to_allocvec(&params) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        match postcard::from_bytes::<CurvesParams>(&bytes) {
            Ok(decoded) => assert_eq!(decoded, params),
            Err(err) => unreachable!("must round trip: {err:?}"),
        }

        // A point list that fails validation: out of order (a moved
        // endpoint, `(0.1, 0.0)` first, is valid since 0.158.0).
        let bad: Vec<(f32, f32)> = vec![(0.5, 0.0), (0.1, 1.0)];
        let bytes = match postcard::to_allocvec(&bad) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(postcard::from_bytes::<ToneCurve>(&bytes).is_err());

        // Twenty points, each individually valid-looking: one past
        // Photoshop's 19 (`MAX_POINTS` since 0.157.0; the old test used 17,
        // one past the old cap of 16).
        #[allow(clippy::cast_precision_loss)]
        let many: Vec<(f32, f32)> = (0..20).map(|i| (i as f32 / 19.0, 0.5)).collect();
        let bytes = match postcard::to_allocvec(&many) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(postcard::from_bytes::<ToneCurve>(&bytes).is_err());

        // A hostile length prefix with no elements behind it: an error, not
        // an allocation of the claimed size.
        assert!(postcard::from_bytes::<ToneCurve>(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).is_err());
        // NaN coordinates.
        let nan: Vec<(f32, f32)> = vec![(0.0, f32::NAN), (1.0, 1.0)];
        let bytes = match postcard::to_allocvec(&nan) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(postcard::from_bytes::<ToneCurve>(&bytes).is_err());
    }

    /// 0.157.0 raised the cap from 16 to 19 points: a 19-point curve now
    /// decodes, and so does every curve a 0.155.0/0.156.0 build could
    /// write (16 points or fewer) — the point-list encoding itself did not
    /// change. (An older build refuses a 17-to-19-point curve.)
    #[test]
    fn nineteen_points_decode_and_sixteen_still_do() {
        for n in [2_u16, 16, 17, 19] {
            let points: Vec<(f32, f32)> = (0..n)
                .map(|i| (f32::from(i) / f32::from(n - 1), 0.25))
                .collect();
            let bytes = match postcard::to_allocvec(&points) {
                Ok(bytes) => bytes,
                Err(err) => unreachable!("{err:?}"),
            };
            match postcard::from_bytes::<ToneCurve>(&bytes) {
                Ok(curve) => assert_eq!(curve.points().len(), usize::from(n)),
                Err(err) => unreachable!("{n} points must decode: {err:?}"),
            }
        }
    }
}
