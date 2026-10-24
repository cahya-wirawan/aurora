//! Tests of the band-parallel pixel writer (task 154): every path equals
//! the per-sample, row-at-a-time loop it replaced.

use aurora_color::{promote_u16, quantize_u16};
use half::f16;

use super::*;
use crate::psd::write::{pack_bits, sample_hash};

/// The 0.276.0 writer's per-row work, verbatim in effect: big-endian
/// bytes, both hashes with the direct promotion, `pack_bits`.
fn reference_channel(
    band: &[u16],
    row_samples: usize,
    rows: usize,
    slot: u8,
    first_ry: u32,
) -> (Vec<Vec<u8>>, u64, u64) {
    let (mut s, mut d) = (0u64, 0u64);
    let mut out = Vec::new();
    for (r, row) in band.chunks(row_samples.max(1)).take(rows).enumerate() {
        let ry = first_ry + r as u32;
        let mut bytes = Vec::new();
        for (x, &q) in row.iter().enumerate() {
            bytes.extend_from_slice(&q.to_be_bytes());
            s = s.wrapping_add(sample_hash(slot, x as u32, ry, u64::from(q)));
            let decoded = f16::from_f32(promote_u16(q)).to_bits();
            d = d.wrapping_add(sample_hash(slot, x as u32, ry, u64::from(decoded)));
        }
        out.push(pack_bits(&bytes));
    }
    (out, s, d)
}

fn reference_quantize(value: f32, counts: &mut SampleCounts) -> u16 {
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

/// A deterministic mix of runs (which `PackBits` repeats) and noise.
fn band_of(len: usize, seed: u64) -> Vec<u16> {
    let mut z = seed;
    (0..len)
        .map(|i| {
            z = z
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            if (i / 37) % 3 == 0 {
                0x1234
            } else {
                (z >> 40) as u16
            }
        })
        .collect()
}

#[test]
fn the_f16_table_quantises_every_bit_pattern_like_the_direct_path() {
    for bits in 0..=u16::MAX {
        let v = f16::from_bits(bits);
        let (mut a, mut b) = (SampleCounts::default(), SampleCounts::default());
        let got = quantize_f16(v, &mut a);
        let want = reference_quantize(v.to_f32(), &mut b);
        assert_eq!((got, a), (want, b), "f16 bits {bits:#06x}");
    }
}

#[test]
fn quantize_counted_is_the_writer_quantize() {
    for v in [
        f32::NAN,
        -0.0,
        0.0,
        1.0,
        1.000_001,
        -1e-9,
        0.5,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        let (mut a, mut b) = (SampleCounts::default(), SampleCounts::default());
        assert_eq!(quantize_counted(v, &mut a), reference_quantize(v, &mut b));
        assert_eq!(a, b, "{v}");
    }
}

#[test]
fn a_channel_band_packs_identically_on_every_thread_count() {
    for (row_samples, rows) in [(1, 1), (7, 3), (300, 256), (1031, 17), (5, 0)] {
        let band = band_of(row_samples * rows, (row_samples * 31 + rows) as u64);
        let want = reference_channel(&band, row_samples, rows, 2, 4_100);
        for threads in 1..=9 {
            let got = pack_channel_band_on(&band, row_samples, rows, 2, 4_100, threads);
            assert_eq!(
                (got.rows, got.sample_sum, got.decoded_sum),
                want,
                "{row_samples} x {rows} on {threads} threads"
            );
        }
        let got = pack_channel_band(&band, row_samples, rows, 2, 4_100);
        assert_eq!((got.rows, got.sample_sum, got.decoded_sum), want);
    }
}

#[test]
fn merged_rows_pack_identically_on_every_thread_count() {
    let width = 129;
    let rows = 11;
    let mut z = 7u64;
    let mut samples: Vec<f16> = (0..width * rows * 4)
        .map(|_| {
            z = z
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            f16::from_bits((z >> 48) as u16)
        })
        .collect();
    // Some ordinary values, out-of-range ones and a NaN among the noise.
    for (i, v) in [0.25f32, 1.5, -0.5, f32::NAN, 1.0, 0.0].iter().enumerate() {
        if let Some(s) = samples.get_mut(i * 13) {
            *s = f16::from_f32(*v);
        }
    }
    for plane in 0..4u8 {
        let mut counts = SampleCounts::default();
        let mut hash = 0u64;
        let mut want = Vec::new();
        for (r, src) in samples.chunks(width * 4).enumerate() {
            let mut bytes = Vec::new();
            for (x, px) in src.chunks_exact(4).enumerate() {
                let sample = |c: usize| px.get(c).map_or(0.0, |v| v.to_f32());
                let alpha = sample(3);
                let value = if plane == 3 {
                    alpha
                } else {
                    sample(usize::from(plane)) * alpha + 1.0 - alpha
                };
                let q = reference_quantize(value, &mut counts);
                hash = hash.wrapping_add(sample_hash(plane, x as u32, 90 + r as u32, u64::from(q)));
                bytes.extend_from_slice(&q.to_be_bytes());
            }
            want.push(pack_bits(&bytes));
        }
        assert!(counts.clipped > 0 && counts.nan > 0, "the fixture clips");
        for threads in 1..=9 {
            let got = pack_merged_rows_on(&samples, width, rows, plane, 90, threads);
            assert_eq!(
                got,
                (want.clone(), hash, counts),
                "plane {plane}, {threads} threads"
            );
        }
    }
}
