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
//! **Scope, stated honestly.** `paint_widget` covers `Button`,
//! `Checkbox`, `Slider`, `TextField`, and `CommandPalette` — this
//! gallery now covers all five, each its own small tree, its own
//! `render_gallery` call, its own golden PNG, the same "add a tree,
//! call [`render_gallery`], bless a golden" shape the very first
//! version of this file's own doc comment already predicted. Nothing
//! here re-litigates what each widget's own `paint_widget` case
//! already decides (colours, which states exist) — see `src/paint.rs`
//! for that; this file only proves the render pipeline actually
//! produces the pixels those decisions imply.
//!
//! Uses only `aurora_widgets`' public API, the same "exercised exactly
//! as an external consumer would use it" discipline `tests/headless.rs`
//! already established for this crate's integration tests.

use aurora_theme::{Palette, Scales, Theme, ThemeSet};
use aurora_widgets::widgets::{
    CommandEntry, WidgetKind, insert_button, insert_checkbox, insert_command_palette,
    insert_slider, insert_text_field, new_tree, set_button_disabled, set_button_pressed,
    set_checkbox_disabled, set_slider_disabled, set_text_field_disabled, toggle_checkbox,
};
use aurora_widgets::{GpuMesh, PathPipeline, WidgetId, WidgetTree, paint_widget};
use std::sync::{Mutex, MutexGuard};
use taffy::style_helpers::length;
use taffy::{FlexDirection, Rect as LayoutRect, Size, Style};

const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

/// `Button`'s own gallery cell size. Deliberately explicit, not the
/// button's own real padding-derived content size
/// (`widgets::button`'s own internal `style` function isn't public,
/// and this crate has no text layout wired in yet to give a label its
/// own real content size either) — a real, deterministic pixel size is
/// what a golden-image test needs; this gallery is testing
/// `paint_widget`'s own background-rectangle output, not button
/// content layout. Every other widget's own gallery below picks its
/// own cell size the same way, for the same reason.
const BUTTON_CELL: (u32, u32) = (64, 64);
/// Three states, side by side. `64 * 3 * 4 = 768 = 3 * 256`, already a
/// multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` — see
/// [`render_gallery`]'s own doc comment for why that matters and why
/// this harness doesn't (yet) handle a size that isn't. Every gallery
/// cell size below is chosen as a multiple of 64 for the same reason:
/// `64 * 4 = 256`, so *any* number of same-sized cells side by side
/// keeps the whole row aligned, not just this particular count of 3.
const BUTTON_GALLERY_SIZE: (u32, u32) = (BUTTON_CELL.0 * 3, BUTTON_CELL.1);

const CHECKBOX_CELL: (u32, u32) = (64, 64);
const CHECKBOX_GALLERY_SIZE: (u32, u32) = (CHECKBOX_CELL.0 * 3, CHECKBOX_CELL.1);

/// Wider than tall — a slider needs real horizontal travel for its own
/// thumb position to mean anything, unlike the other widgets' own
/// roughly-square cells.
const SLIDER_CELL: (u32, u32) = (128, 32);
const SLIDER_GALLERY_SIZE: (u32, u32) = (SLIDER_CELL.0 * 3, SLIDER_CELL.1);
/// How far into each slider cell, from its own left edge,
/// `render_gallery_produces_distinct_pixels_for_each_slider_state`
/// samples — well within the thumb's own 32px width when the thumb is
/// at that cell's own left edge (the minimum-value cell), but past it
/// once the thumb has moved away (see that test's own doc comment).
const SLIDER_THUMB_SAMPLE_OFFSET_X: u32 = 16;

const TEXT_FIELD_CELL: (u32, u32) = (192, 32);
const TEXT_FIELD_GALLERY_SIZE: (u32, u32) = (TEXT_FIELD_CELL.0 * 2, TEXT_FIELD_CELL.1);

/// `CommandPalette` has exactly one visual state today (`paint_widget`'s
/// own doc comment: no query text, no result-row highlighting, just the
/// outer panel) — one cell, not a row of them.
const COMMAND_PALETTE_CELL: (u32, u32) = (192, 128);
/// A real margin around the panel on every side, not just a bigger
/// backdrop colour — found necessary the hard way (2026-08-07):
/// `NEUTRAL_CLEAR` alone wasn't enough, because the panel was still
/// filling essentially the whole frame with no border to contrast
/// against at all, `scales.radius.md`'s own small rounded corners
/// being the only pixels that ever showed the backdrop. See
/// `command_palette_style`'s own doc comment for how this actually
/// gets applied.
const COMMAND_PALETTE_MARGIN: u32 = 32;
const COMMAND_PALETTE_GALLERY_SIZE: (u32, u32) = (
    COMMAND_PALETTE_CELL.0 + COMMAND_PALETTE_MARGIN * 2,
    COMMAND_PALETTE_CELL.1 + COMMAND_PALETTE_MARGIN * 2,
);

/// `render_gallery`'s own clear colour for `CommandPalette`/`TextField`
/// specifically, not the plain `wgpu::Color::BLACK` every other
/// gallery uses. Found by actually decoding the committed goldens'
/// own raw pixel bytes, not by eye: `surface.raised`
/// (`CommandPalette`'s own panel, `#28282c`) and `surface.sunken`
/// (`TextField`'s own background, `#141414`, `#0a0a0a` once
/// `disabled_opacity`-dimmed) are deliberately near-black by design
/// (FR-027's own "near-neutral" principle) — correct data, confirmed
/// pixel-for-pixel, but against a pure black backdrop there is
/// nothing for a human reviewing the golden to visually anchor
/// against, since neither widget ever resolves a bright `accent.*`
/// colour the way `Button`/`Checkbox`/`Slider` each do in at least one
/// of their own states. Those three keep the plain black backdrop —
/// already blessed, already real reference points within each image,
/// no reason to force a re-bless. **On its own, this alone turned out
/// not to be enough for `CommandPalette` specifically** — it has only
/// one visual state, so unlike `TextField` (two states, genuinely
/// contrastable against each other even with no margin at all) its own
/// gallery still filled essentially the whole frame with nothing to
/// contrast the new backdrop against; `command_palette_style`'s own
/// real margin was the second, necessary half of that particular fix.
const NEUTRAL_CLEAR: wgpu::Color = wgpu::Color {
    r: 0.5,
    g: 0.5,
    b: 0.5,
    a: 1.0,
};

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

fn sized_style(size: (u32, u32)) -> Style {
    Style {
        size: Size {
            width: length(size.0 as f32),
            height: length(size.1 as f32),
        },
        ..Default::default()
    }
}

/// A real, laid-out tree with one `Button` per state — `paint_widget`'s
/// own natural minimal fixture, side by side in `BUTTON_CELL`-sized
/// cells.
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
        if let Err(err) = tree.set_style(id, sized_style(BUTTON_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(BUTTON_GALLERY_SIZE.0 as f32, BUTTON_GALLERY_SIZE.1 as f32);
    (tree, [enabled, pressed, disabled])
}

/// A real, laid-out tree with one `Checkbox` per state, mirroring
/// exactly the three cases `src/paint.rs`'s own unit tests already
/// cover: unchecked (enabled), checked (enabled), and unchecked
/// disabled — not checked-and-disabled, so this exercises the
/// `surface.sunken` + `disabled_opacity` combination specifically,
/// distinct from the checked cell's own `accent.primary`.
fn checkbox_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let unchecked = match insert_checkbox(&mut tree, root, scales, "Unchecked") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let checked = match insert_checkbox(&mut tree, root, scales, "Checked") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = toggle_checkbox(&mut tree, checked) {
        unreachable!("{err:?}");
    }
    let disabled = match insert_checkbox(&mut tree, root, scales, "Disabled") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_checkbox_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [unchecked, checked, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(CHECKBOX_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        CHECKBOX_GALLERY_SIZE.0 as f32,
        CHECKBOX_GALLERY_SIZE.1 as f32,
    );
    (tree, [unchecked, checked, disabled])
}

/// A real, laid-out tree with one `Slider` per state: at its own
/// minimum value, at its own maximum value, and disabled (at a value
/// in between, so its own thumb sits somewhere a bare min/max
/// comparison wouldn't already cover).
fn slider_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 3]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let at_min = match insert_slider(&mut tree, root, scales, "Min", 0.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let at_max = match insert_slider(&mut tree, root, scales, "Max", 100.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled = match insert_slider(&mut tree, root, scales, "Disabled", 50.0, 0.0, 100.0) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_slider_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [at_min, at_max, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(SLIDER_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(SLIDER_GALLERY_SIZE.0 as f32, SLIDER_GALLERY_SIZE.1 as f32);
    (tree, [at_min, at_max, disabled])
}

/// A real, laid-out tree with one `TextField` per state: enabled, and
/// disabled.
fn text_field_gallery_tree(scales: &Scales) -> (WidgetTree<WidgetKind>, [WidgetId; 2]) {
    let (mut tree, root) = new_tree(Style {
        flex_direction: FlexDirection::Row,
        ..Default::default()
    });
    let enabled = match insert_text_field(&mut tree, root, scales, "name", "hello") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let disabled = match insert_text_field(&mut tree, root, scales, "name", "") {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = set_text_field_disabled(&mut tree, disabled, true) {
        unreachable!("{err:?}");
    }

    for id in [enabled, disabled] {
        if let Err(err) = tree.set_style(id, sized_style(TEXT_FIELD_CELL)) {
            unreachable!("{err:?}");
        }
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        TEXT_FIELD_GALLERY_SIZE.0 as f32,
        TEXT_FIELD_GALLERY_SIZE.1 as f32,
    );
    (tree, [enabled, disabled])
}

/// `sized_style(COMMAND_PALETTE_CELL)` plus a real `COMMAND_PALETTE_
/// MARGIN` on every side. Margin, not a bigger backdrop colour alone
/// (`NEUTRAL_CLEAR`): the root this gets applied to (`new_tree`'s own
/// default, `Style::default()`) shrink-wraps to its one child's own
/// margin box, the standard flexbox content-sizing behaviour a
/// single-child `Auto`-sized container already has — giving the child
/// real margin is what makes that shrink-wrapped box, and therefore
/// this gallery's own render target
/// (`COMMAND_PALETTE_GALLERY_SIZE` = `COMMAND_PALETTE_CELL` padded out
/// by `COMMAND_PALETTE_MARGIN` on every side), bigger than the panel
/// itself in the first place. Without it, a bigger `size` passed to
/// `render_gallery` alone would just leave extra untouched canvas the
/// tree's own layout never actually reaches — checked directly by
/// `command_palette_style_positions_the_panel_with_a_real_margin`
/// below, headlessly, no GPU needed to prove the *layout* half of this
/// is correct.
fn command_palette_style() -> Style {
    let margin = length(COMMAND_PALETTE_MARGIN as f32);
    Style {
        size: Size {
            width: length(COMMAND_PALETTE_CELL.0 as f32),
            height: length(COMMAND_PALETTE_CELL.1 as f32),
        },
        margin: LayoutRect {
            left: margin,
            right: margin,
            top: margin,
            bottom: margin,
        },
        ..Default::default()
    }
}

/// A real, laid-out tree with one `CommandPalette` — `paint_widget`
/// resolves exactly one visual state for it today (see this file's own
/// module doc comment), so there's only one cell, not a row. No
/// `&Scales` parameter, unlike every other `*_gallery_tree` function
/// here — `insert_command_palette` genuinely doesn't need one.
fn command_palette_gallery_tree() -> (WidgetTree<WidgetKind>, [WidgetId; 1]) {
    let (mut tree, root) = new_tree(Style::default());
    let commands = vec![CommandEntry::new("edit.undo", "Undo")];
    let palette = match insert_command_palette(&mut tree, root, commands) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    if let Err(err) = tree.set_style(palette, command_palette_style()) {
        unreachable!("{err:?}");
    }
    #[allow(clippy::cast_precision_loss)]
    tree.compute_layout(
        COMMAND_PALETTE_GALLERY_SIZE.0 as f32,
        COMMAND_PALETTE_GALLERY_SIZE.1 as f32,
    );
    (tree, [palette])
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

/// Reads the pixel at absolute image coordinates `(x, y)`.
fn sample_at(image: &aurora_testkit::Image, px: u32, py: u32) -> [u8; 4] {
    let offset = (py * image.width + px) as usize * 4;
    let Some(pixel) = image.rgba.get(offset..offset + 4) else {
        unreachable!("sample point is always within a real gallery image");
    };
    match pixel {
        &[r, g, b, a] => [r, g, b, a],
        _ => unreachable!("sliced exactly 4 bytes"),
    }
}

/// Reads the pixel at the centre of gallery cell `cell` (`cell_size`
/// wide/tall each, side by side left to right) — enough to distinguish
/// each state's own fill colour without needing a golden image at all.
fn sample_cell_centre(image: &aurora_testkit::Image, cell_size: (u32, u32), cell: u32) -> [u8; 4] {
    let cx = cell * cell_size.0 + cell_size.0 / 2;
    let cy = cell_size.1 / 2;
    sample_at(image, cx, cy)
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
        BUTTON_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, BUTTON_GALLERY_SIZE.0);
    assert_eq!(image.height, BUTTON_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, BUTTON_CELL, 0);
    let pressed_px = sample_cell_centre(&image, BUTTON_CELL, 1);
    let disabled_px = sample_cell_centre(&image, BUTTON_CELL, 2);
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
        BUTTON_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/button_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same "distinct pixels, no golden needed" proof as `Button`'s
/// own, for `Checkbox`: unchecked (`surface.sunken`) vs checked
/// (`accent.primary`) must render differently outright; unchecked vs
/// unchecked-disabled must render as the same token dimmed.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_checkbox_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, CHECKBOX_GALLERY_SIZE.0);
    assert_eq!(image.height, CHECKBOX_GALLERY_SIZE.1);

    let unchecked_px = sample_cell_centre(&image, CHECKBOX_CELL, 0);
    let checked_px = sample_cell_centre(&image, CHECKBOX_CELL, 1);
    let disabled_px = sample_cell_centre(&image, CHECKBOX_CELL, 2);
    assert_ne!(
        unchecked_px, checked_px,
        "surface.sunken vs accent.primary must render differently"
    );
    assert_ne!(
        unchecked_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended over the clear colour must render dimmer than full opacity"
    );
}

/// **Blessed and reviewed 2026-08-07**: Cahya ran the bless command on
/// real macOS GPU hardware and committed `tests/golden/
/// checkbox_gallery.png`. Confirmed by decoding its own raw pixel
/// bytes (not just eyeballing a thumbnail): unchecked `[20,20,20]`
/// (`surface.sunken`), checked `[120,172,255]` (`accent.primary`,
/// clearly visible), disabled `[8,8,8]` (`surface.sunken` dimmed) —
/// the checked cell's own real, bright colour gives a human reviewing
/// this image a genuine reference point the way `TextField`'s own
/// gallery doesn't (see `NEUTRAL_CLEAR`'s own doc comment), so plain
/// black stayed the right backdrop here.
#[test]
fn checkbox_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = checkbox_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        CHECKBOX_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/checkbox_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// `Slider`'s own "distinct pixels" proof is shaped differently from
/// the others: instead of comparing each cell's own centre (which
/// would just show the track, not the thumb, for anything but a
/// dead-centre value), this samples a fixed offset from each cell's
/// own left edge (`x = 16`, well within the thumb's own 32px width) —
/// at the minimum value the thumb sits right there (`accent.primary`);
/// at the maximum value the thumb has moved to the cell's own right
/// edge, so that same offset now shows bare track (`surface.sunken`)
/// instead. A real, direct proof the thumb's own position actually
/// moved, via rendered pixels rather than mesh vertices
/// (`src/paint.rs`'s own `a_sliders_thumb_moves_right_as_its_value_
/// increases` unit test already covers the geometry; this covers the
/// GPU pipeline actually drawing it).
#[test]
fn render_gallery_produces_distinct_pixels_for_each_slider_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    assert_eq!(image.width, SLIDER_GALLERY_SIZE.0);
    assert_eq!(image.height, SLIDER_GALLERY_SIZE.1);

    let near_left_edge = |cell: u32| {
        sample_at(
            &image,
            cell * SLIDER_CELL.0 + SLIDER_THUMB_SAMPLE_OFFSET_X,
            SLIDER_CELL.1 / 2,
        )
    };
    let at_min = near_left_edge(0);
    let at_max = near_left_edge(1);
    let disabled = near_left_edge(2);
    assert_ne!(
        at_min, at_max,
        "the thumb must be at a different x offset for a slider at its own minimum vs maximum value"
    );
    assert_ne!(
        at_max[..3],
        disabled[..3],
        "state.disabled_opacity must render the track dimmer than full opacity, at the same offset"
    );
}

/// **Blessed and reviewed 2026-08-07**: Cahya ran the bless command on
/// real macOS GPU hardware and committed `tests/golden/
/// slider_gallery.png`, clearly showing the thumb at the left edge
/// (minimum value), the right edge (maximum value), and a dimmed thumb
/// at its own middle position (disabled) — visually unambiguous, no
/// low-contrast concern the way `CommandPalette`/`TextField`'s own
/// galleries had (see `NEUTRAL_CLEAR`'s own doc comment).
#[test]
fn slider_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = slider_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        SLIDER_GALLERY_SIZE,
        wgpu::Color::BLACK,
    );
    let golden_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/slider_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// The same "distinct pixels, no golden needed" proof as the other
/// widgets', for `TextField`: enabled vs disabled must render as the
/// same `surface.sunken` token dimmed, the only state `paint_text_field`
/// resolves today.
#[test]
fn render_gallery_produces_distinct_pixels_for_each_text_field_state() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    assert_eq!(image.width, TEXT_FIELD_GALLERY_SIZE.0);
    assert_eq!(image.height, TEXT_FIELD_GALLERY_SIZE.1);

    let enabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 0);
    let disabled_px = sample_cell_centre(&image, TEXT_FIELD_CELL, 1);
    assert_ne!(
        enabled_px[..3],
        disabled_px[..3],
        "state.disabled_opacity blended toward the clear colour must render differently from full \
         opacity -- lighter here specifically, since NEUTRAL_CLEAR is lighter than surface.sunken, \
         unlike the other galleries' own black backdrop"
    );
}

/// **Blessed and reviewed 2026-08-07** (a second time — the first
/// committed golden here rendered against plain black and was
/// genuinely correct data but effectively unreviewable by eye, same
/// root cause as `CommandPalette`'s own, see `NEUTRAL_CLEAR`'s own doc
/// comment): Cahya re-ran the bless command against this
/// `NEUTRAL_CLEAR`-backed version and committed the result. Confirmed
/// by decoding its own raw pixel bytes directly: enabled `[20,20,20]`
/// (`surface.sunken`, unchanged by the backdrop since full opacity
/// never blends with anything), disabled `[85,85,85]` — genuinely,
/// clearly distinct from the enabled cell (a human can see two real
/// rectangles of different shades), even with no margin around either
/// one — unlike `CommandPalette`, `TextField` has two different states
/// to contrast *against each other*, so it never needed the margin fix
/// `CommandPalette`'s own single-state gallery did.
#[test]
fn text_field_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = text_field_gallery_tree(&scales);

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        TEXT_FIELD_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/text_field_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}

/// `CommandPalette` has exactly one visual state today (`paint_widget`'s
/// own doc comment), so there's no second state to compare against —
/// this instead compares the panel's own centre against its own
/// corner pixel `(0, 0)`. `scales.radius.md`'s real rounded corner
/// (`paint_command_palette`'s own doc comment) means the very corner
/// is *outside* the filled rounded rect regardless of what colour
/// filled it, so this is a real, self-contained proof the panel was
/// actually painted distinctly from its own surrounding clear colour
/// — without hardcoding what that clear colour's own exact byte value
/// is (see `NEUTRAL_CLEAR`'s own doc comment for why this gallery
/// doesn't use plain black).
#[test]
fn render_gallery_produces_the_command_palettes_own_panel() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    assert_eq!(image.width, COMMAND_PALETTE_GALLERY_SIZE.0);
    assert_eq!(image.height, COMMAND_PALETTE_GALLERY_SIZE.1);

    // Not `sample_cell_centre` -- that assumes cell 0 starts at x = 0,
    // but `command_palette_style`'s own margin pushes the panel's real
    // centre over by `COMMAND_PALETTE_MARGIN` on both axes.
    let panel_centre = sample_at(
        &image,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.0 / 2,
        COMMAND_PALETTE_MARGIN + COMMAND_PALETTE_CELL.1 / 2,
    );
    let corner = sample_at(&image, 0, 0);
    assert_ne!(
        corner, panel_centre,
        "the panel's own centre must show surface.raised, not the same clear colour its own \
         margin (and rounded-off corner) shows"
    );
}

/// A headless (no GPU needed) proof of the *layout* half of the fix
/// above: `command_palette_style`'s own margin actually reaches the
/// tree's own computed bounds, not just the render target's own,
/// larger `size`. Written specifically to build confidence *before*
/// asking for another real-hardware bless — this sandbox can check
/// `WidgetTree::bounds` (pure CPU layout math) but never the actual
/// rendered pixels (no GPU adapter at all), so this is the one piece
/// of the fix this sandbox can actually prove outright.
#[test]
fn command_palette_style_positions_the_panel_with_a_real_margin() {
    let (tree, [palette]) = command_palette_gallery_tree();
    let Some(bounds) = tree.bounds(palette) else {
        unreachable!("just laid out");
    };
    let margin = i64::from(COMMAND_PALETTE_MARGIN);
    assert_eq!(
        bounds.x, margin,
        "the panel must sit inset from the gallery's own left edge"
    );
    assert_eq!(
        bounds.y, margin,
        "the panel must sit inset from the gallery's own top edge"
    );
    assert_eq!(bounds.width, COMMAND_PALETTE_CELL.0);
    assert_eq!(bounds.height, COMMAND_PALETTE_CELL.1);
    assert_eq!(
        bounds.x + i64::from(bounds.width) + margin,
        i64::from(COMMAND_PALETTE_GALLERY_SIZE.0),
        "the panel plus its own margin on both sides must exactly fill the gallery's own width"
    );
}

/// **`#[ignore]`, deliberately, until a human blesses a real golden**
/// — see `button_gallery_matches_the_golden_image`'s own doc comment
/// for the general "never bless blind" reasoning. This one has been
/// blessed twice already and rejected both times, for two different,
/// real reasons, each found only *after* a human actually looked:
/// first a plain-black backdrop made a correct `surface.raised` panel
/// unreviewable by eye (fixed with `NEUTRAL_CLEAR`); then, even with
/// that fixed, the panel still filled essentially the whole frame with
/// no margin to contrast against at all (only `scales.radius.md`'s own
/// tiny rounded corners ever showed the backdrop) — Cahya reported it
/// "still looks dark" against the second attempt too. Fixed this time
/// with a real `COMMAND_PALETTE_MARGIN` on every side
/// (`command_palette_style`), and — since this sandbox can check
/// layout math headlessly even without a GPU —
/// `command_palette_style_positions_the_panel_with_a_real_margin`
/// already proves the panel's own computed bounds sit inset exactly as
/// intended, real confidence before asking for a third bless rather
/// than just hoping the fix is right this time. Run
/// `AURORA_BLESS_GOLDEN=1 cargo test -p aurora-widgets --test gallery
/// -- --ignored`, open the written
/// `tests/golden/command_palette_gallery.png`, confirm the panel now
/// shows a real, visible grey margin on every side (not just the
/// corners), then remove this attribute and commit the PNG alongside
/// that commit.
#[test]
#[ignore = "needs a human on real GPU hardware to bless and review tests/golden/command_palette_gallery.png first"]
fn command_palette_gallery_matches_the_golden_image() {
    let Some(context) = real_context() else {
        return;
    };
    let scales = scales();
    let theme = dark_theme();
    let (tree, _ids) = command_palette_gallery_tree();

    let image = render_gallery(
        &context,
        &tree,
        &theme,
        &scales,
        COMMAND_PALETTE_GALLERY_SIZE,
        NEUTRAL_CLEAR,
    );
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/command_palette_gallery.png");
    if let Err(err) = aurora_testkit::compare_to_golden(&golden_path, &image, 1) {
        unreachable!("{err}");
    }
}
