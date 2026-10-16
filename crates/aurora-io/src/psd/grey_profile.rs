//! The grey ICC profile of a Grayscale PSD (image resource 1039), read only
//! for its tone curve (task 112).
//!
//! `adjustments/exposure_grayscale.psd` (and every other Grayscale fixture
//! in the psd-tools corpus) embeds a 912-byte `GRAY` profile named "Dot
//! Gain 20%": the 128-byte ICC header, a tag table (`cprt`, `desc`, `wtpt`,
//! `bkpt`, `kTRC`) and a `kTRC` tag of type `curv` holding a `u32` count
//! (256) and 256 big-endian `u16` values, device value to linear Y. That
//! table is all Exposure needs (see `aurora_core::exposure_trc`).
//!
//! **Only the form that was measured is used**: a `curv` table of exactly
//! 256 entries that runs from `0` to `65535` without decreasing. A profile
//! of any other colour space, without a `kTRC`, with an identity or gamma
//! `curv` (count 0 or 1), a parametric `para` curve or a table of another
//! length is classified, not used, and an Exposure layer in such a file is
//! left out with its own report line.
//!
//! The parser is total and bounded: the tag count is checked against the
//! bytes present before any tag is read, a tag must lie inside the
//! profile, and the table length is compared with 256 before anything is
//! allocated.

use aurora_core::GreyTrc;

/// The only table length that was measured.
pub(super) const MEASURED_TABLE_ENTRIES: usize = 256;

/// The most tags read from the table.
const MAX_TAGS: usize = 1024;

/// Why an embedded grey profile is not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum GreyReason {
    /// The profile is not a `GRAY` profile.
    NotGray,
    /// Truncated, no `kTRC` tag, or a curve of an unknown type.
    Unreadable,
    /// A `curv` with count 0 (identity) or 1 (a gamma).
    GammaCurve,
    /// A parametric `para` curve.
    Parametric,
    /// A `curv` table that is not 256 entries long.
    TableLength,
    /// A table that decreases or does not run from 0 to 65535.
    TableShape,
}

/// What the file's resource 1039 gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum GreyProfile {
    /// No profile resource (or none read yet).
    #[default]
    Absent,
    /// A usable tone curve.
    Table(GreyTrc),
    /// A profile that is not used, and why.
    Unsupported(GreyReason),
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let b = data.get(at..end)?;
    Some(u32::from_be_bytes([
        *b.first()?,
        *b.get(1)?,
        *b.get(2)?,
        *b.get(3)?,
    ]))
}

fn usize32(data: &[u8], at: usize) -> Option<usize> {
    usize::try_from(be32(data, at)?).ok()
}

/// Classifies the bytes of resource 1039.
pub(super) fn classify(profile: &[u8]) -> GreyProfile {
    if profile.get(16..20) != Some(b"GRAY".as_slice()) {
        return GreyProfile::Unsupported(GreyReason::NotGray);
    }
    let unreadable = GreyProfile::Unsupported(GreyReason::Unreadable);
    let Some(count) = usize32(profile, 128) else {
        return unreadable;
    };
    let table_bytes = count.checked_mul(12).and_then(|n| n.checked_add(132));
    if count > MAX_TAGS || table_bytes.is_none_or(|end| end > profile.len()) {
        return unreadable;
    }
    for i in 0..count {
        let at = 132 + i * 12;
        if profile.get(at..at + 4) != Some(b"kTRC".as_slice()) {
            continue;
        }
        let (Some(offset), Some(size)) = (usize32(profile, at + 4), usize32(profile, at + 8))
        else {
            return unreadable;
        };
        let Some(end) = offset.checked_add(size).filter(|e| *e <= profile.len()) else {
            return unreadable;
        };
        let Some(tag) = profile.get(offset..end) else {
            return unreadable;
        };
        return match tag.get(..4) {
            Some(b"curv") => curv(tag),
            Some(b"para") => GreyProfile::Unsupported(GreyReason::Parametric),
            _ => unreadable,
        };
    }
    unreadable
}

/// A `curv` tag: type signature, 4 reserved bytes, `u32` count, `u16` data.
fn curv(tag: &[u8]) -> GreyProfile {
    let Some(n) = usize32(tag, 8) else {
        return GreyProfile::Unsupported(GreyReason::Unreadable);
    };
    if n <= 1 {
        return GreyProfile::Unsupported(GreyReason::GammaCurve);
    }
    if n != MEASURED_TABLE_ENTRIES {
        return GreyProfile::Unsupported(GreyReason::TableLength);
    }
    let Some(data) = tag.get(12..12 + 2 * n) else {
        return GreyProfile::Unsupported(GreyReason::Unreadable);
    };
    let entries: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| {
            u16::from_be_bytes([
                c.first().copied().unwrap_or(0),
                c.get(1).copied().unwrap_or(0),
            ])
        })
        .collect();
    match GreyTrc::new(entries) {
        Ok(trc) => GreyProfile::Table(trc),
        Err(_) => GreyProfile::Unsupported(GreyReason::TableShape),
    }
}

impl GreyReason {
    /// The clause the report line gives after "left out because".
    pub(super) const fn clause(self) -> &'static str {
        match self {
            Self::NotGray => "the file's embedded profile is not a grey profile",
            Self::Unreadable => "the file's grey profile could not be read",
            Self::GammaCurve => {
                "the file's grey profile is a plain gamma curve, which Aurora has not checked \
                 against Photoshop"
            }
            Self::Parametric => {
                "the file's grey profile uses a parametric curve, which Aurora has not checked \
                 against Photoshop"
            }
            Self::TableLength => {
                "the file's grey profile has a tone-curve table of a length Aurora has not \
                 checked (it has checked 256 entries)"
            }
            Self::TableShape => {
                "the file's grey profile has a tone-curve table that decreases or does not run \
                 from 0 to 1"
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::{GreyProfile, GreyReason, classify};

    /// A `GRAY` profile with one `kTRC` tag holding `tag` (the whole tag
    /// bytes, type signature first).
    pub(super) fn profile_with(tag: &[u8]) -> Vec<u8> {
        let mut p = vec![0_u8; 128];
        p[16..20].copy_from_slice(b"GRAY");
        p.extend_from_slice(&1_u32.to_be_bytes());
        p.extend_from_slice(b"kTRC");
        p.extend_from_slice(&144_u32.to_be_bytes());
        p.extend_from_slice(&(tag.len() as u32).to_be_bytes());
        p.extend_from_slice(tag);
        p
    }

    pub(super) fn curv_tag(entries: &[u16]) -> Vec<u8> {
        let mut t = b"curv".to_vec();
        t.extend_from_slice(&[0; 4]);
        t.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for e in entries {
            t.extend_from_slice(&e.to_be_bytes());
        }
        t
    }

    fn ramp(n: usize) -> Vec<u16> {
        (0..n).map(|i| (i * 65535 / (n - 1)) as u16).collect()
    }

    #[test]
    fn a_256_entry_table_is_used() {
        match classify(&profile_with(&curv_tag(&ramp(256)))) {
            GreyProfile::Table(t) => assert_eq!(t.entries(), ramp(256).as_slice()),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn every_other_form_is_classified_not_used() {
        let reason = |p: Vec<u8>| match classify(&p) {
            GreyProfile::Unsupported(r) => r,
            other => unreachable!("{other:?}"),
        };
        assert_eq!(reason(profile_with(&curv_tag(&[]))), GreyReason::GammaCurve);
        assert_eq!(
            reason(profile_with(&curv_tag(&[0x0233]))),
            GreyReason::GammaCurve
        );
        assert_eq!(
            reason(profile_with(&curv_tag(&ramp(1024)))),
            GreyReason::TableLength
        );
        assert_eq!(
            reason(profile_with(&curv_tag(&ramp(255)))),
            GreyReason::TableLength
        );
        let mut down = ramp(256);
        down.swap(10, 11);
        assert_eq!(
            reason(profile_with(&curv_tag(&down))),
            GreyReason::TableShape
        );
        let mut short_end = ramp(256);
        short_end[255] = 65000;
        assert_eq!(
            reason(profile_with(&curv_tag(&short_end))),
            GreyReason::TableShape
        );
        assert_eq!(
            reason(profile_with(b"para\0\0\0\0\0\0\0\0")),
            GreyReason::Parametric
        );
        assert_eq!(
            reason(profile_with(b"mft2\0\0\0\0\0\0\0\0")),
            GreyReason::Unreadable
        );
        let mut rgb = profile_with(&curv_tag(&ramp(256)));
        rgb[16..20].copy_from_slice(b"RGB ");
        assert_eq!(reason(rgb), GreyReason::NotGray);
        // A GRAY profile with no kTRC tag.
        let mut none = profile_with(&curv_tag(&ramp(256)));
        none[132..136].copy_from_slice(b"wtpt");
        assert_eq!(reason(none), GreyReason::Unreadable);
    }

    #[test]
    fn hostile_profiles_are_unreadable_without_panicking_or_allocating() {
        let good = profile_with(&curv_tag(&ramp(256)));
        // Every truncation.
        for n in 0..good.len() {
            let _ = classify(&good[..n]);
            if n < good.len() {
                assert!(
                    !matches!(classify(&good[..n]), GreyProfile::Table(_)),
                    "{n}"
                );
            }
        }
        // A tag count far past the bytes present.
        let mut many = good.clone();
        many[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            classify(&many),
            GreyProfile::Unsupported(GreyReason::Unreadable)
        );
        // A tag offset/size that overflows or runs past the end.
        for (off, size) in [
            (u32::MAX, 16_u32),
            (144, u32::MAX),
            (u32::MAX, u32::MAX),
            (10_000, 4),
        ] {
            let mut p = good.clone();
            p[136..140].copy_from_slice(&off.to_be_bytes());
            p[140..144].copy_from_slice(&size.to_be_bytes());
            assert_eq!(
                classify(&p),
                GreyProfile::Unsupported(GreyReason::Unreadable),
                "{off} {size}"
            );
        }
        // A curv count of 0xFFFFFFFF with 12 bytes of tag: refused as a
        // length, never allocated.
        let mut huge = b"curv".to_vec();
        huge.extend_from_slice(&[0; 4]);
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            classify(&profile_with(&huge)),
            GreyProfile::Unsupported(GreyReason::TableLength)
        );
        // A 256-entry count with the data cut short.
        let mut cut = curv_tag(&ramp(256));
        cut.truncate(100);
        assert_eq!(
            classify(&profile_with(&cut)),
            GreyProfile::Unsupported(GreyReason::Unreadable)
        );
    }
}
