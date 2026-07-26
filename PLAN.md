# Aurora — Implementation Plan

**Living document.** Tracks what is done, what is in progress, and what comes next.
Last updated: **2026-07-26**.

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

**Phase 0 (technical de-risking) — roughly half done.** No product features exist
and none should yet. Eleven commits, CI green on Linux, macOS, and Windows.

| Area | State |
|---|---|
| Requirements & architecture | Settled and written down (PRD v1.6, 4 ADRs) |
| Workspace & CI | Built and green |
| Performance validation | **Measured** — budgets hold, with one correction |
| Accessibility & IME | **Partially verified — the decisive test is unrun** |
| Design language | Not started — blocked on design owner time |
| Format feasibility (RAW/ICC/PSD) | Not started |

**The single most important open item:** nobody has confirmed a screen reader
speaks a custom-drawn text field, or that CJK composition works. Until that is
done, [ADR 0001](docs/adr/0001-custom-wgpu-ui.md) (custom UI on `wgpu`) is not
de-risked, and it is the most expensive decision in the project to reverse.

---

## Phase 0 — Technical de-risking

**Goal:** answer every question whose wrong answer would be expensive after Phase 1
code exists. **Exit criterion** (PRD §9): a prototype paints on a huge tiled
document at 60 FPS with sub-10 ms latency on all three platforms, with
custom-rendered panels in the same frame, a screen reader reading a panel, and
CJK composing into a custom text field.

### 0.1 Decisions and documentation

- [x] PRD written and revised to v1.6 — [PRD.md](PRD.md)
- [x] ADR 0001 — custom UI toolkit on `wgpu` — [adr/0001](docs/adr/0001-custom-wgpu-ui.md)
- [x] ADR 0002 — 300,000 px document ceiling (PSB parity) — [adr/0002](docs/adr/0002-document-size-ceiling.md)
- [x] ADR 0003 — ≥16-bit float precision floor — [adr/0003](docs/adr/0003-float-precision-floor.md)
- [x] ADR 0004 — full layered PSD/PSB write — [adr/0004](docs/adr/0004-psd-full-write.md)
- [x] Licence chosen: MIT — [LICENSE](LICENSE), PRD §14
- [x] Design owner named: Cahya Wirawan — PRD FR-027 *Ownership*
- [x] Name/trademark investigated — PRD §12 Q2b (retained; not a legal clearance)
- [ ] ADR 0005 — tile size and scratch-disk budget *(needs 0.4 numbers)*
- [ ] ADR 0006 — accessibility conformance target (WCAG 2.1 AA / Section 508 / EN 301 549)

### 0.2 Workspace and CI

- [x] Cargo workspace, 19 crates, layered per PRD §7.2
- [x] Layering rule enforced mechanically — `scripts/check_layering.py`
- [x] Lints: `unwrap`/`expect`/`panic`/`indexing_slicing` denied workspace-wide
- [x] CI: fmt, clippy, layering, tests, rustdoc, `cargo-deny` on Linux/macOS/Windows
- [x] Toolchain pinned — 1.97 (raised from 1.88; `cosmic-text` needs ≥1.89)
- [ ] Brush-latency regression test in CI *(must exist before Phase 1 feature work — the budget has <1 ms margin)*
- [ ] Golden-image diff harness *(needed before the first filter)*

### 0.3 Vertical slice — **done**

Evidence: [spike/FINDINGS.md](spike/FINDINGS.md), `spike/vertical-slice/`

- [x] Window → `wgpu` → tiled half-float document → stroke → save/reload
- [x] UI and canvas in one device and one frame (invariant §7.3.8)
- [x] 80 GB document edited in a 64 MB budget, with real eviction and page-in (§7.3.1)
- [x] Half-float round-trip verified bit-exact (§7.3.6b)
- [x] Latency, frame, paging, and I/O measured
- [ ] Run on Windows *(DX12 backend unvalidated)*
- [ ] Run on Linux *(Vulkan backend unvalidated)*
- [ ] Re-run at the 300,000 px ceiling *(only 100,000 px tested)*

### 0.4 Accessibility and IME spike — **partial, and blocking**

Evidence: [spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)

- [x] `accesskit` tree construction — role, label, value, focus, composition state
- [x] Platform adapter initializes; window runs stably
- [x] `winit` IME plumbing wired (`set_ime_allowed`, `set_ime_cursor_area`)
- [x] Custom text field: insert, backspace (char-wise), cursor motion, preedit
- [!] **VoiceOver announces the field (macOS)** — needs a human to listen
- [!] **Narrator announces the field (Windows, UIA — a different API; macOS success does not carry over)**
- [!] **Orca announces the field (Linux, AT-SPI)**
- [!] **CJK composition commits correctly** (Pinyin, kana→kanji, jamo)
- [!] **IME candidate window appears at the field**, not the window corner
- [ ] Screen-reader-driven actions (set value, navigate by word/line)
- [ ] `TextSelection` exposed in the tree

> **If any of the blocked rows fails *structurally* — AccessKit cannot express it
> on that platform, rather than needing more code — that is ADR 0001's
> escape-hatch trigger. Reconsider CXX-Qt before the widget toolkit is written.**

### 0.5 Design language — not started

Owner: Cahya Wirawan. Blocks all widget code (invariant §7.3.10 — tokens cannot
be retrofitted cheaply). Runs in parallel with 0.3/0.6; needs no engine code.

- [ ] Token vocabulary — semantic names widgets resolve against *(highest value; this is the interface everything else is written to)*
- [ ] Type scale, spacing scale, radius, elevation, motion values
- [ ] One complete built-in theme (Dark), all pairs passing contrast
- [ ] Static mockups: main workspace + 2–3 panels
- [ ] Component gallery skeleton — review surface and golden-image target
- [ ] Outside critique on the mockups before they harden (risk R2f mitigation)

### 0.6 Format feasibility — not started

- [ ] PSD/PSB write spike — produce a file **Photoshop reopens**, not merely parse one (ADR 0004 commits to write; this is a 10-month phase resting on an assumption)
- [ ] RAW decode spike — `rawler` vs LibRaw FFI, one file per major vendor
- [ ] ICC transform spike — `lcms2` binding, and the LGPL linking question (PRD §14)
- [ ] Decide RAW and ICC libraries, record as ADRs

### 0.7 Test corpora — not started

Assemble *before* the parsers exist, so the parser is written against reality.

- [ ] 1,000 real-world PSDs (Phase 3 gate)
- [ ] RAW samples per camera vendor
- [ ] ICC profile set
- [ ] Fetch scripts (corpora are gitignored — too large to commit)

### 0.8 Re-plan — not started

- [ ] Re-ground the §9 phase durations against slice measurements (PRD §13 Step 7)
- [ ] Answer PRD §12 Q2 (team size) and Q3 (revenue model) — both shape scope

---

## Phase 1 — Document, canvas, layers, rendering, shell

**9 months.** Do not start feature work until 0.2 (CI), 0.3 (slice), 0.4
(a11y verdict), and 0.5 (tokens) are complete.

**Exit criterion:** create, edit, save, reopen, and export a multi-layer document
with blend modes and unlimited undo at 60 FPS — *and* pass an accessibility audit
and an IME audit on all three platforms — *and* the component gallery renders
every widget in every state across all built-in themes with contrast checks green.

### M1.1 — Core and tile store (`aurora-core`, `aurora-tile`)

- [ ] Geometry, colour types, pixel formats, IDs, error types
- [ ] Coordinate types sized for 300,000 px with defined overflow behaviour
- [ ] Sparse tile store, LRU residency, scratch-disk paging
- [ ] **Per-tile dirty rectangles** — the largest single win from the slice
- [ ] Tile compression (`zstd`/`lz4`) — the memory budget already assumes it
- [ ] Background writer so eviction does not block the frame
- [ ] Bench: paging throughput, eviction cost, compression ratio

### M1.2 — GPU layer (`aurora-gpu`)

- [ ] Device/queue management, surface configuration, resize
- [ ] Shader library and WGSL pipeline cache
- [ ] GPU tile residency with toroidal slot addressing *(from the slice — awkward to retrofit)*
- [ ] Upload scheduling with a per-frame budget
- [ ] Validate on DX12, Metal, Vulkan

### M1.3 — Render graph and renderer (`aurora-graph`, `aurora-render`)

- [ ] Node definitions, dependency graph, dirty propagation
- [ ] Tile-granular scheduling
- [ ] **GPU-side compositing** — CPU path is the fallback, not the default
- [ ] Progressive rendering: low mip while interacting, refine when still
- [ ] Async evaluation — the UI thread never blocks (§7.3.4)

### M1.4 — Document model (`aurora-doc`)

- [ ] Layer tree: pixel, group, nesting, ordering
- [ ] Opacity, fill opacity, blend modes, visibility, locking
- [ ] Layer masks
- [ ] Selection representation
- [ ] History as reversible operations + dirtied tiles (§7.3.3), unlimited undo/redo
- [ ] Crash recovery journal

### M1.5 — Colour (`aurora-color`)

- [ ] Colour space types; every buffer tagged (§7.3.6)
- [ ] ICC transforms via the 0.6 decision
- [ ] Working spaces, linear-light conversion
- [ ] Promote-on-import, dither-on-export

### M1.6 — Design system (`aurora-theme`)

- [ ] Token types and semantic vocabulary from 0.5
- [ ] TOML theme parsing, inheritance, hot reload
- [ ] Built-in themes: Dark, Light, 2× high contrast, Colour-Critical
- [ ] Automated WCAG contrast validation over the token set
- [ ] CI lint rejecting hardcoded style values (§7.3.10)

### M1.7 — Widget toolkit (`aurora-widgets`)

*Roughly a third of Phase 1. Document-agnostic and headlessly testable.*

- [ ] Layout engine (flexbox-style; `taffy` if it fits)
- [ ] Retained-mode tree with damage tracking
- [ ] Input routing, focus management, keyboard navigation
- [ ] `accesskit` node per widget — part of the definition, not a pass (§7.3.9)
- [ ] Text field: selection, caret, word motion, clipboard, undo
- [ ] IME composition rendering (platform underline styles)
- [ ] Widget set: button, checkbox, slider, number field, dropdown, scrollbar, tree, tab bar, menu, tooltip, colour picker, curve editor
- [ ] Vector-first rendering via `aurora-vector` (resolution-independent)
- [ ] Component gallery + golden-image tests per theme and density
- [ ] Headless mode for automated UI tests

### M1.8 — Application shell (`aurora-ui`, `aurora-app`)

- [ ] Window/event loop; **create hidden → attach a11y adapter → show** *(from the a11y spike)*
- [ ] Docking, panels, custom workspaces
- [ ] Canvas: infinite zoom, rotation, pan, rulers, guides, grid, snap
- [ ] Layers, history, tool-options panels
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

## Phases 2–5 — outline

Detailed when the preceding phase closes; planning further ahead than the
evidence supports is how the original 6-month Phase 1 estimate happened.

- **Phase 2 (8 mo)** — selections, brush engine, masks, filters, adjustments.
  *Gate: an illustrator completes a real piece without leaving Aurora.*
- **Phase 3 (10 mo)** — smart objects, Camera RAW, colour management, PSD/PSB
  read+write. *Gate: 1,000 PSDs round-trip through Photoshop with no layer loss.*
- **Phase 4 (10 mo)** — AI, plugin SDK (WASM), automation, cloud sync.
  *Gate: a third-party ships a sandboxed plugin from public docs alone.*
- **Phase 5 (12 mo)** — collaboration, animation, mobile, web.

Total ~52 months, **estimated before any code existed**. Due for revision (0.8).

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

---

## Next three actions

1. **Run the a11y/IME checklist on macOS** — 10 minutes, and it is the last thing
   that can overturn a foundational decision. [Checklist](spike/a11y-ime/FINDINGS.md).
2. **Start 0.5, the token vocabulary** — blocks every widget, needs no engine code,
   and can run in parallel with everything else.
3. **PSD write spike (0.6)** — a 10-month phase currently rests on an untested
   assumption.
