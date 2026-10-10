//! The one open document's own state (0.170.0, document tabs round R1).
//!
//! Everything here describes *a document*, not the window or the session:
//! its layers and their undo history, the view onto it, its selection,
//! what it is known to be missing, which composite tiles are current, the
//! active layer, and its own tile store (ADR 0010 — one store per
//! document, since layer ids restart at zero in every tree, a mask's
//! surface is its layer id with the top bit set, and the composite
//! surface is a fixed `u64::MAX`, so two documents could not share one).
//!
//! This round is a pure extraction: [`crate::App`] holds exactly one
//! session in `App::doc`, and every former `self.<field>` is now
//! `self.doc.<field>`, with no change in what any path does. Holding
//! more than one (parked sessions, switching, tabs) is a later round, as
//! is giving a session its own path, dirty state and autosave slot.
//!
//! What deliberately stays on `App`: the window, GPU and surface; the
//! open and autosave workers; the workspace, focus, shortcuts, palette
//! and dialog; the tool, colour and tool settings; the Layers-panel row
//! map (`layer_rows`) and scroll follow, which are *panel* state rebuilt
//! from a document; the live pointer drag; and the GPU residency atlas,
//! a cache over whichever document is drawn.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::{CompositeCache, UndoOrder};

/// A process-unique, monotonic identity for one [`DocumentSession`]
/// (0.170.0): assigned once at creation and never reused, so a later
/// round can name a document (a dialog's target, an autosave slot)
/// without an index that shifts when a tab closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DocumentId(u64);

impl DocumentId {
    /// The next id from one process-wide counter, starting at 1.
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The id's value — what names the session's autosave file
    /// (`crate::autosave_files::session_file_name`) and keys its slot in
    /// the autosave worker (0.171.0).
    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// The startup document's name (0.172.0).
pub(crate) const UNTITLED: &str = "Untitled";

/// The open documents that are not active (0.172.0, document tabs R3).
///
/// `App::doc` stays the active session, so every existing path that edits
/// "the document" still edits exactly one; the others wait here, intact, in
/// tab order. `active_position` is where the active session sits in the
/// full tab order (`0..=parked.len()`), so the full order is `parked` with
/// the active one inserted there. No indexing: every move is a checked
/// `position`/`remove`/`insert`.
#[derive(Default)]
pub(crate) struct DocumentShelf {
    pub(crate) parked: Vec<DocumentSession>,
    pub(crate) active_position: usize,
    /// The tab-strip selection the code itself last wrote (0.173.0 review
    /// D-1), `None` until a sync succeeds or after one failed: the strip
    /// follower switches only when the user moved the strip away from it.
    pub(crate) strip_selection: Option<usize>,
}

impl DocumentShelf {
    /// How many documents are open, the active one included.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.parked.len() + 1
    }

    /// Every open document's id in tab order, `active` at its position.
    pub(crate) fn tab_order(&self, active: &DocumentSession) -> Vec<DocumentId> {
        let mut order: Vec<DocumentId> = self.parked.iter().map(|session| session.id).collect();
        let position = self.active_position.min(order.len());
        order.insert(position, active.id);
        order
    }

    /// Whether a parked document has this id.
    pub(crate) fn holds(&self, id: DocumentId) -> bool {
        self.parked.iter().any(|session| session.id == id)
    }

    /// The id `step` tabs away from the active one, wrapping (`None` with
    /// one document open).
    pub(crate) fn cycled(&self, active: &DocumentSession, forward: bool) -> Option<DocumentId> {
        let order = self.tab_order(active);
        let count = order.len();
        if count < 2 {
            return None;
        }
        let here = order.iter().position(|&id| id == active.id)?;
        let next = if forward {
            (here + 1) % count
        } else {
            (here + count - 1) % count
        };
        order.get(next).copied()
    }

    /// Makes the parked `target` active and parks the outgoing `active` at
    /// its own tab position. `false` (nothing moved) when no parked session
    /// has that id.
    pub(crate) fn swap_in(&mut self, active: &mut DocumentSession, target: DocumentId) -> bool {
        let Some(index) = self.parked.iter().position(|session| session.id == target) else {
            return false;
        };
        let outgoing_position = self.active_position.min(self.parked.len());
        // The target's position in the full order.
        let target_position = if index < outgoing_position {
            index
        } else {
            index + 1
        };
        let incoming = self.parked.remove(index);
        let outgoing = std::mem::replace(active, incoming);
        // Back to where it was in the full order, which the target's
        // removal shifted by one when the target sat before it.
        let park_at = if target_position < outgoing_position {
            outgoing_position - 1
        } else {
            outgoing_position
        };
        self.parked.insert(park_at.min(self.parked.len()), outgoing);
        self.active_position = target_position;
        true
    }

    /// Makes a new session active, appended at the end of the tab order,
    /// and parks the outgoing one at its own position. Returns where the
    /// outgoing one was parked, for [`Self::revert_push`].
    pub(crate) fn push_active(
        &mut self,
        active: &mut DocumentSession,
        incoming: DocumentSession,
    ) -> usize {
        let outgoing = std::mem::replace(active, incoming);
        let park_at = self.active_position.min(self.parked.len());
        self.parked.insert(park_at, outgoing);
        self.active_position = self.parked.len();
        park_at
    }

    /// Undoes [`Self::push_active`] (an open that failed after its session
    /// was created): the session parked at `parked_at` is active again and
    /// the new one is handed back to be dropped. `None` (nothing moved)
    /// when `parked_at` names no parked session.
    pub(crate) fn revert_push(
        &mut self,
        active: &mut DocumentSession,
        parked_at: usize,
    ) -> Option<DocumentSession> {
        if parked_at >= self.parked.len() {
            return None;
        }
        let previous = self.parked.remove(parked_at);
        let abandoned = std::mem::replace(active, previous);
        self.active_position = parked_at;
        Some(abandoned)
    }

    /// Removes the parked session with this id (0.174.0, closing a
    /// document that is not active), keeping the active one's position
    /// in the tab order.
    pub(crate) fn remove(&mut self, id: DocumentId) -> Option<DocumentSession> {
        let index = self.parked.iter().position(|session| session.id == id)?;
        let removed = self.parked.remove(index);
        if index < self.active_position {
            self.active_position -= 1;
        }
        self.active_position = self.active_position.min(self.parked.len());
        Some(removed)
    }

    /// Closes the active session (0.174.0): the next tab becomes active,
    /// else the previous one, else `replacement` (the last document
    /// closed leaves a fresh one). Returns the closed session; `None`
    /// (nothing moved) for the last document with no replacement.
    pub(crate) fn close_active(
        &mut self,
        active: &mut DocumentSession,
        replacement: Option<DocumentSession>,
    ) -> Option<DocumentSession> {
        let position = self.active_position.min(self.parked.len());
        let incoming = if position < self.parked.len() {
            // The next tab sits at the active one's own position.
            self.parked.remove(position)
        } else if let Some(previous) = self.parked.pop() {
            self.active_position = self.parked.len();
            previous
        } else {
            self.active_position = 0;
            replacement?
        };
        Some(std::mem::replace(active, incoming))
    }

    /// The parked session with this id, to read (tests, palette names).
    pub(crate) fn get(&self, id: DocumentId) -> Option<&DocumentSession> {
        self.parked.iter().find(|session| session.id == id)
    }
}

/// What a session is built from: the document's own contents and the
/// three values derived from them before the first frame (see
/// [`DocumentSession::new`]).
pub(crate) struct DocumentContents {
    pub(crate) layers: aurora_doc::LayerTree,
    pub(crate) history: aurora_doc::History,
    pub(crate) canvas_size: (u32, u32),
    pub(crate) skipped_tiles: aurora_io::SkippedTiles,
    pub(crate) active_layer: Option<aurora_doc::LayerId>,
    pub(crate) canvas_view: aurora_ui::CanvasView,
    pub(crate) tile_store: Option<aurora_tile::TileStore>,
}

/// The open document (0.170.0) — see this module's own doc comment.
pub(crate) struct DocumentSession {
    /// This session's identity, fixed for its lifetime. Since 0.171.0 it
    /// names the session's autosave file and its autosave worker slot.
    /// An open replaces the session's contents in place and keeps the
    /// id, so the opened document's autosave lands on the same file by
    /// one atomic rename (the session is the slot, not the document).
    pub(crate) id: DocumentId,
    /// What the window title and the palette's "Document: `name`" entry
    /// call this document (0.172.0): the opened file's name, "Untitled"
    /// for the startup document, "Untitled N" for a New Document and
    /// "Recovered N" for a crash-recovered one. Not a path (R5).
    pub(crate) name: String,
    /// The [`crate::UndoOrder`] revision of this session's last park
    /// autosave and the worker generation of its *complete* write
    /// (0.173.0, R3 carry-over R-1, review-revised): a park at the same
    /// revision skips the snapshot only once the worker has landed that
    /// generation. A failed or partial snapshot, or a failed synchronous
    /// write, records `None`, so the next park retries.
    pub(crate) park_autosave: Option<(crate::UndoRevision, u64)>,
    /// The file this document came from or was last saved to as `.aur`
    /// (0.174.0): set by a successful open (any format) and by a
    /// successful `.aur` save, never by an export. **For display, not a
    /// save target**: Save always asks where with the native picker (it
    /// is "Save As…"), so no path here is ever written to without the
    /// user choosing it — Aurora writes no PSD, and an export is lossy.
    pub(crate) path: Option<std::path::PathBuf>,
    /// The [`crate::UndoOrder`] revision this document was last known to
    /// match a file at (0.174.0): set at creation (a New Document, the
    /// startup demo, an open before its install), re-set after a
    /// successful open's install and after a successful `.aur` save;
    /// `None` for a crash-recovered document, whose contents exist in no
    /// file the user chose. See [`Self::is_dirty`].
    pub(crate) saved_revision: Option<crate::UndoRevision>,
    /// The canvas pan/zoom transform ([`aurora_ui::CanvasView`]). Per
    /// document, the way Photoshop remembers each open document's own
    /// zoom and scroll independent of its pixel content.
    pub(crate) canvas_view: aurora_ui::CanvasView,
    /// The document-level selection ([`aurora_doc::SelectionSet`]) the
    /// Marquee Select tool drags out — a document-level concept in its
    /// own right (see that type's own doc comment), not something that
    /// needs a live `LayerTree` alongside it to exist.
    pub(crate) selection: aurora_doc::SelectionSet,
    /// The live document's own layer structure — built once in
    /// [`App::new`](crate::App::new) (from [`demo_document`](crate::demo_document) or a recovered autosave) and
    /// kept alive from then on. This is what [`Self::active_layer`]/
    /// [`aurora_doc::LayerTree::surface_id`] read to find somewhere for
    /// the Brush tool to actually paint.
    pub(crate) layers: aurora_doc::LayerTree,
    /// The document's own real, independent canvas size — `(width,
    /// height)`, in document-space pixels. **Not** derived from any
    /// one layer's own `bounds` on every read ([`document_canvas_size`](crate::document_canvas_size)
    /// used to be the only source, silently following whichever layer
    /// happened to be on top or shrinking to nothing if that layer was
    /// deleted or resized) — a real editor's canvas can be larger,
    /// smaller, or offset from any single layer it contains. Set once
    /// from [`document_canvas_size`](crate::document_canvas_size) for a document built without a
    /// real, independent canvas size of its own ([`demo_document`](crate::demo_document) —
    /// a recovered autosave used to be in that same boat, since the
    /// raw crash-recovery journal persisted no canvas size, but the
    /// autosave file is a real `.aur` container now and its manifest
    /// carries one), from a decoded image's own real dimensions
    /// ([`App::open_file`](crate::App::open_file)), or from a `.aur` file's own manifest
    /// (`aurora_io::read_aur`'s own third return value,
    /// [`App::open_aur_file`](crate::App::open_aur_file) and [`recover_document`](crate::recover_document)) — the one case
    /// this was actually wrong
    /// before: re-saving a `.aur` file whose real canvas size differed
    /// from its topmost layer's own bounds used to silently shrink (or
    /// grow) the canvas to match that layer instead of preserving it.
    pub(crate) canvas_size: (u32, u32),
    /// What this document is already known to be **missing** — the
    /// tiles some earlier best-effort write could not read
    /// (`aurora_io::SkippedTiles`).
    ///
    /// **State, not a one-shot notification** (0.74.1). It is populated
    /// from a `.aur` file's own `skipped-tiles` entry when one is opened
    /// ([`App::open_aur_file`](crate::App::open_aur_file)) and handed to *every* subsequent write
    /// of that document ([`write_autosave`](crate::write_autosave), [`App::save_aur_file`](crate::App::save_aur_file)),
    /// because nothing else can carry it: a tile a previous writer
    /// dropped is not in the tile store at all, so a later write walks
    /// past it as "never painted" and rediscovers nothing. 0.74.0 read
    /// the list, built one dialog line out of it and discarded it, so
    /// the very next save — including the autosave `open_aur_file`
    /// itself performs, before the warning has even appeared on
    /// screen — wrote a file that no longer recorded the loss at all.
    /// Open, Save, reopen, and the file said nothing; open and crash,
    /// and crash recovery restored a document with no record either.
    ///
    /// Reset to empty wherever the document is *replaced* by one with
    /// no such history ([`App::open_file`](crate::App::open_file)'s flat-image path), for the
    /// same reason `pixel_history` and `undo_order` are: it describes
    /// the document that is open, not the session.
    pub(crate) skipped_tiles: aurora_io::SkippedTiles,
    /// `layers`' own undo/redo history — built alongside it (same
    /// source: [`demo_document`](crate::demo_document) or a recovered autosave) and, since
    /// Undo/Redo (`Ctrl+Z`/`Ctrl+Shift+Z`, [`run_command`](crate::run_command)), also kept
    /// alive alongside it, not dropped after startup the way it used to
    /// be. `App::apply_move` is the one live-editing path that records
    /// through this — raw pixel edits (`App::paint_dab`/
    /// `App::erase_dab`) still bypass it entirely, since they have no
    /// `aurora_doc::LayerOp` equivalent to record; see
    /// [`Self::pixel_history`] for their own, separate undo instead.
    pub(crate) history: aurora_doc::History,
    /// Undo/redo for completed Brush/Eraser strokes
    /// (`aurora_brush::PixelHistory`) — the pixel-edit half `history`
    /// structurally can't cover (a stroke is raw pixel data, not a
    /// `LayerOp`). Still a separate stack internally (neither type knows
    /// about the other), but `Ctrl+Z`/`Ctrl+Shift+Z` walk it and
    /// `history` as one true chronological sequence via
    /// [`Self::undo_order`]. Populated by `App::handle_pointer_released`
    /// once a `Drag::Brush`/`Drag::Eraser`'s own accumulated
    /// `StrokeSnapshot` completes.
    pub(crate) pixel_history: aurora_brush::PixelHistory,
    /// The real interleaving order `Ctrl+Z`/`Ctrl+Shift+Z` walk across
    /// `history`'s own structural entries and `pixel_history`'s own
    /// stroke entries — see [`UndoOrder`]'s own doc comment for why this
    /// exists at all (`aurora-brush` and `aurora-doc` are sibling
    /// crates, neither depending on the other, so neither can know about
    /// the other's own activity). `App::apply_move`/
    /// `App::handle_pointer_released` record into it; `run_command`
    /// consults it to decide which backing store `Ctrl+Z`/
    /// `Ctrl+Shift+Z` should actually reach into next.
    pub(crate) undo_order: UndoOrder,
    /// Which composite tiles [`recomposite_visible_tiles`](crate::recomposite_visible_tiles) can skip
    /// recomputing this redraw — see [`CompositeCache`]'s own doc
    /// comment. Bumped by every operation that could change what a
    /// composite tile now shows.
    pub(crate) composite_cache: CompositeCache,
    /// The layer the Brush/Eraser tools paint/erase into and the Move
    /// tool repositions, if any — the topmost pixel layer of `layers`
    /// at construction time ([`topmost_pixel_layer`](crate::topmost_pixel_layer)), real-time-
    /// changeable now by clicking a row in the Layers panel
    /// ([`App::layer_rows`](crate::App::layer_rows), [`App::handle_pointer_pressed`](crate::App::handle_pointer_pressed)). `None`
    /// for a document with no pixel layer at all.
    ///
    /// **This can hold a group's `LayerId`, not just a pixel layer's.**
    /// `layer_rows` maps every row `populate_layers_panel` inserts,
    /// group rows included (`aurora_ui::layers_panel::insert_layer_row`
    /// inserts unconditionally), and [`select_layer`](crate::select_layer) sets `*active_layer`
    /// from whichever row was clicked with no `LayerKind` check. A caller
    /// reading this field for anything pixel-specific (paint/erase
    /// targets, the Move tool) must itself confirm the kind rather than
    /// assume it, the same way [`topmost_pixel_layer`](crate::topmost_pixel_layer) already does at
    /// construction time.
    ///
    /// **The canvas pan boundary is a function of this field, of this
    /// layer's own `bounds`, and of the canvas area's own size.** The
    /// bound ([`PanBounds`](crate::PanBounds)) is measured against the active layer's
    /// document-space origin ([`active_layer_origin`](crate::active_layer_origin), what
    /// [`canvas_local_origin`](crate::canvas_local_origin) subtracts) on the near edge, and against
    /// that origin plus the document ceiling
    /// ([`aurora_gpu::TileResidency::MAX_DOC_ORIGIN_PX`]) on the far
    /// one — so it moves when *any* of the three inputs moves, and a pan
    /// that never moved is then outside it, reopening the render/paint
    /// divergence the clamps exist to close. Writing this field is
    /// therefore only one third of what has to re-clamp.
    ///
    /// All three, and where each re-establishes the bound:
    ///
    /// - *Which layer is active.* [`App::new`](crate::App::new), [`App::open_file`](crate::App::open_file) and
    ///   [`App::open_aur_file`](crate::App::open_aur_file) set it and then build the view through
    ///   [`load_document_view`](crate::load_document_view), which clamps as part of the same step.
    ///   [`select_layer`](crate::select_layer) takes the view and clamps itself.
    /// - *That layer's own bounds.* [`App::apply_move`](crate::App::apply_move) rewrites them
    ///   live, per pointer-move event, and deliberately does **not**
    ///   clamp there (it would feed back into `continue_drag`'s own
    ///   fixed `start_doc` — see [`commit_ending_drag`](crate::commit_ending_drag)); the clamp
    ///   happens once, at the commit, in [`commit_ending_drag`](crate::commit_ending_drag)'s own
    ///   `Drag::Move` arm. [`App::run_undo_redo`](crate::App::run_undo_redo) can revert or reapply
    ///   a recorded bounds change without this field changing at all,
    ///   and clamps via [`perform_undo_redo`](crate::perform_undo_redo)'s own [`after_undo_redo`](crate::after_undo_redo)
    ///   step.
    /// - *The canvas area's own size* (0.57.10), which only the **far**
    ///   half of the bound depends on: the document position at the
    ///   canvas area's bottom-right corner moves when that area grows,
    ///   with neither this field nor any layer's `bounds` changing.
    ///   [`App::apply_resize`](crate::App::apply_resize) and [`App::redraw`](crate::App::redraw) both re-apply
    ///   [`PanBounds`](crate::PanBounds) through [`apply_canvas_min_zoom`](crate::apply_canvas_min_zoom), which is why
    ///   that function takes one at all. The near half is provably
    ///   unaffected by a resize — see its doc comment.
    ///
    /// A new writer of any of the three that skips the clamp is a bug
    /// with no visible symptom until someone paints.
    ///
    /// **And a clamp that runs while a drag is still live is its own
    /// bug** (0.57.7), in the opposite direction: the drag holds a
    /// document-space reference point fixed from the moment it began,
    /// so a view that moves under it makes the next pointer-move event
    /// measure against a view the drag knows nothing about — for a
    /// `Drag::Brush`, a line of dabs the user never drew. Every path
    /// that moves the view has to say which it does: end the drag first
    /// ([`press_layer_row`](crate::press_layer_row), [`perform_undo_redo`](crate::perform_undo_redo), and the Zoom-tool
    /// click branch of [`App::handle_pointer_pressed`](crate::App::handle_pointer_pressed), all through
    /// [`commit_ending_drag`](crate::commit_ending_drag)), or re-anchor it
    /// ([`shift_drag_reference`](crate::shift_drag_reference), for [`apply_scroll_zoom`](crate::apply_scroll_zoom) and
    /// [`apply_canvas_min_zoom`](crate::apply_canvas_min_zoom), where the gesture is not "I am done
    /// dragging").
    ///
    /// The second of those two was the rule's own first exception, and
    /// is worth knowing about as a shape rather than a one-off:
    /// `aurora_ui::CanvasView::set_min_zoom` moves the view without
    /// reading like it does (0.57.8), so `App::apply_resize`/
    /// `App::redraw` broke the rule for a whole round while stating it.
    /// A "setter" that ends up in `zoom_at` or `pan_by` is a path that
    /// moves the view.
    pub(crate) active_layer: Option<aurora_doc::LayerId>,
    /// This document's own shared tile store (ADR 0010) — `None` if it
    /// failed to open (e.g. an unwritable scratch directory), logged as
    /// a warning rather than treated as fatal, the same "must never stop
    /// the application starting" shape [`write_session_marker`](crate::write_session_marker) already
    /// uses for its own I/O. Painting is silently disabled for the
    /// session when this is `None`.
    pub(crate) tile_store: Option<aurora_tile::TileStore>,
}

impl DocumentSession {
    /// A session over `contents`, with a fresh id, an empty selection,
    /// empty pixel and interleaved undo stacks (the History panel's
    /// origin row reading "New Document", [`UndoOrder`]'s own
    /// `new_document`), and no composite tile current yet — exactly the
    /// values [`crate::App`]'s startup assigned field by field before
    /// 0.170.0.
    pub(crate) fn new(contents: DocumentContents) -> Self {
        Self::with_id(DocumentId::next(), contents)
    }

    /// Whether the document has changes no file holds (0.174.0): its
    /// revision differs from the one it was last saved or opened at, or
    /// it never matched a file at all (recovered). Every recorded, undone
    /// or redone step takes a fresh revision, so this is conservative:
    /// undoing back past a save is dirty, and so is undoing and redoing
    /// back *to* it (the revision is new, never the saved one again).
    /// Live gestures not yet committed are not counted; every close and
    /// quit path commits them first.
    pub(crate) fn is_dirty(&self) -> bool {
        self.saved_revision != Some(self.undo_order.revision)
    }

    /// Records that the document now matches a file (an open's install,
    /// a successful `.aur` save).
    pub(crate) fn mark_clean(&mut self) {
        self.saved_revision = Some(self.undo_order.revision);
    }

    /// [`Self::new`] with an id taken earlier — `App::new` needs it to
    /// name the startup autosave before the session exists (0.171.0).
    pub(crate) fn with_id(id: DocumentId, contents: DocumentContents) -> Self {
        let DocumentContents {
            layers,
            history,
            canvas_size,
            skipped_tiles,
            active_layer,
            canvas_view,
            tile_store,
        } = contents;
        let undo_order = UndoOrder::new_document();
        Self {
            id,
            name: UNTITLED.to_owned(),
            path: None,
            saved_revision: Some(undo_order.revision),
            park_autosave: None,
            canvas_view,
            selection: aurora_doc::SelectionSet::new(),
            layers,
            canvas_size,
            skipped_tiles,
            history,
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order,
            composite_cache: CompositeCache::default(),
            active_layer,
            tile_store,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentContents, DocumentId, DocumentSession};
    use crate::{
        NEW_DOCUMENT_HISTORY_ORIGIN, StartupPanels, UndoOrder, demo_document, document_canvas_size,
        install_startup_panels, load_document_view, load_scales, startup_document,
        topmost_pixel_layer,
    };

    #[test]
    fn document_ids_are_unique_and_increase_in_creation_order() {
        let ids: Vec<DocumentId> = (0..64).map(|_| DocumentId::next()).collect();
        for pair in ids.windows(2) {
            if let [earlier, later] = pair {
                assert!(earlier < later, "{earlier:?} then {later:?}");
            }
        }
        let unique: std::collections::HashSet<DocumentId> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "no id is handed out twice");
    }

    /// The startup document as `App::new` builds it, minus the window:
    /// a fresh (no crash marker) startup with no tile store, through the
    /// same `startup_document`, `install_startup_panels` and
    /// `load_document_view` calls, into [`DocumentSession::new`].
    fn startup_session() -> DocumentSession {
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        let mut workspace = aurora_ui::build_workspace(&scales);
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let mut tile_store = None;
        let startup = startup_document(false, &dir.path().join("none.aur"), &mut tile_store);
        assert!(!startup.was_recovered);
        let StartupPanels { active_layer, .. } = install_startup_panels(
            &mut workspace,
            &scales,
            &startup.layers,
            &UndoOrder::new_document(),
            aurora_ui::Tool::default(),
            &crate::ToolSettings::default(),
        );
        let canvas_view = load_document_view(
            &aurora_ui::CanvasView::default(),
            &startup.layers,
            active_layer,
            None,
            None,
            1.0,
        );
        DocumentSession::new(DocumentContents {
            layers: startup.layers,
            history: startup.history,
            canvas_size: startup.canvas_size,
            skipped_tiles: startup.skipped_tiles,
            active_layer,
            canvas_view,
            tile_store,
        })
    }

    #[test]
    fn the_startup_session_holds_what_the_old_startup_fields_held() {
        let session = startup_session();
        let (demo_layers, demo_history) = demo_document();
        // The document itself: the demo document, unchanged.
        assert_eq!(session.layers.len(), demo_layers.len());
        assert_eq!(session.canvas_size, document_canvas_size(&demo_layers));
        assert_eq!(
            session.history.journal_descriptions(),
            demo_history.journal_descriptions()
        );
        // Not an empty `History`: the demo document's own building steps
        // sit on its undo stack, exactly as before 0.170.0. What the user
        // can reach is empty -- `undo_order` below names no step, so
        // Ctrl+Z walks nothing and the History panel shows only its
        // origin row.
        assert_eq!(session.history.can_undo(), demo_history.can_undo());
        assert_eq!(session.history.can_redo(), demo_history.can_redo());
        assert_eq!(session.history.journal_len(), demo_history.journal_len());
        assert!(session.skipped_tiles.is_empty());
        // The active layer and the view: the topmost pixel layer, at the
        // same unzoomed view `App::new` built.
        assert_eq!(session.active_layer, topmost_pixel_layer(&demo_layers));
        assert!(session.active_layer.is_some());
        assert_eq!(
            session.canvas_view,
            load_document_view(
                &aurora_ui::CanvasView::default(),
                &demo_layers,
                session.active_layer,
                None,
                None,
                1.0,
            )
        );
        // The per-session state `App::new` used to assign field by field.
        assert!(session.selection.active().is_none(), "no selection");
        assert!(!session.pixel_history.can_undo());
        assert!(!session.pixel_history.can_redo());
        assert!(session.undo_order.undo.is_empty());
        assert!(session.undo_order.redo.is_empty());
        assert_eq!(
            session.undo_order.origin, NEW_DOCUMENT_HISTORY_ORIGIN,
            "the History panel's origin row reads \"New Document\", not \"Open\""
        );
        assert!(
            session.composite_cache.current.is_empty(),
            "no composite tile is current before the first frame"
        );
        assert!(session.tile_store.is_none(), "no store was opened here");
    }

    fn named(name: &str) -> DocumentSession {
        let mut session = DocumentSession::new(DocumentContents {
            layers: aurora_doc::LayerTree::new(),
            history: aurora_doc::History::new(),
            canvas_size: (1, 1),
            skipped_tiles: aurora_io::SkippedTiles::new(),
            active_layer: None,
            canvas_view: aurora_ui::CanvasView::default(),
            tile_store: None,
        });
        name.clone_into(&mut session.name);
        session
    }

    fn names_in_order(shelf: &super::DocumentShelf, active: &DocumentSession) -> Vec<String> {
        shelf
            .tab_order(active)
            .into_iter()
            .map(|id| {
                if id == active.id {
                    format!("*{}", active.name)
                } else {
                    shelf
                        .get(id)
                        .map_or_else(String::new, |session| session.name.clone())
                }
            })
            .collect()
    }

    /// 0.172.0: pushing, swapping and reverting keep the tab order, and
    /// every document is held exactly once.
    #[test]
    fn the_shelf_keeps_tab_order_through_push_swap_and_revert() {
        let mut shelf = super::DocumentShelf::default();
        let mut active = named("a");
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf.cycled(&active, true), None);
        let _ = shelf.push_active(&mut active, named("b"));
        let parked_c = shelf.push_active(&mut active, named("c"));
        assert_eq!(names_in_order(&shelf, &active), ["a", "b", "*c"]);
        let a = shelf.tab_order(&active).first().copied();
        let Some(a) = a else {
            unreachable!("three documents are open")
        };
        assert!(shelf.swap_in(&mut active, a));
        assert_eq!(names_in_order(&shelf, &active), ["*a", "b", "c"]);
        assert_eq!(
            shelf
                .cycled(&active, false)
                .and_then(|id| shelf.get(id))
                .map(|s| s.name.as_str()),
            Some("c")
        );
        let Some(b) = shelf.cycled(&active, true) else {
            unreachable!("b follows a")
        };
        assert!(shelf.swap_in(&mut active, b));
        assert_eq!(names_in_order(&shelf, &active), ["a", "*b", "c"]);
        let own = active.id;
        assert!(
            !shelf.swap_in(&mut active, own),
            "the active one is not parked"
        );
        // A push from the middle appends and parks the outgoing in place;
        // its revert restores exactly the previous arrangement.
        let parked_at = shelf.push_active(&mut active, named("d"));
        assert_eq!(names_in_order(&shelf, &active), ["a", "b", "c", "*d"]);
        let abandoned = shelf.revert_push(&mut active, parked_at);
        assert_eq!(abandoned.map(|s| s.name), Some("d".to_owned()));
        assert_eq!(names_in_order(&shelf, &active), ["a", "*b", "c"]);
        assert!(shelf.revert_push(&mut active, 99).is_none());
        let _ = parked_c;
    }

    #[test]
    fn two_sessions_get_different_ids() {
        let first = startup_session();
        let second = startup_session();
        assert_ne!(first.id, second.id);
        assert!(first.id < second.id);
    }
}
