//! Error types for `aurora-color`.

use aurora_core::Channels;

/// Errors from loading an ICC profile or building/using a colour
/// transform.
///
/// `#[non_exhaustive]`: more variants will be added as this crate grows
/// (working-space conversion, promote/dither); downstream `match`es must
/// already handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ColorError {
    /// [`crate::IccProfile::from_bytes`] couldn't parse the given data as
    /// an ICC profile.
    #[error("failed to parse ICC profile data")]
    InvalidProfile(#[source] lcms2::Error),
    /// [`crate::Transform::new`] failed to build a transform between two
    /// otherwise-valid profiles.
    #[error("failed to build a colour transform")]
    TransformCreationFailed(#[source] lcms2::Error),
    /// [`crate::IccProfile::to_bytes`] failed to re-serialize a loaded
    /// profile back to raw ICC bytes — not expected in practice (`lcms2`
    /// can always re-encode a profile it successfully parsed or built),
    /// but a real, checked possibility rather than an assumption, the
    /// same discipline every other `lcms2` call in this crate already
    /// applies.
    #[error("failed to serialize ICC profile data")]
    SerializeFailed(#[source] lcms2::Error),
    /// [`crate::Transform::new`] was asked for a channel layout this
    /// crate doesn't wire up to `lcms2` yet — see [`Channels`]'s own
    /// variants against [`crate::Transform`]'s doc comment for exactly
    /// which four of the six it currently supports.
    #[error("{0:?} is not a supported transform channel layout yet")]
    UnsupportedChannels(Channels),
    /// [`crate::Transform::transform`]'s `src`/`dst` slices didn't both
    /// have a length that's an exact multiple of the transform's channel
    /// count, or the two lengths didn't match each other.
    #[error(
        "pixel buffers must both have a length that's an exact multiple of \
         {channels:?}'s {channel_count} channels, and match each other: got \
         src.len() = {src_len}, dst.len() = {dst_len}"
    )]
    BufferLengthMismatch {
        channels: Channels,
        channel_count: u8,
        src_len: usize,
        dst_len: usize,
    },
}
