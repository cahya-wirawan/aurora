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
/// sRGB over a lattice of colours and still count as sRGB: half an 8-bit
/// step. A profile inside it changes no 8-bit pixel by more than rounding,
/// so converting through it would only add noise (almost every JPEG carries
/// "sRGB IEC61966-2.1", whose tables differ from `lcms2`'s built-in by
/// roughly a thousandth). A choice, not a standard.
pub const SRGB_EQUIVALENCE_TOLERANCE: f32 = 0.5 / 255.0;

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
    src.iter()
        .zip(&dst)
        .all(|(a, b)| (a - b).abs() <= SRGB_EQUIVALENCE_TOLERANCE)
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
