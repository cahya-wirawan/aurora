# 0004. Full layered PSD/PSB write

**Status:** Accepted
**Date:** 2026-07-25
**Related:** PRD §6 FR-001, §9 Phase 3, risks R3 and R3b

## Context

"PSD compatibility" was a stated goal without a defined scope. Read-only import and full layered round-trip are very different commitments — write fidelity is substantially harder, and PSD has no complete public specification, so much of it is reverse-engineered.

The question is really about adoption. A professional cannot use Aurora inside an existing Photoshop team if files only travel one way. Import-only makes Aurora a destination, not a participant.

## Decision

**Full layered read *and* write.** A file opened from Photoshop, edited in Aurora, and saved back must reopen in Photoshop with its layer structure intact.

Preserved on round-trip: layer tree and groups, names and colour labels, opacity and fill opacity, blend modes, visibility, locking, layer masks, vector masks, clipping masks, editable text layers, shape and path data, adjustment layers, embedded and linked smart objects, layer styles, ICC profile, and document metadata. 8/16/32-bit and Lab/CMYK/Grayscale documents are supported.

Documents above 30,000 px are written as PSB automatically (ADR 0002).

**Lossy-conversion policy:** where an Aurora feature has no Photoshop representation, Aurora warns and **itemizes what will be lost or flattened before writing**. Silent degradation is not acceptable.

**Write safety:** never overwrite in place. Write to a temporary file, verify by reopening and diffing, then swap.

## Alternatives considered

**Read-only import** — much cheaper and lower-risk; users export flattened when they need to go back. Rejected: it blocks the collaborative workflows that make adoption possible, and quietly reduces Aurora to a one-way tool.

**Write flattened PSD only** — trivial, and honest about its limits. Rejected: a flattened file is not interoperability. Layers are the reason PSD exists.

**Use an existing Rust PSD crate** — rejected: available crates are read-only and incomplete. Write support has to be built regardless, so `aurora-io` owns both directions and shares one model between them.

## Consequences

**Gained:** Aurora participates in existing Photoshop workflows rather than replacing them wholesale, which is the realistic adoption path. Round-trip fidelity is a strong, testable differentiator against other alternatives.

**Cost:** the largest single item in `aurora-io`, and Phase 3 was extended from 8 to 10 months to hold it. It also introduces the project's worst failure mode (R3b): **corrupting a professional's working file** — reputationally far worse than a missing feature, which is why write-to-temp-and-verify is part of the decision rather than an implementation detail.

**Follow-on work:** a 1,000-file real-world PSD corpus assembled *before* the parser (PRD §13 Step 6); automated round-trip diffing in CI from the first layer type; a Phase 0 spike that produces a file Photoshop reopens, not merely parses one; mapping tables between Aurora and Photoshop adjustment and blend semantics.

## Reconsider if…

- The Phase 0 write spike shows fidelity for text layers, smart objects, or layer styles is unreachable within Phase 3 — in which case scope narrows to a documented subset with warnings, rather than dropping write entirely
- Adobe changes the format such that round-tripping current files becomes infeasible
- The 1,000-file corpus reveals a long tail of structures too varied to support, which would argue for a defined "supported subset" contract instead of implied completeness
