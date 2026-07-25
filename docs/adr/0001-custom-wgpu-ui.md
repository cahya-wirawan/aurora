# 0001. Custom UI toolkit on wgpu

**Status:** Accepted
**Date:** 2026-07-25
**Related:** PRD §8.3, FR-024, FR-027, invariants §7.3.8–7.3.10

## Context

Aurora needs a professional desktop UI: dockable panels, custom workspaces, a contextual toolbar, command palette, and a canvas that must sustain 60 FPS with sub-10 ms brush latency (PRD §6). It must run on Windows, macOS, and Linux, support screen readers and CJK IME, and be fully themeable (FR-027).

Rust has no equivalent of Qt 6's maturity for this class of application. Every option involves accepting a significant deficiency, so the decision is which deficiency is most survivable.

The canvas constrains the choice more than the widgets do. With any third-party toolkit, Aurora's GPU canvas is a foreign surface embedded in that toolkit's frame — requiring interop, and putting the toolkit's event loop and frame pacing directly inside the latency budget.

## Decision

Aurora builds its own retained-mode widget toolkit rendered through `wgpu`, sharing one GPU device and one frame with the canvas. No third-party UI toolkit.

Supporting libraries: `winit` (windowing, input, platform IME), `cosmic-text` (shaping and text editing, shared with canvas text), `accesskit` (accessibility), `rfd` (native dialogs), `aurora-vector` (resolution-independent geometry).

The toolkit lives in `aurora-widgets`, which knows nothing about documents or layers. Aurora-specific panels live in `aurora-ui`.

## Alternatives considered

**egui** — immediate-mode, trivial `wgpu` integration, fastest path to a working UI. Rejected: weak accessibility, non-native text input and IME, and no real docking. Immediate-mode also redraws the full UI every frame, spending budget the canvas needs.

**Iced** — retained-mode with a real widget model, already `wgpu`-based. Rejected: docking and the panel model are unsolved, and the ecosystem is young enough that we would be building the missing half anyway, without control over the other half.

**Qt 6 via CXX-Qt** — mature widgets, docking, accessibility, and IME all solved; genuinely the lowest-risk option for everything except the canvas. Rejected: reintroduces C++ and a two-language build to a project whose language choice was deliberate (ADR pending, PRD §8.1), imposes Qt's event loop on the latency path, and makes the canvas a foreign surface. Retained as the escape hatch below.

**Blender/Figma precedent** — both built custom UI over a GPU surface for the same reasons. A professional creative tool's UI is mostly custom anyway: canvas, docking, timeline, curve editors, colour wheels, layer trees. Stock widgets contribute little.

## Consequences

**Gained:** one device and one frame for UI and canvas, so no interop layer, no texture copies, no compositing seam. Full control of the input→present path, which is what makes the 10 ms budget achievable. Identical appearance and behaviour on all three platforms. Themeability (FR-027) becomes natural rather than a fight with a toolkit's styling system.

**Cost:** the largest engineering commitment in the project. Aurora must build and own text input and editing, IME composition, accessibility, DPI and multi-monitor scaling, native menus, drag & drop, clipboard, file dialogs, and text selection. **These are Phase 1 scope, not polish** — the standard failure mode for custom-UI applications is deferring them. Phase 1 was extended from 6 to 9 months to hold this work explicitly rather than absorbing it silently.

**Follow-on work:** `accesskit` integration from the first widget (invariant §7.3.9); design tokens before any widget is written (invariant §7.3.10, ADR pending); component gallery as the review and golden-image surface; Phase 0 spikes proving screen readers and CJK IME on all three platforms.

## Reconsider if…

- A Phase 0 spike shows `accesskit` cannot drive a real screen reader acceptably on any target platform
- CJK IME composition through `winit` + `cosmic-text` proves unreliable in practice
- The Phase 1 accessibility or IME audit fails and the gap looks structural rather than incomplete

The contained fallback is to keep the custom canvas and host it inside Qt 6 via CXX-Qt for chrome only. This is why `aurora-ui`'s widget API stays free of `wgpu`-specific assumptions — the renderer must be swappable without rewriting panel logic.
