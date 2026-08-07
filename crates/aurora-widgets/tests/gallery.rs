//! The headless render harness for PLAN.md's still-open component
//! gallery: renders a real [`WidgetTree`]'s own paint
//! (`aurora_widgets::paint_widget`) through the real GPU path renderer
//! to an offscreen target, reads the result back as a real
//! `aurora_testkit::Image`, and diffs it against a checked-in golden
//! PNG via `aurora_testkit::compare_to_golden` — no window, no event
//! loop, only a real GPU adapter. This is the headless half
//! `tests/headless.rs` doesn't need (no GPU at all there) and
//! `render_test.rs` only partly covers (needs GPU, but samples one
//! pixel, not a whole image against a golden).
//!
//! **Scope, stated honestly.** This gallery covers `Button` only, in
//! the three states `paint_widget` itself resolves (enabled, pressed,
//! disabled) — `paint_widget` itself now covers `Checkbox`/`Slider`/
//! `TextField` too, but extending *this* gallery to them is separate,
//! still-open work: the same "add a tree, call [`render_gallery`],
//! bless a golden" shape once each is worth its own golden-image
//! coverage — not new harness work.
//!
//! Uses only `aurora_widgets`' public API, the same "exercised exactly
//! as an external consumer would use it" discipline `tests/headless.rs`
//! already established for this crate's integration tests.

use aurora_theme::{Palette, Scales, Theme, ThemeSet};
use aurora_widgets::widgets::{
    WidgetKind, insert_button, new_tree, set_button_disabled, set_button_pressed,
};
use aurora_widgets::{GpuMesh, PathPipeline, WidgetId, WidgetTree, paint_widget};
use std::sync::{Mutex, MutexGuard};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Size, Style};

const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

/// One gallery cell's own fixed size. Deliberately explicit, not the
/// button's own real padding-derived content size
/// (`widgets::button`'s own internal `style` function isn't public,
/// and this crate has no text layout wired in yet to give a label its
/// own real content size either) — a real, deterministic pixel size is
/// what a golden-image test needs; this gallery is testing
/// `paint_widget`'s own background-rectangle output, not button
/// content layout.
const CELL: (u32, u32) = (64, 64);
/// Three states, side by side. `64 * 3 * 4 = 768 = 3 * 256`, already a
/// multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` — see
/// [`render_gallery`]'s own doc comment for why that matters and why
/// this harness doesn't (yet) handle a size that isn't.
const GALLERY_SIZE: (u32, u32) = (CELL.0 * 3, CELL.1);

/// Serializes this file's real-GPU tests, this integration test's own
/// copy of the same "one `wgpu::Instance`/`Device` at a time" lock
/// every other real-GPU test file in this workspace carries
/// independently (`src/test_support.rs`'s own doc comment has the full
/// story) — an integration test file compiles to its own separate test
/// binary, so no other crate's or file's lock covers it.
static GPU_TEST_LOCK: Mutex<()> = Mutex::new(());

struct GpuTestContext {
    _guard: MutexGuard<'static, ()>,
    context: aurora_gpu::GpuContext,
}

impl std::ops::Deref for GpuTestContext {
    type Target = aurora_gpu::GpuContext;
    fn deref(&self) -> &aurora_gpu::GpuContext {
        &self.context
    }
}

/// `None` is an inconclusive skip (no GPU adapter on this machine/CI
/// runner); any other failure is a real bug and panics.
fn real_context() -> Option<GpuTestContext> {
    let guard = GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match aurora_gpu::GpuContext::new() {
        Ok(context) => Some(GpuTestContext {
            _guard: guard,
            context,
        }),
        Err(aurora_gpu::GpuError::NoSuitableAdapter) => {
            eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
            None
        }
        Err(err) => {
            #[allow(clippy::panic)]
            {
                panic!("device request failed with a real adapter present: {err}");
            }
        }
    }
}

fn dark_theme() -> Theme {
    let palette = match Palette::from_toml_str(PALETTE_TOML) {
        Ok(palette) => palette,
        Err(err) => unreachable!("the committed palette must parse: {err:?}"),
    };
    let mut themes = ThemeSet::new();
    if let Err(err) = themes.register(DARK_THEME_TOML) {
        unreachable!("the committed Dark theme must register: {err:?}");
    }
    match themes.resolve("Dark", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("the committed Dark theme must resolve: {err:?}"),
    }
}

fn scales() -> Scales {
    match Scales::from_toml_str(SCALES_TOML) {
        Ok(scales) => scales,
        Err(err) => unreachable!("the committed scales must parse: {err:?}"),
    }
}

fn sized_style() -> Style {
    Style {
        size: Size {
            width: length(CELL.0 as f32),
            height: length(CELL.1 as f32),
        },
        ..Default::default()
    }
}

/// A real, laid-out tree with one `Button` per state — `paint_widget`'s
/// own natural minimal fixture, side by side in `CELL`-sized cells.
fn button_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let enabled = match insert_button(&mut tree, root, scales, "Enabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let pressed = match insert_button(&mut tree, root, scales, "Pressed") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_button_pressed(&mut tree, pressed, true) {
        unreachable!("{err:?}");
    }
    let disabled = match insert_button(&mut tree, root, scales, "Disabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_button_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [enabled, pressed, disabled] {
        if let Err(err) = tree.set_style(id, sized_style()) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(GALLERY_SIZE.0 as f32, GALLERY_SIZE.1 as f32);
    (tree, [enabled, pressed, disabled])
}

/// Renders every widget in `tree` (`paint_widget`, in
/// `WidgetTree::paint_order`) onto a `size` offscreen `Rgba8Unorm`
/// target and reads the result back as a real `aurora_testkit::Image`.
///
/// `Rgba8Unorm`, deliberately not the `Bgra8UnormSrgb` `aurora-app`'s
/// own real swapchain uses: a golden PNG stores straight sRGB-gamma
/// bytes directly (what `paint_widget` itself already returns), so
/// this target needs none of `aurora-app`'s own
/// `linearize_paint_color` conversion — applying it here would
/// double-encode, the same bug in the opposite direction from the one
/// that conversion exists to prevent for a real sRGB-aware swapchain.
///
/// All `GpuMesh`es are uploaded and collected *before* the render pass
/// begins, not inside it — `PathPipeline::draw` needs
/// `mesh: &'pass GpuMesh`, so every mesh it draws must outlive the
/// pass, the same constraint `aurora-app`'s own
/// `collect_widget_paints`/`draw_widget_paints` split exists for.
///
/// `size.0 * 4` must already be a multiple of
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (asserted below) — this
/// function doesn't pad/strip row bytes the way a fully general
/// readback would need to for an arbitrary width; every other
/// real-GPU readback helper in this workspace has the same
/// restriction today (each just picks an already-aligned size),
/// matching that established shape rather than introducing new,
/// unexercised padding logic this harness's own single caller doesn't
/// need yet.
fn render_gallery(
    context: &GpuTestContext,
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    size: (u32, u32),
    clear: wgpu::Color,
) -> aurora_testkit::Image {
    let device = context.device();
    let queue = context.queue();

    let widget_paints = collect_gallery_paints(tree, theme, scales, device, queue);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gallery"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gallery"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gallery"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        draw_gallery_paints(&mut pass, device, queue, size, &widget_paints);
    }

    let rgba = read_rgba8(device, queue, encoder, &target, size);
    match aurora_testkit::Image::new(size.0, size.1, rgba) {
        Ok(image) => image,
        Err(err) => {
            unreachable!("read_rgba8 always returns width * height * 4 bytes, no padding: {err}")
        }
    }
}

/// [`render_gallery`]'s own "upload every widget's paint" step, split
/// out so `render_gallery` itself stays under `clippy::too_many_lines`
/// — see that function's own doc comment for *why* this has to happen
/// before the render pass begins, which is the real reason this can't
/// just be a closure inline in `render_gallery`.
fn collect_gallery_paints(
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<(GpuMesh, [f32; 4])> {
    let mut widget_paints = Vec::new();
    for id in tree.paint_order() {
        if let Ok(paints) = paint_widget(tree, id, theme, scales) {
            for (mesh, color) in paints {
                let gpu_mesh = GpuMesh::upload(device, queue, &mesh);
                widget_paints.push((gpu_mesh, color));
            }
        }
    }
    widget_paints
}

/// [`render_gallery`]'s own "draw every widget's paint within the
/// already-begun pass" step.
fn draw_gallery_paints<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    widget_paints: &'pass [(GpuMesh, [f32; 4])],
) {
    if widget_paints.is_empty() {
        return;
    }
    let mut path = PathPipeline::new(device);
    let pipeline = path.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
    pass.set_pipeline(pipeline);
    #[allow(clippy::cast_precision_loss)]
    let viewport_size = (size.0 as f32, size.1 as f32);
    for (mesh, color) in widget_paints {
        let bind_group = path.bind_group(device, queue, viewport_size, *color);
        pass.set_bind_group(0, &bind_group, &[]);
        path.draw(pass, mesh);
    }
}

/// [`render_gallery`]'s own final "copy the rendered target back to the
/// CPU" step — `size.0 * 4` must already be a multiple of
/// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (asserted below); this function
/// doesn't pad/strip row bytes the way a fully general readback would
/// need to for an arbitrary width. Every other real-GPU readback helper
/// in this workspace has the same restriction today (each just picks
/// an already-aligned size) — matching that established shape rather
/// than introducing new, unexercised padding logic this harness's own
/// single caller doesn't need yet.
fn read_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    target: &wgpu::Texture,
    size: (u32, u32),
) -> Vec<u8> {
    let bytes_per_row = size.0 * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "gallery width must keep rows 256-byte aligned -- see read_rgba8's own doc comment"
    );
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gallery-readback"),
        size: u64::from(bytes_per_row) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async must complete once the device has been polled to idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("the buffer was just confirmed mapped successfully above");
    };
    let rgba = data.to_vec();
    drop(data);
    readback_buffer.unmap();
    rgba
}

/// Reads the pixel at the centre of gallery cell `cell` (0 = enabled,
/// 1 = pressed, 2 = disabled) — enough to distinguish the three
/// states' own fill colours without needing a golden image at all.
fn sample_cell_centre(image: &aurora_testkit::Image, cell: u32) -> [u8; 4] {
    let cx = cell * CELL.0 + CELL.0 / 2;
    let cy = CELL.1 / 2;
    let offset = (cy * image.width + cx) as usize * 4;
    let Some(pixel) = image.rgba.get(offset..offset + 4) else {
        unreachable!("cell centre is always within a real gallery image");
    };
    match pixel {
        &[r, g, b, a] => [r, g, b, a],
        _ => unreachable!("sliced exactly 4 bytes"),
    }
}

/// A real, self-contained proof the harness itself works, needing no
/// golden image: the three states resolve to genuinely different
/// pixels. Pressed uses `accent.primary_active` instead of
/// `accent.primary`, so its RGB must differ outright; disabled applies
/// `state.disabled_opacity` alpha-blended over the clear colour, so
/// its RGB reads dimmer than the enabled button's full-strength
/// colour even though the stored *alpha* in the target is opaque
/// either way (blending over an opaque clear always yields opaque
/// output — the final alpha channel alone can't distinguish them).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_button_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, GALLERY_SIZE.0);
    assert_eq!(image.height, GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, 0);
    let pressed_px = sample_cell_centre(&image, 1);
    let disabled_px = sample_cell_centre(&image, 2);
    assert_ne!(
        enabled_px, pressed_px,
        "accent.primary vs accent.primary_active must render differently"
    );
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// The real golden-image regression test this whole harness exists
/// for — `Button`'s own three states, rendered together, diffed
/// against a checked-in golden PNG the same way
/// `aurora-render::composite_over_matches_the_golden_image` already
/// proves this project's own golden-image discipline for the canvas
/// compositor.
///
/// **Blessed and reviewed 2026-08-07**: Cahya ran
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery
/// -- --ignored` on real macOS GPU hardware and confirmed
/// `tests/golden/button_gallery.png` actually shows three visually
/// distinct buttons in Dark theme colours (enabled: bright
/// `accent.primary`; pressed: a darker, more saturated
/// `accent.primary_active`; disabled: a dark, desaturated navy —
/// `accent.primary` alpha-blended at `state.disabled_opacity` over the
/// black clear colour) before this test's own `#[ignore]` was removed
/// — never bless blind, the same discipline
/// `aurora_testkit::compare_to_golden`'s own `AURORA_BLESS_GOLDEN` gate
/// exists to enforce.
#[test]
fn button_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = button_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/button_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}
