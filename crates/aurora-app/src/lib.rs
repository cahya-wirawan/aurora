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
//! real; the one remaining `aurora_doc::BlendMode` variant (`Dissolve`
//! — this family's own explicit, now sole boundary) still silently
//! falls back to `Normal`
//! — a real, still-open gap. `LayerTree::paint_order` now recurses into
//! layer groups at any depth (ancestor-visibility-gated: an invisible
//! group hides its whole subtree), so a layer nested inside a group
//! does composite and export now; a group's own `opacity`/`blend_mode`
//! are not yet aggregated into its children's effective compositing,
//! which remains a separate, still-open gap. **A `.aur`
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
//! current document's journal to a second small file
//! (`std::env::temp_dir()` again) every startup, and — if a previous
//! run's marker is there *and* that autosave file parses and replays —
//! opens with the recovered document instead of the fake demo one.
//! **Scope, stated honestly**: still just one dialog action ("Continue"
//! — its message changes depending on whether recovery actually
//! happened), because recovery itself is unconditional and automatic
//! rather than a user choice, and autosave is written once at startup,
//! not on a repeating timer or after edits, since there is no live
//! editing loop yet to re-trigger it from. See this module's own "crash
//! recovery" section for the full reasoning.
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
//! `aurora_brush::erase_dab`/`erase_stroke` (subtractive — reduces
//! existing alpha instead of blending a colour) in place of
//! `stamp_dab`/`stamp_stroke`; bound to `e`, matching `b` for Brush.
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
//! place needed one more real fix: `tile_origin_for_view` used to
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
//! layering). `Self::paint_dab`/`Self::erase_dab` now call
//! `aurora_brush::touched_tiles` before each real write and record the
//! result into the active `Drag::Brush`/`Drag::Eraser`'s own `stroke`
//! field; `Self::handle_pointer_released` pushes the completed stroke
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
//! `Self::finish_move`, called from `Self::handle_pointer_released` when
//! the drag ends, via `aurora_doc::History::record_bounds_change` (the
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
//! `tile_origin_for_view` goes through `CanvasView::to_document` instead
//! of assuming `zoom() == 1.0`, so panning while zoomed picks the right
//! tile too. **Scope, stated honestly**: the atlas's own uv offset is
//! still only tile-granular (no sub-tile fractional scroll — see
//! `tile_origin_for_view`'s own doc comment); the atlas is sized once at
//! startup and does not resize with the window
//! (`TileResidency`'s own documented limitation); rendering a lower mip
//! while zoomed out or panning (`spike/FINDINGS.md`'s own progressive-
//! rendering finding), rotation, rulers, guides, grid, and snap all
//! remain this bullet's own still-open remainder.
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
/// for a document (a freshly built [`demo_document`], or one recovered
/// from an autosave, whose journal has no canvas-size concept of its
/// own). Once a document is live, `App::canvas_size` is the real source
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
/// scratch files — deliberately separate from [`tile_store_scratch_dir`]
/// (the live document's own store): verifying a fresh export must never
/// touch the live document's real tiles. Not a proper per-platform
/// app-support directory yet, the same scope this crate's other
/// `std::env::temp_dir()`-based paths already accept.
fn aur_verify_scratch_dir() -> PathBuf {
    std::env::temp_dir().join("aurora-aur-verify")
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
#[must_use]
fn verify_aur(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        tracing::warn!(path = %path.display(), "failed to reopen the exported .aur file to verify it");
        return false;
    };
    let Some(budget) = std::num::NonZeroUsize::new(16) else {
        unreachable!("16 is non-zero");
    };
    let mut store = match aurora_tile::TileStore::new(aur_verify_scratch_dir(), budget) {
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

/// Clears and repopulates `workspace`'s Layers/History panels for a
/// freshly opened `layers`/`history` — the real "replace the current
/// document" step [`App::open_file`] needs, shared by both routes that
/// reach it: a single-image import ([`document_from_image`], always
/// exactly one new pixel layer) and a real, possibly multi-layer `.aur`
/// open (`aurora_io::read_aur`). Returns the new `WidgetId -> LayerId`
/// map (`aurora_ui::populate_layers_panel`'s own return value) and
/// which layer should become the new active one — `layers`' own
/// topmost pixel layer ([`topmost_pixel_layer`]; for a freshly
/// imported single-layer document this is trivially the layer that was
/// just created) — for the caller to assign onto its own fields
/// alongside `layers`/`history` themselves (kept by the caller, not
/// threaded through here, since this function only needs to read them).
///
/// # Errors
///
/// Propagates [`aurora_widgets::WidgetError`] if clearing or
/// repopulating either panel fails — structurally unreachable in
/// practice (`workspace` is always a real `aurora_ui::build_workspace`
/// with real panel bodies), but this function doesn't itself know that,
/// so it reports rather than assumes.
fn replace_document(
    workspace: &mut aurora_ui::Workspace,
    scales: &Scales,
    layers: &aurora_doc::LayerTree,
    history: &aurora_doc::History,
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
// does two things: writes the current document's journal to a small
// autosave file every startup ([`write_autosave`]), and — if a previous
// run's marker is present — tries to read that file back and replay it
// ([`recover_document`]), falling back to the fake demo document if
// there's nothing to recover or it doesn't parse.
//
// Still deliberately narrow: recovery is unconditional (there is no
// "Recover Document" vs. "Discard" choice — the dialog just reports
// what already happened), and autosave is written once at startup, not
// on a repeating timer or after edits, since there is no live editing
// loop yet to re-trigger it. A real `.aur` file (ADR 0009's ZIP
// container, with a manifest and tile data) is separate follow-on work
// — there's still nothing but the journal to put in one.
//
// Both the marker and the autosave file live in `std::env::temp_dir()`
// under fixed names — deliberately not a proper per-platform app-support
// directory (no `directories`-style crate is a dependency yet).

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

/// Where this run's own autosave journal lives — analogous to
/// [`marker_path`], and for the same reason not a proper per-platform
/// app-support directory yet.
fn autosave_path() -> PathBuf {
    std::env::temp_dir().join("aurora-autosave.postcard")
}

/// Writes `history`'s journal to `path` — call once, early, the same
/// "errors are logged, not fatal" shape [`write_session_marker`] already
/// uses: failing to autosave must never stop the application starting.
fn write_autosave(path: &Path, history: &aurora_doc::History) {
    match history.save_journal() {
        Ok(bytes) => {
            if let Err(err) = std::fs::write(path, bytes) {
                tracing::warn!(?err, path = %path.display(), "failed to write the autosave journal");
            }
        }
        Err(err) => {
            tracing::warn!(?err, "failed to serialize the autosave journal");
        }
    }
}

/// Reads and replays the autosave journal at `path`, if one is present
/// and usable. Returns `None` — not an error — for anything that keeps
/// this from producing a usable document (no file, unreadable bytes, a
/// journal `postcard` can't parse, or a journal that fails to replay):
/// a missing or corrupt autosave means falling back to
/// [`demo_document`], not failing to start.
fn recover_document(path: &Path) -> Option<(aurora_doc::LayerTree, aurora_doc::History)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(?err, path = %path.display(), "failed to read the autosave journal");
            }
            return None;
        }
    };
    let history = match aurora_doc::History::load_journal(&bytes) {
        Ok(history) => history,
        Err(err) => {
            tracing::warn!(?err, "failed to deserialize the autosave journal");
            return None;
        }
    };
    let layers = match history.replay() {
        Ok(layers) => layers,
        Err(err) => {
            tracing::warn!(?err, "failed to replay the recovered autosave journal");
            return None;
        }
    };
    Some((layers, history))
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
        AppCommand::SelectTool(selected) => *tool = selected,
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
    },
    Eraser {
        last_doc: (f32, f32),
        carry: f32,
        /// Same as `Drag::Brush`'s own `stroke` field, above.
        stroke: Option<aurora_brush::StrokeSnapshot>,
    },
    Move {
        layer_id: aurora_doc::LayerId,
        start_doc: (f32, f32),
        start_bounds: aurora_core::Rect,
        current_bounds: aurora_core::Rect,
    },
    Eyedropper,
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
/// paint/erase/sample (a live store, an active layer) — that check
/// happens where the real pixel work does
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
        }),
        (aurora_ui::Tool::Eraser, PointerButton::Primary) => Some(Drag::Eraser {
            last_doc: view.to_document(canvas_point),
            carry: 0.0,
            stroke: active_pixel_layer
                .map(|(id, _)| aurora_brush::StrokeSnapshot::new(surface_id_for(id))),
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
#[must_use]
fn continue_drag(
    drag: &mut Drag,
    canvas_point: (f32, f32),
    view: &mut aurora_ui::CanvasView,
    selection: &mut aurora_doc::SelectionSet,
) -> Vec<(f32, f32)> {
    match drag {
        Drag::Pan { last_screen } => {
            let delta = (
                canvas_point.0 - last_screen.0,
                canvas_point.1 - last_screen.1,
            );
            view.pan_by(delta);
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
fn apply_scroll_zoom(
    view: &mut aurora_ui::CanvasView,
    anchor: (f32, f32),
    delta: winit::event::MouseScrollDelta,
) {
    let steps = zoom_steps_for_scroll(delta);
    let factor = ZOOM_WHEEL_BASE.powf(steps);
    view.zoom_at(anchor, view.zoom() * factor);
}

/// How much one Zoom-tool click zooms in (or, with `Alt` held, out) —
/// [`handle_zoom_tool_click`]'s own factor.
const ZOOM_CLICK_FACTOR: f32 = 2.0;

/// Handles a Zoom-tool primary click at `canvas_point`: zooms in by
/// [`ZOOM_CLICK_FACTOR`], or out (the reciprocal) if `modifiers.alt` is
/// held — Photoshop's own Zoom-tool convention (`Alt`+click to zoom
/// out), distinct from [`apply_scroll_zoom`], which works with any tool
/// active.
fn handle_zoom_tool_click(
    view: &mut aurora_ui::CanvasView,
    canvas_point: (f32, f32),
    modifiers: Modifiers,
) {
    let factor = if modifiers.alt {
        1.0 / ZOOM_CLICK_FACTOR
    } else {
        ZOOM_CLICK_FACTOR
    };
    view.zoom_at(canvas_point, view.zoom() * factor);
}

// -- Brush painting, eraser, and layer selection: a live document, a
// -- live tile store, and a way to pick which layer is active --
//
// PLAN.md M1.9's "basic brush and eraser" bullet, picking up exactly
// where `aurora_brush::stamp_dab`/`stamp_stroke` (ADR 0010) left off:
// this crate's first *live* document (`App::layers`, kept alive instead
// of being discarded after populating the panels, as it was through
// M1.8/M1.9 until now) and first real `aurora_tile::TileStore`. Eraser
// (`App::erase_dab`, `Drag::Eraser`) reuses that same live store and
// active layer, calling `aurora_brush::erase_dab`/`erase_stroke` instead
// of `stamp_dab`/`stamp_stroke` -- the bullet's other named half, now
// closed. `select_layer` closes the layer-selection half: `active_layer`
// no longer just defaults to the topmost pixel layer and stays there
// forever -- a real click on a real, clickable Layers-panel row
// (`aurora_ui::layers_panel`, `aurora_widgets::WidgetTree::hit_test`)
// changes it, live. Move's own drag-to-reposition logic (`Drag::Move`,
// `App::apply_move`) followed once `aurora_doc::LayerTree::set_bounds`
// gave it somewhere real to land, and `tile_origin_for_view` learned to
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
/// [`active_pixel_layer`] already uses. What [`tile_origin_for_view`]
/// needs to convert a document-space point into the active layer's own
/// surface-local space, now that a layer can actually sit somewhere
/// other than the document's own origin (`aurora_doc::LayerTree::set_bounds`,
/// the Move tool's own document-model support).
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn active_layer_origin(
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
) -> (f32, f32) {
    active_pixel_layer(layers, active_layer)
        .map_or((0.0, 0.0), |(_, bounds)| (bounds.x as f32, bounds.y as f32))
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
/// remaining 1 (`Dissolve` — this family's own explicit, now sole
/// boundary, qualitatively different from every mode mapped above)
/// explicitly mapped to `Normal` — an honest, documented fallback
/// (`Dissolve`'s real math is separate, still-open follow-on work;
/// falling back to `Normal` degrades a layer's *appearance* without
/// corrupting or losing any document data, the same "unpainted
/// `WidgetKind` returns `Ok(vec![])` rather than erroring" honesty
/// `paint_widget` already uses elsewhere in this codebase).
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
        // Not yet implemented in `aurora_render::BlendMode` — real,
        // separate, still-open follow-on work, not an oversight. Named
        // individually rather than behind a wildcard so a future
        // `aurora_render::BlendMode` addition forces this match to be
        // revisited instead of silently staying stubbed. This is the
        // family's own explicit, now sole remainder: `Dissolve` is
        // stochastic per-pixel selection, not a deterministic blend
        // function at all — it needs its own reproducibility design
        // decision (does a pixel's outcome need to be stable across
        // re-renders? seeded by what?) before any implementation, not
        // just new math — qualitatively different from every mode
        // implemented so far, not just an unimplemented instance of the
        // same shape.
        aurora_doc::BlendMode::Dissolve => aurora_render::BlendMode::Normal,
    }
}

/// Recomposites every tile in `residency`'s own currently-visible grid
/// from `layers.paint_order()`'s own bottom-to-top, visible pixel
/// layers into `store`'s reserved composite surface
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
/// (`tile_origin_for_view`'s own doc comment) — for a layer that
/// shares that exact origin, its own `TileId` space already lines up,
/// so its tile is read directly; for one that doesn't (a document with
/// two layers at different `bounds`, e.g. after a Move), a document-
/// space tile is converted into that specific layer's own local
/// window via [`read_layer_window`], which may need blending up to
/// four of that layer's own tiles together when origins aren't
/// tile-aligned.
///
/// **Blend modes, real for 26 of 27**: each layer's own real
/// `blend_mode` is read here and translated via `translate_blend_mode`
/// into `aurora_render::BlendMode` before reaching
/// `composite_tile_cpu` — `Normal`, the 8-mode "simple separable"
/// family (`Darken`/`Multiply`/`Lighten`/`Screen`/`Difference`/
/// `Exclusion`/`Subtract`/`Divide`), the 4-mode "dodge and burn"
/// family (`ColorDodge`/`LinearDodge`/`ColorBurn`/`LinearBurn`), the
/// 7-mode "overlay and light" family (`Overlay`/`SoftLight`/
/// `HardLight`/`VividLight`/`LinearLight`/`PinLight`/`HardMix`), the
/// 4-mode non-separable HSL family (`Hue`/`Saturation`/`Color`/
/// `Luminosity`), and the 2-mode whole-colour-selection family
/// (`DarkerColor`/`LighterColor`) composite with their own real math;
/// the one remaining `aurora_doc::BlendMode` variant (`Dissolve` —
/// this family's own explicit, now sole boundary) still silently falls
/// back to `Normal` at that same translation boundary — a real,
/// separate, still-open gap, not silently glossed over.
///
/// **Performance, incremental but coarse**: a visible tile already
/// current in `cache` is skipped entirely — see [`CompositeCache`]'s
/// own doc comment for what invalidates it. Still not per-tile-dirty-
/// aware *within* one invalidation: a single edit anywhere forces a
/// full recompute of every visible tile on the next redraw, not just
/// the one(s) it actually touched, so active painting still pays
/// something close to `spike/FINDINGS.md`'s own ~20ms "merging whole
/// tiles" cost per redraw — the exact cost that finding named as the
/// reason GPU tile compositing (`aurora_render::TileCompositor`) exists
/// at all. True per-tile dirty tracking across layers, and GPU-side
/// compositing, are both separate, still-open follow-on work. A
/// document with zero or one visible pixel layer (the common case so
/// far) is unaffected in practice either way:
/// `aurora_render::composite_tile_cpu` reproduces a single full-opacity
/// layer's own texels exactly.
fn recomposite_visible_tiles(
    residency: &aurora_gpu::TileResidency,
    layers: &aurora_doc::LayerTree,
    active_layer: Option<aurora_doc::LayerId>,
    store: &mut aurora_tile::TileStore,
    cache: &mut CompositeCache,
) {
    let mut paint_layers = Vec::new();
    for id in layers.paint_order() {
        if let (Some(surface), Some(opacity)) = (layers.surface_id(id), layers.opacity(id)) {
            let origin = layers.bounds(id).map_or((0, 0), |b| (b.x, b.y));
            let blend_mode = translate_blend_mode(
                layers
                    .blend_mode(id)
                    .unwrap_or(aurora_doc::BlendMode::Normal),
            );
            paint_layers.push((surface, opacity, origin, blend_mode));
        }
    }
    // The tile grid `residency.visible_tiles()` walks is anchored to the
    // *active* layer's own origin (`tile_origin_for_view`'s own doc
    // comment) — every other layer's own document-space tile boundaries
    // only line up with it by coincidence, so this is the one origin
    // every `tile_id` below needs converting back out of.
    let reference_origin =
        active_pixel_layer(layers, active_layer).map_or((0, 0), |(_, b)| (b.x, b.y));

    let full_tile = aurora_core::Rect {
        x: 0,
        y: 0,
        width: aurora_tile::TILE,
        height: aurora_tile::TILE,
    };
    let tile_size = i64::from(aurora_tile::TILE);
    for tile_id in residency.visible_tiles() {
        if cache.is_current(tile_id) {
            continue;
        }
        let doc_origin = (
            reference_origin.0 + i64::from(tile_id.x) * tile_size,
            reference_origin.1 + i64::from(tile_id.y) * tile_size,
        );
        let mut layer_texels: Vec<(Vec<half::f16>, f32, aurora_render::BlendMode)> =
            Vec::with_capacity(paint_layers.len());
        for &(surface, opacity, origin, blend_mode) in &paint_layers {
            let texels = if origin == reference_origin {
                match store.get(surface, tile_id) {
                    Ok(tile) => tile.texels().to_vec(),
                    Err(err) => {
                        tracing::warn!(?err, ?tile_id, "skipping layer for this composite tile");
                        continue;
                    }
                }
            } else {
                read_layer_window(store, surface, origin, doc_origin)
            };
            layer_texels.push((texels, opacity, blend_mode));
        }
        let refs: Vec<(&[half::f16], f32, aurora_render::BlendMode)> = layer_texels
            .iter()
            .map(|(texels, opacity, blend_mode)| (texels.as_slice(), *opacity, *blend_mode))
            .collect();
        let composited = aurora_render::composite_tile_cpu(&refs);
        let Ok(dest) = store.get_mut(composite_surface_id(), tile_id) else {
            continue;
        };
        dest.texels_mut().copy_from_slice(&composited);
        dest.mark_dirty(full_tile);
        cache.mark_current(tile_id);
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
/// [`Self::bump`] is the one invalidation primitive, called by every
/// `aurora-app` operation that could change what a given `TileId` now
/// composites to: a brush/eraser dab, a live Move, Undo/Redo, opening
/// or replacing the active document, and selecting a different active
/// layer (which changes the reference origin every `TileId` is measured
/// from).
///
/// **Coarse, stated honestly**: one bump invalidates every currently
/// cached tile at once, not just the one(s) the triggering edit actually
/// touched — true per-tile dirty tracking across layers is separate,
/// still-open follow-on work. `aurora_tile::TileStore`'s own per-tile
/// dirty flags (`Tile::mark_dirty`/`TileStore::take_dirty`) are
/// deliberately *not* reused for finer-grained invalidation here: they
/// only track resident tiles, so a tile dirtied by an edit and then
/// evicted before a redraw ever consumes its flag would silently stop
/// being reported dirty at all — a real correctness risk (a stale
/// composite shown as current) this coarser, explicitly-triggered design
/// avoids entirely.
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
            let Ok(src) = store.get(
                surface,
                aurora_tile::TileId {
                    x: tile_col,
                    y: tile_row,
                },
            ) else {
                continue;
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
/// real; the one remaining `aurora_doc::BlendMode` variant (`Dissolve`
/// — this family's own explicit, now sole boundary) still silently
/// falls back to `Normal` at that same translation boundary — a real,
/// still-open gap. Layer groups are now recursed into at any depth
/// (`LayerTree::paint_order`'s own documented, tested behaviour), so a
/// layer nested inside a visible group's ancestor chain composites into
/// this export too; a group's own `opacity`/`blend_mode` are not
/// aggregated into its children's effective compositing, which remains
/// a separate, still-open gap this function does not attempt to close.
///
/// A layer whose own tile fails to load for a given output tile is
/// logged and skipped for that tile only, the same "one bad tile
/// shouldn't abort the rest" discipline [`recomposite_visible_tiles`]
/// already uses — not grounds to abort the whole export.
///
/// # Errors
///
/// Returns [`aurora_io::IoError`] if the assembled buffer doesn't come
/// out to exactly `width * height * 4` samples — structurally
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
        let mut paint_layers = Vec::new();
        for id in layers.paint_order() {
            if let (Some(surface), Some(opacity)) = (layers.surface_id(id), layers.opacity(id)) {
                let origin = layers.bounds(id).map_or((0, 0), |b| (b.x, b.y));
                let blend_mode = translate_blend_mode(
                    layers
                        .blend_mode(id)
                        .unwrap_or(aurora_doc::BlendMode::Normal),
                );
                paint_layers.push((surface, opacity, origin, blend_mode));
            }
        }

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

                let mut layer_texels: Vec<(Vec<half::f16>, f32, aurora_render::BlendMode)> =
                    Vec::with_capacity(paint_layers.len());
                for &(surface, opacity, origin, blend_mode) in &paint_layers {
                    let texels = if origin == (0, 0) {
                        match store.get(surface, tile_id) {
                            Ok(tile) => tile.texels().to_vec(),
                            Err(err) => {
                                tracing::warn!(
                                    ?err,
                                    ?tile_id,
                                    "skipping layer for this export tile"
                                );
                                continue;
                            }
                        }
                    } else {
                        read_layer_window(store, surface, origin, doc_origin)
                    };
                    layer_texels.push((texels, opacity, blend_mode));
                }
                let refs: Vec<(&[half::f16], f32, aurora_render::BlendMode)> = layer_texels
                    .iter()
                    .map(|(texels, opacity, blend_mode)| (texels.as_slice(), *opacity, *blend_mode))
                    .collect();
                let composited = aurora_render::composite_tile_cpu(&refs);

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
    }

    aurora_io::Image::new(width, height, aurora_color::IccProfile::srgb(), samples)
}

/// Selects `layer_id` as the active layer: sets `*active_layer` and
/// marks its own Layers-panel row (`layer_rows` —
/// `aurora_ui::populate_layers_panel`'s own return value) as accessibly
/// selected (`accesskit::Node::set_selected`), clearing that state from
/// every other row. Pushing the updated accessibility tree to the
/// platform is the caller's job (`App::push_accessibility`) — this
/// function only touches `workspace`/`active_layer`, the same "pure
/// dispatch, caller owns the one real platform side-effect" split every
/// other function in this crate already uses
/// (`open_crash_recovery_dialog`, `begin_drag`, ...).
fn select_layer(
    workspace: &mut aurora_ui::Workspace,
    layer_rows: &HashMap<WidgetId, aurora_doc::LayerId>,
    active_layer: &mut Option<aurora_doc::LayerId>,
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
}

/// Where this session's shared tile store keeps its scratch files —
/// analogous to [`marker_path`]/[`autosave_path`], and for the same
/// reason not a proper per-platform app-support directory yet.
fn tile_store_scratch_dir() -> PathBuf {
    std::env::temp_dir().join("aurora-tiles")
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
/// silently disabled for the session, not a crash.
fn open_tile_store() -> Option<aurora_tile::TileStore> {
    let Some(budget) = std::num::NonZeroUsize::new(TILE_BUDGET) else {
        unreachable!("TILE_BUDGET is a fixed, non-zero constant");
    };
    match aurora_tile::TileStore::new(tile_store_scratch_dir(), budget) {
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
/// [`tile_origin_for_view`]'s own doc comment names) or if paging the
/// touched tile in fails.
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
// `tile_origin_for_view` picks the right tile via `CanvasView::to_document`
// instead of assuming 100% zoom.
//
// Scope, stated honestly: the atlas's own uv offset is still only
// tile-granular (no sub-tile fractional scroll); the atlas is sized once
// at startup and does not resize with the window (`TileResidency`'s own
// documented limitation); and rendering a lower mip while zoomed out or
// panning (the progressive-rendering finding `spike/FINDINGS.md` names),
// rotation, rulers, guides, grid, and snap are all still separately
// open, exactly as the bullet's own name says.

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

/// The active pixel layer's own *surface-local* tile currently at the
/// canvas area's own top-left corner, given `view`'s own pan *and
/// zoom*, and `layer_origin` ([`active_layer_origin`] — the active
/// layer's own document-space `(bounds.x, bounds.y)`, `(0.0, 0.0)` if
/// there isn't one) — the [`aurora_tile::TileId`] this maps to is where
/// [`aurora_gpu::TileResidency::set_origin`] should point the atlas.
///
/// Goes through [`aurora_ui::CanvasView::to_document`] rather than
/// dividing `view.pan()` by [`aurora_tile::TILE`] directly, so a
/// non-100% zoom is accounted for too (`to_document` already divides by
/// `view.zoom()`) — real zoom-aware panning, not the "assumes
/// `view.zoom() == 1.0`" approximation this function used before
/// [`aurora_gpu::TileResidency::set_origin`] gained real scale support.
/// Subtracting `layer_origin` from that document-space point before
/// dividing into tiles is what makes a *moved* layer
/// (`aurora_doc::LayerTree::set_bounds`) actually render in its new
/// place, not just update the document model: `aurora_tile::TileStore`
/// addresses a surface from its own local `(0, 0)`, not the document's
/// (the same conversion `layer_local_point` already does for
/// painting), and every layer built before the Move tool existed
/// happened to sit at document `(0, 0)`, so this function never needed
/// to make the distinction until now.
///
/// Still only tile-*granular*, though: the atlas's own uv offset always
/// starts at a whole tile's own top-left corner, so any sub-tile
/// fractional scroll within that tile isn't reflected — a real fix
/// needs a fractional uv offset alongside `TileResidency`'s existing
/// zoom-scaled `uv_scale`, separate follow-on work this bullet's own
/// "infinite zoom" remainder still names. Negative surface-local
/// coordinates (panning above/left of the layer's own origin, or a
/// layer moved so its origin is right of/below the canvas area's own
/// top-left corner) clamp to `0` — `TileId`'s own fields are unsigned,
/// so there is no tile to point to there; a real fix needs either
/// signed tile coordinates or a document-relative origin convention,
/// not invented here.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn tile_origin_for_view(
    view: &aurora_ui::CanvasView,
    layer_origin: (f32, f32),
) -> aurora_tile::TileId {
    let (doc_x, doc_y) = view.to_document((0.0, 0.0));
    let (local_x, local_y) = (doc_x - layer_origin.0, doc_y - layer_origin.1);
    #[allow(clippy::cast_precision_loss)]
    let tile_size = aurora_tile::TILE as f32;
    let x = (local_x / tile_size).floor().max(0.0);
    let y = (local_y / tile_size).floor().max(0.0);
    aurora_tile::TileId {
        x: x as u32,
        y: y as u32,
    }
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
    /// `WindowEvent::CloseRequested`) — the same "write once, at a real
    /// lifecycle boundary" discipline [`write_autosave`] already uses,
    /// not a reactive save on every resize/collapse.
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
    /// real, independent canvas size of its own ([`demo_document`], a
    /// recovered autosave — the crash-recovery journal doesn't persist
    /// one either, a real, honest, separate limitation from the `.aur`
    /// case below), from a decoded image's own real dimensions
    /// ([`Self::open_file`]), or from a `.aur` file's own manifest
    /// (`aurora_io::read_aur`'s own third return value,
    /// [`Self::open_aur_file`]) — the one case this was actually wrong
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
        // Only even try reading an autosave if the previous run left a
        // marker behind -- a clean shutdown never needs its own autosave
        // read back, and skipping the attempt means an autosave file left
        // over from a much older, already-recovered-from crash can't
        // resurface later.
        let recovered = had_previous_marker
            .then(|| recover_document(autosave_path))
            .flatten();
        let was_recovered = recovered.is_some();
        let (layers, history) = recovered.unwrap_or_else(demo_document);
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
        // Written unconditionally, whether this session opened the demo
        // document or a recovered one -- either way, it's the current
        // document, and is what the *next* run should recover to if this
        // one doesn't shut down cleanly.
        write_autosave(autosave_path, &history);
        let active_layer = topmost_pixel_layer(&layers);
        // Neither `demo_document` nor a recovered autosave carries a
        // real, independent canvas size (the crash-recovery journal is
        // just a `LayerOp` sequence, see `Self::canvas_size`'s own doc
        // comment) -- derived from the topmost layer here, the same
        // fallback `document_canvas_size` has always been.
        let canvas_size = document_canvas_size(&layers);
        let tile_store = open_tile_store();

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
            canvas_view: aurora_ui::CanvasView::default(),
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

    /// Runs `command` (`AppCommand::Undo` or `::Redo`) via
    /// [`run_command`] against this app's own live state — what the
    /// command palette's and (macOS) native menu's own Undo/Redo
    /// entries fall back to once `activate_command` hands the bare
    /// command back up (deliberately kept free of `layers`/`history`/
    /// `pixel_history`/the tile store — see [`ActivatedCommand`]'s own
    /// doc comment for why), the same path `Ctrl+Z`/`Ctrl+Shift+Z`
    /// themselves already run through.
    fn run_undo_redo(&mut self, command: AppCommand) {
        run_command(
            &mut self.workspace,
            &mut self.focus,
            &mut self.command_palette,
            &mut self.tool,
            &mut self.layers,
            &mut self.history,
            &mut self.pixel_history,
            self.tile_store.as_mut(),
            &mut self.undo_order,
            command,
        );
        // Either command could revert/reapply a bounds change (Move) as
        // well as a pixel edit -- coarse but safe, matching
        // `CompositeCache`'s own documented "any edit invalidates
        // everything" scoping.
        self.composite_cache.bump();
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
            match replace_document(&mut self.workspace, &scales, &layers, &history) {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to rebuild the workspace panels for the opened document"
                    );
                    return;
                }
            };

        if let Some(store) = self.tile_store.as_mut()
            && let Some(surface) = layers.surface_id(layer_id)
            && let Err(err) = aurora_io::write_into_store(&image, store, surface)
        {
            tracing::warn!(
                ?err,
                "failed to write the opened image's pixels into the tile store"
            );
        }
        write_autosave(&autosave_path(), &history);

        self.layers = layers;
        // The image's own real, decoded dimensions -- known exactly
        // here, rather than derived back out of the one layer just
        // built from it (`document_canvas_size`'s own fallback role).
        self.canvas_size = (image.width(), image.height());
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
        self.canvas_view = aurora_ui::CanvasView::default();
        self.selection = aurora_doc::SelectionSet::new();
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
            match replace_document(&mut self.workspace, &scales, &layers, &history) {
                Ok(result) => result,
                Err(err) => {
                    tracing::error!(
                        ?err,
                        "failed to rebuild the workspace panels for the opened document"
                    );
                    return;
                }
            };
        write_autosave(&autosave_path(), &history);

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
        self.canvas_view = aurora_ui::CanvasView::default();
        self.selection = aurora_doc::SelectionSet::new();
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
    /// groups are now recursed into at any depth, ancestor-visibility-
    /// gated (`aurora_doc::LayerTree::paint_order`'s own documented,
    /// tested behaviour) — a group's own `opacity`/`blend_mode` are not
    /// yet aggregated into its children's effective compositing, which
    /// remains a separate, still-open gap.
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
                tracing::warn!(?err, "failed to composite the document for export");
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
            select_layer(
                &mut self.workspace,
                &self.layer_rows,
                &mut self.active_layer,
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

        if self.tool == aurora_ui::Tool::Zoom && button == PointerButton::Primary {
            handle_zoom_tool_click(&mut self.canvas_view, canvas_point, self.modifiers);
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
    /// Before the real write, captures every tile this dab is about to
    /// touch ([`aurora_brush::touched_tiles`]) into the active
    /// `Drag::Brush`'s own `stroke` snapshot
    /// ([`aurora_brush::StrokeSnapshot::record_touch`]), if there is
    /// one — the pixel-edit half of `Self::history`'s own Undo/Redo,
    /// closed by [`Self::handle_pointer_released`] once the stroke
    /// completes. A no-op for that half specifically (still paints)
    /// when `self.drag` isn't actually a `Drag::Brush` with a real
    /// `stroke` — shouldn't happen given how this is always called, but
    /// this doesn't assume it.
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
        if let Some(Drag::Brush {
            stroke: Some(stroke),
            ..
        }) = self.drag.as_mut()
        {
            for tile in aurora_brush::touched_tiles(local, BRUSH_RADIUS) {
                if let Err(err) = stroke.record_touch(store, tile) {
                    tracing::warn!(?err, ?tile, "failed to capture a pixel-undo snapshot");
                }
            }
        }
        if let Err(err) =
            aurora_brush::stamp_dab(store, surface, local, BRUSH_RADIUS, self.current_colour)
        {
            tracing::warn!(?err, "failed to stamp a brush dab");
        }
        self.composite_cache.bump();
    }

    /// Erases one dab at `doc_point` (document space) from the active
    /// layer's own surface in the live tile store — `aurora_brush::erase_dab`,
    /// [`Self::paint_dab`]'s subtractive counterpart, sharing every one
    /// of its preconditions, silent-no-op cases, and undo-snapshot
    /// capture (against `Drag::Eraser`'s own `stroke` field instead).
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
        if let Some(Drag::Eraser {
            stroke: Some(stroke),
            ..
        }) = self.drag.as_mut()
        {
            for tile in aurora_brush::touched_tiles(local, ERASER_RADIUS) {
                if let Err(err) = stroke.record_touch(store, tile) {
                    tracing::warn!(?err, ?tile, "failed to capture a pixel-undo snapshot");
                }
            }
        }
        if let Err(err) = aurora_brush::erase_dab(store, surface, local, ERASER_RADIUS) {
            tracing::warn!(?err, "failed to erase a dab");
        }
        self.composite_cache.bump();
    }

    /// Applies `bounds` to `layer_id` directly in the live document
    /// (`aurora_doc::LayerTree::set_bounds`) — the one real mutation a
    /// `Drag::Move` needs, called every pointer-move event while one is
    /// active with that drag's own live `current_bounds`
    /// ([`Self::handle_pointer_moved`]), for live visual feedback only.
    /// Deliberately bypasses `self.history`/`self.undo_order` — the
    /// whole point of coalescing a drag into one undo step
    /// ([`Self::finish_move`]) is *not* recording an entry for every
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

    /// Records a completed `Drag::Move` as a single undo step, from
    /// `start_bounds` to wherever `layer_id` actually ended up — already
    /// applied, live, by every [`Self::apply_move`] call during the drag
    /// — via `aurora_doc::History::record_bounds_change`, which journals
    /// the move without re-applying it (the tree already reflects it).
    /// Called once, from [`Self::handle_pointer_released`], when the
    /// drag that just ended was a `Drag::Move`.
    ///
    /// A no-op if the layer never actually ended up anywhere different
    /// (`start_bounds` still matches its current bounds — e.g. a click
    /// that started and ended a drag with no real pointer movement, or
    /// `layer_id` no longer exists at all): nothing for a later undo to
    /// meaningfully reverse. A real, logged failure otherwise is worth a
    /// warning, the same discipline [`Self::apply_move`] already uses.
    fn finish_move(&mut self, layer_id: aurora_doc::LayerId, start_bounds: aurora_core::Rect) {
        if self.layers.bounds(layer_id) == Some(start_bounds) {
            return;
        }
        match self
            .history
            .record_bounds_change(&self.layers, layer_id, start_bounds)
        {
            Ok(()) => {
                self.undo_order.record(
                    UndoKind::Structural,
                    &mut self.history,
                    &mut self.pixel_history,
                );
            }
            Err(err) => tracing::warn!(?err, "failed to record the completed move"),
        }
    }

    /// Samples the active layer's own pixel at `doc_point` (document
    /// space) and, if it's actually painted (alpha `> 0.0`), sets it as
    /// the new [`Self::current_colour`] — what the Eyedropper tool does
    /// on a click or while dragging. A fully transparent texel (never
    /// painted, or painted then erased down to nothing — `Self::erase_dab`
    /// leaves RGB untouched even at zero alpha) is treated as "nothing
    /// to pick," not a valid sample, the same way a real image editor's
    /// eyedropper has nothing meaningful to pick from empty canvas. A
    /// silent no-op if there's no live store, no active layer, that
    /// layer isn't a pixel layer, or `doc_point` falls outside the
    /// surface entirely — the same absent-precondition honesty
    /// [`Self::paint_dab`] already uses.
    fn sample_eyedropper(&mut self, doc_point: (f32, f32)) {
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
        let Some([r, g, b, a]) = sample_pixel(store, surface, local) else {
            return;
        };
        if a > 0.0 {
            self.current_colour = [r, g, b];
        }
    }

    /// A real `WindowEvent::MouseInput { state: Released, .. }`: ends
    /// whatever drag is in progress. Any button release ends it — this
    /// crate has no multi-touch/multi-pointer support to disambiguate
    /// which button a drag actually started with, and a single active
    /// window only ever has one drag in progress at a time.
    ///
    /// If the ending drag was a `Drag::Brush`/`Drag::Eraser` with a real
    /// `stroke`, pushes it onto [`Self::pixel_history`] and, if that
    /// actually recorded something, into [`Self::undo_order`] too — a
    /// completed stroke becomes a real, `Ctrl+Z`-undoable step in the
    /// unified order. `PixelHistory::push`'s own `bool` return (`false`
    /// for an empty snapshot) is exactly what lets this tell "a real
    /// stroke happened" apart from "a click/drag that never actually
    /// touched a tile" (e.g. a zero-radius brush, or no active layer at
    /// all) without checking `stroke.is_empty()` itself. If the ending
    /// drag was a `Drag::Move`, [`Self::finish_move`] records the whole
    /// gesture as one coalesced undo step, from wherever it started.
    /// Also ends any in-progress rail resize ([`RailResize`]) — nothing
    /// further to record for that one; `aurora_ui::set_rail_width` has
    /// already applied every intermediate width live, on each move
    /// event, not just the final one.
    fn handle_pointer_released(&mut self) {
        self.rail_resize = None;
        match self.drag.take() {
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
                if self.pixel_history.push(stroke) {
                    self.undo_order.record(
                        UndoKind::Pixel,
                        &mut self.history,
                        &mut self.pixel_history,
                    );
                }
            }
            Some(Drag::Move {
                layer_id,
                start_bounds,
                ..
            }) => {
                self.finish_move(layer_id, start_bounds);
            }
            _ => {}
        }
    }

    /// A real `WindowEvent::MouseWheel`: zooms around the pointer's last
    /// known position ([`apply_scroll_zoom`]) if it's over the canvas
    /// area — a no-op otherwise (e.g. scrolling while the pointer is
    /// over a dock panel must not zoom the canvas).
    fn handle_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let Some(position) = self.pointer_position else {
            return;
        };
        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };
        apply_scroll_zoom(&mut self.canvas_view, canvas_point, delta);
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
    /// reconfigures the presentation surface to match — layout is pure
    /// geometry (no GPU needed) and stays current even before a
    /// window/device exist, unlike the surface resize below, which does
    /// need both.
    ///
    /// `physical_size` is converted to logical pixels via
    /// [`logical_size`]/`self.scale_factor` before it reaches
    /// `compute_layout`: every widget's own layout style
    /// (`aurora_theme::Scales`-derived padding/spacing) is defined in
    /// logical, DPI-independent units, so feeding it raw physical pixels
    /// would make widgets balloon to the wrong on-screen size on any
    /// display where `scale_factor != 1.0` — exactly the class of bug
    /// PLAN.md M1.8's "per-monitor DPI and fractional scaling" bullet is
    /// named for. The GPU surface itself still resizes to the real
    /// physical size — a render target's pixel dimensions are never
    /// logical.
    fn apply_resize(&mut self, physical_size: (u32, u32)) {
        let (width, height) = logical_size(physical_size, self.scale_factor);
        self.workspace.tree.compute_layout(width, height);

        let (Some(gpu), Some(surface)) = (self.gpu.as_ref(), self.surface.as_mut()) else {
            return;
        };
        surface.resize(gpu.device(), physical_size);
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
    // One linear per-frame flow (build widget paints, sync the canvas
    // atlas, one shared render pass drawing both) -- splitting further
    // would just relocate lines across more functions without reducing
    // the real complexity a GPU frame has, the same call
    // `render_test.rs::render_and_sample_pixel` already makes for an
    // analogous reason.
    #[allow(clippy::too_many_lines)]
    fn redraw(&mut self) {
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
                let widget_paints =
                    collect_widget_paints(&self.workspace.tree, &self.theme, &self.scales, gpu);

                // Sync before drawing, so this frame shows the latest
                // painted pixels rather than lagging one frame behind.
                if let Some(residency) = self.residency.as_mut() {
                    if let Some(canvas_size) =
                        canvas_area_physical_size(&self.workspace, self.scale_factor)
                    {
                        residency.set_origin(
                            gpu.queue(),
                            tile_origin_for_view(
                                &self.canvas_view,
                                active_layer_origin(&self.layers, self.active_layer),
                            ),
                            canvas_size,
                            self.canvas_view.zoom(),
                        );
                    }
                    if let Some(store) = self.tile_store.as_mut() {
                        recomposite_visible_tiles(
                            residency,
                            &self.layers,
                            self.active_layer,
                            store,
                            &mut self.composite_cache,
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
fn collect_widget_paints(
    tree: &WidgetTree<WidgetKind>,
    theme: &Theme,
    scales: &Scales,
    gpu: &GpuContext,
) -> Vec<(GpuMesh, [f32; 4])> {
    let mut widget_paints = Vec::new();
    for id in tree.paint_order() {
        match paint_widget(tree, id, theme, scales) {
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
                clear_session_marker(&self.marker_path);
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
                self.drag = None;
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
        ActivatedCommand, AppCommand, COMMAND_CLOSE_HISTORY, COMMAND_CLOSE_LAYERS,
        COMMAND_CLOSE_PROPERTIES, COMMAND_FILE_OPEN, COMMAND_FILE_SAVE, COMMAND_FOCUS_HISTORY,
        COMMAND_FOCUS_LAYERS, COMMAND_FOCUS_PROPERTIES, COMMAND_REDO, COMMAND_TOGGLE_HISTORY,
        COMMAND_TOGGLE_LAYERS, COMMAND_TOGGLE_PROPERTIES, COMMAND_UNDO, CRASH_RECOVERY_CONTINUE,
        ClipboardAccess, CompositeCache, Drag, FileDialogAccess, Key, KeyChord, Modifiers,
        NamedKey, PointerButton, RAIL_DIVIDER_HIT_TOLERANCE, RailResize, UndoKind, UndoOrder,
        activate_command, apply_scroll_zoom, autosave_path, background_color_from_theme,
        begin_drag, canvas_area_physical_rect, canvas_area_physical_size, clear_session_marker,
        close_command_palette, close_crash_recovery_dialog, collect_widget_paints,
        composite_document, composite_surface_id, continue_drag, crash_recovery_dialog_message,
        default_shortcuts, demo_document, document_canvas_size, document_from_image,
        handle_dialog_key, handle_dialog_pointer, handle_key, handle_palette_key,
        handle_zoom_tool_click, is_aur_path, layer_local_point, load_scales, load_theme,
        logical_point, logical_size, open_command_palette, open_crash_recovery_dialog, open_image,
        open_tile_store, palette_commands, pointer_in_canvas, pointer_on_rail_divider,
        previous_session_left_a_marker, recomposite_visible_tiles, recover_document,
        replace_document, resized_rail_width, run_command, sample_pixel, select_layer,
        tile_origin_for_view, tile_store_scratch_dir, toggle_command_palette, topmost_pixel_layer,
        translate_key, translate_modifiers, translate_pointer_button, verify_aur, write_autosave,
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

        select_layer(&mut workspace, &layer_rows, &mut active_layer, a);
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
        select_layer(&mut workspace, &layer_rows, &mut active_layer, b);
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
        let (layer_rows, active_layer) =
            match replace_document(&mut workspace, &scales, &new_layers, &new_history) {
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
        // Simulates what `App::finish_move` itself does after a
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
        let scratch = tile_store_scratch_dir();
        assert_ne!(scratch, super::marker_path());
        assert_ne!(scratch, autosave_path());
    }

    #[test]
    fn open_tile_store_succeeds_against_the_real_scratch_directory() {
        // A real, if unremarkable, assertion: this crate's own scratch
        // directory is always writable in a real environment (the same
        // assumption `write_session_marker`'s own `std::env::temp_dir()`
        // use already makes) -- confirms `open_tile_store` doesn't
        // always return `None` in ordinary conditions, not this
        // function's own I/O error path (real disk-failure injection is
        // not something this sandbox can do).
        assert!(open_tile_store().is_some());
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
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);

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

        let paints = collect_widget_paints(&tree, &theme, &scales, &context);
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
        recomposite_visible_tiles(&residency, &layers, Some(bottom), &mut store, &mut cache);

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
    fn recomposite_visible_tiles_of_an_empty_document_is_fully_transparent() {
        let Some(context) = real_gpu_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store();
        let layers = aurora_doc::LayerTree::new();
        let residency =
            aurora_gpu::TileResidency::new(context.device(), context.queue(), (256, 256));
        let mut cache = CompositeCache::default();
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);

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
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);
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
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);
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
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);

        fill_solid(
            &mut store,
            composite_surface_id(),
            tile_id,
            [0.0, 1.0, 0.0, 1.0],
        );
        cache.bump();
        recomposite_visible_tiles(&residency, &layers, None, &mut store, &mut cache);
        assert_eq!(
            read_first_texel(&mut store, composite_surface_id(), tile_id),
            (1.0, 0.0, 0.0, 1.0),
            "bump must force a real recompute, overwriting the poked value"
        );
    }

    #[test]
    fn sample_pixel_reads_back_a_real_stamped_dab() {
        let (_dir, mut store) = real_tile_store();
        let surface = aurora_tile::SurfaceId::from_raw(0);
        if let Err(err) =
            aurora_brush::stamp_dab(&mut store, surface, (10.5, 10.5), 20.0, [1.0, 0.0, 0.0])
        {
            unreachable!("{err:?}");
        }
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

    #[test]
    fn recovering_a_missing_autosave_returns_none() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.postcard");
        assert!(recover_document(&path).is_none());
    }

    #[test]
    fn recovering_garbage_bytes_returns_none() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.postcard");
        if let Err(err) = std::fs::write(&path, b"not a postcard journal") {
            unreachable!("{err}");
        }
        assert!(recover_document(&path).is_none());
    }

    #[test]
    fn writing_then_recovering_an_autosave_round_trips_the_same_journal_descriptions() {
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => unreachable!("{err}"),
        };
        let path = dir.path().join("aurora-autosave.postcard");
        let (_layers, history) = demo_document();
        let original_descriptions = history.journal_descriptions();

        write_autosave(&path, &history);
        let Some((_recovered_layers, recovered_history)) = recover_document(&path) else {
            unreachable!("just wrote a real autosave");
        };
        assert_eq!(
            recovered_history.journal_descriptions(),
            original_descriptions
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
    fn tile_origin_for_view_is_zero_zero_with_no_pan() {
        let view = CanvasView::new();
        assert_eq!(
            tile_origin_for_view(&view, (0.0, 0.0)),
            aurora_tile::TileId { x: 0, y: 0 }
        );
    }

    #[test]
    fn tile_origin_for_view_follows_a_positive_pan() {
        // Panning the view so document (0, 0) renders 300 logical px to
        // the right/down of the canvas area's own top-left corner means
        // the tile now at that corner is one tile up and to the left of
        // the document's own origin tile... but since tile coordinates
        // are unsigned, this must clamp to (0, 0), not go negative.
        let mut view = CanvasView::new();
        view.pan_by((300.0, 300.0));
        assert_eq!(
            tile_origin_for_view(&view, (0.0, 0.0)),
            aurora_tile::TileId { x: 0, y: 0 }
        );
    }

    #[test]
    fn tile_origin_for_view_follows_a_negative_pan() {
        // Panning left/up by more than one tile's worth (256px) means
        // the canvas area's own top-left corner now shows document
        // pixels *past* one whole tile -- origin must advance to (1, 1).
        let mut view = CanvasView::new();
        view.pan_by((-300.0, -300.0));
        assert_eq!(
            tile_origin_for_view(&view, (0.0, 0.0)),
            aurora_tile::TileId { x: 1, y: 1 }
        );
    }

    #[test]
    fn tile_origin_for_view_accounts_for_zoom_not_just_pan() {
        // The same 600px pan means a different document-space top-left
        // depending on zoom (`to_document` divides by `zoom`) -- at 2x
        // zoom, 600 screen px is only 300 document px, landing in tile
        // (1, 1), not the (2, 2) a zoom-blind computation (dividing pan
        // by `TILE` directly, as this function used to) would give.
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 2.0);
        view.pan_by((-600.0, -600.0));
        assert_eq!(
            tile_origin_for_view(&view, (0.0, 0.0)),
            aurora_tile::TileId { x: 1, y: 1 }
        );
    }

    #[test]
    fn tile_origin_for_view_accounts_for_a_moved_layers_own_origin() {
        // No pan/zoom at all, but the active layer itself sits at
        // document (300, 300) -- the canvas area's own top-left corner
        // (document (0, 0)) is now *before* the layer even starts, in
        // surface-local space (-300, -300), which clamps to tile (0, 0)
        // the same way a pan past the document's own edge already does.
        let view = CanvasView::new();
        assert_eq!(
            tile_origin_for_view(&view, (300.0, 300.0)),
            aurora_tile::TileId { x: 0, y: 0 }
        );

        // A layer at (300, 300), *plus* enough pan to put document
        // (600, 600) at the canvas area's own top-left corner: surface-
        // local (600 - 300, 600 - 300) = (300, 300), landing in tile
        // (1, 1) -- proving the layer's own origin and the view's own
        // pan combine, neither alone.
        let mut panned = CanvasView::new();
        panned.pan_by((-600.0, -600.0));
        assert_eq!(
            tile_origin_for_view(&panned, (300.0, 300.0)),
            aurora_tile::TileId { x: 1, y: 1 }
        );
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
            }) => {
                assert_eq!(last_doc, (10.0, 20.0));
                assert_eq!(carry, 0.0);
                assert!(
                    stroke.is_none(),
                    "no active pixel layer means nothing to snapshot"
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
            }) => {
                assert_eq!(last_doc, (10.0, 20.0));
                assert_eq!(carry, 0.0);
                assert!(
                    stroke.is_none(),
                    "no active pixel layer means nothing to snapshot"
                );
            }
            other => unreachable!("{other:?}"),
        }
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
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Pan {
            last_screen: (10.0, 10.0),
        };
        let dabs = continue_drag(&mut drag, (15.0, 8.0), &mut view, &mut selection);
        assert_eq!(view.pan(), (5.0, -2.0));
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
        let dabs = continue_drag(&mut drag, (30.0, 25.0), &mut view, &mut selection);
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
        let _ = continue_drag(&mut drag, (50.0, 5.0), &mut view, &mut selection);
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
        };
        // radius 24, DEFAULT_SPACING 0.25 -> step 6; a 12-unit segment
        // lands dabs at 6 and 12 (the segment's own start, 0, is not
        // re-emitted -- it was already painted by whatever started the
        // drag or the previous event).
        let dabs = continue_drag(&mut drag, (12.0, 0.0), &mut view, &mut selection);
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
        };
        let first = continue_drag(&mut drag, (3.0, 0.0), &mut view, &mut selection);
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
        let second = continue_drag(&mut drag, (7.0, 0.0), &mut view, &mut selection);
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
        };
        // Same radius/spacing as Brush (ERASER_RADIUS == BRUSH_RADIUS ==
        // 24, DEFAULT_SPACING 0.25 -> step 6), so the same dab positions
        // land for the same segment.
        let dabs = continue_drag(&mut drag, (12.0, 0.0), &mut view, &mut selection);
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
        let dabs = continue_drag(&mut drag, (15.0, -8.0), &mut view, &mut selection);
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
        let dabs = continue_drag(&mut drag, (15.0, -8.0), &mut view, &mut selection);
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
            (100.0, 100.0),
            winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
        );
        assert!(view.zoom() > 1.0, "zoom was {}", view.zoom());
    }

    #[test]
    fn apply_scroll_zoom_zooms_out_on_a_negative_scroll() {
        let mut view = CanvasView::new();
        apply_scroll_zoom(
            &mut view,
            (100.0, 100.0),
            winit::event::MouseScrollDelta::LineDelta(0.0, -1.0),
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
        handle_zoom_tool_click(&mut view, (50.0, 50.0), Modifiers::none());
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
        handle_zoom_tool_click(&mut view, (50.0, 50.0), alt_held);
        assert_eq!(view.zoom(), 0.5);
    }
}
