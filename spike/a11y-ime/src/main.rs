//! Accessibility + IME spike — ADR 0001's escape-hatch triggers.
//!
//! Aurora renders its own UI, so a text field is just pixels: nothing about it
//! is announced to a screen reader, and nothing routes CJK composition into it,
//! unless we build that. This spike finds out whether the Rust stack can do it
//! before the widget toolkit is written, because a failure here triggers the
//! CXX-Qt fallback and that is far cheaper to act on now.
//!
//!   cargo run                  windowed: type, compose CJK, run a screen reader
//!   cargo run -- --dump-tree   print the accessibility tree and exit
//!
//! What "passing" means is deliberately manual: a screen reader either speaks
//! the field or it does not, and no automated check substitutes for hearing it.
//! `--dump-tree` proves the tree is *built*; the checklist below proves it is
//! *delivered*.

mod checklist;
mod field;
mod tree;

use accesskit_winit::Adapter;
use checklist::{Checklist, Context, Verdict};
use field::TextField;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const FIELD_X: f32 = 40.0;
const FIELD_Y: f32 = 120.0;
const FIELD_W: f32 = 700.0;
const FIELD_H: f32 = 44.0;

fn main() {
    if std::env::args().any(|a| a == "--dump-tree") {
        tree::dump();
        return;
    }
    // Renders the result file from sample answers, so the report format can be
    // reviewed without a GUI, a screen reader, or an IME.
    if std::env::args().any(|a| a == "--demo-report") {
        checklist::demo_report();
        return;
    }
    windowed();
}

struct Gfx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    buffer: Buffer,
}

struct App {
    window: Option<Arc<Window>>,
    proxy: winit::event_loop::EventLoopProxy<accesskit_winit::Event>,
    gfx: Option<Gfx>,
    adapter: Option<Adapter>,
    field: TextField,
    ime_events: Vec<String>,
    focused: bool,
    modifiers: winit::keyboard::ModifiersState,
    checklist: Checklist,
    /// Set when the platform actually asks for the tree, i.e. an assistive
    /// technology connected. Observed rather than self-reported.
    a11y_activated: bool,
    backend: String,
}

impl App {
    fn rebuild_text(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        // Preedit is shown inline, bracketed, so composition is visible without a
        // full underline-attribute renderer. A real implementation draws the
        // platform's underline styles.
        let display = self.field.display_text();
        let info = format!(
            "{}\n\
             ----------------------------------------------------------\n\
             Type here: {}\n\
             value {:?}   preedit {:?}\n\
             AT connected: {}   IME events: {}\n{}",
            self.checklist.screen_text(),
            display,
            self.field.text,
            self.field.preedit,
            if self.a11y_activated { "YES" } else { "not yet" },
            self.ime_events.len(),
            self.ime_events
                .iter()
                .rev()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        );
        gfx.buffer.set_text(
            &info,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        gfx.buffer.shape_until_scroll(&mut gfx.font_system, false);
    }

    /// Push the current field state to the platform accessibility API.
    fn push_a11y(&mut self) {
        let Some(adapter) = self.adapter.as_mut() else {
            return;
        };
        let field = &self.field;
        let focused = self.focused;
        adapter.update_if_active(|| tree::update(field, focused));
    }
}

impl ApplicationHandler<accesskit_winit::Event> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Created hidden on purpose: accesskit_winit panics if the adapter is
        // constructed after the window is first shown. Accessibility therefore
        // constrains window *creation order*, not just widget code — worth
        // knowing before a window-management layer exists to retrofit.
        let attrs = Window::default_attributes()
            .with_title("Aurora a11y + IME spike")
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(820.0, 520.0));
        let window = Arc::new(el.create_window(attrs).expect("window"));

        // Without this, winit never delivers Ime events and CJK input is
        // silently impossible — the single most important line in this spike.
        window.set_ime_allowed(true);
        window.set_ime_cursor_area(
            winit::dpi::LogicalPosition::new(FIELD_X, FIELD_Y),
            winit::dpi::LogicalSize::new(FIELD_W, FIELD_H),
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter_gpu = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter_gpu.request_device(&wgpu::DeviceDescriptor {
            label: Some("a11y-ime"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .expect("device");

        self.backend = format!("{:?}", adapter_gpu.get_info().backend);

        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter_gpu, size.width.max(1), size.height.max(1))
            .expect("surface config");
        surface.configure(&device, &config);

        let mut font_system = FontSystem::new();
        let swash = SwashCache::new();
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let viewport = Viewport::new(&device, &cache);
        let renderer = TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(18.0, 26.0));
        buffer.set_size(Some(760.0), Some(460.0));

        self.gfx = Some(Gfx {
            device,
            queue,
            surface,
            config,
            font_system,
            swash,
            atlas,
            viewport,
            renderer,
            buffer,
        });

        // accesskit_winit hooks the platform accessibility API for this window.
        // On macOS/Windows this is what a screen reader actually talks to.
        self.adapter = Some(Adapter::with_event_loop_proxy(el, &window, self.proxy.clone()));
        window.set_visible(true);
        self.window = Some(window);
        self.focused = true;
        self.rebuild_text();
        self.push_a11y();
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: accesskit_winit::Event) {
        // A screen reader can drive the app: focus changes, and actions like
        // "set value" or "move cursor" arrive here rather than from the keyboard.
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                // The platform only asks for a tree when something is listening.
                self.a11y_activated = true;
                self.push_a11y();
                self.rebuild_text();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            accesskit_winit::WindowEvent::ActionRequested(req) => {
                println!("screen reader requested action: {:?}", req.action);
                self.a11y_activated = true;
                self.field.handle_action(&req);
                self.rebuild_text();
                self.push_a11y();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                println!("accessibility deactivated");
            }
        }
        let _ = el;
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(adapter), Some(window)) = (self.adapter.as_mut(), self.window.as_ref()) {
            adapter.process_event(window, &event);
        }

        match event {
            WindowEvent::CloseRequested => {
                self.report();
                el.exit();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                self.push_a11y();
            }
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.config.width = size.width.max(1);
                    gfx.config.height = size.height.max(1);
                    gfx.surface.configure(&gfx.device, &gfx.config);
                }
            }
            // IME composition. Preedit is uncommitted text the platform is still
            // building; Commit is what actually enters the field.
            WindowEvent::Ime(ime) => {
                match &ime {
                    Ime::Enabled => self.ime_events.push("Enabled".into()),
                    Ime::Preedit(text, cursor) => {
                        self.ime_events
                            .push(format!("Preedit({text:?}, cursor={cursor:?})"));
                        self.field.set_preedit(text.clone());
                    }
                    Ime::Commit(text) => {
                        self.ime_events.push(format!("Commit({text:?})"));
                        self.field.commit(text);
                    }
                    Ime::Disabled => self.ime_events.push("Disabled".into()),
                }
                self.rebuild_text();
                self.push_a11y();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                use winit::event::ElementState;
                use winit::keyboard::{Key, NamedKey};
                if event.state != ElementState::Pressed {
                    return;
                }
                // Checklist controls use ctrl+ so plain typing — and IME
                // composition — still reach the field untouched.
                if self.modifiers.control_key() {
                    let handled = match &event.logical_key {
                        Key::Character(c) => match c.as_str() {
                            "y" | "Y" => {
                                self.checklist.record(Verdict::Pass);
                                true
                            }
                            "n" | "N" => {
                                self.checklist.record(Verdict::Fail);
                                true
                            }
                            "k" | "K" => {
                                self.checklist.record(Verdict::Skipped);
                                true
                            }
                            "b" | "B" => {
                                self.checklist.back();
                                true
                            }
                            _ => false,
                        },
                        _ => false,
                    };
                    if handled {
                        self.rebuild_text();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }

                // While composing, the platform owns the keystrokes.
                if !self.field.preedit.is_empty() {
                    return;
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        self.report();
                        el.exit();
                        return;
                    }
                    Key::Named(NamedKey::Backspace) => self.field.backspace(),
                    Key::Named(NamedKey::ArrowLeft) => self.field.move_left(),
                    Key::Named(NamedKey::ArrowRight) => self.field.move_right(),
                    Key::Named(NamedKey::Space) => self.field.insert(" "),
                    Key::Character(s) => self.field.insert(s),
                    _ => return,
                }
                self.rebuild_text();
                self.push_a11y();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }
}

impl App {
    fn draw(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        gfx.viewport.update(
            &gfx.queue,
            Resolution {
                width: gfx.config.width,
                height: gfx.config.height,
            },
        );
        gfx.renderer
            .prepare(
                &gfx.device,
                &gfx.queue,
                &mut gfx.font_system,
                &mut gfx.atlas,
                &gfx.viewport,
                [TextArea {
                    buffer: &gfx.buffer,
                    left: 30.0,
                    top: 24.0,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: gfx.config.width as i32,
                        bottom: gfx.config.height as i32,
                    },
                    default_color: Color::rgb(0xE8, 0xE8, 0xEC),
                    custom_glyphs: &[],
                }],
                &mut gfx.swash,
            )
            .expect("prepare");

        let mut enc = gfx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.09,
                            g: 0.09,
                            b: 0.11,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            gfx.renderer
                .render(&gfx.atlas, &gfx.viewport, &mut pass)
                .expect("render");
        }
        gfx.queue.submit(Some(enc.finish()));
        gfx.queue.present(frame);
    }

    fn report(&self) {
        let ctx = Context {
            platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            backend: self.backend.clone(),
            a11y_activated: self.a11y_activated,
            ime_events: self.ime_events.clone(),
            field_text: self.field.text.clone(),
        };
        let report = self.checklist.report(&ctx);
        let name = format!("result-{}.md", std::env::consts::OS);
        match std::fs::write(&name, &report) {
            Ok(()) => println!("\n{report}\n--- written to {name} ---"),
            Err(e) => println!("\n{report}\n(could not write {name}: {e})"),
        }
    }
}

fn windowed() {
    let el = EventLoop::<accesskit_winit::Event>::with_user_event()
        .build()
        .expect("event loop");
    el.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let proxy = el.create_proxy();

    println!("Aurora accessibility + IME spike");
    println!("  1. Turn on a screen reader (VoiceOver ⌘F5 / Narrator ⊞Ctrl+Enter / Orca).");
    println!("     It should announce a text field, its label, and its current value.");
    println!("  2. Switch to a CJK input method and compose — e.g. Pinyin \"ni hao\".");
    println!("     Preedit should appear inline and Commit should insert the characters.");
    println!("  3. Answer each item with ctrl+Y / ctrl+N / ctrl+K (skip).");
    println!("  4. Esc when done — a result-<os>.md file is written for pasting.\n");

    let mut app = App {
        window: None,
        gfx: None,
        adapter: None,
        field: TextField::default(),
        ime_events: Vec::new(),
        focused: false,
        modifiers: winit::keyboard::ModifiersState::empty(),
        checklist: Checklist::default(),
        a11y_activated: false,
        backend: String::from("unknown"),
        proxy,
    };
    el.run_app(&mut app).expect("run");
}
