# 0008. ICC transform library: lcms2 via FFI

**Status:** Accepted
**Date:** 2026-07-28
**Deciders:** Cahya Wirawan
**Related:** PRD §5 FR-016, §8.2, §14; ADR 0003 (float precision floor); [spike/raw-icc/FINDINGS.md](../../spike/raw-icc/FINDINGS.md)

## Context

Aurora's color pipeline (ADR 0003: ≥16-bit float, no 8-bit intermediates) needs ICC profile transforms (FR-016), with every buffer carrying an explicit color space (invariant §7.3.6) and HDR/scene-referred values preserved rather than clipped (invariant §7.3.1b). PRD §8.2 posed `lcms2` (FFI, wrapping Little CMS) against pure-Rust alternatives, and §14 assumed no mature pure-Rust ICC engine existed — grouping ICC with RAW as an "FFI means copyleft-linking risk" pair.

`spike/raw-icc` checked that grouping directly rather than accepting it. Little CMS's core engine is MIT-licensed, not LGPL (only one optional, unused plugin is GPL-3) — ICC carries none of RAW's licensing complexity. `lcms2-sys` vendors and statically compiles Little CMS's own C source, so there is no dynamic-linking packaging burden at all, unlike the RAW decision (ADR 0007). The spike also found and cross-validated a real pure-Rust alternative, `moxcms` — already a transitive dependency of `rawler` itself, so not a hypothetical — to exact agreement with `lcms2` on a real sRGB→ECI-RGBv2 transform, including out-of-gamut extended-range values (after finding and enabling `moxcms`'s `allow_extended_range_rgb_xyz` option, off by default, without which out-of-gamut values silently clamp — a real footgun against invariant §7.3.1b).

## Decision

**`lcms2`, statically linked (via `lcms2-sys`'s vendored build), for ICC transforms.**

## Alternatives considered

**`moxcms` (pure Rust)** — genuinely viable, not dismissed lightly: correctly licensed (BSD-3-Clause/Apache-2.0), numerically verified against `lcms2` on a real transform including the extended-range case Aurora specifically needs. Rejected for now in favor of `lcms2`'s much longer production track record — decades of real-world use in Photoshop, GIMP, and browsers, across profile types this spike didn't test (CMYK, LUT-based profiles; only RGB matrix-shaper profiles were checked). Unlike the RAW decision, there is no licensing or packaging cost to offset by preferring the pure-Rust option here, so the deciding factor is production maturity, not risk-avoidance.

**Both, with a runtime or build-time switch** — rejected as unnecessary complexity for a Phase 0 decision; nothing in the findings suggests Aurora needs two ICC engines. Revisit only if a real, specific gap in `lcms2` surfaces later (see Reconsider).

## Consequences

**Gained:** an industry-standard, extremely mature ICC engine with no packaging complexity — statically linked, MIT-licensed, no shared-library or relinking obligations of any kind (a much simpler position than ADR 0007's RAW decision).

**Cost:** a C dependency, though a low-operational-cost one given it's vendored and statically compiled rather than dynamically linked. Only RGB matrix-shaper profiles (`sRGB.icc`, `ECI-RGBv2.icc`) were tested this session — CMYK and LUT-based profile transforms are unverified, and Aurora's own color modes (FR-016: RGB, CMYK, Lab, XYZ, Grayscale, HDR) will exercise more of `lcms2`'s surface than this spike covered.

**Follow-on work:** wire `lcms2` into `aurora-color`; test CMYK and LUT-based profile transforms directly; add a permanent regression test asserting extended-range/out-of-gamut values survive a transform rather than clamping (this session's own near-miss — the correct configuration exists but isn't the default, in either library); the ICC profile test corpus (PLAN 0.7).

## Reconsider if…

- Profiling in Phase 3 shows `lcms2` is a real performance bottleneck for Aurora's specific transform patterns — `moxcms` is already proven numerically correct and would be the first thing to benchmark against
- A CMYK or LUT-based profile transform, once actually tested, reveals a gap in `lcms2` this spike's narrower RGB-only testing didn't surface
- Little CMS's licensing changes
