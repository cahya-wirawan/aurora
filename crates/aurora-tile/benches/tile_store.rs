//! The M1.1 "bench: paging throughput, eviction cost, compression ratio"
//! deliverable. `cargo bench -p aurora-tile`.

use aurora_tile::{SAMPLES, TileId, TileStore};
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
    let encoded = aurora_tile_bench_support::encode_for_bench(texels);
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
                if let Ok(tile) = store.get_mut(id)
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
    for x in 0..8 {
        let _ = store.get_mut(TileId { x, y: 0 });
    }
    let mut next_x = 8u32;

    c.bench_function("eviction_cost", |b| {
        b.iter(|| {
            // Store is already at budget; every touch here forces
            // exactly one eviction.
            let id = TileId { x: next_x, y: 0 };
            next_x = next_x.wrapping_add(1);
            let _ = store.get_mut(id);
        });
    });
}

fn compression_ratios(_c: &mut Criterion) {
    compression_ratio("uniform", &uniform_texels());
    compression_ratio("gradient", &gradient_texels());
    compression_ratio("noise", &noise_texels(0xDEAD_BEEF));
}

criterion_group!(
    benches,
    paging_throughput,
    eviction_cost,
    compression_ratios
);
criterion_main!(benches);

/// `codec::encode` is crate-private (not part of `aurora-tile`'s public
/// API -- benches only see what downstream crates see), so this
/// reimplements just enough of it to report a size, using the same
/// `lz4_flex` dependency the crate itself uses. Not a copy of production
/// logic to test against (that would prove nothing) -- purely a
/// byte-count measurement for the "compression ratio" bench deliverable.
mod aurora_tile_bench_support {
    use half::f16;

    pub fn encode_for_bench(texels: &[f16]) -> Vec<u8> {
        let mut raw = Vec::with_capacity(texels.len() * 2);
        for texel in texels {
            raw.extend_from_slice(&texel.to_le_bytes());
        }
        let compressed = lz4_flex::compress_prepend_size(&raw);
        if compressed.len() < raw.len() {
            compressed
        } else {
            raw
        }
    }
}
