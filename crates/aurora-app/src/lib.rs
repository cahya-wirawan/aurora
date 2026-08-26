//! Application shell: window/event-loop lifecycle, wired to a real
//! `accesskit_winit` accessibility adapter and `aurora-gpu`'s
//! already-proven presentation surface. PLAN.md M1.8's first
//! deliverable.
//!
//! **Scope, stated honestly.** This is the "create hidden → attach
//! adapter → show" ordering ADR 0001's escape-hatch check found
//! (`spike/a11y-ime/FINDINGS.md` finding #1) as real, production code —
//! not yet the actual application. The accessibility tree is
//! `aurora_ui::build_workspace`'s real (but still static — no
//! drag-to-redock, resize, or persisted layouts yet) canvas-area +
//! docked-panel structure, matching the owner-approved workspace
//! mockup, with real Layers/History panel content
//! (`aurora_ui::populate_layers_panel`/`populate_history_panel`) built
//! from a small, clearly-fake demo document; the window's background is
//! a real theme token (`design/themes/dark.toml`'s `surface.app`, the
//! only built-in theme that exists as a real design yet). Still nothing
//! renders visually beyond the background clear — the canvas itself and
//! IME are this milestone's other, separate, still-open bullets.
//!
//! **Native menu bar, macOS only**: `muda`, scoped to macOS because PRD
//! §8.3/§14 only name macOS for the native menu bar, and because neither
//! Windows nor Linux is actually a good fit right now — see the "native
//! menu bar" section further down for the full reasoning (Windows needs
//! its own real `unsafe`-code decision; Linux's only muda backend needs
//! a real `gtk::Window`, which a plain `winit` window structurally never
//! is). `build_menu`/`activate_command` are cross-platform, real logic;
//! only the attachment (`Menu::init_for_nsapp` in `resumed`) and event
//! polling (`about_to_wait`) are behind `#[cfg(target_os = "macos")]`.
//! The menu reuses the exact same `COMMAND_*` ids the command palette
//! does, through the same `activate_command` — one underlying action,
//! two UI surfaces. `activate_command` itself is unit-tested;
//! `build_menu` is not — real macOS CI found `muda::Menu::new()` panics
//! off the main thread, which every `#[test]` in this workspace's
//! harness runs on by construction (see that function's own test-module
//! note), so it's exercised only by actually running the app.
//!
//! **System clipboard, native file dialogs, and drag & drop**:
//! `rfd`/`arboard`, PRD §8.3's own pre-decided choices, plus `winit`'s
//! own native `WindowEvent::DroppedFile` (no extra dependency needed).
//! The command palette's `Ctrl+C`/`Ctrl+V` read and write the real
//! system clipboard, and its "Open File…"/"Save As…" entries show a
//! real, native `rfd::FileDialog`; a real drag-and-dropped file is
//! opened the same way "Open File…" is. Both `arboard`/`rfd` are real
//! platform calls with no meaningful headless behaviour, so
//! `handle_palette_key` takes them as
//! `&mut dyn ClipboardAccess`/`&mut dyn FileDialogAccess` rather than
//! calling them directly — the same "keep the pure dispatch logic
//! testable, isolate the untestable platform call" seam `translate_key`/
//! `translate_modifiers` already use for keyboard input. **A file
//! chosen via "Open File…" *or* dropped onto the window is really
//! opened** (`App::open_file` — the same method either route calls,
//! since both are the same "the user wants to open this" signal):
//! `aurora_io::decode_by_extension` decodes it (PNG/JPEG/TIFF), and the
//! current document is replaced by a fresh, single-layer one sized to
//! the image, its own pixels written into the live tile store via
//! `aurora_io::write_into_store` so the canvas actually shows it. A bad
//! file (unreadable, undecodable, unrecognised extension) is logged and
//! leaves the current document untouched. **"Save As…" is the reverse**
//! (`App::save_file`): every visible pixel layer is composited across
//! the real document extent (`composite_document`, reusing
//! `recomposite_visible_tiles`'s own per-tile, per-layer-blend-mode-aware
//! and moved-layer origin-conversion logic against the whole
//! `self.canvas_size` rect rather than just the on-screen viewport),
//! encoded by the chosen path's own extension
//! (`aurora_io::encode_by_extension`), and written to disk via
//! `write_verified` — a sibling temp file, verified by reading it back
//! and decoding it, then renamed over the real destination, so a failed
//! export never corrupts or overwrites whatever was already there. This
//! is the same real multi-layer composite the canvas itself already
//! shows (`App::redraw`), not just the active layer's own pixels — the
//! bug this path used to have. Each layer's own real `blend_mode` is
//! read and translated (`translate_blend_mode`) into
//! `aurora_render::BlendMode` — `Normal`, the 8-mode "simple
//! separable" family (`Darken`/`Multiply`/`Lighten`/`Screen`/
//! `Difference`/`Exclusion`/`Subtract`/`Divide`), the 4-mode
//! "dodge and burn" family (`ColorDodge`/`LinearDodge`/`ColorBurn`/
//! `LinearBurn`), the 7-mode "overlay and light" family
//! (`Overlay`/`SoftLight`/`HardLight`/`VividLight`/`LinearLight`/
//! `PinLight`/`HardMix`), the 4-mode non-separable HSL family
//! (`Hue`/`Saturation`/`Color`/`Luminosity`), and the 2-mode
//! whole-colour-selection family (`DarkerColor`/`LighterColor`) are
//! real; the one remaining `aurora_doc::BlendMode` variant (`Dissolve`)
//! is real too, but not via this translation — `resolve_tile` intercepts
//! it before `translate_blend_mode` ever runs and applies its own
//! stochastic `dissolve_gate` instead, since Dissolve's per-pixel
//! keep-or-drop decision isn't expressible as a per-pixel-colour blend
//! function the way every other mode is (see `resolve_tile`'s own doc
//! comment for the full account). Layer groups recurse at any depth
//! `aurora-doc` will accept (bounded, as its own second line of
//! defence, by `aurora_doc::MAX_LAYER_TREE_DEPTH` and a per-tile node
//! budget — see `resolve_tile`'s own doc comment) and are
//! ancestor-visibility-gated (an invisible group hides its whole
//! subtree), so a layer nested inside a group does composite and export
//! now; a group's own `opacity`/`blend_mode` **are** now aggregated
//! into its children's effective compositing too, via `resolve_tile`'s
//! shared recursion (`recomposite_visible_tiles`/`composite_document`
//! both call it once per `LayerTree::roots` entry): every group
//! composites its own visible direct children in isolation, then that
//! isolated result is composited one level up using the group's own
//! opacity/blend mode — the only semantic `aurora_doc::BlendMode`'s
//! 27 variants can actually express, since the schema has no "Pass
//! Through" mode to model Photoshop's own isolated-vs-pass-through
//! distinction with. Real for the common cases — a single child of any
//! opacity, or multiple children combining via `Normal` — after
//! `resolve_tile`'s own un-premultiply fix; a group whose *own*
//! children combine via a non-`Normal` blend mode against each other
//! while the isolation buffer is still translucent partway through
//! remains a real, narrower, still-open gap — see `resolve_tile`'s own
//! doc comment for the precise boundary. **A `.aur`
//! path (ADR 0009) takes a different, real route
//! through both**: `App::open_aur_file`/`App::save_aur_file` call
//! `aurora_io::read_aur`/`write_aur` directly — a real, possibly
//! multi-layer document, not a single flat image, verified on save the
//! same `write_verified`-style way (`verify_aur`, reading the temp file
//! back with a throwaway `aurora_tile::TileStore`) since `.aur` has no
//! single "does this decode to the right width/height" check the way a
//! flat image does. `App::open_file`/`save_file` dispatch to the `.aur`
//! path or the flat-image path by extension (`is_aur_path`). `.aur`
//! saves every real pixel layer's own tiles plus real history/layer
//! metadata the flat-image path has no format to carry — and, since
//! Undo/Redo (below) gave `App` a real, kept-alive `history` field,
//! `history` written is that real journal, not the fresh empty one this
//! path used to write; it's still partial, since `Brush`/`Eraser` don't
//! record through it (see the Undo/Redo paragraph below).
//!
//! **DPI/scale-factor aware layout**: `logical_size` divides a real
//! physical window size by `Window::scale_factor` before it reaches
//! `WidgetTree::compute_layout` — every widget's own layout style is
//! defined in logical, DPI-independent units
//! (`aurora_theme::Scales`-derived), so feeding it raw physical pixels
//! would make widgets the wrong on-screen size on any display where
//! `scale_factor != 1.0`. Kept current via
//! `WindowEvent::ScaleFactorChanged` (e.g. the window moves to a
//! monitor with a different DPI), which is the same mechanism a genuine
//! multi-monitor, mixed-DPI setup relies on — real and exercised for a
//! single monitor's scale factor changing, though not yet verified on
//! actual multi-monitor hardware.
//!
//! **Real keyboard input, for the first time in this crate**: a fixed
//! set of global shortcuts (`default_shortcuts` — `Tab`/`Shift+Tab` for
//! `aurora_widgets::FocusManager` navigation, `Ctrl+Shift+P` to open a
//! real command palette), and the palette itself
//! (`aurora_widgets::widgets::command_palette`) captures the keyboard
//! while open to filter-as-you-type and `Enter`/`Escape`/arrow-key
//! navigate its results. Every bit of the dispatch logic (`handle_key`
//! and everything it calls) is deliberately free of `winit`'s own event-
//! loop/window types so it's headlessly testable in this sandbox (no
//! display server — see below); only `translate_key`/
//! `translate_modifiers` touch real `winit::keyboard` types, and those
//! are plain data, constructible with no window either.
//!
//! **Crash recovery and autosave** (PLAN.md M1.9): `run` writes a small
//! marker file (`std::env::temp_dir()`) at startup and clears it on a
//! clean `WindowEvent::CloseRequested` shutdown; if a *previous* run's
//! marker is still there, this run shows a real, modal
//! `Role::AlertDialog` (`aurora_widgets::widgets::dialog`) saying so.
//! Document recovery is now real, not just detected: `aurora-doc`'s
//! `History::save_journal`/`load_journal` (ADR 0009) give this crate an
//! on-disk journal encoding to write and read, so `App::new` writes the
//! current document to a second file (`std::env::temp_dir()` again) at
//! startup, and — if a previous run's marker is there *and* that
//! autosave file parses — opens with the recovered document instead of
//! the fake demo one (in which case there is nothing to write back:
//! the file already holds exactly that document).
//!
//! **That autosave file is a real `.aur` container now**, not the raw
//! `postcard` journal it started as: `write_autosave` builds it with
//! `aurora_io::write_aur` (mimetype sentinel, manifest carrying the
//! whole `LayerTree` and the document's own canvas size, the history
//! journal, and one entry per non-blank tile) and `recover_document`
//! reads it back with `aurora_io::read_aur` straight into the live
//! `aurora_tile::TileStore`. So a crash now recovers real painted
//! pixels and the real canvas size, where before it recovered only
//! structural `LayerOp`s and lost every painted pixel.
//!
//! **Scope, stated honestly**: still just one dialog action ("Continue"
//! — its message changes depending on whether recovery actually
//! happened), because recovery itself is unconditional and automatic
//! rather than a user choice. And autosave now happens **only at
//! lifecycle boundaries** — a fresh session's startup document, and
//! each document replacement (`App::open_file`/`App::open_aur_file`).
//! Live per-edit re-triggering was removed in 0.49.0: building a `.aur`
//! container walks the whole tile grid, which measured 687 ms on a
//! modest document and has to run on the thread that owns the tile
//! store (the UI thread), so re-triggering it from stroke commit —
//! even rate-gated — was a real violation of §7.3.4 against a
//! 10 ms brush budget. A crash therefore recovers the document as of
//! the last such boundary, with its real pixels; mid-session edits
//! since then are not autosaved at all. See this module's own "crash
//! recovery" section for the full reasoning and what would lift the
//! restriction.
//!
//! **Basic tools, brush painting, and eraser** (PLAN.md M1.9): this
//! crate's first pointer input at all
//! (`CursorMoved`/`MouseInput`/`MouseWheel`) drives `aurora_ui::Tool`'s
//! seven variants — Zoom (click and scroll-wheel), Pan (drag), and
//! Marquee Select (drag, into a real `aurora_doc::SelectionSet`) are
//! fully wired; Brush is too, as of the same milestone's "wire a live
//! document" step: `App` now keeps its own `LayerTree` alive (previously
//! built, used to populate the panels, and discarded every run) plus a
//! real `aurora_tile::TileStore` (ADR 0010), and a Brush drag calls
//! `aurora_brush::stamp_dab`/`advance_segment` against the active pixel
//! layer's own surface — a real mouse drag really paints real pixels
//! into a real, live document for the first time in this project.
//! **Eraser followed the same day**: the same drag/dab-spacing
//! machinery, a new `Drag::Eraser` variant alongside `Drag::Brush`, and
//! `aurora_brush::erase_dab` (subtractive — reduces existing alpha
//! instead of blending a colour) in place of `stamp_dab`; bound to `e`,
//! matching `b` for Brush.
//! **Active-layer selection followed the brush milestone**:
//! `aurora_ui::layers_panel`'s own rows are now real, non-zero-sized,
//! clickable widgets (`aurora_widgets::WidgetTree::hit_test`, new for
//! this), so clicking one calls `select_layer` — updates `active_layer`
//! (what Brush/Eraser paint/erase into) and marks the row accessibly
//! selected, both instead of always targeting the topmost pixel layer
//! with no way to change it. **Move followed later the same week**,
//! once `aurora_doc::LayerTree::set_bounds` gave the document model
//! somewhere to actually put a reposition: a new `Drag::Move` tracks
//! the active layer's own bounds at drag-start and shifts them by the
//! pointer's own travelled delta each move event, applied via
//! `App::apply_move`. Making a moved layer actually *render* in its new
//! place needed one more real fix: `canvas_local_origin` used to
//! assume the active layer always sat at document `(0, 0)` (true of
//! every layer built until Move existed) — it now subtracts the active
//! layer's own bounds offset before converting a document-space point
//! into a surface-local tile, the same conversion `layer_local_point`
//! already does for painting. **Eyedropper finished the same week**:
//! `sample_pixel` reads one texel straight out of the active layer's
//! own tile store surface (`TileStore::get`, no interpolation), and
//! `App::sample_eyedropper` sets it as `current_colour` — what `Brush`
//! now actually paints with, replacing what used to be a fixed
//! constant — as long as the sampled texel is actually painted (alpha
//! `> 0.0`; a fully transparent one, painted-then-erased or never
//! touched, has nothing meaningful to pick). Every M1.9 "basic tools"
//! variant is real now. See `aurora_ui::tool`'s own doc comment and this
//! module's "brush painting"/"layer selection" sections for the full
//! reasoning.
//!
//! **Eyedropper corrected to sample the composite, 2026-08-13**: the
//! paragraph above described the eyedropper's *original* behaviour, and
//! it was wrong — it read one texel straight out of the *active layer's
//! own* tile store surface, so a different, non-active visible layer
//! sitting above it (any opacity/blend mode), or an active layer that
//! was simply transparent at the clicked point, made it pick up the
//! wrong colour: not what the user was actually looking at and clicking
//! on. `App::sample_eyedropper` now reads `composite_surface_id()`
//! instead — the same reserved surface `App::redraw`'s own
//! `recomposite_visible_tiles` keeps current with the real, merged,
//! bottom-to-top blended document every frame — via the same
//! document-space -> surface-local conversion, since
//! `recomposite_visible_tiles`'s own `reference_origin` and
//! `active_layer_origin` share the identical active-layer-bounds-or-
//! `(0, 0)` fallback. That shared fallback also means the eyedropper no
//! longer requires an active layer at all: with none selected, it now
//! samples the merged document directly at `doc_point`, matching
//! `Drag::Eyedropper` itself, which never had that precondition.
//!
//! **Undo/Redo** (PLAN.md's Undo/Redo bullet): `App` now keeps a live
//! `history: aurora_doc::History` alongside `layers` (previously built
//! once in `App::new`, used only to populate the History panel and
//! write the autosave, then dropped) — `Ctrl+Z`/`Ctrl+Shift+Z`
//! (`AppCommand::Undo`/`Redo`, `run_command`) call `History::undo`/
//! `redo` against it directly and refresh the History panel
//! (`refresh_history_panel`) to show the result, since `History`'s own
//! doc comment already establishes that undoing/redoing is itself a
//! journaled step. `App::apply_move` now records through `history`
//! instead of calling `LayerTree::set_bounds` directly, so a completed
//! Move is really undoable — one undo step per pointer-move event
//! during the drag, not one per whole drag gesture at the time (see the
//! "Move-drag coalescing" paragraph further down for how that changed).
//! **Scope, stated honestly (at the time)**: `Undo`/`Redo` were
//! shortcut-only when this bullet first landed — see the "command
//! palette/native menu Undo/Redo" paragraph further down for how that
//! closed the same week.
//!
//! **Pixel-edit undo, 2026-08-06** — the gap the paragraph above named
//! (`App::paint_dab`/`Self::erase_dab` bypassing `History` entirely,
//! since a stroke has no `LayerOp` equivalent to record) is closed, via
//! `aurora_brush::StrokeSnapshot`/`PixelHistory` rather than extending
//! `History` itself: a stroke's pixel diff (the tiles it touched, their
//! before/after content — invariant §7.3.3's own "dirtied tiles"
//! wording, applied to raw pixel data instead of a layer's own scalar
//! properties) has no home in `aurora_doc::LayerOp`, and `aurora-brush`
//! can't depend on `aurora-doc` to add one anyway (PRD §7.2's own
//! layering). `Self::paint_dab`/`Self::erase_dab` hand the active
//! `Drag::Brush`/`Drag::Eraser`'s own `stroke` field to
//! `aurora_brush::stamp_dab`/`erase_dab`, which captures each tile as
//! it acquires it (since 0.55.0 — before that, `App` captured every
//! tile `aurora_brush::touched_tiles` listed *before* stamping, so a
//! dab whose paint then failed still left a real but useless undo
//! entry); `Self::handle_pointer_released` pushes the completed stroke
//! onto `Self::pixel_history` once the drag ends. `Ctrl+Z`/
//! `Ctrl+Shift+Z` (`run_command`) checked `pixel_history` first and
//! fell back to `history` at the time — a real, useful default, but not
//! the actual chronological order; see the "unified undo/redo"
//! paragraph below for how that was later replaced with the real thing.
//!
//! **Command palette and native menu Undo/Redo, also 2026-08-06**:
//! `Ctrl+Z`/`Ctrl+Shift+Z` were the only way to reach either command
//! until now — a real, named accessibility/discoverability gap (a
//! screen-reader user driving this crate through the palette had no
//! way to trigger either one). `ChosenFile` (the enum
//! `activate_command` returns for whatever it can't finish itself)
//! is renamed `ActivatedCommand` and gained `Undo`/`Redo` variants
//! alongside its existing `OpenFile`/`SaveFile`; `activate_command`
//! itself stays free of `layers`/`history`/`pixel_history`/the tile
//! store (resolving `COMMAND_UNDO`/`COMMAND_REDO` to their own bare
//! variant, nothing more), so `App::handle_key_event`/
//! `App::handle_menu_event` run the real command via a new
//! `App::run_undo_redo` — the same `run_command` path `Ctrl+Z`/
//! `Ctrl+Shift+Z` themselves already use, so there's exactly one place
//! either command's own logic lives. The native menu (macOS only)
//! gained an Edit submenu; deliberately no accelerator hint on its own
//! Undo/Redo items, since this crate's shortcuts bind literal `Ctrl+Z`
//! even on macOS and showing a `⌘Z` hint the app doesn't actually
//! respond to would be misleading.
//!
//! **Unified undo/redo, also 2026-08-06** — closes the gap the
//! Pixel-edit undo paragraph above named: `Ctrl+Z`/`Ctrl+Shift+Z` now
//! walk `history`'s structural entries and `pixel_history`'s stroke
//! entries as one true chronological sequence, not "pixel first, then
//! fall back." `UndoOrder`, a new small type, is the mechanism — it
//! doesn't hold either kind of edit itself (`aurora_doc::LayerOp` stays
//! private to that crate; a `StrokeSnapshot` stays owned by
//! `pixel_history`), only a `Vec<UndoKind>` tagging *which* backing
//! store's own top entry is actually next, in the order edits were
//! really committed. `Self::apply_move`/`Self::handle_pointer_released`
//! record into it (via `UndoOrder::record`, which also clears both
//! backing stores' own redo stacks — `history.clear_redo`/
//! `pixel_history.clear_redo`, both new — so a pixel edit correctly
//! invalidates a pending structural redo and vice versa, something two
//! fully independent stacks couldn't do for each other before);
//! `run_command`'s `Undo`/`Redo` arms consult `UndoOrder` first to find
//! out which store to actually call. Opening a new document
//! (`Self::open_file`/`Self::open_aur_file`) now resets
//! `pixel_history`/`undo_order` alongside `history` — a real, related
//! bug fixed as a side effect: previously `pixel_history` outlived a
//! document switch untouched, so `Ctrl+Z` could reach into a stroke
//! from a document that was no longer open.
//!
//! **Move-drag coalescing, also 2026-08-06** — closes the other gap the
//! Pixel-edit undo paragraph above named: dragging a layer used to
//! record one `history` entry per pointer-move event, so `Ctrl+Z` after
//! a single drag undid it one tiny step at a time instead of returning
//! the layer to where the drag started. `Self::apply_move` now bypasses
//! `history`/`undo_order` entirely and calls `LayerTree::set_bounds`
//! directly, purely for live visual feedback while the pointer is still
//! down; the actual undo entry is recorded once, retroactively, by a new
//! `finish_move`, called (from 0.57.0) through `commit_ending_drag` on
//! every path that ends a drag rather than only from
//! `Self::handle_pointer_released`, via
//! `aurora_doc::History::record_bounds_change` (the
//! start bounds captured when the drag began, the tree's own current
//! bounds as the end point) and `UndoOrder::record`, the same coalescing
//! shape `Self::handle_pointer_released` already used for a completed
//! Brush/Eraser stroke. A drag that ends back where it started (a click
//! with no real movement, or a pointer-up right after pointer-down)
//! records nothing — `finish_move` checks the layer's current bounds
//! against `start_bounds` first.
//!
//! **Per-layer-origin-aware compositing, also 2026-08-06** — closes the
//! "same-origin assumption" gap Multi-layer compositing named the same
//! day it landed: `recomposite_visible_tiles` used to read the *same*
//! `TileId` from every visible layer's own surface, correct only when
//! every layer shared the active layer's own document-space origin
//! (true of every document this crate's UI could build before Move-drag
//! coalescing above, since nothing had actually moved a second layer
//! away from another's origin yet). A new `read_layer_window` is the
//! real fix: given a document-space tile position and a specific
//! layer's own origin, it converts back into that layer's own local
//! space and, when the two origins aren't a whole number of tiles
//! apart, blends together whichever of that layer's own tiles overlap
//! the result — up to four, the same "one dab can span four tiles"
//! shape `aurora_brush::stamp::touched_tiles` already has for a much
//! smaller window. A layer that *does* share the active layer's own
//! origin still takes the cheap direct-read path — the common case
//! costs nothing extra. Any part of the window before a layer's own
//! local `(0, 0)` reads as transparent, the same as a pixel that layer
//! genuinely never painted.
//!
//! **Incremental compositing, also 2026-08-06** — the other named
//! Multi-layer compositing gap: `recomposite_visible_tiles` recomputed
//! the entire visible grid unconditionally on every redraw, even one
//! with nothing to do (a pure UI interaction, an idle repaint) or one
//! looking at territory already composited under the current document
//! state (panning back over it). New `CompositeCache` tracks which
//! composite `TileId`s are already known current and lets
//! `recomposite_visible_tiles` skip recomputing them; `Self::bump`
//! invalidates the whole cache at once, called from every place that
//! could change what a `TileId` now composites to (`Self::paint_dab`/
//! `Self::erase_dab`, `Self::apply_move`, selecting a different active
//! layer, Undo/Redo, opening or replacing the document). Coarse,
//! stated honestly: one bump invalidates every cached tile, not just
//! the one(s) an edit actually touched, so painting still recomposites
//! the whole visible grid each redraw the way it always did — the real
//! win is every redraw that isn't an edit at all. `aurora_tile::TileStore`'s
//! own per-tile dirty flags were deliberately *not* reused for finer
//! granularity: they only track resident tiles, so a tile dirtied then
//! evicted before a redraw ever consumes its flag would silently stop
//! reporting dirty at all — a real correctness risk this coarser,
//! explicitly-triggered design avoids entirely. True per-tile dirty
//! tracking across layers remains separate, still-open follow-on work,
//! and so does GPU-side compositing (`aurora_render::TileCompositor`
//! already exists for it, just not wired in here yet).
//!
//! **Document-level canvas size and real ICC round-trip, also
//! 2026-08-06**: two gaps the `.aur` bullet had named since it landed.
//! `App::canvas_size` is a new, real, independent field — until now,
//! `.aur` saves re-derived canvas size from the topmost pixel layer's
//! own bounds on *every* save (`document_canvas_size`), which is wrong
//! for a real editor: opening a `.aur` file whose stored canvas size
//! differed from its topmost layer, then saving again with no edits,
//! silently shrank or grew the canvas to match that layer instead of
//! preserving it. `canvas_size` is now seeded once (from
//! `document_canvas_size` for `demo_document`/a recovered autosave,
//! from a decoded image's own real dimensions for `Self::open_file`, or
//! restored directly from a `.aur` file's own manifest for
//! `Self::open_aur_file`) and stays live from then on, read (not
//! re-derived) by `Self::save_aur_file`. `document_canvas_size` itself
//! is unchanged — it's the fallback `canvas_size` is seeded *from* when
//! nothing else names a real one, not replaced.
//!
//! `aurora_io::aur`'s own `write`/`read` gained a real
//! `Option<&aurora_color::IccProfile>`/`Option<IccProfile>` parameter
//! (`aurora_color::IccProfile::to_bytes`, new the same day, wraps
//! `lcms2`'s own `Profile::icc`) — a `.aur` file can now embed and
//! restore a genuine ICC profile, not just a bare `Srgb` tag, tested
//! against a real non-sRGB profile from `corpora/icc/`.
//! `Self::save_aur_file`/`Self::open_aur_file` both still only ever
//! pass/discard `None` — this crate has no colour-management UI yet to
//! have set a non-sRGB profile with in the first place — stated
//! honestly rather than inventing an `App`-level field nothing could
//! ever set to something else yet.
//!
//! **Real rendering, for the first time** (PLAN.md M1.8's own "Canvas"
//! bullet): `resumed` builds an `aurora_gpu::TileResidency` and
//! `aurora_gpu::CanvasPipeline` sized to the canvas dock area; `redraw`
//! syncs the atlas from `tile_store` (whatever `active_layer` actually
//! holds, painted or not) and draws it within that area's own viewport,
//! in the same pass that already clears the background — a Brush stroke
//! is now actually visible, not just written into an otherwise-invisible
//! store. **Zoom followed the same way pan did**: `redraw` now passes
//! `canvas_view.zoom()` into `aurora_gpu::TileResidency::set_origin`,
//! which shrinks/grows the atlas's own sampled `uv_scale` by that
//! factor (shader-side scaling — no bigger upload, no mip selection);
//! `canvas_local_origin` goes through `CanvasView::to_document` instead
//! of assuming `zoom() == 1.0`, so panning while zoomed picks the right
//! tile too. **Sub-tile fractional scroll fixed 2026-08-13**: the
//! atlas's own uv offset used to be floored to a whole tile before
//! `set_origin` ever saw it (visible as painted content landing offset
//! from the cursor after any zoom/pan, and panning under one tile not
//! moving anything); `TileResidency::set_origin` now takes the
//! continuous position directly and folds the fractional remainder into
//! the sampled UV offset itself. **Scope, stated honestly**: rendering a
//! lower mip while zoomed out or panning (`spike/FINDINGS.md`'s own
//! progressive-rendering finding), rotation, rulers, guides, grid, snap,
//! and true infinite zoom all remain this bullet's own still-open
//! remainder. **Window resize is handled** (`apply_resize` calls
//! `TileResidency::resize`, see that method's own doc comment) — no
//! longer part of this remainder.
//!
//! **A real bug fixed, 2026-08-06**: real hardware (macOS) reported
//! ~100% CPU at idle. `about_to_wait` was requesting a redraw
//! unconditionally on *every* event-loop iteration, including the
//! iteration its own previous request had just woken — a self-
//! sustaining loop that defeated `ControlFlow::Wait` entirely, and got
//! measurably worse the same day multi-layer compositing gave every
//! redraw real per-tile CPU work to do. Fixed with a `needs_redraw`
//! flag, set on real `WindowEvent`s and cleared once a redraw is
//! actually requested for them — see that field's own doc comment.
//! macOS still needs *some* periodic wakeup (muda's own menu-event
//! channel has no event-loop integration to interrupt a true `Wait`),
//! now scoped to a short `ControlFlow::WaitUntil` poll instead of a
//! permanent busy loop; non-macOS stays on plain, fully blocking
//! `Wait`.
//! Not yet real-hardware-verified (this sandbox has no display server;
//! real GPU tests pass here, but nothing has shown this crate's own
//! window on an actual screen since M1.8's original human-verification
//! pass, which predates this work).
//!
//! **Human-verified on macOS, 2026-08-03** (real hardware, real desktop
//! session): the window opens, resizes without crashing, and `VoiceOver`
//! announces it — the create-hidden → attach-adapter → show ordering
//! and the accessibility tree both reach a real screen reader. Windows
//! and Linux remain unverified on real hardware — see PLAN.md M1.8. The
//! keyboard-shortcut/command-palette/crash-recovery/DPI-scaling/
//! clipboard/file-dialog/drag-and-drop/native-menu work above has not
//! yet had its own real-hardware pass — the native menu bar in
//! particular has never been compiled at all outside CI (this
//! development sandbox is Linux, where the dependency isn't even
//! present).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aurora_gpu::{GpuContext, GpuSurface};
use aurora_theme::{Palette, Scales, Theme, ThemeSet};
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey, ShortcutRegistry};
use aurora_widgets::widgets::{
    CommandEntry, DialogAction, DialogHandle, WidgetKind, command_palette_state,
    insert_command_palette, insert_dialog, move_command_palette_selection,
    set_command_palette_query,
};
use aurora_widgets::{FocusManager, GpuMesh, PathPipeline, WidgetId, WidgetTree, paint_widget};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

const PALETTE_TOML: &str = include_str!("../../../design/tokens/palette.toml");
const DARK_THEME_TOML: &str = include_str!("../../../design/themes/dark.toml");
const SCALES_TOML: &str = include_str!("../../../design/tokens/scales.toml");

/// Loads the real, owner-approved Dark theme (`design/themes/dark.toml`
/// — the only built-in theme that exists as a real design yet; Light/
/// high-contrast/Colour-Critical are Cahya's own design decisions still
/// to make, per `aurora-theme`'s own doc comment).
///
/// Theme *selection* (choosing among built-ins, a user preference) is
/// separate, later work; this always loads Dark. [`App`] keeps the
/// result alive for the whole session (`App::theme`) — real chrome
/// (window background, a widget's own paint) reads it every frame,
/// not just once at startup.
///
/// # Errors
///
/// Returns an error if the built-in palette/theme TOML fails to parse —
/// which would mean the checked-in design files themselves are broken,
/// not a runtime condition a user could hit.
fn load_theme() -> anyhow::Result<Theme> {
    let palette = Palette::from_toml_str(PALETTE_TOML)?;
    let mut themes = ThemeSet::new();
    themes.register(DARK_THEME_TOML)?;
    Ok(themes.resolve("Dark", &palette)?)
}

/// Converts `theme`'s `surface.app` token (the overall application
/// chrome background — `surface.canvas` is reserved for the document
/// canvas area, which doesn't exist yet) into the linear-light
/// `wgpu::Color` a window clear needs.
#[must_use]
fn background_color_from_theme(theme: &Theme) -> wgpu::Color {
    let [r, g, b] = theme.surface.app.to_srgb_f32();
    wgpu::Color {
        // The surface format is sRGB-aware (`Bgra8UnormSrgb`, per
        // `aurora-gpu`'s own `create_surface`/`examples/surface_smoke.rs`),
        // and every graphics API's clear-colour convention expects
        // linear values for an sRGB-typed render target, not the
        // token's own sRGB-gamma-encoded bytes — using those directly
        // would wash the colour out (a classic double-encoding bug).
        r: f64::from(aurora_color::srgb_to_linear(r)),
        g: f64::from(aurora_color::srgb_to_linear(g)),
        b: f64::from(aurora_color::srgb_to_linear(b)),
        a: 1.0,
    }
}

/// Linearizes a widget's own straight, sRGB-gamma-encoded paint colour
/// ([`aurora_widgets::paint_widget`]'s own return convention) for the
/// swapchain surface's sRGB-aware target format — the same "the target
/// expects linear, using gamma-encoded values directly double-encodes
/// and washes the colour out" reasoning [`background_color_from_theme`]
/// already applies to the window's own clear colour. Alpha is a
/// blend-equation coefficient, not a gamma-encoded sample, so it passes
/// through unchanged.
#[must_use]
fn linearize_paint_color(color: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = color;
    [
        aurora_color::srgb_to_linear(r),
        aurora_color::srgb_to_linear(g),
        aurora_color::srgb_to_linear(b),
        a,
    ]
}

/// Loads the real, owner-approved scales (`design/tokens/scales.toml`)
/// — needed by any widget with real chrome (buttons, the crash-recovery
/// dialog built from them) per invariant §7.3.10, the same "resolve
/// from tokens, never a literal" discipline [`load_theme`] already
/// applies to colour.
///
/// # Errors
///
/// Returns an error if the built-in scales TOML fails to parse — same
/// caveat as [`load_theme`]: this would mean the checked-in design file
/// itself is broken, not a runtime condition a user could hit.
fn load_scales() -> anyhow::Result<Scales> {
    Ok(Scales::from_toml_str(SCALES_TOML)?)
}

/// Reads the real, current OS accessibility preferences (PLAN.md M1.8's
/// "OS settings: reduced motion, high contrast, text size" bullet) —
/// this crate is the one place in the workspace allowed to do this kind
/// of platform-facing detection; [`aurora_theme::Scales`] itself stays
/// plain data (`aurora_theme::AccessibilityPreferences`'s own doc
/// comment).
///
/// **macOS only, for now.** `NSWorkspace`'s accessibility-display API
/// (`objc2_app_kit`, already a transitive dependency via `winit`'s own
/// macOS backend, promoted to direct here) is real and needs no
/// `unsafe` to call — its generated bindings are the FFI boundary, not
/// this function. Windows (`SystemParametersInfo`'s
/// `SPI_GETCLIENTAREAANIMATION`/`SPI_GETHIGHCONTRAST`) and Linux (no
/// OS-standard signal at all — desktop-environment-specific, e.g.
/// GNOME's own D-Bus settings portal) are real, separate follow-on
/// spikes, honestly left open rather than guessed at; every other
/// target here returns [`AccessibilityPreferences::default`].
/// `text_scale` stays `1.0` even on macOS: `AppKit` has no systemwide
/// text-scale preference equivalent to iOS's Dynamic Type, so there is
/// nothing real to read yet.
#[must_use]
#[cfg(target_os = "macos")]
fn detect_accessibility_preferences() -> aurora_theme::AccessibilityPreferences {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    aurora_theme::AccessibilityPreferences {
        reduced_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        high_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
        text_scale: 1.0,
    }
}

/// See the `#[cfg(target_os = "macos")]` overload's own doc comment —
/// this platform has no real detection wired up yet, so the honest
/// default applies rather than a guess.
#[must_use]
#[cfg(not(target_os = "macos"))]
fn detect_accessibility_preferences() -> aurora_theme::AccessibilityPreferences {
    aurora_theme::AccessibilityPreferences::default()
}

/// A small, clearly-fake document — there is no real "open a document"
/// flow in this crate yet (separate, still-open M1.9 work), so this
/// exists purely to give the Layers *and* History panels real content
/// to expose, rather than empty regions. Illustrative names matching
/// `design/mockups/workspace.html`'s own example layers, which are
/// explicitly "structure and token usage for review," not real content
/// either. Built entirely through `History`'s own methods, not direct
/// `LayerTree` calls, specifically so its journal (what the History
/// panel reads) is a real, meaningful record of these actions rather
/// than empty. `add_pixel_layer` inserts each new layer as the new
/// topmost root, so inserting Background, then Color balance, then
/// Retouch leaves them in that same top-to-bottom order the mockup
/// shows.
#[must_use]
fn demo_document() -> (aurora_doc::LayerTree, aurora_doc::History) {
    let canvas = aurora_core::Rect {
        x: 0,
        y: 0,
        width: 4000,
        height: 3000,
    };
    let mut layers = aurora_doc::LayerTree::new();
    let mut history = aurora_doc::History::new();

    if let Err(err) = history.add_pixel_layer(&mut layers, "Background", canvas, None) {
        unreachable!("a fresh tree with parent: None cannot fail: {err:?}");
    }

    let color_balance = match history.add_pixel_layer(&mut layers, "Color balance", canvas, None) {
        Ok(id) => id,
        Err(err) => unreachable!("a fresh tree with parent: None cannot fail: {err:?}"),
    };
    if let Err(err) =
        history.set_blend_mode(&mut layers, color_balance, aurora_doc::BlendMode::Multiply)
    {
        unreachable!("color_balance was just created in this same tree: {err:?}");
    }
    if let Err(err) = history.set_opacity(&mut layers, color_balance, 0.8) {
        unreachable!("0.8 is within 0.0..=1.0 and color_balance exists: {err:?}");
    }

    if let Err(err) = history.add_pixel_layer(&mut layers, "Retouch — skin", canvas, None) {
        unreachable!("a fresh tree with parent: None cannot fail: {err:?}");
    }

    (layers, history)
}

/// Builds a fresh, single-layer document from a decoded
/// `aurora_io::Image` — the real "open a file" document construction
/// [`Self::open_file`] needs, mirroring [`demo_document`]'s own shape
/// (built through `History`, not `LayerTree` directly, so the journal
/// stays a meaningful record for the History panel and autosave) but
/// with exactly the one real layer the opened file actually has, sized
/// to `image`'s own `width`/`height` at document-space `(0, 0)`.
/// Returns the new layer's own id alongside the built tree/history —
/// the caller needs it both to write `image`'s own pixels into the
/// right tile-store surface and to set it as the new active layer.
#[must_use]
fn document_from_image(
    name: impl Into<String>,
    image: &aurora_io::Image,
) -> (
    aurora_doc::LayerTree,
    aurora_doc::History,
    aurora_doc::LayerId,
) {
    let bounds = aurora_core::Rect {
        x: 0,
        y: 0,
        width: image.width(),
        height: image.height(),
    };
    let mut layers = aurora_doc::LayerTree::new();
    let mut history = aurora_doc::History::new();
    let id = match history.add_pixel_layer(&mut layers, name, bounds, None) {
        Ok(id) => id,
        Err(err) => unreachable!("a fresh tree with parent: None cannot fail: {err:?}"),
    };
    (layers, history, id)
}

/// Reads `path` from disk and decodes it via
/// `aurora_io::decode_by_extension` — the real "open a file" read+decode
/// step. `None` on any real failure (I/O, decode, unrecognised
/// extension), logged as a warning rather than propagated: a bad chosen
/// file must never crash or leave `App` in a half-updated state, the
/// same honesty [`recover_document`] already applies to a bad autosave.
#[must_use]
fn open_image(path: &Path) -> Option<aurora_io::Image> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to read the chosen file");
            return None;
        }
    };
    match aurora_io::decode_by_extension(path, &bytes) {
        Ok(image) => Some(image),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to decode the chosen file");
            None
        }
    }
}

/// Writes `bytes` to `path` without ever leaving a corrupt or partial
/// file in `path`'s own place if something goes wrong partway through:
/// writes to a sibling `.tmp` file first, verifies it actually reads
/// and decodes back as a real image of exactly `width`x`height`
/// (`aurora_io::decode_by_extension` against the file re-read from
/// disk, not just the in-memory `bytes` — catching a truncated write,
/// e.g. a full disk mid-write, not only a wrong encoder output), then
/// atomically renames it over `path`. CLAUDE.md's own PSD/PSB
/// round-trip rule applies here too, even though PNG/JPEG/TIFF aren't
/// the round-trip-critical case that rule names: "never overwrite a
/// user's file in place... write to temp, verify by reopening, then
/// swap" — a half-written export silently destroying whatever used to
/// be at `path` is exactly as bad regardless of format.
///
/// **Known gap, not built here**: CLAUDE.md's other half of that same
/// rule — "warn with an itemized list before any lossy save" — has no
/// UI to attach to yet (no warning-dialog widget exists, and this
/// function has no way to know *why* a save might be lossy, e.g. a
/// JPEG export silently dropping an alpha channel). Real, separate
/// follow-on work.
///
/// Returns `false` (logged) on any failure — writing the temp file,
/// reading/decoding it back, a size mismatch, or the final rename —
/// leaving `path` itself untouched in every one of those cases; the
/// temp file is cleaned up rather than left behind.
#[must_use]
fn write_verified(path: &Path, bytes: &[u8], width: u32, height: u32) -> bool {
    let Some(file_name) = path.file_name() else {
        tracing::warn!(path = %path.display(), "save path has no file name");
        return false;
    };
    let mut temp_name = file_name.to_os_string();
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);

    if let Err(err) = std::fs::write(&temp_path, bytes) {
        tracing::warn!(path = %temp_path.display(), %err, "failed to write the temp export file");
        return false;
    }

    let verified = std::fs::read(&temp_path)
        .ok()
        .and_then(|reread| aurora_io::decode_by_extension(path, &reread).ok())
        .is_some_and(|image| image.width() == width && image.height() == height);
    if !verified {
        tracing::warn!(path = %temp_path.display(), "exported file failed to verify by reading it back");
        let _ = std::fs::remove_file(&temp_path);
        return false;
    }

    if let Err(err) = std::fs::rename(&temp_path, path) {
        tracing::warn!(path = %path.display(), %err, "failed to replace the destination with the verified export");
        let _ = std::fs::remove_file(&temp_path);
        return false;
    }
    true
}

/// Whether `path`'s own extension names a real `.aur` document (ADR
/// 0009), case-insensitively — [`App::open_file`]/[`App::save_file`]'s
/// own dispatch between a whole-document `.aur` and a flat image.
#[must_use]
fn is_aur_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("aur"))
}

/// `layers`' own topmost pixel layer's `bounds` (`(width, height)`), or
/// `(0, 0)` if there isn't one — the *fallback* [`App::canvas_size`] is
/// seeded from when nothing else names a real, independent canvas size
/// for a document (a freshly built [`demo_document`]; a recovered
/// autosave no longer needs it, since its `.aur` manifest carries a
/// real one). Once a document is live, `App::canvas_size` is the real source
/// of truth — this is deliberately *not* called again on every save;
/// see that field's own doc comment for the bug re-deriving it used to
/// cause.
#[must_use]
fn document_canvas_size(layers: &aurora_doc::LayerTree) -> (u32, u32) {
    topmost_pixel_layer(layers)
        .and_then(|id| layers.bounds(id))
        .map_or((0, 0), |bounds| (bounds.width, bounds.height))
}

/// Where a `.aur` export's own "verify by reading it back"
/// ([`verify_aur`]) keeps its throwaway `aurora_tile::TileStore`
/// scratch files: a fresh directory *per call*, created inside
/// [`tile_store_scratch_dir`]'s per-session one, and returned as the
/// owning `tempfile::TempDir` so that dropping it deletes the whole
/// directory and every tile the verification paged into it.
///
/// **Per call, not per session, and that is the point.** Verifying an
/// export builds a brand-new store, whose per-instance filename token
/// ([`aurora_tile::TileStore`], 0.53.0) makes every tile it writes a
/// *new* file. A directory shared across calls would therefore
/// accumulate one full set of paged-out tiles per save, for the life of
/// the session, with nothing ever deleting them — `TileStore` has no
/// `Drop` that removes its own files. Returning the `TempDir` binds
/// that cleanup to the verification's own stack frame instead, so it
/// happens on every return path, success and failure alike, without
/// this having to enumerate them.
///
/// **Nested under the session directory, not beside it.** Until 0.53.0
/// this was a *second* fixed, world-readable, cross-process path
/// (`std::env::temp_dir().join("aurora-aur-verify")`) with exactly the
/// collision and confidentiality problems the live store's own fixed
/// path had. Being a child of the session directory means it inherits
/// its `0o700` mode, its unpredictable name, and its clean-shutdown
/// removal; verifying a fresh export still never touches the live
/// document's real tiles.
///
/// **It never returns `None` merely because the session directory went
/// away.** The session directory is a memoized *path*, not a guaranteed
/// directory: a temp cleaner or a user clearing `/tmp` can delete it
/// mid-run. This recreates it (owner-only) and, if even that fails,
/// falls back to an independent temp directory rather than failing —
/// because failing here reaches [`App::save_aur_file`] as "the export
/// did not verify", which *deletes the export*. Silently discarding a
/// professional's save is the worst thing this project can do, so the
/// degraded case gives up the nesting, not the save.
fn aur_verify_scratch_dir() -> Option<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("aur-verify-");
    // Same reasoning as `create_tile_store_scratch_dir`'s own call:
    // `tempfile` gives temp *directories* plain umask-derived
    // permissions. The parent is already `0o700`, so this is
    // belt-and-braces rather than load-bearing -- but it costs one line
    // and does not depend on the parent for its own correctness.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }

    if let Some(session) = tile_store_scratch_dir() {
        // Recreate the session directory if it is gone. It is a *path*
        // that is memoized, not a directory that is guaranteed to exist:
        // a temp cleaner sweeping `/tmp`, or a user clearing temp files,
        // removes it out from under a running session, and
        // `tempdir_in` against a missing parent just fails. That failure
        // used to reach `App::save_aur_file` as "verification failed",
        // which deletes the export it has just written -- so a swept
        // temp directory silently discarded every subsequent save for
        // the rest of the run. `recursive(true)` makes this a no-op in
        // the overwhelmingly common case where the directory is still
        // there. The live tile store already self-heals this way
        // (`aurora_tile::TileStore::new` creates its directory on every
        // open); this is the same property for the verifier.
        if let Err(err) = create_dir_owner_only(session) {
            tracing::warn!(
                ?err,
                path = %session.display(),
                "could not recreate this session's scratch directory for .aur verification"
            );
        }
        match builder.tempdir_in(session) {
            Ok(dir) => return Some(dir),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    path = %session.display(),
                    "could not nest the .aur verification scratch directory under this session's; \
                     falling back to an independent one"
                );
            }
        }
    }

    // Degraded, but never silently: an independent temp directory,
    // still randomly named, still exclusively created, still `0o700`.
    // It gives up inheriting the session directory's clean-shutdown
    // removal -- but this call's own `TempDir` guard is what actually
    // deletes it, and a verification that cannot run at all is far
    // worse than one running a directory further out.
    match builder.tempdir() {
        Ok(dir) => Some(dir),
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to create any scratch directory for the .aur verification store"
            );
            None
        }
    }
}

/// Creates `dir` (and any missing parent) owner-only on Unix, and
/// accepts one that already exists — `aurora_tile`'s own
/// `create_private_dir` without the hardening checks, which is all this
/// needs: the only caller ([`aur_verify_scratch_dir`]) is recreating a
/// directory this process itself created under a random name, not
/// adopting an arbitrary caller-supplied path.
///
/// Windows gets the parent's inherited ACL, the same gap
/// [`create_autosave_temp`] already discloses.
#[cfg(unix)]
fn create_dir_owner_only(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Non-Unix counterpart of the above — see its doc comment.
#[cfg(not(unix))]
fn create_dir_owner_only(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Reads `path` back as a `.aur` file, against a fresh, throwaway
/// `aurora_tile::TileStore` ([`aur_verify_scratch_dir`]) — [`App::save_aur_file`]'s
/// own "never leave a corrupt file in place" check, the `.aur`
/// counterpart to [`write_verified`]'s own "decodes to the right
/// width/height" check for a flat image (`.aur` has no single such
/// number to compare; successfully parsing the whole container,
/// manifest, history, and every tile entry it names is itself the
/// check). `false` (logged) if the scratch store fails to open or
/// `aurora_io::read_aur` itself fails for any reason.
///
/// The scratch directory is a `tempfile::TempDir` bound for the whole
/// body: it is declared *before* `store`, so on every return path the
/// store drops first (joining its background writer thread, so no write
/// can still be in flight) and the directory's own `Drop` then removes
/// it and everything the verification paged into it. Verifying a large
/// document evicts real tiles, and before 0.53.0 those files were never
/// deleted at all.
#[must_use]
fn verify_aur(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        tracing::warn!(path = %path.display(), "failed to reopen the exported .aur file to verify it");
        return false;
    };
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let Some(scratch_dir) = aur_verify_scratch_dir() else {
        tracing::warn!("no scratch directory for the .aur verification store");
        return false;
    };
    let mut store = match aurora_tile::TileStore::new(scratch_dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => {
            tracing::warn!(?err, "failed to open the .aur verification scratch store");
            return false;
        }
    };
    match aurora_io::read_aur(file, &mut store) {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!(path = %path.display(), ?err, "exported .aur file failed to read back");
            false
        }
    }
}

/// Clears and repopulates `workspace`'s Layers/History/Properties panels
/// for a freshly opened `layers`/`history` — the real "replace the
/// current document" step [`App::open_file`] needs, shared by both
/// routes that reach it: a single-image import ([`document_from_image`],
/// always exactly one new pixel layer) and a real, possibly multi-layer
/// `.aur` open (`aurora_io::read_aur`). Returns the new
/// `WidgetId -> LayerId` map (`aurora_ui::populate_layers_panel`'s own
/// return value) and which layer should become the new active one —
/// `layers`' own topmost pixel layer ([`topmost_pixel_layer`]; for a
/// freshly imported single-layer document this is trivially the layer
/// that was just created) — for the caller to assign onto its own
/// fields alongside `layers`/`history` themselves (kept by the caller,
/// not threaded through here, since this function only needs to read
/// them).
///
/// `tool` reseeds the Properties panel with `tool`'s own current
/// options ([`tool_options`]) — opening a different document doesn't
/// change which tool is selected, so the caller's own current
/// `self.tool` is what this should show, not [`aurora_ui::Tool::
/// default`].
///
/// # Errors
///
/// Propagates [`aurora_widgets::WidgetError`] if clearing or
/// repopulating any panel fails — structurally unreachable in practice
/// (`workspace` is always a real `aurora_ui::build_workspace` with real
/// panel bodies), but this function doesn't itself know that, so it
/// reports rather than assumes.
fn replace_document(
    workspace: &mut aurora_ui::Workspace,
    scales: &Scales,
    layers: &aurora_doc::LayerTree,
    history: &aurora_doc::History,
    tool: aurora_ui::Tool,
) -> Result<
    (
        HashMap<WidgetId, aurora_doc::LayerId>,
        Option<aurora_doc::LayerId>,
    ),
    aurora_widgets::WidgetError,
> {
    aurora_ui::clear_panel_body(&mut workspace.tree, workspace.layers.body)?;
    let layer_rows =
        aurora_ui::populate_layers_panel(&mut workspace.tree, workspace.layers, scales, layers)?;
    aurora_ui::clear_panel_body(&mut workspace.tree, workspace.history.body)?;
    aurora_ui::populate_history_panel(&mut workspace.tree, workspace.history, history)?;
    aurora_ui::clear_panel_body(&mut workspace.tree, workspace.properties.body)?;
    let options = tool_options(tool);
    aurora_ui::populate_properties_panel(
        &mut workspace.tree,
        workspace.properties,
        tool,
        &options,
    )?;
    Ok((layer_rows, topmost_pixel_layer(layers)))
}

// -- Crash recovery: an unclosed-session marker, plus a real autosave --
//
// PLAN.md M1.8's "crash recovery UI" bullet detected whether the
// *previous* run reached its own clean-shutdown step, but couldn't
// restore any actual document state: `aurora-doc`'s crash-recovery
// journal only had its in-memory half built (`History::replay`) — no
// on-disk encoding for `LayerOp`'s recursive shape had been decided yet.
// PLAN.md M1.9's "autosave and recovery" bullet closes that gap: ADR
// 0009 picked `postcard` for `.aur`'s manifest/history encoding, and
// `History::save_journal`/`load_journal` now use it. So this section now
// does two things: writes this session's own document to an autosave
// file at startup ([`write_autosave`], skipped when recovery just
// succeeded — see [`startup_document`]), and — if a previous run's
// marker is present — tries to read that file back
// ([`recover_document`]), falling back to the fake demo document if
// there's nothing to recover or it doesn't parse.
//
// Still deliberately narrow: recovery is unconditional (there is no
// "Recover Document" vs. "Discard" choice — the dialog just reports
// what already happened).
//
// **The autosave file is a real `.aur` container now** (ADR 0009's ZIP
// archive: `mimetype` sentinel, `postcard` manifest carrying the whole
// `LayerTree` *and* the document's own canvas size, the history
// journal, and one entry per non-blank tile) — written by
// [`write_autosave`] through `aurora_io::write_aur` and read back by
// [`recover_document`] through `aurora_io::read_aur`, straight into the
// live `aurora_tile::TileStore`. Before this, the autosave was raw
// `postcard` journal bytes: a `LayerOp` sequence and nothing else, so a
// crash recovered a document's *structure* (layers, bounds, opacity,
// blend modes) and lost 100% of its actual painted pixels, since
// nothing in the journal has ever described pixel content
// (`aurora_brush::PixelHistory`'s own doc comment). Recovery now
// restores the real painted tiles and the real canvas size too.
//
// **Written at lifecycle boundaries only, and that is a deliberate
// scope reduction.** Between 0.41.0 and 0.48.1 this crate re-triggered
// the autosave after every committed edit (`App::trigger_autosave`),
// which was affordable while the file held nothing but a `postcard`
// journal: encoding it was pure, cheap CPU, and the write itself went
// to a background thread. Making the file a real `.aur` container
// changed that completely. `aurora_io::write_aur` walks every pixel
// layer's own tile grid and calls `TileStore::get` on each in-bounds
// tile, which can page a tile in from the scratch disk *and* evict
// another to stay inside the store's budget — real I/O and real LRU
// churn against the live document's own store, measured at **687 ms**
// for a 4000x3000, three-layer document on this dev box, and unbounded
// at the 300,000 x 300,000 px ceiling (§7.3.1). That work cannot be
// moved off the calling thread: `TileStore` is owned outright by `App`
// and every tile-touching call site in this crate runs on the UI
// thread. Making it shareable is the separate, parked
// `TileStore`-threading redesign (PLAN.md's own "Next action"), not
// something to do as a side effect here.
//
// Rate-gating the trigger was tried first and rejected on review: a
// gate bounds *how often* a 687 ms stall happens, not whether one ever
// lands on a stroke-commit path measured at 9.1 ms p99 against a 10 ms
// budget (spike/FINDINGS.md), and the eviction it causes degrades the
// *next* strokes too, since a full grid walk pushes exactly the tiles
// the user is painting on out of the LRU. So the live trigger is gone
// entirely. `write_autosave` is called at real lifecycle boundaries and
// nowhere else: a fresh session's startup document (`App::new`, skipped
// when recovery just succeeded — the file already holds that document)
// and each document replacement (`App::open_file`/`App::open_aur_file`).
//
// **The named limitation, stated plainly**: a crash loses every edit
// made since the last boundary — every painted pixel *and* every
// structural change. Against 0.48.1 this is better in one direction
// (a recovered document now has its real pixels, which it never did
// before) and worse in another (0.48.1 re-saved structure on every
// edit, cheaply, because structure was all it saved). Calling it a
// clean win would be dishonest. What would lift it is the same
// still-open work either way: an incremental, dirty-tile-only autosave,
// or a tile store readable from a background thread.
//
// Both the marker and the autosave file live in `std::env::temp_dir()`
// under fixed names — deliberately not a proper per-platform app-support
// directory, a pre-existing choice from when both files held only
// structure. **That is now a real, if narrow, confidentiality
// limitation, and it is only half-fixed**: since 0.49.0 the autosave
// holds the user's actual painted pixels at a predictable path in a
// world-readable directory. The file itself is created `0o600` on Unix
// ([`create_autosave_temp`]) and deleted on a clean shutdown
// ([`remove_autosave`]), which closes the read side there; Windows ACLs
// are *not* addressed, and neither is the directory choice itself. The
// real fix for both is the same move to a per-user app-support
// directory (`directories::ProjectDirs`, already a dependency for
// [`layout_path`]), which is separate, still-open work rather than
// something to fold into this change. The *tile scratch directory*, the
// third temp-directory path this crate used to keep at a fixed name, is
// no longer one of them: 0.53.0 moved it to a randomly named, `0o700`,
// per-session directory ([`create_tile_store_scratch_dir`]) removed on a
// clean shutdown. The marker and the autosave still use fixed temp
// paths.

/// Where this run's own "I'm still running" marker lives.
fn marker_path() -> PathBuf {
    std::env::temp_dir().join("aurora-session.marker")
}

/// True if a marker from a *previous* run is still present at `path` —
/// meaning that run never reached [`clear_session_marker`], i.e. it
/// didn't shut down cleanly (a crash, a force-quit, a killed process).
#[must_use]
fn previous_session_left_a_marker(path: &Path) -> bool {
    path.exists()
}

/// Writes this run's own marker at `path` — call once, early, before
/// [`previous_session_left_a_marker`] would see it as a *previous*
/// run's. Errors are logged, not fatal: failing to write a marker file
/// must never stop the application starting.
fn write_session_marker(path: &Path) {
    if let Err(err) = std::fs::write(path, []) {
        tracing::warn!(?err, path = %path.display(), "failed to write the crash-recovery session marker");
    }
}

/// Removes this run's own marker at `path` — call on a clean shutdown.
/// If this never runs, the marker left behind is exactly the signal the
/// *next* run's [`previous_session_left_a_marker`] needs. A missing
/// marker is not an error (e.g. shutting down twice, or a marker that
/// was never successfully written in the first place).
fn clear_session_marker(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %path.display(), "failed to remove the crash-recovery session marker");
    }
}

/// Where this run's own autosave document lives — analogous to
/// [`marker_path`], and for the same reason not a proper per-platform
/// app-support directory yet. A real `.aur` container (ADR 0009), not
/// the raw `postcard` journal this used to be — see this section's own
/// doc comment.
fn autosave_path() -> PathBuf {
    std::env::temp_dir().join("aurora-autosave.aur")
}

/// Writes a complete `.aur` autosave container to `path` — `layers`,
/// `history`, `canvas_size`, and every non-blank tile currently in
/// `store` (`aurora_io::write_aur`). Call at a real lifecycle boundary
/// (startup with a fresh demo document, a document replacement), and
/// **only** there: this walks every pixel layer's own tile grid and can
/// page tiles in from the scratch disk, evicting others from `store`'s
/// LRU to stay inside its budget — measured at 687 ms for a 4000x3000,
/// three-layer document on this dev box, and unbounded at the
/// 300,000 px document ceiling. See this section's own doc comment for
/// why that measurement is exactly what took the live per-edit
/// re-trigger back out.
///
/// Streamed straight into the temp `File` rather than built in memory
/// first: `aurora_io::write_aur` is generic over `W: Write + Seek`
/// precisely so a caller need not hold a whole document's compressed
/// pixel payload as a `Vec<u8>`, which at the documented ceiling is
/// exactly the "assumes a document fits in memory" that CLAUDE.md
/// §7.3.1 forbids.
///
/// Written to a **unique** sibling temp path and `rename`d into place,
/// so a crash *during* an autosave can't leave a half-written container
/// where the previous, complete one used to be — the same
/// write-to-temp-then-swap discipline [`write_verified`]/
/// [`App::save_aur_file`] already apply to a user's real file.
/// (`std::fs::rename` replaces an existing destination file on Windows
/// as well as on Unix — `MOVEFILE_REPLACE_EXISTING` — so the swap is
/// the same one operation on every platform this ships to.)
/// Deliberately *not* verified by reopening the way an explicit "Save
/// As" is ([`verify_aur`]): that roughly doubles a cost this path is
/// already trying to keep off the user's way, and is the right trade
/// for a file the user asked for, not for a background autosave.
///
/// Errors are logged, never fatal — the same shape
/// [`write_session_marker`] already uses: failing to autosave must
/// never stop the application starting or interrupt an edit.
///
/// **A knowingly incomplete result never lands on `path`.** A write that
/// had to skip tiles is renamed to [`partial_autosave_path`] instead, and
/// whatever complete autosave already exists is left exactly where it is;
/// a complete write lands on `path` and deletes any partial left over
/// from before. Crash-recovery protection is therefore monotonic — a
/// snapshot is only ever replaced by one at least as good — which the
/// first shape of the best-effort change got wrong: it renamed the
/// degraded result over the single fixed [`autosave_path`], so a document
/// that already had a complete autosave lost it the moment the scratch
/// disk went bad.
///
/// **One unreadable tile no longer costs the whole document its
/// crash-recovery protection** (0.52.2). This goes through
/// `aurora_io::write_aur_best_effort`, which leaves a tile it cannot page
/// in out of the container and names it, rather than `write_aur`, which
/// refuses the entire write. The explicit Save/Export path
/// ([`App::save_aur_file`], `composite_document`) still refuses, and that
/// difference is deliberate: a deliberate user action on a user's own
/// file must not quietly write incomplete content, while an automatic
/// background snapshot the user never asked for and cannot see fail has
/// nothing better to offer than "protect what is still readable". It is
/// the same split `recomposite_visible_tiles` (degrades, repaints) and
/// `composite_document` (refuses) already draw for the live canvas.
/// Since 0.52.2 an unreadable tile fails on *every* read rather than
/// healing into a blank one, so without this one bad tile would have
/// aborted every autosave for the rest of the session — every other
/// layer, every subsequent edit, silently unprotected.
fn write_autosave(
    path: &Path,
    layers: &aurora_doc::LayerTree,
    history: &aurora_doc::History,
    canvas_size: (u32, u32),
    store: &mut aurora_tile::TileStore,
) {
    let temp_path = autosave_temp_path(path);
    let Some(mut file) = create_autosave_temp(&temp_path) else {
        return;
    };
    // `profile: None`, the same reason [`App::save_aur_file`] already
    // passes it: no colour-management UI exists to have set a document
    // profile in the first place, so there is nothing real to embed.
    //
    // `write_aur_best_effort`, not `write_aur` -- see this function's own
    // doc comment for why an autosave is the one caller that degrades
    // rather than refusing.
    // Where this write is allowed to land, decided by whether it turned
    // out to be complete -- see this function's own doc comment.
    let destination;
    match aurora_io::write_aur_best_effort(&mut file, layers, history, canvas_size, None, store) {
        Ok(skipped) if skipped.is_empty() => {
            destination = path.to_path_buf();
        }
        Ok(skipped) => {
            // Loud, and every time: the file about to be written is
            // knowingly incomplete, which is exactly the thing that must
            // never be silent. The first one is named in full; the count
            // covers the rest without turning a broken scratch disk into
            // an unbounded log.
            let first = skipped
                .first()
                .map_or_else(String::new, |tile| format!("{tile:?}"));
            destination = partial_autosave_path(path);
            tracing::warn!(
                skipped = skipped.len(),
                %first,
                path = %destination.display(),
                "autosaving with tiles missing to the *partial* autosave path; the last complete \
                 autosave is left in place"
            );
        }
        Err(err) => {
            tracing::warn!(?err, path = %temp_path.display(), "failed to write the autosave container");
            drop(file);
            remove_autosave_temp(&temp_path);
            return;
        }
    }
    // Before the rename, not after: a `rename` of a file whose contents
    // are still only in the page cache is exactly how a power loss
    // leaves a correctly named, empty autosave in place of the real one.
    if let Err(err) = file.sync_all() {
        tracing::warn!(?err, path = %temp_path.display(), "failed to flush the autosave container");
        drop(file);
        remove_autosave_temp(&temp_path);
        return;
    }
    drop(file);
    if let Err(err) = std::fs::rename(&temp_path, &destination) {
        tracing::warn!(?err, path = %destination.display(), "failed to swap the autosave container into place");
        remove_autosave_temp(&temp_path);
        return;
    }
    // A complete snapshot supersedes any partial one: leaving a stale
    // partial around is how an *older*, lossy snapshot could later
    // resurface as if it were current.
    if destination == path {
        remove_partial_autosave(path);
    }
}

/// Where a *knowingly incomplete* autosave goes — a sibling of `path`,
/// never `path` itself.
///
/// The distinction is the whole point (0.52.2, second review round).
/// Best-effort autosaving means a write can succeed while quietly
/// dropping tiles the scratch disk could no longer supply, and the
/// original shape of that change renamed the result over the single
/// fixed [`autosave_path`] regardless — so a document that already had a
/// **complete** autosave lost it to a degraded one the moment the
/// scratch disk went bad, with nothing left to recover the dropped
/// content from. Crash-recovery protection has to be monotonic: a
/// snapshot may only ever be replaced by one at least as good.
fn partial_autosave_path(path: &Path) -> PathBuf {
    path.with_extension("partial.aur")
}

/// Deletes the partial autosave beside `path`, if there is one. A
/// missing file is not an error; anything else is logged, never fatal.
fn remove_partial_autosave(path: &Path) {
    let partial = partial_autosave_path(path);
    if let Err(err) = std::fs::remove_file(&partial)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %partial.display(), "failed to remove the partial autosave");
    }
}

/// A temp path beside `path`, unique per process **and** per call —
/// `<name>.<pid>.<n>.tmp`.
///
/// Not cosmetic: a single fixed `.tmp` name is shared state between
/// every writer that exists, and two writers landing on it interleave
/// their bytes and destroy the crash-recovery file with no crash
/// involved. Two Aurora processes are enough on their own (both use the
/// same fixed [`autosave_path`]), and any future concurrent writer
/// inside one process would be too — cheaper to make impossible here
/// than to re-derive the argument every time a call site moves.
fn autosave_temp_path(path: &Path) -> PathBuf {
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().map_or_else(
        || std::ffi::OsString::from("aurora-autosave.aur"),
        std::ffi::OsStr::to_os_string,
    );
    name.push(format!(".{}.{sequence}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Creates `temp_path` for a fresh autosave write, owner-only where the
/// platform lets this crate say so. `None` (logged) if it can't be
/// created — including because it already exists, which
/// [`autosave_temp_path`]'s own uniqueness makes a real signal rather
/// than an expected collision.
///
/// **The permissions matter here.** Both this file and the autosave it
/// becomes live in `std::env::temp_dir()`, a world-readable directory
/// on a shared Unix machine, at a predictable name — and since 0.49.0
/// they hold the document's real painted pixels, not just its layer
/// structure. `0o600` closes the read side of that on Unix. Windows
/// ACLs are *not* addressed here: `OpenOptions` has no portable
/// equivalent, and the real fix for both is the same one — moving this
/// file out of the temp directory into a proper per-user app-support
/// directory, the pre-existing pattern [`marker_path`] shares and
/// separate, still-open work.
fn create_autosave_temp(temp_path: &Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(temp_path) {
        Ok(file) => Some(file),
        Err(err) => {
            tracing::warn!(?err, path = %temp_path.display(), "failed to create the autosave temp file");
            None
        }
    }
}

/// Removes a leftover autosave temp file after a failed write/rename —
/// a missing one is not an error (the write may never have created it),
/// and any other failure is logged rather than propagated, since this
/// is already the cleanup path of something that failed.
fn remove_autosave_temp(temp_path: &Path) {
    if let Err(err) = std::fs::remove_file(temp_path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %temp_path.display(), "failed to remove the autosave temp file");
    }
}

/// Deletes this session's own autosave container on a clean shutdown —
/// the same lifecycle point [`clear_session_marker`] runs at, and for
/// the same reason: once this run has ended cleanly there is nothing
/// left to recover, and leaving a file full of the user's real pixels
/// sitting at a predictable path in a shared temp directory is a
/// confidentiality cost with no remaining benefit. A missing file is
/// not an error; any other failure is logged, never fatal.
fn remove_autosave(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %path.display(), "failed to remove this session's autosave");
    }
    // Both files, for the same confidentiality reason and so a partial
    // snapshot from this session can never resurface in a later one.
    remove_partial_autosave(path);
}

/// Reads the `.aur` autosave container at `path`, if one is present and
/// usable, writing its tiles straight into `store` and returning the
/// restored `LayerTree`/`History` plus the document's own real canvas
/// size. Returns `None` — not an error — for anything that keeps this
/// from producing a usable document (no file, unreadable bytes, a
/// truncated or corrupt ZIP, a missing manifest/history entry, an
/// unsupported manifest version, or a tile entry that won't decode): a
/// missing or corrupt autosave means falling back to [`demo_document`],
/// not failing to start.
///
/// **Startup-only, in practice.** The one caller is [`App::new`], before
/// any painting has happened, so writing recovered tiles directly into
/// the live store can't clobber pixels the user is working on. Calling
/// this mid-session would need more thought than just a fresh call site
/// — the recovered surfaces would overwrite whatever those same
/// `SurfaceId`s currently hold.
fn recover_document(
    path: &Path,
    store: &mut aurora_tile::TileStore,
) -> Option<(aurora_doc::LayerTree, aurora_doc::History, (u32, u32))> {
    // A complete autosave always wins, even if a partial one is newer
    // (0.52.2, second review round). The partial file exists only for
    // the case where the scratch disk went bad before any complete
    // snapshot could be written; preferring it over a complete one would
    // trade known-good content for a few more recent edits on the layers
    // that still happened to be readable, which is the wrong side of
    // that trade for a crash-recovery file. `write_autosave` deletes any
    // partial as soon as a complete write lands, and
    // [`remove_autosave`] deletes both on a clean shutdown, so a stale
    // partial cannot linger to be picked up here later.
    //
    // A canonical file that exists but does not read back is handled one
    // level up, by [`recover_partial_after_a_failed_read`] -- it needs
    // the tile store replaced between the two attempts, which needs the
    // slot this function does not have.
    if !path.exists() {
        let partial = partial_autosave_path(path);
        if !partial.exists() {
            return None;
        }
        tracing::warn!(
            path = %partial.display(),
            "no complete autosave; recovering from a partial one, which is missing whatever tiles \
             the scratch disk could not supply when it was written"
        );
        return read_autosave_container(&partial, store);
    }
    read_autosave_container(path, store)
}

/// Reads the partial autosave beside `path` after the canonical
/// container was found and *failed* to read back — corruption, a
/// missing entry, an unsupported version, an undecodable tile. Without
/// this, keeping a partial snapshot protected nothing in precisely the
/// case it exists for: [`recover_document`] only reaches the partial
/// when the canonical file is absent, so a corrupt one shadowed it
/// completely.
///
/// **The store is replaced first**, the same reopen (and for the same
/// reason) [`startup_document`] already performs after a failed
/// recovery: `aurora_io::read_aur` writes each tile into the store as it
/// goes, so an attempt that fails partway through leaves real pixels
/// behind on surfaces the partial container's own layers are about to
/// claim. Recovering *into* those leftovers would show fragments of the
/// container that failed to read, mixed into the document that
/// succeeded. This takes `&mut Option<_>` for exactly that reason —
/// `None` from [`open_tile_store`] means the session simply continues
/// without painting, which is what `None` already means everywhere else
/// on this path.
fn recover_partial_after_a_failed_read(
    path: &Path,
    store_slot: &mut Option<aurora_tile::TileStore>,
) -> Option<(aurora_doc::LayerTree, aurora_doc::History, (u32, u32))> {
    let partial = partial_autosave_path(path);
    if !partial.exists() {
        return None;
    }
    *store_slot = open_tile_store();
    let store = store_slot.as_mut()?;
    tracing::warn!(
        path = %partial.display(),
        "the complete autosave could not be read; falling back to the partial one, which is \
         missing whatever tiles the scratch disk could not supply when it was written"
    );
    read_autosave_container(&partial, store)
}

/// [`recover_document`]'s own single-file half: opens and reads one
/// `.aur` container, with every failure answered by `None` rather than
/// an error. Split out so the complete-vs-partial choice above reads as
/// the policy it is.
fn read_autosave_container(
    path: &Path,
    store: &mut aurora_tile::TileStore,
) -> Option<(aurora_doc::LayerTree, aurora_doc::History, (u32, u32))> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(?err, path = %path.display(), "failed to open the autosave container");
            }
            return None;
        }
    };
    // The profile (4th element) is discarded for the same honest reason
    // `App::open_aur_file` discards its own: nothing in this crate yet
    // tracks a "current document profile" to restore it into, because no
    // colour-management UI exists to have set one -- and an autosave this
    // crate wrote always carries `None` anyway ([`write_autosave`]).
    match aurora_io::read_aur(file, store) {
        Ok((layers, history, canvas_size, _profile)) => Some((layers, history, canvas_size)),
        Err(err) => {
            tracing::warn!(?err, path = %path.display(), "failed to read the autosave container");
            None
        }
    }
}

/// What [`App::new`] starts this session with — see
/// [`startup_document`], which is the only thing that builds one.
struct StartupDocument {
    layers: aurora_doc::LayerTree,
    history: aurora_doc::History,
    /// The recovered document's own real canvas size (its `.aur`
    /// manifest carries one), or [`document_canvas_size`]'s fallback for
    /// a [`demo_document`], which has none of its own.
    canvas_size: (u32, u32),
    /// Whether this really came from an autosave — what the
    /// crash-recovery dialog's own message reports
    /// ([`crash_recovery_dialog_message`]).
    was_recovered: bool,
}

/// Resolves the document [`App::new`] opens with: a crash-recovered one
/// if the previous run left a marker behind *and* its autosave container
/// reads back ([`recover_document`]), otherwise [`demo_document`].
/// A *fresh* document is written straight back out as this session's
/// own autosave ([`write_autosave`]), so the next run has something
/// real to recover to; a *recovered* one is not, since the file it came
/// from already holds exactly those bytes.
///
/// A clean shutdown never needs its own autosave read back, and skipping
/// the attempt means an autosave file left over from a much older,
/// already-recovered-from crash can't resurface later — hence
/// `had_previous_marker` gating the read.
///
/// `store_slot` empty (no live tile store — painting is already
/// disabled for the session, see [`open_tile_store`]) skips both
/// halves, logged: there would be nowhere to put recovered pixels and
/// nothing real to put in a container, and a structure-only autosave is
/// exactly what this path stopped writing. It is taken as
/// `&mut Option<_>` rather than `Option<&mut _>` so a *failed* recovery
/// can replace the store outright — see the reopen below for why that
/// matters.
fn startup_document(
    had_previous_marker: bool,
    autosave_path: &Path,
    store_slot: &mut Option<aurora_tile::TileStore>,
) -> StartupDocument {
    let fresh = || {
        let (layers, history) = demo_document();
        let canvas_size = document_canvas_size(&layers);
        (layers, history, canvas_size)
    };
    if store_slot.is_none() {
        tracing::warn!("no live tile store; skipping crash recovery and this session's autosave");
        let (layers, history, canvas_size) = fresh();
        return StartupDocument {
            layers,
            history,
            canvas_size,
            was_recovered: false,
        };
    }
    let recovered = match (had_previous_marker, store_slot.as_mut()) {
        (true, Some(store)) => recover_document(autosave_path, store),
        _ => None,
    };
    // A canonical container that *exists* but does not read back is
    // exactly the case a partial snapshot is kept for -- corruption --
    // and [`recover_document`] cannot try it on its own, because
    // recovering from the partial needs the store replaced first and only
    // this function owns the slot to replace. When the canonical file is
    // simply absent, `recover_document` has already tried the partial and
    // this must not re-read it.
    let recovered = match recovered {
        Some(document) => Some(document),
        None if had_previous_marker && autosave_path.exists() => {
            recover_partial_after_a_failed_read(autosave_path, store_slot)
        }
        None => None,
    };
    if let Some((layers, history, canvas_size)) = recovered {
        // Nothing written back out: the file on disk *is* this
        // document, read back a few lines ago and not touched since.
        // Rewriting it here would be a full container rebuild (see
        // [`write_autosave`]'s own measured cost) for a byte-identical
        // result, on the pre-window startup path, against a <3 s startup
        // budget (PRD §6). The fresh-document case below still writes,
        // because there the file either doesn't exist or describes some
        // older session's document. Even that one could move off the
        // pre-window path in later work if startup measurement ever says
        // it needs to; it isn't turned into a background task now on
        // speculation.
        return StartupDocument {
            layers,
            history,
            canvas_size,
            was_recovered: true,
        };
    }
    if had_previous_marker {
        // A recovery attempt that failed can still have committed real
        // pixels: `aurora_io::read_aur` writes each tile into the store
        // as it goes, so a container whose central directory is intact
        // but whose *last* tile entry is corrupt leaves earlier
        // surfaces already populated before it returns `Err`. Those are
        // the same `SurfaceId`s [`demo_document`]'s own fresh layers are
        // about to claim, so keeping the store would show the user
        // fragments of the document that failed to recover, painted
        // into a document that has nothing to do with it. A fresh store
        // starts with no resident and no paged-out tiles, which is the
        // whole fix; if reopening itself fails, the session simply
        // continues without painting, exactly as
        // [`open_tile_store`]'s own `None` already means.
        *store_slot = open_tile_store();
    }
    let (layers, history, canvas_size) = fresh();
    if let Some(store) = store_slot.as_mut() {
        write_autosave(autosave_path, &layers, &history, canvas_size, store);
    } else {
        tracing::warn!("no live tile store; skipping this session's autosave");
    }
    StartupDocument {
        layers,
        history,
        canvas_size,
        was_recovered: false,
    }
}

// -- Persisted workspace layout (rail width, panel collapsed state) --
//
// PLAN.md M1.8's docking bullet, "persisted layouts" — the one piece of
// that bullet not scoped to real drag-state interaction. Unlike
// `marker_path`/`autosave_path` above, which deliberately use
// `std::env::temp_dir()` for genuinely ephemeral crash-recovery data, a
// layout preference should survive a reboot, not just a clean run —
// Cahya's own choice (`AskUserQuestion`) to use a real per-platform
// app-support directory (`directories::ProjectDirs`) instead of
// reusing `temp_dir()` for consistency with that existing precedent.
// Applied once at construction, saved once on a clean shutdown — the
// same "write once, at a real lifecycle boundary" discipline
// `write_autosave` already established, not a reactive save on every
// resize/collapse.

/// The persisted half of a [`aurora_ui::Workspace`]'s own dock layout —
/// rail width and whether each of the three panels is collapsed. A
/// snapshot taken right before writing it to disk
/// ([`save_workspace_layout`]), not a live view.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct WorkspaceLayout {
    rail_width: f32,
    layers_collapsed: bool,
    properties_collapsed: bool,
    history_collapsed: bool,
}

/// Where this crate's own persisted workspace layout lives — `None` if
/// the OS can't even report a home directory
/// (`directories::ProjectDirs::from`'s own documented failure case);
/// callers treat that the same as "no saved layout, and nowhere to
/// save one this run," not a hard error. No qualifier or organization
/// (both optional, only affecting macOS/Windows) — this project has
/// neither, matching how `aurora_ui::workspace` already labels the
/// window itself plain `"Aurora"`.
fn layout_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "Aurora")?;
    Some(dirs.config_dir().join("workspace-layout.postcard"))
}

/// Reads `workspace`'s own current rail width and each panel's
/// collapsed state (`aurora_ui::rail_width`/`panel_is_collapsed`) and
/// writes them to `path`, creating its parent directory first if
/// needed. The same "errors are logged, not fatal" shape
/// [`write_session_marker`]/[`write_autosave`] already use — failing to
/// save a layout preference must never stop the application from
/// closing.
fn save_workspace_layout(path: &Path, workspace: &aurora_ui::Workspace) {
    let Some(rail_width) = aurora_ui::rail_width(&workspace.tree, workspace.rail) else {
        return;
    };
    let collapsed = |panel| aurora_ui::panel_is_collapsed(&workspace.tree, panel).unwrap_or(false);
    let layout = WorkspaceLayout {
        rail_width,
        layers_collapsed: collapsed(workspace.layers),
        properties_collapsed: collapsed(workspace.properties),
        history_collapsed: collapsed(workspace.history),
    };
    let bytes = match postcard::to_allocvec(&layout) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(?err, "failed to serialize the workspace layout");
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            ?err,
            path = %parent.display(),
            "failed to create the workspace layout's own directory"
        );
        return;
    }
    if let Err(err) = std::fs::write(path, bytes) {
        tracing::warn!(?err, path = %path.display(), "failed to write the workspace layout");
    }
}

/// Reads a previously saved layout at `path` and applies it to
/// `workspace` — a real, silent no-op (not an error) for anything that
/// keeps this from producing a usable layout (no file yet, unreadable
/// bytes, `postcard` failing to parse), the same "missing/corrupt is a
/// silent fallback, not a failure to start" shape [`recover_document`]
/// already uses. Clamping a stale saved width (e.g. from a since-
/// narrowed window) is `aurora_ui::set_rail_width`'s own job, not
/// repeated here.
fn load_workspace_layout(path: &Path, workspace: &mut aurora_ui::Workspace) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(?err, path = %path.display(), "failed to read the workspace layout");
            }
            return;
        }
    };
    let layout: WorkspaceLayout = match postcard::from_bytes(&bytes) {
        Ok(layout) => layout,
        Err(err) => {
            tracing::warn!(?err, "failed to deserialize the workspace layout");
            return;
        }
    };
    if let Err(err) = aurora_ui::set_rail_width(
        &mut workspace.tree,
        workspace.rail,
        workspace.divider,
        layout.rail_width,
    ) {
        tracing::warn!(?err, "failed to apply the saved rail width");
    }
    for (panel, collapsed) in [
        (workspace.layers, layout.layers_collapsed),
        (workspace.properties, layout.properties_collapsed),
        (workspace.history, layout.history_collapsed),
    ] {
        if let Err(err) = aurora_ui::set_panel_collapsed(&mut workspace.tree, panel, collapsed) {
            tracing::warn!(?err, "failed to apply a saved panel's collapsed state");
        }
    }
}

const CRASH_RECOVERY_CONTINUE: &str = "recovery.continue";

/// The crash-recovery dialog's own, honest content — a single "Continue"
/// action either way (see this section's own doc comment for why there
/// is no separate "Recover Document" choice); only the message differs.
fn crash_recovery_dialog_actions() -> Vec<DialogAction> {
    vec![DialogAction::new(CRASH_RECOVERY_CONTINUE, "Continue")]
}

/// The crash-recovery dialog's message — reports whether [`recover_document`]
/// actually found and replayed a usable autosave, since that's the one
/// thing that changed since the previous (M1.8) version of this dialog.
fn crash_recovery_dialog_message(recovered: bool) -> &'static str {
    if recovered {
        "The previous session didn't shut down cleanly. Its autosaved \
         document was recovered and is now open."
    } else {
        "The previous session didn't shut down cleanly, and no autosaved \
         document could be recovered. This is a fresh document."
    }
}

/// Opens the crash-recovery dialog (a no-op if one is already open):
/// inserts it into `workspace.tree` under `workspace.root` and moves
/// keyboard focus to its first (only, today) action.
fn open_crash_recovery_dialog(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    scales: &Scales,
    recovered: bool,
) {
    if dialog.is_some() {
        return;
    }
    let handle = match insert_dialog(
        &mut workspace.tree,
        workspace.root,
        scales,
        "Aurora Didn't Close Properly",
        crash_recovery_dialog_message(recovered),
        crash_recovery_dialog_actions(),
    ) {
        Ok(handle) => handle,
        Err(err) => {
            tracing::warn!(?err, "failed to open the crash recovery dialog");
            return;
        }
    };
    if let Some(button) = handle.first_action()
        && let Err(err) = focus.focus(&mut workspace.tree, button)
    {
        tracing::warn!(?err, "failed to focus the crash recovery dialog");
    }
    *dialog = Some(handle);
}

/// Closes the crash-recovery dialog (a no-op if none is open): removes
/// it from `workspace.tree` and clears any focus left dangling on it —
/// the same [`FocusManager::validate`] pattern
/// [`close_command_palette`] already uses.
fn close_crash_recovery_dialog(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
) {
    let Some(handle) = dialog.take() else {
        return;
    };
    if let Err(err) = workspace.tree.remove(handle.root) {
        tracing::warn!(?err, "failed to close the crash recovery dialog");
    }
    focus.validate(&workspace.tree);
}

/// Routes one key press while the crash-recovery dialog is open —
/// captures the keyboard directly, the same modal precedence
/// [`handle_palette_key`] uses for the command palette (and, per
/// [`handle_key`]'s own routing order, this dialog takes priority over
/// the palette: a modal alert blocks everything else).
fn handle_dialog_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    chord: KeyChord,
) {
    let Some(handle) = dialog.as_ref() else {
        return;
    };
    match chord.key {
        Key::Named(NamedKey::Escape) => close_crash_recovery_dialog(workspace, focus, dialog),
        Key::Named(NamedKey::Enter) => {
            let action = focus
                .focused()
                .and_then(|id| handle.action_id(id))
                .map(str::to_owned);
            run_dialog_action(workspace, focus, dialog, action);
        }
        _ => {}
    }
}

/// Closes the crash-recovery dialog and, if `action` names one of its
/// own action ids, logs it as chosen — the shared "resolve, then close"
/// step [`handle_dialog_key`]'s own `Enter` case and
/// [`handle_dialog_pointer`]'s own button-click case both need, factored
/// out so there's exactly one place this dialog's actions actually run.
fn run_dialog_action(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    action: Option<String>,
) {
    close_crash_recovery_dialog(workspace, focus, dialog);
    if let Some(action) = action {
        tracing::info!(action, "crash recovery dialog action chosen");
    }
}

/// Routes a real pointer press while the crash-recovery dialog is open
/// — the same modal precedence [`handle_dialog_key`] already gives the
/// keyboard, extended to the pointer now that this crate has real
/// pointer input (PLAN.md M1.9) — previously this dialog's own named,
/// still-open gap (`aurora_widgets::widgets::dialog`'s own doc comment:
/// "no click routing"). A `Primary`-button click on one of the
/// dialog's own action buttons runs it, the same as `Enter` on the
/// focused one; any other click — a different button, or anywhere
/// else, including past the dialog's own edge — is swallowed, not
/// passed through to whatever's underneath, matching a real modal
/// alert's usual behaviour (unlike a popover, it doesn't dismiss on an
/// outside click). Returns whether a dialog was actually open to route
/// to, so [`App::handle_pointer_pressed`] knows whether to fall through
/// to its own, non-modal hit-testing.
fn handle_dialog_pointer(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    button: PointerButton,
    position: (f32, f32),
) -> bool {
    if dialog.is_none() {
        return false;
    }
    if button == PointerButton::Primary {
        let action = dialog.as_ref().and_then(|handle| {
            workspace
                .tree
                .hit_test(position)
                .and_then(|hit| handle.action_id(hit))
        });
        let action = action.map(str::to_owned);
        if action.is_some() {
            run_dialog_action(workspace, focus, dialog, action);
        }
    }
    true
}

// -- Command dispatch: keyboard shortcuts and the command palette --
//
// PLAN.md M1.8's "command palette, keyboard shortcuts" bullet. Every
// function below is deliberately free (not a method on `App`) and
// platform-free (`aurora_widgets`/`aurora_ui` types only, no
// `winit::event_loop`/GPU state) — the same "pure logic, headlessly
// testable" shape `demo_document`/`load_theme` already use,
// so this crate's first real keyboard-input routing doesn't need a live
// window, `EventLoopProxy`, or display server to test (this sandbox has
// none of those — see this crate's own doc comment). `translate_key`/
// `translate_modifiers` are the one seam that does touch `winit` types,
// but only plain data ones (`winit::keyboard::Key`/`ModifiersState`),
// which are constructible with no window either.

/// One command this crate's own keyboard shortcuts and command-palette
/// entries can name. Deliberately small: `FocusNext`/`FocusPrevious` are
/// the first time `aurora_widgets::FocusManager` (built in M1.7, never
/// wired to a real key event until now) actually reaches a keyboard,
/// `ToggleCommandPalette` is the one entry point into the palette
/// itself, `SelectTool` switches `App`'s own active `aurora_ui::Tool`
/// (PLAN.md M1.9's "basic tools" bullet), and `Undo`/`Redo` drive
/// `App`'s own live `aurora_doc::History` (PLAN.md's Undo/Redo bullet).
/// Save is still real, separate follow-on work once this crate has a
/// keyboard-triggerable action for it to invoke — inventing a
/// placeholder command with nothing behind it would be exactly the kind
/// of half-finished feature CLAUDE.md warns against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    FocusNext,
    FocusPrevious,
    ToggleCommandPalette,
    SelectTool(aurora_ui::Tool),
    Undo,
    Redo,
}

/// This build's fixed, checked-in global shortcut bindings. Not (yet)
/// user-configurable — `KeyChord::parse` on a literal string here is
/// exactly the same mechanism a future settings-driven rebind would use,
/// just with the source string checked in rather than read from
/// preferences.
fn default_shortcuts() -> ShortcutRegistry<AppCommand> {
    let bindings = [
        ("Tab", AppCommand::FocusNext),
        ("Shift+Tab", AppCommand::FocusPrevious),
        ("Ctrl+Shift+P", AppCommand::ToggleCommandPalette),
        // Matches Photoshop's/every mainstream editor's own convention
        // (`Ctrl+Z`/`Ctrl+Shift+Z`, not a separate `Ctrl+Y` for redo).
        // Literally `Ctrl`, even on macOS -- this registry has no
        // per-platform rebinding yet, so a macOS user's own muscle-memory
        // `Cmd+Z` doesn't resolve; a real, named gap, not silently
        // missing.
        ("Ctrl+Z", AppCommand::Undo),
        ("Ctrl+Shift+Z", AppCommand::Redo),
        // Tool-switch letters match Photoshop's own single-key bindings
        // (no modifier) -- the same convention this project's target
        // users already carry in muscle memory. Every tool here does
        // something real once selected now.
        ("v", AppCommand::SelectTool(aurora_ui::Tool::Move)),
        ("m", AppCommand::SelectTool(aurora_ui::Tool::MarqueeSelect)),
        ("z", AppCommand::SelectTool(aurora_ui::Tool::Zoom)),
        ("h", AppCommand::SelectTool(aurora_ui::Tool::Pan)),
        ("i", AppCommand::SelectTool(aurora_ui::Tool::Eyedropper)),
        ("b", AppCommand::SelectTool(aurora_ui::Tool::Brush)),
        ("e", AppCommand::SelectTool(aurora_ui::Tool::Eraser)),
    ];
    let mut registry = ShortcutRegistry::new();
    for (source, command) in bindings {
        let chord = match KeyChord::parse(source) {
            Ok(chord) => chord,
            Err(err) => unreachable!("{source:?} is a fixed, checked-in shortcut string: {err:?}"),
        };
        if let Err(err) = registry.bind(chord, command) {
            unreachable!("fixed, checked-in shortcuts don't collide with each other: {err:?}");
        }
    }
    registry
}

// -- Native platform access: clipboard and file dialogs --
//
// PLAN.md M1.8's "clipboard"/"file dialogs" bullets — PRD §8.3's own
// pre-decided choices (`rfd` for native dialogs; `arboard`, text-only,
// for the system clipboard — image support is a real, separate need
// this crate has none of yet). Both are real, synchronous platform
// calls with no meaningful headless behaviour (no display server, no
// clipboard owner in this sandbox), so — the same "keep the pure
// dispatch logic testable, isolate the untestable platform call behind
// a small seam" shape `translate_key`/`translate_modifiers` already
// established for keyboard input — [`handle_palette_key`] takes these
// as `&mut dyn` trait objects rather than calling `arboard`/`rfd`
// directly, so a test can inject a fake and never touch the real OS.

/// Whatever the caller uses to read/write the system clipboard —
/// [`SystemClipboard`] in real use, a fake in tests.
trait ClipboardAccess {
    fn get_text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: String);
}

/// The real system clipboard, via `arboard`. Construction can fail (no
/// clipboard owner on this platform/session — exactly this sandbox's
/// own situation, no display server at all), so this lazily retries
/// once per process: if `arboard::Clipboard::new()` fails, that failure
/// is logged once and remembered (`unavailable`), not retried on every
/// keystroke.
struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
    unavailable: bool,
}

impl SystemClipboard {
    fn new() -> Self {
        Self {
            inner: None,
            unavailable: false,
        }
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() && !self.unavailable {
            match arboard::Clipboard::new() {
                Ok(clipboard) => self.inner = Some(clipboard),
                Err(err) => {
                    tracing::warn!(?err, "system clipboard unavailable");
                    self.unavailable = true;
                }
            }
        }
        self.inner.as_mut()
    }
}

impl ClipboardAccess for SystemClipboard {
    fn get_text(&mut self) -> Option<String> {
        match self.clipboard()?.get_text() {
            Ok(text) => Some(text),
            Err(err) => {
                tracing::warn!(?err, "failed to read the system clipboard");
                None
            }
        }
    }

    fn set_text(&mut self, text: String) {
        if let Some(clipboard) = self.clipboard()
            && let Err(err) = clipboard.set_text(text)
        {
            tracing::warn!(?err, "failed to write to the system clipboard");
        }
    }
}

/// Whatever the caller uses to show a native "open file"/"save file"
/// dialog — [`SystemFileDialog`] in real use, a fake in tests.
trait FileDialogAccess {
    fn pick_file(&mut self) -> Option<PathBuf>;
    fn save_file(&mut self) -> Option<PathBuf>;
}

/// The real native file picker, via `rfd`. Synchronous — the call
/// blocks this thread until the user picks a file or cancels, which is
/// how every desktop platform's own native modal file dialog behaves;
/// there is no async runtime in this crate to hand it off to.
struct SystemFileDialog;

impl FileDialogAccess for SystemFileDialog {
    fn pick_file(&mut self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_file()
    }

    fn save_file(&mut self) -> Option<PathBuf> {
        rfd::FileDialog::new().save_file()
    }
}

/// One real outcome of activating a command-palette entry (or, on
/// macOS, a native menu item) that [`activate_command`] itself can't
/// finish — either because it needs a real platform call this crate
/// keeps behind a testable seam (`Open`/`Save`, a real native
/// `rfd::FileDialog`'s own picked path — [`App::open_file`]/
/// [`App::save_file`] still need to read/decode or encode/write it),
/// or because it needs live document state `activate_command` is
/// deliberately kept free of (`Undo`/`Redo` — [`App::handle_key_event`]/
/// [`App::handle_menu_event`] run them via the same [`run_command`]
/// path `Ctrl+Z`/`Ctrl+Shift+Z` already use, so there is exactly one
/// place either command's own logic lives, not a second copy here).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivatedCommand {
    OpenFile(PathBuf),
    SaveFile(PathBuf),
    Undo,
    Redo,
}

/// Command ids [`palette_commands`] emits — `aurora_widgets::widgets::
/// CommandEntry::id` is opaque to `aurora-widgets` itself (see that
/// module's own doc comment); these constants are where this crate
/// gives its own ids meaning, matched in [`command_target`].
const COMMAND_FOCUS_LAYERS: &str = "view.focus_layers";
const COMMAND_FOCUS_PROPERTIES: &str = "view.focus_properties";
const COMMAND_FOCUS_HISTORY: &str = "view.focus_history";
const COMMAND_TOGGLE_LAYERS: &str = "view.toggle_layers";
const COMMAND_TOGGLE_PROPERTIES: &str = "view.toggle_properties";
const COMMAND_TOGGLE_HISTORY: &str = "view.toggle_history";
const COMMAND_CLOSE_LAYERS: &str = "view.close_layers";
const COMMAND_CLOSE_PROPERTIES: &str = "view.close_properties";
const COMMAND_CLOSE_HISTORY: &str = "view.close_history";
const COMMAND_FILE_OPEN: &str = "file.open";
const COMMAND_FILE_SAVE: &str = "file.save";
const COMMAND_UNDO: &str = "edit.undo";
const COMMAND_REDO: &str = "edit.redo";

/// The command palette's own, real content: one command per docked
/// panel, focusing it; one more per panel, toggling its own
/// collapsed/expanded state (`aurora_ui::set_panel_collapsed`); one
/// more still, closing it outright (`aurora_ui::close_panel` — collapse
/// plus really freeing its current content, PLAN.md M1.8's docking
/// bullet, Cahya's own scoping call: close reuses collapse's layout
/// mechanism rather than making `Workspace`'s own panel fields
/// optional, since nothing renders yet to make that extra honesty
/// worth the ripple through every place this crate already assumes a
/// docked panel exists); real native "Open File…"/"Save As…" pickers;
/// and `Undo`/`Redo`, the same commands `Ctrl+Z`/`Ctrl+Shift+Z` already
/// run — previously shortcut-only, a real, named gap (a screen-reader
/// user driving this crate through the palette had no way to reach
/// either one). Genuine, not placeholder — each focus command moves
/// real keyboard focus to a real, already-focusable panel region (see
/// `aurora-ui`'s `insert_panel`), verifiable the same way any other
/// focus change is (`push_accessibility`); each toggle command really
/// collapses/expands its panel (`command_collapse_target`); each close
/// command really collapses *and* empties its panel's body
/// (`command_close_target`) — reopening it (the same toggle command)
/// shows it empty until the next real document-state change
/// repopulates it, `close_panel`'s own doc comment names this
/// honestly. Command titles don't reflect current state (a palette
/// listing "Hide Layers Panel" vs "Show Layers Panel" would need the
/// list rebuilt on every state change — real, separate follow-on work,
/// fixed "Toggle"/"Close" labels are the honest baseline this pass
/// lands); `COMMAND_FILE_OPEN`/`COMMAND_FILE_SAVE` show a real, native
/// `rfd::FileDialog` and the chosen path is really opened/saved
/// (`App::open_file`/`save_file`); `COMMAND_UNDO`/`COMMAND_REDO` really
/// undo/redo (see [`ActivatedCommand`]'s own doc comment for why
/// `activate_command` itself doesn't run them directly). "Save As…",
/// not "Save" — this crate tracks no "current document path" to reuse
/// yet, so every save shows a picker.
fn palette_commands() -> Vec<CommandEntry> {
    vec![
        CommandEntry::new(COMMAND_FOCUS_LAYERS, "Focus Layers Panel"),
        CommandEntry::new(COMMAND_FOCUS_PROPERTIES, "Focus Properties Panel"),
        CommandEntry::new(COMMAND_FOCUS_HISTORY, "Focus History Panel"),
        CommandEntry::new(COMMAND_TOGGLE_LAYERS, "Toggle Layers Panel"),
        CommandEntry::new(COMMAND_TOGGLE_PROPERTIES, "Toggle Properties Panel"),
        CommandEntry::new(COMMAND_TOGGLE_HISTORY, "Toggle History Panel"),
        CommandEntry::new(COMMAND_CLOSE_LAYERS, "Close Layers Panel"),
        CommandEntry::new(COMMAND_CLOSE_PROPERTIES, "Close Properties Panel"),
        CommandEntry::new(COMMAND_CLOSE_HISTORY, "Close History Panel"),
        CommandEntry::new(COMMAND_FILE_OPEN, "Open File…"),
        CommandEntry::new(COMMAND_FILE_SAVE, "Save As…"),
        CommandEntry::new(COMMAND_UNDO, "Undo"),
        CommandEntry::new(COMMAND_REDO, "Redo"),
    ]
}

/// Resolves an activated command-palette entry's own `id` (one of the
/// `COMMAND_*` constants above) to the widget it should focus. `None`
/// for an id this build doesn't recognise, including `COMMAND_FILE_OPEN`/
/// `COMMAND_FILE_SAVE`/`COMMAND_UNDO`/`COMMAND_REDO` — none of those
/// focus anything, they're handled directly in [`activate_command`].
fn command_target(workspace: &aurora_ui::Workspace, id: &str) -> Option<WidgetId> {
    match id {
        COMMAND_FOCUS_LAYERS => Some(workspace.layers.root),
        COMMAND_FOCUS_PROPERTIES => Some(workspace.properties.root),
        COMMAND_FOCUS_HISTORY => Some(workspace.history.root),
        _ => None,
    }
}

/// Resolves an activated command-palette entry's own `id` to the panel
/// it should collapse/expand — the toggle-command counterpart to
/// [`command_target`]'s own focus-command resolution.
fn command_collapse_target(
    workspace: &aurora_ui::Workspace,
    id: &str,
) -> Option<aurora_ui::PanelHandle> {
    match id {
        COMMAND_TOGGLE_LAYERS => Some(workspace.layers),
        COMMAND_TOGGLE_PROPERTIES => Some(workspace.properties),
        COMMAND_TOGGLE_HISTORY => Some(workspace.history),
        _ => None,
    }
}

/// Resolves an activated command-palette entry's own `id` to the panel
/// it should close — the close-command counterpart to
/// [`command_collapse_target`]'s own toggle-command resolution.
fn command_close_target(
    workspace: &aurora_ui::Workspace,
    id: &str,
) -> Option<aurora_ui::PanelHandle> {
    match id {
        COMMAND_CLOSE_LAYERS => Some(workspace.layers),
        COMMAND_CLOSE_PROPERTIES => Some(workspace.properties),
        COMMAND_CLOSE_HISTORY => Some(workspace.history),
        _ => None,
    }
}

/// Activates a command by its own opaque id — shared by the command
/// palette's `Enter` key and, on macOS, the native menu bar
/// (`App::handle_menu_event`): the same underlying action, reachable
/// from two different UI surfaces, rather than two parallel
/// implementations of "what does this command do." Moves focus for a
/// panel-focus command ([`command_target`]); flips collapsed/expanded
/// for a panel-toggle command ([`command_collapse_target`]); closes a
/// panel outright for a panel-close command ([`command_close_target`]);
/// shows the native file dialog and returns the picked path, tagged by
/// which one it was ([`ActivatedCommand`]), for [`COMMAND_FILE_OPEN`]/
/// [`COMMAND_FILE_SAVE`]; resolves `COMMAND_UNDO`/`COMMAND_REDO` to
/// their own [`ActivatedCommand`] variant without running them here —
/// this function is deliberately kept free of `layers`/`history`/
/// `pixel_history`/`aurora_tile::TileStore`, so it stays exactly as
/// pure and unit-testable as it already was; the caller
/// ([`App::handle_key_event`]/[`App::handle_menu_event`], both of
/// which already own that state) runs the real undo/redo via
/// [`run_command`], the same path `Ctrl+Z`/`Ctrl+Shift+Z` themselves
/// use. Logs and returns `None` for any other id.
fn activate_command(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    id: &str,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<ActivatedCommand> {
    if let Some(target) = command_target(workspace, id) {
        if let Err(err) = focus.focus(&mut workspace.tree, target) {
            tracing::warn!(?err, "activated command's target isn't focusable");
        }
        return None;
    }
    if let Some(panel) = command_collapse_target(workspace, id) {
        let collapsed = aurora_ui::panel_is_collapsed(&workspace.tree, panel).unwrap_or(false);
        if let Err(err) = aurora_ui::set_panel_collapsed(&mut workspace.tree, panel, !collapsed) {
            tracing::warn!(?err, "failed to toggle panel collapse");
        }
        return None;
    }
    if let Some(panel) = command_close_target(workspace, id) {
        if let Err(err) = aurora_ui::close_panel(&mut workspace.tree, panel) {
            tracing::warn!(?err, "failed to close panel");
        }
        return None;
    }
    if id == COMMAND_FILE_OPEN {
        return file_dialog.pick_file().map(ActivatedCommand::OpenFile);
    }
    if id == COMMAND_FILE_SAVE {
        return file_dialog.save_file().map(ActivatedCommand::SaveFile);
    }
    if id == COMMAND_UNDO {
        return Some(ActivatedCommand::Undo);
    }
    if id == COMMAND_REDO {
        return Some(ActivatedCommand::Redo);
    }
    tracing::warn!(command = id, "unknown command activated");
    None
}

// -- Native menu bar (macOS only) --
//
// PLAN.md M1.8's "native menus" bullet, scoped to macOS per PRD §8.3/
// §14's own wording ("Native menu bar (macOS)..." — Windows/Linux
// aren't named there for the menu bar specifically). On inspection
// neither of the other two is a good fit for `muda` right now: Windows
// would need a separate, real `unsafe`-code decision (a raw `HWND` via
// `muda::Menu::init_for_hwnd`); Linux's only muda backend needs a real
// `gtk::Window`, which this project's plain `winit`-created X11/Wayland
// window structurally never is (and muda doesn't even compile on Linux
// without the heavy `gtk` feature, for a backend that couldn't attach
// to anything anyway). Both left for whenever Aurora draws its own
// in-window menu (`aurora-vector`, still an empty skeleton) — the more
// natural cross-platform answer this project's own "we own our UI"
// architecture already points toward, not a native OS integration on
// every platform.

/// Builds the native menu's own cross-platform structure: an app menu
/// (About/Services/Hide/Quit — see this function's own doc comment
/// below for why this exists at all), File > Open File…/Save As…,
/// Edit > Undo/Redo, View > Focus Layers/Properties/History Panel, then
/// (past a separator) Toggle Layers/Properties/History Panel, then
/// (past another) Close Layers/Properties/History Panel — every one of
/// those ten reusing the exact same `COMMAND_*` ids the
/// command palette already uses (via `MenuItem::with_id`), so
/// [`activate_command`] drives both UI surfaces identically; nothing
/// here invents a second command vocabulary. No accelerator hint on
/// Undo/Redo (the trailing `None`) — this crate's own keyboard
/// shortcuts bind literal `Ctrl+Z`/`Ctrl+Shift+Z` even on macOS (no
/// per-platform rebinding exists yet, `default_shortcuts`'s own doc
/// comment), and showing a `⌘Z` hint the app doesn't actually respond
/// to would be actively misleading. Building the model (as opposed to
/// attaching it to a window) is the same on every platform muda
/// supports, which is why this function itself needs no further
/// `#[cfg]` beyond the module section's own macOS gate.
///
/// **The app menu's own real, load-bearing purpose**: on macOS, Cocoa
/// always overwrites the *first* top-level menu's own displayed title
/// with the running application's own process name, regardless of what
/// string it was actually given — a real, first-hand finding (Cahya
/// running this on real macOS hardware saw exactly three menus, "aurora-
/// app / Edit / View," with `File` seemingly missing). Without a
/// dedicated first item, Cocoa's own auto-rename lands on `File`
/// itself, silently relabelling it rather than leaving it out — the
/// menu isn't missing, it's wearing the wrong name. A real app menu
/// here (the conventional macOS slot for About/Services/Hide/Quit, via
/// `PredefinedMenuItem` rather than any `COMMAND_*` id this crate
/// invented — there is no document-level "quit" command to route
/// through `activate_command`, since quitting isn't a document
/// operation) gives Cocoa's rename a proper target and leaves `File`
/// alone.
#[cfg(target_os = "macos")]
fn build_menu() -> muda::Menu {
    let menu = muda::Menu::new();

    // Title is irrelevant -- Cocoa overwrites it with the process name
    // regardless, per this function's own doc comment above; this
    // submenu's only job is to occupy that slot so `File` doesn't.
    let about = muda::AboutMetadata {
        name: Some("Aurora".to_owned()),
        ..Default::default()
    };
    let app_menu = match muda::Submenu::with_items(
        "Aurora",
        true,
        &[
            &muda::PredefinedMenuItem::about(None, Some(about)),
            &muda::PredefinedMenuItem::separator(),
            &muda::PredefinedMenuItem::services(None),
            &muda::PredefinedMenuItem::separator(),
            &muda::PredefinedMenuItem::hide(None),
            &muda::PredefinedMenuItem::hide_others(None),
            &muda::PredefinedMenuItem::show_all(None),
            &muda::PredefinedMenuItem::separator(),
            &muda::PredefinedMenuItem::quit(None),
        ],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("freshly built items cannot fail to append: {err:?}"),
    };

    let file_menu = match muda::Submenu::with_items(
        "File",
        true,
        &[
            &muda::MenuItem::with_id(COMMAND_FILE_OPEN, "Open File…", true, None),
            &muda::MenuItem::with_id(COMMAND_FILE_SAVE, "Save As…", true, None),
        ],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("freshly built items cannot fail to append: {err:?}"),
    };

    let edit_menu = match muda::Submenu::with_items(
        "Edit",
        true,
        &[
            &muda::MenuItem::with_id(COMMAND_UNDO, "Undo", true, None),
            &muda::MenuItem::with_id(COMMAND_REDO, "Redo", true, None),
        ],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("freshly built items cannot fail to append: {err:?}"),
    };

    let view_menu = match muda::Submenu::with_items(
        "View",
        true,
        &[
            &muda::MenuItem::with_id(COMMAND_FOCUS_LAYERS, "Focus Layers Panel", true, None),
            &muda::MenuItem::with_id(
                COMMAND_FOCUS_PROPERTIES,
                "Focus Properties Panel",
                true,
                None,
            ),
            &muda::MenuItem::with_id(COMMAND_FOCUS_HISTORY, "Focus History Panel", true, None),
            &muda::PredefinedMenuItem::separator(),
            &muda::MenuItem::with_id(COMMAND_TOGGLE_LAYERS, "Toggle Layers Panel", true, None),
            &muda::MenuItem::with_id(
                COMMAND_TOGGLE_PROPERTIES,
                "Toggle Properties Panel",
                true,
                None,
            ),
            &muda::MenuItem::with_id(COMMAND_TOGGLE_HISTORY, "Toggle History Panel", true, None),
            &muda::PredefinedMenuItem::separator(),
            &muda::MenuItem::with_id(COMMAND_CLOSE_LAYERS, "Close Layers Panel", true, None),
            &muda::MenuItem::with_id(
                COMMAND_CLOSE_PROPERTIES,
                "Close Properties Panel",
                true,
                None,
            ),
            &muda::MenuItem::with_id(COMMAND_CLOSE_HISTORY, "Close History Panel", true, None),
        ],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("freshly built items cannot fail to append: {err:?}"),
    };

    if let Err(err) = menu.append_items(&[&app_menu, &file_menu, &edit_menu, &view_menu]) {
        tracing::warn!(?err, "failed to build the native menu bar structure");
    }
    menu
}

/// The command palette's own real size and position within the window —
/// `Position::Absolute` so it floats above `workspace.root`'s own
/// canvas/divider/rail row instead of competing with them for space in
/// it (`insert_command_palette` itself gives the palette's root only
/// `Style::default()`, deliberately: the toolkit crate has no opinion
/// on where its caller wants to place a popover).
///
/// **A real bug, found by Cahya actually trying `Ctrl+Shift+P` in the
/// running app and seeing nothing.** Root cause: nothing in this crate
/// had ever applied a real style to the palette's own root, so in a
/// live window it resolved to 0×0 layout bounds — the same "an empty
/// leaf gets zero size without an explicit style" issue this crate's
/// own gallery harness already hit and fixed for the palette's *body*/
/// *rows* (`aurora_widgets::widgets::command_palette`'s own
/// `body_style`/`row_style`), except nothing had ever fixed it for the
/// palette's own *root* outside that test file — `tests/gallery.rs`'s
/// own `command_palette_style` is private to that file, so the real
/// app never inherited the fix.
///
/// Fixed pixel width/height/top-inset, not a design token: no
/// "command palette dimensions" token exists in `design/tokens/
/// scales.toml`, and inventing one ad hoc is a design decision to
/// raise, not a gap to fill locally — the same reasoning
/// `aurora_ui::workspace`'s own `RAIL_MIN_WIDTH`/`RAIL_MAX_WIDTH`
/// engineering defaults already established. Horizontally centred via
/// `inset.left = inset.right = length(0.0)` plus
/// `margin.left = margin.right = auto()`, with a definite `width` also
/// set — the standard CSS absolute-position centring combination,
/// confirmed here by a real headless layout test rather than assumed
/// to work.
fn command_palette_style() -> taffy::Style {
    const WIDTH: f32 = 480.0;
    const HEIGHT: f32 = 320.0;
    const TOP_INSET: f32 = 96.0;

    taffy::Style {
        position: taffy::Position::Absolute,
        size: taffy::Size {
            width: taffy::style_helpers::length(WIDTH),
            height: taffy::style_helpers::length(HEIGHT),
        },
        inset: taffy::Rect {
            top: taffy::style_helpers::length(TOP_INSET),
            left: taffy::style_helpers::length(0.0_f32),
            right: taffy::style_helpers::length(0.0_f32),
            bottom: taffy::style_helpers::auto(),
        },
        margin: taffy::Rect {
            left: taffy::style_helpers::auto(),
            right: taffy::style_helpers::auto(),
            top: taffy::style_helpers::length(0.0_f32),
            bottom: taffy::style_helpers::length(0.0_f32),
        },
        ..Default::default()
    }
}

/// Opens the command palette (a no-op if one is already open): inserts
/// it into `workspace.tree` under `workspace.root` with
/// [`palette_commands`]'s own list, sizes and positions it
/// ([`command_palette_style`] — see that function's own doc comment for
/// why this step is real, not decorative), then moves keyboard focus to
/// it. The caller still needs to re-run `WidgetTree::compute_layout`
/// afterward for the new style to actually reach `WidgetTree::bounds`
/// ([`App::handle_key_event`] does, right after the `Ctrl+Shift+P` that
/// reaches this).
fn open_command_palette(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
) {
    if palette.is_some() {
        return;
    }
    let root = match insert_command_palette(&mut workspace.tree, workspace.root, palette_commands())
    {
        Ok(root) => root,
        Err(err) => {
            tracing::warn!(?err, "failed to open the command palette");
            return;
        }
    };
    if let Err(err) = workspace.tree.set_style(root, command_palette_style()) {
        tracing::warn!(?err, "failed to size the newly opened command palette");
    }
    if let Err(err) = focus.focus(&mut workspace.tree, root) {
        tracing::warn!(?err, "failed to focus the newly opened command palette");
    }
    *palette = Some(root);
}

/// Closes the command palette (a no-op if none is open): removes it
/// from `workspace.tree` and clears any focus left dangling on it —
/// [`FocusManager::validate`] is exactly the "focus target was removed
/// out from under the manager" case that method's own doc comment
/// describes.
fn close_command_palette(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
) {
    let Some(root) = palette.take() else {
        return;
    };
    if let Err(err) = workspace.tree.remove(root) {
        tracing::warn!(?err, "failed to close the command palette");
    }
    focus.validate(&workspace.tree);
}

fn toggle_command_palette(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
) {
    if palette.is_some() {
        close_command_palette(workspace, focus, palette);
    } else {
        open_command_palette(workspace, focus, palette);
    }
}

/// Which backing stack a completed edit actually lives in — the tag
/// [`UndoOrder`] itself is built from. See that type's own doc comment
/// for why this indirection exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UndoKind {
    Structural,
    Pixel,
}

/// The one true chronological order `Ctrl+Z`/`Ctrl+Shift+Z` walk across
/// `aurora_doc::History`'s own structural entries (so far, just Move)
/// and `aurora_brush::PixelHistory`'s own stroke entries — what makes
/// two otherwise fully independent stacks undo/redo as a single
/// sequence. Neither backing type knows about the other (PRD §7.2's own
/// layering: `aurora-brush` and `aurora-doc` are sibling crates,
/// neither depending on the other), so `aurora-app`, which depends on
/// both, is where the interleaving has to live. This records only
/// *which* stack a completed edit belongs to, in the order it actually
/// happened — the edit's own content (the `LayerOp`, the
/// `StrokeSnapshot`) stays exactly where it already lived, in that
/// backing store's own stack; undoing/redoing here means popping a tag
/// from `self` and asking the matching backing store to actually do it.
#[derive(Debug, Default)]
struct UndoOrder {
    undo: Vec<UndoKind>,
    redo: Vec<UndoKind>,
}

impl UndoOrder {
    /// Records that an edit of `kind` was just committed to its own
    /// backing store: pushes it onto the unified undo order and
    /// invalidates every pending redo — in this order's own
    /// bookkeeping *and* in both backing stores' own internal redo
    /// stacks (`history.clear_redo`/`pixel_history.clear_redo`), so
    /// neither can report a redo the unified order itself has already
    /// discarded. The backing store `kind` itself just recorded through
    /// already cleared its own redo stack as a side effect of being
    /// recorded through (`History`'s/`PixelHistory`'s own `push`-style
    /// methods) — clearing it again here is a harmless no-op; the
    /// *other* store's clear is the one that actually matters.
    fn record(
        &mut self,
        kind: UndoKind,
        history: &mut aurora_doc::History,
        pixel_history: &mut aurora_brush::PixelHistory,
    ) {
        self.undo.push(kind);
        self.redo.clear();
        history.clear_redo();
        pixel_history.clear_redo();
    }
}

/// Runs a global shortcut's own command — [`handle_key`]'s dispatch
/// target once a [`KeyChord`] resolves via [`ShortcutRegistry::resolve`].
///
/// `Undo`/`Redo` consult [`UndoOrder`] first to find out *which*
/// backing store's own top entry is actually next, then delegate to
/// that store (`history.undo`/`pixel_history.undo`, or the `redo`
/// equivalents) to do the real work — see `UndoOrder`'s own doc comment
/// for why this indirection exists. A pixel undo/redo with no live
/// `TileStore` is logged and left exactly as it was (the popped-but-
/// not-yet-removed order entry is never removed, so nothing desyncs);
/// either path failing for its own reason (an unknown layer id mixed in
/// from outside `History`; a real `TileError` restoring a captured
/// tile) is logged too, the same "a bad input mustn't crash the event
/// loop" shape every other handler in this section already follows. A
/// structural undo/redo refreshes the History panel afterward
/// ([`refresh_history_panel`]) since undoing/redoing is itself a
/// journaled step; a pixel undo/redo has no such panel to refresh — the
/// canvas alone shows the result, on the next redraw.
#[allow(clippy::too_many_arguments)]
fn run_command(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    tool: &mut aurora_ui::Tool,
    layers: &mut aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    store: Option<&mut aurora_tile::TileStore>,
    undo_order: &mut UndoOrder,
    command: AppCommand,
) {
    match command {
        AppCommand::FocusNext => {
            focus.focus_next(&mut workspace.tree);
        }
        AppCommand::FocusPrevious => {
            focus.focus_previous(&mut workspace.tree);
        }
        AppCommand::ToggleCommandPalette => toggle_command_palette(workspace, focus, palette),
        AppCommand::SelectTool(selected) => {
            *tool = selected;
            refresh_properties_panel(workspace, selected);
        }
        AppCommand::Undo => match undo_order.undo.last().copied() {
            Some(UndoKind::Structural) => match history.undo(layers) {
                Ok(_) => {
                    undo_order.undo.pop();
                    undo_order.redo.push(UndoKind::Structural);
                    refresh_history_panel(workspace, history);
                }
                Err(err) => tracing::warn!(?err, "undo failed"),
            },
            Some(UndoKind::Pixel) => {
                if let Some(store) = store {
                    match pixel_history.undo(store) {
                        Ok(true) => {
                            undo_order.undo.pop();
                            undo_order.redo.push(UndoKind::Pixel);
                        }
                        Ok(false) => {
                            tracing::warn!("pixel history had nothing to undo, unexpectedly");
                        }
                        Err(err) => tracing::warn!(?err, "pixel undo failed"),
                    }
                } else {
                    tracing::warn!("no live tile store; cannot undo a pixel edit");
                }
            }
            None => {}
        },
        AppCommand::Redo => match undo_order.redo.last().copied() {
            Some(UndoKind::Structural) => match history.redo(layers) {
                Ok(_) => {
                    undo_order.redo.pop();
                    undo_order.undo.push(UndoKind::Structural);
                    refresh_history_panel(workspace, history);
                }
                Err(err) => tracing::warn!(?err, "redo failed"),
            },
            Some(UndoKind::Pixel) => {
                if let Some(store) = store {
                    match pixel_history.redo(store) {
                        Ok(true) => {
                            undo_order.redo.pop();
                            undo_order.undo.push(UndoKind::Pixel);
                        }
                        Ok(false) => {
                            tracing::warn!("pixel history had nothing to redo, unexpectedly");
                        }
                        Err(err) => tracing::warn!(?err, "pixel redo failed"),
                    }
                } else {
                    tracing::warn!("no live tile store; cannot redo a pixel edit");
                }
            }
            None => {}
        },
    }
}

/// Clears and repopulates the History panel from `history`'s own
/// current journal — [`AppCommand::Undo`]/[`AppCommand::Redo`]'s own
/// shared refresh step, the same `clear_panel_body` + `populate_*`
/// pattern [`replace_document`] already uses for a freshly opened
/// document, just for one panel instead of two.
fn refresh_history_panel(workspace: &mut aurora_ui::Workspace, history: &aurora_doc::History) {
    if let Err(err) = aurora_ui::clear_panel_body(&mut workspace.tree, workspace.history.body) {
        tracing::warn!(
            ?err,
            "failed to clear the History panel before refreshing it"
        );
        return;
    }
    if let Err(err) =
        aurora_ui::populate_history_panel(&mut workspace.tree, workspace.history, history)
    {
        tracing::warn!(?err, "failed to repopulate the History panel");
    }
}

/// The real, non-hardcoded label/value pairs [`aurora_ui::
/// populate_properties_panel`] shows for `tool` — this crate's own
/// per-tool parameters, not `aurora-ui`'s (that crate carries no
/// Brush/Eraser-specific knowledge at all, see
/// `aurora_ui::properties_panel`'s own doc comment). Only [`Tool::Brush`]
/// and [`Tool::Eraser`] have a real parameter today ([`BRUSH_RADIUS`]/
/// [`ERASER_RADIUS`]); every other tool (`Move`, `MarqueeSelect`, `Zoom`,
/// `Pan`, `Eyedropper`) has no real backing data anywhere in this crate
/// yet, so it gets an honest empty list rather than an invented option —
/// the same "nothing to show" pattern the command palette already uses
/// for an unselected row.
#[allow(clippy::match_same_arms)]
fn tool_options(tool: aurora_ui::Tool) -> Vec<(&'static str, String)> {
    match tool {
        aurora_ui::Tool::Brush => vec![("Radius", format!("{BRUSH_RADIUS}px"))],
        aurora_ui::Tool::Eraser => vec![("Radius", format!("{ERASER_RADIUS}px"))],
        aurora_ui::Tool::Move
        | aurora_ui::Tool::MarqueeSelect
        | aurora_ui::Tool::Zoom
        | aurora_ui::Tool::Pan
        | aurora_ui::Tool::Eyedropper => vec![],
    }
}

/// Clears and repopulates the Properties panel for `tool` — the same
/// `clear_panel_body` + `populate_*` pattern [`refresh_history_panel`]
/// already uses, just for the Properties panel and [`tool_options`]
/// instead of a `History` journal. [`AppCommand::SelectTool`]'s own
/// refresh step: clearing first matters here specifically, since without
/// it switching from a tool with real options (Brush) to one without
/// (Move) would leave the previous tool's stale rows sitting in the
/// panel instead of a real empty state.
fn refresh_properties_panel(workspace: &mut aurora_ui::Workspace, tool: aurora_ui::Tool) {
    if let Err(err) = aurora_ui::clear_panel_body(&mut workspace.tree, workspace.properties.body) {
        tracing::warn!(
            ?err,
            "failed to clear the Properties panel before refreshing it"
        );
        return;
    }
    let options = tool_options(tool);
    if let Err(err) = aurora_ui::populate_properties_panel(
        &mut workspace.tree,
        workspace.properties,
        tool,
        &options,
    ) {
        tracing::warn!(?err, "failed to repopulate the Properties panel");
    }
}

/// Routes one key press while the command palette is open — captures
/// input directly rather than going through [`ShortcutRegistry`], the
/// same "a modal dialog owns the keyboard while open" behaviour every
/// mainstream command palette (VS Code, Sublime) uses. A no-op if
/// `palette` is `None` (defensive; [`handle_key`] only calls this when
/// it's `Some`).
/// Routes one key press while the command palette is open. Returns
/// `Some(ActivatedCommand)` when `Enter` just activated a command
/// [`activate_command`] itself couldn't fully resolve — a real file
/// path picked for [`COMMAND_FILE_OPEN`]/[`COMMAND_FILE_SAVE`], or
/// `COMMAND_UNDO`/`COMMAND_REDO` — hands it back up to
/// [`handle_key`]/`App` instead, which has the live document state (or,
/// for a file, the read/decode step) `activate_command` deliberately
/// doesn't. Every other case returns `None`.
#[allow(clippy::too_many_arguments)]
fn handle_palette_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    chord: KeyChord,
    text: Option<&str>,
    clipboard: &mut dyn ClipboardAccess,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<ActivatedCommand> {
    let root = (*palette)?;
    match chord.key {
        Key::Named(NamedKey::Escape) => close_command_palette(workspace, focus, palette),
        Key::Named(NamedKey::ArrowDown) => {
            if let Err(err) = move_command_palette_selection(&mut workspace.tree, root, true) {
                tracing::warn!(?err, "failed to move command palette selection");
            }
        }
        Key::Named(NamedKey::ArrowUp) => {
            if let Err(err) = move_command_palette_selection(&mut workspace.tree, root, false) {
                tracing::warn!(?err, "failed to move command palette selection");
            }
        }
        Key::Named(NamedKey::Enter) => {
            let selected = command_palette_state(&workspace.tree, root)
                .ok()
                .and_then(|state| state.selected())
                .map(|entry| entry.id.clone());
            close_command_palette(workspace, focus, palette);
            let id = selected?;
            return activate_command(workspace, focus, &id, file_dialog);
        }
        Key::Named(NamedKey::Backspace) => {
            if let Ok(state) = command_palette_state(&workspace.tree, root) {
                let mut query = state.query().to_owned();
                query.pop();
                if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
                    tracing::warn!(?err, "failed to update command palette query");
                }
            }
        }
        // `Ctrl+C`/`Ctrl+V` against the real system clipboard. Paste
        // appends at the query's own end, matching how typing a plain
        // character already works below -- this palette has no cursor
        // position of its own to insert at.
        Key::Character('c') if chord.modifiers.control => {
            if let Ok(state) = command_palette_state(&workspace.tree, root) {
                clipboard.set_text(state.query().to_owned());
            }
        }
        Key::Character('v') if chord.modifiers.control => {
            if let (Ok(state), Some(pasted)) = (
                command_palette_state(&workspace.tree, root),
                clipboard.get_text(),
            ) {
                let query = format!("{}{pasted}", state.query());
                if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
                    tracing::warn!(?err, "failed to update command palette query");
                }
            }
        }
        // A plain, unmodified character types into the query. A
        // `Ctrl`/`Alt`/`Cmd`-held character is presumably some other
        // shortcut attempt, not text -- left unhandled rather than
        // typed literally, the same restraint a real text field would
        // apply.
        Key::Character(_)
            if !chord.modifiers.control && !chord.modifiers.alt && !chord.modifiers.meta =>
        {
            if let (Ok(state), Some(text)) = (command_palette_state(&workspace.tree, root), text) {
                let query = format!("{}{text}", state.query());
                if let Err(err) = set_command_palette_query(&mut workspace.tree, root, &query) {
                    tracing::warn!(?err, "failed to update command palette query");
                }
            }
        }
        _ => {}
    }
    None
}

/// One key press's full routing, most-modal-first: the crash-recovery
/// dialog owns the keyboard while open ([`handle_dialog_key`]) — a modal
/// alert blocks everything else, including the palette; otherwise the
/// command palette owns it while open ([`handle_palette_key`]);
/// otherwise a chord that resolves in `shortcuts` runs its command
/// ([`run_command`], which is also where a tool-switch shortcut updates
/// `tool` and `Ctrl+Z`/`Ctrl+Shift+Z` undo/redo against `layers`/
/// `history`/`pixel_history`, in `undo_order`'s own unified sequence).
/// Anything else (an unbound chord, with nothing modal open) is
/// silently ignored — there's no text field to fall back to routing
/// into yet.
///
/// Returns `Some(ActivatedCommand)` only in the one case no pure
/// `WidgetTree` mutation can finish on its own — see
/// [`handle_palette_key`]'s own doc comment for exactly which commands
/// that covers. The caller (`App::handle_key_event`) is what actually
/// has somewhere to put it (real document state for `Undo`/`Redo`, a
/// read/decode or encode/write step for an opened/saved file).
#[allow(clippy::too_many_arguments)]
fn handle_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    palette: &mut Option<WidgetId>,
    tool: &mut aurora_ui::Tool,
    layers: &mut aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    store: Option<&mut aurora_tile::TileStore>,
    undo_order: &mut UndoOrder,
    composite_cache: &mut CompositeCache,
    shortcuts: &ShortcutRegistry<AppCommand>,
    modifiers: Modifiers,
    key: Key,
    text: Option<&str>,
    clipboard: &mut dyn ClipboardAccess,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<ActivatedCommand> {
    let chord = KeyChord::new(modifiers, key);
    if dialog.is_some() {
        handle_dialog_key(workspace, focus, dialog, chord);
        return None;
    }
    if palette.is_some() {
        return handle_palette_key(
            workspace,
            focus,
            palette,
            chord,
            text,
            clipboard,
            file_dialog,
        );
    }
    if let Some(&command) = shortcuts.resolve(chord) {
        run_command(
            workspace,
            focus,
            palette,
            tool,
            layers,
            history,
            pixel_history,
            store,
            undo_order,
            command,
        );
        // Only Undo/Redo can change what a composite tile shows -- see
        // `App::run_undo_redo`'s own matching bump for the command-
        // palette/menu path to the same two commands.
        //
        // And that split is a disclosed, still-open gap, not a detail:
        // `Ctrl+Z`/`Ctrl+Shift+Z` runs Undo/Redo *here*, inline, so it
        // reaches neither half of `perform_undo_redo` -- no commit of a
        // live stroke before the undo, and no pan re-clamp after an
        // undone Move. See PLAN.md's own residual disclosure (0.57.7)
        // before editing this branch; closing it is a change to this
        // function's contract, not a line here.
        if matches!(command, AppCommand::Undo | AppCommand::Redo) {
            composite_cache.bump();
        }
    }
    None
}

/// Translates a real `winit::keyboard::Key` into this crate's own,
/// platform-free [`Key`] — `aurora_widgets::shortcut`'s own doc comment
/// names this exact translation as `aurora-app`'s job. Only the
/// characters/named keys [`NamedKey`] itself covers are recognised;
/// anything else (dead keys, `Key::Unidentified`, media keys, ...)
/// yields `None`, the same "not every platform key needs a shortcut
/// vocabulary entry yet" scope `NamedKey`'s own `#[non_exhaustive]`
/// documents.
fn translate_key(key: &winit::keyboard::Key) -> Option<Key> {
    match key {
        winit::keyboard::Key::Character(text) => text
            .chars()
            .next()
            .map(|ch| Key::Character(ch.to_ascii_lowercase())),
        winit::keyboard::Key::Named(named) => translate_named_key(*named).map(Key::Named),
        _ => None,
    }
}

fn translate_named_key(named: winit::keyboard::NamedKey) -> Option<NamedKey> {
    use winit::keyboard::NamedKey as Winit;
    Some(match named {
        Winit::Enter => NamedKey::Enter,
        Winit::Escape => NamedKey::Escape,
        Winit::Tab => NamedKey::Tab,
        Winit::Backspace => NamedKey::Backspace,
        Winit::Delete => NamedKey::Delete,
        Winit::Space => NamedKey::Space,
        Winit::ArrowUp => NamedKey::ArrowUp,
        Winit::ArrowDown => NamedKey::ArrowDown,
        Winit::ArrowLeft => NamedKey::ArrowLeft,
        Winit::ArrowRight => NamedKey::ArrowRight,
        Winit::F1 => NamedKey::F1,
        Winit::F2 => NamedKey::F2,
        Winit::F3 => NamedKey::F3,
        Winit::F4 => NamedKey::F4,
        Winit::F5 => NamedKey::F5,
        Winit::F6 => NamedKey::F6,
        Winit::F7 => NamedKey::F7,
        Winit::F8 => NamedKey::F8,
        Winit::F9 => NamedKey::F9,
        Winit::F10 => NamedKey::F10,
        Winit::F11 => NamedKey::F11,
        Winit::F12 => NamedKey::F12,
        _ => return None,
    })
}

/// Translates a real `winit::keyboard::ModifiersState` into this crate's
/// own, platform-free [`Modifiers`] — the `ModifiersChanged` half of the
/// same translation seam [`translate_key`] documents.
fn translate_modifiers(state: winit::keyboard::ModifiersState) -> Modifiers {
    Modifiers {
        control: state.control_key(),
        shift: state.shift_key(),
        alt: state.alt_key(),
        meta: state.super_key(),
    }
}

/// Converts a real, physical-pixel window size into the logical pixels
/// `aurora_widgets::WidgetTree::compute_layout` expects, dividing out
/// `scale_factor` — `winit`'s own physical/logical distinction
/// (`winit::dpi`'s own doc module), applied at the one seam in this
/// crate where a physical size reaches layout. Fractional scale factors
/// below `1.0` are real (some Linux compositors allow scaling down, not
/// just up), so this only falls back to `1.0` for a value `winit` should
/// never actually report — non-positive or non-finite — rather than
/// clamping every small-but-legitimate factor to `1.0` and getting the
/// conversion wrong for them.
#[must_use]
fn logical_size(physical: (u32, u32), scale_factor: f64) -> (f32, f32) {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    #[allow(clippy::cast_precision_loss)]
    let (width, height) = (f64::from(physical.0), f64::from(physical.1));
    #[allow(clippy::cast_possible_truncation)]
    (
        (width / scale_factor) as f32,
        (height / scale_factor) as f32,
    )
}

// -- Basic tools: pointer input, canvas view, tool dispatch --
//
// PLAN.md M1.9's "basic tools" bullet (Move, Marquee Select, Zoom, Pan,
// Eyedropper). This crate's first pointer input at all — until now only
// `KeyboardInput` was wired (see the "command dispatch" section above).
// Every function below is pure, free, and platform-free (real `winit`
// event *types* like `MouseButton`/`MouseScrollDelta` are plain,
// window-less data, the same reason `translate_key` can take a real
// `winit::keyboard::Key` and still be unit-tested), so the actual
// dispatch logic is headlessly testable in this sandbox (no display
// server) — the same shape this crate's keyboard input already uses.
//
// Move gained real pointer handling later the same week (`Drag::Move`,
// below) once `aurora_doc::LayerTree::set_bounds` gave the document
// model somewhere for a reposition to actually land. Eyedropper
// followed the same week (`Drag::Eyedropper`, `sample_pixel`,
// `App::sample_eyedropper`) — every tool this bullet named now has real
// pointer handling.

/// One user-visible pointer button, decoupled from `winit::event::
/// MouseButton` — the same "pure dispatch logic, isolate the real
/// platform type" seam `translate_key`/`translate_modifiers` already use
/// for keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

/// Converts a real `winit::event::MouseButton` — `None` for anything
/// this crate doesn't give a meaning to yet (`Back`/`Forward`/an
/// arbitrary numbered button).
#[must_use]
fn translate_pointer_button(button: winit::event::MouseButton) -> Option<PointerButton> {
    match button {
        winit::event::MouseButton::Left => Some(PointerButton::Primary),
        winit::event::MouseButton::Right => Some(PointerButton::Secondary),
        winit::event::MouseButton::Middle => Some(PointerButton::Middle),
        _ => None,
    }
}

/// Converts a physical point (e.g. `WindowEvent::CursorMoved`'s own
/// position) into the window's logical space — the single-point twin of
/// [`logical_size`]; see that function's own doc comment for the
/// scale-factor fallback reasoning, which applies identically here.
#[must_use]
fn logical_point(physical: (f64, f64), scale_factor: f64) -> (f32, f32) {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    #[allow(clippy::cast_possible_truncation)]
    (
        (physical.0 / scale_factor) as f32,
        (physical.1 / scale_factor) as f32,
    )
}

/// Converts a pointer position in the *window's* own logical space into
/// a canvas-area-relative logical position, if the pointer is actually
/// over the canvas area — `None` if it's over a dock panel, outside the
/// window, or before the first layout has run (`workspace.tree.bounds`
/// returns `None` either way, so this doesn't need to tell those cases
/// apart).
#[must_use]
fn pointer_in_canvas(
    workspace: &aurora_ui::Workspace,
    window_position: (f32, f32),
) -> Option<(f32, f32)> {
    let bounds = workspace.tree.bounds(workspace.canvas_area)?;
    #[allow(clippy::cast_precision_loss)]
    let (bx, by, bw, bh) = (
        bounds.x as f32,
        bounds.y as f32,
        bounds.width as f32,
        bounds.height as f32,
    );
    let (x, y) = window_position;
    if x < bx || y < by || x >= bx + bw || y >= by + bh {
        return None;
    }
    Some((x - bx, y - by))
}

/// How close (logical px) a pointer needs to land to
/// `Workspace::divider`'s own `x` to count as "grabbing" it —
/// `aurora_ui::Workspace::divider` is a real `Role::Splitter` widget
/// (invariant §7.3.9: every widget carries an accesskit node), but it
/// has zero layout width today (no pixel rendering exists yet to draw a
/// visible grab handle — same "real node, no pixels yet" gap every
/// widget in this workspace already has), so hit-testing against its
/// own bare bounds alone would need pixel-perfect precision. This is a
/// plain interaction-tolerance heuristic living in the app shell, not a
/// widget's own chrome value — invariant §7.3.10 (resolve style from
/// tokens) governs what a widget draws, not how forgiving this crate's
/// own hit-testing is, the same distinction `paint.rs`'s own doc
/// comments already draw elsewhere in this workspace between chrome and
/// non-chrome numbers.
const RAIL_DIVIDER_HIT_TOLERANCE: f32 = 4.0;

/// Whether `window_position` (window-logical space) is close enough to
/// [`aurora_ui::Workspace::divider`] to start a resize —
/// [`App::handle_pointer_pressed`]'s own gate before falling through to
/// canvas-tool dragging, checked ahead of [`pointer_in_canvas`] since
/// the divider sits *outside* the canvas area entirely.
#[must_use]
fn pointer_on_rail_divider(workspace: &aurora_ui::Workspace, window_position: (f32, f32)) -> bool {
    let Some(bounds) = workspace.tree.bounds(workspace.divider) else {
        return false;
    };
    #[allow(clippy::cast_precision_loss)]
    let (dx, dy, dh) = (bounds.x as f32, bounds.y as f32, bounds.height as f32);
    let (x, y) = window_position;
    (x - dx).abs() <= RAIL_DIVIDER_HIT_TOLERANCE && y >= dy && y < dy + dh
}

/// An in-progress dock-rail resize — started by a primary-button press
/// on [`aurora_ui::Workspace::divider`] ([`pointer_on_rail_divider`]),
/// advanced on every subsequent `CursorMoved` by
/// [`App::handle_pointer_moved`] via `aurora_ui::set_rail_width`, ended
/// on release. Deliberately not a [`Drag`] variant: resizing the rail
/// is neither canvas-relative (it's a window-logical `x` position, not
/// a document/canvas one) nor tool-dependent (it starts regardless of
/// the active `aurora_ui::Tool`), unlike everything `Drag` itself
/// models.
#[derive(Debug, Clone, Copy)]
struct RailResize {
    /// The pointer's own window-logical `x` when the drag began.
    start_pointer_x: f32,
    /// The rail's own width (`aurora_ui::rail_width`) when the drag
    /// began.
    start_width: f32,
}

/// The rail's own candidate new width once the pointer has moved to
/// `pointer_x` — pure arithmetic, [`App::handle_pointer_moved`]'s own
/// delta computation extracted so it's testable without a real window
/// or `App` (the same "pure function `App` just calls" shape
/// [`continue_drag`] already uses). The rail sits to the *right* of the
/// divider (`design/mockups/workspace.html`'s own canvas-then-rail
/// ordering), so moving the pointer right shrinks it by exactly that
/// rightward travel, and moving left grows it by exactly that leftward
/// travel — clamping to a sane range happens downstream, in
/// `aurora_ui::set_rail_width` itself, not here.
#[must_use]
fn resized_rail_width(resize: RailResize, pointer_x: f32) -> f32 {
    resize.start_width - (pointer_x - resize.start_pointer_x)
}

/// One in-progress pointer drag. `Pan` tracks the last *screen*-space
/// position (panning moves the view itself, so re-deriving a document
/// point from a moving view on every event would be circular); `Marquee`
/// tracks the fixed document-space point the drag started at, since a
/// selection rectangle is defined in document space regardless of how
/// the view is panned/zoomed mid-drag; `Brush`/`Eraser` track the last
/// document-space point painted/erased, the same "delta since last
/// event" shape `Pan` uses, plus `carry` — how far the stroke has
/// already travelled past the last placed dab
/// (`aurora_brush::advance_segment`'s own carry parameter) — so spacing
/// stays correct across many small move events, not just within one
/// event's own segment (see [`continue_drag`]'s own doc comment for why
/// this matters). `Eraser` is otherwise identical to `Brush` — same
/// dab-spacing math, different pixel operation once a dab lands (see
/// [`App::erase_dab`]) — so it gets its own variant rather than reusing
/// `Brush`'s, keeping "which pixel operation to perform" a property of
/// the active `Drag`, not a runtime flag alongside it.
///
/// `Move` tracks `layer_id` (which layer is being repositioned),
/// `start_doc`/`start_bounds` (the drag's own fixed starting point and
/// that layer's own bounds at that moment), and `current_bounds` — the
/// live result of shifting `start_bounds` by however far the pointer
/// has travelled since `start_doc` ([`continue_drag`] updates this
/// field in place, mirroring `Pan`'s own `last_screen`); the caller
/// (`App::handle_pointer_moved`) reads it back out afterward and
/// applies it to the document (`App::apply_move`) — the same
/// "`continue_drag` stays pure, `App` does the one real mutation" split
/// `Brush`/`Eraser` already use for painting.
///
/// `Eyedropper` carries no fields at all: unlike every other drag here,
/// sampling a pixel needs no state carried between events (no carry, no
/// start point, nothing) — each event just samples wherever the
/// pointer currently is, independent of the last one.
#[derive(Debug)]
enum Drag {
    Pan {
        last_screen: (f32, f32),
    },
    Marquee {
        start_doc: (f32, f32),
    },
    Brush {
        last_doc: (f32, f32),
        carry: f32,
        /// This stroke's own accumulated undo snapshot — `None` when the
        /// drag began with no real active pixel layer to paint into
        /// (`begin_drag`'s own doc comment: Brush/Eraser start
        /// unconditionally, whether or not there's actually anywhere to
        /// paint), matching [`App::paint_dab`]'s own absent-precondition
        /// honesty rather than picking an arbitrary placeholder
        /// `SurfaceId`.
        stroke: Option<aurora_brush::StrokeSnapshot>,
        /// Tiles this stroke has already logged a page-in failure for.
        ///
        /// A corrupt tile fails *every* dab for the rest of the drag, so
        /// collapsing the log to one line per dab (0.55.0) still left a
        /// ~600 px drag across one broken tile emitting ~100 identical
        /// warnings. This dedupes across the whole stroke instead: one
        /// line per broken tile, per stroke. It lives on the drag rather
        /// than on `App` so its lifetime is exactly the stroke's by
        /// construction — a new drag cannot inherit a stale set, and no
        /// caller has to remember to clear it.
        warned: std::collections::HashSet<aurora_tile::TileId>,
    },
    Eraser {
        last_doc: (f32, f32),
        carry: f32,
        /// Same as `Drag::Brush`'s own `stroke` field, above.
        stroke: Option<aurora_brush::StrokeSnapshot>,
        /// Same as `Drag::Brush`'s own `warned` field, above.
        warned: std::collections::HashSet<aurora_tile::TileId>,
    },
    Move {
        layer_id: aurora_doc::LayerId,
        start_doc: (f32, f32),
        start_bounds: aurora_core::Rect,
        current_bounds: aurora_core::Rect,
    },
    Eyedropper,
}

/// The active `Drag::Brush`'s own accumulated stroke snapshot, if there
/// is one — `None` for any other drag (including a `Drag::Eraser`, which
/// has its own snapshot and its own accessor), for a `Drag::Brush` that
/// began with no real active pixel layer to paint into, and for no drag
/// at all.
///
/// A free function over `&mut Option<Drag>` rather than an `&mut self`
/// method on purpose: [`App::paint_dab`] needs this *and* `self.tile_store`
/// borrowed at the same time, and a `&mut self` helper would borrow all
/// of `App`. Extracted in 0.56.0 so the variant-matching itself is
/// testable headlessly, with no `App` (and therefore no GPU) to build.
fn brush_stroke_mut(drag: &mut Option<Drag>) -> Option<&mut aurora_brush::StrokeSnapshot> {
    match drag {
        Some(Drag::Brush { stroke, .. }) => stroke.as_mut(),
        _ => None,
    }
}

/// [`brush_stroke_mut`]'s eraser counterpart, on exactly the same terms:
/// a `Drag::Eraser`'s own snapshot only, never a `Drag::Brush`'s.
fn eraser_stroke_mut(drag: &mut Option<Drag>) -> Option<&mut aurora_brush::StrokeSnapshot> {
    match drag {
        Some(Drag::Eraser { stroke, .. }) => stroke.as_mut(),
        _ => None,
    }
}

/// The failures in `outcome` this stroke hasn't already logged, marking
/// each one logged as it goes — so one permanently broken tile costs one
/// warning per stroke rather than one per dab.
///
/// 0.55.0 collapsed the log from one line per failing *tile* to one per
/// *dab*, which is not where the flood actually comes from: a corrupt
/// tile fails every dab for the rest of the drag, so a ~600 px drag
/// across one still emitted ~100 identical lines. Nothing is dropped —
/// the *first* failure on each tile is always reported, and a second
/// broken tile later in the same stroke gets its own line.
///
/// Allocates nothing on the overwhelmingly common path (a dab that
/// failed on no tile at all returns an empty `Vec`, which does not
/// allocate).
fn unwarned_failures<'a>(
    drag: &mut Option<Drag>,
    outcome: &'a aurora_brush::DabOutcome,
) -> Vec<(aurora_tile::TileId, &'a aurora_tile::TileError)> {
    if outcome.is_complete() {
        return Vec::new();
    }
    let all = || {
        outcome
            .failed()
            .iter()
            .map(|(tile, err)| (*tile, err))
            .collect()
    };
    let Some(Drag::Brush { warned, .. } | Drag::Eraser { warned, .. }) = drag else {
        // No stroke to remember them in. Unreachable in practice (only
        // a brush/eraser drag reaches a dab at all), but under-reporting
        // a scratch-disk failure is the wrong way to be wrong about it.
        return all();
    };
    outcome
        .failed()
        .iter()
        .filter(|(tile, _)| warned.insert(*tile))
        .map(|(tile, err)| (*tile, err))
        .collect()
}

/// Records a completed `Drag::Move` as a single undo step, from
/// `start_bounds` to wherever `layer_id` actually ended up — already
/// applied, live, by every [`App::apply_move`] call during the drag —
/// via `aurora_doc::History::record_bounds_change`, which journals the
/// move without re-applying it (the tree already reflects it). Called
/// once, from [`commit_ending_drag`], when the drag that just ended was
/// a `Drag::Move`.
///
/// A no-op if the layer never actually ended up anywhere different
/// (`start_bounds` still matches its current bounds — e.g. a click that
/// started and ended a drag with no real pointer movement, or `layer_id`
/// no longer exists at all): nothing for a later undo to meaningfully
/// reverse. A real, logged failure otherwise is worth a warning, the
/// same discipline [`App::apply_move`] already uses.
///
/// A free function over the four pieces of state it actually needs
/// rather than an `&mut self` method (0.57.0), for the same reason
/// [`brush_stroke_mut`] is one: it makes the behaviour testable without
/// an `App`, which needs a real window and GPU surface to construct.
fn finish_move(
    layers: &aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    undo_order: &mut UndoOrder,
    layer_id: aurora_doc::LayerId,
    start_bounds: aurora_core::Rect,
) {
    if layers.bounds(layer_id) == Some(start_bounds) {
        return;
    }
    match history.record_bounds_change(layers, layer_id, start_bounds) {
        Ok(()) => undo_order.record(UndoKind::Structural, history, pixel_history),
        Err(err) => tracing::warn!(?err, "failed to record the completed move"),
    }
}

/// Commits a drag that is ending — for whatever reason — into the undo
/// state its own kind belongs in: a `Drag::Brush`/`Drag::Eraser`
/// carrying a real `stroke` becomes a `PixelHistory` entry (and, if
/// `PixelHistory::push` reports it recorded something, an entry in
/// `undo_order`'s own unified sequence); a `Drag::Move` is coalesced
/// into one structural entry by [`finish_move`]. Every other variant,
/// and `None`, is a no-op — a pan, a marquee, an eyedropper sample and
/// "no drag at all" have nothing to commit.
///
/// **The only place a stroke becomes undoable, and it is called from
/// every path that ends one** (0.57.0). This logic used to live inside
/// [`App::handle_pointer_released`], which is not the only way a drag
/// ends: [`App::handle_pointer_pressed`] overwrote `App::drag`
/// unconditionally, and the `CursorLeft` handler cleared it outright.
/// So pressing the middle button to pan mid-stroke, pressing the right
/// button mid-stroke, or simply dragging the cursor off the window edge
/// — all routine gestures — dropped a live `Drag::Brush` whose pixels
/// were already on the layer, and no undo entry was ever pushed for
/// them. The next `Ctrl+Z` then undid the *previous* stroke: strictly
/// worse than the phantom entry 0.56.0 removed, because a phantom entry
/// at least did nothing, while this silently mis-targeted a real edit.
///
/// `Drag::Move` is handled here too, not only in the release path. It
/// is the same extraction, not extra scope: leaving it behind would
/// have left `handle_pointer_pressed` and `CursorLeft` dropping a live
/// `Drag::Move`'s coalesced undo entry in exactly the way this function
/// exists to stop, with the layer already repositioned on screen.
///
/// The two document-replacement sites (`App::open_file`'s flat-image
/// path and `App::open_aur_file`) deliberately do *not* call this. They
/// clear `drag` as part of replacing `layers`/`history`/`pixel_history`
/// wholesale with the newly opened document's own; committing the
/// outgoing stroke there would push an entry onto a `PixelHistory` that
/// is discarded two lines later, and — worse — one whose captured tiles
/// name a surface belonging to a document that is no longer open.
/// Dropping it is correct there, and it is the only place dropping one
/// is.
///
/// **Takes `view`/`active_layer` because committing a `Drag::Move` is
/// one of the two ways the pan boundary moves** (the other is the
/// active layer itself changing — [`select_layer`]). A Move rewrites
/// the layer's own `bounds` (`App::apply_move`), and the pan bound is
/// measured against exactly that origin ([`active_layer_origin`]), so a
/// finished Move can leave a pan that never moved sitting outside its
/// own bound — the same `(-300, -150)` `canvas_local_origin` divergence
/// [`clamp_pan_to_active_layer`] describes. Re-establishing it *here*,
/// rather than in each of the three callers, is what stops the next
/// path that ends a drag from silently reopening it — the same reason
/// the commit itself lives in one shared place.
///
/// Only at the commit, deliberately not per pointer-move event:
/// `continue_drag`'s own `Drag::Move` arm derives its delta from a
/// *fixed* `start_doc` through `view.to_document`, so clamping the view
/// mid-drag would feed the moved view back into the next event's delta
/// and chase itself. At the commit there is no drag left in progress,
/// so there is no loop to close. The cost is a transient violation for
/// the duration of the drag itself: the canvas can render clamped while
/// `to_document` does not agree, which is a visual artefact under the
/// pointer that is *not* being used to paint (a `Drag::Move` is not a
/// `Drag::Brush`), and it is resolved by this clamp the moment the drag
/// ends.
fn commit_ending_drag(
    drag: Option<Drag>,
    layers: &aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    undo_order: &mut UndoOrder,
    view: &mut aurora_ui::CanvasView,
    active_layer: Option<aurora_doc::LayerId>,
) {
    match drag {
        Some(
            Drag::Brush {
                stroke: Some(stroke),
                ..
            }
            | Drag::Eraser {
                stroke: Some(stroke),
                ..
            },
        ) => {
            // `push`'s own `bool` is what tells "a real stroke happened"
            // apart from "a click that never touched a tile" without
            // asking the snapshot itself.
            if pixel_history.push(stroke) {
                undo_order.record(UndoKind::Pixel, history, pixel_history);
            }
        }
        Some(Drag::Move {
            layer_id,
            start_bounds,
            ..
        }) => {
            finish_move(
                layers,
                history,
                pixel_history,
                undo_order,
                layer_id,
                start_bounds,
            );
            // The move is over, `layers` already carries the new
            // bounds, and no drag is in progress -- see this function's
            // own doc comment for why the clamp belongs at exactly this
            // point and nowhere earlier. Clamped against the *active*
            // layer, not this drag's own `layer_id`: the pan bound is
            // defined by whichever layer is active, and those are the
            // same layer for every move a user can actually start
            // (`begin_drag` takes `Move`'s id from `active_pixel_layer`)
            // -- so this is the definition, spelled out, rather than a
            // second source of truth that could drift from it.
            clamp_pan_to_active_layer(view, layers, active_layer);
        }
        _ => {}
    }
}

/// Starts a drag for `tool`/`button` at `canvas_point` (already
/// canvas-area-relative — see [`pointer_in_canvas`]), or `None` if this
/// tool/button combination doesn't start one. The middle button always
/// pans, regardless of the active tool — the usual "hand tool"
/// convention professional raster editors already use as a universal
/// pan gesture.
///
/// `Brush`/`Eraser`/`Eyedropper` start unconditionally on a primary
/// click, regardless of whether there's actually anywhere to
/// paint/erase/sample (a live store, and — for `Brush`/`Eraser` only —
/// an active pixel layer; `Eyedropper` samples the composited document
/// and needs no active layer at all) — that check happens where the
/// real pixel work does
/// (`App::paint_dab`/`App::erase_dab`/`App::sample_eyedropper`),
/// keeping this function pure and not needing to know about either.
/// `Move` is the one drag that *does* need to know up front —
/// repositioning needs a real pixel layer to reposition, and
/// `start_bounds` has to be that layer's bounds at this exact moment,
/// not looked up later — so it takes `active_pixel_layer` (`None` for
/// no active layer, an active layer that's a group, or no active layer
/// at all: [`active_pixel_layer`]) and starts nothing if that's `None`.
#[must_use]
fn begin_drag(
    tool: aurora_ui::Tool,
    button: PointerButton,
    canvas_point: (f32, f32),
    view: &aurora_ui::CanvasView,
    active_pixel_layer: Option<(aurora_doc::LayerId, aurora_core::Rect)>,
) -> Option<Drag> {
    match (tool, button) {
        (_, PointerButton::Middle) | (aurora_ui::Tool::Pan, PointerButton::Primary) => {
            Some(Drag::Pan {
                last_screen: canvas_point,
            })
        }
        (aurora_ui::Tool::MarqueeSelect, PointerButton::Primary) => Some(Drag::Marquee {
            start_doc: view.to_document(canvas_point),
        }),
        (aurora_ui::Tool::Brush, PointerButton::Primary) => Some(Drag::Brush {
            last_doc: view.to_document(canvas_point),
            carry: 0.0,
            stroke: active_pixel_layer
                .map(|(id, _)| aurora_brush::StrokeSnapshot::new(surface_id_for(id))),
            warned: std::collections::HashSet::new(),
        }),
        (aurora_ui::Tool::Eraser, PointerButton::Primary) => Some(Drag::Eraser {
            last_doc: view.to_document(canvas_point),
            carry: 0.0,
            stroke: active_pixel_layer
                .map(|(id, _)| aurora_brush::StrokeSnapshot::new(surface_id_for(id))),
            warned: std::collections::HashSet::new(),
        }),
        (aurora_ui::Tool::Move, PointerButton::Primary) => {
            let (layer_id, bounds) = active_pixel_layer?;
            Some(Drag::Move {
                layer_id,
                start_doc: view.to_document(canvas_point),
                start_bounds: bounds,
                current_bounds: bounds,
            })
        }
        (aurora_ui::Tool::Eyedropper, PointerButton::Primary) => Some(Drag::Eyedropper),
        _ => None,
    }
}

/// Advances an in-progress `drag` to `canvas_point`: pans the view by
/// the screen-space delta since the last event, updates the active
/// selection to the marquee rectangle spanned so far
/// (`aurora_ui::tool::marquee_rect`) — live, so the selection visibly
/// grows/shrinks as the user drags, not just once on release — or, for
/// `Brush`/`Eraser`, returns the document-space dab centers this
/// event's own new segment placed, via `aurora_brush::advance_segment`
/// (**not** `dabs_along_path` on a fresh two-point slice each time,
/// which would restart spacing's own `carry` at `0.0` on every single
/// move event — for a slow drag whose per-event segments are each
/// shorter than one dab's own spacing, that would silently place no
/// dabs at all past the first, despite real distance covered over many
/// events; `advance_segment` carries `Drag::Brush`/`Drag::Eraser`'s own
/// `carry` field forward across events instead, exactly the problem it
/// exists to solve).
///
/// Deliberately returns dab positions as plain data rather than
/// stamping/erasing them itself: that needs a live `aurora_tile::TileStore`
/// and the active layer's bounds, neither of which this function (or
/// `Drag`/`begin_drag` above) needs to know about to stay exactly as
/// pure and testable as `Pan`/`Marquee` already are — the caller
/// (`App::handle_pointer_moved`) does the actual pixel work, and which
/// of `paint_dab`/`erase_dab` to call for each returned position.
/// `Move` follows the same split: this function only updates
/// `Drag::Move`'s own `current_bounds` field (via [`shift_bounds`]) and
/// always returns an empty `Vec` (there are no dabs to paint) — the
/// caller reads `current_bounds` back out of `drag` afterward and
/// applies it to the document (`App::apply_move`), the same "update a
/// field in place, caller does the one real mutation" shape `Pan`'s own
/// `last_screen` already uses. `Eyedropper` has nothing at all to
/// update (it carries no state — see [`Drag`]'s own doc comment) and
/// always returns an empty `Vec` too; the caller samples directly at
/// `canvas_point` itself (`App::sample_eyedropper`).
///
/// `min_doc` is the active layer's own document-space origin
/// (`active_layer_origin`) — after `Drag::Pan`'s own `pan_by` call,
/// this clamps the view (`CanvasView::clamp_pan_to_minimum`) so it can
/// never scroll past that edge. Without this, panning right/down kept
/// moving `view`'s pan arbitrarily far, making `to_document` (used both
/// here, for `Marquee`/`Brush`/`Eraser`/`Move`, and by the caller for
/// `Eyedropper`) report a true, unbounded — eventually negative —
/// document position, while the renderer (`canvas_local_origin` /
/// `aurora_gpu::TileResidency::set_origin`) silently pinned the
/// *drawn* view at the document's own top-left tile forever, since
/// `aurora_tile::TileId`'s unsigned fields have no way to represent a
/// negative tile. Paint and render then silently disagreed. Clamping
/// here keeps both reading the same, already-bounded `view` instead.
#[must_use]
fn continue_drag(
    drag: &mut Drag,
    canvas_point: (f32, f32),
    view: &mut aurora_ui::CanvasView,
    selection: &mut aurora_doc::SelectionSet,
    min_doc: (f32, f32),
) -> Vec<(f32, f32)> {
    match drag {
        Drag::Pan { last_screen } => {
            let delta = (
                canvas_point.0 - last_screen.0,
                canvas_point.1 - last_screen.1,
            );
            view.pan_by(delta);
            view.clamp_pan_to_minimum(min_doc);
            *last_screen = canvas_point;
            Vec::new()
        }
        Drag::Marquee { start_doc } => {
            let current_doc = view.to_document(canvas_point);
            let rect = aurora_ui::tool::marquee_rect(*start_doc, current_doc);
            selection.select(aurora_doc::Selection::new(rect));
            Vec::new()
        }
        Drag::Brush {
            last_doc, carry, ..
        } => {
            let current_doc = view.to_document(canvas_point);
            let step = aurora_brush::dab_step(BRUSH_RADIUS, aurora_brush::DEFAULT_SPACING);
            let (dabs, new_carry) =
                aurora_brush::advance_segment(*last_doc, current_doc, *carry, step);
            *last_doc = current_doc;
            *carry = new_carry;
            dabs
        }
        Drag::Eraser {
            last_doc, carry, ..
        } => {
            let current_doc = view.to_document(canvas_point);
            let step = aurora_brush::dab_step(ERASER_RADIUS, aurora_brush::DEFAULT_SPACING);
            let (dabs, new_carry) =
                aurora_brush::advance_segment(*last_doc, current_doc, *carry, step);
            *last_doc = current_doc;
            *carry = new_carry;
            dabs
        }
        Drag::Move {
            start_doc,
            start_bounds,
            current_bounds,
            ..
        } => {
            let current_doc = view.to_document(canvas_point);
            let delta = (current_doc.0 - start_doc.0, current_doc.1 - start_doc.1);
            *current_bounds = shift_bounds(*start_bounds, delta);
            Vec::new()
        }
        // Nothing to update in place (see `Drag::Eyedropper`'s own doc
        // comment) -- the caller (`App::handle_pointer_moved`) samples
        // directly at `canvas_point` itself once this returns.
        Drag::Eyedropper => Vec::new(),
    }
}

/// `bounds` shifted by `delta` document-space pixels, rounding to the
/// nearest whole pixel — [`Drag::Move`]'s own per-event position
/// update, kept as its own function so `continue_drag`'s own `Move` arm
/// stays a one-liner.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn shift_bounds(bounds: aurora_core::Rect, delta: (f32, f32)) -> aurora_core::Rect {
    aurora_core::Rect {
        x: bounds.x + delta.0.round() as i64,
        y: bounds.y + delta.1.round() as i64,
        width: bounds.width,
        height: bounds.height,
    }
}

/// Base of the exponential zoom-per-scroll-unit curve `apply_scroll_zoom`
/// uses — `1.1^steps` rather than a linear `1.0 + steps * k`, so it's
/// always positive (never yields a zero/negative zoom, whatever `steps`
/// is) and composes smoothly across many small scroll events, matching
/// how trackpad-driven zoom feels in practice.
const ZOOM_WHEEL_BASE: f32 = 1.1;

/// How many "steps" of zoom one scroll event represents — a wheel
/// `LineDelta` step is one unit per notch; a trackpad `PixelDelta` is
/// scaled down since it reports much finer-grained deltas.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn zoom_steps_for_scroll(delta: winit::event::MouseScrollDelta) -> f32 {
    match delta {
        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
        winit::event::MouseScrollDelta::PixelDelta(position) => (position.y / 20.0) as f32,
    }
}

/// Zooms `view` in/out around `anchor` (already canvas-area-relative) in
/// response to one `WindowEvent::MouseWheel` — the "scroll to zoom"
/// gesture that works regardless of which tool is active, matching
/// every professional raster editor's own convention.
///
/// `min_doc` is the active layer's own document-space origin
/// (`active_layer_origin`), passed to `CanvasView::clamp_pan_to_minimum`
/// after `zoom_at` — `zoom_at` recomputes `pan` from scratch to keep
/// `anchor` fixed, and that new `pan` can land past the document's own
/// top-left edge just as easily as a plain `pan_by` can (see
/// `continue_drag`'s own doc comment for why that must never happen).
///
/// **Takes the live `drag`, if there is one, because that clamp moves
/// the view** (0.57.7) — and a drag in progress is holding a
/// document-space reference point fixed from the moment it began. The
/// two are paired here, in the one function, rather than left to each
/// caller to remember: see [`shift_drag_reference`] for why re-anchoring
/// is the right answer for *this* gesture where ending the drag is the
/// right answer for a layer-row click ([`press_layer_row`]) or an undo
/// ([`perform_undo_redo`]).
fn apply_scroll_zoom(
    view: &mut aurora_ui::CanvasView,
    drag: Option<&mut Drag>,
    anchor: (f32, f32),
    delta: winit::event::MouseScrollDelta,
    min_doc: (f32, f32),
) {
    let before = view.to_document(anchor);
    let steps = zoom_steps_for_scroll(delta);
    let factor = ZOOM_WHEEL_BASE.powf(steps);
    view.zoom_at(anchor, view.zoom() * factor);
    view.clamp_pan_to_minimum(min_doc);
    let after = view.to_document(anchor);
    if let Some(drag) = drag {
        shift_drag_reference(drag, (after.0 - before.0, after.1 - before.1));
    }
}

/// Moves every document-space reference point a live `drag` is holding
/// by `delta`, after something moved `view` out from under it.
///
/// **Why a drag survives this at all, unlike the ones
/// [`press_layer_row`] and [`perform_undo_redo`] end outright**
/// (0.57.7). Clicking a Layers-panel row, or invoking Undo, is a
/// gesture that says "I am done with this drag"; scrolling to zoom
/// while painting is not — it is an ordinary thing to do mid-stroke,
/// and forcibly ending the stroke would be worse than the bug. So this
/// path keeps the drag and re-anchors it instead.
///
/// **Why a single uniform `delta` is the exact correction.**
/// [`aurora_ui::CanvasView::zoom_at`] holds the document point under
/// its own anchor fixed, and a stored document-space reference *is* a
/// document position, so a pure zoom leaves every one of them still
/// naming the same place: nothing to correct. The clamp that follows it
/// ([`aurora_ui::CanvasView::clamp_pan_to_minimum`]) is what actually
/// bites — and it only ever changes `pan`, at a zoom now fixed, so it
/// shifts `to_document(p)` by the same amount for every `p`. Measuring
/// that shift at the zoom anchor (where the zoom's own contribution is
/// exactly zero) therefore yields the whole correction, for reference
/// points anywhere on the canvas.
///
/// Without it, a scroll-zoom that hits the pan bound left a live
/// `Drag::Brush`'s own `last_doc` naming the pre-clamp document
/// position while `continue_drag` read the post-clamp one from the
/// moved view — and the next pointer-move event interpolated a whole
/// segment of dabs between them, paint the user never drew. The same
/// stale reference shifts a `Drag::Move`'s layer and a
/// `Drag::Marquee`'s rect by the same jump.
///
/// `Drag::Pan` and `Drag::Eyedropper` are deliberately untouched, not
/// overlooked: a pan's own `last_screen` is a *screen* position, which a
/// view move does not invalidate (and its arm re-clamps on its own next
/// event), and an eyedropper holds no reference point at all — it
/// samples wherever the pointer currently is.
fn shift_drag_reference(drag: &mut Drag, delta: (f32, f32)) {
    match drag {
        Drag::Marquee { start_doc } | Drag::Move { start_doc, .. } => {
            start_doc.0 += delta.0;
            start_doc.1 += delta.1;
        }
        Drag::Brush { last_doc, .. } | Drag::Eraser { last_doc, .. } => {
            last_doc.0 += delta.0;
            last_doc.1 += delta.1;
        }
        Drag::Pan { .. } | Drag::Eyedropper => {}
    }
}

/// How much one Zoom-tool click zooms in (or, with `Alt` held, out) —
/// [`handle_zoom_tool_click`]'s own factor.
const ZOOM_CLICK_FACTOR: f32 = 2.0;

/// Handles a Zoom-tool primary click at `canvas_point`: zooms in by
/// [`ZOOM_CLICK_FACTOR`], or out (the reciprocal) if `modifiers.alt` is
/// held — Photoshop's own Zoom-tool convention (`Alt`+click to zoom
/// out), distinct from [`apply_scroll_zoom`], which works with any tool
/// active.
///
/// `min_doc` is the active layer's own document-space origin
/// (`active_layer_origin`) — same reasoning as `apply_scroll_zoom`'s own
/// doc comment: `zoom_at` recomputes `pan` to keep `canvas_point` fixed
/// on screen, and that new `pan` needs the same post-hoc
/// `clamp_pan_to_minimum` call to stay within the document's own edge.
fn handle_zoom_tool_click(
    view: &mut aurora_ui::CanvasView,
    canvas_point: (f32, f32),
    modifiers: Modifiers,
    min_doc: (f32, f32),
) {
    let factor = if modifiers.alt {
        1.0 / ZOOM_CLICK_FACTOR
    } else {
        ZOOM_CLICK_FACTOR
    };
    view.zoom_at(canvas_point, view.zoom() * factor);
    view.clamp_pan_to_minimum(min_doc);
}

// -- Brush painting, eraser, and layer selection: a live document, a
// -- live tile store, and a way to pick which layer is active --
//
// PLAN.md M1.9's "basic brush and eraser" bullet, picking up exactly
// where `aurora_brush::stamp_dab` (ADR 0010) left off:
// this crate's first *live* document (`App::layers`, kept alive instead
// of being discarded after populating the panels, as it was through
// M1.8/M1.9 until now) and first real `aurora_tile::TileStore`. Eraser
// (`App::erase_dab`, `Drag::Eraser`) reuses that same live store and
// active layer, calling `aurora_brush::erase_dab` instead of
// `stamp_dab` -- the bullet's other named half, now
// closed. `select_layer` closes the layer-selection half: `active_layer`
// no longer just defaults to the topmost pixel layer and stays there
// forever -- a real click on a real, clickable Layers-panel row
// (`aurora_ui::layers_panel`, `aurora_widgets::WidgetTree::hit_test`)
// changes it, live. Move's own drag-to-reposition logic (`Drag::Move`,
// `App::apply_move`) followed once `aurora_doc::LayerTree::set_bounds`
// gave it somewhere real to land, and `canvas_local_origin` learned to
// read the active layer's own bounds offset so a moved layer actually
// renders in its new place, not just in the document model. Undo-as-
// you-drag remains separate, still-open follow-on work.

/// The topmost pixel layer in `layers` — [`App::active_layer`]'s own
/// initial value. `layers.roots()` is already ordered top-to-bottom
/// (index 0 topmost, matching every panel in this workspace), so the
/// first root that's a pixel layer (skipping any group) is it. `None`
/// for a document with no pixel layer at all.
#[must_use]
fn topmost_pixel_layer(layers: &aurora_doc::LayerTree) -> Option<aurora_doc::LayerId> {
    layers
        .roots()
        .iter()
        .copied()
        .find(|&id| matches!(layers.kind(id), Some(aurora_doc::LayerKind::Pixel { .. })))
}

/// `active_layer`, together with its own bounds, if it names a real
/// pixel layer in `layers` — `None` for no active layer, an unknown id,
/// or one that names a group (a group has no `bounds` of its own to
/// move). What [`begin_drag`]'s own `Move` arm needs to start a drag,
/// via [`aurora_doc::LayerTree::bounds`].
#[must_use]
fn active_pixel_layer(
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
) -> Option<(aurora_doc::LayerId, aurora_core::Rect)> {
    let id = active_layer?;
    let bounds = layers.bounds(id)?;
    Some((id, bounds))
}

/// The active layer's own document-space origin (`bounds.x`/`bounds.y`,
/// as `f32`), or `(0.0, 0.0)` if there's no active layer or it isn't a
/// pixel layer — the same absent-precondition honesty
/// [`active_pixel_layer`] already uses. What [`canvas_local_origin`]
/// needs to convert a document-space point into the active layer's own
/// surface-local space, now that a layer can actually sit somewhere
/// other than the document's own origin (`aurora_doc::LayerTree::set_bounds`,
/// the Move tool's own document-model support).
///
/// **This value is also the canvas pan boundary**
/// ([`clamp_pan_to_active_layer`]), so it moves when *either* of its two
/// inputs does: a different layer becoming active, or the active
/// layer's own `bounds` changing under it. Any new code path that does
/// either must re-clamp — see [`App::active_layer`]'s own doc comment
/// for the full list of the ones that already do.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn active_layer_origin(
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
) -> (f32, f32) {
    active_pixel_layer(layers, active_layer)
        .map_or((0.0, 0.0), |(_, bounds)| (bounds.x as f32, bounds.y as f32))
}

/// Re-establishes the pan bound after the *boundary* moved rather than
/// the pan.
///
/// [`aurora_ui::CanvasView::clamp_pan_to_minimum`] bounds the view
/// against the active layer's own origin ([`active_layer_origin`], the
/// value [`canvas_local_origin`] subtracts), and every gesture that
/// moves the *pan* already calls it ([`continue_drag`],
/// [`apply_scroll_zoom`], [`handle_zoom_tool_click`]). This is the
/// counterpart for the other way the same invariant breaks: the active
/// layer's own origin changing moves the boundary out from under a pan
/// that never moved. Switching from a layer at document `(0, 0)` to one
/// at `(300, 150)` leaves `canvas_local_origin` at `(-300, -150)` with
/// no clamp ever running — and a negative local origin is precisely the
/// render/paint divergence `clamp_pan_to_minimum`'s own doc comment
/// describes (`aurora_gpu::TileResidency::set_origin` clamps it to the
/// layer's own corner; `CanvasView::to_document`, which turns a click
/// into the document point a dab lands on, does not).
///
/// The active layer's own origin is not the only way this breaks:
/// *that layer's own `bounds` changing* moves the same boundary without
/// the active layer changing at all (the Move tool, and an undo/redo of
/// one). See [`App::active_layer`]'s own doc comment for the full list
/// of paths that have to re-clamp and where each does it.
///
/// **Ordering matters**: this must run *after* any
/// [`reset_canvas_view`] call, since that resets the pan to `(0, 0)` —
/// which is not within a moved layer's own bound. Callers do not get to
/// choose: [`load_document_view`] is the two of them as one step, and
/// is what the document-open paths call.
fn clamp_pan_to_active_layer(
    view: &mut aurora_ui::CanvasView,
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
) {
    view.clamp_pan_to_minimum(active_layer_origin(layers, active_layer));
}

/// Everything an `Undo`/`Redo` invalidates *outside* the document model
/// itself — run after [`run_command`] has already applied the command.
///
/// Both halves are here for the same reason: either command can
/// revert/reapply a `LayerOp::SetBounds` (`aurora_doc::History::undo`
/// and `::redo` both return the dirtied `Rect`), so the active layer's
/// own origin can move without [`App::active_layer`] itself changing.
/// That invalidates the composite cache (a moved layer's content lands
/// at different composite tiles) *and* the pan bound (which is measured
/// against exactly that origin — [`clamp_pan_to_active_layer`]). The
/// cache half was already unconditional for precisely this reason; the
/// pan half was the missing counterpart, and an undone Move reproduced
/// the same `canvas_local_origin` divergence a layer *switch* does.
///
/// Unconditional rather than "only when the command really was a
/// structural one": both are idempotent no-ops when nothing moved (the
/// clamp leaves a pan already within its bound untouched), and
/// `run_command` deliberately reports nothing back about what it ran.
/// Kept a free function so the pairing is testable with no `App` — and
/// therefore no GPU adapter — to build.
fn after_undo_redo(
    view: &mut aurora_ui::CanvasView,
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    composite_cache: &mut CompositeCache,
) {
    composite_cache.bump();
    clamp_pan_to_active_layer(view, layers, active_layer);
}

/// One `Undo`/`Redo` activation, whole: end whatever drag was still
/// live ([`commit_ending_drag`]), *then* run the command
/// ([`run_command`]), *then* re-establish everything it invalidated
/// outside the document model ([`after_undo_redo`]).
///
/// **The order is the point, which is why this is a function** — the
/// same reason [`press_layer_row`] is one, closing the same hazard at
/// the second site it turned up at (0.57.7). `after_undo_redo` clamps
/// the pan, i.e. it moves the view. A drag in progress holds a
/// document-space reference point fixed at the moment it began
/// (`Drag::Brush`/`Drag::Eraser`'s own `last_doc`, `Drag::Move`/
/// `Drag::Marquee`'s own `start_doc`), so clamping the view out from
/// under one makes the next pointer-move event compute its delta
/// against a view that moved for reasons the drag knows nothing about.
/// For a `Drag::Brush` that is not a cosmetic glitch: undoing while a
/// stroke is still held moved the view `(40, 40)` -> `(340, 190)` and
/// the very next event interpolated **55 dabs across a 335 px line
/// with the pointer completely still** — real paint the user never
/// drew.
///
/// Committing first also closes the same second, older hole
/// `press_layer_row` found in its own branch: the live stroke's pixels
/// were already on the layer with no undo entry naming them, so the
/// very `Undo` that triggered this reached *past* them into the
/// previous step — [`commit_ending_drag`]'s own 0.57.0 bug, at a fourth
/// site. With the commit in place the mid-stroke `Undo` undoes the
/// stroke the user is actually drawing, which is what pressing it means.
///
/// Committing **before** `run_command`, not after, is what makes that
/// true: the commit is what turns the in-progress stroke into the
/// entry the command is then free to reverse. Reversed, the command
/// would undo the step underneath a stroke whose pixels are still
/// live, and the commit would then push an entry whose captured tiles
/// describe content the undo had already replaced.
///
/// Every path that reaches `Undo`/`Redo` with a `Drag` to worry about
/// goes through here ([`App::run_undo_redo`], the `&mut self` wrapper
/// [`App::commit_drag`] is for `commit_ending_drag`). Calling
/// `after_undo_redo` directly instead is what "clamp without first
/// ending any live drag" now costs.
#[allow(clippy::too_many_arguments)]
fn perform_undo_redo(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    tool: &mut aurora_ui::Tool,
    layers: &mut aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    store: Option<&mut aurora_tile::TileStore>,
    undo_order: &mut UndoOrder,
    composite_cache: &mut CompositeCache,
    view: &mut aurora_ui::CanvasView,
    active_layer: Option<aurora_doc::LayerId>,
    drag: &mut Option<Drag>,
    command: AppCommand,
) {
    commit_ending_drag(
        drag.take(),
        layers,
        history,
        pixel_history,
        undo_order,
        view,
        active_layer,
    );
    run_command(
        workspace,
        focus,
        palette,
        tool,
        layers,
        history,
        pixel_history,
        store,
        undo_order,
        command,
    );
    after_undo_redo(view, layers, active_layer, composite_cache);
}

/// The reserved `aurora_tile::SurfaceId` this crate uses for its own
/// live, composited multi-layer preview — never a real layer's own
/// surface. Every real one reuses a `LayerId`'s own raw value
/// (`LayerTree::surface_id`), sequentially allocated from near zero by
/// `aurora_core::IdGenerator`; `u64::MAX` is guaranteed never to
/// collide with one in any document this process could ever build.
#[must_use]
fn composite_surface_id() -> aurora_tile::SurfaceId {
    aurora_tile::SurfaceId::from_raw(u64::MAX)
}

/// `id`'s own `SurfaceId`, computed directly rather than through
/// `LayerTree::surface_id` — a pure conversion of `id`'s own raw value
/// (`aurora_tile::SurfaceId::from_raw(id.to_raw())`, exactly what that
/// method does for a pixel layer), usable from a context like
/// [`begin_drag`] that already knows `id` names a real pixel layer
/// (`active_pixel_layer`'s own contract) without needing a `&LayerTree`
/// reference just to re-derive what the id itself already encodes.
#[must_use]
fn surface_id_for(id: aurora_doc::LayerId) -> aurora_tile::SurfaceId {
    aurora_tile::SurfaceId::from_raw(id.to_raw())
}

/// Converts a real, stored `aurora_doc::BlendMode` (the 27-variant,
/// PSD-round-trippable enum a layer's own `blend_mode` actually holds)
/// into `aurora_render`'s own narrower [`aurora_render::BlendMode`] (26
/// variants: `Normal`, the 8 "simple separable" modes, the 4
/// "dodge and burn" modes, the 7 "overlay and light" modes, the 4
/// non-separable HSL modes, and the 2 whole-colour-selection modes
/// `DarkerColor`/`LighterColor` it has real math for) — the same "app
/// translates a doc-crate type into an engine-crate type at the
/// boundary" pattern [`surface_id_for`] above already establishes for
/// `LayerId` -> `SurfaceId`, needed for the same structural reason:
/// `aurora-render` and `aurora-doc` are sibling crates in PRD §7.2's
/// layering (neither may depend on the other), so neither can name the
/// other's type directly, and `aurora-app` (depending on both) is where
/// the translation has to happen.
///
/// Deliberately an **exhaustive match, no wildcard arm**: every one of
/// `aurora_doc::BlendMode`'s 27 real variants is named individually, 26
/// mapped to their real `aurora_render::BlendMode` counterpart and the
/// remaining 1 (`Dissolve`) explicitly mapped to `Normal` — but that
/// mapped value is **never actually consumed for a real `Dissolve`
/// layer**: `Dissolve` *is* fully implemented (`dissolve_gate`, a real,
/// deterministic, position-seeded stochastic gate — see that function's
/// own doc comment), just not inside `aurora_render`/this translation.
/// [`resolve_tile`] checks a layer's own *raw*, untranslated
/// `aurora_doc::BlendMode` for `Dissolve` in both its `Pixel` and
/// `Group` branches, ahead of ever using this function's own return
/// value, and substitutes the gated result at `(1.0, Normal)` instead —
/// so this arm's `Normal` fallback exists purely so the match stays
/// exhaustive (a future `aurora_doc::BlendMode` variant this crate
/// hasn't implemented yet would need the same kind of explicit,
/// reviewed arm, not a silent wildcard), not because `Dissolve` is
/// still unimplemented. A first pass of the `Dissolve` feature only
/// added the interception to the `Pixel` branch, leaving a *group's*
/// own `Dissolve` blend mode silently falling back to this function's
/// `Normal` mapping for real — an independent review caught the gap
/// before it shipped; both branches now intercept it symmetrically.
/// Exhaustiveness matters here specifically: a wildcard `_ => Normal`
/// arm would make a *future* `aurora_doc::BlendMode` variant compile
/// silently into an unreviewed `Normal` fallback forever; without one,
/// the compiler forces this function itself to be revisited the next
/// time either enum grows.
#[must_use]
// `clippy::match_same_arms` wants the literal `Normal` arm merged into
// the identical-bodied fallback arm below it — rejected deliberately:
// collapsing them would blur the one distinction this function exists
// to keep legible, "this variant's own real mapping is Normal" versus
// "this variant has no real mapping yet, so it falls back to Normal",
// even though both currently produce the same value. Every arm still
// names its own variant explicitly rather than using a wildcard, so
// exhaustiveness checking is unaffected by this allow.
#[allow(clippy::match_same_arms)]
const fn translate_blend_mode(mode: aurora_doc::BlendMode) -> aurora_render::BlendMode {
    match mode {
        aurora_doc::BlendMode::Normal => aurora_render::BlendMode::Normal,
        aurora_doc::BlendMode::Darken => aurora_render::BlendMode::Darken,
        aurora_doc::BlendMode::Multiply => aurora_render::BlendMode::Multiply,
        aurora_doc::BlendMode::Lighten => aurora_render::BlendMode::Lighten,
        aurora_doc::BlendMode::Screen => aurora_render::BlendMode::Screen,
        aurora_doc::BlendMode::Difference => aurora_render::BlendMode::Difference,
        aurora_doc::BlendMode::Exclusion => aurora_render::BlendMode::Exclusion,
        aurora_doc::BlendMode::Subtract => aurora_render::BlendMode::Subtract,
        aurora_doc::BlendMode::Divide => aurora_render::BlendMode::Divide,
        aurora_doc::BlendMode::ColorDodge => aurora_render::BlendMode::ColorDodge,
        aurora_doc::BlendMode::LinearDodge => aurora_render::BlendMode::LinearDodge,
        aurora_doc::BlendMode::ColorBurn => aurora_render::BlendMode::ColorBurn,
        aurora_doc::BlendMode::LinearBurn => aurora_render::BlendMode::LinearBurn,
        aurora_doc::BlendMode::Overlay => aurora_render::BlendMode::Overlay,
        aurora_doc::BlendMode::SoftLight => aurora_render::BlendMode::SoftLight,
        aurora_doc::BlendMode::HardLight => aurora_render::BlendMode::HardLight,
        aurora_doc::BlendMode::VividLight => aurora_render::BlendMode::VividLight,
        aurora_doc::BlendMode::LinearLight => aurora_render::BlendMode::LinearLight,
        aurora_doc::BlendMode::PinLight => aurora_render::BlendMode::PinLight,
        aurora_doc::BlendMode::HardMix => aurora_render::BlendMode::HardMix,
        aurora_doc::BlendMode::Hue => aurora_render::BlendMode::Hue,
        aurora_doc::BlendMode::Saturation => aurora_render::BlendMode::Saturation,
        aurora_doc::BlendMode::Color => aurora_render::BlendMode::Color,
        aurora_doc::BlendMode::Luminosity => aurora_render::BlendMode::Luminosity,
        aurora_doc::BlendMode::DarkerColor => aurora_render::BlendMode::DarkerColor,
        aurora_doc::BlendMode::LighterColor => aurora_render::BlendMode::LighterColor,
        // `Dissolve` has no real mapping here, on purpose, permanently —
        // not a placeholder waiting on `aurora_render::BlendMode` to grow
        // a variant. Dissolve is stochastic per-pixel selection, not a
        // per-pixel-color blend *function* at all (it never computes a
        // new colour from source+backdrop the way every other variant
        // above does), so it was never going to fit this enum. The real,
        // deterministic implementation lives one level up, in
        // `resolve_tile`, which inspects a `Pixel` layer's own raw
        // `aurora_doc::BlendMode` *before* calling this function at all:
        // when it's `Dissolve`, `resolve_tile` runs `dissolve_gate`
        // itself and returns `(gated_texels, 1.0, aurora_render::
        // BlendMode::Normal)` directly, so this arm is never actually
        // reached for a real Dissolve layer today. It stays mapped to
        // `Normal` here anyway — matching every other still-open arm's
        // "safe, honest fallback" shape — purely so this match stays
        // exhaustive and so any *other*, hypothetical future caller of
        // `translate_blend_mode` that doesn't do `resolve_tile`'s own
        // Dissolve interception first still gets a defined (if
        // non-stochastic) answer instead of a compile error or a panic.
        aurora_doc::BlendMode::Dissolve => aurora_render::BlendMode::Normal,
    }
}

/// `SplitMix64` — Sebastiano Vigna's well-known, widely-used 64-bit
/// state-advance/output-mixing function (the generator originally paired
/// with `xorshift128+`'s own seeding step, and the same algorithm behind
/// Java's `SplittableRandom`; public-domain reference:
/// <https://prng.di.unimi.it/splitmix64.c>). Used here purely as a
/// stateless *hash*, not a stream generator: fed a seed built from a
/// pixel's own absolute document-space position (see [`hash_position`]),
/// it returns a value indistinguishable from uniform noise for that
/// position, with no internal state carried between calls — the same
/// seed always yields the same output, which is exactly the determinism
/// [`dissolve_gate`] needs (see `resolve_tile`'s own doc comment for why
/// determinism matters here at all).
///
/// This is `seed.wrapping_add(GAMMA)` followed by the reference
/// generator's own three xor-shift-multiply rounds — i.e. calling this
/// with `seed = x` reproduces exactly the reference C generator's
/// `next()` output for a generator whose internal state starts at `x`
/// (the C code increments state *then* mixes, on every call, including
/// the first). Verified in this module's own tests against real output
/// values independently re-derived from that reference algorithm (a
/// from-scratch Python re-implementation, not by trusting this
/// function's own Rust output) for seed `0` and 3 other seeds — not just
/// self-consistency.
const fn splitmix64(seed: u64) -> u64 {
    let z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Maps a signed document-space coordinate onto `u64` bijectively via
/// zigzag encoding — the same scheme protobuf varints use for signed
/// fields, and standard wherever a signed value needs to become an
/// unsigned one without losing any information or colliding two distinct
/// inputs: `0, -1, 1, -2, 2, ...` -> `0, 1, 2, 3, 4, ...`. Needed because
/// [`hash_position`]'s combination step wants an unsigned value to
/// multiply, and document coordinates are signed (`resolve_tile`'s own
/// `doc_origin: (i64, i64)`, which goes negative once a layer or the
/// canvas view has scrolled past the document origin).
///
/// `n << 1` cannot overflow-panic — Rust's arithmetic-overflow checks
/// apply to `+`/`-`/`*`, not to bits shifted out of a left shift, so this
/// is safe for every `i64` value including `i64::MIN`/`i64::MAX`. The
/// final `as u64` is an intentional same-width bit reinterpretation (the
/// whole point of zigzag encoding), not a lossy truncation — hence the
/// scoped `allow` rather than a workspace-wide one.
#[allow(clippy::cast_sign_loss)]
const fn zigzag_encode(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

/// Combines one absolute document-space pixel position `(x, y)` into the
/// single `u64` seed [`splitmix64`] hashes — [`dissolve_gate`]'s own
/// "your call, state it precisely" combination. Each axis is
/// zigzag-encoded (see [`zigzag_encode`]) then multiplied by its own odd,
/// high-bit-set 64-bit constant (`x`'s and `y`'s constants differ — the
/// first is `splitmix64`'s own golden-ratio gamma, the second is
/// `splitmix64`'s own `0x94D049BB133111EB` output-mix constant rotated
/// into a different-looking odd constant by convention, `0xC2B2AE3D27D4EB4F`,
/// widely reused across hashing libraries as a second Fibonacci-hashing
/// multiplier for exactly this "combine two already-good values without
/// them cancelling" purpose); the two products are combined with XOR and
/// the result is run through `splitmix64` once more to finish mixing.
///
/// **Why two different constants, not a direct
/// `zigzag_encode(x) ^ zigzag_encode(y)`**: XOR is commutative, so a
/// naive combine of that shape would make `(x, y)` and `(y, x)` hash
/// *identically* for every pair of distinct coordinates, not just `x ==
/// y` — a real, visible defect for a 2D dissolve pattern, since it would
/// mirror the whole noise field across the diagonal. Multiplying each
/// axis by its own distinct constant before combining breaks that
/// symmetry: `hash_position(3, 7) != hash_position(7, 3)` (checked
/// directly in this module's own tests), so the noise field is genuinely
/// 2-dimensional, not foldable onto one diagonal-symmetric axis.
const fn hash_position(x: i64, y: i64) -> u64 {
    let xu = zigzag_encode(x).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let yu = zigzag_encode(y).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    splitmix64(xu ^ yu)
}

/// Converts a `splitmix64`/[`hash_position`] output into a uniform `f32`
/// in `[0, 1)`. Takes the top 24 bits of the 64-bit hash (`f32`'s own
/// mantissa precision: 1 implicit + 23 explicit bits — every integer up
/// to `2^24` is exactly representable) as an integer in `[0, 2^24)`, then
/// divides by `2^24`. Deliberately not a naive `hash as f32 /
/// u64::MAX as f32`: that keeps every one of the 64 input bits nominally
/// "in play" but throws almost all of them away to rounding once packed
/// into an `f32`'s far narrower mantissa, unevenly across the range —
/// taking the top N bits first and scaling is the standard technique
/// (the same shape general-purpose PRNGs' own `f32`/`f64` output helpers
/// use, e.g. `xoshiro`'s).
///
/// The maximum possible `top_bits` value is `2^24 - 1`, so the maximum
/// possible return value is `(2^24 - 1) / 2^24`, strictly less than
/// `1.0` — the output range really is the standard half-open `[0, 1)`,
/// not `[0, 1]`, which matters for [`dissolve_gate`]'s `opacity = 1.0`
/// edge case (see its own doc comment).
fn hash_to_unit_f32(hash: u64) -> f32 {
    const MANTISSA_BITS: u32 = 24;
    let top_bits = hash >> (u64::BITS - MANTISSA_BITS);
    top_bits as f32 / (1u64 << MANTISSA_BITS) as f32
}

/// Applies [`aurora_doc::BlendMode::Dissolve`]'s own stochastic,
/// per-pixel opaque-or-nothing gate to one `Pixel` layer's own
/// straight-alpha `texels`, ahead of ever handing them to
/// `aurora_render::composite_tile_cpu` — see `resolve_tile`'s own doc
/// comment for why this happens here, in `aurora-app`, rather than as a
/// new `aurora_render::BlendMode` variant: Dissolve isn't a per-pixel-
/// color blend *function* the way every other mode is (it never computes
/// a new colour from source+backdrop), it's a binary visibility decision
/// made once per pixel, weighted by that pixel's own effective alpha
/// (`texel_alpha * opacity`) as a probability — real Photoshop
/// semantics, not a smooth opacity fade: a layer at 30% opacity shows
/// ~30% of its pixels at full strength and ~70% not at all, not every
/// pixel at 30%-transparent everywhere.
///
/// **Determinism, precisely**: the "random" decision for a texel is a
/// pure function of that texel's own *absolute document-space* position
/// — `doc_origin` plus its own row/col within the `aurora_tile::TILE`
/// grid `texels` covers, **never** `tile_id` or any tile-relative
/// coordinate alone — via [`hash_position`]/[`splitmix64`]. No RNG
/// state, no system time, nothing thread-local: the same absolute
/// position always produces the same noise value, so the same document
/// composites bit-identically every time, regardless of which tile-grid
/// alignment happens to be showing (a scrolled view, a re-render, an
/// export, a reopened file). `doc_origin` alone is sufficient to recover
/// every texel's true absolute position — that is what it already means
/// (`resolve_tile`'s own doc comment) — so this function deliberately
/// takes no `tile_id` parameter at all: threading one through would
/// invite exactly the tile-relative mistake this design has to avoid.
/// Using a tile-relative coordinate instead of an absolute one would
/// make the dissolve pattern visibly repeat, identically, at every
/// `TILE`-pixel boundary — precisely the artifact this function exists
/// to prevent, however a caller's own `doc_origin`/`tile_id` pair happens
/// to be constructed.
///
/// `texels` is one layer's own straight-alpha buffer for the
/// `aurora_tile::TILE`×`aurora_tile::TILE` window at `doc_origin`
/// (row-major, `aurora_tile::CHANNELS` `half::f16` samples per texel —
/// the same layout `aurora_tile::TileStore`/`composite_tile_cpu` already
/// use, confirmed against `aurora_tile::store`'s own `(y * TILE + x) *
/// CHANNELS` indexing). Each texel's outcome: `noise < texel_alpha *
/// opacity` shows the source at full strength (its own RGB unchanged,
/// alpha forced to `1.0`); otherwise it comes back fully transparent
/// (`(0, 0, 0, 0)`). The returned buffer is always the same length as
/// `texels`; any chunk that isn't a full `CHANNELS`-length texel (should
/// never happen for a real tile buffer, but this function has no way to
/// prove that of its caller) is skipped, left fully transparent in the
/// output, the same defensive shape
/// `aurora_render::un_premultiply_in_place` (the un-premultiply step
/// `resolve_tile`'s `Group` arm calls) already uses for its own
/// `chunks_exact_mut` loop.
///
/// `resolve_tile` calls this from both its `Pixel` and `Group` arms, so a
/// `Dissolve`-mode group containing a `Dissolve`-mode child is possible.
/// Because the noise is a pure function of absolute position only (never
/// layer identity), both gates draw the *same* noise value at a given
/// pixel — the two thresholds are correlated, not independent, so the
/// visible fraction converges toward `min(child_alpha, group_alpha)`
/// rather than their product. This is real, deliberate behaviour (not a
/// bug: independent per-layer noise would need per-layer seeding, which
/// would break the position-only reproducibility this function exists
/// for), just an obscure enough authoring combination that it is called
/// out here rather than covered by its own test.
fn dissolve_gate(texels: &[half::f16], opacity: f32, doc_origin: (i64, i64)) -> Vec<half::f16> {
    let tile_side = i64::from(aurora_tile::TILE);
    let mut gated = vec![half::f16::from_f32(0.0); texels.len()];
    for (index, (src, dst)) in texels
        .chunks_exact(aurora_tile::CHANNELS)
        .zip(gated.chunks_exact_mut(aurora_tile::CHANNELS))
        .enumerate()
    {
        let [r, g, b, a] = src else { continue };
        let [dst_r, dst_g, dst_b, dst_a] = dst else {
            continue;
        };
        let Ok(index) = i64::try_from(index) else {
            continue;
        };
        let col = index % tile_side;
        let row = index / tile_side;
        let abs_x = doc_origin.0 + col;
        let abs_y = doc_origin.1 + row;

        let effective_alpha = a.to_f32() * opacity;
        let noise = hash_to_unit_f32(hash_position(abs_x, abs_y));

        if noise < effective_alpha {
            *dst_r = *r;
            *dst_g = *g;
            *dst_b = *b;
            *dst_a = half::f16::from_f32(1.0);
        }
        // else: leave fully transparent -- `gated` is already
        // zero-initialized above, and that is exactly `(0, 0, 0, 0)`.
    }
    gated
}

/// Clips one layer's own straight-alpha `texels` (the same
/// `aurora_tile::TILE`×`aurora_tile::TILE`-window-at-`doc_origin` buffer
/// shape [`dissolve_gate`] takes — row-major, `aurora_tile::CHANNELS`
/// `half::f16` samples per texel) to `mask`'s own rectangular
/// `mask.bounds`, which — like a [`aurora_doc::LayerKind::Pixel`]
/// layer's own `bounds` — is already document-absolute, so each texel's
/// absolute position is recovered the same way [`dissolve_gate`] already
/// does: `doc_origin` plus its own row/col within the tile. A texel whose
/// absolute position falls inside `mask.bounds`
/// ([`aurora_core::Rect::contains_point`]) passes through unchanged;
/// outside it, it comes back fully transparent (`(0, 0, 0, 0)`, the same
/// "hidden" convention [`dissolve_gate`] uses). `mask.inverted` flips
/// which side is shown — XOR'd against the containment test, mirroring
/// the two toggles [`aurora_doc::LayerMask`] itself exposes
/// (`LayerTree::set_mask_enabled`/`set_mask_inverted`).
///
/// **A rectangular clip — not real grayscale masking.**
/// [`aurora_doc::LayerMask`] carries no per-pixel mask data yet (its own
/// doc comment explains why: the same one-`TileStore`-per-layer-vs-
/// shared resource-management question [`aurora_doc::LayerKind::Pixel`]'s
/// own `bounds` field already flags, not yet decided), so there is
/// nothing to sample per-pixel beyond the mask's own bounding rectangle —
/// no feathering, no soft edges, no partial coverage. A texel is either
/// fully shown or fully hidden; there is no in-between, and this function
/// does not pretend otherwise.
///
/// Callers are responsible for checking `mask.enabled` themselves before
/// calling this — a disabled mask should never reach here at all (see
/// `resolve_tile`'s own call sites, which only call this when
/// `mask.enabled` is true). Any chunk that isn't a full
/// `CHANNELS`-length texel is skipped, left fully transparent in the
/// output — the same defensive shape [`dissolve_gate`] and
/// `aurora_render::un_premultiply_in_place` (the un-premultiply step
/// `resolve_tile`'s `Group` arm calls) both already use.
fn apply_mask_clip(
    texels: &[half::f16],
    mask: &aurora_doc::LayerMask,
    doc_origin: (i64, i64),
) -> Vec<half::f16> {
    let tile_side = i64::from(aurora_tile::TILE);
    let mut clipped = vec![half::f16::from_f32(0.0); texels.len()];
    for (index, (src, dst)) in texels
        .chunks_exact(aurora_tile::CHANNELS)
        .zip(clipped.chunks_exact_mut(aurora_tile::CHANNELS))
        .enumerate()
    {
        let [r, g, b, a] = src else { continue };
        let [dst_r, dst_g, dst_b, dst_a] = dst else {
            continue;
        };
        let Ok(index) = i64::try_from(index) else {
            continue;
        };
        let col = index % tile_side;
        let row = index / tile_side;
        let abs_x = doc_origin.0 + col;
        let abs_y = doc_origin.1 + row;

        let inside = mask.bounds.contains_point(abs_x, abs_y);
        let shown = inside != mask.inverted;

        if shown {
            *dst_r = *r;
            *dst_g = *g;
            *dst_b = *b;
            *dst_a = *a;
        }
        // else: leave fully transparent -- `clipped` is already
        // zero-initialized above, and that is exactly `(0, 0, 0, 0)`.
    }
    clipped
}

/// The shared, per-composite-pass half of [`resolve_tile`]'s recursion
/// bounds: a monotone node budget, plus a one-shot "already reported"
/// flag. `depth`, the other half, stays a plain by-value parameter,
/// because depth has to unwind as the recursion returns where these two
/// deliberately must not — what bounds a fan-out cycle is what has
/// already been spent, not what is on the stack right now.
///
/// **Why the budget is the document's own layer count.** In any tree
/// `aurora-doc` accepts, every layer is reachable from
/// `aurora_doc::LayerTree::roots` at most once — `validate_shape`
/// rejects an id named by two parents, or twice by one — so a whole
/// tile's composite (every root walked, every descendant visited)
/// enters [`resolve_tile`] at most `aurora_doc::LayerTree::len()`
/// times. The budget is exactly that count, which makes it both
/// impossible for a well-formed document to be truncated by it and
/// tight enough that a duplicate-reachability cycle runs out almost
/// immediately. A fixed constant was the alternative and is the wrong
/// tool here: PRD §6 promises *unlimited* layers, so any constant is
/// either a real ceiling on a legitimate document or so far above one
/// that it stops bounding anything. (Contrast `AUTOSAVE_EDIT_THRESHOLD`
/// and the other tuned constants in this crate, which trade off two
/// costs and have no derivable right answer. This one does.)
///
/// The node count is recomputed per tile, via [`Self::next_tile`] —
/// it bounds one tile's work, not a whole pass's. `reported` is
/// deliberately *not* reset with it: [`recomposite_visible_tiles`]
/// calls [`resolve_tile`] once per invalidated tile per frame, so a
/// per-breach `tracing::warn!` on a malformed document would fire at
/// interactive-rate frequency on a path already over its latency
/// budget (CLAUDE.md's own measurements). One report per pass is all
/// the signal there is anyway — every tile after the first is
/// re-reporting the same broken tree.
#[derive(Debug)]
struct CompositeBudget {
    /// How many more times [`resolve_tile`] may be entered for the tile
    /// currently being composited. Charged on entry, never refunded.
    nodes: usize,
    /// Whether either bound has already been reported during this
    /// composite pass. See the type's own doc comment for why this
    /// survives [`Self::next_tile`].
    reported: bool,
    /// How many layer tiles this whole pass skipped because
    /// `aurora_tile::TileStore::get` failed — see [`Self::note_store_error`]
    /// for why this rides along here. Per *pass*, so like `reported` it
    /// deliberately survives [`Self::next_tile`].
    store_errors: usize,
    /// The first such failure's own message, kept because
    /// `aurora_tile::TileError` is neither `Clone` nor cheap to hold on
    /// to while the store it borrows from is still being mutated.
    first_store_error: Option<String>,
}

impl CompositeBudget {
    /// A budget for the first tile of a fresh composite pass over
    /// `layers`, with nothing reported yet.
    fn for_pass(layers: &aurora_doc::LayerTree) -> Self {
        Self {
            nodes: layers.len(),
            reported: false,
            store_errors: 0,
            first_store_error: None,
        }
    }

    /// Recharges the node budget for the next tile of the same pass.
    /// Leaves `reported` alone on purpose — that is what keeps the
    /// warning to once per pass rather than once per tile.
    fn next_tile(&mut self, layers: &aurora_doc::LayerTree) {
        self.nodes = layers.len();
    }

    /// Charges one visited node, returning `false` once the budget for
    /// this tile is spent. `checked_sub` rather than a comparison plus a
    /// decrement so that exhaustion and underflow are the same branch.
    fn charge_node(&mut self) -> bool {
        match self.nodes.checked_sub(1) {
            Some(remaining) => {
                self.nodes = remaining;
                true
            }
            None => false,
        }
    }

    /// `true` exactly once per composite pass, for the first bound
    /// breach it sees; `false` for every breach after that.
    fn should_report(&mut self) -> bool {
        let first = !self.reported;
        self.reported = true;
        first
    }

    /// Records that one layer's tile could not be read out of the store
    /// and was therefore skipped — that layer contributed *nothing* to
    /// the tile being composited, rather than its real pixels.
    ///
    /// **Why this lives on the budget.** It is the one piece of state
    /// already threaded, by `&mut`, through every frame of
    /// [`resolve_tile`]'s recursion and back out to both of its
    /// top-level callers, and it is already the pass's diagnostics
    /// carrier (`reported`), not purely a bound. Giving the skip count a
    /// second `&mut` parameter of its own would have meant the same
    /// thread-through with two things to keep in step.
    ///
    /// The two callers then do deliberately different things with it.
    /// [`recomposite_visible_tiles`] ignores it: the live canvas must
    /// keep painting what it can, because hard-failing every repaint
    /// over one corrupt scratch-disk tile is far worse to use than a
    /// visibly missing layer plus a log line, and that graceful-degrade
    /// behaviour long predates this. [`composite_document`] — the
    /// export/save path — checks it and refuses to hand back an
    /// `aurora_io::Image` that is quietly missing content, because a
    /// *file* written that way is the failure CLAUDE.md names as the
    /// worst this project can have.
    fn note_store_error(&mut self, err: &aurora_tile::TileError) {
        self.store_errors = self.store_errors.saturating_add(1);
        if self.first_store_error.is_none() {
            self.first_store_error = Some(err.to_string());
        }
    }

    /// `Some((how many skips, the first one's message))` if any layer
    /// tile was skipped during this pass because the store could not
    /// read it; `None` if every tile read cleanly. See
    /// [`Self::note_store_error`].
    fn store_error(&self) -> Option<(usize, &str)> {
        let first = self.first_store_error.as_deref()?;
        Some((self.store_errors, first))
    }

    /// Whether this tile's node budget is already spent. Lets a sibling
    /// loop stop calling [`resolve_tile`] once every remaining call would
    /// just re-fail [`Self::charge_node`] — without this, a group whose
    /// `children` names far more entries than the tree actually holds
    /// (the exact shape [`resolve_tile`]'s own doc comment names as the
    /// thing this budget exists to bound) still costs one no-op call per
    /// listed child rather than stopping at the first one, quadratic
    /// rather than linear in a crafted tree's own size.
    fn is_exhausted(&self) -> bool {
        self.nodes == 0
    }
}

/// Resolves `id`'s own composited texels for one `aurora_tile::TILE`-
/// sized window at document-space `doc_origin` (`tile_id`'s own meaning
/// once converted out of `reference_origin`'s frame — see
/// [`recomposite_visible_tiles`]'s own doc comment) — the shared
/// recursive worker both [`recomposite_visible_tiles`] and
/// [`composite_document`] call once per root-level visible entry
/// (`aurora_doc::LayerTree::roots`), so the "a group composites in
/// isolation" recursion below lives in exactly one place rather than
/// two independently-drifting copies.
///
/// For a [`aurora_doc::LayerKind::Pixel`] layer: its own texels, read
/// directly from `store` when its `bounds` origin already matches
/// `reference_origin`, or re-tiled via [`read_layer_window`] when it
/// doesn't — exactly what every existing call site already did for one
/// layer in a flat list, unchanged.
///
/// For a [`aurora_doc::LayerKind::Group`]: its own **isolated**
/// composite of its visible, direct children only
/// ([`aurora_doc::LayerTree::children`], **not**
/// [`aurora_doc::LayerTree::paint_order`] — a nested group's own
/// contents stay scoped to their own immediate parent's compositing
/// pass rather than unpacking into a grandparent's), bottom-to-top,
/// each recursively resolved by this same function and folded, one at
/// a time, into a single running buffer that starts fully transparent
/// (`aurora_render::transparent_tile`) via
/// `aurora_render::composite_layer_into` — the same per-layer primitive
/// this function's own caller folds *this* result into one level up.
/// Folding in place rather than collecting every child's own full
/// 512 KiB tile buffer first is what keeps a group's peak memory
/// proportional to the tree's *depth* instead of to one group's sibling
/// count; it is bit-identical to the batch
/// `aurora_render::composite_tile_cpu` call it replaces, because that
/// primitive's own loop body reads no state beyond the accumulator, the
/// source, the opacity and the mode — so N folds and one batch call are
/// the same computation by construction (see the `Group` arm's own
/// comment below, and `composite_layer_into`'s doc comment, for what
/// does and does not count as evidence for that). A child that is
/// itself a group is resolved by
/// recursing into this branch again, so nesting falls out for free (up
/// to the two bounds below — every tree `aurora-doc` will accept stays
/// inside both, so no well-formed document is affected).
///
/// **`depth` and `budget`, the two recursion bounds — defence in
/// depth, not a duplicate of `aurora-doc`'s validation.** They bound
/// two different things, and neither alone is enough, which is why both
/// are here.
///
/// `depth` bounds how *deep* the recursion goes. Callers pass `1` for a
/// root-level entry, the same seed `aurora_doc`'s own `validate_shape`
/// uses for a manifest's `roots` ("these roots really are the top
/// level, so the depth budget starts from scratch"), and each recursive
/// step below adds one. The check is
/// `depth > `[`aurora_doc::MAX_LAYER_TREE_DEPTH`] — strictly greater,
/// matching that validator's own comparison exactly, so a legitimately
/// 256-deep tree still composites its deepest layer. What that bounds
/// is stack frames, and nothing else.
///
/// `budget` bounds how much *total work* one tile's composite may do,
/// which `depth` provably does not. The recursive call below sits
/// inside a loop over a group's `children`, so a node's own call count
/// is `children.len()`, not one. A group whose `children` names the
/// same id more than once — exactly the duplicate-reachability shape
/// `aurora-doc`'s `validate_shape` checks for, and the shape three
/// successive review rounds found real bugs in — therefore *branches*
/// rather than walks: at fan-out two the visit count doubles per level,
/// `2^slack` visits from however far above the bound the cycle is
/// entered, which finishes at no realistic slack. `depth` does still
/// terminate that traversal in the formal sense, but "terminates" would
/// mean a hang on the compositing thread that cannot be interrupted
/// from inside, rather than the stack-overflow abort this guard exists
/// to prevent — no better for a user holding unsaved work. So
/// [`CompositeBudget`] charges one node per entry into this function
/// and never refunds it on return, bounding total visits per tile
/// outright whatever shape the tree has.
///
/// Both bounds return `None` when breached, which the caller already
/// treats as "this contributor is absent from the composite" (the same
/// contract an invisible layer or a failed tile load already uses), so
/// an over-deep or over-budget branch is dropped rather than the tile
/// being aborted.
///
/// This exists **independently** of `aurora-doc`'s shape validation
/// rather than trusting it: that validator has had real, separately
/// exploitable gaps found in it across successive review rounds, and a
/// cycle reaching this function would otherwise recurse until the stack
/// overflows — an abort, which for an image editor holding unsaved work
/// is the worst possible failure.
///
/// Two monotone counters rather than a visited set, still: a set would
/// need correct remove-on-return discipline (get it wrong and
/// legitimate sibling subtrees silently vanish) and would allocate per
/// branch in the hot compositing path. A monotone budget is strictly
/// cheaper than that and terminates just as hard; the one thing it
/// gives up is telling a cycle apart from a legitimately enormous
/// document, and sizing it from the document itself is what pays that
/// back — see [`CompositeBudget`].
///
/// **No "Pass Through" mode**: `aurora_doc::BlendMode`'s 27 variants
/// have no such variant — Photoshop's real distinction between an
/// isolated and a pass-through group isn't modeled in this schema at
/// all. Given that, isolating *every* group, always, is the only
/// semantic this data model can actually express, so that is what this
/// function implements — a deliberate, documented simplification, not
/// an oversight; real Pass Through semantics (if ever wanted) would
/// need a new field on [`aurora_doc::LayerKind::Group`] first, which is
/// separate, still-open follow-on work.
///
/// The returned `(texels, opacity, blend_mode)` are **`id`'s own,
/// unapplied** — the caller (whichever level actually contains `id`:
/// the document root, or a parent group's own recursive call into this
/// function) is the one that applies them, via its own
/// `aurora_render::composite_layer_into` call.
///
/// **[`aurora_doc::BlendMode::Dissolve`] is the one exception to "this
/// function never applies anything itself"**: for a
/// [`aurora_doc::LayerKind::Pixel`]
/// layer whose own raw, untranslated blend mode is `Dissolve`, this
/// function intercepts it *before* `translate_blend_mode` ever runs,
/// applies [`dissolve_gate`] to that layer's own texels using its own
/// real opacity, and returns the gated result already at
/// `(opacity = 1.0, blend_mode = Normal)` — Dissolve's stochastic,
/// position-weighted visibility decision is not a per-pixel-color blend
/// *function* `aurora_render::composite_tile_cpu` could express at all
/// (every other mode computes a new colour from source+backdrop;
/// Dissolve picks, per pixel, either the source untouched or nothing),
/// so it has to be resolved here, where real document/tile coordinates
/// are available, rather than inside that crate. See `dissolve_gate`'s
/// own doc comment for the full design and its determinism guarantee.
/// Every other blend mode is unaffected: this check only ever matches
/// `Dissolve` specifically, and only on the `Pixel` branch below — a
/// `Group`'s own `blend_mode` field is never inspected for `Dissolve`
/// this way (see that branch's own code for why: a group has no single
/// buffer of its own texels to gate until *after* its isolated composite
/// already ran, a genuinely different shape this round didn't need to
/// solve).
///
/// **[`aurora_doc::LayerMask`] aggregation, the other real gap this
/// function used to leave open**: both branches below now check
/// `layers.mask(id)` and, when a mask is present and `mask.enabled`,
/// clip that branch's own texels through [`apply_mask_clip`] before
/// anything else touches them — for `Pixel`, right after reading
/// `texels`, ahead of the `Dissolve` interception above; for `Group`,
/// right after the un-premultiply step below, on the group's own
/// isolated buffer as a single unit, ahead of that branch's own
/// `Dissolve` interception. That ordering matters: the mask restricts
/// *which* pixels are even in play first, and `Dissolve` (or any other
/// blend mode) only ever acts within whatever the mask left visible — a
/// masked-out pixel never gets a chance to win the stochastic gate,
/// because it never reaches `dissolve_gate` with nonzero alpha. A
/// disabled mask (`mask.enabled == false`) is skipped entirely, same as
/// having no mask at all. **Rectangular clip only, stated the same way
/// [`apply_mask_clip`]'s own doc comment states it**: `LayerMask` has no
/// per-pixel mask data yet, so this is a hard inside/outside test against
/// `mask.bounds`, not real grayscale masking — no feathering, no soft
/// edges.
///
/// **The regression-safety property, precisely**: `composite_tile_cpu`
/// already reproduces a single full-opacity (`opacity = 1.0`) layer's
/// own texels bit-exactly (its own doc comment; also
/// [`recomposite_visible_tiles`]'s own doc comment names this). A group
/// whose own isolated content is therefore **fully opaque wherever it
/// isn't fully transparent** — trivially true for the schema's own
/// default, a single visible child left at *its own* default opacity —
/// composites identically to today's pre-recursion flattening: applying
/// the group's own default `opacity = 1.0`/`blend_mode = Normal` one
/// level up is then a bit-exact round trip through that same "reproduces
/// a full-opacity layer exactly" property, applied to the isolated
/// buffer as if it were one flat layer. This is the regression safety
/// net for every document with no groups, or only default-settings
/// groups wrapping default-settings (fully opaque) content.
///
/// **A group whose isolated content has fractional alpha anywhere** (a
/// lone child at `opacity < 1`, a feathered/partially-transparent edge,
/// or overlapping semi-transparent children) needs more than that
/// round trip, because `composite_tile_cpu` accumulates straight-alpha
/// "over" math onto a starting-*transparent* destination, which
/// produces a **premultiplied** result whenever the accumulated alpha
/// ends up fractional — confirmed by direct calculation, not assumed: a
/// lone child at `opacity = 0.5` isolated alone onto a transparent
/// backdrop yields `(0.0, 0.0, 0.5, 0.5)`, not that child's own straight
/// `(0.0, 0.0, 1.0, 1.0)` at reduced alpha. Handing that premultiplied
/// buffer back up as if it were straight-alpha texels — what every
/// other branch of this function actually returns, and what the
/// caller's own `composite_layer_into` call one level up actually expects
/// — would double-attenuate the colour on the next pass. **Fixed**: the
/// code below this doc comment un-premultiplies the isolated buffer
/// (dividing `r`/`g`/`b` by `a`, guarded against `a == 0.0`) before
/// returning it, so a group's own isolated content is always handed
/// back as true straight alpha. Verified this closes the gap for the
/// common case — a single child of any opacity, or multiple children
/// combining via `Normal` — where un-premultiplying reproduces the
/// exact flat (non-isolated) result: e.g. `(0, 0, 0.5, 0.5)` →
/// un-premultiply → `(0, 0, 1.0, 0.5)`, and re-compositing that at the
/// group's own default settings over an opaque white backdrop gives
/// `(0.5, 0.5, 1.0, 1.0)`, bit-for-bit what direct (non-isolated)
/// compositing of the same content gives.
///
/// **Now fixed at its actual source, one level below this function**:
/// when a group's own children combine via a **non-`Normal` blend
/// mode** against each other (e.g. one child set to `Multiply` against
/// another child, both inside the same isolation pass),
/// `composite_tile_cpu`'s own `blend_channel`/`blend_rgb` math runs
/// *during* accumulation, against whatever the accumulating buffer's
/// `Cb` (backdrop) value already is at that intermediate step — which,
/// partway through a still-translucent accumulation, is not yet a true
/// straight colour (the un-premultiply step above, which only runs once
/// this function's own isolation pass has finished, cannot retroactively
/// correct blend math that already ran mid-pass against a premultiplied
/// intermediate — a deeper limitation of `composite_tile_cpu`'s own
/// accumulate-in-place design, not something fixable from `resolve_tile`
/// alone). `composite_tile_cpu` itself now recovers the backdrop's true
/// straight-alpha colour — dividing the running accumulator's `r`/`g`/`b`
/// by its own `a`, guarded against `a == 0.0` the same way
/// `aurora_render::un_premultiply_in_place` already is — before ever handing it to
/// `blend_rgb` as `Cb`, so every blend mode now reacts to the correct
/// backdrop colour regardless of how translucent the accumulator is
/// partway through a group's isolation pass. See `composite_tile_cpu`'s
/// own doc comment and
/// `composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator`
/// (`aurora-render`) for the mechanism and a worked example, and
/// `composite_document_blends_two_group_children_via_a_non_normal_blend_mode_against_a_translucent_backdrop`
/// below for this fix verified end to end through a real group.
///
/// `None` if `id` doesn't exist, isn't visible, or (for a
/// [`aurora_doc::LayerKind::Pixel`]) its tile fails to load — the same
/// "one bad tile shouldn't abort the rest" discipline
/// [`recomposite_visible_tiles`]/[`composite_document`] already use:
/// the caller simply omits this contributor from its own composite
/// rather than propagating the error further.
///
/// **Origin handling doesn't get any harder with nesting**:
/// [`aurora_doc::LayerKind::Group`] has no `bounds`/offset of its own —
/// every [`aurora_doc::LayerKind::Pixel`] layer's own `bounds` is
/// already document-absolute regardless of how deeply it's nested — so
/// `(tile_id, doc_origin, reference_origin)` are threaded through every
/// recursive call completely unchanged; no new origin logic is needed
/// beyond what a `Pixel` layer's own branch already had.
// Eight arguments, and the alternative is worse: the seven that were
// here before are all genuinely per-call, and `budget` is genuinely
// shared, so bundling any of them into a struct would only move the
// same values behind a name that explains less than they do.
//
// Over 100 lines by three, since the two bound checks went in. Splitting
// the `Pixel` and `Group` arms into their own functions would mean
// threading the same eight arguments through two more signatures and
// separating each arm from the doc comment that explains it; the length
// here is one `match` with two arms, not accumulated logic.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn resolve_tile(
    id: aurora_doc::LayerId,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tile_id: aurora_tile::TileId,
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    depth: usize,
    budget: &mut CompositeBudget,
) -> Option<(Vec<half::f16>, f32, aurora_render::BlendMode)> {
    // Both bounds are checked before anything else this function does,
    // including the visibility test below: `aurora-doc`'s own shape
    // validator is the primary guarantee that this recursion terminates,
    // and these are the independent second one. See this function's own
    // doc comment for the depth convention, and why bounding depth alone
    // is not enough to bound the work.
    if depth > aurora_doc::MAX_LAYER_TREE_DEPTH {
        if budget.should_report() {
            tracing::warn!(
                ?id,
                depth,
                max = aurora_doc::MAX_LAYER_TREE_DEPTH,
                "layer nesting past the tree depth bound; skipping this branch for this \
                 composite tile"
            );
        }
        return None;
    }
    // Charged after the depth check, not before it: a call refused for
    // depth does no work worth charging, and the number of such refused
    // calls is still bounded, because every one of them is issued by a
    // call that *was* charged. Charging here also means the budget
    // counts entries into this function, which is exactly the quantity a
    // duplicate-child fan-out inflates.
    if !budget.charge_node() {
        if budget.should_report() {
            tracing::warn!(
                ?id,
                depth,
                nodes = layers.len(),
                "composite recursion visited more layers than this document has; skipping \
                 this branch for this composite tile"
            );
        }
        return None;
    }
    if layers.visible(id) != Some(true) {
        return None;
    }
    let opacity = layers.opacity(id)?;
    let raw_blend_mode = layers
        .blend_mode(id)
        .unwrap_or(aurora_doc::BlendMode::Normal);
    let blend_mode = translate_blend_mode(raw_blend_mode);
    match layers.kind(id)? {
        aurora_doc::LayerKind::Pixel { .. } => {
            let surface = layers.surface_id(id)?;
            let origin = layers.bounds(id).map_or((0, 0), |b| (b.x, b.y));
            let texels = if origin == reference_origin {
                match store.get(surface, tile_id) {
                    Ok(tile) => tile.texels().to_vec(),
                    Err(err) => {
                        // Recorded, not just logged: the live canvas is
                        // content to skip this layer and repaint, but the
                        // export path must be able to tell that it did.
                        // See `CompositeBudget::note_store_error`.
                        budget.note_store_error(&err);
                        // Gated like both bound warnings above, and for a
                        // reason 0.52.2 made concrete: a tile whose
                        // page-in fails now fails on *every* touch rather
                        // than healing into a blank one, so an ungated
                        // warning here is one log line per layer per tile
                        // per frame for as long as the scratch file stays
                        // broken. The count still reaches the caller
                        // exactly (`note_store_error`), which is what the
                        // export refusal is built on -- only the logging
                        // is rate-limited.
                        if budget.should_report() {
                            tracing::warn!(
                                ?err,
                                ?tile_id,
                                "skipping layer for this composite tile"
                            );
                        }
                        return None;
                    }
                }
            } else {
                read_layer_window(store, surface, origin, doc_origin, budget)
            };
            // Mask clip runs first, ahead of `Dissolve` below: it
            // restricts which pixels are even in play before any blend
            // mode acts on what's left. See `resolve_tile`'s own doc
            // comment for the full ordering rationale and
            // `apply_mask_clip`'s own doc comment for the rectangular-
            // clip-only scope boundary.
            let texels = match layers.mask(id) {
                Some(mask) if mask.enabled => apply_mask_clip(&texels, mask, doc_origin),
                _ => texels,
            };
            // `Dissolve` is intercepted here, ahead of the translated
            // `blend_mode` below, rather than as a real
            // `aurora_render::BlendMode` variant -- see `dissolve_gate`'s
            // own doc comment for why. The stochastic decision already
            // fully accounts for this layer's own opacity (each texel's
            // gate is weighted by `texel_alpha * opacity`), so the gated
            // result is handed up at opacity `1.0`/`Normal` -- a straight
            // Normal composite of an already-binary-alpha buffer is
            // exactly correct, and re-applying `opacity` a second time
            // here would double-attenuate it.
            if raw_blend_mode == aurora_doc::BlendMode::Dissolve {
                let gated = dissolve_gate(&texels, opacity, doc_origin);
                return Some((gated, 1.0, aurora_render::BlendMode::Normal));
            }
            Some((texels, opacity, blend_mode))
        }
        aurora_doc::LayerKind::Group { children } => {
            // Folded in place, one child at a time, rather than collected
            // into a `Vec` of every child's own full tile buffer for a
            // single batch `aurora_render::composite_tile_cpu` call: each
            // child's `Vec<f16>` (512 KiB, `aurora_tile::SAMPLES` `f16`s)
            // is dropped at the end of its own loop iteration, before the
            // next child recurses. Peak memory is therefore bounded by
            // the layer tree's *depth*, not by any one group's sibling
            // count -- it is not constant, just no longer proportional to
            // how many layers a user happened to put in one group. See
            // `aurora_doc::MAX_LAYER_TREE_DEPTH`'s own doc comment for
            // why the bound is `depth + 1` buffers rather than `2 *
            // depth`: only the frame just returned into ever holds an
            // accumulator and a child buffer at the same time.
            //
            // Bit-identical to the batch `composite_tile_cpu` call it
            // replaces **by construction**, not by test:
            // `composite_layer_into`'s loop body reads no state beyond
            // `dst`/`src`/`opacity`/`mode`, so there is nothing a batch
            // call could carry across layers that N separate calls
            // could not. (The `aurora-render` test named
            // `composite_layer_into_folded_one_at_a_time_matches_the_batch_composite`
            // used to be cited here as proof of that; it is not one --
            // `composite_tile_cpu` is now defined as this same fold, so
            // both of its sides are the same calls. The math itself is
            // pinned by
            // `composite_layer_into_folded_matches_hand_computed_golden_values`.)
            let mut isolated = aurora_render::transparent_tile();
            for &child_id in children.iter().rev() {
                // Stop as soon as the budget is spent rather than still
                // making one no-op `resolve_tile` call per remaining
                // listed child -- see `CompositeBudget::is_exhausted`'s
                // own doc comment for why this matters for a crafted
                // tree, not just a well-formed one (where this never
                // fires, since every child that gets this far is real).
                if budget.is_exhausted() {
                    break;
                }
                // `saturating_add` rather than plain `+`, mirroring
                // `aurora-doc`'s own `validate_shape`: the guard at the
                // top of this function makes an overflow structurally
                // unreachable (`depth` can never get past
                // `MAX_LAYER_TREE_DEPTH + 1` here), so this is about
                // matching the validator's style, not about a real
                // wrap this code could hit.
                if let Some(resolved) = resolve_tile(
                    child_id,
                    layers,
                    store,
                    tile_id,
                    doc_origin,
                    reference_origin,
                    depth.saturating_add(1),
                    budget,
                ) {
                    let (child, child_opacity, child_blend_mode) = resolved;
                    aurora_render::composite_layer_into(
                        &mut isolated,
                        &child,
                        child_opacity,
                        child_blend_mode,
                    );
                }
            }
            // Un-premultiply: `composite_layer_into` accumulates straight-
            // alpha "over" math onto a starting-*transparent* destination,
            // which yields a *premultiplied* result whenever the
            // accumulated alpha ends up fractional (see this function's
            // own doc comment's worked example: a lone `opacity = 0.5`
            // child alone on transparent gives `(0, 0, 0.5, 0.5)`, not the
            // straight `(0, 0, 1.0, 0.5)`). Every other branch of this
            // function returns true straight-alpha texels, and the
            // caller's own `composite_layer_into` call one level up
            // expects straight-alpha inputs too -- so divide `r`/`g`/`b`
            // by `a`
            // here to convert this group's own isolated buffer back to
            // straight alpha before handing it back as `id`'s own
            // pseudo-layer texels. Guarded against `a == 0.0` (fully
            // transparent texels have no meaningful colour to recover;
            // leave them at `0.0` rather than dividing by zero).
            //
            // The loop itself lives in
            // `aurora_render::un_premultiply_in_place` — this arm was
            // the only place it existed until `composite_roots_into_tile`
            // and the GPU compositing path's own readback
            // (`finish_tile_readback`) were found to be missing the
            // identical step; see that function's own doc comment for
            // the invariant (straighten exactly once, at the top of an
            // accumulation, never inside `composite_layer_into`'s own
            // fold).
            aurora_render::un_premultiply_in_place(&mut isolated);
            // A group's own mask clips its *whole* isolated composite as
            // one unit, ahead of `Dissolve` below -- the same "group's
            // own opacity/blend mode apply one level up, to the isolated
            // result, not per-child" precedent this function's own doc
            // comment already establishes for opacity/blend mode, applied
            // identically here. See `resolve_tile`'s own doc comment for
            // the full ordering rationale and `apply_mask_clip`'s own doc
            // comment for the rectangular-clip-only scope boundary.
            if let Some(mask) = layers.mask(id)
                && mask.enabled
            {
                isolated = apply_mask_clip(&isolated, mask, doc_origin);
            }
            // `Dissolve` on a *group* is intercepted here too, symmetric
            // with the `Pixel` branch above (see `dissolve_gate`'s own
            // doc comment for the mechanism) — a group's own isolated,
            // now-straight-alpha buffer is exactly the same shape a
            // pixel layer's own texels are (a real `f16` RGBA buffer
            // plus an opacity scalar), so the identical gate applies. A
            // prior review found this arm missing on the first pass of
            // this feature: `translate_blend_mode`'s "unimplemented
            // mode falls back to Normal" fallback is meant for modes
            // this crate genuinely doesn't implement, not for a mode
            // that *is* implemented but was only wired into one of the
            // two `LayerKind` branches — leaving a group's own Dissolve
            // silently downgraded to Normal would have been exactly
            // that bug, not a documented, deliberate scope boundary.
            if raw_blend_mode == aurora_doc::BlendMode::Dissolve {
                let gated = dissolve_gate(&isolated, opacity, doc_origin);
                return Some((gated, 1.0, aurora_render::BlendMode::Normal));
            }
            Some((isolated, opacity, blend_mode))
        }
    }
}

/// Composites one tile of `layers`' **root level** on the CPU: every
/// root layer resolved by [`resolve_tile`] in paint order (bottom-to-top
/// — [`aurora_doc::LayerTree::roots`] is newest-first) and folded, one
/// at a time, into a single running accumulator.
///
/// Extracted because the two callers that need it —
/// [`recomposite_visible_tiles`]' own CPU path and
/// [`composite_document`]'s export loop — had the identical loop written
/// out twice, and 0.51.0's fold-in-place change had to be made in both
/// places independently (a third copy, over a *group's* children rather
/// than the document's roots, stays inline in [`resolve_tile`]'s own
/// `Group` arm: it also has the depth increment, the per-child budget
/// exhaustion break, and the un-premultiply/mask/`Dissolve` tail, so
/// unifying it here would take more than it gave). Keeping one copy also
/// meant the premultiplied-alpha gap PLAN.md tracked — the un-premultiply
/// step that ran in the `Group` arm and was missing from both of these
/// paths — had exactly one place to be fixed, which is where 0.52.0
/// fixed it (below).
///
/// The accumulator starts fully transparent
/// (`aurora_render::transparent_tile`) and each resolved child buffer is
/// dropped at the end of its own iteration, so peak memory here is one
/// tile buffer plus whatever the recursion below it holds, rather than
/// one full [`aurora_tile::SAMPLES`]-length buffer per root layer —
/// see `composite_document_composites_five_hundred_root_level_sibling_layers`
/// for the regression test, and `MAX_LAYER_TREE_DEPTH`'s own doc comment
/// in `aurora-doc` for what does still bound it.
///
/// **Returns straight-alpha texels.** The fold itself
/// (`aurora_render::composite_layer_into` onto a transparent start)
/// leaves a *premultiplied* result whenever the accumulated alpha ends
/// up fractional — a lone opaque-white root layer at 50% opacity folds
/// to `(0.5, 0.5, 0.5, 0.5)` — so this function runs
/// `aurora_render::un_premultiply_in_place` on the finished accumulator
/// before returning it, recovering the true `(1.0, 1.0, 1.0, 0.5)`. That
/// is the same step `resolve_tile`'s `Group` arm has always run on a
/// group's isolated buffer, and it was missing here (and from the GPU
/// compositing path, which now reaches the identical call through
/// [`finish_tile_readback`]) until 0.52.0: every exported
/// PNG/TIFF/`.aur` file with translucent content, and every eyedropper
/// read of a translucent pixel, carried premultiplied values. See
/// `composite_document_un_premultiplies_a_translucent_root_level_layer`
/// for the regression test and
/// `aurora_render::un_premultiply_in_place`'s own doc comment for why
/// the straightening belongs here, at the top of the accumulation,
/// rather than inside the per-layer fold.
///
/// Placement is last, on the finished root accumulator: unlike
/// `resolve_tile`'s `Group` arm there is no mask-clip or `Dissolve`
/// tail to order against here (both are per-layer, inside
/// `resolve_tile`), so there is nothing after this step.
///
/// Charging the tile against `budget` (`CompositeBudget::next_tile`) is
/// the caller's job, kept at the call site where the per-tile loop
/// itself is visible. `1` is the depth passed to [`resolve_tile`], the
/// same depth `aurora-doc`'s own validator starts its budget at for a
/// root-level layer.
fn composite_roots_into_tile(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tile_id: aurora_tile::TileId,
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    budget: &mut CompositeBudget,
) -> Vec<half::f16> {
    let mut composited = aurora_render::transparent_tile();
    for &id in layers.roots().iter().rev() {
        if let Some((texels, opacity, blend_mode)) = resolve_tile(
            id,
            layers,
            store,
            tile_id,
            doc_origin,
            reference_origin,
            1,
            budget,
        ) {
            aurora_render::composite_layer_into(&mut composited, &texels, opacity, blend_mode);
        }
    }
    // The accumulator has stopped being an accumulator and is now this
    // tile's finished composite, handed to callers (export, the
    // eyedropper, the canvas atlas) that all expect straight alpha --
    // see this function's own doc comment above.
    aurora_render::un_premultiply_in_place(&mut composited);
    composited
}

/// Whether every visible root-level layer in `layers` is a `Normal`-blend
/// [`aurora_doc::LayerKind::Pixel`] layer — no groups, no other blend
/// mode — the exact case [`begin_gpu_composite_tile`] can correctly express via
/// `aurora_render::TileCompositor::composite_over_with_opacity`'s
/// fixed-function alpha blend unit (opacity-scaled `Normal` "source-
/// over," nothing else). A single disqualifying layer (a visible group,
/// or a visible pixel layer at any blend mode other than
/// [`aurora_doc::BlendMode::Normal`]) routes the *whole document* back
/// to the CPU path ([`resolve_tile`]/`composite_tile_cpu`), which already
/// composites every one of those cases correctly — this only exists to
/// find a faster path for the common case, never to replace the CPU
/// path's own correctness. An invisible layer never disqualifies (it
/// contributes nothing on either path), matching [`resolve_tile`]'s own
/// `layers.visible(id) != Some(true)` early return; a layer with no
/// explicit `blend_mode` recorded is treated as `Normal`, matching
/// `resolve_tile`'s own `.unwrap_or(aurora_doc::BlendMode::Normal)`.
///
/// **Document-wide, not per-tile**: a layer's own kind and blend mode
/// don't vary from one composite tile to the next (only its *texels*
/// do), so [`recomposite_visible_tiles`] calls this once per redraw,
/// outside its own per-tile loop, rather than re-checking it once per
/// visible [`aurora_tile::TileId`] for no additional correctness.
#[must_use]
fn document_qualifies_for_gpu_compositing(layers: &aurora_doc::LayerTree) -> bool {
    layers.roots().iter().all(|&id| {
        layers.visible(id) != Some(true)
            || matches!(
                (layers.kind(id), layers.blend_mode(id)),
                (
                    Some(aurora_doc::LayerKind::Pixel { .. }),
                    Some(aurora_doc::BlendMode::Normal) | None
                )
            )
    })
}

/// Decodes a raw, mapped `Rgba16Float`, `TILE`×`TILE` readback buffer's
/// own byte range into a real, [`aurora_tile::SAMPLES`]-length
/// `Vec<half::f16>` — the shared decode step [`finish_tile_readback`]
/// (the batched path) needs, factored out once so the batching restructure
/// below has exactly one place that turns raw mapped bytes into samples,
/// rather than duplicating the loop.
///
/// `None` if `data` doesn't decode to exactly [`aurora_tile::SAMPLES`]
/// `f16`s (a malformed/short readback) — logged, not a panic, for the
/// same "a live user session can hit a real, if rare, GPU condition a
/// test never will" reason [`finish_tile_readback`]'s own doc comment
/// gives.
fn decode_f16_samples(data: &[u8]) -> Option<Vec<half::f16>> {
    let mut out = Vec::with_capacity(aurora_tile::SAMPLES);
    for bytes in data.chunks_exact(2) {
        let Ok(pair) = <[u8; 2]>::try_from(bytes) else {
            continue;
        };
        out.push(half::f16::from_le_bytes(pair));
    }
    if out.len() == aurora_tile::SAMPLES {
        Some(out)
    } else {
        tracing::warn!(
            len = out.len(),
            expected = aurora_tile::SAMPLES,
            "GPU composite readback returned an unexpected sample count; falling back to CPU"
        );
        None
    }
}

/// One composite tile's GPU readback, issued
/// (`copy_texture_to_buffer` + `queue.submit` + `slice.map_async`) but
/// not yet resolved — the "phase 1" unit [`begin_tile_readback`]
/// produces and [`finish_tile_readback`] consumes, once a single
/// per-frame `device.poll` has driven every pending tile's `map_async`
/// callback to completion. See [`recomposite_visible_tiles`]'s own doc
/// comment for the full three-phase shape (issue every tile's GPU work →
/// one poll for the whole frame → drain every tile's result) this exists
/// to support, and why batching the poll this way is the actual fix for
/// the N-blocking-polls-per-frame problem it replaces.
///
/// Owns the `wgpu::Buffer` itself, not a `wgpu::BufferSlice` borrowed
/// from it: a slice can't outlive this struct's own trip through a `Vec`
/// between phase 1 and phase 3, but `wgpu`'s own mapping API is designed
/// for exactly this "submit now, resolve later" shape — a fresh
/// `.slice(..)` call in [`finish_tile_readback`], after the buffer's own
/// map has already resolved, is cheap and needs no unsafe code or
/// lifetime trickery to make the borrow checker happy.
struct PendingGpuReadback {
    tile_id: aurora_tile::TileId,
    buffer: wgpu::Buffer,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// Issues the GPU→CPU readback for one already-composited destination
/// `texture` — `copy_texture_to_buffer`, `queue.submit`, and
/// `slice.map_async` — without blocking on any of it: deliberately no
/// `device.poll` call here at all, unlike this function's own
/// single-tile-at-a-time predecessor. This is phase 1's own per-tile
/// unit of work; [`recomposite_visible_tiles`] calls this once per
/// GPU-qualifying tile, collects every [`PendingGpuReadback`] it
/// returns into a `Vec`, then polls **once** for the whole batch before
/// resolving any of them via [`finish_tile_readback`].
///
/// `texture` must be `Rgba16Float`, `TILE`×`TILE`, with `COPY_SRC` usage,
/// matching [`begin_gpu_composite_tile`]'s own destination texture.
fn begin_tile_readback(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    tile_id: aurora_tile::TileId,
) -> PendingGpuReadback {
    let bytes_per_row = aurora_tile::TILE * 8; // Rgba16Float, already 256-byte aligned.
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu-composite-readback"),
        size: u64::from(bytes_per_row) * u64::from(aurora_tile::TILE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gpu-composite-readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(aurora_tile::TILE),
            },
        },
        wgpu::Extent3d {
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    PendingGpuReadback {
        tile_id,
        buffer: readback,
        rx,
    }
}

/// Resolves one [`PendingGpuReadback`] into its tile's own real,
/// [`aurora_tile::SAMPLES`]-length `Vec<half::f16>` — phase 3's own
/// per-tile unit of work, called only *after*
/// [`recomposite_visible_tiles`]'s single per-frame `device.poll` has
/// already driven every pending tile's `map_async` callback to
/// completion, so `rx.recv()` below returns immediately rather than
/// blocking: the callback has already run by the time this is called,
/// it's just delivering a result that's been sitting in the channel
/// since phase 2's poll resolved it.
///
/// `None` if the map genuinely failed (e.g. a lost device) — logged,
/// not a panic: unlike this crate's own test helpers, which can safely
/// `unreachable!` a map failure because the test just wrote the buffer
/// itself moments earlier under fully controlled conditions, this runs
/// against a real user's live session, where a device loss is a real,
/// if rare, possible event, not a logic bug. [`recomposite_visible_tiles`]
/// routes a `None` here back to the CPU path for that one tile — the
/// same "one bad tile shouldn't abort the rest" discipline
/// [`resolve_tile`]'s own callers already use for a failed
/// [`aurora_tile::TileStore::get`].
///
/// **Straightens the decoded samples** (`aurora_render::un_premultiply_in_place`)
/// before returning them, and this is the single place the GPU
/// compositing path's premultiplied → straight conversion happens.
/// [`begin_gpu_composite_tile`]'s render target holds *premultiplied*
/// alpha once its fold is done — the fixed-function `AlphaBlending` unit
/// accumulating onto a cleared, fully transparent target leaves exactly
/// the state `aurora_render::composite_layer_into` leaves on the CPU
/// side (a lone opaque-white layer at 50% opacity gives
/// `(0.5, 0.5, 0.5, 0.5)`, not the straight `(1.0, 1.0, 1.0, 0.5)`) —
/// and that is `composite_over_with_opacity`'s own correct, unchanged
/// contract, not something to fix on the GPU. The buffer stops being an
/// accumulator and becomes a finished tile exactly here, at the decode,
/// which is where the same "straighten exactly once, at the top of an
/// accumulation" rule [`composite_roots_into_tile`] and `resolve_tile`'s
/// `Group` arm already follow puts the step.
///
/// Doing it on the CPU, on the `Vec<half::f16>` the readback already
/// produces, rather than as an extra GPU render pass, is deliberate and
/// was 0.52.0's second shape: the first ran a WGSL sibling of that loop
/// into a *second* per-tile `Rgba16Float` texture (a texture cannot be
/// sampled and rendered to in one pass), which cost a per-tile
/// allocation and an extra queue submission on a path already measured
/// well over its frame budget, and — measured — the two implementations
/// did not agree at very small alphas. One implementation, called from
/// both paths, makes them agree by construction; see
/// `recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_fractional_final_alpha_document`.
fn finish_tile_readback(pending: PendingGpuReadback) -> Option<Vec<half::f16>> {
    let PendingGpuReadback {
        tile_id,
        buffer,
        rx,
    } = pending;
    match rx.recv() {
        Ok(Ok(())) => {}
        other => {
            tracing::warn!(
                ?other,
                ?tile_id,
                "GPU composite readback map failed; falling back to CPU"
            );
            return None;
        }
    }
    let slice = buffer.slice(..);
    let Ok(data) = slice.get_mapped_range() else {
        tracing::warn!(
            ?tile_id,
            "GPU composite readback reported success but the range is unavailable"
        );
        return None;
    };
    let decoded = decode_f16_samples(&data);
    drop(data);
    buffer.unmap();
    // Straighten once, here: what came back is the GPU fold's finished
    // *premultiplied* accumulator, and everything downstream of this
    // point -- the tile store, export, the eyedropper, the canvas atlas
    // -- is straight alpha. See this function's own doc comment above
    // for why this is the CPU's job on both paths.
    decoded.map(|mut texels| {
        aurora_render::un_premultiply_in_place(&mut texels);
        texels
    })
}

/// GPU-accelerated compositing for one visible composite tile, for the
/// tractable case [`document_qualifies_for_gpu_compositing`] confirms for
/// the whole document: every visible top-level layer is a `Normal`-blend
/// [`aurora_doc::LayerKind::Pixel`] layer, no groups. Callers must check
/// that first — this function does not re-check it itself.
///
/// Reuses [`resolve_tile`] once per visible root-level layer, bottom to
/// top, exactly as [`recomposite_visible_tiles`]'s own CPU path already
/// does — the same per-layer-origin conversion
/// (`read_layer_window`/direct `TileStore::get`) `resolve_tile`'s own
/// `Pixel` branch already establishes, not reimplemented here. Since
/// [`document_qualifies_for_gpu_compositing`] has already ruled out every
/// group and every non-`Normal` blend mode for this document,
/// `resolve_tile`'s own returned `aurora_render::BlendMode` is guaranteed
/// `Normal` for every entry this collects — `composite_over_with_opacity`'s
/// own fixed-function "source-over" *is* that formula exactly, so unlike
/// `composite_tile_cpu` this needs no blend-mode dispatch of its own.
///
/// For each collected layer (bottom to top): uploads its own tile-sized
/// texel window into a fresh scratch `Rgba16Float` source texture
/// (`TEXTURE_BINDING | COPY_DST`), then
/// `aurora_render::TileCompositor::composite_over_with_opacity` blends it
/// onto one shared destination texture (`RENDER_ATTACHMENT | COPY_SRC`,
/// cleared to fully transparent black first, since
/// `composite_over_with_opacity` always uses `LoadOp::Load`).
///
/// **Leaves a premultiplied accumulator behind, on purpose**: once the
/// fold is done that shared destination holds *premultiplied* alpha —
/// the fixed-function `AlphaBlending` unit accumulating onto a cleared,
/// fully transparent target leaves exactly the state
/// `aurora_render::composite_layer_into` leaves on the CPU side (a lone
/// opaque-white layer at 50% opacity gives `(0.5, 0.5, 0.5, 0.5)`, not
/// the straight `(1.0, 1.0, 1.0, 0.5)`), which is
/// `composite_over_with_opacity`'s own correct and unchanged contract.
/// Converting that back to the straight alpha the tile store and
/// everything downstream of it expect is
/// [`finish_tile_readback`]'s job, on the CPU, on the `Vec<half::f16>`
/// its readback decode already produces — one
/// `aurora_render::un_premultiply_in_place` call shared with
/// [`composite_roots_into_tile`], so the GPU and CPU paths cannot
/// disagree about that division by construction (see
/// `recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_fractional_final_alpha_document`).
/// Before 0.52.0 neither top-level path ran this step at all, so every
/// translucent composite tile — and so every export and every eyedropper
/// read — carried premultiplied values.
///
/// **Issues the readback, does not wait for it**: the destination texture's
/// GPU→CPU copy is *started* via [`begin_tile_readback`] — which itself
/// issues `copy_texture_to_buffer` + `queue.submit` + `slice.map_async`
/// with no `device.poll` call — and this function returns the resulting
/// [`PendingGpuReadback`] unresolved. This is the "phase 1" half of the
/// batched shape [`recomposite_visible_tiles`]'s own doc comment
/// describes: every visible tile's GPU work (this function, once per
/// qualifying tile) is issued before that caller polls even once, so one
/// `device.poll(PollType::Wait)` per **frame** resolves every tile's
/// pending map together, not one blocking wait per tile the way this
/// function's own predecessor (`gpu_composite_tile`, which called the
/// now-renamed `read_tile_f16` and blocked on it immediately) used to.
///
/// **Scope, stated honestly**: the CPU readback this issues still lands
/// back in `store`'s composite tile and gets re-uploaded to the atlas by
/// `residency.sync` later — this is still a GPU → CPU (readback) → GPU
/// round trip, not a direct GPU-to-atlas write. Batching the poll (this
/// change) reduces the number of blocking synchronization barriers per
/// frame; it does not remove the round trip itself — eliminating that is
/// separate, still-open follow-on work; see
/// [`recomposite_visible_tiles`]'s own doc comment for the full picture.
/// Export (`composite_document`) is untouched by this — it stays
/// CPU-only, a one-shot operation where this isn't latency-critical the
/// way the live canvas is.
///
/// `None` if there are no visible root-level layers at all (an empty
/// composite tile — cheaper handled by the CPU path's own "empty
/// `layers` → transparent black" default than by a real, empty GPU round
/// trip) — the caller falls back to the CPU path for this one tile
/// immediately, without anything to batch, the same "one bad tile
/// shouldn't abort the rest" discipline [`resolve_tile`]'s own callers
/// already use. A real GPU-side failure (a lost device, a bad map) can no
/// longer be detected here — resolving the map is [`finish_tile_readback`]'s
/// job now, in phase 3, once this function has already returned.
#[allow(clippy::too_many_arguments)]
fn begin_gpu_composite_tile(
    gpu: &aurora_gpu::GpuContext,
    compositor: &mut aurora_render::TileCompositor,
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    tile_id: aurora_tile::TileId,
    doc_origin: (i64, i64),
    reference_origin: (i64, i64),
    budget: &mut CompositeBudget,
) -> Option<PendingGpuReadback> {
    let mut layer_texels: Vec<(Vec<half::f16>, f32)> = Vec::new();
    for &id in layers.roots().iter().rev() {
        // `1`: a root-level layer, the same depth `aurora-doc`'s own
        // validator starts its budget at. One `budget` for all of this
        // tile's roots, not one each: in a well-formed tree their
        // subtrees are disjoint, so the sum of their node counts is
        // still bounded by the tree's own length.
        if let Some((texels, opacity, _blend_mode)) = resolve_tile(
            id,
            layers,
            store,
            tile_id,
            doc_origin,
            reference_origin,
            1,
            budget,
        ) {
            layer_texels.push((texels, opacity));
        }
    }
    if layer_texels.is_empty() {
        return None;
    }

    let device = gpu.device();
    let queue = gpu.queue();
    let tile_extent = wgpu::Extent3d {
        width: aurora_tile::TILE,
        height: aurora_tile::TILE,
        depth_or_array_layers: 1,
    };
    let dst_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-composite-dst"),
        size: tile_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        // `RENDER_ATTACHMENT` for the per-layer blend passes, `COPY_SRC`
        // for the readback below -- nothing samples this texture, so it
        // needs no `TEXTURE_BINDING`.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let dst_view = dst_texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Clear to fully transparent black -- `composite_over_with_opacity`
    // always preserves existing content (`LoadOp::Load`), so the
    // destination needs real, known-transparent content before the first
    // real layer blends onto it.
    {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu-composite-clear"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gpu-composite-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    for (texels, opacity) in &layer_texels {
        let src_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu-composite-src"),
            size: tile_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut bytes = Vec::with_capacity(texels.len() * 2);
        for sample in texels {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aurora_tile::TILE * 8),
                rows_per_image: Some(aurora_tile::TILE),
            },
            tile_extent,
        );
        let src_view = src_texture.create_view(&wgpu::TextureViewDescriptor::default());
        compositor.composite_over_with_opacity(gpu, &dst_view, &src_view, *opacity);
    }

    // `dst_texture` now holds this tile's finished composite in
    // *premultiplied* alpha -- the fixed-function `AlphaBlending` fold
    // onto a cleared, fully transparent target leaves exactly the state
    // `aurora_render::composite_layer_into` leaves on the CPU side (a
    // lone opaque-white layer at 50% opacity gives (0.5, 0.5, 0.5, 0.5),
    // not the straight (1.0, 1.0, 1.0, 0.5)). That is left as it is:
    // `finish_tile_readback` straightens the decoded samples on the CPU,
    // in the one shared `aurora_render::un_premultiply_in_place` the CPU
    // path also goes through, rather than this function spending a
    // second per-tile texture and an extra queue submission on a GPU
    // pass to do the same division a different way.
    Some(begin_tile_readback(device, queue, &dst_texture, tile_id))
}

/// Recomposites every tile in `residency`'s own currently-visible grid
/// from `layers.roots()`'s own bottom-to-top, visible root-level
/// entries into `store`'s reserved composite surface
/// ([`composite_surface_id`]), via `aurora_render::composite_tile_cpu`'s
/// per-texel math — what [`App::redraw`] calls before syncing the
/// atlas, so the canvas shows every visible pixel layer's real content
/// composited together, not just whichever one happens to be active for
/// editing. A layer whose tile fails to load is logged and skipped for
/// that one tile, the same "one bad tile shouldn't abort the rest"
/// discipline `TileResidency::sync` itself already uses.
///
/// **Per-layer origins, handled for real**: the atlas's own visible
/// grid is anchored to `active_layer`'s own document-space origin
/// (`canvas_local_origin`'s own doc comment) — for a layer that
/// shares that exact origin, its own `TileId` space already lines up,
/// so its tile is read directly; for one that doesn't (a document with
/// two layers at different `bounds`, e.g. after a Move), a document-
/// space tile is converted into that specific layer's own local
/// window via [`read_layer_window`], which may need blending up to
/// four of that layer's own tiles together when origins aren't
/// tile-aligned. All of this — including a group's own recursive
/// isolation — lives in [`resolve_tile`], called once per visible
/// root-level entry (`aurora_doc::LayerTree::roots`) for every tile.
///
/// **Blend modes, real for 26 of 27, and groups now aggregated for
/// real**: each layer's own real `blend_mode` is read and translated
/// via `translate_blend_mode` into `aurora_render::BlendMode` before
/// reaching `composite_tile_cpu` — `Normal`, the 8-mode "simple
/// separable" family (`Darken`/`Multiply`/`Lighten`/`Screen`/
/// `Difference`/`Exclusion`/`Subtract`/`Divide`), the 4-mode "dodge and
/// burn" family (`ColorDodge`/`LinearDodge`/`ColorBurn`/`LinearBurn`),
/// the 7-mode "overlay and light" family (`Overlay`/`SoftLight`/
/// `HardLight`/`VividLight`/`LinearLight`/`PinLight`/`HardMix`), the
/// 4-mode non-separable HSL family (`Hue`/`Saturation`/`Color`/
/// `Luminosity`), and the 2-mode whole-colour-selection family
/// (`DarkerColor`/`LighterColor`) composite with their own real math;
/// the one remaining `aurora_doc::BlendMode` variant (`Dissolve` — this
/// family's own explicit, now sole boundary at `translate_blend_mode`)
/// is fully implemented too, the same as every other blend mode — just
/// not inside that translation function: `resolve_tile` checks a
/// layer's own raw `aurora_doc::BlendMode` for `Dissolve` ahead of ever
/// calling `translate_blend_mode`, and substitutes `dissolve_gate`'s own
/// real, deterministic, position-seeded stochastic result instead (see
/// `dissolve_gate`'s and `translate_blend_mode`'s own doc comments for
/// the full mechanism). A group's own
/// `opacity`/`blend_mode` **are** now aggregated into its children's
/// effective compositing — [`resolve_tile`]'s own doc comment has the
/// real isolated-compositing semantic (every group isolates, always;
/// there is no "Pass Through" mode in `aurora_doc::BlendMode` to
/// express Photoshop's own distinction, so this is the only semantic
/// the schema can actually express) — real for the common cases (a
/// single child of any opacity, or multiple children combining via
/// `Normal`) after [`resolve_tile`]'s own un-premultiply fix, with a
/// narrower, still-open gap remaining for a group's own children
/// combining via a non-`Normal` blend mode against each other
/// mid-isolation — see [`resolve_tile`]'s own doc comment for the exact
/// boundary, not "still entirely broken" and not "fully fixed" either.
///
/// **Performance, incremental but coarse**: a visible tile already
/// current in `cache` is skipped entirely — see [`CompositeCache`]'s
/// own doc comment for what invalidates it. Still not per-tile-dirty-
/// aware *within* one invalidation: a single edit anywhere forces a
/// full recompute of every visible tile on the next redraw, not just
/// the one(s) it actually touched.
///
/// **GPU-accelerated for the common case, real now, not just a primitive
/// sitting unwired**: when `gpu`/`compositor` are both `Some` *and*
/// [`document_qualifies_for_gpu_compositing`] confirms the whole document
/// is GPU-tractable (every visible root-level layer a `Normal`-blend
/// [`aurora_doc::LayerKind::Pixel`] layer, no groups), each tile is
/// composited via [`begin_gpu_composite_tile`]/[`finish_tile_readback`] —
/// `aurora_render::TileCompositor::composite_over_with_opacity`'s real
/// fixed-function blend unit, not the CPU loop — closing the exact gap
/// `spike/FINDINGS.md`'s own ~20ms "merging whole tiles" finding named as
/// the reason `aurora_render::TileCompositor` exists at all. A tile whose
/// document doesn't qualify, or whose own GPU work fails
/// ([`finish_tile_readback`]'s own `None`), or when `gpu`/`compositor`
/// aren't available at all (`None`, e.g. no GPU device this session)
/// falls straight back to the exact same CPU path
/// (`resolve_tile`/`composite_tile_cpu`) this function always used before
/// — every blend mode, every group, un-premultiplied isolation, all of
/// it, unchanged. **Explicitly still CPU-only, by design, not by gap**:
/// non-`Normal` blend modes and group isolation on the GPU (would need a
/// full WGSL port of all 26 blend formulas, or per-group isolated GPU
/// passes — separate, much bigger follow-on work), and export
/// (`composite_document`, a one-shot operation, not latency-critical the
/// way the live canvas is).
///
/// **Batched in three phases, one blocking wait per frame instead of one
/// per tile**: this used to call a single `gpu_composite_tile` helper per
/// tile that issued its GPU work *and* immediately blocked on
/// `device.poll(PollType::Wait)` to read it back, before moving on to the
/// next tile — for an 800×600 viewport's own 5×4 = 20-tile grid
/// (`aurora_gpu::TileResidency::new`'s own sizing), that meant 20 separate
/// blocking driver synchronization barriers in one frame, each with fixed
/// per-call overhead (context switches, queue flushes) regardless of how
/// much GPU work was actually pending. This function now issues every
/// GPU-qualifying tile's compositing work up front
/// ([`begin_gpu_composite_tile`], which submits its copy and starts
/// `map_async` but never polls), collects the resulting
/// [`PendingGpuReadback`]s into a `Vec`, calls `device.poll(PollType::
/// Wait)` **exactly once** for the whole batch, then drains every
/// pending tile's result ([`finish_tile_readback`], whose own `rx.recv()`
/// now returns immediately since the single poll above already resolved
/// it). A tile that doesn't qualify for the GPU path at all — a
/// disqualified document, no GPU/compositor this session, or a tile with
/// no visible layers of its own — is composited via the CPU path
/// immediately, inline in the same first pass, since the CPU path never
/// blocks on the GPU and so has nothing to batch; only genuinely
/// GPU-issued tiles wait for the one shared poll. **What this changes and
/// what it doesn't**: only the *synchronization* pattern — the number of
/// blocking `device.poll` calls per frame — changed, from N-per-frame to
/// 1-per-frame; the underlying data path is untouched: this GPU path is
/// still a GPU → CPU (readback, into `store`'s composite tile) → GPU
/// (`residency.sync`'s own later re-upload to the atlas) round trip, not
/// a direct GPU-to-atlas write. Eliminating that round trip entirely is
/// separate, still-open follow-on work, not attempted here — this change
/// only batches the waits *within* the existing round trip. True
/// per-tile dirty tracking across layers (recomposite only the tile(s)
/// an edit actually touched) also remains separate, still-open follow-on
/// work regardless of which path composites a given tile.
///
/// A document with zero or one visible pixel layer (the common case so
/// far) is unaffected in practice either way: `composite_tile_cpu`
/// reproduces a single full-opacity layer's own texels exactly, and (once
/// GPU-tractable) so does the batched GPU path.
// Over 100 lines by six, since the per-pass composite budget went in.
// The body is three named, commented phases (issue, poll, drain) that
// share `pending_gpu`, `budget` and two closures; splitting them apart
// would mean passing all four across the seam for no gain in
// readability.
#[allow(clippy::too_many_lines)]
fn recomposite_visible_tiles(
    residency: &aurora_gpu::TileResidency,
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    store: &mut aurora_tile::TileStore,
    cache: &mut CompositeCache,
    gpu: Option<&aurora_gpu::GpuContext>,
    mut compositor: Option<&mut aurora_render::TileCompositor>,
) {
    // The tile grid `residency.visible_tiles()` walks is anchored to the
    // *active* layer's own origin (`canvas_local_origin`'s own doc
    // comment) — every other layer's own document-space tile boundaries
    // only line up with it by coincidence, so this is the one origin
    // every `tile_id` below needs converting back out of.
    let reference_origin =
        active_pixel_layer(layers, active_layer).map_or((0, 0), |(_, b)| (b.x, b.y));

    // Document-wide, computed once per call, not once per tile — see
    // `document_qualifies_for_gpu_compositing`'s own doc comment for why
    // that's correct, not just an optimization.
    let gpu_qualifies = document_qualifies_for_gpu_compositing(layers);

    let full_tile = aurora_core::Rect {
        x: 0,
        y: 0,
        width: aurora_tile::TILE,
        height: aurora_tile::TILE,
    };
    let tile_size = i64::from(aurora_tile::TILE);
    let doc_origin_for = |tile_id: aurora_tile::TileId| {
        (
            reference_origin.0 + i64::from(tile_id.x) * tile_size,
            reference_origin.1 + i64::from(tile_id.y) * tile_size,
        )
    };
    // Writes one tile's already-composited texels into `store`'s
    // reserved composite surface and, only on success, marks it current
    // in `cache` -- matching this function's own pre-batching behaviour
    // exactly: a `TileStore::get_mut` failure leaves the tile un-cached
    // so a later redraw retries it, rather than silently marking a
    // never-written tile "done".
    //
    // The length guard before `copy_from_slice` is this function's own,
    // deliberately not delegated: `copy_from_slice` *panics* on a
    // mismatch, and this crate holds a professional's unsaved work, so a
    // panic here loses it. Every producer of `composited` is
    // `SAMPLES`-long today (`aurora_render::transparent_tile` via
    // `composite_roots_into_tile`, or `decode_f16_samples`, which
    // enforces its own length) -- but that is an argument about other
    // functions, and `aurora_io::aur::read` already sets the precedent
    // of checking a length itself rather than trusting one. Skipping
    // leaves the tile un-cached, exactly as a `get_mut` failure does, so
    // a later redraw retries it.
    let write_composited = |store: &mut aurora_tile::TileStore,
                            cache: &mut CompositeCache,
                            tile_id: aurora_tile::TileId,
                            composited: &[half::f16]| {
        let Ok(dest) = store.get_mut(composite_surface_id(), tile_id) else {
            return;
        };
        if dest.texels().len() != composited.len() {
            tracing::warn!(
                ?tile_id,
                composited = composited.len(),
                expected = dest.texels().len(),
                "composited tile is not one whole tile; skipping this tile's write"
            );
            return;
        }
        dest.texels_mut().copy_from_slice(composited);
        dest.mark_dirty(full_tile);
        cache.mark_current(tile_id);
    };
    let composite_tile_cpu_path = |layers: &aurora_doc::LayerTree,
                                   store: &mut aurora_tile::TileStore,
                                   tile_id: aurora_tile::TileId,
                                   doc_origin: (i64, i64),
                                   budget: &mut CompositeBudget| {
        budget.next_tile(layers);
        composite_roots_into_tile(layers, store, tile_id, doc_origin, reference_origin, budget)
    };

    // Phase 1: issue every GPU-qualifying, not-yet-current tile's GPU
    // work (clear + per-layer blend + readback submit/map_async), with
    // no blocking wait yet. A tile with nothing to batch -- the document
    // doesn't qualify, `gpu`/`compositor` aren't available this session,
    // or this specific tile has no visible layers at all -- is
    // composited on the CPU path immediately, right here, since that
    // path never blocks on the GPU.
    let mut pending_gpu: Vec<PendingGpuReadback> = Vec::new();
    // One budget for the whole pass: its node count is recharged per
    // tile (by `next_tile`, on both paths below), but its "already
    // reported" flag deliberately is not, so a malformed document warns
    // once per pass rather than once per invalidated tile per frame.
    let mut budget = CompositeBudget::for_pass(layers);
    for tile_id in residency.visible_tiles() {
        if cache.is_current(tile_id) {
            continue;
        }
        let doc_origin = doc_origin_for(tile_id);

        let issued = if gpu_qualifies {
            match (gpu, compositor.as_deref_mut()) {
                (Some(gpu), Some(compositor)) => {
                    budget.next_tile(layers);
                    begin_gpu_composite_tile(
                        gpu,
                        compositor,
                        layers,
                        store,
                        tile_id,
                        doc_origin,
                        reference_origin,
                        &mut budget,
                    )
                }
                _ => None,
            }
        } else {
            None
        };

        if let Some(pending) = issued {
            pending_gpu.push(pending);
        } else {
            let composited =
                composite_tile_cpu_path(layers, store, tile_id, doc_origin, &mut budget);
            write_composited(store, cache, tile_id, &composited);
        }
    }

    // Phase 2: one poll for the whole frame -- drives every pending
    // tile's `map_async` callback to completion in a single blocking
    // wait, instead of one blocking wait per tile.
    if let (Some(gpu), false) = (gpu, pending_gpu.is_empty()) {
        let _ = gpu.device().poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
    }

    // Phase 3: drain every pending tile's result. `rx.recv()` inside
    // `finish_tile_readback` returns immediately here, since phase 2's
    // poll already resolved every pending map. A tile whose own GPU
    // readback failed falls back to the CPU path for that one tile, the
    // same "one bad tile shouldn't abort the rest" discipline this
    // module uses throughout.
    for pending in pending_gpu {
        let tile_id = pending.tile_id;
        let composited = if let Some(texels) = finish_tile_readback(pending) {
            texels
        } else {
            let doc_origin = doc_origin_for(tile_id);
            composite_tile_cpu_path(layers, store, tile_id, doc_origin, &mut budget)
        };
        write_composited(store, cache, tile_id, &composited);
    }
}

/// Which composite `aurora_tile::TileId`s [`recomposite_visible_tiles`]
/// has already computed since the last invalidation — the incremental
/// half of the "recomposites the entire visible grid unconditionally on
/// every redraw" gap Multi-layer compositing named. A `TileId`'s own
/// meaning (which document-space window it names) depends only on the
/// active layer's own origin (`recomposite_visible_tiles`'s own
/// `reference_origin`), never on `CanvasView`'s pan/zoom, so panning
/// back over already-composited, unedited territory is a real cache
/// hit, not just an idle redraw with nothing to do.
///
/// [`Self::bump`] is the coarse invalidation primitive, called by every
/// `aurora-app` operation whose effect on "what a given `TileId` now
/// composites to" isn't confined to a known, small set of tiles: a live
/// Move, Undo/Redo, opening or replacing the active document, and
/// selecting a different active layer (which changes the reference
/// origin every `TileId` is measured from, shifting every tile's own
/// meaning at once). [`Self::invalidate`] is the precise counterpart: a
/// brush/eraser dab (`App::paint_dab`/`App::erase_dab`) knows exactly
/// which tiles it really wrote (`aurora_brush::DabOutcome::painted`, in
/// the same document-space-relative-to-the-active-layer's-own-origin
/// frame `reference_origin` already uses — see those functions' own
/// call sites) and invalidates only those, since a full bump on every dab of
/// a stroke would mean recompositing the *entire* visible grid on every
/// dab rather than just the tile(s) the dab actually changed.
///
/// **Bump itself stays coarse, stated honestly**: it invalidates every
/// currently cached tile at once, not just the one(s) the triggering
/// edit actually touched. `aurora_tile::TileStore`'s own per-tile dirty
/// flags (`Tile::mark_dirty`/`TileStore::take_dirty`) are deliberately
/// *not* reused for either kind of invalidation here: they only track
/// resident tiles, so a tile dirtied by an edit and then evicted before
/// a redraw ever consumes its flag would silently stop being reported
/// dirty at all — a real correctness risk (a stale composite shown as
/// current) both `bump` and `invalidate` avoid by acting synchronously,
/// from data the caller already has in hand, rather than by querying
/// tile-store state later.
///
/// `current` only ever grows within a session between bumps — a tile
/// computed once is never individually evicted, even once panned away
/// from — bounded in practice by how many distinct composite tiles a
/// session ever actually visits, and harmless even if
/// `aurora_tile::TileStore`'s own LRU pages the real tile back out in
/// the meantime (paging back in on next access restores the same
/// content `TileStore` already guarantees elsewhere).
#[derive(Debug, Default)]
struct CompositeCache {
    current: std::collections::HashSet<aurora_tile::TileId>,
}

impl CompositeCache {
    /// Invalidates every cached tile.
    fn bump(&mut self) {
        self.current.clear();
    }

    /// Whether `id` was already computed since the last [`Self::bump`].
    #[must_use]
    fn is_current(&self, id: aurora_tile::TileId) -> bool {
        self.current.contains(&id)
    }

    /// Invalidates just `id`, leaving every other cached tile untouched —
    /// the single-tile analog of [`Self::bump`], for a caller that knows
    /// precisely which tile(s) an edit actually touched (a brush/eraser
    /// dab, via `aurora_brush::DabOutcome::painted` — what the dab
    /// really wrote, not merely what its bounding box covered) rather
    /// than needing to distrust the whole cache. Safe to call with an `id` that isn't
    /// currently cached at all — `HashSet::remove` is a no-op then.
    fn invalidate(&mut self, id: aurora_tile::TileId) {
        self.current.remove(&id);
    }

    /// Records that `id` now holds current composited content.
    fn mark_current(&mut self, id: aurora_tile::TileId) {
        self.current.insert(id);
    }
}

/// Assembles one `aurora_tile::TILE`-sized window of `surface`'s own
/// texels, positioned at document-space `doc_origin`, given that
/// `surface`'s own pixels are addressed from `layer_origin` (that
/// layer's own document-space `(bounds.x, bounds.y)`) rather than
/// `doc_origin`'s own reference frame — the general case
/// [`recomposite_visible_tiles`] needs once two composited layers no
/// longer share an origin. Unless `layer_origin` happens to be a whole
/// number of tiles away from `doc_origin`'s own frame, the window
/// doesn't land on a single one of `surface`'s own tiles: up to four
/// can overlap it (the same "one dab can span four tiles" shape
/// `aurora_brush::stamp::touched_tiles` already has for one small dab,
/// generalized here to a whole tile-sized window), each contributing
/// whatever rectangular sub-block of itself actually falls inside the
/// window.
///
/// A part of the window that falls before `surface`'s own local
/// `(0, 0)` — negative local coordinates, where `aurora_tile::TileId`'s
/// own unsigned fields mean there is no tile to read — is left fully
/// transparent, the same as any pixel `surface` has genuinely never
/// been painted at.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn read_layer_window(
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    layer_origin: (i64, i64),
    doc_origin: (i64, i64),
    budget: &mut CompositeBudget,
) -> Vec<half::f16> {
    let tile_size = i64::from(aurora_tile::TILE);
    let window_x = doc_origin.0 - layer_origin.0;
    let window_y = doc_origin.1 - layer_origin.1;
    let mut out =
        vec![
            half::f16::from_f32(0.0);
            aurora_tile::TILE as usize * aurora_tile::TILE as usize * aurora_tile::CHANNELS
        ];

    for tile_y in [
        window_y.div_euclid(tile_size),
        window_y.div_euclid(tile_size) + 1,
    ] {
        let Ok(tile_row) = u32::try_from(tile_y) else {
            continue;
        };
        let row_lo = (tile_y * tile_size).max(window_y);
        let row_hi = ((tile_y + 1) * tile_size).min(window_y + tile_size);
        if row_lo >= row_hi {
            continue;
        }
        for tile_x in [
            window_x.div_euclid(tile_size),
            window_x.div_euclid(tile_size) + 1,
        ] {
            let Ok(tile_col) = u32::try_from(tile_x) else {
                continue;
            };
            let col_lo = (tile_x * tile_size).max(window_x);
            let col_hi = ((tile_x + 1) * tile_size).min(window_x + tile_size);
            if col_lo >= col_hi {
                continue;
            }
            let src = match store.get(
                surface,
                aurora_tile::TileId {
                    x: tile_col,
                    y: tile_row,
                },
            ) {
                Ok(src) => src,
                Err(err) => {
                    // Same reason as `resolve_tile`'s own direct
                    // `store.get`: skipping leaves this part of the
                    // window transparent, which is silent content loss
                    // unless somebody upstream is told. See
                    // `CompositeBudget::note_store_error`.
                    budget.note_store_error(&err);
                    // Gated for the same reason as `resolve_tile`'s own
                    // sibling warning, and doubly so here: a moved
                    // layer's window reads up to four source tiles per
                    // composite tile, so one broken file is up to four
                    // log lines per tile per frame.
                    if budget.should_report() {
                        tracing::warn!(
                            ?err,
                            tile_x = tile_col,
                            tile_y = tile_row,
                            "skipping a moved layer's source tile for this composite tile"
                        );
                    }
                    continue;
                }
            };
            let texels = src.texels().to_vec();
            for src_row in row_lo..row_hi {
                let dst_row = (src_row - window_y) as usize;
                let in_tile_row = (src_row - tile_y * tile_size) as usize;
                for src_col in col_lo..col_hi {
                    let dst_col = (src_col - window_x) as usize;
                    let in_tile_col = (src_col - tile_x * tile_size) as usize;
                    let src_index = (in_tile_row * aurora_tile::TILE as usize + in_tile_col)
                        * aurora_tile::CHANNELS;
                    let dst_index =
                        (dst_row * aurora_tile::TILE as usize + dst_col) * aurora_tile::CHANNELS;
                    for channel in 0..aurora_tile::CHANNELS {
                        if let (Some(&s), Some(d)) = (
                            texels.get(src_index + channel),
                            out.get_mut(dst_index + channel),
                        ) {
                            *d = s;
                        }
                    }
                }
            }
        }
    }
    out
}

/// Composites every visible pixel layer in `layers` across the whole
/// `width`x`height` document rect into a flat `aurora_io::Image`, ready
/// for `aurora_io::encode_by_extension` — the real multi-layer read
/// [`App::save_file`]'s flat-format export path needs, in place of the
/// old "read the active layer's own surface" behaviour.
///
/// Shares both halves of its logic rather than reinventing either:
/// per-tile, per-layer-blend-mode-aware compositing and moved-layer
/// origin conversion come straight from
/// [`recomposite_visible_tiles`]/[`read_layer_window`]
/// (`aurora_render::composite_tile_cpu`, via `translate_blend_mode`),
/// while the tile-walk/output-buffer
/// shape — deriving `tiles_x`/`tiles_y` from `width`/`height` via
/// `div_ceil`, and copying each tile's real `w`x`h` sub-region (clamped
/// at the bottom/right edge for a non-tile-aligned document) into a flat
/// row-major buffer — is [`aurora_io::read_from_store`]'s own shape.
///
/// The one deliberate difference from [`recomposite_visible_tiles`]:
/// this walks the *document's* own full extent (`width`x`height`,
/// `tile_id`s starting at `(0, 0)`), not
/// `aurora_gpu::TileResidency::visible_tiles()` — export must cover the
/// whole document regardless of which corner the canvas happens to be
/// scrolled to. Document tiles are therefore always anchored at
/// document `(0, 0)`: a layer whose own `bounds` origin is also
/// `(0, 0)` reads directly through `TileStore::get`, matching
/// `recomposite_visible_tiles`'s own `origin == reference_origin` fast
/// path; any other origin (a moved layer) goes through
/// [`read_layer_window`], the same general re-tiling that function
/// already establishes.
///
/// **Scope, same as [`recomposite_visible_tiles`]/`composite_tile_cpu`**:
/// each layer's own real `blend_mode` is read and translated
/// (`translate_blend_mode`) — `Normal`, the 8-mode "simple
/// separable" family (`Darken`/`Multiply`/`Lighten`/`Screen`/
/// `Difference`/`Exclusion`/`Subtract`/`Divide`), the 4-mode
/// "dodge and burn" family (`ColorDodge`/`LinearDodge`/`ColorBurn`/
/// `LinearBurn`), the 7-mode "overlay and light" family
/// (`Overlay`/`SoftLight`/`HardLight`/`VividLight`/`LinearLight`/
/// `PinLight`/`HardMix`), the 4-mode non-separable HSL family
/// (`Hue`/`Saturation`/`Color`/`Luminosity`), and the 2-mode
/// whole-colour-selection family (`DarkerColor`/`LighterColor`) are
/// real; the one remaining `aurora_doc::BlendMode` variant (`Dissolve`)
/// is real too, but [`resolve_tile`] intercepts it before this
/// translation ever runs and applies its own stochastic `dissolve_gate`
/// instead — see [`resolve_tile`]'s own doc comment for why. Layer
/// groups are recursed into at any depth `aurora-doc` will accept
/// (bounded independently by `aurora_doc::MAX_LAYER_TREE_DEPTH` and a
/// per-tile node budget — see [`resolve_tile`]'s own doc comment) via
/// [`resolve_tile`] (walking `aurora_doc::LayerTree::roots`/
/// `LayerTree::children`, not `LayerTree::paint_order`, which stays a
/// flat list for other, non-compositing callers), ancestor-visibility-
/// gated, so a layer nested inside a visible group's ancestor chain
/// composites into this export too — and a group's own `opacity`/
/// `blend_mode` **are** now aggregated into its children's effective
/// compositing, via the same isolated-compositing semantic
/// [`resolve_tile`]'s own doc comment lays out (every group composites
/// in isolation, always — `aurora_doc::BlendMode` has no "Pass Through"
/// variant to express Photoshop's own distinction, so isolation is the
/// only semantic this schema can actually express). Real for the common
/// cases (a single child of any opacity, or multiple children combining
/// via `Normal`) after [`resolve_tile`]'s own un-premultiply fix — a
/// narrower, still-open gap remains for a group's own children
/// combining via a non-`Normal` blend mode against each other
/// mid-isolation; see [`resolve_tile`]'s own doc comment for the exact
/// boundary.
///
/// A layer whose own tile fails to load for a given output tile is
/// logged and skipped for that tile only, the same "one bad tile
/// shouldn't abort the rest" discipline [`recomposite_visible_tiles`]
/// already uses — the *walk* is not aborted. **The export is**, at the
/// end: see `# Errors` below. That split is deliberate and is the whole
/// point of 0.52.1's second half — the live canvas may degrade
/// gracefully, a file may not.
///
/// # Errors
///
/// Returns [`aurora_io::IoError::IncompleteComposite`] if any layer tile
/// could not be read out of `store` during the walk. Every such layer
/// contributed nothing at all to its tile rather than its real pixels,
/// so the assembled image is quietly missing content — a corrupted
/// scratch-disk tile (a crash mid-write, a full disk, another process in
/// the scratch directory) is exactly how that happens to a document that
/// is open and being edited. Until 0.52.1 this returned `Ok` with the
/// holes in it and [`App::save_file`] wrote that straight to the user's
/// file: silent, unannounced content loss, the failure CLAUDE.md names
/// as the worst this project can have. It is refused instead.
///
/// The caller's obligation stops at *not writing the file*. Turning this
/// into the itemized, user-visible warning FR-001's lossy-save rule
/// calls for is real, separate, still-open work — tracked in PLAN.md,
/// not done here.
///
/// Also returns [`aurora_io::IoError`] if the assembled buffer doesn't
/// come out to exactly `width * height * 4` samples — structurally
/// unreachable given this function's own tile walk, but surfaced
/// through the same `aurora_io::Image::new` contract
/// [`aurora_io::read_from_store`] already goes through, rather than
/// asserted away.
fn composite_document(
    layers: &aurora_doc::LayerTree,
    store: &mut aurora_tile::TileStore,
    width: u32,
    height: u32,
) -> Result<aurora_io::Image, aurora_io::IoError> {
    let mut samples =
        vec![half::f16::from_f32(0.0); width as usize * height as usize * aurora_tile::CHANNELS];

    if width > 0 && height > 0 {
        // One budget for the whole export, recharged per tile below --
        // same shape as `recomposite_visible_tiles`', and for the same
        // reason (a malformed document should warn once, not once per
        // tile of a large export).
        let mut budget = CompositeBudget::for_pass(layers);
        let tile_size = aurora_tile::TILE;
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_id = aurora_tile::TileId { x: tx, y: ty };
                let doc_origin = (
                    i64::from(tx) * i64::from(tile_size),
                    i64::from(ty) * i64::from(tile_size),
                );

                budget.next_tile(layers);
                // `(0, 0)` as the reference origin: an export always
                // measures from the document's own origin, unlike the
                // on-screen path, which measures from the viewport.
                let composited = composite_roots_into_tile(
                    layers,
                    store,
                    tile_id,
                    doc_origin,
                    (0, 0),
                    &mut budget,
                );

                let origin_x = tx * tile_size;
                let origin_y = ty * tile_size;
                let w = (width - origin_x).min(tile_size);
                let h = (height - origin_y).min(tile_size);
                for ly in 0..h {
                    let dst_start = ((origin_y + ly) as usize * width as usize + origin_x as usize)
                        * aurora_tile::CHANNELS;
                    let dst_end = dst_start + (w as usize) * aurora_tile::CHANNELS;
                    let src_start = (ly * tile_size) as usize * aurora_tile::CHANNELS;
                    let src_end = src_start + (w as usize) * aurora_tile::CHANNELS;
                    if let (Some(dst), Some(src)) = (
                        samples.get_mut(dst_start..dst_end),
                        composited.get(src_start..src_end),
                    ) {
                        dst.copy_from_slice(src);
                    }
                }
            }
        }

        // Checked once for the whole export, after the tile walk rather
        // than inside it: one unreadable tile does not abort the other
        // thousands (the CPU cost of finishing is trivial next to
        // getting the answer wrong), but it does mean this function has
        // no honest `Image` to return. See this function's own `# Errors`
        // section and `CompositeBudget::note_store_error`.
        if let Some((skipped, first)) = budget.store_error() {
            tracing::error!(
                skipped,
                first,
                "refusing to export a document with tiles that could not be read"
            );
            return Err(aurora_io::IoError::IncompleteComposite {
                skipped,
                first: first.to_owned(),
            });
        }
    }

    aurora_io::Image::new(width, height, aurora_color::IccProfile::srgb(), samples)
}

/// One press on a Layers-panel row, whole: end whatever drag was still
/// live ([`commit_ending_drag`]) and *then* select the row's layer
/// ([`select_layer`]).
///
/// **The order is the point, which is why this is a function.**
/// `select_layer` re-establishes the pan bound against the newly active
/// layer, i.e. it moves the view. A drag in progress holds a reference
/// point fixed at the moment it began — `Drag::Pan`'s own `last_screen`,
/// `Drag::Move`/`Drag::Marquee`'s own `start_doc` — so clamping the view
/// out from under one makes the next pointer-move event compute its
/// delta against a view that moved for reasons the drag knows nothing
/// about. `Drag::Pan` happens to recover on its own next event (its arm
/// re-clamps), but that is a coincidence of one variant, not an
/// invariant: `Drag::Marquee`, `Drag::Brush`, `Drag::Eraser` and
/// `Drag::Eyedropper` never clamp at all. Not having a live drag here is
/// the invariant.
///
/// [`perform_undo_redo`] is the same shape for the same reason, at the
/// second site the hazard turned up at (0.57.7).
///
/// Ending it via the shared commit rather than dropping it also closes a
/// second, older hole in this branch, which used to `return` before ever
/// reaching `handle_pointer_pressed`'s own "a second press ends the live
/// drag" commit: a brush stroke interrupted by a layer-row click lost
/// its whole undo entry, exactly the 0.57.0 bug
/// [`commit_ending_drag`]'s own doc comment describes for the other
/// gestures.
///
/// Pushing the updated accessibility tree and bumping the composite
/// cache stay the caller's job — the same split [`select_layer`] itself
/// already documents.
#[allow(clippy::too_many_arguments)]
fn press_layer_row(
    workspace: &mut aurora_ui::Workspace,
    layer_rows: &HashMap<WidgetId, aurora_doc::LayerId>,
    active_layer: &mut Option<aurora_doc::LayerId>,
    view: &mut aurora_ui::CanvasView,
    layers: &aurora_doc::LayerTree,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    undo_order: &mut UndoOrder,
    drag: &mut Option<Drag>,
    layer_id: aurora_doc::LayerId,
) {
    commit_ending_drag(
        drag.take(),
        layers,
        history,
        pixel_history,
        undo_order,
        view,
        *active_layer,
    );
    select_layer(workspace, layer_rows, active_layer, view, layers, layer_id);
}

/// Selects `layer_id` as the active layer: sets `*active_layer`, marks
/// its own Layers-panel row (`layer_rows` —
/// `aurora_ui::populate_layers_panel`'s own return value) as accessibly
/// selected (`accesskit::Node::set_selected`), clearing that state from
/// every other row, and re-establishes `view`'s own pan bound against
/// the newly active layer ([`clamp_pan_to_active_layer`]). Pushing the
/// updated accessibility tree to the platform is still the caller's job
/// (`App::push_accessibility`), the same "pure dispatch, caller owns the
/// one real platform side-effect" split every other function in this
/// crate already uses (`open_crash_recovery_dialog`, `begin_drag`, ...).
///
/// The pan clamp is **not** an unrelated extra. The pan bound is
/// `view`'s own pan measured against the *active layer's* own origin, so
/// this function moving `*active_layer` is itself what can violate it —
/// see [`clamp_pan_to_active_layer`]. Taking `view`/`layers` rather than
/// leaving the clamp to the caller is what stops the next new call site
/// from silently reopening the bug.
fn select_layer(
    workspace: &mut aurora_ui::Workspace,
    layer_rows: &HashMap<WidgetId, aurora_doc::LayerId>,
    active_layer: &mut Option<aurora_doc::LayerId>,
    view: &mut aurora_ui::CanvasView,
    layers: &aurora_doc::LayerTree,
    layer_id: aurora_doc::LayerId,
) {
    *active_layer = Some(layer_id);
    for (&row, &id) in layer_rows {
        let Some(node) = workspace.tree.accessibility(row) else {
            continue;
        };
        let mut node = node.clone();
        node.set_selected(id == layer_id);
        if let Err(err) = workspace.tree.set_accessibility(row, node) {
            tracing::warn!(?err, "failed to update a layer row's selection state");
        }
    }
    // After the row loop, not before: nothing here depends on the
    // ordering, but keeping the accessibility work untouched and the
    // new bound re-established at the end keeps this function's two
    // jobs readable as two jobs.
    clamp_pan_to_active_layer(view, layers, Some(layer_id));
}

/// Creates a fresh scratch directory for one session's tile store,
/// under the platform temp directory, and returns its path. `None`
/// (logged) if it cannot be created.
///
/// **Why not a fixed path.** Until 0.53.0 this was
/// `std::env::temp_dir().join("aurora-tiles")`: one directory, shared by
/// every process, every document and every *user* on the machine,
/// holding files named only `{surface}_{x}_{y}.tile` — where
/// `SurfaceId` restarts from 0 for each fresh document. Two documents
/// open at once addressed the same files and silently corrupted each
/// other's unsaved pixels, and the directory was world-readable. The
/// name is now random (so it cannot be pre-created or symlinked into
/// place by another local user), the creation is exclusive (`tempfile`
/// retries on `EEXIST` rather than adopting somebody else's directory),
/// and `aurora_tile::TileStore::new` makes it `0o700` on Unix.
///
/// Still under `std::env::temp_dir()`, deliberately: scratch tiles are
/// the definition of ephemeral, run to gigabytes, and are already
/// removed on a clean shutdown ([`clean_shutdown_cleanup`]).
/// Where they *should* live on a given machine is a still-open,
/// user-facing scratch-disk preference (FR-026), not something to decide
/// here by silently redirecting a document's paging traffic into
/// `directories::ProjectDirs`' data directory.
fn create_tile_store_scratch_dir() -> Option<PathBuf> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("aurora-scratch-");
    // `tempfile` creates a *directory* with the process's default
    // permissions (a plain `DirBuilder::create`, so umask-derived and
    // typically `0o755`/`0o775`) -- unlike its temp *files*, which are
    // `0o600`. Saying `0o700` here makes the mode part of the `mkdir`
    // itself, so the directory is never world-readable for even an
    // instant. `aurora_tile::TileStore::new` re-asserts `0o700` when it
    // opens a store here, but that is not a substitute: the store may be
    // opened on a *child* of this directory (`aur_verify_scratch_dir`)
    // and never on this one, in which case nothing else would ever fix
    // the parent's mode.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    match builder.tempdir() {
        Ok(dir) => Some(dir.keep()),
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to create this session's tile scratch directory"
            );
            None
        }
    }
}

/// This process's own scratch directory — created once, on first use,
/// and reused by every [`open_tile_store`] call for the rest of the run.
///
/// Memoized rather than recreated per call because the store is
/// legitimately reopened mid-run ([`startup_document`],
/// [`recover_partial_after_a_failed_read`]): a fresh directory per call
/// would strand the previous one on disk with nothing left holding its
/// path to delete it. `None` means no scratch directory could be created
/// at all, which [`open_tile_store`] turns into its existing "painting
/// is disabled this session" degradation, not a crash.
///
/// Two acknowledged edge cases, neither fixed here and neither with a
/// demonstrated practical impact: a `None` memoizes, so a single
/// transient failure at first use disables paging for the whole run
/// rather than being retried; and a run that creates the directory but
/// never opens a store in it still creates it (and still deletes it at
/// shutdown), so the cost of the memoization is one empty directory in
/// the case where nothing needed one.
fn tile_store_scratch_dir() -> Option<&'static Path> {
    static SCRATCH_DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    SCRATCH_DIR
        .get_or_init(create_tile_store_scratch_dir)
        .as_deref()
}

/// Everything a clean shutdown has to undo, in one directly callable
/// place: clear this run's own "I'm still running" marker so the *next*
/// run's `previous_session_left_a_marker` reads false, delete the
/// autosave (nothing is left to recover once the run has ended
/// cleanly), and delete this session's tile scratch directory and every
/// unsaved pixel paged into it.
///
/// **Why a function and not three statements in the event handler.**
/// The `WindowEvent::CloseRequested` arm needs a real `winit` event
/// loop to reach, so nothing in this crate's test module can execute
/// it; a review round demonstrated that deleting the scratch-directory
/// cleanup from that arm entirely left all 1101 tests green. The state
/// arrives through [`ShutdownState`] precisely so a test *can* call
/// this — against throwaway paths, not the live session's, which
/// deleting would pull the scratch disk out from under every other test
/// sharing this binary. See that trait for why it is a trait and not
/// four parameters.
///
/// **The store is taken and dropped before the directory is removed.**
/// Dropping a `aurora_tile::TileStore` joins its background writer
/// thread (its `BackgroundWriter`'s own `Drop` calls `flush`, which
/// drops the sender and joins), so once that line has returned no
/// eviction can still be in flight. This is not a fix for a race
/// anyone has reproduced: whether `remove_dir_all` can actually lose to
/// a writer creating a file mid-walk (`ENOTEMPTY`, orphaned unsaved
/// pixels) depends on the filesystem, and no one has demonstrated it
/// here. Rust's drop semantics are deterministic and blocking, so
/// ordering it this way removes the question at zero cost rather than
/// answering it.
///
/// **Crash leftovers are not covered** — see PLAN.md's own follow-up
/// item. A run that never reaches this leaves its directory behind for
/// the platform's temp cleaner, and that includes clean quits that
/// bypass `WindowEvent::CloseRequested` (macOS's own menu Quit,
/// [`App::fail`]), not only crashes. The marker and the autosave have
/// exactly the same gap.
fn clean_shutdown_cleanup(state: &mut impl ShutdownState) {
    clear_session_marker(state.marker_path());
    remove_autosave(&state.autosave_path());
    // Taken and dropped *before* the directory goes: dropping a
    // `aurora_tile::TileStore` joins its background writer thread (its
    // `BackgroundWriter`'s own `Drop` calls `flush`, which drops the
    // sender and joins), so no eviction can still be in flight against
    // a directory that is being deleted. Deterministic by construction
    // rather than a race this has to win.
    drop(state.take_tile_store());
    if let Some(dir) = state.scratch_dir() {
        remove_scratch_dir(dir);
    }
}

/// Everything [`clean_shutdown_cleanup`] needs from the running
/// application, as one source of truth rather than four arguments.
///
/// **Why a trait and not four parameters.** A review round showed that
/// changing the single production call site to pass `None` for the
/// store and `None` for the scratch directory compiled, ran, and passed
/// the entire gate — a plausible slip with no protection whatsoever,
/// and one `dead_code` would not catch either, since the function still
/// had callers. With the state behind a trait the call site is
/// `clean_shutdown_cleanup(self)`: there is no argument left to get
/// wrong, and what remains is a four-line `impl` for [`App`] whose
/// bodies are each a single field or function reference.
///
/// It is also what keeps the function testable. The real
/// implementation resolves the live session directory and the live
/// store; a test supplies throwaway paths instead, because removing the
/// live session directory from a test would pull the scratch disk out
/// from under every other test sharing this binary.
trait ShutdownState {
    /// This run's own "I'm still running" marker.
    fn marker_path(&self) -> &Path;
    /// Where the autosave this run has been writing lives.
    fn autosave_path(&self) -> PathBuf;
    /// Takes the live tile store out of the application, so dropping it
    /// joins its writer thread. Leaves nothing behind.
    fn take_tile_store(&mut self) -> Option<aurora_tile::TileStore>;
    /// This session's scratch directory, or `None` if one was never
    /// created (painting disabled for the run).
    fn scratch_dir(&self) -> Option<&Path>;
}

/// [`clean_shutdown_cleanup`]'s scratch-directory step, split out so it
/// can also be called on its own against a *throwaway* directory: the
/// caller in the event handler passes the one live, memoized session
/// directory ([`tile_store_scratch_dir`]), and deleting that from a
/// test would pull the scratch disk out from under every other test
/// sharing the binary.
fn remove_scratch_dir(dir: &Path) {
    if let Err(err) = std::fs::remove_dir_all(dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?err, path = %dir.display(), "failed to remove this session's tile scratch directory");
    }
}

/// A practical resident-tile budget for this crate's first live store —
/// 256 tiles, 128 MiB at ADR 0005's fixed 512 KiB/tile — not a
/// considered, user-facing default (that's FR-026's own, still-open
/// scratch-disk-size preference); enough to paint comfortably in one
/// session without constantly evicting.
const TILE_BUDGET: usize = 256;

/// Opens this session's shared tile store at [`tile_store_scratch_dir`].
/// Errors are logged, not fatal -- the same "must never stop the
/// application starting" shape [`write_session_marker`]'s own I/O
/// already uses; a store that fails to open just means painting is
/// silently disabled for the session, not a crash. Since 0.53.0 a
/// scratch directory that could not be *created* at all takes that same
/// path (it is already logged by [`create_tile_store_scratch_dir`]),
/// rather than falling back to a shared one.
fn open_tile_store() -> Option<aurora_tile::TileStore> {
    let Some(budget) = std::num::NonZeroUsize::new(TILE_BUDGET) else {
        unreachable!("TILE_BUDGET is a fixed, non-zero constant");
    };
    let Some(scratch_dir) = tile_store_scratch_dir() else {
        tracing::warn!("no tile scratch directory; painting is disabled this session");
        return None;
    };
    match aurora_tile::TileStore::new(scratch_dir.to_path_buf(), budget) {
        Ok(store) => Some(store),
        Err(err) => {
            tracing::warn!(
                ?err,
                "failed to open the tile store; painting is disabled this session"
            );
            None
        }
    }
}

/// Converts a document-space point into a pixel layer's own local
/// space — `bounds`'s own `(x, y)` is that layer's position in document
/// space (`aurora_doc::LayerKind::Pixel`'s own field), and
/// `aurora_tile::TileStore` addresses each surface from its own local
/// `(0, 0)`, not the document's.
#[must_use]
fn layer_local_point(bounds: aurora_core::Rect, doc_point: (f32, f32)) -> (f32, f32) {
    #[allow(clippy::cast_precision_loss)]
    (doc_point.0 - bounds.x as f32, doc_point.1 - bounds.y as f32)
}

/// Reads the straight RGBA sample at `local_point` (surface-local space
/// — the same space [`layer_local_point`] produces) from `store`'s
/// `surface`, one texel, no interpolation — what the Eyedropper tool
/// needs to pick a real, already-painted colour. `None` for a negative
/// coordinate (`TileId`'s own fields are unsigned, so there is no tile
/// there — the same "outside the surface" case
/// [`aurora_gpu::TileResidency::set_origin`]'s own doc comment names) or
/// if paging the touched tile in fails.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
// x/y/r/g/b/a are the clearest names for "one pixel coordinate, one
// RGBA sample" -- spelling any of them out would be noise, not clarity.
#[allow(clippy::many_single_char_names)]
fn sample_pixel(
    store: &mut aurora_tile::TileStore,
    surface: aurora_tile::SurfaceId,
    local_point: (f32, f32),
) -> Option<[f32; 4]> {
    let (x, y) = local_point;
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let (px, py) = (x as u32, y as u32);
    let tile_id = aurora_tile::TileId {
        x: px / aurora_tile::TILE,
        y: py / aurora_tile::TILE,
    };
    let tile = store.get(surface, tile_id).ok()?;
    let (lx, ly) = (px % aurora_tile::TILE, py % aurora_tile::TILE);
    let index = (ly * aurora_tile::TILE + lx) as usize * aurora_tile::CHANNELS;
    let texels = tile.texels();
    let r = texels.get(index)?.to_f32();
    let g = texels.get(index + 1)?.to_f32();
    let b = texels.get(index + 2)?.to_f32();
    let a = texels.get(index + 3)?.to_f32();
    Some([r, g, b, a])
}

/// The pure core of [`App::sample_eyedropper`], factored out so it's
/// testable without a real `App` (which needs a live window/GPU surface
/// to construct at all): converts `doc_point` (document space) into
/// [`composite_surface_id`]'s own local space using `origin`
/// (`doc_point` minus `origin`, the same subtraction
/// [`layer_local_point`] does against a layer's own `bounds` — here
/// against [`active_layer_origin`]'s return instead, since the composite
/// surface is anchored to that same reference point, not any one
/// layer's own surface), then reads the already-composited RGB back via
/// [`sample_pixel`]. `None` — "nothing to pick" — for a fully
/// transparent texel (no visible layer painted there) exactly as before,
/// and for the same out-of-bounds/paging-failure cases [`sample_pixel`]
/// itself already returns `None` for.
#[must_use]
fn eyedropper_sample(
    store: &mut aurora_tile::TileStore,
    origin: (f32, f32),
    doc_point: (f32, f32),
) -> Option<[f32; 3]> {
    let local = (doc_point.0 - origin.0, doc_point.1 - origin.1);
    let [r, g, b, a] = sample_pixel(store, composite_surface_id(), local)?;
    (a > 0.0).then_some([r, g, b])
}

/// The Brush tool's fixed radius — a real default, not a placeholder,
/// but not a considered one either: there is no brush options UI yet
/// (size picker, real engine, Phase 2 per PLAN.md's own "(real engine
/// is Phase 2)" framing on this bullet).
const BRUSH_RADIUS: f32 = 24.0;

/// [`App::current_colour`]'s own starting value — black, since there is
/// no colour-picker UI yet to set it any other way at startup. Real
/// after that: the Eyedropper tool changes it to whatever's actually
/// sampled, and every `Brush` dab paints with whatever it currently is,
/// not this constant directly (unlike [`BRUSH_RADIUS`]/[`ERASER_RADIUS`],
/// which stay fixed).
const DEFAULT_COLOUR: [f32; 3] = [0.0, 0.0, 0.0];

/// The Eraser tool's fixed radius — same reasoning and same value as
/// [`BRUSH_RADIUS`] (no options UI yet), kept as its own named constant
/// rather than reusing `BRUSH_RADIUS` directly so the two tools' sizes
/// can diverge later without one silently changing the other.
const ERASER_RADIUS: f32 = 24.0;

// -- Canvas rendering: drawing the live document to the screen --
//
// PLAN.md M1.8's still-open "Canvas" bullet, the piece that finally
// gives the brush painting above (and anything else touching
// `tile_store`) somewhere visible to show up: `aurora_gpu::TileResidency`
// (the GPU atlas) and `aurora_gpu::CanvasPipeline` (the shader that
// draws it) already existed, real and tested, but nothing had ever
// created or drawn either outside that crate's own tests. `App::resumed`
// creates both, sized to the canvas dock area; `App::redraw` syncs the
// atlas from the live store and draws it within that area's own
// viewport, inside the same pass that already clears the background.
//
// `CanvasView`'s own `zoom` is reflected too, added the same week:
// `redraw` passes `canvas_view.zoom()` into `TileResidency::set_origin`,
// which now scales the atlas's own sampled `uv_scale` by it (shader-side
// magnification -- no bigger upload, no mip selection), and
// `canvas_local_origin` picks the right document position via
// `CanvasView::to_document` instead of assuming 100% zoom.
//
// **Sub-tile fractional scroll, fixed 2026-08-13**: `canvas_local_origin`
// used to floor its own result to a whole `TileId` before `set_origin`
// ever saw it, discarding any fractional remainder within that tile --
// invisible at the default view (zoom 100%, no pan) since document (0,0)
// happens to be tile-aligned, but real and visible on any actual zoom or
// pan: painted content landed offset from the cursor, and panning by less
// than one tile didn't visibly move anything. `TileResidency::set_origin`
// now takes the continuous `(f32, f32)` position directly, floors it
// itself for the whole-tile `TileId` its own slot addressing needs, and
// separately stores the fractional remainder (`sub_tile`), which
// `write_uniform` folds into the atlas's own sampled UV offset. See
// `aurora_gpu::residency`'s own doc comment for the mechanism.
//
// Scope, stated honestly: rendering a lower mip while zoomed out or
// panning (the progressive-rendering finding `spike/FINDINGS.md` names),
// rotation, rulers, guides, grid, snap, and true infinite zoom are all
// still separately open, exactly as the bullet's own name says. Window
// resize is handled (`apply_resize` calls `TileResidency::resize`) -- no
// longer part of this remainder.

/// The canvas dock area's own on-screen rectangle, in physical pixels
/// (`bounds`'s logical units scaled by `scale_factor`) — `(x, y, width,
/// height)`. `None` only for a genuinely unknown widget id, which
/// `workspace.canvas_area` never actually is — **not** `None` before any
/// layout has run: `WidgetTree::bounds` returns a widget's *current*
/// bounds unconditionally once it exists, a zero rect by default before
/// the first `compute_layout`, not `None` (confirmed by this function's
/// own tests — a real finding, not assumed from `bounds`'s own doc
/// comment). Used both to size the atlas ([`canvas_area_physical_size`])
/// and to restrict the canvas draw call to this rect via
/// `RenderPass::set_viewport`, so it never draws over the
/// Layers/Properties/History dock.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn canvas_area_physical_rect(
    workspace: &aurora_ui::Workspace,
    scale_factor: f64,
) -> Option<(f32, f32, f32, f32)> {
    let bounds = workspace.tree.bounds(workspace.canvas_area)?;
    let scale = scale_factor as f32;
    Some((
        bounds.x as f32 * scale,
        bounds.y as f32 * scale,
        bounds.width as f32 * scale,
        bounds.height as f32 * scale,
    ))
}

/// [`canvas_area_physical_rect`]'s own width/height, rounded to whole
/// physical pixels (floored at `1` each — `aurora_gpu::TileResidency::new`
/// has no defined behaviour for a zero-sized viewport, and a
/// not-yet-laid-out or momentarily zero-sized dock area is a real
/// transient state, not one worth propagating into a GPU texture size).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn canvas_area_physical_size(
    workspace: &aurora_ui::Workspace,
    scale_factor: f64,
) -> Option<(u32, u32)> {
    let (_, _, width, height) = canvas_area_physical_rect(workspace, scale_factor)?;
    Some((
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
    ))
}

/// Converts [`aurora_ui::CanvasView::zoom`]'s own logical-pixel zoom
/// into the *effective* zoom `redraw`'s real
/// [`aurora_gpu::TileResidency::set_origin`] call site must pass
/// alongside a **physical**-pixel viewport
/// ([`canvas_area_physical_size`]).
///
/// `CanvasView`'s own documented contract (`DEFAULT_ZOOM = 1.0` means
/// "one document pixel occupies exactly one *logical* screen pixel")
/// is expressed entirely in logical pixels — `to_document`/`to_screen`
/// both honour it correctly. But `TileResidency::write_uniform`
/// computes `uv_scale` from a raw `viewport_px / zoom` with no concept
/// of logical vs. physical; it just does the arithmetic on whatever
/// numbers it is given (correctly, on its own terms — this function
/// exists so `redraw` gives it consistent ones, not because that crate
/// has a bug). Feeding it a **physical** `viewport_px` alongside a
/// **logical**-semantics `zoom`, uncorrected, silently doubles the
/// document-pixel count a `scale_factor != 1.0` display's own physical
/// viewport spans versus what `CanvasView`'s contract promises —
/// compressing everything rendered toward the atlas origin (visually:
/// paint landing up-and-left of the cursor, worse the farther from the
/// canvas's own top-left corner, exactly the shape of a real bug report
/// on Retina hardware, root-caused here rather than guessed).
///
/// Multiplying by `scale_factor` restores the identity `redraw` needs:
/// `physical_viewport / (zoom * scale_factor) == logical_viewport /
/// zoom`, since `physical_viewport == logical_viewport * scale_factor`
/// by definition (`winit`'s own physical/logical distinction,
/// `logical_size`/`logical_point`'s own doc comments). Mirrors those
/// two functions' own degenerate-`scale_factor` fallback exactly, for
/// the same reason: a non-finite, zero, or negative `scale_factor` is a
/// value `winit` should never actually report, so this falls back to
/// `1.0` (no scaling) rather than propagating NaN/zero/negative into
/// the GPU uniform.
#[must_use]
fn effective_residency_zoom(canvas_zoom: f32, scale_factor: f64) -> f32 {
    canvas_zoom * guarded_scale_factor(scale_factor)
}

/// `winit`'s reported `scale_factor` as an `f32`, with the degenerate
/// values it should never actually report folded to `1.0` (no scaling)
/// rather than propagating NaN/zero/negative into everything derived
/// from it.
///
/// Factored out of [`effective_residency_zoom`] so [`canvas_min_zoom`]
/// divides by *exactly* the number that function multiplies by. Two
/// spellings of the same guard would be two chances for the two to
/// disagree, which is the whole class of bug this round is closing.
///
/// **The guard runs on the `f32`, after the cast, not on the `f64`
/// before it** — that ordering is the whole point and 0.57.3 had it the
/// wrong way round. `f64 as f32` is a lossy narrowing that can *create*
/// exactly the degenerate values this rejects: an `f64` above
/// `f32::MAX` (~3.4e38) casts to `f32::INFINITY`, and one below
/// `f32::MIN_POSITIVE` (~1.2e-38) casts to `0.0`. Both pass an
/// `is_finite() && > 0.0` test applied to the `f64`, so validating
/// first and casting second let precisely the values the guard names
/// through — `inf` into a multiply and `0.0` into
/// [`canvas_min_zoom`]'s own division. `winit` will report such a
/// scale factor when `WINIT_X11_SCALE_FACTOR` or `Xft.dpi` is
/// misconfigured, so this is reachable, not theoretical.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn guarded_scale_factor(scale_factor: f64) -> f32 {
    let scale = scale_factor as f32;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The lower bound `App`'s own [`aurora_ui::CanvasView`] must be held
/// to, in the *logical*-pixel zoom that view speaks, so that the atlas
/// renders exactly the zoom the view reports.
///
/// [`aurora_gpu::TileResidency::min_zoom_for_viewport`] is the single
/// source of truth for the floor itself; it is expressed against the
/// **physical** viewport the atlas is sized in, so converting it into
/// `CanvasView`'s logical zoom is a division by the same
/// [`guarded_scale_factor`] [`effective_residency_zoom`] multiplies by.
/// The identity that has to hold is
/// `effective_residency_zoom(canvas_min_zoom(v, s), s) >=
/// TileResidency::min_zoom_for_viewport(v)` — i.e. once the view is
/// clamped here, the atlas never clamps again, so the scale it renders
/// at and the scale [`aurora_ui::CanvasView::to_document`] divides by
/// are the same number and a click cannot paint anywhere but under the
/// cursor.
///
/// The `next_up` loop is what makes that an *exact* identity rather than
/// an approximate one: `floor / scale * scale` can land one ulp below
/// `floor`, which would put the atlas's own clamp back in play for that
/// last ulp. Stepping the returned value up until the round trip lands
/// at or above the floor costs at most a few ulps of zoom and removes
/// the whole edge case; the bound is a guard against a much larger
/// number being one, so rounding it up is always the safe direction.
#[must_use]
fn canvas_min_zoom(canvas_size: (u32, u32), scale_factor: f64) -> f32 {
    let floor = aurora_gpu::TileResidency::min_zoom_for_viewport(canvas_size);
    let scale = guarded_scale_factor(scale_factor);
    let mut min_zoom = floor / scale;
    let mut steps = 0;
    while min_zoom * scale < floor && steps < 8 {
        min_zoom = min_zoom.next_up();
        steps += 1;
    }
    min_zoom
}

/// Applies [`canvas_min_zoom`]'s own floor to `view`, re-anchoring a
/// live `drag` against the view move that floor can cause (0.57.8).
///
/// **`aurora_ui::CanvasView::set_min_zoom` moves the view**, which is
/// easy to miss because it reads as a bounds setter. When the current
/// zoom is below a newly raised floor it raises it through `zoom_at`
/// anchored at the canvas area's own top-left corner, and that rewrites
/// `pan` too — so `to_document(p)`, the conversion that turns the
/// pointer into the document point a dab lands on, names a different
/// place for every `p` but the anchor itself. A live drag holds a
/// *document-space* reference point fixed from the moment it began, so
/// this is the same hazard [`apply_scroll_zoom`] closes for
/// scroll-to-zoom, reached from `App::apply_resize` and `App::redraw`:
/// a window resize, a dock layout change, or a scale-factor change
/// while a stroke is still held. Resizing a window mid-stroke is no
/// more "I am done dragging" than scrolling is, so this re-anchors the
/// drag ([`shift_drag_reference`]) rather than ending it, and pairs the
/// two in one function for the same reason `apply_scroll_zoom` does:
/// so moving the view without dealing with the live drag means
/// bypassing a shared function rather than skipping an optional step.
///
/// **Why the correction is measured at the pointer, and why the
/// scroll-zoom argument does not simply transfer.** There, the only
/// thing that moves the view is a pan clamp at an already-fixed zoom,
/// which shifts `to_document(p)` by the *same* amount for every `p` —
/// so any measuring point does, and the zoom anchor is the convenient
/// one. That reasoning does **not** hold here, and assuming it did
/// would correct by the wrong amount: this path changes the *zoom*.
/// Raising `z0` to `z1` anchored at `(0, 0)` leaves `pan` at
/// `p0 * z1 / z0`, so
///
/// ```text
/// to_document_after(x) - to_document_before(x) = x * (1/z1 - 1/z0)
/// ```
///
/// — exactly zero at the anchor and growing linearly away from it, not
/// one shift for the whole canvas. The pointer is the point that makes
/// a single shift right anyway, because every live drag's reference is
/// derived from it and compared against it: `Drag::Brush`/`Eraser`'s
/// `last_doc` *is* `to_document(pointer)` as of the last move event, so
/// shifting it by the change measured there leaves the next event's
/// segment exactly the pointer's own travel (nothing, for a still
/// pointer); `Drag::Move`/`Drag::Marquee`'s `start_doc` enters only as
/// `to_document(pointer) - start_doc`, and shifting both ends by the
/// same amount leaves that difference untouched, so the layer does not
/// teleport.
///
/// `pointer` is the pointer's canvas-area-relative position
/// ([`pointer_in_canvas`]) — `None` when it is over a dock panel, off
/// the window, or before the first layout. There is then no point to
/// measure at, so the floor is applied with no correction rather than a
/// guessed one: no drag advances while the pointer is not over the
/// canvas (`App::handle_pointer_moved` returns before reaching
/// [`continue_drag`]), and a pointer that leaves the canvas area and
/// comes back already interpolates across wherever it went in between.
fn apply_canvas_min_zoom(
    view: &mut aurora_ui::CanvasView,
    drag: Option<&mut Drag>,
    pointer: Option<(f32, f32)>,
    min_zoom: f32,
) {
    let (Some(drag), Some(pointer)) = (drag, pointer) else {
        view.set_min_zoom(min_zoom);
        return;
    };
    let before = view.to_document(pointer);
    view.set_min_zoom(min_zoom);
    let after = view.to_document(pointer);
    shift_drag_reference(drag, (after.0 - before.0, after.1 - before.1));
}

/// A freshly reset [`aurora_ui::CanvasView`] for a newly opened
/// document — default pan and zoom, but **never** a lapsed zoom floor.
///
/// `CanvasView::default()` resets `min_zoom` to `aurora_ui::canvas_view::MIN_ZOOM`,
/// and the document-open paths assign one directly. That left a real
/// window — from the assignment until the *next* `redraw`/`apply_resize`
/// re-applied the floor — in which the view held no floor at all, and
/// any scroll or click processed in that window went through
/// `to_document` dividing by a zoom the atlas would decline to render:
/// the exact render/paint divergence [`canvas_min_zoom`] and
/// [`aurora_ui::CanvasView::set_min_zoom`] exist to close, reopened by
/// the reset itself (measured at up to ~2,000 document px of offset at
/// `MIN_ZOOM` on a 1920 px viewport). Resetting through this function
/// closes it: the floor is re-derived from the live canvas area in the
/// same statement that clears the view, so it is never absent, not even
/// transiently.
///
/// `canvas_size` is [`canvas_area_physical_size`]'s own value (`None`
/// only when the canvas-area widget id is unknown, which does not
/// happen for `workspace.canvas_area` in practice). In that case
/// `previous`'s floor is carried across rather than dropped — a stale
/// floor from the last known canvas size is still a bound, and dropping
/// to `MIN_ZOOM` is the one outcome this must not have.
#[must_use]
fn reset_canvas_view(
    previous: &aurora_ui::CanvasView,
    canvas_size: Option<(u32, u32)>,
    scale_factor: f64,
) -> aurora_ui::CanvasView {
    let min_zoom = canvas_size.map_or_else(
        || previous.min_zoom(),
        |canvas_size| canvas_min_zoom(canvas_size, scale_factor),
    );
    let mut view = aurora_ui::CanvasView::default();
    view.set_min_zoom(min_zoom);
    view
}

/// The whole canvas-view half of adopting a freshly loaded document:
/// [`reset_canvas_view`] and then [`clamp_pan_to_active_layer`], in that
/// order, as one indivisible step.
///
/// **The order is the entire point, which is why this is a function and
/// not two statements at each call site.** `reset_canvas_view` returns
/// a pan of `(0, 0)`, and `(0, 0)` is only *within* the pan bound when
/// the newly active layer's own origin is `(0, 0)` too. A document
/// whose active layer sits elsewhere — a `.aur` file saved after a Move,
/// since `aurora_core::Rect`'s own `x`/`y` round-trip through the
/// manifest and `App::apply_move` never bakes the offset into pixels —
/// would otherwise start with `canvas_local_origin` negative on its very
/// first frame, no panning needed to trigger it. Clamping *before* the
/// reset is worse than useless: the reset would throw the clamp away.
///
/// Every document-adopting path goes through here — [`App::open_file`]'s
/// own flat-image path, [`App::open_aur_file`], and [`App::new`] (which
/// has no window yet, so it passes `canvas_size: None` and leans on
/// `reset_canvas_view`'s own documented "carry `previous`'s floor
/// across" branch, with a default `previous`). Before this existed the
/// sequence was open-coded at each of them and the ordering was
/// enforced only by a comment, which no test could fail.
///
/// `canvas_size` is the *canvas area's* own physical size
/// ([`canvas_area_physical_size`]), not the document's — see
/// [`reset_canvas_view`], whose parameter this is passed straight
/// through to.
#[must_use]
fn load_document_view(
    previous: &aurora_ui::CanvasView,
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    canvas_size: Option<(u32, u32)>,
    scale_factor: f64,
) -> aurora_ui::CanvasView {
    let mut view = reset_canvas_view(previous, canvas_size, scale_factor);
    clamp_pan_to_active_layer(&mut view, layers, active_layer);
    view
}

/// The active pixel layer's own *surface-local, continuous* document
/// position currently at the canvas area's own top-left corner, given
/// `view`'s own pan *and* zoom, and `layer_origin` ([`active_layer_origin`]
/// — the active layer's own document-space `(bounds.x, bounds.y)`,
/// `(0.0, 0.0)` if there isn't one) — what
/// [`aurora_gpu::TileResidency::set_origin`] now takes directly (its own
/// second parameter, since the sub-tile fractional-scroll fix below).
///
/// Goes through [`aurora_ui::CanvasView::to_document`] rather than
/// dividing `view.pan()` by [`aurora_tile::TILE`] directly, so a
/// non-100% zoom is accounted for too (`to_document` already divides by
/// `view.zoom()`) — real zoom-aware panning, not the "assumes
/// `view.zoom() == 1.0`" approximation this function used before
/// [`aurora_gpu::TileResidency::set_origin`] gained real scale support.
/// Subtracting `layer_origin` from that document-space point is what
/// makes a *moved* layer (`aurora_doc::LayerTree::set_bounds`) actually
/// render in its new place, not just update the document model:
/// `aurora_tile::TileStore` addresses a surface from its own local
/// `(0, 0)`, not the document's (the same conversion `layer_local_point`
/// already does for painting), and every layer built before the Move
/// tool existed happened to sit at document `(0, 0)`, so this function
/// never needed to make the distinction until now.
///
/// Deliberately returns the **unfloored, possibly-negative** local
/// point, not a [`aurora_tile::TileId`] — flooring to a whole tile and
/// clamping negative coordinates to `0` are now
/// [`aurora_gpu::TileResidency::set_origin`]'s own job (it needs the
/// whole-tile value for its own slot-addressing bookkeeping and the
/// fractional remainder within that tile for sub-tile-accurate
/// rendering; a caller pre-flooring here would throw the remainder away
/// before `set_origin` ever saw it — exactly the bug this function used
/// to have). Named `canvas_local_origin` rather than the old
/// `tile_origin_for_view`, since "tile origin" no longer describes what
/// this returns.
#[must_use]
fn canvas_local_origin(view: &aurora_ui::CanvasView, layer_origin: (f32, f32)) -> (f32, f32) {
    let (doc_x, doc_y) = view.to_document((0.0, 0.0));
    (doc_x - layer_origin.0, doc_y - layer_origin.1)
}

/// Owns the window, GPU device/surface, and accessibility adapter for
/// one application window. Not part of this crate's public API — [`run`]
/// is the only sanctioned entry point.
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuContext>,
    surface: Option<GpuSurface<'static>>,
    adapter: Option<accesskit_winit::Adapter>,
    proxy: EventLoopProxy<accesskit_winit::Event>,
    /// The real workspace layout (`aurora_ui::build_workspace` — canvas
    /// area + the Layers/Properties/History dock, matching the
    /// owner-approved workspace mockup) — a static structure for now,
    /// no drag-to-redock/resize/persisted layouts yet; see that
    /// function's own doc comment.
    workspace: aurora_ui::Workspace,
    focus: FocusManager,
    /// This build's fixed global keyboard shortcuts ([`default_shortcuts`]).
    shortcuts: ShortcutRegistry<AppCommand>,
    /// The modifier keys currently held down, tracked from
    /// `WindowEvent::ModifiersChanged` — winit reports modifiers and key
    /// presses as separate events, so this is the only way a later
    /// `KeyboardInput` handler knows whether `Ctrl`/`Shift`/... was held
    /// alongside it.
    modifiers: Modifiers,
    /// The open command palette's own root widget, if one is open —
    /// `None` is "closed"; there's no separate visibility flag to keep
    /// in sync (see `aurora_widgets::widgets::command_palette`'s own doc
    /// comment).
    command_palette: Option<WidgetId>,
    /// The open crash-recovery dialog, if one is open — `None` is
    /// "closed" or "never needed one," the same "no separate visibility
    /// flag" shape `command_palette` field above already uses. Opened
    /// once, at construction, if [`previous_session_left_a_marker`] said
    /// so — see this crate's own "crash recovery" section.
    crash_recovery_dialog: Option<DialogHandle>,
    /// This run's own "still running" marker file — written in [`run`]
    /// before this `App` is built, cleared on a clean shutdown
    /// (`WindowEvent::CloseRequested`).
    marker_path: PathBuf,
    /// Where this crate's own persisted workspace layout lives
    /// ([`layout_path`]) — `None` if the OS couldn't report a real
    /// app-support directory to save one in, treated as "layout
    /// preferences aren't being persisted this run," not an error.
    /// Applied once, at construction ([`load_workspace_layout`]); saved
    /// once, on a clean shutdown ([`save_workspace_layout`],
    /// `WindowEvent::CloseRequested`) — the same "write at a real
    /// lifecycle boundary" discipline [`write_autosave`]'s own direct
    /// callers already use, not a reactive save on every
    /// resize/collapse.
    layout_path: Option<PathBuf>,
    /// The window's current DPI scale factor (`Window::scale_factor`) —
    /// read once the real window exists (`resumed`) and kept current via
    /// `WindowEvent::ScaleFactorChanged`, e.g. when the window moves to a
    /// monitor with a different scale. `1.0` until a window exists,
    /// matching `winit`'s own default for a not-yet-realized window.
    scale_factor: f64,
    /// The real system clipboard — [`ClipboardAccess`]'s real
    /// implementation, used by the command palette's `Ctrl+C`/`Ctrl+V`.
    clipboard: SystemClipboard,
    /// The real native file picker — [`FileDialogAccess`]'s real
    /// implementation, used by [`COMMAND_FILE_OPEN`].
    file_dialog: SystemFileDialog,
    /// PLAN.md M1.9's "basic tools" bullet — all seven variants (Move,
    /// Marquee Select, Zoom, Pan, Eyedropper, Brush, Eraser) do
    /// something real once selected; see `aurora_ui::tool`'s own doc
    /// comment for the detail.
    tool: aurora_ui::Tool,
    /// The canvas pan/zoom transform ([`aurora_ui::CanvasView`]) —
    /// deliberately not tied to `layers` (see that field's own doc
    /// comment) since the view itself is a property of the *window*, the
    /// same way Photoshop remembers a document's own zoom/scroll
    /// independent of its pixel content.
    canvas_view: aurora_ui::CanvasView,
    /// The document-level selection ([`aurora_doc::SelectionSet`]) the
    /// Marquee Select tool drags out — a document-level concept in its
    /// own right (see that type's own doc comment), not something that
    /// needs a live `LayerTree` alongside it to exist.
    selection: aurora_doc::SelectionSet,
    /// The live document's own layer structure — built once in
    /// [`App::new`] (from [`demo_document`] or a recovered autosave) and
    /// kept alive from then on. This is what [`Self::active_layer`]/
    /// [`LayerTree::surface_id`] read to find somewhere for the Brush
    /// tool to actually paint.
    layers: aurora_doc::LayerTree,
    /// The document's own real, independent canvas size — `(width,
    /// height)`, in document-space pixels. **Not** derived from any
    /// one layer's own `bounds` on every read ([`document_canvas_size`]
    /// used to be the only source, silently following whichever layer
    /// happened to be on top or shrinking to nothing if that layer was
    /// deleted or resized) — a real editor's canvas can be larger,
    /// smaller, or offset from any single layer it contains. Set once
    /// from [`document_canvas_size`] for a document built without a
    /// real, independent canvas size of its own ([`demo_document`] —
    /// a recovered autosave used to be in that same boat, since the
    /// raw crash-recovery journal persisted no canvas size, but the
    /// autosave file is a real `.aur` container now and its manifest
    /// carries one), from a decoded image's own real dimensions
    /// ([`Self::open_file`]), or from a `.aur` file's own manifest
    /// (`aurora_io::read_aur`'s own third return value,
    /// [`Self::open_aur_file`] and [`recover_document`]) — the one case
    /// this was actually wrong
    /// before: re-saving a `.aur` file whose real canvas size differed
    /// from its topmost layer's own bounds used to silently shrink (or
    /// grow) the canvas to match that layer instead of preserving it.
    canvas_size: (u32, u32),
    /// `layers`' own undo/redo history — built alongside it (same
    /// source: [`demo_document`] or a recovered autosave) and, since
    /// Undo/Redo (`Ctrl+Z`/`Ctrl+Shift+Z`, [`run_command`]), also kept
    /// alive alongside it, not dropped after startup the way it used to
    /// be. `Self::apply_move` is the one live-editing path that records
    /// through this — raw pixel edits (`Self::paint_dab`/
    /// `Self::erase_dab`) still bypass it entirely, since they have no
    /// `aurora_doc::LayerOp` equivalent to record; see
    /// [`Self::pixel_history`] for their own, separate undo instead.
    history: aurora_doc::History,
    /// Undo/redo for completed Brush/Eraser strokes
    /// (`aurora_brush::PixelHistory`) — the pixel-edit half `history`
    /// structurally can't cover (a stroke is raw pixel data, not a
    /// `LayerOp`). Still a separate stack internally (neither type knows
    /// about the other), but `Ctrl+Z`/`Ctrl+Shift+Z` walk it and
    /// `history` as one true chronological sequence via
    /// [`Self::undo_order`]. Populated by `Self::handle_pointer_released`
    /// once a `Drag::Brush`/`Drag::Eraser`'s own accumulated
    /// `StrokeSnapshot` completes.
    pixel_history: aurora_brush::PixelHistory,
    /// The real interleaving order `Ctrl+Z`/`Ctrl+Shift+Z` walk across
    /// `history`'s own structural entries and `pixel_history`'s own
    /// stroke entries — see [`UndoOrder`]'s own doc comment for why this
    /// exists at all (`aurora-brush` and `aurora-doc` are sibling
    /// crates, neither depending on the other, so neither can know about
    /// the other's own activity). `Self::apply_move`/
    /// `Self::handle_pointer_released` record into it; `run_command`
    /// consults it to decide which backing store `Ctrl+Z`/
    /// `Ctrl+Shift+Z` should actually reach into next.
    undo_order: UndoOrder,
    /// Which composite tiles [`recomposite_visible_tiles`] can skip
    /// recomputing this redraw — see [`CompositeCache`]'s own doc
    /// comment. Bumped by every operation that could change what a
    /// composite tile now shows.
    composite_cache: CompositeCache,
    /// The layer the Brush/Eraser tools paint/erase into and the Move
    /// tool repositions, if any — the topmost pixel layer of `layers`
    /// at construction time ([`topmost_pixel_layer`]), real-time-
    /// changeable now by clicking a row in the Layers panel
    /// ([`Self::layer_rows`], [`Self::handle_pointer_pressed`]). `None`
    /// for a document with no pixel layer at all, or once one is
    /// clicked that turns out to be a group (groups are never inserted
    /// into `layer_rows` at all, so this can't actually happen via a
    /// click — only via never having a pixel layer to begin with).
    ///
    /// **The canvas pan boundary is a function of this field — and of
    /// this layer's own `bounds`.** The bound
    /// ([`aurora_ui::CanvasView::clamp_pan_to_minimum`]) is measured
    /// against the active layer's document-space origin
    /// ([`active_layer_origin`], what [`canvas_local_origin`]
    /// subtracts), so it moves when *either* input moves, and a pan that
    /// never moved is then outside it — reopening the render/paint
    /// divergence that clamp exists to close. Writing this field is
    /// therefore only half of what has to re-clamp; the other half is
    /// every path that changes the active layer's `bounds` without
    /// touching this field at all.
    ///
    /// Both halves, and where each re-establishes the bound:
    ///
    /// - *Which layer is active.* [`Self::new`], [`Self::open_file`] and
    ///   [`Self::open_aur_file`] set it and then build the view through
    ///   [`load_document_view`], which clamps as part of the same step.
    ///   [`select_layer`] takes the view and clamps itself.
    /// - *That layer's own bounds.* [`Self::apply_move`] rewrites them
    ///   live, per pointer-move event, and deliberately does **not**
    ///   clamp there (it would feed back into `continue_drag`'s own
    ///   fixed `start_doc` — see [`commit_ending_drag`]); the clamp
    ///   happens once, at the commit, in [`commit_ending_drag`]'s own
    ///   `Drag::Move` arm. [`Self::run_undo_redo`] can revert or reapply
    ///   a recorded bounds change without this field changing at all,
    ///   and clamps via [`perform_undo_redo`]'s own [`after_undo_redo`]
    ///   step.
    ///
    /// A new writer of either that skips the clamp is a bug with no
    /// visible symptom until someone paints.
    ///
    /// **And a clamp that runs while a drag is still live is its own
    /// bug** (0.57.7), in the opposite direction: the drag holds a
    /// document-space reference point fixed from the moment it began,
    /// so a view that moves under it makes the next pointer-move event
    /// measure against a view the drag knows nothing about — for a
    /// `Drag::Brush`, a line of dabs the user never drew. Every path
    /// that moves the view has to say which it does: end the drag first
    /// ([`press_layer_row`], [`perform_undo_redo`], and the Zoom-tool
    /// click branch of [`Self::handle_pointer_pressed`], all through
    /// [`commit_ending_drag`]), or re-anchor it
    /// ([`shift_drag_reference`], for [`apply_scroll_zoom`] and
    /// [`apply_canvas_min_zoom`], where the gesture is not "I am done
    /// dragging").
    ///
    /// The second of those two was the rule's own first exception, and
    /// is worth knowing about as a shape rather than a one-off:
    /// `aurora_ui::CanvasView::set_min_zoom` moves the view without
    /// reading like it does (0.57.8), so `App::apply_resize`/
    /// `App::redraw` broke the rule for a whole round while stating it.
    /// A "setter" that ends up in `zoom_at` or `pan_by` is a path that
    /// moves the view.
    active_layer: Option<aurora_doc::LayerId>,
    /// The colour `Brush` paints with — [`DEFAULT_COLOUR`] until the
    /// Eyedropper tool samples a real pixel and changes it
    /// ([`Self::sample_eyedropper`]). No colour-picker UI exists yet to
    /// set it any other way.
    current_colour: [f32; 3],
    /// Every Layers-panel row's own `WidgetId`, mapped to the `LayerId`
    /// it represents (`aurora_ui::populate_layers_panel`'s own return
    /// value) — what [`Self::handle_pointer_pressed`] looks a
    /// `WidgetTree::hit_test` result up in to turn a click into "select
    /// this layer."
    layer_rows: HashMap<WidgetId, aurora_doc::LayerId>,
    /// This document's own shared tile store (ADR 0010) — `None` if it
    /// failed to open (e.g. an unwritable scratch directory), logged as
    /// a warning rather than treated as fatal, the same "must never stop
    /// the application starting" shape [`write_session_marker`] already
    /// uses for its own I/O. Painting is silently disabled for the
    /// session when this is `None`.
    tile_store: Option<aurora_tile::TileStore>,
    /// The GPU-resident atlas over `tile_store`'s active-layer surface —
    /// `None` until `resumed` has a real device and a computed canvas
    /// area to size it to (real GPU resources can't exist before then).
    /// Sized once, at construction — `aurora_gpu::TileResidency` itself
    /// doesn't support resizing (PLAN.md M1.2's own still-open scope), so
    /// a window resize after startup leaves this showing a fixed-size
    /// sub-window rather than growing/shrinking with the canvas area; a
    /// real, separate follow-on fix, not new scope this field invents.
    residency: Option<aurora_gpu::TileResidency>,
    /// The render pipeline that draws `residency`'s own atlas to the
    /// screen (`aurora_gpu::CanvasPipeline`) — built alongside
    /// `residency`, `None` under the same conditions.
    canvas_pipeline: Option<aurora_gpu::CanvasPipeline>,
    /// The GPU-side multi-layer tile compositor
    /// (`aurora_render::TileCompositor`) `recomposite_visible_tiles`
    /// uses for the qualifying-tile fast path
    /// (`document_qualifies_for_gpu_compositing`/`begin_gpu_composite_tile`) —
    /// built alongside `residency`/`canvas_pipeline` (same "needs a real
    /// device" constraint), `None` under the same conditions, in which
    /// case `recomposite_visible_tiles` falls straight back to its own
    /// CPU path for every tile, same as before this field existed.
    compositor: Option<aurora_render::TileCompositor>,
    /// The GPU path renderer (`aurora_widgets::PathPipeline`) every
    /// widget's own paint (`aurora_widgets::paint_widget`) draws
    /// through — built in `resumed` alongside `canvas_pipeline` (same
    /// "needs a real device" constraint), but unlike `residency`/
    /// `canvas_pipeline` doesn't need a computed canvas area to size
    /// itself to, so it's never skipped once a device exists.
    path_pipeline: Option<PathPipeline>,
    /// The pointer's last known position, in the *window's* own logical
    /// space (already DPI-adjusted — see [`logical_point`]) — `None`
    /// before the first `CursorMoved`, or after `CursorLeft`.
    pointer_position: Option<(f32, f32)>,
    /// An in-progress pointer drag (Pan, Marquee Select, Brush, or
    /// Eraser), if any — `None` is "not dragging," the same "no separate
    /// flag" shape `command_palette`/`crash_recovery_dialog` above
    /// already use.
    drag: Option<Drag>,
    /// An in-progress dock-rail resize, if any — deliberately separate
    /// from `drag`: resizing the rail is neither canvas-relative nor
    /// tool-dependent, unlike everything `Drag` itself models (see
    /// [`RailResize`]'s own doc comment).
    rail_resize: Option<RailResize>,
    /// The native menu bar — macOS only, see this crate's own "native
    /// menu bar" section for why Windows/Linux aren't included. Built
    /// in [`App::new`] (no window needed); attached to the real
    /// application menu bar in `resumed` (`Menu::init_for_nsapp`).
    #[cfg(target_os = "macos")]
    menu: muda::Menu,
    /// The window's background clear colour, resolved from
    /// `design/themes/dark.toml`'s `surface.app` token
    /// (`background_color_from_theme`) — invariant §7.3.10 (no
    /// hardcoded style values) applied to the one thing this crate drew
    /// before real widget painting existed.
    background: wgpu::Color,
    /// The resolved Dark theme ([`load_theme`]) — `background` above is
    /// one, one-time derivation from it (`surface.app`); [`Self::redraw`]
    /// re-reads it every frame for every widget's own paint
    /// (`aurora_widgets::paint_widget`), so the resolved `Theme` itself
    /// has to stay alive for the session, not just the one colour it
    /// used to be reduced to.
    theme: Theme,
    /// The resolved scales (`load_scales`) — kept alive the same way
    /// `theme` is, for the same reason: [`Self::redraw`] needs a real
    /// `&Scales` every frame to resolve each widget's own paint
    /// geometry (button corner radius, currently), and re-parsing
    /// `design/tokens/scales.toml` every frame (the one-off convention
    /// every other call site in this crate still uses, since they only
    /// run on a real user action, not 60 times a second) would be real,
    /// avoidable per-frame work.
    scales: Scales,
    /// Set when a step that can't be retried fails (window/device/surface
    /// creation) — `run` turns this into a nonzero exit, distinguishing
    /// it from the ordinary, successful case of the user closing the
    /// window.
    failed: bool,
    /// Whether the next [`Self::about_to_wait`] should actually request a
    /// redraw — set on every real `WindowEvent` other than
    /// `RedrawRequested` itself, cleared once a redraw has been
    /// requested for it. Without this, `about_to_wait` requesting a
    /// redraw unconditionally on *every* loop iteration (including the
    /// iteration its own previous request just woke) turns
    /// `ControlFlow::Wait` into a permanent busy loop — real, measured
    /// 100% CPU on real hardware, not `ControlFlow::Wait`'s intended
    /// "block until something changes." Starts `true` so the window's
    /// very first frame actually paints.
    needs_redraw: bool,
}

impl ShutdownState for App {
    fn marker_path(&self) -> &Path {
        &self.marker_path
    }

    fn autosave_path(&self) -> PathBuf {
        autosave_path()
    }

    fn take_tile_store(&mut self) -> Option<aurora_tile::TileStore> {
        self.tile_store.take()
    }

    fn scratch_dir(&self) -> Option<&Path> {
        tile_store_scratch_dir()
    }
}

impl App {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    fn new(
        proxy: EventLoopProxy<accesskit_winit::Event>,
        theme: Theme,
        background: wgpu::Color,
        scales: Scales,
        marker_path: PathBuf,
        had_previous_marker: bool,
        autosave_path: &Path,
        layout_path: Option<PathBuf>,
    ) -> Self {
        let mut workspace = aurora_ui::build_workspace();
        if let Some(layout_path) = layout_path.as_deref() {
            load_workspace_layout(layout_path, &mut workspace);
        }
        // Opened *before* recovery, not after: `recover_document` writes
        // the autosave's own tiles straight into a live store, so the
        // store has to exist first.
        let mut tile_store = open_tile_store();
        let StartupDocument {
            layers,
            history,
            canvas_size,
            was_recovered,
        } = startup_document(had_previous_marker, autosave_path, &mut tile_store);
        let layer_rows = match aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            &layers,
        ) {
            Ok(rows) => rows,
            Err(err) => {
                unreachable!("workspace.layers was just built by build_workspace above: {err:?}")
            }
        };
        if let Err(err) =
            aurora_ui::populate_history_panel(&mut workspace.tree, workspace.history, &history)
        {
            unreachable!("workspace.history was just built by build_workspace above: {err:?}");
        }
        // Seeded from the tool this session actually starts with
        // (`aurora_ui::Tool::default()`, `MarqueeSelect` — see
        // `Self::tool`'s own field assignment below), which has no real
        // options yet, so the Properties panel legitimately starts
        // empty here, not populated with a placeholder.
        if let Err(err) = aurora_ui::populate_properties_panel(
            &mut workspace.tree,
            workspace.properties,
            aurora_ui::Tool::default(),
            &tool_options(aurora_ui::Tool::default()),
        ) {
            unreachable!("workspace.properties was just built by build_workspace above: {err:?}");
        }
        let active_layer = topmost_pixel_layer(&layers);
        // The pan bound, established before the first frame. Crash
        // recovery reopens a real `.aur` container
        // (`recover_document`/`read_autosave_container`), so this
        // document's own active layer can already sit away from
        // `(0, 0)` — the same `.aur` round-trip `Self::open_aur_file`
        // has to clamp for, just reached at startup instead.
        //
        // Clamped here at zoom 1.0, before any zoom floor exists (there
        // is no window yet, so no canvas size to derive one from --
        // hence `canvas_size: None`, `load_document_view`'s own
        // "carry `previous`'s floor across" branch, with a default
        // `previous` whose floor is `MIN_ZOOM`; identical to the bare
        // `CanvasView::default()` this used to build, minus the missing
        // clamp). That is sound rather than merely early: the floor is
        // applied later by `set_min_zoom` (from `redraw`/`apply_resize`),
        // which raises zoom through `zoom_at((0.0, 0.0), ..)`, and
        // `zoom_at` sets `pan = anchor - to_document(anchor) * new_zoom`
        // — at the `(0, 0)` anchor that holds `to_document((0, 0))`
        // fixed across the raise. So a view satisfying the bound here
        // still satisfies it after the floor lands; the raise cannot
        // undo this clamp.
        let canvas_view = load_document_view(
            &aurora_ui::CanvasView::default(),
            &layers,
            active_layer,
            None,
            1.0,
        );

        let mut focus = FocusManager::default();
        let mut crash_recovery_dialog = None;
        if had_previous_marker {
            open_crash_recovery_dialog(
                &mut workspace,
                &mut focus,
                &mut crash_recovery_dialog,
                &scales,
                was_recovered,
            );
        }

        Self {
            window: None,
            gpu: None,
            surface: None,
            adapter: None,
            proxy,
            workspace,
            focus,
            shortcuts: default_shortcuts(),
            modifiers: Modifiers::none(),
            command_palette: None,
            crash_recovery_dialog,
            marker_path,
            layout_path,
            scale_factor: 1.0,
            clipboard: SystemClipboard::new(),
            file_dialog: SystemFileDialog,
            tool: aurora_ui::Tool::default(),
            canvas_view,
            selection: aurora_doc::SelectionSet::new(),
            layers,
            canvas_size,
            history,
            pixel_history: aurora_brush::PixelHistory::new(),
            undo_order: UndoOrder::default(),
            composite_cache: CompositeCache::default(),
            active_layer,
            current_colour: DEFAULT_COLOUR,
            layer_rows,
            tile_store,
            residency: None,
            canvas_pipeline: None,
            compositor: None,
            path_pipeline: None,
            pointer_position: None,
            drag: None,
            rail_resize: None,
            #[cfg(target_os = "macos")]
            menu: build_menu(),
            background,
            theme,
            scales,
            failed: false,
            needs_redraw: true,
        }
    }

    /// Whether the app exited because of an earlier unrecoverable error,
    /// rather than an ordinary window close.
    #[must_use]
    fn failed(&self) -> bool {
        self.failed
    }

    /// Sends the current accessibility tree to the platform, if (and
    /// only if) something is actually listening —
    /// `Adapter::update_if_active` is a no-op otherwise, matching
    /// `spike/a11y-ime`'s own `push_a11y` (this project's first, proven
    /// real usage of this exact call).
    fn push_accessibility(&mut self) {
        let Some(adapter) = self.adapter.as_mut() else {
            return;
        };
        let tree = &self.workspace.tree;
        let focused = self.focus.focused().unwrap_or(self.workspace.root);
        adapter.update_if_active(|| tree.accessibility_update(focused));
    }

    /// A real `winit::event::KeyEvent`'s full handling: ignores key-up
    /// (only a press should trigger a shortcut or type a character —
    /// otherwise every binding would fire twice), translates it into
    /// this crate's own platform-free vocabulary, and routes it via
    /// [`handle_key`] — the pure logic this method exists only to feed
    /// real platform input into. Re-runs layout unconditionally
    /// afterward: pure CPU geometry on a small tree, no GPU involved
    /// (`App::apply_resize`'s own doc comment), and genuinely needed
    /// after more than just `Ctrl+Shift+P` opening the palette for the
    /// first time — narrowing the query while it's open changes its own
    /// result-row count, which changes each row's own share of the
    /// body's height (`aurora_widgets::widgets::command_palette`'s own
    /// `row_style`), not just the palette's own first appearance.
    fn handle_key_event(&mut self, event: &winit::event::KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let Some(key) = translate_key(&event.logical_key) else {
            return;
        };
        let picked = handle_key(
            &mut self.workspace,
            &mut self.focus,
            &mut self.crash_recovery_dialog,
            &mut self.command_palette,
            &mut self.tool,
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            self.tile_store.as_mut(),
            &mut self.undo_order,
            &mut self.composite_cache,
            &self.shortcuts,
            self.modifiers,
            key,
            event.text.as_deref(),
            &mut self.clipboard,
            &mut self.file_dialog,
        );
        match picked {
            Some(ActivatedCommand::OpenFile(path)) => self.open_file(&path),
            Some(ActivatedCommand::SaveFile(path)) => self.save_file(&path),
            Some(ActivatedCommand::Undo) => self.run_undo_redo(AppCommand::Undo),
            Some(ActivatedCommand::Redo) => self.run_undo_redo(AppCommand::Redo),
            None => {}
        }
        let window_size = self.window.as_ref().map(|window| window.inner_size());
        if let Some(size) = window_size {
            self.apply_resize((size.width, size.height));
        }
        self.push_accessibility();
    }

    /// Runs `command` (`AppCommand::Undo` or `::Redo`) against this
    /// app's own live state — [`perform_undo_redo`] against `App`'s own
    /// fields. A `&mut self` wrapper so the two call sites can spell it
    /// in one line; the real logic (**and the order the three steps
    /// have to run in** — commit the live drag, run the command, then
    /// re-establish the composite cache and the pan bound) is the free
    /// function, which needs no `App` (and therefore no GPU adapter) to
    /// test, the same split [`Self::commit_drag`] already uses.
    ///
    /// What the command palette's and (macOS) native menu's own
    /// Undo/Redo entries fall back to once `activate_command` hands the
    /// bare command back up (deliberately kept free of `layers`/
    /// `history`/`pixel_history`/the tile store — see
    /// [`ActivatedCommand`]'s own doc comment for why). **Not** the
    /// `Ctrl+Z`/`Ctrl+Shift+Z` path, which [`handle_key`] resolves and
    /// runs through [`run_command`] itself without ever returning an
    /// `ActivatedCommand` — see PLAN.md's own residual disclosure for
    /// what that costs and why closing it is its own change.
    fn run_undo_redo(&mut self, command: AppCommand) {
        perform_undo_redo(
            &mut self.workspace,
            &mut self.focus,
            &mut self.command_palette,
            &mut self.tool,
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            self.tile_store.as_mut(),
            &mut self.undo_order,
            &mut self.composite_cache,
            &mut self.canvas_view,
            self.active_layer,
            &mut self.drag,
            command,
        );
    }

    /// Opens a real, native `WindowEvent::DroppedFile` — the same
    /// [`Self::open_file`] the palette's "Open File…" command uses, since
    /// a dropped file and a chosen one are the same kind of "the user
    /// wants to open this" signal, whichever route it arrived by.
    fn handle_dropped_file(&mut self, path: &Path) {
        self.open_file(path);
    }

    /// Opens `path` as a real document — a real, multi-layer `.aur` file
    /// ([`Self::open_aur_file`]) if the extension names one, otherwise a
    /// flat image ([`open_image`]) replacing the current document with a
    /// fresh, single-layer one sized to it ([`replace_document`]),
    /// writing the image's own pixels into the live tile store
    /// (`aurora_io::write_into_store`) so the canvas actually shows it.
    /// A read/decode failure (bad file, unrecognised extension) is
    /// logged and leaves the current document completely untouched —
    /// the same honesty [`recover_document`] already applies to a bad
    /// autosave, extended here to a bad chosen file.
    ///
    /// Resets `canvas_view`/`selection`/`drag` to their own fresh-
    /// session defaults — a newly opened document has no relationship
    /// to whatever pan/zoom/selection/in-progress-drag the *previous*
    /// one had.
    fn open_file(&mut self, path: &Path) {
        if is_aur_path(path) {
            self.open_aur_file(path);
            return;
        }
        let Some(image) = open_image(path) else {
            return;
        };
        let name = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("Image");
        let (layers, history, layer_id) = document_from_image(name, &image);
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => {
                tracing::error!(%err, "failed to load design scales; cannot open a document");
                return;
            }
        };
        let (layer_rows, active_layer) =
            match replace_document(&mut self.workspace, &scales, &layers, &history, self.tool) {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to rebuild the workspace panels for the opened document"
                    );
                    return;
                }
            };

        // The image's own real, decoded dimensions -- known exactly
        // here, rather than derived back out of the one layer just
        // built from it (`document_canvas_size`'s own fallback role).
        let canvas_size = (image.width(), image.height());
        if let Some(store) = self.tile_store.as_mut() {
            if let Some(surface) = layers.surface_id(layer_id)
                && let Err(err) = aurora_io::write_into_store(&image, store, surface)
            {
                tracing::warn!(
                    ?err,
                    "failed to write the opened image's pixels into the tile store"
                );
            }
            // After the pixels land in the store, not before: the
            // autosave container carries this document's real tiles now,
            // so writing it first would persist an empty one.
            write_autosave(&autosave_path(), &layers, &history, canvas_size, store);
        } else {
            tracing::warn!("no live tile store; skipping the opened document's autosave");
        }

        self.layers = layers;
        self.canvas_size = canvas_size;
        self.history = history;
        // A freshly opened document has no relationship to the previous
        // one's own undo state either -- `self.history` above is a
        // brand-new, empty `History` (not merged with the old one), so
        // keeping the old `pixel_history`/`undo_order` around would let
        // Ctrl+Z reach into a document that's no longer open, and
        // `undo_order` would already be desynced from `history`'s own
        // (now-empty) stacks regardless.
        self.pixel_history = aurora_brush::PixelHistory::new();
        self.undo_order = UndoOrder::default();
        self.composite_cache.bump();
        self.active_layer = active_layer;
        self.layer_rows = layer_rows;
        // Through `load_document_view`, never `reset_canvas_view` or
        // `CanvasView::default()` directly: the default drops the
        // atlas's zoom floor, and the reset on its own drops the pan
        // bound. `load_document_view` is both, in the one order that is
        // correct -- see its own doc comment. Assigned after
        // `self.active_layer`/`self.layers` above, since it reads them.
        self.canvas_view = load_document_view(
            &self.canvas_view,
            &self.layers,
            self.active_layer,
            canvas_area_physical_size(&self.workspace, self.scale_factor),
            self.scale_factor,
        );
        self.selection = aurora_doc::SelectionSet::new();
        // Dropped, deliberately not committed through `commit_drag`:
        // `pixel_history` was replaced wholesale a few lines up, so an
        // entry pushed here would be discarded immediately -- and it
        // would capture tiles on a surface belonging to a document that
        // is no longer open. This is the one place dropping a live drag
        // is right; see `commit_ending_drag`.
        self.drag = None;
        self.push_accessibility();
    }

    /// Opens a real `.aur` file (ADR 0009): `aurora_io::read_aur` gives
    /// back a real, possibly multi-layer `LayerTree`/`History`, its own
    /// tiles written directly into the live tile store — the same
    /// document-replacement shape [`Self::open_file`]'s own flat-image
    /// path uses ([`replace_document`], resetting
    /// `canvas_view`/`selection`/`drag`), just fed by a real document
    /// reader instead of a single decoded image. A silent no-op
    /// (logged) if there's no live tile store, the file fails to open,
    /// or `read_aur` itself fails (corrupt file, missing manifest/
    /// history entry, or an unsupported future schema version).
    fn open_aur_file(&mut self, path: &Path) {
        let Some(store) = self.tile_store.as_mut() else {
            tracing::warn!(path = %path.display(), "no live tile store; cannot open a .aur file");
            return;
        };
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "failed to open the chosen .aur file");
                return;
            }
        };
        // The profile (4th element) is a real, checked value now
        // (`aurora_io::aur`'s own ICC round-trip), but nothing in this
        // crate yet tracks a "current document profile" to restore it
        // into -- no colour-management UI exists to have set one in the
        // first place, so every `.aur` file this app has ever written
        // only ever carries `None` in practice. Discarded here rather
        // than invented a field to hold, honestly, until that UI exists.
        let (layers, history, canvas_size, _profile) = match aurora_io::read_aur(file, store) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(path = %path.display(), ?err, "failed to read the chosen .aur file");
                return;
            }
        };
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => {
                tracing::error!(%err, "failed to load design scales; cannot open a document");
                return;
            }
        };
        let (layer_rows, active_layer) =
            match replace_document(&mut self.workspace, &scales, &layers, &history, self.tool) {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to rebuild the workspace panels for the opened document"
                    );
                    return;
                }
            };
        // Re-borrowed rather than reusing the `store` binding above:
        // that borrow of `self.tile_store` has to end before
        // `replace_document`'s own `&mut self.workspace` above, and
        // `read_aur` has already populated the store by now, so the
        // container this writes carries the opened document's real
        // tiles.
        if let Some(store) = self.tile_store.as_mut() {
            write_autosave(&autosave_path(), &layers, &history, canvas_size, store);
        }

        self.layers = layers;
        // The file's own real, saved canvas size -- restored directly,
        // not re-derived from whichever layer it contains (the bug this
        // field exists to fix; see `Self::canvas_size`'s own doc
        // comment).
        self.canvas_size = canvas_size;
        self.history = history;
        // See the same reset in `Self::open_file`'s own flat-image path.
        self.pixel_history = aurora_brush::PixelHistory::new();
        self.undo_order = UndoOrder::default();
        self.composite_cache.bump();
        self.active_layer = active_layer;
        self.layer_rows = layer_rows;
        // Through `load_document_view`, never `reset_canvas_view` or
        // `CanvasView::default()` directly: the default drops the
        // atlas's zoom floor, and the reset on its own drops the pan
        // bound. `load_document_view` is both, in the one order that is
        // correct -- see its own doc comment. Assigned after
        // `self.active_layer`/`self.layers` above, since it reads them.
        self.canvas_view = load_document_view(
            &self.canvas_view,
            &self.layers,
            self.active_layer,
            canvas_area_physical_size(&self.workspace, self.scale_factor),
            self.scale_factor,
        );
        self.selection = aurora_doc::SelectionSet::new();
        // Dropped, deliberately not committed through `commit_drag`:
        // `pixel_history` was replaced wholesale a few lines up, so an
        // entry pushed here would be discarded immediately -- and it
        // would capture tiles on a surface belonging to a document that
        // is no longer open. This is the one place dropping a live drag
        // is right; see `commit_ending_drag`.
        self.drag = None;
        self.push_accessibility();
    }

    /// Saves to `path` — the whole document, real and multi-layer
    /// ([`Self::save_aur_file`]), if the extension names `.aur`;
    /// otherwise a flat, composited export of the real document, built
    /// by [`composite_document`] (every visible pixel layer, each
    /// composited with its own real, translated blend mode — see below
    /// — walking `self.canvas_size` — see that field's own doc comment
    /// for why it, not any one layer's own `bounds`, is the real
    /// document extent), encoding via whichever format `path`'s own
    /// extension names (`aurora_io::encode_by_extension`), and writing
    /// the result to disk with [`write_verified`]'s own "never leave a
    /// corrupt file in place" discipline.
    ///
    /// **Scope, stated honestly**: this is the same real multi-layer
    /// composite the canvas itself already shows
    /// ([`recomposite_visible_tiles`], called from [`Self::redraw`]) —
    /// no longer just the active layer's own pixels, the bug this
    /// function used to have. Blend modes are real for `Normal` plus 25
    /// of the 26 others — the "simple separable" family
    /// (`Darken`/`Multiply`/`Lighten`/`Screen`/`Difference`/`Exclusion`/
    /// `Subtract`/`Divide`), the "dodge and burn" family
    /// (`ColorDodge`/`LinearDodge`/`ColorBurn`/`LinearBurn`), the
    /// "overlay and light" family (`Overlay`/`SoftLight`/`HardLight`/
    /// `VividLight`/`LinearLight`/`PinLight`/`HardMix`), the
    /// non-separable HSL family (`Hue`/`Saturation`/`Color`/
    /// `Luminosity`), and the whole-colour-selection family
    /// (`DarkerColor`/`LighterColor`),
    /// via `translate_blend_mode` and `aurora_render::composite_tile_cpu`'s
    /// own current scope — with the one remaining `aurora_doc::BlendMode`
    /// variant (`Dissolve` — this family's own explicit, now sole
    /// boundary) still
    /// silently falling back to `Normal` — a real, still-open gap. Layer
    /// groups are recursed into at any depth `aurora-doc` will accept
    /// (bounded independently by `aurora_doc::MAX_LAYER_TREE_DEPTH` —
    /// see `resolve_tile`'s own doc comment), ancestor-visibility-gated
    /// (`resolve_tile`'s own shared recursion, walking
    /// `aurora_doc::LayerTree::roots`/`LayerTree::children`) — a
    /// group's own `opacity`/`blend_mode` **are** now aggregated into
    /// its children's effective compositing, by isolating a group's own
    /// visible direct children first and then compositing that isolated
    /// result one level up using the group's own opacity/blend mode
    /// (the only semantic `aurora_doc::BlendMode` can express, since it
    /// has no "Pass Through" variant) — real for the common cases (a
    /// single child of any opacity, or multiple children combining via
    /// `Normal`) after `resolve_tile`'s own un-premultiply fix, with a
    /// narrower, still-open gap for a group's own children combining
    /// via a non-`Normal` blend mode against each other mid-isolation;
    /// see `resolve_tile`'s own doc comment for the exact boundary.
    /// `.aur`, by contrast, saves every layer's own real tiles
    /// regardless of which one is active, plus history/layer metadata
    /// this flat path has no format to carry — see
    /// [`Self::save_aur_file`]'s own doc comment for that path's own
    /// scope.
    ///
    /// A silent no-op if there's no live tile store — the same
    /// absent-precondition honesty [`Self::paint_dab`] already uses. A
    /// real, logged failure (compositing the pixels, encoding, or
    /// writing the file) is worth a warning, though.
    ///
    /// **Nothing is written if the composite came out incomplete.**
    /// [`composite_document`] returns
    /// [`aurora_io::IoError::IncompleteComposite`] when a layer tile
    /// could not be read out of the store (a corrupted scratch-disk
    /// tile, say), and this function then returns without touching
    /// `path` at all — no partial file, no overwrite of whatever was
    /// there. That refusal is currently log-only: surfacing it as the
    /// itemized, user-visible warning FR-001 wants is separate, still-
    /// open work tracked in PLAN.md.
    fn save_file(&mut self, path: &Path) {
        if is_aur_path(path) {
            self.save_aur_file(path);
            return;
        }
        let Some(store) = self.tile_store.as_mut() else {
            return;
        };
        let (width, height) = self.canvas_size;
        let image = match composite_document(&self.layers, store, width, height) {
            Ok(image) => image,
            Err(err) => {
                // Includes `IoError::IncompleteComposite` -- the export
                // refused because one or more layer tiles could not be
                // read (see `composite_document`'s own `# Errors`). The
                // *file* is safe either way: nothing is written, and
                // whatever was already at `path` is untouched. What is
                // still missing is telling the user, rather than only the
                // log -- tracked in PLAN.md, not solved here.
                tracing::error!(?err, "refusing to export: could not composite the document");
                return;
            }
        };
        let bytes = match aurora_io::encode_by_extension(path, &image) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(path = %path.display(), ?err, "failed to encode the exported image");
                return;
            }
        };
        if write_verified(path, &bytes, image.width(), image.height()) {
            tracing::info!(path = %path.display(), "exported the composited document");
        }
    }

    /// Saves the whole live document to `path` as a real `.aur` file
    /// (ADR 0009, `aurora_io::write_aur`): every pixel layer's own
    /// tiles, not just the active one — the real answer to the
    /// non-`.aur` export path's own "active layer only" limit. Writes
    /// to a sibling `.tmp` file first, verifies it by reading it back
    /// with a throwaway `aurora_tile::TileStore` ([`verify_aur`]), then
    /// atomically renames it over `path` — the same
    /// [`write_verified`]-style discipline the flat-format export path
    /// uses, just against a different verification step (`.aur` has no
    /// single "does this decode to the right width/height" check the way a flat image
    /// does).
    ///
    /// **Scope, stated honestly**: `canvas_size` is `self.canvas_size`,
    /// the document's own real, independent canvas size — no longer
    /// re-derived from the topmost pixel layer's own bounds on every
    /// save (see [`Self::canvas_size`]'s own doc comment for the bug
    /// that used to cause). No colour profile is passed
    /// (`aurora_io::write_aur`'s own `profile: None`) — this crate has
    /// no colour-management UI yet to have set a non-sRGB one with, even
    /// though the format itself round-trips a real one now (`aurora-io`).
    /// `history` is `self.history`, the real live journal — Move
    /// (`Self::apply_move`) records through it, so a `.aur` file this
    /// session writes carries a real, if partial, undo journal;
    /// `Self::paint_dab`/`Self::erase_dab` still bypass it entirely (see
    /// `Self::history`'s own doc comment), so a document with
    /// brush/eraser edits still saves a journal that omits them — a
    /// real, named gap, not the previous "always completely empty" one.
    /// A silent no-op if there's no live tile store.
    fn save_aur_file(&mut self, path: &Path) {
        let Some(store) = self.tile_store.as_mut() else {
            return;
        };

        let Some(file_name) = path.file_name() else {
            tracing::warn!(path = %path.display(), "save path has no file name");
            return;
        };
        let mut temp_name = file_name.to_os_string();
        temp_name.push(".tmp");
        let temp_path = path.with_file_name(temp_name);

        let write_result: Result<(), aurora_io::IoError> = (|| {
            let file = std::fs::File::create(&temp_path)?;
            aurora_io::write_aur(
                file,
                &self.layers,
                &self.history,
                self.canvas_size,
                None,
                store,
            )
        })();
        if let Err(err) = write_result {
            tracing::warn!(path = %temp_path.display(), ?err, "failed to write the temp .aur export file");
            let _ = std::fs::remove_file(&temp_path);
            return;
        }

        if !verify_aur(&temp_path) {
            tracing::warn!(path = %temp_path.display(), "exported .aur file failed to verify by reading it back");
            let _ = std::fs::remove_file(&temp_path);
            return;
        }

        if let Err(err) = std::fs::rename(&temp_path, path) {
            tracing::warn!(path = %path.display(), %err, "failed to replace the destination with the verified export");
            let _ = std::fs::remove_file(&temp_path);
            return;
        }
        tracing::info!(path = %path.display(), "exported the document as .aur");
    }

    /// A real `WindowEvent::CursorMoved`: updates the tracked pointer
    /// position and, if a rail resize is in progress
    /// ([`RailResize`]), applies the new width
    /// (`aurora_ui::set_rail_width`) and re-runs layout — checked first
    /// and returns early, since a resize is neither canvas-relative nor
    /// tool-dependent, the same reason [`Self::handle_pointer_pressed`]
    /// checks [`pointer_on_rail_divider`] before its own canvas gate.
    /// Otherwise, if a canvas drag is in progress, advances it
    /// ([`continue_drag`]), painting or erasing any dab positions it
    /// returns ([`Self::paint_dab`]/[`Self::erase_dab`], chosen by which
    /// `Drag` variant is active) — empty for every drag but
    /// `Brush`/`Eraser`. For `Drag::Move`, applies its own live,
    /// just-updated `current_bounds` to the document
    /// ([`Self::apply_move`]) instead — the "read the field
    /// `continue_drag` just updated, then do the one real mutation"
    /// half of the split that function's own doc comment describes. For
    /// `Drag::Eyedropper`, samples directly at the current point
    /// ([`Self::sample_eyedropper`]) every event, so dragging with the
    /// Eyedropper tool held down updates `current_colour` live, the
    /// same as a real image editor's own eyedropper.
    fn handle_pointer_moved(&mut self, physical_position: (f64, f64)) {
        let position = logical_point(physical_position, self.scale_factor);
        self.pointer_position = Some(position);

        if let Some(resize) = self.rail_resize {
            let new_width = resized_rail_width(resize, position.0);
            if let Err(err) = aurora_ui::set_rail_width(
                &mut self.workspace.tree,
                self.workspace.rail,
                self.workspace.divider,
                new_width,
            ) {
                tracing::warn!(?err, "failed to resize the dock rail");
            }
            let window_size = self.window.as_ref().map(|window| window.inner_size());
            if let Some(size) = window_size {
                self.apply_resize((size.width, size.height));
            }
            return;
        }

        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };
        if let Some(drag) = self.drag.as_mut() {
            let erasing = matches!(drag, Drag::Eraser { .. });
            let dabs = continue_drag(
                drag,
                canvas_point,
                &mut self.canvas_view,
                &mut self.selection,
                active_layer_origin(&self.layers, self.active_layer),
            );
            for doc_point in dabs {
                if erasing {
                    self.erase_dab(doc_point);
                } else {
                    self.paint_dab(doc_point);
                }
            }
        }
        match self.drag.as_ref() {
            Some(Drag::Move {
                layer_id,
                current_bounds,
                ..
            }) => {
                let (layer_id, current_bounds) = (*layer_id, *current_bounds);
                self.apply_move(layer_id, current_bounds);
            }
            Some(Drag::Eyedropper) => {
                let doc_point = self.canvas_view.to_document(canvas_point);
                self.sample_eyedropper(doc_point);
            }
            _ => {}
        }
    }

    /// A real `WindowEvent::MouseInput { state: Pressed, .. }`: starts a
    /// dock-rail resize ([`RailResize`]) on the divider
    /// ([`pointer_on_rail_divider`]), performs the active Zoom tool's
    /// click-to-zoom ([`handle_zoom_tool_click`]), or starts a canvas
    /// drag ([`begin_drag`]) — never more than one of these for the
    /// same press. A fresh `Brush`/`Eraser`/`Eyedropper` drag paints/
    /// erases/samples its own starting point immediately
    /// ([`Self::paint_dab`]/[`Self::erase_dab`]/[`Self::sample_eyedropper`]),
    /// so a plain click (no drag at all) still does something.
    ///
    /// **Every branch that reaches the canvas ends whatever drag was
    /// still live first** — the layer-row branch through
    /// [`press_layer_row`], the Zoom-tool and drag branches through the
    /// shared `self.commit_drag(self.drag.take())` (moved above the
    /// Zoom-tool branch in 0.57.7, which used to `return` past it). A
    /// press is a second gesture, and the two things it would otherwise
    /// do to a live drag — drop its undo entry, and move the view out
    /// from under its fixed reference point — are exactly the pair
    /// [`commit_ending_drag`] and [`Self::active_layer`]'s own doc
    /// comment describe.
    fn handle_pointer_pressed(&mut self, button: winit::event::MouseButton) {
        let Some(button) = translate_pointer_button(button) else {
            return;
        };
        let Some(position) = self.pointer_position else {
            return;
        };

        // The crash-recovery dialog, if open, owns every pointer press
        // the same way it already owns the keyboard (`handle_key`'s own
        // routing order) — a modal alert blocks everything else,
        // including layer selection and canvas tools.
        if handle_dialog_pointer(
            &mut self.workspace,
            &mut self.focus,
            &mut self.crash_recovery_dialog,
            button,
            position,
        ) {
            self.push_accessibility();
            return;
        }

        // Layer selection takes priority over — and is independent of —
        // whichever canvas tool is active: clicking a Layers panel row
        // selects a layer no matter what the Brush/Zoom/Pan/... tool is
        // doing, the same way it would in any real image editor.
        if button == PointerButton::Primary
            && let Some(hit) = self.workspace.tree.hit_test(position)
            && let Some(&layer_id) = self.layer_rows.get(&hit)
        {
            // Through `press_layer_row`, never `select_layer` alone:
            // the selection moves the pan bound, and no drag may still
            // be live under it. See that function's own doc comment.
            press_layer_row(
                &mut self.workspace,
                &self.layer_rows,
                &mut self.active_layer,
                &mut self.canvas_view,
                &self.layers,
                &mut self.history,
                &mut self.pixel_history,
                &mut self.undo_order,
                &mut self.drag,
                layer_id,
            );
            // Changes `recomposite_visible_tiles`'s own reference origin
            // (the newly active layer's own bounds) -- every cached
            // `TileId` would otherwise keep meaning the *previous* active
            // layer's own document-space window.
            self.composite_cache.bump();
            self.push_accessibility();
            return;
        }

        // Grabbing the dock-rail divider takes priority over canvas
        // tools too, and is checked ahead of the `pointer_in_canvas`
        // gate below since the divider sits *outside* the canvas area
        // entirely.
        if button == PointerButton::Primary
            && pointer_on_rail_divider(&self.workspace, position)
            && let Some(start_width) =
                aurora_ui::rail_width(&self.workspace.tree, self.workspace.rail)
        {
            self.rail_resize = Some(RailResize {
                start_pointer_x: position.0,
                start_width,
            });
            return;
        }

        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };

        // A second press while a drag is still in progress -- the middle
        // button to pan mid-stroke, the right button, a stylus barrel
        // button -- ends that drag as surely as a release does, and the
        // assignment below is about to drop it. Commit it first, or the
        // pixels it already painted stay on the layer with no undo entry
        // naming them and the next Ctrl+Z reaches past them into the
        // previous stroke (`commit_ending_drag`).
        //
        // Ahead of the Zoom-tool branch below, not after it (0.57.7):
        // that branch `return`s, so it used to reach neither this commit
        // nor any other -- and it *also* moves the view
        // (`handle_zoom_tool_click` clamps the pan), out from under a
        // drag still holding a fixed document-space reference point.
        // Both halves are exactly what `press_layer_row` was written to
        // fix, one branch further down this same function, and a live
        // `Drag::Brush` really can reach here: `z` switches to the Zoom
        // tool mid-stroke without ending the stroke.
        let interrupted = self.drag.take();
        self.commit_drag(interrupted);

        if self.tool == aurora_ui::Tool::Zoom && button == PointerButton::Primary {
            handle_zoom_tool_click(
                &mut self.canvas_view,
                canvas_point,
                self.modifiers,
                active_layer_origin(&self.layers, self.active_layer),
            );
            return;
        }
        self.drag = begin_drag(
            self.tool,
            button,
            canvas_point,
            &self.canvas_view,
            active_pixel_layer(&self.layers, self.active_layer),
        );
        match self.drag.as_ref() {
            Some(Drag::Brush { last_doc, .. }) => {
                let last_doc = *last_doc;
                self.paint_dab(last_doc);
            }
            Some(Drag::Eraser { last_doc, .. }) => {
                let last_doc = *last_doc;
                self.erase_dab(last_doc);
            }
            Some(Drag::Eyedropper) => {
                let doc_point = self.canvas_view.to_document(canvas_point);
                self.sample_eyedropper(doc_point);
            }
            _ => {}
        }
    }

    /// Stamps one brush dab at `doc_point` (document space) into the
    /// active layer's own surface in the live tile store —
    /// [`aurora_brush::stamp_dab`], via [`layer_local_point`] for the
    /// document-space -> layer-local conversion `aurora_tile::TileStore`
    /// needs. A silent no-op if there's no live store
    /// ([`Self::tile_store`] failed to open), no active layer
    /// ([`Self::active_layer`] is `None`), or that layer isn't (or is no
    /// longer) a pixel layer — a real, absent precondition, not an
    /// error worth logging on its own. A real, logged failure ([`aurora_tile::TileError`],
    /// e.g. the scratch disk failing mid-session) is worth a warning,
    /// though, unlike those absent-precondition cases.
    ///
    /// The active `Drag::Brush`'s own `stroke` snapshot, if there is
    /// one ([`brush_stroke_mut`]), is handed *to*
    /// [`aurora_brush::stamp_dab`] rather than filled in beforehand —
    /// the pixel-edit half of `Self::history`'s own Undo/Redo, closed by
    /// [`Self::handle_pointer_released`] once the stroke completes. A
    /// no-op for that half specifically (still paints) when `self.drag`
    /// isn't actually a `Drag::Brush` with a real `stroke` — shouldn't
    /// happen given how this is always called, but this doesn't assume
    /// it.
    ///
    /// **Capture happens inside the dab now, not before it** (0.55.0).
    /// This used to call [`aurora_brush::touched_tiles`] first and
    /// [`aurora_brush::StrokeSnapshot::record_touch`] on every listed
    /// tile, *then* stamp. A stroke whose paint subsequently failed
    /// therefore still got a captured snapshot and a real — but
    /// useless — undo entry, covering pixels nothing had changed.
    /// `stamp_dab` now captures each tile in the instant before it first
    /// writes to that tile
    /// ([`aurora_brush::StrokeSnapshot::record_content`]), so captured
    /// and painted are the same set — and, since 0.56.0, that set
    /// excludes a tile the dab acquired but then changed nothing in, so
    /// neither an undo entry nor the invalidation loop below names a
    /// tile that still looks exactly as it did. The loop walks
    /// [`aurora_brush::DabOutcome::painted`] — what the dab really
    /// wrote — instead of what it merely aimed at.
    ///
    /// **One warning per broken tile per stroke** (0.56.0), via
    /// [`unwarned_failures`]: a permanently corrupt tile fails every dab
    /// for the rest of the drag, and 0.55.0's own one-line-per-dab
    /// collapse still left a long drag across one emitting ~100
    /// identical lines.
    fn paint_dab(&mut self, doc_point: (f32, f32)) {
        let Some(layer_id) = self.active_layer else {
            return;
        };
        let Some(aurora_doc::LayerKind::Pixel { bounds }) = self.layers.kind(layer_id).cloned()
        else {
            return;
        };
        let Some(surface) = self.layers.surface_id(layer_id) else {
            return;
        };
        let Some(store) = self.tile_store.as_mut() else {
            return;
        };
        let local = layer_local_point(bounds, doc_point);
        // Both borrows are of distinct fields of `self`, so they
        // coexist. This is why the accessor is a free function over
        // `&mut Option<Drag>` and not a `&mut self` helper method: that
        // would borrow all of `self` and conflict with `self.tile_store`
        // above.
        let snapshot = brush_stroke_mut(&mut self.drag);
        let outcome = aurora_brush::stamp_dab(
            store,
            surface,
            local,
            BRUSH_RADIUS,
            self.current_colour,
            snapshot,
        );
        for &tile in outcome.painted() {
            self.composite_cache.invalidate(tile);
        }
        // One line per broken tile per *stroke*, not per dab and not per
        // tile per dab -- see `unwarned_failures`. Every fresh failure
        // gets its own line (0.57.0): a radius-24 dab spans up to four
        // tiles, and logging only `fresh.first()` marked tiles #2..#n
        // warned while never printing their own `TileId`/`TileError` --
        // not on this dab, and never again for the rest of the stroke,
        // flatly contradicting `unwarned_failures`' own promise that
        // the first failure on each tile is always reported.
        let fresh = unwarned_failures(&mut self.drag, &outcome);
        for (tile, err) in &fresh {
            tracing::warn!(
                ?err,
                ?tile,
                first_failures = fresh.len(),
                failed = outcome.failed().len(),
                painted = outcome.painted().len(),
                "failed to stamp part of a brush dab"
            );
        }
    }

    /// Erases one dab at `doc_point` (document space) from the active
    /// layer's own surface in the live tile store — `aurora_brush::erase_dab`,
    /// [`Self::paint_dab`]'s subtractive counterpart, sharing every one
    /// of its preconditions, silent-no-op cases, partial-failure
    /// reporting, per-stroke warning dedupe, and in-dab undo-snapshot
    /// capture (against `Drag::Eraser`'s own `stroke` field instead,
    /// via [`eraser_stroke_mut`]).
    fn erase_dab(&mut self, doc_point: (f32, f32)) {
        let Some(layer_id) = self.active_layer else {
            return;
        };
        let Some(aurora_doc::LayerKind::Pixel { bounds }) = self.layers.kind(layer_id).cloned()
        else {
            return;
        };
        let Some(surface) = self.layers.surface_id(layer_id) else {
            return;
        };
        let Some(store) = self.tile_store.as_mut() else {
            return;
        };
        let local = layer_local_point(bounds, doc_point);
        // Same distinct-field borrow pair `Self::paint_dab` relies on.
        let snapshot = eraser_stroke_mut(&mut self.drag);
        let outcome = aurora_brush::erase_dab(store, surface, local, ERASER_RADIUS, snapshot);
        for &tile in outcome.painted() {
            self.composite_cache.invalidate(tile);
        }
        // One line per broken tile per stroke, every one of them --
        // `Self::paint_dab`'s own reasoning, mirrored.
        let fresh = unwarned_failures(&mut self.drag, &outcome);
        for (tile, err) in &fresh {
            tracing::warn!(
                ?err,
                ?tile,
                first_failures = fresh.len(),
                failed = outcome.failed().len(),
                painted = outcome.painted().len(),
                "failed to erase part of a dab"
            );
        }
    }

    /// Applies `bounds` to `layer_id` directly in the live document
    /// (`aurora_doc::LayerTree::set_bounds`) — the one real mutation a
    /// `Drag::Move` needs, called every pointer-move event while one is
    /// active with that drag's own live `current_bounds`
    /// ([`Self::handle_pointer_moved`]), for live visual feedback only.
    /// Deliberately bypasses `self.history`/`self.undo_order` — the
    /// whole point of coalescing a drag into one undo step
    /// ([`finish_move`]) is *not* recording an entry for every
    /// intermediate position a fast drag passes through. A real, logged
    /// failure (an unknown or non-pixel `layer_id`) shouldn't happen in
    /// practice — `layer_id` always comes from `Drag::Move` itself, set
    /// from a real active pixel layer when the drag began — but this
    /// reports rather than assumes it, the same discipline every other
    /// fallible call in this crate already applies. Bumps
    /// `self.composite_cache` unconditionally — a moved layer's own
    /// content lands at different composite tiles now, whether or not
    /// `set_bounds` itself succeeded.
    fn apply_move(&mut self, layer_id: aurora_doc::LayerId, bounds: aurora_core::Rect) {
        if let Err(err) = self.layers.set_bounds(layer_id, bounds) {
            tracing::warn!(?err, "failed to reposition the active layer");
        }
        self.composite_cache.bump();
    }

    /// Commits `drag`, whatever it turns out to be, into this
    /// application's own undo state — [`commit_ending_drag`] against
    /// `App`'s own fields. A `&mut self` wrapper so every call site can
    /// spell it `self.commit_drag(self.drag.take())`; the real logic is
    /// the free function, which needs no `App` (and therefore no GPU
    /// adapter) to test.
    fn commit_drag(&mut self, drag: Option<Drag>) {
        commit_ending_drag(
            drag,
            &self.layers,
            &mut self.history,
            &mut self.pixel_history,
            &mut self.undo_order,
            &mut self.canvas_view,
            self.active_layer,
        );
    }

    /// Samples the live, **composited** document at `doc_point` (document
    /// space) — every visible layer, in its own real blend order and
    /// opacity, exactly what's on screen — and, if the sampled texel is
    /// actually painted (alpha `> 0.0`), sets it as the new
    /// [`Self::current_colour`] — what the Eyedropper tool does on a
    /// click or while dragging. Reads [`composite_surface_id`] rather
    /// than the active layer's own surface: `App::redraw`'s own
    /// [`recomposite_visible_tiles`] call keeps that reserved surface's
    /// tiles current with the merged document every frame (both its GPU
    /// and CPU paths write there via `write_composited`), so this is the
    /// same content the user is actually looking at — a different,
    /// non-active visible layer sitting above the active one (any
    /// opacity/blend mode), or an active layer that's simply transparent
    /// at that point, used to make the old active-layer-only sample
    /// wrong. A fully transparent texel (no visible layer painted there)
    /// is treated as "nothing to pick," not a valid sample, the same way
    /// a real image editor's eyedropper has nothing meaningful to pick
    /// from empty canvas.
    ///
    /// The document-space -> composite-surface-local conversion uses
    /// [`active_layer_origin`], **not** a `None`-returns-early guard on
    /// [`Self::active_layer`]: [`recomposite_visible_tiles`]'s own
    /// `reference_origin` (the document-space point composite `TileId
    /// (0, 0)` corresponds to) is exactly the active layer's own
    /// `bounds.(x, y)`, falling back to `(0, 0)` — the document's own
    /// origin — with no active layer selected or a group active
    /// ([`active_pixel_layer`]'s own contract, which both functions
    /// share); `active_layer_origin` already implements that identical
    /// fallback. So with no active layer, `doc_point` needs no
    /// subtraction at all and this still samples the merged document
    /// correctly at its own coordinates — the more honest reading of
    /// "sample what's on screen," which doesn't stop being true just
    /// because nothing happens to be selected in the Layers panel (and
    /// matches `Drag::Eyedropper` itself, which `begin_drag` already
    /// starts unconditionally with no active-pixel-layer precondition —
    /// see that function's own doc comment). A silent no-op only if
    /// there's no live store, or `doc_point` falls outside the
    /// composited surface entirely — the same absent-precondition
    /// honesty [`Self::paint_dab`] already uses.
    ///
    /// The actual sampling is [`eyedropper_sample`], a free function
    /// taking the store, origin, and point directly rather than `&mut
    /// self` — this method only supplies those three from live `App`
    /// state, which needs a real window/GPU surface to construct at all
    /// and so can't be built directly in a unit test; `eyedropper_sample`
    /// can, and that's what this crate's own tests exercise.
    fn sample_eyedropper(&mut self, doc_point: (f32, f32)) {
        let Some(store) = self.tile_store.as_mut() else {
            return;
        };
        let origin = active_layer_origin(&self.layers, self.active_layer);
        if let Some(colour) = eyedropper_sample(store, origin, doc_point) {
            self.current_colour = colour;
        }
    }

    /// A real `WindowEvent::MouseInput { state: Released, .. }`: ends
    /// whatever drag is in progress. Any button release ends it — this
    /// crate has no multi-touch/multi-pointer support to disambiguate
    /// which button a drag actually started with, and a single active
    /// window only ever has one drag in progress at a time.
    ///
    /// Whatever the ending drag turns out to be, committing it is
    /// [`commit_ending_drag`]'s job, not this method's (0.57.0): a
    /// `Drag::Brush`/`Drag::Eraser` with a real `stroke` becomes a
    /// `Ctrl+Z`-undoable step in the unified order, a `Drag::Move` is
    /// coalesced into one structural entry, everything else is a no-op.
    /// **This is no longer the only path that ends a drag** — a second
    /// pointer press and `CursorLeft` end one too, and used to end it by
    /// silently dropping a live stroke's whole undo entry; see
    /// [`commit_ending_drag`] for the bug that shape caused and why the
    /// commit now lives in one shared place.
    ///
    /// Also ends any in-progress rail resize ([`RailResize`]) — nothing
    /// further to record for that one; `aurora_ui::set_rail_width` has
    /// already applied every intermediate width live, on each move
    /// event, not just the final one.
    fn handle_pointer_released(&mut self) {
        self.rail_resize = None;
        let ending = self.drag.take();
        self.commit_drag(ending);
    }

    /// A real `WindowEvent::MouseWheel`: zooms around the pointer's last
    /// known position ([`apply_scroll_zoom`]) if it's over the canvas
    /// area — a no-op otherwise (e.g. scrolling while the pointer is
    /// over a dock panel must not zoom the canvas).
    ///
    /// Hands the live drag, if any, straight to `apply_scroll_zoom`
    /// (0.57.7). Zooming while a stroke is held is an ordinary thing to
    /// do, so unlike the gestures that mean "I am done dragging" this
    /// one keeps the drag and re-anchors it against the moved view —
    /// see [`shift_drag_reference`] for why the pan clamp inside that
    /// zoom would otherwise paint a line the user never drew.
    fn handle_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let Some(position) = self.pointer_position else {
            return;
        };
        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };
        apply_scroll_zoom(
            &mut self.canvas_view,
            self.drag.as_mut(),
            canvas_point,
            delta,
            active_layer_origin(&self.layers, self.active_layer),
        );
    }

    /// Routes one native menu activation to [`activate_command`] — the
    /// same dispatch the command palette's `Enter` key already uses,
    /// just reached from the menu bar instead.
    #[cfg(target_os = "macos")]
    fn handle_menu_event(&mut self, event: &muda::MenuEvent) {
        let picked = activate_command(
            &mut self.workspace,
            &mut self.focus,
            event.id().as_ref(),
            &mut self.file_dialog,
        );
        match picked {
            Some(ActivatedCommand::OpenFile(path)) => self.open_file(&path),
            Some(ActivatedCommand::SaveFile(path)) => self.save_file(&path),
            Some(ActivatedCommand::Undo) => self.run_undo_redo(AppCommand::Undo),
            Some(ActivatedCommand::Redo) => self.run_undo_redo(AppCommand::Redo),
            None => {}
        }
        self.push_accessibility();
    }

    /// Logs `message`, marks this run as failed, and asks the event loop
    /// to exit — the one way an `ApplicationHandler` callback (all of
    /// which are `&mut self` with no `Result` return) can surface an
    /// unrecoverable error, matching `aurora-gpu`'s own
    /// `examples/surface_smoke.rs::report_failure`.
    fn fail(&mut self, el: &ActiveEventLoop, message: &str) {
        tracing::error!("{message}");
        self.failed = true;
        el.exit();
    }

    /// Recomputes the workspace layout for `physical_size`, then
    /// reconfigures the presentation surface *and* the canvas atlas to
    /// match — layout is pure geometry (no GPU needed) and stays current
    /// even before a window/device exist, unlike the GPU resizes below,
    /// which do need both.
    ///
    /// `physical_size` is converted to logical pixels via
    /// [`logical_size`]/`self.scale_factor` before it reaches
    /// `compute_layout`: every widget's own layout style
    /// (`aurora_theme::Scales`-derived padding/spacing) is defined in
    /// logical, DPI-independent units, so feeding it raw physical pixels
    /// would make widgets balloon to the wrong on-screen size on any
    /// display where `scale_factor != 1.0` — exactly the class of bug
    /// PLAN.md M1.8's "per-monitor DPI and fractional scaling" bullet is
    /// named for. The GPU surface and the canvas atlas both still resize
    /// to real physical sizes — a render target's (and a texture's own)
    /// pixel dimensions are never logical.
    ///
    /// `self.residency`'s own resize uses [`canvas_area_physical_size`]
    /// computed *after* `compute_layout` above, deliberately — the
    /// canvas dock area's own bounds only reflect the new window size
    /// once layout has re-run, so sizing the atlas from stale
    /// pre-resize bounds here would leave it one resize behind. A
    /// genuinely unknown canvas-area widget id (`canvas_area_physical_size`
    /// returning `None`) is handled by simply skipping the atlas resize
    /// this call — the same "never actually happens for
    /// `workspace.canvas_area` in practice, but handle it honestly"
    /// stance `resumed`'s own analogous call already takes.
    fn apply_resize(&mut self, physical_size: (u32, u32)) {
        let (width, height) = logical_size(physical_size, self.scale_factor);
        self.workspace.tree.compute_layout(width, height);

        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), self.surface.as_mut()) else {
            return;
        };
        surface.resize(gpu.device(), physical_size);

        if let Some(canvas_size) = canvas_area_physical_size(&self.workspace, self.scale_factor) {
            // The atlas's own zoom floor moves with the canvas size, and
            // a pointer event can arrive before the next frame -- see
            // `redraw`'s own call for the full reasoning. Through
            // `apply_canvas_min_zoom` because raising the floor moves
            // the view, and a resize arriving mid-stroke must not move
            // it out from under the live drag (0.57.8).
            let pointer = self
                .pointer_position
                .and_then(|position| pointer_in_canvas(&self.workspace, position));
            apply_canvas_min_zoom(
                &mut self.canvas_view,
                self.drag.as_mut(),
                pointer,
                canvas_min_zoom(canvas_size, self.scale_factor),
            );
            if let Some(residency) = self.residency.as_mut() {
                residency.resize(gpu.device(), gpu.queue(), canvas_size);
            }
        }
    }

    /// Clears the surface to the real theme background colour, then —
    /// if a live document, tile store, and GPU atlas all exist —
    /// recomposites every visible pixel layer
    /// ([`recomposite_visible_tiles`]) and syncs the atlas from the
    /// result and draws it within the canvas dock area's own viewport,
    /// then draws every widget's own paint
    /// ([`collect_widget_paints`]/[`draw_widget_paints`]) on top —
    /// canvas and UI in the same pass, the same frame, invariant §7.3.8
    /// (they never become separate surfaces composited together).
    ///
    /// **Straight alpha, all the way to the shader**: what
    /// [`recomposite_visible_tiles`] leaves in the composite surface,
    /// what `TileResidency::sync` uploads to the atlas (a plain `f16`
    /// texel copy, no alpha conversion of its own), and what
    /// `aurora-gpu`'s own `fs_canvas` samples are all straight-alpha
    /// texels — `fs_canvas` is the single place in the codebase that
    /// converts, multiplying by alpha as it blends the canvas over its
    /// checkerboard. Before 0.52.0 that was not true and two bugs
    /// cancelled here: the compositing entry points left premultiplied
    /// texels behind and `fs_canvas` used the matching premultiplied
    /// "over" formula, so the screen looked approximately right while
    /// every export and every eyedropper read carried the wrong colour.
    /// Both halves were fixed together; see
    /// `composite_roots_into_tile`'s own doc comment and that shader's
    /// own comment.
    // One linear per-frame flow (build widget paints, sync the canvas
    // atlas, one shared render pass drawing both) -- splitting further
    // would just relocate lines across more functions without reducing
    // the real complexity a GPU frame has, the same call
    // `render_test.rs::render_and_sample_pixel` already makes for an
    // analogous reason.
    #[allow(clippy::too_many_lines)]
    fn redraw(&mut self) {
        // Before anything reads `canvas_view`: hold its zoom to the
        // floor the atlas can actually render at this canvas size
        // (`canvas_min_zoom`). This is the one place guaranteed to run
        // whenever what is on screen can have changed -- a window
        // resize, a scale-factor change, or a dock layout that resized
        // the canvas area without either -- so the view can never be
        // holding a zoom the frame below is about to not honour, which
        // is what made `to_document` (pointer -> document, i.e. where a
        // brush dab lands) disagree with what was drawn. `apply_resize`
        // does it too, so a pointer event arriving between a resize and
        // the next frame is bounded as well. Through
        // `apply_canvas_min_zoom`, because raising the floor moves the
        // view: a live drag has to be re-anchored against it, not left
        // measuring against a view it knows nothing about (0.57.8).
        if let Some(canvas_size) = canvas_area_physical_size(&self.workspace, self.scale_factor) {
            let pointer = self
                .pointer_position
                .and_then(|position| pointer_in_canvas(&self.workspace, position));
            apply_canvas_min_zoom(
                &mut self.canvas_view,
                self.drag.as_mut(),
                pointer,
                canvas_min_zoom(canvas_size, self.scale_factor),
            );
        }
        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), self.surface.as_mut()) else {
            return;
        };
        match surface.acquire() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                let view = texture
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    gpu.device()
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("aurora-app-frame"),
                        });

                // Built before the render pass below, not inside it --
                // see `collect_widget_paints`'s own doc comment for why.
                let widget_paints = collect_widget_paints(
                    &self.workspace.tree,
                    &self.theme,
                    &self.scales,
                    gpu,
                    self.scale_factor,
                );

                // Sync before drawing, so this frame shows the latest
                // painted pixels rather than lagging one frame behind.
                if let Some(residency) = self.residency.as_mut() {
                    if let Some(canvas_size) =
                        canvas_area_physical_size(&self.workspace, self.scale_factor)
                    {
                        residency.set_origin(
                            gpu.queue(),
                            canvas_local_origin(
                                &self.canvas_view,
                                active_layer_origin(&self.layers, self.active_layer),
                            ),
                            canvas_size,
                            effective_residency_zoom(self.canvas_view.zoom(), self.scale_factor),
                        );
                    }
                    if let Some(store) = self.tile_store.as_mut() {
                        recomposite_visible_tiles(
                            residency,
                            &self.layers,
                            self.active_layer,
                            store,
                            &mut self.composite_cache,
                            Some(gpu),
                            self.compositor.as_mut(),
                        );
                        let _ = residency.sync(
                            gpu.queue(),
                            store,
                            composite_surface_id(),
                            false,
                            usize::MAX,
                        );
                    }
                }

                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("aurora-app-frame"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(self.background),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });

                    let viewport = canvas_area_physical_rect(&self.workspace, self.scale_factor);
                    if let (Some(residency), Some(canvas_pipeline), Some((x, y, w, h))) = (
                        self.residency.as_ref(),
                        self.canvas_pipeline.as_mut(),
                        viewport,
                    ) {
                        let bind_group = canvas_pipeline.bind_group(gpu.device(), residency);
                        let pipeline = canvas_pipeline.pipeline(gpu.device(), surface.format());
                        pass.set_viewport(x, y, w, h, 0.0, 1.0);
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &bind_group, &[]);
                        pass.draw(0..3, 0..1);
                    }

                    if !widget_paints.is_empty()
                        && let Some(path_pipeline) = self.path_pipeline.as_mut()
                    {
                        draw_widget_paints(
                            &mut pass,
                            path_pipeline,
                            gpu,
                            surface.format(),
                            surface.size(),
                            self.scale_factor,
                            &widget_paints,
                        );
                    }
                }
                gpu.queue().submit(std::iter::once(encoder.finish()));
                gpu.queue().present(texture);
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {}
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                if let Some(window) = self.window.as_ref() {
                    let size = window.inner_size();
                    surface.resize(gpu.device(), (size.width, size.height));
                }
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::error!("surface texture acquisition raised a validation error");
            }
        }
    }
}

/// Resolves every widget in `tree` into a real, uploaded `GpuMesh`
/// (`aurora_widgets::paint_widget`, in [`WidgetTree::paint_order`]'s own
/// order — root first, each child subtree before the next sibling's
/// own, so a later entry in the returned `Vec` draws on top of an
/// earlier one, the same "last-painted child is topmost" convention
/// `WidgetTree::hit_test` already assumes for the reverse (pointer-hit)
/// direction).
///
/// Called *before* [`App::redraw`]'s own render pass begins, not from
/// inside it: [`PathPipeline::draw`] needs `mesh: &'pass GpuMesh`, so
/// every `GpuMesh` it draws must outlive the pass — one uploaded fresh
/// inside the pass's own draw loop would be dropped at the end of that
/// iteration, before the pass (borrowed for `'pass`) is done with it.
/// Building the whole list first, then only borrowing from it inside
/// the pass ([`draw_widget_paints`]), is the shape that forces.
///
/// A widget whose own paint fails to tessellate (`WidgetError::Paint`)
/// is logged and skipped, not fatal to the frame — one broken widget's
/// own geometry shouldn't blank the rest of a real user's UI. Colour is
/// linearized ([`linearize_paint_color`]) here, once, rather than by
/// [`draw_widget_paints`] on every draw call.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn collect_widget_paints(
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    gpu: &GpuContext,
    scale_factor: f64,
) -> Vec<(GpuMesh, [f32; 4])> {
    let scale_factor = scale_factor as f32;
    let mut widget_paints = Vec::new();
    for id in tree.paint_order() {
        match paint_widget(tree, id, theme, scales, scale_factor) {
            Ok(paints) => {
                for (mesh, color) in paints {
                    let gpu_mesh = GpuMesh::upload(gpu.device(), gpu.queue(), &mesh);
                    widget_paints.push((gpu_mesh, linearize_paint_color(color)));
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    ?id,
                    "failed to paint a widget; skipping it this frame"
                );
            }
        }
    }
    widget_paints
}

/// Draws `widget_paints` ([`collect_widget_paints`]) within `pass`,
/// which must already be mid-render-pass (this only sets the pipeline,
/// resets the viewport, and issues the draw calls — it doesn't begin or
/// end the pass itself, the same division [`App::redraw`]'s own canvas
/// draw block already keeps).
///
/// Resets the viewport to the *whole* render target first — widget
/// chrome (the side rail, an open dialog) isn't confined to the canvas
/// area's own restricted viewport the canvas draw sets before this
/// runs. `viewport_size` (passed to every [`PathPipeline::bind_group`]
/// call) is `physical_size` converted to *logical* pixels
/// ([`logical_size`]): `PathPipeline`'s own `vs_path.wgsl` expects mesh
/// vertex positions and `viewport_size` in the same unit, and
/// `paint_widget`'s own mesh comes from `WidgetTree::bounds`, which
/// `compute_layout` (`App::resumed`/the resize handler) always runs
/// with logical, not physical, size — a fraction of the window is the
/// same fraction regardless of which pixel unit measures it, so this is
/// correct at any DPI scale, not just `1.0`.
fn draw_widget_paints<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    path_pipeline: &mut PathPipeline,
    gpu: &GpuContext,
    format: wgpu::TextureFormat,
    physical_size: (u32, u32),
    scale_factor: f64,
    widget_paints: &'pass [(GpuMesh, [f32; 4])],
) {
    let (physical_width, physical_height) = physical_size;
    #[allow(clippy::cast_precision_loss)]
    pass.set_viewport(
        0.0,
        0.0,
        physical_width as f32,
        physical_height as f32,
        0.0,
        1.0,
    );
    let viewport_size = logical_size(physical_size, scale_factor);
    let pipeline = path_pipeline.pipeline(gpu.device(), format);
    pass.set_pipeline(pipeline);
    for (mesh, color) in widget_paints {
        let bind_group = path_pipeline.bind_group(gpu.device(), gpu.queue(), viewport_size, *color);
        pass.set_bind_group(0, &bind_group, &[]);
        path_pipeline.draw(pass, mesh);
    }
}

/// How often [`App::about_to_wait`] re-checks muda's own menu-event
/// channel on macOS — see that method's own doc comment. Short enough
/// that a menu click feels instant to a human (who just consciously
/// clicked something), long enough to spend negligible CPU polling an
/// empty channel the rest of the time.
#[cfg(target_os = "macos")]
const MUDA_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

impl ApplicationHandler<accesskit_winit::Event> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Created hidden on purpose: `accesskit_winit::Adapter` panics
        // if constructed after the window is first shown
        // (`spike/a11y-ime/FINDINGS.md` finding #1) — this is that
        // ordering, as real production code, not a spike anymore.
        let attrs = Window::default_attributes()
            .with_title("Aurora")
            .with_visible(false)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = match el.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.fail(el, &format!("window creation failed: {err}"));
                return;
            }
        };

        let gpu = match GpuContext::new() {
            Ok(gpu) => gpu,
            Err(err) => {
                self.fail(el, &format!("GpuContext::new failed: {err}"));
                return;
            }
        };
        let size = window.inner_size();
        let surface =
            match gpu.create_surface(window.clone(), (size.width.max(1), size.height.max(1))) {
                Ok(surface) => surface,
                Err(err) => {
                    self.fail(el, &format!("create_surface failed: {err}"));
                    return;
                }
            };

        let adapter =
            accesskit_winit::Adapter::with_event_loop_proxy(el, &window, self.proxy.clone());
        window.set_visible(true);

        // The native application menu bar -- macOS only, see this
        // crate's own "native menu bar" section. No ordering constraint
        // like the accessibility adapter's own create-hidden dance;
        // this can happen any time before the app is fully active.
        #[cfg(target_os = "macos")]
        self.menu.init_for_nsapp();

        self.scale_factor = window.scale_factor();
        let (width, height) = logical_size((size.width, size.height), self.scale_factor);
        self.workspace.tree.compute_layout(width, height);

        // Sized once, here, to the canvas area's own physical size --
        // see `residency`/`canvas_pipeline`'s own doc comments for why a
        // later window resize isn't reflected.
        if let Some(canvas_size) = canvas_area_physical_size(&self.workspace, self.scale_factor) {
            self.residency = Some(aurora_gpu::TileResidency::new(
                gpu.device(),
                gpu.queue(),
                canvas_size,
            ));
            self.canvas_pipeline = Some(aurora_gpu::CanvasPipeline::new(gpu.device()));
            self.compositor = Some(aurora_render::TileCompositor::new(gpu.device()));
        } else {
            tracing::warn!(
                "canvas area has no computed layout yet; canvas rendering disabled this session"
            );
        }
        // Unlike `residency`/`canvas_pipeline`, needs nothing but a real
        // device -- no computed canvas area to size itself to -- so it's
        // never skipped here.
        self.path_pipeline = Some(PathPipeline::new(gpu.device()));

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.surface = Some(surface);
        self.adapter = Some(adapter);
        self.push_accessibility();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: accesskit_winit::Event) {
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                // `Adapter::with_event_loop_proxy` can't synchronously
                // return an initial tree (see its own doc comment); this
                // is the deferred push it expects in response.
                self.push_accessibility();
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                // No interactive widgets exist yet to route this to —
                // real input/focus wiring is separate, still-open M1.8
                // work. Logged, not dropped silently.
                tracing::debug!(action = ?request.action, "accessibility action requested (not yet routed to a widget)");
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                tracing::debug!("accessibility deactivated");
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(adapter), Some(window)) = (self.adapter.as_mut(), self.window.as_ref()) {
            adapter.process_event(window, &event);
        }

        // Every real event other than `RedrawRequested` itself might
        // change what the next frame should show (a moved cursor, a
        // resize, a keystroke, ...) — coarse (any event, not just ones
        // that actually touched pixels) but correct and cheap, and
        // exactly what keeps `about_to_wait` from ever needing to guess.
        // `RedrawRequested` is excluded so handling one doesn't
        // immediately ask for another — see `needs_redraw`'s own doc
        // comment for why that distinction is the whole fix.
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.needs_redraw = true;
        }

        match event {
            WindowEvent::CloseRequested => {
                // A clean shutdown -- clear this run's own marker so the
                // *next* run's `previous_session_left_a_marker` reads
                // false, not true (see this crate's own "crash
                // recovery" section), and save the current dock layout
                // so the *next* run's own `App::new` can restore it
                // (see the "persisted workspace layout" section's own
                // doc comment for why this is the one point this crate
                // writes it).
                // All three cleanups -- the marker, the autosave
                // (nothing is left to recover once this run has ended
                // cleanly, and the file holds real pixel content at a
                // predictable path in a shared temp directory; see
                // [`remove_autosave`]'s own doc comment), and this
                // session's paged-out tiles, which are its unsaved
                // pixels and which nothing recovers after a clean exit
                // -- go through one function so a test can execute them
                // as a unit; this arm itself needs a real event loop
                // to reach. `self` is the only argument on purpose --
                // an earlier four-argument shape let this call site
                // silently pass `None` for the store and the scratch
                // directory and still pass the whole gate.
                clean_shutdown_cleanup(self);
                if let Some(layout_path) = self.layout_path.as_deref() {
                    save_workspace_layout(layout_path, &self.workspace);
                }
                el.exit();
            }
            WindowEvent::Resized(size) => self.apply_resize((size.width, size.height)),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = translate_modifiers(modifiers.state());
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(&event),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // e.g. the window moved to a monitor with a different
                // DPI scale -- the physical size winit reports for the
                // *same* window may not have changed at all, but the
                // logical size `apply_resize` computes from it has, so
                // this always re-applies even when `inner_size` itself
                // is unchanged.
                self.scale_factor = scale_factor;
                if let Some(window) = self.window.as_ref() {
                    let size = window.inner_size();
                    self.apply_resize((size.width, size.height));
                }
            }
            WindowEvent::DroppedFile(path) => self.handle_dropped_file(&path),
            // No drop-target visual affordance exists yet (nothing
            // renders a pixel in this crate regardless of drag state),
            // so a hover just gets a debug-level trace -- there's
            // nothing else to react to it with yet.
            WindowEvent::HoveredFile(path) => {
                tracing::debug!(path = %path.display(), "file hovered over the window");
            }
            WindowEvent::HoveredFileCancelled => {
                tracing::debug!("file drag cancelled");
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_pointer_moved((position.x, position.y));
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => self.handle_pointer_pressed(button),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } => self.handle_pointer_released(),
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            WindowEvent::CursorLeft { .. } => {
                self.pointer_position = None;
                // Dragging off the window edge ends the drag, and used
                // to end it by simply dropping it -- losing a live
                // stroke's whole undo entry along with it. Commit it the
                // same way a release would (`commit_ending_drag`).
                let interrupted = self.drag.take();
                self.commit_drag(interrupted);
            }
            _ => {}
        }
    }

    // `el` is only used to re-arm the poll timer on macOS (below) --
    // unused on every other platform, which stays on the plain `Wait`
    // set once in `run`.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        // muda's own events arrive on a plain channel, not through this
        // crate's `accesskit_winit::Event` user-event type (the two
        // don't share one enum -- restructuring the accessibility
        // integration around a combined event type is a bigger, separate
        // change) -- polled here since `about_to_wait` already runs on
        // every loop iteration. Real `winit` `WindowEvent`s never arrive
        // for a native menu action, so `needs_redraw` alone can't catch
        // one -- set explicitly here instead, for the same reason
        // `window_event` sets it for every other real input.
        #[cfg(target_os = "macos")]
        {
            // A real, found-not-assumed bug: a menu action (e.g. "Toggle
            // Layers Panel") changes a panel's own layout *style*
            // (`aurora_ui::set_panel_collapsed`/`close_panel`), but
            // nothing re-runs `WidgetTree::compute_layout` afterward on
            // this path — unlike `App::handle_key_event`, which already
            // does this unconditionally after every key event for
            // exactly this reason (see that method's own doc comment).
            // Without it, the new style never reaches `WidgetTree::
            // bounds`, so the menu action visibly does nothing at all
            // until some *other* event (a real window resize) happens
            // to force a relayout.
            let mut menu_event_handled = false;
            while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
                self.handle_menu_event(&event);
                self.needs_redraw = true;
                menu_event_handled = true;
            }
            if menu_event_handled && let Some(window) = self.window.as_ref() {
                let size = window.inner_size();
                self.apply_resize((size.width, size.height));
            }
            // `ControlFlow::Wait` (set once in `run`) would otherwise
            // block indefinitely, and muda's channel has no event-loop
            // wakeup of its own to interrupt that wait -- so macOS
            // alone re-polls on a short timer instead, the cost of
            // catching a menu click promptly without the unconditional
            // per-frame `request_redraw` this whole mechanism used to
            // rely on (and which pegged a full CPU core doing it, since
            // it never let the loop go idle at all -- see
            // `needs_redraw`'s own doc comment). Non-macOS has no
            // channel to poll and stays on plain, fully blocking `Wait`.
            el.set_control_flow(ControlFlow::WaitUntil(
                std::time::Instant::now() + MUDA_POLL_INTERVAL,
            ));
        }

        if self.needs_redraw
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
            self.needs_redraw = false;
        }
    }
}

impl std::fmt::Debug for App {
    // Manual, summary-only impl: `accesskit_winit::Adapter` doesn't
    // itself implement `Debug`, the same reason `aurora_widgets::WidgetTree`
    // already writes its own rather than deriving one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("window", &self.window.is_some())
            .field("gpu", &self.gpu.is_some())
            .field("surface", &self.surface.is_some())
            .field("adapter", &self.adapter.is_some())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

/// Runs the application to completion. An ordinary window close is
/// `Ok(())`; a failure creating the event loop, or an unrecoverable
/// error during a `resumed` step (window/device/surface creation), is
/// an error.
///
/// # Errors
///
/// Returns an error if the built-in theme fails to load, if the event
/// loop can't be created or fails while running, or if the app
/// recorded an unrecoverable error during the run (e.g. window, GPU
/// device, or surface creation failing).
pub fn run() -> anyhow::Result<()> {
    let theme = load_theme()?;
    let background = background_color_from_theme(&theme);
    // Order doesn't matter between these two: `with_accessibility_preferences`
    // only ever touches `motion`/`typography.size`, `with_density` only
    // ever touches `spacing` (see each method's own doc comment) -- disjoint
    // fields, so the two are commutative. `Density::Comfortable` is a
    // hardcoded default here, the same honest starting point
    // `load_theme()` above already uses for the Dark theme -- a real
    // density preference (settings UI, persisted choice) is separate,
    // later work, not a regression introduced here.
    let scales = load_scales()?
        .with_accessibility_preferences(detect_accessibility_preferences())
        .with_density(aurora_theme::Density::Comfortable);
    let marker_path = marker_path();
    // Checked *before* writing this run's own marker below -- otherwise
    // every run would see its own, brand-new marker and think the
    // *previous* run crashed.
    let had_previous_marker = previous_session_left_a_marker(&marker_path);
    write_session_marker(&marker_path);
    let autosave_path = autosave_path();
    let layout_path = layout_path();

    let event_loop = EventLoop::<accesskit_winit::Event>::with_user_event()
        .build()
        .map_err(|err| anyhow::anyhow!("event loop creation failed: {err}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let mut app = App::new(
        proxy,
        theme,
        background,
        scales,
        marker_path,
        had_previous_marker,
        &autosave_path,
        layout_path,
    );
    event_loop
        .run_app(&mut app)
        .map_err(|err| anyhow::anyhow!("event loop run failed: {err}"))?;

    anyhow::ensure!(
        !app.failed(),
        "application exited due to an earlier unrecoverable error"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ActivatedCommand, AppCommand, BRUSH_RADIUS, COMMAND_CLOSE_HISTORY, COMMAND_CLOSE_LAYERS,
        COMMAND_CLOSE_PROPERTIES, COMMAND_FILE_OPEN, COMMAND_FILE_SAVE, COMMAND_FOCUS_HISTORY,
        COMMAND_FOCUS_LAYERS, COMMAND_FOCUS_PROPERTIES, COMMAND_REDO, COMMAND_TOGGLE_HISTORY,
        COMMAND_TOGGLE_LAYERS, COMMAND_TOGGLE_PROPERTIES, COMMAND_UNDO, CRASH_RECOVERY_CONTINUE,
        ClipboardAccess, CompositeBudget, CompositeCache, Drag, ERASER_RADIUS, FileDialogAccess,
        Key, KeyChord, Modifiers, NamedKey, PointerButton, RAIL_DIVIDER_HIT_TOLERANCE, RailResize,
        ShutdownState, UndoKind, UndoOrder, activate_command, active_layer_origin, after_undo_redo,
        apply_canvas_min_zoom, apply_mask_clip, apply_scroll_zoom, aur_verify_scratch_dir,
        autosave_path, background_color_from_theme, begin_drag, brush_stroke_mut,
        canvas_area_physical_rect, canvas_area_physical_size, canvas_local_origin, canvas_min_zoom,
        clamp_pan_to_active_layer, clean_shutdown_cleanup, clear_session_marker,
        close_command_palette, close_crash_recovery_dialog, collect_widget_paints,
        commit_ending_drag, composite_document, composite_surface_id, continue_drag,
        crash_recovery_dialog_message, create_tile_store_scratch_dir, default_shortcuts,
        demo_document, dissolve_gate, document_canvas_size, document_from_image,
        document_qualifies_for_gpu_compositing, effective_residency_zoom, eraser_stroke_mut,
        eyedropper_sample, guarded_scale_factor, handle_dialog_key, handle_dialog_pointer,
        handle_key, handle_palette_key, handle_zoom_tool_click, hash_position, hash_to_unit_f32,
        is_aur_path, layer_local_point, load_document_view, load_scales, load_theme, logical_point,
        logical_size, open_command_palette, open_crash_recovery_dialog, open_image,
        open_tile_store, palette_commands, partial_autosave_path, perform_undo_redo,
        pointer_in_canvas, pointer_on_rail_divider, press_layer_row,
        previous_session_left_a_marker, recomposite_visible_tiles, recover_document,
        replace_document, reset_canvas_view, resized_rail_width, resolve_tile, run_command,
        sample_pixel, select_layer, shift_bounds, splitmix64, tile_store_scratch_dir,
        toggle_command_palette, topmost_pixel_layer, translate_key, translate_modifiers,
        translate_pointer_button, unwarned_failures, verify_aur, write_autosave,
        write_session_marker, write_verified, zoom_steps_for_scroll,
    };
    use aurora_doc::SelectionSet;
    use aurora_ui::{CanvasView, Tool};
    use aurora_widgets::widgets::{insert_button, new_tree};
    use aurora_widgets::{FocusManager, WidgetId};
    use std::path::PathBuf;

    /// [`ClipboardAccess`]'s test double — a plain in-memory slot, no
    /// real OS clipboard involved (this sandbox has no display server
    /// for a real one to attach to anyway).
    #[derive(Debug, Default)]
    struct FakeClipboard {
        contents: Option<String>,
    }

    impl ClipboardAccess for FakeClipboard {
        fn get_text(&mut self) -> Option<String> {
            self.contents.clone()
        }

        fn set_text(&mut self, text: String) {
            self.contents = Some(text);
        }
    }

    /// [`FileDialogAccess`]'s test double — returns a canned path (or
    /// none, simulating a cancelled dialog) instead of showing a real
    /// native picker. `next_pick`/`next_save` are separate slots since a
    /// real "Open File…"/"Save As…" activation would show two different
    /// dialogs, never the same one.
    #[derive(Debug, Default)]
    struct FakeFileDialog {
        next_pick: Option<PathBuf>,
        next_save: Option<PathBuf>,
    }

    impl FileDialogAccess for FakeFileDialog {
        fn pick_file(&mut self) -> Option<PathBuf> {
            self.next_pick.take()
        }

        fn save_file(&mut self) -> Option<PathBuf> {
            self.next_save.take()
        }
    }

    /// The workspace structure itself (`aurora_ui::build_workspace`) has
    /// its own, thorough tests in `aurora-ui` — nothing app-specific to
    /// add here beyond wiring it in. The headlessly-answerable piece
    /// that *is* specific to this crate: loading the real Dark
    /// theme and converting its `surface.app` token to a clear colour
    /// doesn't need a window either. Checks real, known values from
    /// `design/themes/dark.toml`/`design/tokens/palette.toml` — not
    /// just "it parses" — and specifically that the result is *not*
    /// the token's own sRGB-encoded value (the double-encoding bug this
    /// function's own doc comment names: using sRGB bytes directly as a
    /// linear clear colour would wash the colour out).
    #[test]
    fn load_background_color_resolves_a_real_linear_token() {
        let theme = match load_theme() {
            Ok(theme) => theme,
            Err(err) => unreachable!("the checked-in design files must parse: {err}"),
        };
        let color = background_color_from_theme(&theme);
        // Exact-literal comparison, not accumulated computation noise --
        // `background_color_from_theme` sets `a: 1.0` directly, never
        // through float math -- same reasoning `aurora-color`'s own
        // tests already document for their float_cmp allows.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(color.a, 1.0, "the app background is always opaque");
        }
        assert!(
            (0.0..1.0).contains(&color.r)
                && (0.0..1.0).contains(&color.g)
                && (0.0..1.0).contains(&color.b),
            "expected in-range linear channel values, got {color:?}"
        );
        // `surface.app` resolves to `neutral.50` = `#1a1a1b` -- already
        // dark in sRGB (~0.10), and srgb_to_linear always maps any
        // value in (0, 1) to something smaller (the sRGB curve
        // perceptually brightens midtones on encode, so decode does the
        // reverse) -- so the linear result must land well under 0.5,
        // not merely under the sRGB value itself.
        assert!(
            color.r < 0.5 && color.g < 0.5 && color.b < 0.5,
            "expected a dark background from the Dark theme, got {color:?}"
        );
    }

    /// The other headlessly-answerable, app-specific piece: `demo_document`
    /// needs no window either. Checks the real top-to-bottom order
    /// `add_pixel_layer`'s "new topmost root" rule produces (Retouch,
    /// then Color balance, then Background — insertion order reversed),
    /// not just a layer count.
    #[test]
    fn demo_document_puts_retouch_on_top_with_color_balance_multiplied() {
        let (layers, _history) = demo_document();
        let roots = layers.roots();
        assert_eq!(roots.len(), 3);
        let names: Vec<&str> = roots
            .iter()
            .map(|&id| match layers.name(id) {
                Some(name) => name,
                None => unreachable!("every root id in this tree must resolve to a name"),
            })
            .collect();
        assert_eq!(names, vec!["Retouch — skin", "Color balance", "Background"]);

        let Some(&color_balance) = roots.get(1) else {
            unreachable!("just asserted 3 roots");
        };
        assert_eq!(
            layers.blend_mode(color_balance),
            Some(aurora_doc::BlendMode::Multiply)
        );
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(layers.opacity(color_balance), Some(0.8));
        }
    }

    #[test]
    fn topmost_pixel_layer_of_the_demo_document_is_the_topmost_root() {
        let (layers, _history) = demo_document();
        let Some(&expected) = layers.roots().first() else {
            unreachable!("demo_document always has at least one root");
        };
        assert_eq!(topmost_pixel_layer(&layers), Some(expected));
    }

    #[test]
    fn topmost_pixel_layer_is_none_for_an_empty_tree() {
        let layers = aurora_doc::LayerTree::new();
        assert_eq!(topmost_pixel_layer(&layers), None);
    }

    #[test]
    fn topmost_pixel_layer_skips_a_topmost_group_and_finds_the_pixel_layer_beneath() {
        let mut layers = aurora_doc::LayerTree::new();
        let pixel = match layers.add_pixel_layer(
            "background",
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Added after the pixel layer, so it's the new topmost root --
        // `topmost_pixel_layer` must skip it and still find the pixel
        // layer underneath, not just check `roots()[0]`.
        if let Err(err) = layers.add_group("a group on top", None) {
            unreachable!("{err:?}");
        }
        assert_eq!(topmost_pixel_layer(&layers), Some(pixel));
    }

    fn layer_bounds() -> aurora_core::Rect {
        aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn select_layer_sets_active_layer_and_marks_only_its_own_row_selected() {
        let mut workspace = aurora_ui::build_workspace();
        let mut layers = aurora_doc::LayerTree::new();
        let a = match layers.add_pixel_layer("a", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match layers.add_pixel_layer("b", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        let layer_rows = match aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            &layers,
        ) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some((&row_a, _)) = layer_rows.iter().find(|&(_, &id)| id == a) else {
            unreachable!("a must have a row");
        };
        let Some((&row_b, _)) = layer_rows.iter().find(|&(_, &id)| id == b) else {
            unreachable!("b must have a row");
        };
        let mut active_layer = None;

        let mut view = CanvasView::new();
        select_layer(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            a,
        );
        assert_eq!(active_layer, Some(a));
        let Some(node_a) = workspace.tree.accessibility(row_a) else {
            unreachable!("just populated");
        };
        assert_eq!(node_a.is_selected(), Some(true));
        let Some(node_b) = workspace.tree.accessibility(row_b) else {
            unreachable!("just populated");
        };
        assert_eq!(node_b.is_selected(), Some(false));

        // Selecting the other layer must flip both rows, not just add
        // to whatever was already selected.
        select_layer(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            b,
        );
        assert_eq!(active_layer, Some(b));
        let Some(node_a) = workspace.tree.accessibility(row_a) else {
            unreachable!("just populated");
        };
        assert_eq!(node_a.is_selected(), Some(false));
        let Some(node_b) = workspace.tree.accessibility(row_b) else {
            unreachable!("just populated");
        };
        assert_eq!(node_b.is_selected(), Some(true));
    }

    // -- the pan bound and a *changing* active layer --
    //
    // `CanvasView::clamp_pan_to_minimum` bounds the view against the
    // active layer's own origin, and every pan-moving gesture calls it.
    // These cover the other half: the boundary itself moving, because
    // the active layer changed. See `clamp_pan_to_active_layer`.

    /// A layer deliberately away from the document origin — the shape a
    /// `.aur` file saved after a Move actually round-trips (`Rect`'s own
    /// `x`/`y` are serialized, and `App::apply_move` never bakes the
    /// offset into pixels).
    fn moved_layer_bounds() -> aurora_core::Rect {
        aurora_core::Rect {
            x: 300,
            y: 150,
            width: 10,
            height: 10,
        }
    }

    /// A `LayerTree` with `a` at the document origin and `b` moved to
    /// `(300, 150)`, plus the populated Layers-panel rows `select_layer`
    /// needs.
    fn two_layers_one_moved() -> (
        aurora_ui::Workspace,
        aurora_doc::LayerTree,
        std::collections::HashMap<WidgetId, aurora_doc::LayerId>,
        aurora_doc::LayerId,
        aurora_doc::LayerId,
    ) {
        let mut workspace = aurora_ui::build_workspace();
        let mut layers = aurora_doc::LayerTree::new();
        let a = match layers.add_pixel_layer("a", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let b = match layers.add_pixel_layer("b", moved_layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        let layer_rows = match aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            &layers,
        ) {
            Ok(rows) => rows,
            Err(err) => unreachable!("{err:?}"),
        };
        (workspace, layers, layer_rows, a, b)
    }

    #[test]
    fn changing_the_active_layer_re_establishes_the_pan_bound() {
        let (mut workspace, layers, layer_rows, a, b) = two_layers_one_moved();
        let mut active_layer = Some(a);
        // Panned right/down past `a`'s own boundary and clamped back to
        // it — the state any real session is in after a hand-tool drag
        // toward the top-left of the document.
        let mut view = CanvasView::new();
        view.pan_by((40.0, 40.0));
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);

        select_layer(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            b,
        );

        assert_eq!(active_layer, Some(b));
        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(
            local_x >= -1e-3,
            "the surface-local origin must not go negative on x: {local_x}"
        );
        assert!(
            local_y >= -1e-3,
            "the surface-local origin must not go negative on y: {local_y}"
        );
        // Equivalently, in document space: the canvas area's own
        // top-left corner cannot show anything before `b`'s own corner.
        let (doc_x, doc_y) = view.to_document((0.0, 0.0));
        assert!(doc_x >= 300.0 - 1e-3, "{doc_x}");
        assert!(doc_y >= 150.0 - 1e-3, "{doc_y}");
    }

    /// The negative control for the test above: the same active-layer
    /// change with only the *old* layer's bound ever applied — which is
    /// exactly what this crate did before `clamp_pan_to_active_layer`
    /// existed. Asserts the divergence is large, not marginal, so this
    /// cannot pass by a coincidence of small numbers.
    #[test]
    fn an_active_layer_change_without_the_re_clamp_is_the_divergence_this_prevents() {
        let (_workspace, layers, _layer_rows, a, b) = two_layers_one_moved();
        let mut view = CanvasView::new();
        view.pan_by((40.0, 40.0));
        // Clamped against `a` only — the boundary as it was *before* the
        // active layer changed.
        clamp_pan_to_active_layer(&mut view, &layers, Some(a));

        // The active layer becomes `b`, and nothing re-clamps.
        let (local_x, local_y) = canvas_local_origin(&view, active_layer_origin(&layers, Some(b)));
        assert!(
            local_x < -100.0,
            "without the re-clamp the local origin is far negative on x: {local_x}"
        );
        assert!(
            local_y < -50.0,
            "without the re-clamp the local origin is far negative on y: {local_y}"
        );
    }

    /// The evidence that the *open* paths are genuinely exposed, not
    /// hypothetically: a `.aur` container written from a document whose
    /// topmost layer sits at `(300, 150)` reopens with that origin
    /// intact. `App::open_aur_file` and `App::new`'s own crash-recovery
    /// branch both go through this reader, and `reset_canvas_view`
    /// zeroes the pan — so without the clamp those documents start with
    /// a negative surface-local origin on their very first frame, with
    /// no panning needed to trigger it.
    #[test]
    fn an_aur_round_trip_preserves_a_moved_layers_origin_so_the_open_paths_need_the_clamp() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_scratch, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let base = match history.add_pixel_layer(&mut layers, "base", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let _painted = paint_one_texel(&mut store, &layers, base);
        // Added last, so it is the topmost root — what
        // `topmost_pixel_layer` (and therefore `active_layer`) picks.
        let moved = match history.add_pixel_layer(&mut layers, "moved", moved_layer_bounds(), None)
        {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let _painted = paint_one_texel(&mut store, &layers, moved);

        let path = dir.path().join("aurora-autosave.aur");
        write_autosave(&path, &layers, &history, (320, 160), &mut store);

        let (_fresh_dir, mut fresh_store) = real_tile_store();
        let Some((recovered, _history, _canvas)) = recover_document(&path, &mut fresh_store) else {
            unreachable!("the autosave just written must reopen");
        };
        let Some(active) = topmost_pixel_layer(&recovered) else {
            unreachable!("the recovered document has pixel layers");
        };
        match recovered.kind(active) {
            Some(aurora_doc::LayerKind::Pixel { bounds }) => {
                assert_eq!(
                    *bounds,
                    moved_layer_bounds(),
                    "a moved layer's own origin must survive the .aur round trip"
                );
            }
            other => unreachable!("expected a pixel layer, got {other:?}"),
        }
        // And that is exactly the state the open paths hand to
        // `reset_canvas_view`.
        assert_eq!(
            active_layer_origin(&recovered, Some(active)),
            (300.0, 150.0)
        );
    }

    /// The autosave pair above is what `App::new`'s own crash-recovery
    /// branch reaches; this is the *user-facing* one — the exact
    /// `aurora_io::write_aur`/`read_aur` pair `App::save_aur_file` and
    /// `App::open_aur_file` themselves call. Same conclusion, on the
    /// path a user actually takes: File > Save As a `.aur`, reopen it,
    /// and the topmost layer is still at `(300, 150)`, so
    /// `load_document_view`'s clamp is load-bearing on the very first
    /// frame.
    #[test]
    fn a_user_facing_aur_save_and_open_preserves_a_moved_layers_origin() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_scratch, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let base = match history.add_pixel_layer(&mut layers, "base", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let _painted = paint_one_texel(&mut store, &layers, base);
        // Added last, so it is the topmost root -- what
        // `topmost_pixel_layer` (and therefore `active_layer`) picks.
        let moved = match history.add_pixel_layer(&mut layers, "moved", moved_layer_bounds(), None)
        {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let _painted = paint_one_texel(&mut store, &layers, moved);

        let path = dir.path().join("moved.aur");
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err}"),
        };
        if let Err(err) =
            aurora_io::write_aur(file, &layers, &history, (320, 160), None, &mut store)
        {
            unreachable!("{err:?}");
        }

        let (_fresh_dir, mut fresh_store) = real_tile_store();
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err}"),
        };
        let (reopened, _history, _canvas, _profile) =
            match aurora_io::read_aur(file, &mut fresh_store) {
                Ok(result) => result,
                Err(err) => unreachable!("{err:?}"),
            };
        let Some(active) = topmost_pixel_layer(&reopened) else {
            unreachable!("the reopened document has pixel layers");
        };
        assert_eq!(
            active_layer_origin(&reopened, Some(active)),
            (300.0, 150.0),
            "a moved layer's own origin must survive the user-facing .aur round trip"
        );

        // And that origin, fed through the very step the open path runs,
        // is what the clamp is for.
        let view = load_document_view(
            &CanvasView::new(),
            &reopened,
            Some(active),
            Some((750, 800)),
            1.0,
        );
        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&reopened, Some(active)));
        assert!(local_x >= -1e-3, "{local_x}");
        assert!(local_y >= -1e-3, "{local_y}");
    }

    /// `App` itself is not constructible under test (it needs a real
    /// `EventLoopProxy`), so no test can call `App::open_file`/
    /// `App::open_aur_file`/`App::new` themselves. This calls the one
    /// thing all three of them *delegate* the whole canvas-view step to
    /// — [`load_document_view`] — rather than re-spelling its two
    /// statements here, which is what this test used to do and why
    /// deleting the clamp from either open path left the suite green.
    /// Reordering or removing the clamp inside `load_document_view` now
    /// fails right here.
    ///
    /// The ordering is the point: the reset returns a pan of `(0, 0)`,
    /// which is outside a moved layer's own bound, so the clamp has to
    /// come second.
    #[test]
    fn loading_a_documents_view_leaves_a_moved_layers_origin_non_negative() {
        let mut layers = aurora_doc::LayerTree::new();
        let moved = match layers.add_pixel_layer("moved", moved_layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let active_layer = Some(moved);

        // A view left panned somewhere by the previous document, exactly
        // as `App` would have it on entry.
        let mut previous = CanvasView::new();
        previous.pan_by((-500.0, -500.0));
        let view = load_document_view(&previous, &layers, active_layer, Some((750, 800)), 1.0);

        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(local_x >= -1e-3, "{local_x}");
        assert!(local_y >= -1e-3, "{local_y}");
        // The reset's own half still happened too -- this is both
        // statements as one unit, not the clamp having replaced the
        // reset. `canvas_min_zoom` for this canvas is the floor a bare
        // `CanvasView::default()` would have dropped (0.57.4).
        assert!(
            (view.min_zoom() - canvas_min_zoom((750, 800), 1.0)).abs() < 1e-6,
            "the zoom floor must be re-derived, not carried from `previous`: {}",
            view.min_zoom()
        );
    }

    /// [`App::new`]'s own spelling of the call above: no window yet, so
    /// no canvas size to derive a floor from. Covers the branch
    /// separately because it is the one that used to be a bare
    /// `aurora_ui::CanvasView::default()` — the exact spelling
    /// `reset_canvas_view` exists to ban — and because `canvas_size:
    /// None` takes `reset_canvas_view`'s other, less-travelled path.
    #[test]
    fn loading_a_documents_view_with_no_window_yet_still_clamps() {
        let mut layers = aurora_doc::LayerTree::new();
        let moved = match layers.add_pixel_layer("moved", moved_layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let active_layer = Some(moved);

        let view = load_document_view(&CanvasView::default(), &layers, active_layer, None, 1.0);

        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(local_x >= -1e-3, "{local_x}");
        assert!(local_y >= -1e-3, "{local_y}");
        assert!(
            (view.min_zoom() - CanvasView::default().min_zoom()).abs() < 1e-6,
            "with no canvas size the previous view's own floor is carried across"
        );
    }

    /// Turns "these two construction paths are safe today" into an
    /// executable check rather than a claim in a comment: `open_file`'s
    /// own flat-image path and `App::new`'s own fresh-document path both
    /// build their layers at the document origin, so their clamp is a
    /// no-op. If either ever stops being true, this breaks here instead
    /// of silently in the canvas.
    #[test]
    fn the_flat_image_and_demo_documents_active_layer_is_always_at_the_document_origin() {
        let image = fake_image(64, 48);
        let (layers, _history, id) = document_from_image("photo", &image);
        assert_eq!(topmost_pixel_layer(&layers), Some(id));
        assert_eq!(active_layer_origin(&layers, Some(id)), (0.0, 0.0));

        let (demo, _history) = demo_document();
        let Some(active) = topmost_pixel_layer(&demo) else {
            unreachable!("demo_document has pixel layers");
        };
        assert_eq!(active_layer_origin(&demo, Some(active)), (0.0, 0.0));
    }

    /// The pan bound (this round) and the zoom floor (0.57.4) are two
    /// clamps on the same view, applied from different call sites in an
    /// order nothing guarantees — `App::new` clamps the pan before any
    /// floor exists, while `redraw`/`apply_resize` raise the floor
    /// later. They have to commute. They do, because `set_min_zoom`
    /// raises zoom through `zoom_at((0, 0), ..)`, which holds
    /// `to_document((0, 0))` fixed across the raise. Asserted here
    /// rather than assumed, and without touching `canvas_view.rs`.
    #[test]
    fn the_pan_bound_and_the_zoom_floor_compose_in_either_order() {
        let mut layers = aurora_doc::LayerTree::new();
        let moved = match layers.add_pixel_layer("moved", moved_layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let active_layer = Some(moved);
        let floor = 2.0;

        let start = || {
            let mut view = CanvasView::new();
            view.zoom_at((0.0, 0.0), 0.5);
            view.pan_by((80.0, 80.0));
            view
        };

        let mut pan_first = start();
        clamp_pan_to_active_layer(&mut pan_first, &layers, active_layer);
        pan_first.set_min_zoom(floor);

        let mut zoom_first = start();
        zoom_first.set_min_zoom(floor);
        clamp_pan_to_active_layer(&mut zoom_first, &layers, active_layer);

        for (label, view) in [("pan first", &pan_first), ("zoom first", &zoom_first)] {
            let (local_x, local_y) =
                canvas_local_origin(view, active_layer_origin(&layers, active_layer));
            assert!(local_x >= -1e-3, "{label}: {local_x}");
            assert!(local_y >= -1e-3, "{label}: {local_y}");
        }
        let (pan_first_x, pan_first_y) = pan_first.to_document((0.0, 0.0));
        let (zoom_first_x, zoom_first_y) = zoom_first.to_document((0.0, 0.0));
        assert!(
            (pan_first_x - zoom_first_x).abs() < 1e-3,
            "the two orders must agree on x: {pan_first_x} vs {zoom_first_x}"
        );
        assert!(
            (pan_first_y - zoom_first_y).abs() < 1e-3,
            "the two orders must agree on y: {pan_first_y} vs {zoom_first_y}"
        );
    }

    /// The *other* way the boundary moves: the active layer stays the
    /// same and its own `bounds` change under it. `App::apply_move`
    /// rewrites them on every pointer-move event of a `Drag::Move` and
    /// deliberately does not clamp there (it would feed back into
    /// `continue_drag`'s own fixed `start_doc`); the clamp happens once,
    /// at the commit. This asserts both halves — the violation really is
    /// live while the drag is (its own negative control, inline) and
    /// really is closed by the commit.
    #[test]
    fn committing_a_move_re_establishes_the_pan_bound() {
        let mut layers = aurora_doc::LayerTree::new();
        let dragged = match layers.add_pixel_layer("dragged", layer_bounds(), None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let active_layer = Some(dragged);

        // Panned to the document's own corner and clamped there against
        // the layer's starting origin of (0, 0).
        let mut view = CanvasView::new();
        view.pan_by((40.0, 40.0));
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);

        // The drag itself: `App::apply_move`'s own `set_bounds`, with no
        // clamp, exactly as the live handler runs it.
        if let Err(err) = layers.set_bounds(dragged, moved_layer_bounds()) {
            unreachable!("{err:?}");
        }
        let (mid_x, mid_y) = canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(
            mid_x < -100.0 && mid_y < -50.0,
            "setup: mid-drag the bound really is violated, which is what the commit has to close: ({mid_x}, {mid_y})"
        );

        commit_ending_drag(
            Some(Drag::Move {
                layer_id: dragged,
                start_doc: (0.0, 0.0),
                start_bounds: layer_bounds(),
                current_bounds: moved_layer_bounds(),
            }),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut view,
            active_layer,
        );

        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Structural],
            "setup: the move still records its one structural step"
        );
        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(
            local_x >= -1e-3,
            "the commit must re-establish the bound on x: {local_x}"
        );
        assert!(
            local_y >= -1e-3,
            "the commit must re-establish the bound on y: {local_y}"
        );
    }

    /// And the third way: an `Undo`. It restores a recorded
    /// `LayerOp::SetBounds` without [`App::active_layer`] changing at
    /// all, so the boundary moves with nothing else in the app aware of
    /// it. (Reached from the command palette and the macOS menu, via
    /// [`App::run_undo_redo`]. The `Ctrl+Z` chord itself resolves and
    /// runs inside [`handle_key`] and never gets here — PLAN.md's own
    /// residual disclosure covers what that still costs.) The sequence is an ordinary session — move a layer to the
    /// document's own corner, pan all the way into that corner (which
    /// the relaxed bound now allows), then undo the move.
    ///
    /// Goes through the real [`run_command`] and the real
    /// [`after_undo_redo`], not a re-spelling of either, so deleting the
    /// clamp from `after_undo_redo` fails here.
    #[test]
    fn undoing_a_move_re_establishes_the_pan_bound() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut cache = CompositeCache::default();

        let moved = match history.add_pixel_layer(&mut layers, "moved", moved_layer_bounds(), None)
        {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let active_layer = Some(moved);
        let mut view = CanvasView::new();
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);

        // Drag it back to the document origin and let go.
        if let Err(err) = layers.set_bounds(moved, layer_bounds()) {
            unreachable!("{err:?}");
        }
        commit_ending_drag(
            Some(Drag::Move {
                layer_id: moved,
                start_doc: (0.0, 0.0),
                start_bounds: moved_layer_bounds(),
                current_bounds: layer_bounds(),
            }),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut view,
            active_layer,
        );
        assert_eq!(undo_order.undo, vec![UndoKind::Structural], "setup");

        // Now pan up/left into the corner the move just freed up.
        view.pan_by((1000.0, 1000.0));
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);
        let (corner_x, corner_y) = view.to_document((0.0, 0.0));
        assert!(
            corner_x.abs() < 1e-3 && corner_y.abs() < 1e-3,
            "setup: the view really is at the document's own corner: ({corner_x}, {corner_y})"
        );

        cache.mark_current(aurora_tile::TileId { x: 0, y: 0 });
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::Undo,
        );
        assert_eq!(
            layers.bounds(moved),
            Some(moved_layer_bounds()),
            "setup: the undo really did restore the moved bounds"
        );
        // The state `run_undo_redo` is in between the two statements:
        // the boundary has moved and nothing has re-clamped yet.
        let (before_x, before_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(
            before_x < -100.0 && before_y < -50.0,
            "setup: the undo really does reopen the divergence: ({before_x}, {before_y})"
        );

        after_undo_redo(&mut view, &layers, active_layer, &mut cache);

        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(local_x >= -1e-3, "{local_x}");
        assert!(local_y >= -1e-3, "{local_y}");
        assert!(
            !cache.is_current(aurora_tile::TileId { x: 0, y: 0 }),
            "the composite cache half of `after_undo_redo` must still run too"
        );
    }

    /// **RT-01 (0.57.7).** The same clamp, reached while a brush stroke
    /// is still live — the hazard `press_layer_row` was written to close
    /// at the *other* site, never audited at this one. `run_undo_redo`
    /// moved the view out from under a `Drag::Brush` whose own
    /// `last_doc` was fixed when the stroke began, and the very next
    /// pointer-move event then interpolated a full segment between the
    /// stale reference and the moved view: a line of dabs the user
    /// never drew, painted with the pointer completely still. Worse,
    /// the live stroke's own pixels were left on the layer with no undo
    /// entry naming them, so the `Undo` that caused it reached past
    /// them into the previous step (`commit_ending_drag`'s own 0.57.0
    /// bug, at a fourth site).
    ///
    /// Drives the real [`perform_undo_redo`] and the real
    /// [`continue_drag`], not a re-spelling of either, so removing the
    /// commit from `perform_undo_redo` fails here.
    #[test]
    fn an_undo_during_a_live_stroke_commits_it_instead_of_painting_a_line_the_user_never_drew() {
        let (_dir, mut store) = commit_test_store();
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut cache = CompositeCache::default();
        let mut selection = aurora_doc::SelectionSet::new();

        // Exactly `undoing_a_move_re_establishes_the_pan_bound`'s own
        // setup: a layer moved to (300, 150), dragged back to the
        // document origin, with the view panned into the corner that
        // move freed up. Undoing the move is what moves the boundary.
        let moved = match history.add_pixel_layer(&mut layers, "moved", moved_layer_bounds(), None)
        {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let active_layer = Some(moved);
        let mut view = CanvasView::new();
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);
        if let Err(err) = layers.set_bounds(moved, layer_bounds()) {
            unreachable!("{err:?}");
        }
        commit_ending_drag(
            Some(Drag::Move {
                layer_id: moved,
                start_doc: (0.0, 0.0),
                start_bounds: moved_layer_bounds(),
                current_bounds: layer_bounds(),
            }),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut view,
            active_layer,
        );
        view.pan_by((1000.0, 1000.0));
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);
        assert_eq!(undo_order.undo, vec![UndoKind::Structural], "setup");

        // The stroke still in progress when the Undo arrives, with real
        // pixels already on the layer and its own reference point at
        // the pointer's current document position.
        let pointer = (40.0, 40.0);
        let mut drag = Some(a_brush_drag_that_painted(&mut store, (30.5, 30.5)));
        match drag.as_mut() {
            Some(Drag::Brush { last_doc, .. }) => *last_doc = view.to_document(pointer),
            _ => unreachable!("just built a brush drag"),
        }
        assert!(commit_test_alpha(&mut store, 30, 30) > 0.5, "setup");

        perform_undo_redo(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            &mut cache,
            &mut view,
            active_layer,
            &mut drag,
            AppCommand::Undo,
        );

        assert!(
            drag.is_none(),
            "no drag may still be live once the undo has clamped the view under it"
        );
        // The pointer has not moved at all. Whatever the view did, the
        // next move event must not paint.
        let dabs = match drag.as_mut() {
            Some(live) => continue_drag(
                live,
                pointer,
                &mut view,
                &mut selection,
                active_layer_origin(&layers, active_layer),
            ),
            None => Vec::new(),
        };
        assert!(
            dabs.is_empty(),
            "a still pointer must not paint: {} dabs were placed",
            dabs.len()
        );
        // And the Undo has to have reached the live stroke, not past it
        // into the move underneath.
        assert_eq!(
            undo_order.redo,
            vec![UndoKind::Pixel],
            "the undo must have undone the interrupted stroke's own entry"
        );
        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Structural],
            "and must have left the move underneath it alone"
        );
        assert!(
            commit_test_alpha(&mut store, 30, 30) < 0.01,
            "the interrupted stroke's own pixels are what the undo removes"
        );
        assert_eq!(
            layers.bounds(moved),
            Some(layer_bounds()),
            "the move underneath must still be applied"
        );
    }

    /// Selecting a Layers-panel row now moves the pan bound
    /// ([`select_layer`]), and that branch of `handle_pointer_pressed`
    /// used to `return` before ever reaching the shared "a second press
    /// ends the live drag" commit — so a middle-button pan held down
    /// while left-clicking a row left the view being clamped out from
    /// under a drag that still holds a fixed reference point. Ending the
    /// drag first is the fix; this is the sequence that branch now runs.
    #[test]
    fn selecting_a_layer_row_during_a_live_drag_ends_the_drag_before_the_view_moves() {
        let (mut workspace, layers, layer_rows, a, b) = two_layers_one_moved();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut active_layer = Some(a);
        let mut view = CanvasView::new();
        view.pan_by((40.0, 40.0));
        clamp_pan_to_active_layer(&mut view, &layers, active_layer);

        // The middle button goes down over the canvas and stays down.
        let mut drag = begin_drag(
            Tool::Brush,
            PointerButton::Middle,
            (10.0, 10.0),
            &view,
            None,
        );
        assert!(
            matches!(drag, Some(Drag::Pan { .. })),
            "setup: the middle button really starts a pan"
        );

        // The left button then clicks a Layers-panel row.
        press_layer_row(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut drag,
            b,
        );

        assert!(
            drag.is_none(),
            "no drag may still be live once the selection has clamped the view under it"
        );
        assert_eq!(active_layer, Some(b));
        let (local_x, local_y) =
            canvas_local_origin(&view, active_layer_origin(&layers, active_layer));
        assert!(local_x >= -1e-3, "{local_x}");
        assert!(local_y >= -1e-3, "{local_y}");
    }

    /// The ordering *within* [`press_layer_row`], made observable: a
    /// `Drag::Move` interrupted by a layer-row click has to be committed
    /// against the layer that was active while it was being dragged, not
    /// against the one the click is about to select. Both orders leave
    /// the same undo entry, so only the resulting pan tells them apart —
    /// which it does whenever the dragged layer ends up further from the
    /// document origin than the newly selected one.
    #[test]
    fn a_layer_row_click_commits_a_move_against_the_outgoing_layer_not_the_incoming_one() {
        let (mut workspace, mut layers, layer_rows, a, b) = two_layers_one_moved();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut active_layer = Some(a);
        let mut view = CanvasView::new();

        // `a` is dragged well past `b`'s own origin -- far enough that
        // the two clamps disagree.
        let dragged_to = aurora_core::Rect {
            x: 500,
            y: 500,
            ..layer_bounds()
        };
        if let Err(err) = layers.set_bounds(a, dragged_to) {
            unreachable!("{err:?}");
        }
        let mut drag = Some(Drag::Move {
            layer_id: a,
            start_doc: (0.0, 0.0),
            start_bounds: layer_bounds(),
            current_bounds: dragged_to,
        });

        press_layer_row(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut drag,
            b,
        );

        assert_eq!(active_layer, Some(b));
        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Structural],
            "setup: the interrupted move still records its own step"
        );
        let (doc_x, doc_y) = view.to_document((0.0, 0.0));
        assert!(
            (doc_x - 500.0).abs() < 1e-3 && (doc_y - 500.0).abs() < 1e-3,
            "the move must be committed against `a`'s own dragged origin (500, 500) before the \
             selection relaxes the bound to `b`'s (300, 150); selecting first would leave this at \
             (300, 150): ({doc_x}, {doc_y})"
        );
    }

    /// The same branch's other, pre-existing casualty, now fixed by the
    /// same line: a live brush stroke interrupted by a layer-row click
    /// used to be dropped outright, losing its whole undo entry the way
    /// `commit_ending_drag`'s own doc comment describes for the gestures
    /// 0.57.0 already covered.
    #[test]
    fn a_stroke_interrupted_by_a_layer_row_click_still_becomes_its_own_undo_entry() {
        let (_dir, mut store) = commit_test_store();
        let (mut workspace, layers, layer_rows, _a, b) = two_layers_one_moved();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut active_layer = None;
        let mut view = CanvasView::new();

        let mut drag = Some(a_brush_drag_that_painted(&mut store, (30.5, 30.5)));
        press_layer_row(
            &mut workspace,
            &layer_rows,
            &mut active_layer,
            &mut view,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut drag,
            b,
        );

        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Pixel],
            "the interrupted stroke must have its own entry in the unified order"
        );
        assert!(pixel_history.can_undo());
    }

    /// `demo_document`'s whole point (unlike a plain `LayerTree` built
    /// directly) is that its `History` has a real, meaningful journal
    /// for the History panel to show — confirms that, not just that
    /// the layers themselves are right.
    #[test]
    fn demo_document_history_describes_every_demo_action() {
        let (_layers, history) = demo_document();
        let descriptions = history.journal_descriptions();
        // Layer ids are process-local and monotonic (0, 1, 2, ...) for a
        // fresh tree, so "Color balance" (the second layer added) is
        // always id 1 -- a real, deterministic value, not a guess.
        assert_eq!(
            descriptions,
            vec![
                "Added layer \"Background\"".to_owned(),
                "Added layer \"Color balance\"".to_owned(),
                "Set blend mode of layer #1 to Multiply".to_owned(),
                "Set opacity of layer #1 to 80%".to_owned(),
                "Added layer \"Retouch — skin\"".to_owned(),
            ]
        );
    }

    // -- opening a real file --
    //
    // PLAN.md M1.9's "no document-import pipeline" gap: `document_from_image`,
    // `open_image`, and `replace_document` are the pure/near-pure pieces
    // `App::open_file` composes -- each independently testable with no
    // window/GPU/event-loop, the same seam every other dispatch function
    // in this crate already uses.

    /// A small, solid-colour `aurora_io::Image` -- a real, valid
    /// decoded image these tests treat as if `png::decode` had just
    /// produced it, without needing a real file on disk for tests that
    /// don't care about the read/decode step itself.
    fn fake_image(width: u32, height: u32) -> aurora_io::Image {
        let samples: Vec<half::f16> = (0..width as usize * height as usize)
            .flat_map(|_| [0.0, 1.0, 0.0, 1.0].map(half::f16::from_f32))
            .collect();
        match aurora_io::Image::new(width, height, aurora_color::IccProfile::srgb(), samples) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn document_from_image_sizes_the_new_layer_to_the_images_own_dimensions() {
        let image = fake_image(640, 480);
        let (layers, history, id) = document_from_image("photo", &image);

        assert_eq!(layers.roots(), &[id]);
        match layers.kind(id) {
            Some(aurora_doc::LayerKind::Pixel { bounds }) => {
                assert_eq!(
                    *bounds,
                    aurora_core::Rect {
                        x: 0,
                        y: 0,
                        width: 640,
                        height: 480
                    }
                );
            }
            other => unreachable!("expected a pixel layer, got {other:?}"),
        }
        assert_eq!(
            history.journal_descriptions(),
            vec!["Added layer \"photo\"".to_owned()]
        );
    }

    #[test]
    fn open_image_reads_and_decodes_a_real_png_file() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let image = fake_image(4, 4);
        let bytes = match aurora_io::png::encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("photo.png");
        if let Err(err) = std::fs::write(&path, bytes) {
            unreachable!("{err:?}");
        }

        let Some(decoded) = open_image(&path) else {
            unreachable!("a real, freshly written PNG must decode");
        };
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    #[test]
    fn open_image_returns_none_for_a_path_that_does_not_exist() {
        assert!(open_image(std::path::Path::new("/no/such/file.png")).is_none());
    }

    #[test]
    fn open_image_returns_none_for_an_unsupported_extension() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("document.psd");
        if let Err(err) = std::fs::write(&path, b"whatever") {
            unreachable!("{err:?}");
        }
        assert!(open_image(&path).is_none());
    }

    #[test]
    fn replace_document_clears_the_old_rows_and_populates_exactly_the_new_layer() {
        let mut workspace = aurora_ui::build_workspace();
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        };
        let (old_layers, old_history) = demo_document();
        if aurora_ui::populate_layers_panel(
            &mut workspace.tree,
            workspace.layers,
            &scales,
            &old_layers,
        )
        .is_err()
        {
            unreachable!("a freshly built workspace's own panel body must accept this");
        }
        if aurora_ui::populate_history_panel(&mut workspace.tree, workspace.history, &old_history)
            .is_err()
        {
            unreachable!("a freshly built workspace's own panel body must accept this");
        }
        assert!(
            workspace
                .tree
                .children(workspace.layers.body)
                .unwrap_or(&[])
                .len()
                > 1,
            "the demo document must have seeded more than one row"
        );

        let image = fake_image(8, 8);
        let (new_layers, new_history, new_layer_id) = document_from_image("photo", &image);
        let (layer_rows, active_layer) = match replace_document(
            &mut workspace,
            &scales,
            &new_layers,
            &new_history,
            aurora_ui::Tool::default(),
        ) {
            Ok(result) => result,
            Err(err) => unreachable!("{err:?}"),
        };

        assert_eq!(
            workspace
                .tree
                .children(workspace.layers.body)
                .map(<[_]>::len),
            Some(1),
            "old demo rows must be gone, replaced by exactly the new layer's own row"
        );
        assert_eq!(
            workspace
                .tree
                .children(workspace.history.body)
                .map(<[_]>::len),
            Some(1),
            "old demo history rows must be gone too"
        );
        assert_eq!(new_layers.roots(), &[new_layer_id]);
        assert_eq!(active_layer, Some(new_layer_id));
        assert_eq!(layer_rows.len(), 1);
        assert_eq!(layer_rows.values().copied().next(), Some(new_layer_id));
    }

    #[test]
    fn is_aur_path_matches_case_insensitively_and_rejects_other_extensions() {
        assert!(is_aur_path(std::path::Path::new("photo.aur")));
        assert!(is_aur_path(std::path::Path::new("photo.AUR")));
        assert!(!is_aur_path(std::path::Path::new("photo.png")));
        assert!(!is_aur_path(std::path::Path::new("photo")));
    }

    #[test]
    fn document_canvas_size_reads_the_topmost_pixel_layers_bounds() {
        let image = fake_image(12, 34);
        let (layers, _history, _id) = document_from_image("photo", &image);
        assert_eq!(document_canvas_size(&layers), (12, 34));
    }

    #[test]
    fn document_canvas_size_is_zero_zero_for_an_empty_tree() {
        let layers = aurora_doc::LayerTree::new();
        assert_eq!(document_canvas_size(&layers), (0, 0));
    }

    #[test]
    fn verify_aur_accepts_a_real_written_file_and_rejects_garbage() {
        // Shared with `repeated_aur_verification_does_not_accumulate_
        // scratch_tiles`, which counts what verification leaves in the
        // session scratch directory and cannot do so while another
        // verification is in flight.
        let _guard = AUR_VERIFY_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let image = fake_image(4, 4);
        let (layers, history, _id) = document_from_image("photo", &image);

        let good_path = dir.path().join("real.aur");
        let file = match std::fs::File::create(&good_path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = aurora_io::write_aur(file, &layers, &history, (4, 4), None, &mut store) {
            unreachable!("{err:?}");
        }
        assert!(
            verify_aur(&good_path),
            "a real, just-written .aur file must verify"
        );

        let garbage_path = dir.path().join("garbage.aur");
        if let Err(err) = std::fs::write(&garbage_path, b"not a real .aur file") {
            unreachable!("{err:?}");
        }
        assert!(
            !verify_aur(&garbage_path),
            "garbage bytes must not verify as a real .aur file"
        );
    }

    #[test]
    fn write_verified_writes_a_real_verifiable_file() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let image = fake_image(4, 4);
        let bytes = match aurora_io::png::encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("export.png");

        assert!(write_verified(&path, &bytes, 4, 4));
        assert!(path.exists());
        assert!(
            !path.with_file_name("export.png.tmp").exists(),
            "the temp file must be renamed away, not left behind"
        );
        let written = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let decoded = match aurora_io::decode_by_extension(&path, &written) {
            Ok(image) => image,
            Err(err) => {
                unreachable!("the written file must itself be a real, decodable PNG: {err:?}")
            }
        };
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }

    #[test]
    fn write_verified_rejects_corrupt_bytes_and_creates_no_destination_file() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("export.png");

        assert!(!write_verified(&path, b"not a real png", 4, 4));
        assert!(
            !path.exists(),
            "a failed verify must not create the destination"
        );
        assert!(
            !path.with_file_name("export.png.tmp").exists(),
            "the failed temp file must be cleaned up, not left behind"
        );
    }

    #[test]
    fn write_verified_never_touches_an_existing_destination_on_failure() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("export.png");
        if let Err(err) = std::fs::write(&path, b"pre-existing content") {
            unreachable!("{err:?}");
        }

        assert!(!write_verified(&path, b"not a real png", 4, 4));
        let survived = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            survived, b"pre-existing content",
            "a failed export must never overwrite what was already at the destination"
        );
    }

    #[test]
    fn write_verified_rejects_a_size_mismatch() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let image = fake_image(4, 4);
        let bytes = match aurora_io::png::encode(&image) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        let path = dir.path().join("export.png");

        // Claims the export is 8x8, but the real PNG bytes decode back
        // as 4x4 -- must be treated as a verify failure, not silently
        // accepted.
        assert!(!write_verified(&path, &bytes, 8, 8));
        assert!(!path.exists());
    }

    // -- keyboard shortcuts and the command palette --
    //
    // Every function under test here is deliberately free of `winit`
    // window/event-loop types (see this module's own doc comment), so
    // these run with no window, no `EventLoopProxy`, and no display
    // server -- exactly what this sandbox has never had for the rest of
    // this crate's own real-hardware-gated pieces.

    #[test]
    fn default_shortcuts_binds_tab_shift_tab_and_toggle_palette() {
        let shortcuts = default_shortcuts();
        let tab = match KeyChord::parse("Tab") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        let shift_tab = match KeyChord::parse("Shift+Tab") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        let toggle = match KeyChord::parse("Ctrl+Shift+P") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(shortcuts.resolve(tab), Some(&AppCommand::FocusNext));
        assert_eq!(
            shortcuts.resolve(shift_tab),
            Some(&AppCommand::FocusPrevious)
        );
        assert_eq!(
            shortcuts.resolve(toggle),
            Some(&AppCommand::ToggleCommandPalette)
        );
    }

    #[test]
    fn default_shortcuts_binds_ctrl_z_and_ctrl_shift_z_to_undo_and_redo() {
        let shortcuts = default_shortcuts();
        let undo = match KeyChord::parse("Ctrl+Z") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        let redo = match KeyChord::parse("Ctrl+Shift+Z") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(shortcuts.resolve(undo), Some(&AppCommand::Undo));
        assert_eq!(shortcuts.resolve(redo), Some(&AppCommand::Redo));
    }

    #[test]
    fn translate_key_lowercases_a_character_and_maps_named_keys() {
        assert_eq!(
            translate_key(&winit::keyboard::Key::Character("P".into())),
            Some(Key::Character('p'))
        );
        assert_eq!(
            translate_key(&winit::keyboard::Key::Named(
                winit::keyboard::NamedKey::Escape
            )),
            Some(Key::Named(NamedKey::Escape))
        );
    }

    #[test]
    fn translate_key_returns_none_for_a_key_with_no_shortcut_vocabulary_entry() {
        // `CapsLock` is a real `winit::keyboard::NamedKey` variant with
        // deliberately no `aurora_widgets::shortcut::NamedKey`
        // counterpart (see that type's own doc comment) -- confirms the
        // fallback is a real `None`, not a panic or a silent wrong
        // mapping.
        assert_eq!(
            translate_key(&winit::keyboard::Key::Named(
                winit::keyboard::NamedKey::CapsLock
            )),
            None
        );
    }

    #[test]
    fn translate_modifiers_reads_every_flag_independently() {
        let state =
            winit::keyboard::ModifiersState::CONTROL | winit::keyboard::ModifiersState::SHIFT;
        let modifiers = translate_modifiers(state);
        assert!(modifiers.control);
        assert!(modifiers.shift);
        assert!(!modifiers.alt);
        assert!(!modifiers.meta);
    }

    // -- DPI/scale-factor conversion --

    #[test]
    fn logical_size_is_unchanged_at_a_scale_factor_of_one() {
        assert_eq!(logical_size((1280, 800), 1.0), (1280.0, 800.0));
    }

    #[test]
    fn logical_size_divides_out_a_scale_factor_above_one() {
        // A real, common HiDPI factor -- 2560x1600 physical at 2.0x is
        // exactly the 1280x800 logical size `resumed`'s own initial
        // window request asks for.
        assert_eq!(logical_size((2560, 1600), 2.0), (1280.0, 800.0));
    }

    #[test]
    fn logical_size_handles_a_fractional_scale_factor() {
        assert_eq!(logical_size((1920, 1080), 1.25), (1536.0, 864.0));
    }

    #[test]
    fn logical_size_handles_a_scale_factor_below_one() {
        // Real on some Linux compositors that allow scaling down, not
        // just up -- must divide, not silently clamp to 1.0.
        assert_eq!(logical_size((800, 600), 0.8), (1000.0, 750.0));
    }

    #[test]
    fn logical_size_falls_back_to_one_for_a_non_positive_scale_factor() {
        // Not a value `winit` should ever actually report, but this
        // function stays total (no division by zero or a negative
        // result) rather than trusting an external value blindly.
        assert_eq!(logical_size((1280, 800), 0.0), (1280.0, 800.0));
        assert_eq!(logical_size((1280, 800), -2.0), (1280.0, 800.0));
    }

    #[test]
    fn logical_size_falls_back_to_one_for_a_non_finite_scale_factor() {
        assert_eq!(logical_size((1280, 800), f64::NAN), (1280.0, 800.0));
        assert_eq!(logical_size((1280, 800), f64::INFINITY), (1280.0, 800.0));
    }

    #[test]
    fn run_command_focus_next_visits_every_docked_panel_in_order() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.layers.root));
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.properties.root));
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.history.root));

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::FocusPrevious,
        );
        assert_eq!(
            focus.focused(),
            Some(workspace.properties.root),
            "Shift+Tab must step backward through the same order"
        );
    }

    #[test]
    fn run_command_select_tool_switches_the_active_tool() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Pan),
        );
        assert_eq!(tool, Tool::Pan);
    }

    #[test]
    fn run_command_select_tool_to_brush_populates_a_real_radius_row() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Brush),
        );

        let Some(rows) = workspace.tree.children(workspace.properties.body) else {
            unreachable!("just refreshed");
        };
        assert_eq!(rows.len(), 1);
        let Some(&row) = rows.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(accessibility) = workspace.tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        let expected = format!("Radius: {BRUSH_RADIUS}px");
        assert_eq!(accessibility.label(), Some(expected.as_str()));
    }

    #[test]
    fn run_command_select_tool_to_eraser_populates_a_real_radius_row() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Eraser),
        );

        let Some(rows) = workspace.tree.children(workspace.properties.body) else {
            unreachable!("just refreshed");
        };
        assert_eq!(rows.len(), 1);
        let Some(&row) = rows.first() else {
            unreachable!("just asserted len() == 1");
        };
        let Some(accessibility) = workspace.tree.accessibility(row) else {
            unreachable!("just inserted");
        };
        let expected = format!("Radius: {ERASER_RADIUS}px");
        assert_eq!(accessibility.label(), Some(expected.as_str()));
    }

    #[test]
    fn run_command_select_tool_to_move_leaves_the_properties_panel_empty() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Move),
        );

        assert_eq!(
            workspace.tree.children(workspace.properties.body),
            Some([].as_slice()),
            "Move has no real backing parameter yet -- an honest empty panel, not an invented row"
        );
    }

    /// The most likely place for a real bug: forgetting to
    /// `clear_panel_body` before repopulating would leave a previous
    /// tool's rows sitting alongside (or instead of clearing away for) the
    /// newly selected tool's own rows. Exercises both directions: a
    /// tool with real options (Brush) followed by one with none (Move)
    /// must really empty the panel, not leave Brush's row behind; and
    /// switching between two tools that both have real options (Move,
    /// then Eraser) must land on exactly the new tool's own row count,
    /// not an accumulated total.
    #[test]
    fn run_command_select_tool_clears_stale_rows_from_the_previous_tool() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Brush),
        );
        assert_eq!(
            workspace
                .tree
                .children(workspace.properties.body)
                .map(<[_]>::len),
            Some(1),
            "Brush must seed its own Radius row first"
        );

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Move),
        );
        assert_eq!(
            workspace.tree.children(workspace.properties.body),
            Some([].as_slice()),
            "switching to a tool with no real options must really empty the panel, not leave \
             the previous tool's row behind"
        );

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::SelectTool(Tool::Eraser),
        );
        assert_eq!(
            workspace
                .tree
                .children(workspace.properties.body)
                .map(<[_]>::len),
            Some(1),
            "switching tools repeatedly must not accumulate rows from earlier tools"
        );
    }

    #[test]
    fn run_command_undo_reverts_a_bounds_change_and_refreshes_the_history_panel() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let id = match history.add_pixel_layer(&mut layers, "a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let moved = aurora_core::Rect {
            x: 5,
            y: 5,
            ..bounds
        };
        if let Err(err) = history.set_bounds(&mut layers, id, moved) {
            unreachable!("{err:?}");
        }
        assert_eq!(layers.bounds(id), Some(moved));
        // Simulates what `finish_move` itself does after a
        // successful `history.record_bounds_change` call, since this
        // test drives `history` directly rather than through `App`.
        let mut undo_order = UndoOrder::default();
        undo_order.record(UndoKind::Structural, &mut history, &mut pixel_history);

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::Undo,
        );
        assert_eq!(layers.bounds(id), Some(bounds), "undo must revert the move");
        let Some(rows) = workspace.tree.children(workspace.history.body) else {
            unreachable!("populate_history_panel always inserts a body");
        };
        assert_eq!(
            rows.len(),
            history.journal_len(),
            "the History panel must reflect the undo's own journal entry"
        );

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::Redo,
        );
        assert_eq!(layers.bounds(id), Some(moved), "redo must reapply the move");
    }

    #[test]
    fn run_command_undo_with_nothing_to_undo_is_a_safe_no_op() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::Undo,
        );
        assert!(layers.is_empty());
        assert_eq!(history.journal_len(), 0);
    }

    #[test]
    // Exact-literal round-trip through f16 storage, no arithmetic --
    // same reasoning `aurora-doc`'s own tests already document for
    // their float_cmp allows. Long because it walks a real four-step
    // undo/redo sequence end to end -- splitting it into several
    // same-length-total helper functions would just relocate the same
    // lines, not reduce real complexity, the same reasoning
    // `aurora_doc::history::apply` already documents for its own allow.
    #[allow(clippy::float_cmp, clippy::too_many_lines)]
    fn run_command_undo_redo_walk_structural_and_pixel_edits_in_true_chronological_order() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let (_dir, mut store) = real_tile_store();

        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let id = match history.add_pixel_layer(&mut layers, "a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let original_opacity = layers.opacity(id);

        // 1) A structural edit first...
        if let Err(err) = history.set_opacity(&mut layers, id, 0.5) {
            unreachable!("{err:?}");
        }
        undo_order.record(UndoKind::Structural, &mut history, &mut pixel_history);

        // 2) ...then a pixel stroke, on the very same layer's surface.
        let surface = aurora_tile::SurfaceId::from_raw(id.to_raw());
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        let mut stroke = aurora_brush::StrokeSnapshot::new(surface);
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        let Ok(painted) = store.get_mut(surface, tile) else {
            unreachable!("a real store must accept this write");
        };
        for sample in painted.texels_mut() {
            *sample = half::f16::from_f32(0.75);
        }
        assert!(pixel_history.push(stroke));
        undo_order.record(UndoKind::Pixel, &mut history, &mut pixel_history);

        // The pixel stroke was the more recent edit -- Ctrl+Z must
        // reach it first, leaving the structural change untouched.
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            AppCommand::Undo,
        );
        let Ok(after_first_undo) = store.get(surface, tile) else {
            unreachable!("just written");
        };
        let Some(&sample) = after_first_undo.texels().first() else {
            unreachable!("a real tile always has at least one sample");
        };
        assert_eq!(sample.to_f32(), 0.0, "the pixel stroke must undo first");
        assert_eq!(
            layers.opacity(id),
            Some(0.5),
            "the structural opacity change must still be untouched"
        );

        // A second Ctrl+Z must now reach the structural edit.
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            AppCommand::Undo,
        );
        assert_eq!(
            layers.opacity(id),
            original_opacity,
            "the second undo must reach the structural edit"
        );

        // Redo walks the exact same order back: structural first, then
        // pixel.
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            AppCommand::Redo,
        );
        assert_eq!(
            layers.opacity(id),
            Some(0.5),
            "the first redo must reapply the structural edit"
        );

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            Some(&mut store),
            &mut undo_order,
            AppCommand::Redo,
        );
        let Ok(after_second_redo) = store.get(surface, tile) else {
            unreachable!("just written");
        };
        let Some(&sample) = after_second_redo.texels().first() else {
            unreachable!("a real tile always has at least one sample");
        };
        assert_eq!(
            sample.to_f32(),
            0.75,
            "the second redo must reapply the pixel stroke"
        );
    }

    #[test]
    fn run_command_pixel_undo_with_no_live_store_leaves_the_unified_order_untouched() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let (_dir, mut store) = real_tile_store();

        let surface = aurora_tile::SurfaceId::from_raw(0);
        let tile = aurora_tile::TileId { x: 0, y: 0 };
        let mut stroke = aurora_brush::StrokeSnapshot::new(surface);
        if let Err(err) = stroke.record_touch(&mut store, tile) {
            unreachable!("{err:?}");
        }
        assert!(pixel_history.push(stroke));
        let mut undo_order = UndoOrder::default();
        undo_order.record(UndoKind::Pixel, &mut history, &mut pixel_history);
        assert!(pixel_history.can_undo());

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            AppCommand::Undo,
        );
        assert!(
            pixel_history.can_undo(),
            "with no live store to restore into, the pending stroke must be left exactly as it was"
        );
        assert_eq!(
            undo_order.undo,
            [UndoKind::Pixel],
            "a failed attempt must not desync the unified order from the backing store"
        );
    }

    #[test]
    fn toggle_command_palette_opens_then_closes() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;

        toggle_command_palette(&mut workspace, &mut focus, &mut palette);
        let Some(root) = palette else {
            unreachable!("toggling from closed must open the palette");
        };
        assert_eq!(
            workspace
                .tree
                .accessibility(root)
                .map(accesskit::Node::role),
            Some(accesskit::Role::TextInput)
        );
        assert_eq!(
            focus.focused(),
            Some(root),
            "opening the palette must focus it"
        );

        toggle_command_palette(&mut workspace, &mut focus, &mut palette);
        assert_eq!(palette, None);
        assert!(
            !workspace.tree.contains(root),
            "closing must remove the palette from the tree, not just hide it"
        );
        assert_eq!(
            focus.focused(),
            None,
            "focus left on the now-removed palette must be cleared"
        );
    }

    /// The real bug Cahya found by actually trying `Ctrl+Shift+P`:
    /// nothing had ever given the palette's own root a real style, so
    /// it resolved to 0x0 in a live window. This proves the fix
    /// headlessly (no GPU needed) -- real, nonzero bounds, and
    /// genuinely centred horizontally, not just "not zero."
    #[test]
    fn opening_the_palette_gives_it_a_real_centred_size_and_position() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;

        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let Some(root) = palette else {
            unreachable!("just opened");
        };
        workspace.tree.compute_layout(1000.0, 800.0);

        let Some(bounds) = workspace.tree.bounds(root) else {
            unreachable!("just laid out");
        };
        assert_eq!(bounds.width, 480, "must be the real, fixed palette width");
        assert_eq!(bounds.height, 320, "must be the real, fixed palette height");
        assert_eq!(bounds.y, 96, "must sit at the real, fixed top inset");
        assert_eq!(
            bounds.x, 260,
            "must be horizontally centred: (1000 - 480) / 2"
        );
    }

    #[test]
    fn opening_the_palette_a_second_time_is_a_no_op() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let first = palette;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        assert_eq!(palette, first, "already open must not reopen or replace it");
    }

    #[test]
    fn typing_into_the_open_palette_filters_to_a_matching_command() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);

        let mut clipboard = FakeClipboard::default();
        let mut file_dialog = FakeFileDialog::default();
        for ch in ['l', 'a', 'y'] {
            handle_palette_key(
                &mut workspace,
                &mut focus,
                &mut palette,
                KeyChord::new(Modifiers::none(), Key::Character(ch)),
                Some(&ch.to_string()),
                &mut clipboard,
                &mut file_dialog,
            );
        }

        let Some(root) = palette else {
            unreachable!("typing must not close the palette");
        };
        let state = match aurora_widgets::widgets::command_palette_state(&workspace.tree, root) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.query(), "lay");
        // "Focus Layers Panel", "Toggle Layers Panel", and "Close Layers
        // Panel" all match -- the first inserted (`palette_commands`'s
        // own order) is what ends up selected.
        assert_eq!(state.results().len(), 3);
        assert_eq!(
            state.selected().map(|entry| entry.id.as_str()),
            Some(COMMAND_FOCUS_LAYERS)
        );
    }

    #[test]
    fn activating_a_command_with_enter_closes_the_palette_and_focuses_its_target() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        // The first result (`palette_commands`' own order) is already
        // "Focus Layers Panel" -- activate it directly, no typing needed.
        handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Enter)),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );

        assert_eq!(palette, None, "activating a command must close the palette");
        assert_eq!(
            focus.focused(),
            Some(workspace.layers.root),
            "the activated command's own target must end up focused"
        );
    }

    #[test]
    fn escape_closes_the_palette_without_focusing_any_command_target() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Escape)),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert_eq!(palette, None);
        assert_eq!(focus.focused(), None);
    }

    #[test]
    fn close_command_palette_on_an_already_closed_palette_is_a_no_op() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        close_command_palette(&mut workspace, &mut focus, &mut palette);
        assert_eq!(palette, None);
    }

    #[test]
    fn handle_key_routes_tab_to_focus_next_when_the_palette_is_closed() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers::none(),
            Key::Named(NamedKey::Tab),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert_eq!(focus.focused(), Some(workspace.layers.root));
    }

    #[test]
    fn handle_key_routes_a_tool_letter_to_select_tool_when_the_palette_is_closed() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers::none(),
            Key::Character('h'),
            Some("h"),
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert_eq!(tool, Tool::Pan);
    }

    #[test]
    fn handle_key_routes_ctrl_z_to_undo_when_the_palette_is_closed() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let id = match history.add_pixel_layer(
            &mut layers,
            "a",
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut pixel_history = aurora_brush::PixelHistory::new();
        // Simulates what a real App-level recording call (like
        // `App::apply_move`) would have done when `add_pixel_layer` was
        // called above.
        let mut undo_order = UndoOrder::default();
        undo_order.record(UndoKind::Structural, &mut history, &mut pixel_history);
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
            Key::Character('z'),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert!(
            !layers.contains(id),
            "Ctrl+Z must undo the just-added layer"
        );
    }

    #[test]
    fn handle_key_ignores_an_unbound_chord() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        // 'q' is deliberately not one of `default_shortcuts`' own
        // tool-switch letters (v/m/z/h/i) or anything else bound.
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers::none(),
            Key::Character('q'),
            Some("q"),
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );
        assert_eq!(focus.focused(), None);
        assert_eq!(palette, None);
        assert_eq!(tool, Tool::default(), "must not have switched tools");
    }

    #[test]
    fn handle_key_routes_typing_to_the_palette_instead_of_shortcuts_while_open() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        open_command_palette(&mut workspace, &mut focus, &mut palette);

        // `p` alone isn't a bound shortcut (`Ctrl+Shift+P` is), so this
        // also confirms typing a plain character doesn't accidentally
        // fall through to shortcut resolution while the palette is open.
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers::none(),
            Key::Character('p'),
            Some("p"),
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );

        let Some(root) = palette else {
            unreachable!("typing must not close the palette");
        };
        let state = match aurora_widgets::widgets::command_palette_state(&workspace.tree, root) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.query(), "p");
    }

    // -- clipboard and file dialogs (M1.8) --
    //
    // `handle_palette_key` takes its clipboard/file-dialog access as
    // trait objects specifically so these can run against
    // `FakeClipboard`/`FakeFileDialog` -- no real OS clipboard or
    // native picker involved, matching this module's own doc comment.

    #[test]
    fn ctrl_c_copies_the_current_query_to_the_clipboard() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let mut clipboard = FakeClipboard::default();
        let mut file_dialog = FakeFileDialog::default();

        for ch in ['l', 'a', 'y'] {
            handle_palette_key(
                &mut workspace,
                &mut focus,
                &mut palette,
                KeyChord::new(Modifiers::none(), Key::Character(ch)),
                Some(&ch.to_string()),
                &mut clipboard,
                &mut file_dialog,
            );
        }
        handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(
                Modifiers {
                    control: true,
                    ..Modifiers::none()
                },
                Key::Character('c'),
            ),
            None,
            &mut clipboard,
            &mut file_dialog,
        );

        assert_eq!(clipboard.get_text().as_deref(), Some("lay"));
        assert!(palette.is_some(), "copying must not close the palette");
    }

    #[test]
    fn ctrl_v_pastes_the_clipboard_into_the_query() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let mut clipboard = FakeClipboard {
            contents: Some("history".to_owned()),
        };
        let mut file_dialog = FakeFileDialog::default();

        handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(
                Modifiers {
                    control: true,
                    ..Modifiers::none()
                },
                Key::Character('v'),
            ),
            None,
            &mut clipboard,
            &mut file_dialog,
        );

        let Some(root) = palette else {
            unreachable!("pasting must not close the palette");
        };
        let state = match aurora_widgets::widgets::command_palette_state(&workspace.tree, root) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.query(), "history");
        assert_eq!(
            state.selected().map(|entry| entry.id.as_str()),
            Some(COMMAND_FOCUS_HISTORY)
        );
    }

    #[test]
    fn pasting_an_empty_clipboard_leaves_the_query_unchanged() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let mut clipboard = FakeClipboard::default();
        let mut file_dialog = FakeFileDialog::default();

        handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(
                Modifiers {
                    control: true,
                    ..Modifiers::none()
                },
                Key::Character('v'),
            ),
            None,
            &mut clipboard,
            &mut file_dialog,
        );

        let Some(root) = palette else {
            unreachable!("pasting must not close the palette");
        };
        let state = match aurora_widgets::widgets::command_palette_state(&workspace.tree, root) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(state.query(), "");
    }

    #[test]
    fn activating_open_file_returns_the_picked_path() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let mut clipboard = FakeClipboard::default();
        let mut file_dialog = FakeFileDialog {
            next_pick: Some(PathBuf::from("/tmp/example.psd")),
            ..FakeFileDialog::default()
        };

        for ch in "open file".chars() {
            handle_palette_key(
                &mut workspace,
                &mut focus,
                &mut palette,
                KeyChord::new(Modifiers::none(), Key::Character(ch)),
                Some(&ch.to_string()),
                &mut clipboard,
                &mut file_dialog,
            );
        }
        let Some(root) = palette else {
            unreachable!("typing must not close the palette");
        };
        let state = match aurora_widgets::widgets::command_palette_state(&workspace.tree, root) {
            Ok(state) => state,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            state.selected().map(|entry| entry.id.as_str()),
            Some(COMMAND_FILE_OPEN),
            "sanity check: the query must actually narrow to the Open File command"
        );

        let picked = handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Enter)),
            None,
            &mut clipboard,
            &mut file_dialog,
        );

        assert_eq!(
            picked,
            Some(ActivatedCommand::OpenFile(PathBuf::from(
                "/tmp/example.psd"
            )))
        );
        assert_eq!(palette, None, "activating a command must close the palette");
    }

    #[test]
    fn cancelling_the_file_dialog_returns_none_and_still_closes_the_palette() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut palette = None;
        open_command_palette(&mut workspace, &mut focus, &mut palette);
        let mut clipboard = FakeClipboard::default();
        // `next_pick: None` -- simulates the user cancelling the native
        // dialog rather than picking a file.
        let mut file_dialog = FakeFileDialog::default();

        for ch in "open file".chars() {
            handle_palette_key(
                &mut workspace,
                &mut focus,
                &mut palette,
                KeyChord::new(Modifiers::none(), Key::Character(ch)),
                Some(&ch.to_string()),
                &mut clipboard,
                &mut file_dialog,
            );
        }
        let picked = handle_palette_key(
            &mut workspace,
            &mut focus,
            &mut palette,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Enter)),
            None,
            &mut clipboard,
            &mut file_dialog,
        );

        assert_eq!(picked, None);
        assert_eq!(palette, None, "must still close, even on a cancelled pick");
    }

    // -- shared command activation (palette + native menu) --

    #[test]
    fn activate_command_focuses_the_matching_panel_for_every_known_id() {
        fn check(id: &str, expected: impl Fn(&aurora_ui::Workspace) -> WidgetId) {
            let mut workspace = aurora_ui::build_workspace();
            let mut focus = FocusManager::default();
            let mut file_dialog = FakeFileDialog::default();
            let expected = expected(&workspace);

            let picked = activate_command(&mut workspace, &mut focus, id, &mut file_dialog);

            assert_eq!(picked, None);
            assert_eq!(focus.focused(), Some(expected));
        }

        check(COMMAND_FOCUS_LAYERS, |workspace| workspace.layers.root);
        check(COMMAND_FOCUS_PROPERTIES, |workspace| {
            workspace.properties.root
        });
        check(COMMAND_FOCUS_HISTORY, |workspace| workspace.history.root);
    }

    #[test]
    fn activate_command_toggles_collapse_for_every_known_toggle_id() {
        fn check(id: &str, expected: impl Fn(&aurora_ui::Workspace) -> aurora_ui::PanelHandle) {
            let mut workspace = aurora_ui::build_workspace();
            let mut focus = FocusManager::default();
            let mut file_dialog = FakeFileDialog::default();
            let panel = expected(&workspace);
            match aurora_ui::panel_is_collapsed(&workspace.tree, panel) {
                Ok(collapsed) => assert!(!collapsed, "starts expanded"),
                Err(err) => unreachable!("{err:?}"),
            }

            let picked = activate_command(&mut workspace, &mut focus, id, &mut file_dialog);

            assert_eq!(picked, None);
            match aurora_ui::panel_is_collapsed(&workspace.tree, panel) {
                Ok(collapsed) => assert!(collapsed, "one toggle must collapse it"),
                Err(err) => unreachable!("{err:?}"),
            }

            let picked_again = activate_command(&mut workspace, &mut focus, id, &mut file_dialog);
            assert_eq!(picked_again, None);
            match aurora_ui::panel_is_collapsed(&workspace.tree, panel) {
                Ok(collapsed) => assert!(!collapsed, "a second toggle must expand it back"),
                Err(err) => unreachable!("{err:?}"),
            }
        }

        check(COMMAND_TOGGLE_LAYERS, |workspace| workspace.layers);
        check(COMMAND_TOGGLE_PROPERTIES, |workspace| workspace.properties);
        check(COMMAND_TOGGLE_HISTORY, |workspace| workspace.history);
    }

    #[test]
    fn activate_command_closes_the_matching_panel_for_every_known_close_id() {
        fn check(id: &str, expected: impl Fn(&aurora_ui::Workspace) -> aurora_ui::PanelHandle) {
            let mut workspace = aurora_ui::build_workspace();
            let mut focus = FocusManager::default();
            let mut file_dialog = FakeFileDialog::default();
            let panel = expected(&workspace);
            if let Err(err) = aurora_widgets::widgets::insert_container(
                &mut workspace.tree,
                panel.body,
                taffy::Style::default(),
            ) {
                unreachable!("{err:?}");
            }

            let picked = activate_command(&mut workspace, &mut focus, id, &mut file_dialog);

            assert_eq!(picked, None);
            match aurora_ui::panel_is_collapsed(&workspace.tree, panel) {
                Ok(collapsed) => assert!(collapsed, "closing must also collapse"),
                Err(err) => unreachable!("{err:?}"),
            }
            assert_eq!(
                workspace.tree.children(panel.body),
                Some([].as_slice()),
                "closing must really empty the body, not just hide it"
            );
        }

        check(COMMAND_CLOSE_LAYERS, |workspace| workspace.layers);
        check(COMMAND_CLOSE_PROPERTIES, |workspace| workspace.properties);
        check(COMMAND_CLOSE_HISTORY, |workspace| workspace.history);
    }

    #[test]
    fn palette_commands_includes_a_close_for_every_panel() {
        let commands = palette_commands();
        let ids: Vec<&str> = commands.iter().map(|entry| entry.id.as_str()).collect();
        assert!(ids.contains(&COMMAND_CLOSE_LAYERS), "{ids:?}");
        assert!(ids.contains(&COMMAND_CLOSE_PROPERTIES), "{ids:?}");
        assert!(ids.contains(&COMMAND_CLOSE_HISTORY), "{ids:?}");
    }

    #[test]
    fn activate_command_returns_the_picked_path_for_file_open() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut file_dialog = FakeFileDialog {
            next_pick: Some(PathBuf::from("/tmp/example.psd")),
            ..FakeFileDialog::default()
        };

        let picked = activate_command(
            &mut workspace,
            &mut focus,
            COMMAND_FILE_OPEN,
            &mut file_dialog,
        );

        assert_eq!(
            picked,
            Some(ActivatedCommand::OpenFile(PathBuf::from(
                "/tmp/example.psd"
            )))
        );
        assert_eq!(
            focus.focused(),
            None,
            "COMMAND_FILE_OPEN has no focus target of its own"
        );
    }

    #[test]
    fn activate_command_returns_the_picked_path_for_file_save() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut file_dialog = FakeFileDialog {
            next_save: Some(PathBuf::from("/tmp/example.png")),
            ..FakeFileDialog::default()
        };

        let picked = activate_command(
            &mut workspace,
            &mut focus,
            COMMAND_FILE_SAVE,
            &mut file_dialog,
        );

        assert_eq!(
            picked,
            Some(ActivatedCommand::SaveFile(PathBuf::from(
                "/tmp/example.png"
            )))
        );
        assert_eq!(
            focus.focused(),
            None,
            "COMMAND_FILE_SAVE has no focus target of its own"
        );
    }

    #[test]
    fn activate_command_resolves_undo_and_redo_without_focusing_anything() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut file_dialog = FakeFileDialog::default();

        let undo = activate_command(&mut workspace, &mut focus, COMMAND_UNDO, &mut file_dialog);
        assert_eq!(undo, Some(ActivatedCommand::Undo));

        let redo = activate_command(&mut workspace, &mut focus, COMMAND_REDO, &mut file_dialog);
        assert_eq!(redo, Some(ActivatedCommand::Redo));

        assert_eq!(
            focus.focused(),
            None,
            "neither command has a focus target of its own"
        );
    }

    #[test]
    fn palette_commands_includes_undo_and_redo() {
        let commands = palette_commands();
        let ids: Vec<&str> = commands.iter().map(|entry| entry.id.as_str()).collect();
        assert!(ids.contains(&COMMAND_UNDO), "{ids:?}");
        assert!(ids.contains(&COMMAND_REDO), "{ids:?}");
    }

    #[test]
    fn palette_commands_includes_a_toggle_for_every_panel() {
        let commands = palette_commands();
        let ids: Vec<&str> = commands.iter().map(|entry| entry.id.as_str()).collect();
        assert!(ids.contains(&COMMAND_TOGGLE_LAYERS), "{ids:?}");
        assert!(ids.contains(&COMMAND_TOGGLE_PROPERTIES), "{ids:?}");
        assert!(ids.contains(&COMMAND_TOGGLE_HISTORY), "{ids:?}");
    }

    #[test]
    fn activate_command_returns_none_for_a_cancelled_save_dialog() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        // `next_save: None` -- simulates the user cancelling the native
        // dialog rather than picking a destination.
        let mut file_dialog = FakeFileDialog::default();

        let picked = activate_command(
            &mut workspace,
            &mut focus,
            COMMAND_FILE_SAVE,
            &mut file_dialog,
        );

        assert_eq!(picked, None);
    }

    #[test]
    fn activate_command_returns_none_and_focuses_nothing_for_an_unknown_id() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut file_dialog = FakeFileDialog::default();

        let picked = activate_command(
            &mut workspace,
            &mut focus,
            "bogus.command",
            &mut file_dialog,
        );

        assert_eq!(picked, None);
        assert_eq!(focus.focused(), None);
    }

    // -- native menu bar (macOS only) --
    //
    // No `#[test]` here for `build_menu`, deliberately: real macOS CI
    // (2026-08-05) found that `muda::Menu::new()` panics with "can only
    // be created on the main thread" when called from a
    // `cargo nextest run` worker -- and this isn't a nextest quirk to
    // work around. Neither `nextest` nor libtest's own default harness
    // ever runs an individual `#[test]` fn on the process's actual main
    // thread (both dispatch to worker threads even at
    // `--test-threads=1`), so no combination of test attributes or
    // flags makes a `muda`-constructing test satisfy this constraint --
    // it needs a real, separate test binary invoked directly, which
    // this workspace's test infrastructure doesn't build. `build_menu`
    // itself remains real production code (called from `App::new` on
    // the winit event loop's own main thread, where this constraint is
    // naturally satisfied); it's just unreachable from this crate's
    // `#[test]` suite. See PLAN.md M1.8's own note on this finding.

    // -- crash recovery --
    //
    // The marker-file functions do real filesystem I/O, so they're
    // tested against a real `tempfile::TempDir` rather than mocked --
    // consistent with how the rest of this session has preferred real
    // I/O over mocks (`aurora-testkit`'s golden-image tests are the
    // nearest precedent). The dialog/dispatch functions are pure
    // `WidgetTree` logic, same as the command-palette tests above.

    #[test]
    fn load_scales_resolves_the_checked_in_design_file() {
        if let Err(err) = load_scales() {
            unreachable!("the checked-in design file must parse: {err}");
        }
    }

    #[test]
    fn a_marker_that_was_never_written_is_not_seen_as_a_previous_session() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-session.marker");
        assert!(!previous_session_left_a_marker(&path));
    }

    #[test]
    fn writing_then_checking_the_marker_reports_a_previous_session() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-session.marker");
        write_session_marker(&path);
        assert!(previous_session_left_a_marker(&path));
    }

    #[test]
    fn clearing_the_marker_makes_it_look_like_a_clean_shutdown_again() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-session.marker");
        write_session_marker(&path);
        clear_session_marker(&path);
        assert!(!previous_session_left_a_marker(&path));
    }

    #[test]
    fn clearing_a_marker_that_was_never_written_does_not_panic() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-session.marker");
        clear_session_marker(&path);
        assert!(!previous_session_left_a_marker(&path));
    }

    #[test]
    fn autosave_path_and_marker_path_are_distinct() {
        // Both live under `std::env::temp_dir()` -- must not collide with
        // each other or overwrite the wrong file.
        assert_ne!(autosave_path(), super::marker_path());
    }

    #[test]
    fn tile_store_scratch_dir_is_distinct_from_the_marker_and_autosave_paths() {
        // Since 0.53.0 the scratch directory is per-session and randomly
        // named rather than the fixed `aurora-tiles`, so this can no
        // longer collide by construction -- still asserted, because all
        // three still live under `std::env::temp_dir()` and the marker
        // and autosave paths *are* still fixed.
        let Some(scratch) = tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        assert_ne!(scratch, super::marker_path());
        assert_ne!(scratch, autosave_path());
    }

    #[test]
    fn open_tile_store_succeeds_against_the_real_scratch_directory() {
        // A real, if unremarkable, assertion: this crate's own scratch
        // directory (per-session since 0.53.0, created on first use) is
        // always creatable and writable in a real environment (the same
        // assumption `write_session_marker`'s own `std::env::temp_dir()`
        // use already makes) -- confirms `open_tile_store` doesn't
        // always return `None` in ordinary conditions, not this
        // function's own I/O error path (real disk-failure injection is
        // not something this sandbox can do).
        assert!(open_tile_store().is_some());
    }

    /// Two calls must not hand back the same directory -- the whole
    /// point of 0.53.0's change away from the one fixed `aurora-tiles`
    /// path every process and every user shared.
    #[test]
    fn each_scratch_directory_is_a_new_one() {
        let (Some(first), Some(second)) = (
            create_tile_store_scratch_dir(),
            create_tile_store_scratch_dir(),
        ) else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        assert_ne!(first, second);
        assert!(first.is_dir());
        assert!(second.is_dir());
        let fixed = std::env::temp_dir().join("aurora-tiles");
        assert_ne!(first, fixed);
        assert_ne!(second, fixed);
        // These two are deliberately *not* the memoized session
        // directory, so nothing else is using them -- clean up rather
        // than leaving two directories behind per test run.
        for dir in [first, second] {
            if let Err(err) = std::fs::remove_dir_all(&dir) {
                unreachable!("a directory this test just created must be removable: {err}");
            }
        }
    }

    /// A freshly created scratch directory must *already* be owner-only,
    /// before any `aurora_tile::TileStore` has been opened in it.
    ///
    /// Not redundant with
    /// [`the_session_scratch_directory_is_owner_only`]: `tempfile`
    /// creates directories with default (umask-derived, typically
    /// world-readable) permissions, and this was observed on disk at
    /// `0o775` when the only store opened during a run was the `.aur`
    /// verifier's, which lives in a *child* of this directory and so
    /// never re-asserts the parent's mode.
    #[cfg(unix)]
    #[test]
    fn a_fresh_scratch_directory_is_owner_only_before_any_store_opens() {
        use std::os::unix::fs::PermissionsExt as _;

        let Some(dir) = create_tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let mode = match std::fs::metadata(&dir) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(err) => unreachable!("the directory was just created: {err}"),
        };
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            unreachable!("a directory this test just created must be removable: {err}");
        }
        // The security property, not the exact mode. This directory's
        // permissions come from `tempfile`'s `mkdir` mode argument,
        // which *is* masked by the process umask -- unlike
        // `set_permissions`, which is not, and which is why
        // [`the_session_scratch_directory_is_owner_only`] can afford an
        // exact `0o700`. Under a umask that clears owner bits (`umask
        // 0177`, say) this would land at `0o500` and an `assert_eq!`
        // would fail on a directory that is *more* private than
        // required, not less.
        assert_eq!(
            mode & 0o077,
            0,
            "a fresh scratch directory must grant nothing to group or other (mode {mode:o})"
        );
    }

    /// The directory holds the document's real unsaved pixels, in a
    /// world-readable temp directory. `aurora_tile::TileStore::new` is
    /// what re-asserts the mode when a store opens directly in it; this
    /// asserts the app actually gets that benefit end to end.
    ///
    /// Takes `AUR_VERIFY_SCRATCH_LOCK` because
    /// `aur_verification_survives_the_session_scratch_directory_being_swept_away`
    /// deletes this same live, memoized session directory under that
    /// lock -- without it, `cargo test --workspace`'s shared-binary,
    /// multi-threaded run (unlike `cargo nextest`'s process-per-test
    /// isolation, which CI actually uses) can observe the directory gone
    /// between this test's own `open_tile_store()` and `metadata()`.
    #[cfg(unix)]
    #[test]
    fn the_session_scratch_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let _guard = AUR_VERIFY_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        assert!(open_tile_store().is_some());
        let Some(dir) = tile_store_scratch_dir() else {
            unreachable!("open_tile_store just succeeded against it");
        };
        let mode = match std::fs::metadata(dir) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(err) => unreachable!("the directory was just used by a live store: {err}"),
        };
        assert_eq!(mode, 0o700);
    }

    /// Load-bearing, not a tautology: the store is legitimately reopened
    /// mid-run (`startup_document`,
    /// `recover_partial_after_a_failed_read`), and each reopen must land
    /// in the *same* directory -- a fresh one per call would strand the
    /// previous one on disk with nothing left holding its path.
    #[test]
    fn tile_store_scratch_dir_is_stable_within_one_process() {
        assert_eq!(tile_store_scratch_dir(), tile_store_scratch_dir());
    }

    #[test]
    fn removing_the_session_scratch_directory_removes_its_tiles_too() {
        // Deliberately a throwaway directory, not the live memoized
        // session one: removing the real one would pull the scratch
        // disk out from under every other test sharing this binary.
        let Some(dir) = create_tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let tile = dir.join("0-0-0_0_0_0.tile");
        if let Err(err) = std::fs::write(&tile, [0_u8; 4]) {
            unreachable!("a fresh scratch directory must be writable: {err}");
        }
        super::remove_scratch_dir(&dir);
        assert!(!tile.exists(), "the paged-out tiles go with the directory");
        assert!(!dir.exists());

        // And it tolerates an absent directory -- the "shutting down
        // twice" / "never created one" case, the same shape
        // `clear_session_marker` and `remove_autosave` already accept.
        // Called for its (lack of) panic, not a value.
        super::remove_scratch_dir(&dir);
    }

    /// Serializes the tests that count what `.aur` verification leaves
    /// behind in the session scratch directory. `verify_aur` creates its
    /// directory *inside* the one live, memoized session directory, so
    /// two verifications running concurrently in this binary would see
    /// each other's in-flight directory and make the count
    /// non-deterministic.
    static AUR_VERIFY_SCRATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `(subdirectories, files under them)` sitting in the session
    /// scratch directory right now.
    ///
    /// Deliberately every *subdirectory*, not just ones matching some
    /// `aur-verify-` name: `.aur` verification is the only thing that
    /// ever nests a directory inside the session directory, so this
    /// counts what verification left behind however that directory
    /// comes to be named — including the pre-0.53.0 shape, a single
    /// fixed child reused by every save, whose file count is what grows
    /// there. The session directory's own top-level `*.tile` files
    /// belong to the live store other tests share and are not counted.
    fn aur_verify_leftovers() -> (usize, usize) {
        fn count_files(dir: &std::path::Path) -> usize {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            entries
                .flatten()
                .map(|entry| {
                    if entry.path().is_dir() {
                        count_files(&entry.path())
                    } else {
                        1
                    }
                })
                .sum()
        }

        let Some(session) = tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let entries = match std::fs::read_dir(session) {
            Ok(entries) => entries,
            Err(err) => unreachable!("the session scratch directory is readable: {err}"),
        };
        let mut dirs = 0;
        let mut files = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            dirs += 1;
            files += count_files(&path);
        }
        (dirs, files)
    }

    /// The `.aur` verifier's scratch store must not leak its paged-out
    /// tiles across saves.
    ///
    /// Every `verify_aur` call builds a fresh `aurora_tile::TileStore`,
    /// and since 0.53.0 every store folds a per-instance token into
    /// every filename it writes. A directory shared across calls would
    /// therefore gain a *new* full set of evicted tiles per save, with
    /// nothing ever deleting them (`TileStore` has no `Drop` that
    /// removes its own files) — a document saved repeatedly in one
    /// session would grow an unbounded pile of full-resolution
    /// compressed pixel data until the whole session directory went
    /// away at shutdown.
    ///
    /// The document here is deliberately larger than the verifier's own
    /// 16-tile budget (1280 × 1280 px is 5 × 5 = 25 tiles at ADR 0005's
    /// 256 px tile), so verification really does evict to disk. A test
    /// on a small document would pass against the leaking version and
    /// prove nothing.
    #[test]
    fn repeated_aur_verification_does_not_accumulate_scratch_tiles() {
        /// 5 x 5 = 25 tiles at ADR 0005's 256 px tile, against
        /// `verify_aur`'s own 16-tile budget.
        const SIDE: u32 = 1280;

        let _guard = AUR_VERIFY_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let image = fake_image(SIDE, SIDE);
        let (layers, history, id) = document_from_image("big", &image);
        // Real painted tiles, not just a layer of the right size:
        // `document_from_image` only adds the layer, and a `.aur` whose
        // manifest names no tiles pages nothing in when it is read back,
        // so the store under test would never evict and the leak would
        // not reproduce.
        let surface = super::surface_id_for(id);
        let tiles = SIDE.div_ceil(aurora_tile::TILE);
        for y in 0..tiles {
            for x in 0..tiles {
                let tile_id = aurora_tile::TileId { x, y };
                let tile = match store.get_mut(surface, tile_id) {
                    Ok(tile) => tile,
                    Err(err) => unreachable!("touching a blank tile cannot fail: {err:?}"),
                };
                let Some(sample) = tile.texels_mut().first_mut() else {
                    unreachable!("a full tile has texels");
                };
                *sample = half::f16::from_f32(0.5);
            }
        }
        assert!(
            tiles * tiles > 16,
            "the document must exceed the verifier's own tile budget"
        );
        let path = dir.path().join("big.aur");
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) =
            aurora_io::write_aur(file, &layers, &history, (SIDE, SIDE), None, &mut store)
        {
            unreachable!("{err:?}");
        }

        let baseline = aur_verify_leftovers();
        assert_eq!(
            baseline,
            (0, 0),
            "nothing else may be mid-verification while this test holds the lock"
        );

        let mut after = Vec::new();
        for _ in 0..3 {
            assert!(
                verify_aur(&path),
                "a real, just-written .aur file must verify"
            );
            after.push(aur_verify_leftovers());
        }
        assert_eq!(
            after,
            vec![(0, 0), (0, 0), (0, 0)],
            "each verification must take its own scratch directory with it; leftovers that grow \
             per save are the leak this test exists for"
        );
    }

    /// The `.aur` verifier's scratch directory must live *under* the
    /// per-session one, and must not be the pre-0.53.0 fixed,
    /// cross-process path.
    ///
    /// Not a tautology: a review round demonstrated that reverting this
    /// half of the fix — putting the verifier back on
    /// `std::env::temp_dir().join("aurora-aur-verify")`, a second fixed,
    /// world-readable directory shared by every process and every user
    /// on the machine — passed the entire gate with nothing failing.
    /// This is the assertion that would have caught it, and it mirrors
    /// [`each_scratch_directory_is_a_new_one`]'s own `assert_ne!`
    /// against the live store's old fixed path.
    #[test]
    fn the_aur_verify_scratch_directory_is_nested_and_not_the_old_fixed_path() {
        let _guard = AUR_VERIFY_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let Some(session) = tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let Some(first) = aur_verify_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let Some(second) = aur_verify_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let fixed = std::env::temp_dir().join("aurora-aur-verify");
        for dir in [first.path(), second.path()] {
            assert!(dir.is_dir());
            assert!(
                dir.starts_with(session),
                "the verifier's scratch directory must be a child of the session directory \
                 ({}), not a sibling or a fixed path of its own: {}",
                session.display(),
                dir.display()
            );
            assert_ne!(dir, fixed);
        }
        // Per call, not per session -- the whole reason the leak above
        // cannot come back by sharing one directory across saves.
        assert_ne!(first.path(), second.path());

        // And the `TempDir` guard is what deletes it: dropping must
        // leave nothing behind.
        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists(), "dropping the guard removes the directory");
        drop(second);
    }

    /// The three things a clean shutdown must undo, exercised as a unit.
    ///
    /// The `WindowEvent::CloseRequested` arm that calls this needs a
    /// real `winit` event loop, so no test can execute the arm itself —
    /// a review round showed that deleting the scratch-directory
    /// cleanup from it left every test green. Each step is asserted here
    /// against throwaway paths (never the live session's, which every
    /// other test in this binary shares), so only the single call in the
    /// handler is left to inspection.
    #[test]
    fn clean_shutdown_cleanup_removes_the_marker_the_autosave_and_the_scratch_tiles() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let marker = dir.path().join("aurora-session.marker");
        if let Err(err) = std::fs::write(&marker, b"") {
            unreachable!("a fresh tempdir must be writable: {err}");
        }
        let autosave = dir.path().join("aurora-autosave.aur");
        if let Err(err) = std::fs::write(&autosave, b"") {
            unreachable!("a fresh tempdir must be writable: {err}");
        }

        // A real, live store with a real paged-out tile in a real
        // scratch directory -- not an empty directory, which would pass
        // even if the removal never ran against anything.
        let Some(scratch) = create_tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        let Some(budget) = std::num::NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(scratch.clone(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("a fresh scratch directory must be usable: {err}"),
        };
        let surface = aurora_tile::SurfaceId::from_raw(0);
        for id in [
            aurora_tile::TileId { x: 0, y: 0 },
            aurora_tile::TileId { x: 1, y: 0 },
        ] {
            if let Err(err) = store.get_mut(surface, id) {
                unreachable!("touching a blank tile cannot fail: {err}");
            }
        }
        if let Err(err) = store.flush() {
            unreachable!("a test-local scratch disk must accept the write: {err}");
        }
        let tiles = match std::fs::read_dir(&scratch) {
            Ok(entries) => entries.flatten().count(),
            Err(err) => unreachable!("the scratch directory is readable: {err}"),
        };
        assert!(
            tiles > 0,
            "this test's premise is a scratch directory with real paged-out tiles in it"
        );

        let mut state = FakeShutdownState {
            marker: marker.clone(),
            autosave: autosave.clone(),
            store: Some(store),
            scratch: Some(scratch.clone()),
        };
        clean_shutdown_cleanup(&mut state);
        assert!(
            state.store.is_none(),
            "the store must be taken out of the slot and dropped, not left alive holding a \
             writer thread against a directory that is being deleted"
        );

        assert!(!marker.exists(), "the session marker must be cleared");
        assert!(!autosave.exists(), "the autosave must be removed");
        assert!(
            !scratch.exists(),
            "the session's scratch directory and its unsaved pixels must be removed"
        );
    }

    /// [`ShutdownState`]'s test double — the four things a clean
    /// shutdown reads out of the running application, backed by
    /// throwaway paths instead of the live session's.
    struct FakeShutdownState {
        marker: PathBuf,
        autosave: PathBuf,
        store: Option<aurora_tile::TileStore>,
        scratch: Option<PathBuf>,
    }

    impl ShutdownState for FakeShutdownState {
        fn marker_path(&self) -> &std::path::Path {
            &self.marker
        }

        fn autosave_path(&self) -> PathBuf {
            self.autosave.clone()
        }

        fn take_tile_store(&mut self) -> Option<aurora_tile::TileStore> {
            self.store.take()
        }

        fn scratch_dir(&self) -> Option<&std::path::Path> {
            self.scratch.as_deref()
        }
    }

    /// A session scratch directory swept out from under a running
    /// process must not silently discard every save for the rest of the
    /// run.
    ///
    /// `tile_store_scratch_dir` memoizes a *path*, not a directory that
    /// is guaranteed to still exist: a temp cleaner sweeping `/tmp`, or
    /// a user clearing temp files, deletes it mid-session. Nesting the
    /// verifier's scratch directory under it (0.53.0) made
    /// `tempdir_in` fail in that state, `verify_aur` return `false`,
    /// and `App::save_aur_file` respond by deleting the export it had
    /// just written — the save gone, with nothing but a `tracing::warn!`
    /// to show for it, for every save afterwards too. CLAUDE.md names
    /// silently degrading a professional's file as the worst failure
    /// this project can have.
    #[test]
    fn aur_verification_survives_the_session_scratch_directory_being_swept_away() {
        // Deliberately removes the one live session directory, so it
        // takes the same lock the other verification tests do. The
        // directory is recreated by the very call under test, so the
        // window in which it is absent is a single `verify_aur` call.
        let _guard = AUR_VERIFY_SCRATCH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let image = fake_image(4, 4);
        let (layers, history, _id) = document_from_image("photo", &image);
        let path = dir.path().join("real.aur");
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = aurora_io::write_aur(file, &layers, &history, (4, 4), None, &mut store) {
            unreachable!("{err:?}");
        }

        let Some(session) = tile_store_scratch_dir() else {
            unreachable!("a scratch directory is always creatable in a real test environment");
        };
        super::remove_scratch_dir(session);
        assert!(
            !session.exists(),
            "this test's premise is a session directory that has been swept away"
        );

        assert!(
            verify_aur(&path),
            "a real, just-written .aur file must still verify after the session scratch \
             directory has been swept away -- returning false here deletes the user's export"
        );
        assert!(
            session.is_dir(),
            "the session directory must be recreated, not merely worked around once"
        );
        // And it is owner-only again, not whatever the umask says.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = match std::fs::metadata(session) {
                Ok(meta) => meta.permissions().mode() & 0o777,
                Err(err) => unreachable!("the directory was just recreated: {err}"),
            };
            assert_eq!(
                mode & 0o077,
                0,
                "a recreated session directory holds the same unsaved pixels the original did \
                 (mode {mode:o})"
            );
        }
    }

    /// The "nothing to clean up" shape, which a clean shutdown really
    /// can reach: no scratch directory was ever created (painting was
    /// disabled for the session), and the marker/autosave are already
    /// gone. Called for its lack of panic, and to pin that an absent
    /// path is not treated as an error.
    #[test]
    fn clean_shutdown_cleanup_tolerates_having_nothing_to_remove() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let marker = dir.path().join("never-written.marker");
        let autosave = dir.path().join("never-written.aur");
        let mut state = FakeShutdownState {
            marker: marker.clone(),
            autosave: autosave.clone(),
            store: None,
            scratch: None,
        };
        clean_shutdown_cleanup(&mut state);
        assert!(!marker.exists());
        assert!(!autosave.exists());
    }

    #[test]
    fn layer_local_point_subtracts_the_layers_own_origin() {
        let bounds = aurora_core::Rect {
            x: 100,
            y: 50,
            width: 10,
            height: 10,
        };
        assert_eq!(layer_local_point(bounds, (110.0, 60.0)), (10.0, 10.0));
    }

    #[test]
    fn layer_local_point_is_identity_for_an_origin_at_zero() {
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(layer_local_point(bounds, (5.0, 7.0)), (5.0, 7.0));
    }

    /// Serializes every real-GPU test in this module — mirrors
    /// `aurora-gpu`'s own `test_support::GPU_TEST_LOCK`, which found
    /// this necessary (concurrent real-device creation reproducibly
    /// deadlocked under plain `cargo test`); this crate's own tests are
    /// a separate binary from that crate's, so its lock doesn't cover
    /// this process too.
    static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct GpuTestContext {
        _guard: std::sync::MutexGuard<'static, ()>,
        context: aurora_gpu::GpuContext,
    }

    impl std::ops::Deref for GpuTestContext {
        type Target = aurora_gpu::GpuContext;
        fn deref(&self) -> &aurora_gpu::GpuContext {
            &self.context
        }
    }

    /// `NoSuitableAdapter` is an inconclusive skip (this sandbox/CI
    /// runner may genuinely have no usable GPU); any other error means
    /// an adapter *was* found but device/queue creation failed, a real
    /// bug worth a hard test failure — same distinction
    /// `aurora-gpu::test_support::real_context` already draws.
    fn real_gpu_context() -> Option<GpuTestContext> {
        let guard = GPU_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match aurora_gpu::GpuContext::new() {
            Ok(context) => Some(GpuTestContext {
                _guard: guard,
                context,
            }),
            Err(aurora_gpu::GpuError::NoSuitableAdapter) => {
                eprintln!("SKIPPED: no GPU adapter available on this machine/CI runner");
                None
            }
            Err(err) => {
                #[allow(clippy::panic)]
                {
                    panic!("device request failed with a real adapter present: {err}");
                }
            }
        }
    }

    fn real_tile_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        (dir, store)
    }

    #[test]
    fn composite_surface_id_never_collides_with_a_real_layers_surface() {
        let mut layers = aurora_doc::LayerTree::new();
        let id = match layers.add_pixel_layer(
            "a",
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_ne!(layers.surface_id(id), Some(composite_surface_id()));
    }

    /// Writes `rgba` into every texel of `tile` on `surface`, marking it
    /// dirty — the same shape `aurora-gpu`'s own `residency::tests::paint`
    /// helper already uses.
    fn fill_solid(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
        tile: aurora_tile::TileId,
        rgba: [f32; 4],
    ) {
        let Ok(t) = store.get_mut(surface, tile) else {
            unreachable!("a real store must accept this write");
        };
        for (i, sample) in t.texels_mut().iter_mut().enumerate() {
            let Some(&channel) = rgba.get(i % 4) else {
                unreachable!("i % 4 is always in range 0..4");
            };
            *sample = half::f16::from_f32(channel);
        }
        t.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }

    /// Reads back `tile`'s own first texel as `(r, g, b, a)` floats.
    // The clearest names for one pixel's own RGBA sample, the same
    // justification `sample_pixel` already uses for the same lint.
    #[allow(clippy::many_single_char_names)]
    fn read_first_texel(
        store: &mut aurora_tile::TileStore,
        surface: aurora_tile::SurfaceId,
        tile: aurora_tile::TileId,
    ) -> (f32, f32, f32, f32) {
        let Ok(t) = store.get(surface, tile) else {
            unreachable!("just written");
        };
        let texels = t.texels();
        let (Some(r), Some(g), Some(b), Some(a)) =
            (texels.first(), texels.get(1), texels.get(2), texels.get(3))
        else {
            unreachable!("a real tile always has at least one full texel");
        };
        (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32())
    }

    /// Renders `residency` through `canvas`/`pipeline` into a
    /// `viewport`-sized offscreen target and reads back one pixel — this
    /// crate's own real render+readback flow (the same shape
    /// `aurora-gpu`'s own `render_test::render_and_sample_pixel` uses,
    /// duplicated here rather than shared since that helper is private
    /// to that crate's own test binary and this crate's tests are a
    /// separate binary).
    #[allow(clippy::too_many_arguments)]
    fn render_and_sample_pixel(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &mut aurora_gpu::CanvasPipeline,
        residency: &aurora_gpu::TileResidency,
        viewport: (u32, u32),
        sample: (u32, u32),
    ) -> [u8; 4] {
        let bind_group = canvas.bind_group(device, residency);
        let pipeline = canvas.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("effective-zoom-target"),
            size: wgpu::Extent3d {
                width: viewport.0,
                height: viewport.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("effective-zoom-render"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effective-zoom-canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        let bytes_per_row = viewport.0 * 4;
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("effective-zoom-readback"),
            size: u64::from(bytes_per_row) * u64::from(viewport.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(viewport.1),
                },
            },
            wgpu::Extent3d {
                width: viewport.0,
                height: viewport.1,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = readback_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let Ok(Ok(())) = rx.recv() else {
            unreachable!("map_async must complete once the device has been polled to idle");
        };
        let Ok(data) = slice.get_mapped_range() else {
            unreachable!("the buffer was just confirmed mapped successfully above");
        };
        let (sx, sy) = sample;
        let offset = (sy as usize) * (bytes_per_row as usize) + (sx as usize) * 4;
        let Some(pixel) = data.get(offset..offset + 4) else {
            unreachable!("sample is well within the readback buffer's own bounds");
        };
        let result = match pixel {
            &[r, g, b, a] => [r, g, b, a],
            _ => unreachable!("sliced exactly 4 bytes"),
        };
        drop(data);
        readback_buffer.unmap();
        result
    }

    #[test]
    /// The real, end-to-end proof of this crate's own Retina/HiDPI fix
    /// (`effective_residency_zoom`, used at `redraw`'s one real
    /// `residency.set_origin` call site): not just that the arithmetic
    /// works out on paper, but that feeding the *effective* zoom into a
    /// real `aurora_gpu::TileResidency`/`CanvasPipeline` render actually
    /// samples the pixels `aurora_ui::CanvasView`'s own "one document
    /// pixel = one logical pixel" contract promises, where the raw,
    /// unscaled `CanvasView::zoom()` alone does not.
    ///
    /// Simulates a real Retina window: a notional logical canvas of
    /// 256x256 (one tile) with `scale_factor = 2.0`, so the real
    /// physical viewport `redraw` actually builds
    /// (`canvas_area_physical_size`) is 512x512 — exactly the scenario
    /// from Cahya's real bug report on real Retina macOS hardware, not a
    /// synthetic zoom level. Two adjacent, differently-coloured tiles
    /// (green at `(0, 0)`, red at `(1, 0)`), the same "sample near the
    /// right edge" technique `aurora-gpu`'s own
    /// `canvas_pipeline_reflects_zoom_by_magnifying_the_atlas` uses.
    ///
    /// At the *raw*, unscaled `canvas_view.zoom()` (1.0) fed directly
    /// into `set_origin` alongside the 512x512 physical viewport — the
    /// bug, before this fix — `uv_scale` spans both tiles across the
    /// viewport (twice the document content `CanvasView`'s own logical
    /// contract promises for that physical size), so the sample point
    /// lands in the *red* tile. At the *effective* zoom
    /// (`effective_residency_zoom(1.0, 2.0) == 2.0`) — the fix —
    /// `uv_scale` halves, showing only the single green tile's own
    /// document-pixel extent stretched across the same physical
    /// viewport, exactly matching what a real 256x256-logical Retina
    /// canvas at 100% zoom should show, so the same sample point lands
    /// back in *green*. This is the exact assertion that would flip if
    /// `redraw` regressed to passing `self.canvas_view.zoom()` directly
    /// instead of `effective_residency_zoom(...)`.
    #[allow(clippy::too_many_lines)]
    fn effective_residency_zoom_fixes_the_real_retina_sampling_bug() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let device = context.device();
        let queue = context.queue();
        let (_dir, mut store) = real_tile_store();
        let surface = composite_surface_id();

        fill_solid(
            &mut store,
            surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [0.0, 1.0, 0.0, 1.0],
        );
        fill_solid(
            &mut store,
            surface,
            aurora_tile::TileId { x: 1, y: 0 },
            [1.0, 0.0, 0.0, 1.0],
        );

        // The real physical viewport `canvas_area_physical_size` would
        // compute for a 256x256 logical canvas at a real Retina
        // `scale_factor` of 2.0.
        let physical_viewport = (512, 512);
        let mut residency = aurora_gpu::TileResidency::new(device, queue, physical_viewport);
        let stats = residency.sync(queue, &mut store, surface, false, usize::MAX);
        assert_eq!(stats.errors, 0);

        let mut canvas_pipeline = aurora_gpu::CanvasPipeline::new(device);

        // `CanvasView::new()` is the exact real type `redraw` reads
        // `self.canvas_view.zoom()` from — 100% zoom, matching the bug
        // report (the offset was present at any zoom, but 100% is the
        // simplest real case).
        let canvas_view = aurora_ui::CanvasView::new();
        let scale_factor = 2.0_f64;

        let sample = (480, 128);

        // The bug: the raw, unscaled zoom fed directly into
        // `set_origin` alongside a physical-pixel viewport.
        residency.set_origin(queue, (0.0, 0.0), physical_viewport, canvas_view.zoom());
        let with_raw_zoom = render_and_sample_pixel(
            device,
            queue,
            &mut canvas_pipeline,
            &residency,
            physical_viewport,
            sample,
        );
        assert_eq!(
            with_raw_zoom,
            [255, 0, 0, 255],
            "the pre-fix bug: an uncorrected physical viewport with a raw \
             logical zoom shows twice the document content, so this sample \
             point falls into the red tile instead of the green one"
        );

        // The fix: `effective_residency_zoom` folds `scale_factor` in.
        residency.set_origin(
            queue,
            (0.0, 0.0),
            physical_viewport,
            effective_residency_zoom(canvas_view.zoom(), scale_factor),
        );
        let with_effective_zoom = render_and_sample_pixel(
            device,
            queue,
            &mut canvas_pipeline,
            &residency,
            physical_viewport,
            sample,
        );
        assert_eq!(
            with_effective_zoom,
            [0, 255, 0, 255],
            "the fix: correcting for scale_factor must show exactly the \
             document-pixel extent CanvasView's own logical-pixel contract \
             promises, landing this same sample point back in the green tile"
        );
    }

    #[test]
    fn recomposite_visible_tiles_blends_visible_layers_bottom_to_top_and_skips_hidden_ones() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let hidden = match layers.add_pixel_layer("hidden", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_visible(hidden, false) {
            unreachable!("{err:?}");
        }
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (bottom, [1.0, 0.0, 0.0, 1.0]),
            (hidden, [0.0, 1.0, 0.0, 1.0]),
            (top, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let result = read_first_texel(&mut store, composite_surface_id(), tile_id);
        assert_eq!(
            result,
            (0.5, 0.0, 0.5, 1.0),
            "opaque red bottom, opaque blue top at 50% opacity, hidden green never contributes"
        );
    }

    #[test]
    fn collect_widget_paints_uploads_a_mesh_for_every_paintable_widget_and_skips_the_rest() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err:?}"),
        };
        let theme = match load_theme() {
            Ok(theme) => theme,
            Err(err) => unreachable!("{err:?}"),
        };
        let (mut tree, root) = new_tree(taffy::Style::default());
        let button = match insert_button(&mut tree, root, &scales, "OK") {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = tree.set_bounds(
            button,
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 32,
            },
        ) {
            unreachable!("{err:?}");
        }

        let paints = collect_widget_paints(&tree, &theme, &scales, &context, 1.0);
        assert_eq!(
            paints.len(),
            1,
            "only the Button has paint defined -- the plain Container root does not"
        );
    }

    #[test]
    fn recomposite_visible_tiles_blends_a_layer_at_a_different_origin_than_the_active_layer() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let origin_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        // Offset by less than one tile on each axis, so the composite
        // tile at (0, 0) straddles up to four of `shifted`'s own tiles
        // -- exactly the case `read_layer_window` exists for.
        let shifted_bounds = aurora_core::Rect {
            x: 40,
            y: 40,
            ..origin_bounds
        };

        let bottom = match layers.add_pixel_layer("bottom", origin_bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let shifted = match layers.add_pixel_layer("shifted", shifted_bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(bottom_surface) = layers.surface_id(bottom) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(
            &mut store,
            bottom_surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [1.0, 0.0, 0.0, 1.0],
        );
        let Some(shifted_surface) = layers.surface_id(shifted) else {
            unreachable!("just created as a pixel layer");
        };
        // `shifted`'s own bounds start at document (40, 40); painting a
        // solid tile at its own local (0, 0) covers document
        // [40, 40 + TILE) on each axis -- squarely inside the active
        // layer's own composite tile at (0, 0) (document [0, TILE)),
        // since TILE (256) is far bigger than the 40px offset.
        fill_solid(
            &mut store,
            shifted_surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [0.0, 0.0, 1.0, 1.0],
        );

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        // `bottom` is the active layer, so the composite tile grid is
        // anchored to its own origin (0, 0) -- `shifted` is the one that
        // needs `read_layer_window`'s own re-tiling.
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            Some(bottom),
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let composite_surface = composite_surface_id();
        let at_shifted_origin =
            sample_pixel(&mut store, composite_surface, (40.0, 40.0)).unwrap_or([-1.0; 4]);
        let outside_shifted =
            sample_pixel(&mut store, composite_surface, (5.0, 5.0)).unwrap_or([-1.0; 4]);
        // Exact-literal comparison, not accumulated computation noise --
        // both `fill_solid` calls write exact 0.0/1.0 literals, and a
        // single fully-opaque layer over a transparent background
        // multiplies/adds only by 0.0 and 1.0 through `composite_tile_cpu`,
        // same reasoning this crate's other float_cmp allows already
        // document.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                at_shifted_origin,
                [0.0, 0.0, 1.0, 1.0],
                "shifted's own opaque blue must land at its real document position, not (0, 0)"
            );
            assert_eq!(
                outside_shifted,
                [1.0, 0.0, 0.0, 1.0],
                "outside shifted's own bounds, only bottom's opaque red should show"
            );
        }
    }

    /// The actual regression proof for "Eyedropper only samples the
    /// active layer, not the merged document" (PLAN.md's Basic-tools
    /// bullet): a non-active, visible `top` layer sits above the active
    /// `bottom` layer with its own real 50% opacity, so the two real,
    /// different colours blend to a third, distinct one
    /// (`recomposite_visible_tiles_blends_visible_layers_bottom_to_top_and_skips_hidden_ones`'s
    /// own exact combo: opaque red under 50%-opacity opaque blue ->
    /// `(0.5, 0.0, 0.5, 1.0)`). The old, pre-fix implementation read
    /// `bottom`'s own surface directly and would have returned its plain
    /// opaque red -- this asserts [`eyedropper_sample`] returns the real
    /// composited purple instead, proven against `bottom`'s own surface
    /// sampled directly as the sanity check for what the bug used to
    /// return.
    #[test]
    fn eyedropper_sample_reads_the_composited_colour_not_the_active_layers_own() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        let Some(bottom_surface) = layers.surface_id(bottom) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(&mut store, bottom_surface, tile_id, [1.0, 0.0, 0.0, 1.0]);
        let Some(top_surface) = layers.surface_id(top) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(&mut store, top_surface, tile_id, [0.0, 0.0, 1.0, 1.0]);

        // Sanity check: `bottom`'s own surface, sampled directly, really
        // is plain opaque red -- exactly what the old, pre-fix
        // `sample_eyedropper` would have picked, and the wrong answer.
        let bottoms_own = sample_pixel(&mut store, bottom_surface, (5.0, 5.0)).unwrap_or([-1.0; 4]);
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(bottoms_own, [1.0, 0.0, 0.0, 1.0]);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        // `bottom` is the active layer; `top`, non-active but visible,
        // sits above it in real stacking order with its own real 50%
        // opacity -- the exact scenario the old code got wrong.
        recomposite_visible_tiles(
            &residency,
            &layers,
            Some(bottom),
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let origin = active_layer_origin(&layers, Some(bottom));
        let sampled = eyedropper_sample(&mut store, origin, (5.0, 5.0));
        assert_eq!(
            sampled,
            Some([0.5, 0.0, 0.5]),
            "must pick up the real composited blend (opaque red bottom under 50%-opacity \
             opaque blue top), not bottom's own opaque red"
        );
    }

    /// The other half of "the active layer, not the merged document":
    /// here `active` (the active layer) is never painted at all --
    /// fully transparent everywhere -- while a non-active, visible layer
    /// above it has real opaque content at the same point. The old
    /// code's `alpha > 0.0` guard, checked against `active`'s own
    /// surface, would have found nothing to pick at all; the composited
    /// surface has the real visible colour.
    #[test]
    fn eyedropper_sample_reads_the_composited_colour_when_the_active_layer_itself_is_transparent_there()
     {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let active = match layers.add_pixel_layer("active", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let visible_above = match layers.add_pixel_layer("visible-above", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(above_surface) = layers.surface_id(visible_above) else {
            unreachable!("just created as a pixel layer");
        };
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        fill_solid(&mut store, above_surface, tile_id, [0.0, 1.0, 0.0, 1.0]);

        // Sanity check: `active`'s own surface really is transparent at
        // this point -- never painted at all.
        let Some(active_surface) = layers.surface_id(active) else {
            unreachable!("just created as a pixel layer");
        };
        let actives_own = sample_pixel(&mut store, active_surface, (5.0, 5.0)).unwrap_or([-1.0; 4]);
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(actives_own, [0.0, 0.0, 0.0, 0.0]);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            Some(active),
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let origin = active_layer_origin(&layers, Some(active));
        let sampled = eyedropper_sample(&mut store, origin, (5.0, 5.0));
        assert_eq!(
            sampled,
            Some([0.0, 1.0, 0.0]),
            "must pick up the visible layer above, not report nothing just because the \
             active layer itself is transparent here"
        );
    }

    /// The no-active-layer design decision this fix made: with no
    /// active layer selected at all, [`active_layer_origin`]'s own
    /// fallback is `(0.0, 0.0)` -- the document's own origin, not any
    /// layer's -- so [`eyedropper_sample`] must sample the merged
    /// document directly at `doc_point`, with no subtraction. `only`'s
    /// own bounds are deliberately *not* at the document origin (`(40,
    /// 40)`, mirroring
    /// `recomposite_visible_tiles_blends_a_layer_at_a_different_origin_than_the_active_layer`'s
    /// own offset): if this incorrectly subtracted `only`'s own bounds
    /// instead of using the document's origin, it would sample the wrong
    /// composite location and this would fail.
    #[test]
    fn eyedropper_sample_reads_the_merged_document_with_no_active_layer_selected() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 40,
            y: 40,
            width: 10,
            height: 10,
        };
        let only = match layers.add_pixel_layer("only", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(only) else {
            unreachable!("just created as a pixel layer");
        };
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        // Exact powers of two (unlike e.g. 0.2/0.4/0.6), so the `f16`
        // round trip through the tile store is bit-exact and this can
        // assert equality against the same literals written, not an
        // approximation.
        fill_solid(&mut store, surface, tile_id, [0.25, 0.5, 0.75, 1.0]);

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        // No active layer at all -- `reference_origin` (and
        // `active_layer_origin`, below) both fall back to the document's
        // own origin, `(0, 0)`.
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let origin = active_layer_origin(&layers, None);
        assert_eq!(origin, (0.0, 0.0));
        let sampled = eyedropper_sample(&mut store, origin, (45.0, 45.0));
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(sampled, Some([0.25, 0.5, 0.75]));
        }
    }

    /// `image`'s own real RGBA sample at document-space `(x, y)`, read
    /// straight out of `aurora_io::Image::samples`' own row-major
    /// layout -- the flat-buffer counterpart to `read_first_texel`/
    /// `sample_pixel` above, which read a *tile store*'s own texels
    /// instead.
    // The clearest names for one pixel's own coordinate/RGBA sample,
    // the same justification `sample_pixel`/`read_first_texel` already
    // use for the same lint.
    #[allow(clippy::many_single_char_names)]
    fn image_pixel(image: &aurora_io::Image, x: u32, y: u32) -> [f32; 4] {
        let idx = (y as usize * image.width() as usize + x as usize) * aurora_tile::CHANNELS;
        let samples = image.samples();
        let (Some(r), Some(g), Some(b), Some(a)) = (
            samples.get(idx),
            samples.get(idx + 1),
            samples.get(idx + 2),
            samples.get(idx + 3),
        ) else {
            unreachable!("(x, y) is within the image's own real width/height");
        };
        [r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32()]
    }

    #[test]
    // Hand-computed expected value, following `composite_tile_cpu`'s own
    // documented `src*alpha + dst*(1-alpha)` straight-alpha "over" math:
    // bottom (opaque red, opacity 1.0) composites first over fully
    // transparent black and reproduces itself exactly: (1, 0, 0, 1).
    // Top (opaque blue, opacity 0.5) then composites over that with
    // effective alpha = 1.0 (its own texel alpha) * 0.5 (layer opacity)
    // = 0.5:
    //   r = 0*0.5 + 1*0.5 = 0.5
    //   g = 0*0.5 + 0*0.5 = 0.0
    //   b = 1*0.5 + 0*0.5 = 0.5
    //   a = 0.5   + 1*0.5 = 1.0
    // -> (0.5, 0.0, 0.5, 1.0), the same result
    // `recomposite_visible_tiles_blends_visible_layers_bottom_to_top_and_skips_hidden_ones`
    // asserts for the live-canvas path -- this proves the export path
    // reaches the same real composite, not just "something different
    // from the old active-layer-only read".
    fn composite_document_blends_two_layers_normal_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.width(), 10);
        assert_eq!(image.height(), 10);
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.5, 0.0, 0.5, 1.0],
                "opaque red bottom under opaque blue top at 50% opacity"
            );
        }
    }

    /// The export half of 0.52.1. `aurora-tile`'s own exact-length check
    /// turns a corrupted scratch-disk tile into a real `TileError`
    /// instead of a short buffer -- but `resolve_tile` *catches* that
    /// error, logs it, and skips the layer, which on the export path
    /// meant `composite_document` returned `Ok` with that layer's pixels
    /// silently missing and `App::save_file` wrote the result straight
    /// over the user's file. Unannounced content loss in a saved file is
    /// the failure CLAUDE.md names as the worst this project can have,
    /// so the export refuses. Extended in 0.52.2 with the *retry*: the
    /// refusal is only worth anything if pressing Save again refuses
    /// too, which it did not until `TileStore::ensure_resident` stopped
    /// dropping the paged-out mapping of a tile whose page-in failed.
    ///
    /// Deliberately *not* changed, and therefore not asserted here: the
    /// live canvas (`recomposite_visible_tiles`) still skips-and-repaints,
    /// because failing every frame over one bad tile is worse to use than
    /// a visibly missing layer.
    ///
    /// The corruption is real, not mocked: a budget-of-1 store evicts the
    /// bottom layer's tile to the scratch directory, `flush` confirms the
    /// write, and the file is then truncated the way a crash mid-write or
    /// a full disk leaves it. Every sibling `composite_document_*` test
    /// here is the positive control -- they run against an uncorrupted
    /// store and get `Ok`.
    #[test]
    fn composite_document_refuses_to_export_when_a_layer_tile_cannot_be_read() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        // One resident tile at a time, so the second layer's first touch
        // evicts the first layer's tile to disk.
        let Some(budget) = std::num::NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };

        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }
        // `bottom`'s tile is now evicted and in flight; `flush` makes the
        // write real so the file below is the one `page_in` will read.
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        let mut scratch_files: Vec<std::path::PathBuf> = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        for entry in entries.flatten() {
            scratch_files.push(entry.path());
        }
        assert_eq!(
            scratch_files.len(),
            1,
            "exactly one tile should have been evicted: {scratch_files:?}"
        );
        let Some(victim) = scratch_files.first() else {
            unreachable!("just asserted there is exactly one");
        };
        let Ok(bytes) = std::fs::read(victim) else {
            unreachable!("the evicted tile file must be readable");
        };
        let Some(truncated) = bytes.get(..bytes.len() / 2) else {
            unreachable!("half of a slice's own length is always in range");
        };
        if let Err(err) = std::fs::write(victim, truncated) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        match composite_document(&layers, &mut store, 10, 10) {
            Err(aurora_io::IoError::IncompleteComposite { skipped, first }) => {
                assert_eq!(skipped, 1, "exactly one layer tile was unreadable");
                assert!(
                    first.contains("corrupt tile file"),
                    "the refusal must carry the real underlying tile error: {first}"
                );
            }
            Ok(_) => unreachable!(
                "exporting a document whose bottom layer cannot be read must not quietly \
                 succeed with that layer missing"
            ),
            Err(other) => unreachable!("expected IncompleteComposite, got {other:?}"),
        }

        // The retry, and the point of this addition (0.52.2): before
        // `aurora_tile::TileStore::ensure_resident` stopped forgetting a
        // tile whose page-in failed, this second call returned
        // `Ok(Image)` with the bottom layer silently blank -- so a user
        // who hit the refusal above and simply pressed Save again got
        // exactly the quietly-incomplete file the first refusal exists to
        // prevent. Nothing in this crate changed to fix that; the store
        // returning a real `Err` on every read is the whole of it.
        match composite_document(&layers, &mut store, 10, 10) {
            Err(aurora_io::IoError::IncompleteComposite { skipped, .. }) => {
                assert_eq!(
                    skipped, 1,
                    "the retry must refuse for the same one unreadable tile"
                );
            }
            Ok(_) => unreachable!(
                "a retried export of a still-corrupt document must not quietly succeed with the \
                 layer blank"
            ),
            Err(other) => unreachable!("expected IncompleteComposite, got {other:?}"),
        }
    }

    #[test]
    // Proves the real per-layer `blend_mode` stored on the document
    // actually reaches `composite_tile_cpu` through
    // `composite_document`/`translate_blend_mode` -- not just that
    // `Multiply`'s own math is correct in isolation (already covered by
    // `aurora-render`'s own
    // `composite_tile_cpu_multiply_blends_two_mid_greys_to_a_quarter_grey`),
    // but that setting `BlendMode::Multiply` on a real `LayerTree` layer
    // changes what this export path actually produces, versus the
    // Normal-blend result the sibling test just above asserts for the
    // same-shaped document. Both layers are 50% grey, opaque, full
    // opacity: bottom composites first over transparent black and
    // reproduces itself exactly (0.5, 0.5, 0.5, 1.0); top then
    // composites over that with `Multiply`, and since both layers are
    // fully opaque at full layer opacity (`as = ab = 1.0`), the general
    // formula reduces to exactly `B(Cb, Cs) = Cb * Cs` per channel:
    // 0.5 * 0.5 = 0.25 -> (0.25, 0.25, 0.25, 1.0). A Normal blend of the
    // same two layers would instead just reproduce the top layer
    // unchanged (0.5, 0.5, 0.5, 1.0) -- the two results are different,
    // so this genuinely distinguishes "blend mode was read and applied"
    // from "blend mode was silently ignored".
    fn composite_document_blends_two_layers_multiply_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [0.5, 0.5, 0.5, 1.0]), (top, [0.5, 0.5, 0.5, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.25, 0.25, 0.25, 1.0],
                "50% grey multiplied by 50% grey must darken to 25% grey, not stay 50%"
            );
        }
    }

    #[test]
    // Same shape as the Multiply test just above, retargeted at
    // `BlendMode::ColorDodge` -- one of the 4 "dodge and burn" modes
    // added this round. Proves the real per-layer blend mode set on a
    // `LayerTree` layer actually reaches `translate_blend_mode` and
    // `composite_tile_cpu` through the export path, not just that
    // `ColorDodge`'s own formula is correct in isolation (already
    // covered by `aurora-render`'s own
    // `composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio`).
    // Bottom (backdrop) composites first over transparent black and
    // reproduces itself exactly: (0.375, 0.375, 0.375, 1.0). Top (source,
    // full opacity) then composites over that with `ColorDodge`, and
    // since both layers are fully opaque at full layer opacity
    // (`as = ab = 1.0`), the general formula reduces to exactly
    // `B(Cb, Cs) = min(1, Cb / (1 - Cs))` per channel:
    // min(1, 0.375 / (1 - 0.5)) = min(1, 0.75) = 0.75 ->
    // (0.75, 0.75, 0.75, 1.0). 0.375/0.5/0.75 (unlike 0.4/0.6/0.8) are
    // exact eighths, so they round-trip bit-exact through `f16` -- the
    // same values and reasoning `aurora-render`'s own
    // `composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio`
    // uses. A Normal blend of the same two layers would instead just
    // reproduce the top layer unchanged (0.5, 0.5, 0.5, 1.0), and a
    // Multiply blend would darken to (0.1875, 0.1875, 0.1875, 1.0) --
    // all three results differ, so this genuinely distinguishes
    // "ColorDodge was read and applied" from either "blend mode was
    // silently ignored" or "the wrong mode's math ran".
    fn composite_document_blends_two_layers_color_dodge_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::ColorDodge) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (bottom, [0.375, 0.375, 0.375, 1.0]),
            (top, [0.5, 0.5, 0.5, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.75, 0.75, 0.75, 1.0],
                "ColorDodge of backdrop 0.375 by source 0.5 must yield 0.75, not the Normal (0.5) or Multiply (0.1875) result"
            );
        }
    }

    #[test]
    // Same shape as the Multiply/ColorDodge tests just above, retargeted
    // at `BlendMode::Overlay` -- one of the 7 "overlay and light" modes
    // added this round. Proves the real per-layer blend mode set on a
    // `LayerTree` layer actually reaches `translate_blend_mode` and
    // `composite_tile_cpu` through the export path, not just that
    // `Overlay`'s own formula is correct in isolation (already covered
    // by `aurora-render`'s own
    // `composite_tile_cpu_overlay_uses_the_direct_multiply_form_when_the_backdrop_is_at_or_below_half`).
    // Bottom (backdrop) composites first over transparent black and
    // reproduces itself exactly: (0.25, 0.25, 0.25, 1.0). Top (source,
    // full opacity) then composites over that with `Overlay`, and since
    // both layers are fully opaque at full layer opacity (`as = ab =
    // 1.0`), the general formula reduces to exactly `B(Cb, Cs)` per
    // channel; the backdrop (0.25) is `<= 0.5`, so Overlay's own direct
    // branch fires: `2 * Cb * Cs = 2 * 0.25 * 0.75 = 0.375` ->
    // (0.375, 0.375, 0.375, 1.0). A Normal blend of the same two layers
    // would instead just reproduce the top layer unchanged
    // (0.75, 0.75, 0.75, 1.0), a Multiply blend would darken to
    // (0.1875, 0.1875, 0.1875, 1.0), and a ColorDodge blend would yield
    // (1.0, 1.0, 1.0, 1.0) (backdrop 0.25 is `Cb / (1-Cs) = 0.25/0.25 =
    // 1.0`) -- all differ from Overlay's own 0.375, so this genuinely
    // distinguishes "Overlay was read and applied" from either "blend
    // mode was silently ignored" or "the wrong mode's math ran".
    fn composite_document_blends_two_layers_overlay_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Overlay) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (bottom, [0.25, 0.25, 0.25, 1.0]),
            (top, [0.75, 0.75, 0.75, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.375, 0.375, 0.375, 1.0],
                "Overlay of backdrop 0.25 by source 0.75 must yield 0.375, not the Normal (0.75), Multiply (0.1875), or ColorDodge (1.0) result"
            );
        }
    }

    #[test]
    // Same shape as the Multiply/ColorDodge/Overlay tests above,
    // retargeted at `BlendMode::Luminosity` -- one of the 4
    // non-separable modes added this round, proving the real per-layer
    // blend mode set on a `LayerTree` layer reaches `translate_blend_mode`
    // and `aurora_render::composite_tile_cpu`'s own `blend_rgb` path
    // through the export path, not just that `Luminosity`'s own formula
    // is correct in isolation (already covered by `aurora-render`'s own
    // `composite_tile_cpu_luminosity_matches_the_worked_example`). Uses
    // this round's own worked example: backdrop (bottom) 50% grey,
    // source (top) pure red. Bottom composites first over transparent
    // black and reproduces itself exactly: (0.5, 0.5, 0.5, 1.0). Top
    // then composites over that with `Luminosity`, and since both
    // layers are fully opaque at full layer opacity (`as = ab = 1.0`),
    // the general formula reduces to exactly `B(Cb, Cs) =
    // Luminosity(Cb, Cs) = SetLum(Cb, Lum(Cs))`: `Lum(Cs) = 0.3`, `d =
    // 0.3 - 0.5 = -0.2`, `C' = (0.3, 0.3, 0.3)`, already in gamut ->
    // (0.3, 0.3, 0.3, 1.0). A Normal blend of the same two layers would
    // instead just reproduce the top layer unchanged (1.0, 0.0, 0.0,
    // 1.0) -- clearly different from Luminosity's own (0.3, 0.3, 0.3),
    // so this genuinely distinguishes "Luminosity was read and applied"
    // from "blend mode was silently ignored" (which would fall back to
    // `Normal` at the `translate_blend_mode` boundary, exactly as it
    // did before this round). Epsilon tolerance, not exact equality:
    // the W3C spec's own `Lum` weights (0.3/0.59/0.11) aren't exact
    // binary fractions, so `aurora-render`'s own
    // `composite_tile_cpu_luminosity_matches_the_worked_example` and
    // this integration test both use the same `1e-3` tolerance rather
    // than `assert_eq!`.
    fn composite_document_blends_two_layers_luminosity_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Luminosity) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [0.5, 0.5, 0.5, 1.0]), (top, [1.0, 0.0, 0.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let [r, g, b, a] = image_pixel(&image, 0, 0);
        let epsilon = 1e-3;
        assert!(
            (r - 0.3).abs() < epsilon && (g - 0.3).abs() < epsilon && (b - 0.3).abs() < epsilon,
            "Luminosity of backdrop (0.5,0.5,0.5) by source (1,0,0) must land near (0.3, 0.3, 0.3), got ({r}, {g}, {b})"
        );
        assert!((a - 1.0).abs() < epsilon);
    }

    #[test]
    // Same shape as the Multiply/ColorDodge/Overlay/Luminosity tests
    // above, retargeted at `BlendMode::DarkerColor` -- one of the 2
    // whole-colour-selection modes added this round, proving the real
    // per-layer blend mode set on a `LayerTree` layer reaches
    // `translate_blend_mode` and `aurora_render::composite_tile_cpu`'s
    // own `blend_rgb` path through the export path, not just that
    // DarkerColor's own formula is correct in isolation (already
    // covered by `aurora-render`'s own
    // `composite_tile_cpu_darker_color_picks_the_whole_lower_luminance_backdrop`).
    // Backdrop (bottom) `Cb=(0.5, 0.2, 0.9)` -- `Lum(Cb) = 0.3*0.5 +
    // 0.59*0.2 + 0.11*0.9 = 0.15 + 0.118 + 0.099 = 0.367` -- source
    // (top) `Cs=(0.4, 0.4, 0.4)`, an achromatic grey whose own `Lum`
    // always equals its own channel value (the spec's weights sum to
    // exactly `0.3+0.59+0.11=1.0`), so `Lum(Cs)=0.4`. Since
    // `Lum(Cb)=0.367 < Lum(Cs)=0.4`, and both layers are fully opaque at
    // full layer opacity (`as = ab = 1.0`, so the general compositing
    // formula reduces to exactly `B(Cb,Cs)`), the whole document must
    // land at the *whole backdrop* colour, `(0.5, 0.2, 0.9)` -- not the
    // per-channel minimum `(min(0.5,0.4), min(0.2,0.4), min(0.9,0.4)) =
    // (0.4, 0.2, 0.4)` a separable `Darken` would give for the same two
    // layers, and not the top layer unchanged (`(0.4,0.4,0.4)`, what a
    // Normal blend or a silently-ignored blend mode would produce) --
    // so this genuinely distinguishes "DarkerColor was read and
    // applied, as a whole-colour selection" from both "blend mode was
    // silently ignored" and "a separable per-channel mode ran instead."
    fn composite_document_blends_two_layers_darker_color_blend_matching_the_hand_computed_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::DarkerColor) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [0.5, 0.2, 0.9, 1.0]), (top, [0.4, 0.4, 0.4, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let [r, g, b, a] = image_pixel(&image, 0, 0);
        let epsilon = 1e-3;
        assert!(
            (r - 0.5).abs() < epsilon && (g - 0.2).abs() < epsilon && (b - 0.9).abs() < epsilon,
            "DarkerColor of backdrop (0.5,0.2,0.9) by source (0.4,0.4,0.4) must land at the whole backdrop (0.5, 0.2, 0.9), got ({r}, {g}, {b})"
        );
        assert!((a - 1.0).abs() < epsilon);
        // Distinguish from the separable Darken hybrid this same pair
        // would give: (0.4, 0.2, 0.4).
        assert!(
            !((r - 0.4).abs() < epsilon && (g - 0.2).abs() < epsilon && (b - 0.4).abs() < epsilon),
            "result must not be Darken's own per-channel-minimum hybrid (0.4, 0.2, 0.4)"
        );
    }

    #[test]
    // The mirror image of the DarkerColor test above, same two layers
    // and same reasoning, retargeted at `BlendMode::LighterColor`: since
    // `Lum(Cb)=0.367 < Lum(Cs)=0.4`, the whole document must land at the
    // whole *source* colour, `(0.4, 0.4, 0.4)` -- distinct from
    // DarkerColor's own result for this pair (`(0.5, 0.2, 0.9)`) and
    // from the separable Lighten hybrid the same pair would give
    // (`(max(0.5,0.4), max(0.2,0.4), max(0.9,0.4)) = (0.5, 0.4, 0.9)`).
    fn composite_document_blends_two_layers_lighter_color_blend_matching_the_hand_computed_result()
    {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::LighterColor) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [0.5, 0.2, 0.9, 1.0]), (top, [0.4, 0.4, 0.4, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let [r, g, b, a] = image_pixel(&image, 0, 0);
        let epsilon = 1e-3;
        assert!(
            (r - 0.4).abs() < epsilon && (g - 0.4).abs() < epsilon && (b - 0.4).abs() < epsilon,
            "LighterColor of backdrop (0.5,0.2,0.9) by source (0.4,0.4,0.4) must land at the whole source (0.4, 0.4, 0.4), got ({r}, {g}, {b})"
        );
        assert!((a - 1.0).abs() < epsilon);
        // Distinguish from the separable Lighten hybrid this same pair
        // would give: (0.5, 0.4, 0.9).
        assert!(
            !((r - 0.5).abs() < epsilon && (g - 0.4).abs() < epsilon && (b - 0.9).abs() < epsilon),
            "result must not be Lighten's own per-channel-maximum hybrid (0.5, 0.4, 0.9)"
        );
    }

    // ---- Dissolve: SplitMix64, position hashing, and the gate itself ----
    //
    // Dissolve closes out the 27-mode blend series (see PLAN.md), but its
    // correctness bar is different from every prior round: it's the only
    // mode where "the math is right" isn't enough on its own -- it also
    // has to be *the same* math every single time the same document is
    // composited, or a user would see a different speckle pattern on
    // every redraw, pan, export, and reopen. The tests below are ordered
    // bottom-up: the hash primitive first (verified against real,
    // independently re-derived reference output, not just internal
    // self-consistency), then the position-combination function's own
    // real properties (reproducible, position-sensitive, asymmetric,
    // and -- the one that matters most -- a pure function of *absolute*
    // position regardless of how a caller's own tile/doc_origin
    // decomposition happens to reach it), then `dissolve_gate` itself
    // (the probability weighting, both edge cases), then a real
    // `composite_document` integration test proving the whole path is
    // wired together end to end.

    #[test]
    // Reference: Sebastiano Vigna's public-domain `splitmix64.c`
    // (<https://prng.di.unimi.it/splitmix64.c>). Its `next()` increments
    // a `uint64_t` state by the golden-ratio gamma *then* mixes on every
    // call, including the first -- so `splitmix64(seed)` (this crate's
    // stateless, seed-in/hash-out shape) reproduces exactly the
    // reference generator's first output for a generator whose state
    // starts at `seed`. These 4 expected values were independently
    // re-derived from that reference algorithm via a from-scratch Python
    // transcription (arbitrary-precision integers masked to 64 bits at
    // every step, not trusting this Rust implementation at all) rather
    // than eyeballed or copied from this function's own output -- the
    // same "independently re-implement the spec, don't trust this
    // crate's own answer" discipline the non-separable-HSL round's own
    // Python cross-check already established for `lum`/`SetLum`. `0` is
    // the canonical, most commonly cited SplitMix64 test seed; `1`,
    // `42`, and `u64::MAX` (all-ones, the opposite extreme of all-zero
    // `0`) are included so this isn't a single-point coincidence.
    fn splitmix64_matches_the_reference_algorithms_own_output_for_several_seeds() {
        assert_eq!(splitmix64(0), 0xe220_a839_7b1d_cdaf);
        assert_eq!(splitmix64(1), 0x910a_2dec_8902_5cc1);
        assert_eq!(splitmix64(42), 0xbdd7_3226_2feb_6e95);
        assert_eq!(splitmix64(u64::MAX), 0xe4d9_7177_1b65_2c20);
    }

    #[test]
    fn hash_position_is_reproducible_for_the_same_absolute_position() {
        assert_eq!(hash_position(10, 10), hash_position(10, 10));
        assert_eq!(hash_position(-500, 300), hash_position(-500, 300));
    }

    #[test]
    // Not a degenerate constant hash: two different positions inside the
    // same `TILE`-sized tile, and a position in a genuinely different
    // tile, all land on different noise values. `(300, 10)` is in tile
    // column 1 (`300 / 256 == 1`) while the other two are in tile column
    // 0 -- a real cross-tile comparison, not just "different numbers".
    fn hash_position_differs_for_different_positions_same_tile_and_different_tiles() {
        let a = hash_position(10, 10);
        let b = hash_position(20, 10);
        let c = hash_position(300, 10);
        assert_ne!(
            a, b,
            "two different positions in the same tile must not collide"
        );
        assert_ne!(a, c, "positions in different tiles must not collide");
        assert_ne!(b, c);
    }

    #[test]
    // The asymmetry `hash_position`'s own doc comment names: swapping the
    // two coordinates must not produce the same hash (a naive symmetric
    // combine, e.g. plain `zigzag_encode(x) ^ zigzag_encode(y)`, would
    // fail this).
    fn hash_position_is_asymmetric_under_coordinate_swap() {
        assert_ne!(hash_position(3, 7), hash_position(7, 3));
    }

    #[test]
    // The core correctness property this whole design exists for: the
    // *same* absolute document-space position must hash identically no
    // matter how a caller's own `doc_origin`/local-coordinate split
    // happens to reach it. `(200, 100)` is reached two different ways
    // here -- once as tile `(0, 0)`'s own local `(200, 100)`, once as a
    // `doc_origin` shifted by `(-1, -1)` (simulating a canvas scrolled by
    // one pixel, so the tile grid no longer lines up the same way) whose
    // local `(201, 101)` lands on the same absolute point. A tile-
    // relative implementation (hashing local row/col alone, or hashing
    // `tile_id` instead of `doc_origin`) would fail this.
    fn hash_position_depends_only_on_absolute_position_not_on_how_it_was_decomposed() {
        let via_tile_a = hash_position(200, 100);
        let via_tile_b = hash_position(-1 + 201, -1 + 101);
        assert_eq!(via_tile_a, via_tile_b);
    }

    #[test]
    fn hash_to_unit_f32_never_reaches_1_0() {
        // The maximum possible 24-bit top value is `2^24 - 1`, so the
        // maximum possible output is strictly less than `1.0` -- load-
        // bearing for `dissolve_gate`'s own `opacity = 1.0` edge case
        // (see the dedicated test below): `noise < 1.0` must always be
        // `true` for every real hash output, not just almost always.
        assert!(hash_to_unit_f32(u64::MAX) < 1.0);
        assert!(hash_to_unit_f32(0) >= 0.0);
    }

    /// Builds a real `aurora_tile::TILE`×`aurora_tile::TILE` texel buffer
    /// with every texel set to `rgba` -- the same shape `fill_solid`
    /// gives a real tile-store tile, but as a standalone `Vec` for
    /// exercising `dissolve_gate` directly without a `TileStore`.
    fn solid_tile_buffer(rgba: [f32; 4]) -> Vec<half::f16> {
        let texel_count = aurora_tile::TILE as usize * aurora_tile::TILE as usize;
        let mut texels = vec![half::f16::from_f32(0.0); texel_count * aurora_tile::CHANNELS];
        for chunk in texels.chunks_exact_mut(aurora_tile::CHANNELS) {
            for (sample, &channel) in chunk.iter_mut().zip(rgba.iter()) {
                *sample = half::f16::from_f32(channel);
            }
        }
        texels
    }

    #[test]
    // Edge case named directly in the task: `opacity = 1.0` with fully-
    // opaque source texels (`texel_alpha = 1.0`) must show the source at
    // *every* pixel, deterministically, no per-pixel variation -- because
    // `effective_alpha = 1.0`, and `hash_to_unit_f32_never_reaches_1_0`
    // above already proved `noise < 1.0` holds for every real hash
    // output.
    fn dissolve_gate_at_full_opacity_and_full_source_alpha_shows_every_pixel() {
        let texels = solid_tile_buffer([0.25, 0.5, 0.75, 1.0]);
        let gated = dissolve_gate(&texels, 1.0, (0, 0));
        assert_eq!(gated.len(), texels.len());
        // `0.25`/`0.5`/`0.75`/`1.0` are all exact binary fractions, so bit-
        // exact comparison is legitimate here (the same reasoning
        // `composite_document_blends_two_layers_normal_blend_matching_the_hand_computed_result`
        // and `composite_tile_cpu_color_dodge_computes_the_clamped_per_channel_ratio`
        // already use for the same lint).
        #[allow(clippy::float_cmp)]
        for chunk in gated.chunks_exact(aurora_tile::CHANNELS) {
            let [r, g, b, a] = chunk else {
                unreachable!("CHANNELS-sized chunks always destructure to 4 elements");
            };
            assert_eq!(a.to_f32(), 1.0);
            assert_eq!(r.to_f32(), 0.25);
            assert_eq!(g.to_f32(), 0.5);
            assert_eq!(b.to_f32(), 0.75);
        }
    }

    #[test]
    // The opposite edge case: `opacity = 0.0` must show the source at
    // *no* pixel, regardless of the source's own texel alpha, since
    // `effective_alpha = texel_alpha * 0.0 = 0.0` and `noise >= 0.0`
    // always (`hash_to_unit_f32`'s own doc comment), so `noise <
    // effective_alpha` is never true.
    fn dissolve_gate_at_zero_opacity_shows_no_pixel() {
        let texels = solid_tile_buffer([1.0, 1.0, 1.0, 1.0]);
        let gated = dissolve_gate(&texels, 0.0, (0, 0));
        for chunk in gated.chunks_exact(aurora_tile::CHANNELS) {
            let [r, g, b, a] = chunk else {
                unreachable!("CHANNELS-sized chunks always destructure to 4 elements");
            };
            assert_eq!(
                (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32()),
                (0.0, 0.0, 0.0, 0.0)
            );
        }
    }

    #[test]
    // The headline statistical property: a layer at a real, known
    // opacity shows on roughly that fraction of pixels, not uniformly
    // everywhere and not a smooth per-pixel fade. Sampled over one whole
    // real tile (65,536 pixels, `TEXELS`) at `opacity = 0.3`, each pixel
    // is an independent Bernoulli trial (independent because each one's
    // outcome depends only on its own distinct absolute position) with
    // `p = 0.3`, so the count of "shown" pixels is Binomial(n = 65536,
    // p = 0.3): expected count `n*p = 19660.8`, standard deviation
    // `sqrt(n*p*(1-p)) ~= 117.3`. Tolerance used here is `0.05` absolute
    // (fraction must land in `[0.25, 0.35]`), which is `~28` standard
    // deviations wide (`0.05 / (117.3/65536) ~= 27.9`) -- astronomically
    // safe against any real flake (this is a deterministic function
    // being measured once, not literally re-sampled per CI run, but the
    // bound is stated in real binomial terms rather than picked
    // arbitrarily). The actual measured fraction, from a real run of
    // this exact test: 19624 / 65536 = 0.299438..., about 0.31 standard
    // deviations from the 0.3 target -- see PLAN.md for this same figure
    // recorded as this round's own measured result.
    fn dissolve_gate_shows_approximately_the_layers_own_opacity_fraction_of_pixels() {
        let texels = solid_tile_buffer([1.0, 1.0, 1.0, 1.0]);
        let opacity = 0.3;
        let gated = dissolve_gate(&texels, opacity, (0, 0));
        let shown = gated
            .chunks_exact(aurora_tile::CHANNELS)
            .filter(|chunk| {
                let [_, _, _, a] = chunk else {
                    return false;
                };
                a.to_f32() > 0.0
            })
            .count();
        let total = gated.len() / aurora_tile::CHANNELS;
        #[allow(clippy::cast_precision_loss)]
        let fraction = shown as f64 / total as f64;
        assert!(
            (fraction - f64::from(opacity)).abs() < 0.05,
            "shown fraction {fraction} (of {total} pixels, {shown} shown) too far from \
             the target opacity {opacity} -- see this test's own doc comment for the \
             binomial tolerance reasoning"
        );
    }

    #[test]
    // Reproducibility is the single most important property of this
    // whole feature (see this module's own `dissolve_gate` doc comment):
    // the exact same inputs, called twice, in two entirely separate
    // calls, must produce bit-identical output -- proving there is no
    // hidden state, thread-local RNG, or time-based input anywhere in
    // the path.
    fn dissolve_gate_is_bit_identical_across_two_separate_calls() {
        let texels = solid_tile_buffer([0.1, 0.2, 0.3, 0.7]);
        let first = dissolve_gate(&texels, 0.4, (123, -456));
        let second = dissolve_gate(&texels, 0.4, (123, -456));
        assert_eq!(first, second);
    }

    #[test]
    // The core correctness property, exercised through the real
    // `dissolve_gate` entry point rather than `hash_position` alone: the
    // same absolute position must gate identically whether it's reached
    // as tile `(0, 0)`'s own local `(200, 100)`, or via a `doc_origin`
    // shifted by `(-1, -1)` whose local `(201, 101)` lands on that same
    // absolute point -- the exact scenario a scrolled canvas view or a
    // layer whose own tile boundaries don't line up with the document
    // grid produces in practice. Uses a fractional opacity (not `0.0`/
    // `1.0`) so the comparison genuinely depends on the noise value
    // matching, not a degenerate always-same-answer edge case.
    fn dissolve_gate_matches_at_the_same_absolute_position_reached_via_different_doc_origins() {
        let rgba = [0.2, 0.4, 0.6, 1.0];
        let texels_a = solid_tile_buffer(rgba);
        let texels_b = solid_tile_buffer(rgba);
        let opacity = 0.5;

        let tile_side = aurora_tile::TILE as usize;
        let gated_a = dissolve_gate(&texels_a, opacity, (0, 0));
        let index_a = 100 * tile_side + 200;

        let gated_b = dissolve_gate(&texels_b, opacity, (-1, -1));
        let index_b = 101 * tile_side + 201;

        let channels = aurora_tile::CHANNELS;
        let (Some(texel_a), Some(texel_b)) = (
            gated_a.get(index_a * channels..index_a * channels + channels),
            gated_b.get(index_b * channels..index_b * channels + channels),
        ) else {
            unreachable!("indices constructed to be in range for a real TILE-sized buffer");
        };
        assert_eq!(
            texel_a, texel_b,
            "the same absolute document position (200, 100), reached via two different \
             doc_origin/local-coordinate decompositions, must gate identically"
        );
    }

    fn rect_mask(x: i64, y: i64, width: u32, height: u32) -> aurora_doc::LayerMask {
        aurora_doc::LayerMask {
            bounds: aurora_core::Rect {
                x,
                y,
                width,
                height,
            },
            enabled: true,
            inverted: false,
        }
    }

    #[test]
    // A texel whose absolute position lands inside `mask.bounds` passes
    // through unchanged -- exactly its own source colour and alpha, not
    // forced to any particular value the way `dissolve_gate`'s own
    // "shown" branch forces alpha to `1.0`.
    fn apply_mask_clip_passes_through_a_texel_inside_the_mask_bounds() {
        let texels = solid_tile_buffer([0.25, 0.5, 0.75, 0.6]);
        let mask = rect_mask(0, 0, aurora_tile::TILE, aurora_tile::TILE);
        let clipped = apply_mask_clip(&texels, &mask, (0, 0));
        assert_eq!(clipped, texels);
    }

    #[test]
    // A texel outside `mask.bounds` comes back fully transparent -- the
    // same `(0, 0, 0, 0)` "hidden" convention `dissolve_gate` uses.
    fn apply_mask_clip_zeroes_a_texel_outside_the_mask_bounds() {
        let texels = solid_tile_buffer([1.0, 1.0, 1.0, 1.0]);
        // Bounds cover none of doc-space (0, 0)..(TILE, TILE) -- entirely
        // to the right of the tile this buffer covers.
        let mask = rect_mask(
            i64::from(aurora_tile::TILE),
            0,
            aurora_tile::TILE,
            aurora_tile::TILE,
        );
        let clipped = apply_mask_clip(&texels, &mask, (0, 0));
        for chunk in clipped.chunks_exact(aurora_tile::CHANNELS) {
            let [r, g, b, a] = chunk else {
                unreachable!("CHANNELS-sized chunks always destructure to 4 elements");
            };
            assert_eq!(
                (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32()),
                (0.0, 0.0, 0.0, 0.0)
            );
        }
    }

    #[test]
    // `inverted` flips both of the above cases: what was shown becomes
    // hidden and vice versa.
    fn apply_mask_clip_inverted_flips_shown_and_hidden() {
        let texels = solid_tile_buffer([0.1, 0.2, 0.3, 1.0]);

        let mut inside_mask = rect_mask(0, 0, aurora_tile::TILE, aurora_tile::TILE);
        inside_mask.inverted = true;
        let clipped_inside = apply_mask_clip(&texels, &inside_mask, (0, 0));
        for chunk in clipped_inside.chunks_exact(aurora_tile::CHANNELS) {
            let [r, g, b, a] = chunk else {
                unreachable!("CHANNELS-sized chunks always destructure to 4 elements");
            };
            assert_eq!(
                (r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32()),
                (0.0, 0.0, 0.0, 0.0),
                "inverting a mask that would otherwise show this texel must hide it"
            );
        }

        let mut outside_mask = rect_mask(
            i64::from(aurora_tile::TILE),
            0,
            aurora_tile::TILE,
            aurora_tile::TILE,
        );
        outside_mask.inverted = true;
        let clipped_outside = apply_mask_clip(&texels, &outside_mask, (0, 0));
        assert_eq!(
            clipped_outside, texels,
            "inverting a mask that would otherwise hide this texel must show it unchanged"
        );
    }

    #[test]
    // The same absolute-position correctness `dissolve_gate`'s own
    // `dissolve_gate_matches_at_the_same_absolute_position_reached_via_different_doc_origins`
    // proves matters here too: `mask.bounds` is document-absolute, so a
    // texel at the same real position must clip identically whichever
    // `doc_origin`/local-coordinate split reaches it -- tile `(0, 0)`'s
    // own local `(200, 100)` vs. a `doc_origin` shifted by `(-1, -1)`
    // whose local `(201, 101)` lands on that same absolute point.
    fn apply_mask_clip_matches_at_the_same_absolute_position_reached_via_different_doc_origins() {
        let rgba = [0.2, 0.4, 0.6, 1.0];
        let texels_a = solid_tile_buffer(rgba);
        let texels_b = solid_tile_buffer(rgba);
        // Bounds cover absolute (200, 100) but not much else nearby --
        // a small rect straddling that one point.
        let mask = rect_mask(195, 95, 10, 10);

        let tile_side = aurora_tile::TILE as usize;
        let clipped_a = apply_mask_clip(&texels_a, &mask, (0, 0));
        let index_a = 100 * tile_side + 200;

        let clipped_b = apply_mask_clip(&texels_b, &mask, (-1, -1));
        let index_b = 101 * tile_side + 201;

        let channels = aurora_tile::CHANNELS;
        let (Some(texel_a), Some(texel_b)) = (
            clipped_a.get(index_a * channels..index_a * channels + channels),
            clipped_b.get(index_b * channels..index_b * channels + channels),
        ) else {
            unreachable!("indices constructed to be in range for a real TILE-sized buffer");
        };
        assert_eq!(
            texel_a, texel_b,
            "the same absolute document position (200, 100), reached via two different \
             doc_origin/local-coordinate decompositions, must clip identically"
        );
        // And both must actually be shown (inside the mask's own
        // bounds), not just equal to each other by both being zeroed.
        let [_, _, _, a] = texel_a else {
            unreachable!("CHANNELS-sized chunk always destructures to 4 elements");
        };
        assert!(
            a.to_f32() > 0.0,
            "absolute (200, 100) is inside mask.bounds and must be shown"
        );
    }

    #[test]
    // Real integration test, mirroring the shape of every prior blend-
    // mode round's own `composite_document_blends_two_layers_*` test:
    // proves `aurora_doc::BlendMode::Dissolve`, set via the real
    // `LayerTree::set_blend_mode` API, actually reaches `resolve_tile`'s
    // own interception and produces a genuine stochastic mix through the
    // real `composite_document` export path -- not a no-op, and not
    // silently falling back to `Normal` (which would show the top
    // layer's own blue at every pixel, uniformly). Bottom: opaque red,
    // full opacity, `Normal`. Top: opaque blue, `Dissolve` at opacity
    // `0.5`, covering the same 10x10 region. With 100 independent
    // pixels each showing blue with probability 0.5, the odds of every
    // single one landing the same way are `2 * 0.5^100` -- so finding
    // both colours present in the composited result is not a
    // coincidence of this specific test, it is what a real,
    // functioning stochastic gate has to produce.
    fn composite_document_blends_two_layers_dissolve_blend_produces_a_real_stochastic_mix() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Dissolve) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        let mut saw_red = false;
        let mut saw_blue = false;
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    (a - 1.0).abs() < epsilon,
                    "backdrop is opaque, result must stay opaque"
                );
                if (r - 1.0).abs() < epsilon && g.abs() < epsilon && b.abs() < epsilon {
                    saw_red = true;
                } else if r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon {
                    saw_blue = true;
                } else {
                    unreachable!(
                        "Dissolve must gate to exactly the backdrop or exactly the source, \
                         never a blended in-between value, got ({r}, {g}, {b}, {a})"
                    );
                }
            }
        }
        assert!(
            saw_red,
            "at least one pixel must show the untouched red backdrop"
        );
        assert!(
            saw_blue,
            "at least one pixel must show the fully opaque blue source"
        );
    }

    #[test]
    // Proves the fix for a real gap an independent review caught: the
    // first pass of the `Dissolve` feature only intercepted it in
    // `resolve_tile`'s `Pixel` branch, so setting `Dissolve` on a
    // *group's* own `blend_mode` silently fell back to `Normal` via
    // `translate_blend_mode` -- exactly the "unimplemented mode" honesty
    // that fallback exists for, except `Dissolve` genuinely *is*
    // implemented, just not (at the time) for this one `LayerKind`. Same
    // shape as the plain-pixel `Dissolve` test above, except the blue
    // "top" layer lives inside a group and the group itself (not the
    // pixel layer within it) carries `Dissolve` at 50% opacity -- the
    // group's own isolated buffer (just the opaque blue pixel layer,
    // Normal-blended with itself) must then be stochastically gated
    // against the red backdrop the same way a plain pixel layer would
    // be, not silently shown at full, ungated opacity.
    fn composite_document_blends_a_dissolve_group_produces_a_real_stochastic_mix() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("dissolve-group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(group, aurora_doc::BlendMode::Dissolve) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(group, 0.5) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        let mut saw_red = false;
        let mut saw_blue = false;
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    (a - 1.0).abs() < epsilon,
                    "backdrop is opaque, result must stay opaque"
                );
                if (r - 1.0).abs() < epsilon && g.abs() < epsilon && b.abs() < epsilon {
                    saw_red = true;
                } else if r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon {
                    saw_blue = true;
                } else {
                    unreachable!(
                        "a Dissolve group must gate to exactly the backdrop or exactly its own \
                         isolated content, never a blended in-between value \
                         (the pre-fix bug would have shown blue at every pixel, ungated), \
                         got ({r}, {g}, {b}, {a})"
                    );
                }
            }
        }
        assert!(
            saw_red,
            "at least one pixel must show the untouched red backdrop -- \
             the pre-fix bug (Dissolve group silently falling back to Normal) \
             would have shown blue everywhere and failed this assertion"
        );
        assert!(
            saw_blue,
            "at least one pixel must show the group's own opaque blue content"
        );
    }

    #[test]
    // Reproducibility at the full document level, not just
    // `dissolve_gate` in isolation: compositing the *same* document
    // twice, in two entirely separate `composite_document` calls, must
    // produce bit-identical `f16` samples -- the property named directly
    // in the task ("the same document must composite identically every
    // time -- on screen, on re-render after scrolling, on export, on
    // reopening a saved file"). Reuses the same two-layer Dissolve
    // document as the stochastic-mix test above.
    fn composite_document_with_a_dissolve_layer_is_bit_identical_across_two_separate_calls() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Dissolve) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(top, 0.3) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 1.0, 0.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let first = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        let second = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(first.samples(), second.samples());
    }

    #[test]
    // The subtlest part of the export path: a layer whose own `bounds`
    // origin isn't the document's `(0, 0)` (a moved layer) must still
    // land at its real document-space position, not silently misalign
    // to the reference tile grid -- the same case
    // `recomposite_visible_tiles_blends_a_layer_at_a_different_origin_than_the_active_layer`
    // proves for the live-canvas path, retargeted at `composite_document`
    // since export has no "active layer" concept to anchor against --
    // every document tile is anchored at document `(0, 0)` instead.
    fn composite_document_places_a_moved_layers_content_at_its_real_document_position() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bottom_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 60,
        };
        let shifted_bounds = aurora_core::Rect {
            x: 40,
            y: 40,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bottom_bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let shifted = match layers.add_pixel_layer("shifted", shifted_bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let Some(bottom_surface) = layers.surface_id(bottom) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(
            &mut store,
            bottom_surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [1.0, 0.0, 0.0, 1.0],
        );
        let Some(shifted_surface) = layers.surface_id(shifted) else {
            unreachable!("just created as a pixel layer");
        };
        // `shifted`'s own bounds start at document (40, 40); painting a
        // solid tile at its own local (0, 0) covers document
        // [40, 40 + TILE) on each axis, same reasoning the
        // `recomposite_visible_tiles` sibling test above documents.
        fill_solid(
            &mut store,
            shifted_surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [0.0, 0.0, 1.0, 1.0],
        );

        let image = match composite_document(&layers, &mut store, 60, 60) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 40, 40),
                [0.0, 0.0, 1.0, 1.0],
                "shifted's own opaque blue must land at its real document position, not (0, 0)"
            );
            assert_eq!(
                image_pixel(&image, 5, 5),
                [1.0, 0.0, 0.0, 1.0],
                "outside shifted's own bounds, only bottom's opaque red should show"
            );
        }
    }

    #[test]
    // A document whose extent isn't a whole number of `aurora_tile::TILE`
    // (256px) needs the same bottom/right partial-tile clamp
    // `aurora_io::read_from_store` already proves
    // (`write_into_store_spanning_multiple_tiles_touches_exactly_the_overlapping_ones`)
    // -- this is `composite_document`'s own version of that same case,
    // now across two tiles in each axis.
    fn composite_document_of_a_non_tile_aligned_document_covers_its_whole_real_extent() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 300,
            height: 300,
        };
        let solid = match layers.add_pixel_layer("solid", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(solid) else {
            unreachable!("just created as a pixel layer");
        };
        for tile in [
            aurora_tile::TileId { x: 0, y: 0 },
            aurora_tile::TileId { x: 1, y: 0 },
            aurora_tile::TileId { x: 0, y: 1 },
            aurora_tile::TileId { x: 1, y: 1 },
        ] {
            fill_solid(&mut store, surface, tile, [0.0, 1.0, 0.0, 1.0]);
        }

        let image = match composite_document(&layers, &mut store, 300, 300) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(image.width(), 300, "the real document width, not 512");
        assert_eq!(image.height(), 300, "the real document height, not 512");
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 299, 299),
                [0.0, 1.0, 0.0, 1.0],
                "the bottom-right corner, inside the partially-covered edge tile"
            );
        }
    }

    #[test]
    // The real end-to-end proof that `aurora_doc::LayerTree::paint_order`'s
    // group-recursion fix actually reaches `composite_document`, not just
    // that `LayerTree` in isolation now returns the right ids -- this is
    // the exact shape of test that would have caught the original bug
    // (a nested pixel layer silently never composited or exported).
    // `outer` (opaque red, full extent) sits at the bottom of the
    // document; `group` (a visible group) sits on top of it and contains
    // one pixel layer, `nested` (opaque green) -- a colour `outer` never
    // has. `nested`'s own opacity/blend mode are both left at their
    // documented defaults (full opacity, `Normal`), so it fully covers
    // `outer` wherever they overlap; asserting the composited output is
    // exactly `nested`'s own green, not `outer`'s red or a blend of the
    // two, proves the nested layer's real pixels reached the real
    // compositor through a real, multi-layer `LayerTree`, not just that
    // `paint_order()` returns the right id in isolation.
    fn composite_document_includes_a_layer_nested_inside_a_group() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let outer = match layers.add_pixel_layer("outer", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let nested = match layers.add_pixel_layer("nested", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // group was added after outer, so it's the topmost root -- its
        // own contents must paint over outer.
        assert_eq!(layers.roots(), [group, outer]);
        assert_eq!(layers.children(group), Some([nested].as_slice()));

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (outer, [1.0, 0.0, 0.0, 1.0]),
            (nested, [0.0, 1.0, 0.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.0, 1.0, 0.0, 1.0],
                "the nested layer's own green must reach the real composite, over outer's red"
            );
        }
    }

    #[test]
    // The negative counterpart to
    // `composite_document_includes_a_layer_nested_inside_a_group`: an
    // *invisible* group must still hide its whole subtree in the real,
    // end-to-end export path -- not just in `LayerTree::paint_order`'s
    // own isolated return value.
    fn composite_document_excludes_a_layer_nested_inside_an_invisible_group() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let outer = match layers.add_pixel_layer("outer", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let nested = match layers.add_pixel_layer("nested", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_visible(group, false) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (outer, [1.0, 0.0, 0.0, 1.0]),
            (nested, [0.0, 1.0, 0.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [1.0, 0.0, 0.0, 1.0],
                "the invisible group's nested green layer must not reach the real composite"
            );
        }
    }

    #[test]
    // Distinguishes "the fix works" from "still ignoring group settings":
    // `group` (opacity 0.5, `Normal`) contains one opaque blue child at
    // its own documented-default opacity (1.0)/blend mode (`Normal`).
    // `background` (opaque green) sits at the root, below `group`.
    //
    // Hand-computed, following `resolve_tile`'s own isolate-then-apply
    // semantic: `group`'s own isolated buffer is exactly its child's
    // opaque blue (compositing one full-opacity `Normal` layer onto a
    // transparent backdrop reproduces that layer's own texels exactly,
    // `composite_tile_cpu`'s own documented property) -- then that
    // isolated blue buffer is composited over `background`'s green using
    // `group`'s *own* opacity (0.5): straight-alpha "over" an *opaque*
    // backdrop reduces to `alpha*src + (1-alpha)*dst` per channel, so
    // r = 0.5*0 + 0.5*0 = 0.0, g = 0.5*0 + 0.5*1 = 0.5,
    // b = 0.5*1 + 0.5*0 = 0.5, a = 1.0 -> (0.0, 0.5, 0.5, 1.0), a real
    // 50/50 blue-green blend.
    //
    // If group opacity were still being ignored (the pre-fix bug this
    // test exists to catch), the child would composite at its own full
    // opacity as if it were a root layer, fully occluding `background`:
    // the result would be pure blue (0.0, 0.0, 1.0, 1.0), not the
    // blended value asserted below.
    fn composite_document_applies_a_groups_own_opacity_to_its_isolated_children() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match layers.add_pixel_layer("child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(group, 0.5) {
            unreachable!("{err:?}");
        }
        // `blend_mode` defaults to `Normal` and is left untouched --
        // this test isolates the opacity aggregation specifically.
        assert_eq!(layers.roots(), [group, background]);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [0.0, 1.0, 0.0, 1.0]),
            (child, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [0.0, 0.5, 0.5, 1.0],
                "group opacity must attenuate its own isolated composite, not be ignored"
            );
            assert_ne!(
                result,
                [0.0, 0.0, 1.0, 1.0],
                "pure blue would mean the child ignored the group's own opacity entirely"
            );
        }
    }

    #[test]
    // Proves `resolve_tile`'s own un-premultiply fix (Finding 1): `group`
    // is left at its own documented defaults (opacity 1.0, `Normal`) and
    // contains one **translucent** (`opacity = 0.5`) opaque-blue
    // (0, 0, 1) child. `background` is opaque white, below `group` at
    // the root.
    //
    // Hand-computed, following `resolve_tile`'s own doc comment:
    //   `group`'s own isolated buffer, pre-fix (premultiplied, the bug):
    //     compositing the translucent blue child alone onto a
    //     transparent backdrop gives alpha = 0.5, then
    //     b = inverse(0)*0 + alpha(0.5)*blended_b(1.0) = 0.5,
    //     so `(0, 0, 0.5, 0.5)` -- note the blue channel (0.5) does not
    //     equal the straight colour (1.0) at this alpha; that's the
    //     premultiplied contamination the fix removes.
    //   `group`'s own isolated buffer, post-fix (straight, un-
    //     premultiplied): b = 0.5 / 0.5 = 1.0, so `(0, 0, 1.0, 0.5)` --
    //     the child's own true straight colour (opaque blue) at its own
    //     0.5 alpha, exactly what un-premultiplying should recover.
    //   Root composite: `background`'s opaque white (1, 1, 1, 1), then
    //     `group`'s own straight `(0, 0, 1.0, 0.5)` composited at
    //     `group`'s own opacity 1.0 over that *opaque* backdrop --
    //     alpha = 0.5, inverse = 0.5, backdrop_alpha = 1.0,
    //     backdrop_inverse = 0.0, so per channel:
    //     r = 0.5*1 + 0.5*0 = 0.5, g = 0.5*1 + 0.5*0 = 0.5,
    //     b = 0.5*1 + 0.5*1.0 = 1.0, a = 0.5 + 1.0*0.5 = 1.0
    //     -> `(0.5, 0.5, 1.0, 1.0)`, bit-for-bit the same result a flat
    //     (non-isolated) composite of a single 0.5-opacity blue layer
    //     over opaque white would give -- proving isolation is no longer
    //     lossy for this shape.
    //
    // Before the fix, the *premultiplied* `(0, 0, 0.5, 0.5)` would have
    // been composited instead: alpha = 0.5, inverse = 0.5,
    // backdrop_alpha = 1.0, backdrop_inverse = 0.0, so
    // b = 0.5*1 + 0.5*0.5 = 0.75 (the other channels unchanged) --
    // `(0.5, 0.5, 0.75, 1.0)`, double-attenuated blue, the exact bug
    // Finding 1 fixes.
    fn composite_document_un_premultiplies_a_groups_own_translucent_isolated_child() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match layers.add_pixel_layer("child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(child, 0.5) {
            unreachable!("{err:?}");
        }
        // `group` itself is left at its own documented defaults
        // (opacity 1.0, `Normal`) -- this test isolates the
        // un-premultiply fix specifically, not group-level aggregation
        // (already covered by the sibling opacity/blend-mode tests
        // above).
        assert_eq!(layers.roots(), [group, background]);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 1.0, 1.0, 1.0]),
            (child, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [0.5, 0.5, 1.0, 1.0],
                "un-premultiplying the group's own isolated buffer must reproduce the flat \
                 (non-isolated) result exactly"
            );
            assert_ne!(
                result,
                [0.5, 0.5, 0.75, 1.0],
                "0.75 blue would mean the isolated buffer is still premultiplied \
                 (double-attenuated), the pre-fix Finding-1 bug"
            );
        }
    }

    #[test]
    // The root-level sibling of the group test above, and AC-1's own
    // regression test: the identical un-premultiply step that arm has
    // always run was missing from `composite_roots_into_tile`, the
    // shared root-level fold that both `composite_document` (export)
    // and `recomposite_visible_tiles`' own CPU path go through.
    //
    // Fixture: one opaque-white pixel layer at layer opacity 0.5,
    // root-level, over nothing (an otherwise empty document, so the
    // accumulator it folds onto is `transparent_tile`'s own fully
    // transparent black -- exactly the case that makes the fold's
    // premultiplied-out contract visible).
    //
    // Hand-computed, straight-alpha "over" onto transparent black:
    // `as = src_a * opacity = 1.0 * 0.5 = 0.5`, so each colour channel
    // is `0.5*0.0 + 0.5*1.0 = 0.5` and alpha is `0.5 + 0.0*0.5 = 0.5`
    // -- a premultiplied `(0.5, 0.5, 0.5, 0.5)`. Straightened, that is
    // `(1.0, 1.0, 1.0, 0.5)`: the layer really is opaque white, shown at
    // half opacity, and the colour channels must say so.
    //
    // This is the value that reaches an exported PNG/TIFF/`.aur` file
    // and the eyedropper, so the pre-fix `(0.5, 0.5, 0.5, 0.5)` was a
    // real, user-visible wrong colour in every export with translucent
    // content -- asserted against explicitly below so this test would
    // have failed before the fix rather than merely not covering it.
    //
    // Fully headless: no GPU adapter needed, unlike the
    // `recomposite_visible_tiles` sibling below.
    fn composite_document_un_premultiplies_a_translucent_root_level_layer() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let layer = match layers.add_pixel_layer("translucent", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(layer, 0.5) {
            unreachable!("{err:?}");
        }
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(
            &mut store,
            surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [1.0, 1.0, 1.0, 1.0],
        );

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [1.0, 1.0, 1.0, 0.5],
                "an opaque-white layer at 50% opacity is straight-alpha white at half alpha"
            );
            assert_ne!(
                result,
                [0.5, 0.5, 0.5, 0.5],
                "the premultiplied value the root-level fold leaves behind -- what every \
                 export of translucent content carried before 0.52.0"
            );
        }
    }

    #[test]
    // Both levels of straightening in one fixture -- the case neither of
    // the two tests above covers, because each exercises only one of
    // them: `resolve_tile`'s `Group` arm straightens a group's isolated
    // buffer, `composite_roots_into_tile` straightens the finished root
    // accumulator, and only a document that ends fractional at *both*
    // levels can tell a double-straighten (or a straighten at the wrong
    // level) from correct behaviour.
    //
    // Fixture: one group at opacity 0.5, holding a single opaque-blue
    // (0, 0, 1, 1) child pixel layer, as the document's only root-level
    // layer, over nothing (a transparent background).
    //
    // Hand-computed, level by level:
    //   * the group's isolated buffer: one full-opacity `Normal` child
    //     folded onto transparent black reproduces the child exactly, so
    //     the accumulator is (0, 0, 1, 1) -- already opaque, so the
    //     `Group` arm's own straightening divides by one and is an exact
    //     identity here, leaving (0, 0, 1, 1).
    //   * folded into the root at the group's own opacity 0.5, onto
    //     transparent black: `as = 1.0 * 0.5 = 0.5`, so
    //     b = 0.5*0 + 0.5*1 = 0.5 and a = 0.5 + 0*0.5 = 0.5 -- a
    //     premultiplied root accumulator of (0, 0, 0.5, 0.5).
    //   * `composite_roots_into_tile`'s own straightening: 0.5 / 0.5 =
    //     1.0, giving the finished (0, 0, 1.0, 0.5).
    //
    // The two wrong answers are asserted against by name, because each
    // is a distinguishable failure signature: (0, 0, 2.0, 0.5) is what a
    // *double* straighten produces (the group's buffer divided by the
    // root's 0.5 alpha as well as its own), and (0, 0, 0.5, 0.5) is what
    // a *missing* root-level straighten produces -- the premultiplied
    // value the fold leaves behind.
    fn composite_document_straightens_a_fractional_group_and_a_fractional_root_fold_exactly_once() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match layers.add_pixel_layer("child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(group, 0.5) {
            unreachable!("{err:?}");
        }
        assert_eq!(
            layers.roots(),
            [group],
            "the group must be the document's only root-level layer, so the root fold ends \
             at the group's own fractional alpha rather than on top of an opaque backdrop"
        );

        let Some(surface) = layers.surface_id(child) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(
            &mut store,
            surface,
            aurora_tile::TileId { x: 0, y: 0 },
            [0.0, 0.0, 1.0, 1.0],
        );

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [0.0, 0.0, 1.0, 0.5],
                "an opaque-blue child in a 50%-opacity group is straight-alpha pure blue at \
                 half alpha"
            );
            assert_ne!(
                result,
                [0.0, 0.0, 2.0, 0.5],
                "a blue channel above 1.0 would mean the buffer was straightened twice"
            );
            assert_ne!(
                result,
                [0.0, 0.0, 0.5, 0.5],
                "the premultiplied value would mean the root-level straightening is missing"
            );
        }
    }

    #[test]
    // The blend-mode counterpart to
    // `composite_document_applies_a_groups_own_opacity_to_its_isolated_children`:
    // `group` (opacity 1.0, `Multiply`) contains one opaque pure-blue
    // child (0, 0, 1) at its own documented defaults (opacity 1.0,
    // `Normal`). `background` is opaque pure-green (0, 1, 0), below
    // `group` at the root.
    //
    // Hand-computed: `group`'s own isolated buffer is exactly opaque
    // blue (same reasoning as the opacity test above -- compositing over
    // a transparent backdrop reproduces the single child's own texels
    // regardless of mode, since the backdrop contributes nothing).  That
    // isolated blue buffer is then composited over `background` using
    // `group`'s *own* `Multiply` -- over an opaque backdrop,
    // `Multiply(Cb, Cs)` per channel: (0*0, 1*0, 0*1) = (0, 0, 0), so the
    // real result is opaque black: (0.0, 0.0, 0.0, 1.0).
    //
    // If the group's own blend mode were still being ignored, the child
    // would composite directly with its *own* `Normal` mode, giving pure
    // blue (0.0, 0.0, 1.0, 1.0), not black -- proving real isolate-then-
    // blend happened, not per-child blending with the group's mode
    // merely read and discarded.
    fn composite_document_applies_a_groups_own_blend_mode_to_its_isolated_result() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match layers.add_pixel_layer("child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(group, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        assert_eq!(layers.roots(), [group, background]);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [0.0, 1.0, 0.0, 1.0]),
            (child, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [0.0, 0.0, 0.0, 1.0],
                "group's own Multiply must apply to its isolated result, not be ignored"
            );
            assert_ne!(
                result,
                [0.0, 0.0, 1.0, 1.0],
                "pure blue would mean the child used its own Normal mode instead of the group's Multiply"
            );
        }
    }

    #[test]
    // End-to-end regression test for the gap `resolve_tile`'s own doc
    // comment used to name as "still genuinely open, and this fix does
    // not reach it" (now fixed — see that doc comment's current text):
    // when a group's own children combine via a **non-`Normal` blend
    // mode against each other** while the group's own isolation buffer
    // is still translucent partway through, `composite_tile_cpu`'s
    // `blend_rgb` used to see the raw, still-premultiplied accumulator
    // as `Cb` instead of its true straight-alpha colour. This exercises
    // that path through a real `LayerTree`/`composite_document` call,
    // not just the `aurora-render`-level unit test
    // (`composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator`)
    // that proves the same fix at the `composite_tile_cpu` level alone.
    //
    // `group` (left at its own default settings, opacity 1.0/`Normal`)
    // contains two children: `bottom_child` (opacity 0.5, `Normal`,
    // straight colour (1.0, 0.5, 0.25)) and, on top of it,
    // `top_child` (opacity 1.0, fully opaque, `Multiply`, straight
    // colour (0.5, 0.5, 0.75)). `background`, below `group` at the
    // root, is opaque white -- irrelevant to the final result here,
    // since `group`'s own isolated buffer ends up fully opaque
    // (`bottom_child` alone already brings the accumulator's alpha to
    // 0.5, and `top_child`'s own alpha is 1.0, so the accumulated alpha
    // after both children is 1.0), so it fully occludes `background`
    // once composited one level up.
    //
    // Hand-computed exactly as `composite_tile_cpu_recovers_the_true_straight_alpha_backdrop_for_a_still_translucent_accumulator`'s
    // own doc comment (`aurora-render`) works out, since this is the
    // same two-layer shape run through `resolve_tile`'s own group
    // isolation: `bottom_child` alone onto the isolation buffer's
    // starting fully-transparent state gives a *premultiplied*
    // accumulator, (0.5, 0.25, 0.125, 0.5) -- true straight colour
    // (1.0, 0.5, 0.25) at alpha 0.5. `top_child`'s `Multiply` against
    // the *correct*, recovered backdrop gives
    // (1.0*0.5, 0.5*0.5, 0.25*0.75) = (0.5, 0.25, 0.1875); since
    // `top_child` is fully opaque the "over" formula collapses to
    // `Co = (1-backdrop_alpha)*Cs + backdrop_alpha*B(Cb,Cs)`, giving
    // (0.5, 0.375, 0.46875) at alpha 1.0. `group`'s own isolation
    // buffer is already fully opaque, so the un-premultiply step above
    // (dividing by alpha = 1.0) is a no-op, and `group`'s own default
    // opacity 1.0/`Normal` reproduces that buffer exactly one level up,
    // fully occluding `background`. Expected final pixel:
    // (0.5, 0.375, 0.46875, 1.0).
    //
    // Before this fix, `Multiply` would have reacted to the *raw*,
    // still-premultiplied accumulator, (0.5, 0.25, 0.125), instead:
    // (0.5*0.5, 0.25*0.5, 0.125*0.75) = (0.25, 0.125, 0.09375), giving
    // (0.375, 0.3125, 0.421875, 1.0) -- silently wrong in every
    // channel, and explicitly asserted against below.
    fn composite_document_blends_two_group_children_via_a_non_normal_blend_mode_against_a_translucent_backdrop()
     {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // `add_pixel_layer` inserts as the new *topmost* child, so
        // `bottom_child` (added first) ends up below `top_child` (added
        // second) -- `children(group) == [top_child, bottom_child]`,
        // which `resolve_tile`'s own `.rev()` turns into bottom-to-top
        // order for `composite_tile_cpu`, exactly the shape this test's
        // own hand-computation above assumes.
        let bottom_child = match layers.add_pixel_layer("bottom_child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top_child = match layers.add_pixel_layer("top_child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(bottom_child, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_blend_mode(top_child, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        assert_eq!(layers.roots(), [group, background]);
        let Some(children) = layers.children(group) else {
            unreachable!("group was just created with two children");
        };
        assert_eq!(children, [top_child, bottom_child]);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 1.0, 1.0, 1.0]),
            (bottom_child, [1.0, 0.5, 0.25, 1.0]),
            (top_child, [0.5, 0.5, 0.75, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            let result = image_pixel(&image, 0, 0);
            assert_eq!(
                result,
                [0.5, 0.375, 0.46875, 1.0],
                "Multiply between two group children must react to the true straight-alpha \
                 backdrop colour, not the raw premultiplied accumulator, even while the \
                 group's own isolation buffer is still translucent partway through"
            );
            assert_ne!(
                result,
                [0.375, 0.3125, 0.421_875, 1.0],
                "this is the pre-fix value: Multiply run directly against the raw \
                 premultiplied accumulator instead of its recovered straight colour"
            );
        }
    }

    #[test]
    // Proves the recursion actually recurses, not just one level:
    // `outer_group` (opacity 0.5, `Normal`) contains `inner_group`
    // (opacity 0.5, `Normal`), which contains one opaque pure-red pixel
    // child `x` at its own documented defaults. `background` is opaque
    // white, below `outer_group` at the root.
    //
    // Hand-computed with `resolve_tile`'s own un-premultiply fix applied
    // at *every* group level (it runs bottom-up, so `inner_group`'s own
    // return is fixed before `outer_group` ever uses it, and
    // `outer_group`'s own return is fixed before the root uses it):
    //   inner_group's own isolated buffer: opaque red (1, 0, 0, 1) --
    //     one full-opacity `Normal` child over a transparent backdrop
    //     reproduces itself exactly (alpha = 1, so un-premultiplying is a
    //     no-op here).
    //   outer_group's own isolated buffer, pre-un-premultiply: inner_
    //     group's own red buffer composited (as `inner_group`'s own
    //     opacity 0.5, `Normal`) onto a transparent backdrop ->
    //     (0.5, 0.0, 0.0, 0.5), premultiplied (0.5 red at 0.5 alpha, not
    //     straight red at 0.5 alpha). Un-premultiplied:
    //     r = 0.5 / 0.5 = 1.0 -> (1.0, 0.0, 0.0, 0.5), straight red at
    //     0.5 alpha -- `outer_group`'s own real isolated content.
    //   Root composite: `background`'s opaque white, then that straight
    //     `(1.0, 0.0, 0.0, 0.5)` composited at outer_group's own opacity
    //     0.5 over that *opaque* white backdrop -- alpha = 0.5*0.5 =
    //     0.25, inverse = 0.75:
    //     r = 0.75*1 + 0.25*1.0 = 1.0, g = 0.75*1 + 0.25*0 = 0.75,
    //     b = 0.75*1 + 0.25*0 = 0.75, a = 1.0
    //     -> (1.0, 0.75, 0.75, 1.0) -- exactly what directly compositing
    //     `x` at its own combined effective opacity (0.5 * 0.5 = 0.25)
    //     over white would give, confirming two un-premultiplied
    //     isolation levels compound correctly.
    //
    // Before the un-premultiply fix, `outer_group`'s own *premultiplied*
    // `(0.5, 0.0, 0.0, 0.5)` was handed to the root as if straight,
    // double-attenuating the red channel a second time:
    // r = 0.75*1 + 0.25*0.5 = 0.875 -- the old, buggy expected value
    // this test used to assert, now known to be Finding 1's same bug
    // surfacing one level up.
    fn composite_document_recurses_two_levels_of_nested_groups() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let outer_group = match layers.add_group("outer_group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let inner_group = match layers.add_group("inner_group", Some(outer_group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let x = match layers.add_pixel_layer("x", bounds, Some(inner_group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(outer_group, 0.5) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_opacity(inner_group, 0.5) {
            unreachable!("{err:?}");
        }
        assert_eq!(layers.roots(), [outer_group, background]);
        assert_eq!(layers.children(outer_group), Some([inner_group].as_slice()));
        assert_eq!(layers.children(inner_group), Some([x].as_slice()));

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 1.0, 1.0, 1.0]),
            (x, [1.0, 0.0, 0.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [1.0, 0.75, 0.75, 1.0],
                "two levels of nested, non-default-opacity groups must both actually apply, \
                 with each level's own isolated buffer correctly un-premultiplied"
            );
        }
    }
    /// How many ordinary sibling pixel layers the many-sibling group
    /// test below puts inside one group — the exact count PLAN.md's own
    /// diagnosis measured at ~1 GB RSS before `resolve_tile` folded its
    /// children in one at a time instead of collecting every sibling's
    /// full tile buffer first.
    const SIBLINGS: usize = 2_000;

    /// How many ordinary *root-level* pixel layers the no-group version
    /// of that test below uses. Deliberately a quarter of `SIBLINGS`:
    /// this one covers a second, separate pair of fold sites
    /// (`recomposite_visible_tiles`' CPU path and `composite_document`'s
    /// export loop, which had the identical collect-all-first shape and
    /// now share `composite_roots_into_tile`), and 500 is already an
    /// unmistakable separation — the same binary, this test alone,
    /// measured at **~264 MiB** peak RSS with the export loop reverted
    /// to its collect-all-first form and **~15 MiB** with the fold in
    /// place, independently across two separate reviewers' runs (264.3 /
    /// 15.1, then 264.2 / 15.3 on re-verification) — the exact figure
    /// moves by a fraction of a megabyte between runs (RSS measurement
    /// noise, not a regression signal), the two-orders-of-magnitude drop
    /// does not — without paying a second time for 2,000 real scratch-tile
    /// writes on every CI platform. The 2,000 figure is kept for the
    /// group test alone, where it matches the scenario PLAN.md actually
    /// measured.
    const ROOT_SIBLINGS: usize = 500;

    #[test]
    // The regression test for the *other* two fold sites: no group is
    // involved at all. `resolve_tile`'s `Group` arm was not the only
    // place that collected one full `aurora_tile::SAMPLES`-length `f16`
    // buffer (512 KiB) per contributor before a single batch composite —
    // `recomposite_visible_tiles`' own CPU closure and
    // `composite_document`'s export loop both did it over
    // `layers.roots()`, so a flat document with no groups at all reached
    // the same peak-memory shape. Review 2026-08-24 noted the original
    // memory test exercised only the `Group` arm; this one goes through
    // `composite_document` directly, which is the export path and always
    // runs on the CPU whatever the GPU situation is.
    //
    // Same construction and same reasoning as the group test below: only
    // the bottom-most and top-most of the `ROOT_SIBLINGS` layers are
    // filled, and the untouched ones still materialise a real, full,
    // blank tile through `TileStore::get`, so all `ROOT_SIBLINGS`
    // buffers are genuinely resolved and folded while contributing
    // nothing (an `alpha = 0` source is an exact identity in
    // `aurora_render::composite_layer_into`).
    //
    // Hand-computed: roots fold bottom-to-top (`roots().iter().rev()`,
    // and `roots()` is newest-first, so the first-added layer is
    // bottom-most). Opaque blue lands first over the transparent start
    // and reproduces itself exactly; the 498 blank layers are exact
    // no-ops; opaque green at layer opacity 0.5 folds last over an
    // opaque blue backdrop, where straight-alpha "over" reduces to
    // `alpha*src + (1-alpha)*dst`: r = 0.5*0 + 0.5*0 = 0.0,
    // g = 0.5*1 + 0.5*0 = 0.5, b = 0.5*0 + 0.5*1 = 0.5,
    // a = 0.5 + 1*0.5 = 1.0 -> (0.0, 0.5, 0.5, 1.0). The accumulator
    // ends fully opaque, so `composite_roots_into_tile`'s own
    // un-premultiply step (0.52.0) divides by one and is an exact
    // identity here -- which is why this expectation is unchanged across
    // that fix, and equally why an opaque-only fixture like this one
    // could never have caught the gap it closed. The fractional-alpha
    // sibling that does catch it is
    // `composite_document_un_premultiplies_a_translucent_root_level_layer`.
    fn composite_document_composites_five_hundred_root_level_sibling_layers() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let mut roots = Vec::with_capacity(ROOT_SIBLINGS);
        for i in 0..ROOT_SIBLINGS {
            let id = match layers.add_pixel_layer(format!("root {i}"), bounds, None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            roots.push(id);
        }
        let (Some(&bottom), Some(&top)) = (roots.first(), roots.last()) else {
            unreachable!("ROOT_SIBLINGS is a non-zero constant");
        };
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }
        // Nothing but the root layers themselves, and `CompositeBudget`
        // is seeded to exactly `layers.len()` with no slack — so a
        // future edit that changes this fixture's shape fails here
        // loudly rather than silently truncating the walk partway.
        assert_eq!(layers.len(), ROOT_SIBLINGS);
        assert_eq!(layers.roots().len(), ROOT_SIBLINGS);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [0.0, 0.0, 1.0, 1.0]), (top, [0.0, 1.0, 0.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.0, 0.5, 0.5, 1.0],
                "{ROOT_SIBLINGS} root-level sibling layers, no group involved, must still \
                 composite to the exact hand-computed blend of the two filled ones"
            );
        }
    }

    #[test]
    // The direct regression test for `resolve_tile`'s own peak-memory
    // shape: one group holding `SIBLINGS` ordinary pixel-layer children,
    // on a trivial 10x10 document. Before the fold-in-place fix, the
    // `Group` arm collected one full `aurora_tile::SAMPLES`-length `f16`
    // buffer (512 KiB) per child before compositing them all in a single
    // batch call, so peak memory scaled with sibling count rather than
    // with nesting depth — reachable by nothing more exotic than adding
    // a lot of layers.
    //
    // This is a *correctness* test that happens to exercise that shape:
    // it passes against the pre-fix code too (the fix is bit-identical
    // code motion, not a behaviour change), and what it guards is that
    // folding children in one at a time still produces the exact same
    // pixel. Only two of the `SIBLINGS` children are filled; the rest
    // are left untouched, which is deliberate — `TileStore::get` returns
    // a real, full, blank tile for an untouched surface, so all
    // `SIBLINGS` real buffers are genuinely materialised and folded,
    // while an `alpha = 0` source is an exact identity in
    // `aurora_render::composite_layer_into` (`alpha = 0` makes
    // `inverse = 1`, so every channel writes back its own current value
    // and the alpha accumulation is `0 + da * 1`) and contributes
    // nothing to the expected value.
    //
    // Hand-computed, following `resolve_tile`'s own isolate-then-apply
    // semantic (same style as
    // `composite_document_applies_a_groups_own_opacity_to_its_isolated_children`):
    // children fold bottom-up, so the first-added child (opaque blue)
    // lands first, reproducing itself exactly over the transparent
    // start; the 1,998 blank children are exact no-ops; the last-added
    // child (opaque green at layer opacity 0.5) folds last over an
    // opaque blue backdrop, and straight-alpha "over" an opaque backdrop
    // reduces to `alpha*src + (1-alpha)*dst` per channel:
    // r = 0.5*0 + 0.5*0 = 0.0, g = 0.5*1 + 0.5*0 = 0.5,
    // b = 0.5*0 + 0.5*1 = 0.5, a = 0.5 + 1*0.5 = 1.0 -> the group's own
    // isolated buffer is (0.0, 0.5, 0.5, 1.0). It is already opaque, so
    // both un-premultiply steps it passes through (the `Group` arm's own
    // and, one level up, `composite_roots_into_tile`'s) are the
    // identity, and the group's own
    // documented defaults (opacity 1.0, `Normal`) composite that fully
    // opaque buffer over the opaque red background unchanged.
    fn composite_document_composites_two_thousand_sibling_layers_in_one_group() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // `group` is added after `background`, so it sits above it.
        assert_eq!(layers.roots(), [group, background]);

        let mut children = Vec::with_capacity(SIBLINGS);
        for i in 0..SIBLINGS {
            let child = match layers.add_pixel_layer(format!("child {i}"), bounds, Some(group)) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            children.push(child);
        }
        let (Some(&bottom), Some(&top)) = (children.first(), children.last()) else {
            unreachable!("SIBLINGS is a non-zero constant");
        };
        if let Err(err) = layers.set_opacity(top, 0.5) {
            unreachable!("{err:?}");
        }
        // Every layer this document has: the background, the group, and
        // its children. `CompositeBudget` is seeded to exactly
        // `layers.len()` nodes with no slack, so a future edit that
        // changes the fixture's shape must fail here loudly rather than
        // silently truncating the walk partway through the siblings.
        assert_eq!(layers.len(), SIBLINGS + 2);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 0.0, 0.0, 1.0]),
            (bottom, [0.0, 0.0, 1.0, 1.0]),
            (top, [0.0, 1.0, 0.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [0.0, 0.5, 0.5, 1.0],
                "a group with {SIBLINGS} ordinary sibling children must still composite to \
                 the exact hand-computed blend of the two filled ones"
            );
        }
    }

    /// Builds `layers` into: an opaque white full-coverage background
    /// pixel layer at root level, plus a chain of `groups` nested groups
    /// (the outermost at root level, i.e. depth 1, so the innermost sits
    /// at depth `groups`) with one opaque red full-coverage pixel layer
    /// inside the innermost group, at depth `groups + 1`. Both pixel
    /// layers are filled in `store`. Returns nothing but the filled
    /// tree/store — the caller composites and inspects the result.
    ///
    /// Shared by the two depth-bound tests below so the *only*
    /// difference between them is the one extra level of nesting.
    fn nested_group_chain_over_a_white_background(
        layers: &mut aurora_doc::LayerTree,
        store: &mut aurora_tile::TileStore,
        groups: usize,
    ) {
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        // Depth 1 is the outermost group (`parent: None`), matching the
        // seed every real `resolve_tile` call site passes for a root.
        let mut parent = None;
        for level in 0..groups {
            parent = match layers.add_group(format!("group-{level}"), parent) {
                Ok(id) => Some(id),
                Err(err) => unreachable!("{err:?}"),
            };
        }
        // For every legal `groups` the innermost pixel layer lands at
        // `groups + 1 <= MAX_LAYER_TREE_DEPTH`, so the real, guarded API
        // builds it. Only the over-deep caller below asks for a layer at
        // `MAX_LAYER_TREE_DEPTH + 1`, which `add_pixel_layer` now
        // rightly refuses (0.50.0) -- that one branch goes through
        // `aurora-doc`'s `test-support` escape hatch instead, which is
        // the only way left to build the genuinely over-deep tree
        // `resolve_tile`'s own independent depth guard is defence
        // against. The group chain itself stays on the real API in both
        // cases: even at `groups == MAX_LAYER_TREE_DEPTH` every group
        // lands at depth <= 256.
        let deepest_result = if groups < aurora_doc::MAX_LAYER_TREE_DEPTH {
            layers.add_pixel_layer("deepest", bounds, parent)
        } else {
            layers.insert_pixel_ignoring_the_depth_limit("deepest", bounds, parent)
        };
        let deepest = match deepest_result {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 1.0, 1.0, 1.0]),
            (deepest, [1.0, 0.0, 0.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(store, surface, tile_id, rgba);
        }
    }

    #[test]
    // The boundary itself, isolated from any nesting: one ordinary
    // visible pixel layer, resolved twice at two adjacent depths. This
    // pins the exact comparison operator (`>`, not `>=`) — the one
    // thing most likely to silently drift from `aurora-doc`'s own
    // `validate_shape`, which rejects with `depth > MAX_LAYER_TREE_DEPTH`
    // after seeding roots at 1.
    fn resolve_tile_refuses_to_recurse_past_the_layer_tree_depth_bound() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let id = match layers.add_pixel_layer(
            "solo",
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            None,
        ) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(&mut store, surface, tile_id, [1.0, 0.0, 0.0, 1.0]);

        assert!(
            resolve_tile(
                id,
                &layers,
                &mut store,
                tile_id,
                (0, 0),
                (0, 0),
                aurora_doc::MAX_LAYER_TREE_DEPTH,
                &mut CompositeBudget::for_pass(&layers),
            )
            .is_some(),
            "a layer sitting exactly at the maximum tree depth is still a legitimate \
             contributor and must resolve"
        );
        assert!(
            resolve_tile(
                id,
                &layers,
                &mut store,
                tile_id,
                (0, 0),
                (0, 0),
                aurora_doc::MAX_LAYER_TREE_DEPTH + 1,
                &mut CompositeBudget::for_pass(&layers),
            )
            .is_none(),
            "one level past the maximum tree depth must be refused outright"
        );
    }

    #[test]
    // The "no regression" half of the depth bound, end to end through a
    // real composite: 255 nested groups (depths 1..=255) with an opaque
    // red pixel layer at depth 256 — exactly `MAX_LAYER_TREE_DEPTH`, the
    // deepest a tree `aurora-doc` accepts can legitimately go. The red
    // must survive to the composited image.
    fn composite_document_still_includes_a_layer_at_exactly_the_maximum_tree_depth() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        // `- 1` groups, because the pixel layer inside them occupies the
        // last level of the budget itself.
        nested_group_chain_over_a_white_background(
            &mut layers,
            &mut store,
            aurora_doc::MAX_LAYER_TREE_DEPTH - 1,
        );

        // This is what proves `resolve_tile`'s bound and `aurora-doc`'s
        // bound actually agree, rather than that a number was picked
        // that happens to work: the very same tree must round-trip
        // through `LayerTree`'s own `#[serde(try_from = "LayerTreeRepr")]`
        // validation, which is where `validate_shape` runs.
        let bytes = match postcard::to_allocvec(&layers) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            postcard::from_bytes::<aurora_doc::LayerTree>(&bytes).is_ok(),
            "a tree this deep is accepted by `aurora-doc`'s own validator, so `resolve_tile` \
             must not be stricter than it"
        );

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [1.0, 0.0, 0.0, 1.0],
                "the layer at exactly the maximum tree depth must still be composited, \
                 covering the white background with its own opaque red"
            );
        }
    }

    #[test]
    // The other half: one more level of nesting than the maximum. The
    // over-deep branch must be dropped — the assertion is on the *white*
    // background showing through untouched, not merely on the call
    // returning, so this proves the branch was treated as absent rather
    // than that nothing crashed.
    fn composite_document_drops_the_branch_that_nests_one_level_past_the_maximum_tree_depth() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        // One more group than the test above, so the red pixel layer
        // lands at `MAX_LAYER_TREE_DEPTH + 1`.
        nested_group_chain_over_a_white_background(
            &mut layers,
            &mut store,
            aurora_doc::MAX_LAYER_TREE_DEPTH,
        );

        // The mirror of the round trip above: `aurora-doc` refuses this
        // tree, which is exactly why `resolve_tile` must refuse it too.
        // `postcard`'s own error type discards the underlying
        // `DocError`, so only `is_err()` can be asserted here — the
        // specific `LayerTreeTooDeep` variant is pinned by
        // `aurora-doc`'s own tests instead.
        let bytes = match postcard::to_allocvec(&layers) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            postcard::from_bytes::<aurora_doc::LayerTree>(&bytes).is_err(),
            "a tree one level past the maximum is rejected by `aurora-doc`'s own validator"
        );

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                image_pixel(&image, 0, 0),
                [1.0, 1.0, 1.0, 1.0],
                "the over-deep branch must be dropped from the composite entirely, leaving \
                 the white background untouched"
            );
        }
    }

    /// Builds a perfect binary tree of groups `levels` deep under one
    /// new root group, with one opaque full-coverage pixel layer at
    /// every leaf, and returns that root's id. `levels = 3` gives 7
    /// groups and 8 pixel layers, i.e. 15 nodes reachable from the root
    /// and nothing else in the tree.
    ///
    /// The shape matters: this is the *fan-out* direction, the one the
    /// depth counter alone says nothing about, so it is what the node
    /// budget's own tests are built on.
    fn balanced_group_tree(
        layers: &mut aurora_doc::LayerTree,
        store: &mut aurora_tile::TileStore,
        levels: usize,
    ) -> aurora_doc::LayerId {
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let root = match layers.add_group("root", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mut frontier = vec![root];
        for level in 1..levels {
            let mut next = Vec::new();
            for parent in frontier {
                for branch in 0..2 {
                    match layers.add_group(format!("g-{level}-{branch}"), Some(parent)) {
                        Ok(id) => next.push(id),
                        Err(err) => unreachable!("{err:?}"),
                    }
                }
            }
            frontier = next;
        }
        for (leaf, parent) in frontier.into_iter().enumerate() {
            for branch in 0..2 {
                let id = match layers.add_pixel_layer(
                    format!("leaf-{leaf}-{branch}"),
                    bounds,
                    Some(parent),
                ) {
                    Ok(id) => id,
                    Err(err) => unreachable!("{err:?}"),
                };
                let Some(surface) = layers.surface_id(id) else {
                    unreachable!("just created as a pixel layer");
                };
                fill_solid(
                    store,
                    surface,
                    aurora_tile::TileId { x: 0, y: 0 },
                    [0.0, 1.0, 0.0, 1.0],
                );
            }
        }
        root
    }

    #[test]
    // The node budget's "no false positives" half, and the reason it is
    // sized from the document rather than from a constant: a legitimate
    // tree that is *wide* as well as deep must composite in full, and
    // must fit the budget exactly rather than merely comfortably. 15
    // nodes, 15 charges, nothing left over and nothing dropped -- which
    // is only true because every layer in a tree `aurora-doc` accepts is
    // reachable from the roots at most once.
    fn resolve_tile_spends_exactly_one_budget_node_per_layer_of_a_well_formed_tree() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let root = balanced_group_tree(&mut layers, &mut store, 3);
        assert_eq!(layers.len(), 15, "7 groups plus 8 leaf pixel layers");

        let mut budget = CompositeBudget::for_pass(&layers);
        let before = budget.nodes;
        assert_eq!(before, layers.len(), "the budget is the tree's own size");

        let resolved = resolve_tile(
            root,
            &layers,
            &mut store,
            aurora_tile::TileId { x: 0, y: 0 },
            (0, 0),
            (0, 0),
            1,
            &mut budget,
        );
        assert!(
            resolved.is_some(),
            "a well-formed 15-layer tree must composite in full, not be truncated by its \
             own budget"
        );
        assert_eq!(
            before - budget.nodes,
            layers.len(),
            "every layer is entered exactly once, so the budget lands exactly at zero -- \
             one node tighter and this well-formed document would lose a layer"
        );
    }

    #[test]
    // The half that actually bounds the attack. The doubling red-team
    // demonstrated -- a group whose `children` names the same id twice,
    // costing `2^slack` visits from a shallow entry point -- inflates
    // exactly one quantity: how many times `resolve_tile` is entered for
    // one tile. That quantity is what this budget caps, monotonically
    // and without refund on return, so the cap holds whatever shape the
    // tree has.
    //
    // The malformed tree itself cannot be built from this crate:
    // `aurora-doc`'s public API refuses to create a duplicate child, and
    // its `Deserialize` refuses to accept one -- which is the whole
    // premise of this guard being *defence in depth* rather than the
    // primary check. So the budget is exercised the way the depth
    // bound's own unit test exercises the depth counter: by handing
    // `resolve_tile` a starting value directly. Burning one node up
    // front leaves room for the group and the lower of its two children
    // and nothing else, and the assertion is on the upper child being
    // *absent from the composited result* -- not merely on the call
    // returning -- so this pins that an over-budget branch is dropped
    // the same way an over-deep one is.
    fn resolve_tile_drops_the_branch_it_has_no_budget_left_for() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // Added lower first: every add lands at index 0, so `children`
        // ends up `[upper, lower]` and the bottom-to-top walk in
        // `resolve_tile` reaches `lower` first.
        for (name, rgba) in [
            ("lower", [1.0, 1.0, 1.0, 1.0]),
            ("upper", [1.0, 0.0, 0.0, 1.0]),
        ] {
            let id = match layers.add_pixel_layer(name, bounds, Some(group)) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(
                &mut store,
                surface,
                aurora_tile::TileId { x: 0, y: 0 },
                rgba,
            );
        }

        let group_texel =
            |store: &mut aurora_tile::TileStore, budget: &mut CompositeBudget| -> [f32; 4] {
                let resolved = resolve_tile(
                    group,
                    &layers,
                    store,
                    aurora_tile::TileId { x: 0, y: 0 },
                    (0, 0),
                    (0, 0),
                    1,
                    budget,
                );
                let Some((texels, _, _)) = resolved else {
                    unreachable!("the group itself is inside every budget used here");
                };
                let Some([r, g, b, a]) = texels.get(..aurora_tile::CHANNELS) else {
                    unreachable!("a resolved tile is at least one texel wide");
                };
                [r.to_f32(), g.to_f32(), b.to_f32(), a.to_f32()]
            };

        let mut full = CompositeBudget::for_pass(&layers);
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                group_texel(&mut store, &mut full),
                [1.0, 0.0, 0.0, 1.0],
                "with the real budget both children composite, so the upper (red) one wins"
            );
        }

        let mut short = CompositeBudget::for_pass(&layers);
        assert!(short.charge_node(), "burn one of the three nodes up front");
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(
                group_texel(&mut store, &mut short),
                [1.0, 1.0, 1.0, 1.0],
                "one node short, the upper child is dropped from the composite entirely -- \
                 the white lower one shows through untouched"
            );
        }
        assert_eq!(
            short.nodes, 0,
            "and the traversal stopped there rather than continuing to charge"
        );
    }

    #[test]
    // The throttle. `recomposite_visible_tiles` resolves once per
    // invalidated tile per frame, so a per-breach warning on a malformed
    // document would fire thousands of times a second on a path that is
    // already over its latency budget. The node count is per tile; the
    // "already said so" flag is per pass.
    fn composite_budget_reports_a_breach_once_per_pass_and_recharges_per_tile() {
        let mut layers = aurora_doc::LayerTree::new();
        if let Err(err) = layers.add_group("group", None) {
            unreachable!("{err:?}");
        }

        let mut budget = CompositeBudget::for_pass(&layers);
        assert!(
            budget.should_report(),
            "the first breach of a pass is worth saying"
        );
        assert!(
            !budget.should_report(),
            "the second is the same broken tree"
        );

        assert!(budget.charge_node());
        budget.next_tile(&layers);
        assert_eq!(
            budget.nodes,
            layers.len(),
            "the next tile gets its own full node budget"
        );
        assert!(
            !budget.should_report(),
            "but not its own warning -- one report per composite pass, not per tile"
        );
    }

    #[test]
    // Real integration test for `LayerMask` aggregation on a plain
    // `Pixel` layer, mirroring the shape of every prior aggregation
    // round's own `composite_document_*` test. `top` is opaque blue,
    // covering the full 10x10 document, with a real mask
    // (`LayerTree::add_mask`, the same API the Layers panel would use)
    // covering only its left half (x in [0, 5)). `bottom` is opaque red,
    // full coverage, no mask. Expected: left half shows `top`'s own
    // blue (inside the mask), right half shows `bottom`'s red through
    // (outside the mask, `top` contributes nothing there) -- a hard
    // edge at x = 5, not a blend, since this is a rectangular clip, not
    // real grayscale masking.
    fn composite_document_clips_a_masked_pixel_layer_to_its_mask_bounds() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        if let Err(err) = layers.add_mask(top, mask_bounds) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        #[allow(clippy::needless_range_loop)]
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    (a - 1.0).abs() < epsilon,
                    "backdrop is opaque, must stay opaque"
                );
                if x < 5 {
                    assert!(
                        r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon,
                        "inside the mask (x={x}) must show top's own blue, got ({r}, {g}, {b}, {a})"
                    );
                } else {
                    assert!(
                        (r - 1.0).abs() < epsilon && g.abs() < epsilon && b.abs() < epsilon,
                        "outside the mask (x={x}) must show bottom's red through, \
                         got ({r}, {g}, {b}, {a})"
                    );
                }
            }
        }
    }

    #[test]
    // Same shape as the masked-`Pixel` test above, but the mask lives on
    // the *group* itself, clipping its whole isolated composite as one
    // unit -- the same "group's own opacity/blend mode apply one level
    // up, to the isolated result" precedent
    // `composite_document_applies_a_groups_own_opacity_to_its_isolated_children`
    // already established, now proven for mask too. `child` (inside
    // `group`) is opaque blue, full coverage; `group` itself carries the
    // left-half mask, left at its own default opacity/`Normal`.
    // `background` is opaque red, full coverage, no mask.
    fn composite_document_clips_a_masked_groups_whole_isolated_content() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let background = match layers.add_pixel_layer("background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let child = match layers.add_pixel_layer("child", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        if let Err(err) = layers.add_mask(group, mask_bounds) {
            unreachable!("{err:?}");
        }
        assert_eq!(layers.roots(), [group, background]);

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (background, [1.0, 0.0, 0.0, 1.0]),
            (child, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        #[allow(clippy::needless_range_loop)]
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    (a - 1.0).abs() < epsilon,
                    "backdrop is opaque, must stay opaque"
                );
                if x < 5 {
                    assert!(
                        r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon,
                        "inside the group's own mask (x={x}) must show the group's own blue \
                         content, got ({r}, {g}, {b}, {a})"
                    );
                } else {
                    assert!(
                        (r - 1.0).abs() < epsilon && g.abs() < epsilon && b.abs() < epsilon,
                        "outside the group's own mask (x={x}) must show background's red \
                         through, got ({r}, {g}, {b}, {a})"
                    );
                }
            }
        }
    }

    #[test]
    // A disabled mask must have zero compositing effect -- falls back to
    // exactly the fully unmasked result, `add_mask` itself only ever
    // creates an *enabled* mask (`LayerTree::add_mask`'s own doc
    // comment), so this exercises `set_mask_enabled(id, false)` reaching
    // all the way through `resolve_tile`'s own `mask.enabled` check.
    // Same setup as `composite_document_clips_a_masked_pixel_layer_to_its_mask_bounds`
    // (mask covering only the left half), but disabled -- `top`'s own
    // blue must now cover the *entire* document, same as if it had no
    // mask at all.
    fn composite_document_ignores_a_disabled_mask() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        if let Err(err) = layers.add_mask(top, mask_bounds) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_mask_enabled(top, false) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon,
                    "a disabled mask must have zero effect -- top's own blue must cover the \
                     whole document, same as having no mask at all, got ({r}, {g}, {b}, {a}) \
                     at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    // `inverted` on the same mask shows exactly the complementary
    // region: the same setup as
    // `composite_document_clips_a_masked_pixel_layer_to_its_mask_bounds`
    // (left-half mask on `top`), but inverted -- now the *right* half
    // shows `top`'s own blue and the *left* half shows `bottom`'s red
    // through, the exact mirror image of the non-inverted result.
    fn composite_document_inverting_a_mask_shows_the_complementary_region() {
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let mask_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 10,
        };
        if let Err(err) = layers.add_mask(top, mask_bounds) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.set_mask_inverted(top, true) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [(bottom, [1.0, 0.0, 0.0, 1.0]), (top, [0.0, 0.0, 1.0, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let image = match composite_document(&layers, &mut store, 10, 10) {
            Ok(image) => image,
            Err(err) => unreachable!("{err:?}"),
        };

        let epsilon = 1e-3;
        #[allow(clippy::needless_range_loop)]
        for y in 0..10 {
            for x in 0..10 {
                let [r, g, b, a] = image_pixel(&image, x, y);
                assert!(
                    (a - 1.0).abs() < epsilon,
                    "backdrop is opaque, must stay opaque"
                );
                if x < 5 {
                    assert!(
                        (r - 1.0).abs() < epsilon && g.abs() < epsilon && b.abs() < epsilon,
                        "inverted: outside-turned-shown region (x={x}) is now the left half, \
                         which is now *excluded* by the inverted mask, so bottom's red must \
                         show through, got ({r}, {g}, {b}, {a})"
                    );
                } else {
                    assert!(
                        r.abs() < epsilon && g.abs() < epsilon && (b - 1.0).abs() < epsilon,
                        "inverted: the right half is now *included* by the inverted mask, so \
                         top's own blue must show, got ({r}, {g}, {b}, {a})"
                    );
                }
            }
        }
    }

    #[test]
    // Regression, live-canvas path: the same
    // `bottom`/`hidden`/`top` shape
    // `recomposite_visible_tiles_blends_visible_layers_bottom_to_top_and_skips_hidden_ones`
    // already proves for a flat root-level list, with `top` moved inside
    // a group left at the schema's own default opacity (1.0)/blend mode
    // (`Normal`) -- and `top` itself *also* left at its own default
    // opacity, deliberately: `resolve_tile`'s own doc comment names
    // exactly this as the real, narrower condition under which isolation
    // is bit-identical to flattening (`composite_tile_cpu` already
    // reproduces one full-opacity layer's own texels exactly, so
    // isolating a lone full-opacity child and re-applying the group's
    // own default opacity/`Normal` is a bit-exact round trip through
    // that same property) -- a lone *translucent* child would not
    // reproduce the flat result (`composite_tile_cpu`'s own straight-
    // alpha accumulation isn't exactly re-entrant for fractional alpha),
    // so this test deliberately keeps `top` fully opaque to exercise the
    // real invariant rather than a false one.
    fn recomposite_visible_tiles_of_a_default_settings_group_matches_flat_compositing() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let hidden = match layers.add_pixel_layer("hidden", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_visible(hidden, false) {
            unreachable!("{err:?}");
        }
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        // `group` and `top` are both left at the schema's own defaults
        // (opacity 1.0, blend mode `Normal`) -- deliberately untouched,
        // the condition that makes isolation a bit-exact round trip.
        let top = match layers.add_pixel_layer("top", bounds, Some(group)) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (bottom, [1.0, 0.0, 0.0, 1.0]),
            (hidden, [0.0, 1.0, 0.0, 1.0]),
            (top, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let result = read_first_texel(&mut store, composite_surface_id(), tile_id);
        assert_eq!(
            result,
            (0.0, 0.0, 1.0, 1.0),
            "an opaque top layer nested in a default-settings group must fully occlude bottom, \
             exactly as it would as a plain root layer -- and the invisible hidden layer must \
             still never contribute"
        );
    }

    #[test]
    fn recomposite_visible_tiles_of_an_empty_document_is_fully_transparent() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let layers = aurora_doc::LayerTree::new();
        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        let result = read_first_texel(&mut store, composite_surface_id(), tile_id);
        assert_eq!(result, (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn recomposite_visible_tiles_skips_an_already_current_tile() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let id = match layers.add_pixel_layer("a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("just created as a pixel layer");
        };
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        fill_solid(&mut store, surface, tile_id, [1.0, 0.0, 0.0, 1.0]);

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), tile_id),
            (1.0, 0.0, 0.0, 1.0),
            "the first real compute must reflect the layer's own content"
        );

        // Poke the composite surface directly -- content only a fresh
        // recompute (not a skip) could ever produce, so its survival
        // through a second call is the only way to confirm the tile was
        // genuinely skipped rather than recomputed to the same value by
        // coincidence.
        fill_solid(
            &mut store,
            composite_surface_id(),
            tile_id,
            [0.0, 1.0, 0.0, 1.0],
        );
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), tile_id),
            (0.0, 1.0, 0.0, 1.0),
            "a tile already current in the cache, with no invalidation in between, must be left alone"
        );
    }

    #[test]
    fn recomposite_visible_tiles_recomputes_a_tile_after_the_cache_is_bumped() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let id = match layers.add_pixel_layer("a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("just created as a pixel layer");
        };
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        fill_solid(&mut store, surface, tile_id, [1.0, 0.0, 0.0, 1.0]);

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        fill_solid(
            &mut store,
            composite_surface_id(),
            tile_id,
            [0.0, 1.0, 0.0, 1.0],
        );
        cache.bump();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), tile_id),
            (1.0, 0.0, 0.0, 1.0),
            "bump must force a real recompute, overwriting the poked value"
        );
    }

    // -- `CompositeCache::invalidate`: the per-tile counterpart to `bump`
    // that `App::paint_dab`/`App::erase_dab` now use instead of a full
    // `bump()` on every dab. `App` itself can't be constructed headlessly
    // (it needs a real `winit` window), so these tests replicate the
    // exact sequence those two methods run -- `aurora_brush::stamp_dab`,
    // then `cache.invalidate` per tile in its returned
    // `aurora_brush::DabOutcome::painted` (they keep a
    // `aurora_brush::touched_tiles` assertion alongside it only to state
    // the geometry each dab is aimed at) -- directly against a real `TileStore` and a real
    // `CompositeCache`, the same technique `measure_pan_and_paint_frames`
    // above already uses to exercise `App::redraw`'s own frame loop
    // without a real `App`.

    /// Everything [`multi_tile_grid_with_all_four_tiles_current`] hands
    /// back to each test below: the real GPU context and scratch dir
    /// (kept alive for the test's own duration), the live store/layer
    /// tree/layer id/surface id the dab tests paint into, and the
    /// residency/cache pair `recomposite_visible_tiles` already
    /// populated with all four visible tiles marked current.
    type MultiTileGridFixture = (
        GpuTestContext,
        tempfile::TempDir,
        aurora_tile::TileStore,
        aurora_doc::LayerTree,
        aurora_doc::LayerId,
        aurora_tile::SurfaceId,
        aurora_gpu::TileResidency,
        CompositeCache,
    );

    /// Sets up a real GPU context, a real `TileStore`, one pixel layer
    /// at document origin `(0, 0)` sized to exactly cover a 2x2 visible
    /// tile grid, and a `TileResidency`/`CompositeCache` whose first
    /// `recomposite_visible_tiles` call marks all four visible tiles
    /// `(0, 0)`, `(1, 0)`, `(0, 1)`, `(1, 1)` current -- the common
    /// starting point every test below needs. Returns `None` if no real
    /// GPU adapter is available (the same inconclusive-skip case
    /// `real_gpu_context` itself already handles).
    fn multi_tile_grid_with_all_four_tiles_current() -> Option<MultiTileGridFixture> {
        let context = real_gpu_context()?;
        let (dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 512,
            height: 512,
        };
        let id = match layers.add_pixel_layer("a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("just created as a pixel layer");
        };

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            Some(id),
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        for tile in [
            aurora_tile::TileId { x: 0, y: 0 },
            aurora_tile::TileId { x: 1, y: 0 },
            aurora_tile::TileId { x: 0, y: 1 },
            aurora_tile::TileId { x: 1, y: 1 },
        ] {
            assert!(
                cache.is_current(tile),
                "setup: the first recomposite must mark every visible tile current"
            );
        }

        Some((context, dir, store, layers, id, surface, residency, cache))
    }

    #[test]
    fn a_dab_confined_to_one_tile_invalidates_only_that_tile() {
        let Some((_context, _dir, mut store, _layers, _id, surface, _residency, mut cache)) =
            multi_tile_grid_with_all_four_tiles_current()
        else {
            return;
        };

        // Well inside tile (0, 0) -- `min`/`max` stay in [26, 74], nowhere
        // near the tile-256 boundary -- so this dab touches exactly one
        // tile, the same shape most dabs of a real stroke have.
        let local = (50.0, 50.0);
        let touched = aurora_brush::touched_tiles(local, BRUSH_RADIUS);
        assert_eq!(
            touched,
            vec![aurora_tile::TileId { x: 0, y: 0 }],
            "setup: this dab must touch exactly tile (0, 0)"
        );
        let outcome = aurora_brush::stamp_dab(
            &mut store,
            surface,
            local,
            BRUSH_RADIUS,
            [0.8, 0.1, 0.05],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        for &tile in outcome.painted() {
            cache.invalidate(tile);
        }

        assert!(
            !cache.is_current(aurora_tile::TileId { x: 0, y: 0 }),
            "the touched tile must need recompute"
        );
        for tile in [
            aurora_tile::TileId { x: 1, y: 0 },
            aurora_tile::TileId { x: 0, y: 1 },
            aurora_tile::TileId { x: 1, y: 1 },
        ] {
            assert!(
                cache.is_current(tile),
                "a tile the dab never touched must not be invalidated -- \
                 this is the actual performance claim: a full bump() would \
                 fail this assertion for every one of these three"
            );
        }
    }

    #[test]
    fn a_dab_confined_to_one_tile_forces_a_real_recompute_there_and_leaves_other_tiles_alone() {
        let Some((context, _dir, mut store, layers, id, surface, residency, mut cache)) =
            multi_tile_grid_with_all_four_tiles_current()
        else {
            return;
        };

        let local = (50.0, 50.0);
        assert_eq!(
            aurora_brush::touched_tiles(local, BRUSH_RADIUS),
            vec![aurora_tile::TileId { x: 0, y: 0 }],
            "setup: this dab must be aimed at exactly tile (0, 0)"
        );
        let outcome = aurora_brush::stamp_dab(
            &mut store,
            surface,
            local,
            BRUSH_RADIUS,
            [0.8, 0.1, 0.05],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        for &tile in outcome.painted() {
            cache.invalidate(tile);
        }

        // Poke garbage into both the touched tile's own composite surface
        // and an untouched tile's -- content only a fresh recompute (for
        // the touched tile) or a genuine skip (for the untouched tile)
        // could ever explain, the same technique
        // `recomposite_visible_tiles_skips_an_already_current_tile` above
        // already uses.
        let touched_tile = aurora_tile::TileId { x: 0, y: 0 };
        let untouched_tile = aurora_tile::TileId { x: 1, y: 1 };
        fill_solid(
            &mut store,
            composite_surface_id(),
            touched_tile,
            [0.0, 1.0, 0.0, 1.0],
        );
        fill_solid(
            &mut store,
            composite_surface_id(),
            untouched_tile,
            [0.0, 1.0, 0.0, 1.0],
        );

        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            Some(id),
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        // The touched tile: its own first texel (0, 0) is nowhere near
        // the dab (centered at (50, 50), radius 24 -- distance ~70 from
        // the tile's own origin), so a real recompute must leave it
        // transparent, not the poked green -- proof the tile was
        // genuinely recomputed rather than skipped as still-cached.
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), touched_tile),
            (0.0, 0.0, 0.0, 0.0),
            "the invalidated tile must be genuinely recomputed, not skipped"
        );
        // And the dab's own new paint really is visible in the fresh
        // composite, sampled at its own center where alpha peaks at 1.0.
        let Some([r, g, b, a]) = sample_pixel(&mut store, composite_surface_id(), local) else {
            unreachable!("(50, 50) is a valid in-bounds composite sample");
        };
        assert!(a > 0.9, "the dab's own center must be near-opaque: {a}");
        // Not an exact-value check -- the falloff/compositing math isn't
        // this test's own concern, and is already covered by
        // `stamp_dab`'s and `composite_tile_cpu`'s own tests -- just that
        // this is unmistakably the dab's own warm colour (real red,
        // negligible green) rather than the poked garbage green (0, 1, 0).
        assert!(
            r > 0.5 && g < 0.3,
            "the composite must show the dab's real colour, not the poked green: ({r}, {g}, {b}, {a})"
        );

        // The untouched tile: no invalidation ever touched it, so it must
        // still be exactly the poked garbage -- recomputing it too would
        // be the whole bug this fix targets.
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), untouched_tile),
            (0.0, 1.0, 0.0, 1.0),
            "a tile the dab never touched must retain its existing composited content unchanged"
        );
    }

    #[test]
    fn a_dab_straddling_a_tile_corner_invalidates_every_tile_it_touches() {
        let Some((_context, _dir, mut store, _layers, _id, surface, _residency, mut cache)) =
            multi_tile_grid_with_all_four_tiles_current()
        else {
            return;
        };

        // Centered exactly on the shared corner of all four visible
        // tiles: [232, 280) in both axes straddles the 256 boundary, so
        // `stamp_dab`'s own documented "up to four tiles near a corner"
        // case applies here for real.
        let local = (256.0, 256.0);
        let touched = aurora_brush::touched_tiles(local, BRUSH_RADIUS);
        let touched_set: std::collections::HashSet<_> = touched.iter().copied().collect();
        assert_eq!(
            touched_set,
            std::collections::HashSet::from([
                aurora_tile::TileId { x: 0, y: 0 },
                aurora_tile::TileId { x: 1, y: 0 },
                aurora_tile::TileId { x: 0, y: 1 },
                aurora_tile::TileId { x: 1, y: 1 },
            ]),
            "setup: a corner dab must touch all four visible tiles"
        );
        let outcome = aurora_brush::stamp_dab(
            &mut store,
            surface,
            local,
            BRUSH_RADIUS,
            [0.8, 0.1, 0.05],
            None,
        );
        assert!(
            outcome.is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        for &tile in outcome.painted() {
            cache.invalidate(tile);
        }

        for tile in touched_set {
            assert!(
                !cache.is_current(tile),
                "every tile the corner dab actually touched must be invalidated: {tile:?}"
            );
        }
    }

    #[test]
    fn a_short_stroke_of_dabs_in_one_tile_invalidates_a_bounded_number_of_tiles() {
        let Some((_context, _dir, mut store, _layers, _id, surface, _residency, mut cache)) =
            multi_tile_grid_with_all_four_tiles_current()
        else {
            return;
        };

        // 20 dabs, all landing in tile (0, 0) -- the common shape of a
        // slow-drag stroke, where most successive dabs land in the same
        // or an adjacent tile. If this fix worked, the number of tiles
        // needing recompute afterward is bounded by what the stroke
        // actually touched (one tile here), not by how many dabs were
        // stamped -- the reported bug's exact shape, where every single
        // dab used to invalidate the *entire* visible grid.
        for step in 0_u8..20 {
            let local = (40.0 + f32::from(step), 40.0 + f32::from(step));
            let touched = aurora_brush::touched_tiles(local, BRUSH_RADIUS);
            assert_eq!(
                touched,
                vec![aurora_tile::TileId { x: 0, y: 0 }],
                "setup: every dab in this stroke must stay within tile (0, 0)"
            );
            let outcome = aurora_brush::stamp_dab(
                &mut store,
                surface,
                local,
                BRUSH_RADIUS,
                [0.8, 0.1, 0.05],
                None,
            );
            assert!(
                outcome.is_complete(),
                "a healthy store must paint every tile this dab covers"
            );
            for &tile in outcome.painted() {
                cache.invalidate(tile);
            }
        }

        let still_current = [
            aurora_tile::TileId { x: 1, y: 0 },
            aurora_tile::TileId { x: 0, y: 1 },
            aurora_tile::TileId { x: 1, y: 1 },
        ]
        .into_iter()
        .filter(|&tile| cache.is_current(tile))
        .count();
        assert_eq!(
            still_current, 3,
            "20 dabs confined to one tile must still leave the other three tiles current -- \
             the number of tiles needing recompute must not scale with dab count"
        );
        assert!(
            !cache.is_current(aurora_tile::TileId { x: 0, y: 0 }),
            "the one tile the whole stroke actually touched must need recompute"
        );
    }

    // -- `document_qualifies_for_gpu_compositing`: pure, headless, no real
    // GPU device needed -- these check `aurora_doc::LayerTree` state only.

    #[test]
    fn document_qualifies_for_gpu_compositing_of_normal_blend_pixel_layers_only() {
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        if let Err(err) = layers.add_pixel_layer("a", bounds, None) {
            unreachable!("{err:?}");
        }
        if let Err(err) = layers.add_pixel_layer("b", bounds, None) {
            unreachable!("{err:?}");
        }
        assert!(document_qualifies_for_gpu_compositing(&layers));
    }

    #[test]
    fn document_qualifies_for_gpu_compositing_is_false_for_a_visible_group() {
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        if let Err(err) = layers.add_pixel_layer("a", bounds, None) {
            unreachable!("{err:?}");
        }
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.add_pixel_layer("b", bounds, Some(group)) {
            unreachable!("{err:?}");
        }
        assert!(
            !document_qualifies_for_gpu_compositing(&layers),
            "a visible group at the root must disqualify the whole document"
        );
    }

    #[test]
    fn document_qualifies_for_gpu_compositing_is_false_for_a_non_normal_blend_mode() {
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        if let Err(err) = layers.add_pixel_layer("a", bounds, None) {
            unreachable!("{err:?}");
        }
        let top = match layers.add_pixel_layer("b", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        assert!(
            !document_qualifies_for_gpu_compositing(&layers),
            "a non-Normal blend mode anywhere at the root must disqualify the whole document"
        );
    }

    #[test]
    fn document_qualifies_for_gpu_compositing_ignores_an_invisible_disqualifying_layer() {
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        if let Err(err) = layers.add_pixel_layer("a", bounds, None) {
            unreachable!("{err:?}");
        }
        // A hidden group would disqualify the document if it were visible
        // (previous test), but a hidden one contributes nothing to either
        // path, so it must not disqualify.
        let group = match layers.add_group("group", None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_visible(group, false) {
            unreachable!("{err:?}");
        }
        assert!(document_qualifies_for_gpu_compositing(&layers));
    }

    /// The most important test in this round: a real, multi-layer,
    /// several-layer document (three `Normal`-blend, non-grouped pixel
    /// layers at different opacities) run through **both**
    /// `recomposite_visible_tiles`'s new GPU path and its pre-existing CPU
    /// path — controlled directly via this function's own `gpu`/
    /// `compositor` parameters, `Some` for the GPU run and `None` to force
    /// the CPU fallback for the second run — and asserted to land on the
    /// exact same pixels. Also checked against an independently
    /// hand-computed expected value (worked below), so this proves not
    /// just "the two paths agree with each other" but "both are actually
    /// correct."
    ///
    /// **Exact equality, not a tolerance**: every input/intermediate value
    /// here (1.0, 0.5, 0.25, and the sums/products of the straight-alpha
    /// "over" formula they produce) is an exact power-of-two binary
    /// fraction, so both the CPU path (`f32` arithmetic rounded to `f16`)
    /// and the GPU path (the same formula, computed by real hardware) are
    /// expected to land on bit-identical results, not just close ones —
    /// and this test confirms that expectation actually holds by running
    /// against real GPU hardware, not just asserting it should.
    #[test]
    fn recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_real_multi_layer_document() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let middle = match layers.add_pixel_layer("middle", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(middle, 0.5) {
            unreachable!("{err:?}");
        }
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(top, 0.25) {
            unreachable!("{err:?}");
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for (id, rgba) in [
            (bottom, [1.0, 0.0, 0.0, 1.0]),
            (middle, [0.0, 1.0, 0.0, 1.0]),
            (top, [0.0, 0.0, 1.0, 1.0]),
        ] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }
        assert!(
            document_qualifies_for_gpu_compositing(&layers),
            "three Normal-blend, non-grouped pixel layers must qualify for the GPU path"
        );

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));

        // Run 1: the real GPU path.
        let mut gpu_cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut gpu_cache,
            Some(&context),
            Some(&mut compositor),
        );
        let gpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);

        // Run 2: force the CPU path by passing no GPU/compositor at all --
        // `document_qualifies_for_gpu_compositing` still says yes, but
        // `recomposite_visible_tiles` must fall back cleanly when the GPU
        // side simply isn't available, exactly like a session with no GPU
        // device.
        let mut cpu_cache = CompositeCache::default();
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cpu_cache,
            None,
            None,
        );
        let cpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);

        assert_eq!(
            gpu_result, cpu_result,
            "the GPU path and the CPU path must composite this real multi-layer document to \
             the exact same pixels"
        );

        // Hand-computed, bottom to top, straight-alpha "over"
        // (`Co = (1-as)*Cb + as*Cs` per channel, `Ca = as + Cb.a*(1-as)`,
        // `as = src_a * opacity`):
        //   bottom (opaque red, opacity 1.0) over transparent black
        //     -> (1.0, 0.0, 0.0, 1.0), reproduces the source exactly.
        //   middle (opaque green, opacity 0.5): as = 1.0*0.5 = 0.5
        //     r = 0.5*1.0 + 0.5*0.0 = 0.5, g = 0.5*0.0 + 0.5*1.0 = 0.5,
        //     b = 0.0, a = 0.5 + 1.0*0.5 = 1.0 -> (0.5, 0.5, 0.0, 1.0).
        //   top (opaque blue, opacity 0.25): as = 1.0*0.25 = 0.25
        //     r = 0.75*0.5 + 0.25*0.0 = 0.375,
        //     g = 0.75*0.5 + 0.25*0.0 = 0.375,
        //     b = 0.75*0.0 + 0.25*1.0 = 0.25,
        //     a = 0.25 + 1.0*0.75 = 1.0 -> (0.375, 0.375, 0.25, 1.0).
        assert_eq!(gpu_result, (0.375, 0.375, 0.25, 1.0));
    }

    /// AC-2's own regression test: the same fixture as
    /// `composite_document_un_premultiplies_a_translucent_root_level_layer`
    /// (one opaque-white root-level pixel layer at layer opacity 0.5,
    /// over nothing), but reaching `composite_roots_into_tile` through
    /// the *live canvas* entry point rather than the export one — the
    /// two callers that shared the missing un-premultiply step. The
    /// expected value and its hand-computation are identical; see that
    /// test's own comment.
    ///
    /// **Honest note about the harness**: this exercises the CPU
    /// compositing fallback (`gpu`/`compositor` both `None`, the same
    /// technique
    /// `recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_real_multi_layer_document`
    /// already uses for its second run), yet it still needs a real
    /// `aurora_gpu::TileResidency` — and therefore a real `GpuContext` —
    /// because `recomposite_visible_tiles` takes the residency to learn
    /// which tiles are visible. That is a harness constraint of this
    /// entry point's signature, not evidence about the CPU math, and it
    /// means this test self-skips where no adapter exists. The
    /// always-headless proof of the same math is the `composite_document`
    /// sibling above.
    #[test]
    fn recomposite_visible_tiles_un_premultiplies_a_translucent_root_level_layer() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let layer = match layers.add_pixel_layer("translucent", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_opacity(layer, 0.5) {
            unreachable!("{err:?}");
        }
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        let Some(surface) = layers.surface_id(layer) else {
            unreachable!("just created as a pixel layer");
        };
        fill_solid(&mut store, surface, tile_id, [1.0, 1.0, 1.0, 1.0]);

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        recomposite_visible_tiles(
            &residency, &layers, None, &mut store, &mut cache, None, None,
        );

        let result = read_first_texel(&mut store, composite_surface_id(), tile_id);
        assert_eq!(
            result,
            (1.0, 1.0, 1.0, 0.5),
            "the live canvas composite must hold straight alpha, the same as an export"
        );
        assert_ne!(
            result,
            (0.5, 0.5, 0.5, 0.5),
            "the premultiplied value the root-level fold leaves behind -- what the composite \
             surface (and so the eyedropper) carried before 0.52.0"
        );
    }

    /// AC-4's own sibling of
    /// `recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_real_multi_layer_document`,
    /// which stays deliberately unchanged: that test's fixture ends at
    /// `alpha = 1.0`, where un-premultiplying divides by one and is an
    /// exact identity on both paths — which is precisely why it kept
    /// passing across 0.52.0 (real evidence of no regression) *and* why
    /// it could never have caught the bug in the first place. This one
    /// ends at a **fractional** final alpha, where the straightening
    /// step 0.52.0 added is the only thing that can make the two paths
    /// agree.
    ///
    /// Both paths reach that step through the *same* implementation:
    /// `aurora_render::un_premultiply_in_place`, called by
    /// `composite_roots_into_tile` on the CPU path and by
    /// `finish_tile_readback` on the GPU path's readback decode. So this
    /// test proves the two paths' *compositing* agrees; it can no longer
    /// diverge on the division itself, which is by construction now
    /// rather than by two implementations happening to match. (0.52.0's
    /// first shape did run a separate WGSL division as an extra GPU
    /// pass, and that pair was measured not to agree at very small
    /// alphas.)
    ///
    /// Fixture: two root-level `Normal`-blend pixel layers, both filled
    /// opaque red `(1, 0, 0, 1)`, both at layer opacity 0.5, no groups
    /// (so the whole document is GPU-tractable — asserted below, so a
    /// future change that silently routed this to the CPU would fail
    /// loudly rather than pass vacuously).
    ///
    /// Hand-computed, bottom to top, straight-alpha "over"
    /// (`Co = (1-as)*Cb + as*Cs`, `Ca = as + Cb.a*(1-as)`,
    /// `as = src_a * opacity`), onto a transparent start:
    ///   bottom: `as = 0.5`, `r = 0.5*0 + 0.5*1 = 0.5`,
    ///     `a = 0.5 + 0*0.5 = 0.5` -> premultiplied `(0.5, 0, 0, 0.5)`.
    ///   top: `as = 0.5`, `r = 0.5*0.5 + 0.5*1 = 0.75`,
    ///     `a = 0.5 + 0.5*0.5 = 0.75` -> premultiplied `(0.75, 0, 0, 0.75)`.
    /// Straightened: `0.75 / 0.75 = 1.0`, so the finished tile is
    /// `(1.0, 0.0, 0.0, 0.75)` — two half-opacity opaque-red layers stack
    /// to three-quarters-opaque *fully saturated* red, which is what an
    /// export and the eyedropper must report.
    ///
    /// **Exact equality, not a tolerance**, on both paths and between
    /// them: every value here (0.5, 0.25, 0.75, 1.0) is an exact binary
    /// fraction, so the ~2.5-ULP latitude Vulkan permits an `f32`
    /// multiply-add cannot move the GPU fold's result off the CPU's, and
    /// the one division (`0.75 / 0.75`) is the same CPU code on both
    /// paths. If this ever does diverge, that is a finding to report,
    /// not a reason to loosen the assertion.
    #[test]
    fn recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_fractional_final_alpha_document() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        for id in [bottom, top] {
            if let Err(err) = layers.set_opacity(id, 0.5) {
                unreachable!("{err:?}");
            }
        }

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        for id in [bottom, top] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, [1.0, 0.0, 0.0, 1.0]);
        }
        assert!(
            document_qualifies_for_gpu_compositing(&layers),
            "two Normal-blend, non-grouped pixel layers must qualify for the GPU path -- \
             otherwise this test would compare the CPU path against itself"
        );

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));

        // Run 1: the real GPU path.
        let mut gpu_cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut gpu_cache,
            Some(&context),
            Some(&mut compositor),
        );
        let gpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);

        // Run 2: the CPU fallback, forced by passing no GPU/compositor.
        let mut cpu_cache = CompositeCache::default();
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cpu_cache,
            None,
            None,
        );
        let cpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);

        assert_eq!(
            gpu_result, cpu_result,
            "the GPU and CPU paths must agree on a fractional-final-alpha document too, not \
             only on the fully-opaque one the sibling test covers"
        );
        for (result, path) in [(gpu_result, "GPU"), (cpu_result, "CPU")] {
            assert_eq!(
                result,
                (1.0, 0.0, 0.0, 0.75),
                "{path} path: two half-opacity opaque-red layers straighten to fully \
                 saturated red at 0.75 alpha"
            );
            assert_ne!(
                result,
                (0.75, 0.0, 0.0, 0.75),
                "{path} path: the premultiplied value both paths produced before 0.52.0"
            );
        }
    }

    /// The batched-poll restructuring's own correctness proof: the
    /// sibling test above (`..._gpu_and_cpu_paths_agree_on_a_real_multi_
    /// layer_document`) uses a 256×256 viewport over a 10×10-px layer, so
    /// only its single `(0, 0)` tile ever has real layer content — every
    /// other visible tile in its 2×2 grid has none, so
    /// `begin_gpu_composite_tile` returns `None` for them and they never
    /// reach `pending_gpu` at all. That test alone would pass even if
    /// phase 3's drain mixed up which decoded result belongs to which
    /// tile (a real risk this restructuring introduces: `PendingGpuReadback`
    /// now travels through a `Vec` between issue and resolve, so a bug
    /// that dropped or misassigned a tile's own result partway through
    /// would only show up with more than one tile genuinely in flight at
    /// once). This test forces that: a single 512×512 `Normal`-blend
    /// pixel layer spans exactly the four `(0, 0)`/`(1, 0)`/`(0, 1)`/
    /// `(1, 1)` tiles, each filled with its own distinct solid colour, so
    /// a 512×512 viewport's own 3×3 visible grid puts all four through
    /// `begin_gpu_composite_tile` (real, non-empty GPU work) in the same
    /// call to [`recomposite_visible_tiles`], all four batched into one
    /// shared `pending_gpu` and resolved by the same single
    /// `device.poll` — exactly the batching this round's fix introduces.
    /// The remaining five visible tiles (row/column index 2) fall
    /// entirely outside the 512×512 layer bounds, so they still exercise
    /// the immediate-CPU-fallback branch of phase 1 (no visible layers to
    /// batch) in the same call, confirming the two branches coexist
    /// correctly.
    ///
    /// Each of the four populated tiles is checked against its own
    /// distinct expected colour (not just "some tile has some colour"),
    /// which is exactly what would catch a tile-identity mixup a less
    /// specific assertion would miss. The CPU path (GPU/compositor both
    /// `None`, forcing the exact same fallback
    /// `recomposite_visible_tiles_gpu_and_cpu_paths_agree_on_a_real_
    /// multi_layer_document` already uses) is run second, against a
    /// fresh cache, and checked to agree with the GPU path on all four
    /// tiles too.
    #[test]
    fn recomposite_visible_tiles_gpu_path_batches_multiple_real_tiles_in_one_call_without_mixing_them_up()
     {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 512,
            height: 512,
        };
        let layer_id = match layers.add_pixel_layer("canvas", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(layer_id) else {
            unreachable!("just created as a pixel layer");
        };
        assert!(
            document_qualifies_for_gpu_compositing(&layers),
            "a single Normal-blend, full-bounds pixel layer must qualify for the GPU path"
        );

        // Four distinct, opaque solid colours, one per tile -- a mixup
        // between any two of these would be visible in the assertions
        // below, unlike four identical fills.
        let expected: [(aurora_tile::TileId, [f32; 4]); 4] = [
            (aurora_tile::TileId { x: 0, y: 0 }, [1.0, 0.0, 0.0, 1.0]),
            (aurora_tile::TileId { x: 1, y: 0 }, [0.0, 1.0, 0.0, 1.0]),
            (aurora_tile::TileId { x: 0, y: 1 }, [0.0, 0.0, 1.0, 1.0]),
            (aurora_tile::TileId { x: 1, y: 1 }, [1.0, 1.0, 0.0, 1.0]),
        ];
        for (tile_id, rgba) in expected {
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (512, 512));

        // Run 1: the real, batched GPU path.
        let mut gpu_cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut gpu_cache,
            Some(&context),
            Some(&mut compositor),
        );
        for (tile_id, rgba) in expected {
            let gpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (rgba.first(), rgba.get(1), rgba.get(2), rgba.get(3))
            else {
                unreachable!("rgba always has four channels");
            };
            assert_eq!(
                gpu_result,
                (r, g, b, a),
                "tile {tile_id:?} must composite to its own distinct colour, not another \
                 tile's -- a mismatch here means phase 3's drain misassigned a batched \
                 readback"
            );
        }

        // Run 2: force the CPU path, same as the sibling multi-layer
        // test, and confirm it agrees with the GPU path on every tile.
        let mut cpu_cache = CompositeCache::default();
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cpu_cache,
            None,
            None,
        );
        for (tile_id, rgba) in expected {
            let cpu_result = read_first_texel(&mut store, composite_surface_id(), tile_id);
            let (Some(&r), Some(&g), Some(&b), Some(&a)) =
                (rgba.first(), rgba.get(1), rgba.get(2), rgba.get(3))
            else {
                unreachable!("rgba always has four channels");
            };
            assert_eq!(
                cpu_result,
                (r, g, b, a),
                "the CPU fallback must agree with the batched GPU path on tile {tile_id:?}"
            );
        }
    }

    /// The fallback's own correctness proof, not just that it was taken:
    /// a document with one `Multiply`-blend layer (a case the GPU path
    /// structurally cannot express — `composite_over_with_opacity`'s own
    /// fixed-function blend unit only ever computes `Normal`'s "source
    /// over") must still composite to `Multiply`'s own real result, not to
    /// whatever `Normal` would have produced for the same inputs — which
    /// would be a different, wrong value here, so this genuinely
    /// distinguishes "fell back and composited correctly" from "silently
    /// used the GPU's own Normal-only math anyway."
    #[test]
    fn recomposite_visible_tiles_falls_back_to_the_cpu_path_for_a_non_normal_blend_mode() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };

        let bottom = match layers.add_pixel_layer("bottom", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let top = match layers.add_pixel_layer("top", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(top, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        assert!(
            !document_qualifies_for_gpu_compositing(&layers),
            "a Multiply-blend layer must disqualify the document"
        );

        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        // Two mid-greys: Multiply(0.5, 0.5) = 0.25, the same worked case
        // `composite_tile_cpu_multiply_blends_two_mid_greys_to_a_quarter_grey`
        // (aurora-render) already proves for the CPU formula in isolation
        // -- if the GPU path were mistakenly used here anyway (treating
        // `Multiply` as `Normal`, opaque top over opaque bottom at full
        // opacity), the result would be the top layer's own colour
        // unchanged, (0.5, 0.5, 0.5, 1.0), not (0.25, 0.25, 0.25, 1.0).
        for (id, rgba) in [(bottom, [0.5, 0.5, 0.5, 1.0]), (top, [0.5, 0.5, 0.5, 1.0])] {
            let Some(surface) = layers.surface_id(id) else {
                unreachable!("just created as a pixel layer");
            };
            fill_solid(&mut store, surface, tile_id, rgba);
        }

        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        let mut compositor = aurora_render::TileCompositor::new(context.device());
        // GPU/compositor are both real and available here -- proving the
        // fallback is `document_qualifies_for_gpu_compositing`-driven, not
        // just "happens to fall back because nothing GPU was passed in,"
        // the same distinction the other GPU-path tests above are
        // structured to rule out.
        recomposite_visible_tiles(
            &residency,
            &layers,
            None,
            &mut store,
            &mut cache,
            Some(&context),
            Some(&mut compositor),
        );

        let result = read_first_texel(&mut store, composite_surface_id(), tile_id);
        assert_eq!(
            result,
            (0.25, 0.25, 0.25, 1.0),
            "Multiply's own real math must run via the CPU fallback, not Normal's"
        );
    }

    /// Runs `frames` iterations of "pan by `pan_step_px` per frame, then
    /// stamp one brush dab", each iteration driving `aurora-app`'s own real
    /// compositing + present path end to end: [`aurora_brush::stamp_dab`]
    /// (paint) -> [`recomposite_visible_tiles`] (this crate's own per-tile
    /// compositing, GPU or CPU depending on what `layers` qualifies for) ->
    /// [`aurora_gpu::TileResidency::sync`] (real GPU upload) -> a real
    /// [`aurora_gpu::CanvasPipeline`] render pass presenting the atlas to an
    /// offscreen `Rgba8Unorm` target sized `viewport` -> `queue.submit` plus
    /// a blocking `device.poll(Wait)`, so the timed interval covers
    /// submission through GPU completion, not just CPU-side command
    /// recording -- the same "offscreen texture standing in for the
    /// swapchain" technique `spike/vertical-slice`'s own `headless_bench`
    /// already validated, exercised here against `aurora-app`'s own real
    /// types instead of that spike's separate, simplified renderer.
    ///
    /// Shared by both `recomposite_and_present_loop_*` tests below so each
    /// can set up its own `layers`/`store` (GPU-qualifying or not) without
    /// duplicating this frame loop. Returns one measured frame time in
    /// milliseconds per iteration, in order.
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation
    )]
    fn measure_pan_and_paint_frames(
        gpu: &aurora_gpu::GpuContext,
        layers: &aurora_doc::LayerTree,
        active_layer: aurora_doc::LayerId,
        surface: aurora_tile::SurfaceId,
        store: &mut aurora_tile::TileStore,
        viewport: (u32, u32),
        frames: u32,
        start: (u32, u32),
        pan_step_px: (u32, u32),
    ) -> Vec<f64> {
        let device = gpu.device();
        let queue = gpu.queue();
        let mut residency = aurora_gpu::TileResidency::new(device, queue, viewport);
        let mut canvas_pipeline = aurora_gpu::CanvasPipeline::new(device);
        let mut compositor = aurora_render::TileCompositor::new(device);
        let mut cache = CompositeCache::default();

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame-timing-target"),
            size: wgpu::Extent3d {
                width: viewport.0,
                height: viewport.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut timings = Vec::with_capacity(frames as usize);
        for step in 0..frames {
            let x = start.0 + step * pan_step_px.0;
            let y = start.1 + step * pan_step_px.1;
            let t0 = std::time::Instant::now();

            #[allow(clippy::cast_precision_loss)]
            residency.set_origin(queue, (x as f32, y as f32), viewport, 1.0);

            // A `TileError` here (observed in practice under this
            // helper's own tight `real_tile_store()` budget: a page-in
            // racing the background writer's still-in-flight eviction
            // write for the same tile, surfacing as a transient
            // `CorruptFile`/`Io` read) is tolerated exactly like
            // `App::paint_dab`'s own real production idiom -- logged, not
            // fatal to the frame -- rather than treated as a structurally
            // impossible case. That race is itself a real finding about
            // `aurora_tile::TileStore` under heavy paging pressure,
            // reported honestly in this test's own doc comment and
            // PLAN.md's M1.10 entry rather than hidden behind a generous
            // budget or a `flush()` call this loop's real counterpart
            // (`App::redraw`) never makes either.
            let dab_center = (x as f32 + 300.0, y as f32 + 300.0);
            let outcome = aurora_brush::stamp_dab(
                store,
                surface,
                dab_center,
                BRUSH_RADIUS,
                [0.95, 0.62, 0.25],
                None,
            );
            if let Some(err) = outcome.first_error() {
                tracing::warn!(
                    ?err,
                    failed = outcome.failed().len(),
                    painted = outcome.painted().len(),
                    "failed to stamp part of a brush dab this frame"
                );
            }
            cache.bump();

            recomposite_visible_tiles(
                &residency,
                layers,
                Some(active_layer),
                store,
                &mut cache,
                Some(gpu),
                Some(&mut compositor),
            );
            let _ = residency.sync(queue, store, composite_surface_id(), false, usize::MAX);

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-timing"),
            });
            {
                let bind_group = canvas_pipeline.bind_group(device, &residency);
                let pipeline = canvas_pipeline.pipeline(device, wgpu::TextureFormat::Rgba8Unorm);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("frame-timing"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(encoder.finish()));
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });

            timings.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
        timings
    }

    /// Mean, p50, p99, and max of `values` (sorted in place) -- the same
    /// shape `spike/vertical-slice`'s own `report_ms` reports, reimplemented
    /// here rather than reused (that binary is deliberately excluded from
    /// the workspace -- root `Cargo.toml`'s `exclude`, see this crate's own
    /// `CLAUDE.md` -- so it can never become a dependency of real code)
    /// so both tests below can report and assert on real numbers.
    fn ms_stats(values: &mut [f64]) -> (f64, f64, f64, f64) {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let len = values.len();
        #[allow(clippy::cast_precision_loss)]
        let mean = values.iter().sum::<f64>() / len as f64;
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation
        )]
        let percentile = |p: f64| -> usize { ((len - 1) as f64 * p).round() as usize };
        let Some(&p50) = values.get(percentile(0.50)) else {
            unreachable!("index computed from this same slice's own length")
        };
        let Some(&p99) = values.get(percentile(0.99)) else {
            unreachable!("index computed from this same slice's own length")
        };
        let Some(&max) = values.last() else {
            unreachable!("caller always passes a non-empty slice")
        };
        (mean, p50, p99, max)
    }

    /// Real, headless, GPU-gated end-to-end frame-timing measurement of
    /// `aurora-app`'s own real compositing + present path -- the "true
    /// end-to-end regression test" PLAN.md's M1.10 entry (search "a true
    /// end-to-end regression test needs a real frame/present loop in
    /// `aurora-app`") names as still-open follow-on work, built for real
    /// here instead of deferred again.
    ///
    /// **Document size**: 300,000 x 300,000 px -- the real Phase 0 ceiling
    /// (ADR 0002, invariant #7.3.1), matching `spike/FINDINGS.md`'s own
    /// "Third run: the 300,000 px ceiling" re-measurement, not the smaller
    /// 100,000 px stand-in used earlier in the project's history.
    ///
    /// **Workload**: 40 frames, an 800x600 viewport panning diagonally
    /// across the document by the same (200, 120) px/frame step
    /// `spike/vertical-slice`'s own "Frame breakdown while painting and
    /// panning" scenario uses, while one brush dab is stamped into the
    /// active layer every frame at a fixed offset from the pan position --
    /// mirroring that exact spike scenario, not idle panning, because
    /// `spike/FINDINGS.md`'s "Third run" section found normal-drag panning
    /// (16.83 ms p99 at both 100,000px and 300,000px) already marginal
    /// against the 16.7 ms budget even before painting is added on top.
    /// Panning here steps by raw pixels (200, 120 px/frame, not rounded
    /// to whole `aurora_tile::TILE` boundaries) and lands sub-tile-accurate
    /// now that `TileResidency::set_origin` carries the fractional
    /// remainder through to the sampled UV offset — this benchmark's own
    /// timing isn't sensitive to that (it measures frame cost, not visual
    /// correctness), but it exercises the real, now-fixed path rather
    /// than the old floored one.
    ///
    /// **Real path exercised, per frame, end to end**: see
    /// [`measure_pan_and_paint_frames`]'s own doc comment.
    ///
    /// **Compositing path taken: GPU.** The document is a single, visible,
    /// `Normal`-blend, full-bounds `Pixel` layer -- confirmed directly
    /// below via `document_qualifies_for_gpu_compositing`, not assumed --
    /// so every tile recomposited here goes through the batched GPU path
    /// (`begin_gpu_composite_tile`/`finish_tile_readback`).
    /// The CPU fallback is exercised separately, by
    /// [`recomposite_and_present_loop_exercises_the_cpu_fallback_path`]
    /// below.
    ///
    /// **Budget**: nominally 16.7 ms (60 FPS). **Measured locally (this
    /// sandbox's real Vulkan adapter -- Mesa llvmpipe, software
    /// rendering, confirmed via `GpuContext::adapter_info()`; an earlier
    /// pass through this comment and PLAN.md mislabeled this "NVIDIA RTX
    /// 3090," corrected once actually checked rather than assumed --
    /// release build): mean 34.60 ms, p50 35.44 ms, p99
    /// 98.75 ms, max 98.75 ms (n=40) -- well over the 16.7 ms budget**,
    /// an honest, real finding, not a rounded-up pass: this is the exact
    /// "pan while painting" scenario `spike/FINDINGS.md`'s "Third run"
    /// section already found marginal/over budget for panning alone
    /// (16.83-20.01 ms p99), now measured end to end (paint + real
    /// composite + real GPU upload + a real present pass, not just the
    /// spike's own narrower panning-only figure) for the first time
    /// through `aurora-app`'s own real path, at a larger 800x600 viewport
    /// than the spike's panning figures used alone. The assertion below
    /// uses 3000 ms -- roughly 3.4x the worst p99 (889ms) real GitHub
    /// Actions CI runs actually produced (2026-08-12), not the ~99ms this
    /// local sandbox measures -- as the CI-safety threshold. The original
    /// 350ms figure (~3.5x this sandbox's own local p99) caused real CI
    /// failures: GitHub's own runner turned out to be far slower/noisier
    /// than any dev sandbox this was tuned against. Generous enough to
    /// absorb a slow, shared, three-OS CI runner without flaking (the same
    /// reasoning this file's `aurora-brush` sibling test,
    /// `stamp_dab_latency_stays_within_a_generous_ci_safe_budget`, and this
    /// crate's own GPU-gated latency tests already use), while still a
    /// real trip-wire against a multiples-worse algorithmic regression.
    /// See PLAN.md's M1.10 section for this same number recorded with an
    /// honest verdict (over budget, not passing).
    ///
    /// **A store-budget confound an independent review caught and this
    /// fixed**: the first version of this test used the shared
    /// `real_tile_store()` helper (a 16-tile budget, sized for this
    /// file's many *smaller*-viewport tests). An 800x600 viewport's own
    /// `TileResidency` grid needs `(div_ceil(800,256)+1) *
    /// (div_ceil(600,256)+1) = 5 * 4 = 20` slots on its own -- so a
    /// 16-tile budget couldn't even hold one frame's
    /// own visible tiles, forcing intra-frame evict-and-reload thrashing
    /// before panning was even a factor, inflating the originally-reported
    /// numbers (mean 43.95 ms / p99 109.38 ms) by roughly 10-15%. Fixed by
    /// giving this test its own 32-tile store -- comfortably above one
    /// frame's 20-tile need, while still tight relative to a 40-frame pan
    /// across a 300,000px document, so real cross-frame eviction (the
    /// thing actually meant to be under test) still happens; see the
    /// store construction below for the full reasoning. The qualitative
    /// verdict (well over budget) is unchanged by this fix -- only the
    /// specific multiplier was overstated before it.
    ///
    /// **A real, separate finding surfaced while building this test,
    /// unrelated to the budget confound above**: even with a correctly-
    /// sized store, `aurora_brush::stamp_dab` can still occasionally
    /// return a `TileError` (`CorruptFile`/`Io`) from a page-in racing the
    /// background writer's still-in-flight write for a tile evicted
    /// moments earlier -- a real, previously-unexercised race in
    /// `aurora_tile::TileStore` between eviction and an immediate revisit
    /// under genuine cross-frame paging pressure, not a bug in this test
    /// or an artifact of the confound above. It's tolerated here exactly
    /// the way `App::paint_dab`'s own real production code already
    /// tolerates any `stamp_dab` failure (logged via `tracing::warn!`, not
    /// fatal to the frame) -- see [`measure_pan_and_paint_frames`]'s own
    /// inline comment at the call site. Reported honestly rather than
    /// routed around with a bigger budget or a per-frame `flush()` this
    /// loop's real counterpart (`App::redraw`) never calls either. This is
    /// a real data-integrity finding, not a footnote: see PLAN.md's M1.1
    /// section (`aurora-tile`) for the tracked, still-open follow-up item,
    /// not just this test's own doc comment.
    ///
    /// **What this does NOT cover** (stated honestly, matching this file's
    /// own brush-latency tests): no real widget/UI paint alongside the
    /// canvas (`collect_widget_paints`/`draw_widget_paints` are separate
    /// and not exercised here); no real `winit` event loop or window
    /// surface/present overhead (this targets an offscreen texture, the
    /// same headless technique `spike/vertical-slice`'s own
    /// `headless_bench` already validated, never a real swapchain); no
    /// CPU-fallback path in *this* test (the sibling test below covers
    /// that); no real GPU hardware has been confirmed for this test at
    /// all yet, only this sandbox's software Vulkan adapter (see the
    /// budget paragraph above); and this is one adapter, one platform
    /// (Linux) -- not cross-platform evidence.
    #[test]
    fn recomposite_and_present_loop_measures_pan_while_painting_at_the_300000px_ceiling() {
        // Real GitHub Actions CI runs (2026-08-12) hit p99 up to 889ms --
        // far past this local sandbox's own ~99ms figure and the 350ms
        // budget calibrated against it, causing real CI failures. GitHub's
        // runner is evidently far slower/noisier than the dev sandboxes
        // this was originally tuned against (a shared runner, likely also
        // software-rendered, under unpredictable contention) -- so the
        // budget is now set with real headroom over the worst *CI* number
        // actually observed, not the local one: ~3.4x 889ms. See PLAN.md's
        // M1.10 section for the full account of this correction.
        const BUDGET_MS: f64 = 3000.0;

        let Some(context) = real_gpu_context() else {
            return;
        };
        // Deliberately *not* the shared `real_tile_store()` helper (budget
        // 16 tiles): an 800x600 viewport's own `TileResidency` grid needs
        // `(800/256 + 1) * (600/256 + 1) = 5 * 4 = 20` slots on its own, so
        // a 16-tile budget can't even hold one frame's own visible tiles --
        // every frame would self-evict-and-reload before panning is even
        // considered, an intra-frame pathology rather than the realistic
        // cross-frame paging pressure this loop means to exercise (the
        // same distinction the vertical slice's own `MEMORY_BUDGET` doc
        // comment draws: "deliberately smaller than the working set", not
        // smaller than one screenful -- its own 64 MB budget is ~128
        // tiles against a ~9-tile screenful, generously above it). 32
        // tiles comfortably covers one frame's own 20-tile need while
        // staying tight enough, relative to a 40-frame pan across a
        // 300,000px document, that real cross-frame eviction still
        // happens as the view moves.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(32) else {
            unreachable!("32 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 300_000,
            height: 300_000,
        };
        let layer_id = match layers.add_pixel_layer("canvas", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(surface) = layers.surface_id(layer_id) else {
            unreachable!("just created as a pixel layer");
        };
        assert!(
            document_qualifies_for_gpu_compositing(&layers),
            "a single Normal-blend, full-bounds pixel layer must qualify for the GPU path"
        );

        let mut timings = measure_pan_and_paint_frames(
            &context,
            &layers,
            layer_id,
            surface,
            &mut store,
            (800, 600),
            40,
            (100_000, 100_000),
            (200, 120),
        );
        let (mean, p50, p99, max) = ms_stats(&mut timings);
        println!(
            "recomposite_and_present_loop (GPU path, 300,000px ceiling, pan+paint): n={} \
             mean={mean:.2}ms p50={p50:.2}ms p99={p99:.2}ms max={max:.2}ms (nominal budget \
             16.7ms)",
            timings.len()
        );

        assert!(
            p99 < BUDGET_MS,
            "p99 frame time {p99:.2}ms exceeded the generous {BUDGET_MS:.0}ms CI-safety budget \
             (mean {mean:.2}ms, p50 {p50:.2}ms, max {max:.2}ms) -- a real regression, not noise"
        );
    }

    /// A second, smaller measurement, exercising the CPU compositing
    /// fallback specifically -- `document_qualifies_for_gpu_compositing`
    /// returns `false` here (the single root layer's own blend mode is
    /// `Multiply`, not `Normal`), so every tile in this loop goes through
    /// `resolve_tile`/`aurora_render::composite_tile_cpu`, not
    /// `begin_gpu_composite_tile`. Otherwise the same shape as
    /// [`recomposite_and_present_loop_measures_pan_while_painting_at_the_300000px_ceiling`]
    /// above (same 300,000px document, same tile-granular pan-while-paint
    /// pattern via [`measure_pan_and_paint_frames`]) -- deliberately
    /// smaller (a 512x512 viewport, 12 frames) since this test exists to
    /// confirm the fallback path is genuinely exercised and produce an
    /// honest number for it, not to re-measure the GPU-path scenario a
    /// second time. Same "generous CI-safe budget" reasoning and the same
    /// four NOT-covered caveats as the sibling test above apply here too.
    ///
    /// **Measured locally (this sandbox's real Vulkan adapter -- Mesa
    /// llvmpipe, software rendering, not the "NVIDIA RTX 3090" this
    /// comment originally and wrongly said, see the sibling test's own
    /// budget paragraph above for how that was found -- release build):
    /// mean 25.08 ms, p50 22.57 ms, p99 54.10 ms, max 54.10 ms (n=12) -- also
    /// over the 16.7 ms nominal budget**, reported honestly, not rounded
    /// up (the same figures recorded in PLAN.md's M1.10 section --
    /// reconciled to one canonical run rather than two separately-quoted
    /// local runs a couple ms apart). Smaller than the GPU-path test's
    /// own numbers, consistent with this test's own smaller 512x512
    /// viewport (fewer atlas slots to upload/present per frame) rather
    /// than the CPU fallback itself being cheaper than the GPU path in
    /// general -- the two tests use different viewport sizes on purpose
    /// (see above) and are not a controlled GPU-vs-CPU comparison.
    /// Budget below: 1500 ms, roughly 3.7x the worst p99 (409ms) real
    /// GitHub Actions CI runs actually produced (2026-08-12) -- the
    /// original 180ms figure (~3.3x this sandbox's own local p99) caused
    /// real CI failures, the same correction as the sibling test above.
    #[test]
    fn recomposite_and_present_loop_exercises_the_cpu_fallback_path() {
        // Real GitHub Actions CI runs (2026-08-12) hit p99 up to 409ms --
        // far past this local sandbox's own ~54ms figure and the 180ms
        // budget calibrated against it, causing real CI failures. Same
        // correction as the sibling GPU-path test above: budget now set
        // with real headroom over the worst *CI* number actually observed
        // (~3.7x 409ms), not the local sandbox's own, much lower figure.
        // See PLAN.md's M1.10 section for the full account.
        const BUDGET_MS: f64 = 1500.0;

        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 300_000,
            height: 300_000,
        };
        let layer_id = match layers.add_pixel_layer("canvas", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = layers.set_blend_mode(layer_id, aurora_doc::BlendMode::Multiply) {
            unreachable!("{err:?}");
        }
        let Some(surface) = layers.surface_id(layer_id) else {
            unreachable!("just created as a pixel layer");
        };
        assert!(
            !document_qualifies_for_gpu_compositing(&layers),
            "a Multiply-blend root layer must disqualify the document from the GPU path"
        );

        let mut timings = measure_pan_and_paint_frames(
            &context,
            &layers,
            layer_id,
            surface,
            &mut store,
            (512, 512),
            12,
            (50_000, 50_000),
            (200, 120),
        );
        let (mean, p50, p99, max) = ms_stats(&mut timings);
        println!(
            "recomposite_and_present_loop (CPU fallback path, 300,000px ceiling, pan+paint): \
             n={} mean={mean:.2}ms p50={p50:.2}ms p99={p99:.2}ms max={max:.2}ms (nominal budget \
             16.7ms)",
            timings.len()
        );

        assert!(
            p99 < BUDGET_MS,
            "p99 frame time {p99:.2}ms exceeded the generous {BUDGET_MS:.0}ms CI-safety budget \
             (mean {mean:.2}ms, p50 {p50:.2}ms, max {max:.2}ms)"
        );
    }

    #[test]
    fn sample_pixel_reads_back_a_real_stamped_dab() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0);
        assert!(
            aurora_brush::stamp_dab(
                &mut store,
                surface,
                (10.5, 10.5),
                20.0,
                [1.0, 0.0, 0.0],
                None
            )
            .is_complete(),
            "a healthy store must paint every tile this dab covers"
        );
        let Some([r, g, b, a]) = sample_pixel(&mut store, surface, (10.5, 10.5)) else {
            unreachable!("a dab was just stamped exactly here");
        };
        assert!(r > 0.9, "red channel should be near-opaque red: {r}");
        assert!(g < 0.1, "{g}");
        assert!(b < 0.1, "{b}");
        assert!(a > 0.9, "{a}");
    }

    #[test]
    fn sample_pixel_of_an_untouched_surface_reads_transparent() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0);
        assert_eq!(
            sample_pixel(&mut store, surface, (5.0, 5.0)),
            Some([0.0, 0.0, 0.0, 0.0])
        );
    }

    #[test]
    fn sample_pixel_returns_none_for_negative_coordinates() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0);
        assert_eq!(sample_pixel(&mut store, surface, (-1.0, 5.0)), None);
        assert_eq!(sample_pixel(&mut store, surface, (5.0, -1.0)), None);
    }

    /// A deliberately tiny one-pixel-layer document for the autosave
    /// tests — a single 10x10 layer is one tile, where
    /// [`demo_document`]'s own 4000x3000 canvas is 192 tiles *per
    /// layer*, all of which `aurora_io::write_aur` would page into (and
    /// evict out of) the store on every single write. Small on purpose,
    /// not by accident: these tests are about the autosave path, not
    /// about tile paging throughput.
    fn small_autosave_document() -> (
        aurora_doc::LayerTree,
        aurora_doc::History,
        aurora_doc::LayerId,
    ) {
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let id = match history.add_pixel_layer(&mut layers, "Background", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        (layers, history, id)
    }

    /// Paints one genuinely non-zero texel into `id`'s own surface —
    /// the whole point of the `.aur`-based autosave, and the thing an
    /// all-blank test document could never prove, since
    /// `aurora_io::write_aur` skips every all-zero tile. Returns the
    /// `(surface, texel index, value)` a recovery assertion can check.
    fn paint_one_texel(
        store: &mut aurora_tile::TileStore,
        layers: &aurora_doc::LayerTree,
        id: aurora_doc::LayerId,
    ) -> (aurora_tile::SurfaceId, usize, f32) {
        let Some(surface) = layers.surface_id(id) else {
            unreachable!("id was built as a pixel layer");
        };
        let tile = match store.get_mut(surface, aurora_tile::TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        // Index 3 is texel (0, 0)'s own alpha channel -- a real,
        // non-default value, so the tile is genuinely not all-zero.
        let Some(sample) = tile.texels_mut().get_mut(3) else {
            unreachable!("index 3 is in bounds for a full tile");
        };
        *sample = half::f16::from_f32(0.75);
        (surface, 3, 0.75)
    }

    #[test]
    fn recovering_a_missing_autosave_returns_none() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let path = dir.path().join("aurora-autosave.aur");
        assert!(recover_document(&path, &mut store).is_none());
    }

    #[test]
    fn recovering_garbage_bytes_returns_none() {
        // Garbage now fails at the *ZIP* layer (`ZipArchive::new` can't
        // find a central directory), not at `postcard` journal parsing
        // the way it did when the autosave was raw journal bytes -- a
        // different `aurora_io::IoError` variant reaching the same
        // "fall back to `demo_document`, don't fail to start" answer.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let path = dir.path().join("aurora-autosave.aur");
        if let Err(err) = std::fs::write(&path, b"not a .aur container") {
            unreachable!("{err}");
        }
        assert!(recover_document(&path, &mut store).is_none());
    }

    #[test]
    fn recovering_a_truncated_autosave_container_returns_none() {
        // A crash *during* a write is the realistic way an autosave
        // file goes bad, and a half-written ZIP has no readable central
        // directory. Must still be a silent fall back to
        // `demo_document`, not a panic or a failed start.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let path = dir.path().join("aurora-autosave.aur");
        let (layers, history, id) = small_autosave_document();
        let _painted = paint_one_texel(&mut store, &layers, id);
        write_autosave(&path, &layers, &history, (10, 10), &mut store);

        let full_len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(err) => unreachable!("{err}"),
        };
        assert!(full_len > 0, "the autosave must have written real bytes");
        let file = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err}"),
        };
        if let Err(err) = file.set_len(full_len / 2) {
            unreachable!("{err}");
        }
        drop(file);

        let (_fresh_dir, mut fresh_store) = real_tile_store();
        assert!(
            recover_document(&path, &mut fresh_store).is_none(),
            "a truncated container must fall back, not panic or half-recover"
        );
    }

    /// Red-team's own reproduction, as a regression test: a complete
    /// autosave exists, the scratch disk then goes bad, and the next
    /// autosave — which now succeeds best-effort with tiles missing —
    /// must **not** replace it. Crash-recovery protection has to be
    /// monotonic: a snapshot may only ever be replaced by one at least as
    /// good. The first shape of the best-effort change renamed the
    /// degraded result straight over the single fixed `autosave_path`, so
    /// the complete snapshot was gone and its dropped tiles were
    /// unrecoverable from anywhere.
    #[test]
    fn a_degraded_autosave_never_overwrites_a_complete_one() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let scratch = match tempfile::tempdir() {
            Ok(scratch) => scratch,
            Err(err) => unreachable!("{err}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(scratch.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };

        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        for name in ["first", "second"] {
            let id = match history.add_pixel_layer(&mut layers, name, bounds, None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            let _painted = paint_one_texel(&mut store, &layers, id);
        }

        // Autosave #1, with every tile readable: complete, and it lands
        // on the canonical path.
        let path = dir.path().join("aurora-autosave.aur");
        write_autosave(&path, &layers, &history, (10, 10), &mut store);
        let Ok(complete) = std::fs::read(&path) else {
            unreachable!("the complete autosave must have been written");
        };
        assert!(!complete.is_empty());
        assert!(!partial_autosave_path(&path).exists());

        // The scratch disk goes bad: one tile is now permanently
        // unreadable.
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        let Ok(entries) = std::fs::read_dir(scratch.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        let files: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        assert!(
            !files.is_empty(),
            "at least one tile should have been evicted"
        );
        // Every scratch file, not just one: with a one-tile budget
        // exactly one tile is resident and served from memory, and which
        // one that is depends on the order `write_aur_best_effort`
        // happened to walk the layers in. Corrupting all of them makes
        // "at least one tile is unreadable" true without depending on
        // that order.
        for victim in &files {
            let Ok(bytes) = std::fs::read(victim) else {
                unreachable!("the evicted tile file must be readable");
            };
            let Some(truncated) = bytes.get(..bytes.len() / 2) else {
                unreachable!("half of a slice's own length is always in range");
            };
            if let Err(err) = std::fs::write(victim, truncated) {
                unreachable!("test-local scratch disk must accept the write: {err:?}");
            }
        }

        // Autosave #2, degraded.
        write_autosave(&path, &layers, &history, (10, 10), &mut store);

        let Ok(after) = std::fs::read(&path) else {
            unreachable!("the complete autosave must still be there");
        };
        assert_eq!(
            after, complete,
            "a degraded autosave must leave the complete one byte-for-byte untouched"
        );
        assert!(
            partial_autosave_path(&path).exists(),
            "the degraded snapshot must still be kept, beside the complete one"
        );
        // And recovery still prefers the complete snapshot.
        let (_fresh_dir, mut fresh_store) = real_tile_store();
        let Some((recovered, ..)) = recover_document(&path, &mut fresh_store) else {
            unreachable!("the complete autosave must reopen");
        };
        assert_eq!(recovered.len(), 2);
    }

    /// A tile that cannot be read back must cost the autosave that
    /// *tile*, not the whole document (0.52.2). Since this round made an
    /// unreadable tile fail on every read rather than healing into a
    /// blank one, `write_autosave` calling the refusing `write_aur` would
    /// have meant one bad tile permanently ending crash-recovery
    /// protection for every layer and every later edit in the session,
    /// with nothing visible to the user. It calls
    /// `aurora_io::write_aur_best_effort` instead — see its own doc
    /// comment for why autosave and an explicit Save deliberately differ.
    ///
    /// Two layers, a one-tile store budget: touching the second layer
    /// evicts the first's tile, `flush` makes that write real, and
    /// truncating the file it landed in leaves a tile whose every
    /// subsequent read fails.
    #[test]
    fn write_autosave_still_protects_the_rest_of_the_document_when_one_tile_is_unreadable() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let scratch = match tempfile::tempdir() {
            Ok(scratch) => scratch,
            Err(err) => unreachable!("{err}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(1) else {
            unreachable!("1 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(scratch.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };

        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let mut painted = Vec::new();
        for name in ["broken", "intact"] {
            let id = match history.add_pixel_layer(&mut layers, name, bounds, None) {
                Ok(id) => id,
                Err(err) => unreachable!("{err:?}"),
            };
            painted.push(paint_one_texel(&mut store, &layers, id));
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        // Corrupt the one evicted tile file: "broken"'s own tile.
        let Ok(entries) = std::fs::read_dir(scratch.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        let files: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        let [victim] = files.as_slice() else {
            unreachable!("exactly one tile should have been evicted: {files:?}");
        };
        let Ok(bytes) = std::fs::read(victim) else {
            unreachable!("the evicted tile file must be readable");
        };
        let Some(truncated) = bytes.get(..bytes.len() / 2) else {
            unreachable!("half of a slice's own length is always in range");
        };
        if let Err(err) = std::fs::write(victim, truncated) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        let path = dir.path().join("aurora-autosave.aur");
        write_autosave(&path, &layers, &history, (10, 10), &mut store);

        // Nothing complete was ever written here, so the salvaged
        // snapshot is all there is -- and it lands on the *partial* path,
        // never on the canonical one.
        assert!(
            !path.exists(),
            "a knowingly incomplete autosave must not claim the canonical path"
        );
        assert!(
            partial_autosave_path(&path).exists(),
            "one unreadable tile must not leave the document with no autosave at all"
        );
        let (_fresh_dir, mut fresh_store) = real_tile_store();
        let Some((recovered_layers, ..)) = recover_document(&path, &mut fresh_store) else {
            unreachable!("the salvaged autosave must reopen from the partial path");
        };
        assert_eq!(recovered_layers.len(), 2, "no layer was dropped");
        // The readable layer's own painted texel survived; the
        // unreadable one's tile is simply absent, which reads back
        // blank -- lost either way, since its pixels are gone from the
        // scratch disk, but everything else is still protected.
        let [
            (broken_surface, index, _),
            (intact_surface, intact_index, intact_value),
        ] = painted.as_slice()
        else {
            unreachable!("two layers were just painted");
        };
        for (surface, index, expected) in [
            (*broken_surface, *index, 0.0),
            (*intact_surface, *intact_index, *intact_value),
        ] {
            let tile = match fresh_store.get(surface, aurora_tile::TileId { x: 0, y: 0 }) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(&sample) = tile.texels().get(index) else {
                unreachable!("index 3 is in bounds for a full tile");
            };
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(sample.to_f32(), expected);
            }
        }
    }

    #[test]
    fn write_autosave_produces_a_real_aur_container_with_its_own_archive_entries() {
        // Asserts the container's *actual* ZIP structure (ADR 0009),
        // independent of `aurora_io::read_aur` -- so this test would
        // still catch the autosave path silently going back to writing
        // raw journal bytes even if the reader were changed to match.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let (_store_dir, mut store) = real_tile_store();
        let path = dir.path().join("aurora-autosave.aur");
        let (layers, history, id) = small_autosave_document();
        let _painted = paint_one_texel(&mut store, &layers, id);

        write_autosave(&path, &layers, &history, (10, 10), &mut store);

        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("the autosave was just written: {err}"),
        };
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(archive) => archive,
            Err(err) => unreachable!("the autosave must be a real ZIP container: {err}"),
        };
        let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
        for entry in ["mimetype", "manifest", "history"] {
            assert!(
                names.iter().any(|name| name == entry),
                "the autosave container must hold a `{entry}` entry; found {names:?}"
            );
        }
        // The whole point of the format change: a real painted texel
        // (`paint_one_texel` above) must appear as a real tile entry.
        // Without this the test would still pass on a container that
        // persisted structure only -- exactly the pre-0.49.0 behaviour
        // this path exists to replace.
        assert!(
            names.iter().any(|name| name.starts_with("tiles/")),
            "a painted document's autosave must hold at least one tile entry; found {names:?}"
        );

        let mut mimetype = match archive.by_name("mimetype") {
            Ok(entry) => entry,
            Err(err) => unreachable!("just asserted the entry exists: {err}"),
        };
        assert_eq!(
            mimetype.compression(),
            zip::CompressionMethod::Stored,
            "the mimetype sentinel must be stored uncompressed so a magic-byte sniff can find it"
        );
        let mut contents = String::new();
        if let Err(err) = std::io::Read::read_to_string(&mut mimetype, &mut contents) {
            unreachable!("{err}");
        }
        assert_eq!(contents, "application/vnd.aurora.document");
        drop(mimetype);

        for entry in ["manifest", "history"] {
            let mut bytes = Vec::new();
            let mut read = match archive.by_name(entry) {
                Ok(read) => read,
                Err(err) => unreachable!("just asserted the entry exists: {err}"),
            };
            if let Err(err) = std::io::Read::read_to_end(&mut read, &mut bytes) {
                unreachable!("{err}");
            }
            assert!(
                !bytes.is_empty(),
                "the `{entry}` entry must hold real, non-empty encoded bytes"
            );
        }
    }

    #[test]
    fn writing_then_recovering_an_autosave_restores_real_painted_pixels_and_canvas_size() {
        // The gap this whole path exists to close: before the autosave
        // was a real `.aur` container it held structural `LayerOp`s
        // only, so a crash lost every painted pixel. A *fresh* store in
        // a *fresh* scratch directory below is what makes this a real
        // recovery test rather than the original store happening to
        // still hold the tile.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let (layers, history, id) = small_autosave_document();
        let original_descriptions = history.journal_descriptions();
        let (surface, index, painted) = {
            let (_store_dir, mut store) = real_tile_store();
            let painted = paint_one_texel(&mut store, &layers, id);
            // A canvas size deliberately unequal to the layer's own
            // 10x10 bounds, so this proves the manifest's own value came
            // back rather than `document_canvas_size` re-deriving it.
            write_autosave(&path, &layers, &history, (37, 21), &mut store);
            painted
        };
        drop(layers);
        drop(history);

        let (_fresh_dir, mut fresh_store) = real_tile_store();
        let Some((recovered_layers, recovered_history, canvas_size)) =
            recover_document(&path, &mut fresh_store)
        else {
            unreachable!("just wrote a real autosave container");
        };
        assert_eq!(canvas_size, (37, 21), "the manifest's own canvas size");
        assert_eq!(
            recovered_history.journal_descriptions(),
            original_descriptions
        );
        assert_eq!(recovered_layers.name(id), Some("Background"));

        let tile = match fresh_store.get(surface, aurora_tile::TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(&sample) = tile.texels().get(index) else {
            unreachable!("index is in bounds for a full tile");
        };
        assert!(
            (sample.to_f32() - painted).abs() < f32::EPSILON,
            "the recovered store must hold the painted texel, not a blank tile"
        );
    }

    #[test]
    fn write_autosave_uses_a_unique_temp_path_and_leaves_none_behind() {
        // Two writes in a row must never reuse one fixed `.tmp` name:
        // that name is shared state between every writer that exists
        // (two Aurora processes are enough), and two writers on it
        // interleave their bytes into the crash-recovery file. Also
        // asserts the successful path cleans up after itself -- the
        // temp file is renamed, not left as litter.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let first = super::autosave_temp_path(&path);
        let second = super::autosave_temp_path(&path);
        assert_ne!(
            first, second,
            "each write must claim its own temp path, not share one fixed name"
        );
        assert_eq!(first.parent(), path.parent(), "the temp file is a sibling");

        let (_store_dir, mut store) = real_tile_store();
        let (layers, history, id) = small_autosave_document();
        let _painted = paint_one_texel(&mut store, &layers, id);
        write_autosave(&path, &layers, &history, (10, 10), &mut store);
        write_autosave(&path, &layers, &history, (10, 10), &mut store);

        let leftovers: Vec<String> = match std::fs::read_dir(dir.path()) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| {
                    std::path::Path::new(name)
                        .extension()
                        .is_some_and(|ext| ext == "tmp")
                })
                .collect(),
            Err(err) => unreachable!("{err}"),
        };
        assert!(
            leftovers.is_empty(),
            "a successful autosave must rename its temp file away, not leave it: {leftovers:?}"
        );
        assert!(path.exists(), "the autosave itself must be in place");
    }

    #[cfg(unix)]
    #[test]
    fn write_autosave_creates_an_owner_only_file() {
        // The autosave lives at a predictable name in a world-readable
        // temp directory and now holds the document's real pixels, not
        // just its layer structure -- so the file's own mode is the
        // only thing keeping another local user from reading it. (The
        // directory itself, and Windows ACLs, are the separate
        // app-support-directory move `create_autosave_temp` names.)
        use std::os::unix::fs::PermissionsExt as _;
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let (_store_dir, mut store) = real_tile_store();
        let (layers, history, _id) = small_autosave_document();
        write_autosave(&path, &layers, &history, (10, 10), &mut store);
        let mode = match std::fs::metadata(&path) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(err) => unreachable!("{err}"),
        };
        assert_eq!(mode, 0o600, "the autosave must be owner-only");
    }

    /// A canonical autosave that exists but cannot be read must not
    /// shadow a usable partial one (0.52.2, final review round).
    /// Corruption is precisely the case the partial snapshot is kept for,
    /// and `recover_document` alone reaches the partial only when the
    /// canonical file is *absent* — so before this, a corrupt canonical
    /// container defeated the fallback entirely.
    #[test]
    fn startup_recovers_from_the_partial_autosave_when_the_complete_one_is_corrupt() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let (layers, history, id) = small_autosave_document();
        {
            let (_store_dir, mut store) = real_tile_store();
            let _painted = paint_one_texel(&mut store, &layers, id);
            write_autosave(&path, &layers, &history, (37, 21), &mut store);
        }
        // A good partial snapshot beside a canonical file that is then
        // corrupted -- a half-written ZIP, the realistic shape.
        let partial = partial_autosave_path(&path);
        if let Err(err) = std::fs::copy(&path, &partial) {
            unreachable!("{err}");
        }
        let full_len = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(err) => unreachable!("{err}"),
        };
        let file = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(file) => file,
            Err(err) => unreachable!("{err}"),
        };
        if let Err(err) = file.set_len(full_len / 2) {
            unreachable!("{err}");
        }
        drop(file);

        let (_fresh_dir, fresh_store) = real_tile_store();
        let mut slot = Some(fresh_store);
        let startup = super::startup_document(true, &path, &mut slot);

        assert!(
            startup.was_recovered,
            "a corrupt complete autosave must fall back to the partial one, not to a demo document"
        );
        assert_eq!(
            startup.canvas_size,
            (37, 21),
            "the recovered document must be the partial container's own, not a fresh one"
        );
    }

    #[test]
    fn startup_document_leaves_a_recovered_autosave_untouched() {
        // The startup write is skipped when recovery succeeded: the
        // file already *is* this document, so rebuilding the container
        // would be a full tile-grid walk (measured in hundreds of ms)
        // on the pre-window path for a byte-identical result. Compares
        // the real bytes, not a timestamp, so it can't pass by
        // coincidence of clock granularity.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let (layers, history, id) = small_autosave_document();
        {
            let (_store_dir, mut store) = real_tile_store();
            let _painted = paint_one_texel(&mut store, &layers, id);
            write_autosave(&path, &layers, &history, (10, 10), &mut store);
        }
        let before = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err}"),
        };

        let (_fresh_dir, fresh_store) = real_tile_store();
        let mut slot = Some(fresh_store);
        let startup = super::startup_document(true, &path, &mut slot);
        assert!(
            startup.was_recovered,
            "the container just written must read back"
        );
        let after = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("{err}"),
        };
        assert_eq!(
            before, after,
            "a successful recovery must not rewrite the file it just read"
        );
    }

    #[test]
    fn startup_document_writes_a_fresh_documents_autosave() {
        // The other half: with nothing to recover, the session's own
        // document does get written out, so the *next* run has
        // something real to recover to.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        let (_store_dir, store) = real_tile_store();
        let mut slot = Some(store);
        let startup = super::startup_document(false, &path, &mut slot);
        assert!(!startup.was_recovered);
        assert!(
            path.exists(),
            "a fresh session must leave a recoverable autosave behind"
        );
    }

    #[test]
    fn startup_document_reopens_the_store_after_a_failed_recovery() {
        // A recovery attempt that fails part-way can leave real pixels
        // committed to surfaces the fallback document is about to
        // reuse. The store is replaced rather than reused, so those
        // fragments can't show up painted into a document they have
        // nothing to do with.
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.aur");
        if let Err(err) = std::fs::write(&path, b"not a .aur container") {
            unreachable!("{err}");
        }
        let (_store_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0);
        let tile_id = aurora_tile::TileId { x: 0, y: 0 };
        {
            let tile = match store.get_mut(surface, tile_id) {
                Ok(tile) => tile,
                Err(err) => unreachable!("{err:?}"),
            };
            let Some(sample) = tile.texels_mut().get_mut(0) else {
                unreachable!("index is in bounds for a full tile");
            };
            *sample = half::f16::from_f32(0.5);
        }
        let mut slot = Some(store);
        let startup = super::startup_document(true, &path, &mut slot);
        assert!(!startup.was_recovered, "garbage must not read back");
        let Some(store) = slot.as_mut() else {
            unreachable!("reopening a real scratch directory must succeed here");
        };
        let tile = match store.get(surface, tile_id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        assert!(
            tile.texels().iter().all(|sample| sample.to_f32() == 0.0),
            "the fallback document must start from a blank store, not one holding \
             fragments of a document that failed to recover"
        );
    }

    #[test]
    fn layout_path_is_real_and_distinct_from_the_marker_and_autosave_paths() {
        // A real, if unremarkable, assertion: this sandbox's own home
        // directory is real, so `directories::ProjectDirs::from` must
        // succeed here -- the same "ordinary conditions" assumption
        // `open_tile_store_succeeds_against_the_real_scratch_directory`
        // already makes for `std::env::temp_dir()`.
        let Some(path) = super::layout_path() else {
            unreachable!("a real home directory exists in this environment");
        };
        assert_ne!(path, super::marker_path());
        assert_ne!(path, autosave_path());
    }

    #[test]
    fn writing_then_loading_a_workspace_layout_round_trips_the_real_values() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("workspace-layout.postcard");
        let mut original = aurora_ui::build_workspace();
        if let Err(err) =
            aurora_ui::set_rail_width(&mut original.tree, original.rail, original.divider, 300.0)
        {
            unreachable!("{err:?}");
        }
        if let Err(err) =
            aurora_ui::set_panel_collapsed(&mut original.tree, original.properties, true)
        {
            unreachable!("{err:?}");
        }

        super::save_workspace_layout(&path, &original);
        let mut loaded = aurora_ui::build_workspace();
        super::load_workspace_layout(&path, &mut loaded);

        assert_eq!(
            aurora_ui::rail_width(&loaded.tree, loaded.rail),
            Some(300.0)
        );
        match aurora_ui::panel_is_collapsed(&loaded.tree, loaded.layers) {
            Ok(collapsed) => assert!(!collapsed, "layers was never collapsed in the original"),
            Err(err) => unreachable!("{err:?}"),
        }
        match aurora_ui::panel_is_collapsed(&loaded.tree, loaded.properties) {
            Ok(collapsed) => assert!(collapsed, "properties must round-trip as collapsed"),
            Err(err) => unreachable!("{err:?}"),
        }
        match aurora_ui::panel_is_collapsed(&loaded.tree, loaded.history) {
            Ok(collapsed) => assert!(!collapsed, "history was never collapsed in the original"),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn loading_a_missing_workspace_layout_leaves_the_workspace_at_its_own_defaults() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("does-not-exist.postcard");
        let mut workspace = aurora_ui::build_workspace();

        super::load_workspace_layout(&path, &mut workspace);

        assert_eq!(
            aurora_ui::rail_width(&workspace.tree, workspace.rail),
            Some(250.0)
        );
        match aurora_ui::panel_is_collapsed(&workspace.tree, workspace.layers) {
            Ok(collapsed) => assert!(!collapsed),
            Err(err) => unreachable!("{err:?}"),
        }
    }

    #[test]
    fn crash_recovery_dialog_message_differs_by_whether_recovery_happened() {
        assert_ne!(
            crash_recovery_dialog_message(true),
            crash_recovery_dialog_message(false)
        );
    }

    #[test]
    fn open_crash_recovery_dialog_focuses_its_only_action() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);

        let Some(handle) = dialog else {
            unreachable!("must open");
        };
        assert_eq!(
            workspace
                .tree
                .accessibility(handle.root)
                .map(accesskit::Node::role),
            Some(accesskit::Role::AlertDialog)
        );
        assert_eq!(focus.focused(), handle.first_action());
        assert_eq!(handle.actions.len(), 1);
        let Some((id, _)) = handle.actions.first() else {
            unreachable!("just asserted len() == 1");
        };
        assert_eq!(id, CRASH_RECOVERY_CONTINUE);
    }

    #[test]
    fn opening_the_crash_recovery_dialog_a_second_time_is_a_no_op() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        let first = dialog.clone();
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        assert_eq!(dialog, first, "already open must not reopen or replace it");
    }

    #[test]
    fn enter_on_the_focused_action_closes_the_dialog() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        let Some(handle) = dialog.clone() else {
            unreachable!("just opened");
        };

        handle_dialog_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Enter)),
        );

        assert_eq!(dialog, None);
        assert!(!workspace.tree.contains(handle.root));
        assert_eq!(focus.focused(), None);
    }

    #[test]
    fn escape_also_closes_the_dialog() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);

        handle_dialog_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            KeyChord::new(Modifiers::none(), Key::Named(NamedKey::Escape)),
        );

        assert_eq!(dialog, None);
    }

    #[test]
    fn clicking_the_dialogs_action_button_closes_it() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        let Some(handle) = dialog.clone() else {
            unreachable!("just opened");
        };
        let Some(button) = handle.first_action() else {
            unreachable!("the crash recovery dialog always has one action");
        };
        // `hit_test` requires every ancestor along the path, not just
        // the button itself, to actually contain the point -- no real
        // `compute_layout` has run in this test, so every node defaults
        // to zero-size bounds; set all three explicitly, the same
        // isolated-geometry shape `hit_test`'s own unit tests in
        // `aurora-widgets` already use.
        let big = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        };
        for id in [workspace.root, handle.root, button] {
            if let Err(err) = workspace.tree.set_bounds(id, big) {
                unreachable!("{err:?}");
            }
        }

        let opened = handle_dialog_pointer(
            &mut workspace,
            &mut focus,
            &mut dialog,
            PointerButton::Primary,
            (10.0, 10.0),
        );

        assert!(opened, "a dialog was open to route the click to");
        assert_eq!(dialog, None, "clicking the action button must close it");
        assert!(!workspace.tree.contains(handle.root));
    }

    #[test]
    fn clicking_elsewhere_while_the_dialog_is_open_swallows_the_click_without_closing_it() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        let Some(handle) = dialog.clone() else {
            unreachable!("just opened");
        };
        let Some(button) = handle.first_action() else {
            unreachable!("the crash recovery dialog always has one action");
        };
        // Root and the dialog's own root are real and hit-testable
        // (same reasoning as `clicking_the_dialogs_action_button_closes_it`),
        // but the button itself sits in just one corner -- so a click
        // inside the dialog, but outside the button, hits the dialog's
        // own root instead, which has no `action_id` of its own.
        if let Err(err) = workspace.tree.set_bounds(
            workspace.root,
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 1000,
                height: 1000,
            },
        ) {
            unreachable!("{err:?}");
        }
        if let Err(err) = workspace.tree.set_bounds(
            handle.root,
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 1000,
                height: 1000,
            },
        ) {
            unreachable!("{err:?}");
        }
        if let Err(err) = workspace.tree.set_bounds(
            button,
            aurora_core::Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
        ) {
            unreachable!("{err:?}");
        }

        // Inside the dialog's own root, but past the button's own
        // bottom-right corner.
        let opened = handle_dialog_pointer(
            &mut workspace,
            &mut focus,
            &mut dialog,
            PointerButton::Primary,
            (500.0, 500.0),
        );

        assert!(opened, "a dialog was open, so the click must be swallowed");
        assert_eq!(
            dialog,
            Some(handle),
            "clicking outside the dialog's own buttons must not close it"
        );
    }

    #[test]
    fn handle_dialog_pointer_returns_false_when_no_dialog_is_open() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        assert!(!handle_dialog_pointer(
            &mut workspace,
            &mut focus,
            &mut dialog,
            PointerButton::Primary,
            (0.0, 0.0),
        ));
    }

    #[test]
    fn close_crash_recovery_dialog_on_an_already_closed_dialog_is_a_no_op() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        close_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog);
        assert_eq!(dialog, None);
    }

    #[test]
    fn handle_key_routes_to_the_dialog_before_the_palette_when_both_could_be_open() {
        // The dialog takes priority per `handle_key`'s own routing order
        // -- a modal alert blocks everything else. Since only one can
        // ever actually be open in practice (the dialog only opens at
        // startup, before any shortcut could open the palette), this
        // confirms the *routing rule itself*, not a reachable real
        // scenario.
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut composite_cache = CompositeCache::default();
        let shortcuts = default_shortcuts();
        let scales = match load_scales() {
            Ok(scales) => scales,
            Err(err) => unreachable!("{err}"),
        };
        open_crash_recovery_dialog(&mut workspace, &mut focus, &mut dialog, &scales, false);
        open_command_palette(&mut workspace, &mut focus, &mut palette);

        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
            &mut layers,
            &mut history,
            &mut pixel_history,
            None,
            &mut undo_order,
            &mut composite_cache,
            &shortcuts,
            Modifiers::none(),
            Key::Named(NamedKey::Escape),
            None,
            &mut FakeClipboard::default(),
            &mut FakeFileDialog::default(),
        );

        assert_eq!(
            dialog, None,
            "Escape must close the dialog, not the palette"
        );
        assert!(
            palette.is_some(),
            "the palette must be untouched while the dialog was still open"
        );
    }

    // -- Basic tools: pointer input, canvas view, tool dispatch --

    #[test]
    fn translate_pointer_button_maps_the_three_known_buttons() {
        assert_eq!(
            translate_pointer_button(winit::event::MouseButton::Left),
            Some(PointerButton::Primary)
        );
        assert_eq!(
            translate_pointer_button(winit::event::MouseButton::Right),
            Some(PointerButton::Secondary)
        );
        assert_eq!(
            translate_pointer_button(winit::event::MouseButton::Middle),
            Some(PointerButton::Middle)
        );
    }

    #[test]
    fn translate_pointer_button_returns_none_for_an_unmapped_button() {
        assert_eq!(
            translate_pointer_button(winit::event::MouseButton::Back),
            None
        );
    }

    #[test]
    fn logical_point_divides_out_a_scale_factor() {
        assert_eq!(logical_point((200.0, 100.0), 2.0), (100.0, 50.0));
    }

    #[test]
    fn logical_point_falls_back_to_one_for_a_non_positive_scale_factor() {
        assert_eq!(logical_point((50.0, 25.0), 0.0), (50.0, 25.0));
        assert_eq!(logical_point((50.0, 25.0), -1.0), (50.0, 25.0));
    }

    // -- Retina/HiDPI `redraw` fix: effective residency zoom --
    //
    // A real bug report on real Retina macOS hardware (`scale_factor`
    // ~2.0): painting landed up-and-left of the cursor, worse the
    // farther from the canvas's own top-left corner. Root cause:
    // `redraw`'s one real `residency.set_origin` call site fed
    // `aurora_gpu::TileResidency` a **physical**-pixel viewport
    // (`canvas_area_physical_size`) alongside `CanvasView::zoom`'s own
    // **logical**-pixel-semantics zoom, uncorrected -- so a
    // `scale_factor != 1.0` display rendered at half the intended
    // magnification. `effective_residency_zoom` is the fix's whole
    // arithmetic; these tests are its headless proof, mirroring
    // `logical_size`/`logical_point`'s own tests immediately above for
    // the identity/scaling/fallback shape.

    #[test]
    // Exact-literal comparisons of plain multiplication by powers of two
    // (no accumulated float noise) -- same reasoning this crate's other
    // float_cmp allows already document (e.g.
    // `load_background_color_resolves_a_real_linear_token`, just above).
    #[allow(clippy::float_cmp)]
    fn effective_residency_zoom_is_unchanged_at_a_scale_factor_of_one() {
        // Every prior test/sandbox run, lacking a real Retina display,
        // only ever exercised `scale_factor == 1.0` -- this case must
        // stay exactly as it was: effective zoom equals the raw canvas
        // zoom, unscaled.
        assert_eq!(effective_residency_zoom(1.0, 1.0), 1.0);
        assert_eq!(effective_residency_zoom(2.5, 1.0), 2.5);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn effective_residency_zoom_scales_by_a_real_retina_factor() {
        // The real reported scenario: a Retina `scale_factor` of 2.0 at
        // 100% canvas zoom must double the zoom actually handed to
        // `TileResidency::set_origin`, so a physical-pixel viewport
        // resolves to the same document-pixel count `CanvasView`'s own
        // logical-pixel contract promises.
        assert_eq!(effective_residency_zoom(1.0, 2.0), 2.0);
        assert_eq!(effective_residency_zoom(1.5, 2.0), 3.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn effective_residency_zoom_falls_back_to_one_for_a_non_positive_scale_factor() {
        assert_eq!(effective_residency_zoom(1.0, 0.0), 1.0);
        assert_eq!(effective_residency_zoom(1.0, -2.0), 1.0);
        // A non-1.0 `canvas_zoom` here proves the fallback actually
        // multiplies by `1.0` (i.e. leaves `canvas_zoom` unchanged),
        // not that it hardcodes a return of `1.0` regardless of input —
        // `logical_size_falls_back_to_one_for_a_non_positive_scale_factor`'s
        // own non-trivial-input idiom, applied here too.
        assert_eq!(effective_residency_zoom(2.5, 0.0), 2.5);
        assert_eq!(effective_residency_zoom(2.5, -2.0), 2.5);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn effective_residency_zoom_falls_back_to_one_for_a_non_finite_scale_factor() {
        assert_eq!(effective_residency_zoom(1.0, f64::NAN), 1.0);
        assert_eq!(effective_residency_zoom(1.0, f64::INFINITY), 1.0);
        // Same non-trivial-`canvas_zoom` proof as the non-positive case
        // above.
        assert_eq!(effective_residency_zoom(2.5, f64::NAN), 2.5);
        assert_eq!(effective_residency_zoom(2.5, f64::INFINITY), 2.5);
    }

    // -- The render path and the pointer path must read one zoom --
    //
    // RT12-04. `redraw` hands the atlas
    // `effective_residency_zoom(canvas_view.zoom(), scale_factor)`, and
    // `aurora_gpu::TileResidency` will not render below its own
    // `min_zoom_for_viewport`; `CanvasView::to_document` -- what turns a
    // click into the document position a brush dab lands on -- divides
    // by `canvas_view.zoom()`. If those two numbers can differ, paint
    // lands somewhere other than the pixel under the cursor, and nothing
    // reports it. Measured before the fix, at canvas zoom 0.25 on a
    // 1920 px viewport: a click at screen x = 960 converted to document
    // x = 3840 while the pixel drawn there was document x ~= 1152; at
    // `MIN_ZOOM` the two were ~83x apart. It is the same failure shape
    // `CanvasView::clamp_pan_to_minimum` already documents on the pan
    // axis, and the fix is the same: one bound, applied at the source.

    /// Every canvas size and scale factor these tests sweep — a 1x and a
    /// 2x display, tile-aligned and not, wide and tall.
    const CANVAS_CASES: [((u32, u32), f64); 6] = [
        ((1920, 1080), 1.0),
        ((1920, 1080), 2.0),
        ((3840, 2160), 2.0),
        ((1366, 768), 1.0),
        ((1024, 1024), 1.0),
        ((777, 333), 1.25),
    ];

    #[test]
    fn canvas_min_zoom_leaves_the_atlas_with_nothing_left_to_clamp() {
        // The identity the whole fix rests on: once `CanvasView` is held
        // at `canvas_min_zoom`, the zoom `redraw` passes the atlas is
        // one the atlas renders *unchanged*. `TileResidency::
        // effective_zoom` is that crate's own statement of what it will
        // actually render, so comparing against it -- rather than
        // re-deriving the floor here -- is what keeps the two crates
        // from drifting apart on some later change.
        for (canvas, scale_factor) in CANVAS_CASES {
            let floor = canvas_min_zoom(canvas, scale_factor);
            let passed = effective_residency_zoom(floor, scale_factor);
            let rendered = aurora_gpu::TileResidency::effective_zoom(canvas, passed);
            assert!(
                (rendered - passed).abs() <= f32::EPSILON * passed,
                "canvas {canvas:?} at scale factor {scale_factor}: the view's \
                 own floor {floor} reaches the atlas as {passed}, which it \
                 renders at {rendered}. Any gap here is a gap between where a \
                 click paints and where the pixel under it was drawn"
            );
        }
    }

    #[test]
    fn the_render_path_and_the_pointer_path_agree_on_scale_when_zoomed_out() {
        // The reproduction itself, through the real helpers `redraw` and
        // the pointer handlers use: `canvas_local_origin` (what the
        // renderer is told is at the canvas's top-left corner),
        // `effective_residency_zoom` (the zoom it is told to draw at),
        // and `CanvasView::to_document` (where a click lands).
        for (canvas, scale_factor) in CANVAS_CASES {
            for requested in [0.01_f32, 0.25, 0.5, 1.0, 4.0] {
                let mut view = CanvasView::new();
                view.set_min_zoom(canvas_min_zoom(canvas, scale_factor));
                view.zoom_at((0.0, 0.0), requested);
                // An arbitrary, deliberately fractional pan, then the
                // pan bound the real handlers apply.
                view.pan_by((-137.75, -42.5));
                view.clamp_pan_to_minimum((0.0, 0.0));

                let layer_origin = (300.0, 150.0);
                let doc_origin = canvas_local_origin(&view, layer_origin);
                let rendered_zoom = aurora_gpu::TileResidency::effective_zoom(
                    canvas,
                    effective_residency_zoom(view.zoom(), scale_factor),
                );

                #[allow(clippy::cast_precision_loss)]
                let width_logical = canvas.0 as f32 / scale_factor as f32;
                for screen_x in [0.0_f32, 1.0, width_logical / 2.0, width_logical - 1.0] {
                    // Where the renderer actually draws: the atlas
                    // covers `1 / rendered_zoom` document pixels per
                    // *physical* pixel, starting at `doc_origin`.
                    let physical_x = screen_x * scale_factor as f32;
                    let drawn_here = doc_origin.0 + physical_x / rendered_zoom;
                    // Where a click at the same place paints, in the
                    // same layer-local space.
                    let painted_here = view.to_document((screen_x, 0.0)).0 - layer_origin.0;
                    assert!(
                        (drawn_here - painted_here).abs() <= 0.05 + drawn_here.abs() * 1e-4,
                        "canvas {canvas:?} at scale factor {scale_factor}, zoom \
                         requested {requested} (held at {}): a click at canvas \
                         x = {screen_x} paints document x = {painted_here}, but \
                         the pixel actually drawn there is document x = \
                         {drawn_here}",
                        view.zoom()
                    );
                }
            }
        }
    }

    #[test]
    fn applying_the_zoom_floor_cannot_push_the_view_past_a_moved_layers_edge() {
        // The ordering hazard: a view can already be zoomed out and
        // panned to a moved layer's own top-left edge before the floor
        // is known (the first frame of a session, or a scale-factor
        // change). Raising the zoom then must not move
        // `canvas_local_origin` negative -- `TileResidency::set_origin`
        // clamps a negative document origin to zero while
        // `CanvasView::to_document` would not, which is the pan-axis
        // half of this same divergence and the one
        // `clamp_pan_to_minimum` already exists to prevent.
        let canvas = (1920, 1080);
        let layer_origin = (300.0, 150.0);
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.25);
        view.pan_by((-90.0, -30.0));
        view.clamp_pan_to_minimum(layer_origin);

        view.set_min_zoom(canvas_min_zoom(canvas, 1.0));

        let local = canvas_local_origin(&view, layer_origin);
        assert!(
            local.0 >= -1e-3 && local.1 >= -1e-3,
            "after the floor was applied the renderer would be told to draw \
             from layer-local {local:?}, which it clamps to (0, 0) while \
             `to_document` keeps reporting the negative value -- render and \
             paint disagreeing again, on the pan axis this time"
        );
    }

    #[test]
    fn a_view_left_unbounded_is_exactly_the_divergence_this_prevents() {
        // The negative control: the same arithmetic with the floor *not*
        // applied is the measured pre-fix report. Without this, the test
        // above could pass because the numbers happen to be small rather
        // than because anything is bounded.
        let canvas = (1920, 1080);
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.25);
        let doc_origin = canvas_local_origin(&view, (0.0, 0.0));
        let rendered_zoom = aurora_gpu::TileResidency::effective_zoom(
            canvas,
            effective_residency_zoom(view.zoom(), 1.0),
        );
        let drawn_here = doc_origin.0 + 960.0 / rendered_zoom;
        let painted_here = view.to_document((960.0, 0.0)).0;
        assert!(
            (drawn_here - painted_here).abs() > 1000.0,
            "an unbounded 0.25 zoom must still diverge by thousands of \
             document pixels ({painted_here} painted vs {drawn_here} drawn) -- \
             if it no longer does, the test above has stopped proving anything"
        );
    }

    #[test]
    // Exact: the re-derived floor must be bit-identical to the one
    // `canvas_min_zoom` produced, not merely close to it -- an
    // approximate match here would hide exactly the lapse this covers.
    #[allow(clippy::float_cmp)]
    fn resetting_the_canvas_view_never_leaves_the_zoom_floor_absent() {
        // `CanvasView::default()` resets `min_zoom` to `MIN_ZOOM`, and
        // `open_file`/`open_aur_file` reset the view on every document
        // open. Before this went through `reset_canvas_view`, the floor
        // was simply gone until the next `redraw` re-applied it.
        for (canvas, scale_factor) in CANVAS_CASES {
            let mut previous = CanvasView::new();
            previous.set_min_zoom(canvas_min_zoom(canvas, scale_factor));

            let reset = reset_canvas_view(&previous, Some(canvas), scale_factor);
            assert!(
                reset.min_zoom() > aurora_ui::canvas_view::MIN_ZOOM,
                "canvas {canvas:?} at scale factor {scale_factor}: a reset view \
                 came back at the bare MIN_ZOOM {}, i.e. with no atlas floor at \
                 all",
                reset.min_zoom()
            );
            assert_eq!(
                reset.min_zoom(),
                previous.min_zoom(),
                "canvas {canvas:?} at scale factor {scale_factor}: the re-derived \
                 floor must be the same number the view already held"
            );

            // And with no canvas area to re-derive from, the previous
            // floor is carried rather than dropped.
            let carried = reset_canvas_view(&previous, None, scale_factor);
            assert_eq!(carried.min_zoom(), previous.min_zoom());
        }
    }

    #[test]
    fn the_paths_still_agree_immediately_after_a_document_open_reset() {
        // The reproduction for the reset window itself: open a document,
        // then take a pointer position through the real conversion
        // *before* any redraw has run. With `CanvasView::default()` in
        // place of `reset_canvas_view` this diverges by hundreds to
        // thousands of document pixels, because the view accepts a zoom
        // the atlas will not render.
        for (canvas, scale_factor) in CANVAS_CASES {
            let mut previous = CanvasView::new();
            previous.set_min_zoom(canvas_min_zoom(canvas, scale_factor));

            let mut view = reset_canvas_view(&previous, Some(canvas), scale_factor);
            // The scroll event that arrives before the next frame.
            view.zoom_at((0.0, 0.0), aurora_ui::canvas_view::MIN_ZOOM);

            let doc_origin = canvas_local_origin(&view, (0.0, 0.0));
            let rendered_zoom = aurora_gpu::TileResidency::effective_zoom(
                canvas,
                effective_residency_zoom(view.zoom(), scale_factor),
            );
            #[allow(clippy::cast_possible_truncation)]
            let scale = scale_factor as f32;
            #[allow(clippy::cast_precision_loss)]
            let width_logical = canvas.0 as f32 / scale;
            for screen_x in [0.0_f32, width_logical / 2.0, width_logical - 1.0] {
                let drawn_here = doc_origin.0 + screen_x * scale / rendered_zoom;
                let painted_here = view.to_document((screen_x, 0.0)).0;
                assert!(
                    (drawn_here - painted_here).abs() <= 0.05 + drawn_here.abs() * 1e-4,
                    "canvas {canvas:?} at scale factor {scale_factor}: a click at \
                     canvas x = {screen_x} immediately after a document-open reset \
                     paints document x = {painted_here} while the pixel drawn there \
                     is document x = {drawn_here}"
                );
            }
        }
    }

    #[test]
    // Exact: the guard's whole contract is that it returns literally
    // `1.0` for a degenerate input.
    #[allow(clippy::float_cmp)]
    fn the_scale_factor_guard_catches_values_that_only_degenerate_on_the_cast() {
        // `f64 as f32` can *create* the degenerate values the guard
        // rejects: above `f32::MAX` casts to infinity, below
        // `f32::MIN_POSITIVE` casts to zero. Validating the `f64` first
        // and casting second let both straight through.
        for bad in [1e39_f64, -1e39, 1e-320, f64::MAX, f64::MIN_POSITIVE] {
            assert!(
                bad.is_finite(),
                "this case is meant to be finite as an f64 -- the point is that \
                 only the cast makes it degenerate"
            );
            let guarded = guarded_scale_factor(bad);
            assert_eq!(
                guarded, 1.0,
                "scale factor {bad} degenerates on the cast to f32 and must fall \
                 back to 1.0, not reach the zoom arithmetic as {guarded}"
            );
        }
        // And a floor derived through it stays a real, usable number
        // rather than 0 or infinity.
        let floor = canvas_min_zoom((1920, 1080), 1e39);
        assert!(
            floor.is_finite() && floor > 0.0,
            "a floor derived from an extreme scale factor came back as {floor}"
        );
    }

    #[test]
    fn the_ulp_correction_keeps_the_round_trip_at_or_above_the_atlas_floor() {
        // The `next_up` loop in `canvas_min_zoom` fires only when
        // `floor / scale * scale` lands one ulp below `floor`; no
        // `CANVAS_CASES` entry reaches it. Sweeping real viewport and
        // scale-factor combinations does, and the property asserted is
        // the one the loop exists for.
        let mut corrected = 0_u32;
        for width in [1280_u32, 1366, 1440, 1600, 1920, 2560, 3440, 3840] {
            for height in [720_u32, 768, 800, 900, 1080, 1440, 1600, 2160] {
                for scale_factor in [1.0_f64, 1.25, 1.5, 1.75, 2.0, 2.25, 3.0] {
                    let canvas = (width, height);
                    let atlas_floor = aurora_gpu::TileResidency::min_zoom_for_viewport(canvas);
                    let min_zoom = canvas_min_zoom(canvas, scale_factor);
                    let round_trip = effective_residency_zoom(min_zoom, scale_factor);
                    assert!(
                        round_trip >= atlas_floor,
                        "canvas {canvas:?} at scale factor {scale_factor}: the \
                         view's floor {min_zoom} reaches the atlas as \
                         {round_trip}, below its own floor {atlas_floor} -- the \
                         last ulp where render and paint can still disagree"
                    );
                    if min_zoom > atlas_floor / guarded_scale_factor(scale_factor) {
                        corrected += 1;
                    }
                }
            }
        }
        assert!(
            corrected > 0,
            "no combination in this sweep needed the ulp correction, so this test \
             is not exercising the loop it exists to cover"
        );
    }

    fn laid_out_workspace() -> aurora_ui::Workspace {
        let mut workspace = aurora_ui::build_workspace();
        workspace.tree.compute_layout(1000.0, 800.0);
        workspace
    }

    #[test]
    fn pointer_in_canvas_reports_a_canvas_relative_point_when_inside() {
        let workspace = laid_out_workspace();
        // A 1000x800 viewport, a 250px-wide rail -- canvas area is the
        // 750x800 rect at the window's own origin (see
        // `aurora_ui::workspace`'s own layout test).
        assert_eq!(
            pointer_in_canvas(&workspace, (100.0, 50.0)),
            Some((100.0, 50.0))
        );
    }

    #[test]
    fn pointer_in_canvas_returns_none_over_the_rail() {
        let workspace = laid_out_workspace();
        assert_eq!(pointer_in_canvas(&workspace, (900.0, 50.0)), None);
    }

    #[test]
    fn pointer_on_rail_divider_is_true_right_at_the_canvas_rail_boundary() {
        let workspace = laid_out_workspace();
        // The boundary sits at x=750 (see the comment above) -- the
        // zero-width divider's own bounds.x.
        assert!(pointer_on_rail_divider(&workspace, (750.0, 50.0)));
        assert!(pointer_on_rail_divider(
            &workspace,
            (750.0 - RAIL_DIVIDER_HIT_TOLERANCE, 50.0)
        ));
        assert!(pointer_on_rail_divider(
            &workspace,
            (750.0 + RAIL_DIVIDER_HIT_TOLERANCE, 50.0)
        ));
    }

    #[test]
    fn pointer_on_rail_divider_is_false_away_from_the_boundary() {
        let workspace = laid_out_workspace();
        assert!(!pointer_on_rail_divider(&workspace, (100.0, 50.0)));
        assert!(!pointer_on_rail_divider(&workspace, (900.0, 50.0)));
        assert!(!pointer_on_rail_divider(&workspace, (750.0, 50.0 + 800.0)));
    }

    #[test]
    // Plain subtraction of clean literal values, not accumulated float
    // noise -- the same precedent `aurora_color`'s own round-trip tests
    // already allow this lint for.
    #[allow(clippy::float_cmp)]
    fn resized_rail_width_shrinks_when_the_pointer_moves_right() {
        let resize = RailResize {
            start_pointer_x: 750.0,
            start_width: 250.0,
        };
        assert_eq!(resized_rail_width(resize, 800.0), 200.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn resized_rail_width_grows_when_the_pointer_moves_left() {
        let resize = RailResize {
            start_pointer_x: 750.0,
            start_width: 250.0,
        };
        assert_eq!(resized_rail_width(resize, 700.0), 300.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn resized_rail_width_is_unchanged_at_the_starting_pointer_position() {
        let resize = RailResize {
            start_pointer_x: 750.0,
            start_width: 250.0,
        };
        assert_eq!(resized_rail_width(resize, 750.0), 250.0);
    }

    #[test]
    fn pointer_in_canvas_returns_none_before_any_layout_has_run() {
        let workspace = aurora_ui::build_workspace();
        assert_eq!(pointer_in_canvas(&workspace, (10.0, 10.0)), None);
    }

    #[test]
    fn canvas_area_physical_rect_scales_by_the_dpi_factor() {
        let workspace = laid_out_workspace();
        // Logical canvas area is (0, 0, 750, 800) (see
        // `pointer_in_canvas_reports_a_canvas_relative_point_when_inside`'s
        // own comment); at a 2x scale factor, physical is double that.
        assert_eq!(
            canvas_area_physical_rect(&workspace, 2.0),
            Some((0.0, 0.0, 1500.0, 1600.0))
        );
    }

    #[test]
    fn canvas_area_physical_rect_is_zero_sized_before_any_layout_has_run() {
        // A real finding, not assumed: `WidgetTree::bounds` returns a
        // widget's current (zero, by default) bounds unconditionally
        // once it exists, not `None`, before the first `compute_layout`
        // -- `canvas_area_physical_rect`'s own `Option` only covers a
        // genuinely unknown widget id, which `canvas_area` never is.
        let workspace = aurora_ui::build_workspace();
        assert_eq!(
            canvas_area_physical_rect(&workspace, 1.0),
            Some((0.0, 0.0, 0.0, 0.0))
        );
    }

    #[test]
    fn canvas_area_physical_size_rounds_to_whole_pixels() {
        let workspace = laid_out_workspace();
        assert_eq!(canvas_area_physical_size(&workspace, 1.0), Some((750, 800)));
    }

    #[test]
    fn canvas_local_origin_is_zero_zero_with_no_pan() {
        let view = CanvasView::new();
        assert_eq!(canvas_local_origin(&view, (0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    fn canvas_local_origin_follows_a_positive_pan() {
        // Panning the view so document (0, 0) renders 300 logical px to
        // the right/down of the canvas area's own top-left corner means
        // the point now at that corner is document (-300, -300) -- a
        // real, negative surface-local position. Previously this
        // function floored *and* clamped its own result to `TileId {0,
        // 0}` before returning, silently discarding that this position
        // is actually off the top-left of the document by a full 300px;
        // clamping is now `TileResidency::set_origin`'s own job, so this
        // continuous function returns the true (negative) value.
        let mut view = CanvasView::new();
        view.pan_by((300.0, 300.0));
        assert_eq!(canvas_local_origin(&view, (0.0, 0.0)), (-300.0, -300.0));
    }

    #[test]
    fn canvas_local_origin_follows_a_negative_pan() {
        // Panning left/up by 300px (more than one tile's worth, 256px)
        // means the canvas area's own top-left corner now shows document
        // (300, 300) -- a genuinely sub-tile-fractional position (300 is
        // not a multiple of 256). The old, floored version of this
        // function returned `TileId { x: 1, y: 1 }` here, which was only
        // the *whole-tile* part of the true answer -- the 44px
        // remainder within that tile was silently discarded, exactly the
        // bug this round fixes. The continuous function returns the true
        // (300.0, 300.0); `TileResidency::set_origin` is now the one that
        // splits this into `TileId { 1, 1 }` plus `sub_tile (44.0, 44.0)`.
        let mut view = CanvasView::new();
        view.pan_by((-300.0, -300.0));
        assert_eq!(canvas_local_origin(&view, (0.0, 0.0)), (300.0, 300.0));
    }

    #[test]
    fn canvas_local_origin_accounts_for_zoom_not_just_pan() {
        // The same 600px pan means a different document-space top-left
        // depending on zoom (`to_document` divides by `zoom`) -- at 2x
        // zoom, 600 screen px is only 300 document px. Same fractional
        // point as `canvas_local_origin_follows_a_negative_pan` above
        // (300.0, 300.0), not the whole-tile-only (1, 1) the old floored
        // version returned -- another real fractional case this fix
        // corrects, not just a coincidentally-exact one.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 2.0);
        view.pan_by((-600.0, -600.0));
        assert_eq!(canvas_local_origin(&view, (0.0, 0.0)), (300.0, 300.0));
    }

    #[test]
    fn canvas_local_origin_accounts_for_a_moved_layers_own_origin() {
        // No pan/zoom at all, but the active layer itself sits at
        // document (300, 300) -- the canvas area's own top-left corner
        // (document (0, 0)) is now *before* the layer even starts, in
        // surface-local space (-300, -300) -- the true negative value,
        // not the old version's clamped-to-`TileId{0,0}` result.
        let view = CanvasView::new();
        assert_eq!(canvas_local_origin(&view, (300.0, 300.0)), (-300.0, -300.0));

        // A layer at (300, 300), *plus* enough pan to put document
        // (600, 600) at the canvas area's own top-left corner:
        // surface-local (600 - 300, 600 - 300) = (300, 300) -- proving
        // the layer's own origin and the view's own pan combine, neither
        // alone.
        let mut panned = CanvasView::new();
        panned.pan_by((-600.0, -600.0));
        assert_eq!(canvas_local_origin(&panned, (300.0, 300.0)), (300.0, 300.0));
    }

    #[test]
    // A `LineDelta`'s own `y` is returned unchanged, not computed --
    // exact equality is correct here.
    #[allow(clippy::float_cmp)]
    fn zoom_steps_for_scroll_reads_the_y_axis_of_a_line_delta() {
        assert_eq!(
            zoom_steps_for_scroll(winit::event::MouseScrollDelta::LineDelta(0.0, 2.0)),
            2.0
        );
    }

    #[test]
    fn begin_drag_with_the_middle_button_always_pans_regardless_of_tool() {
        let view = CanvasView::new();
        for tool in Tool::ALL {
            match begin_drag(tool, PointerButton::Middle, (10.0, 20.0), &view, None) {
                Some(Drag::Pan { last_screen }) => assert_eq!(last_screen, (10.0, 20.0)),
                other => {
                    unreachable!("tool {tool:?} must still pan on a middle-button drag: {other:?}")
                }
            }
        }
    }

    #[test]
    fn begin_drag_with_pan_tool_and_primary_button_pans() {
        let view = CanvasView::new();
        match begin_drag(Tool::Pan, PointerButton::Primary, (5.0, 5.0), &view, None) {
            Some(Drag::Pan { last_screen }) => assert_eq!(last_screen, (5.0, 5.0)),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn begin_drag_with_marquee_tool_and_primary_button_starts_a_marquee_in_document_space() {
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 2.0);
        match begin_drag(
            Tool::MarqueeSelect,
            PointerButton::Primary,
            (20.0, 40.0),
            &view,
            None,
        ) {
            Some(Drag::Marquee { start_doc }) => assert_eq!(start_doc, (10.0, 20.0)),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn begin_drag_with_eyedropper_tool_and_primary_button_starts_an_eyedropper_drag() {
        let view = CanvasView::new();
        let drag = begin_drag(
            Tool::Eyedropper,
            PointerButton::Primary,
            (1.0, 1.0),
            &view,
            None,
        );
        assert!(matches!(drag, Some(Drag::Eyedropper)), "{drag:?}");
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn begin_drag_with_brush_tool_and_primary_button_starts_a_brush_drag_at_zero_carry() {
        let view = CanvasView::new();
        match begin_drag(
            Tool::Brush,
            PointerButton::Primary,
            (10.0, 20.0),
            &view,
            None,
        ) {
            Some(Drag::Brush {
                last_doc,
                carry,
                stroke,
                warned,
            }) => {
                assert_eq!(last_doc, (10.0, 20.0));
                assert_eq!(carry, 0.0);
                assert!(
                    stroke.is_none(),
                    "no active pixel layer means nothing to snapshot"
                );
                assert!(
                    warned.is_empty(),
                    "a fresh stroke has warned about nothing yet"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn begin_drag_with_eraser_tool_and_primary_button_starts_an_eraser_drag_at_zero_carry() {
        let view = CanvasView::new();
        match begin_drag(
            Tool::Eraser,
            PointerButton::Primary,
            (10.0, 20.0),
            &view,
            None,
        ) {
            Some(Drag::Eraser {
                last_doc,
                carry,
                stroke,
                warned,
            }) => {
                assert_eq!(last_doc, (10.0, 20.0));
                assert_eq!(carry, 0.0);
                assert!(
                    stroke.is_none(),
                    "no active pixel layer means nothing to snapshot"
                );
                assert!(
                    warned.is_empty(),
                    "a fresh stroke has warned about nothing yet"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    /// A `Drag::Brush` for a real active pixel layer, and the two
    /// accessors that pull its snapshot out. Headless by construction:
    /// `Drag` is a plain enum, so none of this needs an `App` (and
    /// therefore no GPU adapter) to exercise.
    fn brush_drag_with_a_stroke() -> Drag {
        Drag::Brush {
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: Some(aurora_brush::StrokeSnapshot::new(
                aurora_tile::SurfaceId::from_raw(7),
            )),
            warned: std::collections::HashSet::new(),
        }
    }

    fn eraser_drag_with_a_stroke() -> Drag {
        Drag::Eraser {
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: Some(aurora_brush::StrokeSnapshot::new(
                aurora_tile::SurfaceId::from_raw(7),
            )),
            warned: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn brush_stroke_mut_finds_an_active_brush_strokes_own_snapshot() {
        let mut drag = Some(brush_drag_with_a_stroke());
        let Some(stroke) = brush_stroke_mut(&mut drag) else {
            unreachable!("a Drag::Brush carrying a real stroke must yield it");
        };
        assert_eq!(stroke.surface(), aurora_tile::SurfaceId::from_raw(7));
        assert!(
            brush_stroke_mut(&mut Some(Drag::Brush {
                last_doc: (0.0, 0.0),
                carry: 0.0,
                stroke: None,
                warned: std::collections::HashSet::new(),
            }))
            .is_none(),
            "a brush drag that began with no active pixel layer has no snapshot to find"
        );
    }

    #[test]
    fn the_two_stroke_accessors_never_cross_tools() {
        let mut eraser = Some(eraser_drag_with_a_stroke());
        assert!(
            brush_stroke_mut(&mut eraser).is_none(),
            "an eraser stroke must never be handed to `stamp_dab` -- it would capture the \
             right tiles for the wrong operation"
        );
        assert!(eraser_stroke_mut(&mut eraser).is_some());

        let mut brush = Some(brush_drag_with_a_stroke());
        assert!(
            eraser_stroke_mut(&mut brush).is_none(),
            "and the mirror of the same"
        );
        assert!(brush_stroke_mut(&mut brush).is_some());
    }

    #[test]
    fn neither_stroke_accessor_finds_anything_in_another_drag_or_in_no_drag() {
        let mut pan = Some(Drag::Pan {
            last_screen: (0.0, 0.0),
        });
        assert!(brush_stroke_mut(&mut pan).is_none());
        assert!(eraser_stroke_mut(&mut pan).is_none());

        let mut none: Option<Drag> = None;
        assert!(brush_stroke_mut(&mut none).is_none());
        assert!(eraser_stroke_mut(&mut none).is_none());
    }

    /// One permanently broken tile must cost one warning for the whole
    /// stroke, not one per dab. 0.55.0 collapsed per-tile to per-dab,
    /// which is not where the flood is: the tile fails *every* dab for
    /// the rest of the drag, so a ~600 px drag across it emitted ~100
    /// identical lines.
    #[test]
    fn a_broken_tile_is_reported_once_per_stroke_not_once_per_dab() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        // Budget 2, three tiles touched: the dab's own tile (0, 0) is the
        // LRU victim the third touch evicts, so exactly one scratch file
        // exists to corrupt.
        let Some(budget) = std::num::NonZeroUsize::new(2) else {
            unreachable!("2 is non-zero");
        };
        let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        let surface = aurora_tile::SurfaceId::from_raw(0);
        for tile in [
            aurora_tile::TileId { x: 0, y: 0 },
            aurora_tile::TileId { x: 1, y: 0 },
            aurora_tile::TileId { x: 50, y: 50 },
        ] {
            if let Err(err) = store.get_mut(surface, tile) {
                unreachable!("a fresh store must accept a first touch of {tile:?}: {err:?}");
            }
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        let files: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        let [victim] = files.as_slice() else {
            unreachable!("exactly one tile should have been evicted: {files:?}");
        };
        let Ok(bytes) = std::fs::read(victim) else {
            unreachable!("the evicted tile file must be readable");
        };
        let Some(truncated) = bytes.get(..bytes.len() / 2) else {
            unreachable!("half of a slice's own length is always in range");
        };
        if let Err(err) = std::fs::write(victim, truncated) {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }

        let mut drag = Some(brush_drag_with_a_stroke());
        // Ten dabs, all landing on the same permanently broken tile --
        // the shape of a drag crossing one.
        let mut reported = 0;
        for _ in 0..10 {
            let outcome = aurora_brush::stamp_dab(
                &mut store,
                surface,
                (128.0, 128.0),
                BRUSH_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            );
            assert_eq!(
                outcome.failed().len(),
                1,
                "setup: the tile really is broken"
            );
            reported += unwarned_failures(&mut drag, &outcome).len();
        }
        assert_eq!(
            reported, 1,
            "one broken tile must produce exactly one warning across the whole stroke"
        );

        // A *new* stroke starts from a clean slate -- the set lives on
        // the drag, so it cannot outlive the stroke it belongs to.
        let mut next = Some(brush_drag_with_a_stroke());
        let outcome = aurora_brush::stamp_dab(
            &mut store,
            surface,
            (128.0, 128.0),
            BRUSH_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );
        assert_eq!(
            unwarned_failures(&mut next, &outcome).len(),
            1,
            "a new stroke must report the failure again rather than inheriting a stale set"
        );
    }

    /// A scratch store in which `broken` are all permanently unreadable
    /// and every other tile is fine — the only portable way to make a
    /// `TileStore` read fail on demand from outside `aurora-tile`, in
    /// the multi-tile form `a_dab_failing_on_several_tiles...` needs.
    ///
    /// Budget `broken.len()`, so touching one filler tile per broken
    /// tile after them evicts exactly the broken ones and leaves exactly
    /// that many scratch files to truncate.
    fn store_with_broken_tiles(
        broken: &[aurora_tile::TileId],
    ) -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(broken.len()) else {
            unreachable!("at least one tile must be asked for");
        };
        let mut store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => {
                unreachable!("scratch dir just created by tempfile must be usable: {err:?}")
            }
        };
        let surface = aurora_tile::SurfaceId::from_raw(0);
        for tile in broken {
            if let Err(err) = store.get_mut(surface, *tile) {
                unreachable!("a fresh store must accept a first touch of {tile:?}: {err:?}");
            }
        }
        // One filler per broken tile, far away, so each broken tile is
        // in turn the LRU victim and gets written out.
        for (n, _) in broken.iter().enumerate() {
            let filler = aurora_tile::TileId {
                x: 50 + u32::try_from(n).unwrap_or(0),
                y: 50,
            };
            if let Err(err) = store.get_mut(surface, filler) {
                unreachable!("a fresh store must accept a first touch of {filler:?}: {err:?}");
            }
        }
        if let Err(err) = store.flush() {
            unreachable!("test-local scratch disk must accept the write: {err:?}");
        }
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            unreachable!("the scratch directory must be readable");
        };
        let files: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        assert_eq!(
            files.len(),
            broken.len(),
            "exactly the broken tiles should have been evicted: {files:?}"
        );
        for victim in &files {
            let Ok(bytes) = std::fs::read(victim) else {
                unreachable!("the evicted tile file must be readable");
            };
            let Some(truncated) = bytes.get(..bytes.len() / 2) else {
                unreachable!("half of a slice's own length is always in range");
            };
            if let Err(err) = std::fs::write(victim, truncated) {
                unreachable!("test-local scratch disk must accept the write: {err:?}");
            }
        }
        (dir, store)
    }

    /// **Every** fresh failure has to reach the caller, not just the
    /// first (0.57.0). `unwarned_failures` marks every failing tile
    /// warned as it goes, so a caller that logged only `fresh.first()`
    /// — which both `App::paint_dab` and `App::erase_dab` did —
    /// permanently swallowed tiles #2..#n: marked as already reported,
    /// never actually printed, this stroke or ever. A radius-24 dab
    /// spans up to four tiles, so a failing scratch directory hits that
    /// case immediately, and the doc comment on `unwarned_failures`
    /// explicitly promises the opposite ("the first failure on each tile
    /// is always reported").
    #[test]
    fn a_dab_failing_on_several_tiles_reports_every_one_of_them_exactly_once() {
        let first = aurora_tile::TileId { x: 0, y: 0 };
        let second = aurora_tile::TileId { x: 1, y: 0 };
        let (_dir, mut store) = store_with_broken_tiles(&[first, second]);
        let surface = aurora_tile::SurfaceId::from_raw(0);
        // Centred on the boundary between them, so one dab spans both.
        let spanning = (256.5, 128.5);

        let mut drag = Some(brush_drag_with_a_stroke());
        let outcome = aurora_brush::stamp_dab(
            &mut store,
            surface,
            spanning,
            BRUSH_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );
        assert_eq!(
            outcome.failed().len(),
            2,
            "setup: one dab, two permanently broken tiles"
        );

        let fresh = unwarned_failures(&mut drag, &outcome);
        let mut reported: Vec<aurora_tile::TileId> = fresh.iter().map(|(tile, _)| *tile).collect();
        reported.sort_unstable_by_key(|tile| (tile.y, tile.x));
        assert_eq!(
            reported,
            [first, second],
            "both broken tiles must be handed to the caller, each with its own error -- the \
             call sites log one line per entry, so this is exactly what gets printed"
        );

        // And the once-per-stroke dedupe still holds for both of them.
        let again = aurora_brush::stamp_dab(
            &mut store,
            surface,
            spanning,
            BRUSH_RADIUS,
            [1.0, 0.0, 0.0],
            None,
        );
        assert_eq!(again.failed().len(), 2, "setup: still broken");
        assert!(
            unwarned_failures(&mut drag, &again).is_empty(),
            "a second dab across the same two tiles must report nothing further"
        );
    }

    /// The surface `brush_drag_with_a_stroke`/`eraser_drag_with_a_stroke`
    /// already build their snapshots for — the one every drag-commit
    /// test below paints into.
    fn commit_test_surface() -> aurora_tile::SurfaceId {
        aurora_tile::SurfaceId::from_raw(7)
    }

    fn commit_test_store() -> (tempfile::TempDir, aurora_tile::TileStore) {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err:?}"),
        };
        let Some(budget) = std::num::NonZeroUsize::new(16) else {
            unreachable!("16 is non-zero");
        };
        let store = match aurora_tile::TileStore::new(dir.path().to_path_buf(), budget) {
            Ok(store) => store,
            Err(err) => unreachable!("scratch dir just created by tempfile must work: {err:?}"),
        };
        (dir, store)
    }

    /// Tile-local texel `(lx, ly)`'s alpha in tile (0, 0) — how the
    /// drag-commit tests tell "this stroke's pixels" apart from "some
    /// other stroke's pixels" rather than merely counting undo entries.
    fn commit_test_alpha(store: &mut aurora_tile::TileStore, lx: u32, ly: u32) -> f32 {
        let tile = match store.get(commit_test_surface(), aurora_tile::TileId { x: 0, y: 0 }) {
            Ok(tile) => tile,
            Err(err) => unreachable!("{err:?}"),
        };
        let index = (ly * aurora_tile::TILE + lx) as usize * aurora_tile::CHANNELS;
        let Some(&alpha) = tile.texels().get(index + 3) else {
            unreachable!("index is in bounds for a full tile");
        };
        alpha.to_f32()
    }

    /// One real dab painted through a fresh brush drag — the state `App`
    /// holds mid-stroke, with pixels genuinely on the layer and the
    /// pre-dab content genuinely captured in the drag's own snapshot.
    fn a_brush_drag_that_painted(store: &mut aurora_tile::TileStore, centre: (f32, f32)) -> Drag {
        let mut drag = Some(brush_drag_with_a_stroke());
        let outcome = aurora_brush::stamp_dab(
            store,
            commit_test_surface(),
            centre,
            BRUSH_RADIUS,
            [1.0, 0.0, 0.0],
            brush_stroke_mut(&mut drag),
        );
        assert!(
            !outcome.painted().is_empty(),
            "setup: the dab must really have painted"
        );
        match drag {
            Some(drag) => drag,
            None => unreachable!("just built"),
        }
    }

    /// `a_brush_drag_that_painted`'s eraser counterpart, over pixels it
    /// paints first (erasing transparent pixels is a documented no-op).
    fn an_eraser_drag_that_erased(store: &mut aurora_tile::TileStore, centre: (f32, f32)) -> Drag {
        assert!(
            !aurora_brush::stamp_dab(
                store,
                commit_test_surface(),
                centre,
                BRUSH_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            )
            .painted()
            .is_empty(),
            "setup: there must be paint to erase"
        );
        let mut drag = Some(eraser_drag_with_a_stroke());
        let outcome = aurora_brush::erase_dab(
            store,
            commit_test_surface(),
            centre,
            ERASER_RADIUS,
            eraser_stroke_mut(&mut drag),
        );
        assert!(
            !outcome.painted().is_empty(),
            "setup: the erase must really have erased"
        );
        match drag {
            Some(drag) => drag,
            None => unreachable!("just built"),
        }
    }

    /// **RT-08.** Press the middle button to pan without releasing the
    /// brush first — an ordinary gesture — and until 0.57.0 the live
    /// `Drag::Brush` was simply overwritten. Its pixels stayed on the
    /// layer and no undo entry was ever pushed for them, so the next
    /// `Ctrl+Z` reached past them and undid the *previous* stroke:
    /// worse than the phantom entry 0.56.0 removed, because a phantom
    /// entry at least did nothing.
    ///
    /// The load-bearing assertion is the last pair: the entry that
    /// exists has to be the interrupted stroke's own, not just *an*
    /// entry.
    #[test]
    fn a_stroke_interrupted_by_a_second_press_becomes_its_own_undo_entry() {
        let (_dir, mut store) = commit_test_store();
        let layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        // An earlier, properly released stroke -- the one a mis-targeted
        // Ctrl+Z would reach into.
        let earlier = a_brush_drag_that_painted(&mut store, (30.5, 30.5));
        commit_ending_drag(
            Some(earlier),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );

        // The stroke still in progress when the middle button goes down.
        let mut drag = Some(a_brush_drag_that_painted(&mut store, (200.5, 200.5)));
        assert!(commit_test_alpha(&mut store, 30, 30) > 0.5, "setup");
        assert!(commit_test_alpha(&mut store, 200, 200) > 0.5, "setup");

        // Exactly the sequence `App::handle_pointer_pressed` now runs:
        // take the in-progress drag, commit it, then start the new one.
        let interrupted = drag.take();
        commit_ending_drag(
            interrupted,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );
        drag = begin_drag(
            Tool::Brush,
            PointerButton::Middle,
            (1.0, 1.0),
            &CanvasView::new(),
            None,
        );
        assert!(
            matches!(drag, Some(Drag::Pan { .. })),
            "setup: the interrupting press really does start its own drag"
        );

        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Pixel, UndoKind::Pixel],
            "the interrupted stroke must have its own entry in the unified order"
        );
        match pixel_history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(
            commit_test_alpha(&mut store, 200, 200) < 0.01,
            "Ctrl+Z must remove the interrupted stroke's own pixels"
        );
        assert!(
            commit_test_alpha(&mut store, 30, 30) > 0.5,
            "and must not have reached past them into the earlier stroke"
        );
    }

    /// **RT-02 (0.57.7).** The Zoom tool's own click branch in
    /// [`App::handle_pointer_pressed`] `return`ed before ever reaching
    /// that same shared commit — and it *also* moves the view
    /// ([`handle_zoom_tool_click`] clamps the pan) out from under
    /// whatever drag was still live. That is the identical pair of bugs
    /// [`press_layer_row`] was written to fix, one branch further down
    /// the same function, and a live `Drag::Brush` really does reach
    /// it: `z` switches to the Zoom tool mid-stroke without ending the
    /// stroke.
    ///
    /// The branch now runs the commit first, exactly this sequence.
    #[test]
    fn a_stroke_interrupted_by_a_zoom_tool_click_still_becomes_its_own_undo_entry() {
        let (_dir, mut store) = commit_test_store();
        let layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();
        let mut view = CanvasView::new();

        // An earlier, properly released stroke -- the one a mis-targeted
        // Ctrl+Z would reach into.
        let earlier = a_brush_drag_that_painted(&mut store, (30.5, 30.5));
        commit_ending_drag(
            Some(earlier),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut view,
            None,
        );
        let mut drag = Some(a_brush_drag_that_painted(&mut store, (200.5, 200.5)));

        // Exactly the sequence that branch now runs.
        let interrupted = drag.take();
        commit_ending_drag(
            interrupted,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut view,
            None,
        );
        handle_zoom_tool_click(&mut view, (100.0, 100.0), Modifiers::none(), (0.0, 0.0));

        assert!(
            drag.is_none(),
            "no drag may still be live once the zoom has clamped the view under it"
        );
        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Pixel, UndoKind::Pixel],
            "the interrupted stroke must have its own entry in the unified order"
        );
        match pixel_history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(
            commit_test_alpha(&mut store, 200, 200) < 0.01,
            "Ctrl+Z must remove the interrupted stroke's own pixels"
        );
        assert!(
            commit_test_alpha(&mut store, 30, 30) > 0.5,
            "and must not have reached past them into the earlier stroke"
        );
    }

    /// The same, for dragging the cursor off the window edge
    /// (`WindowEvent::CursorLeft`), which used to do `self.drag = None`
    /// outright. An eraser drag here, so both stroke-carrying variants
    /// are covered across the two tests.
    #[test]
    fn a_stroke_interrupted_by_the_cursor_leaving_becomes_its_own_undo_entry() {
        let (_dir, mut store) = commit_test_store();
        let layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        // Paint something elsewhere first and leave it alone: whatever
        // the undo does, it must not touch this.
        assert!(
            !aurora_brush::stamp_dab(
                &mut store,
                commit_test_surface(),
                (30.5, 30.5),
                BRUSH_RADIUS,
                [1.0, 0.0, 0.0],
                None,
            )
            .painted()
            .is_empty(),
            "setup"
        );

        let mut drag = Some(an_eraser_drag_that_erased(&mut store, (200.5, 200.5)));
        assert!(
            commit_test_alpha(&mut store, 200, 200) < 0.01,
            "setup: really erased"
        );

        // Exactly the sequence the `CursorLeft` arm now runs.
        let interrupted = drag.take();
        commit_ending_drag(
            interrupted,
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );
        assert!(drag.is_none(), "setup: the drag really is gone");

        assert_eq!(undo_order.undo, vec![UndoKind::Pixel]);
        match pixel_history.undo(&mut store) {
            Ok(true) => {}
            other => unreachable!("expected Ok(true), got {other:?}"),
        }
        assert!(
            commit_test_alpha(&mut store, 200, 200) > 0.5,
            "Ctrl+Z must put the interrupted eraser stroke's own pixels back"
        );
        assert!(
            commit_test_alpha(&mut store, 30, 30) > 0.5,
            "and must have left the untouched paint alone"
        );
    }

    /// The ordinary path has to behave exactly as it always did — one
    /// entry per released stroke, nothing at all for a drag with no
    /// stroke, a drag that painted nothing, a non-painting drag, or no
    /// drag — and `Drag::Move` must still coalesce into one structural
    /// entry now that `finish_move` is reached through the same shared
    /// commit rather than from `handle_pointer_released` directly.
    // Four commits, each now carrying the view and active layer
    // `commit_ending_drag` re-establishes the pan bound against (0.57.6)
    // -- eight lines of parameters past the 100-line lint, with the
    // assertions themselves unchanged. Splitting the four cases apart
    // would lose the "one released drag of each kind, in sequence"
    // shape that is the point of the test.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn the_ordinary_release_path_commits_exactly_what_it_always_did() {
        let (_dir, mut store) = commit_test_store();
        let mut layers = aurora_doc::LayerTree::new();
        let mut history = aurora_doc::History::new();
        let mut pixel_history = aurora_brush::PixelHistory::new();
        let mut undo_order = UndoOrder::default();

        // (a) A released stroke: one entry, and it undoes its own paint.
        let released = a_brush_drag_that_painted(&mut store, (30.5, 30.5));
        commit_ending_drag(
            Some(released),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );
        assert_eq!(undo_order.undo, vec![UndoKind::Pixel]);
        assert!(pixel_history.can_undo());

        // (b) Nothing to commit: a brush drag that began with no active
        // pixel layer, a brush drag whose snapshot is empty, a pan, and
        // no drag at all.
        for nothing in [
            Some(Drag::Brush {
                last_doc: (0.0, 0.0),
                carry: 0.0,
                stroke: None,
                warned: std::collections::HashSet::new(),
            }),
            Some(brush_drag_with_a_stroke()),
            Some(Drag::Pan {
                last_screen: (0.0, 0.0),
            }),
            Some(Drag::Eyedropper),
            None,
        ] {
            commit_ending_drag(
                nothing,
                &layers,
                &mut history,
                &mut pixel_history,
                &mut undo_order,
                &mut CanvasView::new(),
                None,
            );
            assert_eq!(
                undo_order.undo,
                vec![UndoKind::Pixel],
                "nothing here changed a pixel, so nothing may become an undo step"
            );
        }

        // (c) A move still coalesces into exactly one structural entry.
        let bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let layer = match history.add_pixel_layer(&mut layers, "a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        let moved = aurora_core::Rect {
            x: 5,
            y: 5,
            ..bounds
        };
        if let Err(err) = layers.set_bounds(layer, moved) {
            unreachable!("{err:?}");
        }
        commit_ending_drag(
            Some(Drag::Move {
                layer_id: layer,
                start_doc: (0.0, 0.0),
                start_bounds: bounds,
                current_bounds: moved,
            }),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );
        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Pixel, UndoKind::Structural],
            "a completed move must still record one structural step"
        );

        // (d) A move that ended where it started records nothing.
        commit_ending_drag(
            Some(Drag::Move {
                layer_id: layer,
                start_doc: (0.0, 0.0),
                start_bounds: moved,
                current_bounds: moved,
            }),
            &layers,
            &mut history,
            &mut pixel_history,
            &mut undo_order,
            &mut CanvasView::new(),
            None,
        );
        assert_eq!(
            undo_order.undo,
            vec![UndoKind::Pixel, UndoKind::Structural],
            "a move that ended where it started has nothing to reverse"
        );
    }

    #[test]
    fn begin_drag_with_zoom_tool_and_secondary_button_does_nothing() {
        let view = CanvasView::new();
        assert!(
            begin_drag(
                Tool::Zoom,
                PointerButton::Secondary,
                (1.0, 1.0),
                &view,
                None
            )
            .is_none()
        );
    }

    #[test]
    fn begin_drag_with_move_tool_and_no_active_pixel_layer_does_nothing() {
        let view = CanvasView::new();
        assert!(begin_drag(Tool::Move, PointerButton::Primary, (1.0, 1.0), &view, None).is_none());
    }

    #[test]
    fn begin_drag_with_move_tool_and_an_active_pixel_layer_starts_a_move_drag() {
        let view = CanvasView::new();
        let mut layers = aurora_doc::LayerTree::new();
        let bounds = aurora_core::Rect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        let id = match layers.add_pixel_layer("a", bounds, None) {
            Ok(id) => id,
            Err(err) => unreachable!("{err:?}"),
        };
        match begin_drag(
            Tool::Move,
            PointerButton::Primary,
            (5.0, 5.0),
            &view,
            Some((id, bounds)),
        ) {
            Some(Drag::Move {
                layer_id,
                start_doc,
                start_bounds,
                current_bounds,
            }) => {
                assert_eq!(layer_id, id);
                assert_eq!(start_doc, (5.0, 5.0));
                assert_eq!(start_bounds, bounds);
                assert_eq!(current_bounds, bounds);
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn continue_drag_pan_moves_the_view_by_the_delta_since_the_last_point() {
        let mut view = CanvasView::new();
        // Pre-panned left/up, away from the document's own top-left
        // edge -- a fresh `CanvasView::new()` already sits exactly at
        // that boundary (`to_document((0, 0)) == (0, 0)`), so a
        // rightward/downward delta from there would immediately hit
        // `clamp_pan_to_minimum` and this test would no longer be
        // exercising plain delta application, which is what it's for.
        view.pan_by((-50.0, -50.0));
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Pan {
            last_screen: (10.0, 10.0),
        };
        let dabs = continue_drag(
            &mut drag,
            (15.0, 8.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        assert_eq!(view.pan(), (-45.0, -52.0));
        match drag {
            Drag::Pan { last_screen } => assert_eq!(
                last_screen,
                (15.0, 8.0),
                "must advance its own last-known point for the next event"
            ),
            other => unreachable!("{other:?}"),
        }
        assert_eq!(dabs, Vec::new(), "Pan must never produce dabs to paint");
    }

    #[test]
    fn continue_drag_marquee_updates_the_active_selection_live() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Marquee {
            start_doc: (10.0, 10.0),
        };
        let dabs = continue_drag(
            &mut drag,
            (30.0, 25.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        let Some(active) = selection.active() else {
            unreachable!("must select something");
        };
        assert_eq!(active.bounds.x, 10);
        assert_eq!(active.bounds.y, 10);
        assert_eq!(active.bounds.width, 20);
        assert_eq!(active.bounds.height, 15);
        assert_eq!(dabs, Vec::new(), "Marquee must never produce dabs to paint");

        // A second move further extends the same drag -- the selection
        // must track the *current* rect, not just the first one.
        let _ = continue_drag(
            &mut drag,
            (50.0, 5.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        let Some(active) = selection.active() else {
            unreachable!("must still be selected");
        };
        assert_eq!(active.bounds.width, 40);
    }

    #[test]
    fn continue_drag_brush_returns_the_new_segments_dabs() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Brush {
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: None,
            warned: std::collections::HashSet::new(),
        };
        // radius 24, DEFAULT_SPACING 0.25 -> step 6; a 12-unit segment
        // lands dabs at 6 and 12 (the segment's own start, 0, is not
        // re-emitted -- it was already painted by whatever started the
        // drag or the previous event).
        let dabs = continue_drag(
            &mut drag,
            (12.0, 0.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        assert_eq!(dabs, vec![(6.0, 0.0), (12.0, 0.0)]);
        match drag {
            Drag::Brush {
                last_doc, carry, ..
            } => {
                assert_eq!(
                    (last_doc, carry),
                    ((12.0, 0.0), 0.0),
                    "must advance its own last-known point and carry for the next event"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn continue_drag_brush_carries_spacing_across_multiple_short_move_events() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Brush {
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: None,
            warned: std::collections::HashSet::new(),
        };
        let first = continue_drag(&mut drag, (3.0, 0.0), &mut view, &mut selection, (0.0, 0.0));
        // Segment shorter than one step (6): no new dab yet, but the 3
        // units already travelled must carry forward, not reset to 0 --
        // the exact bug a fresh `dabs_along_path` call each event would
        // have (see `continue_drag`'s own doc comment).
        assert_eq!(first, Vec::new());
        match drag {
            Drag::Brush {
                last_doc, carry, ..
            } => assert_eq!((last_doc, carry), ((3.0, 0.0), 3.0)),
            other => unreachable!("{other:?}"),
        }

        // Second event: 4 more units. 3 (carried) + 4 = 7 >= step (6),
        // so exactly one dab lands (at the 6-unit mark, i.e. 3 units
        // into *this* segment) -- proving the carry from the first,
        // sub-step event was not lost.
        let second = continue_drag(&mut drag, (7.0, 0.0), &mut view, &mut selection, (0.0, 0.0));
        assert_eq!(second, vec![(6.0, 0.0)]);
    }

    #[test]
    fn continue_drag_eraser_returns_the_new_segments_dabs() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Eraser {
            last_doc: (0.0, 0.0),
            carry: 0.0,
            stroke: None,
            warned: std::collections::HashSet::new(),
        };
        // Same radius/spacing as Brush (ERASER_RADIUS == BRUSH_RADIUS ==
        // 24, DEFAULT_SPACING 0.25 -> step 6), so the same dab positions
        // land for the same segment.
        let dabs = continue_drag(
            &mut drag,
            (12.0, 0.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        assert_eq!(dabs, vec![(6.0, 0.0), (12.0, 0.0)]);
        match drag {
            Drag::Eraser {
                last_doc, carry, ..
            } => {
                assert_eq!(
                    (last_doc, carry),
                    ((12.0, 0.0), 0.0),
                    "must advance its own last-known point and carry for the next event"
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn continue_drag_move_shifts_current_bounds_by_the_pointer_delta_and_returns_no_dabs() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let start_bounds = aurora_core::Rect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        let mut drag = Drag::Move {
            layer_id: aurora_core::Id::from_raw(0),
            start_doc: (0.0, 0.0),
            start_bounds,
            current_bounds: start_bounds,
        };
        let dabs = continue_drag(
            &mut drag,
            (15.0, -8.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        assert_eq!(dabs, Vec::new(), "Move must never produce dabs to paint");
        let Drag::Move { current_bounds, .. } = drag else {
            unreachable!("still a Move drag");
        };
        assert_eq!(
            current_bounds,
            aurora_core::Rect {
                x: 25,
                y: 12,
                width: 100,
                height: 50,
            },
            "must shift start_bounds by the same delta the pointer travelled"
        );
    }

    #[test]
    fn continue_drag_eyedropper_returns_no_dabs_and_updates_nothing() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Eyedropper;
        let dabs = continue_drag(
            &mut drag,
            (15.0, -8.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        assert_eq!(
            dabs,
            Vec::new(),
            "Eyedropper must never produce dabs to paint"
        );
        assert!(
            matches!(drag, Drag::Eyedropper),
            "carries no state to update"
        );
    }

    #[test]
    fn apply_scroll_zoom_zooms_in_on_a_positive_scroll() {
        let mut view = CanvasView::new();
        apply_scroll_zoom(
            &mut view,
            None,
            (100.0, 100.0),
            winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
            (0.0, 0.0),
        );
        assert!(view.zoom() > 1.0, "zoom was {}", view.zoom());
    }

    #[test]
    fn apply_scroll_zoom_zooms_out_on_a_negative_scroll() {
        let mut view = CanvasView::new();
        apply_scroll_zoom(
            &mut view,
            None,
            (100.0, 100.0),
            winit::event::MouseScrollDelta::LineDelta(0.0, -1.0),
            (0.0, 0.0),
        );
        assert!(view.zoom() < 1.0, "zoom was {}", view.zoom());
    }

    #[test]
    // `zoom()` here is `1.0 * ZOOM_CLICK_FACTOR` from a fresh view,
    // computed via one exact multiplication, not accumulated float
    // error -- exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn handle_zoom_tool_click_zooms_in_without_alt() {
        let mut view = CanvasView::new();
        handle_zoom_tool_click(&mut view, (50.0, 50.0), Modifiers::none(), (0.0, 0.0));
        assert_eq!(view.zoom(), 2.0);
    }

    #[test]
    // Same reasoning as `handle_zoom_tool_click_zooms_in_without_alt`
    // above.
    #[allow(clippy::float_cmp)]
    fn handle_zoom_tool_click_zooms_out_with_alt_held() {
        let mut view = CanvasView::new();
        let alt_held = Modifiers {
            alt: true,
            ..Modifiers::none()
        };
        handle_zoom_tool_click(&mut view, (50.0, 50.0), alt_held, (0.0, 0.0));
        assert_eq!(view.zoom(), 0.5);
    }

    // -- Proof that the real reported bug (panning right/down froze the
    // -- rendered canvas while painting silently kept tracking the true,
    // -- unbounded position) is fixed: `continue_drag`/`apply_scroll_zoom`/
    // -- `handle_zoom_tool_click` all now clamp `view`'s own pan via
    // -- `CanvasView::clamp_pan_to_minimum`, so `canvas_local_origin`
    // -- (what `App::redraw` feeds `aurora_gpu::TileResidency::set_origin`,
    // -- i.e. what actually gets rendered) and `view.to_document` (what
    // -- painting/`Eyedropper`/`Marquee` use to place a point) are
    // -- guaranteed to read the same, already-bounded `pan` -- never two
    // -- different, silently-diverged notions of "where the pointer is
    // -- in document space" again.

    #[test]
    fn continue_drag_pan_past_the_document_edge_keeps_render_and_paint_in_agreement() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Pan {
            last_screen: (0.0, 0.0),
        };
        // Drag far past what would have been the old unclamped-negative
        // boundary -- right/down, the exact direction Cahya reported.
        let _ = continue_drag(
            &mut drag,
            (5_000.0, 5_000.0),
            &mut view,
            &mut selection,
            (0.0, 0.0),
        );
        // Render: the continuous position `redraw` feeds
        // `TileResidency::set_origin` must stay pinned at the document's
        // own origin, not just silently stop advancing while something
        // else keeps moving.
        assert_eq!(canvas_local_origin(&view, (0.0, 0.0)), (0.0, 0.0));
        // Paint: the canvas area's own top-left corner must report the
        // *same* document position the render is showing -- (0.0, 0.0),
        // not the large negative value it would have reported before
        // this fix, which is what made painting land away from the
        // cursor once the render had frozen.
        assert_eq!(view.to_document((0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    fn continue_drag_pan_clamps_to_a_moved_layers_own_origin_not_always_zero() {
        // A layer moved to document (300, 300)
        // (`aurora_doc::LayerTree::set_bounds`) must clamp panning to
        // *its* own top-left corner -- the same boundary
        // `canvas_local_origin`'s own `layer_origin` parameter already
        // uses elsewhere for this purpose.
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Pan {
            last_screen: (0.0, 0.0),
        };
        let _ = continue_drag(
            &mut drag,
            (5_000.0, 5_000.0),
            &mut view,
            &mut selection,
            (300.0, 300.0),
        );
        assert_eq!(canvas_local_origin(&view, (300.0, 300.0)), (0.0, 0.0));
        assert_eq!(view.to_document((0.0, 0.0)), (300.0, 300.0));
    }

    #[test]
    // `to_document((0, 0))` after the clamp is `-0.0 / zoom`, which is
    // exactly `0.0` whatever `zoom` itself landed on -- exact equality
    // is correct here, not a float rounding risk.
    #[allow(clippy::float_cmp)]
    fn apply_scroll_zoom_never_scrolls_the_view_past_the_document_edge() {
        // Scroll-wheel zoom recomputes `pan` from scratch to keep the
        // anchor fixed on screen -- zooming *out* while anchored away
        // from the top-left corner pushes `pan` the same direction a
        // drag-pan does, so it needs the same clamp. Anchoring at (100,
        // 100) and zooming out (negative scroll) is exactly that case:
        // without the clamp, `to_document((0, 0))` would land at
        // (-100, -100) or worse.
        let mut view = CanvasView::new();
        apply_scroll_zoom(
            &mut view,
            None,
            (100.0, 100.0),
            winit::event::MouseScrollDelta::LineDelta(0.0, -10.0),
            (0.0, 0.0),
        );
        assert_eq!(view.to_document((0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    // Same reasoning as
    // `apply_scroll_zoom_never_scrolls_the_view_past_the_document_edge`
    // above -- exact equality is correct.
    #[allow(clippy::float_cmp)]
    fn handle_zoom_tool_click_never_scrolls_the_view_past_the_document_edge() {
        // The Zoom tool's own click-to-zoom-out gesture (`Alt`+click)
        // goes through the same `zoom_at` recomputation as scroll-wheel
        // zoom, so it needs the same clamp -- clicking to zoom out
        // anchored at (100, 100) from a fresh view would otherwise land
        // `to_document((0, 0))` at (-100, -100) (see this function's own
        // derivation in `CanvasView::clamp_pan_to_minimum`'s doc
        // comment).
        let mut view = CanvasView::new();
        let alt_held = Modifiers {
            alt: true,
            ..Modifiers::none()
        };
        handle_zoom_tool_click(&mut view, (100.0, 100.0), alt_held, (0.0, 0.0));
        assert_eq!(view.to_document((0.0, 0.0)), (0.0, 0.0));
    }

    // -- a view that moves under a drag that is still live (0.57.7) --
    //
    // RT-02. The clamp above is the whole hazard `press_layer_row` and
    // `perform_undo_redo` end a drag to avoid, reached from a gesture
    // that is *not* "I am done dragging": scrolling to zoom while
    // painting is an ordinary thing to do mid-stroke. So this path
    // re-anchors the drag instead of ending it
    // (`shift_drag_reference`), and these are the two halves of that.

    /// The bug, with the pointer completely still: zooming out hard
    /// against the document's own top-left edge makes the clamp move
    /// the view, and a `Drag::Brush`'s own `last_doc` then names the
    /// pre-clamp document position while `continue_drag` reads the
    /// post-clamp one. Without the re-anchor the next move event
    /// interpolates a whole segment between them.
    #[test]
    fn scroll_zooming_mid_stroke_re_anchors_the_stroke_instead_of_painting_a_line_never_drawn() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let pointer = (40.0, 40.0);
        let mut drag = Drag::Brush {
            last_doc: view.to_document(pointer),
            carry: 0.0,
            stroke: None,
            warned: std::collections::HashSet::new(),
        };

        let before = view.to_document(pointer);
        apply_scroll_zoom(
            &mut view,
            Some(&mut drag),
            pointer,
            winit::event::MouseScrollDelta::LineDelta(0.0, -10.0),
            (0.0, 0.0),
        );
        let after = view.to_document(pointer);
        assert!(
            (after.0 - before.0).abs() > 50.0 && (after.1 - before.1).abs() > 50.0,
            "setup: the clamp really does move the document point under the still pointer: \
             {before:?} -> {after:?}"
        );

        let dabs = continue_drag(&mut drag, pointer, &mut view, &mut selection, (0.0, 0.0));
        assert!(
            dabs.is_empty(),
            "a still pointer must not paint: {} dabs were placed",
            dabs.len()
        );
    }

    /// The same re-anchor for a `Drag::Move`, where a stale `start_doc`
    /// does not paint but does teleport the layer: `current_bounds` is
    /// `start_bounds` plus `current_doc - start_doc`, so a view that
    /// moved under it shifts the layer by the clamp's own jump on the
    /// next event. Also the negative control for the other half — a
    /// zoom that never reaches the bound must leave the reference
    /// exactly alone, since `zoom_at` already holds it valid.
    #[test]
    fn scroll_zooming_mid_move_leaves_the_layer_where_the_pointer_put_it() {
        let start_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let pointer = (40.0, 40.0);
        let a_move = |view: &aurora_ui::CanvasView| Drag::Move {
            layer_id: aurora_core::Id::from_raw(0),
            start_doc: view.to_document(pointer),
            start_bounds,
            current_bounds: start_bounds,
        };

        // (a) A zoom that hits the bound.
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = a_move(&view);
        apply_scroll_zoom(
            &mut view,
            Some(&mut drag),
            pointer,
            winit::event::MouseScrollDelta::LineDelta(0.0, -10.0),
            (0.0, 0.0),
        );
        let _ = continue_drag(&mut drag, pointer, &mut view, &mut selection, (0.0, 0.0));
        let Drag::Move { current_bounds, .. } = drag else {
            unreachable!("still a Move drag");
        };
        assert_eq!(
            current_bounds, start_bounds,
            "a still pointer must not move the layer"
        );

        // (b) A zoom nowhere near it: nothing to correct, and nothing
        // corrected.
        let mut view = CanvasView::new();
        view.pan_by((-400.0, -400.0));
        let mut drag = a_move(&view);
        let Drag::Move {
            start_doc: start_doc_before,
            ..
        } = drag
        else {
            unreachable!("just built a move drag")
        };
        apply_scroll_zoom(
            &mut view,
            Some(&mut drag),
            pointer,
            winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
            (0.0, 0.0),
        );
        let Drag::Move { start_doc, .. } = drag else {
            unreachable!("still a Move drag");
        };
        assert!(
            (start_doc.0 - start_doc_before.0).abs() < 1e-3
                && (start_doc.1 - start_doc_before.1).abs() < 1e-3,
            "a zoom that never reaches the bound must not shift the reference: \
             {start_doc_before:?} -> {start_doc:?}"
        );
    }

    // RT-02c (0.57.8). The same hazard again, at the one kind of path
    // neither `apply_scroll_zoom` nor `commit_ending_drag` covers:
    // `aurora_ui::CanvasView::set_min_zoom`, which reads like a plain
    // bounds setter and *moves the view*. `App::apply_resize` and
    // `App::redraw` both call it on every canvas-size or scale-factor
    // change, neither of which is a "I am done dragging" gesture -- so
    // a window resize while a stroke is held is the trigger, with the
    // pointer completely still.

    /// The bug and the fix over the same still pointer. Part (a) is the
    /// pre-0.57.8 behaviour, spelled by handing
    /// [`apply_canvas_min_zoom`] no drag to re-anchor, and measures what
    /// it costs; part (b) is the same raise with the live drag passed.
    #[test]
    fn resizing_mid_stroke_re_anchors_the_stroke_instead_of_painting_a_line_never_drawn() {
        let floor = canvas_min_zoom((1920, 1080), 1.0);
        let pointer = (400.0, 300.0);
        // A view below the floor is exactly what a freshly opened
        // document plus a zoom-out leaves behind, until the next
        // frame -- or a resize -- re-applies the floor.
        let below_the_floor = || {
            let mut view = CanvasView::new();
            view.zoom_at((0.0, 0.0), 0.25);
            view
        };
        let a_stroke = |view: &CanvasView| Drag::Brush {
            last_doc: view.to_document(pointer),
            carry: 0.0,
            stroke: None,
            warned: std::collections::HashSet::new(),
        };
        let mut selection = SelectionSet::new();

        // (a) No re-anchor.
        let mut view = below_the_floor();
        let mut drag = a_stroke(&view);
        let before = view.to_document(pointer);
        apply_canvas_min_zoom(&mut view, None, Some(pointer), floor);
        let after = view.to_document(pointer);
        assert!(
            (after.0 - before.0).abs() > 100.0 && (after.1 - before.1).abs() > 100.0,
            "setup: raising the floor really does move the document point under \
             a still pointer: {before:?} -> {after:?}"
        );
        let stale = continue_drag(&mut drag, pointer, &mut view, &mut selection, (0.0, 0.0));
        assert!(
            stale.len() > 50,
            "setup: without the re-anchor a still pointer paints a whole line of \
             dabs; got {}",
            stale.len()
        );

        // (b) The fix.
        let mut view = below_the_floor();
        let mut drag = a_stroke(&view);
        apply_canvas_min_zoom(&mut view, Some(&mut drag), Some(pointer), floor);
        let dabs = continue_drag(&mut drag, pointer, &mut view, &mut selection, (0.0, 0.0));
        assert!(
            dabs.is_empty(),
            "a still pointer must not paint: {} dabs were placed",
            dabs.len()
        );
    }

    /// The same re-anchor for a `Drag::Move`, where a stale `start_doc`
    /// does not paint but does teleport the layer — and the negative
    /// control: a floor the view already satisfies moves nothing, so it
    /// must correct nothing.
    #[test]
    fn resizing_mid_move_leaves_the_layer_where_the_pointer_put_it() {
        let floor = canvas_min_zoom((1920, 1080), 1.0);
        let pointer = (400.0, 300.0);
        let start_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        let a_move = |view: &CanvasView| Drag::Move {
            layer_id: aurora_core::Id::from_raw(0),
            start_doc: view.to_document(pointer),
            start_bounds,
            current_bounds: start_bounds,
        };
        let mut selection = SelectionSet::new();

        // (a) A raise that really moves the view.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.25);
        let mut drag = a_move(&view);
        apply_canvas_min_zoom(&mut view, Some(&mut drag), Some(pointer), floor);
        let _ = continue_drag(&mut drag, pointer, &mut view, &mut selection, (0.0, 0.0));
        let Drag::Move { current_bounds, .. } = drag else {
            unreachable!("still a Move drag");
        };
        assert_eq!(
            current_bounds, start_bounds,
            "a still pointer must not move the layer"
        );

        // (b) A view already above the floor: nothing to correct, and
        // nothing corrected.
        let mut view = CanvasView::new();
        let mut drag = a_move(&view);
        let Drag::Move {
            start_doc: start_doc_before,
            ..
        } = drag
        else {
            unreachable!("just built a move drag");
        };
        apply_canvas_min_zoom(&mut view, Some(&mut drag), Some(pointer), floor);
        let Drag::Move { start_doc, .. } = drag else {
            unreachable!("still a Move drag");
        };
        assert!(
            (start_doc.0 - start_doc_before.0).abs() < 1e-3
                && (start_doc.1 - start_doc_before.1).abs() < 1e-3,
            "a floor the view already satisfies must not shift the reference: \
             {start_doc_before:?} -> {start_doc:?}"
        );
    }

    /// **Why the correction is measured at the pointer**, and why
    /// `apply_scroll_zoom`'s own "one uniform shift, measured anywhere"
    /// argument does not transfer to this path (0.57.8). There, a pan
    /// clamp at an already-fixed zoom shifts `to_document(p)` by the
    /// same amount for every `p`. Here the *zoom* changes, and raising
    /// `z0` to `z1` anchored at `(0, 0)` shifts `to_document(x)` by
    /// `x * (1/z1 - 1/z0)` — exactly zero at the anchor, and different
    /// at every other point. Measuring anywhere but the point each
    /// drag's own reference is derived from would correct by the wrong
    /// amount; measuring at the anchor would correct by nothing at all.
    #[test]
    fn raising_the_zoom_floor_shifts_to_document_by_a_different_amount_at_every_point() {
        let floor = canvas_min_zoom((1920, 1080), 1.0);
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 0.25);
        // A non-zero pan too: the identity is not an artefact of the
        // view starting at the origin.
        view.pan_by((-37.0, 19.0));
        let old_zoom = view.zoom();
        let probes = [(0.0, 0.0), (100.0, 40.0), (900.0, 700.0)];
        let before = probes.map(|probe| view.to_document(probe));

        apply_canvas_min_zoom(&mut view, None, None, floor);

        let new_zoom = view.zoom();
        assert!(
            new_zoom > old_zoom,
            "setup: the floor really raises this view's zoom: \
             {old_zoom} -> {new_zoom}"
        );
        let scale = 1.0 / new_zoom - 1.0 / old_zoom;
        for (probe, was) in probes.iter().zip(before.iter()) {
            let now = view.to_document(*probe);
            let shift = (now.0 - was.0, now.1 - was.1);
            let expected = (probe.0 * scale, probe.1 * scale);
            assert!(
                (shift.0 - expected.0).abs() <= expected.0.abs().mul_add(1e-3, 1e-3)
                    && (shift.1 - expected.1).abs() <= expected.1.abs().mul_add(1e-3, 1e-3),
                "at {probe:?} the shift must be x * (1/z1 - 1/z0): expected \
                 {expected:?}, got {shift:?}"
            );
        }
        // The two halves of "not uniform", stated as assertions rather
        // than left to the formula above: nothing moves at the anchor,
        // and something very much does elsewhere.
        let anchor_now = view.to_document((0.0, 0.0));
        let Some(anchor_was) = before.first() else {
            unreachable!("three probes");
        };
        assert!(
            (anchor_now.0 - anchor_was.0).abs() < 1e-3
                && (anchor_now.1 - anchor_was.1).abs() < 1e-3,
            "the (0, 0) anchor is held fixed across the raise: \
             {anchor_was:?} -> {anchor_now:?}"
        );
        let Some(far_was) = before.last() else {
            unreachable!("three probes");
        };
        let far_now = view.to_document((900.0, 700.0));
        assert!(
            (far_now.0 - far_was.0).abs() > 100.0,
            "and a point far from it is not: {far_was:?} -> {far_now:?}"
        );
    }

    /// **The negative control for `continue_drag`'s own `Drag::Move`
    /// arm** (RT-06). `commit_ending_drag`'s doc comment argues that
    /// arm must *not* clamp — the clamp would feed the moved view back
    /// into the next event's delta, which is derived from a fixed
    /// `start_doc` through `to_document`, and the drag would chase
    /// itself. Until now that argument was defended only by prose:
    /// adding the clamp there passed the whole suite.
    ///
    /// So this re-spells the arm both ways over the same pointer path —
    /// the real [`continue_drag`], and a local copy with exactly the
    /// clamp the design bans (against the moving layer's own origin,
    /// which is what `active_layer_origin` reports once
    /// `App::apply_move` has written `current_bounds` back) — and
    /// asserts they disagree. The real one tracks the pointer exactly;
    /// the fed-back one runs away from it.
    #[test]
    fn clamping_inside_the_move_arm_would_feed_back_into_its_own_delta() {
        let start_bounds = aurora_core::Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 50,
        };
        // Panned right/down, so a clamp against a layer origin at or
        // past the document origin really does have something to bite.
        let start_view = || {
            let mut view = CanvasView::new();
            view.pan_by((40.0, 40.0));
            view
        };
        let path = [(60.0, 60.0), (80.0, 80.0), (100.0, 100.0), (120.0, 120.0)];

        let mut real_view = start_view();
        let mut selection = SelectionSet::new();
        let start_doc = real_view.to_document((40.0, 40.0));
        let mut real = Drag::Move {
            layer_id: aurora_core::Id::from_raw(0),
            start_doc,
            start_bounds,
            current_bounds: start_bounds,
        };
        for point in path {
            let _ = continue_drag(&mut real, point, &mut real_view, &mut selection, (0.0, 0.0));
        }
        let Drag::Move {
            current_bounds: real_bounds,
            ..
        } = real
        else {
            unreachable!("still a Move drag");
        };

        // The same arm, with the clamp added.
        let mut fed_back_view = start_view();
        let mut fed_back = start_bounds;
        for point in path {
            let current_doc = fed_back_view.to_document(point);
            fed_back = shift_bounds(
                start_bounds,
                (current_doc.0 - start_doc.0, current_doc.1 - start_doc.1),
            );
            // What `App::apply_move` writes back, and what a clamp in
            // this arm would then measure against.
            #[allow(clippy::cast_precision_loss)]
            fed_back_view.clamp_pan_to_minimum((fed_back.x as f32, fed_back.y as f32));
        }

        assert_eq!(
            real_bounds,
            aurora_core::Rect {
                x: 80,
                y: 80,
                ..start_bounds
            },
            "the real arm tracks the pointer's own (80, 80) delta exactly"
        );
        assert_ne!(
            fed_back, real_bounds,
            "a clamp in this arm has to visibly diverge, or the design rationale for leaving it \
             out is untested"
        );
        assert!(
            fed_back.x > real_bounds.x && fed_back.y > real_bounds.y,
            "and it diverges by running away from the pointer: {fed_back:?} vs {real_bounds:?}"
        );
    }
}
