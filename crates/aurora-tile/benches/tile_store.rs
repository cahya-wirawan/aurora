//! The M1.1 "bench: paging throughput, eviction cost, compression ratio"
//! deliverable. `cargo bench -p aurora-tile`.

use aurora_tile::{SAMPLES, SurfaceId, TileId, TileStore};
use criterion::{Criterion, criterion_group, criterion_main};
use half::f16;
use std::num::NonZeroUsize;

fn uniform_texels() -> Vec<f16> {
    vec![f16::from_f32(0.25); SAMPLES]
}

fn gradient_texels() -> Vec<f16> {
    (0..SAMPLES)
        .map(|i| f16::from_f32((i % 256) as f32 / 256.0))
        .collect()
}

/// Deterministic xorshift64 -- poorly-compressible content, without
/// pulling in a `rand` dependency for a one-off benchmark input.
fn noise_texels(seed: u64) -> Vec<f16> {
    let mut state = seed.max(1);
    (0..SAMPLES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            f16::from_f32((state % 1000) as f32 / 1000.0)
        })
        .collect()
}

fn compression_ratio(name: &str, texels: &[f16]) {
    // The real encoder, not a bench-local restatement of it: `codec` is
    // a `pub mod` and `encode` a `pub fn`, so a ratio reported here is a
    // ratio the shipped code actually achieves. (This file once carried
    // its own `lz4_flex` reimplementation, justified by a comment
    // claiming `codec::encode` was crate-private. It never was, and a
    // second copy of the encoding rules could silently drift from the
    // one being measured.)
    let encoded = aurora_tile::codec::encode(texels);
    let raw_bytes = texels.len() * 2;
    #[allow(clippy::cast_precision_loss)]
    let ratio = encoded.len() as f64 / raw_bytes as f64;
    eprintln!(
        "compression ratio [{name}]: {} / {raw_bytes} bytes ({ratio:.3})",
        encoded.len()
    );
}

fn new_store(dir: &tempfile::TempDir, budget: usize) -> TileStore {
    let Some(budget) = NonZeroUsize::new(budget) else {
        unreachable!("bench-local budgets are always non-zero literals");
    };
    match TileStore::new(dir.path().to_path_buf(), budget) {
        Ok(store) => store,
        Err(err) => unreachable!("a tempdir-backed scratch dir must be usable: {err}"),
    }
}

fn new_tempdir() -> tempfile::TempDir {
    match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => unreachable!("tempdir creation must succeed in a benchmark environment: {err}"),
    }
}

fn paging_throughput(c: &mut Criterion) {
    let dir = new_tempdir();
    let mut store = new_store(&dir, 16);
    let surface = SurfaceId::from_raw(0);
    let mut next_x = 0u32;

    c.bench_function("paging_throughput", |b| {
        b.iter(|| {
            // 32 distinct tiles against a 16-tile budget forces real
            // eviction and page-in on every iteration, not just fresh
            // allocation -- this is the mixed workload "paging
            // throughput" is actually about.
            for _ in 0..32 {
                let id = TileId { x: next_x, y: 0 };
                next_x = next_x.wrapping_add(1);
                if let Ok(tile) = store.get_mut(surface, id)
                    && let Some(first) = tile.texels_mut().first_mut()
                {
                    *first = f16::from_f32(1.0);
                }
            }
            let _ = store.flush();
        });
    });
}

fn eviction_cost(c: &mut Criterion) {
    let dir = new_tempdir();
    let mut store = new_store(&dir, 8);
    let surface = SurfaceId::from_raw(0);
    for x in 0..8 {
        let _ = store.get_mut(surface, TileId { x, y: 0 });
    }
    let mut next_x = 8u32;

    c.bench_function("eviction_cost", |b| {
        b.iter(|| {
            // Store is already at budget; every touch here forces
            // exactly one eviction.
            let id = TileId { x: next_x, y: 0 };
            next_x = next_x.wrapping_add(1);
            let _ = store.get_mut(surface, id);
        });
    });
}

/// Splits eviction's two synchronous costs apart. Before 0.59.0,
/// `TileStore::make_room` encoded the victim tile and then handed
/// `bytes.clone()` to `pending` while the original went to the
/// background `WriteJob`, so an eviction on the brush thread paid one
/// `codec::encode` *plus* one allocate-and-copy of the compressed
/// bytes. That clone is gone — both holders now share one
/// `Arc<Vec<u8>>`, built by `Arc::new` over the buffer `codec::encode`
/// already returned, so no byte is copied at all — and these benches are
/// kept so the cost it *used* to carry stays measurable rather than
/// being a claim in a changelog.
///
/// The `Arc<Vec<u8>>` is load-bearing, not incidental: an intermediate
/// version of the fix used `Arc<[u8]>` via `.into()`, which does **not**
/// remove this cost. `Arc<[T]>` stores its refcount inline with the
/// payload, so converting from a `Vec` allocates a fresh block and
/// memcpies into it — the same copy, one line earlier. The arms below
/// therefore still measure a real quantity for that variant; they are
/// moot only for the `Arc<Vec<u8>>` code actually shipped.
///
/// Three arms, deliberately:
///
/// - `encode/*` — the encode half, which is unchanged and remains the
///   dominant synchronous cost of an eviction.
/// - `pending_clone/*` — the removed clone, measured the naive way: the
///   copy is dropped at the end of every iteration, so the allocator
///   hands back the same already-page-faulted, cache-warm block each
///   time. **This understates what `make_room` actually did** and is
///   kept only as context for the number the 0.58.x measurement
///   originally reported.
/// - `pending_clone_retained/*` — the honest shape. `make_room`'s clone
///   in `pending` stayed live until its write completed, *alongside*
///   every other in-flight eviction's clone, so the real cost was a
///   copy into cold, freshly-faulted memory. This holds
///   [`RETAINED_DEPTH`] copies live at once before dropping any,
///   mirroring that retention. It measured ~3x the naive arm at depth
///   16 — the difference that reversed this item's conclusion.
///
/// **Absolute numbers here are environment-sensitive** — allocator
/// tuning in particular (`glibc`'s dynamic `mmap` threshold decides
/// whether a 512 KiB copy reuses a warm heap block or faults in fresh
/// pages) — so read them as indicative of a ratio on one machine, not
/// as a portable cost, the same discipline the rest of this project
/// applies to locally-measured numbers.
fn clone_vs_encode(c: &mut Criterion) {
    for (name, texels) in [
        ("uniform", uniform_texels()),
        ("gradient", gradient_texels()),
        ("noise", noise_texels(0xDEAD_BEEF)),
    ] {
        let encoded: Vec<u8> = aurora_tile::codec::encode(&texels);
        eprintln!("encoded len [{name}]: {} bytes", encoded.len());
        c.bench_function(&format!("encode/{name}"), |b| {
            b.iter(|| {
                std::hint::black_box(aurora_tile::codec::encode(std::hint::black_box(&texels)))
            });
        });
        c.bench_function(&format!("pending_clone/{name}"), |b| {
            b.iter(|| std::hint::black_box(std::hint::black_box(&encoded).clone()));
        });
        c.bench_function(&format!("pending_clone_retained/{name}"), |b| {
            // A ring of live copies, refilled one slot per iteration:
            // at steady state RETAINED_DEPTH - 1 clones are still
            // alive when the next one allocates, which is what
            // `pending` looked like with that many writes in flight.
            let mut live: Vec<Vec<u8>> = Vec::with_capacity(RETAINED_DEPTH);
            let mut slot = 0usize;
            b.iter(|| {
                let copy = std::hint::black_box(std::hint::black_box(&encoded).clone());
                if live.len() < RETAINED_DEPTH {
                    live.push(copy);
                } else if let Some(existing) = live.get_mut(slot) {
                    *existing = copy;
                }
                slot = (slot + 1) % RETAINED_DEPTH;
            });
            std::hint::black_box(&live);
        });
    }
}

/// How many simultaneously-live copies `pending_clone_retained` holds.
///
/// 16 is not arbitrary: it is where the measured clone cost crossed the
/// "≥ 15 % of `codec::encode`" limb of this item's pre-committed
/// threshold. That threshold argument is now moot for the shipped code,
/// which performs no copy at any depth, but the depth is kept because it
/// is reachable in production — the background
/// writer's `mpsc` queue is unbounded (`submit` never blocks) and
/// `failed_write_capacity()` equals the store's whole tile budget, so a
/// slow or contended scratch disk can leave that many 512 KiB entries
/// pinned in `pending` at once.
const RETAINED_DEPTH: usize = 16;

fn compression_ratios(_c: &mut Criterion) {
    compression_ratio("uniform", &uniform_texels());
    compression_ratio("gradient", &gradient_texels());
    compression_ratio("noise", &noise_texels(0xDEAD_BEEF));
}

criterion_group!(
    benches,
    paging_throughput,
    eviction_cost,
    clone_vs_encode,
    compression_ratios
);
criterion_main!(benches);
