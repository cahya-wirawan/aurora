# Aurora

A modern, GPU-accelerated, non-destructive professional image editor for Windows, macOS, and Linux — written in Rust.

> **Status: an early but real editor, Phase 1 in progress.** You can already open images and layered Photoshop files, paint, work with layers, masks, blend modes and adjustment layers, and save back out. It is not a finished product, and most of what is new has not yet been seen on every platform. The honest, detailed picture — what works, what is partial, what has been measured and what has never been verified on real hardware — is in **[STATUS.md](STATUS.md)**. Every release is listed in **[CHANGELOG.md](CHANGELOG.md)**.

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
| **Text** | `harfrust` + `swash` (the engines `cosmic-text` uses, taken directly to avoid `fontdb` → `ttf-parser`, RUSTSEC-2026-0192) — one text stack for UI and canvas |
| **Accessibility** | `accesskit` — UIA / NSAccessibility / AT-SPI |
| **Plugins** | WASM via `wasmtime`, capability-based sandbox |
| **Scripting** | Lua in-process; Python out-of-process |

Full stack, including domain libraries and the reasoning behind each choice, is in [PRD §8](PRD.md).

## Architecture

A single Cargo workspace whose crates layer strictly downward — `core` → `tile` → `graph`/`gpu` → `render`/`doc` → feature crates → `widgets` → `ui` → `app`. A lower crate never depends on a higher one, and CI enforces it.

The design rests on a short list of invariants documented in [PRD §7.3](PRD.md), treated as rules rather than preferences because each backs a headline requirement:

- nothing assumes a document fits in memory;
- edits are non-destructive;
- history stores reversible operations, not snapshots;
- the UI thread never blocks on rendering;
- brush input bypasses the general render path;
- colour is always explicit;
- plugins are untrusted;
- no widget hardcodes a style value.

## Roadmap

| Phase | Focus |
|---|---|
| **0** | Technical de-risking: GPU validation, tile paging, accessibility & IME spikes, design language — done, Phase 1 gated on it |
| **1** | Document system, canvas, layers, rendering, widget toolkit, application shell — **in progress** |
| **2** | Selections, brushes, masks, filters, adjustments |
| **3** | Smart objects, Camera RAW, colour management, full PSD/PSB read+write |

Every phase has a measurable exit criterion rather than a feature checklist — see [PRD §9](PRD.md). Calendar durations were dropped 2026-07-28 (PRD §13 Step 7): this is solo development, so phases are milestone-based, not date-committed. What were previously dated "Phase 4" (AI features, plugin SDK, automation, cloud sync) and "Phase 5" (collaboration, animation, mobile, web) moved to an explicitly uncommitted "Beyond v1.0" backlog in [PRD §9](PRD.md) — real ideas, not discarded, just no longer carrying a team-sized commitment for a project of one.

## What exists today

A short overview; [STATUS.md](STATUS.md) has the detail, the caveats and the numbers.

- **A native editor** (`cargo run -p aurora-app`): canvas, brush and eraser with undo/redo, pan and zoom, several documents in tabs, a dockable Layers/Properties/History rail, a command palette and (on macOS) a native menu bar.
- **Layers**: groups, opacity and Fill, masks, clipping masks, all 27 PSD-compatible blend modes, Curves and Levels adjustment layers, and drawn layer effects (drop shadow, colour overlay, stroke, outer glow).
- **Files**: PNG, JPEG, TIFF, Aurora's own `.aur` format, and layered PSD/PSB (read), with Save As `.psd` for layered documents.
- **A custom widget toolkit and design-token system** with five built-in themes, full accessibility nodes and IME support.
- **Measured performance**: the budgets are tested, not assumed. Brush latency passes in the spike; 60 FPS in the live app at the 300,000 px ceiling is **still missed**. See [STATUS.md](STATUS.md) for every number and its caveats.
- **Not yet**: most Photoshop-parity features (selection tools, filters, most adjustments, smart-object editing, RAW import, PSB writing) and verification on Windows, Linux and most of the Mac UI. The remaining open work is tracked in [PLAN.md](PLAN.md).

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

- **[STATUS.md](STATUS.md)** — where Aurora stands now: what works, what is partial, what has been measured, what has been verified on which platform. Updated every round.
- **[CHANGELOG.md](CHANGELOG.md)** — every version, newest first (generated by `scripts/gen_changelog.py`).
- **[PLAN.md](PLAN.md)** — the implementation plan and live progress tracker: what is done, what is in progress, what is blocked, and what comes next.
- **[PRD.md](PRD.md)** — the full product requirements document: functional requirements, architecture, technology decisions, risks, open questions, and the pre-implementation plan.
- **[docs/adr/](docs/adr/)** — architecture decision records. Each states what was decided, what was rejected, and what would justify reopening it.
- **[spike/FINDINGS.md](spike/FINDINGS.md)** — measured results from the vertical slice, including what they invalidated.
- **[spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)** — accessibility and IME spike, with the human verification checklist still outstanding.
- **[spike/psd-write/FINDINGS.md](spike/psd-write/FINDINGS.md)** — PSD write spike: a layered PSD written from scratch and checked against independent readers.
- **[CLAUDE.md](CLAUDE.md)** — orientation for [Claude Code](https://claude.com/claude-code) sessions working in this repository.

## Contributing

Aurora is MIT licensed and open to contribution. It's an early, working editor, not a finished product — most Photoshop-parity features (selection tools, filters, most adjustments, PSB writing and full PSD fidelity, smart objects, RAW) don't exist yet. [PLAN.md](PLAN.md) shows exactly what is open. Contributions are made under the MIT licence; sign off your commits with `git commit -s` (DCO). What is genuinely useful right now:

- **Review the [PRD](PRD.md).** Particularly §11 (Risks) and §12 (Open Questions). Several open questions are unresolved and shape the architecture, notably the PSD round-trip target versions and the handling of Photoshop features with no Aurora equivalent.
- **Tell us about your workflow.** [PRD §13 Step 2](PRD.md) calls for a ranked list of the Photoshop workflows that actually matter to professionals. First-hand accounts are more valuable than speculation.
- **Flag a technology choice you think is wrong.** Cheaper to fix now than in Phase 3.
- **Run the [vertical slice](spike/) on your hardware.** It has only been measured on one GPU under macOS; Windows and Linux numbers, and anything from a different GPU vendor, would be genuinely useful.
- **Test the [accessibility and IME spike](spike/a11y-ime/FINDINGS.md) with a screen reader or a CJK input method.** This is the most valuable thing anyone can contribute right now — it is the last open question that could change a foundational architecture decision, and it cannot be automated.

## License

[MIT](LICENSE) — © 2026 Cahya Wirawan.

Aurora is permissively licensed: use it, fork it, build commercial products on it. Note that shipped binaries will also carry the licences of their dependencies, several of which are C libraries under other terms (see [PRD §8.2](PRD.md)); `cargo deny` enforces licence compatibility in CI.

## Font credit

Aurora bundles [Inter](https://github.com/rsms/inter) 4.1 Regular, copyright The Inter Project Authors, under the SIL Open Font License 1.1 (`crates/aurora-text/fonts/OFL.txt`).

The PSD reader's test fixtures in `crates/aurora-io/tests/fixtures/psd/` come from [psd-tools](https://github.com/psd-tools/psd-tools)' test suite, under the MIT licence (`LICENSE` and `PROVENANCE.md` alongside them).
