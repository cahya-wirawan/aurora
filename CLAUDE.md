# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

**Skeleton plus one measured spike.** The Cargo workspace, all 19 crates, CI, and the ADRs exist; **no functionality is implemented** — each library crate holds a placeholder `crate_name()` and one test so CI has something real to check.

The vertical slice (PRD §13 Step 4) is built and measured: `spike/vertical-slice/`, results in [spike/FINDINGS.md](spike/FINDINGS.md). Read that before writing tile, render, or brush code — it corrected several assumptions, and its numbers are the only real performance data the project has.

A second spike covers accessibility and IME (`spike/a11y-ime/`, [FINDINGS](spike/a11y-ime/FINDINGS.md)). It is **partial**: the tree builds and the platform adapter initializes, but nobody has yet confirmed a screen reader speaks the field or that CJK composition works — those need a human, and until they are done ADR 0001 is not de-risked.

Two constraints it already surfaced: **windows must be created hidden, adapted, then shown** (`accesskit_winit` panics otherwise — this shapes `aurora-app`'s window management), and the text stack sets the toolchain floor (`cosmic-text` needs ≥1.89, which is why the pin moved to 1.97).

Remaining Phase 0 work: finishing the human half of the a11y/IME verification, the design language and token system, running both spikes on Windows and Linux, and the RAW/ICC and PSD-write feasibility spikes.

**[PLAN.md](PLAN.md) is the progress tracker** — task-level status for Phase 0 and a full Phase 1 breakdown. Check it before starting work to see what is done, blocked, or next, and update the relevant checkbox in the same commit as the work. It also lists findings carried forward from the spikes and where each one lands.

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

cargo run -p aurora-app                 # the application
cargo run -p aurora-cli                 # headless binary
```

The spike is a separate crate, deliberately outside the workspace (root `Cargo.toml` `exclude`s it) so it can never become a dependency of real code:

```sh
cd spike/vertical-slice
cargo run --release -- --headless       # the benchmark; no display needed
cargo run --release                     # windowed, drag to paint, Esc for stats
```

The full CI gate locally, in the order CI runs it:

```sh
cargo fmt --all --check && python3 scripts/check_layering.py \
  && python3 scripts/check_no_hardcoded_style.py \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace
```

Toolchain is pinned in `rust-toolchain.toml` (1.97, edition 2024 — `cosmic-text` requires ≥1.89, so the text stack sets the floor). CI runs on Linux, macOS, and Windows from the first commit — cross-platform breakage is cheap to fix now and catastrophic in month 30.

## Lints worth knowing

The workspace denies `unwrap`, `expect`, `panic`, and `indexing_slicing` (root `Cargo.toml`). This is deliberate: Aurora holds a professional's unsaved work, and a panic loses it. Return errors instead. A crate needing `unsafe` (likely `aurora-gpu` for FFI) must override `unsafe_code` in its own `[lints]` table rather than the workspace's, so the exception is visible in review.

## Versioning

SemVer, starting at `0.0.1`. The single source of truth is `[workspace.package].version` in the root `Cargo.toml`; every crate inherits it via `version.workspace = true` — bump it in exactly one place.

- **Minor** (`0.X.0`): every PLAN.md step — a task-level unit of work landing in its own commit (the same granularity PLAN.md's own checkboxes track).
- **Patch** (`0.0.X`): a bug fix — correcting something that was already landed and wrong, not new work.
- Bump the version in the same commit as the work it covers, the same discipline PLAN.md's own checkbox updates already follow.

A release is a `vX.Y.Z` tag (e.g. `v0.1.0`) pushed once the matching version bump is committed — pushing that tag is what actually publishes a GitHub Release, so treat it as a deliberate, user-approved action, not a routine one. `.github/workflows/release.yml` re-runs the full gate (`verify.yml`, the same jobs `ci.yml` runs on every push/PR) against the tagged commit, confirms the tag matches `Cargo.toml`'s own version, then creates the Release. Ordinary commits and PRs are still checked immediately via `ci.yml` regardless of tags — CLAUDE.md's own "cross-platform breakage is cheap to fix now" principle applies to every commit, not just tagged ones.

## What Aurora is

A cross-platform, GPU-accelerated, non-destructive professional image editor (a Photoshop alternative) for Windows, macOS, and Linux. PSD/PSB compatibility and AI-assisted editing are first-class requirements, not add-ons.

## Planned architecture (PRD §7)

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

- **Stroke latency p99 9.1 ms against a 10 ms budget.** Under 1 ms of margin. Add a latency regression test in CI with the first Phase 1 commit; do not assume this holds as the brush engine grows.
- **CPU compositing is the bottleneck, not disk I/O** — the opposite of what was assumed. Page-in panning runs at 7 ms; merging whole tiles costs ~20 ms. So: `aurora-tile` needs **per-tile dirty rectangles**, and compositing belongs on the **GPU** with the CPU path as fallback.
- **Upload bandwidth caps pan speed** (~18 MB per screenful). Render a lower mip while panning and refine when motion stops — this is what the progressive-rendering requirement is for.
- Invariants §7.3.1 and §7.3.8 hold; half-float round-trips bit-exact.

**PSD/PSB is full layered read *and* write** (PRD FR-001) — Aurora round-trips, so a file edited here must reopen in Photoshop with layers intact. Two rules follow: never overwrite a user's file in place (write to temp, verify by reopening, then swap), and warn with an itemized list before any lossy save. Silently degrading a professional's file is the worst failure this project can have.

## Phasing (PRD §9)

**Phase 0 (de-risking) comes first** and is not yet done: `wgpu` validation on all three platforms, tile-paging prototype, screen-reader and CJK-IME spikes (the §8.3 escape-hatch triggers), widget toolkit foundations, the design language and token system (which must exist *before* widgets — tokens can't be retrofitted cheaply), RAW/ICC library decisions, PSD feasibility, and the workspace + CI skeleton. PRD §13 lists the ordered pre-implementation steps; Phase 1 feature work should not start before steps 1, 3, and 4 there are complete.

Phase 1 is 9 months (not 6 — the widget toolkit is roughly a third of it) and Phase 3 is 10 months (not 8 — full PSD write). Total ~52 months. These estimates predate the prototype; PRD §13 Step 7 calls for re-grounding them now that the slice exists.

Each phase has a measurable exit criterion in §9 — prefer working toward the current gate over stubbing later-phase subsystems. Open questions that block design are tracked in PRD §12; risks in §11.
