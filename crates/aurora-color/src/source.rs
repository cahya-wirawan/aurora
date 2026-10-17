//! Judging an embedded ICC profile that arrives with RGB pixels (round 1 of
//! colour management): is it a profile Aurora can honour for RGB data, what
//! does it call itself, and is it the sRGB working space under another
//! name?
//!
//! Everything here is bounded and total. A profile is hostile input: its
//! length is checked before anything parses it ([`MAX_PROFILE_BYTES`]), the
//! header and the description tag are read with checked slicing only (no
//! `lcms2` string call — the crate's `Profile::info` can panic on a
//! non-scalar UTF-16 unit), and a profile that is the wrong kind for RGB
//! pixels comes back as a [`ProfileProblem`] with a sentence a person can
//! read, never as a panic and never as a silent fallback.
//!
//! **Working space.** Aurora's pixels are encoded sRGB ([`IccProfile::srgb`]).
//! A source profile is converted into it with [`Transform::to_working`]:
//! relative colorimetric with black point compensation (Adobe's default
//! engine settings and a no-op for matrix/TRC profiles, where black is
//! already zero). [`Transform::new`] keeps its older defaults (no black
//! point compensation); the difference is stated in
//! [`Transform::new_with_options`].

use aurora_core::Channels;

use crate::error::ColorError;
use crate::profile::IccProfile;
use crate::transform::{RenderingIntent, Transform};

/// The longest profile accepted, in bytes. Real RGB profiles are a few
/// kilobytes (matrix/TRC) to a few hundred (LUT); the largest common ones
/// (some printer profiles) are a few megabytes.
pub const MAX_PROFILE_BYTES: usize = 8 * 1024 * 1024;

/// How far (in encoded sRGB, `0..=1`) a profile may drift from the built-in
/// sRGB over a lattice of colours and still count as sRGB: two 8-bit steps.
/// Real sRGB profiles differ from `lcms2`'s analytic curve by a fraction of
/// a step to about one and a half (the corpus's colord sRGB measures 1.4
/// steps, in the darks), and converting through one would only add noise;
/// a gamma-2.2 stand-in is over 5 steps off and is converted. A choice, not
/// a standard.
pub const SRGB_EQUIVALENCE_TOLERANCE: f32 = 2.0 / 255.0;

/// The longest description kept, in characters.
const MAX_DESCRIPTION_CHARS: usize = 96;

/// The smallest legal ICC profile: the 128-byte header.
const HEADER_LEN: usize = 128;

/// What an ICC header names as the data colour space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSpace {
    Rgb,
    Gray,
    Cmyk,
    Lab,
    /// Anything else (XYZ, YCbCr, n-colour, ...).
    Other,
}

impl ProfileSpace {
    const fn name(self) -> &'static str {
        match self {
            Self::Rgb => "an RGB",
            Self::Gray => "a grayscale",
            Self::Cmyk => "a CMYK",
            Self::Lab => "a Lab",
            Self::Other => "a non-RGB",
        }
    }
}

/// Why a profile cannot be used for RGB pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileProblem {
    /// More than [`MAX_PROFILE_BYTES`].
    TooLarge(usize),
    /// Not an ICC profile at all (too short, no `acsp` signature, a
    /// declared size past the data).
    NotIcc(&'static str),
    /// The profile describes a different colour space than RGB data.
    WrongSpace(ProfileSpace),
    /// A profile class that cannot describe pixels: abstract, device link
    /// or named colour.
    WrongClass([u8; 4]),
    /// `lcms2` refused it, or refused to use it as an input profile.
    Rejected(&'static str),
}

impl ProfileProblem {
    /// A plain-language fragment, for an open report ("`<this>`").
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::TooLarge(len) => {
                format!("it is {len} bytes, larger than the {MAX_PROFILE_BYTES} allowed")
            }
            Self::NotIcc(why) => format!("it is not a valid ICC profile ({why})"),
            Self::WrongSpace(space) => {
                format!("it describes {} colour space, not RGB", space.name())
            }
            Self::WrongClass(class) => format!(
                "its class \"{}\" cannot describe image pixels",
                class
                    .iter()
                    .map(|&b| if b.is_ascii_graphic() {
                        char::from(b)
                    } else {
                        '?'
                    })
                    .collect::<String>()
            ),
            Self::Rejected(why) => (*why).to_owned(),
        }
    }
}

/// The parts of an ICC header the policy needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileHeader {
    pub space: ProfileSpace,
    /// The device class signature (`mntr`, `scnr`, `prtr`, `spac`, `abst`,
    /// `link`, `nmcl`).
    pub class: [u8; 4],
}

/// Reads the header of `bytes` without parsing the profile.
///
/// # Errors
///
/// [`ProfileProblem::TooLarge`] or [`ProfileProblem::NotIcc`].
pub fn parse_header(bytes: &[u8]) -> Result<ProfileHeader, ProfileProblem> {
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(ProfileProblem::TooLarge(bytes.len()));
    }
    if bytes.len() < HEADER_LEN {
        return Err(ProfileProblem::NotIcc("shorter than an ICC header"));
    }
    let word = |at: usize| -> Option<[u8; 4]> {
        bytes
            .get(at..at.checked_add(4)?)
            .and_then(|s| <[u8; 4]>::try_from(s).ok())
    };
    if word(36) != Some(*b"acsp") {
        return Err(ProfileProblem::NotIcc("no ICC signature"));
    }
    let declared = word(0).map_or(0, u32::from_be_bytes);
    if (declared as usize) < HEADER_LEN || declared as usize > bytes.len() {
        return Err(ProfileProblem::NotIcc(
            "its declared size does not fit the data",
        ));
    }
    let space = match word(16) {
        Some(sig) if &sig == b"RGB " => ProfileSpace::Rgb,
        Some(sig) if &sig == b"GRAY" => ProfileSpace::Gray,
        Some(sig) if &sig == b"CMYK" => ProfileSpace::Cmyk,
        Some(sig) if &sig == b"Lab " => ProfileSpace::Lab,
        _ => ProfileSpace::Other,
    };
    Ok(ProfileHeader {
        space,
        class: word(12).unwrap_or([0; 4]),
    })
}

/// The profile's description (its `desc` tag, ICC v2 `desc` or v4 `mluc`),
/// cleaned of control characters and capped; `None` when absent or
/// unreadable. Checked slicing throughout.
#[must_use]
pub fn profile_description(bytes: &[u8]) -> Option<String> {
    let u32_at = |at: usize| -> Option<usize> {
        let s = bytes.get(at..at.checked_add(4)?)?;
        usize::try_from(u32::from_be_bytes(<[u8; 4]>::try_from(s).ok()?)).ok()
    };
    let count = u32_at(HEADER_LEN)?;
    // A tag table entry is twelve bytes: bound the walk by the data.
    let count = count.min(bytes.len().saturating_sub(HEADER_LEN + 4) / 12);
    let mut found = None;
    for i in 0..count {
        let entry = HEADER_LEN + 4 + i * 12;
        if bytes.get(entry..entry + 4) == Some(b"desc") {
            found = Some((u32_at(entry + 4)?, u32_at(entry + 8)?));
            break;
        }
    }
    let (offset, size) = found?;
    let tag = bytes.get(offset..offset.checked_add(size)?)?;
    let text = match tag.get(..4)? {
        b"desc" => {
            // v2 textDescriptionType: sig, reserved, ASCII count, ASCII.
            let n = usize::try_from(u32::from_be_bytes(
                <[u8; 4]>::try_from(tag.get(8..12)?).ok()?,
            ))
            .ok()?;
            let ascii = tag.get(12..12usize.checked_add(n)?)?;
            ascii
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| if b.is_ascii() { char::from(b) } else { '?' })
                .collect::<String>()
        }
        b"mluc" => {
            // v4 multiLocalizedUnicodeType: take the first record.
            let record_count = usize::try_from(u32::from_be_bytes(
                <[u8; 4]>::try_from(tag.get(8..12)?).ok()?,
            ))
            .ok()?;
            if record_count == 0 {
                return None;
            }
            let len = usize::try_from(u32::from_be_bytes(
                <[u8; 4]>::try_from(tag.get(20..24)?).ok()?,
            ))
            .ok()?;
            let at = usize::try_from(u32::from_be_bytes(
                <[u8; 4]>::try_from(tag.get(24..28)?).ok()?,
            ))
            .ok()?;
            let units: Vec<u16> = tag
                .get(at..at.checked_add(len)?)?
                .chunks_exact(2)
                .map(|p| {
                    u16::from_be_bytes([
                        p.first().copied().unwrap_or(0),
                        p.get(1).copied().unwrap_or(0),
                    ])
                })
                .take(MAX_DESCRIPTION_CHARS * 2)
                .collect();
            char::decode_utf16(units)
                .map(|r| r.unwrap_or('\u{FFFD}'))
                .take_while(|&c| c != '\0')
                .collect::<String>()
        }
        _ => return None,
    };
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DESCRIPTION_CHARS)
        .collect();
    let clean = clean.trim().to_owned();
    (!clean.is_empty()).then_some(clean)
}

/// An embedded profile judged usable for RGB pixels.
#[derive(Debug)]
pub struct RgbSource {
    /// The parsed profile.
    pub profile: IccProfile,
    /// Its own name, or a generic one when it has none.
    pub description: String,
    /// Whether it is sRGB for all practical purposes
    /// ([`SRGB_EQUIVALENCE_TOLERANCE`]), so converting through it would
    /// change nothing worth keeping.
    pub equivalent_to_srgb: bool,
}

/// Judges `bytes` as the profile of RGB pixel data.
///
/// # Errors
///
/// A [`ProfileProblem`] naming why the profile cannot be used: too large,
/// not ICC, the wrong colour space or class, or refused by `lcms2`.
pub fn inspect_rgb_source(bytes: &[u8]) -> Result<RgbSource, ProfileProblem> {
    let header = parse_header(bytes)?;
    if header.space != ProfileSpace::Rgb {
        return Err(ProfileProblem::WrongSpace(header.space));
    }
    // Display, input, colour-space conversion and output (printer RGB)
    // profiles can describe RGB pixels; abstract, link and named-colour
    // profiles cannot.
    if !matches!(&header.class, b"mntr" | b"scnr" | b"spac" | b"prtr") {
        return Err(ProfileProblem::WrongClass(header.class));
    }
    let profile = IccProfile::from_bytes(bytes)
        .map_err(|_| ProfileProblem::Rejected("it could not be read as an ICC profile"))?;
    let to_working = Transform::to_working(&profile)
        .map_err(|_| ProfileProblem::Rejected("it cannot be used to describe image pixels"))?;
    let equivalent_to_srgb = within_srgb_tolerance(&to_working);
    let description =
        profile_description(bytes).unwrap_or_else(|| "an unnamed RGB profile".to_owned());
    Ok(RgbSource {
        profile,
        description,
        equivalent_to_srgb,
    })
}

/// Lattice side for the sRGB equivalence test (9 x 9 x 9 plus a ramp).
const LATTICE: u8 = 9;

fn within_srgb_tolerance(to_working: &Transform) -> bool {
    let mut src: Vec<f32> = Vec::new();
    let step = 1.0 / f32::from(LATTICE - 1);
    for r in 0..LATTICE {
        for g in 0..LATTICE {
            for b in 0..LATTICE {
                src.extend([
                    f32::from(r) * step,
                    f32::from(g) * step,
                    f32::from(b) * step,
                ]);
            }
        }
    }
    // A dense neutral ramp, where a wrong tone curve shows first.
    for i in 0..=64u8 {
        let v = f32::from(i) / 64.0;
        src.extend([v, v, v]);
    }
    let mut dst = vec![0.0_f32; src.len()];
    if to_working.transform(&src, &mut dst).is_err() {
        return false;
    }
    let worst = src
        .iter()
        .zip(&dst)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    worst <= SRGB_EQUIVALENCE_TOLERANCE
}

impl Transform {
    /// The transform from an RGB source profile into Aurora's working
    /// space, encoded sRGB: relative colorimetric with black point
    /// compensation ([`Transform::new_with_options`]).
    ///
    /// # Errors
    ///
    /// As [`Transform::new_with_options`].
    pub fn to_working(source: &IccProfile) -> Result<Self, ColorError> {
        Self::new_with_options(
            source,
            &IccProfile::srgb(),
            Channels::Rgb,
            RenderingIntent::RelativeColorimetric,
            true,
        )
    }

    /// The transform from Aurora's working space (encoded sRGB) into an
    /// RGB output profile: the same intent and black point compensation as
    /// [`Self::to_working`].
    ///
    /// # Errors
    ///
    /// As [`Transform::new_with_options`].
    pub fn from_working(output: &IccProfile) -> Result<Self, ColorError> {
        Self::new_with_options(
            &IccProfile::srgb(),
            output,
            Channels::Rgb,
            RenderingIntent::RelativeColorimetric,
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRGB_ICC: &[u8] = include_bytes!("../../../corpora/icc/sRGB.icc");
    const ECI_RGBV2_ICC: &[u8] = include_bytes!("../../../corpora/icc/ECI-RGBv2.icc");

    const D65: [f64; 2] = [0.3127, 0.3290];
    pub(crate) const ADOBE_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];
    pub(crate) const ADOBE_GAMMA: f64 = 2.199_218_75;

    pub(crate) fn adobe_like_bytes() -> Vec<u8> {
        let Ok(p) = IccProfile::synthetic_rgb(D65, ADOBE_PRIMARIES, ADOBE_GAMMA) else {
            unreachable!("synthetic profile")
        };
        let Ok(b) = p.to_bytes() else {
            unreachable!("serialises")
        };
        b
    }

    #[test]
    fn the_corpus_srgb_profile_counts_as_srgb_and_a_wide_gamut_one_does_not() {
        let Ok(srgb) = inspect_rgb_source(SRGB_ICC) else {
            unreachable!("sRGB.icc is a valid RGB profile")
        };
        assert!(srgb.equivalent_to_srgb, "the colord sRGB profile is sRGB");
        let Ok(eci) = inspect_rgb_source(ECI_RGBV2_ICC) else {
            unreachable!("ECI-RGBv2 is a valid RGB profile")
        };
        assert!(!eci.equivalent_to_srgb);
        assert!(
            eci.description.to_lowercase().contains("eci"),
            "{}",
            eci.description
        );
        let Ok(adobe) = inspect_rgb_source(&adobe_like_bytes()) else {
            unreachable!("synthetic profile is valid")
        };
        assert!(!adobe.equivalent_to_srgb);
        // A gamma-2.2 sRGB-primaries profile is close to sRGB but over the
        // half-step tolerance in the darks.
        let Ok(g22) =
            IccProfile::synthetic_rgb(D65, [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], 2.2)
                .and_then(|p| p.to_bytes())
        else {
            unreachable!()
        };
        let Ok(g22) = inspect_rgb_source(&g22) else {
            unreachable!()
        };
        assert!(!g22.equivalent_to_srgb);
    }

    #[test]
    fn the_built_in_srgb_is_srgb() {
        let Ok(bytes) = IccProfile::srgb().to_bytes() else {
            unreachable!()
        };
        let Ok(s) = inspect_rgb_source(&bytes) else {
            unreachable!()
        };
        assert!(s.equivalent_to_srgb);
    }

    #[test]
    fn non_rgb_wrong_class_garbage_and_oversized_profiles_are_refused_with_a_reason() {
        // A gray profile.
        let Ok(gray) = IccProfile::synthetic_gray(2.2).and_then(|p| p.to_bytes()) else {
            unreachable!()
        };
        assert_eq!(
            inspect_rgb_source(&gray).err(),
            Some(ProfileProblem::WrongSpace(ProfileSpace::Gray))
        );
        // Garbage, a truncated header, a size past the data.
        assert!(matches!(
            inspect_rgb_source(b"not an icc profile"),
            Err(ProfileProblem::NotIcc(_))
        ));
        let good = adobe_like_bytes();
        assert!(matches!(
            inspect_rgb_source(good.get(..200).unwrap_or_default()),
            Err(ProfileProblem::NotIcc(_))
        ));
        // Wrong class: an abstract profile ('abst') with the RGB space.
        let mut abst = good.clone();
        if let Some(c) = abst.get_mut(12..16) {
            c.copy_from_slice(b"abst");
        }
        assert_eq!(
            inspect_rgb_source(&abst).err(),
            Some(ProfileProblem::WrongClass(*b"abst"))
        );
        // Too large is refused before anything parses it.
        let big = vec![0_u8; MAX_PROFILE_BYTES + 1];
        assert_eq!(
            inspect_rgb_source(&big).err(),
            Some(ProfileProblem::TooLarge(MAX_PROFILE_BYTES + 1))
        );
        // Every reason reads as a sentence fragment.
        assert!(ProfileProblem::TooLarge(9).describe().contains("larger"));
        assert!(
            ProfileProblem::WrongSpace(ProfileSpace::Cmyk)
                .describe()
                .contains("CMYK")
        );
    }

    #[test]
    fn a_hostile_description_tag_never_panics() {
        // Flip every byte of the tag table region of a real profile.
        let good = adobe_like_bytes();
        for at in 128..good.len().min(400) {
            let mut bad = good.clone();
            if let Some(b) = bad.get_mut(at) {
                *b = 0xFF;
            }
            let _ = profile_description(&bad);
            let _ = inspect_rgb_source(&bad);
        }
        assert_eq!(profile_description(&[0; 10]), None);
    }

    // Fixed 3 x 3 matrix arithmetic in a test: indices are constants or
    // 0..3 loops, and the names are the standard ones (r, g, b, m).
    #[allow(clippy::indexing_slicing, clippy::many_single_char_names)]
    #[test]
    fn adobe_like_to_srgb_matches_an_independent_matrix_computation() {
        // Both spaces are D65 matrix/TRC spaces, so relative colorimetric
        // is exactly: decode the gamma, RGB -> XYZ with the primaries'
        // matrix, XYZ -> sRGB linear, sRGB encode. Computed here in f64,
        // not through lcms2.
        fn matrix(p: [[f64; 2]; 3], w: [f64; 2]) -> [[f64; 3]; 3] {
            let xyz = |c: [f64; 2]| [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]];
            let (r, g, b, wp) = (xyz(p[0]), xyz(p[1]), xyz(p[2]), xyz(w));
            let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
            let inv = invert(m);
            let s: Vec<f64> = (0..3)
                .map(|i| inv[i][0] * wp[0] + inv[i][1] * wp[1] + inv[i][2] * wp[2])
                .collect();
            [
                [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
                [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
                [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
            ]
        }
        fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
            let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
            let c = |a: usize, b: usize, c: usize, d: usize| m[a][b] * m[c][d];
            [
                [
                    (c(1, 1, 2, 2) - c(1, 2, 2, 1)) / det,
                    (c(0, 2, 2, 1) - c(0, 1, 2, 2)) / det,
                    (c(0, 1, 1, 2) - c(0, 2, 1, 1)) / det,
                ],
                [
                    (c(1, 2, 2, 0) - c(1, 0, 2, 2)) / det,
                    (c(0, 0, 2, 2) - c(0, 2, 2, 0)) / det,
                    (c(0, 2, 1, 0) - c(0, 0, 1, 2)) / det,
                ],
                [
                    (c(1, 0, 2, 1) - c(1, 1, 2, 0)) / det,
                    (c(0, 1, 2, 0) - c(0, 0, 2, 1)) / det,
                    (c(0, 0, 1, 1) - c(0, 1, 1, 0)) / det,
                ],
            ]
        }
        let adobe = matrix(ADOBE_PRIMARIES, D65);
        let srgb_inv = invert(matrix([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65));
        let encode = |v: f64| {
            if v <= 0.003_130_8 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        let Ok(src) = IccProfile::synthetic_rgb(D65, ADOBE_PRIMARIES, ADOBE_GAMMA) else {
            unreachable!()
        };
        let Ok(t) = Transform::to_working(&src) else {
            unreachable!()
        };
        for input in [
            [1.0_f32, 0.0, 0.0],
            [0.2, 0.6, 0.4],
            [0.9, 0.9, 0.1],
            [0.5, 0.5, 0.5],
            [0.0, 1.0, 0.0],
        ] {
            let lin: Vec<f64> = input
                .iter()
                .map(|&v| f64::from(v).powf(ADOBE_GAMMA))
                .collect();
            let xyz: Vec<f64> = (0..3)
                .map(|i| adobe[i][0] * lin[0] + adobe[i][1] * lin[1] + adobe[i][2] * lin[2])
                .collect();
            let want: Vec<f64> = (0..3)
                .map(|i| {
                    let v =
                        srgb_inv[i][0] * xyz[0] + srgb_inv[i][1] * xyz[1] + srgb_inv[i][2] * xyz[2];
                    // Extended range: odd extension of the encode.
                    // lcms2 extends the sRGB curve below zero linearly.
                    if v < 0.0 { 12.92 * v } else { encode(v) }
                })
                .collect();
            let mut got = [0.0_f32; 3];
            assert!(t.transform(&input, &mut got).is_ok());
            for (g, w) in got.iter().zip(&want) {
                assert!(
                    (f64::from(*g) - w).abs() < 2e-3,
                    "{input:?}: got {got:?}, want {want:?}"
                );
            }
        }
        // Adobe green is outside sRGB: red goes negative, kept.
        let mut got = [0.0_f32; 3];
        assert!(t.transform(&[0.0, 1.0, 0.0], &mut got).is_ok());
        assert!(got[0] < -0.05, "extended range must survive: {got:?}");
    }

    /// A hand-assembled ICC v2 RGB input profile with a `lut16` `A2B0` into
    /// Lab, linear in `L*` along the grey axis with a **lifted black**
    /// (`L* = 5` at device black, 100 at white) and the given media white
    /// point `wtpt` (XYZ). No matrices or curves: the first profile in the
    /// tests that is not matrix/TRC, so intent and black point compensation
    /// can matter.
    #[allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub(crate) fn lut_profile(white: [f64; 3]) -> Vec<u8> {
        fn s15(v: f64) -> [u8; 4] {
            let n = (v * 65536.0).round() as i32;
            n.to_be_bytes()
        }
        let mut desc = Vec::new();
        desc.extend_from_slice(b"desc\0\0\0\0");
        let text = b"LUT test profile\0";
        desc.extend_from_slice(&(text.len() as u32).to_be_bytes());
        desc.extend_from_slice(text);
        desc.extend_from_slice(&[0; 8]); // unicode language and count
        desc.extend_from_slice(&[0; 2]); // script code
        desc.push(0);
        desc.extend_from_slice(&[0; 67]);
        while desc.len() % 4 != 0 {
            desc.push(0);
        }
        let mut wtpt = Vec::new();
        wtpt.extend_from_slice(b"XYZ \0\0\0\0");
        for v in white {
            wtpt.extend_from_slice(&s15(v));
        }
        let mut a2b = Vec::new();
        a2b.extend_from_slice(b"mft2\0\0\0\0");
        a2b.extend_from_slice(&[3, 3, 2, 0]);
        for (i, v) in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
            .iter()
            .enumerate()
        {
            let _ = i;
            a2b.extend_from_slice(&s15(*v));
        }
        a2b.extend_from_slice(&2_u16.to_be_bytes()); // input table entries
        a2b.extend_from_slice(&2_u16.to_be_bytes()); // output table entries
        for _ in 0..3 {
            a2b.extend_from_slice(&0_u16.to_be_bytes());
            a2b.extend_from_slice(&65535_u16.to_be_bytes());
        }
        for r in 0..2_u16 {
            for g in 0..2_u16 {
                for b in 0..2_u16 {
                    // v2 Lab16: L 0..=0xFF00 for 0..100, a and b 0x8000 = 0.
                    let mean = f64::from(r + g + b) / 3.0;
                    let l = (5.0 + 95.0 * mean) / 100.0 * 65280.0;
                    a2b.extend_from_slice(&(l.round() as u16).to_be_bytes());
                    a2b.extend_from_slice(&0x8000_u16.to_be_bytes());
                    a2b.extend_from_slice(&0x8000_u16.to_be_bytes());
                }
            }
        }
        for _ in 0..3 {
            a2b.extend_from_slice(&0_u16.to_be_bytes());
            a2b.extend_from_slice(&65535_u16.to_be_bytes());
        }
        while a2b.len() % 4 != 0 {
            a2b.push(0);
        }
        // `A2B1` (relative colorimetric) shares `A2B0`'s data: lcms2 only
        // honours black point compensation for an intent whose table the
        // profile lists.
        let tags: [(&[u8; 4], &Vec<u8>); 4] = [
            (b"desc", &desc),
            (b"wtpt", &wtpt),
            (b"A2B0", &a2b),
            (b"A2B1", &a2b),
        ];
        let table_len = 4 + 12 * tags.len();
        let mut offset = 128 + table_len;
        let mut table = (tags.len() as u32).to_be_bytes().to_vec();
        let mut body = Vec::new();
        let mut a2b_offset = 0;
        for (sig, data) in tags {
            if sig == b"A2B1" {
                table.extend_from_slice(sig);
                table.extend_from_slice(&(a2b_offset as u32).to_be_bytes());
                table.extend_from_slice(&(data.len() as u32).to_be_bytes());
                continue;
            }
            if sig == b"A2B0" {
                a2b_offset = offset;
            }
            table.extend_from_slice(sig);
            table.extend_from_slice(&(offset as u32).to_be_bytes());
            table.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
            offset += data.len();
        }
        let mut out = vec![0_u8; 128];
        let total = (128 + table.len() + body.len()) as u32;
        out[0..4].copy_from_slice(&total.to_be_bytes());
        out[8..12].copy_from_slice(&0x0240_0000_u32.to_be_bytes());
        out[12..16].copy_from_slice(b"scnr");
        out[16..20].copy_from_slice(b"RGB ");
        out[20..24].copy_from_slice(b"Lab ");
        out[36..40].copy_from_slice(b"acsp");
        for (i, v) in [0.9642, 1.0, 0.8249].iter().enumerate() {
            out[68 + i * 4..72 + i * 4].copy_from_slice(&s15(*v));
        }
        out.extend_from_slice(&table);
        out.extend_from_slice(&body);
        out
    }

    fn convert(profile: &IccProfile, bpc: bool, intent: RenderingIntent, px: [f32; 3]) -> [f32; 3] {
        let Ok(t) =
            Transform::new_with_options(profile, &IccProfile::srgb(), Channels::Rgb, intent, bpc)
        else {
            unreachable!()
        };
        let mut out = [0.0_f32; 3];
        assert!(t.transform(&px, &mut out).is_ok());
        out
    }

    #[test]
    fn a_lut_profile_with_a_lifted_black_is_accepted_and_not_srgb() {
        let bytes = lut_profile([0.9642, 1.0, 0.8249]);
        let Ok(source) = inspect_rgb_source(&bytes) else {
            unreachable!("the hand-built LUT profile must be valid")
        };
        assert!(!source.equivalent_to_srgb);
        assert_eq!(source.description, "LUT test profile");
    }

    #[test]
    fn black_point_compensation_lowers_a_lifted_black_and_leaves_white() {
        let Ok(p) = IccProfile::from_bytes(&lut_profile([0.9642, 1.0, 0.8249])) else {
            unreachable!()
        };
        let rel = RenderingIntent::RelativeColorimetric;
        let off = convert(&p, false, rel, [0.0; 3]);
        let on = convert(&p, true, rel, [0.0; 3]);
        // Without BPC the lifted black (L* = 5) shows as a clearly non-black
        // sRGB value; with it the black lands on (about) zero.
        assert!(
            off.iter().all(|&c| c > 0.05),
            "BPC off keeps the lifted black: {off:?}"
        );
        assert!(
            on.iter().all(|&c| c < 0.02),
            "BPC on maps black to black: {on:?}"
        );
        // White is not moved by compensation.
        let w_on = convert(&p, true, rel, [1.0; 3]);
        let w_off = convert(&p, false, rel, [1.0; 3]);
        for (a, b) in w_on.iter().zip(w_off) {
            assert!((a - b).abs() < 0.02, "{w_on:?} vs {w_off:?}");
            assert!(*a > 0.97, "{w_on:?}");
        }
    }

    #[test]
    fn absolute_colorimetric_keeps_the_media_white_tint_that_relative_removes() {
        // A warm media white (D50 is the PCS white; this one is not).
        let Ok(p) = IccProfile::from_bytes(&lut_profile([0.90, 0.95, 0.60])) else {
            unreachable!()
        };
        let rel = convert(&p, false, RenderingIntent::RelativeColorimetric, [1.0; 3]);
        let abs = convert(&p, false, RenderingIntent::AbsoluteColorimetric, [1.0; 3]);
        assert!(
            rel.iter().all(|&c| c > 0.97),
            "relative maps white to white: {rel:?}"
        );
        let spread = abs
            .iter()
            .zip(rel)
            .map(|(a, r)| (a - r).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            spread > 0.02,
            "absolute differs from relative: {abs:?} vs {rel:?}"
        );
        // The tint is a reduction (white is dimmer in the channels whose
        // media-white component is below the PCS white's).
        assert!(abs.iter().any(|&c| c < rel[0] - 0.02), "{abs:?}");
    }

    #[test]
    fn the_working_space_transform_uses_relative_intent_with_black_point_compensation() {
        // The policy itself, through `Transform::to_working` (not a hand-built
        // transform): a LUT profile with a lifted black and a tinted media
        // white comes out with black at black and white at white.
        let Ok(p) = IccProfile::from_bytes(&lut_profile([0.90, 0.95, 0.60])) else {
            unreachable!()
        };
        let Ok(t) = Transform::to_working(&p) else {
            unreachable!()
        };
        let mut black = [1.0_f32; 3];
        let mut white = [0.0_f32; 3];
        assert!(t.transform(&[0.0; 3], &mut black).is_ok());
        assert!(t.transform(&[1.0; 3], &mut white).is_ok());
        assert!(
            black.iter().all(|&c| c < 0.02),
            "black stays black: {black:?}"
        );
        assert!(
            white.iter().all(|&c| c > 0.97),
            "white stays white: {white:?}"
        );
        // And the output direction is the same policy.
        let Ok(back) = Transform::from_working(&IccProfile::srgb()) else {
            unreachable!()
        };
        let mut v = [0.0_f32; 3];
        assert!(back.transform(&[0.5; 3], &mut v).is_ok());
    }
}
