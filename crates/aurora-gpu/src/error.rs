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
}
