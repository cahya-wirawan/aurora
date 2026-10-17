//! Per-frame costs of the live redraw outside compositing (0.265.0 perf
//! round): what [`crate::App::redraw`] spends on the widget layer every
//! frame, measured on the real startup workspace with a real GPU device.
//!
//! `App` needs a window, so this drives the same free functions `redraw`
//! calls, in the same order: `TextEngine::begin_frame`,
//! [`aurora_widgets::update_field_scrolls`] and
//! [`crate::collect_widget_paints`] (which tessellates every widget and
//! uploads each mesh). The numbers are printed, not asserted: the one
//! assertion is a deliberately loose CI-safety bound, like the canvas
//! frame benchmarks' (`recomposite_and_present_loop_*`), so a green run
//! is not a claim that any budget is met.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::time::Instant;

use aurora_widgets::GpuPaintOp;

use crate::tests::real_gpu_context;

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn stats(values: &mut [f64]) -> (f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    let n = values.len().max(1) as f64;
    let mean = values.iter().sum::<f64>() / n;
    let min = values.first().copied().unwrap_or(0.0);
    let max = values.last().copied().unwrap_or(0.0);
    (mean, min, max)
}

/// The startup workspace laid out at 1600 x 1000 logical px, with the
/// demo document plus `extra_layers` more pixel layers in the Layers
/// panel.
fn laid_out_workspace(
    extra_layers: usize,
    engine: &mut aurora_text::TextEngine,
    scales: &aurora_theme::Scales,
) -> aurora_ui::Workspace {
    let mut workspace = aurora_ui::build_workspace(scales);
    let (mut layers, _history) = crate::demo_document();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 1024,
        height: 1024,
    };
    for i in 0..extra_layers {
        let _ = ok(layers.add_pixel_layer(format!("Layer {i}"), bounds, None));
    }
    let _panels = crate::install_startup_panels(
        &mut workspace,
        scales,
        &layers,
        &crate::UndoOrder::new_document(),
        aurora_ui::Tool::default(),
        &crate::ToolSettings::default(),
    );
    crate::layout_workspace(&mut workspace, Some(engine), scales, 1.0, 1600.0, 1000.0);
    workspace
}

/// Measures the widget layer of one redraw: tessellation (`target_paint_ops`)
/// and the whole `collect_widget_paints` (tessellation + text resolve + one
/// GPU buffer pair per mesh), `frames` times on an unchanged tree.
fn measure_widget_paints(label: &str, extra_layers: usize, frames: usize) -> f64 {
    let Some(context) = real_gpu_context() else {
        return 0.0;
    };
    let (Ok(scales), Ok(theme)) = (crate::load_scales(), crate::load_theme()) else {
        unreachable!("the committed tokens load");
    };
    let Ok(mut engine) = aurora_text::TextEngine::new() else {
        unreachable!("the bundled font loads");
    };
    let mut atlas = crate::GlyphAtlas::new(context.device());
    let mut workspace = laid_out_workspace(extra_layers, &mut engine, &scales);
    let format = wgpu::TextureFormat::Bgra8UnormSrgb;

    let mut tessellate = Vec::with_capacity(frames);
    let mut whole = Vec::with_capacity(frames);
    let mut ops_count = 0;
    let mut solids = 0;
    for _ in 0..frames {
        let t0 = Instant::now();
        let ops =
            crate::target_paint_ops(&workspace.tree, None, None, &theme, &scales, format, 1.0);
        tessellate.push(t0.elapsed().as_secs_f64() * 1000.0);
        drop(ops);

        let t1 = Instant::now();
        engine.begin_frame();
        aurora_widgets::update_field_scrolls(
            &mut engine,
            &mut workspace.tree,
            &theme,
            &scales,
            1.0,
        );
        let paints = crate::collect_widget_paints(
            &workspace.tree,
            None,
            None,
            &theme,
            &scales,
            &context,
            format,
            1.0,
            Some((&mut engine, &mut atlas)),
        );
        whole.push(t1.elapsed().as_secs_f64() * 1000.0);
        ops_count = paints.len();
        solids = paints
            .iter()
            .filter(|op| matches!(op, GpuPaintOp::Solid(..)))
            .count();
        drop(paints);
        // Let the device retire the frame's buffers, as presenting would.
        let _ = context.device().poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
    // 60 FPS round 3: the same frames through the widget paint cache, on
    // the unchanged tree (after one cold frame) and with one button's hover
    // toggled every frame.
    let mut cache = crate::widget_paint_cache::WidgetPaintCache::default();
    let buttons: Vec<_> = workspace
        .tree
        .paint_order()
        .into_iter()
        .filter(|&id| {
            matches!(
                workspace.tree.payload(id),
                Some(aurora_widgets::widgets::WidgetKind::Button(_))
            )
        })
        .collect();
    let mut cached = Vec::with_capacity(frames);
    let mut hovered = Vec::with_capacity(frames);
    let mut cached_ops = 0;
    for frame in 0..=2 * frames {
        let hover = frame > frames;
        if hover && let Some(&b) = buttons.get(frame % buttons.len().max(1)) {
            let _ =
                aurora_widgets::widgets::set_button_hovered(&mut workspace.tree, b, frame % 2 == 0);
        }
        let t2 = Instant::now();
        engine.begin_frame();
        aurora_widgets::update_field_scrolls(
            &mut engine,
            &mut workspace.tree,
            &theme,
            &scales,
            1.0,
        );
        let paints = cache.frame_gpu_ops(
            &workspace.tree,
            None,
            None,
            &theme,
            &scales,
            &context,
            format,
            1.0,
            Some((&mut engine, &mut atlas)),
        );
        let ms = t2.elapsed().as_secs_f64() * 1000.0;
        if hover {
            hovered.push(ms);
        } else if frame > 0 {
            cached.push(ms);
            cached_ops = paints.len();
        }
        drop(paints);
        let _ = context.device().poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }
    assert_eq!(cached_ops, ops_count, "the cached frame draws as many ops");
    let (c_mean, c_min, c_max) = stats(&mut cached);
    let (h_mean, h_min, h_max) = stats(&mut hovered);
    println!(
        "widget paints ({label}) cached: unchanged mean={c_mean:.2}ms [{c_min:.2}..{c_max:.2}] \
         one hover change/frame mean={h_mean:.2}ms [{h_min:.2}..{h_max:.2}]"
    );
    let (t_mean, t_min, t_max) = stats(&mut tessellate);
    let (w_mean, w_min, w_max) = stats(&mut whole);
    println!(
        "widget paints ({label}): n={frames} widgets={} gpu_ops={ops_count} (solid {solids}) \
         tessellate mean={t_mean:.2}ms [{t_min:.2}..{t_max:.2}] collect_widget_paints \
         mean={w_mean:.2}ms [{w_min:.2}..{w_max:.2}] (per frame, unchanged tree)",
        workspace.tree.paint_order().len(),
    );
    w_mean
}

#[test]
fn widget_paint_cost_per_frame_is_measured_on_the_startup_workspace() {
    // CI-safety bound only (software adapters and shared runners are far
    // slower); it exists to produce a number, not to pass a budget.
    const BUDGET_MS: f64 = 2000.0;
    let demo = measure_widget_paints("demo document", 0, 40);
    let many = measure_widget_paints("demo document + 60 layers", 60, 40);
    assert!(
        demo < BUDGET_MS && many < BUDGET_MS,
        "widget paints took {demo:.2} / {many:.2} ms a frame"
    );
}

/// The per-sample loop `begin_gpu_composite_tile` used before 0.265.0 to
/// turn a source tile into upload bytes: the reference the cast is
/// checked against.
fn reference_le_bytes(samples: &[half::f16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Every bit pattern an `f16` can hold (NaN payloads, signalling NaNs,
/// both zeros, subnormals, infinities), repeated to a whole tile.
fn every_bit_pattern_tile() -> Vec<half::f16> {
    (0..aurora_tile::SAMPLES)
        .map(|i| half::f16::from_bits((i % 65_536) as u16))
        .collect()
}

#[test]
fn f16_le_bytes_matches_the_per_sample_loop() {
    let texels = every_bit_pattern_tile();
    let bytes = crate::f16_le_bytes(&texels);
    assert_eq!(bytes.as_ref(), reference_le_bytes(&texels).as_slice());
    assert!(crate::f16_le_bytes(&[]).is_empty());
}

#[test]
fn f16_samples_from_le_bytes_inverts_the_bytes_bit_for_bit() {
    let texels = every_bit_pattern_tile();
    let mut bytes = reference_le_bytes(&texels);
    let back = crate::f16_samples_from_le_bytes(&bytes);
    assert_eq!(back.len(), texels.len());
    assert!(
        back.iter()
            .zip(&texels)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    // A trailing odd byte is ignored, as the old loop's `chunks_exact` did.
    bytes.push(0xAB);
    assert_eq!(crate::f16_samples_from_le_bytes(&bytes).len(), texels.len());
    // An unaligned start (a mapped range need not be 2-aligned) decodes.
    let shifted = bytes.get(1..).unwrap_or_default();
    let odd = crate::f16_samples_from_le_bytes(shifted);
    assert_eq!(odd.len(), texels.len());
    assert_eq!(
        odd.first().map(|s| s.to_bits()),
        Some(u16::from_le_bytes([0x00, 0x01]))
    );
    assert_eq!(
        crate::decode_f16_samples(&reference_le_bytes(&texels)).map(|v| v.len()),
        Some(aurora_tile::SAMPLES)
    );
    assert!(crate::decode_f16_samples(&[0, 0, 0]).is_none());
}

/// The per-sample byte conversions every GPU-composited tile runs (the
/// source upload in `begin_gpu_composite_tile` and the readback decode in
/// `decode_f16_samples`), timed on one tile in this crate's own profile:
/// the pre-0.265.0 loops against the casts that replaced them.
#[test]
fn gpu_tile_byte_conversions_are_measured() {
    let texels: Vec<half::f16> = (0..aurora_tile::SAMPLES)
        .map(|i| half::f16::from_f32((i % 1024) as f32 / 1024.0))
        .collect();
    let runs = 20;
    let (mut old_encode, mut old_decode, mut new_encode, mut new_decode) = (
        Vec::with_capacity(runs),
        Vec::with_capacity(runs),
        Vec::with_capacity(runs),
        Vec::with_capacity(runs),
    );
    for _ in 0..runs {
        let t0 = Instant::now();
        let bytes = reference_le_bytes(&texels);
        old_encode.push(t0.elapsed().as_secs_f64() * 1000.0);
        let t1 = Instant::now();
        let mut out = Vec::with_capacity(aurora_tile::SAMPLES);
        for pair in bytes.chunks_exact(2) {
            if let Ok(pair) = <[u8; 2]>::try_from(pair) {
                out.push(half::f16::from_le_bytes(pair));
            }
        }
        old_decode.push(t1.elapsed().as_secs_f64() * 1000.0);
        let t2 = Instant::now();
        let cast = crate::f16_le_bytes(&texels);
        let cast_len = cast.len();
        new_encode.push(t2.elapsed().as_secs_f64() * 1000.0);
        let t3 = Instant::now();
        let back = crate::f16_samples_from_le_bytes(&bytes);
        new_decode.push(t3.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(cast_len, bytes.len());
        assert_eq!(back.len(), out.len());
    }
    let (oe, _, _) = stats(&mut old_encode);
    let (od, _, _) = stats(&mut old_decode);
    let (ne, _, _) = stats(&mut new_encode);
    let (nd, _, _) = stats(&mut new_decode);
    println!(
        "gpu tile byte conversions (one 256x256 RGBA f16 tile, n={runs}): encode loop \
         {oe:.3}ms -> cast {ne:.3}ms; decode loop {od:.3}ms -> copy {nd:.3}ms"
    );
}

/// What one transparent seed tile costs (`aurora_render::transparent_tile`,
/// allocated for every recomposited tile, empty ones included).
#[test]
fn transparent_tile_seed_is_measured() {
    let runs = 20;
    let mut seed = Vec::with_capacity(runs);
    let mut zeroed = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t0 = Instant::now();
        let tile = aurora_render::transparent_tile();
        seed.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert!(tile.iter().all(|s| s.to_bits() == 0));
        let t1 = Instant::now();
        let raw = vec![0u16; aurora_tile::SAMPLES];
        zeroed.push(t1.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(raw.len(), tile.len());
    }
    let (s, _, _) = stats(&mut seed);
    let (z, _, _) = stats(&mut zeroed);
    println!(
        "transparent tile seed: n={runs} transparent_tile {s:.3}ms vs zeroed u16 alloc {z:.3}ms"
    );
}

/// A dab on a layer whose origin is inside the document (so the composite
/// grid's anchor, the document corner, is not the layer origin) must make the
/// composite tiles under the painted area stale, and only those. Before
/// the 0.265.0 fix it invalidated composite tile (0, 0) (the painted
/// layer-local id) and left (1, 0), (2, 0), (1, 1), (2, 1) current.
#[test]
fn a_dab_on_a_layer_inside_the_document_invalidates_the_tiles_it_lands_on() {
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 300,
        y: 150,
        width: 512,
        height: 512,
    };
    let id = ok(layers.add_pixel_layer("offset", bounds, None));
    let reference = crate::composite_reference_origin(&layers, Some(id));
    assert_eq!(reference, (0, 0));
    let mut cache = crate::CompositeCache::default();
    for x in 0..4 {
        for y in 0..4 {
            cache.mark_current(aurora_tile::TileId { x, y });
        }
    }
    // Layer-local tile (0, 0) covers document (300..556, 150..406).
    let local = aurora_tile::TileId { x: 0, y: 0 };
    crate::invalidate_painted_tiles(&mut cache, &layers, id, &[local], reference);
    let stale: Vec<(u32, u32)> = (0..4)
        .flat_map(|x| (0..4).map(move |y| (x, y)))
        .filter(|&(x, y)| !cache.is_current(aurora_tile::TileId { x, y }))
        .collect();
    assert_eq!(stale, vec![(1, 0), (1, 1), (2, 0), (2, 1)]);
}

/// A layer whose origin is the anchor keeps the exact id mapping.
#[test]
fn a_dab_on_a_layer_at_the_anchor_invalidates_exactly_its_painted_tile() {
    let mut layers = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 1024,
        height: 1024,
    };
    let id = ok(layers.add_pixel_layer("anchor", bounds, None));
    let reference = crate::composite_reference_origin(&layers, Some(id));
    // 60 FPS round 3: the anchor is the document corner, so a layer there
    // is the one whose painted ids are composite ids.
    assert_eq!(reference, (0, 0));
    let mut cache = crate::CompositeCache::default();
    for x in 0..4 {
        for y in 0..4 {
            cache.mark_current(aurora_tile::TileId { x, y });
        }
    }
    let painted = aurora_tile::TileId { x: 2, y: 1 };
    crate::invalidate_painted_tiles(&mut cache, &layers, id, &[painted], reference);
    let stale: Vec<(u32, u32)> = (0..4)
        .flat_map(|x| (0..4).map(move |y| (x, y)))
        .filter(|&(x, y)| !cache.is_current(aurora_tile::TileId { x, y }))
        .collect();
    assert_eq!(stale, vec![(2, 1)]);
}
