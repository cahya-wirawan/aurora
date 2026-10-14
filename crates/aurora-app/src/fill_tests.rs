//! Paint Bucket and Gradient (0.209.0): the document flows, driven through
//! the same free functions `App::fill_bucket`/`App::apply_gradient` call
//! (`fill_tools::{bucket_fill, gradient_drag}`, [`commit_fill`],
//! [`step_undo_order`]), plus the shortcuts, palette, drag, locks and the
//! options bar. `App` itself needs a window, so its two thin wrappers
//! (which `self` fields they read) are not run here.

#![allow(clippy::float_cmp, clippy::cast_precision_loss)]

use crate::fill_tools::{self, FillRefused, Filled};
use crate::{
    AppCommand, CompositeCache, DabTool, Drag, LockRefusal, ToolSettings, UndoDirection, UndoOrder,
    activate_command, begin_drag, commit_fill, continue_drag, default_shortcuts, handle_key,
    palette_commands, press_refusal, step_undo_order,
};
use aurora_ui::Tool;
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};
use aurora_widgets::widgets::WidgetKind;

const W: f32 = 1000.0;
const H: f32 = 800.0;
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const CANVAS: (u32, u32) = (600, 300);

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// A document: one pixel layer `bounds`, its store, and the undo state.
struct Doc {
    _dir: tempfile::TempDir,
    store: aurora_tile::TileStore,
    layers: aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    surface: aurora_tile::SurfaceId,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    cache: CompositeCache,
    workspace: aurora_ui::Workspace,
    settings: ToolSettings,
}

impl Doc {
    fn new(bounds: aurora_core::Rect) -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(64) else {
            unreachable!("non-zero");
        };
        let store = ok(aurora_tile::TileStore::new(
            dir.path().to_path_buf(),
            budget,
        ));
        let mut layers = aurora_doc::LayerTree::new();
        let layer = ok(layers.add_pixel_layer("Layer", bounds, None));
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("pixel layer");
        };
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        workspace.tree.compute_layout(W, H);
        Self {
            _dir: dir,
            store,
            layers,
            layer,
            surface,
            history: aurora_doc::History::new(),
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            cache: CompositeCache::default(),
            workspace,
            settings: ToolSettings::default(),
        }
    }

    fn rect(x: i64, y: i64, width: u32, height: u32) -> aurora_core::Rect {
        aurora_core::Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn put(&mut self, x: u32, y: u32, rgba: [f32; 4]) {
        let (tx, ty) = (x / 256, y / 256);
        let tile = ok(self
            .store
            .get_mut(self.surface, aurora_tile::TileId { x: tx, y: ty }));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        for (i, v) in rgba.iter().enumerate() {
            if let Some(s) = tile.texels_mut().get_mut(index + i) {
                *s = half::f16::from_f32(*v);
            }
        }
    }

    fn rect_fill(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, rgba: [f32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgba);
            }
        }
    }

    fn at(&mut self, x: u32, y: u32) -> [f32; 4] {
        let id = aurora_tile::TileId {
            x: x / 256,
            y: y / 256,
        };
        if !self.store.contains_tile(self.surface, id) {
            return [0.0; 4];
        }
        let tile = ok(self.store.get(self.surface, id));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        let at = |o: usize| {
            tile.texels()
                .get(index + o)
                .map_or(f32::NAN, |v| v.to_f32())
        };
        [at(0), at(1), at(2), at(3)]
    }

    fn bucket(&mut self, doc_point: (f32, f32)) -> Result<Filled, FillRefused> {
        fill_tools::bucket_fill(
            &mut self.store,
            &self.layers,
            self.layer,
            doc_point,
            &self.settings,
            GREEN,
            None,
        )
    }

    fn gradient(&mut self, a: (f32, f32), b: (f32, f32)) -> Result<Filled, FillRefused> {
        fill_tools::gradient_drag(
            &mut self.store,
            &self.layers,
            self.layer,
            CANVAS,
            a,
            b,
            &self.settings,
            GREEN,
            BLUE,
            None,
        )
    }

    fn commit(&mut self, filled: Filled, label: &str) -> bool {
        commit_fill(
            &mut self.workspace,
            &self.layers,
            self.layer,
            filled,
            label,
            &mut self.cache,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
        )
    }

    fn undo(&mut self, direction: UndoDirection) -> bool {
        step_undo_order(
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            Some(&mut self.store),
            &mut self.undo_order,
            direction,
        )
        .is_some()
    }

    fn lock(&mut self, lock: aurora_doc::LayerLock) {
        ok(self.layers.set_lock(self.layer, lock));
    }
}

fn history_labels(doc: &Doc) -> Vec<String> {
    doc.undo_order.undo_labels.clone()
}

#[test]
fn a_bucket_click_is_one_labelled_undo_step_that_restores_and_redoes_exact_pixels() {
    let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
    doc.rect_fill(0, 0, 100, 100, [1.0, 1.0, 1.0, 1.0]);
    doc.rect_fill(100, 0, 200, 100, [1.0, 0.0, 0.0, 1.0]);
    let before = doc.at(50, 50);
    let filled = ok(doc.bucket((50.0, 50.0)));
    assert!(doc.commit(filled, fill_tools::PAINT_BUCKET_LABEL));
    assert_eq!(
        doc.at(50, 50),
        [0.0, 1.0, 0.0, 1.0],
        "the foreground, opaque"
    );
    assert_eq!(
        doc.at(150, 50),
        [1.0, 0.0, 0.0, 1.0],
        "the red block is a wall"
    );
    assert_eq!(history_labels(&doc), ["Paint Bucket Fill"]);
    assert_eq!(doc.undo_order.undo.len(), 1, "exactly one step");

    assert!(doc.undo(UndoDirection::Undo));
    assert_eq!(doc.at(50, 50), before, "undo restores the exact pixels");
    assert_eq!(doc.at(10, 90), before);
    assert!(doc.undo_order.undo.is_empty());
    assert!(doc.undo(UndoDirection::Redo));
    assert_eq!(doc.at(50, 50), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(doc.at(99, 99), [0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn clicking_the_same_region_again_records_nothing() {
    let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
    doc.rect_fill(0, 0, 64, 64, [1.0, 1.0, 1.0, 1.0]);
    let first = ok(doc.bucket((5.0, 5.0)));
    assert!(doc.commit(first, "x"));
    let again = ok(doc.bucket((5.0, 5.0)));
    assert!(!doc.commit(again, "x"), "already the foreground: no step");
    assert_eq!(doc.undo_order.undo.len(), 1);
}

#[test]
fn the_bucket_uses_the_layer_origin_and_ignores_points_outside_the_layer() {
    let mut doc = Doc::new(Doc::rect(100, 50, 64, 64));
    doc.rect_fill(0, 0, 64, 64, [1.0, 1.0, 1.0, 1.0]);
    // Document (110, 60) is layer-local (10, 10).
    let filled = ok(doc.bucket((110.0, 60.0)));
    assert!(doc.commit(filled, "x"));
    assert_eq!(doc.at(10, 10), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(doc.at(63, 63), [0.0, 1.0, 0.0, 1.0]);
    // Left of / above the layer, and past its far edge: nothing.
    for point in [(99.0, 60.0), (110.0, 49.0), (164.0, 60.0), (110.0, 114.0)] {
        let result = doc.bucket(point);
        assert!(
            matches!(&result, Err(FillRefused::Nothing))
                || matches!(&result, Ok(f) if f.snapshot.is_empty()),
            "{point:?}: {result:?}"
        );
    }
}

#[test]
fn the_bucket_options_reach_the_algorithm() {
    let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
    // Two levels 40 apart (8-bit units): 100 and 140.
    doc.rect_fill(0, 0, 4, 4, [100.0 / 255.0, 0.0, 0.0, 1.0]);
    doc.rect_fill(4, 0, 8, 4, [140.0 / 255.0, 0.0, 0.0, 1.0]);
    doc.settings.tolerance = 32;
    let f = ok(doc.bucket((1.0, 1.0)));
    assert!(doc.commit(f, "x"));
    assert_ne!(
        doc.at(5, 1)[1],
        1.0,
        "40 apart is outside the default tolerance 32"
    );
    assert!(doc.undo(UndoDirection::Undo));
    doc.settings.tolerance = 40;
    let f = ok(doc.bucket((1.0, 1.0)));
    assert!(doc.commit(f, "x"));
    assert_eq!(doc.at(5, 1)[1], 1.0, "tolerance 40 reaches it");
    // Contiguous off reaches a separate island of the same colour.
    assert!(doc.undo(UndoDirection::Undo));
    doc.settings.tolerance = 0;
    doc.rect_fill(20, 20, 24, 24, [100.0 / 255.0, 0.0, 0.0, 1.0]);
    doc.settings.contiguous = true;
    let f = ok(doc.bucket((1.0, 1.0)));
    assert!(doc.commit(f, "x"));
    assert_ne!(doc.at(21, 21)[1], 1.0, "contiguous leaves the island");
    assert!(doc.undo(UndoDirection::Undo));
    doc.settings.contiguous = false;
    let f = ok(doc.bucket((1.0, 1.0)));
    assert!(doc.commit(f, "x"));
    assert_eq!(doc.at(21, 21)[1], 1.0, "non-contiguous fills the island");
}

#[test]
fn a_fill_invalidates_only_the_tiles_it_wrote() {
    let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
    // A wall at x = 256 splits the layer into two tile columns.
    doc.rect_fill(256, 0, 257, 300, [1.0, 0.0, 0.0, 1.0]);
    let near = aurora_tile::TileId { x: 0, y: 0 };
    let far = aurora_tile::TileId { x: 2, y: 0 };
    doc.cache.mark_current(near);
    doc.cache.mark_current(far);
    let stamp = doc.cache.stamp;
    let filled = ok(doc.bucket((10.0, 10.0)));
    assert!(filled.painted.contains(&near));
    assert!(!filled.painted.contains(&far), "the wall confines the fill");
    assert!(doc.commit(filled, "x"));
    assert!(
        !doc.cache.current.contains(&near),
        "the filled tile is stale"
    );
    assert!(
        doc.cache.current.contains(&far),
        "an untouched tile stays current"
    );
    assert!(doc.cache.stamp > stamp);
}

#[test]
fn a_gradient_is_one_labelled_undo_step_and_undoes_exactly() {
    let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
    let filled = ok(doc.gradient((0.0, 0.0), (100.0, 0.0)));
    assert!(doc.commit(filled, fill_tools::GRADIENT_LABEL));
    assert_eq!(history_labels(&doc), ["Gradient"]);
    // Foreground (green) at the start side, background (blue) beyond the end.
    let start = doc.at(0, 5);
    assert!(start[1] > 0.98 && start[2] < 0.02, "{start:?}");
    assert_eq!(doc.at(300, 5), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(doc.at(599, 299), [0.0, 0.0, 1.0, 1.0], "covers the layer");
    assert!(doc.undo(UndoDirection::Undo));
    assert_eq!(doc.at(0, 5), [0.0; 4]);
    assert_eq!(doc.at(300, 5), [0.0; 4]);
    assert!(doc.undo(UndoDirection::Redo));
    assert_eq!(doc.at(300, 5), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn reverse_radial_and_opacity_reach_the_gradient() {
    let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
    doc.settings.gradient_reverse = true;
    let f = ok(doc.gradient((0.0, 0.0), (100.0, 0.0)));
    assert!(doc.commit(f, "x"));
    assert_eq!(
        doc.at(300, 5),
        [0.0, 1.0, 0.0, 1.0],
        "reversed: the foreground is at the end"
    );
    assert!(doc.undo(UndoDirection::Undo));

    doc.settings.gradient_reverse = false;
    doc.settings.gradient_radial = true;
    let f = ok(doc.gradient((100.0, 100.0), (150.0, 100.0)));
    assert!(doc.commit(f, "x"));
    // Equal distance right and down: equal colours; a linear one is not.
    assert_eq!(doc.at(125, 100), doc.at(100, 125));
    assert!(doc.at(100, 100)[1] > 0.9);
    assert!(doc.undo(UndoDirection::Undo));

    doc.settings.gradient_radial = false;
    doc.settings.gradient_opacity = 50;
    doc.rect_fill(0, 0, 600, 300, [1.0, 0.0, 0.0, 1.0]);
    let f = ok(doc.gradient((0.0, 0.0), (100.0, 0.0)));
    assert!(doc.commit(f, "x"));
    let p = doc.at(300, 5);
    assert!(
        (p[0] - 0.5).abs() < 0.01 && (p[2] - 0.5).abs() < 0.01,
        "{p:?}"
    );
}

#[test]
fn a_zero_length_gradient_drag_changes_nothing_and_records_nothing() {
    let mut doc = Doc::new(Doc::rect(0, 0, 100, 100));
    let f = ok(doc.gradient((40.0, 40.0), (40.0, 40.0)));
    assert!(!doc.commit(f, "x"));
    assert!(doc.undo_order.undo.is_empty());
    assert_eq!(doc.at(40, 40), [0.0; 4]);
}

#[test]
fn a_gradient_covers_the_layer_inside_the_canvas_only() {
    let canvas = (600, 300);
    // A layer hanging off the left/top and past the right/bottom.
    let area = fill_tools::gradient_area(Doc::rect(-50, -20, 1000, 1000), canvas);
    assert_eq!(area, (50, 20, 650, 320));
    let area = fill_tools::gradient_area(Doc::rect(10, 5, 100, 50), canvas);
    assert_eq!(area, (0, 0, 100, 50));
    let area = fill_tools::gradient_area(Doc::rect(0, 0, 100, 100), (50, 50));
    assert_eq!(area, (0, 0, 50, 50));
    // A layer entirely outside the canvas: an empty area.
    let (x0, _, x1, _) = fill_tools::gradient_area(Doc::rect(700, 0, 100, 100), canvas);
    assert!(x0 >= x1);
}

#[test]
fn locks_refuse_or_preserve_alpha_for_both_tools() {
    for pixels in [true] {
        let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
        doc.rect_fill(0, 0, 64, 64, [1.0, 1.0, 1.0, 1.0]);
        doc.lock(aurora_doc::LayerLock {
            pixels,
            ..aurora_doc::LayerLock::none()
        });
        let r = doc.bucket((5.0, 5.0));
        assert!(
            matches!(r, Err(FillRefused::Notice(LockRefusal::PixelsLocked))),
            "{r:?}"
        );
        let r = doc.gradient((0.0, 0.0), (50.0, 0.0));
        assert!(
            matches!(r, Err(FillRefused::Notice(LockRefusal::PixelsLocked))),
            "{r:?}"
        );
        assert_eq!(doc.at(5, 5), [1.0; 4], "nothing was written");
        assert!(doc.undo_order.undo.is_empty(), "no history row");
        for tool in [Tool::Gradient, Tool::PaintBucket] {
            assert_eq!(
                press_refusal(tool, doc.layers.lock(doc.layer).unwrap_or_default()),
                Some(LockRefusal::PixelsLocked)
            );
        }
    }
    // Lock All too.
    let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
    doc.lock(aurora_doc::LayerLock::all());
    assert!(matches!(
        doc.bucket((1.0, 1.0)),
        Err(FillRefused::Notice(_))
    ));

    // Transparency lock: alpha untouched, transparent pixels stay empty.
    let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
    doc.rect_fill(0, 0, 8, 8, [0.5, 0.5, 0.5, 0.5]);
    doc.lock(aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::none()
    });
    doc.settings.tolerance = 255;
    let f = ok(doc.bucket((1.0, 1.0)));
    assert!(doc.commit(f, "x"));
    assert_eq!(
        doc.at(1, 1),
        [0.0, 0.5, 0.0, 0.5],
        "green x alpha, alpha kept"
    );
    assert_eq!(doc.at(20, 20), [0.0; 4]);
    let f = ok(doc.gradient((0.0, 0.0), (40.0, 0.0)));
    assert!(doc.commit(f, "y"));
    assert_eq!(doc.at(1, 1)[3], 0.5);
    assert_eq!(doc.at(20, 20), [0.0; 4], "still empty");
    assert_eq!(doc.undo_order.undo.len(), 2);
}

#[test]
fn a_too_large_fill_is_refused_calmly_with_nothing_written() {
    let mut doc = Doc::new(Doc::rect(0, 0, 20_000, 20_000));
    let r = doc.bucket((10.0, 10.0));
    assert!(
        matches!(r, Err(FillRefused::Notice(LockRefusal::FillTooLarge))),
        "{r:?}"
    );
    assert!(doc.undo_order.undo.is_empty());
    assert!(
        !doc.store
            .contains_tile(doc.surface, aurora_tile::TileId { x: 0, y: 0 })
    );
    // The same message class stays on screen only for the fill tools.
    assert!(crate::lock_refusal_applies(
        LockRefusal::FillTooLarge,
        Tool::PaintBucket,
        aurora_doc::LayerLock::none()
    ));
    assert!(!crate::lock_refusal_applies(
        LockRefusal::FillTooLarge,
        Tool::Brush,
        aurora_doc::LayerLock::none()
    ));
    assert!(LockRefusal::FillTooLarge.message().contains("too large"));
    // A big gradient area too: a 9000 x 9000 canvas layer is 1,296 tiles.
    let mut big = Doc::new(Doc::rect(0, 0, 9_000, 9_000));
    let r = fill_tools::gradient_drag(
        &mut big.store,
        &big.layers,
        big.layer,
        (9_000, 9_000),
        (0.0, 0.0),
        (10.0, 0.0),
        &big.settings,
        GREEN,
        BLUE,
        None,
    );
    assert!(
        matches!(r, Err(FillRefused::Notice(LockRefusal::FillTooLarge))),
        "{r:?}"
    );
}

#[test]
fn a_group_or_missing_layer_is_refused_with_a_calm_notice() {
    let mut doc = Doc::new(Doc::rect(0, 0, 64, 64));
    let group = ok(doc.history.add_group_at(
        &mut doc.layers,
        "G",
        None,
        0,
        aurora_doc::BlendMode::PassThrough,
    ));
    let bucket = fill_tools::bucket_fill(
        &mut doc.store,
        &doc.layers,
        group,
        (1.0, 1.0),
        &doc.settings,
        GREEN,
        None,
    );
    assert!(matches!(
        bucket,
        Err(FillRefused::Notice(LockRefusal::FillNeedsPixelLayer))
    ));
    let gradient = fill_tools::gradient_drag(
        &mut doc.store,
        &doc.layers,
        group,
        CANVAS,
        (0.0, 0.0),
        (9.0, 0.0),
        &doc.settings,
        GREEN,
        BLUE,
        None,
    );
    assert!(matches!(
        gradient,
        Err(FillRefused::Notice(LockRefusal::FillNeedsPixelLayer))
    ));
    assert!(doc.undo_order.undo.is_empty(), "no history row");
    // No active layer at all.
    assert_eq!(
        fill_tools::fill_target(None),
        Err(LockRefusal::FillNeedsPixelLayer)
    );
    assert_eq!(fill_tools::fill_target(Some(doc.layer)), Ok(doc.layer));
    // The notice names what to do, and stays only for the fill tools.
    assert!(
        LockRefusal::FillNeedsPixelLayer
            .message()
            .contains("pixel layer")
    );
    for (tool, stays) in [
        (Tool::Gradient, true),
        (Tool::PaintBucket, true),
        (Tool::Brush, false),
        (Tool::Move, false),
    ] {
        assert_eq!(
            crate::lock_refusal_applies(
                LockRefusal::FillNeedsPixelLayer,
                tool,
                aurora_doc::LayerLock::none()
            ),
            stays,
            "{tool:?}"
        );
    }
}

#[test]
fn fills_over_transparent_pixels_under_the_lock_or_at_zero_opacity_create_no_tile() {
    let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
    doc.lock(aurora_doc::LayerLock {
        transparency: true,
        ..aurora_doc::LayerLock::none()
    });
    doc.settings.tolerance = 255;
    let f = ok(doc.bucket((10.0, 10.0)));
    assert!(!doc.commit(f, "x"));
    let f = ok(doc.gradient((0.0, 0.0), (100.0, 0.0)));
    assert!(!doc.commit(f, "x"));
    doc.lock(aurora_doc::LayerLock::none());
    doc.settings.gradient_opacity = 0;
    let f = ok(doc.gradient((0.0, 0.0), (100.0, 0.0)));
    assert!(!doc.commit(f, "x"));
    assert_eq!(doc.store.resident_len(), 0, "no phantom tile");
    assert!(doc.undo_order.undo.is_empty());
}

#[test]
fn a_gradient_drag_applies_on_release_whatever_the_tool_or_button() {
    let drag = Drag::Gradient {
        start_doc: (1.0, 2.0),
        current_doc: (30.0, 40.0),
    };
    assert_eq!(
        crate::released_gradient(Some(&drag)),
        Some(((1.0, 2.0), (30.0, 40.0)))
    );
    assert_eq!(crate::released_gradient(None), None);
    assert_eq!(
        crate::released_gradient(Some(&Drag::Marquee {
            start_doc: (0.0, 0.0),
            elliptical: false,
            before: None,
            current: None,
            modal: crate::selection_modes::ModeGesture::boxed_replace(),
        })),
        None
    );
    assert_eq!(crate::released_gradient(Some(&Drag::Eyedropper)), None);
    // `App::handle_pointer_released` (not runnable without a window) takes
    // the gradient from `released_gradient` before committing the drag, and
    // never looks at `self.tool` or the button: a source guard, compared
    // with whitespace removed.
    let source = include_str!("lib.rs");
    let production = source.split("\nmod tests {").next().unwrap_or(source);
    let squeeze = |text: &str| text.split_whitespace().collect::<String>();
    let squeezed = squeeze(production);
    assert!(squeezed.contains(&squeeze(
        "let ending = self.drag.take(); let ending = lasso_tools::release_lasso(&mut self.drag, ending, &mut self.doc.selection); if let Some((start_doc, end_doc)) = released_gradient(ending.as_ref()) { self.apply_gradient(start_doc, end_doc); } self.commit_drag(ending);"
    )));
}

// -- tools, shortcuts, palette, drag ------------------------------------

fn press(tool: &mut Tool, modifiers: Modifiers, ch: char) {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    workspace.tree.compute_layout(W, H);
    let mut focus = aurora_widgets::FocusManager::default();
    let mut layers = aurora_doc::LayerTree::new();
    let mut history = aurora_doc::History::new();
    let mut pixel_history = aurora_brush::PixelHistory::new();
    let mut undo_order = UndoOrder::default();
    let _ = handle_key(
        &mut workspace,
        &mut focus,
        &mut None,
        &mut None,
        tool,
        &ToolSettings::default(),
        &mut layers,
        &mut history,
        &mut pixel_history,
        None,
        &mut undo_order,
        &default_shortcuts(),
        modifiers,
        Key::Character(ch),
        Some(&ch.to_string()),
        &mut crate::tests::FakeClipboard::default(),
        &mut crate::tests::FakeFileDialog::default(),
    );
    assert_eq!(
        aurora_ui::selected_tool(&workspace.tree, &workspace.tools),
        Some(*tool),
        "the tools panel follows the shortcut"
    );
}

#[test]
fn g_selects_the_gradient_and_shift_g_the_paint_bucket() {
    let mut tool = Tool::Move;
    press(&mut tool, Modifiers::none(), 'g');
    assert_eq!(tool, Tool::Gradient);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::none()
    };
    press(&mut tool, shift, 'g');
    assert_eq!(tool, Tool::PaintBucket);
    // Bare G from the bucket goes back to the gradient.
    press(&mut tool, Modifiers::none(), 'g');
    assert_eq!(tool, Tool::Gradient);
    let shortcuts = default_shortcuts();
    for tool in Tool::ALL {
        let text = tool.shortcut_text().to_ascii_lowercase();
        let chord = ok(KeyChord::parse(&text));
        assert_eq!(
            shortcuts.resolve(chord),
            Some(&AppCommand::SelectTool(tool)),
            "{tool:?} {text}"
        );
    }
}

#[test]
fn the_palette_has_a_select_entry_for_every_tool_and_it_selects_it() {
    let commands = palette_commands();
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    workspace.tree.compute_layout(W, H);
    let mut focus = aurora_widgets::FocusManager::default();
    for tool in Tool::ALL {
        let label = format!("Select {} Tool", tool.label());
        let Some(entry) = commands.iter().find(|c| c.title == label) else {
            unreachable!("{label} is in the palette");
        };
        let activated = activate_command(
            &mut workspace,
            &mut focus,
            &entry.id,
            &mut crate::tests::FakeFileDialog::default(),
        );
        assert!(
            matches!(activated, Some(crate::ActivatedCommand::SelectTool(t)) if t == tool),
            "{label}: {activated:?}"
        );
    }
}

#[test]
fn clicking_a_new_tool_button_selects_it_and_the_buttons_are_named_and_described() {
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    workspace.tree.compute_layout(W, H);
    for (tool, text) in [
        (Tool::Gradient, "Gradient tool, shortcut G"),
        (Tool::PaintBucket, "Paint Bucket tool, shortcut Shift+G"),
    ] {
        let Some(id) = workspace.tools.button_for(tool) else {
            unreachable!("a button");
        };
        let outcome =
            aurora_widgets::PointerOutcome::Action(aurora_widgets::ActionOutcome::Activated(id));
        assert_eq!(crate::tool_button_click(&workspace, &outcome), Some(tool));
        let Some(node) = workspace.tree.accessibility(id) else {
            unreachable!("node");
        };
        assert_eq!(node.label(), Some(tool.label()));
        assert_eq!(node.description(), Some(text));
        let Some(b) = workspace.tree.bounds(id) else {
            unreachable!("laid out");
        };
        assert_eq!((b.width, b.height), (32, 32));
    }
}

#[test]
fn a_gradient_press_starts_a_drag_that_follows_the_pointer_and_a_bucket_press_starts_none() {
    let view = aurora_ui::CanvasView::new();
    let mut drag = begin_drag(
        Tool::Gradient,
        crate::PointerButton::Primary,
        (10.0, 20.0),
        &view,
        None,
    );
    let Some(Drag::Gradient {
        start_doc,
        current_doc,
    }) = &drag
    else {
        unreachable!("a gradient drag: {drag:?}");
    };
    assert_eq!(start_doc, current_doc);
    let start = *start_doc;
    let mut view = view;
    let mut selection = aurora_doc::SelectionSet::new();
    let dabs = continue_drag(
        drag.as_mut().unwrap_or_else(|| unreachable!()),
        (110.0, 20.0),
        &mut view,
        &mut selection,
        &ToolSettings::default(),
        crate::PanBounds {
            min_doc: (0.0, 0.0),
            max_doc: (1.0e6, 1.0e6),
            canvas_size: (0.0, 0.0),
        },
    );
    assert!(dabs.is_empty());
    let Some(Drag::Gradient {
        start_doc,
        current_doc,
    }) = &drag
    else {
        unreachable!();
    };
    assert_eq!(*start_doc, start, "the start is fixed");
    assert!((current_doc.0 - start.0 - 100.0).abs() < 1.0);
    // The bucket acts on the press itself; the middle button still pans.
    assert!(
        begin_drag(
            Tool::PaintBucket,
            crate::PointerButton::Primary,
            (1.0, 1.0),
            &view,
            None
        )
        .is_none()
    );
    assert!(matches!(
        begin_drag(
            Tool::Gradient,
            crate::PointerButton::Middle,
            (1.0, 1.0),
            &view,
            None
        ),
        Some(Drag::Pan { .. })
    ));
    // Neither fill tool is a dab tool.
    let _ = DabTool::Brush;
}

// -- the options bar ------------------------------------------------------

struct Bar {
    workspace: aurora_ui::Workspace,
    controls: aurora_ui::ToolControls,
    settings: ToolSettings,
}

impl Bar {
    fn new() -> Self {
        let scales = crate::test_workspace_scales();
        let mut workspace = aurora_ui::build_workspace(&scales);
        let controls = ok(aurora_ui::insert_tool_controls(
            &mut workspace.tree,
            workspace.options_bar,
            workspace.properties,
            &scales,
        ));
        workspace.tree.compute_layout(W, H);
        Self {
            workspace,
            controls,
            settings: ToolSettings::default(),
        }
    }

    fn outcome(&mut self, tool: Tool, action: aurora_widgets::ActionOutcome) -> bool {
        let changed = crate::apply_tool_control_outcome(
            &mut self.workspace,
            Some(self.controls),
            tool,
            &mut self.settings,
            &aurora_widgets::PointerOutcome::Action(action),
            None,
        );
        self.workspace.tree.compute_layout(W, H);
        changed
    }
}

#[test]
fn the_options_bar_edits_the_fill_settings_and_mirrors_them() {
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let fill = bar.controls.fill;
    assert_eq!(ToolSettings::default().tolerance, 32, "Photoshop's default");
    assert!(ToolSettings::default().contiguous);
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::PaintBucket,
        &bar.settings,
        None,
    );
    assert!(bar.outcome(
        Tool::PaintBucket,
        A::ValueChanged {
            id: fill.tolerance,
            value: 90.4
        }
    ));
    assert_eq!(bar.settings.tolerance, 90);
    assert!(bar.outcome(
        Tool::PaintBucket,
        A::ValueChanged {
            id: fill.tolerance,
            value: 9000.0
        }
    ));
    assert_eq!(bar.settings.tolerance, 255);
    assert!(!bar.outcome(
        Tool::PaintBucket,
        A::ValueChanged {
            id: fill.tolerance,
            value: f64::NAN
        }
    ));
    assert!(bar.outcome(
        Tool::PaintBucket,
        A::Toggled {
            id: fill.contiguous,
            state: accesskit::Toggled::False
        }
    ));
    assert!(!bar.settings.contiguous);

    assert!(bar.outcome(
        Tool::Gradient,
        A::Toggled {
            id: fill.radial,
            state: accesskit::Toggled::True
        }
    ));
    assert!(bar.settings.gradient_radial);
    assert!(
        !bar.outcome(
            Tool::Gradient,
            A::Toggled {
                id: fill.radial,
                state: accesskit::Toggled::True
            }
        ),
        "idempotent"
    );
    assert!(bar.outcome(
        Tool::Gradient,
        A::Toggled {
            id: fill.radial,
            state: accesskit::Toggled::False
        }
    ));
    assert!(!bar.settings.gradient_radial);
    assert!(bar.outcome(
        Tool::Gradient,
        A::Toggled {
            id: fill.reverse,
            state: accesskit::Toggled::True
        }
    ));
    assert!(bar.settings.gradient_reverse);
    assert!(bar.outcome(
        Tool::Gradient,
        A::ValueChanged {
            id: fill.opacity,
            value: 35.0
        }
    ));
    assert_eq!(bar.settings.gradient_opacity, 35);
    assert!(bar.outcome(
        Tool::Gradient,
        A::ValueChanged {
            id: fill.opacity,
            value: -4.0
        }
    ));
    assert_eq!(bar.settings.gradient_opacity, 0);
    // Not a fill widget, not a fill outcome.
    assert!(!bar.outcome(Tool::Gradient, A::Activated(bar.controls.radius)));
    // The widgets show the settings.
    let label = |bar: &Bar, id| match bar.workspace.tree.payload(id) {
        Some(WidgetKind::Label(s)) => s.text.clone(),
        other => unreachable!("{other:?}"),
    };
    assert_eq!(label(&bar, fill.opacity_readout), "Opacity 0 %");
    assert_eq!(label(&bar, fill.tolerance_readout), "Tolerance 255");
}

#[test]
fn brush_and_eraser_keep_their_radius_row_and_fill_tools_replace_it() {
    let mut bar = Bar::new();
    let radius_shown =
        |bar: &Bar| aurora_ui::radius_slider_shown(&bar.workspace.tree, &bar.controls);
    for (tool, radius, group) in [
        (Tool::Brush, true, Some(Tool::Brush)),
        // The Eraser shares the Opacity and Flow group since 0.229.0.
        (Tool::Eraser, true, Some(Tool::Brush)),
        (Tool::Gradient, false, Some(Tool::Gradient)),
        (Tool::PaintBucket, false, Some(Tool::PaintBucket)),
        (Tool::Move, false, None),
        (Tool::Brush, true, Some(Tool::Brush)),
    ] {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
        bar.workspace.tree.compute_layout(W, H);
        assert_eq!(radius_shown(&bar), radius, "{tool:?}");
        assert_eq!(
            aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
            group,
            "{tool:?}"
        );
    }
}

#[test]
fn with_measured_text_every_option_fits_the_options_bar_and_the_labels_have_room() {
    let scales = crate::test_workspace_scales();
    let mut bar = Bar::new();
    let Ok(mut engine) = aurora_text::TextEngine::new() else {
        unreachable!("the bundled font loads");
    };
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Brush,
        &bar.settings,
        None,
    );
    aurora_widgets::compute_text_layout(
        &mut bar.workspace.tree,
        W,
        H,
        Some(aurora_widgets::TextMeasure {
            engine: &mut engine,
            scales: &scales,
            scale_factor: 1.0,
        }),
    );
    let brush_height = bar
        .workspace
        .tree
        .bounds(bar.workspace.options_bar)
        .map_or(0, |b| b.height);
    for tool in [Tool::Gradient, Tool::PaintBucket] {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
        aurora_widgets::compute_text_layout(
            &mut bar.workspace.tree,
            W,
            H,
            Some(aurora_widgets::TextMeasure {
                engine: &mut engine,
                scales: &scales,
                scale_factor: 1.0,
            }),
        );
        let Some(strip) = bar.workspace.tree.bounds(bar.workspace.options_bar) else {
            unreachable!("laid out");
        };
        // The bar is as tall for a fill tool as for the Brush: the canvas
        // below it never jumps on a tool switch.
        assert_eq!(strip.height, brush_height, "{tool:?}");
        let fill = bar.controls.fill;
        let ids = match tool {
            Tool::Gradient => vec![fill.radial, fill.reverse, fill.opacity],
            _ => vec![fill.tolerance, fill.contiguous],
        };
        let mut previous_right = strip.x;
        for id in ids {
            let Some(b) = bar.workspace.tree.bounds(id) else {
                unreachable!("laid out");
            };
            assert!(b.width >= 20, "{tool:?} {id:?} is measured: {b:?}");
            assert!(
                b.x >= previous_right,
                "{tool:?} {id:?} overlaps its left neighbour"
            );
            assert!(b.right() <= strip.right(), "{tool:?} {id:?} fits the bar");
            previous_right = b.right();
        }
    }
}

#[test]
fn the_options_bar_edits_the_smudge_strength_and_mirrors_it() {
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let fill = bar.controls.fill;
    assert_eq!(ToolSettings::default().smudge_strength, 50);
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Smudge,
        &bar.settings,
        None,
    );
    assert!(bar.outcome(
        Tool::Smudge,
        A::ValueChanged {
            id: fill.strength,
            value: 72.4
        }
    ));
    assert_eq!(bar.settings.smudge_strength, 72);
    assert!(bar.outcome(
        Tool::Smudge,
        A::ValueChanged {
            id: fill.strength,
            value: 900.0
        }
    ));
    assert_eq!(bar.settings.smudge_strength, 100);
    assert!(bar.outcome(
        Tool::Smudge,
        A::ValueChanged {
            id: fill.strength,
            value: -3.0
        }
    ));
    assert_eq!(bar.settings.smudge_strength, 1);
    assert!(!bar.outcome(
        Tool::Smudge,
        A::ValueChanged {
            id: fill.strength,
            value: f64::NAN
        }
    ));
    // The widgets show the setting, and only the Smudge shows the group.
    let label = |bar: &Bar, id| match bar.workspace.tree.payload(id) {
        Some(WidgetKind::Label(s)) => s.text.clone(),
        other => unreachable!("{other:?}"),
    };
    assert_eq!(label(&bar, fill.strength_readout), "Strength 1 %");
    assert_eq!(
        aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
        Some(Tool::Smudge)
    );
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Move,
        &bar.settings,
        None,
    );
    assert_eq!(
        aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
        None
    );
}

#[test]
fn the_options_bar_edits_the_blur_and_sharpen_strengths_each_on_its_own() {
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let fill = bar.controls.fill;
    let label = |bar: &Bar, id| match bar.workspace.tree.payload(id) {
        Some(WidgetKind::Label(s)) => s.text.clone(),
        other => unreachable!("{other:?}"),
    };
    for (tool, other, final_value) in [
        (Tool::Blur, Tool::Sharpen, 41.0),
        (Tool::Sharpen, Tool::Blur, 63.0),
    ] {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
        assert_eq!(label(&bar, fill.strength_readout), "Strength 50 %");
        assert!(bar.outcome(
            tool,
            A::ValueChanged {
                id: fill.strength,
                value: 33.3
            }
        ));
        assert!(bar.outcome(
            tool,
            A::ValueChanged {
                id: fill.strength,
                value: 900.0
            }
        ));
        assert!(bar.outcome(
            tool,
            A::ValueChanged {
                id: fill.strength,
                value: final_value
            }
        ));
        assert!(!bar.outcome(
            tool,
            A::ValueChanged {
                id: fill.strength,
                value: f64::NAN
            }
        ));
        assert_eq!(
            label(&bar, fill.strength_readout),
            format!("Strength {final_value} %")
        );
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            other,
            &bar.settings,
            None,
        );
        // The other filter and the Smudge are untouched.
        assert_ne!(
            label(&bar, fill.strength_readout),
            format!("Strength {final_value} %")
        );
    }
    assert_eq!(bar.settings.blur_strength, 41);
    assert_eq!(bar.settings.sharpen_strength, 63);
    assert_eq!(bar.settings.smudge_strength, 50);
}

#[test]
fn the_options_bar_edits_exposure_and_flow_each_on_its_own_under_their_own_name() {
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let fill = bar.controls.fill;
    let label = |bar: &Bar, id| match bar.workspace.tree.payload(id) {
        Some(WidgetKind::Label(s)) => s.text.clone(),
        other => unreachable!("{other:?}"),
    };
    let slider_name = |bar: &Bar| match bar.workspace.tree.payload(fill.strength) {
        Some(WidgetKind::Slider(s)) => s.label.clone(),
        other => unreachable!("{other:?}"),
    };
    let node_name = |bar: &Bar| {
        bar.workspace
            .tree
            .accessibility(fill.strength)
            .and_then(|n| n.label().map(str::to_owned))
    };
    for (tool, word, value) in [
        (Tool::Dodge, "Exposure", 21.0),
        (Tool::Burn, "Exposure", 64.0),
        (Tool::Sponge, "Flow", 77.0),
        (Tool::Smudge, "Strength", 50.0),
    ] {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
        assert_eq!(slider_name(&bar), word, "{tool:?}");
        assert_eq!(node_name(&bar).as_deref(), Some(word), "{tool:?}");
        assert_eq!(
            aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
            Some(Tool::Smudge)
        );
        if tool != Tool::Smudge {
            assert!(bar.outcome(
                tool,
                A::ValueChanged {
                    id: fill.strength,
                    value
                }
            ));
            assert!(!bar.outcome(
                tool,
                A::ValueChanged {
                    id: fill.strength,
                    value: f64::NAN
                }
            ));
        }
        assert_eq!(
            label(&bar, fill.strength_readout),
            format!("{word} {value} %"),
            "{tool:?}"
        );
    }
    assert_eq!(bar.settings.dodge_exposure, 21);
    assert_eq!(bar.settings.burn_exposure, 64);
    assert_eq!(bar.settings.sponge_flow, 77);
    assert_eq!(bar.settings.smudge_strength, 50);
    assert_eq!(bar.settings.blur_strength, 50);
    // Switching back shows each tool's own value again.
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Dodge,
        &bar.settings,
        None,
    );
    assert_eq!(label(&bar, fill.strength_readout), "Exposure 21 %");
    // A tool with no Strength group does not show it.
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Move,
        &bar.settings,
        None,
    );
    assert_eq!(
        aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
        None
    );
}

#[test]
fn the_range_and_mode_checkboxes_set_the_tone_settings_and_stay_radio_groups() {
    use aurora_brush::{SpongeMode, ToneRange};
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let tone = bar.controls.tone;
    assert_eq!(bar.settings.tone_range, ToneRange::Midtones);
    assert_eq!(bar.settings.sponge_mode, SpongeMode::Desaturate);
    let checked = |bar: &Bar, id| {
        matches!(
            bar.workspace.tree.payload(id),
            Some(WidgetKind::Checkbox(s)) if s.checked == accesskit::Toggled::True
        )
    };
    let shown = |bar: &Bar, id| {
        bar.workspace
            .tree
            .style(id)
            .is_some_and(|s| s.display != taffy::Display::None)
    };
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Burn,
        &bar.settings,
        None,
    );
    assert!(shown(&bar, tone.range_group));
    assert!(!shown(&bar, tone.mode_group));
    for range in ToneRange::ALL {
        let _ = bar.outcome(
            Tool::Burn,
            A::Toggled {
                id: tone.range_checkbox(range),
                state: accesskit::Toggled::True,
            },
        );
        assert_eq!(bar.settings.tone_range, range);
        for other in ToneRange::ALL {
            assert_eq!(
                checked(&bar, tone.range_checkbox(other)),
                other == range,
                "{range:?} / {other:?}"
            );
        }
    }
    // Unchecking the one in force changes nothing and re-checks it.
    let _ = bar.outcome(
        Tool::Burn,
        A::Toggled {
            id: tone.range_checkbox(ToneRange::Highlights),
            state: accesskit::Toggled::False,
        },
    );
    assert!(checked(&bar, tone.highlights));
    assert_eq!(bar.settings.tone_range, ToneRange::Highlights);

    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Sponge,
        &bar.settings,
        None,
    );
    assert!(!shown(&bar, tone.range_group));
    assert!(shown(&bar, tone.mode_group));
    let _ = bar.outcome(
        Tool::Sponge,
        A::Toggled {
            id: tone.saturate,
            state: accesskit::Toggled::True,
        },
    );
    assert_eq!(bar.settings.sponge_mode, SpongeMode::Saturate);
    assert!(checked(&bar, tone.saturate));
    assert!(!checked(&bar, tone.desaturate));
    // The range is the Dodge/Burn's alone: the Sponge's choice left it.
    assert_eq!(bar.settings.tone_range, ToneRange::Highlights);
    // Any other tool shows neither group.
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        Tool::Brush,
        &bar.settings,
        None,
    );
    assert!(!shown(&bar, tone.range_group));
    assert!(!shown(&bar, tone.mode_group));
}

// -- Opacity and Flow of the paint tools (0.228.0) --------------------------

#[test]
fn the_paint_settings_default_to_a_hundred_percent_and_map_to_the_engine_per_tool() {
    use aurora_brush::PaintAmount;
    let mut settings = ToolSettings::default();
    for tool in [DabTool::Brush, DabTool::Pencil, DabTool::Eraser] {
        assert_eq!(settings.paint_amount(tool), PaintAmount::FULL, "{tool:?}");
    }
    assert_eq!(settings.clone_amount(), PaintAmount::FULL);
    settings.brush_opacity = 40;
    settings.brush_flow = 70;
    settings.pencil_opacity = 55;
    settings.clone_opacity = 25;
    settings.clone_flow = 90;
    assert_eq!(
        settings.paint_amount(DabTool::Brush),
        PaintAmount::new(0.40, 0.70)
    );
    // The Pencil has an Opacity of its own and no Flow, whatever the Brush's.
    assert_eq!(
        settings.paint_amount(DabTool::Pencil),
        PaintAmount::new(0.55, 1.0)
    );
    // The Eraser is unchanged by every one of them, and has its own pair
    // (0.229.0).
    assert_eq!(settings.paint_amount(DabTool::Eraser), PaintAmount::FULL);
    settings.eraser_opacity = 30;
    settings.eraser_flow = 45;
    assert_eq!(
        settings.paint_amount(DabTool::Eraser),
        PaintAmount::new(0.30, 0.45)
    );
    assert_eq!(
        settings.paint_amount(DabTool::Brush),
        PaintAmount::new(0.40, 0.70),
        "the Eraser's pair is not the Brush's"
    );
    assert_eq!(settings.clone_amount(), PaintAmount::new(0.25, 0.90));
}

#[test]
fn the_setters_round_clamp_and_refuse_a_non_finite_value_and_a_tool_without_the_setting() {
    let mut s = ToolSettings::default();
    assert!(s.set_paint_percent(Tool::Brush, false, 33.4));
    assert_eq!(s.brush_opacity, 33);
    assert!(!s.set_paint_percent(Tool::Brush, false, 33.0), "unchanged");
    assert!(s.set_paint_percent(Tool::Brush, false, 900.0));
    assert_eq!(s.brush_opacity, 100);
    assert!(s.set_paint_percent(Tool::Brush, false, -4.0));
    assert_eq!(s.brush_opacity, 1);
    assert!(!s.set_paint_percent(Tool::Brush, false, f64::NAN));
    assert!(!s.set_paint_percent(Tool::Brush, true, f64::INFINITY));
    assert!(s.set_paint_percent(Tool::Brush, true, 12.0));
    assert_eq!(s.brush_flow, 12);
    // The Pencil: Opacity only.
    assert!(s.set_paint_percent(Tool::Pencil, false, 60.0));
    assert_eq!(s.pencil_opacity, 60);
    assert!(!s.set_paint_percent(Tool::Pencil, true, 60.0));
    assert!(s.set_paint_percent(Tool::CloneStamp, false, 20.0));
    assert!(s.set_paint_percent(Tool::CloneStamp, true, 30.0));
    assert_eq!((s.clone_opacity, s.clone_flow), (20, 30));
    // The Eraser has its own pair (0.229.0).
    assert!(s.set_paint_percent(Tool::Eraser, false, 44.0));
    assert!(s.set_paint_percent(Tool::Eraser, true, 66.0));
    assert!(!s.set_paint_percent(Tool::Eraser, true, f64::NAN));
    assert_eq!((s.eraser_opacity, s.eraser_flow), (44, 66));
    // The rest have none; nothing else moved.
    for tool in [Tool::Smudge, Tool::Move, Tool::Gradient] {
        assert!(!s.set_paint_percent(tool, false, 50.0), "{tool:?}");
        assert!(!s.set_paint_percent(tool, true, 50.0), "{tool:?}");
    }
    assert_eq!((s.brush_opacity, s.brush_flow), (1, 12));
    assert_eq!(s.pencil_opacity, 60);
    assert_eq!((s.eraser_opacity, s.eraser_flow), (44, 66));
}

#[test]
#[allow(clippy::too_many_lines)]
fn the_options_bar_edits_the_brush_pencil_and_clone_opacity_and_flow_each_on_its_own() {
    use aurora_widgets::ActionOutcome as A;
    let mut bar = Bar::new();
    let fill = bar.controls.fill;
    let label = |bar: &Bar, id| match bar.workspace.tree.payload(id) {
        Some(WidgetKind::Label(s)) => s.text.clone(),
        other => unreachable!("{other:?}"),
    };
    let sync = |bar: &mut Bar, tool| {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
    };
    sync(&mut bar, Tool::Brush);
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 100 %");
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 100 %");
    assert!(bar.outcome(
        Tool::Brush,
        A::ValueChanged {
            id: fill.paint_opacity,
            value: 62.4
        }
    ));
    assert!(bar.outcome(
        Tool::Brush,
        A::ValueChanged {
            id: fill.paint_flow,
            value: 35.0
        }
    ));
    assert!(!bar.outcome(
        Tool::Brush,
        A::ValueChanged {
            id: fill.paint_opacity,
            value: f64::NAN
        }
    ));
    assert_eq!(
        (bar.settings.brush_opacity, bar.settings.brush_flow),
        (62, 35)
    );
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 62 %");
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 35 %");
    // The Pencil starts from its own 100 % and leaves the Brush's alone.
    sync(&mut bar, Tool::Pencil);
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 100 %");
    assert!(bar.outcome(
        Tool::Pencil,
        A::ValueChanged {
            id: fill.paint_opacity,
            value: 45.0
        }
    ));
    // The Pencil's hidden Flow slider cannot edit anything.
    assert!(!bar.outcome(
        Tool::Pencil,
        A::ValueChanged {
            id: fill.paint_flow,
            value: 5.0
        }
    ));
    assert_eq!(bar.settings.pencil_opacity, 45);
    assert_eq!(
        (bar.settings.brush_opacity, bar.settings.brush_flow),
        (62, 35)
    );
    // The Clone Stamp has both, its own.
    sync(&mut bar, Tool::CloneStamp);
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 100 %");
    assert!(bar.outcome(
        Tool::CloneStamp,
        A::ValueChanged {
            id: fill.paint_opacity,
            value: 20.0
        }
    ));
    assert!(bar.outcome(
        Tool::CloneStamp,
        A::ValueChanged {
            id: fill.paint_flow,
            value: 80.0
        }
    ));
    assert_eq!(
        (bar.settings.clone_opacity, bar.settings.clone_flow),
        (20, 80)
    );
    assert_eq!(bar.settings.pencil_opacity, 45);
    // The Eraser has both, its own (0.229.0), starting from 100 %.
    sync(&mut bar, Tool::Eraser);
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 100 %");
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 100 %");
    assert!(bar.outcome(
        Tool::Eraser,
        A::ValueChanged {
            id: fill.paint_opacity,
            value: 55.0
        }
    ));
    assert!(bar.outcome(
        Tool::Eraser,
        A::ValueChanged {
            id: fill.paint_flow,
            value: 25.0
        }
    ));
    assert_eq!(
        (bar.settings.eraser_opacity, bar.settings.eraser_flow),
        (55, 25)
    );
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 55 %");
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 25 %");
    assert_eq!(
        (bar.settings.clone_opacity, bar.settings.clone_flow),
        (20, 80),
        "the Clone Stamp's were not touched"
    );
    assert_eq!(
        (bar.settings.brush_opacity, bar.settings.brush_flow),
        (62, 35),
        "nor the Brush's"
    );
    // Back to the Brush: its values are mirrored again.
    sync(&mut bar, Tool::Brush);
    assert_eq!(label(&bar, fill.paint_opacity_readout), "Opacity 62 %");
    assert_eq!(label(&bar, fill.paint_flow_readout), "Flow 35 %");
    assert_eq!(
        aurora_ui::shown_fill_group(&bar.workspace.tree, &bar.controls),
        Some(Tool::Brush)
    );
}

#[test]
fn with_measured_text_the_paint_options_fit_the_bar_without_changing_its_height() {
    // A 1100 px window (a ~780 px options bar, the rail open). Until 0.228.3
    // the readouts collapsed to nothing in a bar narrower than about
    // 1,200 px, so this test had to use a 1,600 px window.
    const WIDE: f32 = 1100.0;
    let scales = crate::test_workspace_scales();
    let mut bar = Bar::new();
    let Ok(mut engine) = aurora_text::TextEngine::new() else {
        unreachable!("the bundled font loads");
    };
    let mut height_of = |bar: &mut Bar, tool: Tool| {
        crate::sync_tool_controls(
            &mut bar.workspace,
            Some(bar.controls),
            tool,
            &bar.settings,
            None,
        );
        aurora_widgets::compute_text_layout(
            &mut bar.workspace.tree,
            WIDE,
            H,
            Some(aurora_widgets::TextMeasure {
                engine: &mut engine,
                scales: &scales,
                scale_factor: 1.0,
            }),
        );
        bar.workspace
            .tree
            .bounds(bar.workspace.options_bar)
            .map_or(0, |b| b.height)
    };
    let reference = height_of(&mut bar, Tool::Move);
    for tool in [Tool::Brush, Tool::Pencil, Tool::CloneStamp, Tool::Eraser] {
        assert_eq!(height_of(&mut bar, tool), reference, "{tool:?}");
        let Some(strip) = bar.workspace.tree.bounds(bar.workspace.options_bar) else {
            unreachable!("laid out");
        };
        let fill = bar.controls.fill;
        let mut ids = vec![fill.paint_opacity_readout, fill.paint_opacity];
        if aurora_ui::has_paint_flow(tool) {
            ids.extend([fill.paint_flow_readout, fill.paint_flow]);
        }
        let mut previous_right = strip.x;
        for id in ids {
            let Some(b) = bar.workspace.tree.bounds(id) else {
                unreachable!("laid out");
            };
            assert!(b.width >= 20, "{tool:?} {id:?} is measured: {b:?}");
            assert!(
                b.x >= previous_right,
                "{tool:?} {id:?} overlaps a neighbour"
            );
            assert!(b.right() <= strip.right(), "{tool:?} {id:?} fits the bar");
            previous_right = b.right();
        }
    }
}

// -- the options bar in a narrow window (0.228.3) ----------------------------

/// One shown leaf of the options bar: a readout, a slider or a checkbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Readout,
    Slider,
    Check,
}

struct Item {
    id: aurora_widgets::WidgetId,
    parent: aurora_widgets::WidgetId,
    leaf: Leaf,
    x: i64,
    right: i64,
}

/// Lays the options bar out for `tool` in a window `width` px wide with
/// measured text and returns its shown leaves in tree order, plus the
/// bar's (left, right).
fn options_items(bar: &mut Bar, tool: Tool, width: f32) -> (Vec<Item>, (i64, i64)) {
    let scales = crate::test_workspace_scales();
    let Ok(mut engine) = aurora_text::TextEngine::new() else {
        unreachable!("the bundled font loads");
    };
    crate::sync_tool_controls(
        &mut bar.workspace,
        Some(bar.controls),
        tool,
        &bar.settings,
        None,
    );
    aurora_widgets::compute_text_layout(
        &mut bar.workspace.tree,
        width,
        H,
        Some(aurora_widgets::TextMeasure {
            engine: &mut engine,
            scales: &scales,
            scale_factor: 1.0,
        }),
    );
    let tree = &bar.workspace.tree;
    let Some(strip) = tree.bounds(bar.workspace.options_bar) else {
        unreachable!("laid out");
    };
    let mut items = Vec::new();
    let mut stack = vec![bar.workspace.options_bar];
    while let Some(id) = stack.pop() {
        if tree
            .style(id)
            .is_some_and(|s| s.display == taffy::Display::None)
        {
            continue;
        }
        let leaf = match tree.payload(id) {
            Some(WidgetKind::Label(_)) => Some(Leaf::Readout),
            Some(WidgetKind::Slider(_)) => Some(Leaf::Slider),
            Some(WidgetKind::Checkbox(_)) => Some(Leaf::Check),
            _ => None,
        };
        if let (Some(leaf), Some(b), Some(parent)) = (leaf, tree.bounds(id), tree.parent(id)) {
            items.push(Item {
                id,
                parent,
                leaf,
                x: b.x,
                right: b.right(),
            });
        }
        if let Some(children) = tree.children(id) {
            stack.extend(children.iter().rev().copied());
        }
    }
    (items, (strip.x, strip.right()))
}

fn item_width(item: &Item) -> i64 {
    item.right - item.x
}

const OPTION_WINDOWS: [f32; 6] = [700.0, 900.0, 1000.0, 1100.0, 1300.0, 1600.0];

#[test]
fn no_options_bar_readout_slider_or_checkbox_collapses_or_overlaps_at_any_width() {
    let mut bar = Bar::new();
    for width in OPTION_WINDOWS {
        for tool in Tool::ALL {
            let (items, (left, _)) = options_items(&mut bar, tool, width);
            assert!(!items.is_empty(), "{tool:?} at {width}: something is shown");
            for item in &items {
                assert!(
                    item_width(item) > 0,
                    "{tool:?} at {width}: a {:?} has no width",
                    item.leaf
                );
                // A readout is its measured text, never a sliver.
                if item.leaf == Leaf::Readout {
                    assert!(
                        item_width(item) >= 40,
                        "{tool:?} at {width}: a readout is {} px",
                        item_width(item)
                    );
                }
                assert!(item.x >= left, "{tool:?} at {width}: starts inside the bar");
            }
            // Siblings never overlap: left to right in tree order.
            for pair in items.windows(2) {
                if let [a, b] = pair
                    && a.parent == b.parent
                {
                    assert!(
                        b.x >= a.right,
                        "{tool:?} at {width}: {:?} overlaps the {:?} before it",
                        b.leaf,
                        a.leaf
                    );
                }
            }
        }
    }
}

#[test]
fn with_room_every_options_bar_control_fits_inside_the_bar() {
    let mut bar = Bar::new();
    for width in [1300.0, 1600.0] {
        for tool in Tool::ALL {
            let (items, (_, right)) = options_items(&mut bar, tool, width);
            for item in &items {
                assert!(
                    item.right <= right,
                    "{tool:?} at {width}: a {:?} runs past the bar",
                    item.leaf
                );
            }
        }
    }
}

#[test]
fn in_a_narrow_bar_the_sliders_give_up_width_before_any_readout() {
    let mut bar = Bar::new();
    let floor = {
        let scales = crate::test_workspace_scales();
        i64::from(scales.size.options_control_width / 2)
    };
    let full = i64::from(crate::test_workspace_scales().size.options_control_width);
    for tool in [
        Tool::Brush,
        Tool::Pencil,
        Tool::CloneStamp,
        Tool::Eraser,
        Tool::Smudge,
        Tool::Dodge,
        Tool::Sponge,
        Tool::Gradient,
        Tool::PaintBucket,
        Tool::MagicWand,
    ] {
        // The readouts' own widths are what they have at the narrowest
        // window; widening only ever lengthens them, never below it.
        let (narrow, _) = options_items(&mut bar, tool, 700.0);
        for width in [900.0, 1000.0, 1100.0, 1300.0, 1600.0] {
            let (wide, _) = options_items(&mut bar, tool, width);
            assert_eq!(narrow.len(), wide.len());
            for (n, w) in narrow.iter().zip(&wide) {
                assert_eq!(n.id, w.id);
                if n.leaf == Leaf::Slider {
                    assert!(
                        (floor..=full).contains(&item_width(w)),
                        "{tool:?} at {width}: a slider is {} px",
                        item_width(w)
                    );
                } else {
                    assert!(
                        item_width(w) >= item_width(n),
                        "{tool:?} at {width}: a {:?} is narrower than at 700",
                        w.leaf
                    );
                }
            }
        }
    }
    // At 700 px the Brush's three sliders are all at their floor: the bar
    // is shorter than their controls, and nothing else was squeezed.
    let (brush, _) = options_items(&mut bar, Tool::Brush, 700.0);
    for item in brush.iter().filter(|i| i.leaf == Leaf::Slider) {
        assert_eq!(item_width(item), floor, "a Brush slider at 700 px");
    }
    // Between the two, the Brush's sliders are wider than the floor but
    // not yet full, with its readouts exactly where they were at 700.
    let (narrow, _) = options_items(&mut bar, Tool::Brush, 700.0);
    let (mid, _) = options_items(&mut bar, Tool::Brush, 1100.0);
    let sliders: Vec<i64> = mid
        .iter()
        .filter(|i| i.leaf == Leaf::Slider)
        .map(item_width)
        .collect();
    assert_eq!(sliders.len(), 3, "radius, opacity, flow");
    assert!(
        sliders.iter().all(|w| *w > floor && *w < full),
        "{sliders:?}"
    );
    for (n, m) in narrow.iter().zip(&mid) {
        if n.leaf == Leaf::Readout {
            assert_eq!(item_width(n), item_width(m), "a readout kept its width");
        }
    }
    // The Eraser's group (0.229.0) follows the same rules: radius, Opacity
    // and Flow sliders, each given up to the floor before any readout.
    let (eraser_narrow, _) = options_items(&mut bar, Tool::Eraser, 700.0);
    let eraser_sliders: Vec<i64> = eraser_narrow
        .iter()
        .filter(|i| i.leaf == Leaf::Slider)
        .map(item_width)
        .collect();
    assert_eq!(eraser_sliders.len(), 3, "radius, opacity, flow");
    assert!(
        eraser_sliders.iter().all(|w| *w == floor),
        "{eraser_sliders:?}"
    );
    let readouts = eraser_narrow
        .iter()
        .filter(|i| i.leaf == Leaf::Readout)
        .count();
    assert_eq!(readouts, 3, "radius, Opacity and Flow readouts");
}

#[test]
fn the_options_bar_clips_what_does_not_fit_instead_of_wrapping_or_changing_height() {
    let mut bar = Bar::new();
    let (_, _) = options_items(&mut bar, Tool::Move, 1100.0);
    let reference = bar
        .workspace
        .tree
        .bounds(bar.workspace.options_bar)
        .map_or(0, |b| b.height);
    for width in OPTION_WINDOWS {
        for tool in Tool::ALL {
            let (_, _) = options_items(&mut bar, tool, width);
            let height = bar
                .workspace
                .tree
                .bounds(bar.workspace.options_bar)
                .map_or(0, |b| b.height);
            assert_eq!(height, reference, "{tool:?} at {width}");
        }
    }
    let clips = bar
        .workspace
        .tree
        .style(bar.workspace.options_bar)
        .is_some_and(|s| s.overflow.x != taffy::Overflow::Visible);
    assert!(clips, "the bar clips its end");
}

// ---- 0.234.0: Fill Path and Stroke Path ----------------------------------

mod path_paint_flows {
    use super::*;
    use crate::path_paint::{self, FILL_PATH_LABEL, STROKE_PATH_LABEL};
    use aurora_doc::{Anchor, Path, Subpath};

    fn square(x: f32, y: f32, side: f32, closed: bool) -> Path {
        Path {
            name: "Work Path".into(),
            subpaths: vec![Subpath {
                anchors: vec![
                    Anchor::corner(x, y),
                    Anchor::corner(x + side, y),
                    Anchor::corner(x + side, y + side),
                    Anchor::corner(x, y + side),
                ],
                closed,
            }],
        }
    }

    fn segment(a: (f32, f32), b: (f32, f32)) -> Path {
        Path {
            name: "Work Path".into(),
            subpaths: vec![Subpath {
                anchors: vec![Anchor::corner(a.0, a.1), Anchor::corner(b.0, b.1)],
                closed: false,
            }],
        }
    }

    fn fill(
        doc: &mut Doc,
        path: &Path,
        sel: Option<aurora_doc::Selection>,
    ) -> Result<Filled, FillRefused> {
        path_paint::fill_path(
            &mut doc.store,
            &doc.layers,
            doc.layer,
            CANVAS,
            path,
            &doc.settings,
            GREEN,
            sel,
        )
    }

    fn stroke(
        doc: &mut Doc,
        path: &Path,
        sel: Option<aurora_doc::Selection>,
    ) -> Result<Filled, FillRefused> {
        path_paint::stroke_path(
            &mut doc.store,
            &doc.layers,
            doc.layer,
            path,
            &doc.settings,
            GREEN,
            sel,
        )
    }

    fn all(doc: &mut Doc, w: u32, h: u32) -> Vec<[f32; 4]> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| doc.at(x, y))
            .collect()
    }

    #[test]
    fn fill_path_paints_the_interior_only_as_one_undoable_step() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        let before = all(&mut doc, 300, 250);
        let filled = ok(fill(&mut doc, &square(100.0, 50.0, 100.0, true), None));
        assert!(doc.commit(filled, FILL_PATH_LABEL));
        assert_eq!(doc.at(150, 100), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(
            doc.at(100, 50),
            [0.0, 1.0, 0.0, 1.0],
            "the first covered pixel"
        );
        assert_eq!(doc.at(99, 100), [0.0; 4]);
        assert_eq!(doc.at(200, 100), [0.0; 4], "the right edge is exclusive");
        assert_eq!(doc.at(150, 150), [0.0; 4]);
        assert_eq!(history_labels(&doc), ["Fill Path"]);
        assert!(doc.undo(UndoDirection::Undo));
        assert_eq!(all(&mut doc, 300, 250), before);
        assert!(doc.undo(UndoDirection::Redo));
        assert_eq!(doc.at(150, 100), [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn filling_the_whole_layer_equals_the_paint_bucket() {
        let mut a = Doc::new(Doc::rect(0, 0, 600, 300));
        let mut b = Doc::new(Doc::rect(0, 0, 600, 300));
        let bucket = ok(a.bucket((10.0, 10.0)));
        assert!(a.commit(bucket, fill_tools::PAINT_BUCKET_LABEL));
        let filled = ok(fill(&mut b, &square(0.0, 0.0, 600.0, true), None));
        assert!(!filled.painted.is_empty());
        // The square is 600 wide and the canvas 300 tall: clipped to it.
        for (x, y) in [(0, 0), (300, 150), (599, 299), (17, 211)] {
            assert_eq!(a.at(x, y), b.at(x, y), "({x},{y})");
        }
    }

    #[test]
    fn fill_path_uses_the_brush_opacity_and_the_selection() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.settings.brush_opacity = 50;
        let selection = aurora_doc::Selection {
            bounds: Doc::rect(0, 0, 150, 300),
            inverted: false,
            shape: aurora_doc::SelectionShape::Rect,
        };
        let filled = ok(fill(
            &mut doc,
            &square(100.0, 50.0, 100.0, true),
            Some(selection),
        ));
        assert!(doc.commit(filled, FILL_PATH_LABEL));
        let inside = doc.at(120, 100);
        assert!(
            (inside[3] - 0.5).abs() < 2e-3 && (inside[1] - 0.5).abs() < 2e-3,
            "{inside:?}"
        );
        assert_eq!(doc.at(170, 100), [0.0; 4], "outside the selection");
    }

    #[test]
    fn fill_path_respects_the_locks_and_the_notices() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.lock(aurora_doc::LayerLock {
            pixels: true,
            ..aurora_doc::LayerLock::none()
        });
        let refused = fill(&mut doc, &square(0.0, 0.0, 50.0, true), None);
        assert!(matches!(
            refused,
            Err(FillRefused::Notice(LockRefusal::PixelsLocked))
        ));
        doc.lock(aurora_doc::LayerLock {
            transparency: true,
            ..aurora_doc::LayerLock::none()
        });
        let locked = ok(fill(&mut doc, &square(0.0, 0.0, 50.0, true), None));
        assert!(
            locked.snapshot.is_empty(),
            "transparent pixels stay untouched"
        );
        assert!(
            !doc.commit(locked, FILL_PATH_LABEL),
            "a no-op records nothing"
        );
        doc.lock(aurora_doc::LayerLock::none());
        // Opaque pixels recolour with the alpha kept.
        doc.rect_fill(0, 0, 50, 50, [1.0, 0.0, 0.0, 0.5]);
        doc.lock(aurora_doc::LayerLock {
            transparency: true,
            ..aurora_doc::LayerLock::none()
        });
        let filled = ok(fill(&mut doc, &square(0.0, 0.0, 50.0, true), None));
        assert!(doc.commit(filled, FILL_PATH_LABEL));
        let px = doc.at(10, 10);
        assert_eq!(px[3], 0.5);
        assert!(px[0] == 0.0 && px[1] == 0.5, "recoloured: {px:?}");
        // A path with no area says so.
        doc.lock(aurora_doc::LayerLock::none());
        let none = fill(&mut doc, &segment((0.0, 0.0), (10.0, 10.0)), None);
        assert!(matches!(
            none,
            Err(FillRefused::Notice(LockRefusal::Path(m))) if m == path_paint::NOTHING_TO_FILL_NOTICE
        ));
    }

    #[test]
    fn stroke_path_paints_along_the_path_as_one_undoable_stroke() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.settings.brush_radius = 10.0;
        let before = all(&mut doc, 300, 200);
        let filled = ok(stroke(
            &mut doc,
            &segment((50.0, 100.0), (250.0, 100.0)),
            None,
        ));
        assert!(doc.commit(filled, STROKE_PATH_LABEL));
        assert!(doc.at(150, 100)[3] > 0.95);
        assert!(doc.at(150, 100)[1] > 0.95);
        assert_eq!(doc.at(150, 130), [0.0; 4]);
        assert_eq!(doc.at(300, 100), [0.0; 4], "past the end");
        assert_eq!(history_labels(&doc), ["Stroke Path"]);
        assert_eq!(doc.undo_order.undo.len(), 1);
        assert!(doc.undo(UndoDirection::Undo));
        assert_eq!(all(&mut doc, 300, 200), before);
    }

    #[test]
    fn stroke_path_is_the_live_brushs_dab_sequence() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        let mut live = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.settings.brush_radius = 12.0;
        doc.settings.brush_opacity = 70;
        doc.settings.brush_flow = 60;
        live.settings = doc.settings.clone();
        let path = segment((40.0, 60.0), (230.0, 150.0));
        ok(stroke(&mut doc, &path, None));
        let flat = vec![(
            path.subpaths
                .iter()
                .flat_map(|s| s.flatten(aurora_doc::FLATTEN_TOLERANCE))
                .collect::<Vec<_>>(),
            false,
        )];
        let step = aurora_brush::dab_step(12.0, aurora_brush::DEFAULT_SPACING);
        let planned = path_paint::path_dabs(&flat, step, 100_000);
        let mut drag = Some(Drag::Brush {
            hard: false,
            clone: None,
            smudge: None,
            filter: None,
            tone: None,
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: Some(aurora_brush::StrokeSnapshot::new(live.surface)),
            warned: std::collections::HashSet::new(),
        });
        for dab in planned.dabs {
            let _ = crate::selection_paint::dab_through_selection(
                DabTool::Brush,
                false,
                &mut live.store,
                live.surface,
                Doc::rect(0, 0, 600, 300),
                dab,
                None,
                &live.settings,
                GREEN,
                &mut drag,
            );
        }
        assert_eq!(all(&mut doc, 260, 180), all(&mut live, 260, 180));
    }

    #[test]
    fn the_opacity_ceiling_holds_over_a_self_overlapping_stroke() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.settings.brush_radius = 10.0;
        doc.settings.brush_opacity = 50;
        // Out and back over the same line.
        let there_and_back = Path {
            name: "Work Path".into(),
            subpaths: vec![Subpath {
                anchors: vec![
                    Anchor::corner(50.0, 100.0),
                    Anchor::corner(250.0, 100.0),
                    Anchor::corner(50.0, 100.0),
                ],
                closed: false,
            }],
        };
        let filled = ok(stroke(&mut doc, &there_and_back, None));
        assert!(doc.commit(filled, STROKE_PATH_LABEL));
        let alpha = doc.at(150, 100)[3];
        assert!(alpha <= 0.5 + 2e-3 && alpha > 0.4, "{alpha}");
    }

    #[test]
    fn stroke_path_closed_includes_the_closing_side_and_open_does_not() {
        let mut closed = Doc::new(Doc::rect(0, 0, 600, 300));
        let mut open = Doc::new(Doc::rect(0, 0, 600, 300));
        closed.settings.brush_radius = 4.0;
        open.settings.brush_radius = 4.0;
        ok(stroke(&mut closed, &square(50.0, 50.0, 100.0, true), None));
        ok(stroke(&mut open, &square(50.0, 50.0, 100.0, false), None));
        assert!(closed.at(50, 100)[3] > 0.9, "the closing side");
        assert_eq!(open.at(50, 100), [0.0; 4]);
        assert!(open.at(100, 50)[3] > 0.9);
    }

    #[test]
    fn stroke_path_respects_the_locks_the_selection_and_the_dab_cap() {
        let mut doc = Doc::new(Doc::rect(0, 0, 600, 300));
        doc.settings.brush_radius = 6.0;
        doc.lock(aurora_doc::LayerLock {
            pixels: true,
            ..aurora_doc::LayerLock::none()
        });
        assert!(matches!(
            stroke(&mut doc, &segment((0.0, 0.0), (50.0, 0.0)), None),
            Err(FillRefused::Notice(LockRefusal::PixelsLocked))
        ));
        doc.lock(aurora_doc::LayerLock {
            transparency: true,
            ..aurora_doc::LayerLock::none()
        });
        let locked = ok(stroke(&mut doc, &segment((0.0, 0.0), (50.0, 0.0)), None));
        assert!(locked.snapshot.is_empty());
        doc.lock(aurora_doc::LayerLock::none());
        let selection = aurora_doc::Selection {
            bounds: Doc::rect(0, 0, 100, 300),
            inverted: false,
            shape: aurora_doc::SelectionShape::Rect,
        };
        let filled = ok(stroke(
            &mut doc,
            &segment((20.0, 50.0), (250.0, 50.0)),
            Some(selection),
        ));
        assert!(doc.commit(filled, STROKE_PATH_LABEL));
        assert!(doc.at(60, 50)[3] > 0.9);
        assert_eq!(doc.at(200, 50), [0.0; 4], "outside the selection");
        // A huge path is refused before anything is painted.
        doc.settings.brush_radius = 1.0;
        let huge = segment((0.0, 0.0), (900_000.0, 0.0));
        let before = doc.undo_order.undo.len();
        assert!(matches!(
            stroke(&mut doc, &huge, None),
            Err(FillRefused::Notice(LockRefusal::Path(m))) if m == path_paint::TOO_MANY_DABS_NOTICE
        ));
        assert_eq!(doc.undo_order.undo.len(), before);
        // A group or no pixel layer is the usual notice.
        assert!(matches!(
            path_paint::stroke_path(
                &mut doc.store,
                &doc.layers,
                aurora_doc::LayerId::from_raw(9999),
                &huge,
                &doc.settings,
                GREEN,
                None
            ),
            Err(FillRefused::Notice(LockRefusal::FillNeedsPixelLayer))
        ));
    }
}
