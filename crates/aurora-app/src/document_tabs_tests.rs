//! Several open documents and switching (0.172.0, document tabs R3): the
//! acceptance tests for failure modes F1–F8 of the tabs plan.

use std::collections::{HashMap, HashSet};

use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey};
use aurora_widgets::{ClickTracker, FocusManager, WidgetId};

use crate::document_session::{DocumentContents, DocumentId, DocumentSession, DocumentShelf};
use crate::document_tabs::{
    self, DocumentCommand, PendingOpenStores, SwitchContext, SwitchOutcome,
};
use crate::tests::{
    FakeClipboard, FakeFileDialog, fill_solid, read_all_texels, real_gpu_context,
    render_and_sample_pixel, with_primary,
};
use crate::{
    ActivatedCommand, AppCommand, CurvesUiState, Drag, LayerControlsState, PendingCurves,
    PendingOpacity, ScrollFollow, StartupPanels, ToolSettings, autosave_files, background_autosave,
    composite_surface_id, install_startup_panels, load_scales, recomposite_visible_tiles,
    surface_id_for,
};

const TILE: aurora_tile::TileId = aurora_tile::TileId { x: 0, y: 0 };

fn store_in(dir: &tempfile::TempDir) -> aurora_tile::TileStore {
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero")
    };
    match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    }
}

fn tempdir() -> tempfile::TempDir {
    match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("{err:?}"),
    }
}

/// A document of `layers` pixel layers whose *bottom* layer is active and
/// holds one solid tile of `rgba`, in its own store.
fn session(
    name: &str,
    layers: usize,
    rgba: [f32; 4],
    dir: &tempfile::TempDir,
) -> (DocumentSession, Vec<aurora_doc::LayerId>) {
    let mut tree = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 256,
        height: 256,
    };
    let ids: Vec<aurora_doc::LayerId> = (0..layers)
        .map(
            |n| match tree.add_pixel_layer(format!("{name} {n}"), bounds, None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            },
        )
        .collect();
    let mut store = store_in(dir);
    let bottom = ids
        .iter()
        .copied()
        .find(|&id| Some(id) != crate::topmost_pixel_layer(&tree) || layers == 1);
    let Some(active) = bottom else {
        unreachable!("at least one layer")
    };
    fill_solid(&mut store, surface_id_for(active), TILE, rgba);
    let mut session = DocumentSession::new(DocumentContents {
        layers: tree,
        history: aurora_doc::History::new(),
        canvas_size: (256, 256),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: Some(active),
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: Some(store),
    });
    name.clone_into(&mut session.name);
    (session, ids)
}

/// `App`'s switch-side state, without a window.
struct Rig {
    scales: aurora_theme::Scales,
    workspace: aurora_ui::Workspace,
    focus: FocusManager,
    doc: DocumentSession,
    shelf: DocumentShelf,
    layer_rows: HashMap<WidgetId, aurora_doc::LayerId>,
    scroll_follow: ScrollFollow,
    drag: Option<Drag>,
    quick_mask: Option<crate::quick_mask::QuickMaskMode>,
    layer_controls: LayerControlsState,
    curves_ui: CurvesUiState,
    tool_controls: Option<aurora_ui::ToolControls>,
    click: ClickTracker,
    blocked: bool,
}

impl Rig {
    fn new(doc: DocumentSession) -> Self {
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        let mut workspace = aurora_ui::build_workspace(&scales);
        let StartupPanels {
            layer_rows,
            layer_controls,
            tool_controls,
            ..
        } = install_startup_panels(
            &mut workspace,
            &scales,
            &doc.layers,
            &doc.undo_order,
            aurora_ui::Tool::default(),
            &ToolSettings::default(),
        );
        let mut rig = Self {
            scales,
            workspace,
            focus: FocusManager::default(),
            doc,
            shelf: DocumentShelf::default(),
            layer_rows,
            scroll_follow: ScrollFollow::default(),
            drag: None,
            quick_mask: None,
            layer_controls,
            curves_ui: CurvesUiState::default(),
            tool_controls,
            click: ClickTracker::default(),
            blocked: false,
        };
        document_tabs::bind_active_document(&mut rig.cx(None));
        rig
    }

    fn cx<'a>(
        &'a mut self,
        residency: Option<&'a mut aurora_gpu::TileResidency>,
    ) -> SwitchContext<'a> {
        self.cx_with(residency, None)
    }

    fn cx_with<'a>(
        &'a mut self,
        residency: Option<&'a mut aurora_gpu::TileResidency>,
        autosave: Option<document_tabs::ParkAutosave<'a>>,
    ) -> SwitchContext<'a> {
        SwitchContext {
            autosave,
            workspace: &mut self.workspace,
            focus: &mut self.focus,
            scales: &self.scales,
            doc: &mut self.doc,
            shelf: &mut self.shelf,
            layer_rows: &mut self.layer_rows,
            scroll_follow: &mut self.scroll_follow,
            drag: &mut self.drag,
            quick_mask: &mut self.quick_mask,
            layer_controls: &mut self.layer_controls,
            curves_ui: &mut self.curves_ui,
            tool_controls: self.tool_controls,
            click: &mut self.click,
            residency,
            canvas_area: None,
            scale_factor: 1.0,
            blocked: self.blocked,
        }
    }

    fn switch(&mut self, target: DocumentId) -> SwitchOutcome {
        document_tabs::switch_document(&mut self.cx(None), target)
    }

    fn open(&mut self, incoming: DocumentSession) -> usize {
        document_tabs::activate_new_document(&mut self.cx(None), incoming)
    }

    fn parked(&self, id: DocumentId) -> &DocumentSession {
        match self.shelf.get(id) {
            Some(session) => session,
            None => unreachable!("{id:?} is parked"),
        }
    }

    /// A live brush stroke on the active layer, with one touched tile.
    fn start_stroke(&mut self) {
        let Some(layer) = self.doc.active_layer else {
            unreachable!("an active layer")
        };
        let mut stroke = aurora_brush::StrokeSnapshot::new(surface_id_for(layer));
        let Some(store) = self.doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        if let Err(err) = stroke.record_touch(store, TILE) {
            unreachable!("{err:?}");
        }
        self.drag = Some(Drag::Brush {
            hard: false,
            clone: None,
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: Some(stroke),
            warned: HashSet::new(),
        });
    }
}

fn tile_bytes(session: &mut DocumentSession, layer: aurora_doc::LayerId) -> Vec<f32> {
    let Some(store) = session.tile_store.as_mut() else {
        unreachable!("a store")
    };
    read_all_texels(store, surface_id_for(layer), TILE)
}

/// AC-1: opening makes a second session with its own store and leaves the
/// first byte-identical — tiles, history and active layer.
#[test]
fn opening_creates_a_second_session_and_leaves_the_first_byte_identical() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 3, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    rig.start_stroke();
    let a_id = rig.doc.id;
    let a_active = rig.doc.active_layer;
    let Some(a_layer) = a_active else {
        unreachable!("active")
    };
    let a_tiles_before = tile_bytes(&mut rig.doc, a_layer);
    let (b, b_ids) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let b_id = b.id;
    let parked_at = rig.open(b);
    assert_eq!(parked_at, 0);
    assert_eq!(rig.doc.id, b_id, "the opened document is active");
    assert_ne!(a_id, b_id);
    // The opened document's pixels go into its own store only.
    let Some(&b_layer) = b_ids.first() else {
        unreachable!("one layer")
    };
    if let Some(store) = rig.doc.tile_store.as_mut() {
        fill_solid(store, surface_id_for(b_layer), TILE, [0.0, 1.0, 0.0, 1.0]);
    }
    let a_journal = rig.parked(a_id).history.journal_len();
    let a_steps = rig.parked(a_id).undo_order.undo.len();
    assert_eq!(
        a_steps, 1,
        "the live stroke was committed into a before parking"
    );
    assert_eq!(rig.parked(a_id).active_layer, a_active);
    assert_eq!(rig.parked(a_id).layers.len(), 3);
    let Some(parked) = rig.shelf.parked.first_mut() else {
        unreachable!("a is parked")
    };
    assert_eq!(
        tile_bytes(parked, a_layer),
        a_tiles_before,
        "a's tiles unchanged"
    );
    // And back: a is exactly as it was.
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    assert_eq!(rig.doc.history.journal_len(), a_journal);
    assert_eq!(rig.doc.active_layer, a_active);
    assert_eq!(tile_bytes(&mut rig.doc, a_layer), a_tiles_before);
}

/// AC-2 (F2): undo after a switch reaches only the active document.
#[test]
fn undo_after_a_switch_reaches_only_the_active_document() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    rig.start_stroke();
    let _ = rig.open(b);
    rig.start_stroke();
    let b_id = rig.doc.id;
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    let mut palette = None;
    let mut tool = aurora_ui::Tool::default();
    let _ = crate::run_command(
        &mut rig.workspace,
        &mut rig.focus,
        &mut palette,
        &mut tool,
        &ToolSettings::default(),
        &mut rig.doc.layers,
        &mut rig.doc.history,
        &mut rig.doc.pixel_history,
        rig.doc.tile_store.as_mut(),
        &mut rig.doc.undo_order,
        AppCommand::Undo,
    );
    assert_eq!(rig.doc.id, a_id);
    assert!(rig.doc.undo_order.undo.is_empty(), "a's stroke was undone");
    assert_eq!(rig.doc.undo_order.redo.len(), 1);
    let b_parked = rig.parked(b_id);
    assert_eq!(b_parked.undo_order.undo.len(), 1, "b's stroke is untouched");
    assert!(b_parked.undo_order.redo.is_empty());
    assert!(b_parked.pixel_history.can_undo());
}

/// AC-3 (F3): a live stroke, opacity drag and Curves drag are committed
/// into the outgoing document's own history, not dropped.
#[test]
fn live_gestures_during_a_switch_are_committed_to_the_outgoing_document() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (mut a, ids) = session("a", 2, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let changed = aurora_core::CurvesParams {
        red: Some(aurora_core::ToneCurve::identity()),
        ..aurora_core::CurvesParams::identity()
    };
    let curves = match a.layers.add_adjustment_layer_at(
        "Curves",
        aurora_doc::Adjustment::Curves(changed),
        None,
        0,
    ) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(&top) = ids.last() else {
        unreachable!("two layers")
    };
    if let Err(err) = a.layers.set_opacity(top, 0.5) {
        unreachable!("{err:?}");
    }
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let b_id = b.id;
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    rig.start_stroke();
    rig.layer_controls.pending = Some(PendingOpacity {
        layer: top,
        start: 1.0,
        fill: false,
    });
    rig.curves_ui.pending = Some(PendingCurves {
        layer: curves,
        start: aurora_core::CurvesParams::identity(),
    });
    assert_eq!(rig.switch(b_id), SwitchOutcome::Switched);
    assert!(rig.drag.is_none());
    assert!(rig.layer_controls.pending.is_none());
    assert!(rig.curves_ui.pending.is_none());
    let a = rig.parked(a_id);
    assert_eq!(
        a.undo_order.undo.len(),
        3,
        "stroke, opacity and Curves steps"
    );
    assert!(a.pixel_history.can_undo());
    assert!(rig.doc.undo_order.undo.is_empty(), "nothing reached b");
}

/// AC-4 (F6, F7, F8): a switch rebuilds Layers and History from the
/// incoming session — row count, active row, current step — keeps each
/// document's own active layer and view, and leaves no focus on a stale row.
#[test]
fn a_switch_rebuilds_the_panels_from_the_incoming_session() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, a_ids) = session("a", 3, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let a_active = a.active_layer;
    assert_ne!(
        a_active,
        crate::topmost_pixel_layer(&a.layers),
        "a's active layer is not its topmost"
    );
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    rig.start_stroke();
    rig.doc.canvas_view.zoom_at((0.0, 0.0), 2.0);
    let a_view = rig.doc.canvas_view;
    let _ = rig.open(b);
    assert_eq!(rig.layer_rows.len(), 1, "b's one row");
    // Focus a b row: it is removed by the switch.
    let Some((&b_row, _)) = rig.layer_rows.iter().next() else {
        unreachable!("one row")
    };
    if let Err(err) = rig.focus.focus(&mut rig.workspace.tree, b_row) {
        unreachable!("{err:?}");
    }
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    assert_eq!(rig.layer_rows.len(), a_ids.len(), "a's three rows");
    assert_eq!(rig.doc.active_layer, a_active, "a's own active layer, kept");
    assert_eq!(rig.doc.canvas_view, a_view, "a's own view, kept");
    let focused = rig.focus.focused();
    assert_ne!(focused, Some(b_row));
    assert!(!rig.workspace.tree.contains(b_row), "the stale row is gone");
    let active_row = rig
        .layer_rows
        .iter()
        .find(|&(_, &layer)| Some(layer) == a_active)
        .map(|(&row, _)| row);
    assert_eq!(focused, active_row, "focus moved to a's active row");
    // History: origin row plus a's one step, the step current.
    assert!(rig.workspace.history_current.is_some());
    assert_eq!(
        rig.workspace
            .tree
            .children(rig.workspace.history.body)
            .map_or(0, <[WidgetId]>::len),
        2,
        "origin row and a's stroke"
    );
    // The status bar was synced by the switch itself.
    assert!(!crate::sync_status_bar(
        &mut rig.workspace,
        &rig.doc.canvas_view,
        rig.doc.canvas_size,
        1.0
    ));
}

/// AC-4: a switch is refused while something modal or a drag is live.
#[test]
fn a_switch_is_refused_while_blocked() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    rig.blocked = true;
    let b_id = rig.doc.id;
    assert_eq!(rig.switch(a_id), SwitchOutcome::Blocked);
    assert_eq!(rig.doc.id, b_id);
    assert_eq!(rig.switch(b_id), SwitchOutcome::AlreadyActive);
}

/// AC-5 (F4): a background open that finishes after a switch lands in a
/// new session, never the active one; a superseded open drops its store.
#[test]
fn an_open_after_a_switch_lands_in_a_new_session_and_a_superseded_store_drops() {
    let (dir_a, dir_b, dir_c) = (tempdir(), tempdir(), tempdir());
    let mut pending = PendingOpenStores::default();
    assert_eq!(pending.start(1, Some(store_in(&dir_c))), None);
    assert_eq!(
        pending.start(2, Some(store_in(&dir_c))),
        Some(1),
        "open 1 superseded"
    );
    assert!(
        pending.take(1).is_none(),
        "the superseded open has no store"
    );
    assert_eq!(pending.generation(), Some(2));
    let Some(Some(store)) = pending.take(2) else {
        unreachable!("open 2's store is held")
    };
    assert_eq!(pending.generation(), None);
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 2, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    // The open finishes now, into a fresh session with open 2's store.
    let mut opened = DocumentSession::new(DocumentContents {
        layers: aurora_doc::LayerTree::new(),
        history: aurora_doc::History::new(),
        canvas_size: (1, 1),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: None,
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: Some(store),
    });
    "c.png".clone_into(&mut opened.name);
    let opened_id = opened.id;
    let _ = rig.open(opened);
    assert_eq!(rig.doc.id, opened_id);
    assert_eq!(rig.shelf.len(), 3);
    assert_eq!(
        rig.parked(a_id).layers.len(),
        1,
        "a, active at the open, parked intact"
    );
    assert_eq!(rig.parked(b_id).layers.len(), 2);
    assert_eq!(
        rig.shelf.tab_order(&rig.doc),
        [a_id, b_id, opened_id],
        "the new document is appended"
    );
    // A failed install reverts to the document that was active.
    let (d, _) = session("d", 1, [0.0, 1.0, 0.0, 1.0], &dir_a);
    let parked_at = rig.open(d);
    let reverted = rig.shelf.revert_push(&mut rig.doc, parked_at);
    assert!(reverted.is_some());
    assert_eq!(rig.doc.id, opened_id);
}

/// One frame's canvas half: recomposite, then sync the atlas.
fn draw_frame(
    gpu: &aurora_gpu::GpuContext,
    doc: &mut DocumentSession,
    residency: &mut aurora_gpu::TileResidency,
    viewport: (u32, u32),
) {
    residency.set_origin(gpu.queue(), (0.0, 0.0), viewport, 1.0);
    let Some(store) = doc.tile_store.as_mut() else {
        unreachable!("a store")
    };
    recomposite_visible_tiles(
        residency,
        &doc.layers,
        doc.active_layer,
        store,
        &mut doc.composite_cache,
        Some(gpu),
        None,
    );
    let stats = residency.sync(
        gpu.queue(),
        store,
        composite_surface_id(),
        false,
        usize::MAX,
    );
    assert_eq!(stats.errors, 0);
}

/// AC-6 (F1), real GPU: after a switch the atlas shows the incoming
/// document. Two documents of distinct solid colours; back and forth. The
/// return to `a` is the case the residency reset exists for: a's composite
/// tile recomposites byte-identically (no dirty mark) into a slot holding
/// the same tile id, so without the reset the atlas keeps showing `b`.
#[test]
fn after_a_switch_the_atlas_shows_the_incoming_document() {
    let Some(context) = real_gpu_context() else {
        return;
    };
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let viewport = (256, 256);
    let mut residency = aurora_gpu::TileResidency::new(context.device(), context.queue(), viewport);
    let mut canvas = aurora_gpu::CanvasPipeline::new(context.device());
    let sample = (128, 128);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let mut colour = |rig: &mut Rig, residency: &mut aurora_gpu::TileResidency| {
        draw_frame(&context, &mut rig.doc, residency, viewport);
        render_and_sample_pixel(
            context.device(),
            context.queue(),
            &mut canvas,
            residency,
            viewport,
            sample,
        )
    };
    let red = colour(&mut rig, &mut residency);
    assert!(red[0] > 200 && red[2] < 50, "a is red: {red:?}");
    let _ = document_tabs::activate_new_document(&mut rig.cx(Some(&mut residency)), b);
    let b_id = rig.doc.id;
    let blue = colour(&mut rig, &mut residency);
    assert!(blue[2] > 200 && blue[0] < 50, "b is blue: {blue:?}");
    assert_eq!(
        document_tabs::switch_document(&mut rig.cx(Some(&mut residency)), a_id),
        SwitchOutcome::Switched
    );
    let back = colour(&mut rig, &mut residency);
    assert!(
        back[0] > 200 && back[2] < 50,
        "back on a, the atlas shows a: {back:?}"
    );
    assert_eq!(
        document_tabs::switch_document(&mut rig.cx(Some(&mut residency)), b_id),
        SwitchOutcome::Switched
    );
    let again = colour(&mut rig, &mut residency);
    assert!(again[2] > 200 && again[0] < 50, "back on b: {again:?}");
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(err) => unreachable!("{err:?}"),
    };
    names.sort();
    names
}

/// AC-7 (F5): every session autosaves to its own file under its own id,
/// and the index lists them all in tab order with the active one.
#[test]
fn every_session_autosaves_to_its_own_file_and_the_index_lists_them_in_tab_order() {
    let (dir_a, dir_b, autosaves) = (tempdir(), tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4242);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 2, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = worker.configure_index(autosave_files::IndexBook::new(
        namespace.index_path(),
        document_tabs::index_entries(&rig.shelf, &rig.doc, namespace.key),
        Some(a_id.get()),
        [],
    ));
    let _ = rig.open(b);
    {
        let doc = &mut rig.doc;
        let path = namespace.session_path(doc.id.get());
        let Some(store) = doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        crate::request_autosave(
            &mut worker,
            &path,
            doc.id.get(),
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            &mut doc.skipped_tiles,
            store,
            None,
            None,
        );
    }
    let b_id = rig.doc.id;
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    let a_path = namespace.session_path(a_id.get());
    {
        let doc = &mut rig.doc;
        let Some(store) = doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        crate::request_autosave(
            &mut worker,
            &a_path,
            doc.id.get(),
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            &mut doc.skipped_tiles,
            store,
            None,
            None,
        );
    }
    assert!(document_tabs::sync_autosave_sessions(
        &mut worker,
        &rig.shelf,
        &rig.doc,
        namespace.key
    ));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(
        a_path.exists(),
        "a's own file: {:?}",
        names(autosaves.path())
    );
    assert!(namespace.session_path(b_id.get()).exists(), "b's own file");
    let index = match autosave_files::read_index(&namespace.index_path(), namespace.key) {
        Ok(index) => index,
        Err(err) => unreachable!("{err:?}"),
    };
    let ids: Vec<u64> = index.entries.iter().map(|entry| entry.id).collect();
    assert_eq!(ids, [a_id.get(), b_id.get()], "tab order");
    assert_eq!(index.active, Some(a_id.get()));
}

/// AC-7 (F5): a crashed run with two documents comes back as two
/// sessions, the active one first; both files are adopted into this run's
/// namespace before the crashed run is retired.
#[test]
fn a_crashed_two_document_run_is_recovered_as_two_sessions() {
    let (autosaves, stores) = (tempdir(), tempdir());
    let Some(crashed) = autosave_files::RunKey::parse("777777") else {
        unreachable!("a valid key")
    };
    let mut paths = Vec::new();
    for (id, layers) in [(1_u64, 1_usize), (2, 2)] {
        let (mut doc, _) = session(&format!("doc{id}"), layers, [1.0, 1.0, 0.0, 1.0], &stores);
        let path = autosaves
            .path()
            .join(autosave_files::session_file_name(crashed, id));
        let Some(store) = doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        crate::write_autosave(
            &path,
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            &mut doc.skipped_tiles,
            store,
            None,
            None,
        );
        assert!(path.exists());
        paths.push(path);
    }
    let candidates: Vec<autosave_files::Candidate> = paths
        .iter()
        .zip([1_u64, 2])
        .map(|(path, id)| autosave_files::Candidate {
            path: path.clone(),
            source: Some(crashed),
            id,
        })
        .collect();
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4243);
    let own = DocumentId::next();
    let mut reopen = || Some(store_in(&stores));
    let mut slot = Some(store_in(&stores));
    let first = crate::startup_document_from(
        true,
        &paths,
        &namespace.session_path(own.get()),
        &mut slot,
        &mut reopen,
    );
    assert_eq!(first.recovered_from, Some(0));
    let (extra, failed, tried) = crate::recover_remaining_candidates(&paths, 1, &mut reopen);
    assert!(failed.is_empty());
    assert_eq!(tried, 2);
    assert_eq!(extra.len(), 1, "the second document is recovered too");
    let sessions: Vec<crate::RecoveredSession> = extra
        .into_iter()
        .map(|(position, document, store)| crate::RecoveredSession {
            id: DocumentId::next(),
            position,
            document,
            store,
        })
        .collect();
    let mut documents = vec![(own.get(), Some(0))];
    documents.extend(sessions.iter().map(|s| (s.id.get(), Some(s.position))));
    let second = sessions.first().map(|s| s.id);
    let mut worker = background_autosave::AutosaveWorker::default();
    let settled = crate::settle_startup_autosaves(
        &mut worker,
        &namespace,
        &documents,
        &[crashed],
        &candidates,
        Some(tried),
    );
    assert!(settled.adopted && settled.index_written, "{settled:?}");
    assert_eq!(settled.retired, [crashed]);
    let shelf = crate::recovered_shelf(sessions);
    assert_eq!(shelf.parked.len(), 1, "two documents open in all");
    let Some(parked) = shelf.parked.first() else {
        unreachable!("one parked")
    };
    assert_eq!(parked.layers.len(), 2, "the second document's own layers");
    assert!(parked.tile_store.is_some(), "its own store");
    assert!(namespace.session_path(own.get()).exists());
    let Some(second) = second else {
        unreachable!("one extra")
    };
    assert!(namespace.session_path(second.get()).exists());
    assert!(
        paths.iter().all(|path| !path.exists()),
        "the crashed run retired"
    );
    let index = match autosave_files::read_index(&namespace.index_path(), namespace.key) {
        Ok(index) => index,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(index.entries.len(), 2);
    assert_eq!(index.active, Some(own.get()));
}

/// AC-7 (carry-over): liveness refreshes this run's session files and the
/// marker, not just the lock and the index; other runs' files stay old.
#[test]
fn liveness_refresh_keeps_this_runs_session_files_and_the_marker_fresh() {
    let dir = tempdir();
    let namespace = autosave_files::AutosaveNamespace::acquire(dir.path().to_path_buf(), 4244);
    let own = namespace.session_path(9);
    let Some(other_key) = autosave_files::RunKey::parse("888888") else {
        unreachable!("a valid key")
    };
    let other = dir
        .path()
        .join(autosave_files::session_file_name(other_key, 9));
    let marker = dir.path().join("marker");
    let old = std::time::SystemTime::now() - std::time::Duration::from_hours(5 * 24);
    for path in [&own, &other, &marker] {
        let file = match std::fs::File::create(path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = file.set_modified(old) {
            unreachable!("{err:?}");
        }
    }
    let liveness = namespace.liveness();
    liveness.watch_marker(marker.clone());
    let _ = liveness.refresh();
    let modified = |path: &std::path::Path| match std::fs::metadata(path).and_then(|m| m.modified())
    {
        Ok(time) => time,
        Err(err) => unreachable!("{err:?}"),
    };
    let recent = std::time::SystemTime::now() - std::time::Duration::from_mins(10);
    assert!(
        modified(&own) > recent,
        "this run's session file is refreshed"
    );
    assert!(modified(&marker) > recent, "the marker is refreshed");
    assert!(
        modified(&other) < recent,
        "another run's file is left alone"
    );
}

/// AC-8: Ctrl+Tab / Ctrl+Shift+Tab resolve to Next/Previous Document while
/// plain Tab still moves focus, and both come back from `handle_key`.
#[test]
fn ctrl_tab_cycles_documents_and_plain_tab_still_moves_focus() {
    let shortcuts = crate::default_shortcuts();
    let chord = |text: &str| match KeyChord::parse(text) {
        Ok(chord) => chord,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(
        shortcuts.resolve(chord("Ctrl+Tab")),
        Some(&AppCommand::NextDocument)
    );
    assert_eq!(
        shortcuts.resolve(chord("Ctrl+Shift+Tab")),
        Some(&AppCommand::PreviousDocument)
    );
    assert_eq!(
        shortcuts.resolve(chord("Tab")),
        Some(&AppCommand::FocusNext)
    );
    assert_eq!(
        shortcuts.resolve(chord("Shift+Tab")),
        Some(&AppCommand::FocusPrevious)
    );
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    for (shift, forward) in [(false, true), (true, false)] {
        let mut dialog = None;
        let mut palette = None;
        let mut tool = aurora_ui::Tool::default();
        let modifiers = Modifiers {
            control: true,
            shift,
            ..Modifiers::none()
        };
        let picked = crate::handle_key(
            &mut rig.workspace,
            &mut rig.focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &ToolSettings::default(),
            &mut rig.doc.layers,
            &mut rig.doc.history,
            &mut rig.doc.pixel_history,
            rig.doc.tile_store.as_mut(),
            &mut rig.doc.undo_order,
            &shortcuts,
            modifiers,
            Key::Named(NamedKey::Tab),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert_eq!(
            picked,
            Some(ActivatedCommand::Document(DocumentCommand::Cycle {
                forward
            }))
        );
    }
}

/// AC-8: the palette's document entries, their ids, the cycle order and
/// the window title.
#[test]
fn palette_document_entries_cycle_order_and_title_name_the_documents() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let entries = document_tabs::document_palette_entries(&rig.shelf, &rig.doc);
    let titles: Vec<&str> = entries.iter().map(|entry| entry.title.as_str()).collect();
    assert_eq!(titles, ["Document: a.png", "Document: b.psd"]);
    let Some(first) = entries.first() else {
        unreachable!("two entries")
    };
    assert_eq!(
        document_tabs::document_command_for(&first.id),
        Some(DocumentCommand::Activate(a_id.get()))
    );
    assert_eq!(
        document_tabs::document_command_for(document_tabs::COMMAND_DOCUMENT_NEXT),
        Some(DocumentCommand::Cycle { forward: true })
    );
    assert_eq!(rig.shelf.cycled(&rig.doc, true), Some(a_id), "wraps around");
    assert_eq!(rig.shelf.cycled(&rig.doc, false), Some(a_id));
    assert_eq!(
        crate::window_title(None, &rig.doc.name),
        "b.psd \u{2014} Aurora"
    );
    let opening = std::path::Path::new("/tmp/c.tif");
    assert!(crate::window_title(Some(opening), &rig.doc.name).starts_with("Opening"));
    let ids: Vec<String> = crate::palette_commands()
        .into_iter()
        .map(|e| e.id)
        .collect();
    for id in [
        document_tabs::COMMAND_FILE_NEW,
        document_tabs::COMMAND_DOCUMENT_NEXT,
        document_tabs::COMMAND_DOCUMENT_PREVIOUS,
    ] {
        assert!(ids.iter().any(|known| known == id), "{id}");
    }
}

/// Decodes an autosave file and reads its topmost pixel layer's tile.
fn autosaved_tile(path: &std::path::Path, dir: &tempfile::TempDir) -> Vec<f32> {
    let mut store = store_in(dir);
    let Some(document) = crate::recover_document(path, &mut store) else {
        unreachable!("{} decodes", path.display())
    };
    let Some(layer) = crate::topmost_pixel_layer(&document.layers) else {
        unreachable!("a pixel layer")
    };
    read_all_texels(&mut store, surface_id_for(layer), TILE)
}

/// Review J-1: the outgoing document is autosaved, under its own id and
/// store, when a switch or an open parks it — so its edits survive a crash
/// while it sits parked. Decoded and compared, for both paths; the
/// incoming document's own autosave (another slot) does not supersede it.
#[test]
fn a_parked_document_is_autosaved_with_its_edits() {
    let (dir_a, dir_b, autosaves, reads) = (tempdir(), tempdir(), tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4245);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, a_ids) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let Some(&a_layer) = a_ids.first() else {
        unreachable!("one layer")
    };
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    // Edit a (green), then open b: a is autosaved as it is parked.
    if let Some(store) = rig.doc.tile_store.as_mut() {
        fill_solid(store, surface_id_for(a_layer), TILE, [0.0, 1.0, 0.0, 1.0]);
    }
    let edited = tile_bytes(&mut rig.doc, a_layer);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let _ = document_tabs::activate_new_document(
        &mut rig.cx_with(
            None,
            Some(document_tabs::ParkAutosave {
                worker: &mut worker,
                namespace: &namespace,
            }),
        ),
        b,
    );
    // The incoming document's own autosave, at once, in its own slot.
    let b_id = rig.doc.id;
    let b_path = namespace.session_path(b_id.get());
    {
        let doc = &mut rig.doc;
        let Some(store) = doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        crate::request_autosave(
            &mut worker,
            &b_path,
            doc.id.get(),
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            &mut doc.skipped_tiles,
            store,
            None,
            None,
        );
    }
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    let a_path = namespace.session_path(a_id.get());
    assert_eq!(
        autosaved_tile(&a_path, &reads),
        edited,
        "a's edits are in a's file"
    );
    assert!(b_path.exists(), "b's own file too");
    // Edit b (white), switch back to a: b is autosaved as it is parked.
    let Some(b_layer) = rig.doc.active_layer else {
        unreachable!("active")
    };
    if let Some(store) = rig.doc.tile_store.as_mut() {
        fill_solid(store, surface_id_for(b_layer), TILE, [1.0, 1.0, 1.0, 1.0]);
    }
    let b_edited = tile_bytes(&mut rig.doc, b_layer);
    let outcome = document_tabs::switch_document(
        &mut rig.cx_with(
            None,
            Some(document_tabs::ParkAutosave {
                worker: &mut worker,
                namespace: &namespace,
            }),
        ),
        a_id,
    );
    assert_eq!(outcome, SwitchOutcome::Switched);
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert_eq!(
        autosaved_tile(&b_path, &reads),
        b_edited,
        "b's edits are in b's file"
    );
    assert_eq!(
        autosaved_tile(&a_path, &reads),
        edited,
        "a's file unchanged"
    );
}

/// Review J-3: a finished open with no pending store is a failed open
/// (reported through "Couldn't Open File"), never a fresh store.
#[test]
fn a_finished_open_without_its_pending_store_is_a_failed_open() {
    let dir = tempdir();
    let mut pending = PendingOpenStores::default();
    assert!(matches!(
        crate::pending_store_for(&mut pending, 1),
        Err(crate::OpenFailure::LostStorage)
    ));
    let _ = pending.start(2, Some(store_in(&dir)));
    assert!(matches!(
        crate::pending_store_for(&mut pending, 1),
        Err(crate::OpenFailure::LostStorage)
    ));
    assert!(matches!(
        crate::pending_store_for(&mut pending, 2),
        Ok(Some(_))
    ));
    let message = crate::open_failure_message("a.png", "png", &crate::OpenFailure::LostStorage);
    assert!(message.contains("a.png"), "{message}");
}

/// Review J-4: choosing "Document: `name`" in the palette closes the
/// palette *before* the command comes back, so the switch it asks for is
/// not refused as blocked by an open palette.
#[test]
fn a_palette_document_entry_closes_the_palette_before_the_switch() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("alpha.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("beta.png", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let shortcuts = crate::default_shortcuts();
    let mut dialog = None;
    let mut palette = None;
    let mut tool = aurora_ui::Tool::default();
    let mut press = |rig: &mut Rig,
                     palette: &mut Option<WidgetId>,
                     modifiers: Modifiers,
                     key: Key,
                     text: Option<&str>| {
        crate::handle_key(
            &mut rig.workspace,
            &mut rig.focus,
            &mut dialog,
            palette,
            &mut tool,
            &ToolSettings::default(),
            &mut rig.doc.layers,
            &mut rig.doc.history,
            &mut rig.doc.pixel_history,
            rig.doc.tile_store.as_mut(),
            &mut rig.doc.undo_order,
            &shortcuts,
            modifiers,
            key,
            text,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        )
    };
    // The platform's primary `Shift+P` (`Cmd` on macOS, 0.175.0).
    let open = with_primary(Modifiers {
        shift: true,
        ..Modifiers::none()
    });
    let _ = press(&mut rig, &mut palette, open, Key::Character('p'), None);
    assert!(palette.is_some(), "the palette opened");
    assert!(document_tabs::add_document_entries_if_opened(
        &mut rig.workspace,
        false,
        palette,
        document_tabs::document_palette_entries(&rig.shelf, &rig.doc),
    ));
    for ch in "alpha".chars() {
        let text = ch.to_string();
        let _ = press(
            &mut rig,
            &mut palette,
            Modifiers::none(),
            Key::Character(ch),
            Some(text.as_str()),
        );
    }
    let picked = press(
        &mut rig,
        &mut palette,
        Modifiers::none(),
        Key::Named(NamedKey::Enter),
        None,
    );
    assert_eq!(
        picked,
        Some(ActivatedCommand::Document(DocumentCommand::Activate(
            a_id.get()
        )))
    );
    assert!(palette.is_none(), "closed before the command runs");
    // So the App's switch context is not blocked by it.
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
}

/// Review J-4: `Ctrl+Tab` in a focused text field inserts nothing and
/// falls through to the document shortcut.
#[test]
fn ctrl_tab_in_a_focused_text_field_inserts_nothing_and_switches() {
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let gallery = match aurora_ui::insert_gallery_panel(
        &mut rig.workspace.tree,
        rig.workspace.root,
        &rig.scales,
    ) {
        Ok(gallery) => gallery,
        Err(err) => unreachable!("{err:?}"),
    };
    let field = gallery.text_field;
    if let Err(err) = rig.focus.focus(&mut rig.workspace.tree, field) {
        unreachable!("{err:?}");
    }
    let text_before = format!("{:?}", rig.workspace.tree.payload(field));
    let ctrl = Modifiers {
        control: true,
        ..Modifiers::none()
    };
    let mut gallery = Some(gallery);
    let routed = crate::route_widget_key(
        &mut rig.workspace,
        &mut rig.focus,
        &mut gallery,
        None,
        None,
        &rig.scales,
        false,
        false,
        ctrl,
        Key::Named(NamedKey::Tab),
        Some("\t"),
        &mut FakeClipboard::default(),
    );
    assert!(
        !matches!(routed, Some((_, aurora_widgets::KeyOutcome::Handled(_)))),
        "the text field does not take Ctrl+Tab: {routed:?}"
    );
    assert_eq!(
        format!("{:?}", rig.workspace.tree.payload(field)),
        text_before,
        "no tab was inserted"
    );
    let shortcuts = crate::default_shortcuts();
    let picked = crate::handle_key(
        &mut rig.workspace,
        &mut rig.focus,
        &mut None,
        &mut None,
        &mut aurora_ui::Tool::default(),
        &ToolSettings::default(),
        &mut rig.doc.layers,
        &mut rig.doc.history,
        &mut rig.doc.pixel_history,
        rig.doc.tile_store.as_mut(),
        &mut rig.doc.undo_order,
        &shortcuts,
        ctrl,
        Key::Named(NamedKey::Tab),
        Some("\t"),
        &mut FakeClipboard::default(),
        &mut FakeFileDialog::default(),
    );
    assert_eq!(
        picked,
        Some(ActivatedCommand::Document(DocumentCommand::Cycle {
            forward: true
        }))
    );
}

// -- 0.173.0 (R4): the document tab strip --

fn strip(rig: &Rig) -> (Vec<String>, usize, Vec<WidgetId>) {
    match aurora_widgets::widgets::tab_bar_state(&rig.workspace.tree, rig.workspace.document_tabs) {
        Ok(state) => (
            state.labels().to_vec(),
            state.selected(),
            state.tabs().to_vec(),
        ),
        Err(err) => unreachable!("{err:?}"),
    }
}

fn tab_names(list: &[&str]) -> Vec<String> {
    list.iter().map(|&name| name.to_owned()).collect()
}

/// AC-1 (R4): the strip lists every document in tab order with the active
/// one selected, and follows an open, a New Document, a switch and a
/// recovered set; AC-4: the canvas is the `TabPanel` labelled by the
/// selected tab, whose AT name is the document's.
#[test]
#[allow(clippy::many_single_char_names)]
fn the_tab_strip_lists_every_document_and_follows_open_new_switch_and_recovery() {
    let (dir_a, dir_b, dir_c) = (tempdir(), tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let (c, _) = session("Untitled 2", 1, [0.0, 1.0, 0.0, 1.0], &dir_c);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    assert_eq!(strip(&rig).0, tab_names(&["a.png"]));
    let _ = rig.open(b);
    assert_eq!(
        (strip(&rig).0, strip(&rig).1),
        (tab_names(&["a.png", "b.psd"]), 1)
    );
    let _ = rig.open(c);
    assert_eq!(strip(&rig).0, tab_names(&["a.png", "b.psd", "Untitled 2"]));
    assert_eq!(strip(&rig).1, 2);
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    let (labels, selected, tabs) = strip(&rig);
    assert_eq!((labels.len(), selected), (3, 0));
    let panel = rig
        .workspace
        .tree
        .accessibility(rig.workspace.canvas_area)
        .cloned();
    assert_eq!(
        panel.as_ref().map(accesskit::Node::role),
        Some(accesskit::Role::TabPanel)
    );
    assert_eq!(
        panel.map(|node| node.labelled_by().to_vec()),
        Some(tabs.first().copied().into_iter().collect::<Vec<_>>())
    );
    let first_name = tabs
        .first()
        .and_then(|&tab| rig.workspace.tree.accessibility(tab))
        .and_then(|node| node.label().map(str::to_owned));
    assert_eq!(first_name.as_deref(), Some("a.png"));
    // A recovered set: the active document plus parked ones, as App::new
    // builds it, shown in order.
    let (d, _) = session("Untitled", 1, [1.0, 1.0, 1.0, 1.0], &dir_a);
    let (e, _) = session("Recovered 2", 1, [1.0, 1.0, 1.0, 1.0], &dir_b);
    let mut recovered = Rig::new(d);
    recovered.shelf.parked.push(e);
    assert!(document_tabs::sync_document_tab_bar(
        &mut recovered.workspace,
        &mut recovered.focus,
        &mut recovered.shelf,
        &recovered.doc,
    ));
    assert_eq!(
        (strip(&recovered).0, strip(&recovered).1),
        (tab_names(&["Untitled", "Recovered 2"]), 0)
    );
}

/// AC-2 (R4): a pointer click, an arrow key and an AT `Click` on the strip
/// each switch through `switch_document` — the documents, undo order and
/// panels follow exactly as for `Ctrl+Tab` — and a switch from the strip
/// keeps focus on its selected tab (AC-5, F8/F11). A refused switch puts
/// the strip back.
#[test]
#[allow(clippy::too_many_lines)] // one scenario, three input paths in sequence
fn click_keyboard_and_at_on_the_tab_strip_switch_like_ctrl_tab() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 3, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    // Keyboard: focus the strip's selected (b) tab, press Left.
    let Some(&b_tab) = strip(&rig).2.get(1) else {
        unreachable!("two tabs")
    };
    if let Err(err) = rig.focus.focus(&mut rig.workspace.tree, b_tab) {
        unreachable!("{err:?}");
    }
    let routed = crate::route_widget_key(
        &mut rig.workspace,
        &mut rig.focus,
        &mut None,
        None,
        None,
        &rig.scales,
        false,
        false,
        Modifiers::none(),
        Key::Named(NamedKey::ArrowLeft),
        None,
        &mut FakeClipboard::default(),
    );
    assert!(
        matches!(
            routed,
            Some((
                crate::WidgetOwner::DocumentTabs,
                aurora_widgets::KeyOutcome::Handled(_)
            ))
        ),
        "{routed:?}"
    );
    assert_eq!(
        document_tabs::follow_document_tabs(&mut rig.cx(None)),
        Some(SwitchOutcome::Switched)
    );
    assert_eq!(rig.doc.id, a_id);
    assert_eq!(rig.layer_rows.len(), 3, "a's panels");
    let a_tab = strip(&rig).2.first().copied();
    assert_eq!(
        rig.focus.focused(),
        a_tab,
        "focus stays on the strip's selected tab"
    );
    assert_eq!(document_tabs::follow_document_tabs(&mut rig.cx(None)), None);
    // AT Click on b's tab.
    let request = accesskit::ActionRequest {
        action: accesskit::Action::Click,
        target_tree: aurora_widgets::ACCESSIBILITY_TREE_ID,
        target_node: b_tab,
        data: None,
    };
    let reaction = crate::route_accessibility_action(
        &mut rig.workspace,
        &mut rig.focus,
        None,
        &HashMap::new(),
        None,
        None,
        &request,
    );
    assert!(
        matches!(reaction, crate::AccessibilityReaction::DocumentTab(_)),
        "{reaction:?}"
    );
    assert_eq!(
        document_tabs::follow_document_tabs(&mut rig.cx(None)),
        Some(SwitchOutcome::Switched)
    );
    assert_eq!(rig.doc.id, b_id);
    assert_eq!(rig.layer_rows.len(), 1, "b's panels");
    // Pointer: press and release on a's tab.
    rig.workspace.tree.compute_layout(1000.0, 800.0);
    let Some(a_tab) = a_tab else {
        unreachable!("two tabs")
    };
    let Some(bounds) = rig.workspace.tree.bounds(a_tab) else {
        unreachable!("laid out")
    };
    #[allow(clippy::cast_precision_loss)]
    let centre = (
        bounds.x as f32 + bounds.width as f32 / 2.0,
        bounds.y as f32 + bounds.height as f32 / 2.0,
    );
    let mut click = ClickTracker::default();
    for phase in [
        aurora_widgets::PointerPhase::Down,
        aurora_widgets::PointerPhase::Up,
    ] {
        let routed = crate::route_widget_pointer(
            &mut rig.workspace,
            &mut rig.focus,
            &mut None,
            None,
            None,
            &mut click,
            &rig.scales,
            false,
            phase,
            centre,
            Modifiers::none(),
            &mut aurora_widgets::NoTextHit,
        );
        assert_eq!(routed.owner, Some(crate::WidgetOwner::DocumentTabs));
    }
    assert_eq!(
        document_tabs::follow_document_tabs(&mut rig.cx(None)),
        Some(SwitchOutcome::Switched)
    );
    assert_eq!(rig.doc.id, a_id);
    // Refused (a modal is open): the strip goes back to the active one.
    rig.blocked = true;
    if let Err(err) =
        aurora_widgets::widgets::select_tab(&mut rig.workspace.tree, rig.workspace.document_tabs, 1)
    {
        unreachable!("{err:?}");
    }
    assert_eq!(
        document_tabs::follow_document_tabs(&mut rig.cx(None)),
        Some(SwitchOutcome::Blocked)
    );
    assert_eq!(rig.doc.id, a_id);
    assert_eq!(
        strip(&rig).1,
        0,
        "the strip shows the active document again"
    );
}

/// AC-3 (R4): the canvas area is one row shorter — it starts under the
/// options bar *and* the strip — and pointer mapping holds at scale 1 and
/// 2: a point on the strip is not on the canvas, the canvas origin maps to
/// (0, 0), and the physical rect is the logical one times the scale.
#[test]
fn the_canvas_is_one_row_shorter_and_pointer_mapping_holds_at_scale_1_and_2() {
    let scales = crate::test_workspace_scales();
    let mut ws = aurora_ui::build_workspace(&scales);
    ws.tree.compute_layout(1000.0, 800.0);
    let bounds = |id| match ws.tree.bounds(id) {
        Some(bounds) => bounds,
        None => unreachable!("laid out"),
    };
    let (options, strip, canvas, status) = (
        bounds(ws.options_bar),
        bounds(ws.document_tabs),
        bounds(ws.canvas_area),
        bounds(ws.status_bar.root),
    );
    #[allow(clippy::cast_precision_loss)]
    let row = aurora_widgets::widgets::row_height(&scales);
    #[allow(clippy::cast_precision_loss)]
    let strip_height = strip.height as f32;
    assert!(strip_height >= row, "one row: {strip_height} >= {row}");
    assert_eq!(strip.y, i64::from(options.height));
    assert_eq!(canvas.y, i64::from(options.height + strip.height));
    assert_eq!(
        canvas.height,
        800 - options.height - strip.height - status.height
    );
    #[allow(clippy::cast_precision_loss)]
    let (ox, oy) = (canvas.x as f32, canvas.y as f32);
    assert_eq!(
        crate::pointer_in_canvas(&ws, (ox + 5.0, oy - 1.0)),
        None,
        "on the strip"
    );
    assert_eq!(crate::pointer_in_canvas(&ws, (ox, oy)), Some((0.0, 0.0)));
    assert_eq!(
        crate::pointer_in_canvas(&ws, (ox + 7.0, oy + 9.0)),
        Some((7.0, 9.0))
    );
    for scale in [1.0_f64, 2.0] {
        #[allow(clippy::cast_possible_truncation)]
        let s = scale as f32;
        #[allow(clippy::cast_precision_loss)]
        let expected = (
            ox * s,
            oy * s,
            canvas.width as f32 * s,
            canvas.height as f32 * s,
        );
        assert_eq!(
            crate::canvas_area_physical_rect(&ws, scale),
            Some(expected),
            "scale {scale}"
        );
    }
}

/// AC-6 (R4): the strip is not persisted — the saved layout of a
/// workspace is the same before and after documents are added and
/// switched on the strip, and it still decodes.
#[test]
fn the_tab_strip_is_not_part_of_the_saved_layout() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let before = crate::workspace_presets::live_layout(&rig.workspace);
    let encoded_before = postcard::to_allocvec(&before).unwrap_or_default();
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    let after = crate::workspace_presets::live_layout(&rig.workspace);
    assert_eq!(after, before);
    let encoded_after = postcard::to_allocvec(&after).unwrap_or_default();
    assert!(!encoded_after.is_empty());
    assert_eq!(encoded_after, encoded_before, "byte-identical");
    assert!(crate::decode_workspace_layout(&encoded_after).is_ok());
}

/// R-1 (R3 carry-over, 0.173.0): a park with no new step since the last
/// park autosave skips it; a recorded step makes the next park write.
#[test]
fn an_unchanged_parked_document_is_not_autosaved_again() {
    let (dir_a, dir_b, autosaves) = (tempdir(), tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4246);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let a_path = namespace.session_path(a_id.get());
    let park = |rig: &mut Rig, worker: &mut background_autosave::AutosaveWorker| {
        document_tabs::autosave_outgoing(&mut rig.cx_with(
            None,
            Some(document_tabs::ParkAutosave {
                worker,
                namespace: &namespace,
            }),
        ))
    };
    assert!(park(&mut rig, &mut worker), "the first park writes");
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(a_path.exists());
    if let Err(err) = std::fs::remove_file(&a_path) {
        unreachable!("{err:?}");
    }
    assert!(!park(&mut rig, &mut worker), "nothing new: skipped");
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(!a_path.exists(), "no write for an unchanged document");
    // A recorded step (a committed stroke) moves the revision.
    rig.start_stroke();
    let _ = document_tabs::commit_live_gestures(&mut rig.cx(None));
    assert!(park(&mut rig, &mut worker), "a new step: written");
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(a_path.exists());
    let _ = rig.open(b);
}

/// 0.218.0: a selection-only change (a recorded selection step, or a
/// saved-selection edit) never dirties the document but IS autosaved on the
/// next park, and the file carries the selection.
#[test]
fn a_selection_only_change_is_autosaved_on_park_and_the_file_holds_it() {
    let (dir_a, autosaves) = (tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4248);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    let a_path = namespace.session_path(rig.doc.id.get());
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    if let Err(err) = std::fs::remove_file(&a_path) {
        unreachable!("{err:?}");
    }
    assert!(!park_now(&mut rig, &mut worker, &namespace), "unchanged");
    let clean = rig.doc.is_dirty();
    // A selection step.
    let selected = aurora_doc::Selection::new(aurora_core::Rect {
        x: 1,
        y: 2,
        width: 5,
        height: 6,
    });
    rig.doc.selection.select(selected.clone());
    let doc = &mut rig.doc;
    assert!(doc.undo_order.record_selection(
        "Rectangular Marquee",
        None,
        Some(selected.clone()),
        &mut doc.history,
        &mut doc.pixel_history,
    ));
    assert_eq!(rig.doc.is_dirty(), clean, "a selection never dirties");
    assert!(
        park_now(&mut rig, &mut worker, &namespace),
        "but is written"
    );
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    let dir = tempdir();
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero")
    };
    let mut fresh = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let file = std::fs::File::open(&a_path).map_err(|e| format!("{e:?}"));
    let Ok(file) = file else {
        unreachable!("the park wrote the autosave")
    };
    let document = match aurora_io::read_aur(std::io::BufReader::new(file), &mut fresh) {
        Ok(document) => document,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(document.selection.active(), Some(selected));
    // A saved-selection edit (metadata, no step) is written on the next park.
    if let Err(err) = std::fs::remove_file(&a_path) {
        unreachable!("{err:?}");
    }
    assert!(
        !park_now(&mut rig, &mut worker, &namespace),
        "unchanged again"
    );
    assert!(rig.doc.selection.save_active("Kept").is_ok());
    rig.doc.undo_order.note_selection_metadata_change();
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(a_path.exists());
}

/// AC-2/AC-5 (R4, F8/F11): with focus resting on the strip, a switch made
/// elsewhere (`Ctrl+Tab`, the palette) moves focus to the strip's newly
/// selected tab — never left on the now-unselected (unfocusable) one.
#[test]
fn focus_on_the_strip_follows_a_ctrl_tab_switch_to_the_selected_tab() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let (_, _, tabs) = strip(&rig);
    let (Some(&a_tab), Some(&b_tab)) = (tabs.first(), tabs.get(1)) else {
        unreachable!("two tabs")
    };
    if let Err(err) = rig.focus.focus(&mut rig.workspace.tree, b_tab) {
        unreachable!("{err:?}");
    }
    let Some(target) = rig.shelf.cycled(&rig.doc, true) else {
        unreachable!("two documents")
    };
    assert_eq!(target, a_id);
    assert_eq!(rig.switch(target), SwitchOutcome::Switched);
    assert_eq!(
        strip(&rig).2,
        tabs,
        "the same tab widgets, relabelled in place"
    );
    assert_eq!(
        rig.focus.focused(),
        Some(a_tab),
        "focus moved to the selected tab"
    );
}

fn park_now(
    rig: &mut Rig,
    worker: &mut background_autosave::AutosaveWorker,
    namespace: &autosave_files::AutosaveNamespace,
) -> bool {
    document_tabs::autosave_outgoing(&mut rig.cx_with(
        None,
        Some(document_tabs::ParkAutosave { worker, namespace }),
    ))
}

/// Review R-1 (0.173.0): a park autosave counts as saved only once its
/// complete write has landed. A failed worker write makes the next park at
/// the same revision retry; a landed one makes it skip.
#[test]
fn a_failed_park_write_is_retried_and_a_landed_one_is_skipped() {
    let (dir_a, root) = (tempdir(), tempdir());
    let autosaves = root.path().join("autosaves");
    if let Err(err) = std::fs::create_dir(&autosaves) {
        unreachable!("{err:?}");
    }
    let namespace = autosave_files::AutosaveNamespace::acquire(autosaves.clone(), 4247);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    let id = rig.doc.id.get();
    // A first park lands, then a recorded step moves the revision.
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    let first = worker.landed(id);
    assert!(first.is_some());
    rig.start_stroke();
    let _ = document_tabs::commit_live_gestures(&mut rig.cx(None));
    // The autosave directory goes away: the worker's newer write fails,
    // and the landed generation stays the older one.
    if let Err(err) = std::fs::remove_dir_all(&autosaves) {
        unreachable!("{err:?}");
    }
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert_eq!(worker.landed(id), first, "the newer write did not land");
    assert!(
        park_now(&mut rig, &mut worker, &namespace),
        "same revision, but the write failed: retried"
    );
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    // Back: the retry lands, and the next park skips.
    if let Err(err) = std::fs::create_dir(&autosaves) {
        unreachable!("{err:?}");
    }
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(worker.landed(id).is_some());
    assert!(namespace.session_path(id).exists());
    assert!(
        !park_now(&mut rig, &mut worker, &namespace),
        "landed: skipped"
    );
}

/// Review R-1: a snapshot that is not complete (tiles unreadable, so the
/// job goes to the *partial* path) records nothing, so the next park at
/// the same revision retries.
#[test]
fn a_partial_park_snapshot_is_retried() {
    let (store_dir, autosaves) = (tempdir(), tempdir());
    let Some(budget) = std::num::NonZeroUsize::new(1) else {
        unreachable!("1 is non-zero")
    };
    let mut store = match aurora_tile::TileStore::new(store_dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let mut tree = aurora_doc::LayerTree::new();
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 256,
        height: 256,
    };
    let mut last = None;
    for (name, rgba) in [
        ("broken", [1.0, 0.0, 0.0, 1.0]),
        ("intact", [0.0, 0.0, 1.0, 1.0]),
    ] {
        let id = match tree.add_pixel_layer(name, bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        fill_solid(&mut store, surface_id_for(id), TILE, rgba);
        last = Some(id);
    }
    if let Err(err) = store.flush() {
        unreachable!("{err:?}");
    }
    // The evicted tile's scratch file goes: the snapshot copies an
    // evicted tile's bytes from disk, so it cannot read this one.
    let files: Vec<std::path::PathBuf> = match std::fs::read_dir(store_dir.path()) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect(),
        Err(err) => unreachable!("{err:?}"),
    };
    assert!(!files.is_empty(), "a tile was evicted to scratch");
    for file in files {
        if let Err(err) = std::fs::remove_file(&file) {
            unreachable!("{err:?}");
        }
    }
    let session = DocumentSession::new(DocumentContents {
        layers: tree,
        history: aurora_doc::History::new(),
        canvas_size: (256, 256),
        skipped_tiles: aurora_io::SkippedTiles::new(),
        active_layer: last,
        canvas_view: aurora_ui::CanvasView::default(),
        tile_store: Some(store),
    });
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4248);
    let mut worker = background_autosave::AutosaveWorker::default();
    let mut rig = Rig::new(session);
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(
        rig.doc.park_autosave.is_none(),
        "a partial snapshot records nothing"
    );
    assert!(
        park_now(&mut rig, &mut worker, &namespace),
        "same revision, not saved complete: retried"
    );
}

/// Review D-1: the strip follower switches only when the user moved the
/// strip away from the selection the code last wrote — a stale selection
/// left by a failed sync (`strip_selection` cleared) never switches.
#[test]
fn the_strip_follower_ignores_a_selection_no_sync_wrote() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    assert_eq!(rig.shelf.strip_selection, Some(1));
    if let Err(err) =
        aurora_widgets::widgets::select_tab(&mut rig.workspace.tree, rig.workspace.document_tabs, 0)
    {
        unreachable!("{err:?}");
    }
    rig.shelf.strip_selection = None;
    assert_eq!(document_tabs::follow_document_tabs(&mut rig.cx(None)), None);
    assert_eq!(rig.doc.id, b_id, "no switch from a selection nothing wrote");
    rig.shelf.strip_selection = Some(1);
    assert_eq!(
        document_tabs::follow_document_tabs(&mut rig.cx(None)),
        Some(SwitchOutcome::Switched)
    );
    assert_eq!(rig.doc.id, a_id);
    assert_eq!(rig.shelf.strip_selection, Some(0));
}

// -- 0.174.0 (R5): unsaved state, Close Document and the quit review --

use crate::document_close::{
    self as close, CloseChoice, CloseEffect, CloseStep, QuitEffect, QuitReview, QuitStep,
};

/// One recorded step on the active document (what any edit does to its
/// `UndoOrder`).
fn edit(doc: &mut DocumentSession) {
    doc.undo_order.record_labelled(
        crate::UndoKind::Pixel,
        "Brush Stroke".to_owned(),
        &mut doc.history,
        &mut doc.pixel_history,
    );
}

fn descriptions(rig: &Rig) -> Vec<String> {
    match aurora_widgets::widgets::tab_bar_state(&rig.workspace.tree, rig.workspace.document_tabs) {
        Ok(state) => state.descriptions().to_vec(),
        Err(err) => unreachable!("{err:?}"),
    }
}

fn refresh(rig: &mut Rig) -> bool {
    close::refresh_dirty_marks(&mut rig.workspace, &mut rig.focus, &mut rig.shelf, &rig.doc)
}

/// AC-1: an open is clean, an edit makes it dirty, a landed `.aur` save
/// makes it clean, undo past the save makes it dirty again — and so does
/// undoing and redoing back *to* it (the chosen, conservative rule: a
/// revision is never reused) — and an export or a failed save never
/// clears it. A recovered document is unsaved from the start.
#[test]
fn dirty_tracking_follows_open_edit_save_undo_and_export() {
    let dir = tempdir();
    let (mut doc, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    assert!(!doc.is_dirty(), "an open (or New) is clean");
    edit(&mut doc);
    assert!(doc.is_dirty(), "an edit is unsaved");
    assert!(!close::after_save(
        &mut doc,
        std::path::Path::new("/x/out.png"),
        true
    ));
    assert!(doc.is_dirty(), "an export does not clear it");
    assert!(!close::after_save(
        &mut doc,
        std::path::Path::new("/x/out.aur"),
        false
    ));
    assert!(doc.is_dirty(), "a failed save does not clear it");
    assert!(close::after_save(
        &mut doc,
        std::path::Path::new("/x/out.aur"),
        true
    ));
    assert!(!doc.is_dirty(), "a landed .aur save is clean");
    assert_eq!(
        doc.path.as_deref(),
        Some(std::path::Path::new("/x/out.aur"))
    );
    assert_eq!(doc.name, "out.aur");
    doc.undo_order.step_back();
    assert!(
        doc.is_dirty(),
        "undo past the save is unsaved, never falsely clean"
    );
    doc.undo_order.step_forward();
    assert!(
        doc.is_dirty(),
        "redo back to the saved state stays unsaved (conservative)"
    );
    let (mut recovered, _) = session("Recovered 2", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    recovered.saved_revision = None;
    assert!(recovered.is_dirty(), "recovered work is in no file");
}

/// AC-2: the tab shows "• name" and the AT description "unsaved" while
/// unsaved, and the strip is relabelled only when that changes.
#[test]
fn tab_marks_follow_the_dirty_state_and_relabel_only_on_change() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let _ = rig.open(b);
    assert!(!refresh(&mut rig), "in sync after the open");
    assert_eq!(strip(&rig).0, tab_names(&["a", "b"]));
    edit(&mut rig.doc);
    assert!(refresh(&mut rig), "relabelled on the change");
    assert_eq!(strip(&rig).0, tab_names(&["a", "\u{2022} b"]));
    assert_eq!(descriptions(&rig), tab_names(&["", "unsaved"]));
    let Some(&b_tab) = strip(&rig).2.get(1) else {
        unreachable!("two tabs")
    };
    assert_eq!(
        rig.workspace
            .tree
            .accessibility(b_tab)
            .and_then(|node| node.description()),
        Some("unsaved")
    );
    assert!(!refresh(&mut rig), "no relabel without a change");
    assert!(!refresh(&mut rig));
    rig.doc.mark_clean();
    assert!(refresh(&mut rig));
    assert_eq!(strip(&rig).0, tab_names(&["a", "b"]));
    assert_eq!(descriptions(&rig), tab_names(&["", ""]));
}

/// AC-3: a clean document closes with no dialog, an unsaved one asks (a
/// live stroke counts); Don't Save closes, Save closes only once it
/// landed, and Cancel, a cancelled picker or a failed save keep it.
#[test]
fn a_clean_close_needs_no_dialog_and_an_unsaved_one_asks() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    assert_eq!(
        close::begin_close(&mut rig.cx(None), b_id),
        CloseStep::Close(b_id)
    );
    rig.start_stroke();
    assert_eq!(
        close::begin_close(&mut rig.cx(None), b_id),
        CloseStep::Ask(b_id),
        "the live stroke was committed and counts"
    );
    assert!(rig.drag.is_none());
    assert_eq!(
        close::begin_close(&mut rig.cx(None), a_id),
        CloseStep::Close(a_id)
    );
    assert_eq!(
        close::begin_close(&mut rig.cx(None), DocumentId::next()),
        CloseStep::Unknown
    );
    assert_eq!(
        close::close_effect(b_id, CloseChoice::DontSave, false),
        CloseEffect::Close(b_id)
    );
    assert_eq!(
        close::close_effect(b_id, CloseChoice::Save, true),
        CloseEffect::Close(b_id)
    );
    assert_eq!(
        close::close_effect(b_id, CloseChoice::Save, false),
        CloseEffect::Keep,
        "a cancelled picker or a failed save keeps the tab"
    );
    assert_eq!(
        close::close_effect(b_id, CloseChoice::Cancel, true),
        CloseEffect::Keep
    );
    assert_eq!(rig.shelf.len(), 2, "deciding closed nothing");
}

/// AC-4: closing activates the next tab (else the previous, else a fresh
/// "Untitled"), focus on the strip lands on a live tab, the store's
/// scratch files go, and the autosave file and index entry are removed.
#[test]
#[allow(clippy::too_many_lines)]
fn a_close_activates_the_neighbour_and_removes_the_store_and_autosave() {
    let (dir_a, dir_b, dir_c, autosaves) = (tempdir(), tempdir(), tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4250);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (mut b, _) = session("b", 1, [0.0, 1.0, 0.0, 1.0], &dir_b);
    // b's store pages tiles out to its scratch directory (budget 1).
    let Some(one) = std::num::NonZeroUsize::new(1) else {
        unreachable!("1 is non-zero")
    };
    let mut paging = match aurora_tile::TileStore::new(dir_b.path().to_path_buf(), one) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    for x in 0..3 {
        fill_solid(
            &mut paging,
            aurora_tile::SurfaceId::from_raw(7),
            aurora_tile::TileId { x, y: 0 },
            [0.5, 0.5, 0.5, 1.0],
        );
    }
    if let Err(err) = paging.flush() {
        unreachable!("{err:?}");
    }
    drop(b.tile_store.replace(paging));
    let tile_files = || {
        std::fs::read_dir(dir_b.path()).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tile"))
                .count()
        })
    };
    let paged = tile_files();
    assert!(paged >= 1, "b has scratch files before the close: {paged}");
    let (c, _) = session("c", 1, [0.0, 0.0, 1.0, 1.0], &dir_c);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = worker.configure_index(autosave_files::IndexBook::new(
        namespace.index_path(),
        document_tabs::index_entries(&rig.shelf, &rig.doc, namespace.key),
        Some(a_id.get()),
        [],
    ));
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    let _ = rig.open(c);
    let c_id = rig.doc.id;
    assert_eq!(rig.switch(b_id), SwitchOutcome::Switched);
    let _ = document_tabs::sync_autosave_sessions(&mut worker, &rig.shelf, &rig.doc, namespace.key);
    let b_path = namespace.session_path(b_id.get());
    {
        let doc = &mut rig.doc;
        let Some(store) = doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        crate::request_autosave(
            &mut worker,
            &b_path,
            doc.id.get(),
            &doc.layers,
            &doc.history,
            doc.canvas_size,
            &mut doc.skipped_tiles,
            store,
            None,
            None,
        );
    }
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(b_path.exists(), "b is autosaved before the close");
    assert!(
        worker
            .index_listing()
            .is_some_and(|index| index.entries.iter().any(|entry| entry.id == b_id.get()))
    );
    // Focus on b's tab (the selected one) when it closes.
    let Some(&b_tab) = strip(&rig).2.get(1) else {
        unreachable!("three tabs")
    };
    if let Err(err) = rig.focus.focus(&mut rig.workspace.tree, b_tab) {
        unreachable!("{err:?}");
    }
    let Some(closed) = close::close_document(&mut rig.cx(None), b_id, None) else {
        unreachable!("b closes")
    };
    assert_eq!(closed.id, b_id);
    assert_eq!(rig.doc.id, c_id, "the next tab becomes active");
    assert_eq!(strip(&rig).0, tab_names(&["a", "c"]));
    assert_eq!(strip(&rig).1, 1);
    let focused = rig.focus.focused();
    assert!(
        focused.is_some_and(|id| strip(&rig).2.contains(&id) && rig.workspace.tree.contains(id)),
        "focus is on a live tab, not the closed one: {focused:?}"
    );
    assert_eq!(focused, strip(&rig).2.get(1).copied(), "the selected tab");
    let removed = close::retire_session(closed, Some(&mut worker));
    assert!(removed >= paged, "{removed} of {paged}");
    assert_eq!(tile_files(), 0, "the store's scratch files are gone");
    let _ = document_tabs::sync_autosave_sessions(&mut worker, &rig.shelf, &rig.doc, namespace.key);
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(!b_path.exists(), "b's autosave file is removed");
    assert!(
        worker
            .index_listing()
            .is_some_and(|index| index.entries.iter().all(|entry| entry.id != b_id.get())),
        "and its index entry"
    );
    // The last tab: the previous one becomes active.
    let Some(closed) = close::close_document(&mut rig.cx(None), c_id, None) else {
        unreachable!("c closes")
    };
    drop(closed);
    assert_eq!(rig.doc.id, a_id, "the previous tab when there is no next");
    // The last document: refused without a replacement, else Untitled.
    assert!(close::close_document(&mut rig.cx(None), a_id, None).is_none());
    assert_eq!(rig.doc.id, a_id, "nothing changed");
    let (mut untitled, _) = session("x", 1, [0.0, 0.0, 0.0, 0.0], &dir_c);
    crate::document_session::UNTITLED.clone_into(&mut untitled.name);
    let untitled_id = untitled.id;
    assert!(close::close_document(&mut rig.cx(None), a_id, Some(untitled)).is_some());
    assert_eq!(rig.doc.id, untitled_id);
    assert_eq!(strip(&rig).0, tab_names(&["Untitled"]));
    assert!(!rig.doc.is_dirty());
}

/// AC-6 (F9): a close for a parked document closes that one, never the
/// active one, and a dialog's result names its own id.
#[test]
fn a_close_applies_to_its_own_document_not_the_active_one() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    // The dialog was for a; b became active meanwhile.
    let effect = crate::dialog_effect(&crate::DialogResult {
        purpose: crate::DialogPurpose::CloseDocument(a_id),
        action: close::CLOSE_DONT_SAVE.to_owned(),
    });
    assert_eq!(
        effect,
        crate::DialogEffect::Close(a_id, CloseChoice::DontSave)
    );
    let Some(closed) = close::close_document(&mut rig.cx(None), a_id, None) else {
        unreachable!("a closes")
    };
    assert_eq!(closed.id, a_id);
    assert_eq!(rig.doc.id, b_id, "the active document is untouched");
    assert_eq!(strip(&rig).0, tab_names(&["b"]));
    assert_eq!(rig.shelf.active_position, 0);
    let quit = crate::dialog_effect(&crate::DialogResult {
        purpose: crate::DialogPurpose::QuitReview(a_id),
        action: close::CLOSE_SAVE.to_owned(),
    });
    assert_eq!(quit, crate::DialogEffect::Quit(a_id, CloseChoice::Save));
}

/// AC-5: the quit review asks about each unsaved document in tab order;
/// Don't Save and a landed Save move on, Cancel or a failed save abort
/// (keeping everything, and never running the cleanup), and with nothing
/// unsaved the quit is immediate.
#[test]
fn the_quit_review_goes_through_unsaved_documents_and_cancel_keeps_everything() {
    let (dir_a, dir_b, dir_c) = (tempdir(), tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 1.0, 0.0, 1.0], &dir_b);
    let (c, _) = session("c", 1, [0.0, 0.0, 1.0, 1.0], &dir_c);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    assert_eq!(
        QuitReview::start(close::unsaved_in_tab_order(&rig.shelf, &rig.doc)).1,
        QuitStep::Quit
    );
    assert_eq!(
        close::quit_effect(QuitStep::Quit),
        QuitEffect::CleanupAndExit
    );
    edit(&mut rig.doc);
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    let _ = rig.open(c);
    let c_id = rig.doc.id;
    edit(&mut rig.doc);
    let unsaved = close::unsaved_in_tab_order(&rig.shelf, &rig.doc);
    assert_eq!(unsaved, vec![a_id, c_id], "tab order, clean b skipped");
    let (review, first) = QuitReview::start(unsaved.clone());
    let Some(mut review) = review else {
        unreachable!("a review")
    };
    assert_eq!(first, QuitStep::Ask(a_id));
    assert_eq!(
        review.answer(c_id, CloseChoice::DontSave, false),
        QuitStep::Abort,
        "out of turn"
    );
    let (Some(mut review2), _) = QuitReview::start(unsaved.clone()) else {
        unreachable!()
    };
    assert_eq!(
        review2.answer(a_id, CloseChoice::DontSave, false),
        QuitStep::Ask(c_id)
    );
    assert_eq!(
        review2.answer(c_id, CloseChoice::Save, true),
        QuitStep::Quit
    );
    assert_eq!(
        review.answer(a_id, CloseChoice::Save, false),
        QuitStep::Abort,
        "a failed save"
    );
    let (Some(mut review3), _) = QuitReview::start(unsaved) else {
        unreachable!()
    };
    assert_eq!(
        review3.answer(a_id, CloseChoice::Save, true),
        QuitStep::Ask(c_id)
    );
    assert_eq!(
        review3.answer(c_id, CloseChoice::Cancel, false),
        QuitStep::Abort
    );
    assert_eq!(
        close::quit_effect(QuitStep::Abort),
        QuitEffect::KeepRunning,
        "no cleanup"
    );
    assert_eq!(
        close::quit_effect(QuitStep::Ask(c_id)),
        QuitEffect::AskAbout(c_id)
    );
    // The review is a value: Cancel touched no document.
    assert_eq!(rig.shelf.len(), 3);
    assert!(rig.doc.is_dirty() && rig.parked(a_id).is_dirty() && !rig.parked(b_id).is_dirty());
}

/// Review Q-1: only an unconfirmed exit with something unsaved keeps the
/// recovery data; a confirmed quit (Don't Save included — the documents
/// stay dirty in memory, so the flag comes from the review) or nothing
/// unsaved cleans up. The final autosave writes each unsaved session, and
/// only those, and the index lists it.
#[test]
fn an_unconfirmed_exit_keeps_unsaved_work_and_writes_its_last_autosave() {
    let (dir_a, dir_b, autosaves) = (tempdir(), tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4252);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = worker.configure_index(autosave_files::IndexBook::new(
        namespace.index_path(),
        document_tabs::index_entries(&rig.shelf, &rig.doc, namespace.key),
        Some(a_id.get()),
        [],
    ));
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    let _ = document_tabs::sync_autosave_sessions(&mut worker, &rig.shelf, &rig.doc, namespace.key);
    assert!(!close::keeps_unsaved_work(
        false,
        &close::unsaved_in_tab_order(&rig.shelf, &rig.doc)
    ));
    edit(&mut rig.doc);
    let unsaved = close::unsaved_in_tab_order(&rig.shelf, &rig.doc);
    assert_eq!(unsaved, vec![b_id]);
    assert!(
        close::keeps_unsaved_work(false, &unsaved),
        "unconfirmed: kept"
    );
    assert!(
        !close::keeps_unsaved_work(true, &unsaved),
        "confirmed: cleaned"
    );
    // Don't Save, then the review ends: confirmed though still dirty.
    let (Some(mut review), _) = QuitReview::start(unsaved.clone()) else {
        unreachable!("a review")
    };
    assert_eq!(
        close::quit_effect(review.answer(b_id, CloseChoice::DontSave, false)),
        QuitEffect::CleanupAndExit
    );
    assert!(rig.doc.is_dirty());
    let written =
        close::final_autosave_unsaved(&mut worker, &namespace, &mut rig.shelf, &mut rig.doc);
    assert_eq!(written, 1, "only the unsaved one");
    assert!(namespace.session_path(b_id.get()).exists());
    assert!(
        !namespace.session_path(a_id.get()).exists(),
        "clean a is not written"
    );
    assert!(
        worker
            .index_listing()
            .is_some_and(|index| index.entries.iter().any(|entry| entry.id == b_id.get())),
        "the index lists the final autosave"
    );
}

/// Review Q-2 and Q-3: a dialog Save onto a non-`.aur` name is an export
/// that is reported; a window close that waited for a dialog starts the
/// review once no dialog is open and no review runs.
#[test]
fn a_dialog_save_to_a_non_aur_name_is_reported_and_a_waiting_quit_resumes() {
    assert_eq!(
        close::dialog_save_kind(std::path::Path::new("/x/a.aur")),
        close::DialogSave::Aur
    );
    assert_eq!(
        close::dialog_save_kind(std::path::Path::new("/x/a.png")),
        close::DialogSave::ExportOnly
    );
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let mut dialog = None;
    assert!(close::open_exported_not_saved_dialog(
        &mut rig.workspace,
        &mut rig.focus,
        &mut dialog,
        &rig.scales,
        "a",
    ));
    assert_eq!(
        dialog.as_ref().map(|open| open.purpose.clone()),
        Some(crate::DialogPurpose::ExportedNotSaved)
    );
    assert_eq!(
        crate::dialog_effect(&crate::DialogResult {
            purpose: crate::DialogPurpose::ExportedNotSaved,
            action: close::EXPORTED_NOT_SAVED_DISMISS.to_owned(),
        }),
        crate::DialogEffect::Acknowledged
    );
    assert_eq!(
        close::exported_not_saved_title("a"),
        "\u{201c}a\u{201d} Was Exported, Not Saved"
    );
    // F-3 (0.175.0): taken once no dialog is open, and cleared then.
    let mut pending = true;
    assert!(
        !close::take_pending_quit(&mut pending, true, false),
        "waits for the dialog"
    );
    assert!(pending);
    assert!(close::take_pending_quit(&mut pending, false, false));
    assert!(
        !pending,
        "cleared: it cannot fire on a later dialog's close"
    );
    assert!(!close::take_pending_quit(&mut pending, false, false));
    let mut pending = true;
    assert!(
        !close::take_pending_quit(&mut pending, false, true),
        "a review already runs"
    );
    assert!(!pending, "and the request is dropped");
}

// -- 0.175.0 (R6) --

/// AC-1: the platform's primary modifier binds the document chords —
/// `Cmd` on macOS, `Ctrl` elsewhere — while `Ctrl+Tab` stays literal.
#[test]
fn the_shortcut_table_uses_cmd_on_macos_and_ctrl_elsewhere() {
    let chord = |source: &str| match KeyChord::parse(source) {
        Ok(chord) => chord,
        Err(err) => unreachable!("{err:?}"),
    };
    assert_eq!(crate::primary_modifier(true), "Cmd");
    assert_eq!(crate::primary_modifier(false), "Ctrl");
    let mac = crate::default_shortcuts_for(true);
    let other = crate::default_shortcuts_for(false);
    for (source, command) in [
        ("W", AppCommand::CloseDocument),
        ("Z", AppCommand::Undo),
        ("Shift+Z", AppCommand::Redo),
        ("Shift+N", AppCommand::NewLayer),
        ("Shift+P", AppCommand::ToggleCommandPalette),
    ] {
        assert_eq!(
            mac.resolve(chord(&format!("Cmd+{source}"))),
            Some(&command),
            "{source}"
        );
        assert_eq!(
            mac.resolve(chord(&format!("Ctrl+{source}"))),
            None,
            "{source}"
        );
        assert_eq!(
            other.resolve(chord(&format!("Ctrl+{source}"))),
            Some(&command),
            "{source}"
        );
        assert_eq!(
            other.resolve(chord(&format!("Cmd+{source}"))),
            None,
            "{source}"
        );
    }
    for table in [&mac, &other] {
        assert_eq!(
            table.resolve(chord("Ctrl+Tab")),
            Some(&AppCommand::NextDocument)
        );
        assert_eq!(
            table.resolve(chord("Ctrl+Shift+Tab")),
            Some(&AppCommand::PreviousDocument)
        );
    }
    assert_eq!(mac.len(), other.len());
}

/// AC-1: the macOS application menu's "Quit Aurora" item maps to the quit
/// review's command — the same `App::request_quit` a window close runs —
/// and is the one menu command allowed past an open dialog.
#[test]
fn the_quit_menu_command_routes_to_the_quit_review() {
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let picked = crate::activate_command(
        &mut rig.workspace,
        &mut rig.focus,
        crate::COMMAND_APP_QUIT,
        &mut FakeFileDialog::default(),
    );
    assert_eq!(picked, Some(ActivatedCommand::Quit));
    assert!(crate::menu_command_allowed_with_dialog(
        crate::COMMAND_APP_QUIT
    ));
    assert!(!crate::menu_command_allowed_with_dialog(
        crate::COMMAND_FILE_SAVE
    ));
    assert!(!crate::menu_command_allowed_with_dialog(
        close::COMMAND_FILE_CLOSE
    ));
}

/// L-1 (0.175.0): superseding raises the stale floor, not `landed` —
/// `landed` is what is on disk.
#[test]
fn superseding_raises_the_floor_and_landed_stays_on_disk() {
    let (dir_a, autosaves) = (tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4253);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    let id = rig.doc.id.get();
    let _superseded = worker.supersede(id);
    assert_eq!(worker.landed(id), None, "nothing is on disk");
    assert!(park_now(&mut rig, &mut worker, &namespace));
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    let landed = worker.landed(id);
    assert!(landed.is_some(), "a newer write lands above the floor");
    let floor = worker.supersede(id);
    assert_eq!(worker.landed(id), landed, "a supersede is not a landing");
    worker.mark_landed(id, floor);
    assert_eq!(
        worker.landed(id),
        Some(floor),
        "a written synchronous autosave is"
    );
}

/// L-2 (0.175.0): a park at an unchanged revision while its write is
/// still in flight submits nothing new.
#[test]
fn a_park_while_its_write_is_in_flight_is_skipped() {
    let (dir_a, autosaves) = (tempdir(), tempdir());
    let namespace =
        autosave_files::AutosaveNamespace::acquire(autosaves.path().to_path_buf(), 4254);
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new((started_tx, release_rx, true));
    let mut worker = background_autosave::AutosaveWorker::new(
        move |job, cancel| {
            if let Ok(mut gate) = gate.lock()
                && gate.2
            {
                gate.2 = false;
                let _ = gate.0.send(());
                let _ = gate.1.recv_timeout(std::time::Duration::from_secs(20));
            }
            crate::write_autosave_temp(job, cancel)
        },
        || false,
    );
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    let id = rig.doc.id.get();
    assert!(
        park_now(&mut rig, &mut worker, &namespace),
        "the first park submits"
    );
    assert!(
        started
            .recv_timeout(std::time::Duration::from_secs(20))
            .is_ok()
    );
    let Some((_, generation)) = rig.doc.park_autosave else {
        unreachable!("a complete snapshot was submitted")
    };
    assert!(worker.outstanding(id, generation), "in flight");
    assert!(
        !park_now(&mut rig, &mut worker, &namespace),
        "same revision, write in flight: skipped"
    );
    let _ = release.send(());
    assert!(worker.wait_idle(std::time::Duration::from_secs(20)));
    assert!(!worker.outstanding(id, generation));
    assert!(worker.landed(id).is_some_and(|landed| landed >= generation));
    assert!(
        !park_now(&mut rig, &mut worker, &namespace),
        "landed: skipped"
    );
}

/// F-1 (0.175.0): a final autosave that did not land is neither counted
/// nor named in the index.
#[test]
fn a_final_autosave_that_does_not_land_is_not_counted_or_indexed() {
    let (dir_a, root) = (tempdir(), tempdir());
    let autosaves = root.path().join("autosaves");
    if let Err(err) = std::fs::create_dir(&autosaves) {
        unreachable!("{err:?}");
    }
    let namespace = autosave_files::AutosaveNamespace::acquire(autosaves.clone(), 4255);
    let mut worker = background_autosave::AutosaveWorker::default();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let mut rig = Rig::new(a);
    let id = rig.doc.id;
    edit(&mut rig.doc);
    // The autosave directory is gone: the write cannot land.
    if let Err(err) = std::fs::remove_dir_all(&autosaves) {
        unreachable!("{err:?}");
    }
    let written =
        close::final_autosave_unsaved(&mut worker, &namespace, &mut rig.shelf, &mut rig.doc);
    assert_eq!(written, 0, "not counted");
    assert_eq!(worker.landed(id.get()), None, "not recorded as landed");
    assert!(!namespace.session_path(id.get()).exists());
}

/// AC-2 (0.175.0): a document tab's "×" is hit by the pointer at its own
/// box, maps to that tab's document by id, and closes it through Close
/// Document's own path — a clean one at once, an unsaved one asks.
#[test]
fn a_tabs_close_button_names_its_document_and_closes_through_close_document() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    edit(&mut rig.doc);
    let _ = refresh(&mut rig);
    rig.workspace.tree.compute_layout(1280.0, 800.0);
    let bar = rig.workspace.document_tabs;
    for (index, id, step) in [
        (0, a_id, CloseStep::Close(a_id)),
        (1, b_id, CloseStep::Ask(b_id)),
    ] {
        let Some(button) =
            aurora_widgets::widgets::tab_close_button(&rig.workspace.tree, bar, index)
        else {
            unreachable!("tab {index} is closable")
        };
        let Some(bounds) = rig.workspace.tree.bounds(button) else {
            unreachable!("laid out")
        };
        #[allow(clippy::cast_precision_loss)]
        let center = (
            bounds.x as f32 + bounds.width as f32 / 2.0,
            bounds.y as f32 + bounds.height as f32 / 2.0,
        );
        assert_eq!(
            aurora_ui::document_tab_close_at(&rig.workspace.tree, bar, center),
            Some(index)
        );
        let name = rig
            .workspace
            .tree
            .accessibility(button)
            .and_then(|node| node.label().map(str::to_owned));
        let expected = if index == 0 { "Close a" } else { "Close b" };
        assert_eq!(name.as_deref(), Some(expected));
        assert_eq!(close::tab_document(&rig.shelf, &rig.doc, index), Some(id));
        assert_eq!(close::begin_close(&mut rig.cx(None), id), step);
    }
    // A press on the tab's label is not a close.
    let Some(tab) = strip(&rig).2.first().copied() else {
        unreachable!("a tab")
    };
    let Some(bounds) = rig.workspace.tree.bounds(tab) else {
        unreachable!("laid out")
    };
    #[allow(clippy::cast_precision_loss)]
    let left = (
        bounds.x as f32 + 2.0,
        bounds.y as f32 + bounds.height as f32 / 2.0,
    );
    assert_eq!(
        aurora_ui::document_tab_close_at(&rig.workspace.tree, bar, left),
        None
    );
    assert_eq!(close::tab_document(&rig.shelf, &rig.doc, 9), None);
}

/// Review P-1 (0.175.0): the drawn label carries the unsaved bullet, but
/// neither the tab's accessible name nor its close button's does — the
/// "unsaved" description says it.
#[test]
fn accessible_names_never_announce_the_unsaved_bullet() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let _ = rig.open(b);
    edit(&mut rig.doc);
    assert!(refresh(&mut rig));
    assert_eq!(strip(&rig).0, tab_names(&["a", "\u{2022} b"]), "drawn");
    let bar = rig.workspace.document_tabs;
    let Some(&b_tab) = strip(&rig).2.get(1) else {
        unreachable!("two tabs")
    };
    let node = rig.workspace.tree.accessibility(b_tab);
    assert_eq!(
        node.and_then(|node| node.label()),
        Some("b"),
        "the tab's name"
    );
    assert_eq!(node.and_then(|node| node.description()), Some("unsaved"));
    let close = aurora_widgets::widgets::tab_close_button(&rig.workspace.tree, bar, 1)
        .and_then(|button| rig.workspace.tree.accessibility(button))
        .and_then(|node| node.label().map(str::to_owned));
    assert_eq!(close.as_deref(), Some("Close b"));
    // A rename follows (the names are re-set on every sync).
    "c.aur".clone_into(&mut rig.doc.name);
    assert!(refresh(&mut rig));
    let node = rig.workspace.tree.accessibility(b_tab);
    assert_eq!(node.and_then(|node| node.label()), Some("c.aur"));
}

/// Review P-2 (0.175.0): a disabled bar's close buttons close nothing, by
/// pointer or assistive technology (both routes go through
/// `tab_close_index`).
#[test]
fn a_disabled_strips_close_buttons_close_nothing() {
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    rig.workspace.tree.compute_layout(1280.0, 800.0);
    let bar = rig.workspace.document_tabs;
    let Some(button) = aurora_widgets::widgets::tab_close_button(&rig.workspace.tree, bar, 0)
    else {
        unreachable!("closable")
    };
    let Some(bounds) = rig.workspace.tree.bounds(button) else {
        unreachable!("laid out")
    };
    #[allow(clippy::cast_precision_loss)]
    let center = (
        bounds.x as f32 + bounds.width as f32 / 2.0,
        bounds.y as f32 + bounds.height as f32 / 2.0,
    );
    assert_eq!(
        aurora_ui::document_tab_close_at(&rig.workspace.tree, bar, center),
        Some(0)
    );
    assert!(
        aurora_widgets::widgets::set_tab_bar_disabled(&mut rig.workspace.tree, bar, true).is_ok()
    );
    assert_eq!(
        aurora_ui::document_tab_close_at(&rig.workspace.tree, bar, center),
        None,
        "pointer"
    );
    assert_eq!(
        aurora_ui::document_tab_close_index(&rig.workspace.tree, bar, button),
        None,
        "assistive technology"
    );
    let node = rig.workspace.tree.accessibility(button);
    assert!(node.is_some_and(|node| !node.supports_action(accesskit::Action::Click)));
}

/// 0.185.0, AC-3: open documents share one residency pool. A parked
/// document painted at a full share is cut to half when a second store
/// joins — through the eviction path, so every pixel comes back — and
/// its share grows back when the other store closes.
#[test]
fn open_documents_share_the_tile_budget_and_a_parked_one_shrinks_without_loss() {
    let Some(total) = std::num::NonZeroUsize::new(8) else {
        unreachable!("non-zero")
    };
    let pool = aurora_tile::ResidencyPool::new(total, std::num::NonZeroUsize::MIN);
    let shared = |dir: &tempfile::TempDir| match aurora_tile::TileStore::new_shared(
        dir.path().to_path_buf(),
        &pool,
    ) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (mut parked, ids) = session("A", 1, [0.25, 0.5, 0.75, 1.0], &dir_a);
    let Some(&layer) = ids.first() else {
        unreachable!("one layer")
    };
    let surface = surface_id_for(layer);
    let mut store_a = shared(&dir_a);
    let tiles: Vec<aurora_tile::TileId> = (0..8).map(|x| aurora_tile::TileId { x, y: 0 }).collect();
    for (n, &tile) in tiles.iter().enumerate() {
        let v = n as f32 / 8.0;
        fill_solid(&mut store_a, surface, tile, [v, 0.5, 1.0 - v, 1.0]);
    }
    let expected: Vec<Vec<f32>> = tiles
        .iter()
        .map(|&tile| read_all_texels(&mut store_a, surface, tile))
        .collect();
    assert_eq!(store_a.resident_len(), 8);
    parked.tile_store = Some(store_a);
    let mut shelf = DocumentShelf::default();
    shelf.parked.push(parked);

    let (mut active, _) = session("B", 1, [1.0, 0.0, 0.0, 1.0], &dir_b);
    active.tile_store = Some(shared(&dir_b));
    assert_eq!(pool.members(), 2);
    // RB-1: one eviction per store per turn here, so the parked store,
    // four over, needs four turns and reports "still over" until then.
    let mut turns = 0;
    loop {
        let (evicted, over) = shelf.enforce_tile_budgets(&mut active, 1);
        turns += 1;
        assert!(evicted <= 1);
        if !over {
            break;
        }
        assert!(turns < 8, "no progress");
    }
    assert_eq!(turns, 4);
    let Some(parked_store) = shelf.parked.first_mut().and_then(|s| s.tile_store.as_mut()) else {
        unreachable!("parked document keeps its store")
    };
    assert_eq!(
        (parked_store.resident_len(), parked_store.budget_tiles()),
        (4, 4)
    );
    if let Err(err) = parked_store.flush() {
        unreachable!("{err:?}");
    }
    for (tile, want) in tiles.iter().zip(&expected) {
        let got = read_all_texels(parked_store, surface, *tile);
        assert!(
            got.iter()
                .zip(want)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "tile {tile:?} round-trips bit for bit after the shrink"
        );
    }

    // Closing the active document's store hands its share back.
    active.tile_store = None;
    assert_eq!(pool.members(), 1);
    let Some(parked_store) = shelf.parked.first_mut().and_then(|s| s.tile_store.as_mut()) else {
        unreachable!("parked document keeps its store")
    };
    assert_eq!(parked_store.budget_tiles(), 8);
    assert_eq!(shelf.enforce_tile_budgets(&mut active, 64), (0, false));
}

/// A one-layer session whose store is a member of `pool`, with `tiles`
/// tiles of its layer painted (each a distinct solid colour).
fn pooled_session(
    name: &str,
    tiles: u32,
    dir: &tempfile::TempDir,
    pool: &std::sync::Arc<aurora_tile::ResidencyPool>,
) -> (DocumentSession, aurora_tile::SurfaceId) {
    let (mut session, ids) = session(name, 1, [0.0, 0.0, 0.0, 0.0], dir);
    let Some(&layer) = ids.first() else {
        unreachable!("one layer")
    };
    let surface = surface_id_for(layer);
    let Ok(sub) = tempfile::tempdir_in(dir.path()) else {
        unreachable!("a scratch subdirectory")
    };
    let mut store = match aurora_tile::TileStore::new_shared(sub.keep(), pool) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    for x in 0..tiles {
        let v = x as f32 / 16.0;
        fill_solid(
            &mut store,
            surface,
            aurora_tile::TileId { x, y: 0 },
            [v, 0.5, 1.0 - v, 1.0],
        );
    }
    session.tile_store = Some(store);
    (session, surface)
}

/// Every open session in `rig` whose store holds the pool's active share.
fn active_share_holders(rig: &Rig) -> Vec<DocumentId> {
    std::iter::once(&rig.doc)
        .chain(rig.shelf.parked.iter())
        .filter(|session| {
            session
                .tile_store
                .as_ref()
                .is_some_and(aurora_tile::TileStore::is_pool_active)
        })
        .map(|session| session.id)
        .collect()
}

fn budget_of(session: &DocumentSession) -> usize {
    session
        .tile_store
        .as_ref()
        .map_or(0, aurora_tile::TileStore::budget_tiles)
}

/// 0.187.0: the active document holds the pool's bigger share (three
/// quarters; the parked split the last quarter), and every change of the
/// active document — an open's install, a switch, a reverted open, a
/// close — moves it, so exactly one open document holds it. A pending
/// open's store counts as a parked member. The document switched away
/// from shrinks through the ordinary eviction path and loses nothing.
#[test]
fn the_active_document_holds_the_bigger_share_through_open_switch_and_close() {
    let Some(total) = std::num::NonZeroUsize::new(16) else {
        unreachable!("non-zero")
    };
    let pool = aurora_tile::ResidencyPool::new(total, std::num::NonZeroUsize::MIN);
    let (dir_a, dir_b, dir_c) = (tempdir(), tempdir(), tempdir());
    let (a, surface_a) = pooled_session("A", 12, &dir_a, &pool);
    let a_id = a.id;
    let mut rig = Rig::new(a);
    // App::new's claim for the startup document.
    assert!(rig.doc.claim_active_tile_share());
    assert_eq!(active_share_holders(&rig), [a_id]);
    assert_eq!(budget_of(&rig.doc), 16, "alone, the whole pool");
    let expected: Vec<Vec<f32>> = {
        let Some(store) = rig.doc.tile_store.as_mut() else {
            unreachable!("a store")
        };
        (0..12)
            .map(|x| read_all_texels(store, surface_a, aurora_tile::TileId { x, y: 0 }))
            .collect()
    };

    // A pending open's store is a parked member: A keeps the bigger share.
    let (b, _) = pooled_session("B", 0, &dir_b, &pool);
    let b_id = b.id;
    assert_eq!(
        (pool.members(), budget_of(&rig.doc), budget_of(&b)),
        (2, 12, 4)
    );
    assert_eq!(active_share_holders(&rig), [a_id]);

    // Its install moves the bigger share to B; A, parked, shrinks
    // through eviction to its quarter and reads back bit for bit.
    let _ = rig.open(b);
    assert_eq!(rig.doc.id, b_id);
    assert_eq!(active_share_holders(&rig), [b_id]);
    assert_eq!((budget_of(&rig.doc), budget_of(rig.parked(a_id))), (12, 4));
    let (evicted, over) = rig.shelf.enforce_tile_budgets(&mut rig.doc, usize::MAX);
    assert_eq!((evicted, over), (8, false));
    {
        let Some(store) = rig
            .shelf
            .parked
            .iter_mut()
            .find(|session| session.id == a_id)
            .and_then(|session| session.tile_store.as_mut())
        else {
            unreachable!("A is parked with its store")
        };
        assert_eq!(store.resident_len(), 4);
        for (x, want) in (0..12).zip(&expected) {
            let got = read_all_texels(store, surface_a, aurora_tile::TileId { x, y: 0 });
            assert!(
                got.iter()
                    .zip(want)
                    .all(|(g, w)| g.to_bits() == w.to_bits()),
                "tile {x} round-trips after the shrink"
            );
        }
        // Reading every tile back paged them in; the parked store is
        // brought inside its quarter again.
        let _ = store.enforce_budget(usize::MAX);
    }

    // A switch moves it back.
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    assert_eq!(active_share_holders(&rig), [a_id]);
    assert_eq!((budget_of(&rig.doc), budget_of(rig.parked(b_id))), (12, 4));

    // An open that failed after its session went active is reverted:
    // the share returns to A, and C's store leaves the pool.
    let (c, _) = pooled_session("C", 0, &dir_c, &pool);
    let parked_at = rig.open(c);
    assert_eq!(active_share_holders(&rig), [rig.doc.id]);
    assert_ne!(rig.doc.id, a_id);
    let abandoned = rig.shelf.revert_push(&mut rig.doc, parked_at);
    assert!(abandoned.is_some());
    drop(abandoned);
    assert_eq!(rig.doc.id, a_id);
    assert_eq!(active_share_holders(&rig), [a_id]);
    assert_eq!(pool.members(), 2);

    // Closing the active document: B, the neighbour, takes the share
    // (alone now, the whole pool).
    let Some(closed) = close::close_document(&mut rig.cx(None), a_id, None) else {
        unreachable!("A closes")
    };
    assert_eq!(active_share_holders(&rig), [b_id]);
    assert!(
        !closed
            .tile_store
            .as_ref()
            .is_some_and(aurora_tile::TileStore::is_pool_active),
        "the closed document no longer holds it"
    );
    drop(closed);
    assert_eq!((pool.members(), budget_of(&rig.doc)), (1, 16));
    assert_eq!(pool.active_members(), 1);
}

/// 0.185.0 review RB-3: residency never changes a pixel. A six-tile
/// document of Normal, Multiply and Screen layers (one in a group, two
/// translucent) composites bit-identically through a two-tile fixed
/// store, which evicts and pages in throughout, and a pooled store that
/// holds it all.
#[test]
fn a_composite_is_bit_identical_at_a_thrashing_budget_and_a_roomy_pooled_one() {
    let canvas = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 768,
        height: 512,
    };
    let mut tree = aurora_doc::LayerTree::new();
    let add = |tree: &mut aurora_doc::LayerTree, name: &str, parent| match tree
        .add_pixel_layer(name, canvas, parent)
    {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let bottom = add(&mut tree, "bottom", None);
    let multiply = add(&mut tree, "multiply", None);
    let group = match tree.add_group("group", None) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    let screen = add(&mut tree, "screen", Some(group));
    for (id, mode, opacity) in [
        (multiply, aurora_doc::BlendMode::Multiply, 0.75),
        (screen, aurora_doc::BlendMode::Screen, 0.5),
    ] {
        if let Err(err) = tree.set_blend_mode(id, mode) {
            unreachable!("{err:?}");
        }
        if let Err(err) = tree.set_opacity(id, opacity) {
            unreachable!("{err:?}");
        }
    }
    let fill = |store: &mut aurora_tile::TileStore| {
        for (n, layer) in [bottom, multiply, screen].into_iter().enumerate() {
            for x in 0..3_u32 {
                for y in 0..2_u32 {
                    if (x + y + n as u32) % 3 == 2 && n > 0 {
                        continue;
                    }
                    let v = (x * 2 + y + n as u32) as f32 / 8.0;
                    let rgba = [v, 1.0 - v, 0.25 + v / 2.0, 1.0 - n as f32 / 8.0];
                    fill_solid(
                        store,
                        surface_id_for(layer),
                        aurora_tile::TileId { x, y },
                        rgba,
                    );
                }
            }
        }
    };
    let (dir_small, dir_pool) = (tempdir(), tempdir());
    let Some(two) = std::num::NonZeroUsize::new(2) else {
        unreachable!("non-zero")
    };
    let mut small = match aurora_tile::TileStore::new(dir_small.path().to_path_buf(), two) {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    let Some(roomy) = std::num::NonZeroUsize::new(4096) else {
        unreachable!("non-zero")
    };
    let pool = aurora_tile::ResidencyPool::new(roomy, std::num::NonZeroUsize::MIN);
    let mut pooled = match aurora_tile::TileStore::new_shared(dir_pool.path().to_path_buf(), &pool)
    {
        Ok(store) => store,
        Err(err) => unreachable!("{err:?}"),
    };
    fill(&mut small);
    fill(&mut pooled);
    let composite = |store: &mut aurora_tile::TileStore| match crate::composite_document(
        &tree, store, 768, 512,
    ) {
        Ok(image) => image
            .samples()
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<u16>>(),
        Err(err) => unreachable!("{err:?}"),
    };
    let (thrashed, held) = (composite(&mut small), composite(&mut pooled));
    // A re-read of an evicted tile is a page-in from the scratch disk
    // (`faults`) or, when the background writer has not landed it yet, a
    // decode of its still-queued bytes (`pending_hits`): which one depends
    // on writer timing (a fast CI runner saw only pending hits), so count
    // both as "paged".
    let small_stats = small.stats();
    assert!(
        small_stats.evictions > 0 && small_stats.faults + small_stats.pending_hits > 0,
        "the small store paged"
    );
    assert_eq!(
        pooled.stats().evictions,
        0,
        "the pooled store held everything"
    );
    assert_eq!(thrashed.len(), 768 * 512 * 4);
    assert!(thrashed.iter().any(|&b| b != 0), "not a blank composite");
    let differ = thrashed.iter().zip(&held).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "residency changed {differ} samples");
}

// -- The Layers panel's filter (0.199.0, round L5b) --

/// Types `text` into the filter field and applies it, as the key path does.
fn type_into_filter(rig: &mut Rig, text: &str) -> bool {
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("the strip exists");
    };
    if let Err(err) = aurora_widgets::widgets::insert_text_field_text(
        &mut rig.workspace.tree,
        controls.filter.field,
        text,
    ) {
        unreachable!("{err:?}");
    }
    apply_filter(rig, None)
}

fn apply_filter(rig: &mut Rig, activated: Option<WidgetId>) -> bool {
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("the strip exists");
    };
    crate::apply_layer_filter_edit(
        &mut rig.workspace,
        &mut rig.focus,
        &rig.scales,
        &rig.doc.layers,
        &mut rig.layer_rows,
        &mut rig.doc.active_layer,
        &mut rig.doc.canvas_view,
        &controls,
        activated,
    )
}

fn filter_text(rig: &Rig) -> String {
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("the strip exists");
    };
    aurora_ui::read_layer_filter(&rig.workspace.tree, &controls.filter).text
}

#[test]
fn each_document_keeps_its_own_filter_and_a_new_document_starts_clean() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("beta", 2, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let (a_id, b_id) = (a.id, b.id);
    let mut rig = Rig::new(a);
    assert!(type_into_filter(&mut rig, "alpha 2"));
    assert_eq!(rig.layer_rows.len(), 1);
    rig.open(b);
    assert_eq!(rig.layer_rows.len(), 2, "a new document shows every layer");
    assert_eq!(filter_text(&rig), "");
    assert!(!rig.workspace.layer_view.filter.is_active());
    assert!(type_into_filter(&mut rig, "beta 1"));
    assert_eq!(rig.layer_rows.len(), 1);
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    assert_eq!(filter_text(&rig), "alpha 2", "saved with the session");
    assert_eq!(rig.layer_rows.len(), 1);
    assert_eq!(rig.switch(b_id), SwitchOutcome::Switched);
    assert_eq!(filter_text(&rig), "beta 1");
    assert_eq!(rig.layer_rows.len(), 1);
    assert_eq!(rig.parked(a_id).layer_filter.text, "alpha 2");
}

#[test]
fn a_filter_edit_is_not_document_state() {
    let dir = tempdir();
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    // One real undoable step, so the History panel has a row to lose.
    let Some(layer) = rig.doc.active_layer else {
        unreachable!("active")
    };
    if let Err(err) = rig.doc.history.set_opacity(&mut rig.doc.layers, layer, 0.5) {
        unreachable!("{err:?}");
    }
    rig.doc.undo_order.record(
        crate::UndoKind::Structural,
        &mut rig.doc.history,
        &mut rig.doc.pixel_history,
    );
    crate::refresh_history_panel(&mut rig.workspace, &rig.doc.undo_order);
    let descendants = |rig: &Rig| {
        let mut count = 0;
        let mut pending = vec![rig.workspace.history.body];
        while let Some(id) = pending.pop() {
            count += 1;
            pending.extend(
                rig.workspace
                    .tree
                    .children(id)
                    .unwrap_or_default()
                    .iter()
                    .copied(),
            );
        }
        count
    };
    let history_rows = descendants(&rig);
    let dirty_before = rig.doc.is_dirty();
    let undo_before = rig.doc.undo_order.undo.len();
    let journal_before = rig.doc.history.journal_len();
    assert!(type_into_filter(&mut rig, "alpha"));
    assert_eq!(descendants(&rig), history_rows, "History is never filtered");
    assert!(type_into_filter(&mut rig, " 3"));
    assert_eq!(rig.doc.undo_order.undo.len(), undo_before, "no undo step");
    assert_eq!(
        rig.doc.history.journal_len(),
        journal_before,
        "no journal op"
    );
    assert_eq!(
        rig.doc.is_dirty(),
        dirty_before,
        "never changes the unsaved flag"
    );
    assert!(!type_into_filter(&mut rig, ""), "no change, no rebuild");
}

#[test]
fn the_active_layer_hidden_by_the_filter_is_still_edited_by_the_controls() {
    let dir = tempdir();
    let (a, ids) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let Some(active) = rig.doc.active_layer else {
        unreachable!("active")
    };
    let Some(&other) = ids.iter().find(|&&id| id != active) else {
        unreachable!("another layer")
    };
    let name = rig.doc.layers.name(other).unwrap_or_default().to_owned();
    assert!(type_into_filter(&mut rig, &name));
    assert_eq!(
        rig.doc.active_layer,
        Some(active),
        "the filter never changes it"
    );
    assert!(
        !rig.layer_rows.values().any(|&id| id == active),
        "its row is hidden"
    );
    let _ = crate::sync_layer_controls(
        &mut rig.workspace,
        &rig.layer_controls,
        &rig.doc.layers,
        rig.doc.active_layer,
        None,
    );
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("strip")
    };
    assert!(matches!(
        rig.workspace.tree.payload(controls.opacity),
        Some(aurora_widgets::widgets::WidgetKind::Slider(s)) if !s.disabled
    ));
}

#[test]
fn a_layer_added_under_a_filter_that_does_not_match_stays_hidden_without_a_scroll_jump() {
    let dir = tempdir();
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    assert!(type_into_filter(&mut rig, "alpha"));
    let before = rig.workspace.tree.scroll_y(rig.workspace.layers.body);
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 256,
        height: 256,
    };
    let added = match rig.doc.layers.add_pixel_layer("zzz", bounds, None) {
        Ok(id) => id,
        Err(err) => unreachable!("{err:?}"),
    };
    crate::rebuild_layer_rows(
        &mut rig.workspace,
        &mut rig.focus,
        &rig.scales,
        &rig.doc.layers,
        &mut rig.layer_rows,
        &mut rig.doc.active_layer,
        &mut rig.doc.canvas_view,
        Some(added),
    );
    assert_eq!(
        rig.layer_rows.len(),
        3,
        "the three alphas, not the new layer"
    );
    assert!(!rig.layer_rows.values().any(|&id| id == added));
    assert_eq!(rig.doc.active_layer, Some(added));
    assert_eq!(
        rig.workspace.tree.scroll_y(rig.workspace.layers.body),
        before
    );
}

#[test]
fn typing_in_the_filter_field_never_reaches_a_global_shortcut() {
    let dir = tempdir();
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("strip")
    };
    if let Err(err) = rig
        .focus
        .focus(&mut rig.workspace.tree, controls.filter.field)
    {
        unreachable!("{err:?}");
    }
    let mut clipboard = FakeClipboard::default();
    // `b` is the brush shortcut and `v` the move tool's: both are typed.
    for letter in ['b', 'v'] {
        let routed = crate::route_widget_key(
            &mut rig.workspace,
            &mut rig.focus,
            &mut None,
            Some(&controls),
            None,
            &rig.scales,
            false,
            false,
            Modifiers::none(),
            Key::Character(letter),
            Some(&letter.to_string()),
            &mut clipboard,
        );
        assert!(
            matches!(routed, Some((crate::WidgetOwner::LayerControls, _))),
            "{letter}: consumed by the field"
        );
    }
    assert_eq!(filter_text(&rig), "bv");
    assert!(apply_filter(&mut rig, None));
    assert_eq!(rig.layer_rows.len(), 0, "no layer is named like that");
}

#[test]
fn a_kind_toggle_press_flips_and_filters() {
    let dir = tempdir();
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let Some(controls) = rig.layer_controls.controls else {
        unreachable!("strip")
    };
    assert!(apply_filter(&mut rig, Some(controls.filter.group)));
    assert!(rig.workspace.layer_view.filter.group);
    assert_eq!(rig.layer_rows.len(), 0, "no groups in this document");
    assert!(apply_filter(&mut rig, Some(controls.filter.group)));
    assert!(!rig.workspace.layer_view.filter.is_active());
    assert_eq!(rig.layer_rows.len(), 3);
}

#[test]
fn trailing_spaces_do_not_rebuild_the_list_and_are_kept_in_the_field() {
    let dir = tempdir();
    let (a, _) = session("alpha", 3, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    assert!(type_into_filter(&mut rig, "alpha 2"));
    let rows = rig.layer_rows.clone();
    assert!(!type_into_filter(&mut rig, " "), "same rows: no rebuild");
    assert_eq!(rig.layer_rows, rows, "the very same row widgets");
    assert_eq!(filter_text(&rig), "alpha 2 ", "the field keeps the space");
    assert_eq!(rig.workspace.layer_view.filter.text, "alpha 2 ");
    // The per-loop sync must not erase it either.
    let _ = crate::sync_layer_controls(
        &mut rig.workspace,
        &rig.layer_controls,
        &rig.doc.layers,
        rig.doc.active_layer,
        None,
    );
    assert_eq!(filter_text(&rig), "alpha 2 ");
}

/// 0.213.0 review: a tab switch, close or quit commits a live Marquee drag
/// into the outgoing document as its selection step, so undoing there
/// undoes that selection.
#[test]
fn a_live_marquee_drag_is_recorded_when_the_document_is_left() {
    let dir = tempdir();
    let (a, _) = session("a", 1, [1.0, 0.0, 0.0, 1.0], &dir);
    let mut rig = Rig::new(a);
    let rect = aurora_core::Rect {
        x: 2,
        y: 3,
        width: 20,
        height: 10,
    };
    rig.doc
        .selection
        .select(aurora_doc::Selection::ellipse(rect));
    rig.drag = Some(Drag::Marquee {
        start_doc: (2.0, 3.0),
        elliptical: true,
        before: None,
        current: rig.doc.selection.active(),
        modal: crate::selection_modes::ModeGesture::boxed_replace(),
    });
    assert!(document_tabs::commit_live_gestures(&mut rig.cx(None)));
    assert!(rig.drag.is_none());
    assert_eq!(
        rig.doc.undo_order.undo.last(),
        Some(&crate::UndoKind::Selection)
    );
    assert!(!rig.doc.is_dirty(), "a selection never dirties");
    let doc = &mut rig.doc;
    assert!(
        crate::step_undo_order(
            &mut doc.layers,
            &mut doc.history,
            &mut doc.pixel_history,
            doc.tile_store.as_mut(),
            &mut doc.undo_order,
            crate::UndoDirection::Undo,
        )
        .is_some()
    );
    crate::selection_view::apply_pending_selection(&mut doc.undo_order, &mut doc.selection);
    assert!(doc.selection.active().is_none());
}

// -- 0.217.0 fix: Quick Mask must survive the per-turn tab follower --

/// An id no open document has.
fn unknown_document_id() -> DocumentId {
    let dir = tempdir();
    let (other, _) = session("x", 1, [0.0; 4], &dir);
    other.id
}

fn quick_mask_with_a_hole(doc: &DocumentSession) -> crate::quick_mask::QuickMaskMode {
    let mut mode = match crate::quick_mask::enter(doc.selection.active().as_ref(), doc.canvas_size)
    {
        Ok(mode) => mode,
        Err(why) => unreachable!("{why:?}"),
    };
    // Deselect a block: an edit, so leaving changes the selection.
    mode.mask.edit(|data, w, _| {
        for y in 10..20_usize {
            for x in 10..20_usize {
                if let Some(v) = data.get_mut(y * w as usize + x) {
                    *v = 0;
                }
            }
        }
        Some(aurora_core::Rect {
            x: 10,
            y: 10,
            width: 10,
            height: 10,
        })
    });
    mode
}

/// The per-loop-turn work `about_to_wait` does for tabs
/// (`follow_document_tabs`, run every turn) must leave Quick Mask alone
/// when no tab changed; a real switch leaves it and applies the grid to the
/// OUTGOING document.
#[test]
fn quick_mask_survives_the_per_turn_follower_and_a_real_switch_applies_it_to_the_outgoing_document()
{
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    let _ = rig.open(b);
    let b_id = rig.doc.id;
    rig.quick_mask = Some(quick_mask_with_a_hole(&rig.doc));
    for _ in 0..50 {
        assert_eq!(document_tabs::follow_document_tabs(&mut rig.cx(None)), None);
        assert!(rig.quick_mask.is_some(), "the idle follower left the mode");
    }
    assert_eq!(rig.doc.id, b_id);
    // A refused or no-op switch leaves it on too.
    assert_eq!(rig.switch(b_id), SwitchOutcome::AlreadyActive);
    assert_eq!(rig.switch(unknown_document_id()), SwitchOutcome::Unknown);
    rig.blocked = true;
    assert_eq!(rig.switch(a_id), SwitchOutcome::Blocked);
    rig.blocked = false;
    assert!(rig.quick_mask.is_some(), "a refused switch keeps the mode");
    assert!(rig.doc.selection.active().is_none());
    // The real switch: the mode ends and b (the outgoing one) gets the step.
    assert_eq!(rig.switch(a_id), SwitchOutcome::Switched);
    assert!(rig.quick_mask.is_none());
    assert!(
        rig.doc.selection.active().is_none(),
        "the incoming document's selection is untouched"
    );
    let outgoing = rig.parked(b_id);
    let applied = outgoing.selection.active();
    assert_eq!(
        applied.as_ref().map(|s| s.coverage_at(15, 15)),
        Some(0.0),
        "the grid became the outgoing document's selection"
    );
    assert_eq!(
        outgoing.undo_order.history_steps().last().map(|s| s.label),
        Some("Quick Mask")
    );
}

/// New and Open (`activate_new_document`) and the quit path
/// (`commit_live_gestures`) end the mode the same way, on the outgoing
/// document.
#[test]
fn opening_a_document_ends_quick_mask_on_the_outgoing_one() {
    let (dir_a, dir_b) = (tempdir(), tempdir());
    let (a, _) = session("a.png", 1, [1.0, 0.0, 0.0, 1.0], &dir_a);
    let (b, _) = session("b.psd", 1, [0.0, 0.0, 1.0, 1.0], &dir_b);
    let mut rig = Rig::new(a);
    let a_id = rig.doc.id;
    rig.quick_mask = Some(quick_mask_with_a_hole(&rig.doc));
    // A mask stroke in progress (no pixel snapshot) must not outlive the
    // mode.
    rig.drag = Some(Drag::Brush {
        hard: false,
        clone: None,
        last_doc: (0.0, 0.0),
        carry: 0.0,
        stroke: None,
        warned: HashSet::new(),
    });
    let _ = rig.open(b);
    assert!(rig.drag.is_none(), "the mask stroke was dropped");
    assert!(rig.quick_mask.is_none());
    assert!(rig.doc.selection.active().is_none());
    assert!(rig.parked(a_id).selection.active().is_some());
}

/// The decision lives in `commit_live_gestures`; `App`'s per-turn
/// `follow_document_tabs_now`, `activate_document` and `about_to_wait`
/// must not leave the mode themselves (the first version of 0.217.0 did, and
/// so could never stay on).
#[test]
fn no_per_turn_app_hook_leaves_quick_mask_unconditionally() {
    let source: String = include_str!("lib.rs")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for name in [
        "fn follow_document_tabs_now(&mut self) {",
        "fn activate_document(&mut self, raw: u64) {",
        "fn about_to_wait(&mut self",
        "fn redraw(&mut self) {",
        "fn sync_status_bar_now(&mut self) {",
    ] {
        let start = source
            .find(name)
            .unwrap_or_else(|| unreachable!("{name} exists"));
        let body = &source[start + name.len()..];
        // Up to the next method of the impl.
        let end = body.find(" fn ").unwrap_or(body.len());
        assert!(
            !body[..end].contains("leave_quick_mask"),
            "{name} leaves Quick Mask"
        );
    }
    // The only callers of the App-level leave are the toggle and a
    // closing active document.
    let calls = source.matches("self.leave_quick_mask(").count();
    assert_eq!(calls, 2, "toggle_quick_mask and finish_close only");
}
