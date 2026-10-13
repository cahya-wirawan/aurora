//! Painting through the active selection (0.211.0), driven the way the app
//! drives it: the Marquee tool makes the selection through `begin_drag` and
//! `continue_drag` (so a real `SelectionSet`), then the Brush, Eraser,
//! Paint Bucket and Gradient act through the very functions `App` calls
//! (`selection_paint::dab_through_selection`, `fill_tools::{bucket_fill,
//! gradient_drag}`, `commit_ending_drag`, `commit_fill`,
//! `step_undo_order`). `App` itself needs a window, so its thin wrappers
//! (which `self` fields they pass) are not run here.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::indexing_slicing
)]

use crate::PointerButton;
use crate::fill_tools::{self, FillRefused};
use crate::{
    CompositeCache, DabTool, Drag, ToolSettings, UndoDirection, UndoOrder, begin_drag,
    commit_ending_drag, commit_fill, continue_drag, pan_bounds, released_gradient, selection_paint,
    step_undo_order,
};
use aurora_core::Rect;
use aurora_ui::{CanvasView, Tool};

const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const RED: [f32; 3] = [1.0, 0.0, 0.0];

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    store: aurora_tile::TileStore,
    layers: aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    surface: aurora_tile::SurfaceId,
    bounds: Rect,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    cache: CompositeCache,
    workspace: aurora_ui::Workspace,
    selection: aurora_doc::SelectionSet,
    view: CanvasView,
    settings: ToolSettings,
}

impl Rig {
    /// A document with one transparent pixel layer at `bounds` (document
    /// space), so `bounds.x/y` is the layer's origin.
    fn new(bounds: Rect) -> Self {
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
        workspace.tree.compute_layout(1000.0, 800.0);
        Self {
            _dir: dir,
            store,
            layers,
            layer,
            surface,
            bounds,
            history: aurora_doc::History::new(),
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            cache: CompositeCache::default(),
            workspace,
            selection: aurora_doc::SelectionSet::new(),
            view: CanvasView::new(),
            settings: ToolSettings::default(),
        }
    }

    /// Drags the Marquee tool from `from` to `to` (canvas points, which are
    /// document points at the default view).
    fn marquee(&mut self, from: (f32, f32), to: (f32, f32)) {
        let mut drag = begin_drag(
            Tool::MarqueeSelect,
            PointerButton::Primary,
            from,
            &self.view,
            None,
        );
        let Some(d) = drag.as_mut() else {
            unreachable!("the marquee tool starts a drag");
        };
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        let dabs = continue_drag(
            d,
            to,
            &mut self.view,
            &mut self.selection,
            &self.settings,
            bounds,
        );
        assert!(dabs.is_empty());
    }

    fn put(&mut self, x: u32, y: u32, rgba: [f32; 4]) {
        let tile = ok(self.store.get_mut(
            self.surface,
            aurora_tile::TileId {
                x: x / 256,
                y: y / 256,
            },
        ));
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

    /// The layer-local texel `(x, y)`.
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

    fn tiles(&self) -> usize {
        let mut n = 0;
        for y in 0..8 {
            for x in 0..8 {
                if self
                    .store
                    .contains_tile(self.surface, aurora_tile::TileId { x, y })
                {
                    n += 1;
                }
            }
        }
        n
    }

    /// A Brush or Eraser stroke along `points` (canvas = document points),
    /// pressed at the first and released after the last. Returns whether a
    /// history step was recorded.
    fn stroke(&mut self, tool: Tool, points: &[(f32, f32)], preserve_alpha: bool) -> bool {
        let Some(&first) = points.first() else {
            unreachable!("a stroke has points");
        };
        let mut drag = begin_drag(
            tool,
            PointerButton::Primary,
            first,
            &self.view,
            crate::active_pixel_layer(&self.layers, Some(self.layer)),
        );
        // The very function `App::paint_dab` reads the Pencil from.
        let dab_tool = if tool == Tool::Eraser {
            DabTool::Eraser
        } else {
            crate::dab_tool_of_drag(drag.as_ref())
        };
        let selection = self.selection.active();
        let colour = if dab_tool == DabTool::Eraser {
            [0.0; 3]
        } else {
            RED
        };
        let press = match drag.as_ref() {
            Some(Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. }) => *last_doc,
            _ => unreachable!("a paint tool starts a stroke"),
        };
        let dab_at = |rig: &mut Self, drag: &mut Option<Drag>, p: (f32, f32)| {
            let _ = selection_paint::dab_through_selection(
                dab_tool,
                preserve_alpha,
                &mut rig.store,
                rig.surface,
                rig.bounds,
                p,
                selection.clone(),
                &rig.settings,
                colour,
                drag,
            );
        };
        dab_at(self, &mut drag, press);
        for &p in points.iter().skip(1) {
            let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
            let dabs = match drag.as_mut() {
                Some(d) => continue_drag(
                    d,
                    p,
                    &mut self.view,
                    &mut self.selection,
                    &self.settings,
                    bounds,
                ),
                None => Vec::new(),
            };
            for dab in dabs {
                dab_at(self, &mut drag, dab);
            }
        }
        commit_ending_drag(
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        )
    }

    fn bucket(&mut self, point: (f32, f32)) -> bool {
        let result = fill_tools::bucket_fill(
            &mut self.store,
            &self.layers,
            self.layer,
            point,
            &self.settings,
            GREEN,
            self.selection.active(),
        );
        self.commit(result, fill_tools::PAINT_BUCKET_LABEL)
    }

    fn gradient(&mut self, from: (f32, f32), to: (f32, f32)) -> bool {
        let mut drag = begin_drag(
            Tool::Gradient,
            PointerButton::Primary,
            from,
            &self.view,
            None,
        );
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        if let Some(d) = drag.as_mut() {
            let _ = continue_drag(
                d,
                to,
                &mut self.view,
                &mut self.selection,
                &self.settings,
                bounds,
            );
        }
        let Some((start, end)) = released_gradient(drag.as_ref()) else {
            unreachable!("a gradient drag");
        };
        let result = fill_tools::gradient_drag(
            &mut self.store,
            &self.layers,
            self.layer,
            (1000, 800),
            start,
            end,
            &self.settings,
            GREEN,
            BLUE,
            self.selection.active(),
        );
        self.commit(result, fill_tools::GRADIENT_LABEL)
    }

    fn commit(&mut self, result: Result<fill_tools::Filled, FillRefused>, label: &str) -> bool {
        match result {
            Ok(filled) => commit_fill(
                &mut self.workspace,
                &self.layers,
                self.layer,
                filled,
                label,
                &mut self.cache,
                &mut self.history,
                &mut self.pixel_history,
                &mut self.undo_order,
            ),
            Err(_) => false,
        }
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

    /// Every texel of the layer's top-left `w x h`, for exact comparisons.
    fn snapshot(&mut self, w: u32, h: u32) -> Vec<[u32; 4]> {
        let mut out = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let t = self.at(x, y);
                out.push([
                    t[0].to_bits(),
                    t[1].to_bits(),
                    t[2].to_bits(),
                    t[3].to_bits(),
                ]);
            }
        }
        out
    }
}

/// Whether document point `(x, y)` is inside the rectangle (half-open).
fn inside(sel: Rect, x: i64, y: i64) -> bool {
    sel.contains_point(x, y)
}

#[test]
fn the_marquee_makes_a_real_selection_the_tools_then_use() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    assert!(rig.selection.active().is_none());
    rig.marquee((100.0, 80.0), (300.0, 260.0));
    let Some(selection) = rig.selection.active() else {
        unreachable!("the marquee selected");
    };
    assert_eq!(selection.bounds, rect(100, 80, 200, 180));
    assert!(!selection.inverted);
}

#[test]
fn a_brush_stroke_paints_only_inside_the_marquee_selection() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 80.0), (300.0, 260.0));
    let sel = rect(100, 80, 200, 180);
    // A fat stroke across the selection and well out of it on both sides.
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(40.0, 170.0), (350.0, 170.0)], false));
    let (mut painted_in, mut painted_out) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            let a = rig.at(x, y)[3];
            if a > 0.0 {
                if inside(sel, i64::from(x), i64::from(y)) {
                    painted_in += 1;
                } else {
                    painted_out += 1;
                }
            }
        }
    }
    assert!(painted_in > 3000, "the stroke paints inside: {painted_in}");
    assert_eq!(painted_out, 0, "and never outside");
    assert_eq!(rig.undo_order.undo_labels, ["Brush Stroke"], "one step");
    // Undo and redo are exact.
    let after = rig.snapshot(600, 400);
    assert!(rig.undo(UndoDirection::Undo));
    assert!(rig.snapshot(600, 400).iter().all(|t| *t == [0; 4]));
    assert!(rig.undo(UndoDirection::Redo));
    assert_eq!(rig.snapshot(600, 400), after);
}

/// 0.212.0 review: a Marquee click (press, then a jittering or
/// one-dimensional move) over an existing selection deselects, and the
/// brush then paints the whole layer again.
#[test]
fn a_marquee_click_on_a_selection_deselects_and_painting_works_again() {
    for to in [(200.4, 170.3), (260.0, 170.0), (200.0, 230.0)] {
        let mut rig = Rig::new(rect(0, 0, 600, 400));
        rig.marquee((100.0, 80.0), (300.0, 260.0));
        assert!(rig.selection.active().is_some());
        let _ = crate::selection_view::press_marquee(&mut rig.selection);
        rig.marquee((200.0, 170.0), to);
        assert_eq!(rig.selection.active(), None, "{to:?}");
        rig.settings.set_radius(Tool::Brush, 20.0);
        // A stroke well outside the old selection paints now.
        assert!(rig.stroke(Tool::Brush, &[(400.0, 340.0), (520.0, 340.0)], false));
        assert!(rig.at(450, 340)[3] > 0.0, "{to:?}: painted");
    }
}

#[test]
fn a_layer_with_an_origin_is_painted_in_document_space() {
    // The layer sits at document (50, 30); the selection is document
    // (100, 80) 100 x 100, i.e. layer-local (50, 50) 100 x 100.
    let mut rig = Rig::new(rect(50, 30, 400, 300));
    rig.marquee((100.0, 80.0), (200.0, 180.0));
    rig.settings.set_radius(Tool::Brush, 25.0);
    assert!(rig.stroke(Tool::Brush, &[(60.0, 130.0), (260.0, 130.0)], false));
    let mut inside_count = 0;
    for ly in 0..300_u32 {
        for lx in 0..400_u32 {
            let doc = (i64::from(lx) + 50, i64::from(ly) + 30);
            let painted = rig.at(lx, ly)[3] > 0.0;
            if painted {
                assert!(
                    inside(rect(100, 80, 100, 100), doc.0, doc.1),
                    "layer-local ({lx}, {ly}) is document {doc:?}, outside the selection"
                );
                inside_count += 1;
            }
        }
    }
    assert!(inside_count > 1500);
    // The selection's own column 100 (document) is layer-local 50: painted;
    // document 99 (layer-local 49) and document 200 (local 150) are not.
    let col = |rig: &mut Rig, lx: u32| (0..300).any(|ly| rig.at(lx, ly)[3] > 0.0);
    assert!(col(&mut rig, 50) && col(&mut rig, 149));
    assert!(!col(&mut rig, 49) && !col(&mut rig, 150));
}

#[test]
fn the_eraser_the_bucket_and_the_gradient_respect_the_selection_too() {
    // Eraser.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [0.5, 0.5, 0.5, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    rig.settings.set_radius(Tool::Eraser, 40.0);
    assert!(rig.stroke(Tool::Eraser, &[(50.0, 175.0), (320.0, 175.0)], false));
    let (mut erased_in, mut erased_out) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] < 1.0 {
                if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
                    erased_in += 1;
                } else {
                    erased_out += 1;
                }
            }
        }
    }
    assert!(erased_in > 2000, "{erased_in}");
    assert_eq!(erased_out, 0, "the eraser never reaches outside");
    assert_eq!(rig.undo_order.undo_labels, ["Eraser Stroke"]);

    // Bucket: a click inside fills only the selected part of the region.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [1.0, 1.0, 1.0, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    assert!(rig.bucket((120.0, 120.0)));
    assert_eq!(rig.at(120, 120), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(rig.at(249, 249), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(rig.at(250, 249), [1.0; 4], "right edge is outside");
    assert_eq!(rig.at(249, 250), [1.0; 4], "bottom edge is outside");
    assert_eq!(rig.at(99, 120), [1.0; 4]);
    assert_eq!(rig.at(500, 300), [1.0; 4]);
    assert_eq!(rig.undo_order.undo_labels, ["Paint Bucket Fill"]);
    assert!(rig.undo(UndoDirection::Undo));
    assert_eq!(rig.at(120, 120), [1.0; 4]);

    // A click outside the selection does nothing and records nothing.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(0, 0, 600, 400, [1.0, 1.0, 1.0, 1.0]);
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    let before = rig.snapshot(600, 400);
    assert!(!rig.bucket((400.0, 300.0)));
    assert_eq!(rig.snapshot(600, 400), before);
    assert!(rig.undo_order.undo.is_empty(), "no history row");

    // Gradient: evaluated as always, painted only through the selection.
    let mut plain = Rig::new(rect(0, 0, 600, 400));
    assert!(plain.gradient((0.0, 0.0), (500.0, 0.0)));
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    assert!(rig.gradient((0.0, 0.0), (500.0, 0.0)));
    for (x, y) in [
        (100, 100),
        (249, 249),
        (180, 200),
        (99, 100),
        (250, 100),
        (100, 250),
        (50, 50),
    ] {
        let want = if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
            plain.at(x, y)
        } else {
            [0.0; 4]
        };
        assert_eq!(rig.at(x, y), want, "({x}, {y})");
    }
    assert_eq!(rig.undo_order.undo_labels, ["Gradient"]);
}

#[test]
fn a_press_entirely_outside_the_selection_paints_nothing_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((300.0, 300.0), (350.0, 350.0));
    rig.settings.set_radius(Tool::Brush, 10.0);
    assert!(!rig.stroke(Tool::Brush, &[(20.0, 20.0), (120.0, 40.0)], false));
    assert!(rig.undo_order.undo.is_empty());
    assert_eq!(rig.tiles(), 0, "no tile was even created");
}

#[test]
fn no_selection_deselect_and_a_whole_canvas_selection_are_all_the_same_as_before() {
    let points = [(30.0, 40.0), (250.0, 90.0), (400.0, 300.0)];
    let paint = |rig: &mut Rig| {
        rig.settings.set_radius(Tool::Brush, 22.0);
        assert!(rig.stroke(Tool::Brush, &points, false));
        rig.settings.set_radius(Tool::Eraser, 14.0);
        assert!(rig.stroke(Tool::Eraser, &[(100.0, 60.0), (300.0, 200.0)], false));
        assert!(rig.gradient((10.0, 10.0), (500.0, 300.0)));
        assert!(rig.bucket((599.0, 399.0)));
        rig.snapshot(600, 400)
    };
    let mut none = Rig::new(rect(0, 0, 600, 400));
    let baseline = paint(&mut none);
    let mut deselected = Rig::new(rect(0, 0, 600, 400));
    deselected.marquee((100.0, 100.0), (200.0, 200.0));
    deselected.selection.deselect();
    assert!(
        paint(&mut deselected) == baseline,
        "Deselect restores painting everywhere"
    );
    let mut whole = Rig::new(rect(0, 0, 600, 400));
    whole.marquee((-50.0, -50.0), (700.0, 500.0));
    assert!(
        paint(&mut whole) == baseline,
        "a selection over the whole canvas"
    );
    let mut inverted_far = Rig::new(rect(0, 0, 600, 400));
    inverted_far.marquee((1500.0, 1500.0), (1600.0, 1600.0));
    ok(inverted_far.selection.invert());
    assert!(
        paint(&mut inverted_far) == baseline,
        "an inverted selection far away"
    );
}

#[test]
fn an_inverted_selection_paints_outside_and_not_inside() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 100.0), (250.0, 250.0));
    ok(rig.selection.invert());
    rig.settings.set_radius(Tool::Brush, 40.0);
    assert!(rig.stroke(Tool::Brush, &[(30.0, 175.0), (330.0, 175.0)], false));
    let (mut a, mut b) = (0, 0);
    for y in 0..400_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] > 0.0 {
                if inside(rect(100, 100, 150, 150), i64::from(x), i64::from(y)) {
                    b += 1;
                } else {
                    a += 1;
                }
            }
        }
    }
    assert!(a > 3000 && b == 0, "outside {a}, inside {b}");
}

#[test]
fn a_selection_partly_outside_the_layer_paints_the_overlap_only() {
    // Layer at (100, 100), 200 x 200; the selection runs off its top-left.
    let mut rig = Rig::new(rect(100, 100, 200, 200));
    rig.marquee((20.0, 30.0), (180.0, 170.0));
    rig.settings.set_radius(Tool::Brush, 60.0);
    assert!(rig.stroke(Tool::Brush, &[(110.0, 110.0), (170.0, 160.0)], false));
    for ly in 0..200_u32 {
        for lx in 0..200_u32 {
            let doc = (i64::from(lx) + 100, i64::from(ly) + 100);
            if rig.at(lx, ly)[3] > 0.0 {
                assert!(inside(rect(20, 30, 160, 140), doc.0, doc.1), "{doc:?}");
            }
        }
    }
    assert!(rig.at(10, 10)[3] > 0.0);
    // A fully offset selection makes nothing at all.
    let mut rig = Rig::new(rect(100, 100, 200, 200));
    rig.marquee((500.0, 500.0), (600.0, 600.0));
    assert!(!rig.stroke(Tool::Brush, &[(150.0, 150.0), (250.0, 250.0)], false));
    assert_eq!(rig.tiles(), 0);
}

#[test]
fn strokes_across_tile_boundaries_stay_inside_and_capture_only_reached_tiles() {
    let mut rig = Rig::new(rect(0, 0, 600, 600));
    // Selection straddling the 256 boundary on both axes.
    rig.marquee((200.0, 200.0), (300.0, 300.0));
    rig.settings.set_radius(Tool::Brush, 50.0);
    assert!(rig.stroke(Tool::Brush, &[(250.0, 250.0), (262.0, 262.0)], false));
    assert_eq!(rig.tiles(), 4);
    for y in 0..600_u32 {
        for x in 0..600_u32 {
            if rig.at(x, y)[3] > 0.0 {
                assert!(inside(rect(200, 200, 100, 100), i64::from(x), i64::from(y)));
            }
        }
    }
    // A selection that reaches only tile (0,0): the others never appear.
    let mut rig = Rig::new(rect(0, 0, 600, 600));
    rig.marquee((200.0, 200.0), (255.0, 255.0));
    assert!(rig.stroke(Tool::Brush, &[(250.0, 250.0), (252.0, 252.0)], false));
    assert_eq!(rig.tiles(), 1, "no phantom tiles outside the selection");
}

#[test]
fn transparency_lock_and_the_selection_compose() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.rect_fill(0, 0, 400, 300, [0.3, 0.3, 0.3, 0.6]);
    let before = rig.snapshot(400, 300);
    rig.marquee((100.0, 80.0), (200.0, 180.0));
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(50.0, 130.0), (260.0, 130.0)], true));
    let after = rig.snapshot(400, 300);
    let mut recoloured = 0;
    for y in 0..300_usize {
        for x in 0..400_usize {
            let (b, a) = (before[y * 400 + x], after[y * 400 + x]);
            assert_eq!(a[3], b[3], "alpha is locked ({x}, {y})");
            let selected = inside(rect(100, 80, 100, 100), x as i64, y as i64);
            if !selected {
                assert_eq!(
                    a, b,
                    "outside the selection nothing is recoloured ({x}, {y})"
                );
            } else if a != b {
                recoloured += 1;
            }
        }
    }
    assert!(recoloured > 1000, "{recoloured}");
}

// ---- 0.213.0: elliptical selections and undoable selection steps -------

use crate::AppCommand;
use crate::selection_view::{self, SelectionCommand};
use aurora_doc::{Selection, SelectionShape};
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers};

impl Rig {
    /// A Marquee press as the app runs it: the press deselects and the new
    /// drag remembers what was selected.
    fn press_marquee(&mut self, tool: Tool, from: (f32, f32)) -> Option<Drag> {
        self.press_marquee_mode(tool, from, aurora_doc::SelectionMode::Replace)
    }

    /// The same in a selection mode (0.215.0), through the production
    /// helper `App::handle_pointer_pressed` calls.
    fn press_marquee_mode(
        &mut self,
        tool: Tool,
        from: (f32, f32),
        mode: aurora_doc::SelectionMode,
    ) -> Option<Drag> {
        let mut drag = begin_drag(tool, PointerButton::Primary, from, &self.view, None);
        crate::selection_modes::press_marquee_drag(&mut drag, &mut self.selection, mode);
        drag
    }

    fn drag_to(&mut self, drag: &mut Option<Drag>, to: (f32, f32)) {
        let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
        if let Some(d) = drag.as_mut() {
            let _ = continue_drag(
                d,
                to,
                &mut self.view,
                &mut self.selection,
                &self.settings,
                bounds,
            );
        }
    }

    /// The release (or any other end) of a drag, as `App::commit_drag`.
    fn end_drag(&mut self, drag: Option<Drag>) -> bool {
        crate::commit_drag_into_history(
            &mut self.workspace,
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        )
    }

    /// A whole Marquee gesture. Returns whether a step was recorded.
    fn marquee_step(&mut self, tool: Tool, from: (f32, f32), to: (f32, f32)) -> bool {
        let mut drag = self.press_marquee(tool, from);
        self.drag_to(&mut drag, to);
        self.end_drag(drag)
    }

    fn command_step(&mut self, command: SelectionCommand) -> bool {
        crate::run_selection_step(
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
            command,
            (400, 300),
        )
    }

    /// One Ctrl+Z or Ctrl+Shift+Z as `App::run_undo_redo` runs it.
    fn step(&mut self, direction: UndoDirection) -> bool {
        let stepped = self.undo(direction);
        selection_view::apply_pending_selection(&mut self.undo_order, &mut self.selection);
        stepped
    }

    fn labels(&self) -> Vec<String> {
        self.undo_order
            .history_steps()
            .iter()
            .map(|s| format!("{}{}", s.label, if s.undone { " (undone)" } else { "" }))
            .collect()
    }

    fn alpha(&mut self, x: u32, y: u32) -> f32 {
        self.at(x, y)[3]
    }
}

#[test]
fn the_elliptical_marquee_tool_draws_an_ellipse_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (50.0, 40.0), (150.0, 100.0)));
    let Some(s) = rig.selection.active() else {
        unreachable!("selected");
    };
    assert!(matches!(s.shape, SelectionShape::Ellipse));
    assert_eq!(s.bounds, rect(50, 40, 100, 60));
    // The rectangular tool still makes rectangles.
    assert!(rig.marquee_step(Tool::MarqueeSelect, (50.0, 40.0), (150.0, 100.0)));
    assert!(matches!(
        rig.selection.active().map(|s| s.shape),
        Some(SelectionShape::Rect)
    ));
    assert_eq!(rig.labels(), ["Elliptical Marquee", "Rectangular Marquee"]);
}

#[test]
fn an_ellipse_selection_paints_partial_values_on_its_edge_and_nothing_outside() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (50.0, 40.0), (150.0, 100.0)));
    assert!(rig.bucket((100.0, 70.0)), "the fill is one undo step");
    let selection = ok_selection(&rig);
    let (mut partial, mut full) = (0, 0);
    for y in 0..300_u32 {
        for x in 0..400_u32 {
            let expected = selection.coverage_at(i64::from(x), i64::from(y));
            let got = rig.alpha(x, y);
            // The fill is opaque green, so alpha is exactly the coverage
            // (multiples of 1/16 are exact in f16).
            assert_eq!(got, expected, "({x}, {y})");
            if got > 0.0 && got < 1.0 {
                partial += 1;
            } else if got == 1.0 {
                full += 1;
            }
        }
    }
    assert!(partial > 40, "an antialiased edge: {partial}");
    assert!(full > 3000, "an interior: {full}");
    // Nothing in the bounds' corners nor outside.
    assert_eq!(rig.alpha(50, 40), 0.0);
    assert_eq!(rig.alpha(10, 70), 0.0);
    assert_eq!(rig.alpha(100, 70), 1.0);
}

fn ok_selection(rig: &Rig) -> Selection {
    match rig.selection.active() {
        Some(s) => s,
        None => unreachable!("a selection is active"),
    }
}

#[test]
fn a_brush_stroke_through_an_ellipse_changes_nothing_outside_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::EllipticalMarquee, (100.0, 100.0), (200.0, 160.0));
    let selection = ok_selection(&rig);
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(60.0, 130.0), (240.0, 130.0)], false));
    let mut partial = 0;
    for y in 0..300_u32 {
        for x in 0..400_u32 {
            let c = selection.coverage_at(i64::from(x), i64::from(y));
            let a = rig.alpha(x, y);
            if c == 0.0 {
                assert_eq!(a, 0.0, "outside the ellipse ({x}, {y})");
            } else {
                // Repeated dabs converge towards the full value (the
                // brush's max-alpha rule, disclosed since 0.211.0), so
                // only the bounds are exact.
                assert!((0.0..=1.0).contains(&a), "({x}, {y}): {a}");
                if a > 0.0 && a < 1.0 {
                    partial += 1;
                }
            }
        }
    }
    assert!(partial > 0, "the ellipse's edge took fractional paint");
}

#[test]
fn selection_changes_are_undoable_steps_with_their_labels() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut states = vec![rig.selection.active()];
    assert!(rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0)));
    states.push(rig.selection.active());
    assert!(rig.marquee_step(Tool::EllipticalMarquee, (20.0, 20.0), (120.0, 90.0)));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::Inverse));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::SelectAll));
    states.push(rig.selection.active());
    assert!(rig.command_step(SelectionCommand::Deselect));
    states.push(rig.selection.active());
    assert_eq!(
        rig.labels(),
        [
            "Rectangular Marquee",
            "Elliptical Marquee",
            "Inverse Selection",
            "Select All",
            "Deselect"
        ]
    );
    // Undo walks back through every state, exactly.
    for expected in states.iter().rev().skip(1) {
        assert!(rig.step(UndoDirection::Undo));
        assert_eq!(&rig.selection.active(), expected);
    }
    assert!(!rig.step(UndoDirection::Undo), "nothing left");
    // And redo forwards again.
    for expected in states.iter().skip(1) {
        assert!(rig.step(UndoDirection::Redo));
        assert_eq!(&rig.selection.active(), expected);
    }
    assert!(!rig.step(UndoDirection::Redo));
    // A new step after an undo discards the redo side, selection steps
    // included.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.command_step(SelectionCommand::Inverse));
    assert!(!rig.step(UndoDirection::Redo));
    assert!(rig.undo_order.selection_redo.is_empty());
}

#[test]
fn a_click_that_deselects_is_a_step_and_a_no_op_is_not() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(
        !rig.marquee_step(Tool::MarqueeSelect, (30.0, 30.0), (30.0, 30.0)),
        "a click over nothing changes nothing"
    );
    assert!(!rig.command_step(SelectionCommand::Deselect));
    assert!(
        !rig.command_step(SelectionCommand::Inverse),
        "nothing to invert"
    );
    rig.marquee_step(Tool::MarqueeSelect, (30.0, 30.0), (90.0, 90.0));
    assert!(rig.marquee_step(Tool::MarqueeSelect, (200.0, 200.0), (200.0, 200.0)));
    assert!(rig.selection.active().is_none());
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee", "Deselect"],
        "a click that cleared the selection reads Deselect"
    );
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
}

#[test]
fn selection_steps_interleave_with_pixel_steps_and_never_touch_the_content() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::EllipticalMarquee, (50.0, 50.0), (150.0, 120.0));
    assert!(rig.bucket((100.0, 85.0)));
    let painted = rig.snapshot(200, 150);
    rig.command_step(SelectionCommand::Deselect);
    // Undo the Deselect: the selection returns, the pixels are untouched.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
    assert_eq!(rig.snapshot(200, 150), painted);
    // Undo the fill: pixels go, the selection stays.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_some());
    assert_eq!(rig.alpha(100, 85), 0.0);
    // Undo the marquee itself.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.selection.active().is_none());
    // Redo all the way: the fill comes back through the ellipse.
    for _ in 0..3 {
        assert!(rig.step(UndoDirection::Redo));
    }
    assert_eq!(rig.snapshot(200, 150), painted);
    assert!(rig.selection.active().is_none());
}

#[test]
fn selection_steps_leave_the_content_revision_alone_and_content_steps_move_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let saved = rig.undo_order.content_revision;
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    rig.command_step(SelectionCommand::SelectAll);
    assert_eq!(rig.undo_order.content_revision, saved);
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.undo_order.content_revision, saved, "undo/redo too");
    assert!(rig.bucket((20.0, 20.0)));
    assert_ne!(rig.undo_order.content_revision, saved);
    let dirty = rig.undo_order.content_revision;
    // Undoing the selection step above the fill's neither cleans nor
    // dirties anything further.
    rig.command_step(SelectionCommand::Deselect);
    assert_eq!(rig.undo_order.content_revision, dirty);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.undo_order.content_revision, dirty);
}

#[test]
fn a_history_panel_jump_applies_the_selection_of_the_row() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    rig.marquee_step(Tool::EllipticalMarquee, (20.0, 20.0), (120.0, 90.0));
    let second = rig.selection.active();
    rig.command_step(SelectionCommand::Inverse);
    let third = rig.selection.active();
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut drag = None;
    let mut focus = aurora_widgets::FocusManager::default();
    for (target, expected) in [(1, &first), (0, &None), (3, &third), (2, &second)] {
        let _ = crate::perform_history_jump(
            &mut crate::HistoryJumpContext {
                workspace: &mut rig.workspace,
                focus: &mut focus,
                scales: &scales,
                layers: &mut rig.layers,
                history: &mut rig.history,
                pixel_history: &mut rig.pixel_history,
                store: Some(&mut rig.store),
                undo_order: &mut rig.undo_order,
                composite_cache: &mut cache,
                view: &mut rig.view,
                layer_rows: &mut layer_rows,
                active_layer: &mut active,
                drag: &mut drag,
                layer_controls: &mut state,
            },
            target,
        );
        selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
        assert_eq!(&rig.selection.active(), expected, "jump to row {target}");
        assert_eq!(rig.undo_order.undo.len(), target);
    }
}

#[test]
fn the_elliptical_marquee_is_wired_to_shift_m_the_palette_and_the_tool_table() {
    assert_eq!(Tool::ALL.len(), 15);
    let table = crate::default_shortcuts_for(false);
    let chord = |shift| {
        KeyChord::new(
            Modifiers {
                shift,
                ..Modifiers::none()
            },
            Key::Character('m'),
        )
    };
    assert_eq!(
        table.resolve(chord(true)),
        Some(&AppCommand::SelectTool(Tool::EllipticalMarquee))
    );
    assert_eq!(
        table.resolve(chord(false)),
        Some(&AppCommand::SelectTool(Tool::MarqueeSelect))
    );
    // Every tool's documented shortcut resolves to that tool.
    for tool in Tool::ALL {
        let c = KeyChord::new(
            Modifiers {
                shift: tool.shortcut_needs_shift(),
                ..Modifiers::none()
            },
            Key::Character(tool.shortcut_key().to_ascii_lowercase()),
        );
        assert_eq!(
            table.resolve(c),
            Some(&AppCommand::SelectTool(tool)),
            "{tool:?}"
        );
    }
    let commands = crate::palette_commands();
    let Some(entry) = commands
        .iter()
        .find(|c| c.title == "Select Elliptical Marquee Tool")
    else {
        unreachable!("the palette lists the tool");
    };
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::default();
    let activated = crate::activate_command(
        &mut workspace,
        &mut focus,
        &entry.id,
        &mut crate::tests::FakeFileDialog::default(),
    );
    assert!(
        matches!(
            activated,
            Some(crate::ActivatedCommand::SelectTool(Tool::EllipticalMarquee))
        ),
        "{activated:?}"
    );
    // The tools panel has one button per tool, the new one wearing its
    // own icon.
    assert_eq!(workspace.tools.buttons.len(), 15);
    assert_eq!(
        aurora_ui::tool_icon(Tool::EllipticalMarquee),
        aurora_widgets::icons::IconKind::EllipseMarquee
    );
}

#[test]
fn escape_during_an_elliptical_drag_restores_the_selection_without_a_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let steps = rig.undo_order.undo.len();
    let before = selection_view::press_marquee(&mut rig.selection);
    let mut drag = begin_drag(
        Tool::EllipticalMarquee,
        PointerButton::Primary,
        (100.0, 100.0),
        &rig.view,
        None,
    );
    let bounds = pan_bounds(&rig.layers, Some(rig.layer), Some((1000.0, 800.0)));
    if let Some(d) = drag.as_mut() {
        let _ = continue_drag(
            d,
            (180.0, 160.0),
            &mut rig.view,
            &mut rig.selection,
            &rig.settings,
            bounds,
        );
    }
    assert!(matches!(
        rig.selection.active().map(|s| s.shape),
        Some(SelectionShape::Ellipse)
    ));
    selection_view::cancel_marquee(&mut rig.selection, before);
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.undo_order.undo.len(), steps, "no step was recorded");
}

/// 0.213.0 review: a Marquee drag that is still live when something else
/// ends it is recorded as its selection step by every path (the rule is
/// "record", not "cancel"; only Escape and a selection command cancel), so
/// the undo that follows undoes the drag's selection, never an older one.
#[test]
fn ctrl_z_during_a_live_marquee_drag_undoes_that_drags_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    // A new drag is live (pressed, moved) when Ctrl+Z arrives.
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    let drawn = rig.selection.active();
    assert!(matches!(
        drawn.as_ref().map(|s| &s.shape),
        Some(SelectionShape::Ellipse)
    ));
    let mut tool = Tool::EllipticalMarquee;
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    let mut cache = CompositeCache::default();
    let mut controls = crate::LayerControlsState::default();
    let _ = crate::perform_undo_redo(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        &mut tool,
        &rig.settings,
        &mut rig.layers,
        &mut rig.history,
        &mut rig.pixel_history,
        Some(&mut rig.store),
        &mut rig.undo_order,
        &mut cache,
        &mut rig.view,
        Some(rig.layer),
        &mut drag,
        &mut controls,
        AppCommand::Undo,
    );
    assert!(drag.is_none(), "the drag was ended");
    selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
    // The drag's own step was recorded and then undone: back to the first
    // selection, and redo brings the drawn ellipse back.
    assert_eq!(rig.selection.active(), first);
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee", "Elliptical Marquee (undone)"]
    );
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), drawn);
}

#[test]
fn a_history_jump_during_a_live_marquee_drag_records_it_first() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    rig.command_step(SelectionCommand::Inverse);
    let first = {
        // The state after the first row.
        let mut s = Selection::new(rect(10, 10, 50, 40));
        s.inverted = false;
        Some(s)
    };
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut focus = aurora_widgets::FocusManager::default();
    let _ = crate::perform_history_jump(
        &mut crate::HistoryJumpContext {
            workspace: &mut rig.workspace,
            focus: &mut focus,
            scales: &scales,
            layers: &mut rig.layers,
            history: &mut rig.history,
            pixel_history: &mut rig.pixel_history,
            store: Some(&mut rig.store),
            undo_order: &mut rig.undo_order,
            composite_cache: &mut cache,
            view: &mut rig.view,
            layer_rows: &mut layer_rows,
            active_layer: &mut active,
            drag: &mut drag,
            layer_controls: &mut state,
        },
        1,
    );
    selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
    assert!(drag.is_none());
    assert_eq!(rig.selection.active(), first);
    // The live drag became a third step before the jump, and the jump kept
    // everything else redoable.
    assert_eq!(
        rig.labels(),
        [
            "Rectangular Marquee",
            "Inverse Selection (undone)",
            "Elliptical Marquee (undone)"
        ]
    );
}

#[test]
fn a_selection_command_cancels_a_live_marquee_drag_back_to_what_it_replaced() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let mut drag = rig.press_marquee(Tool::EllipticalMarquee, (100.0, 100.0));
    rig.drag_to(&mut drag, (180.0, 160.0));
    // What `App::run_selection_command` does with a live Marquee drag.
    if let Some(Drag::Marquee { before, .. }) = drag.take() {
        selection_view::cancel_marquee(&mut rig.selection, before);
    }
    assert_eq!(rig.selection.active(), existing);
    assert!(rig.command_step(SelectionCommand::Inverse));
    assert_eq!(rig.labels(), ["Rectangular Marquee", "Inverse Selection"]);
}

// ---- 0.214.0: Lasso, Polygonal Lasso and Magic Wand --------------------

use crate::lasso_tools::{self, PolygonClick, WandRefused};

const SQUARE: [(f32, f32); 4] = [(100.0, 80.0), (200.0, 80.0), (200.0, 180.0), (100.0, 180.0)];

impl Rig {
    /// A Lasso press as the app runs it.
    fn press_lasso(&mut self, tool: Tool, from: (f32, f32)) -> Option<Drag> {
        self.press_lasso_mode(tool, from, aurora_doc::SelectionMode::Replace)
    }

    fn press_lasso_mode(
        &mut self,
        tool: Tool,
        from: (f32, f32),
        mode: aurora_doc::SelectionMode,
    ) -> Option<Drag> {
        let mut drag = begin_drag(tool, PointerButton::Primary, from, &self.view, None);
        lasso_tools::press_lasso(&mut drag, &mut self.selection, (400, 300), mode);
        drag
    }

    /// A whole freehand Lasso gesture: press, moves, release. Whether a
    /// step was recorded.
    fn lasso(&mut self, points: &[(f32, f32)]) -> bool {
        let Some((&first, rest)) = points.split_first() else {
            unreachable!("a lasso has points");
        };
        let mut drag = self.press_lasso(Tool::Lasso, first);
        for &p in rest {
            self.drag_to(&mut drag, p);
        }
        let mut slot = None;
        let ending = lasso_tools::release_lasso(&mut slot, drag, &mut self.selection);
        assert!(slot.is_none(), "a freehand lasso ends on release");
        self.end_drag(ending)
    }

    /// One click of the Polygonal Lasso: the first starts the gesture, the
    /// rest add vertices or close it. Whether it closed (and so recorded).
    fn poly_click(&mut self, drag: &mut Option<Drag>, at: (f32, f32)) -> bool {
        if let Some(d) = drag.as_mut() {
            if lasso_tools::polygon_click(d, at, &self.view, &mut self.selection)
                == PolygonClick::Closes
            {
                let ending = drag.take();
                return self.end_drag(ending);
            }
            return false;
        }
        *drag = self.press_lasso(Tool::PolygonalLasso, at);
        // The click's release leaves the polygon alive.
        let ending = drag.take();
        let ending = lasso_tools::release_lasso(drag, ending, &mut self.selection);
        assert!(ending.is_none());
        false
    }

    fn wand(&mut self, point: (f32, f32)) -> Result<bool, WandRefused> {
        self.wand_mode(point, aurora_doc::SelectionMode::Replace)
    }

    fn wand_mode(
        &mut self,
        point: (f32, f32),
        mode: aurora_doc::SelectionMode,
    ) -> Result<bool, WandRefused> {
        lasso_tools::wand_step(
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
            &mut self.store,
            &self.layers,
            self.layer,
            point,
            &self.settings,
            (400, 300),
            mode,
        )
    }

    fn selected_bounds(&self) -> Option<Rect> {
        self.selection.active().map(|s| s.bounds)
    }
}

fn mask_of_selection(rig: &Rig) -> Selection {
    let selection = ok_selection(rig);
    assert!(matches!(selection.shape, SelectionShape::Mask(_)));
    selection
}

#[test]
fn a_lasso_drag_is_one_labelled_mask_step_that_undoes_and_redoes() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.lasso(&[(100.0, 80.0), (200.0, 90.0), (180.0, 180.0), (90.0, 150.0)]));
    let drawn = rig.selection.active();
    let selection = mask_of_selection(&rig);
    // Tight: the box of the drawn quadrilateral.
    assert_eq!(selection.bounds, rect(90, 80, 110, 100));
    assert_eq!(rig.labels(), ["Lasso"]);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), drawn);
    assert_eq!(rig.labels(), ["Lasso"]);
}

#[test]
fn a_lasso_click_or_a_degenerate_path_deselects_and_a_no_op_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // Nothing selected, a click: nothing changed, nothing recorded.
    assert!(!rig.lasso(&[(50.0, 50.0)]));
    assert!(rig.lasso(&SQUARE));
    // A straight line selects nothing: it deselects, as a step.
    assert!(rig.lasso(&[(10.0, 10.0), (60.0, 60.0), (110.0, 110.0)]));
    assert_eq!(rig.selection.active(), None);
    assert_eq!(rig.labels(), ["Lasso", "Deselect"]);
}

#[test]
fn the_lasso_mask_is_clipped_to_the_document() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.lasso(&[
        (-50.0, -50.0),
        (100.0, -50.0),
        (100.0, 100.0),
        (-50.0, 100.0)
    ]));
    assert_eq!(rig.selected_bounds(), Some(rect(0, 0, 100, 100)));
}

#[test]
fn a_brush_through_a_lasso_paints_fractional_edges_and_nothing_outside() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // A right triangle: its hypotenuse crosses row 150 at x = 200.
    assert!(rig.lasso(&[(100.0, 100.0), (300.0, 100.0), (100.0, 200.0)]));
    rig.settings.set_radius(Tool::Brush, 40.0);
    assert!(rig.stroke(Tool::Brush, &[(40.0, 150.0), (320.0, 150.0)], false));
    let full = rig.at(130, 150)[3];
    assert!(full > 0.5, "inside is painted: {full}");
    assert_eq!(rig.at(60, 150)[3], 0.0, "left of the selection");
    let (mut partial, mut beyond) = (0, 0);
    for x in 150..320_u32 {
        let a = rig.at(x, 150)[3];
        if a > 0.0 && a < full - 0.05 {
            partial += 1;
        }
        if x > 203 && a > 0.0 {
            beyond += 1;
        }
    }
    assert!(partial >= 1, "an antialiased edge paints fractions");
    assert_eq!(beyond, 0, "nothing past the hypotenuse");
    assert_eq!(rig.at(150, 90)[3], 0.0, "above the top edge");
}

#[test]
fn a_figure_eight_lasso_selects_both_lobes() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.lasso(&[
        (100.0, 100.0),
        (200.0, 200.0),
        (200.0, 100.0),
        (100.0, 200.0)
    ]));
    let s = mask_of_selection(&rig);
    assert_eq!(s.coverage_at(110, 150), 1.0, "left lobe");
    assert_eq!(s.coverage_at(190, 150), 1.0, "right lobe");
    assert_eq!(s.coverage_at(150, 110), 0.0, "between the lobes, above");
}

#[test]
fn a_polygon_closes_on_the_first_vertex_and_is_one_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = None;
    for p in SQUARE {
        assert!(!rig.poly_click(&mut drag, p));
    }
    // Live while building: the closed polygon so far.
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 100)));
    assert!(matches!(
        drag,
        Some(Drag::Lasso {
            polygonal: true,
            ..
        })
    ));
    // A click on the first vertex closes it.
    assert!(rig.poly_click(&mut drag, (100.0, 80.0)));
    assert!(drag.is_none());
    assert_eq!(rig.labels(), ["Polygonal Lasso"]);
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 100)));
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
}

#[test]
fn the_close_tolerance_is_six_screen_pixels_and_a_double_click_closes() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = None;
    for p in SQUARE {
        rig.poly_click(&mut drag, p);
    }
    // 7 px from the first vertex: another vertex, not a close.
    assert!(!rig.poly_click(&mut drag, (107.0, 80.0)));
    assert!(drag.is_some());
    // 5 px from the first vertex closes.
    assert!(rig.poly_click(&mut drag, (105.0, 80.0)));

    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = None;
    for p in [SQUARE[0], SQUARE[1], SQUARE[2]] {
        rig.poly_click(&mut drag, p);
    }
    // The second click of a double click lands on the last vertex.
    assert!(rig.poly_click(&mut drag, (201.0, 181.0)));
    assert_eq!(rig.labels(), ["Polygonal Lasso"]);
    // Two vertices: a click on the first one does not close (a polygon
    // needs three).
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = None;
    rig.poly_click(&mut drag, SQUARE[0]);
    rig.poly_click(&mut drag, SQUARE[1]);
    assert!(!rig.poly_click(&mut drag, SQUARE[0]));
    assert!(drag.is_some());
}

#[test]
fn backspace_removes_the_last_vertex_and_the_only_vertex_cancels() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let existing = {
        rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
        rig.selection.active()
    };
    let mut drag = None;
    for p in SQUARE {
        rig.poly_click(&mut drag, p);
    }
    let four = rig.selection.active();
    let Some(d) = drag.as_mut() else {
        unreachable!("building");
    };
    assert!(!lasso_tools::polygon_backspace(d, &mut rig.selection));
    // The triangle of the first three vertices: not the square, and the
    // first vertex (100, 80) is still in it.
    let three = rig.selection.active();
    assert_ne!(three, four);
    let Some(three) = three else {
        unreachable!("three vertices select");
    };
    assert_eq!(three.bounds, rect(100, 80, 100, 100));
    assert_eq!(three.coverage_at(190, 100), 1.0, "inside the triangle");
    assert_eq!(three.coverage_at(110, 170), 0.0, "the dropped corner");
    assert!(!lasso_tools::polygon_backspace(d, &mut rig.selection));
    // Two vertices left: nothing selected yet.
    assert_eq!(rig.selection.active(), None);
    // The last vertex goes: the gesture is over and cancels.
    assert!(!lasso_tools::polygon_backspace(d, &mut rig.selection));
    assert!(lasso_tools::polygon_backspace(d, &mut rig.selection));
    if let Some(Drag::Lasso { before, .. }) = drag.take() {
        selection_view::cancel_marquee(&mut rig.selection, before);
    }
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.labels(), ["Rectangular Marquee"], "no step recorded");
}

#[test]
fn escape_during_a_polygon_restores_the_old_selection_without_a_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let mut drag = None;
    for p in SQUARE {
        rig.poly_click(&mut drag, p);
    }
    assert_ne!(rig.selection.active(), existing);
    if let Some(Drag::Lasso { before, .. }) = drag.take() {
        selection_view::cancel_marquee(&mut rig.selection, before);
    }
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.labels(), ["Rectangular Marquee"]);
}

#[test]
fn ctrl_z_during_a_live_polygon_records_it_then_undoes_it() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    let mut drag = None;
    for p in SQUARE {
        rig.poly_click(&mut drag, p);
    }
    let drawn = rig.selection.active();
    let mut tool = Tool::PolygonalLasso;
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    let mut cache = CompositeCache::default();
    let mut controls = crate::LayerControlsState::default();
    let _ = crate::perform_undo_redo(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        &mut tool,
        &rig.settings,
        &mut rig.layers,
        &mut rig.history,
        &mut rig.pixel_history,
        Some(&mut rig.store),
        &mut rig.undo_order,
        &mut cache,
        &mut rig.view,
        Some(rig.layer),
        &mut drag,
        &mut controls,
        AppCommand::Undo,
    );
    assert!(drag.is_none());
    selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
    assert_eq!(rig.selection.active(), first);
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee", "Polygonal Lasso (undone)"]
    );
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), drawn);
}

#[test]
fn an_interrupted_freehand_lasso_keeps_what_was_built_as_its_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = rig.press_lasso(Tool::Lasso, (100.0, 80.0));
    for p in [(200.0, 80.0), (200.0, 180.0), (100.0, 180.0)] {
        rig.drag_to(&mut drag, p);
    }
    let built = rig.selection.active();
    assert!(built.is_some());
    // A second press (or anything) takes the drag: no release rebuild.
    assert!(rig.end_drag(drag));
    assert_eq!(rig.selection.active(), built);
    assert_eq!(rig.labels(), ["Lasso"]);
}

#[test]
fn lasso_paths_are_thinned_and_capped() {
    let mut points = vec![(0.0_f32, 0.0_f32)];
    assert!(
        !lasso_tools::push_lasso_point(&mut points, (0.5, 0.5)),
        "too close"
    );
    assert!(lasso_tools::push_lasso_point(&mut points, (3.0, 0.0)));
    assert!(!lasso_tools::push_lasso_point(&mut points, (f32::NAN, 0.0)));
    let mut full: Vec<(f32, f32)> = (0..lasso_tools::MAX_LASSO_POINTS)
        .map(|i| (i as f32 * 2.0, 0.0))
        .collect();
    assert!(
        !lasso_tools::push_lasso_point(&mut full, (1.0e7, 5.0)),
        "full"
    );
    // A small path rebuilds on every point; a big one on every nth.
    assert_eq!(
        lasso_tools::rebuild_period(&[(0.0, 0.0), (100.0, 100.0)]),
        1
    );
    assert_eq!(
        lasso_tools::rebuild_period(&[(0.0, 0.0), (1000.0, 1000.0)]),
        1 + 3
    );
    assert!(lasso_tools::rebuild_period(&[(0.0, 0.0), (8000.0, 8000.0)]) > 200);
    // A long scribble in a small box thins out by its point count too.
    assert_eq!(lasso_tools::rebuild_period(&full), 1 + 50_000 / 512);
}

#[test]
fn a_path_over_the_mask_cap_leaves_the_selection_as_it_was() {
    let clip = Rect {
        x: 0,
        y: 0,
        width: 10_000,
        height: 10_000,
    };
    let mut selection = aurora_doc::SelectionSet::new();
    selection.select(Selection::new(rect(5, 5, 10, 10)));
    let mut current = selection.active();
    let before = selection.active();
    lasso_tools::rebuild_live(
        &[(0.0, 0.0), (9999.0, 0.0), (9999.0, 9999.0), (0.0, 9999.0)],
        &clip,
        &crate::selection_modes::ModeGesture::replace(),
        &mut selection,
        &mut current,
    );
    assert_eq!(selection.active(), before);
}

// ---- the Magic Wand ----

fn wand_rig() -> Rig {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.rect_fill(200, 100, 300, 180, [1.0, 0.0, 0.0, 1.0]);
    rig.rect_fill(40, 40, 60, 60, [1.0, 0.0, 0.0, 1.0]);
    rig
}

#[test]
fn the_wand_selects_the_similar_region_as_a_softened_mask_in_one_step() {
    let mut rig = wand_rig();
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    let s = mask_of_selection(&rig);
    // The red rectangle grown by the one-texel softening; the far red
    // square is not contiguous.
    assert_eq!(s.bounds, rect(199, 99, 102, 82));
    assert_eq!(s.coverage_at(250, 140), 1.0);
    assert!((s.coverage_at(199, 140) - 0.25).abs() < 0.01);
    assert_eq!(s.coverage_at(150, 140), 0.0);
    assert_eq!(s.coverage_at(50, 50), 0.0);
    assert_eq!(rig.labels(), ["Magic Wand"]);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), None);
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selected_bounds(), Some(rect(199, 99, 102, 82)));
}

#[test]
fn non_contiguous_takes_every_similar_region_and_tolerance_widens_the_match() {
    let mut rig = wand_rig();
    rig.settings.contiguous = false;
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    assert_eq!(rig.selected_bounds(), Some(rect(39, 39, 262, 142)));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(50, 50), 1.0);
    // A slightly different red joins the click's region only within the
    // tolerance.
    let mut rig = wand_rig();
    rig.rect_fill(300, 100, 320, 180, [0.9, 0.0, 0.0, 1.0]);
    rig.settings.set_tolerance(0.0);
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    assert_eq!(rig.selected_bounds(), Some(rect(199, 99, 102, 82)));
    rig.settings.set_tolerance(40.0);
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    assert_eq!(rig.selected_bounds(), Some(rect(199, 99, 122, 82)));
}

#[test]
fn the_wand_works_in_document_space_for_a_layer_with_an_origin() {
    let mut rig = Rig::new(rect(50, 40, 200, 150));
    rig.rect_fill(10, 10, 30, 30, [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(rig.wand((70.0, 60.0)), Ok(true));
    assert_eq!(rig.selected_bounds(), Some(rect(59, 49, 22, 22)));
}

#[test]
fn a_wand_click_outside_the_layer_or_on_a_group_selects_nothing() {
    let mut rig = Rig::new(rect(50, 40, 200, 150));
    // Left of and above the layer, and past its far edge.
    assert_eq!(rig.wand((10.0, 10.0)), Err(WandRefused::Nothing));
    assert_eq!(rig.wand((400.0, 100.0)), Err(WandRefused::Nothing));
    assert_eq!(rig.selection.active(), None);
    assert!(rig.labels().is_empty());
    let group = ok(rig.layers.add_group("Group", None));
    rig.layer = group;
    assert_eq!(
        rig.wand((60.0, 60.0)),
        Err(WandRefused::Notice(crate::LockRefusal::WandNeedsPixelLayer))
    );
    assert!(rig.labels().is_empty());
}

#[test]
fn a_wand_region_over_the_cap_is_refused_with_a_notice_and_changes_nothing() {
    // A blank layer is one region: 2 x 2 tiles of 256 is within the caps,
    // so use a layer that is wider than 64 Mpx / 1024 tiles allow.
    let mut rig = Rig::new(rect(0, 0, 16_384, 8_192));
    assert_eq!(
        rig.wand((10.0, 10.0)),
        Err(WandRefused::Notice(crate::LockRefusal::SelectionTooLarge))
    );
    assert_eq!(rig.selection.active(), None);
    assert!(rig.labels().is_empty());
}

#[test]
fn a_brush_through_a_wand_mask_paints_the_softened_edge_fractionally() {
    let mut rig = wand_rig();
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    rig.settings.set_radius(Tool::Brush, 30.0);
    assert!(rig.stroke(Tool::Brush, &[(150.0, 140.0), (350.0, 140.0)], false));
    let inside = rig.at(250, 140)[3];
    assert!(inside > 0.5);
    assert_eq!(rig.at(150, 140)[3], 0.0, "outside the mask");
    // The outer softened texel (coverage 1/4) is painted, but less than the
    // inside (repeated dabs converge towards the full value, so the exact
    // fraction is not asserted).
    let outer = rig.at(199, 140)[3];
    assert!(outer > 0.0 && outer < inside, "{outer} {inside}");
}

#[test]
fn the_new_tools_are_wired_to_their_shortcuts_the_palette_and_the_panel() {
    let table = crate::default_shortcuts_for(false);
    let key = |c, shift| {
        table.resolve(KeyChord::new(
            Modifiers {
                shift,
                ..Modifiers::none()
            },
            Key::Character(c),
        ))
    };
    assert_eq!(key('l', false), Some(&AppCommand::SelectTool(Tool::Lasso)));
    assert_eq!(
        key('l', true),
        Some(&AppCommand::SelectTool(Tool::PolygonalLasso))
    );
    assert_eq!(
        key('w', false),
        Some(&AppCommand::SelectTool(Tool::MagicWand))
    );
    let commands = crate::palette_commands();
    for title in [
        "Select Lasso Tool",
        "Select Polygonal Lasso Tool",
        "Select Magic Wand Tool",
    ] {
        assert!(commands.iter().any(|c| c.title == title), "{title}");
    }
    // Each tool starts the right drag (the wand and the polygon start
    // theirs on a press).
    let view = CanvasView::new();
    for (tool, polygonal) in [(Tool::Lasso, false), (Tool::PolygonalLasso, true)] {
        let drag = begin_drag(tool, PointerButton::Primary, (5.0, 6.0), &view, None);
        assert!(matches!(
            drag,
            Some(Drag::Lasso { polygonal: p, ref points, .. }) if p == polygonal && points == &[(5.0, 6.0)]
        ));
    }
    assert!(
        begin_drag(
            Tool::MagicWand,
            PointerButton::Primary,
            (5.0, 6.0),
            &view,
            None
        )
        .is_none()
    );
}

#[test]
fn lasso_labels_name_the_steps_and_a_scroll_clamp_moves_every_point() {
    assert_eq!(lasso_tools::lasso_step_label(false, true), "Lasso");
    assert_eq!(lasso_tools::lasso_step_label(true, true), "Polygonal Lasso");
    assert_eq!(lasso_tools::lasso_step_label(true, false), "Deselect");
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = rig.press_lasso(Tool::Lasso, (10.0, 10.0));
    rig.drag_to(&mut drag, (50.0, 10.0));
    let Some(d) = drag.as_mut() else {
        unreachable!("live");
    };
    crate::shift_drag_reference(d, (5.0, -2.0));
    assert!(matches!(
        d,
        Drag::Lasso { points, .. } if points == &[(15.0, 8.0), (55.0, 8.0)]
    ));
    assert_eq!(crate::drag_reference(d), Some((15.0, 8.0)));
}

#[test]
fn a_big_lasso_rebuilds_on_a_cadence_but_the_release_is_always_exact() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = begin_drag(
        Tool::Lasso,
        PointerButton::Primary,
        (0.0, 0.0),
        &rig.view,
        None,
    );
    lasso_tools::press_lasso(
        &mut drag,
        &mut rig.selection,
        (4000, 4000),
        aurora_doc::SelectionMode::Replace,
    );
    for p in [
        (1000.0, 0.0),
        (1000.0, 1000.0),
        (0.0, 1000.0),
        (500.0, 1500.0),
    ] {
        rig.drag_to(&mut drag, p);
    }
    // A 1000 x 1000 box rebuilds every fourth point: the square is live,
    // the fifth point is not in the mask yet.
    assert_eq!(rig.selected_bounds(), Some(rect(0, 0, 1000, 1000)));
    let mut slot = None;
    let ending = lasso_tools::release_lasso(&mut slot, drag, &mut rig.selection);
    assert_eq!(rig.selected_bounds(), Some(rect(0, 0, 1000, 1500)));
    assert!(rig.end_drag(ending));
    assert_eq!(rig.labels(), ["Lasso"]);
}

#[test]
fn a_one_pixel_wand_region_stays_fully_selected() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.put(120, 90, [1.0, 0.0, 0.0, 1.0]);
    rig.settings.set_tolerance(0.0);
    assert_eq!(rig.wand((120.5, 90.5)), Ok(true));
    let s = mask_of_selection(&rig);
    assert_eq!(s.bounds, rect(119, 89, 3, 3));
    assert_eq!(s.coverage_at(120, 90), 1.0, "the region itself is full");
    assert!(s.coverage_at(119, 90) < 0.5, "the softened rim is partial");
    assert!(s.coverage_at(119, 90) > 0.0);
}

fn building_polygon(rig: &mut Rig) -> Option<Drag> {
    let mut drag = None;
    for p in SQUARE {
        rig.poly_click(&mut drag, p);
    }
    drag
}

#[test]
fn enter_closes_a_polygon_through_the_production_helper() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = building_polygon(&mut rig);
    let lasso_tools::PolygonKey::Close(ending) = lasso_tools::polygon_key(
        &mut drag,
        Tool::PolygonalLasso,
        Key::Named(aurora_widgets::shortcut::NamedKey::Enter),
        &mut rig.selection,
    ) else {
        unreachable!("Enter closes");
    };
    assert!(drag.is_none());
    assert!(rig.end_drag(ending));
    assert_eq!(rig.labels(), ["Polygonal Lasso"]);
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 100)));
}

#[test]
fn backspace_and_escape_go_through_the_production_helper() {
    use aurora_widgets::shortcut::NamedKey;
    let key = |k| Key::Named(k);
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let mut drag = building_polygon(&mut rig);
    let three = |rig: &mut Rig, drag: &mut Option<Drag>| {
        matches!(
            lasso_tools::polygon_key(
                drag,
                Tool::PolygonalLasso,
                key(NamedKey::Backspace),
                &mut rig.selection
            ),
            lasso_tools::PolygonKey::Handled
        )
    };
    assert!(three(&mut rig, &mut drag));
    assert_eq!(rig.selected_bounds(), Some(rect(100, 80, 100, 100)));
    assert!(matches!(&drag, Some(Drag::Lasso { points, .. }) if points.len() == 3));
    // Down to nothing: the last Backspace cancels the gesture.
    for _ in 0..3 {
        assert!(three(&mut rig, &mut drag));
    }
    assert!(drag.is_none());
    assert_eq!(rig.selection.active(), existing);
    // Escape cancels a fresh polygon back to the same selection.
    let mut drag = building_polygon(&mut rig);
    assert!(matches!(
        lasso_tools::polygon_key(
            &mut drag,
            Tool::PolygonalLasso,
            key(NamedKey::Escape),
            &mut rig.selection
        ),
        lasso_tools::PolygonKey::Handled
    ));
    assert!(drag.is_none());
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(
        rig.labels(),
        ["Rectangular Marquee"],
        "no step was recorded"
    );
    // Other keys, other tools and other drags are not the polygon's.
    let mut drag = building_polygon(&mut rig);
    for (tool, k) in [
        (Tool::Brush, NamedKey::Enter),
        (Tool::Lasso, NamedKey::Backspace),
        (Tool::PolygonalLasso, NamedKey::Tab),
    ] {
        assert!(matches!(
            lasso_tools::polygon_key(&mut drag, tool, key(k), &mut rig.selection),
            lasso_tools::PolygonKey::Ignored
        ));
    }
    assert!(drag.is_some());
    let mut freehand = rig.press_lasso(Tool::Lasso, (5.0, 5.0));
    assert!(matches!(
        lasso_tools::polygon_key(
            &mut freehand,
            Tool::PolygonalLasso,
            key(NamedKey::Enter),
            &mut rig.selection
        ),
        lasso_tools::PolygonKey::Ignored
    ));
}

#[test]
fn switching_tool_records_a_live_polygon_or_marquee_and_only_then() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = building_polygon(&mut rig);
    let drawn = rig.selection.active();
    // The same tool: nothing happens.
    assert!(
        lasso_tools::take_on_tool_switch(&mut drag, Tool::PolygonalLasso, Tool::PolygonalLasso)
            .is_none()
    );
    assert!(drag.is_some());
    let ending = lasso_tools::take_on_tool_switch(&mut drag, Tool::PolygonalLasso, Tool::Brush);
    assert!(drag.is_none(), "the polygon is not left stale");
    assert!(rig.end_drag(ending));
    assert_eq!(rig.labels(), ["Polygonal Lasso"]);
    assert_eq!(rig.selection.active(), drawn);
    // A Marquee gesture too; a Brush stroke is not touched.
    let mut marquee = rig.press_marquee(Tool::MarqueeSelect, (10.0, 10.0));
    assert!(
        lasso_tools::take_on_tool_switch(&mut marquee, Tool::MarqueeSelect, Tool::Zoom).is_some()
    );
    let mut eyedropper = Some(Drag::Eyedropper);
    assert!(
        lasso_tools::take_on_tool_switch(&mut eyedropper, Tool::Eyedropper, Tool::Zoom).is_none()
    );
    assert!(eyedropper.is_some());
}

#[test]
fn the_pointer_leaving_the_window_keeps_a_polygon_but_ends_a_freehand_lasso() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = building_polygon(&mut rig);
    assert!(lasso_tools::take_on_cursor_left(&mut drag).is_none());
    assert!(lasso_tools::is_polygon(drag.as_ref()));
    let mut freehand = rig.press_lasso(Tool::Lasso, (10.0, 10.0));
    assert!(lasso_tools::take_on_cursor_left(&mut freehand).is_some());
    assert!(freehand.is_none());
    let mut brush = Some(Drag::Eyedropper);
    assert!(lasso_tools::take_on_cursor_left(&mut brush).is_some());
}

#[test]
fn a_press_belongs_to_the_polygon_only_for_its_tool_and_the_primary_button() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut drag = building_polygon(&mut rig);
    let view = rig.view;
    let press = |rig: &mut Rig, drag: &mut Option<Drag>, tool, button, at| {
        lasso_tools::polygon_press(drag, tool, button, at, &view, &mut rig.selection)
    };
    assert!(matches!(
        press(
            &mut rig,
            &mut drag,
            Tool::Brush,
            PointerButton::Primary,
            (300.0, 250.0)
        ),
        lasso_tools::PolygonPress::NotApplicable
    ));
    assert!(matches!(
        press(
            &mut rig,
            &mut drag,
            Tool::PolygonalLasso,
            PointerButton::Middle,
            (300.0, 250.0)
        ),
        lasso_tools::PolygonPress::NotApplicable
    ));
    assert!(matches!(
        press(
            &mut rig,
            &mut drag,
            Tool::PolygonalLasso,
            PointerButton::Primary,
            (300.0, 250.0)
        ),
        lasso_tools::PolygonPress::Added
    ));
    let lasso_tools::PolygonPress::Closes(ending) = press(
        &mut rig,
        &mut drag,
        Tool::PolygonalLasso,
        PointerButton::Primary,
        (101.0, 81.0),
    ) else {
        unreachable!("the first vertex closes");
    };
    assert!(rig.end_drag(ending));
    assert_eq!(rig.labels(), ["Polygonal Lasso"]);
}

#[test]
fn the_wand_mask_is_clipped_to_the_document() {
    // A layer wider and taller than the 400 x 300 document.
    let mut rig = Rig::new(rect(0, 0, 600, 500));
    assert_eq!(rig.wand((10.0, 10.0)), Ok(true));
    // The whole layer matches; the document-sized part is kept, plus the
    // one-texel softening rim.
    assert_eq!(rig.selected_bounds(), Some(rect(-1, -1, 402, 302)));
    let s = ok_selection(&rig);
    assert_eq!(s.coverage_at(399, 299), 1.0);
    assert!(s.coverage_at(400, 299) < 0.5, "past the document edge");
    // A layer origin left of the document is clipped on that side too.
    let mut rig = Rig::new(rect(-100, -50, 300, 200));
    assert_eq!(rig.wand((10.0, 10.0)), Ok(true));
    assert_eq!(rig.selected_bounds(), Some(rect(-1, -1, 202, 152)));
}

#[test]
fn crop_binary_keeps_the_overlap_rows_and_refuses_no_overlap() {
    let data: Vec<u8> = (0..12).collect();
    let cropped = lasso_tools::crop_binary(rect(10, 10, 4, 3), &data, &rect(11, 11, 2, 5));
    assert_eq!(cropped, Some((rect(11, 11, 2, 2), vec![5, 6, 9, 10])));
    assert_eq!(
        lasso_tools::crop_binary(rect(10, 10, 4, 3), &data, &rect(20, 20, 5, 5)),
        None
    );
}

#[test]
fn a_slow_live_rebuild_backs_off_and_a_quick_one_does_not() {
    let now = std::time::Instant::now();
    let ms = std::time::Duration::from_millis;
    assert_eq!(lasso_tools::backoff_until(now, ms(8)), None);
    assert_eq!(lasso_tools::backoff_until(now, ms(9)), Some(now + ms(27)));
    // Inside the back-off the live mask is not rebuilt; after it, it is.
    let clip = lasso_tools::document_clip((400, 300));
    let path = [(100.0, 80.0), (200.0, 80.0), (200.0, 180.0)];
    let mut selection = aurora_doc::SelectionSet::new();
    let mut current = None;
    let mut wait = Some(std::time::Instant::now() + std::time::Duration::from_hours(1));
    lasso_tools::live_rebuild_step(
        &path,
        &clip,
        &crate::selection_modes::ModeGesture::replace(),
        &mut selection,
        &mut current,
        &mut wait,
    );
    assert_eq!(selection.active(), None, "skipped while backing off");
    let mut wait = Some(std::time::Instant::now());
    lasso_tools::live_rebuild_step(
        &path,
        &clip,
        &crate::selection_modes::ModeGesture::replace(),
        &mut selection,
        &mut current,
        &mut wait,
    );
    assert!(
        selection.active().is_some(),
        "rebuilt once the wait is over"
    );
    assert_eq!(wait, None, "a quick rebuild clears the wait");
}

// ---- 0.215.0: selection modes -------------------------------------------

use crate::selection_modes::{self, mode_from_modifiers, resolve_mode};
use aurora_doc::SelectionMode;

fn mods(shift: bool, alt: bool) -> Modifiers {
    Modifiers {
        shift,
        alt,
        control: false,
        meta: false,
    }
}

impl Rig {
    /// A whole Marquee gesture in `mode`; whether a step was recorded.
    fn marquee_mode(
        &mut self,
        tool: Tool,
        mode: SelectionMode,
        from: (f32, f32),
        to: (f32, f32),
    ) -> bool {
        let mut drag = self.press_marquee_mode(tool, from, mode);
        self.drag_to(&mut drag, to);
        self.end_drag(drag)
    }

    /// A whole freehand Lasso gesture in `mode`.
    fn lasso_mode(&mut self, mode: SelectionMode, points: &[(f32, f32)]) -> bool {
        let Some((&first, rest)) = points.split_first() else {
            unreachable!("a lasso has points");
        };
        let mut drag = self.press_lasso_mode(Tool::Lasso, first, mode);
        for &p in rest {
            self.drag_to(&mut drag, p);
        }
        let mut slot = None;
        let ending = lasso_tools::release_lasso(&mut slot, drag, &mut self.selection);
        self.end_drag(ending)
    }

    fn covered(&self, x: i64, y: i64) -> f32 {
        self.selection.active().map_or(0.0, |s| s.coverage_at(x, y))
    }
}

#[test]
fn modifiers_at_the_press_pick_the_mode_and_the_mode_button_overrides_them() {
    assert_eq!(
        mode_from_modifiers(mods(false, false)),
        SelectionMode::Replace
    );
    assert_eq!(mode_from_modifiers(mods(true, false)), SelectionMode::Add);
    assert_eq!(
        mode_from_modifiers(mods(false, true)),
        SelectionMode::Subtract
    );
    assert_eq!(
        mode_from_modifiers(mods(true, true)),
        SelectionMode::Intersect
    );
    // The button at New defers to the modifiers; any other button wins.
    for (shift, alt) in [(false, false), (true, false), (false, true), (true, true)] {
        let m = mods(shift, alt);
        assert_eq!(
            resolve_mode(SelectionMode::Replace, m),
            mode_from_modifiers(m)
        );
        for button in [
            SelectionMode::Add,
            SelectionMode::Subtract,
            SelectionMode::Intersect,
        ] {
            assert_eq!(resolve_mode(button, m), button, "{shift} {alt}");
        }
    }
    assert_eq!(
        crate::ToolSettings::default().selection_mode,
        SelectionMode::Replace
    );
}

#[test]
fn add_unites_two_marquees_in_one_labelled_step_and_undo_restores_the_first() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    assert!(rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0)));
    let first = rig.selection.active();
    assert!(rig.marquee_mode(
        Tool::MarqueeSelect,
        SelectionMode::Add,
        (40.0, 30.0),
        (100.0, 90.0)
    ));
    let union = rig.selection.active();
    assert_eq!(rig.labels(), ["Rectangular Marquee", "Add to Selection"]);
    assert_eq!(rig.covered(20, 20), 1.0, "only in the first");
    assert_eq!(rig.covered(90, 80), 1.0, "only in the second");
    assert_eq!(rig.covered(70, 20), 0.0, "in neither");
    assert_eq!(rig.selected_bounds(), Some(rect(10, 10, 90, 80)));
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first, "exactly the previous one");
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), union);
}

#[test]
fn subtract_and_intersect_cut_the_existing_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (110.0, 110.0));
    assert!(rig.marquee_mode(
        Tool::EllipticalMarquee,
        SelectionMode::Subtract,
        (40.0, 40.0),
        (80.0, 80.0)
    ));
    assert_eq!(rig.covered(60, 60), 0.0, "the hole");
    assert_eq!(rig.covered(15, 15), 1.0, "the rest");
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Subtract from Selection")
    );
    let holed = rig.selection.active();
    assert!(rig.marquee_mode(
        Tool::MarqueeSelect,
        SelectionMode::Intersect,
        (0.0, 0.0),
        (50.0, 50.0)
    ));
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Intersect Selection")
    );
    assert_eq!(rig.covered(15, 15), 1.0);
    assert_eq!(rig.covered(100, 100), 0.0, "outside the intersecting box");
    assert_eq!(rig.covered(55, 55), 0.0, "inside the hole");
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), holed);
}

#[test]
fn a_combining_press_keeps_the_selection_and_a_release_without_a_move_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let steps = rig.undo_order.undo.len();
    for mode in [
        SelectionMode::Add,
        SelectionMode::Subtract,
        SelectionMode::Intersect,
    ] {
        let drag = rig.press_marquee_mode(Tool::MarqueeSelect, (200.0, 200.0), mode);
        assert_eq!(rig.selection.active(), existing, "{mode:?}: still visible");
        assert!(!rig.end_drag(drag), "{mode:?}: a click changes nothing");
        assert_eq!(rig.selection.active(), existing);
    }
    assert_eq!(rig.undo_order.undo.len(), steps);
    // New mode still deselects on a click.
    let drag = rig.press_marquee_mode(Tool::MarqueeSelect, (200.0, 200.0), SelectionMode::Replace);
    assert_eq!(rig.selection.active(), None);
    assert!(rig.end_drag(drag));
}

#[test]
fn the_live_selection_while_dragging_in_add_mode_is_the_combined_result() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let mut drag = rig.press_marquee_mode(Tool::MarqueeSelect, (100.0, 100.0), SelectionMode::Add);
    rig.drag_to(&mut drag, (140.0, 130.0));
    assert_eq!(rig.covered(20.0 as i64, 20), 1.0, "the old part stays");
    assert_eq!(rig.covered(120, 110), 1.0, "the new part joins");
    rig.drag_to(&mut drag, (110.0, 105.0));
    assert_eq!(
        rig.covered(120, 110),
        0.0,
        "shrinking the drag shrinks the addition"
    );
    assert_eq!(rig.covered(20, 20), 1.0);
    assert!(rig.end_drag(drag));
}

#[test]
fn escape_during_a_combining_drag_restores_the_selection_without_a_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let steps = rig.undo_order.undo.len();
    let mut drag =
        rig.press_marquee_mode(Tool::MarqueeSelect, (100.0, 100.0), SelectionMode::Subtract);
    rig.drag_to(&mut drag, (30.0, 30.0));
    assert_ne!(rig.selection.active(), existing);
    assert!(lasso_tools::cancel_selection_gesture(
        &mut drag,
        &mut rig.selection
    ));
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.undo_order.undo.len(), steps);
}

#[test]
fn a_tool_switch_mid_combining_drag_records_the_combined_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    let mut drag = rig.press_marquee_mode(Tool::MarqueeSelect, (100.0, 100.0), SelectionMode::Add);
    rig.drag_to(&mut drag, (140.0, 130.0));
    let ending = lasso_tools::take_on_tool_switch(&mut drag, Tool::MarqueeSelect, Tool::Brush);
    assert!(rig.end_drag(ending));
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Add to Selection")
    );
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first);
}

#[test]
fn a_combination_over_the_mask_cap_leaves_the_selection_unchanged_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (20.0, 20.0));
    let existing = rig.selection.active();
    let steps = rig.undo_order.undo.len();
    // The union box would be about 30000 x 30000.
    assert!(!rig.marquee_mode(
        Tool::MarqueeSelect,
        SelectionMode::Add,
        (30000.0, 30000.0),
        (30010.0, 30010.0)
    ));
    assert_eq!(rig.selection.active(), existing);
    assert_eq!(rig.undo_order.undo.len(), steps);
}

#[test]
fn history_jumps_over_combined_steps_apply_exactly_each_rows_selection() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let first = rig.selection.active();
    rig.marquee_mode(
        Tool::MarqueeSelect,
        SelectionMode::Add,
        (40.0, 30.0),
        (100.0, 90.0),
    );
    let second = rig.selection.active();
    rig.marquee_mode(
        Tool::EllipticalMarquee,
        SelectionMode::Subtract,
        (20.0, 20.0),
        (50.0, 40.0),
    );
    let third = rig.selection.active();
    assert!(first != second && second != third);
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut drag = None;
    let mut focus = aurora_widgets::FocusManager::default();
    for (target, expected) in [(1, &first), (0, &None), (3, &third), (2, &second)] {
        let _ = crate::perform_history_jump(
            &mut crate::HistoryJumpContext {
                workspace: &mut rig.workspace,
                focus: &mut focus,
                scales: &scales,
                layers: &mut rig.layers,
                history: &mut rig.history,
                pixel_history: &mut rig.pixel_history,
                store: Some(&mut rig.store),
                undo_order: &mut rig.undo_order,
                composite_cache: &mut cache,
                view: &mut rig.view,
                layer_rows: &mut layer_rows,
                active_layer: &mut active,
                drag: &mut drag,
                layer_controls: &mut state,
            },
            target,
        );
        selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
        assert_eq!(&rig.selection.active(), expected, "jump to row {target}");
    }
}

#[test]
fn painting_goes_through_the_combined_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    rig.marquee_mode(
        Tool::MarqueeSelect,
        SelectionMode::Add,
        (100.0, 10.0),
        (150.0, 60.0),
    );
    rig.settings.set_radius(Tool::Brush, 10.0);
    // A horizontal stroke across both boxes and the gap between them.
    assert!(rig.stroke(Tool::Brush, &[(5.0, 35.0), (160.0, 35.0)], false));
    assert!(rig.alpha(30, 35) > 0.0, "inside the first box");
    assert!(rig.alpha(120, 35) > 0.0, "inside the added box");
    assert_eq!(rig.alpha(80, 35), 0.0, "the gap between them stays clean");
    assert_eq!(rig.alpha(5, 35), 0.0, "left of both");
}

#[test]
fn lasso_modes_combine_with_the_existing_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (110.0, 110.0));
    let first = rig.selection.active();
    assert!(rig.lasso_mode(SelectionMode::Subtract, &SQUARE));
    // SQUARE is (100,80)-(200,180): only the corner overlapping the box
    // is removed.
    assert_eq!(rig.covered(105, 95), 0.0, "removed");
    assert_eq!(rig.covered(50, 50), 1.0, "kept");
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Subtract from Selection")
    );
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first);
    assert!(rig.lasso_mode(SelectionMode::Add, &SQUARE));
    assert_eq!(rig.covered(150, 150), 1.0, "added");
    assert_eq!(rig.covered(50, 50), 1.0, "kept");
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.lasso_mode(SelectionMode::Intersect, &SQUARE));
    assert_eq!(rig.covered(105, 95), 1.0, "the overlap");
    assert_eq!(rig.covered(50, 50), 0.0, "outside the lasso");
    assert_eq!(rig.covered(150, 150), 0.0, "outside the old box");
}

#[test]
fn a_polygonal_lasso_in_add_mode_keeps_the_selection_while_building_and_adds_on_close() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 50.0));
    let existing = rig.selection.active();
    let mut drag = rig.press_lasso_mode(Tool::PolygonalLasso, SQUARE[0], SelectionMode::Add);
    assert_eq!(rig.selection.active(), existing, "kept at the press");
    let ending = drag.take();
    let ending = lasso_tools::release_lasso(&mut drag, ending, &mut rig.selection);
    assert!(ending.is_none());
    for p in &SQUARE[1..] {
        let Some(d) = drag.as_mut() else {
            unreachable!("alive");
        };
        lasso_tools::polygon_click(d, *p, &rig.view, &mut rig.selection);
    }
    assert_eq!(rig.covered(30, 30), 1.0, "old part kept while building");
    assert_eq!(rig.covered(150, 120), 1.0, "polygon so far added");
    let Some(d) = drag.as_mut() else {
        unreachable!("alive");
    };
    assert_eq!(
        lasso_tools::polygon_click(d, SQUARE[0], &rig.view, &mut rig.selection),
        PolygonClick::Closes
    );
    let ending = drag.take();
    assert!(rig.end_drag(ending));
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Add to Selection")
    );
}

#[test]
fn the_wand_combines_by_its_mode() {
    let mut rig = wand_rig();
    assert_eq!(rig.wand((250.0, 140.0)), Ok(true));
    let red = rig.selection.active();
    // Add the far square.
    assert_eq!(rig.wand_mode((50.0, 50.0), SelectionMode::Add), Ok(true));
    assert_eq!(rig.covered(250, 140), 1.0);
    assert_eq!(rig.covered(50, 50), 1.0);
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Add to Selection")
    );
    // Subtract the big rectangle again.
    assert_eq!(
        rig.wand_mode((250.0, 140.0), SelectionMode::Subtract),
        Ok(true)
    );
    assert_eq!(rig.covered(250, 140), 0.0);
    assert_eq!(rig.covered(50, 50), 1.0);
    // Intersect with the big rectangle: nothing left, so it deselects.
    assert_eq!(
        rig.wand_mode((250.0, 140.0), SelectionMode::Intersect),
        Ok(true)
    );
    assert_eq!(rig.selection.active(), None);
    assert_eq!(
        rig.labels().last().map(String::as_str),
        Some("Intersect Selection")
    );
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.covered(50, 50), 1.0);
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), red);
}

#[test]
fn the_wand_in_a_combining_mode_with_nothing_selected_follows_photoshop() {
    let mut rig = wand_rig();
    // Add with nothing selected selects; Subtract and Intersect select
    // nothing and record nothing.
    assert_eq!(
        rig.wand_mode((250.0, 140.0), SelectionMode::Subtract),
        Ok(false)
    );
    assert_eq!(
        rig.wand_mode((250.0, 140.0), SelectionMode::Intersect),
        Ok(false)
    );
    assert!(rig.labels().is_empty());
    assert_eq!(rig.wand_mode((250.0, 140.0), SelectionMode::Add), Ok(true));
    assert_eq!(rig.covered(250, 140), 1.0);
}

#[test]
fn the_mode_labels_and_palette_names_are_distinct_and_new_keeps_the_tool_label() {
    assert_eq!(
        selection_modes::step_label(SelectionMode::Replace, "Lasso"),
        "Lasso"
    );
    let labels: Vec<_> = SelectionMode::ALL
        .iter()
        .map(|m| selection_modes::palette_label(*m))
        .collect();
    for (i, a) in labels.iter().enumerate() {
        for b in &labels[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert_eq!(
        selection_modes::mode_step_label(SelectionMode::Replace),
        None
    );
}

// ---- Select > Modify (0.216.0) ----

use crate::selection_refine::{self, RefineKind, RefineOutcome};

impl Rig {
    /// One Select > Modify command as `App::run_refine` runs it.
    fn refine(&mut self, kind: RefineKind, amount: f32) -> RefineOutcome {
        selection_refine::refine_step(
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
            kind,
            amount,
        )
    }
}

#[test]
fn every_refinement_is_one_labelled_dirty_neutral_step_that_undoes_and_redoes_exactly() {
    for (kind, amount, label) in [
        (RefineKind::Feather, 6.0, "Feather Selection"),
        (RefineKind::Expand, 4.0, "Expand Selection"),
        (RefineKind::Contract, 4.0, "Contract Selection"),
        (RefineKind::Smooth, 3.0, "Smooth Selection"),
        (RefineKind::Border, 4.0, "Border Selection"),
    ] {
        let mut rig = Rig::new(rect(0, 0, 400, 300));
        rig.marquee_step(Tool::EllipticalMarquee, (50.0, 40.0), (150.0, 120.0));
        let before = rig.selection.active();
        let content = rig.undo_order.content_revision;
        assert_eq!(rig.refine(kind, amount), RefineOutcome::Applied, "{label}");
        let after = rig.selection.active();
        assert_ne!(after, before, "{label}");
        assert_eq!(rig.labels().last().map(String::as_str), Some(label));
        assert_eq!(
            rig.undo_order.content_revision, content,
            "{label} is dirty-neutral"
        );
        // Undo puts back exactly what was there; redo exactly the result.
        assert!(rig.step(UndoDirection::Undo));
        assert_eq!(rig.selection.active(), before, "{label} undo");
        assert!(rig.step(UndoDirection::Redo));
        assert_eq!(rig.selection.active(), after, "{label} redo");
    }
}

#[test]
fn a_history_jump_over_a_refine_step_restores_the_exact_selection() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 40.0), (150.0, 120.0));
    let first = rig.selection.active();
    assert_eq!(rig.refine(RefineKind::Expand, 5.0), RefineOutcome::Applied);
    let second = rig.selection.active();
    assert_eq!(rig.refine(RefineKind::Feather, 5.0), RefineOutcome::Applied);
    let third = rig.selection.active();
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut drag = None;
    let mut focus = aurora_widgets::FocusManager::default();
    for (target, expected) in [(1, &first), (3, &third), (2, &second)] {
        let _ = crate::perform_history_jump(
            &mut crate::HistoryJumpContext {
                workspace: &mut rig.workspace,
                focus: &mut focus,
                scales: &scales,
                layers: &mut rig.layers,
                history: &mut rig.history,
                pixel_history: &mut rig.pixel_history,
                store: Some(&mut rig.store),
                undo_order: &mut rig.undo_order,
                composite_cache: &mut cache,
                view: &mut rig.view,
                layer_rows: &mut layer_rows,
                active_layer: &mut active,
                drag: &mut drag,
                layer_controls: &mut state,
            },
            target,
        );
        selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
        assert_eq!(&rig.selection.active(), expected, "jump to row {target}");
    }
}

#[test]
fn modifying_with_no_selection_says_so_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    for kind in RefineKind::ALL {
        assert_eq!(
            rig.refine(kind, 3.0),
            RefineOutcome::Refused(crate::LockRefusal::RefineNeedsSelection)
        );
    }
    assert!(rig.labels().is_empty());
    assert!(rig.selection.active().is_none());
    let notice = crate::LockRefusal::RefineNeedsSelection.message();
    assert!(notice.contains("selection"));
}

#[test]
fn a_selection_too_large_to_modify_is_refused_calmly_and_unchanged() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let big = Selection::new(rect(0, 0, 7000, 7000));
    rig.selection.select(big.clone());
    for kind in RefineKind::ALL {
        assert_eq!(
            rig.refine(kind, 3.0),
            RefineOutcome::Refused(crate::LockRefusal::RefineTooLarge),
            "{kind:?}"
        );
    }
    assert_eq!(rig.selection.active(), Some(big));
    assert!(rig.labels().is_empty());
}

#[test]
fn a_contract_that_leaves_nothing_deselects_as_one_step() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 40.0), (56.0, 120.0));
    let before = rig.selection.active();
    assert_eq!(
        rig.refine(RefineKind::Contract, 5.0),
        RefineOutcome::Applied
    );
    assert!(rig.selection.active().is_none());
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), before);
}

#[test]
fn a_feather_below_one_pixel_changes_nothing_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 40.0), (150.0, 120.0));
    let steps = rig.labels().len();
    assert_eq!(
        rig.refine(RefineKind::Feather, 0.5),
        RefineOutcome::Unchanged
    );
    assert_eq!(rig.labels().len(), steps);
}

#[test]
fn painting_through_a_feathered_selection_takes_fractional_coverage() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (100.0, 100.0), (220.0, 180.0));
    assert_eq!(
        rig.refine(RefineKind::Feather, 12.0),
        RefineOutcome::Applied
    );
    let selection = ok_selection(&rig);
    assert!(matches!(selection.shape, SelectionShape::Mask(_)));
    rig.settings.set_radius(Tool::Brush, 40.0);
    assert!(rig.stroke(Tool::Brush, &[(40.0, 140.0), (300.0, 140.0)], false));
    let mut partial = 0;
    for y in 0..300_u32 {
        for x in 0..400_u32 {
            let c = selection.coverage_at(i64::from(x), i64::from(y));
            let a = rig.alpha(x, y);
            if c == 0.0 {
                assert_eq!(a, 0.0, "outside the feather ({x}, {y})");
            } else if a > 0.0 && a < 1.0 {
                partial += 1;
            }
        }
    }
    assert!(
        partial > 20,
        "the feathered edge took fractional paint ({partial})"
    );
}

#[test]
fn the_prompt_accepts_numbers_by_the_kinds_own_rules() {
    use selection_refine::parse_amount;
    assert_eq!(parse_amount(RefineKind::Expand, "5"), Ok(5.0));
    assert_eq!(parse_amount(RefineKind::Expand, " 12 px "), Ok(12.0));
    assert_eq!(parse_amount(RefineKind::Feather, "2.5"), Ok(2.5));
    assert_eq!(parse_amount(RefineKind::Contract, "500"), Ok(500.0));
    assert!(parse_amount(RefineKind::Contract, "501").is_err());
    assert!(parse_amount(RefineKind::Smooth, "100").is_ok());
    assert!(parse_amount(RefineKind::Smooth, "101").is_err());
    assert!(parse_amount(RefineKind::Feather, "250").is_ok());
    assert!(parse_amount(RefineKind::Feather, "250.5").is_err());
    for bad in [
        "", "  ", "0", "-3", "abc", "1e2", "NaN", "inf", "5.5", "3 4", ".", "0.5",
    ] {
        assert!(
            parse_amount(RefineKind::Border, bad).is_err(),
            "Border accepted {bad:?}"
        );
    }
    assert!(parse_amount(RefineKind::Feather, "0.5").is_err());
    assert!(parse_amount(RefineKind::Feather, "5.5.5").is_err());
}

#[test]
fn the_prompt_opens_with_the_last_value_validates_on_enter_and_cancels() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut focus = aurora_widgets::FocusManager::default();
    let mut palette = None;
    assert!(selection_refine::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        RefineKind::Expand,
        7.0
    ));
    let Some(root) = palette else {
        unreachable!("open");
    };
    // A second prompt while one is open is refused.
    assert!(!selection_refine::open_prompt(
        &mut rig.workspace,
        &mut focus,
        &mut palette,
        RefineKind::Feather,
        7.0
    ));
    let state = aurora_widgets::widgets::command_palette_state(&rig.workspace.tree, root);
    let Ok(state) = state else {
        unreachable!("state")
    };
    assert_eq!(state.query(), "7");
    assert!(
        state
            .selected()
            .is_some_and(|e| e.title.contains("Expand Selection by 7 px"))
    );
    // Enter on the opening value is valid.
    assert_eq!(
        selection_refine::prompt_enter(&mut rig.workspace.tree, root),
        Some(Ok((RefineKind::Expand, 7.0)))
    );
    // A bad value keeps the prompt and shows the reason; typing withdraws it.
    let _ = aurora_widgets::widgets::set_command_palette_query(&mut rig.workspace.tree, root, "9x");
    assert!(selection_refine::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
    assert_eq!(
        selection_refine::prompt_enter(&mut rig.workspace.tree, root),
        Some(Err(()))
    );
    let Ok(state) = aurora_widgets::widgets::command_palette_state(&rig.workspace.tree, root)
    else {
        unreachable!("state")
    };
    assert!(state.selected().is_some_and(|e| e.title.contains("digits")));
    let _ =
        aurora_widgets::widgets::set_command_palette_query(&mut rig.workspace.tree, root, "900");
    assert!(selection_refine::follow_prompt_query(
        &mut rig.workspace.tree,
        root
    ));
    assert_eq!(
        selection_refine::prompt_enter(&mut rig.workspace.tree, root),
        Some(Err(()))
    );
    // Escape closes it as it closes any palette.
    crate::close_command_palette(&mut rig.workspace, &mut focus, &mut palette);
    assert!(palette.is_none());
    // The remembered amount is per command.
    let mut amounts = selection_refine::RefineAmounts::default();
    amounts.set(RefineKind::Smooth, 9.0);
    assert_eq!(amounts.get(RefineKind::Smooth), 9.0);
    assert_eq!(amounts.get(RefineKind::Expand), 5.0);
}

#[test]
fn the_commands_are_in_the_palette_and_route_to_their_prompts() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut focus = aurora_widgets::FocusManager::default();
    let entries = crate::palette_commands();
    for kind in RefineKind::ALL {
        assert_eq!(RefineKind::for_command(kind.command_id()), Some(kind));
        assert_eq!(RefineKind::for_row(kind.row_id()), Some(kind));
        assert!(kind.menu_label().ends_with('\u{2026}'));
        assert!(
            entries
                .iter()
                .any(|e| e.id == kind.command_id() && e.title == kind.menu_label()),
            "{kind:?} is in the palette"
        );
        let mut dialog = crate::tests::FakeFileDialog::default();
        assert!(matches!(
            crate::activate_command(&mut rig.workspace, &mut focus, kind.command_id(), &mut dialog),
            Some(crate::ActivatedCommand::RefinePrompt(k)) if k == kind
        ));
    }
}

#[test]
fn a_slow_live_combination_backs_off_and_the_release_places_the_newest_shape_exactly() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    let mut drag = rig.press_marquee_mode(
        Tool::EllipticalMarquee,
        (100.0, 100.0),
        aurora_doc::SelectionMode::Add,
    );
    rig.drag_to(&mut drag, (140.0, 130.0));
    let shown = rig.selection.active();
    // A slow update was just seen: the next ones wait.
    let far = std::time::Instant::now() + std::time::Duration::from_hours(1);
    if let Some(Drag::Marquee { modal, .. }) = drag.as_mut() {
        modal.not_before = Some(far);
    }
    rig.drag_to(&mut drag, (200.0, 190.0));
    assert_eq!(
        rig.selection.active(),
        shown,
        "the skipped update changed nothing"
    );
    // The release is exact whatever the back-off did.
    let mut slot = None;
    let ending = lasso_tools::release_lasso(&mut slot, drag, &mut rig.selection);
    let exact = rig.selection.active();
    assert_ne!(exact, shown);
    let Some(exact_sel) = exact.clone() else {
        unreachable!("selected")
    };
    assert!(exact_sel.coverage_at(150, 145) > 0.0);
    assert!(rig.end_drag(ending));
    // The same gesture without any back-off gives the same selection.
    let mut other = Rig::new(rect(0, 0, 400, 300));
    other.marquee_step(Tool::MarqueeSelect, (10.0, 10.0), (60.0, 60.0));
    assert!(other.marquee_mode(
        Tool::EllipticalMarquee,
        aurora_doc::SelectionMode::Add,
        (100.0, 100.0),
        (200.0, 190.0)
    ));
    assert_eq!(other.selection.active(), exact);
}

#[test]
fn a_combination_slower_than_the_limit_sets_the_back_off() {
    let mut selection = aurora_doc::SelectionSet::new();
    selection.select(Selection::ellipse(rect(0, 0, 3000, 3000)));
    let mut gesture = selection_modes::ModeGesture {
        mode: aurora_doc::SelectionMode::Add,
        base: selection.active(),
        ..selection_modes::ModeGesture::replace()
    };
    let shape = Selection::ellipse(rect(500, 500, 3000, 3000));
    let started = std::time::Instant::now();
    assert!(gesture.place_throttled(&mut selection, shape.clone(), started));
    assert!(
        gesture.not_before.is_some(),
        "a union of two 3000 px ellipses is slower than {:?}",
        lasso_tools::SLOW_REBUILD
    );
    // Within the back-off the update is only remembered.
    assert!(!gesture.place_throttled(&mut selection, shape.clone(), std::time::Instant::now()));
    assert_eq!(gesture.pending, Some(shape));
    // New mode never backs off.
    let mut replace = selection_modes::ModeGesture::replace();
    replace.not_before = Some(std::time::Instant::now() + std::time::Duration::from_mins(1));
    assert!(replace.place_throttled(
        &mut selection,
        Selection::new(rect(0, 0, 5, 5)),
        std::time::Instant::now()
    ));
}

#[test]
fn a_live_combination_over_the_cap_says_so_once_per_gesture() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // A selection of 81 Mpx: adding anything non-trivial exceeds the mask cap.
    rig.selection.select(Selection::new(rect(0, 0, 9000, 9000)));
    let mut drag = rig.press_marquee_mode(
        Tool::EllipticalMarquee,
        (100.0, 100.0),
        aurora_doc::SelectionMode::Add,
    );
    let before = rig.selection.active();
    assert!(
        !selection_modes::drag_refusal(drag.as_ref()),
        "nothing refused yet"
    );
    rig.drag_to(&mut drag, (160.0, 150.0));
    assert_eq!(
        rig.selection.active(),
        before,
        "a refused combination changes nothing"
    );
    assert!(
        selection_modes::drag_refusal(drag.as_ref()),
        "the first refusal gives the notice"
    );
    rig.drag_to(&mut drag, (180.0, 170.0));
    assert!(
        !selection_modes::drag_refusal(drag.as_ref()),
        "once per gesture"
    );
    // The notice is the Magic Wand's calm one, kept for the selection tools.
    for tool in [
        Tool::MarqueeSelect,
        Tool::EllipticalMarquee,
        Tool::Lasso,
        Tool::PolygonalLasso,
        Tool::MagicWand,
    ] {
        assert!(crate::lock_refusal_applies(
            crate::LockRefusal::SelectionTooLarge,
            tool,
            aurora_doc::LayerLock::default()
        ));
    }
    assert!(!crate::lock_refusal_applies(
        crate::LockRefusal::SelectionTooLarge,
        Tool::Brush,
        aurora_doc::LayerLock::default()
    ));
    // A new gesture gets its own notice.
    let ending = drag;
    assert!(!rig.end_drag(ending));
    let mut again = rig.press_marquee_mode(
        Tool::EllipticalMarquee,
        (100.0, 100.0),
        aurora_doc::SelectionMode::Add,
    );
    rig.drag_to(&mut again, (160.0, 150.0));
    assert!(selection_modes::drag_refusal(again.as_ref()));
}

// ---- Quick Mask (0.217.0) ----

use crate::colour_state::ColourState;
use crate::quick_mask::{self, LeaveOutcome};
use aurora_doc::QuickMaskOutcome;

fn black_on_white() -> ColourState {
    ColourState::default()
}

/// White over black: the Brush selects, the Eraser deselects.
fn white_on_black() -> ColourState {
    let mut c = ColourState::default();
    assert!(c.swap());
    c
}

impl Rig {
    fn qm_enter(&self) -> quick_mask::QuickMaskMode {
        match quick_mask::enter(self.selection.active().as_ref(), (400, 300)) {
            Ok(mode) => mode,
            Err(why) => unreachable!("{why:?}"),
        }
    }

    /// A Brush or Eraser stroke in Quick Mask the way `App` runs one:
    /// `begin_drag` with no layer, `paint_dab` per dab, `commit_ending_drag`.
    /// Returns whether a history step was recorded (it must not be).
    fn qm_stroke(
        &mut self,
        mode: &mut quick_mask::QuickMaskMode,
        tool: Tool,
        points: &[(f32, f32)],
        colours: &ColourState,
    ) -> bool {
        let Some(&first) = points.first() else {
            unreachable!("a stroke has points");
        };
        let mut drag = begin_drag(
            tool,
            PointerButton::Primary,
            first,
            &self.view,
            quick_mask::drag_layer(
                true,
                tool,
                crate::active_pixel_layer(&self.layers, Some(self.layer)),
            ),
        );
        let dab_tool = if tool == Tool::Eraser {
            DabTool::Eraser
        } else {
            crate::dab_tool_of_drag(drag.as_ref())
        };
        let press = match drag.as_ref() {
            Some(
                Drag::Brush {
                    last_doc, stroke, ..
                }
                | Drag::Eraser {
                    last_doc, stroke, ..
                },
            ) => {
                assert!(stroke.is_none(), "no stroke snapshot in Quick Mask");
                *last_doc
            }
            _ => unreachable!("a paint tool starts a stroke"),
        };
        let _ = quick_mask::paint_dab(mode, dab_tool, press, &self.settings, colours);
        for &p in points.iter().skip(1) {
            let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
            let dabs = match drag.as_mut() {
                Some(d) => continue_drag(
                    d,
                    p,
                    &mut self.view,
                    &mut self.selection,
                    &self.settings,
                    bounds,
                ),
                None => Vec::new(),
            };
            for dab in dabs {
                let _ = quick_mask::paint_dab(mode, dab_tool, dab, &self.settings, colours);
            }
        }
        commit_ending_drag(
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        )
    }

    fn qm_leave(&mut self, mode: &quick_mask::QuickMaskMode) -> LeaveOutcome {
        quick_mask::leave(
            mode,
            &mut self.workspace,
            &mut self.undo_order,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.selection,
        )
    }
}

fn fill_mask(mode: &mut quick_mask::QuickMaskMode, value: u8) {
    mode.mask.edit(|data, _, _| {
        data.fill(value);
        Some(rect(0, 0, 1, 1))
    });
}

#[test]
fn luma_is_rec709_on_straight_srgb_with_clamping() {
    assert_eq!(quick_mask::luma([0.0, 0.0, 0.0]), 0);
    assert_eq!(quick_mask::luma([1.0, 1.0, 1.0]), 255);
    assert_eq!(quick_mask::luma([0.5, 0.5, 0.5]), 128);
    assert_eq!(quick_mask::luma([1.0, 0.0, 0.0]), 54);
    assert_eq!(quick_mask::luma([0.0, 1.0, 0.0]), 182);
    assert_eq!(quick_mask::luma([0.0, 0.0, 1.0]), 18);
    assert_eq!(quick_mask::luma([2.0, 2.0, 2.0]), 255);
    assert_eq!(quick_mask::luma([-1.0, -1.0, -1.0]), 0);
    assert_eq!(quick_mask::luma([f32::NAN, 1.0, 1.0]), 201);
}

#[test]
fn the_brush_pulls_toward_the_foreground_and_the_eraser_toward_the_background() {
    let c = black_on_white();
    assert_eq!(quick_mask::target_byte(DabTool::Brush, &c), 0);
    assert_eq!(quick_mask::target_byte(DabTool::Eraser, &c), 255);
    let c = white_on_black();
    assert_eq!(quick_mask::target_byte(DabTool::Brush, &c), 255);
    assert_eq!(quick_mask::target_byte(DabTool::Eraser, &c), 0);
}

#[test]
fn entering_records_nothing_and_a_mask_stroke_never_touches_the_layer_or_history() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 50.0), (250.0, 200.0));
    let steps = rig.labels();
    let tiles = rig.tiles();
    let selection = rig.selection.active();
    let mut mode = rig.qm_enter();
    assert_eq!(rig.labels(), steps, "entering records nothing");
    assert_eq!(
        rig.selection.active(),
        selection,
        "nor changes the selection"
    );
    // Black foreground: the Brush deselects under the dab.
    let recorded = rig.qm_stroke(
        &mut mode,
        Tool::Brush,
        &[(100.5, 100.5), (160.5, 100.5)],
        &black_on_white(),
    );
    assert!(!recorded, "no pixel-history step");
    assert_eq!(rig.labels(), steps);
    assert_eq!(rig.tiles(), tiles, "the layer gained no tile");
    assert_eq!(rig.alpha(100, 100), 0.0);
    assert_eq!(mode.mask.byte_at(100, 100), 0);
    assert!(mode.mask.byte_at(140, 100) <= 2, "along the stroke");
    assert_eq!(mode.mask.byte_at(100, 150), 255, "untouched stays selected");
    assert_eq!(rig.selection.active(), selection, "the selection waits");
}

#[test]
fn the_eraser_in_quick_mask_pulls_toward_the_background_not_the_layer() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // Layer has real pixels the Eraser would remove.
    rig.rect_fill(0, 0, 400, 300, [1.0, 0.0, 0.0, 1.0]);
    let before = rig.alpha(100, 100);
    let mut mode = rig.qm_enter();
    fill_mask(&mut mode, 0);
    // Default colours: background white -> the Eraser selects.
    let recorded = rig.qm_stroke(
        &mut mode,
        Tool::Eraser,
        &[(100.5, 100.5)],
        &black_on_white(),
    );
    assert!(!recorded);
    assert_eq!(mode.mask.byte_at(100, 100), 255);
    assert_eq!(rig.alpha(100, 100), before, "layer pixels unchanged");
}

#[test]
fn a_dab_footprint_matches_the_layer_brushs_within_one_byte() {
    // The same dab on a blank layer (alpha = falloff, white) and on a zero
    // grid pulled toward 255: every texel agrees within one byte.
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut mode = rig.qm_enter();
    fill_mask(&mut mode, 0);
    let _ = rig.qm_stroke(&mut mode, Tool::Brush, &[(150.3, 120.7)], &white_on_black());
    let _ = rig.stroke(Tool::Brush, &[(150.3, 120.7)], false);
    let mut painted = 0;
    for y in 90..150_u32 {
        for x in 120..180_u32 {
            let layer = (rig.alpha(x, y) * 255.0).round() as i32;
            let mask = i32::from(mode.mask.byte_at(i64::from(x), i64::from(y)));
            assert!(
                (layer - mask).abs() <= 1,
                "({x},{y}) layer {layer} mask {mask}"
            );
            if mask > 0 {
                painted += 1;
            }
        }
    }
    assert!(
        painted > 1000,
        "a radius-24 dab covers about 1800 texels: {painted}"
    );
}

#[test]
fn leaving_makes_one_undoable_step_that_restores_the_exact_previous_selection() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::EllipticalMarquee, (50.0, 50.0), (250.0, 200.0));
    let first = rig.selection.active();
    let mut mode = rig.qm_enter();
    let _ = rig.qm_stroke(&mut mode, Tool::Brush, &[(150.5, 120.5)], &black_on_white());
    assert_eq!(rig.qm_leave(&mode), LeaveOutcome::Applied);
    let after = rig.selection.active();
    assert_ne!(after, first);
    assert_eq!(rig.labels().last().map(String::as_str), Some("Quick Mask"));
    // The dab hole is deselected.
    assert_eq!(
        after.as_ref().map(|s| s.coverage_at(150, 120)),
        Some(0.0),
        "the painted hole"
    );
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first, "undo restores it exactly");
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.selection.active(), after, "redo returns the result");
}

#[test]
fn an_unedited_grid_leaves_the_selection_and_history_alone() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    // A selection reaching past the document survives an unedited pass.
    rig.selection
        .select(Selection::new(rect(-50, -50, 700, 500)));
    let before = rig.selection.active();
    let steps = rig.labels();
    let mode = rig.qm_enter();
    assert_eq!(rig.qm_leave(&mode), LeaveOutcome::Unchanged);
    assert_eq!(rig.selection.active(), before);
    assert_eq!(rig.labels(), steps);
}

#[test]
fn everything_selected_becomes_no_selection_and_nothing_selected_says_so() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 50.0), (250.0, 200.0));
    let first = rig.selection.active();
    let mut mode = rig.qm_enter();
    fill_mask(&mut mode, 255);
    assert_eq!(rig.qm_leave(&mode), LeaveOutcome::Applied);
    assert_eq!(rig.selection.active(), None, "all 255 is no selection");
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first);
    let mut mode = rig.qm_enter();
    fill_mask(&mut mode, 0);
    assert_eq!(rig.qm_leave(&mode), LeaveOutcome::Emptied);
    assert_eq!(
        rig.selection.active(),
        None,
        "an empty selection is never stored"
    );
    assert!(
        crate::LockRefusal::QuickMaskNothing
            .message()
            .contains("nothing")
    );
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.selection.active(), first);
}

#[test]
fn a_quick_mask_step_never_dirties_the_document_and_jumps_apply_it() {
    use std::collections::HashMap;
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.marquee_step(Tool::MarqueeSelect, (50.0, 50.0), (250.0, 200.0));
    let first = rig.selection.active();
    let content = rig.undo_order.content_revision;
    let mut mode = rig.qm_enter();
    let _ = rig.qm_stroke(&mut mode, Tool::Brush, &[(100.0, 100.0)], &black_on_white());
    assert_eq!(rig.qm_leave(&mode), LeaveOutcome::Applied);
    let second = rig.selection.active();
    assert_eq!(rig.undo_order.content_revision, content);
    let scales = match crate::load_scales() {
        Ok(s) => s,
        Err(err) => unreachable!("{err}"),
    };
    let mut cache = CompositeCache::default();
    let mut layer_rows = HashMap::new();
    let mut active = Some(rig.layer);
    let mut state = crate::LayerControlsState::default();
    let mut drag = None;
    let mut focus = aurora_widgets::FocusManager::default();
    for (target, expected) in [(1, &first), (2, &second)] {
        let _ = crate::perform_history_jump(
            &mut crate::HistoryJumpContext {
                workspace: &mut rig.workspace,
                focus: &mut focus,
                scales: &scales,
                layers: &mut rig.layers,
                history: &mut rig.history,
                pixel_history: &mut rig.pixel_history,
                store: Some(&mut rig.store),
                undo_order: &mut rig.undo_order,
                composite_cache: &mut cache,
                view: &mut rig.view,
                layer_rows: &mut layer_rows,
                active_layer: &mut active,
                drag: &mut drag,
                layer_controls: &mut state,
            },
            target,
        );
        selection_view::apply_pending_selection(&mut rig.undo_order, &mut rig.selection);
        assert_eq!(&rig.selection.active(), expected, "jump to row {target}");
    }
}

#[test]
fn a_document_over_the_cap_is_refused_and_the_cap_itself_is_allowed() {
    assert!(matches!(
        quick_mask::enter(None, (8193, 8192)),
        Err(crate::LockRefusal::QuickMaskTooLarge)
    ));
    assert!(matches!(
        quick_mask::enter(None, (0, 10)),
        Err(crate::LockRefusal::QuickMaskTooLarge)
    ));
    assert!(quick_mask::enter(None, (8192, 8192)).is_ok());
    assert!(
        crate::LockRefusal::QuickMaskTooLarge
            .message()
            .contains("64")
    );
}

#[test]
fn the_fills_and_selection_tools_are_refused_and_the_rest_work() {
    for tool in Tool::ALL {
        let refused = matches!(
            tool,
            Tool::Gradient
                | Tool::PaintBucket
                | Tool::MarqueeSelect
                | Tool::EllipticalMarquee
                | Tool::Lasso
                | Tool::PolygonalLasso
                | Tool::MagicWand
                | Tool::CloneStamp
        );
        assert_eq!(
            quick_mask::tool_refusal(tool),
            refused.then_some(crate::LockRefusal::QuickMaskToolUnavailable),
            "{tool:?}"
        );
    }
    assert!(
        crate::LockRefusal::QuickMaskToolUnavailable
            .message()
            .contains("Quick Mask")
    );
    // Lock gate and layer hand-over: only the mask tools are special.
    assert!(quick_mask::skips_lock_gate(true, Tool::Brush));
    assert!(quick_mask::skips_lock_gate(true, Tool::Eraser));
    assert!(!quick_mask::skips_lock_gate(true, Tool::Move));
    assert!(!quick_mask::skips_lock_gate(false, Tool::Brush));
    assert_eq!(quick_mask::drag_layer(true, Tool::Brush, Some(1)), None);
    assert_eq!(quick_mask::drag_layer(true, Tool::Move, Some(1)), Some(1));
    assert_eq!(quick_mask::drag_layer(false, Tool::Brush, Some(1)), Some(1));
}

#[test]
fn q_the_palette_and_the_menu_id_toggle_quick_mask() {
    for meta in [false, true] {
        let table = crate::default_shortcuts_for(meta);
        assert_eq!(
            table.resolve(KeyChord::new(Modifiers::none(), Key::Character('q'))),
            Some(&AppCommand::ToggleQuickMask)
        );
    }
    let commands = crate::palette_commands();
    let Some(entry) = commands
        .iter()
        .find(|c| c.title == "Toggle Quick Mask Mode")
    else {
        unreachable!("the palette lists the command");
    };
    assert_eq!(entry.id, quick_mask::COMMAND_ID);
    let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
    let mut focus = aurora_widgets::FocusManager::default();
    let activated = crate::activate_command(
        &mut workspace,
        &mut focus,
        &entry.id,
        &mut crate::tests::FakeFileDialog::default(),
    );
    assert!(
        matches!(activated, Some(crate::ActivatedCommand::ToggleQuickMask)),
        "{activated:?}"
    );
}

#[test]
fn the_overlay_is_rebuilt_only_when_the_grid_the_view_or_the_sizes_change() {
    let rig = Rig::new(rect(0, 0, 400, 300));
    let mut mode = rig.qm_enter();
    let view = CanvasView::new();
    assert!(mode.tint_changed(&view, (800.0, 600.0), 1.0), "first build");
    assert!(!mode.tint_changed(&view, (800.0, 600.0), 1.0));
    assert!(mode.tint_changed(&view, (800.0, 601.0), 1.0), "canvas size");
    assert!(mode.tint_changed(&view, (800.0, 601.0), 2.0), "scale");
    let mut zoomed = CanvasView::new();
    zoomed.zoom_at((0.0, 0.0), 2.0);
    assert!(mode.tint_changed(&zoomed, (800.0, 601.0), 2.0), "view");
    assert!(!mode.tint_changed(&zoomed, (800.0, 601.0), 2.0));
    mode.mask.edit(|data, _, _| {
        data.fill(0);
        Some(rect(0, 0, 1, 1))
    });
    assert!(mode.tint_changed(&zoomed, (800.0, 601.0), 2.0), "grid edit");
}

#[test]
fn the_selection_shapes_round_trip_through_enter_and_leave_with_an_edit() {
    for shape in [
        Selection::new(rect(20, 30, 150, 90)),
        Selection::ellipse(rect(20, 30, 150, 90)),
        Selection {
            inverted: true,
            ..Selection::new(rect(20, 30, 150, 90))
        },
    ] {
        let mut rig = Rig::new(rect(0, 0, 400, 300));
        rig.selection.select(shape.clone());
        let mut mode = rig.qm_enter();
        // An edit that changes nothing visible: a dab pulling a texel
        // already at its target (selected) toward white... then force the
        // version by a no-op rewrite of one byte.
        mode.mask.edit(|data, _, _| {
            data[0] = data[0].saturating_add(0);
            Some(rect(0, 0, 1, 1))
        });
        let outcome = rig.qm_leave(&mode);
        assert_eq!(outcome, LeaveOutcome::Applied);
        let after = rig.selection.active().map(|s| {
            (0..300_i64)
                .flat_map(|y| (0..400_i64).map(move |x| (x, y)))
                .map(|(x, y)| (s.coverage_at(x, y) * 255.0).round() as i32)
                .collect::<Vec<_>>()
        });
        let want: Vec<i32> = (0..300_i64)
            .flat_map(|y| (0..400_i64).map(move |x| (x, y)))
            .map(|(x, y)| (shape.coverage_at(x, y) * 255.0).round() as i32)
            .collect();
        let after = after.unwrap_or_default();
        assert_eq!(after.len(), want.len(), "{:?}", shape.shape);
        assert!(
            after.iter().zip(&want).all(|(a, b)| (a - b).abs() <= 1),
            "{:?} inverted {}",
            shape.shape,
            shape.inverted
        );
    }
    let _ = QuickMaskOutcome::Everything;
}

#[test]
fn dab_cost_on_the_grid_is_small() {
    // Dev-profile numbers are in PLAN.md; this only bounds them loosely.
    // Dabs alternate between two spots so each does real work, with no
    // grid fill inside the timed loop.
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    let mut mode = rig.qm_enter();
    fill_mask(&mut mode, 0);
    let c = white_on_black();
    let black = black_on_white();
    for radius in [24.0_f32, 100.0] {
        rig.settings.brush_radius = radius;
        let start = std::time::Instant::now();
        let n = 400_u32;
        for i in 0..n {
            let colours = if i % 2 == 0 { &c } else { &black };
            let _ = quick_mask::paint_dab(
                &mut mode,
                DabTool::Brush,
                (150.0 + (i % 7) as f32, 150.0),
                &rig.settings,
                colours,
            );
        }
        let per = start.elapsed().as_micros() / u128::from(n);
        eprintln!("quick mask dab radius {radius}: {per} us per dab");
        assert!(per < 20_000, "{per} us");
    }
}

#[test]
fn entering_quick_mask_mid_stroke_commits_the_layer_stroke_first() {
    let rig = Rig::new(rect(0, 0, 400, 300));
    let layer_stroke = Some(Drag::Brush {
        hard: false,
        clone: None,
        last_doc: (0.0, 0.0),
        carry: 0.0,
        stroke: Some(aurora_brush::StrokeSnapshot::new(rig.surface)),
        warned: std::collections::HashSet::new(),
    });
    let mask_stroke = Some(Drag::Eraser {
        last_doc: (0.0, 0.0),
        carry: 0.0,
        stroke: None,
        warned: std::collections::HashSet::new(),
    });
    assert!(quick_mask::drag_needs_commit_before_enter(
        layer_stroke.as_ref()
    ));
    assert!(!quick_mask::drag_needs_commit_before_enter(
        mask_stroke.as_ref()
    ));
    assert!(!quick_mask::drag_needs_commit_before_enter(None));
    assert!(!quick_mask::drag_needs_commit_before_enter(Some(
        &Drag::Eyedropper
    )));
    // A mask stroke is dropped on leaving; a layer stroke is not.
    let mut drag = mask_stroke;
    quick_mask::drop_mask_stroke(&mut drag);
    assert!(drag.is_none());
    let mut drag = layer_stroke;
    quick_mask::drop_mask_stroke(&mut drag);
    assert!(drag.is_some());
}

// -- The Pencil (0.220.0) ------------------------------------------------

/// Every texel of the layer with any non-zero alpha, as `(x, y)`.
fn alpha_set(rig: &mut Rig, w: u32, h: u32) -> std::collections::BTreeSet<(u32, u32)> {
    let mut out = std::collections::BTreeSet::new();
    for y in 0..h {
        for x in 0..w {
            if rig.at(x, y)[3] > 0.0 {
                out.insert((x, y));
            }
        }
    }
    out
}

#[test]
fn a_pencil_press_starts_a_hard_brush_drag_and_the_brush_a_soft_one() {
    let view = CanvasView::new();
    let pencil = begin_drag(
        Tool::Pencil,
        PointerButton::Primary,
        (5.0, 5.0),
        &view,
        None,
    );
    assert!(matches!(pencil, Some(Drag::Brush { hard: true, .. })));
    assert_eq!(crate::dab_tool_of_drag(pencil.as_ref()), DabTool::Pencil);
    let brush = begin_drag(Tool::Brush, PointerButton::Primary, (5.0, 5.0), &view, None);
    assert!(matches!(brush, Some(Drag::Brush { hard: false, .. })));
    assert_eq!(crate::dab_tool_of_drag(brush.as_ref()), DabTool::Brush);
    assert_eq!(crate::dab_tool_of_drag(None), DabTool::Brush);
    // Only the primary button draws.
    assert!(
        begin_drag(
            Tool::Pencil,
            PointerButton::Secondary,
            (5.0, 5.0),
            &view,
            None
        )
        .is_none()
    );
}

#[test]
fn a_pencil_stroke_is_hard_in_foreground_colour_and_one_labelled_undo_step() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.settings.set_radius(Tool::Pencil, 3.0);
    assert_eq!(rig.settings.radius(Tool::Brush), Some(3.0), "shared radius");
    assert!(rig.stroke(Tool::Pencil, &[(100.4, 100.6)], false));
    assert_eq!(rig.undo_order.undo_labels, ["Pencil Stroke"]);
    let set = alpha_set(&mut rig, 600, 400);
    assert_eq!(set.len(), 25, "radius 3 is a 5 x 5 block");
    for &(x, y) in &set {
        assert_eq!(
            rig.at(x, y),
            [1.0, 0.0, 0.0, 1.0],
            "full red, no partial alpha"
        );
    }
    assert!(set.contains(&(100, 100)));
    // The history row is the Pencil's, undo restores, redo repaints.
    let after = rig.snapshot(600, 400);
    assert!(rig.undo(UndoDirection::Undo));
    assert!(alpha_set(&mut rig, 600, 400).is_empty());
    assert!(rig.undo(UndoDirection::Redo));
    assert_eq!(rig.snapshot(600, 400), after);
}

#[test]
fn a_pencil_stroke_has_no_gaps_and_the_brush_still_says_brush_stroke() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.settings.set_radius(Tool::Pencil, 1.0);
    assert!(rig.stroke(Tool::Pencil, &[(10.0, 10.0), (200.0, 90.0)], false));
    let set = alpha_set(&mut rig, 600, 400);
    // 8-connected from the press to the end: every column in range has one.
    for x in 10..=199_u32 {
        assert!(set.iter().any(|&(px, _)| px == x), "column {x} has a texel");
    }
    for &(x, y) in &set {
        let neighbours = set
            .iter()
            .filter(|&&(a, b)| a.abs_diff(x) <= 1 && b.abs_diff(y) <= 1 && (a, b) != (x, y))
            .count();
        assert!(neighbours >= 1, "({x}, {y}) is isolated");
        assert_eq!(rig.at(x, y)[3], 1.0);
    }
    let mut other = Rig::new(rect(0, 0, 600, 400));
    assert!(other.stroke(Tool::Brush, &[(100.0, 100.0)], false));
    assert_eq!(other.undo_order.undo_labels, ["Brush Stroke"]);
}

#[test]
fn a_pencil_stroke_paints_through_a_selection_only() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee((100.0, 80.0), (300.0, 260.0));
    let sel = rect(100, 80, 200, 180);
    rig.settings.set_radius(Tool::Pencil, 8.0);
    assert!(rig.stroke(Tool::Pencil, &[(40.0, 170.0), (350.0, 170.0)], false));
    let set = alpha_set(&mut rig, 600, 400);
    assert!(set.len() > 1000);
    assert!(
        set.iter()
            .all(|&(x, y)| inside(sel, i64::from(x), i64::from(y)))
    );
    assert!(set.iter().any(|&(x, _)| x == 100) && set.iter().any(|&(x, _)| x == 299));
}

#[test]
fn a_pencil_dab_through_a_fractional_selection_blends_by_the_coverage() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.marquee_step(Tool::EllipticalMarquee, (100.0, 100.0), (300.0, 300.0));
    rig.settings.set_radius(Tool::Pencil, 6.0);
    // Along the ellipse's edge some covered texels have coverage < 1.
    assert!(rig.stroke(Tool::Pencil, &[(120.0, 120.0), (140.0, 140.0)], false));
    let set = alpha_set(&mut rig, 600, 400);
    let partial = set
        .iter()
        .filter(|&&(x, y)| {
            let a = rig.at(x, y)[3];
            a > 0.0 && a < 1.0
        })
        .count();
    let full = set.iter().filter(|&&(x, y)| rig.at(x, y)[3] == 1.0).count();
    assert!(partial > 0, "the antialiased selection edge blends");
    assert!(full > 0);
}

#[test]
fn a_transparency_locked_pencil_recolours_without_changing_alpha() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.rect_fill(90, 90, 110, 110, [0.0, 0.5, 0.0, 0.5]);
    let before: Vec<f32> = (90..110).map(|x| rig.at(x, 100)[3]).collect();
    rig.settings.set_radius(Tool::Pencil, 30.0);
    assert!(rig.stroke(Tool::Pencil, &[(100.5, 100.5)], true));
    let after: Vec<f32> = (90..110).map(|x| rig.at(x, 100)[3]).collect();
    assert_eq!(before, after, "alpha untouched");
    assert_eq!(rig.at(100, 100), [0.5, 0.0, 0.0, 0.5], "red at its alpha");
    assert_eq!(rig.at(120, 100), [0.0; 4], "transparent texels stay empty");
    assert_eq!(rig.alpha(89, 100), 0.0);
}

#[test]
fn pencil_locks_and_permissions_follow_the_brush() {
    use aurora_doc::LayerLock as L;
    let none = L::none();
    let t = L {
        transparency: true,
        ..none
    };
    let p = L {
        pixels: true,
        ..none
    };
    assert_eq!(
        crate::paint_permission(DabTool::Pencil, none),
        crate::PaintPermission::Allowed
    );
    assert_eq!(
        crate::paint_permission(DabTool::Pencil, t),
        crate::PaintPermission::PreserveAlpha
    );
    assert_eq!(
        crate::paint_permission(DabTool::Pencil, p),
        crate::PaintPermission::Refused(crate::LockRefusal::PixelsLocked)
    );
    assert_eq!(
        crate::press_refusal(Tool::Pencil, p),
        Some(crate::LockRefusal::PixelsLocked)
    );
    assert_eq!(crate::press_refusal(Tool::Pencil, t), None);
    assert_eq!(crate::press_refusal(Tool::Pencil, none), None);
    // The Pencil and Brush share one radius; the Eraser has its own.
    let mut settings = ToolSettings::default();
    assert!(settings.set_radius(Tool::Pencil, 9.0));
    assert_eq!(settings.dab_radius(DabTool::Pencil), 9.0);
    assert_eq!(settings.dab_radius(DabTool::Brush), 9.0);
    assert_eq!(settings.dab_radius(DabTool::Eraser), 24.0);
}

#[test]
fn a_pencil_with_no_pixel_layer_starts_a_drag_with_no_stroke_and_records_nothing() {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    let drag = begin_drag(
        Tool::Pencil,
        PointerButton::Primary,
        (50.0, 50.0),
        &rig.view,
        None,
    );
    assert!(matches!(
        drag,
        Some(Drag::Brush {
            stroke: None,
            hard: true,
            ..
        })
    ));
    let recorded = commit_ending_drag(
        drag,
        &rig.layers,
        &mut rig.history,
        &mut rig.pixel_history,
        &mut rig.undo_order,
        &mut rig.view,
        Some(rig.layer),
        Some((1000.0, 800.0)),
    );
    assert!(!recorded);
    assert!(rig.undo_order.undo_labels.is_empty());
}

#[test]
fn the_pencil_edits_the_quick_mask_hard_toward_the_foreground_luma() {
    let mut rig = Rig::new(rect(0, 0, 400, 300));
    rig.settings.set_radius(Tool::Pencil, 3.0);
    let mut mode = rig.qm_enter();
    let tiles = rig.tiles();
    assert_eq!(
        quick_mask::target_byte(DabTool::Pencil, &black_on_white()),
        0
    );
    assert_eq!(
        quick_mask::target_byte(DabTool::Pencil, &white_on_black()),
        255
    );
    let recorded = rig.qm_stroke(
        &mut mode,
        Tool::Pencil,
        &[(150.3, 120.7)],
        &black_on_white(),
    );
    assert!(!recorded, "mask strokes record no step");
    assert_eq!(rig.tiles(), tiles, "no layer pixel touched");
    let mut zeros = 0;
    for y in 100..140_i64 {
        for x in 130..170_i64 {
            let b = mode.mask.byte_at(x, y);
            assert!(
                b == 0 || b == 255,
                "hard: no in-between byte at ({x}, {y}): {b}"
            );
            if b == 0 {
                zeros += 1;
            }
        }
    }
    assert_eq!(zeros, 25, "radius 3 is a 5 x 5 block");
    assert_eq!(mode.mask.byte_at(150, 120), 0);
    assert!(quick_mask::skips_lock_gate(true, Tool::Pencil));
    assert_eq!(quick_mask::drag_layer(true, Tool::Pencil, Some(1)), None);
    assert_eq!(
        quick_mask::drag_layer(false, Tool::Pencil, Some(1)),
        Some(1)
    );
    assert!(quick_mask::tool_refusal(Tool::Pencil).is_none());
}

#[test]
fn the_pencil_is_selected_by_shift_b_and_the_palette_and_the_brush_by_b() {
    use aurora_widgets::shortcut::KeyChord;
    let registry = crate::default_shortcuts_for(false);
    let find = |source: &str| match KeyChord::parse(source) {
        Ok(chord) => registry.resolve(chord),
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(matches!(
        find("Shift+B"),
        Some(crate::AppCommand::SelectTool(Tool::Pencil))
    ));
    assert!(matches!(
        find("b"),
        Some(crate::AppCommand::SelectTool(Tool::Brush))
    ));
    assert_eq!(crate::tool_command_id(Tool::Pencil), "tool.pencil");
    assert_eq!(Tool::Pencil.shortcut_text(), "Shift+B");
}

// -- The Clone Stamp (0.221.0) ---------------------------------------------

impl Rig {
    /// A Clone Stamp stroke along `points` driven the way `App` does: the
    /// press decision (`clone_stamp::press`), then the drag, each dab
    /// stamped by `clone_stamp::clone_dab_through_selection`, then the
    /// commit. Returns the press and whether a history step was recorded.
    fn clone_stroke(
        &mut self,
        state: &mut crate::clone_stamp::CloneStamp,
        alt: bool,
        points: &[(f32, f32)],
        preserve_alpha: bool,
    ) -> (crate::clone_stamp::Press, bool) {
        let Some(&first) = points.first() else {
            unreachable!("a stroke has points");
        };
        let press = crate::clone_stamp::press(state, alt, Some(self.layer), first);
        let crate::clone_stamp::Press::Stroke(offset) = press else {
            return (press, false);
        };
        let mut drag = begin_drag(
            Tool::CloneStamp,
            PointerButton::Primary,
            first,
            &self.view,
            crate::active_pixel_layer(&self.layers, Some(self.layer)),
        );
        crate::clone_stamp::set_drag_offset(&mut drag, offset);
        let selection = self.selection.active();
        let dab_at = |rig: &mut Self, drag: &mut Option<Drag>, p: (f32, f32)| {
            let _ = crate::clone_stamp::clone_dab_through_selection(
                preserve_alpha,
                &mut rig.store,
                rig.surface,
                rig.bounds,
                p,
                offset,
                selection.clone(),
                &rig.settings,
                drag,
            );
        };
        dab_at(self, &mut drag, first);
        for &p in points.iter().skip(1) {
            let bounds = pan_bounds(&self.layers, Some(self.layer), Some((1000.0, 800.0)));
            let dabs = match drag.as_mut() {
                Some(d) => continue_drag(
                    d,
                    p,
                    &mut self.view,
                    &mut self.selection,
                    &self.settings,
                    bounds,
                ),
                None => Vec::new(),
            };
            for dab in dabs {
                dab_at(self, &mut drag, dab);
            }
        }
        let recorded = commit_ending_drag(
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.view,
            Some(self.layer),
            Some((1000.0, 800.0)),
        );
        (press, recorded)
    }
}

/// A rig with a distinctive opaque pattern at x 200..232, y 100..132.
fn clone_rig() -> Rig {
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.settings.set_radius(Tool::CloneStamp, 3.0);
    for y in 100..132u32 {
        for x in 200..232u32 {
            rig.put(
                x,
                y,
                [(x - 200) as f32 / 32.0, (y - 100) as f32 / 32.0, 0.5, 1.0],
            );
        }
    }
    rig
}

#[test]
fn a_clone_stroke_copies_the_source_at_the_aligned_offset_as_one_labelled_step() {
    let mut rig = clone_rig();
    let mut state = crate::clone_stamp::CloneStamp::default();
    let (set, recorded) = rig.clone_stroke(&mut state, true, &[(210.5, 110.5)], false);
    assert_eq!(set, crate::clone_stamp::Press::SourceSet);
    assert!(!recorded, "Alt+click is not a stroke");
    assert!(rig.undo_order.undo_labels.is_empty());
    let before = rig.snapshot(600, 400);
    // Press at (50.5, 60.5): offset = (210.5 - 50.5, 110.5 - 60.5) = (160, 50).
    let (press, recorded) = rig.clone_stroke(&mut state, false, &[(50.5, 60.5)], false);
    assert_eq!(press, crate::clone_stamp::Press::Stroke((160, 50)));
    assert!(recorded);
    assert_eq!(rig.undo_order.undo_labels, ["Clone Stamp Stroke"]);
    // The centre texel is an exact copy of the source texel it points at.
    let (src, dst) = (rig.at(210, 110), rig.at(50, 60));
    assert_eq!(dst, src);
    assert!(dst[3] > 0.0);
    // One texel to the right copies the texel to the right of the source.
    let falloff = aurora_brush::dab_falloff(1.0, 3.0);
    let right = rig.at(51, 60);
    let want = rig.at(211, 110);
    for c in 0..3 {
        let diff = (right[c] - want[c] * falloff).abs();
        assert!(diff < 2e-3, "channel {c}: {right:?} vs {want:?}");
    }
    // The source itself is untouched and undo restores everything.
    assert_eq!(rig.at(210, 110), src);
    assert!(rig.undo(UndoDirection::Undo));
    assert_eq!(rig.snapshot(600, 400), before);
    assert!(rig.undo(UndoDirection::Redo));
    assert_eq!(rig.at(50, 60), src);
}

#[test]
fn a_second_stroke_keeps_the_first_strokes_offset() {
    let mut rig = clone_rig();
    let mut state = crate::clone_stamp::CloneStamp::default();
    let _ = rig.clone_stroke(&mut state, true, &[(210.5, 110.5)], false);
    let _ = rig.clone_stroke(&mut state, false, &[(50.5, 60.5)], false);
    // Elsewhere: still offset (160, 50), so (80, 70) copies (240, 120)...
    // which is outside the pattern (x 200..232): transparent. Use (60, 75).
    let (press, _) = rig.clone_stroke(&mut state, false, &[(60.5, 75.5)], false);
    assert_eq!(press, crate::clone_stamp::Press::Stroke((160, 50)));
    assert_eq!(rig.at(60, 75), rig.at(220, 125));
    assert_eq!(rig.undo_order.undo_labels.len(), 2);
}

#[test]
fn a_stroke_never_clones_its_own_output() {
    // One opaque red texel is the only source content. Offset (-8, 0):
    // destination x reads source x - 8. Dragging right from x 50 to 60, a
    // live read would pass the red along (dest 58 reads dest 50, already
    // red); the stroke-start read leaves only dest 50 red.
    let mut rig = Rig::new(rect(0, 0, 600, 400));
    rig.settings.set_radius(Tool::CloneStamp, 3.0);
    rig.put(42, 50, [1.0, 0.0, 0.0, 1.0]);
    let mut state = crate::clone_stamp::CloneStamp::default();
    let _ = rig.clone_stroke(&mut state, true, &[(42.5, 50.5)], false);
    let (press, recorded) = rig.clone_stroke(
        &mut state,
        false,
        &[(50.5, 50.5), (55.5, 50.5), (60.5, 50.5)],
        false,
    );
    assert_eq!(press, crate::clone_stamp::Press::Stroke((-8, 0)));
    assert!(recorded);
    let mut lit = Vec::new();
    for y in 40..60 {
        for x in 40..70 {
            if (x, y) != (42, 50) && rig.at(x, y)[3] > 0.0 {
                lit.push((x, y));
            }
        }
    }
    assert_eq!(lit, vec![(50, 50)], "only the one copied texel");
}

#[test]
fn a_clone_paints_only_through_the_selection() {
    let mut rig = clone_rig();
    rig.marquee((48.0, 0.0), (52.0, 400.0));
    let mut state = crate::clone_stamp::CloneStamp::default();
    let _ = rig.clone_stroke(&mut state, true, &[(210.5, 110.5)], false);
    let (_, recorded) = rig.clone_stroke(&mut state, false, &[(50.5, 60.5)], false);
    assert!(recorded);
    let mut xs = std::collections::BTreeSet::new();
    for y in 0..400 {
        for x in 0..300 {
            if rig.at(x, y)[3] > 0.0 && !(200..232).contains(&x) {
                xs.insert(x);
            }
        }
    }
    assert!(!xs.is_empty());
    assert!(xs.iter().all(|x| (48..52).contains(x)), "{xs:?}");
}

#[test]
fn a_transparency_locked_clone_keeps_alpha() {
    let mut rig = clone_rig();
    rig.rect_fill(40, 50, 60, 70, [0.2, 0.2, 0.2, 0.5]);
    let mut state = crate::clone_stamp::CloneStamp::default();
    let _ = rig.clone_stroke(&mut state, true, &[(210.5, 110.5)], false);
    let (_, recorded) = rig.clone_stroke(&mut state, false, &[(50.5, 60.5)], true);
    assert!(recorded);
    for y in 50..70 {
        for x in 40..60 {
            assert_eq!(rig.at(x, y)[3], 0.5, "alpha kept at ({x}, {y})");
        }
    }
    assert_ne!(rig.at(50, 60)[0], 0.2, "the colour moved");
    // Transparent texels stay empty and no tile is created for them.
    assert_eq!(rig.at(300, 300), [0.0; 4]);
}

#[test]
fn a_press_with_no_source_changes_nothing() {
    let mut rig = clone_rig();
    let before = rig.snapshot(600, 400);
    let mut state = crate::clone_stamp::CloneStamp::default();
    let (press, recorded) = rig.clone_stroke(&mut state, false, &[(50.5, 60.5)], false);
    assert_eq!(
        press,
        crate::clone_stamp::Press::Refused(crate::LockRefusal::CloneNeedsSource)
    );
    assert!(!recorded);
    assert!(rig.undo_order.undo_labels.is_empty());
    assert_eq!(rig.snapshot(600, 400), before);
}
