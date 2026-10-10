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
    render_and_sample_pixel,
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
    let open = Modifiers {
        control: true,
        shift: true,
        ..Modifiers::none()
    };
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
