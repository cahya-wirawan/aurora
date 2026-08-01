//! Colour transforms between two ICC profiles (ADR 0008: `lcms2`,
//! statically linked).

use aurora_core::Channels;

use crate::error::ColorError;
use crate::profile::IccProfile;

/// The four standard ICC rendering intents. Aurora's own enum rather than
/// re-exporting `lcms2::Intent` directly, so a future library swap (ADR
/// 0008 names `moxcms` as the first thing to benchmark against if `lcms2`
/// ever turns out to be a bottleneck) doesn't leak into every caller's
/// own code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderingIntent {
    #[default]
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl RenderingIntent {
    const fn to_lcms2(self) -> lcms2::Intent {
        match self {
            Self::Perceptual => lcms2::Intent::Perceptual,
            Self::RelativeColorimetric => lcms2::Intent::RelativeColorimetric,
            Self::Saturation => lcms2::Intent::Saturation,
            Self::AbsoluteColorimetric => lcms2::Intent::AbsoluteColorimetric,
        }
    }
}

/// The concrete `lcms2` transform for one channel layout. A Rust-level
/// `[f32; 4]` is shared by `Rgba` and `Cmyk` (same array size, different
/// `lcms2` pixel-format tag baked into the transform when it was built),
/// so the enum tag is what actually distinguishes them, not the type
/// parameter.
#[derive(Debug)]
enum Inner {
    Gray(lcms2::Transform<[f32; 1], [f32; 1]>),
    Rgb(lcms2::Transform<[f32; 3], [f32; 3]>),
    Rgba(lcms2::Transform<[f32; 4], [f32; 4]>),
    Cmyk(lcms2::Transform<[f32; 4], [f32; 4]>),
}

/// A colour transform between two loaded ICC profiles, operating on
/// planar-interleaved `f32` pixel data (ADR 0003's compute-precision
/// floor — no 8-bit intermediates anywhere in this crate).
///
/// **Supports four of [`Channels`]'s six variants**: `Gray`, `Rgb`,
/// `Rgba`, `Cmyk`. `GrayAlpha` and `CmykAlpha` are deliberately not wired
/// up yet — `lcms2` only exposes named floating-point pixel-format
/// constants for the four above; the alpha-carrying pair would need
/// hand-constructing `lcms2`'s own bitfield pixel-format encoding rather
/// than using one of its ready-made constants, a real but avoidable
/// expansion of scope with no caller needing it yet. Asking for one of
/// the other two returns [`ColorError::UnsupportedChannels`] rather than
/// silently doing something else.
///
/// **`Cmyk` is wired up but only exercised against RGB profiles in this
/// crate's own tests** (`GRAY_FLT`/`RGB_FLT`/`RGBA_FLT` are all verified
/// against real profiles; `CMYK_FLT` compiles and follows the identical
/// code path, but `corpora/icc/` has no real CMYK ICC profile to build a
/// transform from yet). ADR 0008's own follow-on names "test CMYK and
/// LUT-based profile transforms directly" as remaining work — this is
/// that gap, stated honestly rather than papered over with a test against
/// the wrong profile type.
#[derive(Debug)]
pub struct Transform {
    channels: Channels,
    inner: Inner,
}

impl Transform {
    /// Builds a transform from `src`'s colour space to `dst`'s, for
    /// `channels`-shaped pixels.
    ///
    /// # Errors
    ///
    /// Returns [`ColorError::UnsupportedChannels`] if `channels` is
    /// [`Channels::GrayAlpha`] or [`Channels::CmykAlpha`] (see this
    /// type's own doc comment), or [`ColorError::TransformCreationFailed`]
    /// if `lcms2` itself refuses to build the transform (e.g. a profile
    /// whose colour space doesn't match `channels`).
    pub fn new(
        src: &IccProfile,
        dst: &IccProfile,
        channels: Channels,
        intent: RenderingIntent,
    ) -> Result<Self, ColorError> {
        let intent = intent.to_lcms2();
        let inner = match channels {
            Channels::Gray => Inner::Gray(
                lcms2::Transform::new(
                    &src.inner,
                    lcms2::PixelFormat::GRAY_FLT,
                    &dst.inner,
                    lcms2::PixelFormat::GRAY_FLT,
                    intent,
                )
                .map_err(ColorError::TransformCreationFailed)?,
            ),
            Channels::Rgb => Inner::Rgb(
                lcms2::Transform::new(
                    &src.inner,
                    lcms2::PixelFormat::RGB_FLT,
                    &dst.inner,
                    lcms2::PixelFormat::RGB_FLT,
                    intent,
                )
                .map_err(ColorError::TransformCreationFailed)?,
            ),
            Channels::Rgba => Inner::Rgba(
                lcms2::Transform::new(
                    &src.inner,
                    lcms2::PixelFormat::RGBA_FLT,
                    &dst.inner,
                    lcms2::PixelFormat::RGBA_FLT,
                    intent,
                )
                .map_err(ColorError::TransformCreationFailed)?,
            ),
            Channels::Cmyk => Inner::Cmyk(
                lcms2::Transform::new(
                    &src.inner,
                    lcms2::PixelFormat::CMYK_FLT,
                    &dst.inner,
                    lcms2::PixelFormat::CMYK_FLT,
                    intent,
                )
                .map_err(ColorError::TransformCreationFailed)?,
            ),
            Channels::GrayAlpha | Channels::CmykAlpha => {
                return Err(ColorError::UnsupportedChannels(channels));
            }
        };
        Ok(Self { channels, inner })
    }

    /// Transforms `src` into `dst`, both flat, interleaved `f32` sample
    /// buffers (channel-minor: every `channels.count()` consecutive
    /// samples are one pixel). Extended-range values (negative, or above
    /// `1.0`) survive the transform rather than being clamped — invariant
    /// §7.3.1b, verified against `lcms2`'s actual behaviour in this
    /// crate's own tests (`spike/raw-icc/FINDINGS.md` finding 4's named
    /// follow-on).
    ///
    /// # Errors
    ///
    /// Returns [`ColorError::BufferLengthMismatch`] if `src`/`dst` aren't
    /// the same length, or that length isn't an exact multiple of this
    /// transform's channel count. Nothing is written to `dst` when this
    /// happens.
    pub fn transform(&self, src: &[f32], dst: &mut [f32]) -> Result<(), ColorError> {
        let channel_count = self.channels.count();
        if src.len() != dst.len() || !src.len().is_multiple_of(usize::from(channel_count)) {
            return Err(ColorError::BufferLengthMismatch {
                channels: self.channels,
                channel_count,
                src_len: src.len(),
                dst_len: dst.len(),
            });
        }

        match &self.inner {
            Inner::Gray(transform) => {
                let src_pixels = to_pixels::<1>(src);
                let mut dst_pixels = vec![[0.0f32; 1]; src_pixels.len()];
                transform.transform_pixels(&src_pixels, &mut dst_pixels);
                from_pixels(&dst_pixels, dst);
            }
            Inner::Rgb(transform) => {
                let src_pixels = to_pixels::<3>(src);
                let mut dst_pixels = vec![[0.0f32; 3]; src_pixels.len()];
                transform.transform_pixels(&src_pixels, &mut dst_pixels);
                from_pixels(&dst_pixels, dst);
            }
            Inner::Rgba(transform) | Inner::Cmyk(transform) => {
                let src_pixels = to_pixels::<4>(src);
                let mut dst_pixels = vec![[0.0f32; 4]; src_pixels.len()];
                transform.transform_pixels(&src_pixels, &mut dst_pixels);
                from_pixels(&dst_pixels, dst);
            }
        }
        Ok(())
    }
}

/// Reshapes a flat, channel-minor sample buffer into one fixed-size array
/// per pixel — the shape `lcms2::Transform::transform_pixels` needs.
/// `flat.len()` is already validated to be an exact multiple of `N` by
/// [`Transform::transform`], so `chunks_exact` never silently drops a
/// trailing partial chunk here.
fn to_pixels<const N: usize>(flat: &[f32]) -> Vec<[f32; N]> {
    flat.chunks_exact(N)
        .map(|chunk| {
            let mut pixel = [0.0f32; N];
            pixel.copy_from_slice(chunk);
            pixel
        })
        .collect()
}

/// The inverse of [`to_pixels`]: flattens transformed pixel arrays back
/// into the caller's own flat output buffer.
fn from_pixels<const N: usize>(pixels: &[[f32; N]], flat: &mut [f32]) {
    for (chunk, pixel) in flat.chunks_exact_mut(N).zip(pixels) {
        chunk.copy_from_slice(pixel);
    }
}

#[cfg(test)]
mod tests {
    use super::{RenderingIntent, Transform};
    use crate::ColorError;
    use crate::profile::IccProfile;
    use aurora_core::Channels;

    // CC0-licensed, from the colord-data Debian package -- see
    // corpora/icc/README.md for full provenance. Same two profiles and
    // the same sRGB -> ECI-RGBv2 transform `spike/raw-icc/FINDINGS.md`
    // cross-validated against `moxcms`.
    const SRGB_ICC: &[u8] = include_bytes!("../../../corpora/icc/sRGB.icc");
    const ECI_RGBV2_ICC: &[u8] = include_bytes!("../../../corpora/icc/ECI-RGBv2.icc");

    fn srgb_to_eci_transform() -> Transform {
        let src = match IccProfile::from_bytes(SRGB_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        let dst = match IccProfile::from_bytes(ECI_RGBV2_ICC) {
            Ok(profile) => profile,
            Err(err) => unreachable!("{err:?}"),
        };
        match Transform::new(
            &src,
            &dst,
            Channels::Rgb,
            RenderingIntent::RelativeColorimetric,
        ) {
            Ok(transform) => transform,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    // Comparing against FINDINGS.md's own recorded 4-decimal-place
    // values, not a computed expectation -- same reasoning `tree::tests`
    // already documents for its own float_cmp allows, applied to
    // literals transcribed from a real, cross-validated spike result
    // rather than freshly computed here.
    #[allow(clippy::float_cmp)]
    fn matches_the_spikes_own_cross_validated_results() {
        let transform = srgb_to_eci_transform();

        // spike/raw-icc/FINDINGS.md's own recorded results, to 4 decimal
        // places, for sRGB -> ECI-RGBv2, RelativeColorimetric.
        let cases: &[([f32; 3], [f32; 3])] = &[
            ([1.0, 1.0, 1.0], [1.0000, 1.0000, 1.0000]),
            ([0.0, 0.0, 0.0], [0.0000, 0.0000, 0.0000]),
            ([0.5, 0.5, 0.5], [0.5339, 0.5339, 0.5339]),
            ([1.0, 0.0, 0.0], [0.8514, 0.1237, 0.1387]),
            ([0.0, 1.0, 0.0], [0.6204, 1.0093, 0.2249]),
            ([0.0, 0.0, 1.0], [0.2115, -0.3513, 0.9789]),
        ];

        for (src, expected) in cases {
            let mut dst = [0.0f32; 3];
            if let Err(err) = transform.transform(src, &mut dst) {
                unreachable!("{err:?}");
            }
            for (got, want) in dst.iter().zip(expected) {
                let rounded = (got * 10_000.0).round() / 10_000.0;
                assert_eq!(
                    rounded, *want,
                    "channel of {src:?}: got {dst:?}, expected {expected:?}"
                );
            }
        }
    }

    #[test]
    fn extended_range_values_survive_rather_than_clamp() {
        // The spike's own near-miss (FINDINGS.md finding 4): blue goes
        // out of ECI-RGBv2's gamut on the G channel. A CMS that silently
        // clamps to [0, 1] -- moxcms's own default, before the spike
        // found its `allow_extended_range_rgb_xyz` flag -- would report
        // 0.0 here instead. `lcms2` needed no special flag in the spike;
        // this test is the permanent regression check FINDINGS.md finding
        // 4 asked for, not just a one-off spike observation.
        let transform = srgb_to_eci_transform();
        let mut dst = [0.0f32; 3];
        if let Err(err) = transform.transform(&[0.0, 0.0, 1.0], &mut dst) {
            unreachable!("{err:?}");
        }
        assert!(
            dst[1] < 0.0,
            "expected a negative, out-of-gamut G channel, got {dst:?} -- \
             extended-range values must survive the transform, not clamp to 0"
        );
    }

    #[test]
    fn transform_rejects_a_length_mismatch() {
        let transform = srgb_to_eci_transform();
        let src = [0.0f32; 3];
        let mut dst = [0.0f32; 6];
        match transform.transform(&src, &mut dst) {
            Err(ColorError::BufferLengthMismatch {
                src_len, dst_len, ..
            }) => {
                assert_eq!(src_len, 3);
                assert_eq!(dst_len, 6);
            }
            other => unreachable!("expected BufferLengthMismatch, got {other:?}"),
        }
    }

    #[test]
    fn transform_rejects_a_length_not_a_multiple_of_channel_count() {
        let transform = srgb_to_eci_transform();
        let src = [0.0f32; 4]; // not a multiple of 3 (Rgb)
        let mut dst = [0.0f32; 4];
        match transform.transform(&src, &mut dst) {
            Err(ColorError::BufferLengthMismatch { channel_count, .. }) => {
                assert_eq!(channel_count, 3);
            }
            other => unreachable!("expected BufferLengthMismatch, got {other:?}"),
        }
    }

    #[test]
    fn transform_handles_multiple_pixels_in_one_call() {
        let transform = srgb_to_eci_transform();
        // white, then black -- two pixels in one flat buffer.
        let src = [1.0f32, 1.0, 1.0, 0.0, 0.0, 0.0];
        let mut dst = [0.0f32; 6];
        if let Err(err) = transform.transform(&src, &mut dst) {
            unreachable!("{err:?}");
        }
        // A small epsilon, not exact equality: lcms2's own float transform
        // path has real, tiny (~1e-5) numerical noise even on round-trip-ish
        // colours -- confirmed by running this first without a tolerance
        // and seeing white come back as [0.9999987, 1.0000117, 0.9999912],
        // not exactly [1.0, 1.0, 1.0]. This is the "accumulated computation
        // noise" case clippy::float_cmp warns about, unlike the bit-exact
        // literal comparisons elsewhere in this crate's tests.
        for (got, expected) in dst.iter().zip([1.0, 1.0, 1.0, 0.0, 0.0, 0.0]) {
            assert!(
                (got - expected).abs() < 0.001,
                "got {dst:?}, expected approximately [1.0, 1.0, 1.0, 0.0, 0.0, 0.0]"
            );
        }
    }

    #[test]
    fn new_rejects_unsupported_channel_layouts() {
        let src = IccProfile::srgb();
        let dst = IccProfile::srgb();
        for channels in [Channels::GrayAlpha, Channels::CmykAlpha] {
            match Transform::new(&src, &dst, channels, RenderingIntent::Perceptual) {
                Err(ColorError::UnsupportedChannels(got)) => assert_eq!(got, channels),
                other => unreachable!("expected UnsupportedChannels, got {other:?}"),
            }
        }
    }
}
