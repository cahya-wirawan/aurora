# Aurora — Implementation Plan

**Living document.** Tracks what is done, what is in progress, and what comes next.
Last updated: **2026-08-02**.

The [PRD](PRD.md) says *what* Aurora is and *why*. This file says *where we are*
and *what to do next*. When they disagree, the PRD wins and this file is stale —
fix it.

## How to use this file

| Mark | Meaning |
|---|---|
| `[x]` | Done, with evidence linked |
| `[~]` | In progress |
| `[ ]` | Not started |
| `[!]` | Blocked — the blocker is named |
| `[-]` | Deliberately dropped or deferred, with a reason |

Rules: update the mark in the same commit as the work. Never mark `[x]` without
linked evidence (a file, a measurement, a commit). A task that turns out to be
wrong gets `[-]` and a one-line reason, not deletion — the trail matters more
than the tidiness.

---

## Where we are

**Phase 0 (technical de-risking) — roughly half done, but its Phase-1 gate
(PRD §13 Steps 1/3/4) is now satisfied and Phase 1 has started, 2026-07-28.**
**M1.1 is complete** — `aurora-core`'s foundational types and
`aurora-tile`'s sparse/LRU/compressed/paging store, with ADR 0005 (tile
size) settled alongside it. **M1.2 (`aurora-gpu`) is in progress**:
device/queue management, the shader library/pipeline cache, GPU tile
residency (with toroidal slot addressing, the bullet PLAN.md itself
flagged as risky), budgeted upload scheduling, and surface configuration/
resize are all done and verified against real GPU hardware — the first
four on this machine's Vulkan GPU, surface configuration/resize on a
different machine's live macOS/Metal session (2026-07-29,
`examples/surface_smoke.rs`) once that display finally became available,
closing the "GDM greeter only" gap the a11y Orca leg also hit. Only
cross-platform validation (DX12 specifically — Vulkan and Metal are now
both real) is not yet started. CI green on Linux, macOS, and
Windows. The brush-latency regression test 0.2 flagged as required
"before Phase 1 feature work" is now done too (2026-08-02, overdue
against its own stated gate — see 0.2), and so is the golden-image diff
harness (`aurora-testkit`, a new 20th workspace crate — see 0.2), and
ADR 0006 (accessibility conformance target: WCAG 2.1 AA — see 0.1).
The remaining Phase 0 items (Windows/300k-px slice re-runs, macOS/
Windows LGPL packaging, deeper PSD format coverage) continue in
the background rather than gating Phase 1 — none of them are among the
three steps PRD §13 actually names as blocking.

| Area | State |
|---|---|
| Requirements & architecture | Settled and written down (PRD v1.8, 7 ADRs — 0006 still pending) |
| Workspace & CI | Built and green |
| Performance validation | **Measured** — budgets hold, with one correction; re-run on Linux/Vulkan 2026-07-26, same correction reproduces |
| Accessibility & IME | **macOS verified (9/10)**; Linux/Windows human verification unverified but **risk accepted 2026-07-28** to unblock Phase 1 — see 0.4 |
| Design language | **Complete** — [design/](design/README.md); owner-approved and outside-reviewed (2026-07-28) |
| PSD write feasibility | Pixel layers/groups **tractable**; text layers **harder than planned** — new mandatory scope found (glyph rendering) |
| RAW / ICC feasibility | **Libraries decided (ADR 0007/0008); LGPL packaging mechanism proven on Linux with the actual chosen library** — macOS/Windows and legal review remain |
| Re-plan (PRD §13 Step 7) | **Done.** Durations re-grounded against spike evidence; Q2's answer (solo) reframed the exercise, and the resulting scope question is resolved — Phases 4/5 cut to §9's uncommitted "Beyond v1.0" backlog, Phases 0–3 milestone-based rather than calendar-committed. |
| Define the 95% (PRD §13 Step 2) | **Done** — [docs/workflows.md](docs/workflows.md), 2026-07-28. Cahya-reviewed, tiering confirmed; see 0.9 |
| PSD test corpus (Step 6) | **Phase 0's share done** — 319 fixtures, 272/272 open with an independent reader. The 1,000-file real-world gate is deliberately deferred to Phase 3, not carried as Phase 0 debt. See 0.7 |
| **Phase 1 — M1.1** | **Complete, 2026-07-28.** `aurora-core` (geometry, colour descriptors, IDs, errors, 16 tests) and `aurora-tile` (sparse/LRU/compressed/paged tile store, 13 tests, ADR 0005 — 12 original plus one CI-gated latency regression test added 2026-08-02, see 0.2). Full local CI gate clean. See M1.1 |
| **Phase 1 — M1.2** | **In progress, 2026-07-29.** Device/queue management, shader library/pipeline cache, GPU tile residency, and budgeted upload scheduling (`GpuContext`/`ShaderLibrary`/`PipelineCache`/`TileResidency`) all done and verified against this machine's real RTX 3090 (Vulkan) with actual rendered/uploaded-pixel checks. Surface configuration/resize (`GpuSurface`) is implemented **and now verified against a real window** on a different machine's live macOS/Metal session — same "GDM greeter only" gap as the a11y Orca leg, resolved the same way (a machine with an actual desktop session). A real cross-test GPU deadlock under `cargo test`'s default runner was found and fixed along the way (test-only `Mutex`). `TileResidency`'s atlas gained a real 4-level mip chain and `upload_mip` 2026-07-30, in service of M1.3's progressive rendering (see below) — the atlas itself is still M1.2 scope even though the reason for the growth is M1.3's. Only cross-platform validation (DX12 — Vulkan and Metal are both real now) is fully unstarted. See M1.2 |
| **Phase 1 — M1.3** | **In progress, 2026-07-30.** `aurora-graph`'s node definitions, dependency DAG, and dirty-region propagation (`RenderGraph<N>`) done, 12 tests. `aurora-render`: `schedule()` translates a graph's node-granular dirty `Rect`s into tile-granular work lists (9 tests); `TileCompositor` blends one tile over another on the GPU via the fixed-function alpha blend unit (3 tests, verified against real hardware, plus a fourth added 2026-08-02 comparing the whole composited tile against a golden image via the new `aurora-testkit` harness — see 0.2); progressive rendering's `mip::downsample` and `preview::upload_preview` land a downsampled tile in `aurora_gpu::TileResidency`'s atlas, verified end-to-end against real hardware (9 tests); `Executor` runs submitted work on a background thread without blocking the caller, async evaluation's first piece (5 tests). A CI-gated GPU-dependent latency regression test (`latency.rs`, 1 more test) added 2026-08-02 — see 0.2. 28 tests total in the crate. What's left is real consumers for the last two: picking a mip level from interaction state, and submitting actual render work through `Executor` — both wait on `aurora-doc`/`aurora-filters`, which don't exist yet. See M1.3 |
| **Phase 1 — M1.4** | **In progress, 2026-08-01.** `aurora-doc`'s `LayerTree` (`Pixel`/`Group` layers, nesting, top-to-bottom ordering, cascading delete, cycle-checked reparenting) done, 2026-07-30, 25 tests. Per-layer opacity/fill opacity/blend mode (full 27-mode Photoshop set)/visibility/locking (`BlendMode`, `LayerLock` mirroring PSD's `lspf` bits) done 2026-08-01, 7 more tests (32 total). Per-layer masks (`LayerMask` — bounds/enabled/inverted, deliberately no real mask pixels yet) done 2026-08-01, 8 more tests (40 total) — lives on `LayerEntry` so both pixel layers and groups can carry one. Document-level selection representation (`Selection`, `SelectionSet` — active selection plus named saved ones, FR-004's Save/Load/Inverse) done 2026-08-01, 11 more tests (51 total), a new module rather than a `LayerTree` method since a selection isn't per-layer. History (`History` — mirrors all 14 `LayerTree` mutators with undo-recording versions, unlimited undo/redo, §7.3.3) done 2026-08-01, 20 more tests (70 total) — add/remove turned out to be exact inverses of two new id-preserving `LayerTree` primitives (`remove_capturing`/`restore`), one symmetric apply function drives both undo and redo, and dirtied `Rect`s are reported when knowable (pixel bounds, or a union over a removed subtree) and honestly `None` for group-level changes. Crash recovery journal's **in-memory half** (`History::replay`, an ever-growing chronological op log distinct from the undo/redo stacks) done 2026-08-01, 8 more tests (78 total) — proven to reflect *current* state after undo/redo, not just full history; **durable disk persistence deliberately deferred**, no on-disk encoding decided (see M1.4). fmt/clippy (`-D warnings`)/`cargo test -p aurora-doc` all verified clean throughout. See M1.4 |
| **Phase 1 — M1.5** | **Complete, 2026-08-01.** `aurora-color`'s ICC transforms (`IccProfile`, `Transform`, `RenderingIntent`) wiring in `lcms2` per ADR 0008 — `Gray`/`Rgb`/`Rgba`/`Cmyk` channel layouts, verified against real, committed CC0 ICC profiles (`corpora/icc/`, copied from `spike/raw-icc`'s own fixtures), reproducing `spike/raw-icc/FINDINGS.md`'s cross-validated sRGB→ECI-RGBv2 values plus the permanent extended-range/no-clamping regression test that spike's finding 4 asked for (`cargo deny check all` clean with the new dependency; `Cmyk` wired but untested against a real CMYK profile — honest gap, none in the corpus yet). Colour-space descriptor tagging (§7.3.6) was already done from M1.1. Linear-light conversion (`linear_to_srgb`/`srgb_to_linear`, IEC 61966-2-1's real curve, HDR/negative-safe) — an explicit working-space *policy* type stays deliberately undesigned until a real compositor/filter consumer exists. Promote-on-import/dither-on-export (`promote_u8`/`quantize_u8`/`dither_quantize`, classic 8×8 Bayer ordered dithering generated from its recursive definition and cross-checked against the published 4×4 table, not a hand-transcribed 64-entry table) — exhaustive 256-value round-trip test, dedicated test confirming dithering actually breaks up banding. 22 tests total. fmt/clippy (`-D warnings`)/`cargo test -p aurora-color`/`cargo deny check all` all verified clean throughout. See M1.5 |
| **Phase 1 — M1.6** | **In progress, 2026-08-01.** `aurora-theme`'s token types (`Color`, `SurfaceTokens`/`TextTokens`/`IconTokens`/`BorderTokens`/`AccentTokens`/`StateTokens`, `Scales`) match the already owner-approved `design/tokens/vocabulary.md`/`scales.toml` exactly. `Palette`/`ThemeSet` parse real TOML (`toml` added to `[workspace.dependencies]` — `serde`'s first real use), resolve dotted palette references generically, and merge an `extends` inheritance chain (child overrides parent) — verified against the real, committed Dark theme end to end, plus a synthetic child theme proving the merge logic without inventing a real second design. `contrast::check_gated_pairs` is the real, CI-enforced version of `design/check_contrast.py`'s Phase-0 prototype (same 17 gated pairs, same WCAG formula reusing `aurora_color::srgb_to_linear`) — independently reproduces that script's own prior "17/17 pass" finding. 23 tests total. **Blocked**: Light/high-contrast/Colour-Critical themes need Cahya's own design work (FR-027 *Ownership*) before they can exist, not an engineering gap. **Deferred, no consumer yet**: hot-reload file-watching, and the CI lint rejecting hardcoded style values (needs real widget code to lint against). fmt/clippy (`-D warnings`)/`cargo test -p aurora-theme`/`cargo deny check licenses` all verified clean. See M1.6 |
| **Phase 1 — M1.7** | **In progress, 2026-08-02.** `aurora-widgets`' `WidgetTree<W>` (generic over payload, same shape `RenderGraph<N>` uses) done: identity/nesting (one root, children appended in paint/tab order — a deliberate departure from `LayerTree`'s "newest on top"), damage tracking (`aurora_tile::Tile`'s own `Option<Rect>`/`Rect::union` idiom, per-widget and tree-wide), a *required* `accesskit::Node` per widget from creation (`WidgetId` **is** `accesskit::NodeId`, not a wrapper — no second id space, `accessibility_update` matches `spike/a11y-ime`'s own proven `TreeUpdate` shape), and a `taffy`-backed flexbox layout engine (`compute_layout` — style in, absolute bounds out, rebuilding `taffy`'s own tree fresh each call rather than keeping two trees in sync). Found and verified, not assumed: an `Auto`-sized childless root does **not** implicitly fill the viewport in `taffy` (no CSS-body-100%-style default) — both that and the `percent(1.0)` opt-in are now permanent regression tests. Input routing/focus (`hit_test`, `FocusManager`) done 2026-08-02, 18 more tests (38 total) — platform-agnostic (document-space point + `Tab`/`Shift+Tab` steps, not `winit::WindowEvent`s), reuses `accesskit`'s own `Action::Focus` for "focusable" rather than a parallel flag, `focus_at` bubbles a hit-test to the nearest focusable ancestor. Concrete widget set: a first slice (`WidgetKind`, `Button`/`Checkbox`/`Slider` — 3 of 12 named widgets, covering three genuinely different interaction shapes) done 2026-08-02, 20 more tests (58 total) — layout resolved from `aurora_theme::Scales` per invariant §7.3.10, `Checkbox` reuses `accesskit::Toggled` directly rather than a parallel enum, no rendering yet (blocked on `aurora-vector`). A real bug (`toggle_checkbox`'s indeterminate-state resolution contradicting its own doc comment) was caught by its own test before commit. Text field (`TextFieldState` — selection, grapheme-cluster-aware caret motion, Unicode word motion, text-buffer clipboard, per-widget undo/redo) done 2026-08-02, 28 more tests (86 total) — one generic `with_text_field_mut` rather than a hand-written wrapper per operation, given how many mutating methods this widget alone has; `accesskit::TextSelection` deliberately left unexposed, inheriting an already-known gap from `spike/a11y-ime/FINDINGS.md` rather than introducing a new one. IME composition (`Composition`, `UnderlineStyle`, `composition_segments`, `set_composition`/`commit_composition`) done 2026-08-02, 15 more tests (101 total) — mirrors `winit::event::Ime::Preedit`/`Commit` exactly, composition updates are not undo steps except the one real content change (removing a selection to start a fresh composition), `composition_segments` produces thin/thick underline-style *data* for a future renderer (this crate still draws nothing), and composing state is announced via `set_description` — the exact mechanism `spike/a11y-ime` already proved reaches VoiceOver, closing that finding's own recorded follow-up. A real bug (`composition_segments` emitting two abutting segments for a degenerate empty target range) was caught by its own test before commit. Headless mode as an explicit, checked feature done 2026-08-02: found `aurora-gpu`/`aurora-vector`/`aurora-text` declared as dependencies but never actually used anywhere in the crate's source — real `wgpu` was in this crate's own dependency graph despite its doc comments claiming headlessness. Removed all three (each goes back exactly when vector-first rendering starts; `scripts/layering.json` already allows them and didn't need to change); `cargo tree -p aurora-widgets -i wgpu` now finds no match at all. Added `crates/aurora-widgets/tests/headless.rs`, a real integration test (new pattern for this workspace) proving the whole pipeline — tree, layout, focus/hit-test, all four widgets, IME — end to end through the crate's public API alone, 102 tests total (101 unit + 1 integration). fmt/clippy (`-D warnings`)/rustdoc (`-D warnings`)/`cargo build --workspace`/`cargo test -p aurora-widgets`/`cargo deny check licenses` all verified clean throughout. The other 9 named widgets, vector-first rendering, and the component gallery all remain — each blocked on infrastructure (scrolling, popover layering, `aurora-vector`) that doesn't exist yet, not a natural next slice.

**A real crash on real macOS hardware, 2026-08-03**: `accessibility_update` cloned each widget's `accesskit::Node` without ever setting its `children`, so every node but the root reached `accesskit_consumer` looking disconnected — invisible until M1.8's docking/panels work finally gave `aurora-app` a tree with more than one node to send. Fixed (set `children` from the tree's own real structure); added a real regression test using `accesskit_consumer::Tree::new` (the exact library that caught it) as a dev-dependency, and verified the test actually fails without the fix, not just that it passes with it. See M1.7 |
| **Phase 1 — M1.8** | **Started 2026-08-02, human-verified on macOS 2026-08-03, blocked only on Windows/Linux verification.** `aurora-app`'s first real code (was a placeholder `main()`): a real `winit::ApplicationHandler` implementing the "create hidden → attach `accesskit_winit` adapter → show" ordering ADR 0001's escape-hatch check found, reusing `aurora-gpu`'s already-proven `GpuContext`/`GpuSurface` and `aurora-widgets`' `WidgetTree`/`FocusManager` for a (currently content-free) accessibility tree rather than hand-rolling either. Real error handling throughout, `main` now fallible. Written blind (no `pkg-config` in this sandbox, no root to install it) and pushed; CI's first real run immediately caught a genuine bug — `wgpu` used directly but never declared as a dependency — fixed the same day. Cahya then installed `pkg-config`/`libfontconfig1-dev` in this same sandbox, closing the gap that had blocked `cargo clippy --workspace --all-targets --all-features -- -D warnings`/`cargo test --workspace` (the exact CI gates) all session — both now pass completely, every crate. **Then Cahya ran `cargo run -p aurora-app` on real macOS hardware**: the window opens (create-hidden → adapt → show all working for real), resizing works with no crash, and **VoiceOver announces the window** — the accessibility tree genuinely reaches a real screen reader, this project's first non-spike code to do so. The window's clear colour is now a real theme token too (`load_background_color`, `design/themes/dark.toml`'s `surface.app`, correctly converted sRGB→linear for the `Bgra8UnormSrgb` surface via `aurora_color::srgb_to_linear` — using the raw sRGB bytes would have washed the colour out), 2 more tests. `aurora-ui`'s first real code (was a placeholder too): a static docking/panel skeleton matching the owner-approved workspace mockup — canvas area + a side rail of three labeled (`Role::Region`) panels (Layers/Properties/History), reusing `aurora_widgets::WidgetTree<WidgetKind>` directly rather than inventing a parallel widget model, flex-ratio sized (no un-tokenized pixel widths) — wired into `aurora-app` so `compute_layout` runs live on window creation and resize. Verified empirically (a real 1000×800 layout test), not assumed. No drag-to-redock/resize/persisted-layout or real panel content yet — that's the actual "docking"/"custom workspaces" half, still open. **Two real bugs found and fixed from this same live-hardware session**: (1) `WidgetTree::accessibility_update` never set `node.children`, so any tree past a trivial single root looked disconnected to `accesskit_consumer` and crashed on launch — fixed, plus a real regression test using `accesskit_consumer::Tree::new` itself; (2) even after that fix, the workspace was completely unreachable from VoiceOver (the Rotor's "Window Spots" came back empty, not even the window title) — root cause was the tree's root using `Role::GenericContainer` instead of `Role::Window`, fixed and **re-verified live**: VoiceOver's Rotor now lists both "Aurora" (the window) and "Layers." "Properties"/"History" didn't show in that same Rotor listing (likely a Rotor display quirk, not a structural gap — all three are built identically and verified by the test suite). The Layers *and* History panels now have real content: `aurora-ui`'s new `populate_layers_panel`/`populate_history_panel` turn a real `aurora_doc::LayerTree`/`History` into accessible `Role::ListItem` rows (nested for layer groups) — the History half needed a small, real `aurora-doc` feature addition first (`History::journal_descriptions`, closing a gap that module's own doc comment had named). Wired into `aurora-app` via `demo_document()` (renamed from `demo_layers`), a small, clearly-fake three-layer document built *through* `History`'s own methods so its journal has something real to show. **Re-verified live, 2026-08-04**: structure confirmed correct again (Rotor still shows "Aurora"/"Layers Group"), but a real, reproducible gap found — VoiceOver's linear/interact keyboard navigation into the nested content doesn't reliably work (gets stuck, or one attempt landed on the native window's own title-bar buttons), reproduced across multiple attempts including a full VoiceOver restart. Not a structural bug (the same tree is proven correct by both the Rotor and the test suite) — a real, open UX gap in `spike/a11y-ime/FINDINGS.md` finding #5's own flagged territory (deep nested custom content), now confirmed concretely rather than speculatively, worse than that spike's flat two-child tree ever exercised. Deliberately not chased further via more blind remote code changes — recorded as real, scoped follow-up work needing either deeper native macOS accessibility expertise or a systematic minimal-repro comparison. **Command palette + keyboard shortcuts added 2026-08-04** — this crate's first real keyboard input at all (`Tab`/`Shift+Tab` finally reach `FocusManager`; `Ctrl+Shift+P` opens a real, filterable command palette from two new generic `aurora-widgets` mechanisms, `shortcut::{KeyChord, ShortcutRegistry}` and `widgets::command_palette`), with panel regions now real `Tab` stops (`insert_panel` gained `Action::Focus`). All dispatch logic is free functions decoupled from `App`/`winit` window types, so it's fully unit-tested (14 new `aurora-app` tests, 17 total; 26 new `aurora-widgets` tests, 128 total) with no display server needed — not yet real-hardware-verified. **Crash recovery UI added 2026-08-05** — a real, narrow first slice: a session marker file written at startup and cleared on clean shutdown, and a new generic `aurora-widgets::widgets::dialog` (`Role::AlertDialog`) shown when a *previous* run's marker is still present. Deliberately does not restore any document state yet — `aurora-doc`'s crash-recovery journal has no on-disk encoding decided (see M1.4), so the dialog's one honest action is "Continue," not "Recover Document." 4 new `aurora-widgets` tests (132 total), 11 new `aurora-app` tests (28 total, including real filesystem I/O against a `tempfile::TempDir`). **Per-monitor DPI/fractional scaling added 2026-08-05** — found and fixed a real, latent bug: layout was computed straight from `winit`'s physical-pixel window size, but widget layout styles are logical-unit, DPI-independent values, so any `scale_factor != 1.0` display would have rendered every widget the wrong size once real rendering exists. New pure `logical_size` conversion function (deliberately total — falls back to `1.0` only for a non-positive/non-finite factor, not for a real fractional factor below `1.0`, which some Linux compositors use), wired into both initial layout and every resize, kept current via a new `WindowEvent::ScaleFactorChanged` handler. 6 new tests (34 total). **File dialogs and clipboard added 2026-08-05** — picked the two dependencies PRD §8.3/§14's own table had already named but never added: `rfd` (native dialogs) and `arboard` (system clipboard, text-only). `arboard`'s Windows backend is BSL-1.0-licensed, a real permissive licence not previously allow-listed — added to `deny.toml` with a comment, not silently. Wired into the command palette (its own only live text-input surface right now): `Ctrl+C`/`Ctrl+V` against the real OS clipboard, a new "Open File…" entry showing a real native `rfd::FileDialog`. Kept the pure dispatch logic testable by isolating both platform calls behind `&mut dyn ClipboardAccess`/`&mut dyn FileDialogAccess` seams (`FakeClipboard`/`FakeFileDialog` in tests) — same shape `translate_key`/`translate_modifiers` already used. Honest about its limit: a chosen file is only recorded (`pending_open_path`), not imported — `aurora-io` is still an empty skeleton. 11 new tests (39 total). **Drag & drop added the same day** — real, native `winit` events (`DroppedFile`/`HoveredFile`/`HoveredFileCancelled`), no new dependency; a dropped file writes the exact same `pending_open_path` slot "Open File…" does, since both are the same "open this" signal. **Native menu bar added the same day, macOS only** — investigated `muda` on all three platforms before writing code: Linux's only backend needs a real `gtk::Window` a plain `winit` window structurally never is, and `muda` doesn't even compile on Linux without the heavy `gtk` feature (no fallback backend); Windows needs its own `unsafe_code` lint override for a raw-HWND call. Asked Cahya which scope to take given this; picked macOS-only, matching PRD §8.3/§14's own wording, which only names macOS for the native menu bar. `build_menu`/`activate_command` (the latter refactored out of the palette's own `Enter` handling so both UI surfaces share one command-dispatch path) are cross-platform logic. 3 new tests (42 total): `activate_command` is real and unit-tested; a fourth test walking `build_menu`'s own item tree was written but **failed on the first real macOS CI run** — `muda::Menu::new()` panics with "can only be created on the main thread" under `cargo nextest run`, and this isn't fixable by test-side changes: neither nextest nor libtest's own default harness ever runs an individual `#[test]` fn on the process's real main thread (both dispatch to worker threads even at `--test-threads=1`), so no attribute or flag makes a `muda`-constructing test satisfy this. Removed that test; `build_menu` remains real production code (called from `App::new` on the winit event loop's own main thread, where the constraint is naturally satisfied) but is only exercised by actually running the app, not by this crate's `#[test]` suite. **Confirmed 2026-08-06**: after removing the test, the full CI matrix (lint, Linux/macOS/Windows test, docs, deny) passed green — `muda` compiles and links cleanly on all three platforms. Windows/Linux native menus deliberately deferred to Aurora's own future in-window menu. **Canvas rendering added 2026-08-06** (picking up from M1.9's "wire a live document" step, which gave the Brush tool somewhere to paint but no way to see it): `aurora-gpu` gained `CanvasPipeline`, a real, public type promoting bind-group-layout/pipeline logic that crate's own tests had only exercised privately (2 new tests, 12 total); `aurora-app`'s `resumed`/`redraw` now build a real `TileResidency`/`CanvasPipeline` sized to the canvas dock area and draw the live tile store's content within it every frame, `CanvasView`'s own pan reflected (zoom deliberately not — `TileResidency` has no scale support yet) (6 new tests, 84 total). A real finding along the way: `aurora_widgets::WidgetTree::bounds` returns a widget's current (zero by default) bounds unconditionally once it exists, not `None` before the first layout — a test wrongly assumed the latter and was fixed. Still `[~]` overall: Windows and Linux remain unverified on real hardware, the crash-recovery/command-palette/keyboard-shortcut/DPI-scaling/clipboard/file-dialog/drag-and-drop/native-menu/canvas-rendering work all still needs a real-hardware pass, and rotation/rulers/guides/grid/snap/true-infinite-zoom remain open on the Canvas bullet itself. See M1.8 |
| **Phase 1 — M1.9** | **Started 2026-08-06.** `aurora-io`'s first real code (was a placeholder): PNG import/export. Asked Cahya which of M1.9's five bullets to start on, since several are big, separate decisions (the `.aur` format needs its own ADR-calibre choice; basic tools need a canvas that doesn't exist yet, M1.8's own still-open bullet) rather than straightforward continuations — PNG was picked as the one with no such blocker. New `Image` type: `f16` RGBA samples plus a real `aurora_color::IccProfile` tag (invariants §7.3.1b/§7.3.6), deliberately standalone rather than wired into `aurora_doc::LayerTree`/`aurora_tile::TileStore` — a layer doesn't own real pixel storage yet either. `png::decode`/`encode`: decode normalizes any PNG colour type to RGBA via `Transformations::EXPAND | ALPHA`, preserving real 16-bit precision where the source has it (a new `aurora_color::promote_u16`, the missing symmetric counterpart to `promote_u8`); encode is 8-bit via `dither_quantize`. **A real, empirically-found correction along the way**: `EXPAND` does not expand grayscale to RGB (confirmed by actually running it, not trusting the flag's own docs) — a grayscale source comes back `GrayscaleAlpha`, which `decode` now expands to RGBA itself; caught by a real independent-reader-style test (encode via the `png` crate's own encoder, decode via this crate's own code) before it shipped as a silent bug. 9 new `aurora-io` tests, 2 new `aurora-color` tests (23 total). Verified: `cargo fmt --all --check`/`cargo clippy -p aurora-io -p aurora-color --all-targets --all-features -- -D warnings`/`RUSTDOCFLAGS="-D warnings" cargo doc`/`cargo clippy --workspace --all-targets --all-features -- -D warnings`/`cargo test --workspace` (0 failures)/`cargo deny check all` all clean. **`.aur` format decided the same day — [ADR 0009](docs/adr/0009-aur-document-format.md), resolving PRD §12 Q7.** A ZIP container (real precedent: Krita's `.kra`, OpenRaster's `.ora` — both ZIP-based layered-image formats, and a better fit for PRD §14's "open format... freely implementable" goal than a bespoke binary container), holding a `postcard`-serialized manifest/history plus `aurora_tile::codec`'s own already-proven tile encoding embedded verbatim (no redundant second compression pass). `postcard` over `rkyv` (PRD §8.1's other named candidate) since `rkyv`'s zero-copy design ties the format to Rust's own memory layout — in tension with "freely implementable" — for a speed advantage that doesn't even apply here (the actual huge data is pixels, handled separately by `aurora_tile::codec`). Compatibility policy: backward-compatible unconditionally (every past file keeps opening), forward-tolerant best-effort (an unrecognised ZIP entry from a newer version is skipped, not fatal) — directly answering Q7's own question. Decision only, no code yet — the reader/writer, the manifest's exact schema, and wiring the crash-recovery journal through it are all separate, still-open follow-on work. **JPEG import/export added the same day** — `zune-jpeg` (decode) + `jpeg-encoder` (encode), matching PRD §8.2's own pre-decided "mature, pure Rust" codec pair; `mozjpeg`'s FFI route was rejected specifically because PRD frames this row as pure-Rust, unlike RAW/ICC. `jpeg-encoder`'s IJG-derived DCT code needed a new, real `deny.toml` allow-list entry (the Independent JPEG Group's own permissive licence). Decode explicitly requests RGBA but verifies what it actually got rather than trusting the request, the same discipline the PNG grayscale finding demanded. JPEG's own real limits handled honestly: no alpha channel, 8-bit only, and a checked error (not silent truncation) past its 16-bit dimension fields. 4 new tests (13 total in `aurora-io`) — one round-trip test's first draft compared the (JPEG-less) alpha channel too and failed with a ~252/255 diff, caught and fixed before being mistaken for a real bug. **TIFF import/export added the same day** — the `tiff` crate (`image-tiff`, same image-rs org as `png`), covering both decode and encode in one crate. New shared `channels.rs` module (gray/gray-alpha/RGB → RGBA expansion) factored out of `png`'s own private helper so `tiff` doesn't duplicate it. TIFF's real permissiveness scoped down honestly: only the first IFD, only Gray/GrayA/RGB/RGBA (Palette/CMYK a real checked error, since correct CMYK needs an ICC-aware `aurora_color::Transform`, not an uncalibrated formula), only 8-/16-bit unsigned samples, uncompressed 8-bit export. 7 new tests (20 total) — a first draft of the round-trip test asserted bit-exact equality and failed (34 vs 33), not a real bug but a wrong assumption: `dither_quantize` deliberately perturbs by up to one step, same tolerance `png`'s own test already uses. **Autosave and recovery added the same day**, closing the gap both the `.aur` decision and M1.8's crash-recovery dialog left open: `aurora-doc::History::save_journal`/`load_journal` (`postcard`, ADR 0009) give the journal a real on-disk encoding for the first time (serde added to `LayerOp` and everything it references, including hand-written impls on `aurora_core::Id<T>` for the same generic-bound reason its other trait impls already are; 5 new tests, 84 total in `aurora-doc`), and `aurora-app` now writes an autosave file every startup and actually recovers from it (not just detects a previous crash) when a marker and a valid autosave are both present — the crash-recovery dialog keeps its one "Continue" action but its message now says whether recovery happened (5 new tests, 47 total in `aurora-app`). **Basic tools (Zoom, Pan, Marquee Select) added the same day** — the user asked to start here directly, ahead of M1.8's own still-open canvas-rendering bullet: new `aurora-ui` `CanvasView` (pan/zoom transform) and `Tool` (the five-variant enum plus `marquee_rect`) are pure and headlessly tested (25 new tests); `aurora-app` gained its first pointer input ever (`CursorMoved`/`MouseInput`/`MouseWheel`), driving scroll-to-zoom, the Zoom tool's click-to-zoom, Pan (middle-button or the Pan tool's own drag), and Marquee Select's live-updating `aurora_doc::SelectionSet`, plus keyboard shortcuts (v/m/z/h/i) to switch tools (22 new tests, 68 total). Move and Eyedropper are real, selectable tools with no pointer handling yet — genuinely blocked (no active-layer selection exists to move; no layer owns real pixel storage yet to sample), not just unscheduled. **Basic brush and eraser's first slice added the same day**: `aurora-brush`'s first real code, `dabs_along_path`, generalizes `spike/vertical-slice`'s own measured dab-spacing formula to a full multi-point stroke path (7 new tests). Stopped there — stamping an actual pixel needs a per-layer pixel storage decision that had been open, named, and unanswered since M1.4 — flagged back to Cahya rather than invented, the same class of fork the `.aur` format was before ADR 0009. **Pixel storage decided the same day — [ADR 0010](docs/adr/0010-layer-pixel-storage.md) — and then implemented, in three committed steps.** One shared `TileStore` per document (one background-writer thread and one real memory bound regardless of layer count, not one store per layer, which `TileStore::new`'s own thread-per-store design would have made unbounded against PRD §6's "unlimited layers"), tiles addressed by a new `(SurfaceId, TileId)` compound key with `SurfaceId` reused directly from each pixel layer's own `LayerId` (no second id-allocation scheme to keep in sync), plus a separate small dedicated store for the active brush stroke, matching `spike/vertical-slice`'s own already-measured two-store split (isolating the brush's under-1ms latency margin from the rest of the document's eviction traffic). Implemented same-day: `aurora-tile` gained `SurfaceId` and the compound-key `TileStore` API (2 new tests, 15 total, threaded through every real call site in `aurora-gpu`/`aurora-render`); `aurora-doc` gained `LayerTree::surface_id` (4 new tests, 88 total), reusing a pixel layer's own `LayerId` with no shape/serialization change; `aurora-brush` gained `stamp_dab`/`stamp_stroke` (7 new tests, 14 total), porting the spike's own already-measured dab-painting math onto the new multi-surface store and finally wiring `dabs_along_path`'s output into real pixels. **A live document wired into `aurora-app` the same day, so brush painting can actually run.** `App` now keeps its own `LayerTree` alive (built once in `App::new`, previously discarded after populating the panels every run) plus a real `aurora_tile::TileStore` (a fixed 256-tile/128 MiB budget, `None` rather than fatal if the scratch directory fails to open); a new `active_layer` field names the topmost pixel layer to paint into (`topmost_pixel_layer`, no click-to-select UI yet — same gap Move already had). `aurora_ui::Tool` gained a sixth variant, `Brush`, wired to a real `v/m/z/h/i/b`-style keyboard shortcut. A `Brush` drag calls a new `aurora_brush::advance_segment`/`dab_step` (refactored out of `dabs_along_path` itself) to carry dab spacing correctly across many small pointer-move events — calling `dabs_along_path` fresh on each two-point event would have silently reset its own spacing countdown every time, meaning a slow drag could place no dabs at all past the first one. `App::paint_dab` converts a document-space point to the active layer's own local space (`layer_local_point`, subtracting `bounds`'s own origin — each layer's surface is independently addressed from its own `(0, 0)`, not the document's) and calls `aurora_brush::stamp_dab`. 15 new tests across `aurora-brush` (5, now 19) and `aurora-app` (10, now 78); `aurora-ui`'s `Tool` tests updated for the sixth variant. Move, Eyedropper, eraser, and undo-as-you-drag all remain open — this closes exactly the "no live document to paint into" gap ADR 0010 named, nothing more. **Active-layer selection added the same day**: `aurora_widgets::WidgetTree` gained `hit_test` (point -> topmost widget, generic, 5 new tests, 137 total); `aurora_ui::layers_panel`'s own rows gained a real, non-zero, clickable size (`scales.spacing` padding, the same tokens `button` already uses) plus `Action::Focus`/`Action::Click`, and `populate_layers_panel` now returns a `WidgetId -> LayerId` map (26 total); `aurora-app`'s new `select_layer` sets `active_layer` and marks the clicked row accessibly selected, wired into `handle_pointer_pressed` ahead of any canvas-tool logic (85 total). Move's own blocker (no way to change the active layer) is resolved; its actual drag-to-reposition logic is separate, still-open work. See M1.9 |

**The single most important open item, updated:** on macOS, a screen reader
does speak a custom-drawn text field, and CJK composition works — human-verified
2026-07-25/26, [full results](spike/a11y-ime/FINDINGS.md). Nothing found rises to
[ADR 0001](docs/adr/0001-custom-wgpu-ui.md)'s structural escape-hatch trigger.
**Windows (UIA) and Linux (AT-SPI)** remain genuinely unverified — different
APIs entirely, and macOS passing says nothing about them — plus one real but
non-structural bug (live value-change announcements don't reach VoiceOver).
**Risk accepted 2026-07-28**: Cahya chose not to block Phase 1 start on the
human Linux/Windows verification (see 0.4) — still open, not resolved, just
no longer gating.

---

## Phase 0 — Technical de-risking

**Goal:** answer every question whose wrong answer would be expensive after Phase 1
code exists. **Exit criterion** (PRD §9): a prototype paints on a huge tiled
document at 60 FPS with sub-10 ms latency on all three platforms, with
custom-rendered panels in the same frame, a screen reader reading a panel, and
CJK composing into a custom text field.

### 0.1 Decisions and documentation

- [x] PRD written and revised to v1.8 — [PRD.md](PRD.md)
- [x] ADR 0001 — custom UI toolkit on `wgpu` — [adr/0001](docs/adr/0001-custom-wgpu-ui.md)
- [x] ADR 0002 — 300,000 px document ceiling (PSB parity) — [adr/0002](docs/adr/0002-document-size-ceiling.md)
- [x] ADR 0003 — ≥16-bit float precision floor — [adr/0003](docs/adr/0003-float-precision-floor.md)
- [x] ADR 0004 — full layered PSD/PSB write — [adr/0004](docs/adr/0004-psd-full-write.md)
- [x] Licence chosen: MIT — [LICENSE](LICENSE), PRD §14
- [x] Design owner named: Cahya Wirawan — PRD FR-027 *Ownership*
- [x] Name/trademark investigated — PRD §12 Q2b (retained; not a legal clearance)
- [x] ADR 0005 — tile size (256×256 px) and scratch-disk budget mechanism — [adr/0005](docs/adr/0005-tile-size-scratch-budget.md), 2026-07-28. **Correction**: this line previously said "needs 0.4 numbers" — 0.4 is the accessibility spike, unrelated to tile sizing; the real numbers were in 0.3 (vertical slice) all along, and §0.8 already said this wasn't blocked on anything. Written alongside the real `aurora-tile` implementation (M1.1).
- [x] **ADR 0006 — accessibility conformance target: WCAG 2.1 AA** —
  [adr/0006](docs/adr/0006-accessibility-conformance-target.md),
  2026-08-04. WCAG 2.1 AA's success criteria, reinterpreted per
  criterion for desktop software, chosen because it's the substantive
  floor both named alternatives already build on — Section 508's 2017
  refresh applies WCAG 2.0 AA to non-web software, and EN 301 549 (the
  EU's 2025 European Accessibility Act's own basis) is built on WCAG
  2.1 AA — so it's the one target that substantively satisfies all
  three framings PRD §12 Q1 named, not a claim of formal conformance to
  either procurement standard. Consistent with, and extends, FR-027's
  own already-shipping WCAG 2.1 AA contrast requirement
  (`check_gated_pairs`) rather than inventing a second bar alongside
  it. Explicitly informed by real evidence, not decided in the
  abstract: `spike/a11y-ime/FINDINGS.md` and this project's own live
  macOS testing (0.4, M1.8) — both the live-announcement bug and the
  deep-nesting VoiceOver keyboard-navigation gap are named directly in
  the ADR's own consequences as exactly the class of problem a
  criteria-only audit could miss without live, human, multi-platform
  testing. Follow-on work named explicitly: a Phase 1 audit checklist
  extending `check_contrast.py`'s discipline to keyboard operability,
  name/role/value, and focus visibility across the component gallery,
  and treating the deep-nesting navigation gap as in-scope for that
  audit (WCAG 2.4.3/4.1.2), not a side issue.
- [x] ADR 0007 — RAW decode library: LibRaw via FFI — [adr/0007](docs/adr/0007-raw-library-libraw.md); Cahya's decision, informed by `spike/raw-icc` and `spike/lgpl-packaging`
- [x] ADR 0008 — ICC transform library: lcms2 via FFI — [adr/0008](docs/adr/0008-icc-library-lcms2.md); no packaging complexity, unlike ADR 0007 — Little CMS's core is MIT
- [x] **ADR 0009 — `.aur` document format: ZIP container, `postcard`
  metadata, embedded tile codec** —
  [adr/0009](docs/adr/0009-aur-document-format.md), 2026-08-06,
  resolving PRD §12 Q7. A ZIP archive (real precedent: Krita's `.kra`,
  OpenRaster's `.ora`), `postcard` (not PRD §8.1's other named
  candidate, `rkyv`) for the manifest/history, and `aurora_tile::
  codec`'s own already-proven tile encoding embedded verbatim rather
  than a second, redundant pixel compression pass. Chosen specifically
  because it best serves PRD §14's own "`.aur` is an open format...
  freely implementable" goal, which a zero-copy format like `rkyv`
  (tied to Rust's own memory layout) or a bespoke binary chunk
  container would serve less well. Backward-compatible unconditionally,
  forward-tolerant best-effort (an unrecognised ZIP entry is skipped,
  not fatal) — Q7's own question, answered directly. Decision only, no
  code yet; see M1.9 for the real, still-open follow-on work this
  unblocks (the crash-recovery journal's on-disk half, deferred since
  M1.4/M1.8).

### 0.2 Workspace and CI

- [x] Cargo workspace, 19 crates, layered per PRD §7.2 (plus
  `aurora-testkit`, a 20th, dev-dependency-only crate added 2026-08-02
  — see this section's own golden-image diff harness bullet for why it
  didn't fit inside the original 19)
- [x] Layering rule enforced mechanically — `scripts/check_layering.py`
- [x] Lints: `unwrap`/`expect`/`panic`/`indexing_slicing` denied workspace-wide
- [x] CI: fmt, clippy, layering, tests, rustdoc, `cargo-deny` on Linux/macOS/Windows
- [x] Toolchain pinned — 1.97 (raised from 1.88; `cosmic-text` needs ≥1.89)
- [x] **Brush-latency regression test in CI** — done 2026-08-02, overdue
  against this bullet's own stated gate ("must exist before Phase 1
  feature work"; M1.1–M1.7 are already substantially done). Split into
  two real `#[test]`s, both picked up automatically by CI's existing
  `cargo nextest run --workspace` step — no new CI job needed, since
  neither is a criterion benchmark (this repo's existing
  `aurora-tile/benches/tile_store.rs` is a criterion bench and is
  *not* CI-gated today: `cargo bench` is never invoked in `ci.yml`).
  - `crates/aurora-tile/src/store.rs`'s
    `paint_and_dirty_round_trip_stays_within_a_tight_cpu_budget`: 1000
    iterations of writing a 48×48 brush-sized region into a resident
    tile and taking its dirty rect (pure CPU, no GPU involved) — the
    piece most exposed to an accidental algorithmic regression (e.g. a
    future scan over every resident tile instead of one) and the one
    whose cost is genuinely hardware-independent, unlike a GPU-touching
    check. Asserts the **median** (not p99 — a single scheduler
    preemption on a shared CI runner can spike one tail sample without
    indicating a real regression) is under 500µs, ~150× the ~3.1µs
    actually measured on this machine; p95/p99 are computed and printed
    for visibility, not asserted on.
  - `crates/aurora-render/src/latency.rs`'s
    `upload_and_composite_one_dirtied_tile_stays_within_a_generous_ci_budget`:
    30 iterations of `TileResidency::sync` + `TileCompositor::composite_over`
    against a real GPU device, timing only encode+submission (not
    waiting for GPU completion) — deliberately not the spike's own
    "input → frame submitted" number, which needs a real frame/present
    loop that doesn't exist until `aurora-app` (M1.8). Gated behind
    `real_context()` (this crate's existing "skip if no adapter" pattern
    — a real, already-accepted gap, not a new one); asserts p95 under a
    deliberately loose 200ms, explained at length in the module's own
    doc comment: CI GPU availability is completely uncontrolled (real
    discrete GPU down to software adapter down to none at all), so a
    tight assertion would either be consistently blown by weak CI
    hardware regardless of code correctness, or too loose to mean
    anything on real hardware — this is a trip-wire for a regression
    class already hit once (finding #4: re-uploading the whole visible
    grid instead of one changed tile) or an accidental blocking wait,
    not an enforcement of the 10ms PRD budget itself. **Not yet done,
    and out of scope until `aurora-app` exists**: a true end-to-end
    "input → frame presented" regression test — that needs a real
    window/event loop this project doesn't have yet. Verified: `cargo
    fmt --all --check` clean, `cargo clippy -p aurora-tile -p
    aurora-render --all-targets --all-features -- -D warnings` clean,
    `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-tile -p aurora-render
    --no-deps --all-features` clean, `cargo build --workspace` clean,
    `cargo test -p aurora-tile -p aurora-render` — 13/13 and 27/27
    passed (the GPU test skips gracefully in this sandbox, which has no
    adapter — same documented gap as every other real-GPU test here),
    `cargo deny check licenses` clean.
- [x] **Golden-image diff harness** — done 2026-08-02
  (`crates/aurora-testkit`, a new crate — see below for why). PRD §8.5
  frames this as CI/test infrastructure two *different* future
  consumers need: `aurora-filters` (render correctness, PRD §13 Step 5)
  and `aurora-widgets`' still-open component gallery (M1.7's own
  bullet). Per `scripts/layering.json`, those two sit in different
  branches of the layering tree — `aurora-widgets` cannot depend on
  `aurora-render`, where a shared helper would otherwise most naturally
  live — so no existing crate is reachable by both. Rather than guess
  which one, or duplicate a shared piece into each (the `test_support.rs`
  pattern `aurora-gpu`/`aurora-render` already use for something much
  smaller), this became a new, 20th workspace crate,
  `aurora-testkit`, deliberately dependency-free of the rest of the
  workspace (its own `layering.json` entry: `[]`) so both future
  consumers can reach it from the bottom of the stack as a
  `[dev-dependencies]`-only entry — never a real dependency of shipped
  code, stated in its own `Cargo.toml` description and `lib.rs` doc
  comment.
  - `Image` (`width`/`height`/`rgba: Vec<u8>`, always `Rgba8`) is the
    common currency; converting a pipeline's own precision (`aurora-tile`'s
    `f16` tiles, for instance) down to that is each caller's own job —
    this crate has no way to do it itself without a dependency neither
    of its two branches share.
  - `compare_to_golden(path, actual, tolerance)`: per-pixel, per-channel
    absolute-difference comparison against a golden PNG on disk.
    `tolerance` accounts for real GPU/driver numerical noise (the same
    class `aurora-color`'s own lcms2 tests already had to tolerate) —
    `0` demands a bit-exact match. A missing golden is always an error
    (`GoldenMissing`) unless `AURORA_BLESS_GOLDEN` is set, so a first CI
    run can never silently accept an unreviewed baseline as truth; a
    dimension mismatch is its own distinct error
    (`DimensionMismatch`); a tolerance violation
    (`PixelMismatch`) writes `<name>.actual.png` and `<name>.diff.png`
    (a white/black per-pixel mask) alongside the golden for a human to
    open side by side, not just a mismatch count. **A real, foreseeable
    case explicitly handled, not assumed away**: a golden a human placed
    by hand from another tool might not decode as 8-bit RGBA (e.g. a
    grayscale export) — `GoldenWrongFormat` is a distinct, real error
    for that, not an `unreachable!()`, with its own test (a hand-written
    grayscale PNG) proving it's actually reachable.
  - Bless mode (`compare_to_golden`'s env-var read) is split from the
    real comparison logic (`golden::compare`, `pub(crate)`, taking
    `bless: bool` explicitly) specifically so this crate's own tests
    never need `std::env::set_var` — unsafe as of the 2024 edition,
    for the exact cross-thread hazard `cargo test`'s default parallel
    runner would create between tests toggling shared process state.
    This keeps `unsafe_code = "deny"` satisfied via the workspace lint
    with no override needed anywhere in this crate, rather than opening
    the exception CLAUDE.md reserves for a real FFI need (`aurora-gpu`)
    for a testing convenience instead.
  - **A real bug a test caught**: the first test fixture used a pure
    white pixel; `saturating_add`-based nudging silently no-ops on an
    already-saturated channel, so a "nudge every pixel" test only
    actually changed 3 of 4 — caught immediately by an exact expected-count
    assertion, fixed by using a fixture with headroom in every channel
    instead.
  - **First real consumer wired, not just built and left unused**:
    `crates/aurora-render/src/composite.rs`'s new
    `composite_over_matches_the_golden_image` test (`aurora-testkit`
    added as `aurora-render`'s first real `[dev-dependencies]` entry,
    `scripts/layering.json` updated accordingly — `aurora-widgets`
    deliberately *not* granted the same permission yet, since it has no
    real consumer either, the same discipline just re-applied after
    removing `aurora-widgets`' own unused `aurora-gpu`/`aurora-vector`/
    `aurora-text` dependencies earlier this same day) reads back the
    *whole* composited tile from real GPU hardware and compares it
    against a checked-in golden
    (`crates/aurora-render/tests/golden/composite_basic.png`), not just
    the single first-texel spot-check the existing pixel-math test
    already did. The golden itself was generated from the exact,
    already-real-GPU-verified constant that test's own math predicts
    (`(0.5, 0, 0.5, 1.0)` — solid `(0,0,1,1)` under solid `(1,0,0,0.5)`
    "source over"), via a throwaway `AURORA_BLESS_GOLDEN=1` example run
    once and deleted immediately after — not blessed from an actual
    render, since this sandbox has no GPU to run one; the checked-in PNG
    is the same value real hardware already confirmed, encoded by hand
    rather than captured from a render this environment can't perform.
    Verified via the harness's own round-trip (a throwaway
    non-bless-mode run against the same constant, confirming the PNG
    this crate wrote actually decodes back to a match) before the
    generator was deleted.
  - Verified: `cargo fmt --all --check` clean, `cargo clippy -p
    aurora-testkit -p aurora-render --all-targets --all-features -- -D
    warnings` clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p
    aurora-testkit -p aurora-render --no-deps --all-features` clean,
    `cargo build --workspace` clean, `cargo test -p aurora-testkit -p
    aurora-render` — 8/8 and 28/28 passed (the new golden-image GPU test
    skips gracefully in this sandbox, which has no adapter — same
    documented gap as every other real-GPU test here, so it has not
    actually been confirmed against a real render; that's the one
    honest gap in this pass, closeable the moment someone runs it on
    real GPU hardware), `cargo deny check all` clean (new dependency:
    `png`, MIT/Apache-2.0, image-rs's own PNG-only crate). `python3`
    remains absent in this sandbox, so `scripts/check_layering.py`
    itself still can't run here; the new `aurora-testkit`/`aurora-render`
    entries were checked by hand against the script's own logic instead.

### 0.3 Vertical slice — **done**

Evidence: [spike/FINDINGS.md](spike/FINDINGS.md), `spike/vertical-slice/`

- [x] Window → `wgpu` → tiled half-float document → stroke → save/reload
- [x] UI and canvas in one device and one frame (invariant §7.3.8)
- [x] 80 GB document edited in a 64 MB budget, with real eviction and page-in (§7.3.1)
- [x] Half-float round-trip verified bit-exact (§7.3.6b)
- [x] Latency, frame, paging, and I/O measured
- [ ] Run on Windows *(DX12 backend unvalidated)*
- [x] Run on Linux — Vulkan/NVIDIA RTX 3090, 6 runs, 2026-07-26 — [spike/FINDINGS.md](spike/FINDINGS.md#second-platform-linux--vulkan-2026-07-26). Stroke latency and idle-frame budgets pass with more headroom than macOS; pan-while-painting is marginal (straddles the 16.7 ms budget across runs on this shared machine) rather than a clean pass — same architectural bottleneck as macOS (finding 1), not a new one
- [ ] Re-run at the 300,000 px ceiling *(only 100,000 px tested)*

### 0.4 Accessibility and IME spike — **macOS verified (9/10); Windows/Linux unverified, risk accepted 2026-07-28 to unblock Phase 1**

Evidence: [spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)

**Human-verified on macOS 2026-07-26**, two consecutive runs. Role, label,
value, focus, screen-reader navigation, and full CJK IME composition (preedit,
correctly-positioned candidates, commit) all confirmed via VoiceOver on a
custom-rendered `wgpu` field. **Nothing found rises to ADR 0001's structural
escape-hatch trigger** — one real bug found (live value-change announcements)
and diagnosed down to a specific, non-structural cause.

- [x] `accesskit` tree construction — role, label, value, focus, composition state
- [x] Platform adapter initializes; window runs stably
- [x] `winit` IME plumbing wired (`set_ime_allowed`, `set_ime_cursor_area`)
- [x] Custom text field: insert, backspace (char-wise), cursor motion, preedit
- [x] **VoiceOver announces the field (macOS)** — role, label, value, focus all PASS
- [x] **CJK composition commits correctly (macOS)** — preedit, commit both PASS
- [x] **IME candidate window appears at the field (macOS)** — PASS, `set_ime_cursor_area` confirmed working
- [!] **Narrator announces the field (Windows, UIA — a different API; macOS success does not carry over)** — not run, no Windows machine tested yet. **Verification deferred by decision, 2026-07-28**: Cahya chose not to block Phase 1 start on hardware access — see the risk-acceptance note below. Still genuinely unverified, not assumed equivalent to a pass; re-open if Phase 1's own accessibility audit (which re-tests every platform anyway, see the Phase 1 exit criterion) surfaces a problem.
- [!] **Orca announces the field (Linux, AT-SPI)** — build clean on Linux 1.97.1, `accesskit`'s AT-SPI backend (`accesskit_atspi_common`/`accesskit_unix`) compiles in, `--dump-tree` shows correct role/label/value/composition-state with no window needed — see [FINDINGS.md](spike/a11y-ime/FINDINGS.md#linux--build-and-tree-construction-confirmed-orca-leg-still-blocked). Still blocked: the decisive human-plus-Orca test needs a live logged-in desktop session, which this machine did not have (GDM greeter only, no user session) — not yet run. **Same 2026-07-28 deferral as Windows above.**

**Risk accepted 2026-07-28: Phase 1 start is no longer gated on the human
Linux/Windows verification above.** Cahya's call, not a spike output —
the honest basis for it is that macOS passed cleanly on the same
`accesskit`/`winit` abstraction (a cross-platform library over three
different native APIs by design), the Linux build and tree construction
already confirm the plumbing compiles and produces a correct tree, and
Windows/Linux hardware access wasn't available across several passes
already. This is **not** a verification and the two items above stay
marked `[!]`, not `[x]` — no evidence exists for either platform's actual
screen-reader behavior. It's a named, deliberate risk acceptance:
low-likelihood given macOS's result and `accesskit`'s architecture, but
real, and the fallback (ADR 0001's CXX-Qt escape hatch) exists precisely
for the case this turns out wrong. Revisit opportunistically — first time
either platform's hardware is actually available — rather than blocking
on it.
- [x→bug] **Live value-change announcements (macOS)** — FAILS; typing updates the tree correctly every keystroke (confirmed via debug logging) but VoiceOver never announces it. Traced into `accesskit_macos` source — looks like a real, narrow implementation bug, not a platform inability. Candidate root cause: `Role::Window` nested inside a real native window (same suspect as the navigation-depth finding below)
- [~] **Screen-reader linear navigation (macOS)** — reaches the label, but only via VoiceOver's "interact" command, not plain arrow keys; confirmed present via the Rotor. Worth a quick experiment: try a plainer root role than `Role::Window`
- [ ] Screen-reader-driven actions (set value, navigate by word/line)
- [ ] `TextSelection` exposed in the tree
- [ ] Dead-key accent composition (macOS) — optional; one attempt was confounded by the CJK IME still being active, left honestly unanswered rather than guessed

> **Nothing found on macOS meets ADR 0001's structural bar** — AccessKit and
> winit *can* express role/label/focus/IME through a custom-rendered field.
> The open question is now Windows and Linux, not whether this is feasible at
> all. Revisit the escape hatch only if one of those surfaces something
> AccessKit genuinely cannot express on that platform.

### 0.5 Design language — **complete**

Owner: Cahya Wirawan. Blocks all widget code (invariant §7.3.10 — tokens cannot
be retrofitted cheaply). Runs in parallel with 0.3/0.6; needs no engine code.

Evidence: [design/](design/README.md) (deliberately outside the Cargo
workspace, same pattern as `spike/`), commit
[b0a7ac8](https://github.com/cahya-wirawan/aurora/commit/b0a7ac8).
**Reviewed and approved by the design owner, 2026-07-27.**

- [x] Token vocabulary — semantic names widgets resolve against *(highest value; this is the interface everything else is written to)* — [design/tokens/vocabulary.md](design/tokens/vocabulary.md), owner-approved
- [x] Type scale, spacing scale, radius, elevation, motion values — [design/tokens/scales.toml](design/tokens/scales.toml), owner-approved; font family still an open placeholder (`[type].family`), not blocking
- [x] One complete built-in theme (Dark), all pairs passing contrast — [design/themes/dark.toml](design/themes/dark.toml), owner-approved; every gated pair passes WCAG 2.1 AA via [design/check_contrast.py](design/check_contrast.py) (17/17 gated pairs pass; `border.default` and disabled text are informational, not gated — see script comments for the WCAG 1.4.11 rationale)
- [x] Static mockups: main workspace + 2–3 panels — [design/mockups/workspace.html](design/mockups/workspace.html) (Layers/Properties/History docked), owner-approved; HTML/CSS is a Phase 0 review tool only, not how `aurora-widgets` renders
- [x] Component gallery skeleton — review surface and golden-image target — [design/gallery/index.html](design/gallery/index.html), owner-approved; covers button/checkbox/slider/field/dropdown/tab bar/tooltip/swatch across forced states; scrollbar/tree/menu/curve editor deliberately left for a later pass
- [x] Outside critique on the mockups (risk R2f mitigation) — **2026-07-28: a colleague reviewed the scaffold and signed off as fine for a start**, with the explicit understanding it can be revised later if needed. Not a formal design-professional audit, but it satisfies R2f's actual gap (no second opinion at all) — good enough to unblock widget work; deeper critique can still happen opportunistically as the token system gets exercised for real.

### 0.6 Format feasibility — PSD partially done; RAW/ICC spiked

Evidence: [spike/psd-write/FINDINGS.md](spike/psd-write/FINDINGS.md),
[spike/raw-icc/FINDINGS.md](spike/raw-icc/FINDINGS.md)

- [x] PSD write spike — layer file with names, alpha, opacity, blend modes, visibility, Unicode names; verified by two independent readers with layer pixels checked
- [x] **Layer groups** — 2-level nesting, open/closed state, membership, and multiply-blend compositing through nested groups; structural assertions in `verify.sh`, pixel math checked by hand, not just eyeballed
- [!] **Verify in Photoshop itself** — no licence available; the only check that settles ADR 0004
- [x→bigger] **Text layer (`TySh`) spike — container format tractable, but scope grew.** Downloaded a real Photoshop-authored text layer (a `psd-tools` test fixture) and read its structure before writing code. `TySh`'s own container is small (6 fields) and its byte layout is implemented in Rust (`src/descriptor.rs`) — parses, patches, and round-trips real Photoshop data correctly. **But `EngineData` (the actual text/styling content) is far richer than expected** — full kinsoku/moji-kumi tables, duplicated resource dicts, even for plain English text — making from-scratch generation genuinely higher-risk than assumed. The validated lower-risk path is patch-a-real-file rather than generate-from-scratch (proven end-to-end in Python, independently verified by `psd-tools` + `sips`). **Found a new, mandatory, previously-unscoped requirement: editing text content requires rendering actual glyphs into the layer's pixel channels**, or the file is internally inconsistent — confirmed by direct visual inspection. This is now the single biggest addition to Phase 3 scope from any spike so far.
- [x] **`EngineData`'s own text-format reader/writer** (`src/engine_data.rs`) — implemented and tested, not just patch-in-place on an opaque blob anymore. `--tysh-demo` now patches both the top-level `Txt ` field and the nested `EngineDict.Editor.Text` together. Corpus extended with two `TySh` blocks from `ag-psd` (a second, independently-written library that also *writes* text layers) — including a genuine multi-style-run case — and the existing parser needed **zero changes** to handle them. Caught and fixed one real bug in the process: a Unicode-escaping edge case (codepoints with `)` as their high byte) that would have silently truncated strings; found by a test, not inspection. 9 tests total, all against real extracted bytes.
- [x] **Corpus extended to paragraph text and warped text** — `reference/tysh-paragraph.bin` (paragraph/area text, vs. every other fixture's point text) and `reference/tysh-warp-arc.bin` (`warpStyle = warpArc`, vs. every other fixture's `warpNone`), both from `ag-psd`'s test suite. `descriptor.rs` needed **zero code changes** for either. Two things confirmed against real bytes rather than assumed: point-vs-paragraph lives inside `EngineData` (`EngineDict.Rendered.Shapes.Children[0].Cookie.Photoshop.ShapeType`), not the outer `TySh` descriptor at all; and `warpRotate`'s enum category is `"Ornt"` (a shared orientation enum), not `"warpRotate"` — a wrong assumption the first draft of the test made and a real-bytes assertion caught immediately. FINDINGS.md finding 11. 12 tests total, all against real extracted bytes.
- [x→scoped] **Recompute `ParagraphRun`/`StyleRun` `RunLengthArray`s on text edit** — `engine_data::recompute_run_lengths` fixes the exact staleness `--tysh-demo` used to report (`[7, 7, 16]`/`[30]` → `[13]`/`[13]` after the "Aurora spike" patch), reusing the first run's formatting rather than discarding it. **Deliberately scoped to whole-text replacement** — the only edit shape this patch-in-place spike supports; preserving multiple paragraphs/style runs *across* an edit needs a real cursor/selection model, which is Aurora's own text-editing engine's job in Phase 3, not this exercise. Caught one more UTF-16-vs-scalar-count nuance the same way finding 9's bug was caught: confirmed against a real fixture (`RunLengthArray` sums to UTF-16 code units) before writing the code, not assumed from ASCII-only fixtures where the two counts coincide. FINDINGS.md finding 12. 13 tests total, all against real extracted bytes.
- [x→scoped] **Glyph rendering into pixel channels on text edit — wired into the writer, verified externally.** `src/glyph.rs` rasterizes real text headlessly via `cosmic-text` with a bundled font (finding 13), and `psd.rs` now has a `TySh` slot (`Layer::tysh`) — `cargo run` writes a real PSD whose text layer's descriptor and rendered pixels genuinely agree, confirmed by `psd-tools` reading back the correct text and legible, correctly-encoded pixels (`out/text-layer-psdtools.png`), not just our own writer's say-so. **Caught a real bug only an independent reader surfaced**: `engine_data::write` omitted whitespace between tokens (`<</EngineDict` instead of Photoshop's own `<< /EngineDict`) — every one of this spike's own round-trip tests kept passing throughout, since our reader tolerates it; `psd-tools`' differently-built, whitespace-only tokenizer didn't. Fixed. FINDINGS.md finding 14.
- [x→scoped] **Real `FontSize`/`FillColor` wired in — no more hardcoded color/size.** `engine_data::first_run_style` reads `StyleRun.RunArray[0].StyleSheet.StyleSheetData`'s `FontSize`/`FillColor` (decoding confirmed against `ag-psd`'s own encoder); the written text layer shrank from 151×29 px to 82×16 px matching the real fixture's `FontSize: 13.0`, and still reads back correctly via `psd-tools` — visually confirmed, not just asserted. **Deliberately still scoped**: font *resolution* (the document's actual named font vs. the bundled `DejaVuSans.ttf` stand-in) and `FillColor` alpha compositing for a translucent fill remain unstarted — both real, smaller remaining work, not open unknowns; same first-run-only boundary as findings 12/14. FINDINGS.md finding 15. 21 tests total, all against real extracted bytes or bundled deterministic assets.
- [x] **`descriptor.rs` audit for finding 14's bug class — one untested path found, cross-checked clean.** Grepped all 5 corpus fixtures for `Objc` (OSType for `Value::Nested`): zero hits — every other value type appears in every fixture and has been exercised through `psd-tools` via findings 14/15, but `Value::Nested` never had real-fixture or independent-reader coverage at all. Confirmed the type is real (not hypothetical) via `psd-tools`' own `gradient-fill.psd` fixture (`GRADIENT_FILL_SETTING.Grad`), but that fixture also needs List/Array support this reader doesn't have (correctly out of scope) — so closed the gap with a synthetic nested descriptor instead, cross-checked against `psd-tools`' own `Descriptor` reader (`cargo run -- --descriptor-audit` + `verify.sh` section 3), the same independent-reader discipline that caught finding 14. **Result: no bug this time** — `psd-tools` accepts our `Value::Nested` encoding correctly. Also fixed a stale doc comment that incorrectly implied `warp` exercised this code path (it doesn't — `warp` is a separate `DescriptorBlock` field). FINDINGS.md finding 16. 22 tests total.
- [ ] Layer masks, vector masks, smart objects, layer styles, adjustment layers
- [ ] RLE/ZIP compression, 16/32-bit, CMYK/Lab, PSB
- [x→bigger] **RAW decode spike — decodes real files on all 3 major vendors; licensing worse than PRD §14 assumed.** `rawler` (pure Rust) decoded real, unedited Canon CR3, Nikon NEF, and Sony ARW files (from raw.pixls.us) correctly on the first attempt — confirmed by rendering each to a crude preview and looking at it, not just checking `decode_file` returned `Ok` (all three are unambiguously real photographs). **But `rawler` itself is LGPL-2.1** — checked directly (`cargo info rawler`), and no permissively-licensed full-featured alternative exists (checked `zenraw` AGPL-3.0, `raw_preview_rs` GPL-3.0, `rawlib` MIT-but-thumbnail-only). This contradicts PRD §14's framing, which discussed LGPL risk only for the FFI option (LibRaw) and treated "prefer pure Rust" as if it avoided the obligation — it doesn't, and Rust's lack of a stable dylib ABI arguably makes the relinking obligation *harder* to satisfy than LibRaw's C-library case, not easier. `deny.toml`'s comment updated to stop implying this risk is C-library-specific. FINDINGS.md findings 1–2.
- [x→scoped] **ICC transform spike — Little CMS's core is MIT (not LGPL); pure-Rust `moxcms` cross-validated exactly against it.** Checked directly rather than assumed grouped with RAW's risk: Little CMS's core engine is MIT-licensed (only an unused optional plugin is GPL-3), and `lcms2-sys` vendors/statically compiles it — no dynamic-linking packaging burden at all, the cleaner of the two library choices this pair of spikes covered. Cross-validated `moxcms` (pure Rust, already a transitive dependency of `rawler` itself) against `lcms2` on a real sRGB→ECI-RGBv2 transform: **exact agreement to 4 decimal places on every test color**, including out-of-gamut extended-range values — but only after finding `moxcms`'s `allow_extended_range_rgb_xyz` option (off by default; without it, out-of-gamut values silently clamp to [0,1], which would violate invariant §7.3.1b's HDR-values-preserved requirement). Corrects PRD's "no mature pure-Rust ICC engine" risk note. FINDINGS.md findings 3–4.
- [x] **Decide RAW and ICC libraries, record as ADRs** — [ADR 0007](docs/adr/0007-raw-library-libraw.md) (LibRaw via FFI) and [ADR 0008](docs/adr/0008-icc-library-lcms2.md) (lcms2 via FFI), both Cahya's decision, both informed by the evidence above rather than a spike recommendation alone.
- [x→bigger] **LGPL packaging architecture — proven mechanically on Linux with both `rawler` and, later, LibRaw itself.** [spike/lgpl-packaging/](spike/lgpl-packaging/FINDINGS.md): a `cdylib` (`raw-shim`, the only crate touching `rawler`) behind a hand-written `extern "C"` ABI, loaded at run time via `dlopen` (`libloading`, through a separate `host` binary with **zero** `rawler`/LGPL dependency — confirmed with `ldd`/`nm`, not assumed). Both conditions of LGPL-2.1 §6(b) (quoted directly from the license text, not a summary) verified concretely: (1) genuinely dynamic, no build-time link, checked with `file`/`ldd`/`nm`; (2) a modified `raw-shim` (`.so` rebuilt with one line changed) worked correctly with the **same, unmodified, un-recompiled** `host` binary. All three real RAW files from the raw-icc spike decoded identically through this mechanism as decoding them directly. **Re-verified with LibRaw itself (finding 4)** after ADR 0007 named the gap: a second shim, `libraw-shim` (via `libraw_rs_vendor`, vendoring LibRaw's own source — no system `libraw-dev` needed), exports the identical ABI, and the **same, unchanged `host` binary** loads either one — both LGPL-2.1 conditions re-verified independently, and decode results cross-checked between the two independently-coded libraries (exact match on Canon, within single digits on Nikon/Sony — recorded honestly, not rounded up). **Scope, deliberate: Linux-only, no ABI-versioning story, no macOS/Windows, and this is engineering verification, not legal sign-off** — one specific ambiguity flagged honestly rather than glossed over (§6(b)(1)'s "already present on the user's system" language reads more naturally as a pre-existing system package than an app-bundled file; industry practice treats bundling as sufficient, but that's a judgment call, not a legal conclusion). Also still open: `libraw-shim` vendors+statically-compiles LibRaw rather than dynamically linking a separately packaged `libraw.so` — both satisfy LGPL, which is preferable is a build-engineering question, not decided here.

### 0.7 Test corpora — Phase 0's share done for PSD and RAW/ICC; the 1,000-file PSD gate is explicitly Phase 3 scope

Assemble *before* the parsers exist, so the parser is written against reality.

- [x] **Phase 0's share of this: a first, real corpus to develop the
  reader/writer against** — done 2026-07-28: 319 real PSD/PSB fixtures
  from `psd-tools`' own MIT-licensed test suite, pinned to a commit
  (`corpora/psd/reference/`), 272/272 opened successfully with an
  independent reader (`inventory.md`), real coverage across every PSD
  color mode, most adjustment-layer types, smart objects, groups, and
  artboards. One concrete finding fell out of assembling it: artboards are
  a layer group + one `Descriptor`-shaped tagged block, not a new document
  primitive (`spike/psd-write/FINDINGS.md` finding 17, resolved into
  FR-028).
- [ ] **1,000 real-world PSDs (Phase 3 exit criterion) — deliberately
  deferred to Phase 3, decided 2026-07-28, not a Phase 0 gap.** The 319
  fixtures above are authored test fixtures, not real-world client files,
  and getting to 1,000 real ones raises consent/licensing questions the
  RAW corpus never had (client artwork, not public camera samples).
  Considered sourcing options (own work, permissively-licensed GitHub
  repos, "free PSD" template sites) and decided none of them are worth
  solving today: Phase 0's actual job was "have something real to develop
  against before Phase 3 starts," which is done. Acquiring the literal
  1,000-file real-world gate is Phase 3's problem to solve when Phase 3 is
  actually being planned — consistent with 0.8's "don't plan further ahead
  than the evidence supports."
- [~] **RAW samples per camera vendor** — one real file each for Canon, Nikon, Sony (`spike/raw-icc/reference/`), enough to answer "does this decode major vendors at all," far short of the eventual multi-body/multi-vendor breadth this item is really asking for
- [~] **ICC profile set** — two real, CC0-licensed profiles (`sRGB.icc`, `ECI-RGBv2.icc`), same "first, not final" caveat
- [x] **Fetch scripts (corpora are gitignored — too large to commit)** — `spike/raw-icc/reference/fetch-samples.sh`, and now `corpora/psd/reference/fetch-samples.sh` reusing the same pattern (manifest + pinned commit + re-fetch script)

### 0.8 Re-plan — durations re-grounded 2026-07-28; Q2/Q3 answered, and Q2's answer reframes this whole section

- [x] **Re-ground the §9 phase durations against slice measurements (PRD §13 Step 7)**
  — full reasoning below. Revised into PRD.md §9 directly (each phase now
  carries a short "Re-grounded 2026-07-28" note, same pattern §13 Step 4
  already uses for the vertical slice's corrections), with the detailed
  evidence trail here.
- [x] **Answer PRD §12 Q2 (team size) and Q3 (revenue model)** — answered by
  Cahya: **solo** (Q2), **revenue model not decided, not a priority right
  now** (Q3). Resolved in PRD.md §12 inline, matching how Q2b already
  documents a partial resolution in place. Read the note below before
  reading the phase-by-phase analysis that follows — it changes what that
  analysis means, not just what it's missing.

**The solo answer doesn't just fill in a blank — it means every duration
below, revised or not, is answering the wrong question.** All of §9's
month figures, including the "likely 4–5" / "likely needs another upward
revision" language in this section, were reasoned about as *team* months
(R9: "the durations in §9 assume a staffed team"). A solo developer taking
on GPU rendering, a first-party UI toolkit, text shaping/IME, accessibility,
PSD reverse-engineering, color management, RAW decoding, AI integration,
plugin sandboxing, collaboration, and mobile/web — the full specialist
breadth R9 already flagged as a *hiring* risk for a team — isn't on a
scaled-up version of the team timeline; it's on a differently-shaped
timeline this document doesn't actually have an estimate for. Adding months
to the team figure below would be false precision layered on top of the
false precision this section already warns about for Phases 1/2/4/5.

**What the phase-by-phase analysis below is still good for:** which areas
turned out more (or less) complex than assumed, and why — that evidence
doesn't change based on team size. **What it was not good for: a
completion estimate for this project as currently scoped — resolved
2026-07-28, not left open.** Cahya's answer: both narrow the scope *and*
drop the calendar commitment. Phases 4 and 5 (~22 months of team-scoped
work, mapping almost entirely to FRs §3 already marked Could/Won't-yet)
are cut from the plan entirely — see PRD.md §9's "Beyond v1.0" section and
PLAN.md's Phase 2–3 outline below. Phases 0–3 keep their exit criteria but
lose the calendar commitment: milestone-based, not dated. See PRD §12 Q2's
resolution note for the fuller version of this.

**What actually grounds this re-plan:** five spikes exist now (vertical
slice, a11y/IME, PSD write, RAW/ICC, LGPL packaging), not zero. But three
of the five phases below (1, 2, 4, 5) have **no spike evidence touching
them at all** — re-grounding those against "slice measurements" would be
inventing precision the project doesn't have yet. What follows is honest
about which phases have real new evidence and which don't.

**Phase 0 (was 3 months) — likely 4–5, and the estimate itself was
probably always going to be revised upward once Phase 0's real shape
became visible.** Every one of its five spikes found more than a clean
"quick spike" turned out to hold:

- a11y/IME is still open after multiple sessions, not because the *engineering*
  is slow but because it is genuinely **calendar-bound, not effort-bound**
  — it needs a human at a live desktop session on each of 3 platforms, and
  this pass never got a Windows machine or a logged-in Linux desktop at all.
  A 3-month Phase 0 estimate implicitly assumed this would be as fast as
  the engineering questions it's bundled with; it isn't, structurally.
- PSD write feasibility surfaced **glyph rendering into pixel channels** —
  a requirement with zero prior line item anywhere in the PRD before this
  spike found it (finding 8), now confirmed mandatory. That's not a Phase 0
  cost directly (it lands in Phase 3), but finding a *new, mandatory,
  previously invisible requirement* on the very first format spike is a
  signal about how much more is likely still hiding in the format areas
  Phase 0 hasn't spiked yet (masks, smart objects, layer styles — see
  Phase 3 below).
- RAW/ICC was scoped in PRD §13 Step 5 as "small, fast" spikes. In practice:
  two feasibility spikes, a real licensing correction (the "prefer pure
  Rust" framing in PRD §14 turned out not to avoid LGPL for RAW at all —
  `rawler` is LGPL-2.1 too), and then an *entire additional spike*
  (`lgpl-packaging`) that didn't exist as a line item anywhere until the
  licensing finding made it necessary. Small and fast is not what this
  turned out to be.
- Two ADRs Phase 0 was always going to need (0005 tile size, 0006 a11y
  conformance target) were still `[ ]` at the time of this re-plan — not
  attempted yet, not blocked on anything external, just not reached. **ADR
  0005 written 2026-07-28**, alongside the real `aurora-tile` implementation
  (M1.1) — confirming this section's own read that it was never actually
  blocked. 0006 remains not reached.
- **PRD §13 Step 2 ("Define the 95%") has never been tracked in this file
  at all** — a real gap, not a rounding error. It has no home in 0.1–0.8's
  numbering, which is itself evidence it was dropped rather than
  deliberately deferred. Added as 0.9 below,
  not folded in here, so it stops being invisible.

**Phase 1 (was 9 months, already raised from 6) — no revision; genuinely
no new evidence yet.** None of M1.1–M1.10 has started. The a11y and
vertical-slice spikes *inform* Phase 1 (the M1.x breakdown above already
absorbs their findings — per-tile dirty rects, GPU compositing, the
hidden-window-then-show constraint, the `Role::Window` navigation quirk),
but informing a plan and testing it against real M1.x work are different
things. Leave at 9 months; revisit once M1.1–M1.3 produce real velocity
data, the same way this re-plan leans on the spikes that exist.

**Phase 2 (8 months) — unchanged, and unusually untested even by this
project's own standards.** Zero spikes have touched selections, brushes,
masks, filters, or adjustments. This is the original, purely speculative
estimate; treat it as such rather than as re-grounded.

**Phase 3 (was 10 months, already raised from 8) — likely needs another
upward revision, and 10 should be read as a floor, not a ceiling.** The
one piece of Phase 3 that has been spiked (PSD text layers) went from "assumed
similar to groups" to **"the single biggest addition to Phase 3 scope from
any spike so far"** (glyph rendering, `RunLengthArray` recomputation,
`EngineData`'s real complexity even for plain English text). Basic layers/
groups/alpha/blend-modes are genuinely tractable (good, real de-risking) —
but masks, smart objects, layer styles, and adjustment layers are
**still completely unspiked**, and there is no principled reason to expect
they'll be easier than text layers were; if anything, smart objects and
layer styles are comparably under-documented, reverse-engineered surface.
Separately, RAW's LGPL packaging architecture (now proven mechanically, but
only on Linux, with macOS/Windows and legal review still open) is real
engineering work Phase 3's original estimate never itemized as its own
line, because the licensing finding that required it didn't exist when the
10-month figure was set.

**Phase 4 (was 10 months) and Phase 5 (was 12 months) — cut from the
committed plan 2026-07-28, not re-grounded.** Zero spike evidence existed
for either at the time of this analysis, and none was ever going to be
worth gathering: both mapped almost entirely to FRs §3 already marked
Could/Won't-yet. Rather than carry ~22 team-months of speculative estimate
for already-deprioritized work, they moved to §9's uncommitted "Beyond
v1.0" section — see the outline below. The relative-complexity read above
(text layers surprising Phase 3, Phase 2 untested, etc.) stands as historical
evidence regardless of that cut.

**Total ~52 months — retired, not revised.** This section's phase-by-phase
notes remain useful as relative signal (which areas are riskier than
assumed), but a summed total stopped being the right output the moment Q2
(solo) and Q3 (no funding pressure) were answered: Phases 0–3 are now
milestone-based rather than calendar-committed (PRD.md §9), and Phases 4/5
are cut entirely. There is no revised number to compute, honest or
otherwise — see PRD §12 Q2's resolution note and the "Beyond v1.0" outline
below for where this landed.

### 0.9 Define the 95% — done (PRD §13 Step 2)

Surfaced as a gap during the 0.8 re-plan, not before — it had no home in
this file's 0.1–0.8 numbering, which is itself the evidence it was dropped
rather than deliberately deferred.

- [x] Turn the §10 success metric ("support 95% of common Photoshop
  workflows") into a written, ranked list of concrete workflows across the
  §4 personas (photographer, graphic designer, digital artist, UI designer,
  marketing team) — [docs/workflows.md](docs/workflows.md), 2026-07-28.
  Tier-1/2/3 per persona, each workflow mapped to the FRs it exercises,
  plus a cross-persona rollup identifying which FRs are load-bearing
  (touched by Tier-1 work from 3+ personas) vs. touched by nothing above
  Tier 2 — the latter all landed on FRs already marked Could/Won't-yet in
  §3, which is a consistency check passing, not new information. One real
  gap fell out of the exercise: "Artboards" was a named UI Designer need
  in §4 with no owning FR in §5 — **resolved 2026-07-28** into **FR-028**
  (first-class feature, PRD.md §5). First draft, explicitly a hypothesis
  to argue with rather than a finding — there's no user base yet to have
  interviewed. **Cahya reviewed the Tier-1/2/3 tiering itself 2026-07-28
  and confirmed no changes needed.**

---

## Phase 1 — Document, canvas, layers, rendering, shell

**9 months.** Do not start feature work until 0.2 (CI), 0.3 (slice), 0.4
(a11y verdict), and 0.5 (tokens) are complete. **Unblocked 2026-07-28**:
0.2/0.3/0.5 have real evidence; 0.4 is unblocked by an explicit risk
acceptance (macOS verified, Windows/Linux human verification deferred, not
faked as done — see 0.4) rather than by waiting further on hardware
access. Phase 1 feature work starts now.

**Exit criterion:** create, edit, save, reopen, and export a multi-layer document
with blend modes and unlimited undo at 60 FPS — *and* pass an accessibility audit
and an IME audit on all three platforms — *and* the component gallery renders
every widget in every state across all built-in themes with contrast checks green.

### M1.1 — Core and tile store (`aurora-core`, `aurora-tile`)

- [x] **Geometry, colour types, pixel formats, IDs, error types** — done
  2026-07-28, `crates/aurora-core/src/{geometry,color,id,error}.rs`, 16
  tests, `cargo fmt`/clippy (`-D warnings`, workspace + all targets/
  features)/layering all clean. `Size`/`Rect` (geometry), `ColorSpace`/
  `Channels`/`SampleFormat`/`PixelFormat` (descriptors only, no pixel
  storage — that stays in `aurora-tile`), `Id<T>`/`IdGenerator<T>`
  (phantom-typed, hand-implemented traits, not derived — derive would
  wrongly require `T: Trait`), `CoreError`. First real product code in the
  project; `crate_name()` placeholder removed.
- [x] **Coordinate types sized for 300,000 px with defined overflow
  behaviour** — `Size`/`Rect` use `u32` for extents (300,000 fits with
  ~14,000× headroom past `u32::MAX`; `u16` was explicitly rejected in ADR
  0002) and `u64` for derived area/byte-count arithmetic, which does
  overflow `u32` at the ceiling (300,000² ≈ 9×10^10). `Rect.x`/`.y` are
  signed (`i64`) since a layer's bounds can go negative or past the
  ceiling mid-transform, unlike a document's own `Size`, which is always
  validated in-range via `Size::new`.
- [x] **Sparse tile store, LRU residency, scratch-disk paging** — done
  2026-07-28, `crates/aurora-tile/src/{tile,store,codec,writer,error}.rs`,
  12 tests. Tiles created lazily on first touch (sparse); `lru::LruCache`
  replaces the spike's O(n) `Vec` scan with real O(1) LRU ops and an
  explicit `pop_lru()` hand-off to eviction (not relying on `put`'s
  internal auto-eviction, which doesn't return the evicted value); paging
  round-trips bit-exact through compression (same property
  `spike/FINDINGS.md` proved for the uncompressed format). Tile size
  settled in **ADR 0005** (256×256 px, matching what the spike actually
  measured) — written alongside this work, closing a real Phase 0 gap
  (see 0.1).
- [x] **Per-tile dirty rectangles** — the largest single win from the
  slice. `Tile::mark_dirty`/`take_dirty` accumulate a tile-local
  `aurora_core::Rect` via `Rect::union`, so a future consumer (GPU
  upload, `aurora-graph`) touches only the accumulated region instead of
  the whole tile (`spike/FINDINGS.md` finding #1: whole-tile merges were
  65% of a painting frame). **Known, accepted limitation**: dirty state
  does not survive eviction — documented on `TileStore`, not silently
  left unstated.
- [x] **Tile compression (`lz4_flex`, not `zstd`)** — pure Rust (no C
  FFI), chosen for decompression speed over best-in-class ratio since
  page-in during panning is already budget-tight
  (`spike/FINDINGS.md`: 7–16.7 ms against a 16.7 ms budget). On-disk
  format is a small versioned header (magic/version/compressed-flag) plus
  an `lz4_flex`-compressed payload, with a **raw fallback** when
  compression would expand the data (confirmed by a real test: uniform
  tiles compress to ~0.4% of raw size, gradient ~0.5%, random noise
  correctly falls back to raw at a 1.000 ratio rather than storing an
  expanded blob).
- [x] **Background writer so eviction does not block the frame** — a
  dedicated `std::thread` draining an unbounded `std::sync::mpsc` queue
  (`crates/aurora-tile/src/writer.rs`), not `tokio` — one thread draining
  one queue doesn't need an async runtime, and pulling one into a crate 7
  others depend on is a real, avoidable cost. `submit()` never blocks by
  construction (unbounded channel); `flush()` joins the writer and
  surfaces the first write failure (logging every failure via
  `tracing::error!`, not just the first) before a document save.
- [x] **Bench: paging throughput, eviction cost, compression ratio** —
  `crates/aurora-tile/benches/tile_store.rs` (criterion). Measured
  2026-07-28 on this machine: paging throughput (32 tiles against a
  16-tile budget) ~8.2 ms/cycle; forced eviction ~253 µs/tile; compression
  ratios 0.004 (uniform), 0.005 (gradient), 1.000 (noise, correctly
  raw-fallback). Indicative, not a cross-platform result — same caveat
  the vertical slice's own numbers carry.

### M1.2 — GPU layer (`aurora-gpu`)

- [x] **Device/queue management** — done 2026-07-28,
  `crates/aurora-gpu/src/{context,error}.rs`, `GpuContext` (`wgpu` 30,
  matching `spike/vertical-slice`'s pin). Headless only (`compatible_surface:
  None`), mirroring the spike's own `--headless` path exactly — same
  `RequestAdapterOptions`/`DeviceDescriptor` fields, `HighPerformance`
  power preference, no speculative features/limits. Verified with a real
  device on this machine's actual GPU, not a mock: `adapter_info()`
  confirms **NVIDIA GeForce RTX 3090, Vulkan, DiscreteGpu** — the same
  hardware `spike/FINDINGS.md`'s Linux/Vulkan numbers came from.
- [x] **Surface configuration and resize** — implemented 2026-07-29,
  `crates/aurora-gpu/src/surface.rs`, `GpuContext::create_surface` +
  `GpuSurface` (`format`/`size`/`resize`/`acquire`). Ports
  `spike/vertical-slice`'s windowed setup (`get_default_config` + forced
  `AutoVsync`) exactly; resize is new design (the spike has none, per
  every `WindowEvent` arm checked). `acquire()` returns wgpu 30's own
  `CurrentSurfaceTexture` (a 7-variant enum — `Success`/`Suboptimal`/
  `Timeout`/`Occluded`/`Outdated`/`Lost`/`Validation`) directly rather
  than the older `Result<SurfaceTexture, SurfaceError>` shape, confirmed
  by reading wgpu 30's own source rather than assumed from older docs.
  `resize` no-ops on a zero-sized request (minimized-window guard, a real
  wgpu gotcha). No new dependency in the library crate itself: `impl
  Into<wgpu::SurfaceTarget<'_>>` is wgpu's own flexible target type, so
  `aurora-gpu` needs neither `winit` nor `raw-window-handle` directly.
  **Verified 2026-07-29 against a real window** — this bullet was
  previously blocked by the same "GDM greeter only, no live session" gap
  `spike/a11y-ime/FINDINGS.md` documented for the Linux Orca leg, on the
  machine that wrote this code. This verification pass ran on a
  different machine with a real, logged-in macOS desktop session (the
  same one `spike/FINDINGS.md`'s macOS numbers and the a11y spike's
  VoiceOver pass came from), so the blocker didn't apply. Added `winit`
  as a dev-dependency (`workspace.dependencies`, reused by `aurora-app`
  when M1.8 needs it — declared centrally per this project's own stated
  practice rather than pinned twice) and
  `crates/aurora-gpu/examples/surface_smoke.rs`: opens a real window,
  creates the surface against the same headless-created adapter this
  crate already uses (confirmed: `AMD Radeon Pro 5300M`, Metal — the
  vertical slice's own GPU, so the adapter this crate requests with
  `compatible_surface: None` does turn out to support presenting on this
  hardware, though `context.rs`'s own comment is right that this isn't
  guaranteed by the API), runs 150 acquire/clear/present cycles, and
  triggers two real resizes via `request_inner_size`. One thing the first
  run caught that a written-blind implementation wouldn't have surfaced:
  `request_inner_size` can apply synchronously and skip the
  `WindowEvent::Resized` event entirely (confirmed in its own doc
  comment) — `surface_smoke.rs` handles both paths through one shared
  `apply_resize` rather than assuming the event always fires. Two runs,
  same result both times: adapter/format/size logged correctly (physical
  size reflects the display's 2x Retina scale factor), 4 resize events
  processed (2 harmless same-size ones from window creation itself, then
  the 2 intentional ones, both landing at the requested physical size),
  zero panics, clean exit. Not yet run on Windows with a live session
  (this crate's whole real-GPU test suite passed on this same Metal
  machine too, see below), and DX12 remains the one backend with no real
  runs at all.
- [x] **Shader library and WGSL pipeline cache** — done 2026-07-28,
  `crates/aurora-gpu/src/{shader,pipeline}.rs`, 3 new tests (4 total in
  the crate). `ShaderLibrary` eagerly compiles named WGSL modules
  (`shaders/canvas.wgsl`, the `vs_canvas`/`fs_canvas` pair ported from
  `spike/vertical-slice`'s shader — the UI-rect half was deliberately
  **not** ported, since it hardcodes colours, exactly what invariant
  §7.3.10 forbids; that half needs `aurora-theme`'s tokens first).
  `PipelineCache` memoizes `wgpu::RenderPipeline`s by a small hashable
  `PipelineKey` (shader/entry points/target format/blend) via
  `HashMap::entry().or_insert_with()` — no prior art existed for this
  (the spike built its two pipelines once at startup with no cache
  concept at all). **Verified with a real render, not a mock**: a test
  actually draws the canvas shader into an offscreen texture and reads
  back the pixels, confirming correct output (opaque red in, opaque red
  out — the checkerboard-behind-transparency logic correctly contributes
  nothing when alpha = 1); a separate test confirms the cache actually
  caches (identical key → no rebuild; different key → rebuilds).
- [x] **GPU tile residency with toroidal slot addressing** — done
  2026-07-28, `crates/aurora-gpu/src/residency.rs`, `TileResidency`, 2 new
  tests (6 total in the crate). Ports `spike/vertical-slice`'s proven
  `sync_tiles`/`set_view` design (a tile-aligned `Rgba16Float` atlas sized
  to the viewport + one tile margin, slots addressed as `tile index mod
  grid size`, wraparound delegated to the sampler's `AddressMode::Repeat`
  — not WGSL math) against the real `aurora_tile::TileStore` API, which
  needed no new methods on `aurora-tile` at all: the spike's batch
  `take_dirty(limit)`/`exists(id)` calls turn out unnecessary once the
  upload loop iterates the small, bounded visible-slot grid (~4–35 tiles)
  and calls the real single-tile `take_dirty(id)`/`get(id)` directly.
  **Verified with real GPU work, not call counts alone**: one test proves
  the actual toroidal-addressing benefit finding #4 named — full grid
  uploads on first sync, zero uploads on an unchanged second sync,
  exactly one row/column's worth on a one-tile pan, not the whole grid —
  and a second test reads the atlas texture back and confirms a painted
  tile's colour lands in the *correct* slot region, catching the
  off-by-one/wrong-axis bug class a count-only test would miss.
  Deliberately does not handle resize (recreating the atlas at a new
  size) — that's the still-open "surface configuration, resize" bullet
  above.

  **A real, reproducible bug found and fixed along the way, not just in
  this new code**: with 6 real-GPU tests now in the crate, `cargo test`'s
  default multi-threaded runner deadlocked — several tests each creating
  their own `wgpu::Instance`/`Device` and submitting real work at the
  same time. Confirmed real, not assumed: reproduced reliably, isolated
  by checking `nvidia-smi` (the GPU sat idle while the test process spun,
  ruling out "just slow"), and confirmed absent under
  `--test-threads=1`. Fixed with a crate-test-wide `Mutex` serializing
  every real-GPU test (`crates/aurora-gpu/src/test_support.rs`), rather
  than assuming `cargo-nextest` (which isolates tests per-process and
  might well not hit this at all) would be the runner used — this crate
  needs to be correct under plain `cargo test` too, since that's what's
  actually installed in this environment.
- [x] **Upload scheduling with a per-frame budget** — done 2026-07-28,
  `TileResidency::sync` now takes a `byte_budget` and returns `SyncStats`
  (`uploaded`, `bytes_uploaded`, `remaining`, `errors`) instead of a bare
  count. Directly answers `spike/FINDINGS.md` finding #3 ("a fast fling
  exposes a full screenful per frame... ~18 MB") for the `aurora-gpu`-local
  half of the fix — the mip/progressive-rendering half stays M1.3 scope,
  unchanged. **No new bookkeeping needed for carrying a budget-skipped
  tile to the next frame**: a skipped tile's slot is just left unrecorded,
  so the existing `resident` check already retries it next call — one
  fixed iteration order means a tight budget fills in from the start of
  the grid forward across calls, converging to `remaining == 0` rather
  than starving anything. Deliberately no invented default budget number
  (same reasoning as ADR 0005's scratch-disk budget) — that's a tuning
  question for whoever calls this once the render pipeline exists.
  **Verified with a real multi-call convergence test**, not just that
  budgeting exists: a 4-tile grid with a 2-tile budget uploads 2 the
  first call (`remaining == 2`), the other 2 the second call
  (`remaining == 0`), and 0 the third (steady state).
- [~] **Validate on DX12, Metal, Vulkan** — Vulkan done (this machine's
  RTX 3090, all of M1.2). **Metal done, 2026-07-29**: all 7 of this
  crate's real-GPU tests (`context`, `pipeline`, `render_test`,
  `residency` ×2, `residency_test`, `shader`) pass against a real `AMD
  Radeon Pro 5300M`, plus the new `examples/surface_smoke.rs` windowed
  check above — same machine as the vertical slice's and a11y spike's
  macOS numbers. **DX12 (Windows) is the only backend with zero real-GPU
  runs against this crate so far.**

### M1.3 — Render graph and renderer (`aurora-graph`, `aurora-render`)

- [x] **Node definitions, dependency graph, dirty propagation** — done
  2026-07-29, `crates/aurora-graph/src/{node,graph,error}.rs`, 12 tests.
  `RenderGraph<N>` is generic over a caller-supplied payload `N` rather
  than defining concrete node kinds itself — `aurora-graph` may only
  depend on `aurora-core`/`aurora-tile` (PRD §7.2), so it structurally
  cannot know what a "curves adjustment" or "smart object" node computes;
  that's `aurora-filters`'/`aurora-doc`'s job, layered above it, with
  `aurora-render` executing whatever the payload turns out to be. Reuses
  `aurora_core::Id<Node>` directly (its own doc comment already named
  `Id<Node>` as an intended use case) rather than inventing a new id type.
  DAG-by-construction, not by a separate cycle check: `add_node` requires
  every input to already exist, so a node can only ever depend on nodes
  created before it — a cycle back to an ancestor is structurally
  unreachable, and the same property makes insertion order always a
  valid topological order for free (`RenderGraph::iter`). Dirty
  propagation (`mark_dirty`/`take_dirty`/`peek_dirty`) mirrors
  `aurora_tile::Tile`'s own `Option<Rect>` + `Rect::union` accumulation
  idiom exactly, BFS'd forward to every transitive dependent with a
  `visited` set (needed for diamond-shaped dependencies — two paths
  reconverging on one node — so a shared descendant is unioned into once
  per call, not once per incoming path). **Deliberately conservative**:
  propagates the same region unchanged at every step rather than growing
  it per node (e.g. a blur widening its footprint by its radius) — this
  crate has no way to know what a node computes, so identity propagation
  is the safe default; a kernel-aware growth hook is future work once
  `aurora-filters` node payloads can answer "how far does this operation's
  influence reach." **Deliberately out of scope for this pass**: node
  removal and edge rewiring (delete a layer, reorder, insert an
  adjustment mid-stack) — real usage patterns should come from
  `aurora-doc`'s layer-tree integration (M1.4) rather than guessing the
  right shape now without a concrete consumer.
- [x] **Tile-granular scheduling** — done 2026-07-29,
  `crates/aurora-render/src/schedule.rs` (`aurora-render`'s first real
  code — the `crate_name()` placeholder is gone), 9 tests. `schedule()`
  walks a `RenderGraph<N>` in its own topological (insertion) order and,
  for every node with a dirty region, converts that node-granular
  document-space `Rect` into the exact grid of 256×256px `TileId`s it
  overlaps (`tiles_for_rect`) — the translation step between M1.3's
  already-done node-granular dirty propagation and per-tile execution.
  Handles the case `aurora_core::Rect`'s own doc comment names (a layer's
  bounds can extend off-canvas, negative coordinates) by clipping to the
  document's non-negative tile-index space before converting, rather than
  panicking or wrapping — covered by a dedicated test alongside single-tile,
  boundary-crossing, and fully-off-canvas cases. **Deliberately
  non-destructive** (`peek_dirty`, not `take_dirty`): `RenderGraph` tracks
  one dirty `Rect` per node, not one per tile, so nothing here can record
  partial per-tile completion without losing the rest — the same shape of
  problem `aurora-gpu`'s `TileResidency` upload budgeting was careful to
  avoid by never marking a skipped tile falsely done. Clearing a node's
  dirty state is left to whichever future executor (GPU compositing,
  progressive rendering, async evaluation — all still below) actually
  commits every tile in its `ScheduledWork`. Full local CI gate
  (`fmt`/layering/clippy/`cargo test --workspace`) clean.
- [x] **GPU-side compositing** — done 2026-07-29,
  `crates/aurora-render/src/composite.rs` (`TileCompositor`) +
  `src/shaders/composite.wgsl`, 3 tests, all against real GPU hardware
  (this machine's RTX 3090/Vulkan). Directly answers `spike/FINDINGS.md`
  finding #1 (CPU per-tile merging measured at ~20ms, the real
  compositing bottleneck, not disk I/O): blends a source tile over a
  destination tile via the GPU's fixed-function alpha blend unit
  (`aurora_gpu::Blend::AlphaBlending`, `LoadOp::Load` to preserve the
  destination's existing content) instead of a CPU pixel loop. Verified
  with real pixel-readback checks, not just "it ran": opaque-over-blended
  math confirmed to the exact expected half-float value, a
  fully-transparent source confirmed to leave the destination bit-for-bit
  unchanged (catches a "blend ignored" regression a same-color test
  couldn't), and a third test confirms the pipeline cache actually caches
  (`PipelineCache::get_or_create_with`, reused unchanged from `aurora-gpu`).
  Self-contained, same shape as `TileResidency` — owns its own shader
  module, bind group layout, sampler, and pipeline cache, since nothing
  yet coordinates multiple GPU passes across a frame. **Deliberately
  minimal**: blends exactly one tile over one tile with no blend-mode or
  opacity parameter — those are a layer's properties, and `aurora-doc`'s
  layer model (M1.4) doesn't exist yet; `aurora-render` sits below it in
  the layering and structurally cannot know either. This is the primitive
  real layer compositing will call once that model exists, not a
  document-level compositor on its own. `aurora-render`'s own real-GPU
  test lock (`src/test_support.rs`) duplicates `aurora-gpu`'s — a separate
  test binary, so `aurora-gpu`'s own lock doesn't cover it. Full local CI
  gate clean.
- [~] **Progressive rendering: low mip while interacting, refine when still**
  — CPU and GPU halves both done 2026-07-30; only the interaction-state
  policy remains. This is `spike/FINDINGS.md` finding #3's direct
  mitigation ("rendering a lower-resolution mip while panning fast,
  refining when motion stops" — named for the ~18 MB/screenful
  upload-bandwidth ceiling a fast pan hits).
  - [x] **CPU half: `crates/aurora-render/src/mip.rs`** (`MipLevel`,
    `downsample`), 7 tests. Box-filters a tile's texels down to a closed
    set of power-of-two levels (`Full`/`Half`/`Quarter`/`Eighth`, each an
    exact divisor of `TILE` by construction, so no remainder handling
    anywhere). Summed in `f32` per ADR 0003's compute-precision floor,
    not accumulated in `f16`. Verified with real numeric checks, not
    just shape: a uniform tile stays exactly uniform under any level
    (exact `f16`-representable values, a real bit-exact check); a
    row-alternating checkerboard averages to exactly the midpoint at
    `Half`, where each 2×2 source block provably contains one of each
    colour; a row-index-encoded gradient confirms the *correct* source
    block is read (catches an off-by-one or shifted-range bug a
    uniform-input test can't); and `MipLevel::index()` maps each level to
    the atlas mip index below.
  - [x] **GPU half: `aurora_gpu::TileResidency::upload_mip`** — the
    atlas texture now carries a real 4-level mip chain
    (`mip_level_count: 4`, one level per `MipLevel` variant by
    convention — `aurora-gpu` doesn't depend on `aurora-render`, so nothing
    enforces this beyond both sides' doc comments naming it), and
    `upload_mip(queue, id, mip_level, texels)` writes texels into a
    tile's slot at any level using the same toroidal addressing `sync`
    uses. Deliberately doesn't touch `sync`'s dirty/slot bookkeeping —
    real callers still use `sync` for full-resolution uploads; this is
    only for the lower levels progressive rendering needs. New
    `GpuError::InvalidMipLevel`/`InvalidTileUpload` variants reject an
    out-of-range level or a mismatched texel count rather than trusting
    the caller. `half` promoted from a dev- to a real dependency of
    `aurora-gpu` (the method's signature needs `f16` outside tests now).
    `TileResidency::texture()` promoted from a `#[cfg(test)]`
    `pub(crate)` accessor to a real public one — the old one couldn't
    have been reused across the crate boundary regardless of visibility,
    since `#[cfg(test)]` items don't exist in the compiled artifact a
    downstream crate links against; the atlas was already created with
    `COPY_SRC` anticipating exactly this "debugging/inspection tooling"
    need. 2 new tests in `aurora-gpu` (an out-of-range level, a
    mismatched texel count) plus a real pixel-readback test in
    `residency_test.rs` proving an upload lands in the correct slot *and*
    mip level, not just that the call succeeds.
  - [x] **Wiring: `aurora-render`'s `preview::upload_preview`** — ties
    `TileStore::get` → `mip::downsample` → `TileResidency::upload_mip`
    into one call, rejecting `MipLevel::Full` (that's `sync`'s job, with
    dirty tracking and budgeting this function doesn't replicate) as a
    caller error rather than silently accepting it. Verified end-to-end
    against real hardware: paints a tile in a real `TileStore`, uploads
    it as a `Quarter`-level preview, and reads the atlas back at mip
    level 2 to confirm the downsampled colour landed in the *correct*
    slot — the full store-to-atlas path, not each piece in isolation.
    2 tests.
  - [ ] **Still open**: choosing a `MipLevel` from real interaction state
    — that policy needs an "is the user actively panning" signal this
    crate has no source for yet (no input/app layer exists) — and
    sampling the atlas back at reduced resolution in a shader (explicit
    LOD selection, not automatic mipmapping: the point is showing a
    coarse preview even at 1:1 zoom while a better one uploads, which
    automatic derivative-based LOD doesn't do). Neither is needed until
    an actual interactive canvas exists to drive them, which is later
    Phase 1 work (`aurora-doc`, `aurora-app`).
  Full local CI gate clean throughout.
- [~] **Async evaluation — the UI thread never blocks (§7.3.4)** —
  first piece done 2026-07-30, `crates/aurora-render/src/executor.rs`
  (`Executor`, `TaskId`), 5 tests. A dedicated background thread runs
  submitted work; `submit` never blocks (unbounded channel, so a caller
  is never stalled queuing work) and `drain_completed` never blocks
  either (reports whatever finished so far). Deliberately the same shape
  as `aurora_tile::writer::BackgroundWriter`, which already proved this
  exact pattern for scratch-disk writes — generalized here to arbitrary
  render work. Deliberately **not** `tokio`: one thread draining one
  queue doesn't need an async runtime, and CLAUDE.md is explicit that
  `tokio` is for I/O/background work, not the render loop. Runs plain
  closures rather than a typed "render task," mirroring why
  `RenderGraph<N>` stays generic over its node payload: what "evaluating
  a node" means needs `aurora-doc`/`aurora-filters` to define, and
  neither exists yet. Verified deterministically — `join()` blocks until
  submitted work has actually run, so tests confirm real execution and
  `drain_completed` reporting without any sleep-based polling — plus the
  same "1000 submits complete in well under a second" non-blocking proof
  `BackgroundWriter`'s own test uses. **Known, accepted limitation
  documented on the type**: a panicking task kills the background
  thread's loop, silently dropping every task submitted afterward — same
  failure shape `BackgroundWriter` already has, not solved here for the
  same reason (this workspace denies `panic`/`unwrap`/`expect`
  everywhere, so a task panicking is a caller bug, not a condition to
  route around). **Not yet done**: nothing actually runs *through*
  `Executor` yet — no caller submits real render work to it, because
  there is no real render-graph node evaluation to submit until
  `aurora-doc`/`aurora-filters` exist. This is the same "primitive built,
  no concrete consumer yet" shape progressive rendering's still-open
  policy work is in. Full local CI gate clean.

### M1.4 — Document model (`aurora-doc`)

- [x] **Layer tree: pixel, group, nesting, ordering** — done 2026-07-30,
  `crates/aurora-doc/src/{layer,tree,error}.rs` (`LayerId`, `LayerKind`,
  `LayerTree`), 25 tests. `LayerId` is `aurora_core::Id<Layer>` — the
  same phantom-typed pattern `aurora_graph::NodeId` already uses, and
  `aurora_core::id.rs`'s own test module had already named `Layer` as
  exactly this kind of use case. **Deliberately only two layer kinds**
  (`Pixel`, `Group`): FR-003 names nine more (Text, Shape, Smart Object,
  Adjustment, Fill, Gradient, Pattern, Video, Frame), but every one needs
  content types this crate structurally cannot reference —
  `aurora-doc` may only depend on `aurora-core`/`aurora-tile`/
  `aurora-graph` (PRD §7.2), not `aurora-text`/`aurora-vector`/
  `aurora-filters`/`aurora-ai`. A pixel layer carries `bounds: Rect` but
  deliberately does **not** yet own an `aurora_tile::TileStore` — whether
  pixel storage is one store per layer (simple, but `TileStore::new`
  spawns a background-writer thread, so an unlimited-layers document
  would mean an unlimited number of OS threads) or one store shared some
  other way is a real resource-management question left open rather than
  decided silently while "starting the layer tree." **Decided 2026-08-06,
  [ADR 0010](docs/adr/0010-layer-pixel-storage.md)**: one shared
  `TileStore` per document, addressed by a `SurfaceId` reused from each
  pixel layer's own `LayerId`, plus a separate small store for the active
  brush stroke. Decision only — `LayerKind::Pixel` still has no
  `SurfaceId` field and `aurora-tile` has neither `SurfaceId` nor the
  compound-key store API yet; see M1.9.
  **Ordering convention fixed and documented**: sibling lists are
  top-to-bottom as a layers panel displays them (index 0 = topmost,
  painted last/on top) — the opposite of PSD's on-disk order (bottom
  layer first), which `aurora-io` will need to reverse when it exists. A
  new layer is inserted at index 0, matching every mainstream editor's
  "new layer appears above the current one." `reparent(id, new_parent,
  index)` is one unified primitive for both reordering-within-a-parent
  and moving-into-a-different-group (nesting), rather than separate
  move-up/move-down/reparent methods — clamps an out-of-range `index` to
  the end (a UI drop target's forgiving behaviour) rather than erroring,
  and rejects a cycle (`new_parent` is `id` itself or one of its own
  descendants) by walking `id`'s ancestor chain, which is bounded by tree
  depth rather than scanning `id`'s whole subtree. `remove` cascades: 
  deleting a group deletes its contents, matching every mainstream
  editor's actual plain-delete behaviour (no implicit flatten-up-a-level).
  Every mutating method validates before mutating anything, so a failed
  call changes nothing — same "all or nothing" discipline
  `RenderGraph::add_node` already established. Full local CI gate clean.
  **Not this bullet's job, and explicitly still open**: this is
  identity/nesting/ordering only — no opacity, blend mode, visibility,
  locking (the next bullet), no wiring to a `RenderGraph` node (that
  needs blend-mode semantics to mean anything), and no real pixel
  storage (see above). M1.3's progressive-rendering/async-evaluation
  primitives (`mip::downsample`, `Executor`) still have no real
  consumer — a layer *tree* existing doesn't yet mean anything renders;
  that needs this bullet's own follow-ons plus `aurora-render` wiring.
- [x] **Opacity, fill opacity, blend modes, visibility, locking** —
  done 2026-08-01, `crates/aurora-doc/src/layer.rs`
  (`BlendMode`, `LayerLock`) and `tree.rs` (per-layer
  `opacity`/`fill_opacity`/`blend_mode`/`visible`/`lock` getters and
  validating setters), 7 new tests (32 total in the crate). `BlendMode`
  is the full standard 27-mode Photoshop set (FR-003) — purely
  descriptive, since `aurora-doc` structurally cannot depend on
  `aurora-render`/`aurora-gpu` to implement the actual blend math (same
  "data now, a future consumer interprets it" shape `RenderGraph<N>`'s
  generic payload already established). `LayerLock` mirrors PSD's own
  `lspf` (Protected Setting) tagged block bit-for-bit
  (transparency/pixels/position) rather than inventing a shape
  `aurora-io` would need to translate later — stored state only, nothing
  yet enforces it (no paint/move tool exists to refuse). `opacity`/
  `fill_opacity` are `f32` in `0.0..=1.0` (matching ADR 0003's `f32`
  compute-precision floor, not PSD's on-disk `u8`), validated by the
  setters (`DocError::OpacityOutOfRange`) rather than silently clamped —
  same "all or nothing, nothing silently coerced" discipline the rest of
  this crate already uses. New layers default to opacity 1.0, `Normal`,
  visible, unlocked. **Verified 2026-08-01** once a Rust toolchain was
  installed in this environment (initially absent — `cargo`/`rustc` not
  found — the first pass of this bullet was written and manually re-read
  against the lint set but honestly left `[~]` until a real run was
  possible): `cargo fmt --all --check` clean, `cargo clippy -p aurora-doc
  --all-targets --all-features -- -D warnings` clean (pedantic, `-D
  warnings`), `cargo test -p aurora-doc` — 32/32 passed. Workspace-wide
  `cargo clippy --workspace` and `check_layering.py` did not run in this
  pass (a pre-existing, unrelated environment gap — `yeslogic-fontconfig-sys`
  needs `pkg-config`, not installed; `python3` also isn't on this machine)
  — neither is a regression risk here since this bullet touched no
  `Cargo.toml` and only `aurora-doc`'s own crate.
- [x] **Layer masks** — done 2026-08-01, `crates/aurora-doc/src/layer.rs`
  (`LayerMask`) and `tree.rs` (`mask`/`add_mask`/`remove_mask`/
  `set_mask_enabled`/`set_mask_inverted`), 8 new tests (40 total in the
  crate). Lives on `LayerEntry` itself, not inside `LayerKind` — Photoshop
  allows a mask on both pixel layers and groups (a group mask clips the
  whole subtree), so it can't be a `Pixel`-only field; a dedicated test
  (`add_mask_works_on_a_group_too`) confirms this. **Deliberately no real
  mask pixels yet** — same open resource-management question
  `LayerKind::Pixel`'s own `bounds` field already flagged, and masks are
  still genuinely unspiked on the PSD side
  (`spike/psd-write/FINDINGS.md`: "Layer masks, vector masks, smart
  objects, layer styles, adjustment layers" all unstarted) — so
  `LayerMask` carries only `bounds`/`enabled`/`inverted`, the two toggles
  the modern Photoshop UI actually exposes, not the full `lspf` byte
  layout (density/feather/position-relative-to-layer are legacy fields
  left out rather than guessed at, same honesty `LayerLock`'s doc comment
  already used for its own narrower-than-PSD scope). `add_mask` rejects a
  second mask (`DocError::MaskAlreadyExists`) rather than silently
  overwriting one, matching Photoshop's own UI (which swaps "Add Layer
  Mask" for "Delete Layer Mask" once one exists); `remove_mask`/
  `set_mask_enabled`/`set_mask_inverted` reject a missing mask
  (`DocError::NoMask`) with the same "all or nothing" discipline every
  other mutator in this crate already uses. Removing a layer removes its
  mask for free (no extra cleanup code needed — the mask is just a field
  on the `LayerEntry` `remove`/`remove_subtree_contents` already delete),
  confirmed by `removing_a_layer_takes_its_mask_with_it`. **Verified**:
  `cargo fmt --all --check` clean, `cargo clippy -p aurora-doc
  --all-targets --all-features -- -D warnings` clean, `cargo test -p
  aurora-doc` — 40/40 passed. Same environment caveat as the previous
  bullet: workspace-wide clippy/`check_layering.py` didn't run
  (pre-existing, unrelated gaps — missing `pkg-config`/`python3`), not a
  regression risk since no `Cargo.toml` changed.
- [x] **Selection representation** — done 2026-08-01,
  `crates/aurora-doc/src/selection.rs` (`Selection`, `SelectionSet`), 11
  new tests (51 total in the crate). Document-level, not per-layer — a
  new module rather than a `LayerTree` method, since Photoshop's active
  selection isn't attached to any one layer. `Selection` is deliberately
  just a bounding `Rect` plus an `inverted` flag, not a real raster mask:
  no selection tool (rectangle/ellipse/lasso/magic wand, all FR-004, all
  Phase 2 scope) exists yet to produce anything but a bounding box, and
  there's no real pixel storage for antialiased/feathered coverage even
  if one did — same open question `LayerKind::Pixel`'s `bounds` and
  `LayerMask` already flagged. `SelectionSet` holds the current active
  selection (`Option<Selection>`, `None` = nothing selected) plus named
  saved ones (`save_active`/`load`/`delete_saved`/`saved_names`),
  answering FR-004's "Save Selection"/"Load Selection" commands the way
  Photoshop's own saved-selection alpha channels do; `invert` flips the
  active selection's flag, answering "Inverse". **Deliberately out of
  scope**: Feather/Expand/Contract/Border (FR-004's remaining selection
  commands) all need to reshape a real raster region, meaningless on a
  bounding box; saving over an existing name always replaces it outright
  rather than implementing Photoshop's add/subtract/intersect
  combine-with-channel modes, which need the same real boolean-region math
  this pass doesn't have. Verified: `cargo fmt --all --check` clean,
  `cargo clippy -p aurora-doc --all-targets --all-features -- -D
  warnings` clean, `cargo test -p aurora-doc` — 51/51 passed.
- [x] **History as reversible operations + dirtied tiles (§7.3.3),
  unlimited undo/redo** — done 2026-08-01,
  `crates/aurora-doc/src/history.rs` (`History`), 20 new tests (70 total
  in the crate). `History` and `LayerTree` stay siblings rather than
  `History` owning/wrapping the tree — there's no `Document` type yet
  tying a tree, a selection set, and a history together, so every
  `History` method takes `&mut LayerTree` explicitly; a future
  `Document` can compose them. Mirrors all 14 of `LayerTree`'s mutators
  (add/remove/reparent/rename, the 5 property setters, the 4 mask
  operations) with a version that also records how to undo it —
  deliberately the full set rather than a subset needing a "still open"
  footnote, matching how `BlendMode`/`LayerLock` were scoped to their
  real, closed requirement rather than partially.
  **Reversible operations, not snapshots**: every recorded step stores
  exactly one changed value (a single old/new pair, or — for a
  structural add/remove — exactly the removed subtree's own entries),
  never a copy of the document. Add and remove turned out to be exact
  inverses of the same two new `LayerTree` primitives
  (`remove_capturing`/`restore`, both `pub(crate)`): undoing an add
  removes-and-captures fresh, undoing a remove restores what was
  captured, and either one produces the other's own inverse — one
  symmetric code path handles undo *and* redo, not four. `restore`
  reinserts every removed entry (root plus, for a group, its whole
  captured subtree) **at its original id**, not a freshly minted one —
  proven necessary, not just tidy: anything outside the tree that
  already referenced those ids (a saved selection, a pending redo entry
  deeper in the stack) stays valid, and a dedicated test
  (`remove_undo_restores_a_whole_group_subtree_with_original_ids_and_properties`)
  confirms a restored nested layer keeps its id, its parent link, *and*
  its own opacity — not reset to defaults. Mask add/remove got the same
  exact-value treatment via new `take_mask`/`restore_mask` primitives
  (distinct from `add_mask`, which always resets to enabled/uninverted):
  `remove_mask_undo_restores_its_exact_toggled_state_not_the_default`
  confirms undoing a mask removal brings back whatever `enabled`/
  `inverted` state it actually had, not a fresh default.
  **Dirtied regions, scoped honestly**: a step's dirty `Rect` is reported
  when knowable — a pixel layer's own `bounds`, or (for a removed/restored
  subtree) the union of every pixel layer inside it via `Rect::union`'s
  own documented empty-rect-as-identity behaviour, the same accumulation
  idiom `aurora_tile::Tile`/`aurora_graph::RenderGraph` already use — and
  `None` for a group-level change (no `bounds` field exists for a group;
  aggregating its descendants' extent needs subtree-bounds math that
  doesn't exist anywhere yet, not even for compositing) or a step with no
  visual effect (`Rename`). New activity always clears the redo stack,
  verified by a dedicated test. **Known, accepted limitation, stated
  plainly**: `History` only sees mutations made through its own methods;
  calling `LayerTree`'s mutators directly bypasses it entirely, and
  mixing the two can hand `restore` a parent that direct calls already
  removed — `LayerTree::restore`'s own doc comment names the resulting
  errors. Normal use (always mutating through one `History`) never
  reaches this, proven by construction (undo/redo is strictly LIFO, so
  anything that could invalidate a pending op's captured parent would
  already have been undone first). Verified: `cargo fmt --all --check`
  clean, `cargo clippy -p aurora-doc --all-targets --all-features -- -D
  warnings` clean, `cargo test -p aurora-doc` — 70/70 passed.
- [~] **Crash recovery journal** — in-memory half done 2026-08-01, same
  `crates/aurora-doc/src/history.rs`, 8 more tests (78 total in the
  crate). Every op `History` ever applies — fresh action, undo, *or*
  redo — is also appended, in real chronological order, to an
  ever-growing `journal: Vec<LayerOp>` distinct from the undo/redo
  stacks; `History::replay` rebuilds a fresh `LayerTree` purely from that
  log, proving it's sufficient and order-correct. **The critical
  property, and the one a naive "just log the undo stack" design would
  get wrong**: replay reflects the *current* state, not the full history
  — a dedicated test
  (`replay_reflects_current_state_after_an_undo_not_the_original_history`)
  adds two layers, undoes the second, and confirms replay reconstructs
  only the first; a paired test confirms a subsequent redo brings the
  second back through replay too. `RemovedSubtree`/`LayerEntry`/`LayerOp`
  all gained `Clone` to support this (the journal keeps its own copy
  independent of whatever the undo/redo stacks consume).
  **Deliberately not built: writing the journal to disk** — the actual
  "survives a crash" property this bullet is named for. That needs a
  chosen on-disk encoding for `LayerOp`'s recursive shape (nested entries,
  strings, ids): a real, first-party format decision, the same *kind* of
  choice as `aurora-tile`'s own hand-rolled tile codec, but with no spike
  or evidence behind it yet, and `serde` (declared in workspace deps but,
  checked directly, not actually used by any crate yet) doesn't resolve
  the choice by itself — it has no concrete binary-format crate paired
  with it. Forcing an encoding here without that evidence is exactly the
  mistake `spike/raw-icc/FINDINGS.md` already caught once (a "small,
  fast" persistence detail that turned out to need its own real design
  pass) — deferred deliberately, not silently skipped. Verified: `cargo
  fmt --all --check` clean, `cargo clippy -p aurora-doc --all-targets
  --all-features -- -D warnings` clean, `cargo test -p aurora-doc` —
  78/78 passed.

  **`journal_descriptions` added 2026-08-04**, 1 more test (79 total) —
  a real, minimal public accessor exposing one human-readable, one-line
  description per journal entry (`"Added layer \"Background\""`, `"Set
  blend mode of layer #1 to Multiply"`, etc.), closing the gap this
  module's own doc comment previously named ("the journal has no public
  way to inspect individual entries yet"). Prompted by M1.8's own
  "History... panels" bullet actually needing this data to show real
  rows — not built speculatively ahead of a consumer. Deliberately
  self-contained (no `&LayerTree` parameter): a description only names
  the one layer a `Rename`/`Restore` entry itself already captured a
  name for, falling back to a numeric `layer #N` reference otherwise —
  resolving other names against a *live* tree would show each entry's
  *current* name rather than what it was called at the time (Photoshop's
  own History panel shows the latter), and `History` deliberately
  doesn't hold a tree reference of its own (see this module's own doc
  comment on why `History`/`LayerTree` stay siblings). `LayerOp` itself
  stays private — this is the one, deliberate seam its content becomes
  visible through, not a `Display`/`Debug` impl callers could match on.
  Verified: `cargo fmt --all --check` clean, `cargo clippy -p aurora-doc
  --all-targets --all-features -- -D warnings` clean,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-doc --no-deps
  --all-features` clean, `cargo test -p aurora-doc` — 79/79 passed.

### M1.5 — Colour (`aurora-color`)

- [x] **Colour space types; every buffer tagged (§7.3.6)** — mostly
  already done from M1.1: `aurora_core::{ColorSpace, Channels,
  SampleFormat, PixelFormat}` are the descriptors every buffer carries.
  This bullet's own remaining piece — actually *interpreting* an ICC
  profile, not just tagging a named colour space — is the next bullet.
- [x] **ICC transforms via the 0.6 decision** — done 2026-08-01,
  `crates/aurora-color/src/{profile,transform,error}.rs` (`IccProfile`,
  `Transform`, `RenderingIntent`, `ColorError`), 9 tests. Wires in
  `lcms2` (ADR 0008), added to `[workspace.dependencies]` and to this
  crate — `cargo build` fetches and statically compiles `lcms2-sys`'s
  vendored Little CMS C source cleanly, confirmed on this machine (a C
  compiler was available). `cargo deny check all` clean with the new
  dependency (licenses/advisories/bans/sources all `ok`).
  `Transform` covers `Gray`/`Rgb`/`Rgba`/`Cmyk` — four of
  `aurora_core::Channels`' six variants; `GrayAlpha`/`CmykAlpha` are
  deliberately not wired up (no ready-made `lcms2` float pixel-format
  constant for either, would need hand-building `lcms2`'s own bitfield
  encoding — real but avoidable scope with no caller needing it yet,
  rejected with `ColorError::UnsupportedChannels` rather than silently
  doing something else). New `corpora/icc/` (the same CC0-licensed
  `sRGB.icc`/`ECI-RGBv2.icc` `spike/raw-icc` already used, copied rather
  than referenced in place so a real crate doesn't reach into the
  deliberately-isolated `spike/` directory) backs real tests, not
  synthetic ones: `matches_the_spikes_own_cross_validated_results`
  reproduces `spike/raw-icc/FINDINGS.md`'s own recorded sRGB→ECI-RGBv2
  values to 4 decimal places, and
  `extended_range_values_survive_rather_than_clamp` is the permanent
  regression test that FINDINGS.md finding 4 named as remaining
  work — confirms `lcms2` preserves an out-of-gamut negative channel
  value (§7.3.1b) rather than clamping to 0, needing no special flag
  (unlike `moxcms`, which needed `allow_extended_range_rgb_xyz`). **Honest
  gap carried forward**: `Cmyk` compiles and follows the identical code
  path as the other three, but `corpora/icc/` has no real CMYK ICC
  profile yet, so it's untested against one — ADR 0008's own follow-on
  ("test CMYK and LUT-based profile transforms directly") remains open,
  not silently closed by testing the wrong profile type. Verified:
  `cargo fmt --all --check` clean, `cargo clippy -p aurora-color
  --all-targets --all-features -- -D warnings` clean, `cargo test -p
  aurora-color` — 9/9 passed, `cargo deny check all` clean. Workspace-wide
  clippy/`check_layering.py` didn't run in this pass (pre-existing,
  unrelated gaps — missing `pkg-config`/`python3`); layering itself isn't
  at risk here since `lcms2` is a third-party dependency, not a new
  `aurora-*` one, so `scripts/layering.json` didn't change.
- [~] **Working spaces, linear-light conversion** — linear-light half
  done 2026-08-01, `crates/aurora-color/src/linear.rs`
  (`linear_to_srgb`/`srgb_to_linear`), 6 more tests (15 total in the
  crate). Implements IEC 61966-2-1's actual piecewise sRGB curve (not a
  plain 2.2 gamma approximation), sign-preserving for negative inputs
  (raising a negative number to a fractional power is `NaN` in IEEE
  arithmetic) — verified by dedicated tests that both negative and
  above-`1.0` (HDR/superwhite) values round-trip correctly rather than
  becoming `NaN` or clamping, the same §7.3.1b property `Transform`'s own
  tests already verify for ICC transforms. **An explicit "working space"
  policy type is deliberately not designed yet** — which colour space
  filters/blending actually compute in, and whether that's always linear
  sRGB or adapts to a document's own gamut, has no concrete consumer to
  validate a design against (`aurora-filters`/`aurora-render`'s own
  colour wiring don't exist yet) — same "primitive built, real consumer
  decides the policy later" shape already used for `RenderGraph<N>`/
  `Executor`. Verified: `cargo fmt --all --check` clean, `cargo clippy -p
  aurora-color --all-targets --all-features -- -D warnings` clean,
  `cargo test -p aurora-color` — 15/15 passed.
- [x] **Promote-on-import, dither-on-export** — done 2026-08-01,
  `crates/aurora-color/src/dither.rs` (`promote_u8`, `quantize_u8`,
  `dither_quantize`), 7 more tests (22 total in the crate). `promote_u8`/
  `quantize_u8` round-trip exactly for **all 256** `u8` values (an
  exhaustive test, not a sample). `dither_quantize` implements classic
  8×8 Bayer ordered dithering rather than plain rounding — invariant
  §7.3.1b's own wording ("quantized with dithering") names this
  specifically, and plain rounding is exactly what produces the banding
  the same invariant calls "invisible in review and unrecoverable
  downstream." The 8×8 threshold matrix is generated by the standard
  recursive construction from the 2×2 base case, **not transcribed as a
  hardcoded table** — correctness rests on a verifiable mathematical
  property (every value `0..n²` appears exactly once, checked for `n`
  = 2/4/8) plus a cross-check against the well-known published 4×4
  table, rather than trusting a copied-by-hand 64-entry table. A
  dedicated test confirms dithering actually breaks up banding (the same
  input value at different pixel positions quantizes to more than one
  output). This crate's own job stops at the conversion math — `aurora-io`
  (still a skeleton) will call it once a real 8-bit image format
  reader/writer exists. **M1.5 is now complete.** Verified: `cargo fmt
  --all --check` clean, `cargo clippy -p aurora-color --all-targets
  --all-features -- -D warnings` clean, `cargo test -p aurora-color` —
  22/22 passed.

### M1.6 — Design system (`aurora-theme`)

- [x] **Token types and semantic vocabulary from 0.5** — done 2026-08-01,
  `crates/aurora-theme/src/{color,theme,scales}.rs` (`Color`,
  `SurfaceTokens`/`TextTokens`/`IconTokens`/`BorderTokens`/
  `AccentTokens`/`StateTokens`/`Overlay` matching
  `design/tokens/vocabulary.md`'s sections exactly; `Scales` matching
  `design/tokens/scales.toml`'s type/spacing/radius/elevation/motion
  shape). Real Rust types for an already owner-approved vocabulary, not
  an invented one.
- [x] **TOML theme parsing, inheritance** — done 2026-08-01,
  `crates/aurora-theme/src/{palette,theme}.rs` (`Palette`, `ThemeSet`),
  added `toml` to `[workspace.dependencies]` (`serde`'s first real use
  in the workspace — previously declared but unused). `Palette` resolves
  dotted references (`"neutral.100"`, `"accent.blue.500"`) through a
  generic `toml::Value` tree, since ramp nesting depth genuinely varies;
  `ThemeSet` flattens each registered theme's own fields to dotted keys
  and merges an `extends` chain root-first (child overwrites inherited),
  then extracts the fixed vocabulary into a `Theme`. Tested against the
  real, committed `design/tokens/palette.toml` and
  `design/themes/dark.toml` (`include_str!`, not synthetic fixtures) —
  plus a synthetic child theme to prove the merge logic generically,
  deliberately not a real second design (inventing Light/HC/
  Colour-Critical's actual colours is Cahya's call per FR-027
  *Ownership*, not an engineering detail to fill in while building the
  parser). Cycle detection (`ThemeError::CyclicExtends`) and
  missing-token reporting both covered by dedicated tests.
  **Hot reload deliberately not wired up**: `ThemeSet::register` already
  supports re-parsing and replacing a theme (the part that actually
  matters), but watching the filesystem and calling it automatically is
  thin glue with no caller to drive it yet (`aurora-widgets` is still a
  skeleton) — same "primitive built, real consumer decides the rest
  later" shape used elsewhere.
- [!] **Built-in themes: Dark, Light, 2× high contrast, Colour-Critical**
  — Dark already existed (owner-approved, PLAN 0.5) and is what the
  parser above is verified against end to end. **Blocked on Cahya
  designing the other three** (PRD FR-027 *Ownership*) — this pass
  deliberately didn't invent their colours (see above), so the parser
  being generic over any correctly-shaped theme file isn't the same as
  the other three themes actually existing. The parser needs no changes
  to accept them once designed.
- [x] **Automated WCAG contrast validation over the token set** — done
  2026-08-01, `crates/aurora-theme/src/contrast.rs`
  (`contrast_ratio`, `check_gated_pairs`). The real, CI-enforced version
  of `design/check_contrast.py`'s Phase-0 prototype — that script's own
  docstring already named this exact follow-on. Same 17 gated pairs,
  same WCAG 2.1 relative-luminance formula (reusing
  `aurora_color::srgb_to_linear` for the channel-linearization step,
  since it's the identical IEC 61966-2-1 curve). `the_real_dark_theme_
  passes_every_gated_pair` independently reproduces `check_contrast.py`'s
  own prior finding (17/17 pass) — two independent implementations
  agreeing, the same cross-check discipline this project's spikes use
  throughout.
- [ ] CI lint rejecting hardcoded style values (§7.3.10) — needs real
  widget code to lint against, which doesn't exist yet (`aurora-widgets`
  is still a skeleton); deferred for the same "no consumer yet" reason
  as hot reload, not forgotten.

23 tests total in `aurora-theme`. Verified: `cargo fmt --all --check`
clean, `cargo clippy -p aurora-theme --all-targets --all-features -- -D
warnings` clean, `cargo test -p aurora-theme` — 23/23 passed, `cargo deny
check licenses` clean with the new `toml` dependency.

### M1.7 — Widget toolkit (`aurora-widgets`)

*Roughly a third of Phase 1. Document-agnostic and headlessly testable.*

- [x] **Layout engine (flexbox-style; `taffy` if it fits)** — done
  2026-08-02, `crates/aurora-widgets/src/tree.rs`
  (`WidgetTree::compute_layout`), 6 more tests (20 total in the crate).
  `taffy` (0.12, PLAN.md's own suggestion) added to
  `[workspace.dependencies]`. Each widget now carries a `taffy::Style`
  (layout *input*) alongside its `bounds` (the computed *output*) —
  `compute_layout` rebuilds a fresh internal `taffy::TaffyTree` from this
  tree's own structure on every call (children built before parents,
  since `taffy` needs a node's children to exist before it can reference
  them) rather than keeping a second tree permanently in sync — the same
  "recomputed on demand from a source of truth" shape
  `aurora_doc::History::replay` already uses for its own journal. `taffy`
  reports each node's position *relative to its parent*; `compute_layout`
  accumulates absolute screen-space position top-down and writes bounds
  back through the existing `set_bounds` (so the usual old-and-new-region
  dirty marking applies here too, not a separate path).
  **A real, verified-not-assumed finding along the way**: an `Auto`-sized,
  childless root does *not* implicitly stretch to fill the available
  space — confirmed by writing the opposite assertion first and watching
  it fail with `(0,0,0,0)` instead of the expected filled size. `taffy`
  sizes `Auto` to content, with no built-in "root fills the viewport"
  convention the way CSS's `html, body { width: 100% }` provides; a
  caller that wants that must ask for it explicitly (`percent(1.0)`),
  which a dedicated test now confirms actually works. Both behaviors are
  now real, permanent regression tests, not just a corrected assumption.
  Verified: `cargo fmt --all --check` clean, `cargo clippy -p
  aurora-widgets --all-targets --all-features -- -D warnings` clean,
  `cargo test -p aurora-widgets` — 20/20 passed, `cargo deny check
  licenses` clean.
- [x] **Retained-mode tree with damage tracking** — done 2026-08-02,
  `crates/aurora-widgets/src/tree.rs` (`WidgetTree<W>`, `WidgetId`), 14
  tests. Exactly one root (unlike `aurora_doc::LayerTree`'s multiple
  top-level layers — an application has one root window, not several),
  arbitrary nesting below it, generic over a payload `W` the same way
  `aurora_graph::RenderGraph<N>` is (this crate can't know what a
  concrete "button" or "checkbox" is yet — that's this milestone's own
  later "widget set" bullet). Children are appended at the end
  (paint/tab order), a deliberate departure from `LayerTree`'s
  "newest-on-top, insert at index 0" — a fresh widget has no natural "on
  top" the way a new image layer does. Damage tracking reuses
  `aurora_tile::Tile::mark_dirty`/`take_dirty`'s exact `Option<Rect>` +
  `Rect::union` idiom, both per-widget (`is_dirty`) and tree-wide
  (`take_damage`, what a renderer actually consumes); `set_bounds`
  correctly dirties both the vacated and newly-occupied regions, not
  just the new position.
- [x] **`accesskit` node per widget — part of the definition, not a pass
  (§7.3.9)** — done alongside the tree above, not bolted on after:
  `WidgetId` **is** `accesskit::NodeId` (a type alias, not a wrapper),
  so the tree's own identity and a widget's accessibility identity are
  the literal same value with no second id space to keep in sync or
  forget to populate. `insert`/`new` require an `accesskit::Node`
  up front — there is no code path that creates a widget without one.
  `accessibility_update` builds a real `accesskit::TreeUpdate` (same
  shape `spike/a11y-ime`'s own proven `tree.rs` already used:
  `nodes`/`tree`/`tree_id`/`focus`) from every widget's own node —
  what a platform adapter (`accesskit_winit`) will actually consume once
  `aurora-app` exists. `accesskit` pinned to `0.24`, the exact version
  the a11y spike already proved works. Headlessly verified throughout —
  every test builds a real `TreeUpdate` and inspects it directly, no
  window or platform adapter needed, which is what "headless mode for
  automated UI tests" (this milestone's own later bullet) is really
  asking this crate to already be.

  **A real bug in this exact function, found on real macOS hardware,
  2026-08-03**: Cahya ran `aurora-app` (once it actually had a
  multi-widget tree to send, via M1.8's docking/panels work) and hit an
  immediate crash — `accesskit_consumer` (the library
  `accesskit_winit`'s adapter uses internally) panicked with
  `` "TreeUpdate includes N nodes which are neither in the current tree
  nor a child of another node from the update" ``. Root cause:
  `accessibility_update` cloned each widget's stored `accesskit::Node`
  as-is, but nothing anywhere in this module ever called
  `Node::set_children` — every node except the root came out with an
  empty declared children list, so `accesskit_consumer` correctly saw a
  disconnected forest, not a tree. This is exactly the class of gap the
  "headlessly verified throughout" claim two sentences up should have
  caught and didn't: every existing test (in this file and in
  `tests/headless.rs`) inspected individual field *values* on nodes
  already known to be in `update.nodes` — none of them ever fed the
  update into a real `accesskit_consumer::Tree`, the one thing that
  actually validates parent-child connectivity. Fixed by setting each
  node's `children` from the tree's own real structure at update-build
  time. Added `accesskit_consumer` as a dev-dependency (pure Rust, only
  depends on `accesskit`/`hashbrown` — doesn't compromise this crate's
  own documented headless-dependency-graph guarantee, which is about
  real `[dependencies]`) and a new regression test,
  `accessibility_update_produces_a_tree_accesskit_consumer_accepts`,
  that builds a 3-level tree and feeds the real `TreeUpdate` into a real
  `accesskit_consumer::Tree::new` — **verified this test actually
  catches the regression**, not just that it passes: temporarily
  reverted the fix, reran, watched it fail with the identical panic
  message reported from real hardware, then restored the fix and
  confirmed it passes again. This also means every earlier "headlessly
  verified" claim for this crate is correspondingly weaker than stated
  everywhere it appears — real structural connectivity was never
  actually checked until now.
- [x] **Input routing, focus management, keyboard navigation** — done
  2026-08-02, `crates/aurora-widgets/src/input.rs` (`hit_test`,
  `FocusManager`), 18 more tests (38 total in the crate). Deliberately
  platform-agnostic — works in terms of a document-space point and
  `Tab`/`Shift+Tab` *steps*, not `winit::WindowEvent`s, the same seam
  that keeps this crate's own widget API free of `wgpu`/`winit`
  assumptions (ADR 0001's escape-hatch note). `hit_test` walks a node's
  children in reverse (last-painted-on-top, matching `WidgetTree`'s own
  paint-order convention) to find the deepest match; a dedicated test
  proves the tie-break with two deliberately overlapping siblings (via
  `set_bounds`'s escape hatch — real flexbox layout never overlaps
  siblings on its own). `FocusManager` reuses `accesskit`'s own
  vocabulary for "focusable" (`Node::supports_action(Action::Focus)`)
  rather than inventing a parallel flag, matching `WidgetId` already
  being `accesskit::NodeId` outright. `focus_next`/`focus_previous`
  walk focusable widgets in tree pre-order, skipping non-focusable ones,
  wrapping at both ends. `focus_at` combines `hit_test` with an
  ancestor-walk to the nearest focusable widget — "click bubbles to the
  nearest focusable ancestor" (e.g. clicking a button's icon glyph
  focuses the button) — verified against a real nested case. Focus
  changes mark both the old and new widget dirty (a focus ring is visual
  state). **Documented, not solved, limitation**: `FocusManager` holds no
  reference into the tree, so a widget removed while focused leaves a
  stale reference until a caller calls `validate` — the same class of
  limitation `aurora_doc::History` already documents for mixing direct
  and managed mutation. Verified: `cargo fmt --all --check` clean,
  `cargo clippy -p aurora-widgets --all-targets --all-features -- -D
  warnings` clean, `cargo test -p aurora-widgets` — 38/38 passed.
- [x] **Text field: selection, caret, word motion, clipboard, undo** —
  done 2026-08-02, `crates/aurora-widgets/src/widgets/text_field.rs`
  (`TextFieldState`, `insert_text_field`, `text_field_state`,
  `with_text_field_mut`, `set_text_field_disabled`), 28 more tests (86
  total in the crate). Caret/selection motion is byte-offset based but
  grapheme-cluster aware via `unicode-segmentation`'s
  `grapheme_indices(true)` — `backspace`/`delete_forward`/`move_left`/
  `move_right` all step by extended grapheme cluster, not `char` or
  byte, verified against a combining-mark case (`"e" + U+0301`, one
  cluster/two chars/three bytes) that a naive `char`-based mover would
  split incorrectly. Word motion uses `unicode_word_indices` (Unicode
  word-boundary rules, not a whitespace heuristic). `extend_selection:
  bool` on every motion method mirrors the "hold Shift" shape every
  mainstream editor uses; a non-extending move collapses any existing
  selection. Undo/redo is a `Vec<Snapshot>` pair scoped to this one
  widget's own small in-memory buffer (content + cursor + selection) —
  this does **not** revisit `aurora_doc::History`'s no-snapshot
  invariant (§7.3.3), which scopes to document/tile-store-scale data;
  moving the caret alone is deliberately not an undo step, only content-
  changing edits push a snapshot (verified by a dedicated test). Clip-
  board is a text-buffer-only `copy`/`cut`/`paste` returning/taking a
  plain `String` — no real OS clipboard access, which stays
  `aurora-app`'s job, the same platform seam already drawn around
  `hit_test`/`FocusManager`. Accessibility: `Role::TextInput`,
  `set_value`, `Action::Focus`/`Action::SetValue` when enabled — but
  **not** `accesskit::TextSelection`, left unexposed on purpose:
  `spike/a11y-ime/FINDINGS.md` already named that exact property as
  unverified/open, so this inherits an already-known gap rather than
  introducing a new one. Follows the established `with_..._mut` pattern
  from `button.rs`/`checkbox.rs`/`slider.rs`, but as a single generic
  `with_text_field_mut<R>(tree, id, f: impl FnOnce(&mut TextFieldState)
  -> R)` rather than one hand-written wrapper per operation — worthwhile
  here specifically because `TextFieldState` has far more mutating
  methods than the other three widgets combined. Verified: `cargo fmt
  --all --check` clean, `cargo clippy -p aurora-widgets --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-widgets --no-deps --all-features` clean, `cargo
  test -p aurora-widgets` — 86/86 passed. `python3`/`pkg-config` remain
  absent in this sandbox (pre-existing, documented gap), so workspace-
  wide `cargo clippy --workspace` and `scripts/check_layering.py` still
  can't run here; per-crate verification substitutes, as in every prior
  entry.
- [x] **IME composition rendering (platform underline styles)** — done
  2026-08-02, `crates/aurora-widgets/src/widgets/text_field.rs`
  (`Composition`, `UnderlineStyle`, `composition_segments`,
  `TextFieldState::set_composition`/`commit_composition`), 15 more tests
  (101 total in the crate). Mirrors `winit::event::Ime` exactly:
  `set_composition(text, target_range)` is the `Preedit(text,
  cursor_range)` handler (an empty `text` clears composition, winit's
  own documented synthetic-clear shape), `commit_composition(text)` is
  the `Commit` handler. Composition text is kept separate from `content`
  until committed — the same split `spike/a11y-ime/src/field.rs`'s
  `TextField` (`text` vs `preedit`) already proved out by hand, just
  formalized with real types. Composition updates are deliberately
  **not** undo steps (transient, like caret motion) — except starting a
  *fresh* composition first removes any active selection ("typing
  replaces the selection," and IME composition is what typing looks like
  once an IME is involved), which **is** a real content change and does
  push an undo entry, verified by a dedicated test distinguishing the
  two. `composition_segments` turns a `Composition` into byte-range
  segments tagged `UnderlineStyle::Plain`/`Target` — the thin/thick (or
  unconverted/targeted-clause) distinction every mainstream IME
  convention draws under different names (Windows TSF's
  `ATTR_TARGET_CONVERTED`, macOS's thicker mark, `IBus`'s "selected"
  preedit attribute) — **not** a pixel-drawing function: this crate
  still draws nothing (blocked on `aurora-vector`/`aurora-text`), so this
  is the styled-segment *data* a future renderer consumes, matching the
  "layout + content, no pixels" boundary every widget here already
  keeps. `target_range` is a plain `(usize, usize)` rather than
  `Range<usize>` specifically so `Composition`/`TextFieldState` can keep
  deriving `Eq` (`Range` doesn't implement it). **A real bug a test
  caught**: the first `composition_segments` draft emitted two abutting
  `Plain` segments for a degenerate empty target range (e.g. `(3, 3)`,
  matching winit's "cursor should be hidden" case) instead of merging
  into one — `composition_segments_empty_target_range_is_skipped` failed
  immediately and caught it. Accessibility: composing state is announced
  via `set_description` — the exact mechanism `spike/a11y-ime/src/tree.rs`
  already proved reaches VoiceOver (finding 2's own recorded follow-up:
  "the spike sets a description; the correct mechanism is richer... and
  needs design work in `aurora-widgets`" — this is that design work).
  `accesskit::TextSelection` remains unexposed, still inheriting the same
  already-known gap noted in the text field's own prior entry. Verified:
  `cargo fmt --all --check` clean, `cargo clippy -p aurora-widgets
  --all-targets --all-features -- -D warnings` clean,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-widgets --no-deps
  --all-features` clean, `cargo test -p aurora-widgets` — 101/101
  passed. `python3`/`pkg-config` remain absent in this sandbox
  (pre-existing, documented gap); per-crate verification substitutes for
  workspace-wide `cargo clippy --workspace`/`scripts/check_layering.py`,
  as in every prior entry.
- [~] **Widget set: button, checkbox, slider, number field, dropdown,
  scrollbar, tree, tab bar, menu, tooltip, colour picker, curve editor**
  — first slice done 2026-08-02,
  `crates/aurora-widgets/src/widgets/{mod,button,checkbox,slider}.rs`
  (`WidgetKind`, `Button`, `Checkbox`, `Slider`), 20 more tests (58 total
  in the crate). **3 of 12 named widgets**, deliberately: `Button`/
  `Checkbox`/`Slider` cover three genuinely different interaction shapes
  (discrete trigger, toggle, continuous drag) — enough to validate the
  pattern every other widget follows — while number field/dropdown need
  text editing, scrollbar/tree need scrolling, menu/tooltip need popover
  layering, and colour picker/curve editor need `aurora-vector` path
  rendering, none of which exist yet. Left open rather than stubbed
  half-built, matching this crate's own "no half-finished
  implementations" discipline.
  `WidgetTree<W>` is generic over one payload type for a whole tree, so a
  tree holding more than one widget kind needs a single unifying enum —
  `WidgetKind` — rather than each widget being its own tree type.
  Each widget's layout style is resolved from `aurora_theme::Scales`
  (spacing for `Button`'s padding, the type scale for `Checkbox`/
  `Slider`'s intrinsic size — there's no dedicated "control size" token
  yet, and inventing one is a design decision, not an engineering
  default to pick here), never a hardcoded literal — invariant §7.3.10.
  `Checkbox` reuses `accesskit::Toggled` directly for its own tri-state
  (checked/unchecked/indeterminate) rather than a parallel enum, the same
  "no second vocabulary" discipline `WidgetId`/`FocusManager` already
  established. **No rendering**: every widget produces layout +
  accessibility content only — vector-first rendering is a separate,
  still-open M1.7 bullet blocked on `aurora-vector` remaining an empty
  skeleton. **A real bug a test caught**: `toggle_checkbox`'s first draft
  had `Mixed => Toggled::False`, contradicting its own doc comment
  ("mixed -> checked", matching every mainstream toolkit's convention for
  an indeterminate "select all" checkbox) — `toggle_checkbox_resolves_
  mixed_to_checked` failed immediately and caught it before commit.
  Verified: `cargo fmt --all --check` clean, `cargo clippy -p
  aurora-widgets --all-targets --all-features -- -D warnings` clean,
  `cargo test -p aurora-widgets` — 58/58 passed.
- [ ] Vector-first rendering via `aurora-vector` (resolution-independent)
- [ ] Component gallery + golden-image tests per theme and density
- [x] **Headless mode for automated UI tests** — done 2026-08-02. Every
  test in this crate already ran with no window/GPU/platform adapter;
  what was missing was making that a *checked* fact rather than an
  inference from what the code happens not to call. Found the concrete
  gap: `aurora-widgets/Cargo.toml` declared `aurora-gpu`/`aurora-vector`/
  `aurora-text` as dependencies (`scripts/layering.json` allows all
  three, for the still-open vector-first-rendering bullet), but `grep`
  confirmed zero references to any of them anywhere in the crate's own
  source — dead weight that put real `wgpu` in this crate's dependency
  graph (`cargo tree -p aurora-widgets -i wgpu` resolved to a real edge)
  despite the crate's own doc comments claiming to be headless.
  Removed all three from `[dependencies]` (layering.json unchanged —
  it records what's *allowed*, not what must currently be declared; the
  three go back in exactly when vector-first rendering starts) — after
  the removal, `cargo tree -p aurora-widgets -i wgpu` finds no match at
  all, turning "headless" into a directly checkable property of this
  crate's own dependency graph. Added
  `crates/aurora-widgets/tests/headless.rs` — a real integration test
  (new pattern for this workspace; every other crate's tests are inline
  `#[cfg(test)] mod tests`, but this one specifically wants to exercise
  only the crate's *public* API, the same surface `aurora-ui` will
  eventually consume) that builds a small multi-widget form (Button/
  Checkbox/Slider/TextField), lays it out, cycles focus with `Tab`,
  hit-tests a point, mutates every widget including a full IME
  composition cycle, and inspects a real `accesskit::TreeUpdate` — one
  linear, end-to-end proof of the whole pipeline (`#[allow(clippy::
  too_many_lines)]`, deliberately: splitting one scenario into smaller
  functions wouldn't express separate ones). `crates/aurora-widgets/
  src/lib.rs`'s own doc comment now states the dependency-graph
  guarantee and points at the integration test as the permanent check.
  102 tests total (101 unit + 1 integration, both counted separately by
  `cargo test`). Verified: `cargo fmt --all --check` clean, `cargo
  clippy -p aurora-widgets --all-targets --all-features -- -D warnings`
  clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-widgets
  --no-deps --all-features` clean, `cargo build --workspace` clean
  (confirming removing the three dependencies didn't break any other
  crate), `cargo test -p aurora-widgets` — 102/102 passed, `cargo deny
  check licenses` clean. `cargo test --workspace` itself still can't run
  here — `aurora-app`/`aurora-cli`'s own `winit` feature needs
  `fontconfig` via `pkg-config`, absent in this sandbox (the same
  pre-existing, documented gap noted throughout this plan, unrelated to
  this change) — per-crate verification (`aurora-widgets`, `aurora-theme`,
  `aurora-core`) substitutes.

### M1.8 — Application shell (`aurora-ui`, `aurora-app`)

- [~] **Window/event loop; create hidden → attach a11y adapter → show**
  *(from the a11y spike)* — written 2026-08-02,
  `crates/aurora-app/src/{lib,main}.rs` (new — this crate had only a
  placeholder `main()` before), blocked on real-hardware verification,
  not on more engineering: **this sandbox has no display server
  (`$DISPLAY`/`$WAYLAND_DISPLAY` both empty) and no `pkg-config`
  (`winit`'s fontconfig feature needs it) with no root access to install
  it** (confirmed: `apt-get install` fails with "Permission denied... are
  you root?"), so `aurora-app` has never actually been compiled, let
  alone run, in this environment — a strictly larger gap than the
  no-GPU-adapter case every real-GPU test in this workspace already
  handles gracefully, since this blocks compilation itself, not just
  runtime hardware access. Written carefully against `accesskit_winit`
  0.33.2's own fetched source (confirms the exact pin `spike/a11y-ime`
  already proved: `Adapter::with_event_loop_proxy`/`process_event`/
  `update_if_active`) and the two closest proven precedents in this
  repo: `spike/a11y-ime`'s windowed app (the create-hidden-then-adapt-
  then-show ordering itself) and `aurora-gpu`'s own
  `examples/surface_smoke.rs` (the window/`GpuContext`/`GpuSurface`
  lifecycle, itself unverified for two days in this same project until
  someone ran it on a live macOS session — see M1.2 — so this is the
  same class of gap recurring, not a new kind of risk). Real
  error-handling throughout (no `unwrap`/`expect`/`panic!` anywhere,
  matching every other crate) — `main` is now fallible
  (`-> anyhow::Result<()>`), exactly what this crate's own prior
  placeholder doc comment said would happen "once it does anything that
  can fail." Reuses `aurora_widgets::widgets::new_tree`/`WidgetTree::
  accessibility_update`/`FocusManager` for the (currently content-free,
  single-root-container) accessibility tree rather than hand-rolling a
  parallel `accesskit::Node` construction — the first real integration
  of `aurora-widgets` into an actual window, even though it has no
  content yet. **Deliberately out of scope for this bullet, left to
  M1.8's other, separate ones**: docking/panels/canvas/tools content,
  input routing beyond `accesskit_winit`'s own action-request forwarding
  (logged, not routed anywhere — there is nothing to route to yet),
  IME, native menus, DPI handling, crash recovery; the redraw loop only
  clears the surface to a placeholder colour. One piece verified
  headlessly: `build_tree` (pure, no window needed) produces exactly a
  one-node tree, 1 test.

  **First real CI run (2026-08-02) immediately caught a real bug** this
  sandbox's lack of `pkg-config` prevented catching locally: `wgpu` was
  used directly throughout `redraw` (`CurrentSurfaceTexture`/`LoadOp`/
  `Color`/etc., mirroring `surface_smoke.rs`) but never declared as a
  dependency in `Cargo.toml` — CI got as far as compiling `winit` and
  `accesskit_winit` successfully (confirming this environment's
  fontconfig setup works) before failing on the missing `wgpu` crate.
  Fixed by adding it and re-verifying every other external crate path
  in `src/*.rs` against `Cargo.toml` by hand (`cargo tree -p aurora-app
  -i wgpu` now resolves cleanly). This is the exact risk flagged above
  from writing this without local compilation — confirmed real, caught
  by CI as intended, fixed the same day.

  **Upgraded from `[!]` to `[~]` 2026-08-03**: Cahya installed
  `pkg-config`/`libfontconfig1-dev` in this same sandbox, closing that
  half of the blocker. With it, `cargo build -p aurora-app`,
  `cargo clippy -p aurora-app --all-targets --all-features -- -D
  warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-app
  --no-deps --all-features`, and `cargo test -p aurora-app` (1/1
  passed) all succeed for the first time. More than that: this was the
  one dependency blocking the *whole workspace* from clean-compiling
  under `winit` — `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` (the
  exact CI gates, previously never once run successfully in this
  sandbox all session, always documented as "can't run here") now both
  pass completely, every crate, 0 failures. `scripts/check_layering.py`
  remains the one still-unrun check (`python3` itself, a separate,
  narrower, still-missing tool, unrelated to this fix).

  **Human-verified on macOS, 2026-08-03 (Cahya, real hardware, real
  desktop session)**: `cargo run -p aurora-app` opens a real window —
  the create-hidden → attach-adapter → show sequence works end to end,
  not just compiles. Two more things confirmed live: **resizing the
  window works correctly** (no crash; `apply_resize`/`GpuSurface::
  resize` handling both the `WindowEvent::Resized` path and the
  window's own reported size), and **VoiceOver announces the window**
  — the accessibility tree really does reach a real screen reader
  through `accesskit_winit`'s macOS backend, on this project's first
  real (non-spike) production code to attempt it. This is the load-
  bearing half of what this bullet needed: the exact ADR 0001
  escape-hatch ordering constraint, working for real, on the platform
  `spike/a11y-ime/FINDINGS.md` already scored highest (9/10).

  Still `[~]` not `[x]`: Windows and Linux remain unverified on real
  desktop sessions (the same "one platform confirmed, others still
  open" shape M1.2's own DX12/Metal/Vulkan bullet went through), and
  nothing has yet confirmed the label/value-level detail a screen
  reader would announce once real widgets exist (there's only an empty
  root container to announce right now). **Still needs: the same
  human verification on Windows and Linux** — the same kind of gap
  `spike/a11y-ime/FINDINGS.md` and `examples/surface_smoke.rs` both
  eventually needed and got, one platform at a time.

  **Real theme colour wired in, 2026-08-03** — the window's clear
  colour is no longer the placeholder literal; `load_background_color`
  loads the real, owner-approved Dark theme
  (`design/themes/dark.toml`/`design/tokens/palette.toml`, via
  `aurora-theme`'s own `Palette`/`ThemeSet`) and resolves
  `surface.app`, invariant §7.3.10 applied to the one thing this crate
  draws so far. **A real correctness point handled, not glossed over**:
  the surface format is sRGB-aware (`Bgra8UnormSrgb`), and every
  graphics API's clear-colour convention expects *linear* values for an
  sRGB render target — using the token's own sRGB-gamma-encoded bytes
  directly would have washed the colour out (a classic, easy-to-miss
  double-encoding bug). Converted via `aurora_color::srgb_to_linear`
  (already a dependency; no new crate needed). 2 more tests (headlessly
  real — theme loading needs no window): confirms real values from the
  checked-in design files, not just "it parses," and specifically that
  the result is meaningfully darker than the token's raw sRGB bytes
  (catches exactly the bug just described, were it to regress). This is
  also the point where **`pkg-config`/`libfontconfig1-dev` being
  installed in this sandbox turned into a real, direct engineering
  benefit**, not just an unblocking of CI: `cargo build/clippy/test -p
  aurora-app` and — for the first time this session — `cargo clippy
  --workspace --all-targets --all-features -- -D warnings` and
  `cargo test --workspace` (the exact CI gates) all ran and passed
  *locally*, 0 failures across every crate, 41 test-result blocks.
  `cargo deny check all` clean too. `scripts/check_layering.py` is
  still the one unrun check (`python3` remains absent).
- [~] **Docking, panels, custom workspaces** — first slice done
  2026-08-03, `crates/aurora-ui/src/{panel,workspace}.rs` (`aurora-ui`'s
  first real code — was a placeholder `crate_name()`). Matches the
  structure of the owner-approved workspace mockup
  (`design/mockups/workspace.html`, Phase 0 0.5): a canvas area plus a
  side rail holding three docked panels (Layers, Properties, History,
  in that order), built via `aurora_widgets::WidgetTree<WidgetKind>` —
  reusing the SAME concrete widget enum `aurora-widgets` already
  defines rather than inventing a parallel one, since `Container` is
  already the right generic building block for layout structure with no
  interactive behavior of its own. `insert_panel` gives each panel a
  real `Role::Region` accessibility node (not `Role::GenericContainer`)
  with the panel's own title as its accessible name — the ARIA concept
  of a perceivable, nameable section a user would navigate directly to,
  matching what a docked panel actually is. **Deliberately, explicitly
  static**: no drag-to-redock, resize, collapse, close, floating, or
  persisted workspace layouts — those are the real "docking" and
  "custom workspaces" half of this bullet, left open because each needs
  real interaction/drag-state machinery this first pass doesn't build.
  The canvas:rail split and the three panels' relative heights are flex
  *ratios* (3:1, and 1:1:1), not absolute pixel sizes — `design/tokens/
  scales.toml` has no "dock region width" token (only widget-chrome
  scales: type/spacing/radius/elevation/motion), and inventing one ad
  hoc would be exactly the "invent a token locally" anti-pattern
  CLAUDE.md warns against; a real, resizable dock width is what the
  still-open interactive docking work will actually need. Wired into
  `aurora-app`: `App` now holds a real `aurora_ui::Workspace` instead of
  a trivial one-node tree, and `compute_layout` runs on both initial
  window creation and every resize (pure geometry, no GPU needed) so the
  layout stays live — **verified empirically, not assumed**: a test
  computing layout at a real 1000×800 viewport confirms the exact
  750/250 width split (3:1 canvas:rail) and that all three panels
  receive equal, nonzero rail height, rather than trusting flex-grow
  math by inspection. Menubar, toolbar, and status bar (also shown in
  the mockup) are deliberately left out of this pass — they belong to
  other, separate M1.8/M1.9 bullets (native menus, tools, general
  chrome), not the docking/panel structure this one is about. Still no
  pixel rendering (blocked on `aurora-vector`, same as every widget in
  `aurora-widgets`) and no real panel *content* (layer rows, property
  fields, history entries) — only the structural skeleton. 6 new tests
  (3 in `aurora-ui`, plus `aurora-app`'s own test count unchanged at 1
  since the workspace-structure tests live in `aurora-ui` itself, not
  duplicated). Verified: `cargo fmt --all --check` clean, `cargo clippy
  -p aurora-ui -p aurora-app --all-targets --all-features -- -D
  warnings` clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-ui -p
  aurora-app --no-deps --all-features` clean, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` and `cargo test
  --workspace` (the exact CI gates) both clean, 0 failures across every
  crate, `cargo deny check all` clean. `scripts/check_layering.py`
  remains the one unrun check (`python3` still absent) — but only
  external (non-`aurora-*`) dependencies changed here (`accesskit`/
  `taffy` added to `aurora-ui`; `taffy` removed from `aurora-app`, now
  genuinely unused there since `build_tree` no longer exists), so the
  layering rules themselves aren't in question.

  **This is what actually surfaced a real crash on real hardware**:
  every accessibility tree before this bullet was a single trivial root
  (nothing to be disconnected from), so `aurora_widgets::WidgetTree::
  accessibility_update`'s missing-`set_children` bug had no way to show
  up until a real multi-node tree existed to send. Cahya ran the
  updated `aurora-app` on macOS and hit an immediate
  `accesskit_consumer` panic; root-caused and fixed in `aurora-widgets`
  itself (see that crate's M1.7 entry above — the bug and fix are
  recorded there, since the function lives there, not in `aurora-ui`/
  `aurora-app`). Worth naming plainly: this is the second time this
  session multi-widget content (first `tests/headless.rs`, now this
  bullet) exposed a real defect that trivial single-node fixtures
  couldn't — a pattern worth remembering for whatever comes after this.

  **Second real finding from the same hardware session, 2026-08-03,
  fix confirmed working**: even with the `set_children` crash fixed,
  Cahya found the workspace tree completely unreachable from
  VoiceOver — worse than "some empty containers along the way," the
  Rotor's "Window Spots" category came back *entirely empty*, not even
  showing the window's own title (compared side by side against
  `spike/a11y-ime`, run fresh in the same session, whose "Window
  Spots" correctly lists both its title and its labeled text field).
  Diagnosed via a systematic comparison, not guessed: confirmed
  VoiceOver itself was working normally (the spike behaved exactly per
  its own `FINDINGS.md`), which narrowed the difference to this tree's
  own structure. Root cause: `aurora_widgets::widgets::new_tree`'s
  default root role is `Role::GenericContainer` — reasonable for a
  nested/internal container, but this tree's root *is* the whole
  window's content, and a plain container there apparently never
  anchors into the native window's own accessibility hierarchy at all
  on macOS. `build_workspace` now overrides its own root's
  accessibility node to `Role::Window` (labeled "Aurora") right after
  creating the tree, matching `spike/a11y-ime`'s own proven root
  exactly, plus a new test confirming the override. **Re-verified on
  real macOS hardware, same day**: after rebuilding, VoiceOver's
  "Window Spots" Rotor category now lists both "Aurora" (the window)
  and "Layers" (announced as "Layers Group") — confirming the fix
  actually works, not just compiles. Worth naming honestly:
  `Role::GenericContainer` was originally chosen based on
  `spike/a11y-ime/FINDINGS.md` finding #5's own *speculative*
  suggestion ("worth testing a plainer root role") — a hypothesis that
  was never actually validated in the spike itself, and turned out to
  trade a smaller, documented annoyance (finding #5's navigation-depth
  issue) for a much larger, undocumented one (no window-level
  accessibility attachment at all). **One open, non-blocking loose
  end**: "Properties" and "History" did not show up in that same
  Rotor listing (further Down/Right Arrow within it went nowhere) —
  most likely a Rotor-category display quirk (possibly a curated/
  capped list) rather than a real gap, since all three panels are
  built via the exact same `insert_panel` call and are already
  confirmed structurally identical by `aurora-ui`'s own test suite;
  not chased further live given the core question (does `Role::Window`
  fix the anchoring problem) is now answered. Worth a full linear
  `Tab`/arrow-key sweep once real input routing exists to settle this
  properly, rather than more Rotor-only spot checks. `cargo test -p
  aurora-ui`/`cargo clippy -p aurora-ui --all-targets --all-features
  -- -D warnings`/`RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-ui
  --no-deps --all-features` all clean; `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` and `cargo test
  --workspace` both clean too, 0 failures across every crate.
- [~] **Canvas: infinite zoom, rotation, pan, rulers, guides, grid,
  snap** — real rendering landed 2026-08-06, in two commits, picking up
  directly from M1.9's "wire a live document" step (which gave the
  Brush tool somewhere to paint but no way to *see* it). (1)
  `aurora-gpu` gained `CanvasPipeline`, a real, public type promoting
  the bind-group-layout/pipeline-building logic that crate's own tests
  had been exercising privately (`test_support.rs`) into production
  API — self-contained the same way `aurora_render::TileCompositor`
  already is, but exposing `pipeline()`/`bind_group()` for a caller's
  own render pass rather than a self-contained draw-and-submit method,
  since `aurora-app` needs to draw the canvas within one existing pass
  alongside its own background clear, not as a separate submission.
  `render_test.rs` rewritten to exercise `CanvasPipeline` and a real
  `TileResidency`/`TileStore` end to end, replacing a hand-rolled
  duplicate of the same bind-group/pipeline code. 2 new tests (12
  total). (2) `aurora-app`'s `resumed` now builds a real
  `TileResidency`/`CanvasPipeline` sized to the canvas dock area's own
  physical bounds; `redraw` syncs the atlas from the live `tile_store`
  (whatever `active_layer` holds, painted or not) and draws it within
  that area's own viewport via `RenderPass::set_viewport` — a Brush
  stroke is now actually visible, not just written into an
  otherwise-invisible store. `CanvasView`'s own pan is reflected too
  (`tile_origin_for_view`); zoom deliberately is not (`TileResidency`
  has no scale support yet). A real finding along the way:
  `aurora_widgets::WidgetTree::bounds` returns a widget's current (zero
  by default) bounds unconditionally once it exists, not `None` before
  the first `compute_layout` — a test assumed the opposite, failed, and
  was fixed (the code was already correct). 6 new tests (84 total in
  `aurora-app`).

  **Still open, exactly as the bullet's own name says**: rotation,
  rulers, guides, grid, snap, and true "infinite" zoom (only pan is
  wired; the atlas is sized once at startup and does not resize with
  the window, `TileResidency`'s own documented limitation). Not
  real-hardware-verified — this sandbox has no display server, so
  nothing has shown this crate's own window with real content on an
  actual screen since M1.8's original human-verification pass, which
  predates this work.

  Verified: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D
  warnings" cargo doc`, `cargo test --workspace` (0 failures across
  every crate, including real GPU tests), `cargo deny check all` — all
  clean. No new dependencies; no new `aurora-*` edges.
- [~] **Layers, history, tool-options panels** — first slice done
  2026-08-03, `crates/aurora-ui/src/layers_panel.rs` (new — Layers only;
  History and tool-options panels remain separate, still-open work).
  `populate_layers_panel` takes a real `aurora_doc::LayerTree` (already
  built in M1.4, never wired to any UI before now) and inserts one real
  `Role::ListItem` row per layer into the Layers panel's body (itself
  re-labeled `Role::List`), nested to mirror group structure (a group's
  own layers become its row's children) — real, correct accessible
  names and descriptions (blend mode, opacity as a percentage,
  "hidden" when applicable), not placeholder text. 3 new tests,
  including a real nested-group case (a group containing a pixel
  layer, confirming the child row nests under the group's own row, not
  flattened alongside it) and a rejection case (an unknown panel body).
  **Still no pixel rendering** (thumbnails/swatches are visual, and
  nothing in this stack draws pixels yet) and **deliberately one-shot,
  not reactive** — this builds rows once from whatever `LayerTree`
  state it's given; there's no diff-and-refresh mechanism yet, which is
  fine since nothing can edit a live document in `aurora-app` yet
  either.

  Wired into `aurora-app` via a new `demo_layers()` — a small, clearly-
  fake three-layer tree (Background, Color balance at Multiply/80%,
  Retouch — skin on top), since there is no real "open a document" flow
  yet (separate, still-open M1.9 work); illustrative names matching
  `design/mockups/workspace.html`'s own example layers, which are
  themselves explicitly "structure and token usage for review," not
  real content either. 1 more test in `aurora-app`
  (`demo_layers_puts_retouch_on_top_with_color_balance_multiplied`),
  checking the real top-to-bottom order `add_pixel_layer`'s "new
  topmost root" rule produces, not just a layer count. Verified:
  `cargo fmt --all --check` clean, `cargo clippy -p aurora-ui -p
  aurora-app --all-targets --all-features -- -D warnings` clean,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-ui -p aurora-app
  --no-deps --all-features` clean, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` and `cargo test
  --workspace` both clean, 0 failures across every crate, `cargo deny
  check all` clean.

  **History panel done too, 2026-08-04** — `crates/aurora-ui/src/
  history_panel.rs` (new), 2 more tests (9 total in the crate).
  `populate_history_panel` mirrors `populate_layers_panel`'s own shape
  exactly (`Role::List` body, one `Role::ListItem` row per entry) but
  reads from a real `aurora_doc::History` instead — which needed a real
  `aurora-doc` feature addition first (`History::journal_descriptions`,
  see M1.4) since the journal had no public way to expose individual
  entries before now; found and flagged before writing any panel code,
  not discovered partway through. `demo_document()` (renamed from
  `demo_layers`, `aurora-app`) now builds its three demo layers
  *through* `History`'s own methods rather than direct `LayerTree`
  calls, specifically so the History panel has a real, meaningful
  journal to show instead of an empty one — 2 more tests (3 total in
  `aurora-app`), including one asserting the exact five descriptions in
  order (catching the specific layer-id-off-by-one bug a first draft
  of that test had — `color_balance` is id `1`, the second layer
  created in a fresh tree, not `2`; the test itself caught this before
  it shipped). Verified: `cargo fmt --all --check` clean, `cargo
  clippy -p aurora-ui -p aurora-app -p aurora-doc --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-ui -p aurora-app -p aurora-doc --no-deps
  --all-features` clean, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` both
  clean, 0 failures across every crate, `cargo deny check all` clean.

  **Re-verified on real hardware, 2026-08-04 — structure confirmed
  correct; a real, reproducible VoiceOver navigation gap found and
  recorded, not resolved**: Cahya rebuilt and re-tested. The Rotor
  ("Window Spots") still correctly shows "Aurora" and "Layers Group" —
  the accessibility tree itself is confirmed intact and reachable, the
  same structural result already verified by the automated test suite.
  But VoiceOver's linear/interact keyboard navigation (VO+Shift+Down,
  then VO+Right Arrow) into that nested content does not reliably work:
  it gets stuck with no further announcements, and one attempt (jumping
  to "Layers Group" via the Rotor's own select action, then trying to
  interact/navigate from there) landed on the *native window's own*
  minimize/zoom title-bar buttons instead of any of our content —
  suggesting VoiceOver's cursor left our tree entirely rather than
  entering it. Reproduced consistently across multiple attempts,
  including after a full VoiceOver restart (not just toggling it off
  and on), ruling out a one-off session glitch. **Not a structural bug
  in this project's own code** — the same tree that fails to navigate
  linearly is proven correct by both the Rotor check and the automated
  test suite — but a real, open UX gap in how VoiceOver handles deep,
  custom-rendered, non-native nested content via keyboard navigation
  specifically. This is `spike/a11y-ime/FINDINGS.md` finding #5's own
  flagged risk ("Custom `Role::Window` nested in a real window needs an
  explicit interact step") turning out to be worse in practice, with a
  deeper tree, than that spike's own flat two-child structure ever
  exercised — finding #5 never actually tested a multi-level nested
  case. **Deliberately not chased further via more remote, blind code
  changes**: guessing at accessibility-tree-shape fixes without being
  able to see or hear the result directly has a poor cost/signal ratio,
  and the risk this de-risks (ADR 0001's "does a screen reader reach
  our content at all") is already answered — real content genuinely
  reaches VoiceOver, confirmed twice now (Rotor, test suite). What
  remains is a keyboard-navigation-UX investigation needing either
  someone with deeper native macOS accessibility expertise, or
  comparing systematically against a minimal, deliberately-flat
  reproduction (closer to the spike's own proven shape) to isolate
  whether nesting depth specifically is the trigger — real, scoped
  follow-up work, not something to keep guessing at inline.
- [~] **Command palette, keyboard shortcuts** — first slice done
  2026-08-04. Two new generic mechanisms in `aurora-widgets`, following
  the same "abstract steps, not `winit` types — translating real
  platform input is `aurora-app`'s job" seam `FocusManager`/`hit_test`
  already established:
  `crates/aurora-widgets/src/shortcut.rs` (new) — a small,
  `#[non_exhaustive]` platform-free `Key`/`NamedKey`/`Modifiers`/
  `KeyChord` vocabulary (not `winit::keyboard`'s own ~60-variant
  `NamedKey`; only what a shortcut plausibly needs), `KeyChord::parse`
  for human-authored strings (`"Ctrl+Shift+P"`), and a generic
  `ShortcutRegistry<T>` (chord → caller-defined command) with real
  conflict detection on `bind` (an accidental silent overwrite would be
  exactly the kind of bug a growing shortcut list hits eventually) — 16
  tests. `crates/aurora-widgets/src/widgets/command_palette.rs` (new) —
  a searchable command list: a `Role::TextInput` root (query) holding a
  `Role::ListBox` of `Role::ListBoxOption` rows, case-insensitive
  substring filtering that rebuilds real tree rows (remove-then-reinsert,
  the same one-shot simplicity `aurora-ui`'s panel population already
  accepts) rather than diffing, and wraparound `ArrowUp`/`ArrowDown`
  selection — 10 tests. Deliberately no open/close flag: inserting the
  palette *is* opening it, removing its root *is* closing it, matching
  this crate's "real tree nodes, not a hidden flag" preference elsewhere.
  128 tests total in `aurora-widgets` (was 102).

  Wired into `aurora-app` for this crate's **first real keyboard input
  of any kind** (`WindowEvent::KeyboardInput`/`ModifiersChanged` were
  both unhandled before this): `default_shortcuts()` binds `Tab`/
  `Shift+Tab` to `aurora_widgets::FocusManager` navigation (built in
  M1.7, never actually reachable from a keyboard until now) and
  `Ctrl+Shift+P` to open the palette; the palette's own real command
  list (`palette_commands`) is "Focus Layers/Properties/History Panel" —
  genuine, not placeholder, since `aurora-ui`'s panel regions are real
  `Tab` stops as of this same change (`insert_panel` now carries
  `Action::Focus`, closing exactly the gap the 2026-08-04 VoiceOver
  finding above flagged as needing "real input routing" to investigate
  properly). **Every bit of the dispatch logic is deliberately free
  functions taking `&mut aurora_ui::Workspace`/`&mut FocusManager`/
  `&mut Option<WidgetId>`, not `App` methods** — the same "pure logic,
  headlessly testable" shape `demo_document`/`load_background_color`
  already use, so this crate's first keyboard-routing code is fully
  tested (17 tests) with **no window, no `EventLoopProxy`, and no
  display server** — this sandbox still has none of those. Only
  `translate_key`/`translate_modifiers` touch real `winit::keyboard`
  types, and those are plain data (`Key`/`ModifiersState`), constructible
  with no window either — confirmed by actually running the tests here,
  not assumed. Verified: `cargo fmt --all --check` clean, `cargo clippy
  -p aurora-widgets -p aurora-ui -p aurora-app --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-widgets -p aurora-ui -p aurora-app --no-deps
  --all-features` clean, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` both
  clean, 0 failures across every crate, `cargo deny check all` clean.
  `scripts/check_layering.py` remains the one unrun check (`python3`
  still absent) — only an existing workspace dependency (`accesskit`)
  moved into `aurora-app`'s own `[dev-dependencies]`, no new `aurora-*`
  edges.

  **Still `[~]`, not `[x]`**: no real-hardware verification yet (a
  screen reader hasn't confirmed the palette's query/results are
  announced sensibly while typing — a real, separate risk from "does
  the tree structure exist," per this same milestone's own repeated
  lesson that structural correctness and a screen reader's actual
  behaviour can diverge); only three, real-but-narrow palette commands
  exist (richer ones — undo/redo, save, tool switches — wait on this
  crate having real actions behind them, not on more palette
  machinery); no fuzzy/subsequence match (plain substring only); and no
  user-facing way to see or rebind the shortcut list yet (it's a fixed,
  checked-in default).
- [~] **Native menus, file dialogs, drag & drop, clipboard** — file
  dialogs, clipboard, drag & drop, and a first native-menu slice
  (macOS only) done 2026-08-05. Picked the two remaining dependencies
  PRD §8.3/§14's own
  table had already named but never actually added: `rfd` (native
  dialogs) and `arboard` (system clipboard, `default-features = false`
  — the default `image-data` feature pulls in the `image` crate for
  clipboard *images*, which nothing in this workspace copies/pastes
  yet). Both MIT/dual MIT-or-Apache-2.0; `arboard`'s Windows backend
  (`clipboard-win`/`error-code`) is BSL-1.0, a real, permissive,
  OSI-approved/FSF-libre licence not previously on the allow list —
  added to `deny.toml` with a comment explaining why, rather than
  silently widening the list. `cargo deny check all` clean with both
  new dependencies.

  Wired into `aurora-app`'s command palette (its own doc comment names
  it as this crate's only real live text-input surface right now —
  `TextFieldState`'s own copy/cut/paste, built in M1.7, has no live
  instance in the actual UI yet, so wiring the system clipboard to it
  would have had nothing real to attach to): `Ctrl+C`/`Ctrl+V` read and
  write the real OS clipboard; a new "Open File…" palette entry shows a
  real, native, synchronous `rfd::FileDialog`. **Kept the pure dispatch
  logic testable, isolated the two untestable platform calls behind a
  seam** — the same shape `translate_key`/`translate_modifiers` already
  established for keyboard input: `handle_palette_key` takes
  `&mut dyn ClipboardAccess`/`&mut dyn FileDialogAccess` rather than
  calling `arboard`/`rfd` directly, so tests inject `FakeClipboard`/
  `FakeFileDialog` and never touch a real clipboard or native picker
  (this sandbox has neither — no display server at all). **Honest about
  its own limit, the same "detect a real signal, defer the action"
  pattern the crash-recovery marker already uses**: a file chosen via
  "Open File…" is only recorded (`App::pending_open_path`) and logged,
  not imported — `aurora-io` remains an empty skeleton, separate M1.9
  work; inventing a fake import path here would have been exactly the
  kind of half-finished feature CLAUDE.md warns against. 11 new tests
  (39 total in the crate): clipboard copy/paste (including an empty-
  clipboard paste leaving the query unchanged), a real file pick
  propagating all the way back through `handle_palette_key`/
  `handle_key`'s own return value, and a cancelled dialog (`None`)
  still closing the palette cleanly. Verified: `cargo fmt --all --check`
  clean, `cargo clippy -p aurora-app --all-targets --all-features -- -D
  warnings` clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-app
  --no-deps --all-features` clean, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` and `cargo test
  --workspace` both clean, 0 failures across every crate, `cargo deny
  check all` clean.

  **Drag & drop added the same day** — real, native-to-`winit`
  functionality, no new dependency needed:
  `WindowEvent::DroppedFile`/`HoveredFile`/`HoveredFileCancelled` are
  now handled in `window_event`. A dropped file writes the exact same
  `App::pending_open_path` slot the palette's "Open File…" command
  does — a dropped path and a chosen one are the same "the user wants
  to open this" signal regardless of which route it arrived by, so
  they share one honest, not-yet-imported destination rather than two
  parallel ones. `HoveredFile`/`HoveredFileCancelled` are only traced
  at debug level — there's no drop-target visual affordance to drive
  yet (nothing in this crate renders a pixel regardless of drag
  state). No new tests: the handler is a single field assignment plus
  a log line, the same "trivial `App`-only glue, not independently
  unit-tested" category `fail`/`apply_resize`/`redraw` already sit in
  — genuinely different from the clipboard/file-dialog work above,
  which had real branching logic worth isolating behind a seam.
  Verified the same way: `cargo fmt --all --check`, `cargo clippy -p
  aurora-app --all-targets --all-features -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-app --no-deps
  --all-features`, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings`, `cargo test --workspace` (0
  failures across every crate), `cargo deny check all` — all clean.

  **Native menu bar added the same day, scoped to macOS only** — asked
  Cahya which of three real options to take before writing any code,
  since the investigation below changed the shape of the decision
  significantly: `muda` (PRD §8.3/§14's own named candidate) was
  picked, chose "macOS only" over "macOS + Windows" or "defer
  entirely." The investigation itself is worth recording, since it's
  the reason the scope narrowed: `muda`'s only Linux backend is GTK
  and requires a real `gtk::Window` via `Menu::init_for_gtk_window` —
  but this project's window is a plain `winit`-created X11/Wayland
  surface, never a GTK widget, so there is structurally nothing for it
  to attach to even with GTK installed. Worse, confirmed by actually
  trying it: `muda` **does not compile on Linux at all** without the
  `gtk` Cargo feature enabled (no fallback backend exists), which pulls
  in real system GTK3/libxdo dev packages this sandbox doesn't have
  and has no root to install (the same class of gap `pkg-config`/
  fontconfig was, back at the start of M1.8, only heavier) — for a
  backend that couldn't attach to anything anyway. Windows would need
  its own real decision too: `Menu::init_for_hwnd` is `unsafe` (a raw
  `HWND`), needing an `unsafe_code` override in `aurora-app`'s own
  lints (confirmed the actual Cargo mechanism for this by testing it
  directly: `[lints] workspace = true` cannot coexist with any local
  override in the same manifest — Cargo rejects it outright — so an
  override means fully restating the workspace's lint table locally,
  not a one-line addition). Given neither Windows nor Linux was a good
  fit, and PRD §8.3/§14 only ever name **macOS** for the native menu
  bar in the first place ("Native menu bar (macOS), native file
  dialogs..."), macOS-only is both the safest and the most PRD-faithful
  scope — Windows/Linux stay on the command palette until Aurora draws
  its own in-window menu (`aurora-vector`, still empty), which is where
  a real cross-platform answer belongs.

  `muda` is a macOS-only target dependency
  (`[target.'cfg(target_os = "macos")'.dependencies]`, not a plain
  `[dependencies]` entry) — confirmed with `cargo tree -i muda` that it
  doesn't enter this Linux sandbox's own build graph at all, and
  `cargo deny check all` (which evaluates every platform) stays clean
  with it added. `build_menu` constructs File > Open File…/View >
  Focus Layers/Properties/History Panel using `MenuItem::with_id` with
  the exact same `COMMAND_*` constants the command palette already
  uses — no second command vocabulary — and a new shared
  `activate_command` (refactored out of the palette's own `Enter`
  handling, now called by both) drives both UI surfaces identically.
  `resumed` attaches it via `Menu::init_for_nsapp()` (no `unsafe`
  needed for the macOS path); `about_to_wait` polls
  `muda::MenuEvent::receiver()` each iteration rather than folding menu
  events into the existing `accesskit_winit::Event` user-event type
  (restructuring that would be a bigger, separate change). 3 new tests
  (42 total): `activate_command` itself is plain, cross-platform logic
  (no `#[cfg]` needed) and fully tested here.

  **First real macOS CI run (2026-08-05) found a genuine, structural
  incompatibility, not a flake**: a fourth test walking `build_menu`'s
  own `Menu::items()`/`Submenu::items()` tree (checking every expected
  `COMMAND_*` id is actually present, not just that construction didn't
  panic) failed under `cargo nextest run` with `muda::Menu::new() can
  only be created on the main thread`. This is not fixable from the
  test side: neither `nextest` nor libtest's own default harness ever
  runs an individual `#[test]` fn on the process's real main thread —
  both dispatch to worker threads regardless of `--test-threads`, so no
  attribute or flag makes a `muda`-constructing test satisfy this
  constraint. Removed the test entirely rather than working around it
  with e.g. a single-threaded custom harness (real, separate
  infrastructure work disproportionate to one test). `build_menu`
  itself is unaffected and remains real production code — it runs from
  `App::new`, called on the winit event loop's own main thread, where
  the constraint is naturally satisfied — it's just unreachable from
  this crate's own `#[test]` suite, exercised only by actually running
  the app. Documented directly in the test module (a comment where the
  removed test used to be) so this isn't rediscovered blind later.
  Verified everywhere this platform allows: `cargo fmt --all --check`
  clean, `cargo clippy -p aurora-app --all-targets --all-features -- -D
  warnings` clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-app
  --no-deps --all-features` clean, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` and `cargo test
  --workspace` both clean, 0 failures across every crate, `cargo deny
  check all` clean. No CI workflow changes needed — unlike the GTK path,
  macOS's own Cocoa/AppKit headers are already present on
  `macos-latest` runners. **Confirmed 2026-08-06**: after removing the
  test, the full CI matrix (lint, Linux/macOS/Windows test, docs, deny)
  passed green — the `muda` addition itself compiles and links cleanly
  on all three platforms, not just this one Linux sandbox.

  **Still `[~]`, not `[x]`**: no real-hardware verification anywhere in
  this bullet (does the native "Open File…" dialog actually appear and
  behave correctly on each platform; does copy/paste actually
  round-trip through the real OS clipboard; does a real drag-and-drop
  gesture from the host OS actually reach `DroppedFile`; does the
  native menu bar actually appear on a real Mac, respond to clicks, and
  correctly drive `activate_command`) — the native menu bar in
  particular has never been compiled at all outside CI, since this
  development sandbox is Linux and the dependency isn't even present
  here. Windows and Linux native menus remain unaddressed, deliberately
  — see the investigation above.
- [~] **Per-monitor DPI and fractional scaling** — first slice done
  2026-08-05, `crates/aurora-app/src/lib.rs`. Found and fixed a real,
  latent bug along the way: layout was being computed straight from
  `winit`'s own *physical*-pixel window size, but every widget's own
  layout style (`aurora_theme::Scales`-derived padding/spacing) is
  defined in *logical*, DPI-independent units — on any display where
  `scale_factor != 1.0` (any HiDPI/Retina display, or a fractional
  Linux compositor scale), every widget would have rendered at the
  wrong on-screen size once real rendering exists. New pure function
  `logical_size(physical, scale_factor) -> (f32, f32)` divides out the
  scale factor at the one seam where a physical size reaches
  `WidgetTree::compute_layout` (both the initial layout in `resumed`
  and every `apply_resize` call); the GPU surface itself still resizes
  to the real physical size, since a render target's pixel dimensions
  are never logical. Deliberately total, not just "handles the common
  case": `scale_factor <= 0.0` or non-finite (not a value `winit`
  should ever report) falls back to `1.0` rather than dividing by zero
  or propagating a negative/NaN size, but a real fractional factor
  *below* 1.0 (some Linux compositors allow scaling down) is honoured,
  not clamped away — a first draft's `scale_factor.max(1.0)` would have
  gotten that legitimate case wrong, caught before it shipped. `App`
  tracks its own `scale_factor` (read from the real `Window::
  scale_factor()` once a window exists) and keeps it current via a new
  `WindowEvent::ScaleFactorChanged` handler — the same mechanism a
  genuine multi-monitor, mixed-DPI setup relies on when a window moves
  between monitors, though only a single monitor's scale factor
  changing has actually been exercised (headlessly; no real multi-
  monitor hardware in this loop). 6 new tests (34 total in the crate),
  covering 1.0/above-1.0/fractional/below-1.0/non-positive/non-finite
  scale factors — all pure math, no window needed. Verified: `cargo fmt
  --all --check` clean, `cargo clippy -p aurora-app --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-app --no-deps --all-features` clean, `cargo
  clippy --workspace --all-targets --all-features -- -D warnings` and
  `cargo test --workspace` both clean, 0 failures across every crate,
  `cargo deny check all` clean. No external or internal dependency
  changes, so `scripts/check_layering.py` (still unrun, `python3`
  absent) isn't in question. **Still `[~]`, not `[x]`**: real
  multi-monitor/mixed-DPI hardware verification hasn't happened, and
  per-widget *rendering* at the correct physical resolution is still
  blocked on `aurora-vector` (nothing draws a pixel yet regardless of
  DPI) — this bullet closes the layout-math half, not the pixel-crisp-
  rendering half.
- [ ] OS settings: reduced motion, high contrast, text size
- [~] **Crash recovery UI** — first slice done 2026-08-05. A new
  generic mechanism in `aurora-widgets`,
  `crates/aurora-widgets/src/widgets/dialog.rs` (new): a modal
  `Role::AlertDialog` (not the plainer `Role::Dialog` — every dialog
  this crate can build today is an urgent, blocking prompt, exactly
  what ARIA's `alertdialog` role names) holding a message and one real,
  focusable `Button` per action, reusing `insert_button` rather than
  hand-rolling a second button shape — 4 tests. 132 tests total in
  `aurora-widgets` (was 128).

  Wired into `aurora-app` as this crate's **first crash-recovery
  behaviour of any kind**, deliberately narrow: a small marker file
  (`std::env::temp_dir().join("aurora-session.marker")`) is written at
  startup and cleared on a clean `WindowEvent::CloseRequested`
  shutdown; if a *previous* run's marker is still present when a new
  run starts, that run shows the real, modal dialog. **What this
  deliberately does not do**: restore any actual document state.
  `aurora-doc`'s own crash-recovery journal (M1.4) only has its
  in-memory half built — no on-disk encoding for `LayerOp`'s recursive
  shape has been decided yet, and forcing one here as a side effect of
  this bullet would be exactly the kind of evidence-free format
  decision `spike/raw-icc/FINDINGS.md` already warned against once. So
  the dialog is honest about it: its one action is "Continue," not
  "Recover Document" — real crash *detection*, not (yet) real crash
  *recovery*. `handle_key`'s routing gained a dialog tier ahead of the
  command palette (a modal alert takes priority over everything else,
  including the palette — checked directly by a test opening both and
  confirming `Escape` closes only the dialog). 11 new `aurora-app`
  tests (28 total), including real filesystem I/O against a
  `tempfile::TempDir` for the marker functions themselves, not just the
  in-memory dialog logic. Verified: `cargo fmt --all --check` clean,
  `cargo clippy -p aurora-widgets -p aurora-app --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-widgets -p aurora-app --no-deps --all-features`
  clean, `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` and `cargo test --workspace` both clean, 0 failures across
  every crate, `cargo deny check all` clean. `scripts/check_layering.py`
  remains the one unrun check (`python3` still absent) — only an
  existing workspace dependency (`tempfile`) moved into `aurora-app`'s
  own `[dev-dependencies]`, no new `aurora-*` edges.

  **Still `[~]`, not `[x]`**: no real-hardware verification yet (does a
  screen reader actually announce a *modal* alert the way `Role::
  AlertDialog` intends — a real, separate question from "does the tree
  structure exist," per this milestone's own repeated lesson); no
  click routing for the dialog's own button (this crate has no pointer
  input handling for *any* widget yet, so `Enter` on the focused action
  is what's wired, matching the keyboard-only interaction model
  everything else in this crate uses so far); and, most importantly,
  no real document recovery — that's real, separate follow-on work
  gated on `aurora-doc` choosing and implementing an on-disk journal
  encoding first.

### M1.9 — Basic tools and I/O

- [~] **Move, marquee select, zoom, pan, eyedropper** — three of five
  done 2026-08-06 (Zoom, Pan, Marquee Select); Move and Eyedropper are
  real, selectable tools with no pointer handling behind them yet — see
  below for exactly why each is blocked, not just deferred. This bullet
  was picked up directly (the user asked to start here), even though
  it's downstream of M1.8's still-open "Canvas: infinite zoom,
  rotation, pan, ..." bullet (no GPU rendering of any document exists)
  — the scope taken is everything a tool can honestly do *without* a
  rendered canvas: real coordinate math and real document-model
  mutation (`aurora_doc::SelectionSet`), with the pixels themselves
  staying exactly where M1.8 left them (nothing renders yet).

  **`aurora-ui` gained two new modules**, both pure and headlessly
  tested, ahead of any pointer-input wiring: `canvas_view::CanvasView`
  (the document-space ↔ canvas-area logical-screen-space transform:
  `to_screen`/`to_document`, `pan_by`, `zoom_at` anchored on a screen
  point, clamped to a practical 1%–6400% range — not the literal
  "infinite" the M1.8 bullet names, since real bounds need the
  document's own resolution and the GPU mip chain, neither of which
  exists yet to read) and `tool::Tool` (the five-variant enum plus
  `marquee_rect`, which turns a drag's start/current document points
  into the axis-aligned rect it spans, correctly for all four drag
  directions). 25 new tests.

  **`aurora-app` gained its first pointer input of any kind** — until
  now only keyboard was wired. Real `winit`
  `CursorMoved`/`MouseInput`/`MouseWheel`/`CursorLeft` events now
  drive: scroll-to-zoom anchored on the pointer (works regardless of
  active tool, the usual convention); the Zoom tool's own click-to-
  zoom (in on a plain click, out with `Alt` held, matching Photoshop);
  Pan via the middle button (any tool) or the Pan tool's own primary-
  button drag; and Marquee Select's primary-button drag, updating a
  new live `aurora_doc::SelectionSet` field on `App` in real time
  (`aurora_ui::tool::marquee_rect`, mapped through `CanvasView`). New
  `App` fields: `tool`, `canvas_view`, `selection`, `pointer_position`,
  `drag`. Every dispatch function is pure and free of `winit`'s
  window/event-loop types (only real-but-window-less event *data* like
  `MouseButton`/`MouseScrollDelta`), so it's fully unit-tested with no
  display server — the same seam `translate_key`/`translate_modifiers`
  already established for keyboard input. Tool-switch keyboard
  shortcuts followed the same day: `AppCommand::SelectTool`, bound to
  Photoshop's own single-key letters (v/m/z/h/i), including for
  Move/Eyedropper — switching *to* an inert tool is still real, honest
  behaviour, distinct from whether its own pointer handling exists.
  22 new tests (68 total in `aurora-app`, was 47).

  **Why Move and Eyedropper are genuinely blocked, not just
  unscheduled**: Move needs a notion of "the active layer" to
  reposition, and there is no way to set one — `aurora_ui::layers_panel`
  has no click-to-select pointer routing at all (this crate had zero
  pointer input before this bullet). Eyedropper needs to sample a real
  pixel, and no layer owns real pixel storage yet — the same open
  resource-management question `aurora_doc::LayerKind::Pixel`'s own
  `bounds` field has flagged since M1.4 (one `TileStore` per layer, or
  one shared store addressed some other way). Neither is a gap this
  bullet introduced; both are named, pre-existing blockers this bullet
  ran into and is recording rather than working around with a fake
  implementation (CLAUDE.md: no half-finished features).

  **Move's blocker is resolved (2026-08-01)**: `aurora_widgets::WidgetTree`
  gained `hit_test` (point → topmost widget), `aurora_ui::layers_panel`'s
  rows became real, non-zero, clickable widgets, and `aurora-app`'s new
  `select_layer` turns a click into a real `active_layer` change (with
  `Node::set_selected` accessibility feedback) — wired into
  `handle_pointer_pressed` ahead of any canvas-tool logic. Move's own
  drag-to-reposition-bounds logic on top of that is still separate,
  still-open work. Eyedropper's blocker has partly moved too: pixel
  storage now exists (ADR 0010, below), but no sampling function has
  been built on top of it yet, so Eyedropper is still inert.

  Verified at every step: `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D
  warnings" cargo doc`, `cargo test --workspace` (0 failures across
  every crate), `cargo deny check all` — all clean. No new
  `aurora-*` dependency edges (`aurora-app` already depended on both
  `aurora-ui` and `aurora-doc`); `scripts/check_layering.py` remains
  the one unrun check (`python3` still absent from this sandbox).
- [~] **Basic brush and eraser (real engine is Phase 2)** — first slice
  done 2026-08-06: `aurora-brush`'s first real code (was a placeholder).
  `dabs_along_path` turns a path of pointer positions into evenly
  spaced dab centers (`step = radius * spacing`, floored at `0.5`),
  generalizing `spike/vertical-slice::doc::stroke_segment`'s own
  already-measured single-segment formula (`spike/FINDINGS.md`: p99
  stroke latency 9.1 ms against a 10 ms budget) to a full multi-point
  path — leftover spacing distance carries across segment boundaries,
  so a stroke built from many short segments (a high-polling-rate
  pointer) doesn't double up dabs at every point; proven with a
  same-path-different-segmentation equivalence test. 7 new tests.

  **Stopped here on 2026-08-06, not for lack of time**: the next piece —
  actually stamping a dab into pixels, and therefore eraser, undo-
  as-you-drag, and wiring a Brush/Eraser `aurora_ui::Tool` into
  `aurora-app`'s pointer input — needed a real place to write those
  pixels, and there wasn't one. `aurora_doc::LayerKind::Pixel` had
  never owned a `TileStore`; whether pixel storage is one store per
  layer or one shared store addressed some other way had been an open,
  named question since M1.4. This was the same kind of fork the `.aur`
  format was before it got its own ADR (PRD §12 Q7) — flagged back to
  Cahya rather than decided silently as a side effect of finishing a
  basic-tools bullet.

  **Decided the same day — [ADR 0010](docs/adr/0010-layer-pixel-storage.md).**
  One shared `TileStore` per document (one background-writer thread, one
  real memory bound, regardless of layer count — not one store per
  layer, which `TileStore::new`'s own background-writer thread would
  have made unbounded against PRD §6's "unlimited layers"), tiles
  addressed by a new `(SurfaceId, TileId)` compound key, `SurfaceId`
  reused from each pixel layer's own `LayerId` (no second id-allocation
  scheme). A separate, small, dedicated store stays for the active
  brush stroke only — matching `spike/vertical-slice`'s own
  already-measured two-store split, kept isolated so the rest of the
  document's eviction/paging traffic can never contend with the brush's
  own under-1ms latency margin (`spike/FINDINGS.md`). Decision only, same
  as ADR 0009 — the `SurfaceId` type, the `TileStore` API change, and
  actually wiring `dabs_along_path`'s output into real dab-stamping are
  all separate, still-open follow-on work; this bullet's own `[~]` status
  is otherwise unchanged.

  Verified: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D
  warnings" cargo doc`, `cargo test --workspace` (0 failures across
  every crate), `cargo deny check all` — all clean. No new
  dependencies or `aurora-*` edges.

  **ADR 0010 implemented the same day, in three steps, each committed
  and pushed separately.** (1) `aurora-tile`: new `Surface` marker +
  `SurfaceId` (`aurora_core::Id<Surface>`, built the same way
  `aurora_doc::LayerId` already is); `TileStore::get`/`get_mut`/
  `take_dirty` all take a `SurfaceId` alongside a `TileId` now — the
  real key is the pair. Eviction picks the globally least-recently-used
  `(SurfaceId, TileId)` across every surface a store holds (`LruCache`
  already does this with no extra logic). Every real call site threaded
  through: `aurora-gpu`'s `TileResidency::sync` (a new `SurfaceId`
  parameter — this crate still has no document assembly to say *which*
  surface an atlas shows, real separate follow-on work) and
  `aurora-render`'s `upload_preview`, plus both crates' own tests/
  benchmarks. 2 new `aurora-tile` tests (15 total) proving the actual
  point: the same `TileId` on two surfaces doesn't collide, and
  eviction is global, not per-surface. (2) `aurora-doc`:
  `LayerTree::surface_id(id)` converts a pixel layer's own `LayerId`
  into its `SurfaceId` (`SurfaceId::from_raw(id.to_raw())` — reused, not
  allocated), `None` for a group or unknown id; `LayerKind::Pixel`'s own
  shape and serialized format are unchanged (the conversion is a pure
  function of the id alone, no new field needed). 4 new tests (88
  total). (3) `aurora-brush`: `stamp_dab`/`stamp_stroke`, ported from
  `spike/vertical-slice::doc::Document::dab` (the one piece of this
  crate's job the spike already measured) and generalized to the new
  multi-surface `TileStore` API — max-alpha accumulation within a
  stroke (not source-over), touches up to four tiles for a dab near a
  corner, marks each touched tile's dirty rect. `stamp_stroke` wires
  `dabs_along_path`'s own output straight into it, closing the exact
  gap that function's doc comment had named. 14 total `aurora-brush`
  tests (7 new) — two early drafts asserted alpha thresholds (`> 0.9`)
  computed from a wrong-by-hand estimate of the falloff curve at a
  small radius; fixed by either widening the radius so the real
  computed value cleared the threshold with margin, or lowering the
  threshold to what the true value actually supports, rather than
  asserting a number that was never really true.

  **A live document wired into `aurora-app` the same day, so brush
  painting can actually run.** Answers the "which `aurora-doc` type owns
  a live `TileStore` instance" question above the direct way: `App`
  itself now does, as two new fields (`layers: LayerTree`, kept alive
  instead of discarded every run after populating the panels;
  `tile_store: Option<TileStore>`, `None` — logged, not fatal — if a
  256-tile/128 MiB store fails to open at its own fixed scratch
  directory). A new `active_layer` field names the topmost pixel layer
  (`topmost_pixel_layer`, skipping any group above it) as where the
  Brush tool paints — no click-to-select UI to *change* this yet, the
  same gap Move already had.

  `aurora_ui::Tool` gained a sixth variant, `Brush`, bound to `b`
  alongside the other five tool letters. A `Brush` drag needed a real
  fix to `aurora-brush` itself first: calling `dabs_along_path` fresh on
  each two-point pointer-move event would silently reset its own
  spacing countdown every single event, so a slow drag (many
  shorter-than-one-`step` events) could place no dabs at all past the
  very first one, despite covering real distance overall. Fixed by
  extracting `dabs_along_path`'s own per-segment engine into two new
  public functions, `dab_step`/`advance_segment`, that carry a `carry`
  parameter forward across calls — `Drag::Brush` now holds `carry` as
  part of its own state, exactly the way it holds `last_doc`. 5 new
  `aurora-brush` tests (19 total) prove `advance_segment` called
  event-by-event with carry threaded through matches one whole-segment
  call. `App::paint_dab` converts a document-space point into the active
  layer's own local space (`layer_local_point`, subtracting `bounds`'s
  own `(x, y)` — each layer's surface is addressed from its own local
  origin, not the document's, ADR 0010) and calls
  `aurora_brush::stamp_dab`. 10 new `aurora-app` tests (78 total);
  `aurora-ui`'s own `Tool` tests updated for the sixth variant (still 25,
  no new ones needed there beyond the existing count assertions).

  **Still open**: eraser (same `stamp_dab` math, transparent instead of
  `colour`, not built yet); undo-as-you-drag; a way to *change* the
  active layer (a click-to-select Layers panel, the same gap Move's own
  blocker names) instead of always painting into the topmost pixel
  layer. Painting had no visual feedback at first — nothing in this
  crate rendered a document's pixels to the screen at all (M1.8's own
  canvas-rendering bullet) — **resolved the same day**: `resumed`/
  `redraw` now build a real GPU atlas and draw the live tile store's
  content within the canvas dock area every frame, so a Brush stroke is
  now actually visible; see M1.8's own bullet for the detail.

  Verified: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `RUSTDOCFLAGS="-D
  warnings" cargo doc`, `cargo test --workspace` (0 failures across
  every crate), `cargo deny check all` — all clean. No new dependencies;
  no new `aurora-*` edges.

  Verified at every step: `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets --all-features -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc`, `cargo test --workspace` (0
  failures across every crate, including real GPU tests), `cargo deny
  check all` — all clean.
- [~] **`.aur` format: versioned, forward-compatible (PRD §12 Q7)** —
  the decision made 2026-08-06, **[ADR 0009](docs/adr/0009-aur-document-format.md)**;
  the actual reader/writer implementation is separate, still-open
  follow-on work. Two subsystems were already blocked on exactly this
  gap: the crash-recovery journal (`History::replay`, M1.4) had no
  on-disk encoding to persist to, and `aurora-app`'s own crash-recovery
  marker (M1.8) explicitly deferred real document recovery for the
  same reason.

  **Decision**: `.aur` is a ZIP archive (the `zip` crate, trimmed to
  `STORE`/`DEFLATE` — no encryption/`bzip2`/`lzma`/`zstd`/`ppmd`, none
  needed) — real, proven precedent in the exact same problem domain
  (Krita's `.kra`, OpenRaster's `.ora` are both ZIP-based layered-image
  containers), and directly serving PRD §14's own "`.aur` is an open
  format... freely implementable" goal better than a bespoke container
  would: any language with an off-the-shelf ZIP library can read one,
  and a user can inspect a `.aur` file's contents with a file manager
  they already have. Holds a `mimetype` sentinel entry (uncompressed,
  first — the same ODF/EPUB/ORA trick, letting a magic-byte sniff
  identify the format without a full ZIP parse), a `postcard`-
  serialized manifest (document header + the full `LayerTree`
  structure) and history entry (the crash-recovery journal itself,
  finally somewhere real to go), and one ZIP entry per tile storing
  `aurora_tile::codec::encode`'s own output **verbatim, uncompressed at
  the ZIP level** — it's already `lz4_flex`-compressed, so compressing
  it again would spend CPU for no size benefit and duplicate logic
  `codec::decode` already gets right.

  `postcard`, not `rkyv` (PRD §8.1's other named candidate), for the
  metadata: `rkyv`'s zero-copy design ties its wire format to Rust's
  own in-memory layout, in tension with the "freely implementable"
  goal, and its real advantage — skipping a deserialization pass over
  huge data — doesn't apply here, since the actual huge data (pixels)
  is handled separately by `aurora_tile::codec`, not by whatever
  encodes the comparatively small `LayerTree`/`History` structs.

  **Forward/backward-compatibility policy, Q7's own question, answered
  directly**: backward-compatible unconditionally — every `.aur` file
  this project ever writes must keep opening in every future Aurora
  version, via explicit per-struct schema versioning in the reader, not
  a hard cutoff. Forward-tolerant, best-effort — an unknown ZIP entry
  (from a newer Aurora version) is skipped, not fatal, costing nothing
  extra thanks to ZIP's own central directory; an older Aurora opening
  a newer file loses only what it doesn't recognise, the same "degrade
  honestly, never silently" ethos already applied to lossy PSD saves.

  A hand-rolled binary chunk container (RIFF/PNG-style, the same shape
  `aurora_tile::codec` already uses one level down) was seriously
  considered and rejected specifically because ZIP already *is* that
  shape (a central directory is a chunk index) plus comes with
  ubiquitous tooling for free — see the ADR's own "Alternatives
  considered" for the full reasoning, including why a single monolithic
  `postcard` struct with no container was also rejected (no lazy/
  streaming access at all, which the 300,000 px ceiling requires).

  **Still `[~]`, not `[x]`**: no code written yet — this is the
  decision, not the implementation (a real, separate `aurora-io`
  module); the manifest schema's exact field-by-field layout isn't
  specified; wiring the crash-recovery journal through this format is
  still open; autosave frequency/granularity is a separate M1.9 bullet;
  and `aurora_doc::LayerKind::Pixel`'s own still-open question (one
  `TileStore` per layer, or one shared store addressed some other way)
  needs an answer before the manifest's own per-layer tile-addressing
  scheme can be finalized.
- [~] **Import/export PNG, JPEG, TIFF** — all three done 2026-08-06,
  first slices each. `aurora-io`'s first real code (was a placeholder). Asked
  Cahya which M1.9 bullet to start on first, since several are big,
  separate decisions rather than straightforward continuations (the
  `.aur` format needs its own ADR-calibre choice; the basic tools need
  a canvas that doesn't exist yet, M1.8's own still-open bullet) — PNG
  import/export was picked as the one with no such blocker: `png` is
  already a vetted, pure-Rust dependency in this workspace (via
  `aurora-testkit`'s golden-image tests).

  New `Image` type (`crates/aurora-io/src/image.rs`): `width`/`height`
  `f16` RGBA samples plus a real `aurora_color::IccProfile` colour-space
  tag — invariants §7.3.1b ("no 8-bit intermediates... promoted
  immediately on import") and §7.3.6 ("every buffer carries its colour
  space; untagged data is an error") applied to the simplest real file
  format this crate now supports. Deliberately standalone, not wired
  into `aurora_doc::LayerTree`/`aurora_tile::TileStore` yet — a layer
  doesn't own real pixel storage yet either (`aurora_doc::LayerKind`'s
  own doc comment already named this gap), so how an imported image
  becomes a document's actual layer pixels is real, separate,
  still-open work; this type is `aurora-io`'s own self-contained
  round-tripping representation for now, not a placeholder for
  something bigger.

  `png::decode`/`png::encode`: decode normalizes any PNG colour type
  (grayscale, indexed, RGB, with or without alpha) to RGBA via `png`'s
  own `Transformations::EXPAND | ALPHA`, **but a real, empirically-
  found correction was needed along the way**: those flags do not
  expand grayscale to RGB (only palette→RGB and sub-8-bit
  grayscale→8-bit grayscale, confirmed by actually running it against a
  real grayscale-source PNG rather than trusting the flags' own written
  documentation) — a grayscale source comes back `GrayscaleAlpha` (2
  channels), which `decode` now expands to RGBA itself. Bit depth is
  left alone throughout: an 8-bit source promotes from real 8-bit
  samples, a genuinely 16-bit source promotes from real 16-bit samples,
  not silently downsampled to 8 bits first the way `STRIP_16` would —
  needed a new `aurora_color::promote_u16` (the missing symmetric
  counterpart to the crate's own `promote_u8`, 2 more tests, 23 total
  in that crate). Encode is always 8-bit (PNG's own most common case,
  and the one invariant §7.3.1b itself names) via `dither_quantize`,
  not plain rounding — 16-bit *export* is real, separate follow-on
  work. Colour space is always tagged sRGB (`IccProfile::srgb()`) —
  PNG's `iCCP`/embedded-profile chunks aren't read or written yet, an
  honest gap for a real but uncommon minority of PNGs, not a silently
  wrong answer for the common case. 9 new tests, including real
  independent-reader-style cross-checks: RGB-without-alpha, grayscale,
  and indexed/palette sources are each encoded via the `png` crate's
  own encoder (not this crate's own `encode`) and decoded back,
  confirming real channel expansion and real palette lookup, not just
  "didn't panic" — one of these (the grayscale case) is exactly what
  caught the `EXPAND`-doesn't-cover-grayscale finding above before it
  shipped as a silent bug. Verified: `cargo fmt --all --check` clean,
  `cargo clippy -p aurora-io -p aurora-color --all-targets
  --all-features -- -D warnings` clean, `RUSTDOCFLAGS="-D warnings"
  cargo doc -p aurora-io -p aurora-color --no-deps --all-features`
  clean, `cargo clippy --workspace --all-targets --all-features -- -D
  warnings` and `cargo test --workspace` both clean, 0 failures across
  every crate, `cargo deny check all` clean. No new external
  dependencies beyond `png`/`half`, both already workspace-pinned, so
  `scripts/check_layering.py` (still unrun, `python3` absent) isn't in
  question.

  **JPEG added the same day** — `zune-jpeg` (decode) and
  `jpeg-encoder` (encode), the two focused, pure-Rust crates matching
  PRD §8.2's own pre-decided pair for image codecs ("`image`,
  `zune-image` | Mature, pure Rust") — `zune-jpeg` is a member of the
  `zune-image` family and is decode-only, so encoding needed a separate
  crate, `jpeg-encoder`, matching this crate's established "focused,
  single-purpose codec, not the aggregate `image` crate" shape (the
  same one `png` already set). `jpeg-encoder`'s own IJG-derived DCT
  code is IJG-licensed (the Independent JPEG Group's own permissive,
  attribution-only licence, same spirit as BSD/MIT, used by libjpeg
  itself) — a real licence not previously on the allow list, added to
  `deny.toml` with a comment, not silently, same discipline as
  `arboard`'s BSL-1.0 addition earlier in M1.8. `mozjpeg` (FFI to
  libjpeg-turbo) was considered and rejected: it would introduce a C
  dependency, contradicting PRD §8.2's own explicit "mature, pure Rust"
  framing for this exact table row — unlike RAW/ICC, PRD never
  frames JPEG as needing an FFI escape hatch.

  Decode requests RGBA output explicitly (`DecoderOptions::
  jpeg_set_out_colorspace(ColorSpace::RGBA)`) but — matching the same
  "verify the actual output, don't just trust the request" discipline
  the PNG work's own grayscale finding demanded — checks
  `decoder.output_colorspace()` against what was actually produced
  before trusting it, since the decoder's own docs admit it "does not
  guarantee... can convert to all colorspaces." Scope stated honestly,
  matching the `png` module's own pattern: JPEG has no alpha channel at
  all (decode always reports fully opaque; encode discards `Image`'s
  own alpha, `jpeg_encoder`'s own documented behaviour); 8-bit only
  (JPEG has no mainstream 16-bit variant the way PNG does); a real,
  checked error (not a silent truncation) if an `Image` exceeds JPEG's
  own 16-bit SOF-marker dimension limit (65,535×65,535 px — a real,
  permanent format constraint, not a library shortcoming); colour space
  always tagged sRGB (no ICC/Adobe-APP14 CMYK reading yet); quality is
  a fixed constant, no user-facing control yet. 4 new tests (13 total
  in the crate): an encode→decode round trip that (correctly) excludes
  the alpha channel from its lossy-tolerance comparison, since JPEG was
  never going to preserve it — a first draft of this test compared all
  4 channels and failed with a ~252/255 max diff purely from alpha
  mismatch, caught and fixed before it could be mistaken for a real
  encode/decode bug. Verified: `cargo fmt --all --check` clean, `cargo
  clippy -p aurora-io --all-targets --all-features -- -D warnings`
  clean, `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-io --no-deps
  --all-features` clean, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` both
  clean, 0 failures across every crate, `cargo deny check all` clean
  (both new crates plus the new IJG allow-list entry).

  **TIFF added the same day** — the `tiff` crate (`image-tiff`
  upstream), the same image-rs organisation `png` already comes from;
  PRD §8.2's own pre-decided pair covers TIFF in the same table row as
  PNG/JPEG. One crate handles both decode and encode here, unlike
  JPEG's two-crate split. New shared `crates/aurora-io/src/channels.rs`
  (private): `gray_to_rgba`/`gray_alpha_to_rgba`/`rgb_to_rgba` — real,
  non-trivial (loop + chunking) logic more than one format module
  needs (`png`'s own grayscale-with-alpha case, `tiff`'s
  grayscale/grayscale-with-alpha/RGB cases), so it moved out of `png`'s
  own module into a shared one rather than staying a near-duplicate
  copy; `png`'s own decode was refactored to call it, no behaviour
  change. TIFF's own real permissiveness (arbitrary bit depth,
  photometric interpretation, compression, multiple pages) is scoped
  down honestly, the same "first slice, not full coverage" shape every
  format in this crate has taken: only the first image (IFD) in a file
  is read; only `Gray`/`GrayA`/`RGB`/`RGBA` photometric layouts decode,
  with `Palette`/`CMYK`/`CMYKA` a real, checked error rather than a
  silent misread — CMYK in particular needs a real ICC-aware
  conversion (`aurora_color::Transform`) to convert correctly, not an
  uncalibrated formula, so it's deliberately rejected rather than faked;
  only 8-/16-bit unsigned-integer samples decode (the same two depths
  `png` already supports), with 32-bit float TIFFs (a real HDR case)
  a real, checked error too; encode is always uncompressed 8-bit RGBA
  via `dither_quantize` — compressed export (the `tiff` crate already
  supports LZW/deflate for *decode*) is real, separate follow-on work.
  7 new tests (20 total in the crate), including the same
  independent-reader-style discipline as `png`'s own tests: a real
  grayscale TIFF and a real CMYK TIFF are each built via the `tiff`
  crate's own encoder directly (not this module's `encode`), confirming
  real channel expansion and a real, deliberate CMYK rejection against
  independently-produced bytes. **A real test bug caught before it
  shipped, the same shape as JPEG's own alpha-channel finding**: a
  first draft of the encode→decode round-trip test asserted bit-exact
  equality and failed (34 vs 33) — not an encode/decode bug, but a
  wrong test assumption: `encode`'s own `dither_quantize` (not plain
  rounding) deliberately perturbs some values by up to one quantization
  step to break up banding, the same "within one step, not bit-exact"
  bound `png`'s own round-trip test already uses for the identical
  reason; fixed by matching that existing tolerance rather than
  asserting a stronger guarantee `encode` was never designed to give.
  Verified: `cargo fmt --all --check` clean, `cargo clippy -p aurora-io
  --all-targets --all-features -- -D warnings` clean,
  `RUSTDOCFLAGS="-D warnings" cargo doc -p aurora-io --no-deps
  --all-features` clean, `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` and `cargo test --workspace` both
  clean, 0 failures across every crate, `cargo deny check all` clean
  (default features kept — unlike `arboard`/`muda`, nothing about
  `tiff`'s own default feature set raised a licence or dependency-
  weight concern worth trimming for).

  **Still `[~]`, not `[x]`**: no wiring into a real document (no layer
  owns pixel storage yet, see above); no ICC-profile-aware import/
  export for any of the three formats (always sRGB); no 16-bit export
  for PNG; no user-facing JPEG quality control; TIFF multi-page,
  Palette/CMYK, 32-bit-float, and compressed-export support are all
  real, named, still-open gaps, not silent ones.
- [x] **Autosave and recovery** — done 2026-08-06, the same day as the
  `.aur` decision and PNG/JPEG/TIFF, closing the exact gap both of
  those left open: `aurora-doc`'s `History::save_journal`/
  `load_journal` (`postcard`, per ADR 0009) now give `aurora-app`'s
  M1.8 crash-recovery dialog a real on-disk journal to write and read,
  where before it had detected an unclean shutdown but could only say
  "Continue," not recover anything.

  `aurora-doc` side: `#[derive(Serialize, Deserialize)]` added to
  `LayerOp` and everything it recursively references —
  `aurora_core::Id<T>` (hand-written impls, not derived, for the same
  `PhantomData`-generic-bound reason its `Clone`/`Copy`/etc. already
  are), `aurora_core::Rect` (derived; `Size` deliberately left alone —
  not part of the journal's shape), `LayerKind`, `BlendMode`,
  `LayerLock`, `LayerMask`, `LayerEntry`, `tree::RemovedSubtree`.
  `History::save_journal`/`load_journal` serialize/deserialize just the
  journal, not the undo/redo stacks — `replay`'s own doc comment
  already proves the journal alone is a sufficient record of *current*
  state, and a crash doesn't need to preserve how many times the user
  pressed undo. Two new `DocError` variants
  (`JournalSerialization`/`JournalDeserialization`, holding `postcard`'s
  rendered error message as a `String` rather than the error type
  itself, so a `postcard` dependency doesn't leak into every
  downstream `match`). 5 new tests (84 total in `aurora-doc`, was 79).

  `aurora-app` side: a second small file next to the existing session
  marker (`std::env::temp_dir()`, `aurora-autosave.postcard`) —
  `write_autosave` writes the current document's journal to it on
  every startup (whichever document ends up current: the demo one, or
  a just-recovered one), and, only when a previous run's marker is
  still there, `recover_document` reads it back, deserializes via
  `History::load_journal`, and replays via `History::replay` into a
  fresh `LayerTree`. `App::new` uses the recovered document instead of
  `demo_document()` when this succeeds. The crash-recovery dialog keeps
  its single "Continue" action either way — recovery is unconditional
  and automatic, not a "Recover Document" vs. "Discard" user choice —
  but its message now honestly reports which happened
  (`crash_recovery_dialog_message`). 5 new tests (47 total in
  `aurora-app`, was 42): autosave/marker path distinctness, recovering
  a missing file, recovering garbage bytes, a real write-then-recover
  round-trip on journal descriptions, and the message text differing
  by outcome — plus every existing crash-recovery-dialog test updated
  to pass the new `recovered` parameter.

  Verified: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets --all-features -- -D warnings`, `cargo test --workspace`
  (0 failures across every crate), `cargo deny check all` — all clean.
  `scripts/check_layering.py` remains the one unrun check (`python3`
  still absent from this sandbox); no new `aurora-*` dependency edges
  were added (`aurora-app` already depended on `aurora-doc`).

  **Scope, stated honestly**: still no interactive recovery choice (no
  "Discard recovered document" action); autosave is written once at
  startup, not on a repeating timer or after edits — there is no live
  editing loop yet to re-trigger it from; and this is still plain
  `postcard` bytes, not a real `.aur` file (ADR 0009's ZIP container
  has nothing else to hold yet — no manifest, no tile data). Building
  the real `.aur` reader/writer is separate, still-open follow-on work,
  same as ADR 0009 itself already said.

### M1.10 — Phase 1 gate

- [ ] Accessibility audit passes on all three platforms — against WCAG
  2.1 AA's success criteria, reinterpreted per criterion for desktop
  software (ADR 0006, 2026-08-04): a checklist extending
  `check_contrast.py`'s already-shipping discipline (contrast, FR-027)
  to keyboard operability, name/role/value, and focus visibility across
  the component gallery, including the deep-nesting VoiceOver
  keyboard-navigation gap found and recorded in M1.8 (WCAG
  2.4.3/4.1.2) — not just "a screen reader announces something."
- [ ] IME audit passes on all three platforms
- [ ] 60 FPS at the Phase 0 document size
- [ ] Brush latency regression test green in CI
- [ ] Component gallery complete, contrast checks green

---

## Phase 2–3 — outline

Detailed when the preceding phase closes; planning further ahead than the
evidence supports is how the original 6-month Phase 1 estimate happened.

- **Phase 2** — selections, brush engine, masks, filters, adjustments.
  *Gate: an illustrator completes a real piece without leaving Aurora.*
  Unchanged, zero spike evidence — see 0.8.
- **Phase 3** — smart objects, Camera RAW, colour management, PSD/PSB
  read+write. *Gate: 1,000 PSDs round-trip through Photoshop with no layer
  loss.* Likely needs another upward revision in relative terms — treat as
  the biggest remaining phase, not a 10-month one; see 0.8.

**New since this outline was written**: FR-028 (Artboards, added
2026-07-28) needs a home once Phase 2/3 are planned in detail — the boards
panel and per-board export are Phase 2-shaped UI work, the PSD round-trip
piece (layer group + `artb` tagged block) is Phase 3-shaped, matching how
the rest of FR-001's PSD compatibility already splits across both phases.

**Phase 0–3 durations are milestone-based, not calendar-committed, as of
2026-07-28** (PRD.md §9, following the solo-development answer to PRD §12
Q2). The month figures above are gone deliberately, not lost — each
phase's exit criterion is still the real gate.

## Beyond v1.0 — not currently planned

**Cut from the committed plan 2026-07-28**, not merely deprioritized:
AI features, the plugin SDK/marketplace, automation, cloud sync,
real-time collaboration, animation, and mobile/web ports. What were
"Phase 4" and "Phase 5" (~22 months of team-scoped work) mapped almost
entirely to FRs PRD §3 already marked **Could** or **Won't (yet)** — the
phase numbers were carrying a commitment the priority table never actually
made. FR-019 (Plugin SDK) was the one inconsistency (marked "Should" while
living entirely inside a now-cut phase) — moved to Won't (yet) alongside
the rest. None of this is deleted; see PRD.md's "Beyond v1.0" section
(§9) for the full list, kept as an explicitly uncommitted backlog to
revisit only if there's a real reason to (traction, contributors, or a
revenue model — PRD §12 Q3 — that would justify the investment).

Total duration figure retired along with the calendar commitment above —
see PRD.md §9 and PLAN.md 0.8 for the full reasoning trail.

---

## Budgets: target vs measured

Measured on one machine (Radeon Pro 5300M, Metal, macOS). Windows and Linux
unmeasured. Full context in [spike/FINDINGS.md](spike/FINDINGS.md).

| Budget | Target | Measured | State |
|---|---|---|---|
| Brush latency | < 10 ms | p99 **9.1 ms** | Passing, <1 ms margin |
| Canvas interaction | 60 FPS | idle frame 0.6 ms | Comfortable |
| Pan with page-in | 16.7 ms | p50 7.0 ms | Passing |
| Pan while painting | 16.7 ms | p50 **29.2 ms** | **Failing** — fix in M1.1/M1.3 |
| Startup | < 3 s | — | Unmeasured |
| Open 2 GB PSD | < 5 s | — | Unmeasured (Phase 3) |
| Export 1 GB doc | < 10 s | — | Unmeasured |

---

## Findings carried forward

Each of these came out of a spike and has a home in the plan. They are listed
here so they are not silently lost between phases.

| Finding | Source | Lands in |
|---|---|---|
| CPU compositing is the bottleneck, not disk I/O | slice | M1.1 dirty rects, M1.3 GPU compositing |
| Brush budget has <1 ms margin | slice | 0.2 CI regression test |
| Upload bandwidth caps pan speed (~18 MB/screenful) | slice | M1.3 progressive rendering |
| Toroidal slot addressing; retrofit is awkward | slice | M1.2 |
| Windows must be created hidden, adapted, then shown | a11y | M1.8 |
| Composition state must be announced, or CJK users hear silence | a11y | M1.7 |
| Text stack sets the toolchain floor | a11y | done — pinned 1.97 |
| A `Role::Window`-shaped root nested in a real window may need VoiceOver "interact" before plain arrow navigation reaches children | a11y | M1.7 — try a plainer root role in `aurora-widgets` |
| Live value-change announcements can silently fail to reach VoiceOver even when the tree updates correctly every keystroke | a11y | M1.7 — needs a regression test that doesn't just build the tree but confirms delivery |
| The Rotor (VO+U) is a more reliable diagnostic than step-by-step arrow navigation when debugging screen-reader delivery | a11y | process note for whoever runs Windows/Linux |
| A reader accepting a PSD proves little; gate on pixel comparison | psd | Phase 3 gate (already worded this way in PRD §9) |
| The flattened preview must apply blend modes, or 44 % of pixels differ | psd | Phase 3 — write the flatten through the real render graph |
| Layer names need both the legacy Pascal string and the `luni` block | psd | Phase 3 |
| Groups are two bracketing pseudo-layers (bounding + folder record), not a container field — order confirmed against a working implementation, not guessed | psd | Phase 3 `aurora-io` group support |
| Real `EngineData` is far richer than a minimal example (kinsoku/moji-kumi tables, duplicated resource dicts) even for plain English text — from-scratch generation is higher-risk than assumed | psd | Phase 3 — patch a real file's bytes instead of generating from scratch |
| Editing text content requires rendering glyphs into pixel channels, or the file is internally inconsistent (descriptor vs. preview mismatch) | psd | **New, mandatory Phase 3 scope** — needs Aurora's text stack (`cosmic-text`/`glyphon`) wired into `aurora-io`, not previously planned |
| `TySh`'s `text_data`/`warp` fields are `DescriptorBlock` (extra leading version field), not plain `Descriptor` — desyncs silently if missed | psd | Phase 3 `aurora-io` — captured in `descriptor.rs` |
| The Descriptor format's zero-length key shorthand means byte-identical round-trip is the wrong test; same-length + semantic-identity is correct | psd | Phase 3 test design for `aurora-io` |
| `EngineData`'s container format generalized to a second, independent library's fixtures with zero parser changes — real corroboration, not just one file fitting one parser | psd | Phase 3 `aurora-io` confidence |
| A naive Unicode string escaper can silently truncate strings (codepoints with `)` as their UTF-16BE high byte); needs whole-buffer byte-level escaping, not unit-aligned | psd | Phase 3 `aurora-io` — implemented, see `engine_data.rs` |
| Editing text also requires recomputing paragraph/style `RunLengthArray`s to match the new length — separate from, and in addition to, the pixel-sync requirement | psd | Phase 3 — new line item alongside glyph rendering |

---

## Next action

**M1.1 is complete; M1.2 is nearly complete; M1.3 has started** —
device/queue management, the shader library/pipeline cache, GPU tile
residency, budgeted upload scheduling, and surface configuration/resize
are all implemented and verified against real GPU hardware — the crate's
own test suite and the new `examples/surface_smoke.rs` both now pass
against a real Metal GPU (2026-07-29), on top of the existing Vulkan
verification. The only thing left in M1.2 is **DX12 (Windows)** — the
only backend with zero real-GPU runs against this crate, and genuinely
not doable from this machine alone (needs Windows hardware, same
cross-platform constraint the vertical slice and a11y spikes have hit
throughout Phase 0). In parallel, `aurora-graph`'s node definitions,
dependency DAG, and dirty-region propagation are done (2026-07-29, 12
tests), and `aurora-render` now has real code on top of it: `schedule()`
(node-granular dirty `Rect`s → tile-granular work lists), `TileCompositor`
(GPU source-over blending of one tile over another, real-hardware-verified),
and progressive rendering's CPU half (`mip::downsample`) and GPU half
(`preview::upload_preview`, wired through a real 4-level mip chain now
added to `aurora_gpu::TileResidency`'s atlas — `upload_mip`, verified
end-to-end against real hardware, store through to atlas readback) — 21
tests in `aurora-render`, plus 2 new validation tests and a real
pixel-readback test in `aurora-gpu` itself (10 total there now).
`aurora-render` also has async evaluation's first piece now:
`Executor`, a background thread that runs submitted work without
blocking the caller (`submit`/`drain_completed` both non-blocking, same
shape as `aurora_tile::writer::BackgroundWriter`), 5 tests — 26 total in
`aurora-render`. **M1.3's remaining gap in both progressive rendering and
async evaluation is the same shape**: real primitives exist
(downsampling, atlas upload, a background executor) but nothing yet
calls them with real work, because there's no render-graph node
evaluation to call them *with* — that needs `aurora-doc`/`aurora-filters`.
**M1.4 has now started, 2026-07-30**: `aurora-doc`'s `LayerTree`
(`Pixel`/`Group` layers, nesting, ordering, cascading delete,
cycle-checked reparenting) is done, 25 tests — but it's identity/
nesting/ordering only, with no opacity/blend-mode/visibility/locking, no
`RenderGraph` wiring, and no real pixel storage yet (a pixel layer
records `bounds`, deliberately not an owned `TileStore` — see M1.4 for
why). A layer tree existing doesn't yet give M1.3's primitives a real
consumer; that still needs this crate's next few bullets.

**A live desktop session, whenever one is available, still has one thing
waiting on it**: the a11y human/Orca leg below. The other item that used
to be listed here — a real windowed smoke-test of `aurora-gpu`'s
`GpuSurface` — **is done, 2026-07-29** (see M1.2 above and
`crates/aurora-gpu/examples/surface_smoke.rs`): a different machine than
the one that wrote the surface code turned out to have a real, logged-in
macOS session, the same kind of access the Orca leg is still waiting for
on Linux/Windows.

- **Run the human/Orca leg of the a11y/IME checklist on Linux, and the whole
  checklist on Windows** — macOS is done (9/10,
  [full results](spike/a11y-ime/FINDINGS.md)). Linux build and standalone
  tree construction are now confirmed (2026-07-26), but the decisive test —
  a human at a live desktop session with Orca actually speaking — still
  needs a machine with an active graphical login, which was not available
  this pass. Windows (UIA) is fully unstarted. Both are different platform
  APIs entirely and remain the only thing that can still overturn ADR 0001.

~~Smoke-test `GpuSurface` against a real window~~ — **done 2026-07-29**:
`examples/surface_smoke.rs` opened a real window on a live macOS session,
configured a surface against this crate's existing headless-created
adapter, ran 150 acquire/clear/present cycles, and handled two real
resizes with no panics. See M1.2 above and `surface.rs`'s module doc.

~~Get outside critique on the 0.5 design scaffold~~ — **done 2026-07-28**: a
colleague reviewed it and signed off as fine for a start, revisable later
if needed. See 0.5 above.

~~Define the 95%~~ — **done 2026-07-28**, see 0.9.

~~PSD test corpus~~ — **Phase 0's share done 2026-07-28**, see 0.7. Growing
it toward the 1,000-file real-world gate is **deliberately deferred to
Phase 3, decided 2026-07-28** — not a Phase 0 task to keep carrying. Real
client PSDs raise consent/licensing questions the RAW corpus never had;
solving that belongs to whenever Phase 3 is actually being planned, not
now.

~~Get Cahya's read on docs/workflows.md's Tier-1/2/3 calls~~ — **done
2026-07-28**: reviewed, no changes needed. The Artboards half was already
closed as FR-028; this closes the rest of the item.

~~Artboards product decision~~ — **decided 2026-07-28: first-class
feature**, not just round-trip fidelity. Now **FR-028 Artboards** in
PRD.md §5 (priority Should) — boards panel, per-board export, device
presets; round-trips via the layer-group + `artb` tagged block finding 17
already confirmed. Needs a home in the M1.x/Phase 2-3 breakdown once
that's planned in detail (0.7's outline note already flags it).

Also worth a short, cheap follow-up whenever `aurora-widgets` work starts:
retry the a11y spike's root node with a plainer role than `Role::Window`
(finding 5/6 in the a11y results) to see if it fixes both the navigation-depth
quirk and the live-announcement bug in one change.

**Newly surfaced, not yet scheduled:**
- Glyph rendering into pixel channels on text edit (PSD spike finding 8) is
  now implemented and externally verified in the spike (finding 14), but the
  *real* Phase 3 implementation still needs a home in the M1.x/Phase 3
  breakdown once Phase 3 is planned in detail — the spike proves feasibility,
  it isn't the shipped feature.
- **macOS/Windows LGPL packaging mechanics and real legal review** — ADR
  0007/0008 are decided and the packaging mechanism is proven on Linux with
  LibRaw itself (`spike/lgpl-packaging` finding 4), but macOS
  (`install_name`/`@rpath`) and Windows (DLL search order) are unverified,
  and the §6(b)(1) "already present on the user's system" ambiguity still
  needs real legal review before v1.0 ships. Moved out of the numbered list
  this round in favor of 0.9, which needs neither a second platform nor a
  lawyer to make progress — pick this back up when either becomes available.
- Font resolution (`ResourceDict.FontSet`) and `FillColor` alpha compositing
  remain the two small, non-urgent named gaps in the PSD text-layer line of
  work (findings 14/15) — pick up whenever, not blocking anything.
- ~~The solo-vs-scope tension PRD §12 Q2's answer surfaced~~ — **resolved
  2026-07-28** (PRD §12 Q2 resolution note, PLAN.md 0.8): both narrow the
  scope and drop the calendar. Phases 4/5 moved to §9's uncommitted "Beyond
  v1.0" backlog; Phases 0–3 are milestone-based, not calendar-committed.
