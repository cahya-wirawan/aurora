# 0007. RAW decode library: LibRaw via FFI

**Status:** Accepted
**Date:** 2026-07-28
**Deciders:** Cahya Wirawan
**Related:** PRD §5 FR-015, §8.2, §14; [spike/raw-icc/FINDINGS.md](../../spike/raw-icc/FINDINGS.md), [spike/lgpl-packaging/FINDINGS.md](../../spike/lgpl-packaging/FINDINGS.md)

## Context

Camera RAW import (FR-015) needs a decoder covering the major camera vendors. PRD §8.2 posed the choice as pure-Rust (`rawler`) vs. FFI (LibRaw), and §14 anticipated a licensing cost specifically on the FFI side — LibRaw is LGPL-2.1/CDDL — with "prefer the pure-Rust alternative... even at some capability cost" as the default lean.

Two Phase 0 spikes tested that framing against real libraries and real files rather than assuming it still held. `spike/raw-icc` decoded real, unedited Canon/Nikon/Sony files correctly with `rawler` — but found `rawler` itself is LGPL-2.1, and that no permissively-licensed, full-featured Rust RAW decoder exists at all (the alternatives checked were AGPL-3.0, GPL-3.0, or thumbnail-extraction-only). The "prefer pure Rust" framing does not avoid the licensing question for RAW; it was never available to avoid it. `spike/lgpl-packaging` then prototyped and verified — with `file`/`ldd`/`nm` and an actual before/after modified-library swap, not just argued — that LGPL-2.1 §6(b)'s "suitable shared library mechanism" is satisfiable: a shared library loaded at run time (not statically linked), replaceable by an interface-compatible modified build without recompiling the consuming binary.

With that mechanism proven, the licensing cost is no longer specific to LibRaw — it applies to any full-featured RAW library Aurora could choose. The decision reduces to which library to build that mechanism around.

## Decision

**LibRaw, linked dynamically through a shared-library boundary satisfying LGPL-2.1 §6(b)**, per the mechanism prototyped in `spike/lgpl-packaging` (Linux) — extended to macOS/Windows as follow-on work.

## Alternatives considered

**`rawler` (pure Rust)** — proven to decode all three tested vendors (Canon CR3, Nikon NEF, Sony ARW) correctly, visually confirmed, not just "returned `Ok`." Also LGPL-2.1, so it carries the same packaging obligation LibRaw does — no licensing advantage. It additionally requires Aurora to design, build, and maintain its own hand-written C ABI wrapper (`raw-shim` in the spike) indefinitely: Rust has no native stable-ABI shared-library format, so every decoder feature `aurora-io` needs has to be added to that hand-rolled interface and kept interface-compatible across releases. LibRaw already has a mature, stable C API of its own and native shared-library build support — the same dynamic-linking architecture applies to it with no equivalent wrapper-maintenance cost. Rejected on that basis, not on decode correctness (which was good) or on licensing (which was a wash).

**A permissively-licensed pure-Rust decoder, avoiding the LGPL question entirely** — rejected: none exists today with full-vendor coverage. `zenraw` is AGPL-3.0 (worse than LGPL), `raw_preview_rs` is GPL-3.0, `rawlib` is MIT but thumbnail-extraction only, not a real decoder.

## Consequences

**Gained:** the broadest, most mature RAW format/camera coverage available (LibRaw is the de facto industry standard — used by darktable, RawTherapee, and many others); the LGPL packaging cost is a one-time architecture (a dynamic-linking boundary) rather than an ever-growing hand-maintained FFI surface, since LibRaw's own C API is already the stable interface.

**Cost:** a C++ dependency in an otherwise Rust-end-to-end stack — but this was explicitly anticipated, not a surprise (PRD §8: "FFI wrappers are acceptable where Rust lacks maturity (RAW, ICC)"). LibRaw must be built and packaged as a genuine shared library (not statically linked) on all three platforms; only Linux mechanics are verified so far. The packaging spike prototyped the mechanism using `rawler`, not LibRaw directly — the argument that LibRaw's case is simpler (no custom ABI needed) is sound but not yet independently re-verified with LibRaw itself. The one legal ambiguity `spike/lgpl-packaging/FINDINGS.md` flagged (§6(b)(1)'s "already present on the user's system" wording, read against bundling a bundled-but-separate shared library in Aurora's own installer) still needs real legal review before shipping.

**Follow-on work:** re-run the `spike/lgpl-packaging` mechanism with LibRaw itself, not just as an analogy from the `rawler` prototype; macOS (`.dylib`/`install_name`/`@rpath`) and Windows (`.dll`/search order) packaging, unverified so far; legal review of the relinking obligation before v1.0; the RAW test corpus (PLAN 0.7) built against LibRaw's actual output, not `rawler`'s.

## Reconsider if…

- LibRaw's format/camera coverage proves insufficient for a vendor Aurora specifically needs (unlikely, given it is the broadest option available, but not impossible for an obscure or brand-new format)
- Real legal review finds the LGPL relinking obligation unsatisfiable or commercially unacceptable for Aurora's distribution model — in which case `rawler`, already proven to decode correctly and already covered by the same packaging mechanism, is the documented fallback
- A genuinely permissively-licensed, full-featured Rust RAW decoder emerges later, removing the packaging cost entirely
