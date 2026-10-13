//! A headless render of the Layers panel for a human to look at (0.195.0):
//! the production workspace layout and paint ops on a real GPU, cropped to
//! the panel. `#[ignore]`d: it asserts only that something was drawn.
//! The PNGs (`layers-panel.png` at scale 2, `layers-panel-1x.png`) land in
//! `$AURORA_REVIEW_PNG_DIR`, else the system temp directory; the test prints
//! the paths.
//! Run: `AURORA_REQUIRE_GPU=1 cargo test -p aurora-ui --lib
//! write_layers_panel_png -- --ignored --nocapture`.

#![cfg(test)]

use aurora_core::Rect;
use aurora_doc::{BlendMode, LayerTree};
use aurora_text::TextEngine;
use aurora_theme::{Palette, Scales, Theme, ThemeSet};
use aurora_widgets::widgets::{WidgetKind, set_tree_item_expanded};
use aurora_widgets::{
    GlyphAtlas, GradientPipeline, PathPipeline, TextPipeline, draw_paint_ops,
    paint_widget_ops_frame, upload_paint_ops,
};

/// Where the review PNGs land: `$AURORA_REVIEW_PNG_DIR` if set, else the
/// system temp directory (created if needed).
fn review_png(name: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("AURORA_REVIEW_PNG_DIR")
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn scales() -> Scales {
    match Scales::from_toml_str(include_str!("../../../design/tokens/scales.toml")) {
        Ok(scales) => scales,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn dark_theme() -> Theme {
    let Ok(palette) = Palette::from_toml_str(include_str!("../../../design/tokens/palette.toml"))
    else {
        unreachable!()
    };
    let mut themes = ThemeSet::new();
    if themes
        .register(include_str!("../../../design/themes/dark.toml"))
        .is_err()
    {
        unreachable!()
    }
    match themes.resolve("Dark", &palette) {
        Ok(theme) => theme,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn bounds() -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    }
}

fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// Renders the whole workspace at `scale` and returns the RGBA bytes and
/// the physical size.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn render_workspace(
    context: &aurora_gpu::GpuContext,
    ws: &crate::Workspace,
    scale: f32,
    logical: (f32, f32),
) -> (Vec<u8>, u32, u32) {
    let (theme, scales) = (dark_theme(), scales());
    let (width, height) = ((logical.0 * scale) as u32, (logical.1 * scale) as u32);
    let mut ops = Vec::new();
    for id in ws.tree.paint_order() {
        ops.extend(ok(paint_widget_ops_frame(
            &ws.tree, id, None, None, &theme, &scales, scale,
        )));
    }
    let device = context.device();
    let queue = context.queue();
    let mut engine = ok(TextEngine::new());
    let mut atlas = GlyphAtlas::new(device);
    let gpu = upload_paint_ops(device, queue, &mut engine, &mut atlas, ops, scale, |c| c);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("layers-panel-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut path = PathPipeline::new(device);
    let mut gradient = GradientPipeline::new(device);
    let mut text = TextPipeline::new(device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("layers-panel"),
    });
    {
        let [r, g, b] = theme.surface.app.to_srgb_f32();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("layers-panel"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(r),
                        g: f64::from(g),
                        b: f64::from(b),
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
        draw_paint_ops(
            &mut pass,
            &mut path,
            &mut gradient,
            &mut text,
            &atlas,
            device,
            queue,
            format,
            logical,
            &gpu,
        );
    }
    let bytes_per_row = width * 4;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("layers-panel-readback"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    let Ok(Ok(())) = rx.recv() else {
        unreachable!("map_async completes once the device is idle");
    };
    let Ok(data) = slice.get_mapped_range() else {
        unreachable!("just mapped");
    };
    let bytes = data.to_vec();
    drop(data);
    readback.unmap();
    (bytes, width, height)
}

/// Lays out `ws`, renders it at `scale` and writes the Layers panel's crop
/// to `name`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn write_panel_png(
    context: &aurora_gpu::GpuContext,
    ws: &mut crate::Workspace,
    scale: f32,
    name: &std::path::Path,
) {
    let target = ws.layers.root;
    write_crop_png(context, ws, scale, name, target);
}

/// Lays out `ws`, renders it at `scale` and writes `target`'s crop to
/// `name`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn write_crop_png(
    context: &aurora_gpu::GpuContext,
    ws: &mut crate::Workspace,
    scale: f32,
    name: &std::path::Path,
    target: aurora_widgets::WidgetId,
) {
    write_crop_png_with(context, ws, scale, name, target, false);
}

/// [`write_crop_png`] with the text-aware layout the app uses
/// (`compute_text_layout`: buttons and checkboxes sized to their labels),
/// when `measured`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn write_crop_png_with(
    context: &aurora_gpu::GpuContext,
    ws: &mut crate::Workspace,
    scale: f32,
    name: &std::path::Path,
    target: aurora_widgets::WidgetId,
    measured: bool,
) {
    let logical = (1280.0_f32, 800.0_f32);
    if measured {
        let scales = scales();
        let mut engine = ok(TextEngine::new());
        aurora_widgets::compute_text_layout(
            &mut ws.tree,
            logical.0,
            logical.1,
            Some(aurora_widgets::TextMeasure {
                engine: &mut engine,
                scales: &scales,
                scale_factor: scale,
            }),
        );
    } else {
        ws.tree.compute_layout(logical.0, logical.1);
    }
    let (bytes, width, _height) = render_workspace(context, ws, scale, logical);
    let Some(panel) = ws.tree.bounds(target) else {
        unreachable!("laid out");
    };
    let (x0, y0) = (
        (panel.x as f32 * scale) as u32,
        (panel.y as f32 * scale) as u32,
    );
    let (w, h) = (
        (panel.width as f32 * scale) as u32,
        (panel.height as f32 * scale) as u32,
    );
    let mut crop = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y0 + h {
        let from = ((y * width + x0) * 4) as usize;
        if let Some(row) = bytes.get(from..from + (w * 4) as usize) {
            crop.extend_from_slice(row);
        }
    }
    assert!(crop.iter().any(|&b| b != 0), "something was drawn");
    let file = ok(std::fs::File::create(name));
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = ok(encoder.write_header());
    ok(writer.write_image_data(&crop));
    println!("wrote {}", name.display());
}

#[test]
#[ignore = "writes a PNG for human review"]
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn write_layers_panel_png() {
    let Some(context) = aurora_gpu::test_support::real_context_or_skip() else {
        return;
    };
    let scales = scales();
    // The sample document: an expanded group holding an expanded group, a
    // hidden layer and a Multiply layer; a collapsed group; a Background.
    let mut layers = LayerTree::new();
    let background = ok(layers.add_pixel_layer("Background", bounds(), None));
    // A layer clipped to the Background (0.201.1): the Clip glyph in its
    // indent step.
    let highlights = ok(layers.add_pixel_layer("Highlights", bounds(), None));
    ok(layers.set_clipped(highlights, true));
    let post2 = ok(layers.add_group("Post 2", None));
    let logo = ok(layers.add_pixel_layer("Logo", bounds(), Some(post2)));
    ok(layers.set_visible(logo, false));
    let vso = ok(layers.add_pixel_layer("Vector Smart Object", bounds(), Some(post2)));
    ok(layers.set_blend_mode(vso, BlendMode::Multiply));
    let effect = ok(layers.add_group("Effect", Some(post2)));
    ok(layers.set_blend_mode(effect, BlendMode::Normal));
    let glow = ok(layers.add_pixel_layer("Glow", bounds(), Some(effect)));
    let post1 = ok(layers.add_group("Post 1", None));
    let child = ok(layers.add_pixel_layer("Title", bounds(), Some(post1)));
    ok(layers.set_blend_mode(post1, BlendMode::SoftLight));
    ok(layers.add_group("Post 3", None).map(|_| ()));
    ok(layers
        .add_adjustment_layer_at(
            "Curves 1",
            aurora_doc::Adjustment::Curves(aurora_core::CurvesParams::default()),
            None,
            0,
        )
        .map(|_| ()));

    let mut ws = crate::build_workspace(&scales);
    // Locks (0.198.0): the Background the way Photoshop locks it
    // (transparency + position), "Logo" locked all, and the active layer
    // with its position locked so the Lock row shows a mixed state.
    ok(layers.set_lock(
        background,
        aurora_doc::LayerLock {
            transparency: true,
            pixels: false,
            position: true,
        },
    ));
    ok(layers.set_lock(logo, aurora_doc::LayerLock::all()));
    ok(layers.set_lock(
        vso,
        aurora_doc::LayerLock {
            transparency: false,
            pixels: false,
            position: true,
        },
    ));
    let rows = ok(crate::populate_layers_panel(
        &mut ws.tree,
        ws.layers,
        &scales,
        &layers,
    ));
    let row_of = |layer| {
        rows.iter()
            .find(|&(_, &id)| id == layer)
            .map(|(&row, _)| row)
    };
    // Synthetic pictures for the thumbnails (the app's worker makes the
    // real ones): an opaque gradient, a translucent disc, a wide strip and
    // a half-transparent tile, so the checkerboard shows.
    let picture = |x: u32, y: u32, w: u32, h: u32, f: &dyn Fn(u32, u32) -> [f32; 4]| {
        let mut texels = Vec::new();
        for j in 0..h {
            for i in 0..w {
                texels.push(f(i, j));
            }
        }
        std::sync::Arc::new(
            aurora_widgets::widgets::ThumbnailImage::new(x, y, w, h, texels)
                .unwrap_or_else(|| unreachable!("fits the grid")),
        )
    };
    let gradient = picture(0, 0, 48, 48, &|i, j| {
        let (r, g) = (i as f32 / 47.0, j as f32 / 47.0);
        [r, g, 0.4, 1.0]
    });
    let disc = picture(0, 0, 48, 48, &|i, j| {
        let (dx, dy) = (i as f32 - 23.5, j as f32 - 23.5);
        if dx * dx + dy * dy < 400.0 {
            [0.8, 0.2, 0.2, 1.0].map(|v| v)
        } else {
            [0.0; 4]
        }
    });
    let strip = picture(0, 14, 48, 20, &|i, _| {
        let a = if i < 24 { 1.0 } else { 0.5 };
        [0.1 * a, 0.5 * a, 0.9 * a, a]
    });
    let half = picture(8, 0, 32, 48, &|_, j| {
        if j < 24 {
            [0.9, 0.9, 0.2, 1.0]
        } else {
            [0.0; 4]
        }
    });
    for (layer, image) in [
        (background, &gradient),
        (logo, &disc),
        (vso, &half),
        (glow, &strip),
        (child, &gradient),
    ] {
        if let Some(row) = row_of(layer) {
            ok(crate::set_layer_row_thumbnail(
                &mut ws.tree,
                row,
                Some(std::sync::Arc::clone(image)),
            ));
        }
    }
    // Collapse "Post 1" the way the app does (rows removed, icons synced).
    let Some(post1_row) = row_of(post1) else {
        unreachable!("a row per layer");
    };
    ok(set_tree_item_expanded(&mut ws.tree, post1_row, false));
    ok(crate::sync_layer_row(
        &mut ws.tree,
        post1_row,
        &layers,
        post1,
    ));
    // The active row, and the controls showing it.
    let Some(vso_row) = row_of(vso) else {
        unreachable!("a row per layer");
    };
    ok(crate::set_layer_row_selected(&mut ws.tree, vso_row, true));
    let controls = ok(crate::insert_layer_controls(
        &mut ws.tree,
        ws.layers,
        &scales,
    ));
    ok(crate::sync_layer_controls(
        &mut ws.tree,
        controls,
        &layers,
        Some(vso),
        None,
    ));
    // The toolbar: Delete enabled (a pixel layer survives), the Group
    // button hovered to show the tint.
    ok(crate::sync_layer_toolbar(
        &mut ws.tree,
        &ws.layer_toolbar,
        true,
    ));
    let group_button = ws.layer_toolbar.button_for(crate::LayerAction::Group);
    ok(crate::set_layer_toolbar_hover(
        &mut ws.tree,
        &ws.layer_toolbar,
        group_button,
    ));
    // Scale 2 is the user's Retina case; scale 1 is written beside it.
    for (scale, name) in [
        (2.0_f32, review_png("layers-panel.png")),
        (1.0, review_png("layers-panel-1x.png")),
    ] {
        write_panel_png(&context, &mut ws, scale, &name);
    }
    // The filter (0.199.0): "lo" with the pixel-layer kind not toggled; the
    // active layer ("Vector Smart Object") is hidden by it, so no row is
    // highlighted while the controls still show it.
    ws.layer_view.filter = crate::LayerFilter {
        text: "lo".to_owned(),
        ..crate::LayerFilter::default()
    };
    let shown_rows = ok(crate::populate_layers_panel_view(
        &mut ws.tree,
        ws.layers,
        &scales,
        &layers,
        &mut ws.layer_view,
    ));
    ok(crate::write_layer_filter(
        &mut ws.tree,
        &controls.filter,
        &ws.layer_view.filter,
    ));
    let (shown, total) = ws.layer_view.counts();
    ok(crate::set_layer_filter_summary(
        &mut ws.tree,
        &controls.filter,
        shown,
        total,
        true,
    ));
    assert_eq!(shown_rows.len(), shown);
    write_panel_png(
        &context,
        &mut ws,
        2.0,
        &review_png("layers-panel-filtered.png"),
    );
    let _: Option<&WidgetKind> = None;
}

/// The tools panel (0.207.0) for a human to look at: Brush selected, the
/// Zoom button hovered, at scale 2 and scale 1, then the scale-2 picture
/// again enlarged 3x with nearest-neighbour so the pixels show.
/// Run: `AURORA_REQUIRE_GPU=1 cargo test -p aurora-ui --lib
/// write_tools_panel_png -- --ignored --nocapture`.
#[test]
#[ignore = "writes a PNG for human review"]
fn write_tools_panel_png() {
    let Some(context) = aurora_gpu::test_support::real_context_or_skip() else {
        return;
    };
    let scales = scales();
    let mut ws = crate::build_workspace(&scales);
    ok(crate::sync_tools_panel(
        &mut ws.tree,
        &ws.tools,
        crate::Tool::Brush,
    ));
    let zoom = ws.tools.button_for(crate::Tool::Zoom);
    ok(crate::set_tools_panel_hover(&mut ws.tree, &ws.tools, zoom));
    // 0.208.0: a foreground and a background colour, the swap button
    // hovered.
    ok(crate::sync_colour_controls(
        &mut ws.tree,
        &ws.tools.colours,
        [0.85, 0.25, 0.1],
        [0.1, 0.4, 0.75],
    ));
    let swap = ws.tools.colours.swap;
    ok(aurora_widgets::widgets::set_button_hovered(
        &mut ws.tree,
        swap,
        true,
    ));
    let root = ws.tools.root;
    for (scale, name) in [
        (2.0_f32, review_png("tools-panel.png")),
        (1.0, review_png("tools-panel-1x.png")),
    ] {
        write_crop_png(&context, &mut ws, scale, &name, root);
    }
    // ...and with the foreground's picker open, the whole window at 1x.
    let colours = ws.tools.colours;
    ok(crate::open_colour_popover(
        &mut ws.tree,
        &colours,
        &scales,
        crate::ColourTarget::Foreground,
        [0.85, 0.25, 0.1],
    ));
    let window = ws.root;
    write_crop_png(
        &context,
        &mut ws,
        1.0,
        &review_png("tools-panel-colour-picker.png"),
        window,
    );
}

/// The two fill tools (0.209.0) for a human to look at: the Tools panel
/// with the tool selected (scale 2) and the options bar's group for it
/// (scale 2), one pair of pictures per tool.
/// Run: `AURORA_REQUIRE_GPU=1 AURORA_REVIEW_PNG_DIR=<dir> cargo test -p
/// aurora-ui --lib write_fill_tools_png -- --ignored --nocapture`.
#[test]
#[ignore = "writes a PNG for human review"]
fn write_fill_tools_png() {
    let Some(context) = aurora_gpu::test_support::real_context_or_skip() else {
        return;
    };
    let scales = scales();
    let mut ws = crate::build_workspace(&scales);
    let controls = ok(crate::insert_tool_controls(
        &mut ws.tree,
        ws.options_bar,
        ws.properties,
        &scales,
    ));
    let view = crate::FillView {
        radial: true,
        reverse: false,
        opacity: 80,
        tolerance: 32,
        contiguous: true,
        strength: 50,
        blur_strength: 50,
        sharpen_strength: 50,
        dodge_exposure: 50,
        burn_exposure: 50,
        sponge_flow: 50,
    };
    for (tool, name) in [
        (crate::Tool::Gradient, "gradient"),
        (crate::Tool::PaintBucket, "paint-bucket"),
    ] {
        ok(crate::sync_tools_panel(&mut ws.tree, &ws.tools, tool));
        ok(crate::sync_tool_controls(
            &mut ws.tree,
            controls,
            tool,
            None,
            None,
        ));
        ok(crate::sync_fill_controls(
            &mut ws.tree,
            &controls,
            tool,
            view,
            None,
        ));
        let (tools_root, bar) = (ws.tools.root, ws.options_bar);
        write_crop_png(
            &context,
            &mut ws,
            2.0,
            &review_png(&format!("fill-tools-panel-{name}.png")),
            tools_root,
        );
        write_crop_png_with(
            &context,
            &mut ws,
            2.0,
            &review_png(&format!("fill-tools-options-{name}.png")),
            bar,
            true,
        );
    }
}
