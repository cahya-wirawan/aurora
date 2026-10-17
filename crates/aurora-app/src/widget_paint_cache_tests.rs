//! The widget paint cache (60 FPS round 3) against a fresh paint: after
//! every UI change in a long sequence (hover, scroll, focus and caret,
//! resize, text edits, a dialog, the command palette, a scale-factor
//! change, a theme switch, layers added and removed) the cached frame's
//! ops equal [`crate::target_paint_ops`] op for op, mesh for mesh, and an
//! unchanged frame is all hits.

use aurora_widgets::widgets::WidgetKind;
use aurora_widgets::{PaintOp, WidgetId, WidgetTree};

use crate::widget_paint_cache::WidgetPaintCache;

const LIGHT_THEME_TOML: &str = include_str!("../../../design/themes/light.toml");

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => unreachable!("{e:?}"),
    }
}

fn light_theme() -> aurora_theme::Theme {
    let palette = ok(aurora_theme::Palette::from_toml_str(crate::PALETTE_TOML));
    let mut themes = aurora_theme::ThemeSet::new();
    ok(themes.register(crate::DARK_THEME_TOML));
    ok(themes.register(LIGHT_THEME_TOML));
    ok(themes.resolve("Light", &palette))
}

struct Scene {
    workspace: aurora_ui::Workspace,
    engine: aurora_text::TextEngine,
    scales: aurora_theme::Scales,
    theme: aurora_theme::Theme,
    scale_factor: f64,
    focused: Option<WidgetId>,
    cache: WidgetPaintCache,
    format: wgpu::TextureFormat,
    frames: usize,
}

impl Scene {
    fn new() -> Self {
        let (Ok(scales), Ok(theme)) = (crate::load_scales(), crate::load_theme()) else {
            unreachable!("the committed tokens load");
        };
        let Ok(mut engine) = aurora_text::TextEngine::new() else {
            unreachable!("the bundled font loads");
        };
        let mut workspace = aurora_ui::build_workspace(&scales);
        let (mut layers, _history) = crate::demo_document();
        // Enough layers that the Layers panel scrolls.
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        for i in 0..40 {
            let _ = ok(layers.add_pixel_layer(format!("Layer {i}"), bounds, None));
        }
        let _panels = crate::install_startup_panels(
            &mut workspace,
            &scales,
            &layers,
            &crate::UndoOrder::new_document(),
            aurora_ui::Tool::default(),
            &crate::ToolSettings::default(),
        );
        crate::layout_workspace(
            &mut workspace,
            Some(&mut engine),
            &scales,
            1.0,
            1600.0,
            1000.0,
        );
        Self {
            workspace,
            engine,
            scales,
            theme,
            scale_factor: 1.0,
            focused: None,
            cache: WidgetPaintCache::default(),
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            frames: 0,
        }
    }

    fn tree(&mut self) -> &mut WidgetTree<WidgetKind> {
        &mut self.workspace.tree
    }

    fn relayout(&mut self, w: f32, h: f32) {
        crate::layout_workspace(
            &mut self.workspace,
            Some(&mut self.engine),
            &self.scales,
            self.scale_factor,
            w,
            h,
        );
    }

    /// One frame through the cache, checked against a fresh paint.
    #[track_caller]
    fn frame(&mut self, what: &str) -> crate::widget_paint_cache::PaintCacheStats {
        self.frames += 1;
        let fresh: Vec<PaintOp> = crate::target_paint_ops(
            &self.workspace.tree,
            None,
            self.focused,
            &self.theme,
            &self.scales,
            self.format,
            self.scale_factor,
        );
        let cached = self.cache.frame_paint_ops(
            &self.workspace.tree,
            None,
            self.focused,
            &self.theme,
            &self.scales,
            self.format,
            self.scale_factor,
        );
        assert_eq!(cached.len(), fresh.len(), "{what}: op count");
        for (i, (c, f)) in cached.iter().zip(&fresh).enumerate() {
            assert!(c == f, "{what}: op {i} differs from a fresh paint");
        }
        self.cache.stats()
    }

    /// Ids of every widget of the kind `pick` accepts, in paint order.
    fn ids(&self, pick: impl Fn(&WidgetKind) -> bool) -> Vec<WidgetId> {
        self.workspace
            .tree
            .paint_order()
            .into_iter()
            .filter(|&id| self.workspace.tree.payload(id).is_some_and(&pick))
            .collect()
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn cached_widget_paints_equal_fresh_ones_through_ui_changes() {
    let mut s = Scene::new();
    let first = s.frame("first frame");
    assert_eq!(first.hits, 0, "a cold cache");
    let again = s.frame("unchanged");
    assert_eq!(again.misses, 0, "an unchanged frame is all hits: {again:?}");
    assert!(again.hits > 100, "{again:?}");

    // Hover: one button at a time, on and off.
    let buttons = s.ids(|k| matches!(k, WidgetKind::Button(_)));
    assert!(buttons.len() > 5);
    for &b in buttons.iter().take(6) {
        let _ = ok(aurora_widgets::widgets::set_button_hovered(
            s.tree(),
            b,
            true,
        ));
        let st = s.frame("hover on");
        assert!(st.misses >= 1, "the hovered button repaints: {st:?}");
        let _ = ok(aurora_widgets::widgets::set_button_hovered(
            s.tree(),
            b,
            false,
        ));
        let _ = s.frame("hover off");
    }

    // Labels: text edits.
    let labels = s.ids(|k| matches!(k, WidgetKind::Label(_)));
    for (i, &l) in labels.iter().take(4).enumerate() {
        let _ = ok(aurora_widgets::widgets::set_label_text(
            s.tree(),
            l,
            &format!("edited {i} …"),
        ));
        s.relayout(1600.0, 1000.0);
        let _ = s.frame("label text");
    }

    // Scroll every scrollable container a little, then back.
    let all = s.workspace.tree.paint_order();
    let mut scrolled = 0;
    for &id in &all {
        if s.tree().scroll_by(id, 37.0).unwrap_or(false) {
            scrolled += 1;
            let _ = s.frame("scrolled");
            let _ = s.tree().set_scroll_y(id, 0.0);
            let _ = s.frame("scrolled back");
        }
    }

    // Caret owner (a text field's caret and selection are overlay ops).
    let fields = s.ids(|k| matches!(k, WidgetKind::TextField(_)));
    for &f in fields.iter().take(2) {
        s.focused = Some(f);
        let _ = s.frame("caret owner");
    }
    s.focused = None;

    // Resizes, narrow and wide.
    for (w, h) in [
        (900.0, 700.0),
        (1200.0, 640.0),
        (2000.0, 1200.0),
        (1600.0, 1000.0),
    ] {
        s.relayout(w, h);
        let _ = s.frame("resize");
    }

    // A dialog over everything, then gone.
    let root = s.workspace.tree.root();
    let scales = s.scales.clone();
    let dialog = ok(aurora_widgets::widgets::insert_dialog(
        s.tree(),
        root,
        &scales,
        "Replace Workspace?",
        "A workspace with this name exists.",
        vec![
            aurora_widgets::widgets::DialogAction::new("replace", "Replace"),
            aurora_widgets::widgets::DialogAction::new("cancel", "Cancel"),
        ],
    ));
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("dialog open");
    ok(s.tree().remove(dialog.root));
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("dialog closed");

    // The command palette.
    let commands = vec![
        aurora_widgets::widgets::CommandEntry::new("edit.undo", "Undo"),
        aurora_widgets::widgets::CommandEntry::new("edit.redo", "Redo"),
    ];
    let palette = ok(aurora_widgets::widgets::insert_command_palette(
        s.tree(),
        root,
        commands,
        &scales,
    ));
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("palette open");
    ok(s.tree().remove(palette));
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("palette closed");

    // Scale factor 2 and back.
    s.scale_factor = 2.0;
    s.relayout(1600.0, 1000.0);
    let st = s.frame("scale 2");
    assert_eq!(st.hits, 0, "a scale change repaints everything: {st:?}");
    s.scale_factor = 1.0;
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("scale 1");

    // Theme switch and back.
    let dark = s.theme;
    s.theme = light_theme();
    assert!(s.theme != dark, "setup: the light theme differs");
    let st = s.frame("light theme");
    assert_eq!(st.hits, 0, "a theme switch repaints everything: {st:?}");
    s.theme = dark;
    let _ = s.frame("dark theme");

    // A plain (non-sRGB) target format.
    s.format = wgpu::TextureFormat::Bgra8Unorm;
    let st = s.frame("plain target");
    assert_eq!(st.hits, 0, "a format change repaints everything: {st:?}");
    s.format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let _ = s.frame("srgb target");

    // A clip change alone: a clipping ancestor shrinks while its
    // children keep their bounds (no relayout), so only their paint clip
    // differs.
    let clipping: Vec<WidgetId> = s
        .workspace
        .tree
        .paint_order()
        .into_iter()
        .filter(|&id| {
            s.workspace.tree.style(id).is_some_and(|st| {
                st.overflow.y != taffy::Overflow::Visible
                    && s.workspace.tree.children(id).is_some_and(|c| !c.is_empty())
            })
        })
        .collect();
    let mut clipped = 0;
    for &c in clipping.iter().take(6) {
        let Some(b) = s.workspace.tree.bounds(c) else {
            continue;
        };
        if b.height < 4 {
            continue;
        }
        ok(s.tree().set_bounds(
            c,
            aurora_core::Rect {
                height: b.height / 2,
                ..b
            },
        ));
        clipped += 1;
        let _ = s.frame("clip shrunk");
        ok(s.tree().set_bounds(c, b));
        let _ = s.frame("clip restored");
    }
    assert!(clipped >= 1, "a clip change was exercised");

    // The measured flag alone: a text-blind layout clears it; every
    // checkbox's bounds are then put back, so only the flag differs.
    let boxes = s.ids(|k| matches!(k, WidgetKind::Checkbox(_)));
    let saved: Vec<_> = boxes
        .iter()
        .filter_map(|&b| s.workspace.tree.bounds(b).map(|r| (b, r)))
        .collect();
    s.tree().compute_layout(1600.0, 1000.0);
    for &(b, r) in &saved {
        ok(s.tree().set_bounds(b, r));
    }
    assert!(
        saved
            .iter()
            .any(|&(b, _)| s.workspace.tree.is_measured(b) == Some(false)),
        "setup: a checkbox lost its measured flag"
    );
    let _ = s.frame("measured flag cleared");
    s.relayout(1600.0, 1000.0);
    let _ = s.frame("measured again");

    let last = s.frame("settled");
    assert_eq!(last.misses, 0, "settled again: {last:?}");
    println!(
        "widget paint cache differential: {} frames, {} buttons hovered, {scrolled} containers \
         scrolled, {} labels edited",
        s.frames,
        buttons.len().min(6),
        labels.len().min(4)
    );
    assert!(scrolled >= 1, "scrolling exercised");
}

/// The GPU path of the cache against [`crate::collect_widget_paints`]: the
/// same ops in the same order (kind, colour, index count), cold, warm and
/// after a hover change, and a warm frame reuses the cold frame's buffers.
#[test]
fn cached_gpu_widget_paints_match_fresh_uploads() {
    let Some(context) = crate::tests::real_gpu_context() else {
        return;
    };
    let mut s = Scene::new();
    let mut atlas = crate::GlyphAtlas::new(context.device());
    let buttons = s.ids(|k| matches!(k, WidgetKind::Button(_)));
    for step in 0..4 {
        if step == 2
            && let Some(&b) = buttons.first()
        {
            let _ = ok(aurora_widgets::widgets::set_button_hovered(
                s.tree(),
                b,
                true,
            ));
        }
        s.engine.begin_frame();
        let fresh = crate::collect_widget_paints(
            &s.workspace.tree,
            None,
            None,
            &s.theme,
            &s.scales,
            &context,
            s.format,
            s.scale_factor,
            Some((&mut s.engine, &mut atlas)),
        );
        s.engine.begin_frame();
        let cached = s.cache.frame_gpu_ops(
            &s.workspace.tree,
            None,
            None,
            &s.theme,
            &s.scales,
            &context,
            s.format,
            s.scale_factor,
            Some((&mut s.engine, &mut atlas)),
        );
        let describe = |ops: &[aurora_widgets::GpuPaintOp]| -> Vec<String> {
            ops.iter().map(|op| format!("{op:?}")).collect()
        };
        assert_eq!(describe(&cached), describe(&fresh), "step {step}");
        let stats = s.cache.stats();
        if step == 1 || step == 3 {
            assert_eq!(stats.misses, 0, "step {step}: warm {stats:?}");
        }
    }
}
