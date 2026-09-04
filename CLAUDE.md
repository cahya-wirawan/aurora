# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

**A real, running editor — Phase 1 in progress.** As of `0.100.1`: roughly 115,600 lines across 20 crates, 1,662 tests passing, and the full CI gate green. The app opens PNG/JPEG/TIFF and Aurora's own round-tripping `.aur` format; paints and erases real pixels with undo/redo; pans and zooms; handles multiple layers and groups with opacity, masks with real per-pixel grayscale coverage, and all 27 PSD-compatible blend modes composited for real (a GPU fast path for the common case, a CPU path for groups and every other blend mode); and saves the full composite, not just the active layer. Verified interactively on real macOS hardware, including a screen reader announcing the window.

Five crates are still skeletons holding only a placeholder `crate_name()` and one test: `aurora-text`, `aurora-filters`, `aurora-ai`, `aurora-plugin`, and the `aurora-cli` binary. Everything else is real code.

**[PLAN.md](PLAN.md) is the progress tracker** — task-level status per milestone, every `[x]` backed by linked evidence. Check it before starting work, and update the relevant checkbox in the same commit as the work. Its "Where we are" and "Next action" sections are the maintained, current summary; this file's summary is deliberately shorter and goes stale faster, so **PLAN.md wins on any disagreement**. README.md's status paragraph is also kept current, aimed at outside readers.

### What is not done

- **M1.10, the Phase 1 gate, is what is actually open.** Its remaining items are hardware-gated (below), design-owner-gated (which gallery component to build next is Cahya's call, not an engineering one), tooling-gated (the PSD spike needs `psd-tools` as an independent reader), or genuinely large multi-round work (all 26 blend-mode formulas ported to WGSL — Slice 1, proving the shader can read back a former render target as a texture, landed in 0.83.0–0.83.2 for Multiply only, entirely unwired from the running app; wired into the real GPU compositing predicate in 0.84.0–0.84.2, so a real flat (non-grouped) document with `Normal`/`Multiply`/`Dissolve` layers now genuinely composites on GPU — Dissolve needed no new WGSL, since it's resolved to `Normal` before the GPU dispatch ever sees it. A second real mode, `Darken`, landed in 0.85.0–0.85.2, following the same template and confirming it generalizes to a third expressible mode sharing one ping-pong accumulator pair; the two GPU-side compositor methods were then merged into one shared, per-mode-parameterized helper (0.85.1) once the "wait for a third sample" deferral didn't survive its own diff. A third mode, `Lighten`, landed in 0.95.0 and cost exactly what that merge predicted — one WGSL function, one `BlendPass` const, one wrapper, and no change to the shared helper. Verified on Vulkan/NVIDIA only; Metal/DX12 unverified for this mechanism, which matters because the app's own default startup document already carries a `Multiply` layer, so every user's first frame now takes this path. A real, disclosed gap found in 0.85.0/0.85.1 and worth carrying forward: no test can tell "the GPU path genuinely dispatched" apart from "it silently fell back to CPU, which computes the same correct pixels" — closed for `Darken` with a test-only dispatch counter, and for `Lighten` in its own first round, still open for `Multiply`/`Dissolve` as a named follow-on. **22 of 27** blend modes are still CPU-only at the app's GPU predicate (23 of 26 at the `aurora-render` shader level, which excludes `Dissolve`), and group support remains). Mask *coverage* is real per-pixel grayscale as of 0.70.0 and survives a `.aur` save/load round trip as of 0.71.0; what is still missing there is a brush/tool UI for painting a mask and mask-pixel undo/history proper. Mask-surface lifecycle cleanup is now mostly closed at the library level (0.80.0–0.81.1): a discarded document's tiles (content and mask, including every subtree on the undo/redo stacks and the crash-recovery journal) can now be freed, and re-adding a mask to a layer whose old one was removed no longer resurrects the old coverage — though undoing back *past* that re-add can still restore an old mask reading the newer one's coverage, an accepted, tested residual of the same derived-surface-id root cause. None of this is reachable through the shipped app yet (no UI calls any of it) — see PLAN.md's M1.9 mask entry for the full account.
- **Phase 0 has a tail.** Windows/DX12 validation, the Linux and Windows human legs of the accessibility/IME checklist, and macOS/Windows LGPL packaging all need hardware and a human.
- **[ADR 0001](docs/adr/0001-custom-wgpu-ui.md) is still not de-risked** — the project's most durable open risk. macOS accessibility passes 9/10 ([spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)); Linux (AT-SPI) and Windows (UIA) are entirely unverified, and each is a different platform API. Cahya accepted this as a risk rather than a blocker for starting Phase 1 (2026-07-28). It remains the one open item that could overturn the custom-`wgpu`-UI decision.
- **60 FPS is measured and failing** — see "Measured, not assumed" below.
- **Not started at all**: PSD/PSB in the app (a feasibility spike only, not wired in), filters and adjustments, selection tools beyond a raw data model, smart objects, RAW import, plugins, scripting, AI, and true infinite pasteboard panning (panning deliberately stops at the document's own edge).

### Spikes

Five, all outside the workspace so they can never become dependencies of real code: `vertical-slice` (performance, [FINDINGS](spike/FINDINGS.md) — read before touching tile, render, or brush code), `a11y-ime`, `psd-write`, `raw-icc`, `lgpl-packaging`.

Two constraints from the a11y spike still shape real code: **windows must be created hidden, adapted, then shown** (`accesskit_winit` panics otherwise — this is why `aurora-app` manages windows the way it does), and the text stack sets the toolchain floor (`cosmic-text` needs ≥1.89, which is why the pin is 1.97).

### The lesson from the last round

The first live interactive session on real hardware (2026-08-12/13) found four bugs — stroke slowness, paint offset from the cursor on Retina, sub-tile pan doing nothing, pan freezing at the edge — that 900+ passing tests, a green CI gate, and a headless software-Vulkan sandbox had all missed. GPU tests self-skip when no adapter is present, and a typical Linux dev box here has only Mesa llvmpipe software rendering. **A green test run is not evidence that canvas or UI work is correct.** Say so plainly rather than implying verification that did not happen, and expect anything involving DPI, real GPU timing, or interactive feel to need a human on real hardware.

## Commands

```sh
cargo build --workspace                 # build everything
cargo test --workspace                  # all tests
cargo test -p aurora-tile               # one crate
cargo test -p aurora-tile -- name_of_test   # one test
cargo nextest run --workspace           # what CI runs (faster, better output)

cargo fmt --all                         # format
cargo fmt --all --check                 # CI check
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 scripts/check_layering.py       # crate layering rule (PRD §7.2)
python3 scripts/check_no_hardcoded_style.py  # no literal colours/sizes in widget code (FR-027)
cargo deny check all                    # licences + advisories
cargo doc --workspace --no-deps --open  # docs

python3 design/check_contrast.py        # every token pair, every built-in theme
cd design && python3 build_tokens_css.py > tokens.css   # regenerate the gallery's CSS

cargo run -p aurora-app                 # the application
cargo run -p aurora-cli                 # headless binary
```

The spikes are separate crates, deliberately outside the workspace (root `Cargo.toml` `exclude`s them) so they can never become dependencies of real code:

```sh
cd spike/vertical-slice
cargo run --release -- --headless       # the benchmark; no display needed
cargo run --release                     # windowed, drag to paint, Esc for stats
```

The full CI gate locally, in the order CI runs it:

```sh
cargo fmt --all --check && python3 scripts/check_layering.py \
  && python3 scripts/check_no_hardcoded_style.py \
  && cargo check --workspace --locked \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace
```

`cargo check --workspace --locked` looks redundant next to the two lines under
it and is not. Every other compile step in the gate passes `--all-targets`
and/or `--all-features`, so none of them build the plain configuration Aurora
ships. `aurora-doc`'s `test-support` feature (this workspace's first Cargo
feature) gates a test-only escape hatch, and a non-test `pub fn` in a shipping
crate calling it was measured to pass `cargo build --workspace --all-targets`
*and* `cargo check --workspace --all-features` while failing only a flagless
build. That step is what makes such a leak fail the gate.

Neither design script is in CI, and `design/tokens.css` is a review aid for the
HTML mockups and gallery — Aurora itself never uses CSS. The TOML files under
`design/tokens/` and `design/themes/` are the source of truth; regenerate after
any token edit rather than hand-editing the CSS. (It went un-regenerated from the
original scaffold through 0.48.0, because the generator crashed on the scalar
`border.control_opacity` 0.18.0 added — fixed in 0.48.1.)

`cargo nextest` is not always installed locally — `cargo test --workspace` is an
acceptable stand-in, and CI additionally runs `cargo test --workspace --doc`
(nextest does not run doctests) plus `cargo doc --workspace --no-deps
--all-features`, so a broken intra-doc link fails CI even when every test passes.

GPU-backed tests self-skip when no adapter is present, and print `SKIPPED` rather
than failing. A dev box with only Mesa llvmpipe still runs them, but in software
— see "The lesson from the last round" above before treating those numbers, or
those passes, as evidence about real hardware.

Set `AURORA_REQUIRE_GPU` to turn that self-skip into a hard test failure, so a
runner that is *supposed* to have a real adapter cannot go green while every
GPU-gated test silently skips. With it set, two things fail the test: no adapter
at all, **and** an adapter `wgpu` reports as `DeviceType::Cpu` — llvmpipe here,
WARP / "Microsoft Basic Render Driver" on Windows — since a software rasterizer
standing in for hardware is the same gap wearing a different hat. Unset it is a
complete no-op, `SKIPPED` line included, and a CPU adapter stays perfectly fine
for ordinary dev-box runs. Parsing: unset is off, and so are the present-but-falsy
values `` (empty), `0`, `false`, `off`, `no` (trimmed, case-insensitive) —
GitHub Actions sets a variable to the empty string when a `${{ }}` expression is
empty, and treating that as "on" would fail a whole matrix for a reason the
workflow file never states. Any other present value is on.

**No regular push/PR workflow sets it yet** — which runner, if any, should is
still an open decision, but `.github/workflows/gpu-probe.yml` (0.62.0) is a
manual, `workflow_dispatch`-only job that runs the workspace's GPU-gated
tests on `macos-latest` with it set, specifically to find out whether that
runner has a real adapter before committing either way — see PLAN.md for
how to read its result. The single implementation is
`aurora_gpu::test_support::real_context_or_skip` (behind `aurora-gpu`'s
`test-support` feature); every crate's `real_context()`/`real_gpu_context()`
helper delegates to it, and it prints the selected adapter's name, backend and
device type on every successful creation so a CI log records what was actually
tested. What it asserts is that a real adapter *exists*, not that any particular
test body ran — an `#[ignore]`d test still contributes a silent pass.

Toolchain is pinned in `rust-toolchain.toml` (1.97, edition 2024 — `cosmic-text` requires ≥1.89, so the text stack sets the floor). CI runs on Linux, macOS, and Windows from the first commit — cross-platform breakage is cheap to fix now and catastrophic in month 30.

## Lints worth knowing

The workspace denies `unwrap`, `expect`, `panic`, and `indexing_slicing` (root `Cargo.toml`). This is deliberate: Aurora holds a professional's unsaved work, and a panic loses it. Return errors instead. A crate needing `unsafe` must override `unsafe_code` in its own `[lints]` table rather than the workspace's, so the exception is visible in review. **`aurora-tile` is the one exception** (0.61.0): `store::create_private_dir`'s scratch-directory hardening needs `fchmod` on an open descriptor and `geteuid` for an ownership check, neither exposed by `std`, only by `libc` FFI. Its `[lints]` table copies every other workspace lint verbatim and overrides only `unsafe_code` to `"allow"`; every other crate is still `unsafe_code = "deny"` — keep it that way if you can.

## Versioning

SemVer, started at `0.0.1`, currently `0.100.1`. The single source of truth is `[workspace.package].version` in the root `Cargo.toml`; every crate inherits it via `version.workspace = true` — bump it in exactly one place. The commit subject carries the new version in parentheses, e.g. `Clamp canvas pan to the document's own top-left edge (0.47.1)`.

- **Minor** (`0.X.0`): every PLAN.md step — a task-level unit of work landing in its own commit (the same granularity PLAN.md's own checkboxes track).
- **Patch** (`0.0.X`): a bug fix — correcting something that was already landed and wrong, not new work.
- Bump the version in the same commit as the work it covers, the same discipline PLAN.md's own checkbox updates already follow.

A release is a `vX.Y.Z` tag (e.g. `v0.1.0`) pushed once the matching version bump is committed — pushing that tag is what actually publishes a GitHub Release, so treat it as a deliberate, user-approved action, not a routine one. `.github/workflows/release.yml` re-runs the full gate (`verify.yml`, the same jobs `ci.yml` runs on every push/PR) against the tagged commit, confirms the tag matches `Cargo.toml`'s own version, then creates the Release. Ordinary commits and PRs are still checked immediately via `ci.yml` regardless of tags — CLAUDE.md's own "cross-platform breakage is cheap to fix now" principle applies to every commit, not just tagged ones.

## What Aurora is

A cross-platform, GPU-accelerated, non-destructive professional image editor (a Photoshop alternative) for Windows, macOS, and Linux. PSD/PSB compatibility and AI-assisted editing are first-class requirements, not add-ons.

## Architecture (PRD §7)

A single Cargo workspace with crates layered so dependencies point **downward only** — `core` → `tile` → `graph`/`gpu` → `render`/`doc` → feature crates (`filters`, `brush`, `vector`, `text`, `io`, `ai`, `plugin`, `theme`) → `widgets` → `ui` → `app`/`cli`. PRD §7.2 has the full table. A lower crate must never depend on a higher one; CI enforces this.

`aurora-widgets` (the general-purpose toolkit) knows nothing about documents or layers and must stay headlessly testable; Aurora-specific panels belong in `aurora-ui`. Keep that seam sharp.

The allowed dependency map lives in `scripts/layering.json` and is checked by `scripts/check_layering.py`. If the checker rejects a dependency, the fix is almost always to move shared code *down* the stack — editing the JSON is an architecture decision, not a build fix.

Decisions with lasting consequences are recorded in [docs/adr/](docs/adr/). Read those before revisiting the UI toolkit, document ceiling, precision floor, or PSD scope; each records what would justify reopening it.

### Invariants (PRD §7.3)

These are load-bearing — each one backs a headline requirement, so treat them as rules:

1. Nothing assumes a document fits in memory; all pixel access goes through the tile store. Ceiling is 300,000 × 300,000 px (matching Adobe PSB) — one layer at half-float RGBA is ~720 GB.
1b. No 8-bit intermediates. The pipeline is ≥16-bit float end to end: `f16` tile storage, `f32` compute. 8-bit appears only at import (promoted immediately) and export (quantized with dithering). An 8-bit buffer inside the graph is a bug — the banding is invisible in review and unrecoverable downstream.
2. Edits are non-destructive: adjustments/filters/smart objects are render-graph nodes, never baked pixels.
3. History stores reversible operations plus dirtied tiles, not snapshots.
4. The UI thread never blocks on rendering — rendering is async and progressive.
5. Brush input bypasses the general graph (a scratch layer), or the 10 ms budget is unreachable.
6. Every buffer carries its color space; untagged data is an error, not a default.
7. Plugins are untrusted — sandboxed, no raw pointers into document memory.
8. UI and canvas share one GPU device and one frame — not separate surfaces composited together.
9. Every widget carries an `accesskit` node as part of its definition. Aurora renders its own UI, so nothing is accessible for free; a widget without one is incomplete.
10. No style value is hardcoded. Widgets resolve colors, spacing, sizes, radii, and durations from semantic design tokens in `aurora-theme` (FR-027) — never a literal, never by reading a *theme*. CI lints for this. A hardcoded color is a bug: it's the one thing a user's theme cannot override.

Note: `aurora-core` and `aurora` are taken on crates.io by unrelated projects (PRD §12 Q2b). Harmless — the workspace uses path dependencies and nothing needs publishing — but these crates cannot be published under those names as-is.

Licensed MIT. Two practical consequences when adding dependencies: `cargo deny` enforces an allowed-licence list in CI, and copyleft C libraries (LibRaw is LGPL-2.1/CDDL, `libheif` is LGPL) must be dynamically linked to satisfy LGPL — prefer a pure-Rust alternative where one is viable. See PRD §14.

## Technology stack (PRD §8)

Rust end to end (edition 2024, stable). `wgpu` + WGSL for GPU across Vulkan/Metal/DX12; `winit` for windowing and tablet input; `rayon` for CPU tile parallelism; `tokio` for I/O and background work but **not** the render loop. FFI wrappers are acceptable where Rust lacks maturity (RAW, ICC).

Two deliberate changes from the original C++ plan: plugins are **WASM via `wasmtime`** (native dylibs can't meet the sandbox requirement), and scripting is **Lua in-process + Python out-of-process over IPC**, with the JavaScript API deferred.

**UI: Aurora builds its own retained-mode widget toolkit on `wgpu`** (PRD §8.3) — no third-party toolkit. Supporting crates: `cosmic-text` (shared by UI fields and canvas text), `winit` (input + platform IME), `accesskit` (accessibility), `rfd` (native dialogs), `aurora-vector` (resolution-independent UI geometry).

The consequence to keep in mind when writing UI code: text editing, IME, accessibility, DPI scaling, native menus, drag & drop, and clipboard are **our** work, not inherited. They are Phase 1 scope and gate the phase — don't defer them as polish.

**Visual design and theming are a Must requirement** (PRD FR-027), not polish. Themes are declarative TOML files with semantic tokens, hot-reloaded, inheriting from built-ins — users restyle Aurora without touching code, so themes are data and never executable. Built-ins: Dark (default), Light, two high-contrast, and a neutral Color-Critical theme for color-accurate work. Density (Compact/Comfortable/Spacious), UI scale, accent, and icon set are independent axes.

When adding a widget, it isn't done until it: resolves all styling from tokens, exposes an accessibility node, appears in the component gallery in every state, and passes the contrast check in every built-in theme.

Design owner is Cahya Wirawan (PRD FR-027 *Ownership*) — token vocabulary, scales, and colour decisions are theirs. Don't invent tokens ad hoc when implementing a widget; if one is missing, that's a design decision to raise, not a gap to fill locally. If accessibility or IME proves unworkable in practice, the contained fallback is CXX-Qt for chrome only, which is why `aurora-ui`'s widget API stays free of `wgpu`-specific assumptions.

## Performance budgets that constrain design

From PRD §6 and §10 — these drive implementation choices rather than being measured afterward:

- Startup < 3 s; brush latency < 10 ms; 60 FPS canvas interaction.
- Open a 2 GB PSD in under 5 s (implies lazy/streaming parsing, not a full load).
- Unlimited layers and history — storage must be incremental and compressed.

Note these budgets are set at 8 bytes/px (half-float RGBA), which is 2× an 8-bit pipeline. Tile compression is mandatory, not an optimization.

### Measured, not assumed (spike/FINDINGS.md, 2026-07-26)

One GPU (Radeon Pro 5300M, Metal), so treat as indicative rather than settled — but these are real numbers and they changed the design:

- **Stroke latency p99 9.1 ms against a 10 ms budget.** Under 1 ms of margin. A latency regression test now exists in CI; do not assume the margin holds as the brush engine grows.
- **CPU compositing is the bottleneck, not disk I/O** — the opposite of what was assumed. Page-in panning runs at 7 ms; merging whole tiles costs ~20 ms. So: `aurora-tile` needs **per-tile dirty rectangles**, and compositing belongs on the **GPU** with the CPU path as fallback.
- **Upload bandwidth caps pan speed** (~18 MB per screenful). Render a lower mip while panning and refine when motion stops — this is what the progressive-rendering requirement is for.
- Invariants §7.3.1 and §7.3.8 hold; half-float round-trips bit-exact.

### The live app, measured end to end (0.39.0, PLAN.md M1.10)

The numbers above come from the throwaway slice. The real app has since been
measured on its own path — brush → composite → tile upload → render pass →
present — at the full 300,000 × 300,000 px ceiling, and **it misses the 60 FPS
budget**:

| Path | Budget | p50 | p99 |
|---|---|---|---|
| Pan while painting, GPU composite | 16.7 ms | 35.4 ms | **98.8 ms** (~5.9×) |
| Same, CPU fallback (smaller viewport) | 16.7 ms | 22.6 ms | **54.1 ms** (~3.2×) |

**This table is the 0.39.0 figure and is now well behind the code.** The same
two benchmarks have since been re-measured repeatedly on a real RTX 3090. On an
**idle** machine the GPU path's mean frame is ~7.5–8.6 ms with a worst-of-40 in
the 21–30 ms range. **Do not read that as the whole picture: every one of those
numbers is measured with the benchmark's caller thread otherwise idle, which
flatters it.** Re-measured with 8 competing CPU-bound threads — ordinary
desktop multitasking, not an adversarial setup — the same GPU-path whole-frame
mean roughly doubles, to ~15–18 ms, i.e. at or over the 16.7 ms budget; and
0.96.0/0.96.1's parallel tile-upload serializer pushed it to ~34–37 ms, which
is why 0.96.2 routed the frame path back to the sequential serializer. Quote
the contended figure alongside the idle one or the claim is misleading.
PLAN.md's **0.96.2** entry is still the current real-hardware record **for the
two standing gate fixtures** (its 0.96.1 entry has the full contended tables).
0.99.0 and 0.100.0/0.100.1 re-measured on the same hardware since and did *not*
displace it, deliberately: 0.99.0 re-ran those same two fixtures and found no
detectable change (overlapping ranges), and 0.100.0 measured a **different,
new** fixture (two overlapping roots, a 256-tile store, ~40 ms/frame) whose
absolute numbers are not comparable to the gate rows at all — read its 0.100.1
correction before quoting any of it. Its 0.94.1 entry explains why a "p99"
from these
n=40 benchmarks is a single-sample order statistic that must not be quoted as
a ratio; read those, not this row, for where the numbers stand. The gate is
still missed either way, which is why this section stays — and `recomposite`
is now ~73% of the GPU-path frame mean, so it is the only stage where the
remaining gap can plausibly be closed.

This is an honest, open finding, not a benchmark claim — report it that way. The
tests assert a deliberately loose CI-safety threshold (350 ms / 180 ms) because
they exist to produce a true number, not to pass at 60 FPS; do not read a green
run as the budget being met. Later work reduced *how much* gets recomposited per
edit, not the per-tile cost, and none of it has been re-measured on real GPU
hardware. As of 0.73.4, brush/eraser dabs, pixel-stroke undo/redo, and
same-origin active-layer switches all invalidate the composite cache narrowly.
**Structural undo/redo (visibility, opacity, blend mode, and similar toggles)
does not** — 0.73.0 narrowed it too, but review found the narrowing unsound (a
layer's declared `bounds` is a position hint, not an enforced clip on its real
painted content, so the reported rect could miss real pixels), and 0.73.1
reverted it to a full bump. A live Move still bumps the whole thing either way,
because the composite tile grid is anchored to the *active layer's* own origin,
so dragging that layer re-anchors every tile at once. Re-anchoring the grid to
the document instead is the named follow-on (PLAN.md, Incremental compositing)
that would also be the prerequisite for structural narrowing to become sound.
**None of that is progress against the 60 FPS gate** — it buys Ctrl+Z-after-a-
stroke and Layers-panel click latency, on a different path from the one
measured above.

**PSD/PSB is full layered read *and* write** (PRD FR-001) — Aurora round-trips, so a file edited here must reopen in Photoshop with layers intact. Two rules follow: never overwrite a user's file in place (write to temp, verify by reopening, then swap), and warn with an itemized list before any lossy save. Silently degrading a professional's file is the worst failure this project can have.

## Phasing (PRD §9)

**Phase 0 (de-risking)** satisfied its Phase 1 gate (PRD §13 Steps 1, 3, 4) on 2026-07-28: `wgpu` validation, the tile-paging prototype, the screen-reader and CJK-IME spikes (the §8.3 escape-hatch triggers), widget toolkit foundations, the design language and token system, RAW/ICC library decisions, PSD feasibility, and the workspace + CI skeleton. Its tail is still open — see "What is not done" above.

**Phase 1 (document, canvas, layers, rendering, shell) is where the work is.** M1.1 and M1.6 are fully done; M1.2 through M1.9 are mostly done with named open items in their own PLAN.md sections; M1.10 is the gate.

Calendar durations were dropped on 2026-07-28 (PRD §13 Step 7): this is solo development, so phases are milestone-based and not date-committed. Don't reintroduce month estimates.

Each phase has a measurable exit criterion in §9 — prefer working toward the current gate over stubbing later-phase subsystems. Open questions that block design are tracked in PRD §12; risks in §11.
