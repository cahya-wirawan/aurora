//! Per-call cost of [`aurora_render::composite_layer_into`], the CPU
//! blend-math loop — one 256x256 tile, one fold, all 26 real
//! [`BlendMode`] variants. `cargo bench -p aurora-render --bench composite`.
//!
//! # Why this file exists
//!
//! It is the measurement half of a measurement-first round asking
//! whether that loop is worth parallelizing with `rayon` the way
//! `aurora_gpu::residency`'s serialize loop was in 0.96.0. The
//! threshold was fixed **before** any number here existed (see PLAN.md's
//! M1.10 **0.97.0** entry, committed together with this file, and its
//! **0.97.1** correction):
//!
//! * **T1** — the criterion median for *both* `Normal` and `Multiply`,
//!   in the `fold_onto_opaque` condition, on one whole tile, must each
//!   be >= 2.0 ms.
//! * **T2** — the per-frame aggregate of these calls must be >= 20% of
//!   the CPU-fallback frame benchmark's `recomposite` stage mean.
//!
//! **Correction (0.97.1): T1 was registered against the wrong
//! condition.** `aurora_app::composite_roots_into_tile` seeds every tile
//! with [`aurora_render::transparent_tile`] and folds roots in, so a
//! document's *first* fold always takes the `backdrop_alpha == 0.0` arm.
//! A **single-root** document — the common case, and the only shape
//! 0.97.0's own corroborating T2 fixture
//! (`recomposite_and_present_loop_exercises_the_cpu_fallback_path`, one
//! `Screen` root) has — therefore never reaches `fold_onto_opaque` at
//! all. Read against `fold_onto_transparent`, the condition that fixture
//! actually exercises, `Normal` (~1.80 ms) and `Multiply` (~1.79 ms)
//! both **fail** the 2.0 ms bar by ~10%. ~~T1 passes only for
//! second-root-onward folds, i.e. only on multi-root documents.~~ Do not
//! quote a T1 pass without naming which condition it came from.
//!
//! **Closed (0.101.0): T1 now fails in *both* conditions, so there is no
//! document shape left in which it passes and the `rayon` question this
//! file exists to answer is settled NO-GO.** 0.98.0's batched `f16` ↔
//! `f32` conversions cut `fold_onto_opaque` by ~16–17%, which took the
//! very column the conditional pass depended on below the bar. Re-measured
//! on this box at 0.101.0: `fold_onto_transparent` `Normal` **1.6826 ms**
//! / `Multiply` **1.6911 ms**; `fold_onto_opaque` `Normal` **1.7051 ms** /
//! `Multiply` **1.7335 ms** — four cells, all under 2.0 ms, all with
//! confidence intervals wholly under it. This file stays because the
//! numbers are the standing per-call record of what CPU compositing costs,
//! not because the decision is still open. PLAN.md's 0.101.0 M1.10 entry
//! has the verdict, the two separate soundness blockers on the *other*
//! two framings (fold-across-roots, fold-across-tiles), and the one sound
//! restructuring left unbuilt.
//!
//! **Read the decision off `Normal` and `Multiply` only.** Those two
//! dominate real documents — the app's own default startup document uses
//! exactly those two — so a future reader must not read the go/no-go
//! off a more exotic mode like `Color` or `HardMix`, which cost more per
//! texel but appear in almost no real document. Every mode is measured
//! anyway because the extra arms are nearly free and the relative
//! spread is itself informative.
//!
//! # Why two conditions per mode
//!
//! `composite_layer_into` branches on `backdrop_alpha > 0.0`: a fold
//! onto a fully transparent accumulator short-circuits the backdrop
//! straightening divisions entirely, so measuring only that case would
//! understate a multi-root document's real cost.
//!
//! * `fold_onto_transparent` — the real *first* root's fold, onto
//!   `transparent_tile()`. Cheap arm, and **the only arm a single-root
//!   document ever takes**.
//! * `fold_onto_opaque` — the real *second-root-onward* fold, onto an
//!   accumulator that already has alpha, so it pays the straightening
//!   divisions.
//!
//! **What actually differs between the two conditions is the division
//! arm alone: three divisions, one each for `r`, `g` and `b`**
//! (`src/composite.rs`, the `straight_backdrop` `if`). `blend_rgb` is
//! called *unconditionally*, outside that `if`, on both arms — so the
//! two conditions do **not** differ in blend-dispatch presence, and
//! 0.97.0's claim that the opaque arm pays "the full `blend_rgb`
//! dispatch" as if the transparent arm did not was wrong (corrected
//! 0.97.1). The measured gap is ~0.28 ms/tile, i.e. ~4.3 ns per texel
//! for three `f32` divides plus three extra `f16`→`f32` widenings.
//!
//! Both accumulators are re-cloned in criterion's untimed setup closure
//! on every iteration: folding repeatedly against one accumulator drifts
//! its alpha toward 1.0 and quietly changes which branches are being
//! exercised partway through a run.
//!
//! The blend mode is passed through `black_box` at the call site so LLVM
//! cannot specialize the `match` inside `blend_rgb` for a compile-time
//! constant — the real caller dispatches on a runtime value, and a
//! constant-folded match would make every mode look artificially fast
//! and artificially uniform.

use aurora_render::{BlendMode, composite_layer_into};
use aurora_tile::{CHANNELS, SAMPLES};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use half::f16;

/// Every real variant. `aurora_doc::BlendMode` has 27; `Dissolve` is
/// resolved to `Normal` by `aurora-app` before this crate ever sees it,
/// so it has no representation here and there are 26.
const MODES: [(&str, BlendMode); 26] = [
    ("Normal", BlendMode::Normal),
    ("Darken", BlendMode::Darken),
    ("Multiply", BlendMode::Multiply),
    ("Lighten", BlendMode::Lighten),
    ("Screen", BlendMode::Screen),
    ("Difference", BlendMode::Difference),
    ("Exclusion", BlendMode::Exclusion),
    ("Subtract", BlendMode::Subtract),
    ("Divide", BlendMode::Divide),
    ("ColorDodge", BlendMode::ColorDodge),
    ("LinearDodge", BlendMode::LinearDodge),
    ("ColorBurn", BlendMode::ColorBurn),
    ("LinearBurn", BlendMode::LinearBurn),
    ("Overlay", BlendMode::Overlay),
    ("SoftLight", BlendMode::SoftLight),
    ("HardLight", BlendMode::HardLight),
    ("VividLight", BlendMode::VividLight),
    ("LinearLight", BlendMode::LinearLight),
    ("PinLight", BlendMode::PinLight),
    ("HardMix", BlendMode::HardMix),
    ("Hue", BlendMode::Hue),
    ("Saturation", BlendMode::Saturation),
    ("Color", BlendMode::Color),
    ("Luminosity", BlendMode::Luminosity),
    ("DarkerColor", BlendMode::DarkerColor),
    ("LighterColor", BlendMode::LighterColor),
];

/// Deterministic xorshift64 — the same generator shape
/// `aurora-tile`'s own `benches/tile_store.rs` uses, for the same
/// reason: varied, reproducible input without a `rand` dependency in a
/// one-off benchmark.
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A varied value in `[0, 1)`.
    fn next_unit(&mut self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let scaled = (self.next_u64() % 10_007) as f32;
        scaled / 10_007.0
    }
}

/// One tile of varied colour with varied, strictly-positive alpha.
///
/// `alpha_floor` keeps the accumulator's own alpha away from zero for
/// the `fold_onto_opaque` condition, so every texel takes the
/// `backdrop_alpha > 0.0` arm and pays its three divisions. A
/// floor of `0.0` gives the source tile a genuinely varied alpha
/// including near-transparent texels, which is what a real source layer
/// looks like.
fn seeded_tile(seed: u64, alpha_floor: f32) -> Vec<f16> {
    let mut rng = Xorshift64::new(seed);
    let mut out = Vec::with_capacity(SAMPLES);
    for _ in 0..(SAMPLES / CHANNELS) {
        let alpha = alpha_floor + (1.0 - alpha_floor) * rng.next_unit();
        // Premultiplied-shaped: the accumulator this function stands in
        // for holds `composite_layer_into`'s own running "over"
        // accumulation, whose colour channels never exceed its alpha.
        for _ in 0..(CHANNELS - 1) {
            out.push(f16::from_f32(rng.next_unit() * alpha));
        }
        out.push(f16::from_f32(alpha));
    }
    out
}

/// The source layer being folded in: varied colour, varied alpha all
/// the way down to near-zero, so no mode takes a degenerate branch on
/// every texel.
fn seeded_source(seed: u64) -> Vec<f16> {
    let mut rng = Xorshift64::new(seed);
    let mut out = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        out.push(f16::from_f32(rng.next_unit()));
    }
    out
}

fn fold(c: &mut Criterion, condition: &str, accumulator: &[f16]) {
    let source = seeded_source(0x5EED_1234_ABCD_0001);
    let mut group = c.benchmark_group(condition);
    for (name, mode) in MODES {
        group.bench_function(name, |b| {
            b.iter_batched_ref(
                || accumulator.to_vec(),
                |out| {
                    composite_layer_into(
                        out,
                        std::hint::black_box(&source),
                        std::hint::black_box(0.75),
                        // The load-bearing `black_box`: without it the
                        // `match` inside `blend_rgb` can be specialized
                        // for a compile-time-constant mode, which is not
                        // what the real runtime-dispatched caller pays.
                        std::hint::black_box(mode),
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// The real *first*-root case: the accumulator is
/// `aurora_render::transparent_tile()`, every texel takes the
/// `backdrop_alpha == 0.0` short-circuit.
///
/// 0.97.0 labelled this "measured for completeness; T1 is not read from
/// here". **That was backwards for the single-root documents T2's own
/// fixture uses** — this is the *only* condition such a document ever
/// reaches, so it is the condition a single-root verdict must be read
/// from. See this file's module doc for the 0.97.1 correction.
fn fold_onto_transparent(c: &mut Criterion) {
    fold(
        c,
        "fold_onto_transparent",
        &aurora_render::transparent_tile(),
    );
}

/// The real *second-root-onward* case: a non-zero backdrop alpha at
/// every texel, so every iteration pays the three straightening
/// divisions. **Only a multi-root document reaches this at all** — it is
/// not, as 0.97.0 framed it, "the one the whole decision rests on".
fn fold_onto_opaque(c: &mut Criterion) {
    fold(
        c,
        "fold_onto_opaque",
        &seeded_tile(0x5EED_1234_ABCD_0002, 0.25),
    );
}

criterion_group!(benches, fold_onto_transparent, fold_onto_opaque);
criterion_main!(benches);
