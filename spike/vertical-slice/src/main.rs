//! Aurora vertical slice — PRD §13 Step 4.
//!
//! One narrow path through the whole stack: window → wgpu → tiled half-float
//! document larger than RAM → per-tile composite → brush stroke → save/reload,
//! with a docked panel drawn in the same frame as the canvas.
//!
//! It exists to produce measurements. The §6 performance budgets are currently
//! assertions; this turns them into numbers.
//!
//!   cargo run --release                 windowed, paint with the mouse
//!   cargo run --release -- --headless   benchmark, no display needed
//!
//! NOT covered here, and still open (ADR 0001): screen-reader and CJK IME
//! spikes. Those need real text input and `accesskit`, which is its own slice.

mod doc;
mod renderer;
mod storage;
mod tiles;

use doc::Document;
use renderer::Renderer;
use std::path::PathBuf;
use std::time::Instant;
use tiles::TILE_BYTES;

/// Deliberately smaller than the working set so paging is exercised, not avoided.
const MEMORY_BUDGET: usize = 64 * 1024 * 1024;
const DOC: (u32, u32) = (100_000, 100_000);
const BRUSH_RADIUS: f32 = 24.0;
const BRUSH_SPACING: f32 = 0.15;
const INK: [f32; 3] = [0.95, 0.62, 0.25];

fn scratch_dir() -> PathBuf {
    let d = std::env::temp_dir().join("aurora-slice");
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn main() {
    if std::env::args().any(|a| a == "--headless") {
        pollster::block_on(headless_bench());
    } else {
        windowed();
    }
}

// ---------------------------------------------------------------------------
// Headless: the measurements, runnable anywhere including CI.
// ---------------------------------------------------------------------------

async fn headless_bench() {
    let viewport = (1600u32, 1000u32);
    let scratch = scratch_dir();
    let mut doc = Document::new(DOC.0, DOC.1, &scratch, MEMORY_BUDGET).expect("document");

    println!("Aurora vertical slice — headless benchmark");
    println!("  document      {} × {} px", DOC.0, DOC.1);
    println!(
        "  full size     {:.0} GB at 8 bytes/px (half-float RGBA, ADR 0003)",
        (f64::from(DOC.0) * f64::from(DOC.1) * 8.0) / 1e9
    );
    println!(
        "  tile budget   {} MB = {} resident tiles of {} KB",
        MEMORY_BUDGET / (1024 * 1024),
        doc.base.budget_tiles(),
        TILE_BYTES / 1024
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None, renderer::CANVAS_FORMAT, viewport).await;
    println!();

    // Offscreen target, standing in for the swapchain image.
    let target = r.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: wgpu::Extent3d {
            width: viewport.0,
            height: viewport.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer::CANVAS_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // --- 1. Stroke latency -------------------------------------------------
    // Input → dabs stamped → dirty tiles composited and uploaded → frame
    // submitted. The §6 budget is 10 ms.
    println!("[1] Stroke latency (input → frame submitted), budget 10 ms");
    doc.begin_stroke();
    let mut lat = Vec::new();
    let mut pos = (50_000.0f32, 50_000.0f32);
    r.set_view(viewport, (49_200, 49_500));

    for i in 0..240 {
        let next = (
            pos.0 + f32::cos(i as f32 * 0.11) * 9.0,
            pos.1 + f32::sin(i as f32 * 0.09) * 9.0,
        );
        let t0 = Instant::now();
        doc.stroke_segment(pos, next, BRUSH_RADIUS, BRUSH_SPACING, INK)
            .expect("stroke");
        r.sync_tiles(&mut doc, false).expect("sync");
        r.set_ui(viewport, 0.5);
        r.draw(&target_view);
        r.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok();
        lat.push(t0.elapsed().as_secs_f64() * 1000.0);
        pos = next;
    }
    report_ms(&mut lat, 10.0);

    // --- 2. Steady-state frame cost ---------------------------------------
    println!("\n[2] Frame with no canvas change (UI + present only), budget 16.7 ms");
    let mut frames = Vec::new();
    for _ in 0..120 {
        let t0 = Instant::now();
        r.sync_tiles(&mut doc, false).expect("sync");
        r.set_ui(viewport, 0.2);
        r.draw(&target_view);
        r.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).ok();
        frames.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    report_ms(&mut frames, 16.7);

    // --- 3. Panning ---------------------------------------------------------
    // Two speeds, because "panning is slow" is not a finding until you know
    // whether the speed was realistic. 200 px/frame is a normal drag; 620
    // px/frame is a fast fling, which at 256 px tiles exposes an entire new
    // screenful of tiles every frame.
    println!("\n[3] Panning (upload-bound), budget 16.7 ms");
    let merged = doc.end_stroke().expect("merge");
    println!("    stroke merged into base: {merged} tiles");

    let mut pan_speed = |label: &str, step_px: u32, steps: u32, doc: &mut Document| {
        let mut t = Vec::new();
        for step in 0..steps {
            let x = 5_000 + step * step_px;
            let y = 5_000 + step * step_px * 6 / 10;
            let t0 = Instant::now();
            r.set_view(viewport, (x, y));
            doc.begin_stroke();
            doc.stroke_segment(
                (x as f32 + 300.0, y as f32 + 300.0),
                (x as f32 + 900.0, y as f32 + 700.0),
                BRUSH_RADIUS,
                BRUSH_SPACING,
                INK,
            )
            .expect("stroke");
            doc.end_stroke().expect("merge");
            r.sync_tiles(doc, false).expect("sync");
            r.set_ui(viewport, 1.0);
            r.draw(&target_view);
            r.device
                .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
                .ok();
            t.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        print!("    {label:<22}");
        report_ms(&mut t, 16.7);
    };

    pan_speed("normal drag 200px:", 200, 120, &mut doc);
    pan_speed("fast fling  620px:", 620, 120, &mut doc);

    // Where does the paint-while-panning frame actually go? Guessing here would
    // be the difference between fixing the tile store and fixing the wrong thing.
    {
        let (mut t_paint, mut t_sync, mut t_draw) = (Vec::new(), Vec::new(), Vec::new());
        for step in 0..60u32 {
            let x = 40_000 + step * 200;
            let y = 40_000 + step * 120;
            r.set_view(viewport, (x, y));

            let a = Instant::now();
            doc.begin_stroke();
            doc.stroke_segment(
                (x as f32 + 300.0, y as f32 + 300.0),
                (x as f32 + 900.0, y as f32 + 700.0),
                BRUSH_RADIUS,
                BRUSH_SPACING,
                INK,
            )
            .expect("stroke");
            doc.end_stroke().expect("merge");
            let b = Instant::now();
            r.sync_tiles(&mut doc, false).expect("sync");
            let c = Instant::now();
            r.set_ui(viewport, 1.0);
            r.draw(&target_view);
            r.device
                .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
                .ok();
            let d = Instant::now();
            t_paint.push((b - a).as_secs_f64() * 1000.0);
            t_sync.push((c - b).as_secs_f64() * 1000.0);
            t_draw.push((d - c).as_secs_f64() * 1000.0);
        }
        println!("\n    Frame breakdown while painting and panning:");
        print!("    {:<22}", "stroke + merge (CPU):");
        report_ms(&mut t_paint, 16.7);
        print!("    {:<22}", "composite + upload:");
        report_ms(&mut t_sync, 16.7);
        print!("    {:<22}", "draw + present:");
        report_ms(&mut t_draw, 16.7);
    }

    // Revisit territory already evicted, so tiles must page back in from disk.
    println!("\n    Revisiting evicted regions (forces page-in):");
    let mut revisit = Vec::new();
    for step in (0..120u32).rev() {
        let x = 5_000 + step * 200;
        let y = 5_000 + step * 120;
        let t0 = Instant::now();
        r.set_view(viewport, (x, y));
        r.sync_tiles(&mut doc, false).expect("sync");
        r.set_ui(viewport, 0.6);
        r.draw(&target_view);
        r.device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .ok();
        revisit.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    print!("    {:<22}", "page-in pan:");
    report_ms(&mut revisit, 16.7);

    let s = doc.base.stats;
    println!(
        "\n    tiles created {}  evicted {}  page faults {}  peak resident {}/{}",
        s.tiles_created,
        s.evictions,
        s.faults,
        s.peak_resident,
        doc.base.budget_tiles()
    );
    println!(
        "    scratch I/O   {:.1} MB out, {:.1} MB in",
        s.bytes_out as f64 / 1e6,
        s.bytes_in as f64 / 1e6
    );

    // --- 4. Save and reload ------------------------------------------------
    println!("\n[4] Save and reload");
    let path = scratch.join("slice.aurslice");
    let t0 = Instant::now();
    let bytes = storage::save(&mut doc, &path).expect("save");
    let save_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut reloaded =
        Document::new(DOC.0, DOC.1, &scratch.join("reload"), MEMORY_BUDGET).expect("document");
    let t0 = Instant::now();
    let count = storage::load(&mut reloaded, &path).expect("load");
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;

    println!(
        "    wrote {} tiles, {:.1} MB in {:.0} ms ({:.0} MB/s)",
        count,
        bytes as f64 / 1e6,
        save_ms,
        bytes as f64 / 1e6 / (save_ms / 1000.0)
    );
    println!(
        "    read  {count} tiles in {:.0} ms ({:.0} MB/s)",
        load_ms,
        bytes as f64 / 1e6 / (load_ms / 1000.0)
    );

    // Round-trip must be exact: f16 in, f16 out, no conversion anywhere.
    let mut checked = 0usize;
    let mut mismatched = 0usize;
    for id in doc.base.known_ids().into_iter().take(64) {
        let a = doc.base.get(id).expect("a").map(|t| t.texels.clone());
        let b = reloaded.base.get(id).expect("b").map(|t| t.texels.clone());
        match (a, b) {
            (Some(a), Some(b)) => {
                checked += 1;
                if a != b {
                    mismatched += 1;
                }
            }
            _ => mismatched += 1,
        }
    }
    println!(
        "    round-trip    {checked} tiles compared, {mismatched} mismatched → {}",
        if mismatched == 0 { "EXACT" } else { "LOSSY (bug)" }
    );

    println!("\nScratch left at {}", scratch.display());
}

fn report_ms(v: &mut [f64], budget: f64) {
    v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let pct = |p: f64| v[((v.len() as f64 - 1.0) * p) as usize];
    let mean: f64 = v.iter().sum::<f64>() / v.len() as f64;
    let p99 = pct(0.99);
    println!(
        "    n={:<4} mean {:6.2}  p50 {:6.2}  p95 {:6.2}  p99 {:6.2}  max {:6.2}  [{}]",
        v.len(),
        mean,
        pct(0.50),
        pct(0.95),
        p99,
        v[v.len() - 1],
        if p99 <= budget { "within budget" } else { "OVER BUDGET" }
    );
}

// ---------------------------------------------------------------------------
// Windowed: the same path, driven by a real mouse.
// ---------------------------------------------------------------------------

fn windowed() {
    use winit::application::ApplicationHandler;
    use winit::event::{ElementState, MouseButton, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    struct App {
        window: Option<std::sync::Arc<Window>>,
        surface: Option<wgpu::Surface<'static>>,
        renderer: Option<Renderer>,
        doc: Document,
        viewport: (u32, u32),
        scroll: (u32, u32),
        painting: bool,
        surface_format: wgpu::TextureFormat,
        last: Option<(f32, f32)>,
        latencies: Vec<f64>,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let attrs = Window::default_attributes()
                .with_title("Aurora vertical slice — drag to paint, Esc to quit")
                .with_inner_size(winit::dpi::LogicalSize::new(1400.0, 900.0));
            let window = std::sync::Arc::new(el.create_window(attrs).expect("window"));
            let size = window.inner_size();
            self.viewport = (size.width.max(1), size.height.max(1));

            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let surface = instance.create_surface(window.clone()).expect("surface");
            let renderer = pollster::block_on(Renderer::new(
                &instance,
                Some(&surface),
                wgpu::TextureFormat::Bgra8UnormSrgb,
                self.viewport,
            ));
            let mut config = surface
                .get_default_config(&renderer.adapter, self.viewport.0, self.viewport.1)
                .expect("surface unsupported by adapter");
            config.present_mode = wgpu::PresentMode::AutoVsync;
            surface.configure(&renderer.device, &config);
            self.surface_format = config.format;
            self.surface = Some(surface);
            self.renderer = Some(renderer);
            self.window = Some(window);
        }

        fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            let (Some(r), Some(surface), Some(window)) = (
                self.renderer.as_mut(),
                self.surface.as_ref(),
                self.window.as_ref(),
            ) else {
                return;
            };
            match event {
                WindowEvent::CloseRequested => el.exit(),
                WindowEvent::KeyboardInput { event, .. } => {
                    use winit::keyboard::{Key, NamedKey};
                    if event.logical_key == Key::Named(NamedKey::Escape) {
                        if !self.latencies.is_empty() {
                            println!("\nStroke latency this session:");
                            report_ms(&mut self.latencies, 10.0);
                        }
                        el.exit();
                    }
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    if state == ElementState::Pressed {
                        self.painting = true;
                        self.doc.begin_stroke();
                    } else {
                        self.painting = false;
                        self.last = None;
                        let _ = self.doc.end_stroke();
                        window.request_redraw();
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if !self.painting {
                        return;
                    }
                    let p = (
                        position.x as f32 + self.scroll.0 as f32,
                        position.y as f32 + self.scroll.1 as f32,
                    );
                    let t0 = Instant::now();
                    let from = self.last.unwrap_or(p);
                    let _ = self
                        .doc
                        .stroke_segment(from, p, BRUSH_RADIUS, BRUSH_SPACING, INK);
                    self.last = Some(p);
                    let _ = r.sync_tiles(&mut self.doc, false);
                    self.latencies.push(t0.elapsed().as_secs_f64() * 1000.0);
                    window.request_redraw();
                }
                WindowEvent::RedrawRequested => {
                    let frame = match surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(f)
                        | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                        _ => return,
                    };
                    let view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    r.set_view(self.viewport, self.scroll);
                    let _ = r.sync_tiles(&mut self.doc, false);
                    let busy = f32::from(u16::try_from(r.uploads_last_frame).unwrap_or(u16::MAX))
                        / 24.0;
                    r.set_ui(self.viewport, busy);
                    r.draw(&view);
                    // wgpu 30: presentation moved from SurfaceTexture to Queue.
                    r.queue.present(frame);
                }
                _ => {}
            }
        }
    }

    let scratch = scratch_dir();
    let doc = Document::new(DOC.0, DOC.1, &scratch, MEMORY_BUDGET).expect("document");
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(winit::event_loop::ControlFlow::Wait);
    let mut app = App {
        window: None,
        surface: None,
        renderer: None,
        doc,
        viewport: (1400, 900),
        scroll: (50_000, 50_000),
        painting: false,
        surface_format: wgpu::TextureFormat::Bgra8UnormSrgb,
        last: None,
        latencies: Vec::new(),
    };
    el.run_app(&mut app).expect("run");
}
