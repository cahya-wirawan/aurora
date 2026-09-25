//! Windowed presentation: a configured `wgpu::Surface` plus resize
//! handling.
//!
//! **Verified against a real window, 2026-07-29** — `examples/surface_smoke.rs`
//! opens a real `winit` window on a live macOS desktop session (a different
//! machine than the one that wrote this file originally; that one had no
//! usable display, the same "GDM greeter only" gap `spike/a11y-ime/FINDINGS.md`
//! documented for the Linux Orca leg). Two runs confirmed: `create_surface`
//! against the same headless-created adapter this crate already uses
//! (`AMD Radeon Pro 5300M`, Metal — the vertical slice's own GPU) succeeds
//! and configures correctly (`Bgra8UnormSrgb`, physical size reflecting the
//! display's 2x scale factor); `resize` handles both a real `WindowEvent::Resized`
//! and the synchronous-return case `request_inner_size` can take instead
//! (see that method's doc comment — no event follows on some platforms);
//! and 150 acquire/clear/present cycles ran with no panics and a clean
//! exit. Not yet run on Windows/Linux with a live session, or against
//! DX12/Vulkan — see `PLAN.md` M1.2.

use crate::GpuError;
use crate::context::GpuContext;

/// A configured presentation surface for one window.
pub struct GpuSurface<'window> {
    surface: wgpu::Surface<'window>,
    config: wgpu::SurfaceConfiguration,
}

/// Picks the swapchain format from `formats` (a surface's own
/// `SurfaceCapabilities::formats`, in the backend's preference order):
/// the first sRGB-aware format when there is one, otherwise the first
/// plain 8-bit `*Unorm` format, otherwise the first format of any kind,
/// otherwise `None`.
///
/// Why sRGB first: every colour Aurora draws into the window (the clear
/// colour, solid widget fills, gradients) is authored as sRGB-encoded
/// chrome colour, and the app linearizes solids *only* for an sRGB
/// target (`aurora-app`'s `collect_widget_paints`), so an sRGB-aware
/// swapchain gives blending in linear light and hardware-exact encode.
/// `get_default_config` alone takes `formats[0]`, which no graphics API
/// promises is sRGB (it has been `Bgra8UnormSrgb` on every Metal and
/// Vulkan adapter this project has run on, but that is observation, not
/// contract). When no sRGB format is offered, a plain 8-bit UNORM format
/// is preferred next, because the app's non-sRGB rule ("a plain target
/// stores what it is given") only holds for a UNORM swapchain: a float
/// swapchain such as `Rgba16Float` is usually presented as linear light
/// (scRGB / extended-linear), where gamma-encoded chrome colours would
/// come out too bright. Only when neither is offered does the backend's
/// own first choice win — a residual, not a supported configuration.
#[must_use]
pub fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .or_else(|| {
            formats.iter().copied().find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
        })
        .or_else(|| formats.first().copied())
}

impl GpuContext {
    /// Creates and configures a `wgpu::Surface` for `target` at `size`,
    /// matching `spike/vertical-slice`'s own windowed setup
    /// (`get_default_config`, forced `AutoVsync`), except that the
    /// format is [`choose_surface_format`]'s pick — an sRGB-aware format
    /// whenever the surface offers one.
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
        if let Some(format) =
            choose_surface_format(&surface.get_capabilities(self.adapter()).formats)
        {
            config.format = format;
        }
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

#[cfg(test)]
mod tests {
    use super::choose_surface_format;
    use wgpu::TextureFormat;

    #[test]
    fn choose_surface_format_prefers_the_first_srgb_format() {
        assert_eq!(
            choose_surface_format(&[
                TextureFormat::Bgra8Unorm,
                TextureFormat::Rgba16Float,
                TextureFormat::Bgra8UnormSrgb,
                TextureFormat::Rgba8UnormSrgb,
            ]),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
        assert_eq!(
            choose_surface_format(&[TextureFormat::Bgra8UnormSrgb, TextureFormat::Bgra8Unorm]),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn choose_surface_format_prefers_a_plain_unorm_over_a_float_format() {
        assert_eq!(
            choose_surface_format(&[TextureFormat::Rgba16Float, TextureFormat::Bgra8Unorm]),
            Some(TextureFormat::Bgra8Unorm)
        );
    }

    #[test]
    fn choose_surface_format_falls_back_to_the_backends_first_choice() {
        assert_eq!(
            choose_surface_format(&[TextureFormat::Rgba16Float, TextureFormat::Rgb10a2Unorm]),
            Some(TextureFormat::Rgba16Float)
        );
        assert_eq!(choose_surface_format(&[]), None);
    }
}
