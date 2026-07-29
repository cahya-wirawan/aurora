//! Manual verification for `GpuSurface`: opens a real window, configures a
//! presentation surface against it, drives a couple of seconds of frames,
//! and resizes the window twice — the thing `src/surface.rs` documented as
//! implemented-but-unverified because the machine that wrote it had no
//! usable display (`PLAN.md` M1.2). This one does, so this closes that gap.
//!
//! Run with `cargo run --example surface_smoke -p aurora-gpu`. It exits on
//! its own once the resize sequence completes; Esc quits early. A non-zero
//! exit code means something in the surface path actually failed.

use aurora_gpu::{GpuContext, GpuSurface};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const FIRST_RESIZE_AT_FRAME: u32 = 30;
const SECOND_RESIZE_AT_FRAME: u32 = 90;
const EXIT_AT_FRAME: u32 = 150;

struct App {
    window: Option<Arc<Window>>,
    context: Option<GpuContext>,
    surface: Option<GpuSurface<'static>>,
    frame: u32,
    resizes_seen: u32,
    failed: bool,
}

/// A free function rather than an `App` method so it only ever borrows the
/// one field it needs — `redraw` already holds other fields borrowed via
/// `as_ref()`/`as_mut()` when a failure can occur mid-frame, and a method
/// needing the whole `&mut self` would conflict with those.
fn report_failure(failed: &mut bool, el: &ActiveEventLoop, message: &str) {
    eprintln!("SURFACE_SMOKE_FAILED: {message}");
    *failed = true;
    el.exit();
}

fn cycling_color(frame: u32) -> wgpu::Color {
    let t = f64::from(frame % 180) / 180.0 * std::f64::consts::TAU;
    wgpu::Color {
        r: 0.5 + 0.5 * t.sin(),
        g: 0.5 + 0.5 * (t + std::f64::consts::TAU / 3.0).sin(),
        b: 0.5 + 0.5 * (t + 2.0 * std::f64::consts::TAU / 3.0).sin(),
        a: 1.0,
    }
}

impl App {
    /// Shared by the real `WindowEvent::Resized` path and the synchronous
    /// case `request_inner_size` can return directly (see its doc comment:
    /// on some platforms it applies the size immediately and no `Resized`
    /// event follows at all).
    fn apply_resize(&mut self, size: (u32, u32)) {
        let (Some(context), Some(surface)) = (self.context.as_ref(), self.surface.as_mut()) else {
            return;
        };
        surface.resize(context.device(), size);
        self.resizes_seen += 1;
        println!("resize #{}: now {:?}", self.resizes_seen, surface.size());
    }

    fn redraw(&mut self, el: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let (Some(context), Some(surface)) = (self.context.as_ref(), self.surface.as_mut()) else {
            return;
        };

        match surface.acquire() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                let view = texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    context
                        .device()
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("surface-smoke-clear"),
                        });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("surface-smoke-clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(cycling_color(self.frame)),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                context.queue().submit(std::iter::once(encoder.finish()));
                context.queue().present(texture);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {}
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                let size = window.inner_size();
                surface.resize(context.device(), (size.width, size.height));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                report_failure(
                    &mut self.failed,
                    el,
                    "surface texture acquisition raised a validation error",
                );
                return;
            }
        }

        self.frame += 1;
        match self.frame {
            FIRST_RESIZE_AT_FRAME => {
                if let Some(size) = window.request_inner_size(LogicalSize::new(900.0, 600.0)) {
                    self.apply_resize((size.width, size.height));
                }
            }
            SECOND_RESIZE_AT_FRAME => {
                if let Some(size) = window.request_inner_size(LogicalSize::new(400.0, 300.0)) {
                    self.apply_resize((size.width, size.height));
                }
            }
            EXIT_AT_FRAME => {
                println!(
                    "SURFACE_SMOKE_OK frames={} resizes={}",
                    self.frame, self.resizes_seen
                );
                el.exit();
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("aurora-gpu surface smoke test")
            .with_inner_size(LogicalSize::new(640.0, 480.0));
        let window = match el.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                report_failure(
                    &mut self.failed,
                    el,
                    &format!("window creation failed: {err}"),
                );
                return;
            }
        };

        let context = match GpuContext::new() {
            Ok(context) => context,
            Err(err) => {
                report_failure(
                    &mut self.failed,
                    el,
                    &format!("GpuContext::new failed: {err}"),
                );
                return;
            }
        };
        let info = context.adapter_info();
        println!(
            "adapter: {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );

        let size = window.inner_size();
        let surface =
            match context.create_surface(window.clone(), (size.width.max(1), size.height.max(1))) {
                Ok(surface) => surface,
                Err(err) => {
                    report_failure(
                        &mut self.failed,
                        el,
                        &format!("create_surface failed: {err}"),
                    );
                    return;
                }
            };
        println!(
            "surface configured: format={:?} size={:?}",
            surface.format(),
            surface.size()
        );

        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == Key::Named(NamedKey::Escape) {
                    println!(
                        "Esc pressed after {} frames, {} resizes",
                        self.frame, self.resizes_seen
                    );
                    el.exit();
                }
            }
            WindowEvent::Resized(new_size) => {
                self.apply_resize((new_size.width, new_size.height));
            }
            WindowEvent::RedrawRequested => self.redraw(el),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

fn main() {
    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("SURFACE_SMOKE_FAILED: event loop creation failed: {err}");
            std::process::exit(1);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        window: None,
        context: None,
        surface: None,
        frame: 0,
        resizes_seen: 0,
        failed: false,
    };

    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("SURFACE_SMOKE_FAILED: event loop run failed: {err}");
        std::process::exit(1);
    }

    if app.failed {
        std::process::exit(1);
    }
}
