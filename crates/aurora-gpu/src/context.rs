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

    /// The instance this context was created from — needed later to
    /// create a surface compatible with the same adapter/device
    /// (surface configuration is separate, follow-on M1.2 work).
    #[must_use]
    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
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
    use super::GpuContext;

    #[test]
    fn creates_a_real_headless_device() {
        // Real verification, not a mock -- matching this project's
        // general practice (spike/FINDINGS.md's whole existence is built
        // on measuring real behavior). `NoSuitableAdapter` is treated as
        // an inconclusive skip rather than a failure: this pass can only
        // confirm real device creation on this dev machine's actual GPU
        // (see spike/FINDINGS.md's Linux/Vulkan/RTX 3090 run) -- whether
        // every CI runner image has a usable GPU or software adapter is
        // genuinely unverified, not assumed. `DeviceRequestFailed` (an
        // adapter was found but device creation itself failed) is a real
        // bug and does fail the test.
        match GpuContext::new() {
            Ok(context) => {
                let info = context.adapter_info();
                eprintln!(
                    "adapter: {} ({:?}, {:?})",
                    info.name, info.backend, info.device_type
                );
                assert!(!info.name.is_empty(), "adapter must report a name");
            }
            Err(super::GpuError::NoSuitableAdapter) => {
                eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
            }
            Err(err) => {
                // A real, reachable failure -- not the "this can't
                // happen" case `unreachable!()` is for elsewhere in this
                // codebase, so a plain `assert!(false, ..)` (which
                // clippy flags as always-false and suggests `panic!()`
                // for) would be dishonest either way. This is the one
                // deliberate, visible `panic!` in the crate: an adapter
                // was found but device creation itself failed, which is
                // a real bug this test exists to catch.
                #[allow(clippy::panic)]
                {
                    panic!("device request failed with a real adapter present: {err}");
                }
            }
        }
    }
}
