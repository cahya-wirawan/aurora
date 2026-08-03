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
harness (`aurora-testkit`, a new 20th workspace crate — see 0.2). The
remaining Phase 0 items (ADR 0006, Windows/300k-px slice re-runs,
macOS/Windows LGPL packaging, deeper PSD format coverage) continue in
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
| **Phase 1 — M1.8** | **Started 2026-08-02, human-verified on macOS 2026-08-03, blocked only on Windows/Linux verification.** `aurora-app`'s first real code (was a placeholder `main()`): a real `winit::ApplicationHandler` implementing the "create hidden → attach `accesskit_winit` adapter → show" ordering ADR 0001's escape-hatch check found, reusing `aurora-gpu`'s already-proven `GpuContext`/`GpuSurface` and `aurora-widgets`' `WidgetTree`/`FocusManager` for a (currently content-free) accessibility tree rather than hand-rolling either. Real error handling throughout, `main` now fallible. Written blind (no `pkg-config` in this sandbox, no root to install it) and pushed; CI's first real run immediately caught a genuine bug — `wgpu` used directly but never declared as a dependency — fixed the same day. Cahya then installed `pkg-config`/`libfontconfig1-dev` in this same sandbox, closing the gap that had blocked `cargo clippy --workspace --all-targets --all-features -- -D warnings`/`cargo test --workspace` (the exact CI gates) all session — both now pass completely, every crate. **Then Cahya ran `cargo run -p aurora-app` on real macOS hardware**: the window opens (create-hidden → adapt → show all working for real), resizing works with no crash, and **VoiceOver announces the window** — the accessibility tree genuinely reaches a real screen reader, this project's first non-spike code to do so. The window's clear colour is now a real theme token too (`load_background_color`, `design/themes/dark.toml`'s `surface.app`, correctly converted sRGB→linear for the `Bgra8UnormSrgb` surface via `aurora_color::srgb_to_linear` — using the raw sRGB bytes would have washed the colour out), 2 more tests. `aurora-ui`'s first real code (was a placeholder too): a static docking/panel skeleton matching the owner-approved workspace mockup — canvas area + a side rail of three labeled (`Role::Region`) panels (Layers/Properties/History), reusing `aurora_widgets::WidgetTree<WidgetKind>` directly rather than inventing a parallel widget model, flex-ratio sized (no un-tokenized pixel widths) — wired into `aurora-app` so `compute_layout` runs live on window creation and resize. Verified empirically (a real 1000×800 layout test), not assumed. No drag-to-redock/resize/persisted-layout or real panel content yet — that's the actual "docking"/"custom workspaces" half, still open. **Two real bugs found and fixed from this same live-hardware session**: (1) `WidgetTree::accessibility_update` never set `node.children`, so any tree past a trivial single root looked disconnected to `accesskit_consumer` and crashed on launch — fixed, plus a real regression test using `accesskit_consumer::Tree::new` itself; (2) even after that fix, the workspace was completely unreachable from VoiceOver (the Rotor's "Window Spots" came back empty, not even the window title) — root cause was the tree's root using `Role::GenericContainer` instead of `Role::Window`, fixed and **re-verified live**: VoiceOver's Rotor now lists both "Aurora" (the window) and "Layers." "Properties"/"History" didn't show in that same Rotor listing (likely a Rotor display quirk, not a structural gap — all three are built identically and verified by the test suite). The Layers panel now has real content too: `aurora-ui`'s new `populate_layers_panel` turns a real `aurora_doc::LayerTree` into one accessible `Role::ListItem` row per layer, nested to mirror group structure — wired into `aurora-app` via a small, clearly-fake three-layer `demo_layers()` (Background, Color balance at Multiply/80%, Retouch — skin), since there's no real "open a document" flow yet. Not yet re-verified live. Still `[~]` overall: Windows and Linux remain unverified on real hardware. See M1.8 |

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
- [ ] ADR 0006 — accessibility conformance target (WCAG 2.1 AA / Section 508 / EN 301 549)
- [x] ADR 0007 — RAW decode library: LibRaw via FFI — [adr/0007](docs/adr/0007-raw-library-libraw.md); Cahya's decision, informed by `spike/raw-icc` and `spike/lgpl-packaging`
- [x] ADR 0008 — ICC transform library: lcms2 via FFI — [adr/0008](docs/adr/0008-icc-library-lcms2.md); no packaging complexity, unlike ADR 0007 — Little CMS's core is MIT

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
  decided silently while "starting the layer tree."
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
- [ ] Canvas: infinite zoom, rotation, pan, rulers, guides, grid, snap
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
  check all` clean. **Not yet re-verified on real hardware** — this is
  the next thing worth checking with VoiceOver: does the Layers panel
  now read real layer names/descriptions instead of an empty region?
- [ ] Command palette, keyboard shortcuts
- [ ] Native menus, file dialogs, drag & drop, clipboard
- [ ] Per-monitor DPI and fractional scaling
- [ ] OS settings: reduced motion, high contrast, text size
- [ ] Crash recovery UI

### M1.9 — Basic tools and I/O

- [ ] Move, marquee select, zoom, pan, eyedropper
- [ ] Basic brush and eraser (real engine is Phase 2)
- [ ] `.aur` format: versioned, forward-compatible (PRD §12 Q7)
- [ ] Import/export PNG, JPEG, TIFF
- [ ] Autosave and recovery

### M1.10 — Phase 1 gate

- [ ] Accessibility audit passes on all three platforms
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
