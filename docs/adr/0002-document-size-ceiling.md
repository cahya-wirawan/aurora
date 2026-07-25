# 0002. Document size ceiling of 300,000 × 300,000 px

**Status:** Accepted
**Date:** 2026-07-25
**Related:** PRD §6 *Scalability*, FR-001, invariant §7.3.1

## Context

The original PRD specified 500,000 × 500,000 px. The figure drives the entire tile store, paging strategy, and coordinate types, and is expensive to change once code exists — but it appeared to be chosen arbitrarily rather than derived from anything.

Adobe's actual limits:

| Format / limit | Maximum |
|---|---|
| PSD | 30,000 × 30,000 px |
| PSB (Large Document Format) | 300,000 × 300,000 px |
| Photoshop canvas | 300,000 × 300,000 px |

So 500,000 matched no format, no competitor, and no identified user need. It was 2.8× more area than Photoshop can produce.

## Decision

The ceiling is **300,000 × 300,000 px**, matching PSB. This applies to both `.aur` and imported documents.

Documents exceeding 30,000 px in either dimension are written as PSB rather than PSD automatically, since PSD cannot represent them (FR-001).

## Alternatives considered

**Keep 500,000 × 500,000** — rejected: no requirement traces to it. It would widen coordinate types and inflate the paging design for headroom nobody asked for. Aurora can already open anything Photoshop produces at 300,000.

**Lower it to ~65,536** — genuinely tempting, since it fits comfortably in `u16`-derived tile addressing and covers the overwhelming majority of real work. Rejected: it would make Aurora unable to open a class of file Photoshop can, which directly undercuts the PSD-compatibility goal.

**Make it configurable** — rejected: a ceiling that varies changes which coordinate arithmetic is safe. One fixed, generous limit is simpler to reason about and to test.

## Consequences

**Gained:** a limit derived from a real constraint (PSB parity) rather than a guess, so it can be defended and tested against. Aurora opens anything Photoshop can produce.

**Cost:** still far beyond memory. At the half-float precision floor (ADR 0003), one 300,000 × 300,000 RGBA layer is ~720 GB. Tiling and disk paging remain load-bearing from day one; this decision reduces the number but does not change the architecture.

**Follow-on work:** coordinate types sized for 300,000 px with defined overflow behaviour; tile dimension and scratch-disk budget settled by the Phase 0 paging prototype; PSB auto-promotion in `aurora-io`.

## Measured (2026-07-26)

The vertical slice exercised 100,000 × 100,000 px (80 GB) sparsely in a 64 MB
budget: 930 evictions, 393 page faults, and panning that pages in from disk still
within frame budget. The sparse tile store should be indifferent to the ceiling
itself, but **300,000 px is untested** — only the tile-count scaling would differ,
and that is worth confirming before Phase 1 closes.

## Reconsider if…

- Target users turn out to work far below this, and a lower ceiling would materially simplify the tile store (PRD §13 Step 2 should surface this)
- Adobe raises the PSB limit, since parity is the whole basis for the number
- The Phase 0 prototype shows the paging design cannot hold the performance budgets at this size
