# 0009. `.aur` document format: a ZIP container, `postcard` metadata, embedded tile codec

**Status:** Accepted
**Date:** 2026-08-06
**Deciders:** Cahya Wirawan
**Related:** PRD §8.1, §12 Q7, §14 ("`.aur` is an open format"); ADR 0002 (300,000 px ceiling); ADR 0005 (tile size/scratch-disk budget); invariant §7.3.3 (history stores operations, not snapshots); PLAN.md M1.4 (crash-recovery journal's deferred on-disk half), M1.9

## Context

PRD §12 Q7 asks directly: "must `.aur` be forward-compatible across versions from v1.0? Decide before the first byte is written." Nothing has been written yet — `aurora-doc`'s crash-recovery journal (M1.4) and `aurora-app`'s crash-recovery marker (M1.8) have both already deferred real on-disk persistence specifically because no `.aur` encoding existed to persist to. `aurora-io`'s own PNG work (M1.9) reached the same wall from the other side: a real file format now exists for a single flat image, but the actual document — a `LayerTree`, a `History` journal, and per-layer pixel data potentially spanning up to the 300,000×300,000 px ceiling (ADR 0002) — has nowhere to go.

PRD §8.1's own technology table already narrowed the *serialization* half of this to two named candidates, `serde` + `postcard`/`rkyv`, but named no container shape and answered no forward-compatibility question — those are what this ADR actually decides. Three requirements shape the answer, not just picking a crate:

1. **§14's own consequence: "`.aur` is an open format. Specified and freely implementable, consistent with the local-first goal."** A format description that requires linking a specific Rust crate's own in-memory layout to read (as zero-copy formats like `rkyv` inherently do) is a much harder target for an independent, non-Rust implementation than one built from off-the-shelf, ubiquitous primitives.
2. **The 300,000 px ceiling and "unlimited layers/history" (PRD Scalability) mean whole-file parsing is not an option.** The same reasoning that makes PSD/PSB require lazy, streaming parsing ("open a 2 GB PSD in under 5 s") applies at least as strongly to Aurora's own format — a document must be openable, and individual layers/tiles randomly accessible, without holding the whole file in memory.
3. **`aurora-tile` already has a real, proven, versioned tile codec** (`codec::encode`/`decode` — `ATIL` magic, version byte, `lz4_flex` compression with a raw fallback for incompressible data, bit-exact round-trip proven in that crate's own tests). A document format needs to store bulk pixel data somewhere; inventing a second pixel encoding at the document level, instead of reusing the one that already works, would be redundant compression (paying the CPU cost twice) and a second thing to keep correct.

## Decision

**`.aur` is a ZIP archive** (via the `zip` crate, default features trimmed to just `STORE`/`DEFLATE` — no encryption, no `bzip2`/`lzma`/`zstd`/`ppmd`, none of which this format needs), holding:

- A first entry, `mimetype`, stored *uncompressed* — the same trick ODF/EPUB/OpenRaster (`.ora`) use so a magic-byte sniff can identify the format without parsing a full ZIP central directory.
- A manifest entry (document header: canvas size, colour space, and the full `LayerTree` structure — every `LayerId`/`LayerKind`/`BlendMode`/opacity/lock/mask/etc.) serialized with `serde` + **`postcard`**.
- A history entry: `History`'s own journal (`History::replay`'s log, M1.4), also `postcard` — this is what finally gives the crash-recovery journal (deferred in M1.4 and M1.8) somewhere real to be written.
- One ZIP entry per tile, storing `aurora_tile::codec::encode`'s own output **verbatim, stored without further ZIP-level compression** (it is already `lz4_flex`-compressed; compressing compressed bytes again wastes CPU for no size benefit, and `codec::decode` already handles its own header/fallback logic unchanged).

**Forward/backward compatibility policy (Q7's own question, answered directly):**

- **Backward, unconditionally**: every `.aur` file this project ever writes must keep opening in every future Aurora version. A breaking change to a metadata struct's shape is expressed as a new manifest schema (the postcard-serialized struct carries its own version field), with the reader supporting every past schema version it has ever shipped — never a hard cutoff.
- **Forward, best-effort**: an unknown ZIP entry (from a newer Aurora version a current build doesn't recognise) is skipped, not fatal — ZIP's own central directory already makes "list entries, read only the ones you understand" natural, so this costs nothing extra to support. An older Aurora opening a newer file loses only the parts it doesn't recognise, the same "degrade honestly, never silently" ethos already applied to lossy PSD saves (PRD §5), not a crash.
- Every write always uses the current format version; explicit "save as an older `.aur` version" compatibility export is out of scope here — not requested, no evidence it's needed yet.

**`postcard` over `rkyv`** for the metadata entries: `rkyv`'s zero-copy design ties its wire format to Rust's own in-memory type layout, in direct tension with §14's "specified and freely implementable" goal, and its real selling point — avoiding a deserialization pass over huge data — doesn't apply here, since the actual huge data (pixels) is handled separately by `aurora_tile::codec`, not by whatever serializes the comparatively small `LayerTree`/`History` structs. `postcard`'s simple, documented, `no_std`-friendly wire format is the better fit for something meant to be independently implementable.

## Alternatives considered

**A hand-rolled binary chunk container** (RIFF/PNG-style: magic + version + a flat sequence of length-prefixed, typed chunks), the same shape `aurora_tile::codec` already uses one level down. Rejected in favour of ZIP specifically because ZIP already *is* that shape (a central directory is a chunk index), plus it comes with ubiquitous, battle-tested tooling in every ecosystem — a user can inspect a `.aur` file's contents with a file manager they already have, a real, tangible instance of "open format" a bespoke container wouldn't provide for free. Reinventing chunk framing, a table of contents, and a parser for it would be real, avoidable engineering effort duplicating what ZIP readers already do correctly.

**`rkyv` for metadata** — see Decision above; rejected on the "open format" goal and because its main advantage doesn't apply to the data it would actually be encoding.

**A single monolithic `postcard`-serialized struct for the whole document, no container** — rejected: no lazy/streaming access at all (the entire point of ADR 0002's 300,000 px ceiling and PRD's "open a 2 GB PSD in under 5 s" bar), and no clean forward-compatibility story — a newer field added to one giant struct either breaks every old reader or requires `#[serde(default)]` scattered everywhere with no room to skip a whole *unrecognised feature* the way an unknown ZIP entry can be skipped outright.

**Deferring the decision further, waiting for more evidence** — rejected: PRD §12 Q7 explicitly frames this as a "decide before the first byte is written" question, and two real subsystems (the crash-recovery journal, `aurora-io`'s own PNG work) are already blocked on exactly this gap; there is no additional Phase 0-style spike evidence this decision is waiting on the way RAW/ICC's library choice was.

## Consequences

**Gained:** a real place for the crash-recovery journal (M1.4/M1.8's own deferred "no on-disk encoding decided" gap) to finally write to; lazy/lazy-enough streaming access matching the 300,000 px ceiling, via ZIP's own central directory rather than a hand-built index; reuse of `aurora-tile`'s already-proven, already-tested tile codec rather than a second pixel encoding; a genuinely third-party-implementable format (any language with a ZIP library and a `postcard` decoder — or just enough of `postcard`'s simple wire format description to hand-write one — can read a `.aur` file), matching PRD §14's explicit design goal; real, proven precedent in the exact same problem domain (Krita's `.kra` and OpenRaster's `.ora` are both ZIP-based layered-image containers), not a novel approach with no track record.

**Cost:** a ZIP container has real overhead a bespoke format wouldn't (a central directory, per-entry headers) — negligible next to tile-sized payloads, but not zero. `postcard`'s own schema evolution is manual, not automatic: every future breaking change to a manifest/history struct's shape needs a deliberate new version handled explicitly in the reader, not a framework that does it automatically (`rkyv`'s archived-type approach has the same property, so this isn't a comparative cost, but it is a real, ongoing discipline this project now owns). Encrypting or otherwise protecting a `.aur` file's contents is not addressed by this decision (not asked for; the `zip` crate's `aes-crypto` feature exists if ever needed, deliberately not enabled now).

**Follow-on work:** the actual container reader/writer implementation (a real, separate `aurora-io` module — this ADR decides the shape, not the code); deciding the manifest schema's exact field-by-field layout for `LayerTree`/`History` (real design work, not fully specified here); wiring `aurora-doc`'s crash-recovery journal to actually write through this format (M1.4's own follow-on); deciding autosave frequency/granularity (a separate M1.9 bullet); the real question `aurora_doc::LayerKind::Pixel`'s own doc comment already named and this ADR doesn't resolve — whether pixel storage is one `TileStore` per layer or one shared store addressed some other way — which the manifest's own per-layer tile-addressing scheme will need an answer to, not before.

## Reconsider if…

- A real non-Rust reader implementation hits genuine friction with `postcard`'s wire format specifically (unlikely — it's a simple, documented varint-based format — but not yet tried by anyone outside this project)
- `aurora_tile::codec`'s own on-disk format changes in a way that stops being safely embeddable as an opaque ZIP entry (e.g. if it ever needs random access *within* a tile's own payload, which stored-whole-tile access doesn't provide)
- A future, genuinely huge-single-session workload (e.g. real-time collaborative editing, PRD's own "Beyond v1.0" territory) needs a append-only or memory-mapped access pattern ZIP's central-directory-rewrite-on-update model serves poorly — at that point, `rkyv`'s zero-copy mmap story or a purpose-built log-structured format become worth re-evaluating against the "open format" cost they'd bring back
