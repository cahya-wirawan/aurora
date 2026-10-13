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
        (Tool::Brush, true, None),
        (Tool::Eraser, true, None),
        (Tool::Gradient, false, Some(Tool::Gradient)),
        (Tool::PaintBucket, false, Some(Tool::PaintBucket)),
        (Tool::Move, false, None),
        (Tool::Brush, true, None),
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
        Tool::Brush,
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
