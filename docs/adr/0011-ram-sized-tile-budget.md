# 0011. A RAM-sized, shared resident-tile budget

**Status:** Accepted
**Date:** 2026-10-11
**Deciders:** Cahya Wirawan (design owner: "a share of system RAM"); engineering (the fraction, floor, ceiling and sharing rule)
**Related:** PRD §6 *Scalability*, §7.3.1, ADR 0005, ADR 0010, FR-026

## Context

ADR 0005 settled the mechanism — a tile-count LRU, a byte budget at the fixed 512 KiB tile — and left the number to FR-026. Until 0.184.0 the app used a fixed 256 tiles (128 MiB), chosen as "enough to paint comfortably", never measured against a real document. The 0.183.0/0.184.0 breakdown of the user's Nyepi template (1080×1920, 223 pixel layers, about 2,900 layer tiles) showed every full composite paging about 2,900 tiles back in from the scratch disk at that budget: its working set is about 1.45 GiB, eleven times the budget. On 2026-10-11 Cahya decided the budget becomes a share of system RAM and left the exact numbers to engineering. Since 0.172.0 several documents can be open at once, each with its own store (ADR 0010), so "the budget" also has to say how it is split.

## Decision

**Size.** `aurora_tile::budget_bytes_for_ram`: a quarter of physical RAM, clamped to **[128 MiB, 8 GiB]**, and the floor when the RAM is unknown. In tiles: 256 to 16,384. A 4 GiB machine gets 1 GiB (2,048 tiles), 8 GiB gets 2 GiB, 16 GiB gets 4 GiB, 32 GiB and up get the 8 GiB ceiling.
- *A quarter:* the other three quarters are for the OS, the GPU driver's staging, the 256 MiB drop-shadow cache, the undo journal, the failed-write reserve (up to one budget's worth of compressed tiles, only on a failing scratch disk) and the user's other applications. The budget is a cap on what may be resident, never a preload (§7.3.1): a small document uses no more memory than before.
- *Floor 128 MiB:* the old fixed budget, so no machine gets less than it did.
- *Ceiling 8 GiB:* enough for a template ten times Nyepi's working set; past it, more cache mostly holds tiles no single view needs, and RAM is better left to the rest of the system.
- *Bytes first:* the policy is in bytes and converted through `TILE_BYTES` (computed from the tile's own sample count), so a tile-size change cannot leave it counted in the old unit.

**Detection.** `aurora_tile::physical_ram_bytes`: Linux `/proc/meminfo` `MemTotal` (no `unsafe`), macOS `sysctlbyname("hw.memsize")` (`libc`, already a dependency), Windows `GlobalMemoryStatusEx` (`windows-sys` 0.61, already in the tree; one feature added). Any other OS, or any failure, gives `None` and so the floor. The `unsafe` calls stay in `aurora-tile`, the one crate allowed it.

**Override.** `AURORA_TILE_BUDGET_MB`, a whole number of MiB, clamped to the same range; anything else (empty, signed, fractional, suffixed) is ignored and the RAM-derived budget stands.

**Sharing.** One `aurora_tile::ResidencyPool` per process. Every document's store joins it (`TileStore::new_shared`) and leaves when dropped; each member's budget is `max(floor, total / members)`, re-read on every admission. A closed document's share goes back to the others at once (growth needs no work). The active document shrinks on its own next admission; a parked document admits nothing, so the app calls `DocumentShelf::enforce_tile_budgets` once per event-loop turn, which evicts through the ordinary path (dirty tiles encoded and written to scratch, held in `pending` until the write lands). Stores built with a fixed `TileStore::new` budget (tests, export verification) are outside any pool.

**Bounded shrink (review RB-1).** A shrink never encodes a store's whole excess in one UI-thread turn. Enforcement evicts at most `EVICTION_BURST` (32) tiles per store per event-loop turn, and the loop polls again at once (`tile_budget_poll`) while any store is still over its share. An admission into an over-budget store evicts at most the same burst, and always at least one, so its resident count never grows while it is over. **So residency can overshoot the new share for a few turns:** a store 1,000 tiles over converges in `ceil(1000 / 32)` = 32 turns. The 32 is an estimate of about one frame of encoding, not a measurement.

**Eviction policy:** unchanged, plain LRU. At this machine's budget Nyepi's working set fits, so a scan-resistant policy would change nothing there; see *Reconsider if*.

## Alternatives considered

**Keep a fixed number, raised (e.g. 1 GiB).** Simple, but wrong at both ends: too much for a 4 GiB laptop, a fraction of what a 64 GiB workstation can spare. Rejected by the design owner's decision.

**A per-store budget, not shared.** Each open document gets the whole RAM share. Rejected: with *n* documents the process could hold *n* quarters of RAM.

**The active document gets almost everything; parked ones shrink to the floor.** Best for the document being edited, but switching back to a parked large document then pages its whole working set in again (about 2,900 tiles for Nyepi, seconds). An equal split keeps both warm when they fit, as they do on this machine with two Nyepi-sized documents, and needs no notion of "active" inside `aurora-tile`.

**`sysinfo` for RAM detection.** One call on every OS, but a sizeable new dependency tree for three short platform calls. Rejected.

## Consequences

**Gained:** on this 62 GiB machine the budget is 8 GiB (16,384 tiles); Nyepi's warm full composite no longer pages anything in (PLAN.md 0.185.0 has the numbers). Several open documents share one bounded total.

**Cost:** more resident memory per large document, by design, up to the budget. A clean tile whose scratch copy is still valid is re-encoded and rewritten when evicted (review RB-2, deferred: tracking "unchanged since its confirmed write" touches the pending/generation/tombstone bookkeeping, so it is not a small change). That makes a shrink cost one encode per tile, not a free drop. The floor means the sum can exceed the total by up to `members × 128 MiB` when very many documents are open. A store stays over its share for up to `ceil(excess / 32)` event-loop turns after a share drops. A cgroup or container memory limit is not read, so in a container the host's RAM sets the budget. The macOS and Windows detection paths are compiled and linted (cross-clippy) but have not been run.

**Follow-on work:** FR-026's preference UI (the override is the stopgap); a scan-resistant policy if smaller machines show thrash; reading a cgroup limit on Linux.

## Reconsider if…

- A document's working set exceeds the budget on a realistic machine and an in-order composite pass thrashes the LRU (measured: Nyepi at 2,048 tiles, a 4 GiB machine's budget — see PLAN.md 0.185.0). Then a scan-resistant policy (2Q, CLOCK-Pro) or protecting the current pass's tiles is the next step.
- Users report memory pressure on machines where a quarter of RAM is too much (the GPU driver or other applications need more).
- FR-026's scratch-disk preferences land and own this number.
