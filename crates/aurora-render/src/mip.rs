//! Progressive rendering, first piece: box-filtered tile downsampling —
//! "render a lower-resolution mip while panning fast, refining when
//! motion stops" (`spike/FINDINGS.md` finding #3, named as the mitigation
//! for the ~18 MB/screenful upload-bandwidth ceiling a fast pan hits).
//! PLAN.md M1.3.
//!
//! What this module does not yet do: pick a [`MipLevel`] based on
//! interaction state, upload a downsampled tile into
//! `aurora_gpu::TileResidency`'s atlas, or sample it back at reduced
//! resolution in a shader — all still-open follow-on work. This is the
//! resolution-reduction primitive those steps will call.

use aurora_tile::{CHANNELS, TILE};
use half::f16;

/// A reduced tile resolution for progressive rendering. A closed set of
/// power-of-two divisors of [`TILE`] rather than an arbitrary factor, so
/// every level divides the tile evenly by construction — no remainder or
/// rounding handling needed anywhere downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MipLevel {
    /// Full resolution: `TILE` × `TILE`, no downsampling.
    Full,
    /// `TILE`/2 × `TILE`/2 — 4× fewer bytes than [`Self::Full`].
    Half,
    /// `TILE`/4 × `TILE`/4 — 16× fewer bytes.
    Quarter,
    /// `TILE`/8 × `TILE`/8 — 64× fewer bytes, the coarsest preview this
    /// module offers; below this a tile is a handful of pixels and stops
    /// being a useful preview of anything.
    Eighth,
}

impl MipLevel {
    /// How many source texels, per axis, average into one output texel.
    #[must_use]
    pub const fn factor(self) -> u32 {
        match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
            Self::Eighth => 8,
        }
    }

    /// The side length, in texels, of a tile downsampled to this level.
    #[must_use]
    pub const fn size(self) -> u32 {
        TILE / self.factor()
    }
}

/// Box-filters a full-resolution tile's texels (`TILE` × `TILE` RGBA,
/// [`aurora_tile::SAMPLES`] `f16` samples — the shape every
/// `aurora_tile::Tile::texels()` slice has) down to `level`.
///
/// [`MipLevel::Full`] returns a copy of `texels` unchanged rather than a
/// special case — callers that always route through this function don't
/// need their own full-resolution branch.
///
/// Each output texel is the exact mean of the `factor` × `factor` block
/// of source texels it covers, summed in `f32` (matching the render
/// graph's own compute precision, ADR 0003 — not accumulated in `f16`,
/// which would lose precision across the sum) and rounded back to `f16`
/// once per output channel.
#[must_use]
pub fn downsample(texels: &[f16], level: MipLevel) -> Vec<f16> {
    let factor = level.factor() as usize;
    if factor == 1 {
        return texels.to_vec();
    }

    let tile = TILE as usize;
    let out_dim = level.size() as usize;
    let rows: Vec<&[f16]> = texels.chunks_exact(tile * CHANNELS).collect();

    let mut out = Vec::with_capacity(out_dim * out_dim * CHANNELS);
    for oy in 0..out_dim {
        for ox in 0..out_dim {
            let mut sums = [0f32; CHANNELS];
            for dy in 0..factor {
                let Some(row) = rows.get(oy * factor + dy) else {
                    unreachable!("oy*factor+dy < TILE: oy < out_dim = TILE/factor by construction");
                };
                let start = ox * factor * CHANNELS;
                let Some(block) = row.get(start..start + factor * CHANNELS) else {
                    unreachable!(
                        "ox*factor+factor <= TILE: ox < out_dim = TILE/factor by construction"
                    );
                };
                for texel in block.chunks_exact(CHANNELS) {
                    for (sum, sample) in sums.iter_mut().zip(texel) {
                        *sum += sample.to_f32();
                    }
                }
            }
            let count = (factor * factor) as f32;
            for sum in sums {
                out.push(f16::from_f32(sum / count));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{MipLevel, downsample};
    use aurora_tile::{CHANNELS, SAMPLES, TILE};
    use half::f16;

    #[test]
    fn mip_level_factor_and_size() {
        assert_eq!(MipLevel::Full.factor(), 1);
        assert_eq!(MipLevel::Full.size(), TILE);
        assert_eq!(MipLevel::Half.factor(), 2);
        assert_eq!(MipLevel::Half.size(), TILE / 2);
        assert_eq!(MipLevel::Quarter.factor(), 4);
        assert_eq!(MipLevel::Quarter.size(), TILE / 4);
        assert_eq!(MipLevel::Eighth.factor(), 8);
        assert_eq!(MipLevel::Eighth.size(), TILE / 8);
    }

    fn solid_texels(rgba: [f32; 4]) -> Vec<f16> {
        let mut out = Vec::with_capacity(SAMPLES);
        for _ in 0..(TILE * TILE) {
            for channel in rgba {
                out.push(f16::from_f32(channel));
            }
        }
        out
    }

    #[test]
    fn downsample_full_is_an_identity_copy() {
        let texels = solid_texels([0.25, 0.5, 0.75, 1.0]);
        let out = downsample(&texels, MipLevel::Full);
        assert_eq!(out.len(), SAMPLES);
        assert_eq!(
            out.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
            texels.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn downsample_output_size_matches_the_mip_level() {
        let texels = solid_texels([1.0, 1.0, 1.0, 1.0]);
        for level in [
            MipLevel::Full,
            MipLevel::Half,
            MipLevel::Quarter,
            MipLevel::Eighth,
        ] {
            let out = downsample(&texels, level);
            let expected = (level.size() * level.size()) as usize * CHANNELS;
            assert_eq!(out.len(), expected, "{level:?}");
        }
    }

    #[test]
    fn downsample_of_a_uniform_tile_stays_uniform() {
        // The mean of a block that's all the same colour is that colour,
        // exactly -- 0.25/0.5/0.75/1.0 all have exact f16 representations,
        // so this is a real bit-exact check, not a rounding-tolerant one.
        let rgba = [0.25f32, 0.5, 0.75, 1.0];
        let texels = solid_texels(rgba);
        for level in [MipLevel::Half, MipLevel::Quarter, MipLevel::Eighth] {
            let out = downsample(&texels, level);
            for texel in out.chunks_exact(CHANNELS) {
                for (sample, expected) in texel.iter().zip(rgba) {
                    #[allow(clippy::float_cmp)]
                    {
                        assert_eq!(sample.to_f32(), expected, "{level:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn downsample_averages_a_checkerboard_to_the_exact_midpoint() {
        // Alternate opaque black/white by texel row: at MipLevel::Half
        // (factor 2), every 2x2 source block covers one black row and one
        // white row -- exactly two of each colour, so the mean is exactly
        // 0.5 in every channel, not merely close to it.
        let mut texels = Vec::with_capacity(SAMPLES);
        for row in 0..TILE {
            let value = if row % 2 == 0 { 0.0 } else { 1.0 };
            for _ in 0..TILE {
                for _ in 0..CHANNELS {
                    texels.push(f16::from_f32(value));
                }
            }
        }
        let out = downsample(&texels, MipLevel::Half);
        for sample in &out {
            #[allow(clippy::float_cmp)]
            {
                assert_eq!(sample.to_f32(), 0.5);
            }
        }
    }

    #[test]
    fn downsample_reads_the_correct_source_block_not_a_shifted_one() {
        // Colour every texel by its own row index (as a fraction of
        // TILE), so a wrong row range (off-by-one, or averaging the wrong
        // rows entirely) produces a value distinguishable from the
        // correct one, not coincidentally right.
        let mut texels = Vec::with_capacity(SAMPLES);
        for row in 0..TILE {
            let value = row as f32 / TILE as f32;
            for _ in 0..TILE {
                for _ in 0..CHANNELS {
                    texels.push(f16::from_f32(value));
                }
            }
        }
        let out = downsample(&texels, MipLevel::Quarter);
        // Output row 1 (factor 4) must average source rows 4..8:
        // mean = (4+5+6+7)/4 / TILE = 5.5 / TILE.
        let out_dim = MipLevel::Quarter.size() as usize;
        let out_row = 1;
        let row_start = out_row * out_dim * CHANNELS;
        let Some(row) = out.get(row_start..row_start + CHANNELS) else {
            unreachable!("out has out_dim rows of out_dim texels each");
        };
        let expected = 5.5 / TILE as f32;
        for sample in row {
            let diff = (sample.to_f32() - expected).abs();
            assert!(diff < 1e-3, "expected ~{expected}, got {}", sample.to_f32());
        }
    }
}
