//! CMYK pixels into the working space (task 146): judging an embedded CMYK
//! ICC profile and converting ink coverage through it to encoded sRGB.
//!
//! The input is **ink coverage**, `0.0` (no ink) to `1.0` (full ink) in each
//! of cyan, magenta, yellow and black; a caller reading a Photoshop file
//! must invert its stored channels first (Photoshop stores `255` for "no
//! ink"). The transform is relative colorimetric with black point
//! compensation, the same policy as [`Transform::to_working`] for RGB.
//!
//! `lcms2` takes floating-point CMYK as a percentage (`0..=100`); that scale
//! is hidden here (`transform` multiplies by 100), and a test pins it.

use crate::error::ColorError;
use crate::profile::IccProfile;
use crate::source::{ProfileProblem, ProfileSpace, parse_header, profile_description};

/// An embedded profile judged usable for CMYK pixels.
#[derive(Debug)]
pub struct CmykSource {
    /// The parsed profile.
    pub profile: IccProfile,
    /// Its own name, or a generic one when it has none.
    pub description: String,
}

/// Judges `bytes` as the profile of CMYK pixel data.
///
/// # Errors
///
/// A [`ProfileProblem`] naming why the profile cannot be used: too large,
/// not ICC, not a CMYK profile, a class that cannot describe pixels, or
/// refused by `lcms2`.
pub fn inspect_cmyk_source(bytes: &[u8]) -> Result<CmykSource, ProfileProblem> {
    let header = parse_header(bytes)?;
    if header.space != ProfileSpace::Cmyk {
        return Err(ProfileProblem::WrongSpace(header.space));
    }
    // Printer (output), scanner (input), display and colour-space profiles
    // can describe CMYK pixels; abstract, link and named-colour ones cannot.
    if !matches!(&header.class, b"mntr" | b"scnr" | b"spac" | b"prtr") {
        return Err(ProfileProblem::WrongClass(header.class));
    }
    let profile = IccProfile::from_bytes(bytes)
        .map_err(|_| ProfileProblem::Rejected("it could not be read as an ICC profile"))?;
    // Building the transform is the real test of whether it can be used.
    CmykTransform::to_working(&profile)
        .map_err(|_| ProfileProblem::Rejected("it cannot be used to describe image pixels"))?;
    let description =
        profile_description(bytes).unwrap_or_else(|| "an unnamed CMYK profile".to_owned());
    Ok(CmykSource {
        profile,
        description,
    })
}

/// A transform from a CMYK profile to Aurora's working space (encoded
/// sRGB), over `f32` pixels.
#[derive(Debug)]
pub struct CmykTransform {
    inner: lcms2::Transform<[f32; 4], [f32; 3]>,
}

impl CmykTransform {
    /// CMYK in `profile` to encoded sRGB: relative colorimetric with black
    /// point compensation.
    ///
    /// # Errors
    ///
    /// [`ColorError::TransformCreationFailed`] if `lcms2` refuses (for
    /// example a profile that is not CMYK).
    pub fn to_working(profile: &IccProfile) -> Result<Self, ColorError> {
        let srgb = IccProfile::srgb();
        let inner = lcms2::Transform::new_flags(
            &profile.inner,
            lcms2::PixelFormat::CMYK_FLT,
            &srgb.inner,
            lcms2::PixelFormat::RGB_FLT,
            lcms2::Intent::RelativeColorimetric,
            lcms2::Flags::BLACKPOINT_COMPENSATION,
        )
        .map_err(ColorError::TransformCreationFailed)?;
        Ok(Self { inner })
    }

    /// Converts `ink` (four ink coverages in `0..=1` per pixel) into `rgb`
    /// (three encoded sRGB samples per pixel; out-of-gamut results are not
    /// clamped).
    ///
    /// # Errors
    ///
    /// [`ColorError::BufferLengthMismatch`] if `ink` is not a whole number
    /// of pixels or `rgb` is not exactly three-quarters of its length.
    pub fn transform(&self, ink: &[f32], rgb: &mut [f32]) -> Result<(), ColorError> {
        if !ink.len().is_multiple_of(4) || rgb.len() != ink.len() / 4 * 3 {
            return Err(ColorError::BufferLengthMismatch {
                channels: aurora_core::Channels::Cmyk,
                channel_count: 4,
                src_len: ink.len(),
                dst_len: rgb.len(),
            });
        }
        let src: Vec<[f32; 4]> = ink
            .chunks_exact(4)
            .map(|c| {
                let mut p = [0.0_f32; 4];
                for (slot, v) in p.iter_mut().zip(c) {
                    *slot = v * 100.0;
                }
                p
            })
            .collect();
        let mut dst = vec![[0.0_f32; 3]; src.len()];
        self.inner.transform_pixels(&src, &mut dst);
        for (out, px) in rgb.chunks_exact_mut(3).zip(&dst) {
            out.copy_from_slice(px);
        }
        Ok(())
    }
}

/// The naive CMYK to sRGB formula, `rgb = (1 - ink) * (1 - black)` per
/// channel, on encoded values. What a file with no CMYK profile is shown
/// with: it knows nothing about real inks and paper, so it is saturated and
/// light compared with any press profile.
#[must_use]
pub fn naive_cmyk_to_rgb(ink: [f32; 4]) -> [f32; 3] {
    let k = 1.0 - ink[3];
    [(1.0 - ink[0]) * k, (1.0 - ink[1]) * k, (1.0 - ink[2]) * k]
}

impl IccProfile {
    /// A coarse synthetic CMYK printer profile (two grid nodes per ink): the
    /// sixteen corner colours of the naive formula
    /// ([`naive_cmyk_to_rgb`]) as Lab, interpolated linearly. Not a model of
    /// any press; it lets tests and tools make a valid CMYK profile without
    /// a file.
    ///
    /// # Errors
    ///
    /// [`ColorError::InvalidProfile`] if `lcms2` rejects the generated
    /// bytes (it does not).
    pub fn synthetic_cmyk() -> Result<Self, ColorError> {
        Self::from_bytes(&synthetic_cmyk_bytes())
    }
}

/// `f64` to ICC `s15Fixed16`.
fn s15(v: f64) -> [u8; 4] {
    #[allow(clippy::cast_possible_truncation)]
    let fixed = (v * 65_536.0).round() as i32;
    fixed.to_be_bytes()
}

/// Encoded sRGB to CIE Lab (D50, Bradford-adapted), `f64`.
fn srgb_to_lab(rgb: [f64; 3]) -> [f64; 3] {
    let lin = |c: f64| {
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = rgb.map(lin);
    // Linear sRGB to XYZ, D50 (Lindbloom).
    let x = 0.436_074_7 * r + 0.385_064_9 * g + 0.143_080_4 * b;
    let y = 0.222_504_5 * r + 0.716_878_6 * g + 0.060_616_9 * b;
    let z = 0.013_932_2 * r + 0.097_104_5 * g + 0.714_173_3 * b;
    let f = |t: f64| {
        if t > 216.0 / 24_389.0 {
            t.cbrt()
        } else {
            (24_389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / 0.964_22), f(y), f(z / 0.825_21));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The bytes of the synthetic profile: an ICC 2.1 printer profile with a
/// `lut16` A2B0 table of 4 inputs, 3 outputs and 2 grid nodes.
fn synthetic_cmyk_bytes() -> Vec<u8> {
    let be16 = |v: u16| v.to_be_bytes();
    // --- lut16 (A2B0) ---
    let mut lut: Vec<u8> = Vec::new();
    lut.extend_from_slice(b"mft2");
    lut.extend_from_slice(&[0; 4]);
    lut.extend_from_slice(&[4, 3, 2, 0]);
    for v in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        lut.extend_from_slice(&s15(v));
    }
    lut.extend_from_slice(&be16(2));
    lut.extend_from_slice(&be16(2));
    for _ in 0..4 {
        lut.extend_from_slice(&be16(0));
        lut.extend_from_slice(&be16(65_535));
    }
    // The first input (cyan) varies slowest.
    for index in 0..16_u32 {
        let ink = [
            f64::from((index >> 3) & 1),
            f64::from((index >> 2) & 1),
            f64::from((index >> 1) & 1),
            f64::from(index & 1),
        ];
        let rgb = naive_cmyk_to_rgb(ink.map(|v| v as f32)).map(f64::from);
        let [l, a, b] = srgb_to_lab(rgb);
        // The legacy 16-bit Lab encoding: L 0..100 over 0..0xFF00, a and b
        // -128..127 over 0..0xFF00 (0x8000 is zero).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let enc = |v: f64| (v.clamp(0.0, 65_280.0)).round() as u16;
        for v in [
            enc(l / 100.0 * 65_280.0),
            enc((a + 128.0) * 256.0),
            enc((b + 128.0) * 256.0),
        ] {
            lut.extend_from_slice(&be16(v));
        }
    }
    for _ in 0..3 {
        lut.extend_from_slice(&be16(0));
        lut.extend_from_slice(&be16(65_535));
    }
    // --- the other tags ---
    let mut desc = Vec::new();
    desc.extend_from_slice(b"desc");
    desc.extend_from_slice(&[0; 4]);
    let name = b"Aurora synthetic CMYK\0";
    desc.extend_from_slice(&(name.len() as u32).to_be_bytes());
    desc.extend_from_slice(name);
    desc.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]);
    let mut cprt = Vec::new();
    cprt.extend_from_slice(b"text");
    cprt.extend_from_slice(&[0; 4]);
    cprt.extend_from_slice(b"No copyright, generated\0");
    let mut wtpt = Vec::new();
    wtpt.extend_from_slice(b"XYZ ");
    wtpt.extend_from_slice(&[0; 4]);
    for v in [0.964_22, 1.0, 0.825_21] {
        wtpt.extend_from_slice(&s15(v));
    }
    // --- assemble ---
    let tags: [(&[u8; 4], &[u8]); 4] = [
        (b"desc", &desc),
        (b"cprt", &cprt),
        (b"wtpt", &wtpt),
        (b"A2B0", &lut),
    ];
    let table_len = 4 + tags.len() * 12;
    let mut body: Vec<u8> = Vec::new();
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    for (sig, data) in &tags {
        let offset = 128 + table_len + body.len();
        table.extend_from_slice(*sig);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
    }
    let total = 128 + table.len() + body.len();
    let mut out: Vec<u8> = Vec::with_capacity(total);
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&[0; 4]); // CMM
    out.extend_from_slice(&[2, 0x10, 0, 0]); // version 2.1
    out.extend_from_slice(b"prtrCMYKLab ");
    out.extend_from_slice(&[0; 12]); // date
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0; 4 + 4 + 4 + 4 + 8 + 4]); // platform .. intent
    for v in [0.964_22, 1.0, 0.825_21] {
        out.extend_from_slice(&s15(v)); // illuminant
    }
    out.extend_from_slice(&[0; 4 + 16 + 28]); // creator, id, reserved
    debug_assert_eq!(out.len(), 128);
    out.extend_from_slice(&table);
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(profile: &IccProfile, ink: [f32; 4]) -> [f32; 3] {
        let Ok(t) = CmykTransform::to_working(profile) else {
            unreachable!("transform")
        };
        let mut rgb = [0.0_f32; 3];
        let Ok(()) = t.transform(&ink, &mut rgb) else {
            unreachable!("convert")
        };
        rgb
    }

    fn synthetic() -> IccProfile {
        let Ok(p) = IccProfile::synthetic_cmyk() else {
            unreachable!("synthetic CMYK profile")
        };
        p
    }

    #[test]
    fn the_synthetic_profile_is_a_valid_cmyk_printer_profile_and_converts_its_corners() {
        let bytes = synthetic_cmyk_bytes();
        let Ok(header) = parse_header(&bytes) else {
            unreachable!("header")
        };
        assert_eq!(header.space, ProfileSpace::Cmyk);
        assert_eq!(&header.class, b"prtr");
        assert!(inspect_cmyk_source(&bytes).is_ok());
        let p = synthetic();
        // The corners are the naive formula's colours (to the Lab
        // quantisation and the rendering intent's rounding).
        for (ink, want) in [
            ([0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 1.0]),
            ([0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0, 0.0], [1.0, 1.0, 0.0]),
        ] {
            let got = convert(&p, ink);
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() < 0.03, "{ink:?}: got {got:?}, want {want:?}");
            }
        }
        // Full black ink is dark (black point compensation maps it to black).
        let black = convert(&p, [0.0, 0.0, 0.0, 1.0]);
        assert!(black.iter().all(|c| c.abs() < 0.05), "{black:?}");
    }

    #[test]
    fn lcms_takes_floating_point_cmyk_as_a_percentage_and_this_hides_it() {
        let p = synthetic();
        // Half black ink is a mid grey, neither white (the 0..1 reading of a
        // percentage would give 0.5 percent ink) nor black.
        let half = convert(&p, [0.0, 0.0, 0.0, 0.5]);
        for c in half {
            assert!((0.3..0.8).contains(&c), "{half:?}");
        }
        // And a ramp is monotone.
        let quarter = convert(&p, [0.0, 0.0, 0.0, 0.25])[1];
        let three_quarters = convert(&p, [0.0, 0.0, 0.0, 0.75])[1];
        assert!(quarter > half[1] && half[1] > three_quarters);
    }

    #[test]
    fn a_non_cmyk_or_damaged_profile_is_refused_with_a_reason() {
        let rgb = IccProfile::srgb().to_bytes().unwrap_or_default();
        assert!(matches!(
            inspect_cmyk_source(&rgb),
            Err(ProfileProblem::WrongSpace(ProfileSpace::Rgb))
        ));
        assert!(matches!(
            inspect_cmyk_source(b"junk"),
            Err(ProfileProblem::NotIcc(_))
        ));
        let mut linkish = synthetic_cmyk_bytes();
        linkish[12..16].copy_from_slice(b"link");
        assert!(matches!(
            inspect_cmyk_source(&linkish),
            Err(ProfileProblem::WrongClass(_))
        ));
        // A CMYK profile whose tables are cut off is refused, not a panic.
        let mut cut = synthetic_cmyk_bytes();
        let keep = cut.len() - 40;
        cut.truncate(keep);
        assert!(inspect_cmyk_source(&cut).is_err());
    }

    #[test]
    fn transform_refuses_mismatched_buffers() {
        let Ok(t) = CmykTransform::to_working(&synthetic()) else {
            unreachable!("transform")
        };
        let mut rgb = [0.0_f32; 3];
        assert!(t.transform(&[0.0; 5], &mut rgb).is_err());
        assert!(t.transform(&[0.0; 4], &mut [0.0; 6]).is_err());
    }

    #[test]
    fn the_naive_formula_is_white_without_ink_and_black_with_full_black() {
        assert_eq!(naive_cmyk_to_rgb([0.0; 4]), [1.0; 3]);
        assert_eq!(naive_cmyk_to_rgb([0.0, 0.0, 0.0, 1.0]), [0.0; 3]);
        assert_eq!(naive_cmyk_to_rgb([1.0, 0.0, 0.0, 0.0]), [0.0, 1.0, 1.0]);
    }
}
