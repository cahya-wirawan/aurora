# Architecture Decision Records

Each ADR records one decision that is expensive to reverse: what was decided, what was rejected, and **what would make us reconsider**. The reasoning matters more than the verdict — a future reader needs to know whether the ground has shifted, and that is only answerable if the original reasoning is written down.

## Status values

- **Proposed** — under discussion
- **Accepted** — decided and in force
- **Superseded by NNNN** — replaced; kept for the record, never deleted
- **Deprecated** — no longer applies, nothing replaced it

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-custom-wgpu-ui.md) | Custom UI toolkit on wgpu | Accepted |
| [0002](0002-document-size-ceiling.md) | Document size ceiling of 300,000 px | Accepted |
| [0003](0003-float-precision-floor.md) | Float precision floor (≥16-bit, no 8-bit path) | Accepted |
| [0004](0004-psd-full-write.md) | Full layered PSD/PSB write | Accepted |
| [0005](0005-tile-size-scratch-budget.md) | Tile size (256×256 px) and scratch-disk budget mechanism | Accepted |
| [0006](0006-accessibility-conformance-target.md) | Accessibility conformance target: WCAG 2.1 AA | Accepted |
| [0007](0007-raw-library-libraw.md) | RAW decode library: LibRaw via FFI | Accepted |
| [0008](0008-icc-library-lcms2.md) | ICC transform library: lcms2 via FFI | Accepted |
| [0009](0009-aur-document-format.md) | `.aur` document format: ZIP container, `postcard` metadata, embedded tile codec | Accepted |
| [0010](0010-layer-pixel-storage.md) | Layer pixel storage: one shared `TileStore` per document, addressed by surface | Accepted |

## Writing a new one

Copy [`template.md`](template.md), take the next number, add a row above. Keep it short — an ADR that is a chore to write does not get written. Amend rather than rewrite while Proposed; once Accepted, supersede with a new ADR instead of editing the decision.
