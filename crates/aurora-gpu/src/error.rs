//! Error types for `aurora-gpu`.

/// Errors from GPU device/queue setup.
///
/// `#[non_exhaustive]`: more variants will be added as surface
/// configuration, pipeline creation, etc. land; downstream `match`es must
/// already handle "something else" today.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GpuError {
    /// No adapter satisfied the requested power preference/compatibility
    /// constraints — e.g. no GPU (real or software) is available at all.
    #[error("no suitable GPU adapter found")]
    NoSuitableAdapter,
    /// An adapter was found, but requesting a device/queue from it failed.
    #[error("GPU device request failed: {0}")]
    DeviceRequestFailed(#[source] wgpu::RequestDeviceError),
    /// The window target couldn't be turned into a `wgpu::Surface` at all.
    #[error("failed to create a surface: {0}")]
    SurfaceCreation(#[source] wgpu::CreateSurfaceError),
    /// A surface was created, but the adapter has no usable configuration
    /// for it (`Surface::get_default_config` returned `None`).
    #[error("adapter does not support presenting to this surface")]
    SurfaceUnsupported,
    /// `mip_level` passed to [`crate::TileResidency::upload_mip`] is
    /// outside the atlas's mip chain.
    #[error("mip level {0} is out of range for this atlas")]
    InvalidMipLevel(u32),
    /// The texel buffer passed to [`crate::TileResidency::upload_mip`]
    /// doesn't match the size `mip_level` expects.
    #[error(
        "invalid tile upload at mip level {mip_level}: expected {expected} f16 samples, got {actual}"
    )]
    InvalidTileUpload {
        mip_level: u32,
        expected: usize,
        actual: usize,
    },
}
