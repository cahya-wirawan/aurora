# 0005. Tile size (256×256 px) and scratch-disk budget mechanism

**Status:** Accepted
**Date:** 2026-07-28
**Related:** PRD §6 *Precision/Scalability*, §7.2 (`aurora-tile`), §7.3.1, ADR 0002, ADR 0003, FR-026

## Context

`aurora-tile`'s real implementation (M1.1) needs a concrete tile dimension and a scratch-disk budget model before a line of code is written — retrofitting a different tile size later means re-deriving upload scheduling, GPU slot addressing, and every paging number already measured. PRD §12 named this an open question ("at 8 bytes/px the tile dimension trades GPU upload efficiency against paging granularity; settle it with the Phase 0 paging prototype") and ADR 0002 named it as follow-on work. It was never settled — PLAN.md tracked this ADR as blocked on "0.4 numbers" (the accessibility spike, which produces no tile-sizing data at all); the actual numbers were in 0.3, the vertical slice, all along.

## Decision

**256×256 px tiles.** At 8 bytes/px (half-float RGBA, ADR 0003), that is 512 KiB per resident tile — exactly what `spike/vertical-slice` measured (`tiles.rs`: `pub const TILE: u32 = 256`; `spike/FINDINGS.md`'s 64 MB budget was deliberately set to "128 resident tiles of 512 KiB"). This is the only real data point that exists: the upload-bandwidth finding ("~18 MB per screenful, p99 degrades to 53 ms on a fast fling," `spike/FINDINGS.md` finding #3) was measured at exactly this size, and invariant §7.3.1 was validated against it (80 GB document, 64 MB budget, real eviction and page-in).

**Scratch-disk budget is a mechanism, not a fixed number.** PRD.md already assigns "location and size are user preferences" to FR-026 — inventing a hardcoded byte limit here would just be a number ADR 0005 has no more authority to set than FR-026's own preferences UI does. What this ADR settles instead: the budget is tile-count-based (equivalent to a byte budget at a fixed tile size), enforced by evicting the least-recently-used resident tile whenever a page-in or new-tile allocation would exceed it — the same shape the spike already validated, just with a real LRU instead of a linear scan. `aurora-tile`'s own tests use a small, deliberately paging-forcing budget (following the spike's own practice of setting a budget that exercises eviction rather than avoiding it); the real default surfaced to users is FR-026's job, not this ADR's.

## Alternatives considered

**A larger tile (512×512 or 1024×1024)** — fewer tiles to track per document, less per-tile overhead. Rejected: no measurement exists at this size, and the one number that does exist (upload bandwidth, finding #3) was measured at 256 px specifically — moving to a larger tile changes the exact bandwidth-per-tile-upload tradeoff that finding was about, without new data to justify it. Also directly increases the fixed per-touch upload cost (moving one texel dirties the whole tile at GPU-upload granularity in the naive case), working against the "small brush touches many tiles is fine, but each tile transfer should be cheap" tradeoff the whole architecture depends on.

**A smaller tile (64×64 or 128×128)** — smaller granularity means dirty regions waste less bandwidth on unaffected texels. Rejected: more tiles per document means more per-tile bookkeeping overhead (LRU entries, HashMap entries, file-open/close cycles on paging) for no measured benefit, and PRD's own framing treats upload efficiency and paging granularity as a tradeoff to *balance*, not one to maximize by shrinking tiles indefinitely.

**A fixed scratch-disk byte budget chosen here (e.g. "2 GB default")** — tempting, since it would be concrete and "done." Rejected: PRD.md FR-026 already owns "size" as a user preference; picking a number in an ADR that FR-026's own UI is supposed to control would create exactly the kind of conflicting-source-of-truth problem the project's own preferences model exists to avoid.

## Consequences

**Gained:** a tile size backed by the one real measurement that exists, rather than a guess; a budget model (tile-count LRU) that generalizes directly to a byte budget without new code once FR-026 wires up a real user-facing size.

**Cost:** the 300,000 px ceiling (ADR 0002) has never been tested with real tiles at this size — only 100,000 px (80 GB) has been measured. Tile-count overhead at 300,000² / 256² ≈ 1.37M tiles per fully-populated layer is untested; the sparse store should be indifferent to it (most of a huge canvas is typically not resident at once), but that is an assumption, not a result.

**Follow-on work:** the 300,000 px re-run PLAN.md's 0.3 section already lists as outstanding now doubles as this ADR's own validation; `aurora-gpu`'s toroidal slot addressing (finding #4) needs to agree on this exact tile size for UV computation; FR-026's real scratch-disk size preference, whenever that UI exists.

## Measured (2026-07-28, alongside `aurora-tile`'s implementation)

`crates/aurora-tile`'s own tests exercise real eviction, paging, and compression at 256×256/512 KiB tiles with `lz4_flex` — a real, if small-scale, second measurement at this exact tile size beyond the spike itself. See `crates/aurora-tile/benches/tile_store.rs` for paging-throughput, eviction-cost, and compression-ratio numbers.

## Reconsider if…

- The outstanding 300,000 px re-run shows tile-count overhead at that scale is a real problem the sparse store doesn't absorb gracefully
- `aurora-gpu`'s actual GPU upload/slot-addressing implementation finds 256×256 an awkward texture-atlas granularity in practice
- FR-026's real scratch-disk preferences UI surfaces a use case (e.g. very large canvases on constrained disks) this tile-count-based model handles poorly
