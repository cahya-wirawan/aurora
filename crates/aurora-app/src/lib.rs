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
//! system clipboard, and its "Open File…" entry shows a real, native
//! `rfd::FileDialog`; a real drag-and-dropped file is recorded the same
//! way. Both `arboard`/`rfd` are real platform calls with no meaningful
//! headless behaviour, so `handle_palette_key` takes them as
//! `&mut dyn ClipboardAccess`/`&mut dyn FileDialogAccess` rather than
//! calling them directly — the same "keep the pure dispatch logic
//! testable, isolate the untestable platform call" seam `translate_key`/
//! `translate_modifiers` already use for keyboard input. **Honest about
//! its own limit**: a file chosen via "Open File…" *or* dropped onto the
//! window is only recorded (`App::pending_open_path` — the same slot
//! either route writes to, since both are the same "the user wants to
//! open this" signal), not imported — `aurora-io` remains an empty
//! skeleton (separate M1.9 work), the same "detect a real signal, defer
//! the action" pattern this crate's own crash-recovery marker already
//! uses.
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
//! **Basic tools and brush painting** (PLAN.md M1.9): this crate's first
//! pointer input at all (`CursorMoved`/`MouseInput`/`MouseWheel`) drives
//! `aurora_ui::Tool`'s six variants — Zoom (click and scroll-wheel),
//! Pan (drag), and Marquee Select (drag, into a real
//! `aurora_doc::SelectionSet`) are fully wired; Brush is too, as of the
//! same milestone's "wire a live document" step: `App` now keeps its own
//! `LayerTree` alive (previously built, used to populate the panels, and
//! discarded every run) plus a real `aurora_tile::TileStore` (ADR 0010),
//! and a Brush drag calls `aurora_brush::stamp_dab`/`advance_segment`
//! against the topmost pixel layer's own surface — a real mouse drag
//! really paints real pixels into a real, live document for the first
//! time in this project. **Active-layer selection followed the same
//! milestone**: `aurora_ui::layers_panel`'s own rows are now real,
//! non-zero-sized, clickable widgets (`aurora_widgets::WidgetTree::hit_test`,
//! new for this), so clicking one calls `select_layer` — updates
//! `active_layer` (what Brush paints into) and marks the row accessibly
//! selected, both instead of always targeting the topmost pixel layer
//! with no way to change it. Eyedropper remains a real, selectable,
//! inert tool (no colour-sampling function built on `tile_store` yet);
//! eraser and undo-as-you-drag are separate, still-open follow-on work.
//! See `aurora_ui::tool`'s own doc comment and this module's "brush
//! painting"/"layer selection" sections for the full reasoning.
//!
//! **Real rendering, for the first time** (PLAN.md M1.8's own "Canvas"
//! bullet): `resumed` builds an `aurora_gpu::TileResidency` and
//! `aurora_gpu::CanvasPipeline` sized to the canvas dock area; `redraw`
//! syncs the atlas from `tile_store` (whatever `active_layer` actually
//! holds, painted or not) and draws it within that area's own viewport,
//! in the same pass that already clears the background — a Brush stroke
//! is now actually visible, not just written into an otherwise-invisible
//! store. **Scope, stated honestly**: only `CanvasView`'s pan is
//! reflected (`tile_origin_for_view` deliberately ignores zoom —
//! `TileResidency` has no scale support); the atlas is sized once at
//! startup and does not resize with the window
//! (`TileResidency`'s own documented limitation); rotation, rulers,
//! guides, grid, and snap remain this bullet's own still-open remainder.
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
use aurora_theme::{Palette, Scales, ThemeSet};
use aurora_widgets::shortcut::{Key, KeyChord, Modifiers, NamedKey, ShortcutRegistry};
use aurora_widgets::widgets::{
    CommandEntry, DialogAction, DialogHandle, command_palette_state, insert_command_palette,
    insert_dialog, move_command_palette_selection, set_command_palette_query,
};
use aurora_widgets::{FocusManager, WidgetId};
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
/// to make, per `aurora-theme`'s own doc comment) and converts its
/// `surface.app` token (the overall application chrome background —
/// `surface.canvas` is reserved for the document canvas area, which
/// doesn't exist yet) into the linear-light `wgpu::Color` a window
/// clear needs.
///
/// Theme *selection* (choosing among built-ins, a user preference) is
/// separate, later work; this always loads Dark.
///
/// # Errors
///
/// Returns an error if the built-in palette/theme TOML fails to parse —
/// which would mean the checked-in design files themselves are broken,
/// not a runtime condition a user could hit.
fn load_background_color() -> anyhow::Result<wgpu::Color> {
    let palette = Palette::from_toml_str(PALETTE_TOML)?;
    let mut themes = ThemeSet::new();
    themes.register(DARK_THEME_TOML)?;
    let theme = themes.resolve("Dark", &palette)?;
    let [r, g, b] = theme.surface.app.to_srgb_f32();
    Ok(wgpu::Color {
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
    })
}

/// Loads the real, owner-approved scales (`design/tokens/scales.toml`)
/// — needed by any widget with real chrome (buttons, the crash-recovery
/// dialog built from them) per invariant §7.3.10, the same "resolve
/// from tokens, never a literal" discipline `load_background_color`
/// already applies to colour.
///
/// # Errors
///
/// Returns an error if the built-in scales TOML fails to parse — same
/// caveat as [`load_background_color`]: this would mean the checked-in
/// design file itself is broken, not a runtime condition a user could
/// hit.
fn load_scales() -> anyhow::Result<Scales> {
    Ok(Scales::from_toml_str(SCALES_TOML)?)
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
            close_crash_recovery_dialog(workspace, focus, dialog);
            if let Some(action) = action {
                tracing::info!(action, "crash recovery dialog action chosen");
            }
        }
        _ => {}
    }
}

// -- Command dispatch: keyboard shortcuts and the command palette --
//
// PLAN.md M1.8's "command palette, keyboard shortcuts" bullet. Every
// function below is deliberately free (not a method on `App`) and
// platform-free (`aurora_widgets`/`aurora_ui` types only, no
// `winit::event_loop`/GPU state) — the same "pure logic, headlessly
// testable" shape `demo_document`/`load_background_color` already use,
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
/// itself, and `SelectTool` switches `App`'s own active
/// `aurora_ui::Tool` (PLAN.md M1.9's "basic tools" bullet). More
/// commands (undo/redo, save) are real, separate follow-on work once
/// this crate has real actions for them to invoke — inventing
/// placeholder commands with nothing behind them would be exactly the
/// kind of half-finished feature CLAUDE.md warns against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppCommand {
    FocusNext,
    FocusPrevious,
    ToggleCommandPalette,
    SelectTool(aurora_ui::Tool),
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
        // Tool-switch letters match Photoshop's own single-key bindings
        // (no modifier) -- the same convention this project's target
        // users already carry in muscle memory. Bound even for Move/
        // Eyedropper, which don't do anything yet once selected (see
        // `aurora_ui::tool`'s own doc comment) -- switching *to* one is
        // still real, honest behaviour; only its own pointer handling is
        // the still-open part.
        ("v", AppCommand::SelectTool(aurora_ui::Tool::Move)),
        ("m", AppCommand::SelectTool(aurora_ui::Tool::MarqueeSelect)),
        ("z", AppCommand::SelectTool(aurora_ui::Tool::Zoom)),
        ("h", AppCommand::SelectTool(aurora_ui::Tool::Pan)),
        ("i", AppCommand::SelectTool(aurora_ui::Tool::Eyedropper)),
        ("b", AppCommand::SelectTool(aurora_ui::Tool::Brush)),
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

/// Whatever the caller uses to show a native "open file" dialog —
/// [`SystemFileDialog`] in real use, a fake in tests.
trait FileDialogAccess {
    fn pick_file(&mut self) -> Option<PathBuf>;
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
}

/// Command ids [`palette_commands`] emits — `aurora_widgets::widgets::
/// CommandEntry::id` is opaque to `aurora-widgets` itself (see that
/// module's own doc comment); these constants are where this crate
/// gives its own ids meaning, matched in [`command_target`].
const COMMAND_FOCUS_LAYERS: &str = "view.focus_layers";
const COMMAND_FOCUS_PROPERTIES: &str = "view.focus_properties";
const COMMAND_FOCUS_HISTORY: &str = "view.focus_history";
const COMMAND_FILE_OPEN: &str = "file.open";

/// The command palette's own, real content: one command per docked
/// panel, focusing it, plus a real native "Open File…" picker. Genuine,
/// not placeholder — each focus command moves real keyboard focus to a
/// real, already-focusable panel region (see `aurora-ui`'s
/// `insert_panel`), verifiable the same way any other focus change is
/// (`push_accessibility`); `COMMAND_FILE_OPEN` shows a real, native
/// `rfd::FileDialog`. **Honest about its own limit**: there is still no
/// document-import pipeline (`aurora-io` remains an empty skeleton,
/// separate M1.9 work), so a chosen path is only recorded
/// (`App::pending_open_path`), the same "detect a real signal, defer
/// the action" pattern this crate's own crash-recovery marker already
/// uses. A richer command set (undo, save, tool switches) waits on
/// those real actions existing.
fn palette_commands() -> Vec<CommandEntry> {
    vec![
        CommandEntry::new(COMMAND_FOCUS_LAYERS, "Focus Layers Panel"),
        CommandEntry::new(COMMAND_FOCUS_PROPERTIES, "Focus Properties Panel"),
        CommandEntry::new(COMMAND_FOCUS_HISTORY, "Focus History Panel"),
        CommandEntry::new(COMMAND_FILE_OPEN, "Open File…"),
    ]
}

/// Resolves an activated command-palette entry's own `id` (one of the
/// `COMMAND_*` constants above) to the widget it should focus. `None`
/// for an id this build doesn't recognise — defensive; every id
/// [`palette_commands`] itself emits is handled here.
fn command_target(workspace: &aurora_ui::Workspace, id: &str) -> Option<WidgetId> {
    match id {
        COMMAND_FOCUS_LAYERS => Some(workspace.layers.root),
        COMMAND_FOCUS_PROPERTIES => Some(workspace.properties.root),
        COMMAND_FOCUS_HISTORY => Some(workspace.history.root),
        _ => None,
    }
}

/// Activates a command by its own opaque id — shared by the command
/// palette's `Enter` key and, on macOS, the native menu bar
/// (`App::handle_menu_event`): the same underlying action, reachable
/// from two different UI surfaces, rather than two parallel
/// implementations of "what does this command do." Moves focus for a
/// panel-focus command ([`command_target`]); shows the native file
/// dialog and returns the picked path for [`COMMAND_FILE_OPEN`]; logs
/// and returns `None` for any other id.
fn activate_command(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    id: &str,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<PathBuf> {
    if let Some(target) = command_target(workspace, id) {
        if let Err(err) = focus.focus(&mut workspace.tree, target) {
            tracing::warn!(?err, "activated command's target isn't focusable");
        }
        return None;
    }
    if id == COMMAND_FILE_OPEN {
        return file_dialog.pick_file();
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

/// Builds the native menu's own cross-platform structure: File > Open
/// File…, View > Focus Layers/Properties/History Panel — reusing the
/// exact same `COMMAND_*` ids the command palette already uses (via
/// `MenuItem::with_id`), so [`activate_command`] drives both UI
/// surfaces identically; nothing here invents a second command
/// vocabulary. Building the model (as opposed to attaching it to a
/// window) is the same on every platform muda supports, which is why
/// this function itself needs no further `#[cfg]` beyond the module
/// section's own macOS gate.
#[cfg(target_os = "macos")]
fn build_menu() -> muda::Menu {
    let menu = muda::Menu::new();

    let file_menu = match muda::Submenu::with_items(
        "File",
        true,
        &[&muda::MenuItem::with_id(
            COMMAND_FILE_OPEN,
            "Open File…",
            true,
            None,
        )],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("a single, freshly built item cannot fail to append: {err:?}"),
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
        ],
    ) {
        Ok(submenu) => submenu,
        Err(err) => unreachable!("freshly built items cannot fail to append: {err:?}"),
    };

    if let Err(err) = menu.append_items(&[&file_menu, &view_menu]) {
        tracing::warn!(?err, "failed to build the native menu bar structure");
    }
    menu
}

/// Opens the command palette (a no-op if one is already open): inserts
/// it into `workspace.tree` under `workspace.root` with
/// [`palette_commands`]'s own list, then moves keyboard focus to it.
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

/// Runs a global shortcut's own command — [`handle_key`]'s dispatch
/// target once a [`KeyChord`] resolves via [`ShortcutRegistry::resolve`].
fn run_command(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    tool: &mut aurora_ui::Tool,
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
    }
}

/// Routes one key press while the command palette is open — captures
/// input directly rather than going through [`ShortcutRegistry`], the
/// same "a modal dialog owns the keyboard while open" behaviour every
/// mainstream command palette (VS Code, Sublime) uses. A no-op if
/// `palette` is `None` (defensive; [`handle_key`] only calls this when
/// it's `Some`).
/// Routes one key press while the command palette is open. Returns
/// `Some(path)` only when `Enter` just activated [`COMMAND_FILE_OPEN`]
/// and the user picked a real file — the one case this function can't
/// fully resolve itself (there's no import pipeline for it to hand the
/// path to yet; see [`palette_commands`]'s own doc comment), so it
/// hands the path back up to [`handle_key`]/`App` instead. Every other
/// case returns `None`.
#[allow(clippy::too_many_arguments)]
fn handle_palette_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    palette: &mut Option<WidgetId>,
    chord: KeyChord,
    text: Option<&str>,
    clipboard: &mut dyn ClipboardAccess,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<PathBuf> {
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
/// `tool`). Anything else (an unbound chord, with nothing modal open) is
/// silently ignored — there's no text field to fall back to routing
/// into yet.
///
/// Returns `Some(path)` only in the one case no pure `WidgetTree`
/// mutation can finish on its own: the palette's `Open File…` command
/// was just activated and the user picked a real file via
/// `file_dialog` — see [`handle_palette_key`]'s own doc comment. The
/// caller (`App::handle_key_event`) is what actually has somewhere to
/// put it.
#[allow(clippy::too_many_arguments)]
fn handle_key(
    workspace: &mut aurora_ui::Workspace,
    focus: &mut FocusManager,
    dialog: &mut Option<DialogHandle>,
    palette: &mut Option<WidgetId>,
    tool: &mut aurora_ui::Tool,
    shortcuts: &ShortcutRegistry<AppCommand>,
    modifiers: Modifiers,
    key: Key,
    text: Option<&str>,
    clipboard: &mut dyn ClipboardAccess,
    file_dialog: &mut dyn FileDialogAccess,
) -> Option<PathBuf> {
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
        run_command(workspace, focus, palette, tool, command);
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
// Move and Eyedropper are real, selectable `aurora_ui::Tool` variants
// (see that enum's own doc comment) but have no pointer handling here
// either — the blockers are the same ones named there, not a gap
// introduced by this section.

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

/// One in-progress pointer drag. `Pan` tracks the last *screen*-space
/// position (panning moves the view itself, so re-deriving a document
/// point from a moving view on every event would be circular); `Marquee`
/// tracks the fixed document-space point the drag started at, since a
/// selection rectangle is defined in document space regardless of how
/// the view is panned/zoomed mid-drag; `Brush` tracks the last
/// document-space point painted, the same "delta since last event"
/// shape `Pan` uses, plus `carry` — how far the stroke has already
/// travelled past the last placed dab
/// (`aurora_brush::advance_segment`'s own carry parameter) — so spacing
/// stays correct across many small move events, not just within one
/// event's own segment (see [`continue_drag`]'s own doc comment for why
/// this matters).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Pan { last_screen: (f32, f32) },
    Marquee { start_doc: (f32, f32) },
    Brush { last_doc: (f32, f32), carry: f32 },
}

/// Starts a drag for `tool`/`button` at `canvas_point` (already
/// canvas-area-relative — see [`pointer_in_canvas`]), or `None` if this
/// tool/button combination doesn't start one. The middle button always
/// pans, regardless of the active tool — the usual "hand tool"
/// convention professional raster editors already use as a universal
/// pan gesture.
///
/// `Brush` starts unconditionally on a primary click, regardless of
/// whether there's actually anywhere to paint (a live store, an active
/// layer) — that check happens where the real painting does
/// (`App::paint_dab`), keeping this function pure and not needing to
/// know about either.
#[must_use]
fn begin_drag(
    tool: aurora_ui::Tool,
    button: PointerButton,
    canvas_point: (f32, f32),
    view: &aurora_ui::CanvasView,
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
        }),
        _ => None,
    }
}

/// Advances an in-progress `drag` to `canvas_point`: pans the view by
/// the screen-space delta since the last event, updates the active
/// selection to the marquee rectangle spanned so far
/// (`aurora_ui::tool::marquee_rect`) — live, so the selection visibly
/// grows/shrinks as the user drags, not just once on release — or, for
/// `Brush`, returns the document-space dab centers this event's own new
/// segment placed, via `aurora_brush::advance_segment` (**not**
/// `dabs_along_path` on a fresh two-point slice each time, which would
/// restart spacing's own `carry` at `0.0` on every single move event —
/// for a slow drag whose per-event segments are each shorter than one
/// dab's own spacing, that would silently place no dabs at all past the
/// first, despite real distance covered over many events;
/// `advance_segment` carries `Drag::Brush`'s own `carry` field forward
/// across events instead, exactly the problem it exists to solve).
///
/// Deliberately returns dab positions as plain data rather than
/// stamping them itself: stamping needs a live `aurora_tile::TileStore`
/// and the active layer's bounds, neither of which this function (or
/// `Drag`/`begin_drag` above) needs to know about to stay exactly as
/// pure and testable as `Pan`/`Marquee` already are — the caller
/// (`App::handle_pointer_moved`) does the actual painting.
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
        Drag::Brush { last_doc, carry } => {
            let current_doc = view.to_document(canvas_point);
            let step = aurora_brush::dab_step(BRUSH_RADIUS, aurora_brush::DEFAULT_SPACING);
            let (dabs, new_carry) =
                aurora_brush::advance_segment(*last_doc, current_doc, *carry, step);
            *last_doc = current_doc;
            *carry = new_carry;
            dabs
        }
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

// -- Brush painting and layer selection: a live document, a live tile
// -- store, and a way to pick which layer is active --
//
// PLAN.md M1.9's "basic brush and eraser" bullet, picking up exactly
// where `aurora_brush::stamp_dab`/`stamp_stroke` (ADR 0010) left off:
// this crate's first *live* document (`App::layers`, kept alive instead
// of being discarded after populating the panels, as it was through
// M1.8/M1.9 until now) and first real `aurora_tile::TileStore`.
// `select_layer` closes the other half: `active_layer` no longer just
// defaults to the topmost pixel layer and stays there forever -- a real
// click on a real, clickable Layers-panel row (`aurora_ui::layers_panel`,
// `aurora_widgets::WidgetTree::hit_test`) changes it, live. Eraser and
// undo-as-you-drag remain separate, still-open follow-on work; so does
// Move's own drag-to-reposition logic, even though its *blocker*
// (no active-layer selection) is what this section just resolved.

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

/// The Brush tool's fixed radius and colour — real defaults, not a
/// placeholder, but not a considered one either: there is no brush
/// options UI yet (size/colour picker, real engine, Phase 2 per
/// PLAN.md's own "(real engine is Phase 2)" framing on this bullet).
const BRUSH_RADIUS: f32 = 24.0;
const BRUSH_COLOUR: [f32; 3] = [0.0, 0.0, 0.0];

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
// Scope, stated honestly: only pan is reflected (`CanvasView`'s own
// `zoom`, and rotation/rulers/guides/grid/snap, are not -- this bullet's
// own still-open remainder); the atlas is sized once at startup and
// does not resize with the window (`TileResidency`'s own documented
// limitation); and infinite zoom, rotation, rulers, guides, grid, and
// snap are all still separately open, exactly as the bullet's own name
// says.

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

/// The document-space point currently at the canvas area's own top-left
/// corner, given `view`'s own pan — the [`aurora_tile::TileId`] this
/// maps to is where [`aurora_gpu::TileResidency::set_origin`] should
/// point the atlas.
///
/// **Zoom is deliberately not reflected here** — `TileResidency` has no
/// scale support (it samples its atlas 1:1 into the viewport), so this
/// assumes `view.zoom() == 1.0` regardless of its real value; real
/// zoom-aware rendering needs either shader-side scaling or mip-level
/// selection tied to zoom, separate follow-on work this bullet's own
/// "infinite zoom" remainder still names. Negative document coordinates
/// (panning above/left of the document's own origin) clamp to `0` —
/// `TileId`'s own fields are unsigned, so there is no tile to point to
/// there; a real fix needs either signed tile coordinates or a
/// document-relative origin convention, not invented here.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn tile_origin_for_view(view: &aurora_ui::CanvasView) -> aurora_tile::TileId {
    let (pan_x, pan_y) = view.pan();
    #[allow(clippy::cast_precision_loss)]
    let tile_size = aurora_tile::TILE as f32;
    let x = (-pan_x / tile_size).floor().max(0.0);
    let y = (-pan_y / tile_size).floor().max(0.0);
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
    /// The most recent path chosen via [`COMMAND_FILE_OPEN`], if any —
    /// there is no document-import pipeline yet to hand it to (see
    /// [`palette_commands`]'s own doc comment), so this is only ever
    /// recorded and logged, ready for whenever that pipeline exists.
    pending_open_path: Option<PathBuf>,
    /// PLAN.md M1.9's "basic tools" bullet — see `aurora_ui::tool`'s own
    /// doc comment for exactly which of the six named tools (Move,
    /// Marquee Select, Zoom, Pan, Eyedropper, Brush) actually do
    /// anything yet.
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
    /// kept alive from then on, unlike `history` (used once to populate
    /// the History panel and write the autosave, then dropped — nothing
    /// yet needs it kept alive the way painting needs `layers`). This is
    /// what [`Self::active_layer`]/[`LayerTree::surface_id`] read to find
    /// somewhere for the Brush tool to actually paint.
    layers: aurora_doc::LayerTree,
    /// The layer the Brush tool paints into, if any — the topmost pixel
    /// layer of `layers` at construction time
    /// ([`topmost_pixel_layer`]), real-time-changeable now by clicking a
    /// row in the Layers panel ([`Self::layer_rows`],
    /// [`Self::handle_pointer_pressed`]) — the same click-to-select gap
    /// `aurora_ui::tool`'s own doc comment used to name for the Move
    /// tool, now closed for layer selection (Move's own blocker is
    /// unrelated and remains open). `None` for a document with no pixel
    /// layer at all, or once one is clicked that turns out to be a
    /// group (groups are never inserted into `layer_rows` at all, so
    /// this can't actually happen via a click — only via never having a
    /// pixel layer to begin with).
    active_layer: Option<aurora_doc::LayerId>,
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
    /// The pointer's last known position, in the *window's* own logical
    /// space (already DPI-adjusted — see [`logical_point`]) — `None`
    /// before the first `CursorMoved`, or after `CursorLeft`.
    pointer_position: Option<(f32, f32)>,
    /// An in-progress pointer drag (Pan, Marquee Select, or Brush), if
    /// any — `None` is "not dragging," the same "no separate flag" shape
    /// `command_palette`/`crash_recovery_dialog` above already use.
    drag: Option<Drag>,
    /// The native menu bar — macOS only, see this crate's own "native
    /// menu bar" section for why Windows/Linux aren't included. Built
    /// in [`App::new`] (no window needed); attached to the real
    /// application menu bar in `resumed` (`Menu::init_for_nsapp`).
    #[cfg(target_os = "macos")]
    menu: muda::Menu,
    /// The window's background clear colour, resolved from
    /// `design/themes/dark.toml`'s `surface.app` token
    /// (`load_background_color`) — invariant §7.3.10 (no hardcoded
    /// style values) applied to the one thing this crate draws so far.
    background: wgpu::Color,
    /// Set when a step that can't be retried fails (window/device/surface
    /// creation) — `run` turns this into a nonzero exit, distinguishing
    /// it from the ordinary, successful case of the user closing the
    /// window.
    failed: bool,
}

impl App {
    #[must_use]
    fn new(
        proxy: EventLoopProxy<accesskit_winit::Event>,
        background: wgpu::Color,
        scales: &Scales,
        marker_path: PathBuf,
        had_previous_marker: bool,
        autosave_path: &Path,
    ) -> Self {
        let mut workspace = aurora_ui::build_workspace();
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
            scales,
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
        let tile_store = open_tile_store();

        let mut focus = FocusManager::default();
        let mut crash_recovery_dialog = None;
        if had_previous_marker {
            open_crash_recovery_dialog(
                &mut workspace,
                &mut focus,
                &mut crash_recovery_dialog,
                scales,
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
            scale_factor: 1.0,
            clipboard: SystemClipboard::new(),
            file_dialog: SystemFileDialog,
            pending_open_path: None,
            tool: aurora_ui::Tool::default(),
            canvas_view: aurora_ui::CanvasView::default(),
            selection: aurora_doc::SelectionSet::new(),
            layers,
            active_layer,
            layer_rows,
            tile_store,
            residency: None,
            canvas_pipeline: None,
            pointer_position: None,
            drag: None,
            #[cfg(target_os = "macos")]
            menu: build_menu(),
            background,
            failed: false,
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
    /// real platform input into.
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
            &self.shortcuts,
            self.modifiers,
            key,
            event.text.as_deref(),
            &mut self.clipboard,
            &mut self.file_dialog,
        );
        if let Some(path) = picked {
            tracing::info!(path = %path.display(), "file chosen (no import pipeline yet)");
            self.pending_open_path = Some(path);
        }
        self.push_accessibility();
    }

    /// Records a real, native `WindowEvent::DroppedFile` — the same
    /// "detect a real signal, defer the action" honesty
    /// [`COMMAND_FILE_OPEN`] already applies: there is no import
    /// pipeline yet for a dropped path to feed into, so this only
    /// records it (reusing the exact same [`Self::pending_open_path`]
    /// slot the palette's "Open File…" command already writes to — a
    /// dropped file and a chosen one are the same kind of "the user
    /// wants to open this" signal, whichever route it arrived by).
    fn handle_dropped_file(&mut self, path: PathBuf) {
        tracing::info!(path = %path.display(), "file dropped (no import pipeline yet)");
        self.pending_open_path = Some(path);
    }

    /// A real `WindowEvent::CursorMoved`: updates the tracked pointer
    /// position and, if a drag is in progress, advances it
    /// ([`continue_drag`]), painting any dab positions it returns
    /// ([`Self::paint_dab`]) — empty for every drag but `Brush`.
    fn handle_pointer_moved(&mut self, physical_position: (f64, f64)) {
        let position = logical_point(physical_position, self.scale_factor);
        self.pointer_position = Some(position);
        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };
        if let Some(drag) = self.drag.as_mut() {
            let dabs = continue_drag(
                drag,
                canvas_point,
                &mut self.canvas_view,
                &mut self.selection,
            );
            for doc_point in dabs {
                self.paint_dab(doc_point);
            }
        }
    }

    /// A real `WindowEvent::MouseInput { state: Pressed, .. }`: either
    /// performs the active Zoom tool's click-to-zoom
    /// ([`handle_zoom_tool_click`]), or starts a drag ([`begin_drag`]) —
    /// never both for the same press. A fresh `Brush` drag paints its
    /// own starting point immediately ([`Self::paint_dab`]), so a plain
    /// click (no drag at all) still paints something.
    fn handle_pointer_pressed(&mut self, button: winit::event::MouseButton) {
        let Some(button) = translate_pointer_button(button) else {
            return;
        };
        let Some(position) = self.pointer_position else {
            return;
        };

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
            self.push_accessibility();
            return;
        }

        let Some(canvas_point) = pointer_in_canvas(&self.workspace, position) else {
            return;
        };

        if self.tool == aurora_ui::Tool::Zoom && button == PointerButton::Primary {
            handle_zoom_tool_click(&mut self.canvas_view, canvas_point, self.modifiers);
            return;
        }
        self.drag = begin_drag(self.tool, button, canvas_point, &self.canvas_view);
        if let Some(Drag::Brush { last_doc, .. }) = self.drag {
            self.paint_dab(last_doc);
        }
    }

    /// Stamps one brush dab at `doc_point` (document space) into the
    /// active layer's own surface in the live tile store —
    /// [`aurora_brush::stamp_dab`], via [`layer_local_point`] for the
    /// document-space -> layer-local conversion `aurora_tile::TileStore`
    /// needs. A silent no-op if there's no live store
    /// ([`Self::tile_store`] failed to open), no active layer
    /// ([`Self::active_layer`] is `None`), or that layer isn't (or is no
    /// longer) a pixel layer — the same "detect a real signal, do
    /// nothing more" honesty `pending_open_path` already uses for an
    /// unbuilt pipeline. A real, logged failure ([`aurora_tile::TileError`],
    /// e.g. the scratch disk failing mid-session) is worth a warning,
    /// though, unlike those absent-precondition cases.
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
        if let Err(err) = aurora_brush::stamp_dab(store, surface, local, BRUSH_RADIUS, BRUSH_COLOUR)
        {
            tracing::warn!(?err, "failed to stamp a brush dab");
        }
    }

    /// A real `WindowEvent::MouseInput { state: Released, .. }`: ends
    /// whatever drag is in progress. Any button release ends it — this
    /// crate has no multi-touch/multi-pointer support to disambiguate
    /// which button a drag actually started with, and a single active
    /// window only ever has one drag in progress at a time.
    fn handle_pointer_released(&mut self) {
        self.drag = None;
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
        if let Some(path) = picked {
            tracing::info!(path = %path.display(), "file chosen via native menu (no import pipeline yet)");
            self.pending_open_path = Some(path);
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
    /// if a live document, tile store, and GPU atlas all exist — syncs
    /// the atlas from whatever's actually in the store
    /// ([`Self::active_layer`]'s own surface) and draws it within the
    /// canvas dock area's own viewport, in the same pass as the clear.
    /// Real widget/panel content beyond that is still separate,
    /// still-open M1.8 work.
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

                // Sync before drawing, so this frame shows the latest
                // painted pixels rather than lagging one frame behind.
                if let Some(residency) = self.residency.as_mut() {
                    if let Some(canvas_size) =
                        canvas_area_physical_size(&self.workspace, self.scale_factor)
                    {
                        residency.set_origin(
                            gpu.queue(),
                            tile_origin_for_view(&self.canvas_view),
                            canvas_size,
                        );
                    }
                    if let (Some(layer_id), Some(store)) =
                        (self.active_layer, self.tile_store.as_mut())
                        && let Some(surface_id) = self.layers.surface_id(layer_id)
                    {
                        let _ = residency.sync(gpu.queue(), store, surface_id, false, usize::MAX);
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

        match event {
            WindowEvent::CloseRequested => {
                // A clean shutdown -- clear this run's own marker so the
                // *next* run's `previous_session_left_a_marker` reads
                // false, not true (see this crate's own "crash
                // recovery" section).
                clear_session_marker(&self.marker_path);
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
            WindowEvent::DroppedFile(path) => self.handle_dropped_file(path),
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

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        // muda's own events arrive on a plain channel, not through this
        // crate's `accesskit_winit::Event` user-event type (the two
        // don't share one enum -- restructuring the accessibility
        // integration around a combined event type is a bigger, separate
        // change) -- polled here since `about_to_wait` already runs on
        // every loop iteration.
        #[cfg(target_os = "macos")]
        while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
            self.handle_menu_event(&event);
        }

        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
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
    let background = load_background_color()?;
    let scales = load_scales()?;
    let marker_path = marker_path();
    // Checked *before* writing this run's own marker below -- otherwise
    // every run would see its own, brand-new marker and think the
    // *previous* run crashed.
    let had_previous_marker = previous_session_left_a_marker(&marker_path);
    write_session_marker(&marker_path);
    let autosave_path = autosave_path();

    let event_loop = EventLoop::<accesskit_winit::Event>::with_user_event()
        .build()
        .map_err(|err| anyhow::anyhow!("event loop creation failed: {err}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let mut app = App::new(
        proxy,
        background,
        &scales,
        marker_path,
        had_previous_marker,
        &autosave_path,
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
        AppCommand, COMMAND_FILE_OPEN, COMMAND_FOCUS_HISTORY, COMMAND_FOCUS_LAYERS,
        COMMAND_FOCUS_PROPERTIES, CRASH_RECOVERY_CONTINUE, ClipboardAccess, Drag, FileDialogAccess,
        Key, KeyChord, Modifiers, NamedKey, PointerButton, activate_command, apply_scroll_zoom,
        autosave_path, begin_drag, canvas_area_physical_rect, canvas_area_physical_size,
        clear_session_marker, close_command_palette, close_crash_recovery_dialog, continue_drag,
        crash_recovery_dialog_message, default_shortcuts, demo_document, handle_dialog_key,
        handle_key, handle_palette_key, handle_zoom_tool_click, layer_local_point,
        load_background_color, load_scales, logical_point, logical_size, open_command_palette,
        open_crash_recovery_dialog, open_tile_store, pointer_in_canvas,
        previous_session_left_a_marker, recover_document, run_command, select_layer,
        tile_origin_for_view, tile_store_scratch_dir, toggle_command_palette, topmost_pixel_layer,
        translate_key, translate_modifiers, translate_pointer_button, write_autosave,
        write_session_marker, zoom_steps_for_scroll,
    };
    use aurora_doc::SelectionSet;
    use aurora_ui::{CanvasView, Tool};
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
    /// native picker.
    #[derive(Debug, Default)]
    struct FakeFileDialog {
        next_pick: Option<PathBuf>,
    }

    impl FileDialogAccess for FakeFileDialog {
        fn pick_file(&mut self) -> Option<PathBuf> {
            self.next_pick.take()
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
        let color = match load_background_color() {
            Ok(color) => color,
            Err(err) => unreachable!("the checked-in design files must parse: {err}"),
        };
        // Exact-literal comparison, not accumulated computation noise --
        // `load_background_color` sets `a: 1.0` directly, never through
        // float math -- same reasoning `aurora-color`'s own tests
        // already document for their float_cmp allows.
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

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.layers.root));
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.properties.root));
        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            AppCommand::FocusNext,
        );
        assert_eq!(focus.focused(), Some(workspace.history.root));

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
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

        run_command(
            &mut workspace,
            &mut focus,
            &mut palette,
            &mut tool,
            AppCommand::SelectTool(Tool::Pan),
        );
        assert_eq!(tool, Tool::Pan);
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
        assert_eq!(state.results().len(), 1);
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
        let shortcuts = default_shortcuts();
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
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
        let shortcuts = default_shortcuts();
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
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
    fn handle_key_ignores_an_unbound_chord() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut dialog = None;
        let mut palette = None;
        let mut tool = Tool::default();
        let shortcuts = default_shortcuts();
        // 'q' is deliberately not one of `default_shortcuts`' own
        // tool-switch letters (v/m/z/h/i) or anything else bound.
        handle_key(
            &mut workspace,
            &mut focus,
            &mut dialog,
            &mut palette,
            &mut tool,
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

        assert_eq!(picked, Some(PathBuf::from("/tmp/example.psd")));
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
    fn activate_command_returns_the_picked_path_for_file_open() {
        let mut workspace = aurora_ui::build_workspace();
        let mut focus = FocusManager::default();
        let mut file_dialog = FakeFileDialog {
            next_pick: Some(PathBuf::from("/tmp/example.psd")),
        };

        let picked = activate_command(
            &mut workspace,
            &mut focus,
            COMMAND_FILE_OPEN,
            &mut file_dialog,
        );

        assert_eq!(picked, Some(PathBuf::from("/tmp/example.psd")));
        assert_eq!(
            focus.focused(),
            None,
            "COMMAND_FILE_OPEN has no focus target of its own"
        );
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
        // A 1000x800 viewport, 3:1 canvas:rail flex ratio -- canvas area
        // is the 750x800 rect at the window's own origin (see
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
            tile_origin_for_view(&view),
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
            tile_origin_for_view(&view),
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
            tile_origin_for_view(&view),
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
            assert_eq!(
                begin_drag(tool, PointerButton::Middle, (10.0, 20.0), &view),
                Some(Drag::Pan {
                    last_screen: (10.0, 20.0)
                }),
                "tool {tool:?} must still pan on a middle-button drag"
            );
        }
    }

    #[test]
    fn begin_drag_with_pan_tool_and_primary_button_pans() {
        let view = CanvasView::new();
        assert_eq!(
            begin_drag(Tool::Pan, PointerButton::Primary, (5.0, 5.0), &view),
            Some(Drag::Pan {
                last_screen: (5.0, 5.0)
            })
        );
    }

    #[test]
    fn begin_drag_with_marquee_tool_and_primary_button_starts_a_marquee_in_document_space() {
        let mut view = CanvasView::new();
        view.zoom_at((0.0, 0.0), 2.0);
        assert_eq!(
            begin_drag(
                Tool::MarqueeSelect,
                PointerButton::Primary,
                (20.0, 40.0),
                &view
            ),
            Some(Drag::Marquee {
                start_doc: (10.0, 20.0)
            })
        );
    }

    #[test]
    fn begin_drag_with_move_or_eyedropper_tool_and_primary_button_does_nothing() {
        let view = CanvasView::new();
        assert_eq!(
            begin_drag(Tool::Move, PointerButton::Primary, (1.0, 1.0), &view),
            None
        );
        assert_eq!(
            begin_drag(Tool::Eyedropper, PointerButton::Primary, (1.0, 1.0), &view),
            None
        );
    }

    #[test]
    fn begin_drag_with_brush_tool_and_primary_button_starts_a_brush_drag_at_zero_carry() {
        let view = CanvasView::new();
        assert_eq!(
            begin_drag(Tool::Brush, PointerButton::Primary, (10.0, 20.0), &view),
            Some(Drag::Brush {
                last_doc: (10.0, 20.0),
                carry: 0.0
            })
        );
    }

    #[test]
    fn begin_drag_with_zoom_tool_and_secondary_button_does_nothing() {
        let view = CanvasView::new();
        assert_eq!(
            begin_drag(Tool::Zoom, PointerButton::Secondary, (1.0, 1.0), &view),
            None
        );
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
        assert_eq!(
            drag,
            Drag::Pan {
                last_screen: (15.0, 8.0)
            },
            "must advance its own last-known point for the next event"
        );
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
        };
        // radius 24, DEFAULT_SPACING 0.25 -> step 6; a 12-unit segment
        // lands dabs at 6 and 12 (the segment's own start, 0, is not
        // re-emitted -- it was already painted by whatever started the
        // drag or the previous event).
        let dabs = continue_drag(&mut drag, (12.0, 0.0), &mut view, &mut selection);
        assert_eq!(dabs, vec![(6.0, 0.0), (12.0, 0.0)]);
        assert_eq!(
            drag,
            Drag::Brush {
                last_doc: (12.0, 0.0),
                carry: 0.0
            },
            "must advance its own last-known point and carry for the next event"
        );
    }

    #[test]
    fn continue_drag_brush_carries_spacing_across_multiple_short_move_events() {
        let mut view = CanvasView::new();
        let mut selection = SelectionSet::new();
        let mut drag = Drag::Brush {
            last_doc: (0.0, 0.0),
            carry: 0.0,
        };
        let first = continue_drag(&mut drag, (3.0, 0.0), &mut view, &mut selection);
        // Segment shorter than one step (6): no new dab yet, but the 3
        // units already travelled must carry forward, not reset to 0 --
        // the exact bug a fresh `dabs_along_path` call each event would
        // have (see `continue_drag`'s own doc comment).
        assert_eq!(first, Vec::new());
        assert_eq!(
            drag,
            Drag::Brush {
                last_doc: (3.0, 0.0),
                carry: 3.0
            }
        );

        // Second event: 4 more units. 3 (carried) + 4 = 7 >= step (6),
        // so exactly one dab lands (at the 6-unit mark, i.e. 3 units
        // into *this* segment) -- proving the carry from the first,
        // sub-step event was not lost.
        let second = continue_drag(&mut drag, (7.0, 0.0), &mut view, &mut selection);
        assert_eq!(second, vec![(6.0, 0.0)]);
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
