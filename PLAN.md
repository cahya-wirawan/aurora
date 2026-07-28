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
and none should yet. CI green on Linux, macOS, and Windows.

| Area | State |
|---|---|
| Requirements & architecture | Settled and written down (PRD v1.6, 4 ADRs) |
| Workspace & CI | Built and green |
| Performance validation | **Measured** — budgets hold, with one correction; re-run on Linux/Vulkan 2026-07-26, same correction reproduces |
| Accessibility & IME | **macOS verified (9/10)** — Linux build/tree confirmed, human/Orca leg still outstanding; Windows outstanding |
| Design language | **Owner-approved draft** — [design/](design/README.md); outside critique still needed before it hardens |
| PSD write feasibility | Pixel layers/groups **tractable**; text layers **harder than planned** — new mandatory scope found (glyph rendering) |
| RAW / ICC feasibility | **RAW decodes on all 3 vendors; ICC cross-validated; LGPL packaging mechanism proven on Linux** — RAW licensing worse than assumed, but now has a working answer, not just a named risk |

**The single most important open item, updated:** on macOS, a screen reader
does speak a custom-drawn text field, and CJK composition works — human-verified
2026-07-25/26, [full results](spike/a11y-ime/FINDINGS.md). Nothing found rises to
[ADR 0001](docs/adr/0001-custom-wgpu-ui.md)'s structural escape-hatch trigger. What
remains open is **Windows (UIA) and Linux (AT-SPI)** — different APIs entirely,
and macOS passing says nothing about them — plus one real but non-structural bug
(live value-change announcements don't reach VoiceOver).

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
- [x] Run on Linux — Vulkan/NVIDIA RTX 3090, 6 runs, 2026-07-26 — [spike/FINDINGS.md](spike/FINDINGS.md#second-platform-linux--vulkan-2026-07-26). Stroke latency and idle-frame budgets pass with more headroom than macOS; pan-while-painting is marginal (straddles the 16.7 ms budget across runs on this shared machine) rather than a clean pass — same architectural bottleneck as macOS (finding 1), not a new one
- [ ] Re-run at the 300,000 px ceiling *(only 100,000 px tested)*

### 0.4 Accessibility and IME spike — **macOS verified (9/10); Windows/Linux outstanding**

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
- [!] **Narrator announces the field (Windows, UIA — a different API; macOS success does not carry over)** — not run, no Windows machine tested yet
- [!] **Orca announces the field (Linux, AT-SPI)** — build clean on Linux 1.97.1, `accesskit`'s AT-SPI backend (`accesskit_atspi_common`/`accesskit_unix`) compiles in, `--dump-tree` shows correct role/label/value/composition-state with no window needed — see [FINDINGS.md](spike/a11y-ime/FINDINGS.md#linux--build-and-tree-construction-confirmed-orca-leg-still-blocked). Still blocked: the decisive human-plus-Orca test needs a live logged-in desktop session, which this machine did not have (GDM greeter only, no user session) — not yet run
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

### 0.5 Design language — **owner-approved draft; outside critique still open**

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
- [ ] Outside critique on the mockups before they harden (risk R2f mitigation) — owner sign-off is not a second opinion; this is the one remaining item before 0.5 counts as fully complete

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
- [ ] Decide RAW and ICC libraries, record as ADRs — evidence is now in place (FINDINGS.md); the RAW decision specifically needs Cahya's sign-off given the now-confirmed-unavoidable LGPL packaging architecture question, not just a spike recommendation
- [x→bigger] **LGPL packaging architecture — proven mechanically on Linux, not just designed.** [spike/lgpl-packaging/](spike/lgpl-packaging/FINDINGS.md): a `cdylib` (`raw-shim`, the only crate touching `rawler`) behind a hand-written `extern "C"` ABI, loaded at run time via `dlopen` (`libloading`, through a separate `host` binary with **zero** `rawler`/LGPL dependency — confirmed with `ldd`/`nm`, not assumed). Both conditions of LGPL-2.1 §6(b) (quoted directly from the license text, not a summary) verified concretely: (1) genuinely dynamic, no build-time link, checked with `file`/`ldd`/`nm`; (2) a modified `raw-shim` (`.so` rebuilt with one line changed) worked correctly with the **same, unmodified, un-recompiled** `host` binary — the actual property the license requires, demonstrated rather than argued. All three real RAW files from the raw-icc spike decoded identically through this mechanism as decoding them directly. **Scope, deliberate: Linux-only, no ABI-versioning story, no macOS/Windows, and this is engineering verification, not legal sign-off** — one specific ambiguity flagged honestly rather than glossed over (§6(b)(1)'s "already present on the user's system" language reads more naturally as a pre-existing system package than an app-bundled file; industry practice treats bundling as sufficient, but that's a judgment call, not a legal conclusion).

### 0.7 Test corpora — PSD not started; RAW/ICC have a first, minimal set

Assemble *before* the parsers exist, so the parser is written against reality.

- [ ] 1,000 real-world PSDs (Phase 3 gate)
- [~] **RAW samples per camera vendor** — one real file each for Canon, Nikon, Sony (`spike/raw-icc/reference/`), enough to answer "does this decode major vendors at all," far short of the eventual multi-body/multi-vendor breadth this item is really asking for
- [~] **ICC profile set** — two real, CC0-licensed profiles (`sRGB.icc`, `ECI-RGBv2.icc`), same "first, not final" caveat
- [x] **Fetch scripts (corpora are gitignored — too large to commit)** — `spike/raw-icc/reference/fetch-samples.sh`, same pattern to reuse for the eventual 1,000-PSD corpus

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

## Next three actions

1. **Run the human/Orca leg of the a11y/IME checklist on Linux, and the whole
   checklist on Windows** — macOS is done (9/10,
   [full results](spike/a11y-ime/FINDINGS.md)). Linux build and standalone
   tree construction are now confirmed (2026-07-26), but the decisive test —
   a human at a live desktop session with Orca actually speaking — still
   needs a machine with an active graphical login, which was not available
   this pass. Windows (UIA) is fully unstarted. Both are different platform
   APIs entirely and remain the only thing that can still overturn ADR 0001.
2. **Get outside critique on the 0.5 design scaffold** ([design/](design/README.md),
   [b0a7ac8](https://github.com/cahya-wirawan/aurora/commit/b0a7ac8)) — all
   five Phase 0 deliverables are drafted and now owner-approved (2026-07-27).
   The one item left before 0.5 counts as complete is a second opinion
   (R2f mitigation: a solo design owner has no built-in check) before the
   token vocabulary and Dark theme harden into something every widget
   depends on.
3. **Decide RAW and ICC libraries, record as ADRs** — the evidence is now
   fully in place across two spikes: `spike/raw-icc/FINDINGS.md` (`rawler`
   decodes real Canon/Nikon/Sony files correctly but is LGPL-2.1, no
   permissive full-featured alternative exists; `lcms2` is the
   licensing-cleaner ICC choice, `moxcms` a real pure-Rust alternative) and
   `spike/lgpl-packaging/FINDINGS.md` (the LGPL packaging mechanism itself,
   proven mechanically on Linux — a `cdylib` + hand-written C ABI + `dlopen`
   satisfies both conditions of LGPL-2.1 §6(b), demonstrated concretely
   rather than argued). What's left before the ADRs can be written: extend
   the packaging prototype to macOS/Windows, and get Cahya's sign-off — not
   just on which crate name to pick, but on the packaging architecture and
   the one flagged legal ambiguity (§6(b)(1)'s "already present on the
   user's system" wording), since every viable full-featured RAW option is
   LGPL and this cost doesn't go away by picking a different library.

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
- **A dynamically-loadable-component packaging architecture for LGPL
  dependencies** (raw-icc spike finding 2) — **now prototyped and verified
  mechanically on Linux** ([spike/lgpl-packaging/](spike/lgpl-packaging/FINDINGS.md)):
  a `cdylib` behind a hand-written C ABI, loaded via `dlopen`, both LGPL-2.1
  §6(b) conditions demonstrated concretely (not just argued). Still open:
  macOS/`.dylib` and Windows/`.dll` equivalents (same principle, different
  OS-level mechanics — `install_name`/`RPATH`/DLL search order — all
  unverified); an ABI-versioning story for real interface-compatibility
  across independently-built versions, not just this spike's one-line
  same-compiler edit; and real legal review, specifically on whether
  bundling the `.so` in Aurora's own installer satisfies §6(b)(1)'s "already
  present on the user's system" language as cleanly as engineering judgment
  suggests it does.
- Font resolution (`ResourceDict.FontSet`) and `FillColor` alpha compositing
  remain the two small, non-urgent named gaps in the PSD text-layer line of
  work (findings 14/15) — pick up whenever, not blocking anything.
