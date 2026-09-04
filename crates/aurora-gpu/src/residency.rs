//! GPU tile residency: a fixed-size, tile-aligned atlas texture that
//! slides over the (potentially unbounded) document, with toroidal slot
//! addressing so panning invalidates one row/column of GPU uploads
//! instead of the whole texture (`spike/FINDINGS.md` finding #4).

use std::collections::HashMap;
use std::sync::OnceLock;

use aurora_core::MAX_DOCUMENT_EXTENT;
use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileId, TileStore};
use half::f16;
use half::slice::HalfFloatSliceExt;
use rayon::prelude::*;

use crate::error::GpuError;

/// The `Canvas` uniform `canvas.wgsl` expects: `uv_offset`/`uv_scale`,
/// two `vec2<f32>`s, 16 bytes total.
const UNIFORM_SIZE: u64 = 16;

/// Bytes one tile upload costs — `f16` samples, `SAMPLES` per tile.
const TILE_BYTES: usize = SAMPLES * 2;

/// Mip levels the atlas carries: level 0 is full resolution ([`TILE`] ×
/// [`TILE`]), each level above halves the side length. Fixed at 4 rather
/// than configurable — nothing needs more, and this crate doesn't depend
/// on `aurora-render`'s `MipLevel` enum, but the correspondence is exact
/// by convention: 0 = Full, 1 = Half, 2 = Quarter, 3 = Eighth.
///
/// **Levels 1-3 are allocated but structurally unreachable today** —
/// this is the one place that rationale is written down; everything else
/// points here.
///
/// Nothing in the live frame loop populates them: [`TileResidency::sync`]
/// uploads `mip_level: 0` only, and [`TileResidency::upload_mip`] (which
/// can write the rest) has no call site in `aurora-app`. wgpu lazily
/// zero-initializes what was never written, so a sample that landed on
/// one of those levels read `(0, 0, 0, 0)`, `fs_canvas`'s premultiplied
/// "over" collapsed to pure background, and the canvas rendered as pure
/// checkerboard. (That sentence said "straight-alpha" until 0.68.0 moved
/// the alpha-convention boundary to upload time — see
/// `premultiply_rgba`; the conclusion is unchanged either way, since
/// both formulas collapse to the background at `a = 0`.)
///
/// It *did* land on them. `write_uniform` makes the atlas cover `1/zoom`
/// texels per screen pixel, so the hardware's computed LOD is
/// `-log2(zoom)`; the default `MipmapFilterMode::Nearest` rounds that to
/// a whole level and crosses into level 1 at LOD 0.5 — `zoom = 2^-0.5 ≈
/// 0.7071`. Every zoom below ~71% showed the user nothing at all.
///
/// The fix is at the *view*, not the sampler: [`TileResidency::new`]
/// builds its sampled view with `mip_level_count: Some(1)`, so level 0
/// is the only level that exists as far as sampling is concerned and
/// level selection has nowhere else to go. The obvious alternative,
/// `lod_max_clamp: 0.0` on the sampler, is **not** equivalent and was
/// measured not to be: both the Vulkan and OpenGL specs make the
/// magnification/minification decision on the LOD *after* the clamp, so
/// a clamped LOD is never positive, every access classifies as
/// magnification, and `mag_filter` (`Nearest`) silently replaces
/// `min_filter` (`Linear`) for every sample —
/// `canvas_pipeline_min_filter_linear_still_applies_when_minified` in
/// `render_test.rs` was measured failing with that spelling and passing
/// with this one **on this sandbox's Mesa llvmpipe *software* Vulkan
/// adapter** (`llvmpipe (LLVM 21.1.8, 256 bits) (Vulkan, Cpu)`), the
/// only adapter available here. No real GPU has been asked; the claim
/// this comment used to make -- that it "fails on real hardware" -- was
/// never measured on any.
///
/// **The cost of keeping four levels is accepted, not overlooked**: the
/// unreachable levels are about 33% extra atlas VRAM (`1/4 + 1/16 +
/// 1/64`). They stay allocated because [`TileResidency::upload_mip`] and
/// its tests are real, working infrastructure for the progressive/LOD
/// rendering PLAN.md tracks under M1.3, and deleting them to reclaim
/// memory during a bug-fix round would trade tested groundwork for a
/// saving nothing has asked for. Wiring that path means widening the
/// view's `mip_level_count` and choosing a `mipmap_filter` deliberately,
/// in the same change that starts filling the chain — not as a side
/// effect.
const MIP_LEVELS: u32 = 4;

/// Converts a tile's straight-alpha texels to **premultiplied** alpha in
/// place: each texel's `r`/`g`/`b` multiplied by its own `a`.
///
/// **Test-only since 0.92.1**, and `#[cfg(test)]` so that stays true:
/// this is the scalar *reference* implementation the equivalence tests
/// below pin the real serializer against, not a path any upload takes.
///
/// **Both real upload paths still bottom out in the same one function**,
/// which is what matters here, but as of 0.96.0 neither reaches it the way
/// it used to and the chains differ in length:
/// [`TileResidency::sync`] calls [`serialize_premultiplied_le_bytes`]
/// directly — the sequential core, deliberately, see that call site for the
/// measurement (0.96.2) — and [`TileResidency::upload_mip`] calls
/// [`extend_premultiplied_le_bytes`] → [`write_premultiplied_le_bytes`] →
/// `serialize_premultiplied_le_bytes`, which is where the `rayon` split is
/// still reached. (Through 0.96.0 this paragraph said both paths call
/// `extend_premultiplied_le_bytes`; that stopped being true when 0.96.0
/// moved `sync` off it, and was corrected in 0.96.1.)
/// The two paths did not share an implementation at all between 0.92.0 and
/// 0.92.1 — `upload_mip` still ran this function — and that split is
/// exactly what made them disagree on a double-NaN texel while writing the
/// same atlas texture. Keeping the reference compiled only under
/// `cfg(test)` is what stops that regression from being reintroduced by
/// accident.
///
/// The rest of this comment is the canonical write-up of *why* the atlas
/// holds premultiplied alpha at all. It is still accurate and still the
/// thing `canvas.wgsl`, `render_test.rs` and PLAN.md point at; only the
/// function that applies it on the real upload path has moved.
///
/// This is the alpha-convention boundary for the atlas, and it is here
/// — at upload — rather than in the shader, for one reason:
/// **hardware texture filtering has to happen in the premultiplied
/// domain to be correct.** A bilinear tap is a weighted average of four
/// texels computed per channel, independently. Averaging *straight*
/// colours weights a fully transparent texel's RGB exactly as heavily as
/// an opaque neighbour's, so a hard opaque/transparent boundary drags
/// the transparent side's (arbitrary, usually black or stale) colour
/// into the visible result — the classic dark halo. Premultiplied RGB
/// carries its own alpha weight, so the same average is the correct
/// alpha-weighted one.
///
/// `aurora-tile`'s store stays **straight** — that is the workspace's
/// universal convention, and nothing about it changes here. So do the
/// CPU/GPU composite surfaces. Only the atlas texture, whose whole
/// purpose is to be *sampled with filtering*, holds premultiplied
/// texels, and `canvas.wgsl`'s `fs_canvas` is written against exactly
/// that (see its own comment, which carries the history).
///
/// **Trailing partial chunk**: `chunks_exact_mut` yields only whole
/// [`CHANNELS`]-sized groups, so a slice whose length is not a multiple
/// of `CHANNELS` leaves its final, incomplete texel untouched rather
/// than corrupting it. Both call sites validate their sample counts
/// upstream ([`TileResidency::sync`] uploads whole tiles of exactly
/// [`SAMPLES`]; [`TileResidency::upload_mip`] rejects a wrong-length
/// slice before reaching here), so the case is unreachable in practice
/// — it is defined rather than assumed away. The `[r, g, b, a]` slice
/// pattern below is what ties this to `CHANNELS == 4`, which
/// `premultiply_rgba_is_written_against_a_four_channel_texel` pins
/// against the crate's own constant rather than leaving it implied.
#[cfg(test)]
fn premultiply_rgba(texels: &mut [f16]) {
    for texel in texels.chunks_exact_mut(CHANNELS) {
        let [r, g, b, a] = texel else {
            continue;
        };
        let alpha = f32::from(*a);
        *r = f16::from_f32(f32::from(*r) * alpha);
        *g = f16::from_f32(f32::from(*g) * alpha);
        *b = f16::from_f32(f32::from(*b) * alpha);
    }
}

/// Texels converted per vectorized chunk. 64 texels = 256 samples, so
/// the two scratch arrays in [`extend_premultiplied_le_bytes`] are 1 KiB
/// (`f32`) and 512 B (`f16`) and stay in L1, and `SAMPLES /
/// CHUNK_SAMPLES` is exactly 1024 — the real upload path never reaches
/// the scalar remainder.
const CHUNK_TEXELS: usize = 64;
const CHUNK_SAMPLES: usize = CHUNK_TEXELS * CHANNELS;

/// `CHUNK_SAMPLES` chunks one rayon task converts. 16 chunks = 4096
/// samples = 1024 texels = 8 KiB of output bytes, so a full 256×256 tile
/// ([`SAMPLES`] = 262,144) splits into exactly **64** independent tasks —
/// coarse enough that rayon's per-task bookkeeping is not the cost, fine
/// enough to fill a desktop core count several times over. This is the
/// one tuning knob for the parallel serializer; `SAMPLES % BLOCK_SAMPLES
/// == 0` and `BLOCK_SAMPLES % CHUNK_SAMPLES == 0` are pinned by
/// `the_block_and_chunk_constants_divide_a_whole_tile_evenly` rather than
/// left implied.
const BLOCK_CHUNKS: usize = 16;
const BLOCK_SAMPLES: usize = CHUNK_SAMPLES * BLOCK_CHUNKS;

/// The sequential core of the premultiply/serialize path: converts every
/// whole texel of `texels` into the little-endian `f16` bytes
/// `wgpu::Queue::write_texture` wants, premultiplied on the way, writing
/// them into `out` **in place** rather than appending.
///
/// **This is the hot function**, and as of 0.96.0 it is the *only* place
/// real upload bytes are produced. As of 0.96.2 the frame path
/// ([`TileResidency::sync`]) calls it *directly*, one tile at a time on the
/// frame thread; [`TileResidency::upload_mip`] still reaches it through
/// [`write_premultiplied_le_bytes`], either in parallel via that function's
/// `rayon` splitter or sequentially via its fallback. See `sync`'s call site
/// for why the frame path does not take the parallel arm. The substantive
/// *why* below (the
/// 0.68.0 buffer history, the 0.88.1 measurement that named this loop
/// rather than the bus, the 0.89.0 append batching, the `wide`-vs-`half`
/// crate evaluation, and exactly which bit-exactness guarantees hold
/// against [`premultiply_rgba`]) was moved here in 0.96.1 from
/// [`extend_premultiplied_le_bytes`], which carried it while it was
/// [`TileResidency::sync`]'s entry point and no longer is —
/// `extend_premultiplied_le_bytes` is now a `Vec`-sizing wrapper whose only
/// caller is [`TileResidency::upload_mip`].
///
/// # Why one buffer, written in place (0.68.0)
///
/// The premultiply happens *here*, as the bytes are written, rather than
/// as a separate pass over a separate buffer, so that
/// [`TileResidency::sync`] needs one reusable buffer for the whole call
/// instead of two per tile. 0.68.0 spelled the same work as *copy the tile
/// into a staging `Vec<f16>`, premultiply that in place, then allocate a
/// fresh `Vec<u8>` and serialize into it* — a half-megabyte copy plus an
/// allocation per tile, on an upload path `spike/FINDINGS.md` finding #3
/// already names as bandwidth-bound. The comment justifying the staging
/// buffer said it "avoids allocating a fresh half-megabyte buffer per
/// tile", which the very next line then did anyway.
///
/// # This loop is the real cost, not the bus (measured, 0.88.1)
///
/// On the real pan-while-painting benchmark (`aurora-app`'s M1.10
/// per-stage frame breakdown, PLAN.md), this conversion loop dominated the
/// `upload_sync` stage's time — a single-threaded scalar
/// `f16 -> f32 -> premultiply -> f16` conversion, not GPU bandwidth. The
/// actual GPU DMA of the bytes this produces happens later, at the next
/// `queue.submit`, and measures near line-rate there. Before optimizing
/// this as a *bandwidth* problem (fewer bytes, mip streaming), check
/// whether it's cheaper to fix as a *throughput* problem first — see the
/// PLAN.md entry for the measured numbers. (0.88.1 quoted "~87% of the
/// stage" from a probe of the pre-0.89.0 four-append loop. Do not quote
/// that figure forward: 0.90.0's unchanged-tile skip cut how many tiles
/// reach this function at all, so the share is stale. PLAN.md's 0.92.0
/// entry states what a fresh baseline actually measured instead.)
///
/// **Batching the four channel writes is done (0.89.0)**: the loop body
/// concatenates the four `to_le_bytes()` pairs into one 8-byte write
/// ([`write_texel_le_bytes`]) instead of four separate 2-byte appends.
/// Each append re-checked the `Vec`'s capacity and performed its own small
/// `memcpy`, so this cut a 256×256 tile from 262,144 append calls to
/// 65,536. The arithmetic, the `to_le_bytes()` calls and the R/G/B/A order
/// were unchanged, so the output was bit-for-bit what the four-append
/// version produced.
///
/// **What that actually bought, measured: almost nothing, and it
/// disproved the rationale in the paragraph above.** Roughly 1-5% off
/// the `upload_sync` stage's p50, no improvement at p99 (noise-dominated
/// in both directions), and the GPU-path stage *mean* stayed inside
/// run-to-run noise; the 60 FPS verdict did not move. Removing 3-of-4
/// append calls per texel from a stage this loop dominates should have
/// been dramatic if capacity checks and small `memcpy`s were where the
/// time went — so they are not. **The per-texel
/// `f16 -> f32 -> multiply -> f16` arithmetic is**, which redirected the
/// next attempt here to vectorizing that conversion rather than any
/// further append bookkeeping. PLAN.md's 0.89.0 addendum under M1.10 has
/// the full before/after tables, the sample sizes, and the caveats.
///
/// # Vectorizing the conversion (0.92.0): the crate evaluation
///
/// **Why "just call `from_f32` in a loop, but faster" is not the fix.**
/// `half`'s scalar `f16::from_f32` / `f32::from(f16)` already use the
/// hardware F16C instructions (`vcvtps2ph` / `vcvtph2ps`) on `x86_64` —
/// they are not slow *arithmetic*. What is slow is the per-call
/// overhead around them: each one runs its own
/// `is_x86_feature_detected!("f16c")` check and then calls a
/// `#[target_feature(enable = "f16c")]` function, which cannot be
/// inlined into a caller that was not compiled with that feature. At
/// the pre-0.92.0 per-texel granularity a 256×256 tile paid that overhead
/// **seven times per texel**: one `f32::from` to widen alpha, three more
/// to widen R/G/B, and three `f16::from_f32` calls to narrow the three
/// products back down. That is `7 × 65,536 = 458,752` non-inlinable
/// calls, each preceded by a feature-detection check, per tile. (This
/// paragraph said "three times per texel for RGB plus once for the
/// widening of alpha — on the order of 393,000" through 0.92.0. Both
/// halves were wrong and they were wrong inconsistently: the prose named
/// four operations, which would be 262,144, while the figure implied six.
/// Counted against the pre-0.92.0 body itself — `git show HEAD^` at the
/// 0.92.1 commit — it is seven and 458,752.) That bookkeeping, not the
/// conversion, is the cost.
///
/// **`wide` was evaluated and rejected.** The user's starting
/// suggestion was the `wide` crate. It is the wrong tool here, for two
/// independent reasons. First and decisively: **`wide` has no `f16` lane
/// type at all.** Its lane vocabulary is `i8`/`i16`/`i32`/`i64`,
/// `u8`/`u16`/`u32`/`u64`, `f32` and `f64` — so it cannot vectorize the
/// `f16` ↔ `f32` conversion, which is the part that costs. It would
/// vectorize only the alpha multiply, leaving all 458,752 scalar
/// conversion calls exactly where they are. Second: without a
/// target-feature bump 0.92.0 was not making (no `RUSTFLAGS`, no
/// `.cargo/config.toml`), `wide`'s widest guaranteed lane width on
/// baseline `x86_64` is SSE2's 128 bits — four `f32` lanes for the one
/// operation that was never the bottleneck. It would also be a new
/// runtime dependency, with the licence review that implies.
///
/// **`half`'s own slice API was chosen instead.** `half` is already a
/// dependency (`half = "2"`, resolving to 2.7.1), and it ships
/// [`half::slice::HalfFloatSliceExt`] — `convert_to_f32_slice` and
/// `convert_from_f32_slice` — which reach the *same* F16C instructions
/// through `_mm256_cvtph_ps` / `_mm256_cvtps_ph`, eight lanes per
/// instruction, behind **one** feature-detection check for the whole
/// slice rather than one per sample. That is genuine SIMD
/// vectorization of the expensive half of the work, with **zero new
/// dependencies** and no `unsafe` in this crate — strictly better than
/// what adding `wide` could have given. On a CPU without F16C, `half`
/// falls back to its own software conversion and this code stays
/// correct, just not faster; on `aarch64` it takes `half`'s own `fp16`
/// NEON path. Neither non-F16C `x86_64` nor `aarch64` is verified here.
///
/// # Bit-exactness with the scalar path: what is guaranteed, and the one
/// case that is not
///
/// `half`'s scalar and 8-wide x86 paths issue the same
/// conversion instructions with the same rounding immediate
/// (`_MM_FROUND_TO_NEAREST_INT`), and the multiply keeps the scalar
/// path's `rgb * alpha` operand order. So for **any input where at most
/// one of a texel's RGB channel and its alpha is NaN**, this function is
/// bit-for-bit identical to `premultiply_rgba` followed by a plain
/// little-endian serialize — every finite value, both infinities, both
/// signed zeros, every subnormal, and every single-NaN combination. That
/// half was established exhaustively over all 65,536 × 65,536 (RGB bits ×
/// alpha bits) texels by an independent review pass and holds **in every
/// build profile tested** (`opt-level = 1`, the workspace's default, and
/// `opt-level = 3` / `--release`) — unlike the double-NaN case below,
/// which is profile-dependent, this one follows from IEEE 754's own
/// single-NaN propagation rule, not from what a particular optimizer
/// happens to emit, so there is no reason to expect a third profile to
/// behave differently.
///
/// **When a texel's RGB channel and its alpha are *both* NaN, the two
/// spellings can disagree on the result's NaN payload.** 0.92.0's doc
/// comment claimed bit-exactness without qualification; that claim was
/// false for exactly this case, which is why it is spelled out here
/// instead of hedged. What is still guaranteed is that the result is *a*
/// NaN carrying one of the two NaN operands' payloads, quieted — the pixel
/// was already meaningless before and after, so this turns garbage into
/// different garbage rather than corrupting a good pixel. What is *not*
/// guaranteed is *which* operand's payload survives: `NaN × NaN` returns
/// the quieted **first source operand** on x86, and which of `rgb` and
/// `alpha` ends up "first" is an operand-order detail of whatever code
/// LLVM emits. Neither IEEE 754 nor this source pins it down.
///
/// **Measured, because the shape of it is not what it looks like.** Per
/// channel position, "which operand's payload survives" on `x86_64`
/// (0.92.1, this sandbox, all 30 ordered pairs of six distinct NaN
/// payloads, every texel of a full chunk):
///
/// | profile | this function | `premultiply_rgba` | agree? |
/// |---|---|---|---|
/// | `opt-level = 1` (the default test profile) | rgb, rgb, **alpha** | rgb, rgb, **alpha** | yes, bit-exact |
/// | `opt-level = 3` (`--release`) | **alpha**, rgb, **alpha** | rgb, rgb, **alpha** | no — R diverges |
///
/// Three things worth reading off that table, none of them obvious. The
/// divergence between the two paths is **release-only**: at the profile
/// `cargo test` and `cargo nextest` actually use, they agree bit-for-bit
/// even here. `premultiply_rgba` is **not** a scalar baseline that IEEE
/// 754 pins — LLVM auto-vectorizes its loop too, and it already sources
/// B's payload from `alpha` at every profile measured, so the divergence
/// is between two *different* auto-vectorizations of the same arithmetic
/// rather than between a vector path and a scalar one. And the choice
/// varies by *channel position*, not by texel or by payload: LLVM
/// vectorizes some of the three multiplies and leaves others alone, and
/// which ones move with the optimizer.
///
/// `the_fused_serializer_agrees_on_a_double_nan_texel_up_to_the_payload_operand`
/// below pins what survives all of that — the payload comes from one of
/// the two operands and never from anywhere else, each channel position
/// chooses consistently across every texel and every payload pair, and
/// alpha passes through untouched — so a toolchain change that moved this
/// somewhere else fails a test instead of passing silently. It
/// deliberately does not assert *which* operand, since that is the part
/// the table shows to be profile-dependent.
///
/// Reachability, stated rather than hand-waved: `aurora-io`'s 16-bit-float
/// TIFF reader takes raw `f16` samples verbatim with no NaN filtering, so
/// a malformed or adversarial file can construct this input directly. The
/// consequence is bounded to the above.
///
/// The equivalence tests below pin all of this against
/// [`premultiply_rgba`], which is deliberately left in the obvious scalar
/// spelling as the reference implementation, and which is `#[cfg(test)]`
/// since 0.92.1 because that is now its only role.
///
/// **Why alpha is read from the source chunk, never from the narrowed
/// buffer.** `premultiply_rgba` does not touch alpha at all, so its
/// bits pass through untouched — and `f16 -> f32 -> f16` is *not* the
/// identity for a signalling NaN, which the F16C widening quiets
/// (`0x7c01` becomes `0x7e01`). Taking alpha from the round-tripped
/// buffer would therefore silently change one class of input. It is
/// copied straight from `chunk` instead.
///
/// # Panic-freedom, argued rather than asserted
///
/// The release profile
/// sets `panic = "abort"`, and as of 0.96.0 this function can also run inside
/// a `rayon` worker closure (on [`TileResidency::upload_mip`]'s path; the
/// frame path calls it inline as of 0.96.2), so a panic here is a process
/// abort rather than a recoverable error. There are six places a panic could
/// come from — 0.96.0's own version of this list named four, and both
/// omissions are called out below — and none of them can fire:
///
/// 1. `chunk.convert_to_f32_slice(&mut wide)` — `half`'s only failure mode
///    is an `assert_eq!` that source and destination lengths match.
///    `chunk` comes from `texels.chunks_exact(CHUNK_SAMPLES)`, which by
///    definition yields slices of exactly `CHUNK_SAMPLES` elements, and
///    `wide` is a fixed-size `[f32; CHUNK_SAMPLES]` array. The lengths are
///    equal by construction, not by check.
/// 2. `narrow.convert_from_f32_slice(&wide)` — same assertion, and both
///    operands are fixed-size `CHUNK_SAMPLES` arrays declared three lines
///    apart. Equal for the same reason.
/// 3. Slice indexing — there is none. Every read walks an iterator
///    (`chunks_exact`, `chunks_exact_mut`, `zip`), and every write goes
///    through an irrefutable-on-success slice pattern whose failure arm
///    `continue`s or returns instead of panicking (the two arms in this
///    function `continue`; [`write_texel_le_bytes`]'s returns). No `[i]`,
///    no `copy_from_slice` (which *does* panic on a length mismatch), no
///    `unwrap`, no `expect`.
/// 4. Running out of `out` — impossible to panic on, because `out` is
///    consumed through `chunks_exact_mut(CHANNELS * 2)` zipped against the
///    texel iterators. A `zip` stops at whichever side ends first, so an
///    `out` too short simply writes fewer texels and an `out` too long
///    leaves its tail untouched. That is what makes the reused
///    [`Self::sync`] buffer and the parallel splitter's short trailing
///    block both safe without a length assertion.
/// 5. **A zero chunk size** — `chunks_exact`, `chunks_exact_mut` and
///    `par_chunks*` all panic on a chunk size of 0, which 0.96.0's list
///    did not mention at all. Every chunk size on this path is a
///    compile-time constant computed from [`CHANNELS`] (4): `CHUNK_SAMPLES`
///    is 256, `CHANNELS * 2` is 8, `CHANNELS` is 4, `BLOCK_SAMPLES` is
///    4,096. None can be zero without `CHANNELS` being zero, which would
///    break the `[r, g, b, a]` slice patterns at compile time — so this is
///    unreachable by construction rather than by check.
/// 6. **`rayon`'s thread-pool construction** — the one 0.96.0 missed that
///    could actually fire, and it is not in this function: it is in
///    [`write_premultiplied_le_bytes`], which used to reach `rayon`'s
///    *implicit global* pool. That pool is initialized lazily on first use,
///    and `rayon_core::registry::global_registry` `.expect()`s the
///    `Result` of building it — so a machine that cannot spawn worker
///    threads (`RLIMIT_NPROC`, a cgroup `pids.max`, systemd `TasksMax`, or
///    plain memory pressure; reproduced here with `ulimit -u`) panicked
///    inside `rayon`, i.e. `SIGABRT` under `panic = "abort"`, on a path
///    that at the time ran on every frame including the default startup
///    document's. 0.96.1 replaced the global pool with an **owned, explicitly
///    bounded** one whose `build()` `Result` is captured, falling back to
///    calling *this* function directly when it fails; 0.96.2 additionally
///    took the frame path off the pool entirely, so the abort window is now
///    narrower still — but the fix stays, because `upload_mip` is real code
///    and the frame path is one measurement away from being routed back. See
///    [`write_premultiplied_le_bytes`] and [`serializer_pool`] for the
///    mechanism and the test that exercises the fallback.
///
/// **Trailing partial texel**: a `texels` length that is not a multiple of
/// [`CHANNELS`] contributes nothing for its final incomplete texel, the
/// same contract [`premultiply_rgba`] and the pre-0.96.0 spelling of
/// [`extend_premultiplied_le_bytes`] both have.
fn serialize_premultiplied_le_bytes(texels: &[f16], out: &mut [u8]) {
    // `wide`/`narrow` name the sample width (`f32`/`f16`) of the scratch
    // buffers. `wide` is deliberately *not* a reference to the rejected
    // crate of the same name -- see this function's own doc comment.
    let mut wide = [0f32; CHUNK_SAMPLES];
    let mut narrow = [f16::ZERO; CHUNK_SAMPLES];
    // One texel of output is `CHANNELS` little-endian `f16` samples.
    let mut sink = out.chunks_exact_mut(CHANNELS * 2);
    let mut chunks = texels.chunks_exact(CHUNK_SAMPLES);
    for chunk in chunks.by_ref() {
        // One vectorized f16 -> f32 pass: 8 lanes per `vcvtph2ps`, one
        // feature-detection check for the whole slice.
        chunk.convert_to_f32_slice(&mut wide);
        for texel in wide.chunks_exact_mut(CHANNELS) {
            let [r, g, b, a] = texel else {
                continue;
            };
            let alpha = *a;
            // Operand order (rgb * alpha) matches the scalar path's, so
            // single-NaN products keep the same payload. A *double*-NaN
            // product does not necessarily -- see this function's own
            // "Bit-exactness" section; it is disclosed, tested and
            // deliberately not chased.
            *r *= alpha;
            *g *= alpha;
            *b *= alpha;
        }
        // One vectorized f32 -> f16 pass: 8 lanes per `vcvtps2ph`.
        narrow.convert_from_f32_slice(&wide);
        // Alpha comes from `chunk`, never from `narrow`: f16 -> f32 -> f16
        // is not the identity for a signalling NaN (0x7c01 -> 0x7e01), and
        // `premultiply_rgba` leaves alpha's bits untouched -- this must too.
        for ((premultiplied, source), dest) in narrow
            .chunks_exact(CHANNELS)
            .zip(chunk.chunks_exact(CHANNELS))
            .zip(sink.by_ref())
        {
            let [r, g, b, _] = premultiplied else {
                continue;
            };
            let [_, _, _, a] = source else {
                continue;
            };
            write_texel_le_bytes(*r, *g, *b, *a, dest);
        }
    }
    // Trailing partial chunk, unchanged contract: whole texels in the
    // remainder are premultiplied scalar-wise; a final incomplete texel
    // contributes nothing.
    for (texel, dest) in chunks.remainder().chunks_exact(CHANNELS).zip(sink.by_ref()) {
        let [r, g, b, a] = texel else {
            continue;
        };
        let alpha = f32::from(*a);
        write_texel_le_bytes(
            f16::from_f32(f32::from(*r) * alpha),
            f16::from_f32(f32::from(*g) * alpha),
            f16::from_f32(f32::from(*b) * alpha),
            *a,
            dest,
        );
    }
}

/// Writes one already-premultiplied texel into `dest` as eight
/// little-endian bytes.
///
/// Split out only so the vectorized chunk loop and the scalar remainder
/// loop in [`serialize_premultiplied_le_bytes`] cannot drift apart in byte
/// order — they wrote two independently-spelled copies of this before
/// 0.96.0.
///
/// `dest` is destructured with a slice pattern rather than
/// `copy_from_slice`, which panics on a length mismatch. A `dest` that is
/// not exactly `CHANNELS * 2` bytes writes nothing instead of aborting the
/// process (`panic = "abort"` in release), and cannot occur anyway: every
/// caller obtains `dest` from `chunks_exact_mut(CHANNELS * 2)`.
fn write_texel_le_bytes(r: f16, g: f16, b: f16, a: f16, dest: &mut [u8]) {
    let [r_lo, r_hi] = r.to_le_bytes();
    let [g_lo, g_hi] = g.to_le_bytes();
    let [b_lo, b_hi] = b.to_le_bytes();
    let [a_lo, a_hi] = a.to_le_bytes();
    let [d0, d1, d2, d3, d4, d5, d6, d7] = dest else {
        return;
    };
    *d0 = r_lo;
    *d1 = r_hi;
    *d2 = g_lo;
    *d3 = g_hi;
    *d4 = b_lo;
    *d5 = b_hi;
    *d6 = a_lo;
    *d7 = a_hi;
}

/// Hard ceiling on the tile-upload serializer pool's worker count (0.96.1).
///
/// **Why a ceiling at all, and why this one.** 0.96.0 used `rayon`'s
/// implicit global pool, which sizes itself to
/// `std::thread::available_parallelism()` — every logical core — with no
/// bound. On an otherwise idle machine that is the fastest choice and it
/// measured as one (PLAN.md's 0.96.0 table). Under **CPU contention** it is
/// the opposite: a caller that dispatches onto the pool synchronously blocks
/// until the *slowest* of up to 64 blocks finishes, so every worker that has
/// to wait for a scheduler time-slice adds to that caller's critical path,
/// while the sequential code it replaced only ever needed one core's slice.
/// An independent review measured that as a **4-5× regression** with 8
/// competing CPU-bound threads on a 4-physical/8-logical core box:
/// `upload_sync` mean 28.5 ms against the sequential path's 5.2 ms, i.e. one
/// stage alone exceeding the whole 16.7 ms frame budget.
///
/// **The bound is no longer the only mitigation** (0.96.2): the frame path,
/// [`TileResidency::sync`], now calls the sequential core directly and never
/// touches this pool, because bounding it cut the contended regression by
/// only about a quarter. The ceiling stays for the callers that do use the
/// pool ([`TileResidency::upload_mip`]) and for whatever load-sensing design
/// eventually re-enables it on the frame path.
///
/// Four is chosen as "a small number that is still real parallel width":
/// it is the physical core count of the machine both the win and the
/// regression were measured on, it cannot oversubscribe a machine with
/// hyper-threading, and it keeps `SERIALIZER_MAX_THREADS` well below the
/// core count of the larger machines this code will meet. **This bounds the
/// worst case; it does not eliminate it** — PLAN.md's 0.96.1 entry carries
/// the re-measured contended numbers, including the residual regression.
const SERIALIZER_MAX_THREADS: usize = 4;

/// Workers [`serializer_pool`] asks for: one fewer than the machine's
/// logical core count, capped at [`SERIALIZER_MAX_THREADS`], and 0 (meaning
/// "don't build a pool, run inline") on a machine too small for the split
/// to buy anything.
///
/// The `- 1` leaves headroom for the rest of a running Aurora — the
/// background tile writer, `tokio`'s I/O threads, the compositor — rather
/// than claiming every core for a stage that is a fraction of the frame.
/// A result below 2 returns 0: a one-worker pool is pure handoff cost,
/// because `ThreadPool::install` blocks the calling thread on a latch and
/// does *not* let it join the work, so a single worker would serialize the
/// same bytes on a different thread while the frame thread idled.
fn serializer_pool_threads() -> usize {
    let usable = std::thread::available_parallelism()
        .map_or(0, |n| n.get().saturating_sub(1))
        .min(SERIALIZER_MAX_THREADS);
    if usable < 2 { 0 } else { usable }
}

/// Builds the serializer's pool, or returns `None` — **never panics, and
/// that is the entire point of this function** (0.96.1).
///
/// `rayon`'s implicit global pool, which 0.96.0 used, is initialized lazily
/// and `rayon_core::registry::global_registry` `.expect()`s the `Result` of
/// building it. Anything but `is_unsupported()` (which covers wasm, not a
/// spawn failure) therefore panics *inside `rayon`* — and the release
/// profile sets `panic = "abort"`, so that is `SIGABRT` with no unwind, no
/// save and no crash dialog — and in 0.96.0/0.96.1 it sat on a path
/// [`TileResidency::sync`] ran every frame, including for the app's own
/// default startup document. (0.96.2 took `sync` off the pool for unrelated
/// performance reasons, which shrinks the exposure but does not retire this
/// fix: [`TileResidency::upload_mip`] still dispatches onto the pool, and the
/// frame path is deliberately kept one routing decision away from doing so
/// again.) Reproduced under
/// `ulimit -u`, and equally reachable from a cgroup `pids.max`, systemd
/// `TasksMax`, or memory pressure:
///
/// ```text
/// thread '...' panicked at rayon-core-1.13.0/src/registry.rs:171:10:
/// The global thread pool has not been initialized.: ThreadPoolBuildError {
///     kind: IOError(Os { code: 11, kind: WouldBlock, ... }) }
/// ```
///
/// Owning the pool is what makes the failure a value instead of a panic:
/// [`rayon::ThreadPoolBuilder::build`] returns a `Result`, this captures
/// it, and a failure degrades to the sequential serializer — which is
/// exactly the code that ran before 0.96.0, so the fallback is a known-good
/// path rather than an untried one. Warned once, not per frame, because
/// [`serializer_pool`]'s `OnceLock` initializer runs once.
fn build_serializer_pool(threads: usize) -> Option<rayon::ThreadPool> {
    if threads == 0 {
        return None;
    }
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("aurora-tile-serialize-{i}"))
        .build()
    {
        Ok(pool) => Some(pool),
        Err(err) => {
            tracing::warn!(
                %err,
                threads,
                "tile uploads will serialize on the calling thread: rayon could not \
                 build a thread pool"
            );
            None
        }
    }
}

/// The process-wide, lazily built serializer pool, or `None` when this
/// machine could not or should not have one — see [`build_serializer_pool`]
/// for why it is owned rather than `rayon`'s global, and
/// [`serializer_pool_threads`] for its size.
///
/// One pool for the process, not one per [`TileResidency`]: worker threads
/// are the scarce resource being bounded, and two atlases (a future second
/// viewport) must share the bound rather than double it.
fn serializer_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| build_serializer_pool(serializer_pool_threads()))
        .as_ref()
}

/// [`serialize_premultiplied_le_bytes`], split across the
/// [`serializer_pool`]'s worker threads at [`BLOCK_SAMPLES`]-sample
/// granularity. **This is 0.96.0's actual change**; everything it calls is
/// 0.92.x code.
///
/// `texels` is split by `par_chunks(BLOCK_SAMPLES)` and `out` by
/// `par_chunks_mut(BLOCK_SAMPLES * 2)` — two output *bytes* per input
/// sample — and the two are `zip`ped so block *k* of the input always
/// meets block *k* of the output. A whole [`TILE`]×[`TILE`] tile
/// ([`SAMPLES`] samples) therefore becomes exactly 64 independent work
/// items. (Work *items*, not tasks: `rayon` splits a parallel iterator
/// adaptively by work-stealing, so how many actual jobs those 64 blocks
/// become depends on how many workers are free. 64 is the granularity the
/// split can reach, not a count of spawns.)
///
/// **Why the output is bit-identical to the sequential version, regardless
/// of thread count or scheduling.** There is no shared mutable state: no
/// accumulator, no reduction, no cross-block carry. Each work item owns a
/// disjoint `&mut [u8]` sub-slice (that disjointness is what
/// `par_chunks_mut` guarantees and what the borrow checker enforces), and
/// each one's output depends only on its own input block. Because
/// `BLOCK_SAMPLES % CHANNELS == 0`, a texel never straddles a block
/// boundary, so no worker can see a partial texel that the sequential walk
/// would have seen whole; and because `BLOCK_SAMPLES % CHUNK_SAMPLES ==
/// 0`, every block but possibly the last is a whole number of vectorized
/// chunks, so no block is pushed onto the scalar remainder path that the
/// sequential walk would have vectorized. There is consequently **no
/// run-to-run non-determinism to disclose**: the bytes are a pure function
/// of the input, and `the_parallel_serializer_matches_the_scalar_reference_across_block_boundaries`
/// pins that against the scalar oracle. It is also what makes 0.96.1's
/// sequential *fallback* a drop-in rather than a second implementation to
/// keep in step: both spellings produce the same bytes by construction.
///
/// `par_chunks`, not `par_chunks_exact`: a short trailing block then goes
/// through the *same* code path as every other block, with
/// `serialize_premultiplied_le_bytes`'s own `chunks_exact`/remainder
/// handling doing what it already did sequentially, instead of needing a
/// separate remainder branch here that could disagree with it.
///
/// A mismatched pair of chunk sizes — the one real way to get this wrong —
/// would silently write correct bytes at wrong offsets, which is why
/// `the_block_and_chunk_constants_divide_a_whole_tile_evenly` pins the
/// arithmetic and the boundary sweep below checks every length class.
///
/// # Within a tile, still not across tiles
///
/// Text before 0.96.0 said parallelizing "is NOT done" and gave two
/// reasons: the new dependency, and [`TileResidency::sync`]'s reused
/// `Vec<u8>`. The first is now spent. The second was real but *incidental*
/// — the buffer only ever held one tile at a time, so it was never what
/// stood in the way. **The across-tile blockers are three, and two of them
/// are API facts no amount of restructuring in this crate can move:**
///
/// 1. `aurora_tile::TileStore::get` takes `&mut self`, and the type exposes
///    no `&self` tile accessor at all. A caller therefore cannot hold two
///    tiles' texels at once — not without first copying each tile out
///    sequentially, which is exactly the per-tile half-megabyte copy 0.68.0
///    removed, so the parallel gain would be paid for with the cost the
///    whole path was restructured to avoid. (Through 0.96.0 this said
///    "there is nothing for a `par_iter` across tiles to iterate over",
///    which overstated it: a caller *could* build such a collection, just
///    not for free.)
/// 2. `TileStore` is `Send` but **not** `Sync` — its `BackgroundWriter`
///    holds an `mpsc::Receiver` — so the store cannot be shared with a
///    worker thread either.
/// 3. [`TileResidency::sync`]'s `bytes_left` budget must decide *which*
///    tiles upload, in a fixed row-major order, before any of them is
///    touched (three tests pin the resulting retry-on-a-later-call
///    behaviour and its tile identities). Any across-tile scheme has to
///    reproduce that ordering exactly, which is a redesign of the budget,
///    not a `par_iter`.
///
/// Block-level parallelism has none of those problems, touches no store
/// bookkeeping and no budget logic, and gives *more* parallel width than
/// the across-tile design would have: 64 blocks per tile against a measured
/// mean of ~6.8 tiles per frame. PLAN.md's 0.96.0 entry carries the costed
/// across-tile alternative that was considered and not taken, together with
/// that round's measured result; its 0.96.1 entry carries the contended
/// re-measurement and the pool bound that came out of it; and its 0.96.2
/// entry records the resulting decision — the frame path serializes inline
/// and this splitter serves [`TileResidency::upload_mip`] only, pending a
/// load-sensing design. Note what that makes the parallel-width argument
/// above: an argument about *shape*, not a live claim about frame time. An
/// across-tile scheme would inherit the same contended problem this one has,
/// at more cost, so nothing here reopens it.
fn split_premultiplied_le_bytes(texels: &[f16], out: &mut [u8]) {
    texels
        .par_chunks(BLOCK_SAMPLES)
        .zip(out.par_chunks_mut(BLOCK_SAMPLES * 2))
        .for_each(|(block, bytes)| serialize_premultiplied_le_bytes(block, bytes));
}

/// Serializes `texels` into `out`, in parallel on `pool` when that is worth
/// doing and sequentially inline otherwise. The whole parallel/sequential
/// decision lives here, in one `match`, so both arms are reachable from one
/// test.
///
/// `pool` is a parameter rather than a call to [`serializer_pool`] purely so
/// that `the_serializer_falls_back_to_the_sequential_core_without_a_pool`
/// can drive the `None` arm — the arm a machine that cannot spawn threads
/// takes — with the identical dispatch a real frame uses.
///
/// **Not on the frame path (0.96.2).** [`TileResidency::sync`] called this
/// through [`write_premultiplied_le_bytes`] in 0.96.0/0.96.1 and no longer
/// does: it calls [`serialize_premultiplied_le_bytes`] directly, because the
/// parallel arm was measured regressing the *whole frame* ~2.1x under
/// ordinary CPU contention (34.34/34.57/36.70 ms mean against the sequential
/// path's 14.59/17.95/17.59 ms) to buy ~0.5 ms idle, where the budget already
/// passed. `sync`'s own call site carries the numbers and the argument; this
/// function and everything below it stay in place, correct, tested, and used.
///
/// **The size guard.** A `texels` no longer than one [`BLOCK_SAMPLES`]
/// yields a single `par_chunks` block, so there is no parallel width to
/// win; taking it inline skips `install`'s job injection and latch wait
/// entirely. Real traffic, therefore, is all
/// [`TileResidency::upload_mip`]'s: 16 blocks at mip level 1 (parallel), 4 at
/// level 2 (parallel) and exactly 1 at level 3, which lands on the inline
/// arm. A *load*-sensing heuristic was considered for 0.96.1 and
/// deliberately not built: there is no cheap, portable way to read "is this
/// machine busy right now" from inside a frame — `getloadavg` is a 1-minute
/// average and Linux-only in `std`'s absence, and a per-frame timing feedback
/// loop is a design, not a patch. Bounding the pool
/// ([`SERIALIZER_MAX_THREADS`]) was 0.96.1's mitigation and was not enough,
/// which is what 0.96.2's routing change settles; PLAN.md's 0.96.1 and
/// 0.96.2 entries state both.
fn write_premultiplied_le_bytes_on(
    pool: Option<&rayon::ThreadPool>,
    texels: &[f16],
    out: &mut [u8],
) {
    match pool {
        Some(pool) if texels.len() > BLOCK_SAMPLES => {
            pool.install(|| split_premultiplied_le_bytes(texels, out));
        }
        _ => serialize_premultiplied_le_bytes(texels, out),
    }
}

/// [`write_premultiplied_le_bytes_on`] against the process's own
/// [`serializer_pool`].
///
/// Through 0.96.1 this was "what every real upload path calls". As of 0.96.2
/// its only production caller is [`extend_premultiplied_le_bytes`], i.e.
/// [`TileResidency::upload_mip`]; [`TileResidency::sync`] bypasses it for the
/// sequential core, for the reason its call site records.
fn write_premultiplied_le_bytes(texels: &[f16], out: &mut [u8]) {
    write_premultiplied_le_bytes_on(serializer_pool(), texels, out);
}

/// Appends `texels` to `out` as the little-endian `f16` bytes
/// `wgpu::Queue::write_texture` wants, **premultiplied on the way**
/// ([`premultiply_rgba`]'s arithmetic, applied per texel as the bytes are
/// written rather than in a separate pass over a separate buffer).
///
/// **A thin `Vec`-sizing wrapper as of 0.96.0, and no longer on the frame
/// path.** It resizes `out` to hold the whole-texel prefix and hands the
/// actual work to [`write_premultiplied_le_bytes`]. Its only remaining
/// caller is [`TileResidency::upload_mip`] (which has no call site in
/// `aurora-app` yet — see that method's own doc for why what it writes is
/// not currently visible); [`TileResidency::sync`] calls
/// [`serialize_premultiplied_le_bytes`] directly, against a buffer it has
/// already sized to exactly one tile and reuses.
///
/// **The substantive *why* is not here any more.** Through 0.96.0 this doc
/// comment carried the 0.68.0 one-buffer history, the 0.88.1 measurement
/// naming this loop rather than the bus, the 0.89.0 append batching, the
/// `wide`-vs-`half` crate evaluation and the bit-exactness/NaN analysis —
/// correct while this function *was* `sync`'s entry point and the hot loop,
/// and stale the moment 0.96.0's split moved both elsewhere. 0.96.1 moved
/// all of it onto [`serialize_premultiplied_le_bytes`], the function that
/// now actually produces every upload byte. Read it there; this function
/// contributes only the sizing below.
///
/// Same trailing-partial-chunk contract as [`premultiply_rgba`]: a slice
/// whose length is not a multiple of [`CHANNELS`] contributes nothing for
/// its final incomplete texel rather than emitting corrupt bytes.
fn extend_premultiplied_le_bytes(texels: &[f16], out: &mut Vec<u8>) {
    // Whole texels only, matching the trailing-partial-texel contract
    // above: two output bytes per input sample.
    let start = out.len();
    let whole_texel_bytes = texels.len() / CHANNELS * CHANNELS * 2;
    out.resize(start + whole_texel_bytes, 0);
    // `Some` by construction: `resize` just made `out` exactly
    // `start + whole_texel_bytes` long, and `start <= out.len()`, so
    // `start..` is always a valid range. Spelled as a guard rather than an
    // index or an `expect` because the workspace denies both, and because a
    // hypothetical `None` writing nothing is a blank tile rather than an
    // aborted process.
    if let Some(tail) = out.get_mut(start..) {
        write_premultiplied_le_bytes(texels, tail);
    }
}

/// The result of one [`TileResidency::sync`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub uploaded: u32,
    pub bytes_uploaded: u64,
    /// Tiles that still need upload after this call — budget-limited or
    /// errored. Non-zero means: request another frame, don't consider
    /// this view fully caught up yet.
    pub remaining: u32,
    /// Subset of `remaining` that failed to load (not merely
    /// budget-skipped) — a distinct, exceptional condition worth
    /// surfacing separately from "just not enough budget this frame."
    pub errors: u32,
}

/// A GPU-resident window over a tile store: a tile-aligned atlas texture
/// sized to a viewport (plus one tile of margin), whose slots are
/// addressed toroidally (`tile index modulo grid size`) so that panning
/// by one tile invalidates one row or column of uploads, not the whole
/// texture. Ported from `spike/vertical-slice`'s `Renderer` (real,
/// measured — `spike/FINDINGS.md`), generalized to build against the
/// real `aurora_tile::TileStore` API rather than the spike's own
/// throwaway store.
///
/// Handles window resize via [`Self::resize`], which rebuilds the atlas
/// texture at the new size and resets slot occupancy — see that
/// method's own doc comment for exactly what carries over and what
/// doesn't.
pub struct TileResidency {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    /// Slots across, slots down.
    grid: (u32, u32),
    /// Which tile currently occupies each modulo-addressed slot.
    slots: HashMap<(u32, u32), TileId>,
    /// Top-left visible tile — the whole-tile part of the last
    /// [`Self::set_origin`] call's `doc_origin`, still exactly what
    /// [`Self::sync`]/[`Self::visible_tiles`]/slot addressing need for
    /// atlas upload bookkeeping.
    origin: TileId,
    /// The fractional remainder of the last [`Self::set_origin`] call's
    /// `doc_origin` within `origin`'s own tile, `[0, TILE)` on each axis
    /// (`doc_origin - origin * TILE`) — what makes the atlas's own
    /// sampled UV offset sub-tile-accurate instead of snapping to the
    /// nearest tile boundary (the bug this field exists to fix: painted
    /// content landing offset from the cursor after any zoom/pan, and
    /// panning under one tile not visibly moving anything). Folded into
    /// [`Self::write_uniform`]'s `scroll` alongside `origin * TILE`.
    sub_tile: (f32, f32),
}

impl TileResidency {
    /// The largest document coordinate [`Self::set_origin`] will
    /// address, in document pixels: [`MAX_DOCUMENT_EXTENT`], the
    /// project's own 300,000 px document ceiling (ADR 0002, matching
    /// Adobe PSB). The bound is on the **layer-local** origin
    /// `set_origin` receives (`aurora_app`'s own `canvas_local_origin`,
    /// i.e. the document position at the canvas's top-left corner minus
    /// the active layer's own origin), not on a raw document
    /// coordinate — so a caller mirroring it in document space measures
    /// it *from that layer's own origin*, not from `(0, 0)`.
    ///
    /// This is a **safety bound, not a policy**: `origin` is a
    /// whole-tile index and this type's own internal uniform-buffer
    /// write multiplies it back
    /// by [`TILE`], so an unbounded `doc_origin` (`f32::MAX`,
    /// `f32::INFINITY`, or simply a number around 4.295e9 reached by
    /// sustained panning at a very low zoom) overflows that `u32`
    /// multiply — a hard panic in debug, where this workspace denies
    /// `panic` precisely because "a panic loses unsaved work", and a
    /// silent wrap addressing the wrong tile in release. No real
    /// document extends past this ceiling, so clamping to it cannot
    /// cost a caller anything it was entitled to.
    ///
    /// **Public for the same reason [`Self::min_zoom_for_viewport`] is,
    /// and it is the same bug** (0.57.10). The render path is not the
    /// only consumer of the canvas transform: `aurora_ui::CanvasView::
    /// to_document` converts pointer positions to document space for
    /// painting, and past this ceiling the private `clamp_doc_origin`
    /// saturates the *rendered* origin while `to_document` keeps
    /// reporting the true, unbounded position — render and paint
    /// silently diverge, exactly as they did below zero before
    /// `CanvasView::clamp_pan_to_minimum` existed. The clamp here is
    /// the backstop, not the mechanism:
    /// **`aurora_ui::CanvasView::clamp_pan_to_maximum` is the caller
    /// responsible for keeping the view inside this bound**, driven
    /// from `aurora-app`'s own `PanBounds`, which measures it as the
    /// active layer's origin plus this constant. This crate sits below
    /// `aurora-ui` in the layering (PRD §7.2) and so cannot reach into
    /// `CanvasView` itself.
    pub const MAX_DOC_ORIGIN_PX: f32 = MAX_DOCUMENT_EXTENT as f32;

    /// Sizes the atlas to `viewport_px`, rounded up to whole tiles plus
    /// one tile of margin (matches the spike's `ct = viewport/TILE + 1`
    /// exactly), and establishes an initial origin of `(0, 0)`.
    #[must_use]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, viewport_px: (u32, u32)) -> Self {
        let grid = Self::grid_for(viewport_px);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile-residency"),
            size: wgpu::Extent3d {
                // Saturating for the same reason [`Self::grid_for`]
                // is: an absurd viewport must reach wgpu's own size
                // validation as an error, not panic on the multiply
                // first.
                width: grid.0.saturating_mul(TILE),
                height: grid.1.saturating_mul(TILE),
                depth_or_array_layers: 1,
            },
            mip_level_count: MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            // COPY_SRC beyond the two production-path flags (TEXTURE_BINDING
            // for sampling, COPY_DST for uploads) so the atlas can be read
            // back for verification -- real capability, not test-only scope
            // creep: debugging/inspection tooling will want this too.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // **Sampling sees mip level 0 and nothing else.** Restricting the
        // view rather than clamping the sampler's LOD is deliberate and
        // measured -- see [`MIP_LEVELS`] for the full rationale, the bug
        // this fixes, and why the sampler-side spelling is not
        // equivalent. `upload_mip` still writes through `texture`
        // directly, so the other levels remain writable and readable.
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("tile-residency"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..wgpu::TextureViewDescriptor::default()
        });
        // The wraparound is the hardware sampler's job, not WGSL's --
        // matches the spike exactly (`AddressMode::Repeat` both axes).
        //
        // `Repeat` is load-bearing and must stay: slot addressing is
        // toroidal (`tile index modulo grid size`), so whenever the
        // visible window straddles the atlas's own right or bottom edge
        // -- most pan positions, at *every* zoom, since `uv_scale` is
        // already 0.833 at 100% on a 1920 px viewport -- the far side of
        // the screen legitimately samples `uv > 1.0` and has to wrap
        // around to slot 0 to find the tile that belongs there.
        // `ClampToEdge` cannot tell that wrap apart from an
        // out-of-coverage one and was measured to smear the atlas's edge
        // column across half the canvas during ordinary 100%-zoom
        // panning (`render_test.rs`'s
        // `canvas_pipeline_wraps_to_the_toroidal_slot_when_the_window_straddles_the_atlas_edge`).
        // Keeping the sampled window inside the atlas's real coverage is
        // `write_uniform`'s job instead.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tile-residency"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile-residency-uniform"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let residency = Self {
            texture,
            view,
            sampler,
            uniform_buffer,
            grid,
            slots: HashMap::new(),
            origin: TileId { x: 0, y: 0 },
            sub_tile: (0.0, 0.0),
        };
        residency.write_uniform(queue, viewport_px, 1.0);
        residency
    }

    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    #[must_use]
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// For building a bind group externally — `TileResidency` doesn't
    /// know about bind group layouts, the same boundary `PipelineCache`
    /// already draws.
    #[must_use]
    pub fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    #[must_use]
    pub const fn origin(&self) -> TileId {
        self.origin
    }

    /// Every [`TileId`] currently within this atlas's own visible grid,
    /// document-space, in the same fixed row-major order [`Self::sync`]
    /// itself iterates — what a caller computing its own per-tile
    /// content (rather than reading a single [`TileStore`] surface
    /// [`Self::sync`] can already do directly) needs to know what to
    /// actually produce. `aurora-render`'s multi-layer CPU compositing
    /// (`composite_tile_cpu`) is the first real consumer: its own
    /// orchestration (in `aurora-app`, which depends on both this crate
    /// and `aurora-doc`) must run *before* the next [`Self::sync`] call
    /// each frame, so the surface `sync` then reads has fresh,
    /// already-composited content.
    pub fn visible_tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        let origin = self.origin;
        let grid = self.grid;
        (0..grid.1).flat_map(move |gy| {
            (0..grid.0).map(move |gx| TileId {
                x: origin.x + gx,
                y: origin.y + gy,
            })
        })
    }

    /// Call when the visible top-left document position changes (panning
    /// or zooming) or `zoom` itself changes. Updates the UV uniform
    /// immediately; the texture itself is only touched by the next
    /// [`Self::sync`].
    ///
    /// `doc_origin`: the exact, continuous document-local position (e.g.
    /// `aurora_app`'s own `canvas_local_origin`) that should render at
    /// the canvas's own top-left corner — **not** pre-floored to a whole
    /// tile by the caller. Clamped to `[0, MAX_DOC_ORIGIN_PX]` here: to
    /// non-negative because `TileId`'s own fields are unsigned, so a
    /// negative document position has no tile to point to (the same
    /// "outside the surface" case this atlas has always clamped, now
    /// made explicit at this one boundary instead of happening inside
    /// every caller), and to the document ceiling at the top because
    /// `write_uniform` multiplies the whole-tile part back by [`TILE`]
    /// in `u32`, which an unbounded value overflows — a hard panic in
    /// debug, a wrong tile silently addressed in release (this module's
    /// own [`Self::MAX_DOC_ORIGIN_PX`] constant carries the full
    /// reasoning). A
    /// NaN lands on `0.0`, not on NaN (`f32::max` returns the non-NaN
    /// operand, which is why this is spelled `max`/`min` and not
    /// `clamp` — `f32::clamp` propagates NaN). Split into `origin` (the
    /// whole-tile part, via floor-division by [`TILE`] — still exactly
    /// what [`Self::sync`]/[`Self::visible_tiles`]/slot addressing need)
    /// and `sub_tile` (the fractional remainder within that tile,
    /// `[0, TILE)` on each axis) — both private fields, folded into the
    /// atlas's own sampled UV offset by this type's own internal
    /// uniform-buffer write, so the rendered content lands at the true
    /// fractional position instead of snapping to the nearest tile
    /// boundary.
    ///
    /// `zoom`: document pixels per logical screen pixel, matching
    /// `aurora_ui::CanvasView::zoom`'s own convention (`1.0` = 100%,
    /// `> 1.0` magnifies). Shrinks `uv_scale` by this factor — at 200%
    /// zoom, half as many atlas texels stretch across the same
    /// viewport, magnifying them — the shader-side scaling this
    /// texture-sliding-window design needs instead of an actual bigger
    /// upload (the atlas itself is still sized in document pixels, one
    /// tile of margin at 100%, unrelated to `zoom`).
    ///
    /// Callers should pass a positive, finite `zoom`, but a bad one is
    /// *handled* rather than merely forbidden: this type's own
    /// uniform-buffer write substitutes `1.0` for zero, negative,
    /// infinite and NaN. This used
    /// to claim no guard was needed because `aurora_ui::CanvasView`
    /// clamps to `[MIN_ZOOM, MAX_ZOOM]`, which was wrong — `aurora-app`
    /// passes `effective_residency_zoom(canvas_zoom, scale_factor)`, a
    /// product formed in another crate whose own guard covers
    /// `scale_factor` and not `canvas_zoom`, so the clamped value is not
    /// what arrives here.
    ///
    /// Zooming *out* saturates rather than continuing indefinitely: the
    /// atlas is sized from the viewport alone and holds only what that
    /// viewport needs at 100%, so a `zoom` below
    /// [`Self::min_zoom_for_viewport`] renders at that floor instead of
    /// shrinking further. **Callers that also convert pointer positions
    /// to document space must clamp their own zoom to that same value**
    /// — [`Self::min_zoom_for_viewport`]'s doc comment explains why, and
    /// `aurora-app`/`aurora_ui::CanvasView::set_min_zoom` is the real
    /// caller doing it. The clamp here is the backstop, not the
    /// mechanism.
    pub fn set_origin(
        &mut self,
        queue: &wgpu::Queue,
        doc_origin: (f32, f32),
        viewport_px: (u32, u32),
        zoom: f32,
    ) {
        let (x, y) = Self::clamp_doc_origin(doc_origin);
        let tile_size = TILE as f32;
        #[allow(clippy::cast_sign_loss)]
        let origin = TileId {
            x: (x / tile_size).floor() as u32,
            y: (y / tile_size).floor() as u32,
        };
        self.sub_tile = (
            x - (origin.x as f32) * tile_size,
            y - (origin.y as f32) * tile_size,
        );
        self.origin = origin;
        self.write_uniform(queue, viewport_px, zoom);
    }

    /// Rebuilds the atlas at a new `viewport_px` — the real fix for the
    /// limitation this struct's own doc comment used to name. There is
    /// no in-place way to resize a `wgpu::Texture`, so this reconstructs
    /// `texture`/`view`/`sampler`/`uniform_buffer` via [`Self::new`]'s
    /// own construction logic (`*self = Self::new(...)`) rather than
    /// duplicating it, which also resets `slots` to empty exactly as a
    /// freshly-constructed atlas starts. That reset matters for
    /// correctness, not just tidiness: every slot coordinate in the old
    /// `HashMap` was computed against the *old* `grid` (`tile index
    /// modulo grid size`) and is meaningless — even out of bounds — for
    /// the new one, so the next [`Self::sync`] call must re-upload every
    /// visible tile fresh rather than trusting stale bookkeeping.
    ///
    /// The document-space `origin` (which tile is top-left) carries over
    /// unchanged, and so does `sub_tile`, the fractional position within
    /// that tile — a resize changes how much of the document is
    /// visible, not *which* part is being viewed, and that applies just
    /// as much to a sub-tile pan offset as to the whole-tile part. (Only
    /// `origin` used to be restored, so a resize silently snapped the
    /// view back to the nearest tile boundary; the doc comment listed
    /// what carried over and did not mention that `sub_tile` didn't.)
    /// `slots` is the one thing that deliberately does *not* carry over,
    /// for the reason above. `zoom` isn't carried
    /// over (this method has no way to know the caller's current value,
    /// and `TileResidency` doesn't store it between calls), so the
    /// uniform is rewritten at `zoom = 1.0` same as [`Self::new`]; a
    /// caller that cares about zoom being exactly right for the one
    /// frame between a resize and its next [`Self::set_origin`] call
    /// should pass its current zoom there too. `aurora-app`'s real usage
    /// calls `set_origin` every frame before `sync` regardless, so both
    /// `origin` and `zoom` are corrected before anything is drawn.
    ///
    /// No-ops on a zero-sized request (a minimized window can report
    /// `0x0`), mirroring [`crate::GpuSurface::resize`]'s own guard
    /// against calling into wgpu with an invalid size, which panics.
    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, viewport_px: (u32, u32)) {
        if viewport_px.0 == 0 || viewport_px.1 == 0 {
            return;
        }
        let origin = self.origin;
        let sub_tile = self.sub_tile;
        *self = Self::new(device, queue, viewport_px);
        self.origin = origin;
        self.sub_tile = sub_tile;
        self.write_uniform(queue, viewport_px, 1.0);
    }

    /// The atlas grid — slots across, slots down — for a viewport:
    /// `viewport.div_ceil(TILE) + 1` on each axis (the spike's own
    /// `ct = viewport/TILE + 1`; the `+ 1` is one tile of margin, which
    /// is exactly what a sub-tile pan offset consumes).
    ///
    /// Shared by [`Self::new`] and [`Self::min_zoom_for_viewport`] so
    /// the atlas's real size and the zoom floor derived from that size
    /// can never be computed from two different formulas.
    ///
    /// **Saturating, not wrapping.** A viewport within one tile of
    /// `u32::MAX` makes `div_ceil(TILE) + 1` overflow, and every caller
    /// then multiplies the result back up by `TILE` — which overflows
    /// well before that, from a viewport of about 4.295e9 px. Both are a
    /// hard `panic` in debug (this workspace denies `panic` precisely
    /// because a panic loses unsaved work) and a silently wrong atlas
    /// size in release. No real window reports such a viewport, but
    /// `min_zoom_for_viewport` is now reached from `aurora-app`'s own
    /// `redraw` on **every** frame, before the GPU/surface early-return
    /// and with no GPU involved at all, so a bad viewport no longer has
    /// to survive wgpu's own validation to get here. This is exactly the
    /// defect class [`Self::clamp_doc_origin`] already guards on the
    /// document axis; guarding one and not the other would be an
    /// inconsistency, not a judgement call. Saturating leaves
    /// [`Self::zoom_floor`] computing `viewport / (u32::MAX - TILE)` ≈
    /// 1.0 — a sane, conservative floor, not garbage.
    fn grid_for(viewport_px: (u32, u32)) -> (u32, u32) {
        (
            viewport_px.0.div_ceil(TILE).saturating_add(1),
            viewport_px.1.div_ceil(TILE).saturating_add(1),
        )
    }

    /// The smallest zoom an atlas of `tex_w` × `tex_h` texels can render
    /// `viewport_px` at without either fabricating content or distorting
    /// it. One scalar, both axes.
    ///
    /// Derivation: at `zoom`, the window wants `viewport_px / zoom`
    /// document pixels per axis. The atlas covers `tex_size` texels
    /// starting at `origin * TILE`, and the window starts `sub_tile`
    /// into that, so `tex_size - sub_tile` are genuinely resident ahead
    /// of it. `wanted <= tex_size - sub_tile` for every reachable
    /// `sub_tile ∈ [0, TILE)` therefore means `wanted <= tex_size -
    /// TILE`, i.e. `zoom >= viewport_px / (tex_size - TILE)`.
    ///
    /// **`tex_size - TILE`, not the current frame's `tex_size -
    /// sub_tile`.** Using the live `sub_tile` makes the bound — and with
    /// it the rendered scale — move continuously as the user pans, at a
    /// constant user zoom, then snap back once per tile of panning
    /// (measured: 0.500 → 0.889, a 1.78× scale change across one tile
    /// of pan at zoom 0.5). Taking the worst case instead makes the
    /// bound pan-independent by construction: nothing here reads
    /// `sub_tile` at all.
    ///
    /// **One scalar for both axes, not a floor per axis.** The atlas is
    /// not viewport-proportional (each axis is rounded up to whole tiles
    /// separately), so the two axes saturate at different zooms;
    /// clamping them independently leaves one axis still shrinking while
    /// the other has stopped, which stretches the image — measured at up
    /// to 33.6% on a 512 × 256 viewport, ~18.5% permanent horizontal
    /// stretch on 1920 × 1080. Circles rendered as ellipses. Applying
    /// the larger of the two floors to `zoom` itself keeps both axes at
    /// exactly the same scale by construction.
    fn zoom_floor(viewport_px: (u32, u32), tex_w: f32, tex_h: f32) -> f32 {
        let tile = TILE as f32;
        // `.max(tile)`: a zero-sized viewport gives a one-tile atlas, so
        // `tex - TILE` is `0.0` and the division would be `0/0` (NaN) or
        // `n/0` (infinity). One tile is the smallest coverage that can
        // exist, so it is the right stand-in.
        let fx = viewport_px.0 as f32 / (tex_w - tile).max(tile);
        let fy = viewport_px.1 as f32 / (tex_h - tile).max(tile);
        fx.max(fy)
    }

    /// **The zoom floor every consumer of this atlas's geometry must
    /// clamp to.** Below it, [`Self::set_origin`] renders at this value
    /// instead of the zoom it was given.
    ///
    /// This is public because the *render* path is not the only consumer
    /// of the canvas transform: `aurora_ui::CanvasView::to_document`
    /// converts pointer positions to document space for painting, and if
    /// it divides by a zoom this atlas silently declined to honour, a
    /// click paints somewhere other than where the pixel under the
    /// cursor is drawn. That is not hypothetical — it was measured at
    /// canvas zoom 0.25 on a 1920 px viewport (a click at screen x = 960
    /// converting to document x ≈ 3840 while the pixel drawn there was
    /// document x ≈ 1152) and it is the same failure shape
    /// `CanvasView::clamp_pan_to_minimum` already exists to prevent on
    /// the pan axis. The fix is the same one that method documents:
    /// clamp at the source, so every downstream consumer reads one
    /// already-bounded value and none of them computes its own.
    ///
    /// `aurora-app` is the real caller (`canvas_min_zoom` →
    /// `CanvasView::set_min_zoom`); this crate sits below `aurora-ui` in
    /// the layering (PRD §7.2) and so cannot reach into `CanvasView`
    /// itself.
    ///
    /// **What it costs, stated plainly.** The value is
    /// `viewport_px / (viewport_px rounded up to whole tiles)`, which
    /// for any viewport at least one tile (256 px) across is in
    /// `(0.5, 1.0]` — 0.9375 for a 1920 px axis, exactly 1.0 when the
    /// viewport is a whole number of tiles. (A viewport *smaller* than
    /// one tile has genuine room to zoom out, since the atlas's own
    /// minimum is two tiles; nothing a real window reaches.) So on a 1×
    /// display
    /// zooming out is currently a no-op, and on a 2× display it stops at
    /// about 50% canvas zoom (the atlas is sized in physical pixels).
    /// The atlas is a 1:1 sliding window over the document with exactly
    /// one tile of margin, and one tile of margin is exactly what a
    /// sub-tile pan consumes — it has never had the coverage to render
    /// minified, and the two previous attempts to paper over that
    /// (wrapping the sampler, then clamping the sampled window per axis)
    /// each traded the missing coverage for something worse: duplicated
    /// content, then a pan-dependent, anisotropic scale. Real zoom-out
    /// needs the atlas sized from zoom, or progressive/LOD rendering —
    /// both M1.3 work, both larger than a bug fix.
    #[must_use]
    pub fn min_zoom_for_viewport(viewport_px: (u32, u32)) -> f32 {
        let grid = Self::grid_for(viewport_px);
        Self::zoom_floor(
            viewport_px,
            grid.0.saturating_mul(TILE) as f32,
            grid.1.saturating_mul(TILE) as f32,
        )
    }

    /// The zoom an atlas sized for `viewport_px` will *actually* render
    /// `zoom` at: the degenerate-value guard and
    /// [`Self::min_zoom_for_viewport`] applied, exactly as
    /// [`Self::set_origin`] applies them internally.
    ///
    /// A caller that has already clamped its own zoom to
    /// [`Self::min_zoom_for_viewport`] gets its own value back unchanged
    /// — which is the property worth testing across a crate boundary,
    /// because it is precisely what "the render path and the pointer
    /// path agree" means.
    #[must_use]
    pub fn effective_zoom(viewport_px: (u32, u32), zoom: f32) -> f32 {
        Self::guard_zoom(zoom).max(Self::min_zoom_for_viewport(viewport_px))
    }

    /// Defensive, not decorative. [`Self::uniform_values`] divides by
    /// `zoom`, and [`Self::set_origin`]'s contract used to argue no
    /// guard was needed because `aurora_ui::CanvasView` clamps zoom to
    /// `[MIN_ZOOM, MAX_ZOOM]`. What `aurora-app` actually passes is
    /// `effective_residency_zoom(canvas_zoom, scale_factor)` — a product
    /// formed in a different crate, whose own guard covers
    /// `scale_factor` and not `canvas_zoom` — so zero, negative,
    /// infinite and NaN all reach here, and each poisons `uv_scale` into
    /// a value that samples nothing recognisable. The fallback matches
    /// `effective_residency_zoom`'s own pattern for a bad
    /// `scale_factor`: treat it as 1.0 rather than render garbage.
    ///
    /// A **denormal** is deliberately not one of these cases: it is
    /// finite and positive, so it is a real (if absurd) zoom, and the
    /// zoom floor absorbs it exactly as it absorbs `MIN_ZOOM`.
    fn guard_zoom(zoom: f32) -> f32 {
        if zoom.is_finite() && zoom > 0.0 {
            zoom
        } else {
            1.0
        }
    }

    /// [`Self::set_origin`]'s `doc_origin` bound — see
    /// [`Self::MAX_DOC_ORIGIN_PX`].
    ///
    /// `max` then `min` rather than `clamp`: `f32::clamp` propagates
    /// NaN, while `f32::max` returns the non-NaN operand, so a NaN
    /// `doc_origin` lands on `0.0` (the document's own top-left corner)
    /// instead of poisoning every derived value.
    // `f32::clamp` is exactly what this must *not* be: it propagates
    // NaN, and a NaN document origin has to land on the document's own
    // top-left corner rather than poison `origin`, `sub_tile`, and the
    // UV offset derived from them. `max` then `min` is the spelling that
    // does that (`f32::max` returns the non-NaN operand).
    #[allow(clippy::manual_clamp)]
    fn clamp_doc_origin(doc_origin: (f32, f32)) -> (f32, f32) {
        (
            doc_origin.0.max(0.0).min(Self::MAX_DOC_ORIGIN_PX),
            doc_origin.1.max(0.0).min(Self::MAX_DOC_ORIGIN_PX),
        )
    }

    /// The four floats `canvas.wgsl`'s `Canvas` uniform expects —
    /// `uv_offset.xy`, `uv_scale.xy` — as a pure function of the atlas's
    /// own state, so the geometry can be tested exactly, without a GPU
    /// adapter, at arbitrary pan positions.
    ///
    /// `uv_scale` is computed from **one** clamped zoom for both axes
    /// (see [`Self::zoom_floor`]), never per axis and never from the
    /// current `sub_tile`. That is what makes the rendered scale
    /// isotropic and pan-independent, and it is why there is no
    /// remaining `min` against the atlas's coverage here: `zoom >=
    /// zoom_floor` already implies `viewport_px / zoom <= tex_size -
    /// TILE <= tex_size - sub_tile` on both axes, so no sample can reach
    /// past the atlas's real coverage into the toroidal wrap and
    /// duplicate document content. `uv_scale_never_reaches_past_the_atlas_coverage`
    /// asserts that implication directly rather than leaving it to a
    /// clamp that would then be both untested and, if it ever did fire,
    /// pan-dependent again.
    fn uniform_values(
        grid: (u32, u32),
        origin: TileId,
        sub_tile: (f32, f32),
        viewport_px: (u32, u32),
        zoom: f32,
    ) -> [f32; 4] {
        let tex_w = grid.0.saturating_mul(TILE) as f32;
        let tex_h = grid.1.saturating_mul(TILE) as f32;
        // The floor is taken against whichever atlas is *smaller*: the
        // one this instance really has, or the one `viewport_px` would
        // build. They are identical in every real call (`aurora-app`
        // resizes the atlas and passes the viewport from the same
        // `canvas_area_physical_size` call), but a caller that grew its
        // viewport without calling `resize` first would otherwise get a
        // floor its actual atlas cannot back, and duplication is the one
        // outcome this must never have.
        //
        // `saturating_mul` here matches `grid_for`/`min_zoom_for_viewport`:
        // `grid`/`capped` are already bounded by `grid_for`'s own
        // `saturating_add`, but re-deriving `tex_w`/`tex_h` from them is
        // still a `u32` multiply and must not reintroduce the overflow
        // panic those two functions exist to close.
        let capped = Self::grid_for(viewport_px);
        let floor = Self::zoom_floor(
            viewport_px,
            grid.0.min(capped.0).saturating_mul(TILE) as f32,
            grid.1.min(capped.1).saturating_mul(TILE) as f32,
        );
        let zoom = Self::guard_zoom(zoom).max(floor);
        // Absolute scroll (origin in texels, a genuinely fractional
        // position -- `origin * TILE` plus the sub-tile remainder), then
        // wrapped into [0, tex_w)/[0, tex_h) for the repeat sampler --
        // slot addressing is toroidal, so the texture is a sliding
        // window over the document, exactly as in the spike, just with
        // sub-tile precision now instead of snapping to a tile boundary.
        //
        // `rem_euclid` rather than a plain `%`: `set_origin` clamps its
        // input to non-negative before computing `origin`/`sub_tile`, so
        // `scroll` itself can never be negative here in practice -- but
        // `rem_euclid` costs nothing extra for an already-non-negative
        // input and stays correct if that upstream invariant ever
        // changes, rather than silently reintroducing a negative-modulo
        // bug (`%`'s sign follows the dividend in Rust, so a plain `%` on
        // a hypothetical future negative `scroll` would return a
        // negative remainder, wrapping the sample into the wrong texel
        // entirely) -- the safer default this comment says explicitly.
        let scroll = (
            (origin.x * TILE) as f32 + sub_tile.0,
            (origin.y * TILE) as f32 + sub_tile.1,
        );
        [
            scroll.0.rem_euclid(tex_w) / tex_w,
            scroll.1.rem_euclid(tex_h) / tex_h,
            (viewport_px.0 as f32 / zoom) / tex_w,
            (viewport_px.1 as f32 / zoom) / tex_h,
        ]
    }

    fn write_uniform(&self, queue: &wgpu::Queue, viewport_px: (u32, u32), zoom: f32) {
        let values = Self::uniform_values(self.grid, self.origin, self.sub_tile, viewport_px, zoom);
        let mut bytes = Vec::with_capacity(UNIFORM_SIZE as usize);
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniform_buffer, 0, &bytes);
    }

    /// Uploads visible slots that don't already hold the correct, clean
    /// tile, up to `byte_budget` bytes this call — a fast pan can expose
    /// far more tiles than fit in one frame's bandwidth
    /// (`spike/FINDINGS.md` finding #3: "~18 MB per screenful"), so this
    /// caps the cost per call rather than uploading everything at once.
    ///
    /// Tiles that don't fit in the budget (or fail to load) aren't
    /// marked resident, so the *next* call's own resident check finds
    /// them again automatically — no separate pending-tile queue needed.
    /// Iterating the grid in the same fixed order every call means a
    /// budget smaller than the full backlog fills in from the start of
    /// that order forward, one call at a time, converging to
    /// `remaining == 0` rather than starving any tile.
    ///
    /// `force`: re-upload every visible slot unconditionally. Still not
    /// exercised by [`Self::resize`] — that method clears `slots`
    /// directly (every slot already reads as non-resident against the
    /// new, empty map, so the ordinary resident check already forces a
    /// full re-upload on the next `sync` without needing this flag) —
    /// so this remains without a real caller, kept for a future case
    /// that wants a full re-sync without a slot-mapping change.
    ///
    /// `surface`: which of `store`'s surfaces this atlas is showing
    /// (ADR 0010 — `store` may hold many). This crate has no document
    /// assembly yet to say *which* surface that should be (a single
    /// layer's own preview vs. a whole document's composited result) —
    /// real, separate follow-on work; today's only callers (this
    /// crate's own tests) just pick one.
    pub fn sync(
        &mut self,
        queue: &wgpu::Queue,
        store: &mut TileStore,
        surface: SurfaceId,
        force: bool,
        byte_budget: usize,
    ) -> SyncStats {
        let mut stats = SyncStats::default();
        let mut bytes_left = byte_budget;
        // *One* buffer, reused across every tile this call uploads, and
        // the only one: `serialize_premultiplied_le_bytes` premultiplies
        // as it serializes, so the store's own tile stays straight alpha
        // without a separate mutable copy of it. 0.68.0 had a staging
        // `Vec<f16>` here *and* a fresh `Vec<u8>` per tile, which is the
        // half-megabyte copy and the per-tile allocation the staging
        // buffer's own comment claimed to be avoiding.
        //
        // **Allocated at full length once (0.96.0), not cleared and
        // re-grown per tile.** The serializer writes into a
        // pre-sized `&mut [u8]` rather than appending, so this buffer has
        // to be `TILE_BYTES` long before the first tile, not merely
        // reserved. That makes it a *reused* buffer in the literal sense —
        // tile N+1 overwrites tile N's bytes in place — which is only safe
        // because the `texels().len() != SAMPLES` guard below refuses any
        // tile that would not overwrite all of it. Without that guard a
        // short tile would upload its own bytes followed by the tail of the
        // previous tile's.
        let mut bytes: Vec<u8> = vec![0u8; TILE_BYTES];
        for gy in 0..self.grid.1 {
            for gx in 0..self.grid.0 {
                let id = TileId {
                    x: self.origin.x + gx,
                    y: self.origin.y + gy,
                };
                let slot = (id.x % self.grid.0, id.y % self.grid.1);
                let resident = self.slots.get(&slot) == Some(&id);
                // Peeked, not taken (0.68.7). Until then this was
                // `take_dirty`, three lines above a budget check that can
                // `continue` -- so a tile the budget skipped had already
                // had its dirty flag consumed and was silently never
                // uploaded on a later frame either, until some unrelated
                // edit marked it dirty afresh. The flag is now consumed
                // only once this loop has committed to uploading.
                let dirty = store.is_dirty(surface, id);
                if !force && resident && !dirty {
                    continue;
                }
                if bytes_left < TILE_BYTES {
                    stats.remaining += 1;
                    continue;
                }
                // Committed: the budget is there and the tile is about to
                // be read and written. Consuming it before `get` rather
                // than after is deliberate -- `get` borrows `store`
                // immutably for the rest of the iteration.
                //
                // **What makes that ordering safe is the `Err` arm's
                // `self.slots.remove(&slot)`, not the store's own
                // residency.** The resident check above is against
                // `self.slots` -- this atlas's slot mapping -- and not
                // against `store`, so a `get` that fails after this line
                // has consumed the dirty record leaves nothing else that
                // remembers an upload is owed: the slot would still map to
                // `id`, read as resident on the next call, and the tile
                // would be skipped for the life of the mapping, silently,
                // with `SyncStats` reporting `errors: 0, remaining: 0`.
                // Dropping the mapping is what turns that into a retry.
                //
                // This became load-bearing in `aurora-tile` 0.91.0: before
                // it, `is_dirty` could only be `true` for a *resident*
                // tile, and a resident tile's `get` cannot fail the way a
                // page-in can, so the failure path was unreachable for a
                // dirty tile. It is reachable now (a scratch-disk read
                // error, or the store dropping a failed write's file), so
                // the ordering needs the slot invalidation to stay sound.
                let _ = store.take_dirty(surface, id);
                let tile = match store.get(surface, id) {
                    Ok(tile) => tile,
                    Err(err) => {
                        // One bad tile shouldn't abort uploading the
                        // rest of the visible grid this frame; there is
                        // nothing more localized to retry against here.
                        // Still needs a real upload attempt later, same
                        // as a budget-skipped tile.
                        //
                        // And this line is what makes that later attempt
                        // actually happen -- see the comment above the
                        // `take_dirty` call for why nothing else would.
                        // `slot` was derived from `id`, but by the time this
                        // runs the map might hold a *different* id at that
                        // same slot (panning can revisit a slot before this
                        // tile does). Removing it is still safe either way:
                        // if it still mapped `slot` to `id`, this is exactly
                        // the invalidation this whole comment is about; if it
                        // had already moved on to some other id, dropping
                        // that entry costs at worst one redundant re-upload
                        // of that other id later -- never a skipped one.
                        self.slots.remove(&slot);
                        // A cost worth naming, not just a mechanism: for a
                        // tile that fails every time (a permanently corrupt
                        // scratch file, or one `cap_failed_writes` already
                        // dropped), this is no longer "retried once, then
                        // silently skipped" -- it is "retried, and warned
                        // about, every single frame, forever." Harmless
                        // today because the only real caller
                        // (`aurora-app`'s `redraw`) discards this
                        // `SyncStats` outright, but a future caller that
                        // honors `remaining != 0` as "request another
                        // frame" would spin indefinitely on this tile.
                        tracing::warn!(?id, %err, "skipping tile for this frame's upload");
                        stats.remaining += 1;
                        stats.errors += 1;
                        continue;
                    }
                };
                // A whole tile, or nothing. `TileStore` hands back
                // `SAMPLES` samples for every tile it has, so this is not
                // expected to fire -- it is what makes the reused,
                // pre-sized `bytes` buffer above sound rather than merely
                // probably-sound. A short tile would leave the tail of the
                // *previous* tile's bytes in the buffer and upload them as
                // if they were this tile's. Handled exactly like the `get`
                // failure above, and for the same reason: drop the slot
                // mapping so the tile is retried instead of silently
                // sticking as resident-but-never-uploaded.
                //
                // **This guard is defensive and, as of 0.96.1, provably
                // unreachable through any current API, which is why it has
                // no test** -- disclosed rather than left as a silent
                // coverage gap. `aurora_tile::Tile`'s only non-blank
                // constructor, `Tile::from_texels`, is `pub(crate)`, so no
                // code outside `aurora-tile` (this crate and its tests
                // included) can build a wrong-length tile at all;
                // `Tile::blank()` is `SAMPLES` by construction and
                // `texels_mut()` hands out a `&mut [f16]`, which cannot
                // resize. Inside `aurora-tile`, both `from_texels` call
                // sites are fed by `codec::decode`, whose own exact-length
                // check `a_truncated_scratch_file_pages_in_as_an_error_not_a_short_tile`
                // pins. Kept anyway, because it is a length compare per
                // tile against a half-megabyte memcpy: cheap insurance
                // against a future `aurora-tile` change (a public
                // `from_texels`, a variable tile size) reintroducing the
                // possibility, and the cost of being wrong here is a
                // corrupted upload of another tile's pixels rather than an
                // error.
                if tile.texels().len() != SAMPLES {
                    self.slots.remove(&slot);
                    tracing::warn!(
                        ?id,
                        samples = tile.texels().len(),
                        expected = SAMPLES,
                        "skipping tile for this frame's upload: unexpected sample count"
                    );
                    stats.remaining += 1;
                    stats.errors += 1;
                    continue;
                }
                // Premultiplied on the way in -- see `premultiply_rgba`
                // for why the atlas, and only the atlas, holds that
                // convention (it is the test-only reference now; this
                // function is what actually applies it, on both this path
                // and `upload_mip`'s). The store's own tile is untouched.
                //
                // **The sequential core, called directly, deliberately
                // (0.96.2).** `write_premultiplied_le_bytes` -- which
                // dispatches onto the bounded `rayon` pool when there is
                // parallel width to win -- is still here, still tested, and
                // still what `upload_mip` uses. This frame-critical path does
                // not use it, and that is a measured decision rather than an
                // oversight:
                //
                // - **Idle**, the parallel arm won ~0.5 ms of whole-frame mean
                //   (8.16/8.07/8.63 ms sequential -> 7.89/7.69/7.46 ms
                //   parallel), in a case that was already comfortably inside
                //   the 16.7 ms budget.
                // - **Under 8 competing CPU-bound threads** on the same box (4
                //   physical / 8 logical cores) it *lost* the budget outright:
                //   whole-frame mean 14.59/17.95/17.59 ms sequential ->
                //   34.34/34.57/36.70 ms parallel, i.e. ~2.1x over budget on a
                //   path that had been at or near it. `upload_sync` alone rose
                //   from ~3.9 ms to ~20.8 ms. Bounding the pool to four
                //   workers (0.96.1) cut that by about a quarter and nowhere
                //   near removed it.
                //
                // The cause is structural, not a tuning miss: this call is
                // synchronous on the frame thread, so `install` blocks until
                // the *slowest* of up to 64 blocks gets a scheduler slice,
                // while the sequential walk only ever needs one. Desktop
                // multitasking is the normal case for an editor, so trading a
                // ~0.5 ms win where the budget passed for a ~17 ms loss where
                // it then fails is net-negative against the project's own
                // 60 FPS gate.
                //
                // **What would justify putting the parallel arm back here**:
                // either a load-sensing design that can tell a contended
                // machine from an idle one cheaply enough to ask per frame
                // (`getloadavg` is a 1-minute average and not in `std`; a
                // per-frame timing feedback loop is a design, not a patch), or
                // measurement across several core counts showing no contended
                // regression at all. Neither exists yet. PLAN.md's 0.96.1 and
                // 0.96.2 entries carry the full tables.
                serialize_premultiplied_le_bytes(tile.texels(), &mut bytes);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: slot.0 * TILE,
                            y: slot.1 * TILE,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(TILE * 8),
                        rows_per_image: Some(TILE),
                    },
                    wgpu::Extent3d {
                        width: TILE,
                        height: TILE,
                        depth_or_array_layers: 1,
                    },
                );
                self.slots.insert(slot, id);
                bytes_left -= TILE_BYTES;
                stats.uploaded += 1;
                stats.bytes_uploaded += TILE_BYTES as u64;
            }
        }
        stats
    }

    /// Writes `texels` into the region of the atlas that `id`'s current
    /// slot occupies at `mip_level`, using the same toroidal slot
    /// addressing [`Self::sync`] uses. `mip_level` 0 is full resolution
    /// ([`TILE`] × [`TILE`]); each level above halves the side length.
    ///
    /// This is the GPU half of progressive rendering
    /// (`spike/FINDINGS.md` finding #3: "render a lower-resolution mip
    /// while panning fast, refining when motion stops"). The caller
    /// (`aurora-render`'s `mip::downsample`) produces the
    /// lower-resolution texels; this method lands them in the atlas at
    /// the matching mip level.
    ///
    /// Deliberately doesn't touch `slots` or consult tile
    /// dirtiness the way [`Self::sync`] does — this is a direct,
    /// caller-driven write for a resolution the caller has already
    /// decided to show, not part of the budgeted full-resolution
    /// catch-up loop. Real callers should keep using [`Self::sync`] for
    /// full-resolution (mip level 0) uploads and this only for the lower
    /// levels progressive rendering needs.
    ///
    /// **What lands here is not currently visible on the canvas.** The
    /// view [`Self::new`] builds for sampling exposes mip level 0 only
    /// (`mip_level_count: Some(1)`), so bytes written to levels 1-3 are
    /// stored and can be read back through [`Self::texture`], but are
    /// unreachable from `canvas.wgsl`. That is deliberate, and this
    /// module's private `MIP_LEVELS` constant carries the full reasoning
    /// — including why this is done at the view rather than with
    /// `lod_max_clamp`, and what wiring progressive rendering would
    /// actually take.
    ///
    /// **This call reaches the `rayon`-parallel serializer path
    /// [`Self::sync`]'s own frame-path call deliberately does not, as of
    /// 0.96.2** — see that method's own doc for the measured whole-frame
    /// regression under CPU contention that decision is based on. Fine
    /// today because `upload_mip` has no `aurora-app` call site (nothing
    /// calls it on a real frame yet). Whoever wires up progressive
    /// rendering and puts a real per-frame caller behind this method
    /// inherits that same regression risk silently, without a new
    /// decision being forced — re-read `sync`'s comment and re-measure
    /// under contention before shipping that wiring, rather than
    /// assuming this path is exempt because it once was.
    ///
    /// # Errors
    ///
    /// Returns [`GpuError::InvalidMipLevel`] if `mip_level` is not in
    /// `0..4`, or [`GpuError::InvalidTileUpload`] if `texels`'s length
    /// doesn't match what that level's tile size expects.
    pub fn upload_mip(
        &self,
        queue: &wgpu::Queue,
        id: TileId,
        mip_level: u32,
        texels: &[f16],
    ) -> Result<(), GpuError> {
        if mip_level >= MIP_LEVELS {
            return Err(GpuError::InvalidMipLevel(mip_level));
        }
        let size = TILE >> mip_level;
        let expected = (size as usize) * (size as usize) * CHANNELS;
        if texels.len() != expected {
            return Err(GpuError::InvalidTileUpload {
                mip_level,
                expected,
                actual: texels.len(),
            });
        }

        let slot = (id.x % self.grid.0, id.y % self.grid.1);
        // The same premultiply `sync` applies, for the same reason: both
        // write into the same atlas texture, so both must leave it in
        // the same alpha convention or `fs_canvas` would be right about
        // level 0 and wrong about every other level.
        //
        // **The same *function*, too, as of 0.92.1 — not merely the same
        // arithmetic.** 0.92.0 vectorized `sync`'s serializer and left
        // this path on a separate scalar `premultiply_rgba` +
        // serialize-loop pair, which made the two disagree for the first
        // time: the two spellings are not bit-identical on a texel whose
        // RGB channel *and* alpha are both NaN (see
        // `extend_premultiplied_le_bytes`'s "Bit-exactness" section), so
        // the atlas could hold one NaN payload at level 0 and a different
        // one at level 1 for the same source texel. That is precisely the
        // failure mode the paragraph above warns about, so both paths now
        // call the one function. It also drops this path's `to_vec()`
        // copy and its second buffer: `texels` is a borrowed slice the
        // caller still owns, and the fused serializer reads it without
        // mutating it.
        let mut bytes = Vec::with_capacity(texels.len() * 2);
        extend_premultiplied_le_bytes(texels, &mut bytes);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level,
                origin: wgpu::Origin3d {
                    x: slot.0 * size,
                    y: slot.1 * size,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// The atlas texture itself, beyond the [`Self::view`]/[`Self::sampler`]
    /// pair real drawing needs — for reading it back (`residency_test.rs`'s
    /// own pixel-readback checks, and `aurora-render`'s progressive-rendering
    /// tests, both real consumers) or copying into a different target.
    /// A real, non-test-only accessor: the atlas texture is created with
    /// `COPY_SRC` specifically so this is possible.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

impl std::fmt::Debug for TileResidency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileResidency")
            .field("grid", &self.grid)
            .field("origin", &self.origin)
            .field("resident_slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK_SAMPLES, CHUNK_SAMPLES, CHUNK_TEXELS, SERIALIZER_MAX_THREADS, TILE_BYTES,
        TileResidency, build_serializer_pool, extend_premultiplied_le_bytes, premultiply_rgba,
        serializer_pool, serializer_pool_threads, write_premultiplied_le_bytes,
        write_premultiplied_le_bytes_on,
    };
    use crate::test_support::{real_context, real_tile_store};
    use aurora_tile::{CHANNELS, SAMPLES, SurfaceId, TILE, TileId};
    use half::f16;

    /// The one surface every test in this module addresses — nothing
    /// here exercises multi-surface behaviour (that's `aurora-tile`'s
    /// own job); this crate just needs *a* valid `SurfaceId` to call the
    /// store's API with.
    fn surface() -> SurfaceId {
        SurfaceId::from_raw(0)
    }

    /// Paints tile `id` a known solid colour and marks it dirty, exactly
    /// as a real edit would.
    fn paint(store: &mut aurora_tile::TileStore, id: TileId, rgba: [f32; 4]) {
        let tile = match store.get_mut(surface(), id) {
            Ok(tile) => tile,
            Err(err) => unreachable!("test-local scratch store must accept this: {err}"),
        };
        let samples = tile.texels_mut();
        for (i, sample) in samples.iter_mut().enumerate() {
            let Some(&channel) = rgba.get(i % 4) else {
                unreachable!("i % 4 is always in range 0..4");
            };
            *sample = f16::from_f32(channel);
        }
        tile.mark_dirty(aurora_core::Rect {
            x: 0,
            y: 0,
            width: aurora_tile::TILE,
            height: aurora_tile::TILE,
        });
    }

    #[test]
    fn visible_tiles_covers_exactly_the_grid_from_the_current_origin() {
        let Some(context) = real_context() else {
            return;
        };
        // A 256x256 viewport -> grid = (2, 2), same math
        // `toroidal_addressing_uploads_only_the_newly_exposed_column`
        // below already establishes.
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let tiles: Vec<TileId> = residency.visible_tiles().collect();
        assert_eq!(
            tiles,
            vec![
                TileId { x: 0, y: 0 },
                TileId { x: 1, y: 0 },
                TileId { x: 0, y: 1 },
                TileId { x: 1, y: 1 },
            ],
            "row-major from the origin, matching sync's own iteration order"
        );
    }

    #[test]
    fn visible_tiles_shifts_with_the_origin() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        // A whole-tile-multiple `doc_origin` (5 * TILE, 3 * TILE) --
        // zero sub-tile remainder, so this must behave identically to the
        // old `TileId`-typed `set_origin` (this test's own regression
        // backstop that the refactor didn't change whole-tile behaviour).
        residency.set_origin(
            context.queue(),
            (5.0 * TILE as f32, 3.0 * TILE as f32),
            (256, 256),
            1.0,
        );
        let tiles: Vec<TileId> = residency.visible_tiles().collect();
        assert_eq!(
            tiles,
            vec![
                TileId { x: 5, y: 3 },
                TileId { x: 6, y: 3 },
                TileId { x: 5, y: 4 },
                TileId { x: 6, y: 4 },
            ]
        );
    }

    #[test]
    fn toroidal_addressing_uploads_only_the_newly_exposed_column() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // A 256x256 viewport -> grid = (256/256 + 1, 256/256 + 1) = (2, 2).
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        assert_eq!(residency.grid, (2, 2));

        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 0.0, 0.0, 1.0]);
            }
        }

        // Nothing resident yet: every visible slot must upload.
        let first = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            first.uploaded, 4,
            "first sync must upload the whole visible grid"
        );
        assert_eq!(
            first.remaining, 0,
            "unlimited budget must leave nothing pending"
        );

        // Unchanged: nothing should re-upload.
        let second = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            second.uploaded, 0,
            "unchanged, resident, clean tiles must not re-upload"
        );

        // Pan by exactly one tile on the x axis. Paint the newly-visible
        // column so it has real content to upload.
        paint(&mut store, TileId { x: 2, y: 0 }, [0.0, 1.0, 0.0, 1.0]);
        paint(&mut store, TileId { x: 2, y: 1 }, [0.0, 1.0, 0.0, 1.0]);
        residency.set_origin(context.queue(), (TILE as f32, 0.0), viewport, 1.0);
        let third = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            third.uploaded, 2,
            "panning by one tile must invalidate exactly one column, not the whole grid"
        );
    }

    #[test]
    fn budget_limited_sync_converges_over_multiple_calls() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // 2x2 grid, 4 tiles total, all painted up front.
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [0.0, 0.0, 1.0, 1.0]);
            }
        }

        // Budget for exactly 2 tiles' worth of bytes.
        let budget = TILE_BYTES * 2;

        let first = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(first.uploaded, 2, "budget must cap uploads to what fits");
        assert_eq!(
            first.remaining, 2,
            "the other two must be reported as still pending"
        );
        assert_eq!(first.bytes_uploaded, (TILE_BYTES * 2) as u64);
        assert_eq!(first.errors, 0);

        // Same small budget again, nothing else changed: must pick up
        // exactly the two left over, not re-touch the first two.
        let second = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(
            second.uploaded, 2,
            "second call must finish the backlog, not restart it"
        );
        assert_eq!(
            second.remaining, 0,
            "fully caught up after two budget-limited calls"
        );

        // Steady state: nothing left to do, even with the same tight budget.
        let third = residency.sync(context.queue(), &mut store, surface(), false, budget);
        assert_eq!(third.uploaded, 0);
        assert_eq!(third.remaining, 0);
    }

    /// **A budget-skipped *resident* tile must still be uploaded later.**
    ///
    /// `budget_limited_sync_converges_over_multiple_calls` above cannot
    /// see this: its skipped tiles are also non-resident, so the resident
    /// check alone forces a retry whatever happened to their dirty flags.
    /// The gap is a tile that is already resident and has been *edited* —
    /// until 0.68.7 `sync` called `take_dirty` three lines above the
    /// budget check that skips it, so the flag was consumed for an upload
    /// that never happened and the edit was then invisible until some
    /// unrelated change marked the tile dirty again. That is a
    /// user-visible stale canvas, not just a stat.
    ///
    /// Measured against the pre-fix ordering: the second call reports
    /// `uploaded == 0` and the edit is silently dropped.
    #[test]
    fn a_resident_tile_skipped_for_budget_is_still_uploaded_on_a_later_call() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [0.0, 0.0, 1.0, 1.0]);
            }
        }
        // Everything resident and clean.
        let warmup = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(warmup.uploaded, 4);
        assert_eq!(warmup.remaining, 0);

        // Now edit all four. They stay resident, so *only* the dirty flag
        // distinguishes them from a tile with nothing to do.
        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 0.0, 0.0, 1.0]);
            }
        }

        let tight = residency.sync(
            context.queue(),
            &mut store,
            surface(),
            false,
            TILE_BYTES * 2,
        );
        assert_eq!(tight.uploaded, 2, "the budget caps this call at two");
        assert_eq!(tight.remaining, 2, "and reports the other two as pending");

        let rest = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            rest.uploaded, 2,
            "a resident tile skipped for budget must keep its dirtiness and upload later"
        );
        assert_eq!(rest.remaining, 0);
    }

    #[test]
    fn upload_mip_rejects_an_out_of_range_level() {
        let Some(context) = real_context() else {
            return;
        };
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let texels = vec![f16::from_f32(0.0); 4];
        match residency.upload_mip(context.queue(), TileId { x: 0, y: 0 }, 4, &texels) {
            Err(crate::GpuError::InvalidMipLevel(4)) => {}
            other => unreachable!("expected InvalidMipLevel(4), got {other:?}"),
        }
    }

    #[test]
    fn upload_mip_rejects_a_mismatched_texel_count() {
        let Some(context) = real_context() else {
            return;
        };
        let residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        // Level 1 (Half) expects (TILE/2)^2 * 4 samples, not 4.
        let texels = vec![f16::from_f32(0.0); 4];
        match residency.upload_mip(context.queue(), TileId { x: 0, y: 0 }, 1, &texels) {
            Err(crate::GpuError::InvalidTileUpload {
                mip_level: 1,
                actual: 4,
                ..
            }) => {}
            other => unreachable!("expected InvalidTileUpload, got {other:?}"),
        }
    }

    #[test]
    fn resize_changes_the_atlas_texture_dimensions() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let before = residency.texture().size();
        assert_eq!(before.width, 512, "grid (2,2) -> 512x512 atlas");
        assert_eq!(before.height, 512);

        residency.resize(context.device(), context.queue(), (512, 512));
        let after = residency.texture().size();
        assert_eq!(
            residency.grid,
            (3, 3),
            "512.div_ceil(256) + 1 == 3 on both axes"
        );
        assert_eq!(after.width, 768, "grid (3,3) -> 768x768 atlas");
        assert_eq!(after.height, 768);
        assert_ne!(
            (before.width, before.height),
            (after.width, after.height),
            "resize must actually change the real GPU texture's dimensions"
        );
    }

    #[test]
    fn resize_resets_slots_so_a_smaller_grid_does_not_leak_stale_occupancy() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // 512x512 viewport -> grid (3, 3): slot (2, 2) is occupied.
        let mut residency = TileResidency::new(context.device(), context.queue(), (512, 512));
        assert_eq!(residency.grid, (3, 3));
        for gy in 0..3 {
            for gx in 0..3 {
                paint(&mut store, TileId { x: gx, y: gy }, [1.0, 1.0, 0.0, 1.0]);
            }
        }
        let first = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(first.uploaded, 9, "first sync fills the whole 3x3 grid");
        assert_eq!(residency.slots.len(), 9);

        // Shrink to a 256x256 viewport -> grid (2, 2). Slot (2, 2) from
        // the old grid is now out of bounds for the new one -- if
        // `resize` didn't clear `slots`, that stale entry would simply
        // sit unread (harmless) but any slot coordinate the old map
        // shared with the new grid (e.g. (0, 0), (1, 0), ...) would
        // wrongly read as still-resident and get skipped.
        residency.resize(context.device(), context.queue(), (256, 256));
        assert_eq!(residency.grid, (2, 2));
        assert!(
            residency.slots.is_empty(),
            "resize must reset slot occupancy, not carry over old-grid coordinates"
        );

        // Nothing marked dirty since the paint above (tiles are clean in
        // the store), but every visible slot must still upload because
        // `slots` was reset -- proves the resident check isn't trusting
        // stale bookkeeping across the resize.
        let second = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(
            second.uploaded, 4,
            "post-resize sync must re-upload every visible slot in the new (2,2) grid"
        );
        assert_eq!(second.errors, 0, "no panic, no wrong-tile-shown");
    }

    #[test]
    fn resize_is_a_no_op_on_a_zero_sized_request() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        residency.set_origin(
            context.queue(),
            (3.0 * TILE as f32, 7.0 * TILE as f32),
            (256, 256),
            1.0,
        );
        let before_size = residency.texture().size();
        let before_grid = residency.grid;
        let before_origin = residency.origin;

        residency.resize(context.device(), context.queue(), (0, 256));
        residency.resize(context.device(), context.queue(), (256, 0));
        residency.resize(context.device(), context.queue(), (0, 0));

        assert_eq!(
            residency.texture().size(),
            before_size,
            "a zero-sized resize request must leave the real atlas texture untouched"
        );
        assert_eq!(residency.grid, before_grid);
        assert_eq!(
            residency.origin, before_origin,
            "a no-op resize must not disturb existing origin/pan state either"
        );
    }

    #[test]
    fn resize_preserves_the_document_space_origin() {
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        residency.set_origin(
            context.queue(),
            (5.0 * TILE as f32, 9.0 * TILE as f32),
            (256, 256),
            1.0,
        );
        assert_eq!(residency.origin(), TileId { x: 5, y: 9 });

        residency.resize(context.device(), context.queue(), (512, 512));

        assert_eq!(
            residency.origin(),
            TileId { x: 5, y: 9 },
            "resize changes how much of the document is visible, not which part"
        );
    }

    // -- Canvas geometry: the scale the atlas actually renders at --
    //
    // Every test below drives `TileResidency::uniform_values` directly.
    // That is deliberate: it is the exact function `write_uniform` feeds
    // to the GPU, it is a pure function of the atlas's own state, and
    // driving it needs no adapter -- so these run everywhere, including
    // on the CI runners where every `real_context()` test self-skips.
    // `render_test.rs` covers the same properties end to end through a
    // real render pass, where an adapter exists.

    /// The zoom the atlas renders at, per axis, recovered from the
    /// uniform exactly as the shader consumes it.
    ///
    /// `vs_canvas` computes `uv = t * uv_scale + uv_offset` for
    /// `t ∈ [0, 1]` across the viewport, so the frame spans
    /// `uv_scale * tex_size` document texels over `viewport_px` screen
    /// pixels. Screen pixels per document pixel -- i.e. the zoom the
    /// user actually sees -- is the reciprocal of that ratio.
    fn rendered_zoom(
        grid: (u32, u32),
        sub_tile: (f32, f32),
        viewport_px: (u32, u32),
        zoom: f32,
    ) -> (f32, f32) {
        let values =
            TileResidency::uniform_values(grid, TileId { x: 0, y: 0 }, sub_tile, viewport_px, zoom);
        let (Some(&scale_u), Some(&scale_v)) = (values.get(2), values.get(3)) else {
            unreachable!("uniform_values always returns four floats");
        };
        let tex_w = (grid.0 * TILE) as f32;
        let tex_h = (grid.1 * TILE) as f32;
        (
            viewport_px.0 as f32 / (scale_u * tex_w),
            viewport_px.1 as f32 / (scale_v * tex_h),
        )
    }

    /// Relative difference, so one tolerance reads the same at zoom 0.01
    /// and at zoom 64.
    fn relative_error(a: f32, b: f32) -> f32 {
        ((a - b) / b).abs()
    }

    /// A realistic 1x 1920x1080 canvas: grid (9, 6), atlas 2304x1536,
    /// and the two axes saturate at *different* zooms (0.9375 and
    /// 0.84375) -- the asymmetry the anisotropy regression lived in.
    const CANVAS_1080P: (u32, u32) = (1920, 1080);

    #[test]
    // The whole point is that these are bit-identical: the formula must
    // not read `sub_tile` at all. A tolerance here would hide exactly
    // the drift being tested.
    #[allow(clippy::float_cmp)]
    fn rendered_zoom_is_pan_independent_across_a_full_tile_of_pan() {
        // RT12-02. `uv_scale` used to be clamped to `tex_size -
        // sub_tile`, and `sub_tile` sweeps [0, TILE) continuously as the
        // user pans -- so at a *constant* user zoom the rendered scale
        // ramped smoothly and then snapped back once per tile of
        // panning (measured: 0.500 -> 0.889 across one tile, a 1.78x
        // change, at a constant zoom of 0.5). The floor is now taken at
        // the worst case, so nothing here may move.
        let grid = TileResidency::grid_for(CANVAS_1080P);
        for zoom in [0.5_f32, 0.9, 0.9375, 1.0, 4.0] {
            let baseline = rendered_zoom(grid, (0.0, 0.0), CANVAS_1080P, zoom);
            for step in 0..=64 {
                let offset = (step as f32) * (TILE as f32) / 64.0;
                // The full open range [0, TILE) on each axis, and the
                // two axes at unrelated pan positions -- a sub-tile pan
                // is not diagonal.
                let sub_tile = (offset, (TILE as f32) - offset - 0.001);
                let seen = rendered_zoom(grid, sub_tile, CANVAS_1080P, zoom);
                assert_eq!(
                    seen, baseline,
                    "at a constant zoom of {zoom}, panning to sub-tile \
                     {sub_tile:?} changed the rendered scale from \
                     {baseline:?} to {seen:?}. The scale must depend on \
                     zoom alone; a scale that ramps with the pan position \
                     makes the document visibly breathe as the user pans"
                );
            }
        }
    }

    #[test]
    fn rendered_zoom_is_isotropic_on_a_non_square_viewport() {
        // RT12-03. Clamping each axis to its own coverage let one axis
        // keep shrinking after the other had stopped: measured up to
        // 33.6% divergence on 512x256, and ~18.5% permanent horizontal
        // stretch on 1920x1080 below the y-axis threshold. Circles
        // rendered as ellipses.
        for viewport in [CANVAS_1080P, (512, 256), (1366, 768), (2560, 1080)] {
            let grid = TileResidency::grid_for(viewport);
            for zoom in [0.001_f32, 0.25, 0.5, 0.84, 0.9, 0.99, 1.0, 3.0, 64.0] {
                for sub_tile in [(0.0, 0.0), (200.0, 40.0), (255.9, 255.9)] {
                    let (x, y) = rendered_zoom(grid, sub_tile, viewport, zoom);
                    assert!(
                        relative_error(x, y) < 1e-6,
                        "viewport {viewport:?} at zoom {zoom} rendered \
                         x at {x} and y at {y} -- a {}% aspect-ratio \
                         distortion. Both axes must scale by the same \
                         factor at every zoom",
                        relative_error(x, y) * 100.0
                    );
                }
            }
        }
    }

    #[test]
    fn rendered_zoom_equals_the_requested_zoom_at_or_above_the_floor() {
        // RT12-04's `aurora-gpu` half, and the property
        // `aurora-app`/`CanvasView` depend on: at or above
        // `min_zoom_for_viewport` the atlas renders *exactly* what it
        // was asked for, so a caller that has clamped its own zoom to
        // that floor converts pointer positions with the same number the
        // frame was drawn at. `aurora-app`'s own
        // `the_render_path_and_the_pointer_path_agree_on_scale_when_zoomed_out`
        // closes the loop from the other side.
        for viewport in [
            CANVAS_1080P,
            (512, 256),
            (300, 300),
            (3840, 2160),
            (200, 900),
        ] {
            let grid = TileResidency::grid_for(viewport);
            let floor = TileResidency::min_zoom_for_viewport(viewport);
            for zoom in [floor, floor * 1.0001, 0.99, 1.0, 2.5, 64.0] {
                if zoom < floor {
                    continue;
                }
                for sub_tile in [(0.0, 0.0), (17.5, 250.0)] {
                    let (x, y) = rendered_zoom(grid, sub_tile, viewport, zoom);
                    assert!(
                        relative_error(x, zoom) < 1e-5 && relative_error(y, zoom) < 1e-5,
                        "viewport {viewport:?} asked for zoom {zoom} (floor \
                         {floor}) and rendered {x} x {y}. Above the floor the \
                         atlas must honour the zoom exactly, or every consumer \
                         of the same view transform is drawing to a different \
                         scale than the renderer"
                    );
                }
            }
        }
    }

    #[test]
    // Exact, because the floor is the *same* expression on both sides;
    // see the test below.
    #[allow(clippy::float_cmp)]
    fn the_zoom_floor_survives_an_absurd_viewport_without_overflowing() {
        // `grid_for` is `viewport.div_ceil(TILE) + 1` and every caller
        // multiplies it back by `TILE`; both overflowed `u32` for a
        // viewport near `u32::MAX` -- a hard panic in debug (this
        // workspace denies `panic`) and a silent wrap to garbage in
        // release. `min_zoom_for_viewport` is now called from
        // `aurora-app`'s `redraw` above its GPU early-return, so this
        // arithmetic runs with no GPU and no wgpu validation in front of
        // it. The exact pre-fix threshold is the smallest viewport whose
        // whole-tile grid exceeds `u32::MAX`.
        let threshold = (u32::MAX / TILE - 1) * TILE + 1;
        for viewport in [
            (threshold, 1080),
            (1920, threshold),
            (u32::MAX, u32::MAX),
            (u32::MAX - 1, u32::MAX - 1),
        ] {
            let grid = TileResidency::grid_for(viewport);
            assert!(
                grid.0 > 0 && grid.1 > 0,
                "viewport {viewport:?}: grid {grid:?} wrapped to zero"
            );
            let floor = TileResidency::min_zoom_for_viewport(viewport);
            assert!(
                floor.is_finite() && floor > 0.0 && floor <= 2.0,
                "viewport {viewport:?}: the floor must stay a sane, finite \
                 number rather than wrap to garbage; got {floor}"
            );
            // And it is still a real bound, not a value that quietly
            // disables the clamp.
            assert_eq!(TileResidency::effective_zoom(viewport, floor / 2.0), floor);
        }
    }

    #[test]
    // `min_zoom_for_viewport` and the clamp inside `uniform_values` are
    // the *same* expression applied to the same inputs; an exact
    // comparison is what pins them together.
    #[allow(clippy::float_cmp)]
    fn min_zoom_for_viewport_is_exactly_the_floor_the_uniform_applies() {
        // The public promise (`min_zoom_for_viewport`) and the private
        // behaviour (`uniform_values`) must not be two formulas that
        // merely agree today: `aurora-app` clamps `CanvasView` to the
        // public one, so any drift between them reopens RT12-04.
        for viewport in [CANVAS_1080P, (512, 256), (300, 300), (256, 256), (1, 1)] {
            let grid = TileResidency::grid_for(viewport);
            let floor = TileResidency::min_zoom_for_viewport(viewport);
            assert!(
                floor > 0.0 && floor <= 1.0,
                "viewport {viewport:?}: the floor is \
                 viewport / (viewport rounded up to whole tiles), which can \
                 never exceed 1.0 or reach 0.0; got {floor}"
            );
            for below in [floor * 0.5, floor * 0.01, f32::MIN_POSITIVE / 2.0] {
                let (x, y) = rendered_zoom(grid, (0.0, 0.0), viewport, below);
                assert!(
                    relative_error(x, floor) < 1e-5 && relative_error(y, floor) < 1e-5,
                    "viewport {viewport:?}: zoom {below} is below the floor \
                     {floor} and must render at exactly the floor, not at \
                     {x} x {y}"
                );
            }
            // `effective_zoom` is the same statement as a public API.
            assert_eq!(TileResidency::effective_zoom(viewport, floor), floor);
            assert_eq!(TileResidency::effective_zoom(viewport, floor / 2.0), floor);
            assert_eq!(TileResidency::effective_zoom(viewport, 4.0), 4.0);
            assert_eq!(
                TileResidency::effective_zoom(viewport, f32::NAN),
                1.0_f32.max(floor)
            );
        }
    }

    #[test]
    fn uv_scale_never_reaches_past_the_atlas_coverage() {
        // RT-09, the duplication bug, restated as the invariant that
        // replaced its clamp. The atlas covers `tex_size` texels and the
        // window starts `sub_tile` into them, so a frame spanning more
        // than `tex_size - sub_tile` texels wraps through the toroidal
        // `AddressMode::Repeat` sampler and shows the same document
        // region twice. `zoom >= zoom_floor` is supposed to make that
        // impossible without any clamp on `uv_scale` itself; this
        // asserts the implication directly, at every sub-tile pan
        // position, rather than trusting the algebra.
        for viewport in [CANVAS_1080P, (512, 256), (300, 300), (256, 256), (17, 4000)] {
            let grid = TileResidency::grid_for(viewport);
            let tex_w = (grid.0 * TILE) as f32;
            let tex_h = (grid.1 * TILE) as f32;
            for zoom in [
                f32::MIN_POSITIVE / 2.0,
                0.01,
                0.25,
                0.5,
                TileResidency::min_zoom_for_viewport(viewport),
                1.0,
                64.0,
                f32::NAN,
                0.0,
                -3.0,
            ] {
                for step in 0..16 {
                    let offset = (step as f32) * (TILE as f32) / 16.0;
                    let sub_tile = (offset, (TILE as f32) - offset - 0.5);
                    let values = TileResidency::uniform_values(
                        grid,
                        TileId { x: 3, y: 5 },
                        sub_tile,
                        viewport,
                        zoom,
                    );
                    let (Some(&scale_u), Some(&scale_v)) = (values.get(2), values.get(3)) else {
                        unreachable!("uniform_values always returns four floats");
                    };
                    assert!(
                        scale_u * tex_w <= tex_w - sub_tile.0 + 1e-3
                            && scale_v * tex_h <= tex_h - sub_tile.1 + 1e-3,
                        "viewport {viewport:?} at zoom {zoom}, sub-tile \
                         {sub_tile:?}: the frame spans {} x {} texels of an \
                         atlas holding only {} x {} ahead of the window. \
                         Past that the sampler wraps and the canvas shows a \
                         seamless duplicate of document content that is not \
                         there",
                        scale_u * tex_w,
                        scale_v * tex_h,
                        tex_w - sub_tile.0,
                        tex_h - sub_tile.1
                    );
                }
            }
        }
    }

    #[test]
    fn uv_scale_stays_inside_an_atlas_smaller_than_the_viewport_asks_for() {
        // A caller whose viewport grew without calling `resize` first
        // hands `uniform_values` a grid that predates it. The floor is
        // taken against whichever atlas is smaller for exactly this
        // case: rendering at the wrong scale for one frame is a
        // transient; wrapping the sampler and fabricating document
        // content is not.
        let stale_grid = TileResidency::grid_for((256, 256));
        let tex = ((stale_grid.0 * TILE) as f32, (stale_grid.1 * TILE) as f32);
        for zoom in [0.25_f32, 1.0, 4.0] {
            let sub_tile = (200.0, 30.0);
            let values = TileResidency::uniform_values(
                stale_grid,
                TileId { x: 0, y: 0 },
                sub_tile,
                CANVAS_1080P,
                zoom,
            );
            let (Some(&scale_u), Some(&scale_v)) = (values.get(2), values.get(3)) else {
                unreachable!("uniform_values always returns four floats");
            };
            assert!(
                scale_u * tex.0 <= tex.0 - sub_tile.0 + 1e-3
                    && scale_v * tex.1 <= tex.1 - sub_tile.1 + 1e-3,
                "a 1920x1080 viewport drawn through an atlas still sized for \
                 256x256 must still never sample past that atlas's own \
                 coverage; got {} x {} texels of {} x {} available",
                scale_u * tex.0,
                scale_v * tex.1,
                tex.0 - sub_tile.0,
                tex.1 - sub_tile.1
            );
        }
    }

    #[test]
    // Each assertion is an exact bound landing on a literal, not an
    // accumulated computation.
    #[allow(clippy::float_cmp)]
    fn clamp_doc_origin_bounds_every_degenerate_document_position() {
        // RT12-07's arithmetic half. `write_uniform` computes
        // `origin.x * TILE` in `u32`; an unbounded `doc_origin`
        // overflows it -- a panic in debug (this workspace denies
        // `panic` because "a panic loses unsaved work"), a silently
        // wrong tile in release.
        assert_eq!(
            TileResidency::clamp_doc_origin((12.5, 900.25)),
            (12.5, 900.25)
        );
        assert_eq!(TileResidency::clamp_doc_origin((-5.0, -0.5)), (0.0, 0.0));
        assert_eq!(
            TileResidency::clamp_doc_origin((f32::INFINITY, f32::MAX)),
            (
                TileResidency::MAX_DOC_ORIGIN_PX,
                TileResidency::MAX_DOC_ORIGIN_PX,
            )
        );
        assert_eq!(
            TileResidency::clamp_doc_origin((4.3e9, 1e12)),
            (
                TileResidency::MAX_DOC_ORIGIN_PX,
                TileResidency::MAX_DOC_ORIGIN_PX,
            )
        );
        assert_eq!(
            TileResidency::clamp_doc_origin((f32::NEG_INFINITY, 5.0)),
            (0.0, 5.0)
        );
        // NaN lands on the document's own origin rather than
        // propagating -- `f32::max` returns the non-NaN operand, which
        // is why this is not spelled `clamp`.
        assert_eq!(
            TileResidency::clamp_doc_origin((f32::NAN, f32::NAN)),
            (0.0, 0.0)
        );
    }

    #[test]
    // Every assertion is an exact identity -- the value goes in and the
    // same value must come back out, never an accumulated computation.
    #[allow(clippy::float_cmp)]
    fn clamp_doc_origin_is_the_identity_across_its_whole_public_range() {
        // What [`TileResidency::MAX_DOC_ORIGIN_PX`] promises a caller
        // that keeps itself inside the bound, stated as the property
        // that makes it useful across a crate boundary: every position
        // in `[0, MAX_DOC_ORIGIN_PX]` passes through untouched. That is
        // what lets `aurora-app` prove render/paint agreement by
        // asserting range membership alone -- it never has to reach
        // this private function, which stays private.
        let ceiling = TileResidency::MAX_DOC_ORIGIN_PX;
        for value in [
            0.0_f32,
            1.0,
            TILE as f32,
            ceiling / 2.0,
            ceiling - 1.0,
            ceiling,
        ] {
            assert_eq!(
                TileResidency::clamp_doc_origin((value, value)),
                (value, value),
                "{value} is inside the bound and must pass through unchanged"
            );
        }
        // And it really does saturate just past it -- the identity is a
        // statement about the range, not about the clamp being absent.
        assert_eq!(
            TileResidency::clamp_doc_origin((ceiling + 1.0, ceiling * 2.0)),
            (ceiling, ceiling)
        );
    }

    #[test]
    fn set_origin_survives_a_huge_or_non_finite_document_origin() {
        // RT12-07 end to end: the value has to survive the real
        // `u32` multiply inside `write_uniform`, which is where it
        // overflowed.
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_tile = (TileResidency::MAX_DOC_ORIGIN_PX / (TILE as f32)) as u32;
        for doc_origin in [
            (4.3e9_f32, 4.3e9),
            (f32::MAX, f32::MAX),
            (f32::INFINITY, 0.0),
            (0.0, f32::INFINITY),
            (f32::NAN, f32::NAN),
            (-1.0e9, -1.0e9),
        ] {
            residency.set_origin(context.queue(), doc_origin, (256, 256), 1.0);
            let origin = residency.origin();
            assert!(
                origin.x <= max_tile && origin.y <= max_tile,
                "doc_origin {doc_origin:?} addressed tile {origin:?}, past the \
                 {} px document ceiling -- `origin * TILE` overflows \
                 `u32` from there",
                TileResidency::MAX_DOC_ORIGIN_PX
            );
        }
    }

    #[test]
    // The sub-tile offset is copied, not recomputed -- exact equality is
    // the assertion.
    #[allow(clippy::float_cmp)]
    fn resize_preserves_the_sub_tile_pan_offset() {
        // RT12-08. `resize` restores `origin` *and* `sub_tile`; before
        // it restored `sub_tile` a window resize silently snapped the
        // view back to the nearest tile boundary. Nothing else in the
        // suite pans to a fractional position and then resizes, so
        // deleting that one line left every test green.
        let Some(context) = real_context() else {
            return;
        };
        let mut residency = TileResidency::new(context.device(), context.queue(), (256, 256));
        let doc_origin = (5.0 * (TILE as f32) + 91.5, 3.0 * (TILE as f32) + 200.25);
        residency.set_origin(context.queue(), doc_origin, (256, 256), 1.0);
        assert_eq!(residency.sub_tile, (91.5, 200.25));

        residency.resize(context.device(), context.queue(), (512, 512));

        assert_eq!(
            residency.origin(),
            TileId { x: 5, y: 3 },
            "the whole-tile part of the pan must survive a resize"
        );
        assert_eq!(
            residency.sub_tile,
            (91.5, 200.25),
            "the *fractional* part of the pan must survive a resize too -- \
             dropping it snaps the view back to the nearest tile boundary, a \
             visible jump of up to a tile in each axis on every window resize"
        );
        // And it must reach the uniform, not just the field: the sampled
        // window's own offset is what the user actually sees.
        let values = TileResidency::uniform_values(
            residency.grid,
            residency.origin(),
            residency.sub_tile,
            (512, 512),
            1.0,
        );
        let tex_w = (residency.grid.0 * TILE) as f32;
        let Some(&offset_u) = values.first() else {
            unreachable!("uniform_values always returns four floats");
        };
        assert_eq!(
            (offset_u * tex_w).rem_euclid(TILE as f32),
            91.5,
            "the restored sub-tile offset must show up in the sampled UV \
             origin the shader reads"
        );
    }

    /// The `[r, g, b, a]` slice pattern in `premultiply_rgba` is written
    /// against a four-channel texel. Pinned against the tile crate's own
    /// constant rather than left implied: if `CHANNELS` ever changed,
    /// `chunks_exact_mut(CHANNELS)` would yield chunks the pattern does
    /// not match and the function would silently do **nothing** — an
    /// upload path that quietly stopped premultiplying is exactly the
    /// kind of failure a green test run would otherwise hide.
    #[test]
    fn premultiply_rgba_is_written_against_a_four_channel_texel() {
        assert_eq!(
            aurora_tile::CHANNELS,
            4,
            "premultiply_rgba's [r, g, b, a] pattern assumes four channels"
        );
    }

    /// The fused serializer and the scalar reference must agree exactly, or
    /// the two upload paths (`sync`, which since 0.96.0 calls the in-place
    /// core directly, and `upload_mip`, which appends through the fused
    /// wrapper) would leave the same atlas texture in two different
    /// alpha conventions. Bit-for-bit, over a buffer carrying every
    /// interesting alpha: opaque, half, zero, and a faint one near the
    /// bottom of `f16`'s range.
    #[test]
    fn the_fused_serializer_matches_premultiply_then_serialize_exactly() {
        let source: Vec<f16> = [
            [1.0f32, 0.75, 0.5, 1.0],
            [1.0, 0.75, 0.5, 0.5],
            [1.0, 0.75, 0.5, 0.0],
            [1.0, 0.75, 0.5, 0.001],
        ]
        .iter()
        .flatten()
        .map(|&channel| f16::from_f32(channel))
        .collect();

        let mut in_place = source.clone();
        premultiply_rgba(&mut in_place);
        let mut expected = Vec::new();
        for sample in &in_place {
            expected.extend_from_slice(&sample.to_le_bytes());
        }

        let mut fused = Vec::new();
        extend_premultiplied_le_bytes(&source, &mut fused);

        assert_eq!(fused, expected);
    }

    /// The fused serializer appends rather than replaces (`sync` clears
    /// once per tile and fills one buffer), and honours the same
    /// trailing-partial-chunk contract: an incomplete final texel
    /// contributes nothing rather than corrupt bytes.
    #[test]
    fn the_fused_serializer_appends_and_ignores_a_trailing_partial_texel() {
        let texels: Vec<f16> = [1.0f32, 1.0, 1.0, 0.5, 1.0, 1.0]
            .iter()
            .map(|&channel| f16::from_f32(channel))
            .collect();
        let mut out = vec![0xAA, 0xBB];
        extend_premultiplied_le_bytes(&texels, &mut out);
        assert_eq!(
            out.len(),
            2 + CHANNELS * 2,
            "two pre-existing bytes plus exactly one whole texel"
        );
        assert_eq!(out.first(), Some(&0xAA));
        assert_eq!(out.get(1), Some(&0xBB));
    }

    /// The batched write (0.89.0 replaced four 2-byte appends per texel
    /// with one 8-byte append) must lay the bytes down in exactly the
    /// same order and encoding as before. Both tests above pin that
    /// *relative* to another implementation in this file; this one pins
    /// it *absolutely*, against hand-derived IEEE 754 half-precision bit
    /// patterns, so a mistake made identically in both implementations
    /// still fails here.
    ///
    /// **What two texels can and cannot establish**, since 0.89.0's
    /// version of this comment overclaimed it. Sixteen hand-derived bytes
    /// pin the encoding, the little-endian order, the R/G/B/A channel
    /// order, the premultiply-RGB-but-*not*-alpha rule, and — because
    /// 0.89.1 gave texel 1 a different colour as well as a different
    /// alpha — that the loop re-reads r/g/b per texel instead of reusing
    /// texel 0's. It does **not** stand in for a buffer-length walk; that
    /// is `the_fused_serializer_advances_rgb_and_alpha_for_every_texel`
    /// below, which covers six texels without hand-derived hex.
    ///
    /// The 0.89.1 correction is worth keeping because the original pair
    /// was weaker than it looked in *two* stacked ways. Its two texels
    /// shared one straight RGB **and** premultiplied to the same
    /// 0.25/0.125/0.0625, differing only in alpha — so hoisting the
    /// r/g/b read out of the loop and reusing the first texel's colour
    /// passed every fused-serializer test in this file. Fixing only the
    /// premultiplied halves (leaving the straight RGB shared) still would
    /// not have caught it, since the hoist reads the straight values.
    /// Both had to change.
    ///
    /// Every value is a power of two and every premultiplied product is
    /// too, so both the `f32 -> f16` conversions and the multiplies are
    /// exact and no tolerance is needed. Expected encodings (sign 0, then
    /// 5 exponent bits biased by 15, then 10 zero mantissa bits), stored
    /// low byte first: `0.5 = 2^-1 -> 0x3800`, `0.25 = 2^-2 -> 0x3400`,
    /// `0.125 = 2^-3 -> 0x3000`, `0.0625 = 2^-4 -> 0x2C00`,
    /// `0.03125 = 2^-5 -> 0x2800`.
    ///
    /// It also repeats the call after `out.clear()`, the buffer-reuse
    /// pattern [`TileResidency::sync`] used through 0.89.x and still uses in
    /// spirit (one buffer per call, refilled per tile — pre-sized rather
    /// than cleared since 0.96.0). A batched write that carried any state
    /// between calls would diverge on the second fill.
    #[test]
    fn the_fused_serializer_writes_each_texel_as_eight_little_endian_bytes_in_rgba_order() {
        // Texel 0: alpha 0.5, so the premultiplied RGB is 0.25, 0.125,
        // 0.0625 and alpha stays 0.5.
        // Texel 1: a *different* colour under a different alpha, so all
        // eight of its bytes differ from texel 0's — straight RGB 0.25,
        // 0.125, 0.5 under alpha 0.25, giving premultiplied 0.0625,
        // 0.03125, 0.125. Both the straight *and* the premultiplied
        // triples differ per channel, which is what makes this test fail
        // if the loop stops re-reading r/g/b per texel. Within each texel
        // all four bytes are distinct too, so no channel can be swapped
        // for another without changing the vector below.
        let texels: Vec<f16> = [[0.5f32, 0.25, 0.125, 0.5], [0.25, 0.125, 0.5, 0.25]]
            .iter()
            .flatten()
            .map(|&channel| f16::from_f32(channel))
            .collect();

        let mut out = Vec::new();
        extend_premultiplied_le_bytes(&texels, &mut out);

        assert_eq!(
            out,
            vec![
                // 0.25 -> 0x3400, 0.125 -> 0x3000, 0.0625 -> 0x2C00,
                // alpha 0.5 -> 0x3800 (premultiplying alpha too would
                // have written 0x3400 here).
                0x00, 0x34, 0x00, 0x30, 0x00, 0x2C, 0x00, 0x38, //
                // 0.0625 -> 0x2C00, 0.03125 -> 0x2800, 0.125 -> 0x3000,
                // alpha 0.25 -> 0x3400 (premultiplied would be 0x2C00).
                0x00, 0x2C, 0x00, 0x28, 0x00, 0x30, 0x00, 0x34,
            ],
            "two texels, eight little-endian bytes each, in R G B A order"
        );
        assert_eq!(out.len(), 2 * CHANNELS * 2);

        let first = out.clone();
        out.clear();
        extend_premultiplied_le_bytes(&texels, &mut out);
        assert_eq!(first, out, "clear-then-refill must be byte-identical");
    }

    /// The hot-path serializer's own multi-texel walk — every channel of
    /// every texel, decoded straight back out of the bytes it wrote.
    ///
    /// This exists because of a real gap found reviewing 0.89.0.
    /// `extend_premultiplied_le_bytes` was then what [`TileResidency::sync`]
    /// called on every frame, yet every test it had either compared it
    /// against [`premultiply_rgba`] (its *cold* sibling, reached only
    /// from `upload_mip`) or used two texels whose premultiplied colours
    /// happened to be identical. So the equivalent of
    /// `premultiply_rgba_walks_every_texel_in_the_buffer` — hoist the
    /// r/g/b read out of the loop and reuse texel 0's colour for the rest
    /// — was pinned for the cold function and **not** for the hot one.
    /// Nothing here touches [`premultiply_rgba`]: this is the fused
    /// serializer measured against arithmetic stated in the test itself.
    ///
    /// Every texel's premultiplied RGB triple is distinct from every
    /// other's, and the alphas vary independently, so reusing any earlier
    /// texel's channels — or writing alpha premultiplied — fails. Every
    /// value is a power of two (or zero), so the products and the
    /// `f32 <-> f16` round trips are exact and `assert_eq!` on `f32` is
    /// legitimate rather than a tolerance bug.
    #[test]
    fn the_fused_serializer_advances_rgb_and_alpha_for_every_texel() {
        // (straight r, g, b, a) -> (premultiplied r, g, b, unchanged a).
        let cases = [
            ([1.0f32, 0.5, 0.25, 1.0], [1.0f32, 0.5, 0.25, 1.0]),
            ([1.0, 0.5, 0.25, 0.5], [0.5, 0.25, 0.125, 0.5]),
            ([0.5, 0.25, 0.125, 0.5], [0.25, 0.125, 0.0625, 0.5]),
            ([0.25, 1.0, 0.5, 0.25], [0.0625, 0.25, 0.125, 0.25]),
            // Fully transparent: RGB must zero, alpha must stay 0.0.
            ([1.0, 1.0, 1.0, 0.0], [0.0, 0.0, 0.0, 0.0]),
            ([0.125, 0.0625, 1.0, 1.0], [0.125, 0.0625, 1.0, 1.0]),
        ];

        let texels: Vec<f16> = cases
            .iter()
            .flat_map(|(straight, _)| straight.iter())
            .map(|&channel| f16::from_f32(channel))
            .collect();

        let mut out = Vec::new();
        extend_premultiplied_le_bytes(&texels, &mut out);
        assert_eq!(out.len(), cases.len() * CHANNELS * 2);

        let decoded: Vec<[f32; 4]> = out
            .chunks_exact(CHANNELS * 2)
            .map(|texel| match texel {
                [r_lo, r_hi, g_lo, g_hi, b_lo, b_hi, a_lo, a_hi] => [
                    f16::from_le_bytes([*r_lo, *r_hi]).to_f32(),
                    f16::from_le_bytes([*g_lo, *g_hi]).to_f32(),
                    f16::from_le_bytes([*b_lo, *b_hi]).to_f32(),
                    f16::from_le_bytes([*a_lo, *a_hi]).to_f32(),
                ],
                _ => unreachable!("chunks_exact(8) yields exactly eight bytes"),
            })
            .collect();

        let expected: Vec<[f32; 4]> = cases
            .iter()
            .map(|&(_, premultiplied)| premultiplied)
            .collect();
        assert_eq!(
            decoded, expected,
            "each texel must carry its own premultiplied RGB and its own \
             untouched alpha"
        );
    }

    /// A committed corner of the equivalence an independent review pass
    /// (0.89.0/0.89.1) established exhaustively for every possible f16
    /// bit pattern by hand, off-repo: this pins the same property for
    /// the specific values that class of bug tends to hide in — both
    /// infinities, both signed zeros, a signalling and a quiet NaN, and
    /// the smallest/largest subnormals — so a future change to this
    /// loop's arithmetic or byte order has *something* in the tree to
    /// fail against, not just the ordinary-value tests above.
    #[test]
    fn the_fused_serializer_matches_premultiply_then_serialize_for_extreme_values() {
        let bits: [u16; 10] = [
            0x0000, // +0
            0x8000, // -0
            0x0001, // smallest subnormal
            0x03ff, // largest subnormal
            0x7bff, // largest finite (65504)
            0x7c00, // +infinity
            0xfc00, // -infinity
            0x7e00, // quiet NaN
            0x7c01, // signalling NaN
            0x3800, // 0.5, an ordinary anchor among the extremes
        ];
        let source: Vec<f16> = bits
            .iter()
            .cycle()
            .take(bits.len() * CHANNELS)
            .map(|&b| f16::from_bits(b))
            .collect();

        let mut in_place = source.clone();
        premultiply_rgba(&mut in_place);
        let mut expected = Vec::new();
        for sample in &in_place {
            expected.extend_from_slice(&sample.to_le_bytes());
        }

        let mut fused = Vec::new();
        extend_premultiplied_le_bytes(&source, &mut fused);

        assert_eq!(
            fused, expected,
            "the batched writer must match the in-place premultiply-then-\
             serialize path bit-for-bit, including on NaN, infinity, \
             subnormal and signed-zero inputs"
        );
    }

    /// The vectorized main loop, which nothing else in this module
    /// reaches. **Every other fixture here is 2-10 texels** — all of
    /// them shorter than one [`CHUNK_TEXELS`]-texel chunk, so all of
    /// them land entirely in `extend_premultiplied_le_bytes`'s scalar
    /// remainder. Without this test, 0.92.0's `half::slice`-vectorized
    /// path would be completely untested and every green run above
    /// would be measuring the fallback.
    ///
    /// `CHUNK_TEXELS * 2 + 3` texels straddles the boundary in the one
    /// way that catches an off-by-one in either direction: two full
    /// vectorized chunks *plus* a non-empty scalar remainder, so a
    /// chunk dropped, double-counted, or a remainder started at the
    /// wrong offset all show up as a byte mismatch. Channel values vary
    /// per texel and per channel (derived from the texel index) so that
    /// a swapped R/G/B, a stale scratch-buffer lane carried across
    /// chunks, or an alpha read from the wrong texel cannot hide behind
    /// uniform data — as it would if every texel were identical.
    ///
    /// Every fifth texel is filled from a cycle of extreme bit patterns
    /// instead, so extremes land in both vectorized chunks *and* the
    /// scalar remainder (texels 0, 5, … 125, 130). That placement is the
    /// point:
    /// `the_fused_serializer_matches_premultiply_then_serialize_for_extreme_values`
    /// is only 40 samples, so it never leaves the remainder.
    ///
    /// **Where the signalling NaN actually lands, measured (0.92.1).**
    /// `EXTREMES` has eight entries and a texel consumes four, so
    /// consecutive extreme texels alternate between its first and second
    /// half; `0x7c01` is the last entry, so it reaches the **alpha**
    /// channel only on the odd-numbered extreme texels — indices 5, 15,
    /// 25, … 125, thirteen of them, **all inside the vectorized region**
    /// (`0..128`). Remainder texel 130 is an extreme texel too, but its
    /// alpha is `0x03ff`, not the signalling NaN.
    ///
    /// That matters because the alpha channel is the only one the
    /// "sourced alpha from the round-tripped `f16` scratch buffer instead
    /// of the original chunk" mutation can change: such a path would
    /// quiet `0x7c01` to `0x7e01`, pass the 40-sample extremes test above
    /// anyway, and fail here — at those thirteen texels. 0.92.0 recorded
    /// that mutation as failing "at exactly texel indices 1 and 126";
    /// that was wrong (it appears to have read the hex pattern `0x7e01`'s
    /// two little-endian bytes as indices) and is corrected here. The
    /// mutation was re-run for 0.92.1 and the test is still non-vacuous:
    /// it fails when the mutation is applied.
    #[test]
    fn the_fused_serializer_matches_premultiply_then_serialize_across_a_chunk_boundary() {
        const EXTREMES: [u16; 8] = [
            0x0000, // +0
            0x8000, // -0
            0x0001, // smallest subnormal
            0x03ff, // largest subnormal
            0x7bff, // largest finite (65504)
            0x7c00, // +infinity
            0xfc00, // -infinity
            0x7c01, // signalling NaN — quieted by an f16 -> f32 -> f16
                    // round trip, which is why alpha must not take one
        ];
        let texel_count = CHUNK_TEXELS * 2 + 3;
        let mut extremes = EXTREMES.iter().cycle();
        let mut source: Vec<f16> = Vec::with_capacity(texel_count * CHANNELS);
        for i in 0..texel_count {
            if i % 5 == 0 {
                for _ in 0..CHANNELS {
                    // `cycle()` over a non-empty array never yields
                    // `None`; the fallback is only here because the
                    // workspace denies `unwrap`.
                    let bits = extremes.next().copied().unwrap_or(0x3800);
                    source.push(f16::from_bits(bits));
                }
                continue;
            }
            let n = i as f32;
            // Distinct per channel and per texel, so no two samples in
            // the buffer share a value by accident.
            source.push(f16::from_f32(n / 137.0));
            source.push(f16::from_f32(1.0 - n / 149.0));
            source.push(f16::from_f32(((i * 7) % 137) as f32 / 137.0));
            // Alpha sweeps 0.0 through 1.0 inclusive, so both the
            // fully-transparent and fully-opaque texels appear inside
            // the vectorized region.
            source.push(f16::from_f32(((i * 13) % 131) as f32 / 130.0));
        }
        assert_eq!(
            source.len(),
            texel_count * CHANNELS,
            "the fixture must be a whole number of texels"
        );

        let mut in_place = source.clone();
        premultiply_rgba(&mut in_place);
        let mut expected = Vec::new();
        for sample in &in_place {
            expected.extend_from_slice(&sample.to_le_bytes());
        }

        let mut fused = Vec::new();
        extend_premultiplied_le_bytes(&source, &mut fused);

        assert_eq!(
            fused, expected,
            "the vectorized writer must match the scalar in-place \
             premultiply-then-serialize path bit-for-bit across a chunk \
             boundary and into the scalar remainder"
        );
    }

    /// The one input class where the two premultiply spellings are not
    /// bit-identical, pinned so it stays the only one and so a toolchain
    /// change cannot move it unnoticed.
    ///
    /// 0.92.0's doc comment claimed bit-exactness with `premultiply_rgba`
    /// without qualification. An independent exhaustive pass (all 65,536
    /// RGB bit patterns × all 65,536 alpha bit patterns) found that claim
    /// true everywhere **except** a texel whose RGB channel *and* whose
    /// alpha are both NaN: there the result is still a NaN, but its
    /// payload can come from the other operand, because `NaN × NaN`
    /// returns the quieted *first source operand* on x86 and which operand
    /// is "first" is a property of the code LLVM emits, not of this source.
    ///
    /// **Why this cannot simply assert the divergence.** Measured on
    /// `x86_64` while writing this test, per channel position, as "which
    /// operand's payload survives":
    ///
    /// | profile | `extend_premultiplied_le_bytes` | `premultiply_rgba` |
    /// |---|---|---|
    /// | `opt-level = 1` (the default test profile) | rgb, rgb, **alpha** | rgb, rgb, **alpha** |
    /// | `opt-level = 3` (`--release`) | **alpha**, rgb, **alpha** | rgb, rgb, **alpha** |
    ///
    /// The two paths *agree* at the profile this test actually runs under,
    /// and disagree only under `--release`; `premultiply_rgba` is itself
    /// auto-vectorized and is not an IEEE-pinned scalar baseline; and the
    /// choice moves per channel position rather than per texel or per
    /// payload. Hard-pinning a byte would therefore encode this machine's
    /// optimizer, not the contract, and would break on `--release` or on
    /// `aarch64`.
    ///
    /// **`x86_64`-measured; `aarch64` is a real, disclosed gap, not an
    /// assumed pass.** This test's own "measured, not assumed" two
    /// candidate payloads (below) rest on `NaN × 1.0` propagating one
    /// operand's payload — true under IEEE 754's default rounding on
    /// `x86_64`, but not universal: an `aarch64` target running with
    /// `FPCR.DN` set (default-NaN mode) returns a single canonical NaN for
    /// *any* NaN-involving multiply, collapsing both candidates to the
    /// same value. If that happens, the `assert_ne!` a few lines below
    /// that guards against exactly this fails loudly and immediately,
    /// rather than the test silently passing on a fixture that no longer
    /// distinguishes anything -- but it does mean this test has not been
    /// run on `aarch64`, and may need a second, DN-aware fixture there
    /// rather than an unmodified port.
    ///
    /// So this pins everything that *is* portable, and each of these is a
    /// real constraint rather than a restatement:
    ///
    /// - the result is a NaN whose payload is one of the two operands',
    ///   quieted — never a third value, never a finite number, never an
    ///   infinity;
    /// - for a given channel position, the operand chosen is the **same**
    ///   for every texel in the buffer and for every payload pair, in both
    ///   paths, so a lane-dependent, texel-dependent or payload-dependent
    ///   choice fails even though a channel-dependent one is tolerated;
    /// - the two paths' *agreement* is likewise a per-channel-position
    ///   property, constant across every texel and payload pair — so
    ///   "they diverge, but only on a fixed set of channels" stays true
    ///   whichever profile this runs under, and an input-dependent
    ///   divergence would fail;
    /// - alpha itself passes through bit-for-bit untouched, double NaN
    ///   included.
    ///
    /// The two candidate payloads are **measured, not assumed**: a fixture
    /// with one NaN operand and `1.0` for the other leaves exactly one NaN
    /// in the multiply, and IEEE 754 propagates that one whatever the
    /// operand order, so those two runs define "payload from `rgb`" and
    /// "payload from `alpha`" by construction. The fixture is exactly
    /// [`CHUNK_TEXELS`] texels, so every texel goes through the vectorized
    /// path and none through the scalar remainder.
    #[test]
    // Long because the fixture, the two measured candidate payloads, and
    // the four separate properties being pinned all have to be visible
    // together for the test to be readable at all; the same allow
    // `residency_test.rs`'s own readback tests carry.
    #[allow(clippy::too_many_lines)]
    fn the_fused_serializer_agrees_on_a_double_nan_texel_up_to_the_payload_operand() {
        /// f16 `1.0` — a multiplicative identity that is not itself NaN.
        const ONE: u16 = 0x3c00;
        /// Distinct NaN payloads: quiet and signalling, both signs, plus
        /// an all-ones mantissa and an arbitrary interior pattern.
        const NANS: [u16; 6] = [0x7e00, 0x7c01, 0xfe00, 0xfc01, 0x7fff, 0x7d55];
        /// RGB channels per texel — the ones the multiply touches.
        const COLOUR: usize = 3;

        fn fixture(rgb: u16, alpha: u16) -> Vec<f16> {
            let mut texels = Vec::with_capacity(CHUNK_TEXELS * CHANNELS);
            for _ in 0..CHUNK_TEXELS {
                texels.push(f16::from_bits(rgb));
                texels.push(f16::from_bits(rgb));
                texels.push(f16::from_bits(rgb));
                texels.push(f16::from_bits(alpha));
            }
            texels
        }
        fn fused(rgb: u16, alpha: u16) -> Vec<u8> {
            let mut out = Vec::new();
            extend_premultiplied_le_bytes(&fixture(rgb, alpha), &mut out);
            out
        }
        fn reference(rgb: u16, alpha: u16) -> Vec<u8> {
            let mut in_place = fixture(rgb, alpha);
            premultiply_rgba(&mut in_place);
            let mut out = Vec::new();
            for sample in &in_place {
                out.extend_from_slice(&sample.to_le_bytes());
            }
            out
        }
        /// The four channels of texel `index`, straight back out of the
        /// serialized bytes.
        fn texel(bytes: &[u8], index: usize) -> [u16; CHANNELS] {
            let mut out = [0u16; CHANNELS];
            let start = index * CHANNELS * 2;
            let Some(slice) = bytes.get(start..start + CHANNELS * 2) else {
                unreachable!("the fixture always has this many texels");
            };
            for (channel, pair) in out.iter_mut().zip(slice.chunks_exact(2)) {
                *channel = match pair {
                    [lo, hi] => u16::from_le_bytes([*lo, *hi]),
                    _ => unreachable!("chunks_exact(2) yields exactly two bytes"),
                };
            }
            out
        }
        /// Which operand's payload `bits` came from, or `None` if it came
        /// from neither -- the failure this whole test exists to catch.
        fn sourced_from_alpha(bits: u16, from_rgb: u16, from_alpha: u16) -> Option<bool> {
            if bits == from_alpha {
                Some(true)
            } else if bits == from_rgb {
                Some(false)
            } else {
                None
            }
        }

        // Per channel position, established by the first observation and
        // asserted equal by every one after it, across every texel and
        // every payload pair: which operand each path took, and whether
        // the two paths agreed.
        let mut fused_choice: [Option<bool>; COLOUR] = [None; COLOUR];
        let mut reference_choice: [Option<bool>; COLOUR] = [None; COLOUR];
        let mut discriminating_pairs = 0usize;

        for &rgb in &NANS {
            for &alpha in &NANS {
                // Multiplying by 1.0 leaves exactly one NaN operand, so
                // IEEE 754 propagates *that* one regardless of order --
                // these are the two candidate answers by construction,
                // not by assumption.
                let from_rgb = texel(&fused(rgb, ONE), 0);
                let from_alpha = texel(&fused(ONE, alpha), 0);
                for channel in 0..COLOUR {
                    for (label, probe) in [("rgb", &from_rgb), ("alpha", &from_alpha)] {
                        let bits = probe.get(channel).copied().unwrap_or_default();
                        assert!(
                            f16::from_bits(bits).is_nan(),
                            "the single-NaN {label} probe must stay NaN on channel \
                             {channel} (rgb {rgb:#06x}, alpha {alpha:#06x}), got \
                             {bits:#06x}"
                        );
                    }
                }
                if rgb == alpha {
                    // Identical operands carry identical payloads, so this
                    // pair cannot tell the two sourcings apart. Skipped,
                    // not asserted about -- `discriminating_pairs` below
                    // is what keeps that from hollowing the test out.
                    continue;
                }
                discriminating_pairs += 1;

                let fused_bytes = fused(rgb, alpha);
                let reference_bytes = reference(rgb, alpha);
                assert_eq!(fused_bytes.len(), CHUNK_TEXELS * CHANNELS * 2);
                assert_eq!(reference_bytes.len(), fused_bytes.len());

                for index in 0..CHUNK_TEXELS {
                    // Index 0 is this function's own output, index 1 the
                    // reference's -- both pinned by the same rules.
                    let paths = [
                        (
                            "extend_premultiplied_le_bytes",
                            texel(&fused_bytes, index),
                            &mut fused_choice,
                        ),
                        (
                            "premultiply_rgba",
                            texel(&reference_bytes, index),
                            &mut reference_choice,
                        ),
                    ];
                    for (path, observed, pinned) in paths {
                        for channel in 0..COLOUR {
                            let rgb_payload = from_rgb.get(channel).copied().unwrap_or_default();
                            let alpha_payload =
                                from_alpha.get(channel).copied().unwrap_or_default();
                            // `NANS` is chosen so quieting keeps every
                            // payload distinct -- without that, "came from
                            // alpha" and "came from rgb" would be the same
                            // observation and the pin below would be vacuous.
                            assert_ne!(
                                rgb_payload, alpha_payload,
                                "the two candidate payloads must differ (channel \
                                 {channel}, rgb {rgb:#06x}, alpha {alpha:#06x})"
                            );
                            let bits = observed.get(channel).copied().unwrap_or_default();
                            let Some(took_alpha) =
                                sourced_from_alpha(bits, rgb_payload, alpha_payload)
                            else {
                                unreachable!(
                                    "{path} produced {bits:#06x} on channel {channel} of \
                                     texel {index} for rgb {rgb:#06x} times alpha \
                                     {alpha:#06x}: neither operand's quieted payload \
                                     ({rgb_payload:#06x} from rgb, {alpha_payload:#06x} \
                                     from alpha)"
                                );
                            };
                            match pinned.get_mut(channel) {
                                Some(slot @ None) => *slot = Some(took_alpha),
                                Some(Some(previous)) => assert_eq!(
                                    *previous, took_alpha,
                                    "{path} must source channel {channel}'s payload from \
                                     the same operand for every texel and every payload \
                                     pair; it changed at texel {index}, rgb {rgb:#06x}, \
                                     alpha {alpha:#06x}"
                                ),
                                None => unreachable!("channel is below COLOUR"),
                            }
                        }
                        assert_eq!(
                            observed.get(COLOUR),
                            Some(&alpha),
                            "{path} must pass alpha through untouched even when both it \
                             and rgb are NaN (rgb {rgb:#06x}, texel {index})"
                        );
                    }
                }
            }
        }

        assert_eq!(
            discriminating_pairs, 30,
            "every ordered pair of distinct payloads from NANS must have been \
             checked; a smaller number means the fixture stopped discriminating"
        );
        assert!(
            fused_choice
                .iter()
                .chain(reference_choice.iter())
                .all(Option::is_some),
            "every colour channel of both paths must have been observed, or this \
             test asserted less than it claims"
        );
    }

    /// The whole arithmetic contract, on one texel at a time — no GPU
    /// needed, which is the point of extracting the helper at all.
    #[test]
    fn premultiply_rgba_scales_rgb_by_alpha() {
        let mut texels = [
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(0.5),
        ];
        premultiply_rgba(&mut texels);
        let got: Vec<f32> = texels.iter().map(|&s| f32::from(s)).collect();
        assert_eq!(got, vec![0.5, 0.5, 0.5, 0.5]);
    }

    /// A fully transparent texel becomes fully zero. This is the case
    /// the whole item exists for: it is what stops a transparent texel's
    /// arbitrary RGB from being dragged into a filtered tap next to an
    /// opaque neighbour.
    #[test]
    fn premultiply_rgba_zeroes_a_fully_transparent_texel() {
        // Deliberately *white* under zero alpha -- a straight-alpha
        // store can legitimately hold that, and it is the worst case for
        // bleeding.
        let mut texels = [
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(0.0),
        ];
        premultiply_rgba(&mut texels);
        let got: Vec<f32> = texels.iter().map(|&s| f32::from(s)).collect();
        assert_eq!(got, vec![0.0, 0.0, 0.0, 0.0]);
    }

    /// A fully opaque texel is left exactly as it was — bit-for-bit, not
    /// merely approximately, since multiplying by 1.0 must not perturb
    /// the `f16` representation.
    #[test]
    fn premultiply_rgba_leaves_a_fully_opaque_texel_unchanged() {
        let before = [
            f16::from_f32(0.25),
            f16::from_f32(0.75),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
        ];
        let mut texels = before;
        premultiply_rgba(&mut texels);
        assert_eq!(texels, before);
    }

    /// Every texel in a multi-texel buffer, not just the first — the
    /// `chunks_exact_mut` walk has to advance.
    #[test]
    fn premultiply_rgba_walks_every_texel_in_the_buffer() {
        let mut texels = [
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(0.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
        ];
        premultiply_rgba(&mut texels);
        let got: Vec<f32> = texels.iter().map(|&s| f32::from(s)).collect();
        assert_eq!(got, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    }

    /// `chunks_exact_mut` yields only whole texels, so a trailing
    /// partial one is left alone rather than corrupted. Unreachable
    /// through either real call site (both validate their lengths
    /// upstream), but defined rather than assumed away.
    #[test]
    fn premultiply_rgba_leaves_a_trailing_partial_texel_untouched() {
        let mut texels = [
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(1.0),
            f16::from_f32(0.0),
            // Half a texel.
            f16::from_f32(1.0),
            f16::from_f32(1.0),
        ];
        premultiply_rgba(&mut texels);
        let got: Vec<f32> = texels.iter().map(|&s| f32::from(s)).collect();
        assert_eq!(got, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0]);
    }

    /// `f16` rounding near alpha ~ 0, pinned rather than left implied.
    ///
    /// The worry worth pinning is a *flush to zero*: if a very small but
    /// non-zero alpha collapsed the colour to exactly zero, a barely
    /// visible layer would vanish from the canvas entirely rather than
    /// being faint. It does not — `f16` subnormals reach down to
    /// ~5.96e-8, so all three values below stay non-zero — and the exact
    /// products are pinned so a future change to the arithmetic (a
    /// different intermediate precision, say) is visible rather than
    /// silent.
    #[test]
    fn premultiply_rgba_does_not_flush_a_faint_texel_to_zero() {
        for (colour, alpha, expected) in [
            (1.0_f32, 6.002_188e-5_f32, 6.002_188e-5_f32),
            (0.5, 1.000_166e-4, 5.000_83e-5),
            // The smallest positive `f16` there is, at full brightness.
            (1.0, 5.960_464_5e-8, 5.960_464_5e-8),
            (0.25, 1.000_404_4e-3, 2.501_011e-4),
        ] {
            let mut texels = [
                f16::from_f32(colour),
                f16::from_f32(colour),
                f16::from_f32(colour),
                f16::from_f32(alpha),
            ];
            premultiply_rgba(&mut texels);
            let Some(&red) = texels.first() else {
                unreachable!("a four-element array always has a first element");
            };
            assert!(
                f32::from(red) > 0.0,
                "a faint but non-zero texel must not vanish: {colour} * {alpha} became zero"
            );
            assert_eq!(
                red,
                f16::from_f32(expected),
                "premultiplied {colour} * {alpha}"
            );
        }
    }

    // ---------------------------------------------------------------
    // 0.96.0: the rayon-parallel serializer.
    //
    // **Every fixture above is far shorter than one `BLOCK_SAMPLES`**
    // (4,096 samples), so without the tests below the parallel splitter
    // would never be handed more than a single block and every green run
    // above would be measuring a one-task "parallel" path. These are what
    // actually exercise multiple blocks, a short trailing block, and a
    // whole tile.
    // ---------------------------------------------------------------

    /// The eight `f16` bit patterns a premultiply bug hides in, as used by
    /// `the_fused_serializer_matches_premultiply_then_serialize_across_a_chunk_boundary`
    /// above. `0x7c01` (a signalling NaN) is last on purpose: it lands on
    /// the alpha channel on every other extreme texel, and alpha is the one
    /// channel the "sourced alpha from the round-tripped scratch buffer"
    /// mutation can change.
    const EXTREMES: [u16; 8] = [
        0x0000, // +0
        0x8000, // -0
        0x0001, // smallest subnormal
        0x03ff, // largest subnormal
        0x7bff, // largest finite (65504)
        0x7c00, // +infinity
        0xfc00, // -infinity
        0x7c01, // signalling NaN
    ];

    /// `samples` samples whose value varies per texel *and* per channel,
    /// with every fifth texel filled from a cycle of [`EXTREMES`] instead.
    ///
    /// Varying per channel is what makes a swapped R/G/B, a stale
    /// scratch-buffer lane carried between chunks, an alpha read from the
    /// wrong texel, or a block written at the wrong offset all show up as a
    /// byte mismatch rather than hiding behind uniform data. The every-fifth
    /// extreme texel places NaN/infinity/subnormal/signed-zero inputs inside
    /// *every* block of any fixture longer than 20 texels, and inside the
    /// trailing partial block too, since the texel index keeps counting
    /// across block boundaries.
    fn varied_fixture(samples: usize) -> Vec<f16> {
        let mut extremes = EXTREMES.iter().cycle();
        let mut source: Vec<f16> = Vec::with_capacity(samples);
        let mut texel = 0usize;
        while source.len() < samples {
            if texel.is_multiple_of(5) {
                for _ in 0..CHANNELS {
                    if source.len() == samples {
                        break;
                    }
                    // `cycle()` over a non-empty array never yields `None`;
                    // the fallback is only here because the workspace
                    // denies `unwrap`.
                    let bits = extremes.next().copied().unwrap_or(0x3800);
                    source.push(f16::from_bits(bits));
                }
            } else {
                let n = texel as f32;
                for channel in [
                    n / 137.0,
                    1.0 - n / 149.0,
                    ((texel * 7) % 137) as f32 / 137.0,
                    // Alpha sweeps 0.0 through 1.0 inclusive, so both a
                    // fully transparent and a fully opaque texel appear.
                    ((texel * 13) % 131) as f32 / 130.0,
                ] {
                    if source.len() == samples {
                        break;
                    }
                    source.push(f16::from_f32(channel));
                }
            }
            texel += 1;
        }
        source
    }

    /// The scalar oracle every test below compares against: the *obvious*
    /// spelling — [`premultiply_rgba`] in place, then a plain
    /// little-endian serialize of the whole-texel prefix — with no
    /// vectorization, no blocking, and no `rayon` anywhere near it.
    ///
    /// Deliberately not shared with the parallel path in any way, so an
    /// error common to both cannot cancel out.
    fn scalar_reference(source: &[f16]) -> Vec<u8> {
        let whole = source.len() / CHANNELS * CHANNELS;
        let mut in_place = source.to_vec();
        premultiply_rgba(&mut in_place);
        let mut expected: Vec<u8> = Vec::with_capacity(whole * 2);
        for sample in in_place.iter().take(whole) {
            expected.extend_from_slice(&sample.to_le_bytes());
        }
        expected
    }

    /// **The primary correctness guard for 0.96.0's parallel serializer.**
    ///
    /// The length is chosen so one call covers every structural case at
    /// once: `BLOCK_SAMPLES * 3` gives three *full* rayon blocks (so the
    /// splitter genuinely has to hand disjoint sub-slices to more than one
    /// worker, and a block written at the wrong offset cannot go unnoticed);
    /// `+ CHUNK_SAMPLES` makes the fourth block short but still containing a
    /// whole vectorized chunk; `+ 13 * CHANNELS` adds thirteen whole texels
    /// that fall into the sequential core's *scalar remainder*; and `+ 2`
    /// adds a trailing incomplete texel, which must contribute nothing.
    ///
    /// Extremes (NaN, both infinities, both signed zeros, both subnormal
    /// bounds) land in all four blocks and in the remainder, per
    /// [`varied_fixture`]'s own doc.
    ///
    /// Compared byte-for-byte against [`scalar_reference`]. Non-vacuity was
    /// checked by hand for 0.96.0 with two mutations of
    /// `write_premultiplied_le_bytes`, both of which this test catches: a
    /// mismatched output chunk size (`BLOCK_SAMPLES` instead of
    /// `BLOCK_SAMPLES * 2`), and an off-by-one-block `zip` between the input
    /// and output block iterators.
    #[test]
    fn the_parallel_serializer_matches_the_scalar_reference_across_block_boundaries() {
        let samples = BLOCK_SAMPLES * 3 + CHUNK_SAMPLES + (13 * CHANNELS) + 2;
        let source = varied_fixture(samples);
        assert_eq!(source.len(), samples, "the fixture must be exactly as long");

        let mut fused = Vec::new();
        extend_premultiplied_le_bytes(&source, &mut fused);

        let expected = scalar_reference(&source);
        assert_eq!(
            fused.len(),
            expected.len(),
            "the whole-texel prefix must be serialized and the trailing \
             partial texel dropped"
        );
        assert_eq!(
            fused, expected,
            "the rayon-parallel serializer must match the scalar \
             premultiply-then-serialize reference bit-for-bit across three \
             full blocks, a short block, a scalar remainder and a trailing \
             partial texel"
        );
    }

    /// A whole tile: [`TileResidency::sync`]'s per-frame shape, and the
    /// largest input the splitter can be handed.
    ///
    /// [`SAMPLES`] is `BLOCK_SAMPLES * 64`, so this is the 64-task case the
    /// parallelism was added for — no short block, no remainder. Kept, and
    /// still exactly the right coverage, even though 0.96.2 routed `sync`
    /// itself onto the sequential core: the splitter is still live code
    /// ([`TileResidency::upload_mip`]) and is still the arm a future
    /// load-sensing design would re-enable here, so a whole tile is the
    /// bit-exactness case that must keep holding.
    #[test]
    fn the_parallel_serializer_matches_the_scalar_reference_for_a_whole_tile() {
        let source = varied_fixture(SAMPLES);
        assert_eq!(source.len(), SAMPLES);

        let mut fused = Vec::new();
        extend_premultiplied_le_bytes(&source, &mut fused);

        assert_eq!(
            fused.len(),
            TILE_BYTES,
            "a whole tile must serialize to exactly one tile's worth of bytes"
        );
        assert_eq!(
            fused,
            scalar_reference(&source),
            "a whole tile must match the scalar reference bit-for-bit"
        );
    }

    /// The arithmetic the parallel splitter's correctness rests on, pinned
    /// so that a future edit to [`BLOCK_SAMPLES`]/[`CHUNK_SAMPLES`] which
    /// breaks it fails loudly here instead of silently pushing real uploads
    /// onto an untested path.
    ///
    /// - `SAMPLES % BLOCK_SAMPLES == 0`: a whole tile splits into equal
    ///   blocks with no short trailing one, which is the case every real
    ///   frame takes.
    /// - `BLOCK_SAMPLES % CHUNK_SAMPLES == 0`: every full block is a whole
    ///   number of vectorized chunks, so no block is pushed onto the scalar
    ///   remainder that the sequential walk would have vectorized.
    /// - `BLOCK_SAMPLES % CHANNELS == 0`: a texel never straddles a block
    ///   boundary, so no worker can see a partial texel. This is the one
    ///   that makes "bit-identical regardless of thread count" true rather
    ///   than merely usually-true.
    /// - `TILE_BYTES == SAMPLES * 2`: the `par_chunks_mut(BLOCK_SAMPLES *
    ///   2)` output stride is the right one, i.e. two bytes of output per
    ///   input sample.
    #[test]
    fn the_block_and_chunk_constants_divide_a_whole_tile_evenly() {
        assert_eq!(
            SAMPLES % BLOCK_SAMPLES,
            0,
            "a whole tile must split into whole blocks"
        );
        assert_eq!(
            SAMPLES / BLOCK_SAMPLES,
            64,
            "a whole tile is expected to become 64 rayon tasks; if this \
             changes deliberately, re-measure before accepting it"
        );
        assert_eq!(
            BLOCK_SAMPLES % CHUNK_SAMPLES,
            0,
            "a block must be a whole number of vectorized chunks"
        );
        assert_eq!(
            BLOCK_SAMPLES % CHANNELS,
            0,
            "a texel must never straddle a block boundary"
        );
        assert_eq!(
            TILE_BYTES,
            SAMPLES * 2,
            "two output bytes per input sample is what the output chunk \
             stride assumes"
        );
    }

    /// Every length class around a chunk and a block boundary, in one
    /// sweep. This is the executable half of
    /// [`serialize_premultiplied_le_bytes`]'s panic-freedom argument: any
    /// length-mismatch bug in the `half` conversions fires that crate's own
    /// internal `assert_eq!`, and a test build does not set `panic =
    /// "abort"`, so it surfaces here as an ordinary test failure rather
    /// than as a process abort in a shipped release.
    #[test]
    fn the_parallel_serializer_matches_the_scalar_reference_at_every_boundary_length() {
        for samples in [
            0,
            1,
            CHANNELS - 1,
            CHANNELS,
            CHANNELS + 1,
            CHUNK_SAMPLES - 1,
            CHUNK_SAMPLES,
            CHUNK_SAMPLES + 1,
            BLOCK_SAMPLES - 1,
            BLOCK_SAMPLES,
            BLOCK_SAMPLES + 1,
            2 * BLOCK_SAMPLES - 1,
            2 * BLOCK_SAMPLES,
            2 * BLOCK_SAMPLES + 1,
        ] {
            let source = varied_fixture(samples);
            let mut fused = Vec::new();
            extend_premultiplied_le_bytes(&source, &mut fused);
            assert_eq!(
                fused,
                scalar_reference(&source),
                "mismatch at an input length of {samples} samples"
            );
        }
    }

    /// The splitter called directly with an `out` buffer that is *not*
    /// exactly the right length — one byte short, and one byte long.
    ///
    /// Neither may panic. `write_premultiplied_le_bytes` and its sequential
    /// core consume `out` through `chunks_exact_mut(CHANNELS * 2)` zipped
    /// against the texel iterators, never by indexing or
    /// `copy_from_slice`, so a short buffer writes fewer whole texels and a
    /// long one leaves its tail untouched. That property is what makes both
    /// [`TileResidency::sync`]'s reused pre-sized buffer and the splitter's
    /// short trailing block safe without a length assertion, so it is
    /// checked rather than asserted in prose.
    ///
    /// Checked at **two** fixture sizes (0.96.1): one shorter than a single
    /// [`BLOCK_SAMPLES`], which
    /// [`write_premultiplied_le_bytes_on`]'s size guard runs inline with no
    /// pool at all, and one spanning three blocks, which really does go
    /// through the parallel splitter. Only the second exercises what the
    /// mis-sizing actually threatens — a *short trailing* `par_chunks_mut`
    /// chunk sitting after two full, correctly-offset ones — and through
    /// 0.96.0 this test had only the first.
    #[test]
    fn the_parallel_serializer_tolerates_a_mis_sized_output_buffer() {
        const SENTINEL: u8 = 0xab;
        let single = CHUNK_SAMPLES * 2 + 12;
        let multi = BLOCK_SAMPLES * 2 + CHUNK_SAMPLES + 12;
        assert!(single < BLOCK_SAMPLES, "the first fixture is one block");
        assert!(
            multi > BLOCK_SAMPLES * 2,
            "the second fixture spans more than two blocks, so the last \
             par_chunks_mut chunk is the short one"
        );

        for samples in [single, multi] {
            let source = varied_fixture(samples);
            let expected = scalar_reference(&source);
            let texels = source.len() / CHANNELS;
            assert_eq!(expected.len(), texels * CHANNELS * 2);

            // One byte short: the last texel has nowhere to go, so exactly
            // `texels - 1` texels are written and nothing panics.
            let mut short = vec![0u8; expected.len() - 1];
            write_premultiplied_le_bytes(&source, &mut short);
            let prefix = (texels - 1) * CHANNELS * 2;
            assert_eq!(
                short.get(..prefix),
                expected.get(..prefix),
                "a one-byte-short buffer must still receive every texel that \
                 fits ({samples} samples)"
            );

            // One byte long: the extra byte falls in `chunks_exact_mut`'s
            // remainder and must be left exactly as it was.
            let mut long = vec![SENTINEL; expected.len() + 1];
            write_premultiplied_le_bytes(&source, &mut long);
            assert_eq!(
                long.get(..expected.len()),
                expected.get(..),
                "a one-byte-long buffer must receive every texel ({samples} \
                 samples)"
            );
            assert_eq!(
                long.last(),
                Some(&SENTINEL),
                "the byte past the last whole texel must be untouched \
                 ({samples} samples)"
            );
        }
    }

    /// **Issue 1's regression guard (0.96.1): the sequential fallback a
    /// machine that cannot spawn threads takes is real, reachable, and
    /// produces the same bytes.**
    ///
    /// 0.96.0 reached `rayon`'s *implicit global* pool, whose lazy
    /// initializer `.expect()`s its own `ThreadPoolBuildError` — so under a
    /// process/thread limit (`RLIMIT_NPROC`, a cgroup `pids.max`, systemd
    /// `TasksMax`, memory pressure; reproduced with `ulimit -u`) the tile
    /// upload path panicked inside `rayon`, which `panic = "abort"` turns
    /// into `SIGABRT` on every frame including the default startup
    /// document's.
    ///
    /// This drives that exact failure rather than mocking around it: a
    /// `spawn_handler` that refuses with `ErrorKind::WouldBlock` — `errno`
    /// 11, the same error the `ulimit -u` reproduction produced — makes
    /// [`rayon::ThreadPoolBuilder::build`] return the real
    /// `ThreadPoolBuildError`, `.ok()` collapses it to the `None` the
    /// production code stores, and
    /// [`write_premultiplied_le_bytes_on`] is then called with it: the same
    /// single dispatch point a real frame goes through. The fixture is
    /// deliberately longer than one `BLOCK_SAMPLES`, so a pool *would* have
    /// been used had one built.
    ///
    /// Two things are asserted: it does not panic (reaching the final
    /// assertion is that), and the fallback's bytes are byte-identical to
    /// the scalar oracle — i.e. degrading to it costs correctness nothing.
    #[test]
    fn the_serializer_falls_back_to_the_sequential_core_without_a_pool() {
        let failed = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .spawn_handler(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "simulated RLIMIT_NPROC exhaustion",
                ))
            })
            .build();
        let Err(err) = failed else {
            // Not a failure of the code under test, so not an assertion:
            // if some future rayon builds a pool despite a refusing spawn
            // handler, there is nothing here to check.
            return;
        };
        // Formatted rather than ignored so a failing run's log shows what
        // rayon actually reported -- and so the test records that this is a
        // *value* on the `Err` path, which is the whole fix, rather than the
        // panic 0.96.0's global pool would have produced from the same
        // condition.
        let reported = format!("{err}");
        assert!(
            !reported.is_empty(),
            "rayon must report a real ThreadPoolBuildError, not panic"
        );
        let unavailable: Option<rayon::ThreadPool> = None;

        let source = varied_fixture(BLOCK_SAMPLES * 3 + CHUNK_SAMPLES + 12);
        assert!(
            source.len() > BLOCK_SAMPLES,
            "a pool would have been used if one had built"
        );
        let mut fallback = vec![0u8; source.len() * 2];
        write_premultiplied_le_bytes_on(unavailable.as_ref(), &source, &mut fallback);
        assert_eq!(
            fallback,
            scalar_reference(&source),
            "with no thread pool the serializer must still produce the \
             sequential bytes rather than panicking or writing nothing"
        );
    }

    /// The pool is **bounded** (0.96.1, Issue 2): `rayon`'s global default
    /// takes every logical core, which measured as a 4-5× `upload_sync`
    /// regression under 8 competing CPU-bound threads because a synchronous
    /// `install` blocks its caller until the slowest block finishes — and
    /// through 0.96.1 that caller was [`TileResidency::sync`], on the frame
    /// thread. The bound outlives 0.96.2's routing change because
    /// [`TileResidency::upload_mip`] still dispatches onto this pool.
    ///
    /// Pinned here rather than argued: the worker count never exceeds
    /// [`SERIALIZER_MAX_THREADS`], never exceeds the machine's own logical
    /// core count, and is never 1 (a one-worker pool is pure handoff cost —
    /// `install` blocks the caller instead of letting it join in). A
    /// deliberate change to any of those should re-measure and edit this
    /// test, which is the point of it.
    #[test]
    fn the_serializer_pool_is_bounded_well_below_a_machines_core_count() {
        let threads = serializer_pool_threads();
        assert!(
            threads <= SERIALIZER_MAX_THREADS,
            "{threads} workers exceeds the {SERIALIZER_MAX_THREADS}-worker cap"
        );
        assert_ne!(threads, 1, "a one-worker pool is pure overhead");
        if let Ok(cores) = std::thread::available_parallelism() {
            assert!(
                threads < cores.get(),
                "{threads} workers must leave headroom on a {cores}-core machine"
            );
        }
        // 0 means "no pool, serialize inline" and must not build one.
        assert!(
            build_serializer_pool(0).is_none(),
            "a zero-worker request must not produce a pool"
        );
        // And the real pool agrees with the size decision either way.
        match serializer_pool() {
            Some(pool) => assert_eq!(
                pool.current_num_threads(),
                threads,
                "the built pool must have exactly the bounded worker count"
            ),
            None => assert_eq!(
                threads, 0,
                "no pool is only correct when no pool was asked for"
            ),
        }
    }

    /// **The budget/ordering characterization guard.** `sync`'s
    /// `bytes_left` budget decides *which* tiles upload, in a fixed
    /// row-major order, and the two tests above
    /// (`budget_limited_sync_converges_over_multiple_calls` and
    /// `a_resident_tile_skipped_for_budget_is_still_uploaded_on_a_later_call`)
    /// pin the *counts* that come out of that — but nothing pinned the
    /// *identities*. A change that uploaded the last two tiles first, or in
    /// a `HashMap`'s iteration order, would pass both of them.
    ///
    /// Added in 0.96.0 alongside the block-level parallelism, which
    /// deliberately does not touch this logic. Its real purpose is the
    /// *next* round: an across-tile parallelization (see
    /// [`split_premultiplied_le_bytes`]'s doc for why it is blocked today)
    /// would have to reproduce this ordering exactly, and this test is what
    /// makes an accidental reordering a test failure instead of a silent
    /// behaviour change a user would see as tiles filling in from the wrong
    /// corner.
    ///
    /// Reads `residency.slots` directly, as
    /// `resize_to_a_smaller_grid_drops_the_slot_mapping` above already does.
    #[test]
    fn a_budget_limited_sync_uploads_the_first_tiles_in_row_major_order() {
        let Some(context) = real_context() else {
            return;
        };
        let (_dir, mut store) = real_tile_store(64);

        // 256x256 viewport -> a (2, 2) grid at origin (0, 0), so the
        // row-major visit order is (0,0), (1,0), (0,1), (1,1) and each tile
        // maps to the slot of the same coordinates.
        let viewport = (256, 256);
        let mut residency = TileResidency::new(context.device(), context.queue(), viewport);
        assert_eq!(residency.grid, (2, 2));
        for gy in 0..2 {
            for gx in 0..2 {
                paint(&mut store, TileId { x: gx, y: gy }, [0.0, 1.0, 1.0, 1.0]);
            }
        }

        let first = residency.sync(
            context.queue(),
            &mut store,
            surface(),
            false,
            TILE_BYTES * 2,
        );
        assert_eq!(first.uploaded, 2);
        assert_eq!(first.errors, 0);
        // The other half of "a two-tile budget uploaded the first two":
        // the remaining two must be *reported* as still owed, not silently
        // dropped. Added in 0.96.1 -- without it a `sync` that uploaded two
        // tiles and forgot the rest would pass this test.
        assert_eq!(
            first.remaining, 2,
            "the two tiles the budget skipped must be reported as remaining"
        );

        let mut mapped: Vec<((u32, u32), TileId)> =
            residency.slots.iter().map(|(&s, &id)| (s, id)).collect();
        mapped.sort_by_key(|&(slot, _)| (slot.1, slot.0));
        assert_eq!(
            mapped,
            vec![
                ((0, 0), TileId { x: 0, y: 0 }),
                ((1, 0), TileId { x: 1, y: 0 }),
            ],
            "a budget for two tiles must upload the first two in row-major \
             order -- not the last two, and not an arbitrary pair"
        );

        let second = residency.sync(context.queue(), &mut store, surface(), false, usize::MAX);
        assert_eq!(second.uploaded, 2);
        assert_eq!(second.remaining, 0);

        let mut all: Vec<((u32, u32), TileId)> =
            residency.slots.iter().map(|(&s, &id)| (s, id)).collect();
        all.sort_by_key(|&(slot, _)| (slot.1, slot.0));
        assert_eq!(
            all,
            vec![
                ((0, 0), TileId { x: 0, y: 0 }),
                ((1, 0), TileId { x: 1, y: 0 }),
                ((0, 1), TileId { x: 0, y: 1 }),
                ((1, 1), TileId { x: 1, y: 1 }),
            ],
            "a second, unlimited call must finish the backlog"
        );
    }
}
