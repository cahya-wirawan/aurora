# Aurora

A modern, GPU-accelerated, non-destructive professional image editor for Windows, macOS, and Linux — written in Rust.

> **Status: pre-implementation.** The workspace skeleton, CI, architecture decisions, and a measured prototype are in place; **no features are implemented yet**. The 19 crates compile and CI is green, but nothing does anything. Phase 0 (technical de-risking) is the current work — see [Roadmap](#roadmap). Stars and discussion are welcome; working software is not.

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

| Phase | Focus | Duration |
|---|---|---|
| **0** | Technical de-risking: GPU validation, tile paging, accessibility & IME spikes, design language | 3 months |
| **1** | Document system, canvas, layers, rendering, widget toolkit, application shell | 9 months |
| **2** | Selections, brushes, masks, filters, adjustments | 8 months |
| **3** | Smart objects, Camera RAW, colour management, PSD/PSB read+write | 10 months |
| **4** | AI features, plugin SDK, automation, cloud sync | 10 months |
| **5** | Collaboration, animation, mobile, web | 12 months |

Every phase has a measurable exit criterion rather than a feature checklist — see [PRD §9](PRD.md). The durations assume a staffed team and were estimated before any code existed; now that the prototype has produced real numbers, they are due a revision (PRD §13 Step 7).

## What has been measured

A throwaway [vertical slice](spike/) exercises the whole stack — window → `wgpu` → a 100,000 × 100,000 px half-float tiled document (80 GB) → per-tile composite → brush stroke → save/reload, with UI drawn in the same frame as the canvas. It exists to turn the performance targets from assertions into numbers.

| | Budget | p50 | p99 |
|---|---|---|---|
| Stroke latency (input → frame) | 10 ms | 4.1 ms | 9.1 ms |
| Idle frame | 16.7 ms | 0.6 ms | 0.8 ms |
| Pan with page-in from disk | 16.7 ms | 7.0 ms | 16.7 ms |

An 80 GB document edits comfortably in a 64 MB memory budget, and half-float save/reload is bit-exact. It also corrected the main assumption: the bottleneck is CPU compositing, **not** disk I/O. Full results and limitations — one GPU, one OS, single-threaded — are in [spike/FINDINGS.md](spike/FINDINGS.md).

## Building

```sh
cargo build --workspace
cargo test --workspace
```

Requires Rust 1.88+ (pinned in `rust-toolchain.toml`, edition 2024). The full CI gate:

```sh
cargo fmt --all --check && python3 scripts/check_layering.py \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace
```

To run the prototype (a separate crate, outside the workspace):

```sh
cd spike/vertical-slice
cargo run --release -- --headless   # benchmark, no display needed
cargo run --release                 # windowed — drag to paint
```

## Documentation

- **[PRD.md](PRD.md)** — the full product requirements document: functional requirements, architecture, technology decisions, risks, open questions, and the pre-implementation plan.
- **[docs/adr/](docs/adr/)** — architecture decision records. Each states what was decided, what was rejected, and what would justify reopening it.
- **[spike/FINDINGS.md](spike/FINDINGS.md)** — measured results from the vertical slice, including what they invalidated.
- **[CLAUDE.md](CLAUDE.md)** — orientation for [Claude Code](https://claude.com/claude-code) sessions working in this repository.

## Contributing

Aurora is MIT licensed and open to contribution, but there are no features to build on yet — the workspace is a skeleton and Phase 0 is where the real work starts. Contributions are made under the MIT licence; sign off your commits with `git commit -s` (DCO). What is genuinely useful right now:

- **Review the [PRD](PRD.md).** Particularly §11 (Risks) and §12 (Open Questions). Several open questions are unresolved and shape the architecture, notably the PSD round-trip target versions and the handling of Photoshop features with no Aurora equivalent.
- **Tell us about your workflow.** [PRD §13 Step 2](PRD.md) calls for a ranked list of the Photoshop workflows that actually matter to professionals. First-hand accounts are more valuable than speculation.
- **Flag a technology choice you think is wrong.** Cheaper to fix now than in Phase 3.
- **Run the [vertical slice](spike/) on your hardware.** It has only been measured on one GPU under macOS; Windows and Linux numbers, and anything from a different GPU vendor, would be genuinely useful.

## License

[MIT](LICENSE) — © 2026 Cahya Wirawan.

Aurora is permissively licensed: use it, fork it, build commercial products on it. Note that shipped binaries will also carry the licences of their dependencies, several of which are C libraries under other terms (see [PRD §8.2](PRD.md)); `cargo deny` enforces licence compatibility in CI.
