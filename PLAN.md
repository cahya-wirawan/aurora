# Aurora — Implementation Plan

**Living document.** Tracks what is done, what is in progress, and what comes next.
Last updated: **2026-08-01**.

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
Windows. The remaining Phase 0 items (ADR 0006, Windows/300k-px slice
re-runs, macOS/Windows LGPL packaging, deeper PSD format coverage)
continue in the background rather than gating Phase 1 — none of them are
among the three steps PRD §13 actually names as blocking.

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
| **Phase 1 — M1.1** | **Complete, 2026-07-28.** `aurora-core` (geometry, colour descriptors, IDs, errors, 16 tests) and `aurora-tile` (sparse/LRU/compressed/paged tile store, 12 tests, ADR 0005). Full local CI gate clean. See M1.1 |
| **Phase 1 — M1.2** | **In progress, 2026-07-29.** Device/queue management, shader library/pipeline cache, GPU tile residency, and budgeted upload scheduling (`GpuContext`/`ShaderLibrary`/`PipelineCache`/`TileResidency`) all done and verified against this machine's real RTX 3090 (Vulkan) with actual rendered/uploaded-pixel checks. Surface configuration/resize (`GpuSurface`) is implemented **and now verified against a real window** on a different machine's live macOS/Metal session — same "GDM greeter only" gap as the a11y Orca leg, resolved the same way (a machine with an actual desktop session). A real cross-test GPU deadlock under `cargo test`'s default runner was found and fixed along the way (test-only `Mutex`). `TileResidency`'s atlas gained a real 4-level mip chain and `upload_mip` 2026-07-30, in service of M1.3's progressive rendering (see below) — the atlas itself is still M1.2 scope even though the reason for the growth is M1.3's. Only cross-platform validation (DX12 — Vulkan and Metal are both real now) is fully unstarted. See M1.2 |
| **Phase 1 — M1.3** | **In progress, 2026-07-30.** `aurora-graph`'s node definitions, dependency DAG, and dirty-region propagation (`RenderGraph<N>`) done, 12 tests. `aurora-render`: `schedule()` translates a graph's node-granular dirty `Rect`s into tile-granular work lists (9 tests); `TileCompositor` blends one tile over another on the GPU via the fixed-function alpha blend unit (3 tests, verified against real hardware); progressive rendering's `mip::downsample` and `preview::upload_preview` land a downsampled tile in `aurora_gpu::TileResidency`'s atlas, verified end-to-end against real hardware (9 tests); `Executor` runs submitted work on a background thread without blocking the caller, async evaluation's first piece (5 tests). What's left is real consumers for the last two: picking a mip level from interaction state, and submitting actual render work through `Executor` — both wait on `aurora-doc`/`aurora-filters`, which don't exist yet. See M1.3 |
| **Phase 1 — M1.4** | **In progress, 2026-08-01.** `aurora-doc`'s `LayerTree` (`Pixel`/`Group` layers, nesting, top-to-bottom ordering, cascading delete, cycle-checked reparenting) done, 2026-07-30, 25 tests. Per-layer opacity/fill opacity/blend mode (full 27-mode Photoshop set)/visibility/locking (`BlendMode`, `LayerLock` mirroring PSD's `lspf` bits) done 2026-08-01, 7 more tests (32 total). Per-layer masks (`LayerMask` — bounds/enabled/inverted, deliberately no real mask pixels yet) done 2026-08-01, 8 more tests (40 total) — lives on `LayerEntry` so both pixel layers and groups can carry one. fmt/clippy (`-D warnings`)/`cargo test -p aurora-doc` all verified clean once a Rust toolchain was installed partway through this work, see M1.4. The concrete consumer M1.3's progressive-rendering/async-evaluation primitives were waiting for still doesn't exist yet (that's compositing/rendering wiring, not these bullets). Selections and history remain. See M1.4 |

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
