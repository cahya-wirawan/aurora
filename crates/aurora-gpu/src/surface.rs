//! Windowed presentation: a configured `wgpu::Surface` plus resize
//! handling.
//!
//! **Unverified against a real window in this environment** — this
//! machine has no usable display (the only X server present is owned by
//! `gdm` with an `Xauthority` this user can't read, the same "GDM
//! greeter only, no live session" gap `spike/a11y-ime/FINDINGS.md`
//! already documented blocking the Linux Orca leg). The code follows
//! `spike/vertical-slice`'s own proven windowed setup and wgpu's
//! documented API; real end-to-end verification needs a live desktop
//! session, tracked in `PLAN.md` alongside that same Orca-leg gap.

use crate::GpuError;
use crate::context::GpuContext;

/// A configured presentation surface for one window.
pub struct GpuSurface<'window> {
    surface: wgpu::Surface<'window>,
    config: wgpu::SurfaceConfiguration,
}

impl GpuContext {
    /// Creates and configures a `wgpu::Surface` for `target` at `size`,
    /// matching `spike/vertical-slice`'s own windowed setup
    /// (`get_default_config`, forced `AutoVsync`).
    ///
    /// `target` accepts anything convertible into `wgpu::SurfaceTarget`
    /// — wgpu's own flexible target type already covers anything
    /// implementing the `raw-window-handle` traits (e.g.
    /// `&winit::window::Window`), so callers pass whatever their
    /// windowing crate gives them without `aurora-gpu` needing a
    /// windowing dependency of its own.
    ///
    /// # Errors
    ///
    /// Returns [`GpuError::SurfaceCreation`] if `target` can't be turned
    /// into a surface at all, or [`GpuError::SurfaceUnsupported`] if the
    /// adapter has no usable configuration for it.
    pub fn create_surface<'window>(
        &self,
        target: impl Into<wgpu::SurfaceTarget<'window>>,
        size: (u32, u32),
    ) -> Result<GpuSurface<'window>, GpuError> {
        let surface = self
            .instance()
            .create_surface(target)
            .map_err(GpuError::SurfaceCreation)?;
        let Some(mut config) = surface.get_default_config(self.adapter(), size.0, size.1) else {
            return Err(GpuError::SurfaceUnsupported);
        };
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(self.device(), &config);
        Ok(GpuSurface { surface, config })
    }
}

impl GpuSurface<'_> {
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Reconfigures the surface for a new size.
    ///
    /// No-ops on a zero-sized request (a minimized window can report
    /// `0x0`) rather than calling into wgpu with an invalid size, which
    /// panics — a real, documented gotcha, not a hypothetical one.
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(device, &self.config);
    }

    /// Acquires the next frame to render into.
    ///
    /// Returns wgpu's own `CurrentSurfaceTexture` directly rather than
    /// wrapping it — wgpu 30 changed this from the `Result<SurfaceTexture,
    /// SurfaceError>` shape older wgpu tutorials assume to a 7-variant
    /// enum (`Success`/`Suboptimal`/`Timeout`/`Occluded`/`Outdated`/
    /// `Lost`/`Validation`); callers match on it directly rather than
    /// this crate re-inventing a narrower shape for wgpu's own type.
    #[must_use]
    pub fn acquire(&self) -> wgpu::CurrentSurfaceTexture {
        self.surface.get_current_texture()
    }
}

impl std::fmt::Debug for GpuSurface<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuSurface")
            .field("format", &self.config.format)
            .field("size", &(self.config.width, self.config.height))
            .finish_non_exhaustive()
    }
}
