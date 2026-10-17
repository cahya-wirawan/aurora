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
        let _ = ok(layers.add_pixel_layer(&format!("Layer {i}"), bounds, None));
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
