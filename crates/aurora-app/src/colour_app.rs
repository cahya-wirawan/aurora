//! Document colour management, the app half (colour management, round 1).
//!
//! **Model.** Pixels are always stored in the sRGB working space. A document
//! keeps two pieces of metadata ([`aurora_io::DocumentColour`]): the
//! **source profile** its file came in (informational; the pixels were
//! converted on open) and an **output profile** the user chose with "Set
//! Output Profile\u{2026}". The output profile is what export converts to
//! ([`export_image`]: relative colorimetric with black point compensation,
//! then the exported file embeds exactly that profile) and what soft
//! proofing will use later. With no output profile, export writes sRGB, as
//! before.
//!
//! Setting or clearing the output profile is a metadata change: no undo step
//! and it does not mark the document unsaved (it is written by the next
//! `.aur` save or autosave). A profile that is not a valid RGB profile is
//! refused with a calm notice and nothing changes. A choice for the design
//! owner: whether setting it should count as an edit.

use std::io::Read as _;
use std::path::Path;

use aurora_color::{IccProfile, MAX_PROFILE_BYTES, Transform, inspect_rgb_source};
use aurora_io::{Image, SourceProfile};

use crate::LockRefusal;

pub(crate) const COMMAND_SET_OUTPUT: &str = "colour.output_profile.set";
pub(crate) const COMMAND_CLEAR_OUTPUT: &str = "colour.output_profile.clear";
pub(crate) const TITLE_SET_OUTPUT: &str = "Set Output Profile\u{2026}";
pub(crate) const TITLE_CLEAR_OUTPUT: &str = "Clear Output Profile (sRGB)";

pub(crate) const NOTICE_SET: &str =
    "Output profile set. Export converts to it; the pixels stay in sRGB.";
pub(crate) const NOTICE_CLEARED: &str = "Output profile cleared: export writes sRGB.";
pub(crate) const NOTICE_NOT_FOUND: &str = "That profile couldn't be read.";
pub(crate) const NOTICE_NOT_RGB: &str =
    "That isn't a usable RGB profile, so the output profile was not changed.";
pub(crate) const NOTICE_EXPORT_FALLBACK: &str =
    "The output profile couldn't be applied, so the export was written as sRGB.";

/// Reads at most [`MAX_PROFILE_BYTES`] + 1 bytes of `path`.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_PROFILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Judges the profile file `path` as an output profile.
///
/// # Errors
///
/// The calm notice to show: unreadable, or not a usable RGB profile.
pub(crate) fn load_output_profile(path: &Path) -> Result<SourceProfile, &'static str> {
    let Some(bytes) = read_bounded(path) else {
        return Err(NOTICE_NOT_FOUND);
    };
    judge_output_profile(bytes)
}

/// [`load_output_profile`] on bytes already read.
///
/// # Errors
///
/// [`NOTICE_NOT_RGB`] when `bytes` is not a valid RGB profile (wrong
/// colour space or class, too large, damaged).
pub(crate) fn judge_output_profile(bytes: Vec<u8>) -> Result<SourceProfile, &'static str> {
    match inspect_rgb_source(&bytes) {
        Ok(source) => Ok(SourceProfile {
            description: source.description,
            bytes,
        }),
        Err(_) => Err(NOTICE_NOT_RGB),
    }
}

/// `image` (working-space sRGB) as it should be exported: with no output
/// profile, unchanged; with one, converted to it and **tagged with it**, so
/// the exporter embeds exactly the profile the pixels are in. Alpha is
/// untouched and extended range is kept until the exporter quantises.
///
/// # Errors
///
/// `Err(image)` (the unchanged sRGB image) when the profile cannot be used
/// for a conversion, so the caller exports sRGB (never a mismatched tag).
pub(crate) fn export_image(image: Image, output: Option<&SourceProfile>) -> Result<Image, Image> {
    let Some(output) = output else {
        return Ok(image);
    };
    let Ok(profile) = IccProfile::from_bytes(&output.bytes) else {
        return Err(image);
    };
    let Ok(transform) = Transform::from_working(&profile) else {
        return Err(image);
    };
    let mut samples = image.samples().to_vec();
    if aurora_io::convert_rgba(&mut samples, &transform).is_err() {
        return Err(image);
    }
    match Image::new(image.width(), image.height(), profile, samples) {
        Ok(converted) => Ok(converted),
        Err(_) => Err(image),
    }
}

impl crate::App {
    /// Set Output Profile\u{2026}: the chosen file becomes the document's
    /// output profile, or a calm notice says why not.
    pub(crate) fn set_output_profile(&mut self, path: &Path) {
        match load_output_profile(path) {
            Ok(profile) => {
                self.doc.colour.output = Some(profile);
                self.colour_notice(NOTICE_SET);
            }
            Err(why) => self.colour_notice(why),
        }
    }

    /// Clear Output Profile: export goes back to sRGB.
    pub(crate) fn clear_output_profile(&mut self) {
        self.doc.colour.output = None;
        self.colour_notice(NOTICE_CLEARED);
    }

    pub(crate) fn colour_notice(&mut self, text: &'static str) {
        self.lock_notice = Some((LockRefusal::Colour(text), self.doc.active_layer));
        self.sync_status_bar_now();
    }
}

#[cfg(test)]
#[path = "colour_app_tests.rs"]
mod tests;
