//! Application shell: window/event-loop lifecycle, wired to a real
//! `accesskit_winit` accessibility adapter and `aurora-gpu`'s
//! already-proven presentation surface. PLAN.md M1.8's first
//! deliverable.
//!
//! **Scope, stated honestly.** This is the "create hidden → attach
//! adapter → show" ordering ADR 0001's escape-hatch check found
//! (`spike/a11y-ime/FINDINGS.md` finding #1) as real, production code —
//! not yet the actual application. The accessibility tree `build_tree`
//! returns is a single content-free root container
//! (`aurora_widgets::widgets::new_tree`); the app's redraw clears the
//! surface to a placeholder colour with no widget/theme rendering wired
//! in. Docking, panels, the canvas, tools, input routing, IME, native
//! menus, DPI handling, and crash recovery are this milestone's other,
//! separate, still-open bullets — this module doesn't reach for any of
//! them.
//!
//! **Unverified in this sandbox.** There is no display server here
//! (`$DISPLAY`/`$WAYLAND_DISPLAY` both empty) and no `pkg-config`
//! (needed by `winit`'s fontconfig feature) with no root access to
//! install it, so none of this has actually been compiled or run here —
//! only written carefully against `accesskit_winit` 0.33.2's own
//! fetched source and the two proven precedents it mirrors
//! (`spike/a11y-ime`'s windowed app; `aurora-gpu`'s own
//! `examples/surface_smoke.rs`, itself unverified for two days until
//! someone ran it on a live macOS session — see PLAN.md M1.2). Needs a
//! human on a real desktop session, on all three platforms, to confirm.

use std::sync::Arc;

use aurora_gpu::{GpuContext, GpuSurface};
use aurora_widgets::widgets::{self, WidgetKind};
use aurora_widgets::{FocusManager, WidgetId, WidgetTree};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// The (currently content-free) accessibility tree a fresh window
/// starts with — a single root container, via
/// `aurora_widgets::widgets::new_tree`. Real panel/widget content is
/// separate, still-open M1.8 work; this exists so the window/event-loop
/// lifecycle has something real to feed the accessibility adapter,
/// rather than a hand-rolled `accesskit::Node` this crate would have to
/// throw away once real content exists.
#[must_use]
fn build_tree() -> (WidgetTree<WidgetKind>, WidgetId) {
    widgets::new_tree(taffy::Style::default())
}

/// Owns the window, GPU device/surface, and accessibility adapter for
/// one application window. Not part of this crate's public API — [`run`]
/// is the only sanctioned entry point.
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuContext>,
    surface: Option<GpuSurface<'static>>,
    adapter: Option<accesskit_winit::Adapter>,
    proxy: EventLoopProxy<accesskit_winit::Event>,
    tree: WidgetTree<WidgetKind>,
    root: WidgetId,
    focus: FocusManager,
    /// Set when a step that can't be retried fails (window/device/surface
    /// creation) — `run` turns this into a nonzero exit, distinguishing
    /// it from the ordinary, successful case of the user closing the
    /// window.
    failed: bool,
}

impl App {
    #[must_use]
    fn new(proxy: EventLoopProxy<accesskit_winit::Event>) -> Self {
        let (tree, root) = build_tree();
        Self {
            window: None,
            gpu: None,
            surface: None,
            adapter: None,
            proxy,
            tree,
            root,
            focus: FocusManager::default(),
            failed: false,
        }
    }

    /// Whether the app exited because of an earlier unrecoverable error,
    /// rather than an ordinary window close.
    #[must_use]
    fn failed(&self) -> bool {
        self.failed
    }

    /// Sends the current accessibility tree to the platform, if (and
    /// only if) something is actually listening —
    /// `Adapter::update_if_active` is a no-op otherwise, matching
    /// `spike/a11y-ime`'s own `push_a11y` (this project's first, proven
    /// real usage of this exact call).
    fn push_accessibility(&mut self) {
        let Some(adapter) = self.adapter.as_mut() else {
            return;
        };
        let tree = &self.tree;
        let focused = self.focus.focused().unwrap_or(self.root);
        adapter.update_if_active(|| tree.accessibility_update(focused));
    }

    /// Logs `message`, marks this run as failed, and asks the event loop
    /// to exit — the one way an `ApplicationHandler` callback (all of
    /// which are `&mut self` with no `Result` return) can surface an
    /// unrecoverable error, matching `aurora-gpu`'s own
    /// `examples/surface_smoke.rs::report_failure`.
    fn fail(&mut self, el: &ActiveEventLoop, message: &str) {
        tracing::error!("{message}");
        self.failed = true;
        el.exit();
    }

    fn apply_resize(&mut self, size: (u32, u32)) {
        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), self.surface.as_mut()) else {
            return;
        };
        surface.resize(gpu.device(), size);
    }

    /// Clears the surface to a placeholder colour and presents — real
    /// widget/theme rendering is separate, still-open M1.8 work;
    /// invariant §7.3.10 (no hardcoded style values) governs widget
    /// content, not this temporary "the window paints a frame at all"
    /// stand-in.
    fn redraw(&mut self) {
        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), self.surface.as_mut()) else {
            return;
        };
        match surface.acquire() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                let view = texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    gpu.device()
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("aurora-app-frame"),
                        });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("aurora-app-clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.05,
                                    g: 0.05,
                                    b: 0.06,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                gpu.queue().submit(std::iter::once(encoder.finish()));
                gpu.queue().present(texture);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {}
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                if let Some(window) = self.window.as_ref() {
                    let size = window.inner_size();
                    surface.resize(gpu.device(), (size.width, size.height));
                }
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::error!("surface texture acquisition raised a validation error");
            }
        }
    }
}

impl ApplicationHandler<accesskit_winit::Event> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Created hidden on purpose: `accesskit_winit::Adapter` panics
        // if constructed after the window is first shown
        // (`spike/a11y-ime/FINDINGS.md` finding #1) — this is that
        // ordering, as real production code, not a spike anymore.
        let attrs = Window::default_attributes()
            .with_title("Aurora")
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = match el.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.fail(el, &format!("window creation failed: {err}"));
                return;
            }
        };

        let gpu = match GpuContext::new() {
            Ok(gpu) => gpu,
            Err(err) => {
                self.fail(el, &format!("GpuContext::new failed: {err}"));
                return;
            }
        };
        let size = window.inner_size();
        let surface =
            match gpu.create_surface(window.clone(), (size.width.max(1), size.height.max(1))) {
                Ok(surface) => surface,
                Err(err) => {
                    self.fail(el, &format!("create_surface failed: {err}"));
                    return;
                }
            };

        let adapter =
            accesskit_winit::Adapter::with_event_loop_proxy(el, &window, self.proxy.clone());
        window.set_visible(true);

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.surface = Some(surface);
        self.adapter = Some(adapter);
        self.push_accessibility();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: accesskit_winit::Event) {
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                // `Adapter::with_event_loop_proxy` can't synchronously
                // return an initial tree (see its own doc comment); this
                // is the deferred push it expects in response.
                self.push_accessibility();
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                // No interactive widgets exist yet to route this to —
                // real input/focus wiring is separate, still-open M1.8
                // work. Logged, not dropped silently.
                tracing::debug!(action = ?request.action, "accessibility action requested (not yet routed to a widget)");
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                tracing::debug!("accessibility deactivated");
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(adapter), Some(window)) = (self.adapter.as_mut(), self.window.as_ref()) {
            adapter.process_event(window, &event);
        }

        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => self.apply_resize((size.width, size.height)),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

impl std::fmt::Debug for App {
    // Manual, summary-only impl: `accesskit_winit::Adapter` doesn't
    // itself implement `Debug`, the same reason `aurora_widgets::WidgetTree`
    // already writes its own rather than deriving one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("window", &self.window.is_some())
            .field("gpu", &self.gpu.is_some())
            .field("surface", &self.surface.is_some())
            .field("adapter", &self.adapter.is_some())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

/// Runs the application to completion. An ordinary window close is
/// `Ok(())`; a failure creating the event loop, or an unrecoverable
/// error during a `resumed` step (window/device/surface creation), is
/// an error.
///
/// # Errors
///
/// Returns an error if the event loop can't be created or fails while
/// running, or if the app recorded an unrecoverable error during the
/// run (e.g. window, GPU device, or surface creation failing).
pub fn run() -> anyhow::Result<()> {
    let event_loop = EventLoop::<accesskit_winit::Event>::with_user_event()
        .build()
        .map_err(|err| anyhow::anyhow!("event loop creation failed: {err}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let mut app = App::new(proxy);
    event_loop
        .run_app(&mut app)
        .map_err(|err| anyhow::anyhow!("event loop run failed: {err}"))?;

    anyhow::ensure!(
        !app.failed(),
        "application exited due to an earlier unrecoverable error"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_tree;

    /// The one piece of this module answerable without a real window:
    /// the accessibility tree a fresh app starts with is exactly the
    /// content-free root `aurora_widgets::widgets::new_tree` produces.
    /// Everything else here is an `ApplicationHandler` callback, which
    /// has no meaningful headless test double in `winit` — see this
    /// module's own doc comment for what remains unverified in this
    /// sandbox and why.
    #[test]
    fn build_tree_starts_with_exactly_the_root() {
        let (tree, root) = build_tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.root(), root);
    }
}
