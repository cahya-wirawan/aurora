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
