//! The PSD writer's per-sample work, made cheaper and band-parallel
//! (task 154) without changing a byte of the file.
//!
//! [`super::write`]'s channel writer fills one tile-row band of `u16`
//! samples from the store (sequential: the store is `&mut`), then this
//! module turns the band's rows into `PackBits` rows plus the two
//! order-independent hashes the verify checks, on up to
//! [`MAX_THREADS`] scoped threads, each taking whole rows. The caller
//! writes the packed rows in row order, so the file is the one the
//! single-threaded loop wrote:
//!
//! * a row's packed bytes depend on that row alone ([`pack_bits`]);
//! * the hashes are wrapping sums of per-sample terms, so any split adds
//!   up to the same value;
//! * clipped and `NaN` counts are sums too.
//!
//! The quantisation of an `f16` sample goes through [`quantize_f16`]'s
//! 65,536-entry table, built from exactly `quantize_u16(v.to_f32())` and
//! the same range tests as the per-sample path, and the decoded hash
//! promotes through the reader's own exact `promote_u16` table.
//!
//! Memory: one band (at most `TILE` rows) of samples and its packed
//! rows, or [`MERGED_CHUNK_ROWS`] rows of the merged image, whatever the
//! document's size.

use aurora_color::{promote_u16, quantize_u16};
use half::f16;

use super::write::{pack_bits, sample_hash};

/// The most threads one band is split over.
pub(crate) const MAX_THREADS: usize = 8;

/// The fewest samples in a band worth a second thread.
const PARALLEL_MIN_SAMPLES: usize = 1 << 16;

/// Rows of the merged image read and packed together.
pub(crate) const MERGED_CHUNK_ROWS: usize = 64;

/// What [`quantize_counted`] clipped or zeroed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SampleCounts {
    pub(crate) clipped: u64,
    pub(crate) nan: u64,
}

impl SampleCounts {
    fn add(&mut self, other: Self) {
        self.clipped += other.clipped;
        self.nan += other.nan;
    }
}

/// A float sample to the file's `u16`, counting what is clipped or `NaN`:
/// the writer's `quantize`.
pub(crate) fn quantize_counted(value: f32, counts: &mut SampleCounts) -> u16 {
    if value.is_nan() {
        counts.nan += 1;
        0
    } else {
        if !(0.0..=1.0).contains(&value) {
            counts.clipped += 1;
        }
        quantize_u16(value)
    }
}

/// Bit 16 of a table entry: the sample is clipped; bit 17: it is `NaN`.
const CLIPPED_BIT: u32 = 1 << 16;
const NAN_BIT: u32 = 1 << 17;

/// Every `f16` bit pattern's [`quantize_counted`] result: the `u16` in
/// the low bits, the count it adds in bits 16 and 17.
fn quantize_table() -> &'static [u32] {
    static TABLE: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|bits| {
                let mut counts = SampleCounts::default();
                let q = quantize_counted(f16::from_bits(bits).to_f32(), &mut counts);
                let mut entry = u32::from(q);
                if counts.clipped > 0 {
                    entry |= CLIPPED_BIT;
                }
                if counts.nan > 0 {
                    entry |= NAN_BIT;
                }
                entry
            })
            .collect()
    })
}

/// [`quantize_counted`] of `value.to_f32()`, through the table.
pub(crate) fn quantize_f16(value: f16, counts: &mut SampleCounts) -> u16 {
    match quantize_table().get(usize::from(value.to_bits())) {
        Some(&entry) => {
            if entry & CLIPPED_BIT != 0 {
                counts.clipped += 1;
            }
            if entry & NAN_BIT != 0 {
                counts.nan += 1;
            }
            (entry & 0xffff) as u16
        }
        None => quantize_counted(value.to_f32(), counts),
    }
}

/// The `f16` bits the reader decodes a written `u16` to:
/// `f16::from_f32(promote_u16(q))`, through the reader's table.
fn decoded_bits(q: u16) -> u16 {
    super::promote_u16_table()
        .get(usize::from(q))
        .map_or_else(|| f16::from_f32(promote_u16(q)).to_bits(), |v| v.to_bits())
}

/// The threads to split `samples` over.
fn threads_for(samples: usize, rows: usize) -> usize {
    if samples < PARALLEL_MIN_SAMPLES || rows < 2 {
        return 1;
    }
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_THREADS)
        .min(rows)
}

/// A channel band's rows, packed, with the sample and decoded hashes.
pub(crate) struct PackedBand {
    pub(crate) rows: Vec<Vec<u8>>,
    pub(crate) sample_sum: u64,
    pub(crate) decoded_sum: u64,
}

/// Packs `rows` rows of `row_samples` samples each from `band` (row `r`
/// is file row `first_ry + r` of the channel, hash slot `slot`).
pub(crate) fn pack_channel_band(
    band: &[u16],
    row_samples: usize,
    rows: usize,
    slot: u8,
    first_ry: u32,
) -> PackedBand {
    pack_channel_band_on(
        band,
        row_samples,
        rows,
        slot,
        first_ry,
        threads_for(band.len(), rows),
    )
}

/// [`pack_channel_band`] on at most `threads` threads.
pub(crate) fn pack_channel_band_on(
    band: &[u16],
    row_samples: usize,
    rows: usize,
    slot: u8,
    first_ry: u32,
    threads: usize,
) -> PackedBand {
    let row_samples = row_samples.max(1);
    let all: Vec<&[u16]> = band.chunks(row_samples).take(rows).collect();
    let per = all.len().div_ceil(threads.max(1)).max(1);
    let pack = |first: usize, part: &[&[u16]]| {
        let mut out = PackedBand {
            rows: Vec::with_capacity(part.len()),
            sample_sum: 0,
            decoded_sum: 0,
        };
        let mut bytes = Vec::with_capacity(row_samples.saturating_mul(2));
        for (r, row) in part.iter().enumerate() {
            let ry = first_ry.wrapping_add((first + r) as u32);
            bytes.clear();
            for (x, &q) in row.iter().enumerate() {
                bytes.extend_from_slice(&q.to_be_bytes());
                let x = x as u32;
                out.sample_sum =
                    out.sample_sum
                        .wrapping_add(sample_hash(slot, x, ry, u64::from(q)));
                out.decoded_sum = out.decoded_sum.wrapping_add(sample_hash(
                    slot,
                    x,
                    ry,
                    u64::from(decoded_bits(q)),
                ));
            }
            out.rows.push(pack_bits(&bytes));
        }
        out
    };
    merge(run_parts(&all, threads, per, &pack))
}

/// `work(first_index, part)` over `items` split into parts of `per`, the
/// parts on scoped threads when `threads > 1`; results in part order. A
/// thread the system will not start runs its part here instead.
fn run_parts<'a, I: Sync, T: Send>(
    items: &'a [I],
    threads: usize,
    per: usize,
    work: &(dyn Fn(usize, &'a [I]) -> T + Sync),
) -> Vec<T> {
    run_parts_with(items, threads, per, work, &thread_builder)
}

/// Where a scoped thread's [`std::thread::Builder`] comes from: the
/// writer and the verify use [`thread_builder`]; a test injects one that
/// fails, as a system that will not start a thread does.
pub(crate) type BuilderSource = dyn Fn() -> std::io::Result<std::thread::Builder> + Sync;

/// The real builder source: a default [`std::thread::Builder`].
#[allow(clippy::unnecessary_wraps)] // the signature a failing source shares
pub(crate) fn thread_builder() -> std::io::Result<std::thread::Builder> {
    Ok(std::thread::Builder::new())
}

/// [`run_parts`] with the thread builder injected, so a test can make
/// every spawn fail and reach the run-it-here fallback.
pub(crate) fn run_parts_with<'a, I: Sync, T: Send>(
    items: &'a [I],
    threads: usize,
    per: usize,
    work: &(dyn Fn(usize, &'a [I]) -> T + Sync),
    builder: &BuilderSource,
) -> Vec<T> {
    let per = per.max(1);
    if threads <= 1 || items.len() <= per {
        return items
            .chunks(per)
            .enumerate()
            .map(|(n, part)| work(n * per, part))
            .collect();
    }
    std::thread::scope(|scope| {
        let jobs: Vec<Result<std::thread::ScopedJoinHandle<'_, T>, T>> = items
            .chunks(per)
            .enumerate()
            .map(|(n, part)| {
                builder()
                    .and_then(|b| b.spawn_scoped(scope, move || work(n * per, part)))
                    .map_err(|_| work(n * per, part))
            })
            .collect();
        jobs.into_iter()
            .map(|job| match job {
                Ok(handle) => handle
                    .join()
                    .unwrap_or_else(|e| std::panic::resume_unwind(e)),
                Err(done) => done,
            })
            .collect()
    })
}

fn merge(parts: Vec<PackedBand>) -> PackedBand {
    let mut out = PackedBand {
        rows: Vec::new(),
        sample_sum: 0,
        decoded_sum: 0,
    };
    for part in parts {
        out.rows.extend(part.rows);
        out.sample_sum = out.sample_sum.wrapping_add(part.sample_sum);
        out.decoded_sum = out.decoded_sum.wrapping_add(part.decoded_sum);
    }
    out
}

/// Merged-image rows (`width * 4` `f16` samples each, `rows` of them,
/// the first file row `first_y`) of plane `plane`, matted onto white for
/// colour, packed: the rows, the sample hash and the counts.
pub(crate) fn pack_merged_rows(
    samples: &[f16],
    width: usize,
    rows: usize,
    plane: u8,
    first_y: u32,
) -> (Vec<Vec<u8>>, u64, SampleCounts) {
    let threads = threads_for(samples.len() / 4, rows);
    pack_merged_rows_on(samples, width, rows, plane, first_y, threads)
}

/// [`pack_merged_rows`] on at most `threads` threads.
pub(crate) fn pack_merged_rows_on(
    samples: &[f16],
    width: usize,
    rows: usize,
    plane: u8,
    first_y: u32,
    threads: usize,
) -> (Vec<Vec<u8>>, u64, SampleCounts) {
    let row = width.saturating_mul(4).max(1);
    let all: Vec<&[f16]> = samples.chunks(row).take(rows).collect();
    let per = all.len().div_ceil(threads.max(1)).max(1);
    let pack = |first: usize, part: &[&[f16]]| {
        let mut packed = Vec::with_capacity(part.len());
        let mut hash = 0u64;
        let mut counts = SampleCounts::default();
        let mut bytes = Vec::with_capacity(width.saturating_mul(2));
        for (r, src) in part.iter().enumerate() {
            let y = first_y.wrapping_add((first + r) as u32);
            bytes.clear();
            for (x, px) in src.chunks_exact(4).enumerate() {
                let sample = |c: usize| px.get(c).map_or(0.0, |v| v.to_f32());
                let alpha = sample(3);
                let value = if plane == 3 {
                    alpha
                } else {
                    sample(usize::from(plane)) * alpha + 1.0 - alpha
                };
                let q = quantize_counted(value, &mut counts);
                hash = hash.wrapping_add(sample_hash(plane, x as u32, y, u64::from(q)));
                bytes.extend_from_slice(&q.to_be_bytes());
            }
            packed.push(pack_bits(&bytes));
        }
        (packed, hash, counts)
    };
    let parts = run_parts(&all, threads, per, &pack);
    let mut out = (Vec::new(), 0u64, SampleCounts::default());
    for (rows, hash, counts) in parts {
        out.0.extend(rows);
        out.1 = out.1.wrapping_add(hash);
        out.2.add(counts);
    }
    out
}

#[cfg(test)]
#[path = "write_pixels_tests.rs"]
pub(crate) mod tests;
