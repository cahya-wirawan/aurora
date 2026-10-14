//! Copy, Cut, Copy Merged, Paste, Paste in Place, Layer via Copy and Layer
//! via Cut (0.235.0): the pure extraction, the document flows (driven
//! through the same free functions `App` calls: `clipboard_app::{copy_layer,
//! cut_active, publish, resolve_paste, place_clip_core}`,
//! `perform_layer_command`, `step_undo_order`), the shortcuts, the palette
//! and the menus. `App` itself needs a window, so its thin wrappers are not
//! run here.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    clippy::assertions_on_constants,
    clippy::needless_pass_by_value
)]

use std::collections::HashMap;

use aurora_core::Rect;
use aurora_doc::{LayerId, LayerLock, LayerTree, Selection, SelectionShape};
use aurora_tile::{SurfaceId, TileId, TileStore};
use aurora_widgets::shortcut::{Key, Modifiers};
use aurora_widgets::{ClickTracker, FocusManager, WidgetId};
use half::f16;

use crate::clipboard_app::{
    self, ClipCommand, CutError, PixelClipboard, copy_layer, cut_active, place_clip_core, publish,
    resolve_paste,
};
use crate::clipboard_pixels::{
    self as pixels, ClipError, ClipImage, MAX_CLIP_TEXELS, OsPaste, PixelClip, paste_bounds,
    paste_origin,
};
use crate::{
    AppCommand, ClipboardAccess, CompositeCache, Drag, LayerCommandContext, LayerControlsState,
    UndoDirection, UndoKind, UndoOrder, activate_command, default_shortcuts_for, handle_key,
    palette_commands, step_undo_order,
};

const W: f32 = 1600.0;
const H: f32 = 900.0;
const CANVAS: (u32, u32) = (600, 300);

fn ok<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn ok_opt<T>(v: Option<T>) -> T {
    match v {
        Some(v) => v,
        None => unreachable!("expected Some"),
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

fn rect_selection(r: Rect) -> Selection {
    Selection {
        bounds: r,
        inverted: false,
        shape: SelectionShape::Rect,
    }
}

fn ellipse_selection(r: Rect) -> Selection {
    Selection {
        bounds: r,
        inverted: false,
        shape: SelectionShape::Ellipse,
    }
}

fn h(v: f32) -> f16 {
    f16::from_f32(v)
}

/// A fake system clipboard holding text and an image.
#[derive(Default)]
struct FakeOs {
    text: Option<String>,
    image: Option<ClipImage>,
}

impl ClipboardAccess for FakeOs {
    fn get_text(&mut self) -> Option<String> {
        self.text.clone()
    }
    fn set_text(&mut self, text: String) {
        self.text = Some(text);
    }
    fn get_image(&mut self) -> Option<ClipImage> {
        self.image.clone()
    }
    fn set_image(&mut self, image: &ClipImage) {
        self.image = Some(image.clone());
    }
}

/// `App`'s document state for one layer-command flow, without a window.
struct Rig {
    _dir: tempfile::TempDir,
    store: TileStore,
    workspace: aurora_ui::Workspace,
    focus: FocusManager,
    scales: aurora_theme::Scales,
    layers: LayerTree,
    history: aurora_doc::History,
    pixel_history: aurora_brush::PixelHistory,
    undo_order: UndoOrder,
    layer_rows: HashMap<WidgetId, LayerId>,
    active: Option<LayerId>,
    state: LayerControlsState,
    cache: CompositeCache,
    view: aurora_ui::CanvasView,
    drag: Option<Drag>,
    click: ClickTracker,
    layer: LayerId,
    surface: SurfaceId,
}

impl Rig {
    fn new(bounds: Rect) -> Self {
        let dir = ok(tempfile::tempdir());
        let Some(budget) = std::num::NonZeroUsize::new(256) else {
            unreachable!("non-zero");
        };
        let store = ok(TileStore::new(dir.path().to_path_buf(), budget));
        let mut layers = LayerTree::new();
        let layer = ok(layers.add_pixel_layer("Layer 1", bounds, None));
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("pixel layer");
        };
        let scales = crate::test_workspace_scales();
        let mut workspace = aurora_ui::build_workspace(&scales);
        let layer_rows = ok(aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            &layers,
        ));
        let state = LayerControlsState {
            controls: Some(ok(aurora_ui::insert_layer_controls(
                &mut workspace.tree,
                workspace.layers,
                &scales,
            ))),
            ..LayerControlsState::default()
        };
        let _ = crate::sync_layer_controls(&mut workspace, &state, &layers, Some(layer), None);
        workspace.tree.compute_layout(W, H);
        Self {
            _dir: dir,
            store,
            workspace,
            focus: FocusManager::default(),
            scales,
            layers,
            history: aurora_doc::History::new(),
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            layer_rows,
            active: Some(layer),
            state,
            cache: CompositeCache::default(),
            view: aurora_ui::CanvasView::new(),
            drag: None,
            click: ClickTracker::default(),
            layer,
            surface,
        }
    }

    fn doc() -> Self {
        Self::new(rect(0, 0, CANVAS.0, CANVAS.1))
    }

    /// Writes one layer-local texel.
    fn put_on(&mut self, surface: SurfaceId, x: u32, y: u32, rgba: [f32; 4]) {
        let id = TileId {
            x: x / 256,
            y: y / 256,
        };
        let tile = ok(self.store.get_mut(surface, id));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        for (i, v) in rgba.iter().enumerate() {
            if let Some(s) = tile.texels_mut().get_mut(index + i) {
                *s = f16::from_f32(*v);
            }
        }
    }

    fn put(&mut self, x: u32, y: u32, rgba: [f32; 4]) {
        self.put_on(self.surface, x, y, rgba);
    }

    fn fill(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, rgba: [f32; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgba);
            }
        }
    }

    fn at_on(&mut self, surface: SurfaceId, x: u32, y: u32) -> [f16; 4] {
        let id = TileId {
            x: x / 256,
            y: y / 256,
        };
        if !self.store.contains_tile(surface, id) {
            return [f16::ZERO; 4];
        }
        let tile = ok(self.store.get(surface, id));
        let index = (((y % 256) * 256 + (x % 256)) as usize) * 4;
        let g = |o: usize| tile.texels().get(index + o).copied().unwrap_or(f16::NAN);
        [g(0), g(1), g(2), g(3)]
    }

    fn at(&mut self, x: u32, y: u32) -> [f16; 4] {
        self.at_on(self.surface, x, y)
    }

    /// Every texel of the first six tiles of `surface`.
    fn dump(&mut self, surface: SurfaceId) -> Vec<Vec<f16>> {
        let mut out = Vec::new();
        for ty in 0..2 {
            for tx in 0..3 {
                let id = TileId { x: tx, y: ty };
                if self.store.contains_tile(surface, id) {
                    out.push(ok(self.store.get(surface, id)).texels().to_vec());
                } else {
                    out.push(Vec::new());
                }
            }
        }
        out
    }

    fn copy(&mut self, selection: Option<&Selection>) -> Result<PixelClip, &'static str> {
        copy_layer(&mut self.store, &self.layers, self.active, selection)
    }

    fn paste(
        &mut self,
        clip: PixelClip,
        visible: Option<Rect>,
        in_place: bool,
        label: &'static str,
        cut: Option<crate::fill_tools::Filled>,
    ) -> Result<(), &'static str> {
        let mut cx = LayerCommandContext {
            workspace: &mut self.workspace,
            focus: &mut self.focus,
            scales: &self.scales,
            layers: &mut self.layers,
            history: &mut self.history,
            pixel_history: &mut self.pixel_history,
            undo_order: &mut self.undo_order,
            layer_rows: &mut self.layer_rows,
            active_layer: &mut self.active,
            view: &mut self.view,
            composite_cache: &mut self.cache,
            drag: &mut self.drag,
            layer_controls: &mut self.state,
            click: &mut self.click,
            canvas_size: CANVAS,
        };
        place_clip_core(
            &mut cx,
            &mut self.store,
            &clip,
            visible,
            in_place,
            label,
            cut,
        )
    }

    fn step(&mut self, direction: UndoDirection) -> bool {
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

    fn lock(&mut self, lock: LayerLock) {
        ok(self.layers.set_lock(self.layer, lock));
    }

    fn cut(
        &mut self,
        selection: Option<&Selection>,
    ) -> Result<(PixelClip, crate::fill_tools::Filled), CutError> {
        cut_active(&mut self.store, &self.layers, self.active, selection)
    }

    fn commit_cut(&mut self, filled: crate::fill_tools::Filled) -> bool {
        crate::commit_fill(
            &mut self.workspace,
            &self.layers,
            self.layer,
            filled,
            clipboard_app::CUT_LABEL,
            &mut self.cache,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
        )
    }

    fn labels(&self) -> Vec<String> {
        self.undo_order.undo_labels.clone()
    }
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

// -- extraction ---------------------------------------------------------------

#[test]
fn copy_trims_to_the_tight_box_of_the_content_with_no_selection() {
    let mut rig = Rig::doc();
    rig.fill(30, 40, 40, 45, RED);
    let clip = ok(rig.copy(None));
    assert_eq!(clip.rect(), rect(30, 40, 10, 5));
    assert_eq!(clip.pixels().len(), 10 * 5 * 4);
    assert!(
        clip.pixels()
            .chunks_exact(4)
            .all(|t| t == [h(1.0), f16::ZERO, f16::ZERO, h(1.0)])
    );
}

#[test]
fn copy_with_a_layer_offset_uses_document_coordinates() {
    let mut rig = Rig::new(rect(100, 50, 200, 100));
    rig.put(3, 4, GREEN);
    let clip = ok(rig.copy(None));
    assert_eq!(clip.rect(), rect(103, 54, 1, 1));
    assert_eq!(clip.origin(), (103, 54));
}

#[test]
fn copy_of_nothing_says_so_and_changes_nothing() {
    let mut rig = Rig::doc();
    assert_eq!(rig.copy(None), Err(pixels::NOTHING_TO_COPY_NOTICE));
    // A texel with zero alpha is not content, even with colour in it.
    rig.put(5, 5, [1.0, 1.0, 1.0, 0.0]);
    assert_eq!(rig.copy(None), Err(pixels::NOTHING_TO_COPY_NOTICE));
    // A selection away from the content copies nothing either.
    rig.fill(10, 10, 20, 20, RED);
    let away = rect_selection(rect(200, 200, 50, 50));
    assert_eq!(rig.copy(Some(&away)), Err(pixels::NOTHING_TO_COPY_NOTICE));
}

#[test]
fn copy_is_limited_to_the_selection_bounds_and_makes_no_tile() {
    let mut rig = Rig::doc();
    rig.fill(0, 0, 100, 100, RED);
    let selection = rect_selection(rect(20, 30, 10, 10));
    let clip = ok(rig.copy(Some(&selection)));
    assert_eq!(clip.rect(), rect(20, 30, 10, 10));
    // Tile (2, 0) was never written: copying must not create it.
    let _ = rig.copy(None);
    assert!(!rig.store.contains_tile(rig.surface, TileId { x: 2, y: 0 }));
}

#[test]
fn copy_scales_alpha_by_the_selection_coverage_so_soft_edges_stay_soft() {
    let mut rig = Rig::doc();
    rig.fill(0, 0, 100, 100, RED);
    let selection = ellipse_selection(rect(10, 10, 60, 60));
    let clip = ok(rig.copy(Some(&selection)));
    let mut partial = 0;
    let (ox, oy) = clip.origin();
    for y in 0..i64::from(clip.height()) {
        for x in 0..i64::from(clip.width()) {
            let c = selection.coverage_at(ox + x, oy + y);
            let index = (usize::try_from(y).unwrap_or(0) * clip.width() as usize
                + usize::try_from(x).unwrap_or(0))
                * 4;
            let alpha = clip.pixels().get(index + 3).copied().unwrap_or(f16::NAN);
            assert_eq!(alpha, h(c), "alpha at ({x}, {y}) is the coverage");
            // Premultiplied colour scales with it: red == alpha.
            assert_eq!(clip.pixels().get(index).copied(), Some(h(c)));
            if c > 0.0 && c < 1.0 {
                partial += 1;
            }
        }
    }
    assert!(partial > 0, "the ellipse has soft edge texels");
}

#[test]
fn a_clip_over_the_cap_cannot_exist() {
    assert!(PixelClip::new(8192, 8193, (0, 0), Vec::new()).is_none());
    assert!(8192u64 * 8193 > MAX_CLIP_TEXELS);
    assert_eq!(
        ClipError::TooLarge.notice(),
        pixels::TOO_LARGE_TO_COPY_NOTICE
    );
}

#[test]
fn copy_needs_a_pixel_layer() {
    let mut rig = Rig::doc();
    rig.active = None;
    assert_eq!(rig.copy(None), Err(pixels::NEEDS_PIXEL_LAYER_NOTICE));
    let group = ok(rig.layers.add_group("Group", None));
    rig.active = Some(group);
    assert_eq!(rig.copy(None), Err(pixels::NEEDS_PIXEL_LAYER_NOTICE));
}

// -- cut ----------------------------------------------------------------------

#[test]
fn cut_erases_exactly_the_copied_texels_as_one_undo_step_that_restores_bit_for_bit() {
    let mut rig = Rig::doc();
    rig.fill(10, 10, 60, 40, RED);
    rig.fill(300, 10, 320, 20, GREEN); // another tile
    rig.put(5, 5, [0.3, 0.3, 0.3, 0.0]); // zero alpha with colour: untouched
    let selection = rect_selection(rect(0, 0, 100, 100));
    let before = rig.dump(rig.surface);
    let (clip, filled) = match rig.cut(Some(&selection)) {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(clip.rect(), rect(10, 10, 50, 30));
    assert!(rig.commit_cut(filled));
    assert_eq!(rig.labels(), vec!["Cut".to_owned()]);
    assert_eq!(rig.at(30, 20), [f16::ZERO; 4], "erased");
    assert_eq!(
        rig.at(310, 15),
        [h(0.0), h(1.0), h(0.0), h(1.0)],
        "outside the selection"
    );
    assert_eq!(
        rig.at(5, 5),
        [h(0.3), h(0.3), h(0.3), f16::ZERO],
        "zero alpha is not cut"
    );
    let after = rig.dump(rig.surface);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(
        rig.dump(rig.surface),
        before,
        "undo restores every tile bit for bit"
    );
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.dump(rig.surface), after, "redo erases again");
    assert_eq!(rig.undo_order.undo.len(), 1, "one step");
}

#[test]
fn cut_with_soft_coverage_leaves_the_complement_of_what_the_clip_holds() {
    let mut rig = Rig::doc();
    rig.fill(0, 0, 100, 100, [0.5, 0.5, 0.5, 1.0]);
    let selection = ellipse_selection(rect(10, 10, 60, 60));
    let original = rig.dump(rig.surface);
    let (clip, filled) = match rig.cut(Some(&selection)) {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(rig.commit_cut(filled));
    let (ox, oy) = clip.origin();
    let mut checked = 0;
    for y in 10..70i64 {
        for x in 10..70i64 {
            let c = selection.coverage_at(x, y);
            let left = rig.at(x as u32, y as u32)[3].to_f32();
            let taken = if c <= 0.0 {
                0.0
            } else {
                let i = (usize::try_from(y - oy).unwrap_or(0) * clip.width() as usize
                    + usize::try_from(x - ox).unwrap_or(0))
                    * 4;
                clip.pixels().get(i + 3).map_or(0.0, |a| a.to_f32())
            };
            // What is left plus what was taken is the original alpha, to
            // f16 rounding.
            assert!(
                (left + taken - 1.0).abs() < 2.0e-3,
                "({x}, {y}): {left} + {taken}"
            );
            checked += 1;
        }
    }
    assert!(checked > 0);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(rig.dump(rig.surface), original);
}

#[test]
fn cut_with_no_selection_clears_the_whole_layer_content() {
    let mut rig = Rig::doc();
    rig.fill(5, 5, 15, 15, RED);
    rig.fill(400, 100, 410, 110, GREEN);
    let (clip, filled) = match rig.cut(None) {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(clip.rect(), rect(5, 5, 405, 105));
    assert!(rig.commit_cut(filled));
    assert_eq!(rig.at(7, 7), [f16::ZERO; 4]);
    assert_eq!(rig.at(405, 105), [f16::ZERO; 4]);
}

#[test]
fn cut_is_refused_on_a_pixels_lock_and_on_a_transparency_lock_and_changes_nothing() {
    let mut rig = Rig::doc();
    rig.fill(10, 10, 20, 20, RED);
    let before = rig.dump(rig.surface);
    rig.lock(LayerLock {
        pixels: true,
        ..LayerLock::default()
    });
    assert!(matches!(
        rig.cut(None),
        Err(CutError::Refused(pixels::CUT_PIXELS_LOCKED_NOTICE))
    ));
    rig.lock(LayerLock {
        transparency: true,
        ..LayerLock::default()
    });
    assert!(matches!(
        rig.cut(None),
        Err(CutError::Refused(pixels::CUT_TRANSPARENCY_LOCKED_NOTICE))
    ));
    assert_eq!(rig.dump(rig.surface), before);
    assert!(rig.undo_order.undo.is_empty(), "no history row");
    // Copy is allowed under both locks.
    assert!(rig.copy(None).is_ok());
    // A position lock does not stop a cut.
    rig.lock(LayerLock {
        position: true,
        ..LayerLock::default()
    });
    assert!(rig.cut(None).is_ok());
}

#[test]
fn cut_of_nothing_is_refused_and_creates_no_tile() {
    let mut rig = Rig::doc();
    let selection = rect_selection(rect(300, 100, 40, 40));
    assert!(matches!(
        rig.cut(Some(&selection)),
        Err(CutError::Refused(pixels::NOTHING_TO_COPY_NOTICE))
    ));
    assert!(!rig.store.contains_tile(rig.surface, TileId { x: 1, y: 0 }));
}

// -- copy merged --------------------------------------------------------------

#[test]
fn copy_merged_equals_the_export_composite_cropped() {
    let mut rig = Rig::doc();
    rig.fill(0, 0, 600, 300, [0.2, 0.4, 0.6, 1.0]);
    let top = ok(rig
        .layers
        .add_pixel_layer("Top", rect(0, 0, CANVAS.0, CANVAS.1), None));
    let Some(top_surface) = rig.layers.surface_id(top) else {
        unreachable!("pixel layer");
    };
    for y in 50..120 {
        for x in 100..220 {
            rig.put_on(top_surface, x, y, [0.5, 0.0, 0.0, 0.5]);
        }
    }
    let selection = rect_selection(rect(90, 40, 200, 100));
    let clip = ok(pixels::extract_merged(
        &rig.layers,
        &mut rig.store,
        CANVAS,
        Some(&selection),
    ));
    assert_eq!(
        clip.rect(),
        rect(90, 40, 200, 100),
        "the opaque backdrop fills it"
    );
    let composite = ok(crate::composite_document(
        &rig.layers,
        &mut rig.store,
        CANVAS.0,
        CANVAS.1,
    ));
    let straight = composite.samples();
    for y in 0..100usize {
        for x in 0..200usize {
            let src = ((40 + y) * CANVAS.0 as usize + (90 + x)) * 4;
            let dst = (y * 200 + x) * 4;
            let (Some(s), Some(d)) = (straight.get(src..src + 4), clip.pixels().get(dst..dst + 4))
            else {
                unreachable!("in range");
            };
            let alpha = s[3].to_f32();
            assert_eq!(d[3], s[3]);
            for c in 0..3 {
                assert_eq!(d[c], h(s[c].to_f32() * alpha), "channel {c} at ({x}, {y})");
            }
        }
    }
    // The whole canvas with no selection.
    let all = ok(pixels::extract_merged(
        &rig.layers,
        &mut rig.store,
        CANVAS,
        None,
    ));
    assert_eq!(all.rect(), rect(0, 0, CANVAS.0, CANVAS.1));
}

#[test]
fn copy_merged_scales_alpha_by_the_selection_and_says_when_nothing_shows() {
    let mut rig = Rig::doc();
    let empty = pixels::extract_merged(&rig.layers, &mut rig.store, CANVAS, None);
    assert_eq!(empty, Err(ClipError::Nothing));
    rig.fill(0, 0, 100, 100, RED);
    let selection = ellipse_selection(rect(10, 10, 60, 60));
    let clip = ok(pixels::extract_merged(
        &rig.layers,
        &mut rig.store,
        CANVAS,
        Some(&selection),
    ));
    let (ox, oy) = clip.origin();
    let row = i64::from(clip.height()) / 2;
    let c = selection.coverage_at(ox, oy + row);
    let i = usize::try_from(row).unwrap_or(0) * clip.width() as usize * 4;
    assert_eq!(clip.pixels().get(i + 3).copied(), Some(h(c)));
}

// -- placement ----------------------------------------------------------------

#[test]
fn paste_origin_follows_the_visible_original_else_centres_and_clamps() {
    let clip = rect(100, 80, 40, 20);
    let view = Some(rect(0, 0, 400, 300));
    // Original fully visible: stays.
    assert_eq!(paste_origin(clip, view, CANVAS, false), (100, 80));
    // Not fully visible: centred in the visible canvas area.
    let panned = Some(rect(300, 100, 200, 100));
    assert_eq!(paste_origin(clip, panned, CANVAS, false), (380, 140));
    // Paste in Place ignores the view.
    assert_eq!(paste_origin(clip, panned, CANVAS, true), (100, 80));
    // Partly visible counts as not visible.
    let partly = Some(rect(110, 0, 400, 300));
    assert_ne!(paste_origin(clip, partly, CANVAS, false), (100, 80));
    // Clamped inside the canvas: the visible area reaches past its edge.
    let beyond = Some(rect(580, 280, 400, 400));
    assert_eq!(paste_origin(clip, beyond, CANVAS, false), (560, 280));
    // Larger than the canvas: the origin is the canvas corner.
    let huge = rect(0, 0, 700, 400);
    assert_eq!(
        paste_origin(huge, Some(rect(0, 0, 100, 100)), CANVAS, false),
        (0, 0)
    );
    // No view at all: the canvas centre.
    assert_eq!(paste_origin(clip, None, CANVAS, false), (280, 140));
    // A view away from the document falls back to the canvas centre.
    assert_eq!(
        paste_origin(clip, Some(rect(5000, 5000, 100, 100)), CANVAS, false),
        (280, 140)
    );
}

#[test]
fn paste_bounds_are_the_canvas_grown_to_hold_the_clip() {
    assert_eq!(
        paste_bounds(CANVAS, rect(10, 10, 5, 5)),
        rect(0, 0, 600, 300)
    );
    assert_eq!(
        paste_bounds(CANVAS, rect(-20, 10, 30, 5)),
        rect(-20, 0, 620, 300)
    );
    assert_eq!(
        paste_bounds(CANVAS, rect(590, 290, 30, 30)),
        rect(0, 0, 620, 320)
    );
}

#[test]
fn visible_rect_is_the_document_area_the_view_shows() {
    let view = aurora_ui::CanvasView::new();
    let Some(r) = clipboard_app::visible_rect(&view, (800.0, 600.0)) else {
        unreachable!("finite")
    };
    assert!(r.width >= 799 && r.height >= 599);
    assert_eq!(clipboard_app::visible_rect(&view, (f32::NAN, 1.0)), None);
}

// -- paste flows --------------------------------------------------------------

fn sample_clip(origin: (i64, i64)) -> PixelClip {
    let mut px = Vec::new();
    for i in 0..12u32 {
        px.extend([h(i as f32 / 16.0), h(0.0), h(0.0), h(1.0)]);
    }
    ok_opt(PixelClip::new(4, 3, origin, px))
}

#[test]
fn paste_makes_a_named_layer_above_the_active_one_as_one_step_and_undo_redo_are_exact() {
    let mut rig = Rig::doc();
    rig.fill(0, 0, 10, 10, RED);
    let revision = rig.undo_order.content_revision;
    let layers_before = crate::layer_ids_in_order(&rig.layers);
    let view = Some(rect(0, 0, 400, 300));
    ok(rig.paste(sample_clip((20, 30)), view, false, "Paste", None));
    let after = crate::layer_ids_in_order(&rig.layers);
    assert_eq!(after.len(), layers_before.len() + 1);
    let Some(new) = rig.active else {
        unreachable!("the new layer is active")
    };
    assert_ne!(new, rig.layer);
    assert_eq!(rig.layers.name(new), Some("Layer 2"));
    // Directly above the previously active node.
    assert_eq!(after.first().copied(), Some(new));
    assert_eq!(rig.labels(), vec!["Paste".to_owned()]);
    assert_ne!(
        rig.undo_order.content_revision, revision,
        "a content change: unsaved"
    );
    let Some(surface) = rig.layers.surface_id(new) else {
        unreachable!("pixel layer")
    };
    assert_eq!(rig.at_on(surface, 21, 31)[0], h(5.0 / 16.0));
    assert_eq!(rig.at_on(surface, 19, 30), [f16::ZERO; 4]);
    let snapshot = rig.dump(surface);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(
        crate::layer_ids_in_order(&rig.layers),
        layers_before,
        "layer set restored"
    );
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(crate::layer_ids_in_order(&rig.layers), after);
    assert_eq!(
        rig.dump(surface),
        snapshot,
        "the pasted pixels survive undo and redo"
    );
    assert_eq!(rig.undo_order.undo.len(), 1);
}

#[test]
fn paste_when_the_original_is_off_screen_centres_and_paste_in_place_does_not() {
    let mut rig = Rig::doc();
    let view = Some(rect(300, 100, 200, 100));
    ok(rig.paste(sample_clip((0, 0)), view, false, "Paste", None));
    let Some(new) = rig.active else {
        unreachable!()
    };
    let Some(s) = rig.layers.surface_id(new) else {
        unreachable!()
    };
    assert_ne!(
        rig.at_on(s, 398, 148)[3],
        f16::ZERO,
        "centred at (398, 148)"
    );
    let mut rig = Rig::doc();
    ok(rig.paste(sample_clip((7, 9)), view, true, "Paste", None));
    let Some(new) = rig.active else {
        unreachable!()
    };
    let Some(s) = rig.layers.surface_id(new) else {
        unreachable!()
    };
    assert_ne!(
        rig.at_on(s, 7, 9)[3],
        f16::ZERO,
        "Paste in Place keeps the origin"
    );
}

#[test]
fn a_clip_partly_outside_the_canvas_is_kept_whole_in_a_grown_layer() {
    let mut rig = Rig::doc();
    ok(rig.paste(sample_clip((-2, 1)), None, true, "Paste", None));
    let Some(new) = rig.active else {
        unreachable!()
    };
    assert_eq!(rig.layers.bounds(new), Some(rect(-2, 0, 602, 300)));
    let copy = ok(copy_layer(&mut rig.store, &rig.layers, rig.active, None));
    assert_eq!(
        copy.rect(),
        rect(-2, 1, 4, 3),
        "nothing was lost off the left edge"
    );
}

#[test]
fn layer_via_copy_is_one_step_and_equals_copy_then_paste_in_place() {
    let mut rig = Rig::doc();
    rig.fill(40, 40, 70, 60, RED);
    let selection = rect_selection(rect(30, 30, 30, 30));
    let clip = ok(rig.copy(Some(&selection)));
    let source = rig.dump(rig.surface);
    ok(rig.paste(
        clip.clone(),
        Some(rect(0, 0, 10, 10)),
        true,
        "Layer via Copy",
        None,
    ));
    let Some(via) = rig.active else {
        unreachable!()
    };
    assert_eq!(rig.labels(), vec!["Layer via Copy".to_owned()]);
    assert_eq!(rig.undo_order.undo.len(), 1);
    assert_eq!(
        rig.dump(rig.surface),
        source,
        "the source layer is untouched"
    );
    // The same clip pasted in place by a plain Paste gives the same pixels.
    let mut other = Rig::doc();
    other.fill(40, 40, 70, 60, RED);
    ok(other.paste(clip, Some(rect(0, 0, 10, 10)), true, "Paste", None));
    let Some(plain) = other.active else {
        unreachable!()
    };
    let (Some(a), Some(b)) = (rig.layers.surface_id(via), other.layers.surface_id(plain)) else {
        unreachable!()
    };
    assert_eq!(rig.dump(a), other.dump(b));
}

#[test]
fn layer_via_cut_is_one_step_that_undoes_and_redoes_both_halves_exactly() {
    let mut rig = Rig::doc();
    rig.fill(40, 40, 70, 60, RED);
    rig.fill(300, 40, 310, 50, GREEN);
    let selection = rect_selection(rect(30, 30, 60, 60));
    let source_before = rig.dump(rig.surface);
    let layers_before = crate::layer_ids_in_order(&rig.layers);
    let (clip, filled) = match rig.cut(Some(&selection)) {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    };
    ok(rig.paste(clip, None, true, "Layer via Cut", Some(filled)));
    let Some(new) = rig.active else {
        unreachable!()
    };
    let Some(new_surface) = rig.layers.surface_id(new) else {
        unreachable!()
    };
    assert_eq!(
        rig.undo_order.undo,
        vec![UndoKind::LayerViaCut],
        "one combined step"
    );
    assert_eq!(rig.labels(), vec!["Layer via Cut".to_owned()]);
    assert_eq!(rig.at(50, 50), [f16::ZERO; 4], "the source was cut");
    assert_eq!(rig.at(305, 45)[1], h(1.0), "outside the selection stays");
    assert_ne!(
        rig.at_on(new_surface, 50, 50)[3],
        f16::ZERO,
        "the new layer holds it"
    );
    let after_source = rig.dump(rig.surface);
    let after_new = rig.dump(new_surface);
    assert!(rig.step(UndoDirection::Undo));
    assert_eq!(
        rig.dump(rig.surface),
        source_before,
        "the cut is undone bit for bit"
    );
    assert_eq!(
        crate::layer_ids_in_order(&rig.layers),
        layers_before,
        "the layer is gone"
    );
    assert!(rig.undo_order.undo.is_empty());
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.dump(rig.surface), after_source);
    assert_eq!(rig.dump(new_surface), after_new);
    assert_eq!(rig.undo_order.undo, vec![UndoKind::LayerViaCut]);
    // A second round trip is stable.
    assert!(rig.step(UndoDirection::Undo));
    assert!(rig.step(UndoDirection::Redo));
    assert_eq!(rig.dump(rig.surface), after_source);
}

#[test]
fn a_layer_via_cut_that_cannot_make_its_layer_takes_the_cut_back() {
    let mut rig = Rig::doc();
    rig.fill(40, 40, 70, 60, RED);
    let before = rig.dump(rig.surface);
    let (_, filled) = match rig.cut(None) {
        Ok(v) => v,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(rig.at(50, 50), [f16::ZERO; 4]);
    // A clip wider than a document may be: the layer is refused.
    let width = aurora_core::MAX_DOCUMENT_EXTENT + 1;
    let wide = ok_opt(PixelClip::new(
        width,
        1,
        (0, 0),
        vec![h(1.0); width as usize * 4],
    ));
    let result = rig.paste(wide, None, true, "Layer via Cut", Some(filled));
    assert_eq!(result, Err(pixels::PASTE_FAILED_NOTICE));
    assert_eq!(rig.dump(rig.surface), before, "the erasure was taken back");
    assert!(rig.undo_order.undo.is_empty());
}

// -- the clipboards -----------------------------------------------------------

#[test]
fn copy_exports_8_bit_straight_alpha_and_paste_prefers_the_internal_clip() {
    let mut rig = Rig::doc();
    rig.fill(10, 10, 14, 14, [0.25, 0.5, 0.0, 0.5]);
    let clip = ok(rig.copy(None));
    let mut os = FakeOs::default();
    let mut state = PixelClipboard::default();
    publish(&mut state, clip.clone(), &mut os);
    let Some(image) = os.image.clone() else {
        unreachable!("an image was exported")
    };
    assert_eq!((image.width, image.height), (4, 4));
    assert_eq!(image.rgba.len(), 4 * 4 * 4);
    // Straight alpha: premultiplied (0.25, 0.5, 0) at alpha 0.5 is
    // (0.5, 1.0, 0) at 0.5.
    for p in image.rgba.chunks_exact(4) {
        assert!((i32::from(p[0]) - 128).abs() <= 2, "{p:?}");
        assert!(p[1] >= 253, "{p:?}");
        assert_eq!(p[2], 0);
        assert!((i32::from(p[3]) - 128).abs() <= 2, "{p:?}");
    }
    // The image on the clipboard is the one we put there: the lossless
    // internal clip is used.
    assert_eq!(resolve_paste(&state, &mut os), Ok(clip));
}

#[test]
fn paste_takes_a_foreign_os_image_and_promotes_it_to_f16() {
    let mut os = FakeOs::default();
    let state = PixelClipboard::default();
    assert_eq!(
        resolve_paste(&state, &mut os),
        Err(pixels::NOTHING_TO_PASTE_NOTICE)
    );
    os.image = Some(ClipImage {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
    });
    let clip = ok(resolve_paste(&state, &mut os));
    assert_eq!((clip.width(), clip.height(), clip.origin()), (2, 1, (0, 0)));
    assert_eq!(
        clip.pixels().get(0..4),
        Some(&[h(1.0), h(0.0), h(0.0), h(1.0)][..])
    );
    let a = aurora_color::promote_u8(128);
    assert_eq!(
        clip.pixels().get(4..8),
        Some(&[h(0.0), h(a), h(0.0), h(a)][..]),
        "premultiplied on the way in"
    );
}

#[test]
fn an_image_that_differs_from_the_last_export_wins_over_the_internal_clip() {
    let mut os = FakeOs::default();
    let mut state = PixelClipboard::default();
    publish(&mut state, sample_clip((3, 3)), &mut os);
    let ours = ok(resolve_paste(&state, &mut os));
    assert_eq!(ours.origin(), (3, 3));
    // Something else is copied afterwards.
    os.image = Some(ClipImage {
        width: 1,
        height: 1,
        rgba: vec![1, 2, 3, 255],
    });
    let theirs = ok(resolve_paste(&state, &mut os));
    assert_eq!((theirs.width(), theirs.height()), (1, 1));
    // Text on the clipboard (no image) falls back to the internal clip.
    os.image = None;
    assert_eq!(ok(resolve_paste(&state, &mut os)).origin(), (3, 3));
}

#[test]
fn an_os_image_over_the_cap_is_refused_and_a_broken_one_ignored() {
    let image = |width, height, rgba| {
        Some(ClipImage {
            width,
            height,
            rgba,
        })
    };
    assert_eq!(
        pixels::os_image_paste(image(8192, 8193, Vec::new()), None),
        OsPaste::TooLarge
    );
    let mut os = FakeOs {
        image: image(100_000, 100_000, Vec::new()),
        ..FakeOs::default()
    };
    assert_eq!(
        resolve_paste(&PixelClipboard::default(), &mut os),
        Err(pixels::TOO_LARGE_TO_PASTE_NOTICE)
    );
    // A wrong byte count or an empty image is not an image.
    assert_eq!(
        pixels::os_image_paste(image(2, 2, vec![0; 3]), None),
        OsPaste::Internal
    );
    assert_eq!(
        pixels::os_image_paste(image(0, 4, Vec::new()), None),
        OsPaste::Internal
    );
}

#[test]
fn the_rgba8_round_trip_of_opaque_texels_is_within_one_level() {
    let mut px = Vec::new();
    for v in [0u8, 1, 127, 128, 254, 255] {
        let g = aurora_color::promote_u8(v);
        px.extend([h(g), h(g), h(g), h(1.0)]);
    }
    let clip = ok_opt(PixelClip::new(6, 1, (0, 0), px));
    let rgba = clip.to_rgba8();
    let back = ok_opt(PixelClip::from_rgba8(6, 1, &rgba, (0, 0)));
    assert_eq!(back.pixels().len(), clip.pixels().len());
    for (a, b) in clip.pixels().iter().zip(back.pixels()) {
        assert!((a.to_f32() - b.to_f32()).abs() <= 1.0 / 255.0, "{a} vs {b}");
    }
}

// -- commands, shortcuts, menus -----------------------------------------------

const FAMILY: [(char, bool, ClipCommand); 7] = [
    ('c', false, ClipCommand::Copy),
    ('x', false, ClipCommand::Cut),
    ('c', true, ClipCommand::CopyMerged),
    ('v', false, ClipCommand::Paste),
    ('v', true, ClipCommand::PasteInPlace),
    ('j', false, ClipCommand::LayerViaCopy),
    ('j', true, ClipCommand::LayerViaCut),
];

fn chord(primary_is_meta: bool, shift: bool, ch: char) -> aurora_widgets::shortcut::KeyChord {
    aurora_widgets::shortcut::KeyChord::new(
        Modifiers {
            control: !primary_is_meta,
            meta: primary_is_meta,
            shift,
            alt: false,
        },
        Key::Character(ch),
    )
}

#[test]
fn the_seven_chords_resolve_on_both_platform_tables_without_conflicts() {
    for primary_is_meta in [false, true] {
        let registry = default_shortcuts_for(primary_is_meta);
        for (ch, shift, command) in FAMILY {
            assert_eq!(
                registry.resolve(chord(primary_is_meta, shift, ch)),
                Some(&AppCommand::Clipboard(command)),
                "{ch} shift={shift} meta={primary_is_meta}"
            );
        }
        // The bare tool letters are still tools, and the existing chords
        // kept their bindings.
        let bare = |ch| {
            registry.resolve(aurora_widgets::shortcut::KeyChord::new(
                Modifiers::none(),
                Key::Character(ch),
            ))
        };
        assert!(matches!(bare('c'), Some(AppCommand::SelectTool(_))));
        assert!(matches!(bare('v'), Some(AppCommand::SelectTool(_))));
        let ctrl = |ch, shift| registry.resolve(chord(primary_is_meta, shift, ch));
        assert_eq!(ctrl('z', false), Some(&AppCommand::Undo));
        assert_eq!(ctrl('a', false), Some(&AppCommand::SelectAll));
        assert_eq!(ctrl('i', true), Some(&AppCommand::InverseSelection));
        assert_eq!(ctrl('n', true), Some(&AppCommand::NewLayer));
    }
}

fn run_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    modifiers: Modifiers,
    ch: char,
) -> Option<crate::ActivatedCommand> {
    let mut tool = aurora_ui::Tool::Move;
    handle_key(
        workspace,
        focus,
        &mut None,
        &mut None,
        &mut tool,
        &crate::ToolSettings::default(),
        &mut LayerTree::new(),
        &mut aurora_doc::History::new(),
        &mut aurora_brush::PixelHistory::new(),
        None,
        &mut UndoOrder::default(),
        &crate::default_shortcuts(),
        modifiers,
        Key::Character(ch),
        Some(&ch.to_string()),
        &mut crate::tests::FakeClipboard::default(),
        &mut crate::tests::FakeFileDialog::default(),
    )
}

#[test]
fn a_focused_text_field_keeps_the_chords_and_the_pixel_clipboard_is_untouched() {
    // Primary+C/X/V are claimed by the field before the registry ...
    for (ch, command) in [
        ('c', aurora_widgets::widgets::TextFieldChord::Copy),
        ('x', aurora_widgets::widgets::TextFieldChord::Cut),
        ('v', aurora_widgets::widgets::TextFieldChord::Paste),
    ] {
        for primary_is_meta in [false, true] {
            let modifiers = Modifiers {
                control: !primary_is_meta,
                meta: primary_is_meta,
                shift: false,
                alt: false,
            };
            assert_eq!(
                aurora_widgets::widgets::TextFieldChord::from_key(
                    &Key::Character(ch),
                    modifiers,
                    primary_is_meta
                ),
                Some(command)
            );
        }
    }
    // ... and every chord of the family is not run from inside a field by
    // `handle_key` (the Shift chords and Primary+J are not the field's).
    let scales = crate::test_workspace_scales();
    let mut workspace = aurora_ui::build_workspace(&scales);
    let field = ok(aurora_widgets::widgets::insert_text_field(
        &mut workspace.tree,
        workspace.root,
        &scales,
        "Name",
        "",
    ));
    workspace.tree.compute_layout(W, H);
    let mut focus = FocusManager::default();
    ok(focus.focus(&mut workspace.tree, field));
    for (ch, shift, _) in FAMILY {
        let modifiers = crate::tests::with_primary(Modifiers {
            shift,
            ..Modifiers::none()
        });
        assert_eq!(
            run_key(&mut workspace, &mut focus, modifiers, ch),
            None,
            "{ch} shift={shift} must stay with the field"
        );
    }
    // Without a focused field the same chords are the clipboard commands.
    let mut focus = FocusManager::default();
    for (ch, shift, command) in FAMILY {
        let modifiers = crate::tests::with_primary(Modifiers {
            shift,
            ..Modifiers::none()
        });
        assert_eq!(
            run_key(&mut workspace, &mut focus, modifiers, ch),
            Some(crate::ActivatedCommand::Clipboard(command))
        );
    }
}

#[test]
fn the_palette_lists_and_activates_every_command() {
    let commands = palette_commands();
    for (id, title, command) in clipboard_app::COMMANDS {
        assert!(
            commands.iter().any(|c| c.id == id && c.title == title),
            "{id} is in the palette"
        );
        assert_eq!(clipboard_app::command_for(id), Some(command));
        assert_eq!(command.id(), id);
        let mut workspace = aurora_ui::build_workspace(&crate::test_workspace_scales());
        let mut focus = FocusManager::default();
        assert_eq!(
            activate_command(
                &mut workspace,
                &mut focus,
                id,
                &mut crate::tests::FakeFileDialog::default()
            ),
            Some(crate::ActivatedCommand::Clipboard(command))
        );
    }
    let titles: Vec<&str> = clipboard_app::COMMANDS.iter().map(|c| c.1).collect();
    assert_eq!(
        titles,
        [
            "Copy",
            "Cut",
            "Copy Merged",
            "Paste",
            "Paste in Place",
            "Layer via Copy",
            "Layer via Cut"
        ]
    );
}

fn context() -> crate::menu_model::MenuContext {
    crate::menu_model::MenuContext {
        can_undo: false,
        can_redo: false,
        delete_allowed: true,
        quick_mask: false,
        clipped: false,
        clip_refused: false,
        workspaces: Vec::new(),
        documents: Vec::new(),
        saved_selections: Vec::new(),
    }
}

#[test]
fn the_edit_menu_lists_the_commands_with_hints_and_quick_mask_disables_them() {
    let shortcuts = crate::default_shortcuts();
    let menus = crate::menu_model::build_menus(&context(), &shortcuts);
    let edit = menus
        .iter()
        .find(|m| m.title == "Edit")
        .map(|m| m.entries.clone())
        .unwrap_or_default();
    for (id, _, _) in clipboard_app::COMMANDS {
        let entry = edit.iter().find_map(|e| match e {
            crate::menu_model::Entry::Item {
                command,
                hint,
                enabled,
                ..
            } if command == id => Some((hint.clone(), *enabled)),
            _ => None,
        });
        let Some((hint, enabled)) = entry else {
            unreachable!("{id} is in the Edit menu")
        };
        assert!(enabled);
        assert!(
            hint.is_some_and(|h| !h.is_empty()),
            "{id} shows its shortcut"
        );
    }
    let mut masked = context();
    masked.quick_mask = true;
    for (id, _, _) in clipboard_app::COMMANDS {
        assert!(
            !crate::menu_model::enabled(id, &masked),
            "{id} refused in Quick Mask"
        );
        assert!(crate::menu_model::enabled(id, &context()));
    }
}

#[test]
fn the_native_macos_menu_list_names_every_command() {
    for (id, _, _) in clipboard_app::COMMANDS {
        assert!(
            crate::menu_model::NATIVE_MENU_COMMANDS.contains(&id),
            "{id} is in the native menu cross-check"
        );
    }
}
