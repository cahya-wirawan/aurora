//! The shared GPU device/queue handle.

use crate::error::GpuError;

/// Owns the one shared `wgpu::Device`/`Queue` invariant §7.3.8 requires —
/// UI and canvas draw through the same device, into the same frame, not
/// separate surfaces composited together.
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuContext {
    /// Creates a headless context: requests an adapter with no
    /// compatible-surface constraint, exactly like
    /// `spike/vertical-slice`'s `--headless` path — the code
    /// `spike/FINDINGS.md`'s Linux/Vulkan numbers were measured from.
    ///
    /// Surface-compatible construction (so the chosen adapter can
    /// actually present to a window) is deliberately not this function's
    /// job yet — PLAN.md M1.2 tracks that as separate, follow-on work.
    ///
    /// # Errors
    ///
    /// Returns [`GpuError`] if no adapter is available, or if a device
    /// couldn't be requested from one that was found.
    pub fn new() -> Result<Self, GpuError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|_| GpuError::NoSuitableAdapter)?;

        let info = adapter.get_info();
        tracing::info!(
            backend = ?info.backend,
            adapter = %info.name,
            device_type = ?info.device_type,
            "selected GPU adapter"
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("aurora-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .map_err(GpuError::DeviceRequestFailed)?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Diagnostic info about the selected adapter (backend, name, device
    /// type) — useful for logging and, eventually, FR-026's GPU
    /// preferences surface.
    #[must_use]
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// The instance this context was created from — used by
    /// [`crate::GpuContext::create_surface`] to create a surface
    /// compatible with this same adapter/device.
    #[must_use]
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }

    /// Not part of the public API — [`crate::surface`] needs the raw
    /// adapter to query surface capabilities; nothing outside the crate
    /// has needed it so far.
    pub(crate) const fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter_info", &self.adapter_info())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::real_context;

    #[test]
    fn creates_a_real_headless_device() {
        // Real verification, not a mock -- matching this project's
        // general practice (spike/FINDINGS.md's whole existence is built
        // on measuring real behavior). `real_context()` treats a missing
        // adapter as an inconclusive skip and a real device-request
        // failure as a hard test failure -- see its own doc comment.
        let Some(context) = real_context() else {
            return;
        };
        let info = context.adapter_info();
        eprintln!(
            "adapter: {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        assert!(!info.name.is_empty(), "adapter must report a name");
    }
}
