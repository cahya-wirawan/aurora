# Aurora

A modern, GPU-accelerated, non-destructive professional image editor for Windows, macOS, and Linux — written in Rust.

> **Status: an early but real editor, Phase 1 in progress.** You can already open a PNG/JPEG/TIFF (or Aurora's own round-tripping `.aur` format), paint and erase on real pixels with undo/redo, work across multiple layers and groups with opacity and all 27 PSD-compatible blend modes composited for real (a GPU fast path for the common case, a CPU path — including groups and every blend mode — for everything else), and save back out with every visible layer composited into the result, not just the active one — verified end to end on real macOS hardware, including a screen reader announcing the window. What isn't real yet: PSD/PSB (a feasibility spike only — not wired into the app), filters and adjustments, selection tools beyond a raw data model, mask pixels (bounds only today), smart objects, and RAW import. The 20 crates compile, CI is green on Linux/macOS/Windows, and 914 tests pass. Track progress in **[PLAN.md](PLAN.md)**.

---

## What Aurora aims to be

A professional image editor that covers the great majority of Photoshop workflows — photo editing, digital painting, graphic design, illustration, and print production — built on a modern foundation rather than three decades of accumulated one.

The goals that shape every technical decision:

- **Non-destructive by default.** Adjustments, filters, and smart objects are nodes in a render graph, never baked pixels.
- **Fast, and provably so.** Sub-10 ms brush latency, 60 FPS canvas interaction, under 3 s startup. These are budgets that constrain design, not benchmarks measured afterward.
- **Scales past memory.** Documents up to 300,000 × 300,000 px — matching Adobe's PSB ceiling — via a tiled architecture that pages to disk. Nothing in the codebase assumes an image fits in memory.
- **High precision throughout.** The pipeline is 16-bit float minimum, end to end. No 8-bit intermediates, so HDR and scene-referred values survive the graph intact.
- **Real PSD interoperability.** Full layered read *and* write — edit a Photoshop file in Aurora and reopen it in Photoshop with its layers intact.
- **Local-first.** Your files are yours, on your disk, in an open format. Cloud is optional.
- **Beautiful and yours to restyle.** A custom UI with a semantic design-token system and hand-editable theme files — no code required to reskin it.
- **Open plugin ecosystem.** Sandboxed WASM plugins, so extensions can't compromise the host.

## Technology

| | |
|---|---|
| **Language** | Rust (edition 2024, stable) |
| **GPU** | `wgpu` + WGSL — Vulkan / Metal / DirectX 12 from one backend |
| **UI** | Custom retained-mode toolkit on `wgpu`, sharing one device and frame with the canvas |
| **Text** | `cosmic-text` (`rustybuzz` + `swash`) — one text stack for UI and canvas |
| **Accessibility** | `accesskit` — UIA / NSAccessibility / AT-SPI |
| **Plugins** | WASM via `wasmtime`, capability-based sandbox |
| **Scripting** | Lua in-process; Python out-of-process |

Full stack, including domain libraries and the reasoning behind each choice, is in [PRD §8](PRD.md).

## Architecture

A single Cargo workspace whose crates layer strictly downward — `core` → `tile` → `graph`/`gpu` → `render`/`doc` → feature crates → `widgets` → `ui` → `app`. A lower crate never depends on a higher one, and CI enforces it.

The design rests on a short list of invariants documented in [PRD §7.3](PRD.md) — nothing assumes a document fits in memory, edits are non-destructive, history stores reversible operations rather than snapshots, the UI thread never blocks on rendering, brush input bypasses the general render path, colour is always explicit, plugins are untrusted, and no widget hardcodes a style value. Each one backs a headline requirement, so they are treated as rules rather than preferences.

## Roadmap

| Phase | Focus |
|---|---|
| **0** | Technical de-risking: GPU validation, tile paging, accessibility & IME spikes, design language — done, Phase 1 gated on it |
| **1** | Document system, canvas, layers, rendering, widget toolkit, application shell — **in progress** |
| **2** | Selections, brushes, masks, filters, adjustments |
| **3** | Smart objects, Camera RAW, colour management, full PSD/PSB read+write |

Every phase has a measurable exit criterion rather than a feature checklist — see [PRD §9](PRD.md). Calendar durations were dropped 2026-07-28 (PRD §13 Step 7): this is solo development, so phases are milestone-based, not date-committed. What were previously dated "Phase 4" (AI features, plugin SDK, automation, cloud sync) and "Phase 5" (collaboration, animation, mobile, web) moved to an explicitly uncommitted "Beyond v1.0" backlog in [PRD §9](PRD.md) — real ideas, not discarded, just no longer carrying a team-sized commitment for a project of one.

## What has been measured

A throwaway [vertical slice](spike/) exercises the whole stack — window → `wgpu` → a 100,000 × 100,000 px half-float tiled document (80 GB) → per-tile composite → brush stroke → save/reload, with UI drawn in the same frame as the canvas. It exists to turn the performance targets from assertions into numbers.

| | Budget | p50 | p99 |
|---|---|---|---|
| Stroke latency (input → frame) | 10 ms | 4.1 ms | 9.1 ms |
| Idle frame | 16.7 ms | 0.6 ms | 0.8 ms |
| Pan with page-in from disk | 16.7 ms | 7.0 ms | 16.7 ms |

An 80 GB document edits comfortably in a 64 MB memory budget, and half-float save/reload is bit-exact. It also corrected the main assumption: the bottleneck is CPU compositing, **not** disk I/O. Full results and limitations — one GPU, one OS, single-threaded — are in [spike/FINDINGS.md](spike/FINDINGS.md).

A second spike covers [accessibility and IME](spike/a11y-ime/FINDINGS.md), the two risks that could still overturn the custom-UI decision. It is **partially** complete: the accessibility tree builds and the platform adapter initializes, but confirming that a screen reader speaks the field and that CJK composition works needs a human on each platform. **Help wanted** — the checklist is in that document.

## What has been built

**The app itself is real, not a mockup.** `cargo run -p aurora-app` opens a native window (macOS-verified, including VoiceOver announcing it) with a canvas, a dockable Layers/History/Properties rail, a native menu bar, and a command palette (`Ctrl+Shift+P`). A brush and eraser tool paint real pixels into `aurora-tile`'s tile store with unified undo/redo across both structural edits (add/delete/reorder a layer) and pixel strokes. The document model (`aurora-doc`) has real layers and groups, opacity, a 27-mode blend-mode enum, masks (bounds only, no mask pixels yet), and an unlimited undo/redo history. Import/export (`aurora-io`) reads and writes PNG, JPEG, TIFF, and Aurora's own round-tripping `.aur` format; autosave/crash-recovery is wired up.

**Compositing is real, not a stub.** All 27 `aurora_doc::BlendMode` variants — the dodge/burn and overlay/light families, the non-separable HSL modes (Hue/Saturation/Color/Luminosity), Darker/LighterColor, and Dissolve (a deterministic per-pixel stochastic gate, seeded from absolute document position so it reproduces bit-identically across pans, re-renders, and reopens) — composite for real, on flat layers and inside groups alike. A group composites its own visible children in correct render order into an isolated buffer first, then applies the group's own opacity and blend mode one level up, so nested groups blend correctly against their surroundings. A GPU-accelerated fast path (`wgpu`, one alpha-blend shader pass per layer per tile) handles the common case — an all-Normal-blend, non-grouped document — with automatic, transparent fallback to the CPU path (`aurora-tile` dirty rectangles + per-tile compositing) for every group, non-Normal blend mode, or mix of the two. Full-document export composites every visible layer, not just the active one. What's still open: mask *pixels* (masks currently carry bounds only), non-Normal blend-mode interactions between layers inside a translucent group's own isolation pass, and extending the GPU fast path beyond the Normal/non-grouped case. PSD/PSB has neither a reader nor a writer in the app — only a separate feasibility spike, deliberately excluded from the workspace so it can never become a real dependency.

**The custom widget toolkit** (`aurora-widgets`) and design-token system (`aurora-theme`) are what the app's own UI is built from: nine widget kinds with real layout, focus, hit-testing, and IME composition, eight of them with real GPU-rendered paint through `aurora-vector`'s tessellation (verified with real pixel-readback tests), six of those additionally covered by a golden-image gallery harness. All five of PRD.md's built-in themes exist as real, verified design files, not just names: Dark, Light, High Contrast Dark, High Contrast Light, and a neutral-grey Colour-Critical theme, each automatically checked against the same 17 WCAG 2.1 AA contrast pairs (Colour-Critical additionally has its surfaces checked for genuine chroma neutrality — an automated satisfaction of the PRD's own acceptance criterion for that theme, not just "we picked hex codes that look gray"). What's still outstanding is a human reviewing the ~24 rendered goldens on real GPU hardware before they're trusted as regression baselines — themes are never blessed blind.

## Building

```sh
cargo build --workspace
cargo test --workspace
```

Requires Rust 1.97+ (pinned in `rust-toolchain.toml`, edition 2024). The full CI gate:

```sh
cargo fmt --all --check && python3 scripts/check_layering.py \
  && python3 scripts/check_no_hardcoded_style.py \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace
```

To run the app itself — macOS-verified; Linux/Windows build but haven't had a real desktop session confirm the window and menu behave:

```sh
cargo run -p aurora-app
```

To run the prototype (a separate crate, outside the workspace):

```sh
cd spike/vertical-slice
cargo run --release -- --headless   # benchmark, no display needed
cargo run --release                 # windowed — drag to paint

cd ../a11y-ime
cargo run -- --dump-tree            # accessibility tree
cargo run                           # windowed — needs a screen reader to judge
```

## Documentation

- **[PLAN.md](PLAN.md)** — the implementation plan and live progress tracker: what is done, what is in progress, what is blocked, and what comes next.
- **[PRD.md](PRD.md)** — the full product requirements document: functional requirements, architecture, technology decisions, risks, open questions, and the pre-implementation plan.
- **[docs/adr/](docs/adr/)** — architecture decision records. Each states what was decided, what was rejected, and what would justify reopening it.
- **[spike/FINDINGS.md](spike/FINDINGS.md)** — measured results from the vertical slice, including what they invalidated.
- **[spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)** — accessibility and IME spike, with the human verification checklist still outstanding.
- **[spike/psd-write/FINDINGS.md](spike/psd-write/FINDINGS.md)** — PSD write spike: a layered PSD written from scratch and checked against independent readers.
- **[CLAUDE.md](CLAUDE.md)** — orientation for [Claude Code](https://claude.com/claude-code) sessions working in this repository.

## Contributing

Aurora is MIT licensed and open to contribution. It's an early, working editor, not a finished product — most Photoshop-parity features (selections tooling, filters, adjustments, PSD, smart objects, RAW) don't exist yet. [PLAN.md](PLAN.md) shows exactly what is open. Contributions are made under the MIT licence; sign off your commits with `git commit -s` (DCO). What is genuinely useful right now:

- **Review the [PRD](PRD.md).** Particularly §11 (Risks) and §12 (Open Questions). Several open questions are unresolved and shape the architecture, notably the PSD round-trip target versions and the handling of Photoshop features with no Aurora equivalent.
- **Tell us about your workflow.** [PRD §13 Step 2](PRD.md) calls for a ranked list of the Photoshop workflows that actually matter to professionals. First-hand accounts are more valuable than speculation.
- **Flag a technology choice you think is wrong.** Cheaper to fix now than in Phase 3.
- **Run the [vertical slice](spike/) on your hardware.** It has only been measured on one GPU under macOS; Windows and Linux numbers, and anything from a different GPU vendor, would be genuinely useful.
- **Test the [accessibility and IME spike](spike/a11y-ime/FINDINGS.md) with a screen reader or a CJK input method.** This is the most valuable thing anyone can contribute right now — it is the last open question that could change a foundational architecture decision, and it cannot be automated.

## License

[MIT](LICENSE) — © 2026 Cahya Wirawan.

Aurora is permissively licensed: use it, fork it, build commercial products on it. Note that shipped binaries will also carry the licences of their dependencies, several of which are C libraries under other terms (see [PRD §8.2](PRD.md)); `cargo deny` enforces licence compatibility in CI.
