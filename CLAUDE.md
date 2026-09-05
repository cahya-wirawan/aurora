# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

**A real, running editor — Phase 1 in progress.** As of `0.116.0`: roughly 129,000 lines across 20 crates, 1,776 tests passing, and the full CI gate green. The app opens PNG/JPEG/TIFF and Aurora's own round-tripping `.aur` format; paints and erases real pixels with undo/redo; pans and zooms; handles multiple layers and groups with opacity, masks with real per-pixel grayscale coverage, and all 27 PSD-compatible blend modes composited for real (a GPU fast path for the common case, a CPU path for groups and every other blend mode); and saves the full composite, not just the active layer. Verified interactively on real macOS hardware, including a screen reader announcing the window.

Five crates are still skeletons holding only a placeholder `crate_name()` and one test: `aurora-text`, `aurora-filters`, `aurora-ai`, `aurora-plugin`, and the `aurora-cli` binary. Everything else is real code.

**[PLAN.md](PLAN.md) is the progress tracker** — task-level status per milestone, every `[x]` backed by linked evidence. Check it before starting work, and update the relevant checkbox in the same commit as the work. Its "Where we are" and "Next action" sections are the maintained, current summary; this file's summary is deliberately shorter and goes stale faster, so **PLAN.md wins on any disagreement**. README.md's status paragraph is also kept current, aimed at outside readers.

### What is not done

- **M1.10, the Phase 1 gate, is what is actually open.** Its remaining items are hardware-gated (below), design-owner-gated (which gallery component to build next is Cahya's call, not an engineering one), tooling-gated (the PSD spike needs `psd-tools` as an independent reader), or genuinely large multi-round work (all 26 blend-mode formulas ported to WGSL — Slice 1, proving the shader can read back a former render target as a texture, landed in 0.83.0–0.83.2 for Multiply only, entirely unwired from the running app; wired into the real GPU compositing predicate in 0.84.0–0.84.2, so a real flat (non-grouped) document with `Normal`/`Multiply`/`Dissolve` layers now genuinely composites on GPU — Dissolve needed no new WGSL, since it's resolved to `Normal` before the GPU dispatch ever sees it. A second real mode, `Darken`, landed in 0.85.0–0.85.2, following the same template and confirming it generalizes to a third expressible mode sharing one ping-pong accumulator pair; the two GPU-side compositor methods were then merged into one shared, per-mode-parameterized helper (0.85.1) once the "wait for a third sample" deferral didn't survive its own diff. A third mode, `Lighten`, landed in 0.95.0 and cost exactly what that merge predicted — one WGSL function, one `BlendPass` const, one wrapper, and no change to the shared helper; a fourth, `Screen`, landed in 0.102.0 at exactly the same cost, and was the first whose formula is real arithmetic on both operands rather than a single WGSL intrinsic. That round also had to retarget six test fixtures that had been using `Screen` specifically *because* it disqualified GPU compositing — two of them PLAN.md-tracked CPU-fallback performance benchmarks that would otherwise have gone on passing while silently measuring the GPU path. Verified on Vulkan/NVIDIA only; Metal/DX12 unverified for this mechanism, which matters because the app's own default startup document already carries a `Multiply` layer, so every user's first frame now takes this path. A real, disclosed gap found in 0.85.0/0.85.1 and worth carrying forward: no test can tell "the GPU path genuinely dispatched" apart from "it silently fell back to CPU, which computes the same correct pixels" — closed for `Darken` with a test-only dispatch counter, and for `Lighten` and `Screen` in their own first rounds. **That gap is now closed for all eight GPU-admitted non-`Normal` modes** (0.103.0 for the first five): the three per-mode counter statics were merged into one mode-indexed `GpuBlendDispatches` struct addressed by an exhaustive `match`, and `Multiply` and `Dissolve` — which had no dispatch proof at all — were retrofitted onto it. `Multiply` mattered most of the five, being the arm every user's first frame takes. All eight mutations of that round were killed (seven planned, plus one added when the seventh turned out not to evidence the `Dissolve`-counter-site claim it was meant to prove) — see PLAN.md's 0.103.0 entry for the full account, including which of the five counters is genuinely the *only* thing that notices its mode's arm being deleted and which is not. A fifth mode, `Difference`, landed in 0.104.0 at the same one-entry-point/one-const/one-wrapper/one-arm/one-counter cost, instrumented from its own first round, and needed no fixture retargeting at all (`CPU_ONLY_BLEND_MODE` stays `Exclusion`), so both tracked CPU-fallback benchmarks stayed comparable across that round — unlike `Screen`'s. All seven of its mutations were killed, and deleting its dispatch arm again left every pixel assertion in the app integration test green, confirming the counter is the sole detector a second time on a second mode. A sixth mode, `LinearDodge`, landed in 0.105.0 at that same cost and instrumented the same way, and 0.105.1 closed the one gap its own round disclosed: its app-level fixture now carries a non-opaque `LinearDodge` layer, so a transposed `src`/`backdrop` binding in the *dispatch arm* is caught there and not merely reasoned about. Measuring that one arm at a time showed `Lighten`, `Screen` and `Difference` still had no such coverage; 0.105.2 closed that too, by the same mechanism (each of those three fixtures' top layer moved from opacity `1.0` to `0.5`, goldens re-derived), so **all six blend-math dispatch arms now carry transpose coverage** — none of them from formula asymmetry, since all six formulas are commutative, and each of the three new mutations was really run rather than reasoned about. That round changed no shader, wrapper, dispatch arm or predicate, so it moved no mode count; it also added a standing headless guard (`every_gpu_blend_math_dispatch_arm_has_a_fixture_that_could_see_a_transposed_argument`) so the class needs no third hand discovery. A seventh mode, `LinearBurn`, landed in 0.106.0 at the same one-entry-point/one-const/one-wrapper/one-arm/one-counter cost — the exact mirror image of `LinearDodge` (same sum, opposite offset, opposite clamp direction, three characters apart), so its shader was derived from `blend_channel`'s own Rust arm rather than copied from the sibling entry point. It was the first mode ported *after* that guard existed, which made its fixture's non-unit opacity a precondition of the round rather than a retrofit — and the guard passed on the first attempt. It needed no fixture retargeting (`CPU_ONLY_BLEND_MODE` stays `Exclusion`), so both tracked CPU-fallback benchmarks stayed comparable. Its round also found a genuinely new, mode-specific degeneracy worth carrying forward: in an *unclamped* channel `Cb + Cs - 1 == |Cb - Cs|` exactly when either operand is `0.5`, so such a channel cannot distinguish `LinearBurn` from `Difference` — which is itself on the GPU, making that a live wrong-arm candidate rather than a hypothetical one. An eighth mode, `ColorBurn`, landed in 0.107.0 at that same Rust-side cost but is the first whose *shader* needed more than one changed line: its formula is a three-branch guarded division (`Cb == 1 -> 1`, then `Cs == 0 -> 0`, then `1 - min(1, (1 - Cb) / Cs)`), so it factors into a `color_burn_channel` helper called once per channel rather than one componentwise `vec3` expression. It is also the first ported mode that is **not commutative**, which retires the premise 0.105.3's standing transpose guard was built on: a transposed `src`/`backdrop` dispatch arm is now caught by the blend term itself, confirmed by running that mutation at opacity `1.0` where every prior mode's would have survived (the guard is deliberately left un-special-cased — non-unit opacity is now sufficient-but-not-necessary for this one mode, which makes it a false *alarm* risk and never a false all-clear). Its round also produced the sharpest form yet of the Metal/DX12 gap, and disclosed it rather than papering over it: both of its guards are arithmetically redundant under IEEE-754, deleting the `Cb == 1` one is killed deterministically, but **deleting the `Cs == 0` one survived every test in both crates**, because this adapter divides by zero to `+inf` and `1 - min(1, inf)` is exactly the `0.0` the guard returns. WGSL specifies an *indeterminate value* there, not `+inf`, so that guard is a portability guard that no test on this hardware can exercise — for every prior mode an unverified backend could only differ in rounding; here it could differ in a *value*. A ninth mode, `ColorDodge`, landed in 0.108.0 at the same cost as `ColorBurn` and is its structural mirror — a `color_dodge_channel` helper implementing `Cb == 0 -> 0`, then `Cs == 1 -> 1`, then `min(1, Cb / (1 - Cs))` — and is the second non-commutative mode ported, confirmed to be caught by the blend term itself at opacity `1.0` and not just at its fixture's real `0.5`, exactly as `ColorBurn`'s was. Its round found the same asymmetric guard-redundancy split (`Cb == 0` killed deterministically; `Cs == 1` survives on this adapter for the identical `+inf`-vs-indeterminate-value reason `ColorBurn`'s `Cs == 0` guard did) and two mode-specific degeneracies worth carrying forward: `ColorDodge` and `LinearDodge` clamp under the exact same condition, so a clamped channel can never distinguish the two; and `ColorDodge(Cb, Cs) == Cs` whenever `Cb == Cs * (1 - Cs)`, making that channel indistinguishable from `Normal`. It also found and fixed a real pre-existing defect from `ColorBurn`'s own round: six comments across both crates had mistranscribed `ColorDodge`'s formula with `ColorBurn`'s spurious outer `1 -` (0.108.0 fixed all six but counted only five, omitting `aurora-app`'s `begin_gpu_composite_tile` dispatch-arm comment; 0.108.1 corrected the count, and PLAN.md's 0.108.0 entry now enumerates all six). 0.109.0 ported no new mode — it discharged a twice-deferred refactor, extracting the boilerplate shared by all nine blend-math WGSL entry points (texture sampling aside, which needs `in.uv`) into two helpers, `straight_backdrop()` and `fold_over()`, proven byte-identical to the code they replaced rather than merely equivalent. Its own mutation matrix found something worth carrying forward: deleting the un-premultiply guard is caught by only 4 of the 10 modes' tests (`Multiply`/`Screen`/`Difference`, and — once it landed — `Overlay`), because the other 6 formulas contain a `min`/`max` that launders the resulting NaN into a finite value before it ever reaches the output — confirmed decisively in 0.109.1 by forcing a `bd`-independent NaN through the guard and probing `min(NaN, x)`/`max(NaN, x)` directly on this adapter, both returning the non-NaN operand. That makes the other 6 modes' guard-loss *output-equivalent on this backend*, not merely undetected — no fixture retargeting can fix it, since the mutant is genuinely correct here, and WGSL/SPIR-V leaves `FMin`/`FMax` on a NaN operand undefined, so this is not a portability guarantee. A tenth mode, `Overlay`, landed in 0.110.0 at the cheapest cost yet, thanks to that same 0.109.0 refactor: one `let b = ...` line and the two shared helpers around it. It is the first mode to use WGSL's componentwise `select()` instead of a per-channel helper, legitimate here (unlike `ColorBurn`/`ColorDodge`) because neither of its branches divides — evaluating both, which `select()` does, risks nothing. It joins `Multiply`/`Screen`/`Difference` as a fourth guard-deletion detector (its formula has no `min`/`max` to launder a NaN), and is the third non-commutative mode, but *conditionally* so: it agrees with `HardLight` wherever both operands share a side of `0.5`, differing only where they straddle it — and the two branches agree bit-exactly at `Cb == 0.5` itself, making a `<=`-vs-`<` mutation there provably unkillable, confirmed by an exhaustive sweep of every f16-representable value, not just disclosed. An eleventh mode, `HardLight`, landed in 0.111.0 at the cheapest cost yet and is `Overlay`'s own exact transposed twin (`Overlay(Cb,Cs) = HardLight(Cs,Cb)`) — the first round where the mode being ported and its mirror image were *both* already real, shipped GPU arms, making the wrong-arm hazard bidirectional for the first time (a transposed dispatch arm, a branch on the wrong operand, or a `fragment_entry` typo naming the sibling all now produce the same live wrong answer). It branches on the *source* rather than `Overlay`'s backdrop, joins the guard-deletion detectors as a fifth, and its own mutation matrix found a genuinely new, disclosed nuance: one specific mutation (swapping which operand comes first in its low branch's multiply) is provably unobservable — not merely untested — because IEEE-754 float multiplication is bit-exactly commutative there, confirmed by both an exhaustive brute-force sweep and an independent analytical proof. A twelfth mode, `LinearLight`, landed in 0.113.0 and is the first port in three rounds whose shader is *structurally simpler* rather than more complex: unlike `Overlay`/`HardLight`, its CPU arm is a single unconditional expression (`clamp(Cb + 2*Cs - 1, 0, 1)`) rather than a two-call delegation, so it needed no `select()`, no branch, no branch-boundary test and had no unkillable `<=`-vs-`<` mutation to disclose. It is the first `clamp()` in the shader file, and that turned out to matter twice. First, WGSL specifies float `clamp` as `min(max(e1, e2), e3)`, so it *launders* a NaN exactly as the six min/max modes do — this round predicted a **non**-detection of `straight_backdrop`'s guard removal (the first time 0.110.0's rule was used that way) and then measured it holding, leaving the detector count at five. Second, and the real finding of the round: the mode's blend term is *unconditionally* asymmetric (`B(Cb,Cs) - B(Cs,Cb) = Cs - Cb` pre-clamp, putting it in `ColorBurn`/`ColorDodge`'s class rather than `Overlay`/`HardLight`'s conditional one), and yet a transposed dispatch arm is **not** always observable — the two laundering mechanisms are exactly complementary. A clamp-*railed* channel is blind at effective alpha `1.0`, while a clamp-*interior* channel is blind at exactly `0.5` (`out - out_transposed = (Cb - Cs)*(1 - 2a)`), which is precisely the opacity the standing transpose guard demands. So for the first time that guard's demand is *insufficient on its own*, and the fixture had to carry all three clamp regimes instead; the transpose was then measured caught in two channels at `0.5` and one at `1.0`. Its most dangerous near-miss is arithmetic rather than structural: dropping the `2.0 *` computes `LinearBurn` **exactly** (the upper clamp bound is unreachable for operands in `[0, 1]`), and `LinearBurn` is itself a live GPU arm. No fixture retargeting was needed (`CPU_ONLY_BLEND_MODE` stays `Exclusion`), so both tracked CPU-fallback benchmarks stay comparable. A thirteenth mode, `VividLight`, landed in 0.114.0 and is the first whose blend term is built entirely out of *two other already-ported modes*: its WGSL helper computes nothing but the `2*Cs` / `2*Cs - 1` substitutions and delegates to `color_burn_channel` and `color_dodge_channel`. Deliberately three per-channel calls rather than a componentwise `select()` — both callees' guards are *early returns* keeping a division out of the lanes where its divisor is zero, which a `select()`'s evaluate-both-arms semantics would give up. Three things about it are worth carrying forward. It is the **first mode to inherit both guarded-division modes' portability gaps at once**: `color_burn_channel`'s `cs == 0` and `color_dodge_channel`'s `cs == 1` guards each survive every test here, because this adapter divides by zero to `+inf` where WGSL specifies an indeterminate value — so on Metal/DX12 this one mode could differ in a *value* twice over. Its `<=` branch boundary is **unkillable at this round's own test tolerance** (both arms give `Cb` at `Cs == 0.5` for every `f16`-representable `Cb` in `[0, 1]`, guard points included, confirmed by sweeping all 15,362 such values rather than probing three points), the third such disclosure after `Overlay`/`HardLight` — though not unkillable in principle, since the two arms' `1 - (1 - Cb)` and `Cb` genuinely diverge by up to `2.98e-8` for a non-`f16`-exact `Cb` (the kind `straight_backdrop`'s own division over a non-opaque accumulator produces) and diverge outright once `Cb > 1`, which an unnormalized accumulator can yield since `straight_backdrop` does not clamp. Its asymmetry is unconditional and structural (it branches on the source, so a transposed pair branches on the backdrop), but an early draft of this round wrongly reasoned from "the blend term is not affine in its operands" to "there is no blind opacity at all" — `out - out_transposed = (1-a)*(Cb-Cs) + a*(B(Cb,Cs)-B(Cs,Cb))` is affine *in `a`* for every mode regardless of `B`'s own shape, so a blind alpha exists in `(0,1)` wherever those two differences carry opposite signs, and 0.114.1 found thousands of such `VividLight` operand pairs on a grid search, including several within measurement tolerance of exactly `a = 0.5`. The round's own shipped fixture turns out not to be one of them — each of its three channels has a real blind alpha in `(0,1)`, but none lands at `0.5` or `1.0`, so the transpose is genuinely caught at both tested opacities — but that is a fact about this one fixture, not a property of the mode, which is why 0.113.1's computational transpose assertion is *necessary* here and not merely redundant defense in depth. The round also shipped **one wrong prediction, caught only by really running the mutation**: a `2*Cs`-for-`2*Cs - 1` slip in the dodge branch was predicted to rail that branch to a constant `1.0` via the `cs == 1` guard and so be hard to detect; it in fact drives the divisor `1 - 2*Cs` *negative* across the whole branch, emits negative colour, and every dodge channel kills it. Every affected comment was corrected rather than quietly dropped. A fourteenth mode, `HardMix`, landed in 0.115.0 and is the first whose blend term is one *whole other ported mode's* per-channel helper plus a comparison — `hard_mix_channel` calls `vivid_light_channel` and thresholds it at `0.5`. Its round's real content is a closed form, derived and then verified exhaustively rather than asserted: both of `VividLight`'s arms cross `0.5` at exactly `Cb + Cs == 1`, so **`HardMix(Cb,Cs) = 1[Cb + Cs >= 1]` everywhere except the single point `(0,1)`, where it is `0`** — a guard-ordering artifact, `color_dodge_channel` testing `cb == 0` before `cs == 1`. A `1/256`-grid sweep plus an edge refinement found exactly that one exception and exactly two asymmetric operand pairs, `(0,1)` and `(1,0)`. Three things follow and all three were *measured*, not reasoned about. The blend term is symmetric in every channel but that corner, with **no interior straddling pair at all**, so a corner channel is the only thing that can catch a shader-internal operand transpose or a bare sum-threshold rewrite — `NORMAL_MULTIPLY_HARD_MIX_STACK` is therefore the first roster fixture not to reuse the family's shared bottom layer, and both of those mutations were killed by exactly the two corner-carrying fixtures with the other six green. Its `< 0.5` threshold is the **first branch comparison on this path whose direction is killable** (`Overlay`, `HardLight` and `VividLight` each had to disclose an unkillable `<=`-vs-`<`), because its two arms are the constants `0.0` and `1.0` rather than two formulas that agree at the boundary. And it is a *total* NaN launderer by an argument no prior non-detector has: besides the inherited `min()`, `NaN < 0.5` is `false` on every backend under IEEE-754, so it cannot return a non-finite value — though which mechanism actually saves it here is **not** measurable, since `fold_over`'s `ab == 0` erases the term either way. Two mode-specific indistinguishabilities are disclosed in the tests themselves rather than designed around: every `B = 0` channel provably agrees with `ColorBurn` and every `B = 1` channel with `ColorDodge`, and a corner channel agrees with *both*, so no three-channel corner fixture can separate both grand-callees; and because `B` is only ever `0` or `1`, an output clamp and a clamped source alpha give the same texel. The round also corrected two of its own predictions by really running the mutations — a transposed *dispatch arm* fails only the app differential, **not** the standing transpose guard (that guard is a static check on the fixture roster and can never see the real arm mutated, despite its name), and the `<=` mutation is also killed by the spatially-varying-tile fixture — and one pre-existing drift found rather than looked for: the predicate's own header sentence had said "fourteen modes ... no fifteenth" against a fifteen-bullet list since 0.114.0. 0.115.1 closed a real coverage regression review found: the app-level test named for covering "every expressible mode" had a hand-maintained mode list that silently omitted both `VividLight` and `HardMix` — `VividLight`'s omission predated this round by one, `HardMix`'s was introduced by it — plus six disclosure defects, the sharpest being that WGSL's 2.5 ULP float-division tolerance is, for the first time, amplified by a *discontinuous* blend term into a potential full-channel GPU/CPU divergence near the mode's own `Cb + Cs == 1` boundary, closed by an exhaustive sweep of the entire `f16`-representable domain (not a grid sample) confirming the one-exception closed form holds there exactly. A fifteenth mode, `PinLight`, landed in 0.116.0 and is the cheapest port since `Overlay` — its `blend_channel` arm delegates to `Darken` and `Lighten`, whose arms are a bare `min` and `max`, so the shader is three lines and no per-channel helper, and a componentwise `select()` is legitimate here for `Overlay`/`HardLight`'s reason (neither arm divides) rather than needing `VividLight`'s three-call shape. Its round's real content is its **blind set**, brute-forced over a 129x129 rational grid rather than sampled: writing `D0 = Cb - Cs` and `D1 = B(Cb,Cs) - B(Cs,Cb)`, the blend term is symmetric on exactly four classes with **zero grid members outside them** — `Cb == Cs`, **`|Cb - Cs| == 0.5`**, one operand `0` with the other `<= 0.5`, and one operand `1` with the other `> 0.5`. The second of those is **a hazard class no previously ported mode has**, and it is the thing to carry forward: at that separation the low `min` and the high `max` land on the same value from either operand order, and `(0.25, 0.75)` and `(0.5, 1.0)` are both in it — precisely the operands a fixture reaches for by habit. Only `Cb == Cs` is blind at *every* alpha; the other three are blind at `a = 1` and visible at `a = 0.5`, and getting that qualifier wrong is the one framing error this round had to correct in its own plan (the unqualified version reads as reassuring and is false). Blindness at `a = 0.5` specifically holds at exactly two pairs, `(0,1)` and `(1,0)` — the same corner `HardMix` has, reached here without any guard ordering. The round also **resolved, by measurement, a question three prior rounds could only disclose**: `PinLight`'s `<= 0.5` branch comparison *is* killable, making it the second such mode after `HardMix` and the first whose kill needs an operand no in-gamut fixture can supply. In gamut the two arms agree exactly at `Cs == 0.5` (`min(Cb,1) = Cb = max(Cb,0)`), which is why `Overlay`'s, `HardLight`'s and `VividLight`'s equivalents are unkillable — but `straight_backdrop` divides without clamping, so an accumulator whose premultiplied `rgb` exceeds its own `a` yields `Cb > 1` (an unclamped float-TIFF import, legal under invariant §7.3.1b), and there they diverge; a fixture carrying `Cb = 1.5` on the boundary killed the mutation, alone out of 644 tests. It is **not** a sixth `NaN`-guard detector — predicted from 0.110.0's rule and then measured, with the honest caveat that unlike `HardMix`'s backend-independent argument this one rests wholly on this adapter's undefined-by-WGSL `min`/`max` behaviour, unsurprising since the mode *is* `Darken` and `Lighten` behind a branch. Two further findings are disclosed rather than designed around: every backdrop-wins channel (`B == Cb`) *provably* agrees with `Darken` in the low branch and `Lighten` in the high one, both live GPU arms, so every fixture must carry a source-derived channel per branch; and the shared `patterned_texels` spatial fixture turns out to be **systematically blind to a `Darken` substitution** for this mode (its two seeds differ by one quarter-step in R and G and not at all in B, and `PinLight` equals `Darken` on all four resulting pairs), measured by three separate mutations leaving it green while killing six or seven other tests. All sixteen of the round's mutations were really run on real hardware; the one nothing catches is deleting the mode's entry from the hand-maintained "every expressible mode" list — the same gap 0.104.0 and 0.115.1 each had to fix, now measured rather than argued, and disclosed rather than closed. **10 of 27** blend modes are still CPU-only at the app's GPU predicate (11 of 26 at the `aurora-render` shader level have no dedicated blend-math WGSL entry point — `Normal` is among them and needs none, since it composites via a separate fixed-function path, so that figure is not a CPU-only count; `Dissolve` is in neither set), and group support remains). Mask *coverage* is real per-pixel grayscale as of 0.70.0 and survives a `.aur` save/load round trip as of 0.71.0; what is still missing there is a brush/tool UI for painting a mask and mask-pixel undo/history proper. Mask-surface lifecycle cleanup is now mostly closed at the library level (0.80.0–0.81.1): a discarded document's tiles (content and mask, including every subtree on the undo/redo stacks and the crash-recovery journal) can now be freed, and re-adding a mask to a layer whose old one was removed no longer resurrects the old coverage — though undoing back *past* that re-add can still restore an old mask reading the newer one's coverage, an accepted, tested residual of the same derived-surface-id root cause. None of this is reachable through the shipped app yet (no UI calls any of it) — see PLAN.md's M1.9 mask entry for the full account.
- **Phase 0 has a tail.** Windows/DX12 validation, the Linux and Windows human legs of the accessibility/IME checklist, and macOS/Windows LGPL packaging all need hardware and a human.
- **[ADR 0001](docs/adr/0001-custom-wgpu-ui.md) is still not de-risked** — the project's most durable open risk. macOS accessibility passes 9/10 ([spike/a11y-ime/FINDINGS.md](spike/a11y-ime/FINDINGS.md)); Linux (AT-SPI) and Windows (UIA) are entirely unverified, and each is a different platform API. Cahya accepted this as a risk rather than a blocker for starting Phase 1 (2026-07-28). It remains the one open item that could overturn the custom-`wgpu`-UI decision.
- **60 FPS is measured and failing** — see "Measured, not assumed" below.
- **Not started at all**: PSD/PSB in the app (a feasibility spike only, not wired in), filters and adjustments, selection tools beyond a raw data model, smart objects, RAW import, plugins, scripting, AI, and true infinite pasteboard panning (panning deliberately stops at the document's own edge).

### Spikes

Five, all outside the workspace so they can never become dependencies of real code: `vertical-slice` (performance, [FINDINGS](spike/FINDINGS.md) — read before touching tile, render, or brush code), `a11y-ime`, `psd-write`, `raw-icc`, `lgpl-packaging`.

Two constraints from the a11y spike still shape real code: **windows must be created hidden, adapted, then shown** (`accesskit_winit` panics otherwise — this is why `aurora-app` manages windows the way it does), and the text stack sets the toolchain floor (`cosmic-text` needs ≥1.89, which is why the pin is 1.97).

### The lesson from the last round

The first live interactive session on real hardware (2026-08-12/13) found four bugs — stroke slowness, paint offset from the cursor on Retina, sub-tile pan doing nothing, pan freezing at the edge — that 900+ passing tests, a green CI gate, and a headless software-Vulkan sandbox had all missed. GPU tests self-skip when no adapter is present, and a typical Linux dev box here has only Mesa llvmpipe software rendering. **A green test run is not evidence that canvas or UI work is correct.** Say so plainly rather than implying verification that did not happen, and expect anything involving DPI, real GPU timing, or interactive feel to need a human on real hardware.

## Commands

```sh
cargo build --workspace                 # build everything
cargo test --workspace                  # all tests
cargo test -p aurora-tile               # one crate
cargo test -p aurora-tile -- name_of_test   # one test
cargo nextest run --workspace           # what CI runs (faster, better output)

cargo fmt --all                         # format
cargo fmt --all --check                 # CI check
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 scripts/check_layering.py       # crate layering rule (PRD §7.2)
python3 scripts/check_no_hardcoded_style.py  # no literal colours/sizes in widget code (FR-027)
cargo deny check all                    # licences + advisories
cargo doc --workspace --no-deps --open  # docs

python3 design/check_contrast.py        # every token pair, every built-in theme
cd design && python3 build_tokens_css.py > tokens.css   # regenerate the gallery's CSS

cargo run -p aurora-app                 # the application
cargo run -p aurora-cli                 # headless binary
```

**Hard precondition, not a suggestion: before a full-workspace build, gate run, or real-GPU verification pass, confirm `df -h /home` (or wherever this checkout lives) shows at least 25G free.** Five separate rounds now (0.102.1, 0.103.0, 0.103.1, 0.104.0, 0.104.1) hit `/home` running tight mid-gate, each recovered the same way: `rm -rf target/debug/incremental` (a pure build cache, regenerates automatically; nothing else under `target/` needs touching). This is no longer noise — it is a structural property of this workspace's size against real-hardware GPU test runs, and the one failure mode it risks (an `ENOSPC` mid-run against real GPU hardware) is exactly the evidence a round cannot substitute for. Run the cleanup *before* the gate, not after discovering the failure mid-`cargo test`.

The spikes are separate crates, deliberately outside the workspace (root `Cargo.toml` `exclude`s them) so they can never become dependencies of real code:

```sh
cd spike/vertical-slice
cargo run --release -- --headless       # the benchmark; no display needed
cargo run --release                     # windowed, drag to paint, Esc for stats
```

The full CI gate locally, in the order CI runs it:

```sh
cargo fmt --all --check && python3 scripts/check_layering.py \
  && python3 scripts/check_no_hardcoded_style.py \
  && cargo check --workspace --locked \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace
```

`cargo check --workspace --locked` looks redundant next to the two lines under
it and is not. Every other compile step in the gate passes `--all-targets`
and/or `--all-features`, so none of them build the plain configuration Aurora
ships. `aurora-doc`'s `test-support` feature (this workspace's first Cargo
feature) gates a test-only escape hatch, and a non-test `pub fn` in a shipping
crate calling it was measured to pass `cargo build --workspace --all-targets`
*and* `cargo check --workspace --all-features` while failing only a flagless
build. That step is what makes such a leak fail the gate.

Neither design script is in CI, and `design/tokens.css` is a review aid for the
HTML mockups and gallery — Aurora itself never uses CSS. The TOML files under
`design/tokens/` and `design/themes/` are the source of truth; regenerate after
any token edit rather than hand-editing the CSS. (It went un-regenerated from the
original scaffold through 0.48.0, because the generator crashed on the scalar
`border.control_opacity` 0.18.0 added — fixed in 0.48.1.)

`cargo nextest` is not always installed locally — `cargo test --workspace` is an
acceptable stand-in, and CI additionally runs `cargo test --workspace --doc`
(nextest does not run doctests) plus `cargo doc --workspace --no-deps
--all-features --document-private-items --keep-going`, so a broken intra-doc
link fails CI even when every test passes. `--document-private-items` is
load-bearing, not decoration (0.106.2): these crates are almost entirely
private, so without it rustdoc never reads most of this workspace's doc
comments — 21 broken links across 6 crates had accumulated behind that gap.
`--keep-going` is too: `cargo doc` otherwise stops spawning units at the first
failure and prints only the crates already in flight, which reads exactly like
a complete list and is not one. That job runs on Linux only, so a doc comment
*on* an item gated to another OS is still read by no CI run at all.

GPU-backed tests self-skip when no adapter is present, and print `SKIPPED` rather
than failing. A dev box with only Mesa llvmpipe still runs them, but in software
— see "The lesson from the last round" above before treating those numbers, or
those passes, as evidence about real hardware.

Set `AURORA_REQUIRE_GPU` to turn that self-skip into a hard test failure, so a
runner that is *supposed* to have a real adapter cannot go green while every
GPU-gated test silently skips. With it set, two things fail the test: no adapter
at all, **and** an adapter `wgpu` reports as `DeviceType::Cpu` — llvmpipe here,
WARP / "Microsoft Basic Render Driver" on Windows — since a software rasterizer
standing in for hardware is the same gap wearing a different hat. Unset it is a
complete no-op, `SKIPPED` line included, and a CPU adapter stays perfectly fine
for ordinary dev-box runs. Parsing: unset is off, and so are the present-but-falsy
values `` (empty), `0`, `false`, `off`, `no` (trimmed, case-insensitive) —
GitHub Actions sets a variable to the empty string when a `${{ }}` expression is
empty, and treating that as "on" would fail a whole matrix for a reason the
workflow file never states. Any other present value is on.

**No regular push/PR workflow sets it yet** — which runner, if any, should is
still an open decision, but `.github/workflows/gpu-probe.yml` (0.62.0) is a
manual, `workflow_dispatch`-only job that runs the workspace's GPU-gated
tests on `macos-latest` with it set, specifically to find out whether that
runner has a real adapter before committing either way — see PLAN.md for
how to read its result. The single implementation is
`aurora_gpu::test_support::real_context_or_skip` (behind `aurora-gpu`'s
`test-support` feature); every crate's `real_context()`/`real_gpu_context()`
helper delegates to it, and it prints the selected adapter's name, backend and
device type on every successful creation so a CI log records what was actually
tested. What it asserts is that a real adapter *exists*, not that any particular
test body ran — an `#[ignore]`d test still contributes a silent pass.

Toolchain is pinned in `rust-toolchain.toml` (1.97, edition 2024 — `cosmic-text` requires ≥1.89, so the text stack sets the floor). CI runs on Linux, macOS, and Windows from the first commit — cross-platform breakage is cheap to fix now and catastrophic in month 30.

## Lints worth knowing

The workspace denies `unwrap`, `expect`, `panic`, and `indexing_slicing` (root `Cargo.toml`). This is deliberate: Aurora holds a professional's unsaved work, and a panic loses it. Return errors instead. A crate needing `unsafe` must override `unsafe_code` in its own `[lints]` table rather than the workspace's, so the exception is visible in review. **`aurora-tile` is the one exception** (0.61.0): `store::create_private_dir`'s scratch-directory hardening needs `fchmod` on an open descriptor and `geteuid` for an ownership check, neither exposed by `std`, only by `libc` FFI. Its `[lints]` table copies every other workspace lint verbatim and overrides only `unsafe_code` to `"allow"`; every other crate is still `unsafe_code = "deny"` — keep it that way if you can.

## Versioning

SemVer, started at `0.0.1`, currently `0.116.0`. The single source of truth is `[workspace.package].version` in the root `Cargo.toml`; every crate inherits it via `version.workspace = true` — bump it in exactly one place. The commit subject carries the new version in parentheses, e.g. `Clamp canvas pan to the document's own top-left edge (0.47.1)`.

- **Minor** (`0.X.0`): every PLAN.md step — a task-level unit of work landing in its own commit (the same granularity PLAN.md's own checkboxes track).
- **Patch** (`0.0.X`): a bug fix — correcting something that was already landed and wrong, not new work.
- Bump the version in the same commit as the work it covers, the same discipline PLAN.md's own checkbox updates already follow.

A release is a `vX.Y.Z` tag (e.g. `v0.1.0`) pushed once the matching version bump is committed — pushing that tag is what actually publishes a GitHub Release, so treat it as a deliberate, user-approved action, not a routine one. `.github/workflows/release.yml` re-runs the full gate (`verify.yml`, the same jobs `ci.yml` runs on every push/PR) against the tagged commit, confirms the tag matches `Cargo.toml`'s own version, then creates the Release. Ordinary commits and PRs are still checked immediately via `ci.yml` regardless of tags — CLAUDE.md's own "cross-platform breakage is cheap to fix now" principle applies to every commit, not just tagged ones.

## What Aurora is

A cross-platform, GPU-accelerated, non-destructive professional image editor (a Photoshop alternative) for Windows, macOS, and Linux. PSD/PSB compatibility and AI-assisted editing are first-class requirements, not add-ons.

## Architecture (PRD §7)

A single Cargo workspace with crates layered so dependencies point **downward only** — `core` → `tile` → `graph`/`gpu` → `render`/`doc` → feature crates (`filters`, `brush`, `vector`, `text`, `io`, `ai`, `plugin`, `theme`) → `widgets` → `ui` → `app`/`cli`. PRD §7.2 has the full table. A lower crate must never depend on a higher one; CI enforces this.

`aurora-widgets` (the general-purpose toolkit) knows nothing about documents or layers and must stay headlessly testable; Aurora-specific panels belong in `aurora-ui`. Keep that seam sharp.

The allowed dependency map lives in `scripts/layering.json` and is checked by `scripts/check_layering.py`. If the checker rejects a dependency, the fix is almost always to move shared code *down* the stack — editing the JSON is an architecture decision, not a build fix.

Decisions with lasting consequences are recorded in [docs/adr/](docs/adr/). Read those before revisiting the UI toolkit, document ceiling, precision floor, or PSD scope; each records what would justify reopening it.

### Invariants (PRD §7.3)

These are load-bearing — each one backs a headline requirement, so treat them as rules:

1. Nothing assumes a document fits in memory; all pixel access goes through the tile store. Ceiling is 300,000 × 300,000 px (matching Adobe PSB) — one layer at half-float RGBA is ~720 GB.
1b. No 8-bit intermediates. The pipeline is ≥16-bit float end to end: `f16` tile storage, `f32` compute. 8-bit appears only at import (promoted immediately) and export (quantized with dithering). An 8-bit buffer inside the graph is a bug — the banding is invisible in review and unrecoverable downstream.
2. Edits are non-destructive: adjustments/filters/smart objects are render-graph nodes, never baked pixels.
3. History stores reversible operations plus dirtied tiles, not snapshots.
4. The UI thread never blocks on rendering — rendering is async and progressive.
5. Brush input bypasses the general graph (a scratch layer), or the 10 ms budget is unreachable.
6. Every buffer carries its color space; untagged data is an error, not a default.
7. Plugins are untrusted — sandboxed, no raw pointers into document memory.
8. UI and canvas share one GPU device and one frame — not separate surfaces composited together.
9. Every widget carries an `accesskit` node as part of its definition. Aurora renders its own UI, so nothing is accessible for free; a widget without one is incomplete.
10. No style value is hardcoded. Widgets resolve colors, spacing, sizes, radii, and durations from semantic design tokens in `aurora-theme` (FR-027) — never a literal, never by reading a *theme*. CI lints for this. A hardcoded color is a bug: it's the one thing a user's theme cannot override.

Note: `aurora-core` and `aurora` are taken on crates.io by unrelated projects (PRD §12 Q2b). Harmless — the workspace uses path dependencies and nothing needs publishing — but these crates cannot be published under those names as-is.

Licensed MIT. Two practical consequences when adding dependencies: `cargo deny` enforces an allowed-licence list in CI, and copyleft C libraries (LibRaw is LGPL-2.1/CDDL, `libheif` is LGPL) must be dynamically linked to satisfy LGPL — prefer a pure-Rust alternative where one is viable. See PRD §14.

## Technology stack (PRD §8)

Rust end to end (edition 2024, stable). `wgpu` + WGSL for GPU across Vulkan/Metal/DX12; `winit` for windowing and tablet input; `rayon` for CPU tile parallelism; `tokio` for I/O and background work but **not** the render loop. FFI wrappers are acceptable where Rust lacks maturity (RAW, ICC).

Two deliberate changes from the original C++ plan: plugins are **WASM via `wasmtime`** (native dylibs can't meet the sandbox requirement), and scripting is **Lua in-process + Python out-of-process over IPC**, with the JavaScript API deferred.

**UI: Aurora builds its own retained-mode widget toolkit on `wgpu`** (PRD §8.3) — no third-party toolkit. Supporting crates: `cosmic-text` (shared by UI fields and canvas text), `winit` (input + platform IME), `accesskit` (accessibility), `rfd` (native dialogs), `aurora-vector` (resolution-independent UI geometry).

The consequence to keep in mind when writing UI code: text editing, IME, accessibility, DPI scaling, native menus, drag & drop, and clipboard are **our** work, not inherited. They are Phase 1 scope and gate the phase — don't defer them as polish.

**Visual design and theming are a Must requirement** (PRD FR-027), not polish. Themes are declarative TOML files with semantic tokens, hot-reloaded, inheriting from built-ins — users restyle Aurora without touching code, so themes are data and never executable. Built-ins: Dark (default), Light, two high-contrast, and a neutral Color-Critical theme for color-accurate work. Density (Compact/Comfortable/Spacious), UI scale, accent, and icon set are independent axes.

When adding a widget, it isn't done until it: resolves all styling from tokens, exposes an accessibility node, appears in the component gallery in every state, and passes the contrast check in every built-in theme.

Design owner is Cahya Wirawan (PRD FR-027 *Ownership*) — token vocabulary, scales, and colour decisions are theirs. Don't invent tokens ad hoc when implementing a widget; if one is missing, that's a design decision to raise, not a gap to fill locally. If accessibility or IME proves unworkable in practice, the contained fallback is CXX-Qt for chrome only, which is why `aurora-ui`'s widget API stays free of `wgpu`-specific assumptions.

## Performance budgets that constrain design

From PRD §6 and §10 — these drive implementation choices rather than being measured afterward:

- Startup < 3 s; brush latency < 10 ms; 60 FPS canvas interaction.
- Open a 2 GB PSD in under 5 s (implies lazy/streaming parsing, not a full load).
- Unlimited layers and history — storage must be incremental and compressed.

Note these budgets are set at 8 bytes/px (half-float RGBA), which is 2× an 8-bit pipeline. Tile compression is mandatory, not an optimization.

### Measured, not assumed (spike/FINDINGS.md, 2026-07-26)

One GPU (Radeon Pro 5300M, Metal), so treat as indicative rather than settled — but these are real numbers and they changed the design:

- **Stroke latency p99 9.1 ms against a 10 ms budget.** Under 1 ms of margin. A latency regression test now exists in CI; do not assume the margin holds as the brush engine grows.
- **CPU compositing is the bottleneck, not disk I/O** — the opposite of what was assumed. Page-in panning runs at 7 ms; merging whole tiles costs ~20 ms. So: `aurora-tile` needs **per-tile dirty rectangles**, and compositing belongs on the **GPU** with the CPU path as fallback.
- **Upload bandwidth caps pan speed** (~18 MB per screenful). Render a lower mip while panning and refine when motion stops — this is what the progressive-rendering requirement is for.
- Invariants §7.3.1 and §7.3.8 hold; half-float round-trips bit-exact.

### The live app, measured end to end (0.39.0, PLAN.md M1.10)

The numbers above come from the throwaway slice. The real app has since been
measured on its own path — brush → composite → tile upload → render pass →
present — at the full 300,000 × 300,000 px ceiling, and **it misses the 60 FPS
budget**:

| Path | Budget | p50 | p99 |
|---|---|---|---|
| Pan while painting, GPU composite | 16.7 ms | 35.4 ms | **98.8 ms** (~5.9×) |
| Same, CPU fallback (smaller viewport) | 16.7 ms | 22.6 ms | **54.1 ms** (~3.2×) |

**This table is the 0.39.0 figure and is now well behind the code.** The same
two benchmarks have since been re-measured repeatedly on a real RTX 3090. On an
**idle** machine the GPU path's mean frame is ~7.5–8.6 ms with a worst-of-40 in
the 21–30 ms range. **Do not read that as the whole picture: every one of those
numbers is measured with the benchmark's caller thread otherwise idle, which
flatters it.** Re-measured with 8 competing CPU-bound threads — ordinary
desktop multitasking, not an adversarial setup — the same GPU-path whole-frame
mean roughly doubles, to ~15–18 ms, i.e. at or over the 16.7 ms budget; and
0.96.0/0.96.1's parallel tile-upload serializer pushed it to ~34–37 ms, which
is why 0.96.2 routed the frame path back to the sequential serializer. Quote
the contended figure alongside the idle one or the claim is misleading.
PLAN.md's **0.96.2** entry is still the current real-hardware record **for the
two standing gate fixtures** (its 0.96.1 entry has the full contended tables).
0.99.0 and 0.100.0/0.100.1 re-measured on the same hardware since and did *not*
displace it, deliberately: 0.99.0 re-ran those same two fixtures and found no
detectable change (overlapping ranges), and 0.100.0 measured a **different,
new** fixture (two overlapping roots, a 256-tile store, ~40 ms/frame) whose
absolute numbers are not comparable to the gate rows at all — read its 0.100.1
correction before quoting any of it. Its 0.94.1 entry explains why a "p99"
from these
n=40 benchmarks is a single-sample order statistic that must not be quoted as
a ratio; read those, not this row, for where the numbers stand. The gate is
still missed either way, which is why this section stays — and `recomposite`
is now ~73% of the GPU-path frame mean, so it is the only stage where the
remaining gap can plausibly be closed.

This is an honest, open finding, not a benchmark claim — report it that way. The
tests assert a deliberately loose CI-safety threshold (350 ms / 180 ms) because
they exist to produce a true number, not to pass at 60 FPS; do not read a green
run as the budget being met. Later work reduced *how much* gets recomposited per
edit, not the per-tile cost, and none of it has been re-measured on real GPU
hardware. As of 0.73.4, brush/eraser dabs, pixel-stroke undo/redo, and
same-origin active-layer switches all invalidate the composite cache narrowly.
**Structural undo/redo (visibility, opacity, blend mode, and similar toggles)
does not** — 0.73.0 narrowed it too, but review found the narrowing unsound (a
layer's declared `bounds` is a position hint, not an enforced clip on its real
painted content, so the reported rect could miss real pixels), and 0.73.1
reverted it to a full bump. A live Move still bumps the whole thing either way,
because the composite tile grid is anchored to the *active layer's* own origin,
so dragging that layer re-anchors every tile at once. Re-anchoring the grid to
the document instead is the named follow-on (PLAN.md, Incremental compositing)
that would also be the prerequisite for structural narrowing to become sound.
**None of that is progress against the 60 FPS gate** — it buys Ctrl+Z-after-a-
stroke and Layers-panel click latency, on a different path from the one
measured above.

**PSD/PSB is full layered read *and* write** (PRD FR-001) — Aurora round-trips, so a file edited here must reopen in Photoshop with layers intact. Two rules follow: never overwrite a user's file in place (write to temp, verify by reopening, then swap), and warn with an itemized list before any lossy save. Silently degrading a professional's file is the worst failure this project can have.

## Phasing (PRD §9)

**Phase 0 (de-risking)** satisfied its Phase 1 gate (PRD §13 Steps 1, 3, 4) on 2026-07-28: `wgpu` validation, the tile-paging prototype, the screen-reader and CJK-IME spikes (the §8.3 escape-hatch triggers), widget toolkit foundations, the design language and token system, RAW/ICC library decisions, PSD feasibility, and the workspace + CI skeleton. Its tail is still open — see "What is not done" above.

**Phase 1 (document, canvas, layers, rendering, shell) is where the work is.** M1.1 and M1.6 are fully done; M1.2 through M1.9 are mostly done with named open items in their own PLAN.md sections; M1.10 is the gate.

Calendar durations were dropped on 2026-07-28 (PRD §13 Step 7): this is solo development, so phases are milestone-based and not date-committed. Don't reintroduce month estimates.

Each phase has a measurable exit criterion in §9 — prefer working toward the current gate over stubbing later-phase subsystems. Open questions that block design are tracked in PRD §12; risks in §11.
