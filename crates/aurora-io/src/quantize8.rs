//! The 8-bit export boundary for a whole image (invariant §7.3.1b): every
//! sample quantised with [`aurora_color::dither_quantize`]'s ordered dither,
//! shared by the PNG, JPEG and TIFF encoders.
//!
//! Ordered dithering keeps no state between samples (a sample's byte
//! depends only on its value and its pixel's position), so rows are
//! quantised in bands on several threads and the bytes are the same on any
//! thread count; each sample uses the position's threshold from
//! [`aurora_color::dither_thresholds`] and
//! [`aurora_color::dither_quantize_with`], the same arithmetic as
//! [`aurora_color::dither_quantize`] (pinned exhaustively in
//! `aurora-color`). Before this the encoders rebuilt the Bayer value for
//! every sample on one thread, which was most of a large PNG's encode time.

use aurora_color::{DITHER_SIZE, dither_quantize_with, dither_thresholds};
use half::f16;

/// The fewest samples worth a second thread.
const PARALLEL_MIN_SAMPLES: usize = 1 << 20;

/// `samples` (RGBA, `width` pixels a row) dither-quantised to 8-bit RGBA,
/// on up to eight threads.
pub(crate) fn dither_rgba8(samples: &[f16], width: u32) -> Vec<u8> {
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    dither_rgba8_on(samples, width, threads)
}

/// [`dither_rgba8`] on at most `threads` threads.
pub(crate) fn dither_rgba8_on(samples: &[f16], width: u32, threads: usize) -> Vec<u8> {
    let row = (width.max(1) as usize).saturating_mul(4);
    let mut out = vec![0_u8; samples.len()];
    let rows = samples.len().div_ceil(row);
    if threads <= 1 || samples.len() < PARALLEL_MIN_SAMPLES || rows < 2 {
        quantize_rows(samples, &mut out, row, 0);
        return out;
    }
    let band_rows = rows.div_ceil(threads.min(rows));
    let band = band_rows.saturating_mul(row);
    std::thread::scope(|scope| {
        for (n, (src, dst)) in samples.chunks(band).zip(out.chunks_mut(band)).enumerate() {
            scope.spawn(move || quantize_rows(src, dst, row, n.saturating_mul(band_rows)));
        }
    });
    out
}

/// Quantises whole rows `src` (`row` samples each, the last may be short)
/// into `dst`; the first is document row `first_row`.
fn quantize_rows(src: &[f16], dst: &mut [u8], row: usize, first_row: usize) {
    let table = dither_thresholds();
    let side = DITHER_SIZE as usize;
    for (r, (src_row, dst_row)) in src.chunks(row).zip(dst.chunks_mut(row)).enumerate() {
        let phase = (first_row + r) % side * side;
        let thresholds = table.get(phase..phase + side).unwrap_or(&[]);
        for (x, (px, out)) in src_row.chunks(4).zip(dst_row.chunks_mut(4)).enumerate() {
            let threshold = thresholds.get(x % side).copied().unwrap_or(0.0);
            for (sample, byte) in px.iter().zip(out.iter_mut()) {
                *byte = dither_quantize_with(sample.to_f32(), threshold);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dither_rgba8_on, f16};

    /// The quantiser as every encoder wrote it before (0.276.0).
    fn reference(samples: &[f16], width: u32) -> Vec<u8> {
        let mut rgba8 = Vec::with_capacity(samples.len());
        for (index, &sample) in samples.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let pixel_index = (index / 4) as u32;
            let x = pixel_index % width.max(1);
            let y = pixel_index / width.max(1);
            rgba8.push(aurora_color::dither_quantize(sample.to_f32(), x, y));
        }
        rgba8
    }

    fn noise(len: usize, seed: &mut u64) -> Vec<f16> {
        (0..len)
            .map(|_| {
                *seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                // Every bit pattern: NaN, infinities, negatives, over 1.
                #[allow(clippy::cast_possible_truncation)]
                let bits = (*seed >> 40) as u16;
                if *seed & 1 == 0 {
                    f16::from_bits(bits)
                } else {
                    f16::from_f32(f32::from(bits) / 65535.0)
                }
            })
            .collect()
    }

    /// Same bytes as the old loop, on 1 to 8 threads, for widths that are
    /// and are not multiples of the dither matrix, images over and under
    /// the threading threshold, and an image whose length is not whole rows.
    #[test]
    fn the_banded_quantiser_gives_the_old_bytes_on_any_thread_count() {
        let mut seed = 42;
        for (width, height) in [
            (1_u32, 5_usize),
            (7, 3),
            (613, 517),
            (1024, 300),
            (8, 40_000),
        ] {
            let len = width as usize * height * 4;
            let samples = noise(len, &mut seed);
            let want = reference(&samples, width);
            for threads in [1, 2, 3, 8] {
                assert_eq!(
                    dither_rgba8_on(&samples, width, threads),
                    want,
                    "{width} x {height}, {threads} threads"
                );
            }
        }
        // A partial last row (not whole rows) and an empty image.
        let samples = noise(613 * 4 * 517 + 9, &mut seed);
        assert_eq!(dither_rgba8_on(&samples, 613, 8), reference(&samples, 613));
        assert!(dither_rgba8_on(&[], 10, 8).is_empty());
        let few = samples.get(..12).unwrap_or(&[]);
        assert_eq!(dither_rgba8_on(few, 0, 8), reference(few, 0));
    }
}
