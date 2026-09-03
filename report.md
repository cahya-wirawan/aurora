# Aurora — Project State Report

**Date:** 2026-09-04 (updated after further rounds of work; originally written at 0.90.2)
**Version:** 0.93.1
**Branch:** `gauntlet-new`
**Scope of this report:** the current state of the project as a whole, with a detailed account of this session's work (a long run of Gauntlet Loop rounds, 0.79.1 → 0.93.1).

---

## 1. What Aurora is

A cross-platform, GPU-accelerated, non-destructive professional image editor (a Photoshop alternative) for Windows, macOS, and Linux, built in Rust. PSD/PSB compatibility and AI-assisted editing are first-class requirements, not add-ons. The UI toolkit is custom-built on `wgpu` rather than using a third-party framework — a deliberate, still-open-risk architectural bet (see §4).

Scale: ~110,800 lines of Rust across 20 crates, 1,633 tests passing (real GPU hardware, see §3), full local CI gate green.

## 2. Where the project is, structurally

- **Phase 0 (de-risking)** was declared satisfied on 2026-07-28, with a known tail still open: Windows/DX12 validation, the Linux/Windows human legs of the accessibility/IME checklist, and macOS/Windows LGPL packaging.
- **Phase 1 (document, canvas, layers, rendering, shell)** is where active work is. Most milestones (M1.1–M1.9) are substantially done with named open items each. **M1.10 is the Phase 1 gate**, and it is what is actually blocking progress to Phase 2. Its remaining items are:
  - **Hardware/human-gated**: Linux (AT-SPI) and Windows (UIA) accessibility verification — entirely unverified; only macOS has been checked (9/10, spike/a11y-ime/FINDINGS.md). This is the single most durable open risk in the project (ADR 0001) — it could still overturn the custom-`wgpu`-UI decision if it fails.
  - **Design-owner-gated**: which gallery component to build next, token decisions — Cahya's call, not engineering's.
  - **Tooling-gated**: the PSD spike needs `psd-tools` as an independent reader for cross-validation.
  - **Large, multi-round work**: 25 of 27 blend modes still need GPU ports (2 landed: Multiply, Darken; Dissolve resolves to Normal so needs none; 23 remain). Group support for GPU compositing is not started.
  - **Measured and failing**: the 60 FPS interaction budget (see §3 — this was this session's main focus).
- **Not started at all**: filters/adjustments, selection tools beyond a raw data model, smart objects, RAW import, plugins, scripting, AI, true infinite pasteboard panning.

## 3. The 60 FPS investigation — what this session actually did

This is the most substantial thread of this session and deserves a detailed account, because the *path* to the current numbers matters as much as the numbers themselves.

### The starting point

CLAUDE.md's own "Measured, not assumed" table (from an earlier round, 0.39.0) reported the real app's pan-while-painting frame at **p50 35.4 ms / p99 98.8 ms** against a 16.7 ms (60 FPS) budget — roughly 6× over at the tail. Three consecutive optimization rounds early in this session (GPU blend-mode ports for Multiply/Darken, batching per-tile GPU command submits) each shipped correct, real, tested work — but **none of them moved that number**, because none of them had first established *which stage of the frame actually dominates the cost*. Each was a plausible-sounding guess.

### The correction: measure before optimizing

A diagnostic round (0.88.0–0.88.2) added real per-stage timing instrumentation to the actual end-to-end benchmark and got an evidence-based answer for the first time. Two findings mattered:

1. **The headline numbers themselves were stale.** The 98.8 ms figure was measured on this sandbox's *old Mesa llvmpipe software renderer*. Once re-measured on this sandbox's real hardware (**NVIDIA RTX 3090, Vulkan**), the actual numbers were roughly **half**: ~53 ms mean / ~59 ms p99 (GPU path), ~30 ms / ~33 ms (CPU fallback) — still 2–3.5× over budget, but a materially different starting point than the project's own documentation stated. This discrepancy is flagged in PLAN.md but CLAUDE.md's table itself has *not* been corrected — that remains open, tracked work.
2. **The real cost breakdown**, on the real GPU: **`upload_sync`** (uploading composited tiles to the GPU) was the single largest stage — but review found this was mislabeled. It's not GPU bandwidth; an exhaustive check found ~87% of it is a single-threaded **scalar CPU loop** (converting/premultiplying pixel data before upload), not the GPU transfer itself.

### What was tried against that scalar loop, and what actually worked

- **0.89.0–0.89.2**: batched four small writes per pixel into one. Honestly measured: bought almost nothing (~1–5% on the median, no improvement on the tail). The real cost is the arithmetic itself, not the write calls — correctly diagnosed but not yet fixed (SIMD vectorization and cross-tile parallelization with `rayon` are both viable next steps, deferred because they need a real dependency/toolchain decision this project hasn't made yet: `rayon` isn't currently used by any production crate, and portable SIMD needs nightly Rust, which conflicts with this project's stable-toolchain pin).
- **0.90.0–0.90.2**: a different lever entirely — **stop doing the work at all** for tiles that recompute to unchanged content (most visible tiles, in a typical pan, are the same transparent result every frame). This was the first genuinely successful optimization round this session: **GPU-path frame mean dropped ~30% (26–27 ms → 18–19 ms)**, upload volume dropped from 10 MB/frame to 3.4 MB/frame. This is real, reproduced, and currently shipped.

- **0.91.0–0.91.2**: closed the second lever named above — a tile marked "needs re-upload" that got evicted from memory mid-frame under budget pressure used to lose that flag silently on page-in, permanently skipping its GPU upload. Fixed with a small tracking mechanism in the tile store, proven real with a failing test *before* the fix existed, then fixed and re-verified with three separate mutation checks. Review then found the fix itself had a gap — a page-in *failure* (not just an eviction) could still lose the flag forever, with the store reporting false success — reproduced end to end on real GPU hardware and closed with a one-line change. Both bugs are now regression-tested.

- **0.92.0–0.92.2**: vectorized the remaining scalar arithmetic (f16↔f32 conversion + premultiply) using `half`'s own hardware SIMD API — after checking, and rejecting, the `wide` crate suggested as a starting point, since it has no native f16 lane type and would have left the expensive part of the loop untouched. Zero new dependencies. **First genuinely disjoint-range win this session, including on p99** — a ~2.4× cut to the upload-preparation stage on real GPU hardware. Review's exhaustive testing (4.3 billion input combinations) found one narrow real edge case — a NaN-pixel inconsistency between two code paths writing the same GPU texture — fixed by unifying them; the fix investigation then discovered the original bug report's own root-cause explanation was itself subtly wrong (independently reproduced and corrected).

### 0.93.0–0.93.1: diagnosing the recompositing stage itself

The 0.92.x round left `recompositing` as the dominant remaining cost, without saying which of its branches actually spends the time. 0.93.0 added test-only instrumentation splitting that stage's largest internal phase into five per-branch sub-costs (real GPU dispatch issued / GPU dispatch declined / CPU fallback that resolved empty / CPU fallback that did real blend math / everything else), measured on the same real RTX 3090 hardware.

The first measurement was itself wrong in an instructive way. It showed the GPU-path benchmark's dominant cost (~84% of the phase) sitting in "CPU fallback resolved empty," and the round's own PLAN.md entry attributed that to a previously-disclosed double root-walk — but the code's own new instrumentation *also* measured that double walk directly, in a separate slot, at 300× smaller than the number being blamed for it. Independent review (Critic and Red-team, working in parallel by different methods — one by static tracing, one by an actual mutation test on real hardware) caught this immediately, along with a second, more structural problem: the "reconciliation residual" the round leaned on as a self-check for a misplaced measurement mark turned out to be mathematically incapable of detecting one — proven by literally deleting a mark and rebuilding, which left the residual unchanged while silently misclassifying real GPU time as overhead.

0.93.1 fixed both at the root rather than patching the symptom: the fake self-check was replaced with a real one (a per-tile count that a deleted or misplaced mark actually fails, re-confirmed by re-running the same deletion), and the wrong attribution was corrected using the reviewers' own hand-split measurement — the real dominant cost is an unconditional un-premultiply pass over pixels that were never touched, not a repeated root walk. A racy test-only global counter the first pass introduced was also found and removed outright (not just documented) by changing a function's return signature instead. An independent Verifier then reproduced every one of these claims itself, including redoing the deletion test personally, before a Judge scored the corrected round PASS (0.94).

### Where it stands now

- **Mean frame time improved across two rounds (0.90.x and 0.92.x), and for the first time in 0.92.x, p99 (tail latency) improved too** — cleanly, with non-overlapping measured ranges. Tail latency is what determines whether a stroke feels smooth versus stutters, so that was real progress on the actual open question, not just the average case.
- The 60 FPS verdict, honestly stated: **still a miss** on the GPU path's worst-case tail. 0.93.x did not attempt an optimization — it was diagnosis-only, and its actual, corrected finding is that most of the remaining `recompositing` cost is an unconditional per-pixel un-premultiply pass running over tiles with no real content, which is a concrete, evidence-backed target for the next round (not yet attempted).
- The SIMD/rayon decision from an earlier version of this report is resolved for its SIMD half (done, 0.92.x). Rayon (parallelizing across GPU tiles) remains a separate, unstarted, larger lever.
- Only one GPU backend (Vulkan/NVIDIA) has been used for any of this measurement. Metal and DX12 are unverified for the entire GPU-compositing mechanism this work depends on.

## 4. Other open risks, unrelated to performance

- **ADR 0001 (custom `wgpu` UI) is still not de-risked** on two of three platforms. This is the project's most durable open risk and was not touched this session.
- **Real, live correctness/soundness bugs keep surfacing during review of performance work, and keep getting fixed before shipping — this is the process working as intended, not a red flag.** The 0.90.0 round shipped with a *disclosed but unfixed* residual risk; independent review reproduced it end-to-end on real GPU hardware and found a 3-line fix existed. The 0.91.0 round's own fix for a different bug then introduced a third, subtler one (a page-in failure could permanently lose a pending GPU upload) — caught the same way, fixed the same way. The 0.92.0 SIMD round introduced a fifth, narrower one (a NaN-pixel inconsistency between two paths writing the same texture) — same pattern again. A separate NaN-corruption bug (unrelated pre-existing code, surfaced while reviewing an earlier round's safety claims) was found and fixed in the 0.87.x round. The 0.93.0 diagnostic round then shipped a wrong causal conclusion in its own documentation *and* a self-check that looked real but couldn't actually self-check anything — both caught by independent review before the round was called done, and both fixed at the root (not just re-worded) in 0.93.1, with the fix itself independently re-verified by redoing the reviewers' own reproduction steps. All six are now fixed and regression-tested (or, for the 0.93.x documentation error, corrected with the error itself disclosed in PLAN.md rather than quietly overwritten). The pattern across all six: every one was caught by independent review *before* it reached a shipped state, none by a user report after the fact.

## 5. Process note

All of this session's work went through a consistent independent-review discipline: Analyst scopes acceptance criteria → Planner verifies premises against real source and produces an exact plan → Builder implements → Critic and Red-team review independently and in parallel (Red-team with real GPU hardware access, doing actual mutation testing and adversarial reproduction, not just reading code) → an independent Verifier reproduces every claim with real commands → a Reviser fixes anything found → a second Verifier pass → an independent Judge scores the result. Every round in this session that shipped had at least one real, previously-undetected issue caught by this process before landing — including six genuine correctness/soundness bugs (not just style/documentation issues, though those were caught and fixed too) that would otherwise have shipped silently, most recently a wrong causal claim and a non-functional self-check in the 0.93.0 diagnostic round, both caught by independent review and fixed at the root in 0.93.1.

## 6. Recommended next steps, in rough priority order

1. ~~Vectorize the upload-preparation loop~~ — done, 0.92.0–0.92.2 (SIMD via `half`'s own hardware API, not `wide`). Rayon (parallelizing across tiles) remains open but is a separate, larger, unstarted piece of work — needs `TileResidency::sync`'s loop restructured, not just a dependency decision.
2. ~~Fix the eviction-drops-dirty-flag hole~~ — done, 0.91.0–0.91.2.
3. ~~Profile the `recompositing` stage~~ — done, 0.93.0–0.93.1 (diagnosis only, no optimization attempted; see §3). Its corrected, evidence-backed finding: **skip the unconditional per-pixel un-premultiply pass and the composite tile's full-buffer comparison for a tile that resolved with zero real content** — this is now the best-supported next optimization target, not yet attempted.
4. ~~Correct CLAUDE.md's own stale version/test counts~~ — done (this round; CLAUDE.md and README.md now read 0.93.1 / 1,633 tests / ~110,800 lines). The performance table's own numbers (the p50/p99 pan-while-painting figures) are still the pre-0.90.x values and have not been refreshed — worth a follow-up pass once the next optimization round changes them again, rather than updating them twice in quick succession.
5. **Get a human on Linux and Windows accessibility hardware** — this is the one item that could still force a UI-architecture reversal, and it can't be resolved by more agent work.
6. **Act on §3's corrected finding**: skip the wasted un-premultiply/compare work for zero-content tiles. This is the first concrete, hardware-measured optimization candidate to come out of the recompositing-stage diagnosis.
7. Continue the remaining 23 GPU blend-mode ports, following the now-established template — lower risk, well-understood, incremental.
