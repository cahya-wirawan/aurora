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
}
