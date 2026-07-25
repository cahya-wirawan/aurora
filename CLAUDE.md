# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

This repository contains **no source code yet** — only [PRD.md](PRD.md), a full product requirements document. There is no build system, no test suite, no git repository, and no dependency manifest. The first implementation work will need to establish all of these.

When adding the first code, update this file with the actual build/test/run commands.

## What Aurora is

A cross-platform, GPU-accelerated, non-destructive professional image editor (a Photoshop alternative) for Windows, macOS, and Linux. PSD/PSB compatibility and AI-assisted editing are first-class requirements, not add-ons.

## Planned architecture (PRD §7)

A single Cargo workspace with crates layered so dependencies point **downward only** — `core` → `tile` → `graph`/`gpu` → `render`/`doc` → feature crates (`filters`, `brush`, `vector`, `text`, `io`, `ai`, `plugin`, `theme`) → `widgets` → `ui` → `app`/`cli`. PRD §7.2 has the full table. A lower crate must never depend on a higher one; CI enforces this.

`aurora-widgets` (the general-purpose toolkit) knows nothing about documents or layers and must stay headlessly testable; Aurora-specific panels belong in `aurora-ui`. Keep that seam sharp.

### Invariants (PRD §7.3)

These are load-bearing — each one backs a headline requirement, so treat them as rules:

1. Nothing assumes a document fits in memory; all pixel access goes through the tile store (500,000² px ≈ 1 PB).
2. Edits are non-destructive: adjustments/filters/smart objects are render-graph nodes, never baked pixels.
3. History stores reversible operations plus dirtied tiles, not snapshots.
4. The UI thread never blocks on rendering — rendering is async and progressive.
5. Brush input bypasses the general graph (a scratch layer), or the 10 ms budget is unreachable.
6. Every buffer carries its color space; untagged data is an error, not a default.
7. Plugins are untrusted — sandboxed, no raw pointers into document memory.
8. UI and canvas share one GPU device and one frame — not separate surfaces composited together.
9. Every widget carries an `accesskit` node as part of its definition. Aurora renders its own UI, so nothing is accessible for free; a widget without one is incomplete.
10. No style value is hardcoded. Widgets resolve colors, spacing, sizes, radii, and durations from semantic design tokens in `aurora-theme` (FR-027) — never a literal, never by reading a *theme*. CI lints for this. A hardcoded color is a bug: it's the one thing a user's theme cannot override.

## Technology stack (PRD §8)

Rust end to end (edition 2024, stable). `wgpu` + WGSL for GPU across Vulkan/Metal/DX12; `winit` for windowing and tablet input; `rayon` for CPU tile parallelism; `tokio` for I/O and background work but **not** the render loop. FFI wrappers are acceptable where Rust lacks maturity (RAW, ICC).

Two deliberate changes from the original C++ plan: plugins are **WASM via `wasmtime`** (native dylibs can't meet the sandbox requirement), and scripting is **Lua in-process + Python out-of-process over IPC**, with the JavaScript API deferred.

**UI: Aurora builds its own retained-mode widget toolkit on `wgpu`** (PRD §8.3) — no third-party toolkit. Supporting crates: `cosmic-text` (shared by UI fields and canvas text), `winit` (input + platform IME), `accesskit` (accessibility), `rfd` (native dialogs), `aurora-vector` (resolution-independent UI geometry).

The consequence to keep in mind when writing UI code: text editing, IME, accessibility, DPI scaling, native menus, drag & drop, and clipboard are **our** work, not inherited. They are Phase 1 scope and gate the phase — don't defer them as polish.

**Visual design and theming are a Must requirement** (PRD FR-027), not polish. Themes are declarative TOML files with semantic tokens, hot-reloaded, inheriting from built-ins — users restyle Aurora without touching code, so themes are data and never executable. Built-ins: Dark (default), Light, two high-contrast, and a neutral Color-Critical theme for color-accurate work. Density (Compact/Comfortable/Spacious), UI scale, accent, and icon set are independent axes.

When adding a widget, it isn't done until it: resolves all styling from tokens, exposes an accessibility node, appears in the component gallery in every state, and passes the contrast check in every built-in theme. If accessibility or IME proves unworkable in practice, the contained fallback is CXX-Qt for chrome only, which is why `aurora-ui`'s widget API stays free of `wgpu`-specific assumptions.

## Performance budgets that constrain design

From PRD §6 and §10 — these drive implementation choices rather than being measured afterward:

- Startup < 3 s; brush latency < 10 ms; 60 FPS canvas interaction.
- Open a 2 GB PSD in under 5 s (implies lazy/streaming parsing, not a full load).
- Unlimited layers and history — storage must be incremental and compressed.

## Phasing (PRD §9)

**Phase 0 (de-risking) comes first** and is not yet done: `wgpu` validation on all three platforms, tile-paging prototype, screen-reader and CJK-IME spikes (the §8.3 escape-hatch triggers), widget toolkit foundations, the design language and token system (which must exist *before* widgets — tokens can't be retrofitted cheaply), RAW/ICC library decisions, PSD feasibility, and the workspace + CI skeleton. PRD §13 lists the ordered pre-implementation steps; Phase 1 feature work should not start before steps 1, 3, and 4 there are complete.

Phase 1 is 9 months, not 6 — the widget toolkit is roughly a third of it.

Each phase has a measurable exit criterion in §9 — prefer working toward the current gate over stubbing later-phase subsystems. Open questions that block design are tracked in PRD §12; risks in §11.
