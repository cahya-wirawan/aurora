# 0003. Float precision floor — no 8-bit internal path

**Status:** Accepted
**Date:** 2026-07-25
**Related:** PRD §6 *Precision*, FR-010, FR-015, FR-016, invariant §7.3.6b

## Context

Aurora is non-destructive: a pixel may pass through many graph nodes before it is seen. The internal precision determines whether that composition is lossless, and it types every buffer in the system — the single hardest thing to change later.

Photoshop supports 8, 16, and 32-bit documents, with most work done at 8-bit. Supporting an 8-bit internal path would halve memory but requires every shader, blend mode, and filter to exist and be tested twice.

## Decision

**The internal pipeline is always ≥16-bit float. There is no 8-bit internal path.**

- Storage: half-float (`f16`) RGBA tiles — 8 bytes/px — compressed on disk and in cold cache
- Compute: `f32`, or `f16` where a shader proves equivalent within tolerance
- Import: 8-bit sources promoted to float once, on load, with colour space recorded
- Export: quantized to 8-bit only at export or explicit rasterization, with dithering
- Values above 1.0 are preserved, not clipped

## Alternatives considered

**8-bit internal with float opt-in (Photoshop's model)** — halves memory for typical work and matches user expectations. Rejected: it doubles the shader surface and test matrix permanently, and it makes precision a user-visible mode that people get wrong. In a non-destructive graph, a single 8-bit intermediate quantizes to 256 levels per channel and the error compounds across nodes; the damage appears as banding several operations downstream and cannot be traced back or recovered.

**`f32` throughout** — simplest and most precise. Rejected: 16 bytes/px doubles the cost again for precision beyond what image editing needs. `f16` holds ~11 bits of mantissa, comfortably past the ~10-bit threshold where banding becomes visible.

**Fixed-point 16-bit integer** — half the memory of `f32`, more precise than 8-bit. Rejected: cannot represent values above 1.0, so HDR and scene-referred workflows (FR-015, FR-016) are impossible, and shaders need explicit scaling everywhere.

## Consequences

**Gained:** precision survives arbitrary node chains, so the non-destructive model actually holds. Highlights above 1.0 are recoverable — the precondition for Camera RAW, EXR/HDR, and HDR output. Blend and filter math can work in linear light, which is unusable in 8-bit. Node reordering stops changing results. One code path, one test matrix. `f16`/`f32` are native GPU formats, so there is no arithmetic penalty.

**Cost:** **2× the memory and bandwidth of an 8-bit pipeline** — 8 bytes/px rather than 4. Every performance budget in §6 tightens accordingly, and tile compression becomes mandatory rather than an optimization.

The trade was taken because the costs are asymmetric: memory pressure is measurable and can be engineered against, whereas destroyed precision is invisible in review and unrecoverable afterward. The tiling and paging machinery that absorbs the memory cost is required by ADR 0002 regardless.

**Follow-on work:** `f16` tile format and compression in `aurora-tile`; promote-on-import and dither-on-export in `aurora-io`; Phase 0 prototype validates the budgets at 8 bytes/px rather than assuming them; a CI check that no buffer inside the graph is typed 8-bit.

## Reconsider if…

- The Phase 0 prototype misses the §6 budgets at half-float and profiling attributes it to bandwidth rather than to the implementation
- A target platform's GPU lacks adequate `f16` texture support (would raise cost, not change the decision)
- Real-world use shows a large class of work where an 8-bit fast path is both safe and materially faster — note this reopens the two-code-path cost, which is the main thing this ADR is buying away
