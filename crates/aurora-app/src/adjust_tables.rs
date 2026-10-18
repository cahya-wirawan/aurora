//! Per-channel `f16` tables for the channel-separable adjustments (60 FPS
//! round 9).
//!
//! [`crate::apply_adjustment_layer`] maps every texel of the straightened
//! backdrop through the adjustment, `f16` in and `f16` out:
//! `f16::from_f32(map_channel(i, value.to_f32()))` per colour channel. For
//! Curves and Levels that map is a pure function of one channel's `f16`
//! bits (`CurvesLut::apply` and `LevelsLut::apply` are exactly
//! `[map_channel(0, r), map_channel(1, g), map_channel(2, b)]`), so it has
//! at most 65,536 distinct inputs. This module tabulates it once per
//! parameter set: entry `bits` holds the very `f16` the per-texel code
//! computes for `f16::from_bits(bits)`, every bit pattern included (NaN,
//! infinities, negatives), so a table lookup is that computation, bit for
//! bit.
//!
//! Measured (release, i3-10100, one 256 px tile of noise under a
//! three-point Curves layer, `r9_probe_adjustment_steps`): the per-texel
//! map with its conversions took 2.6 ms a tile, about two thirds of a
//! masked Curves layer's whole CPU cost. A table costs 65,536 maps (about
//! a third of one tile's 196,608), is shared between the channels whose
//! own parameters are equal (only the composite curve or master record
//! set: one table for all three), and is cached process-wide for the
//! last [`CACHE_LEN`] parameter sets, so the frame pool's helpers and the
//! background compositor's threads share it.

use std::sync::{Arc, Mutex, OnceLock};

/// One colour channel's table: 65,536 `f16` bit patterns.
pub(crate) type Table = Arc<[u16]>;

/// How many parameter sets keep their tables (a few layers, or a slider
/// drag's recent values).
const CACHE_LEN: usize = 8;

type Cache = Mutex<Vec<(aurora_doc::Adjustment, [Table; 3])>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Tabulates `map` over every `f16` bit pattern.
fn tabulate(map: impl Fn(f32) -> f32) -> Table {
    (0..=u16::MAX)
        .map(|bits| half::f16::from_f32(map(half::f16::from_bits(bits).to_f32())).to_bits())
        .collect()
}

/// The three channels' tables for `adjustment`, or `None` when it is not
/// channel-separable (only Curves and Levels are tabulated). `curves` and
/// `levels` are the built lookups [`crate::AdjustmentLut`] holds for it.
pub(crate) fn channel_tables(
    adjustment: &aurora_doc::Adjustment,
    curves: Option<&aurora_filters::CurvesLut>,
    levels: Option<&aurora_filters::LevelsLut>,
) -> Option<[Table; 3]> {
    // Whether channels `i` and `j` map identically (their own parameters
    // are equal; the composite curve or master record is shared).
    let same: [bool; 3] = match adjustment {
        aurora_doc::Adjustment::Curves(p) if curves.is_some() => {
            [p.red == p.green, p.red == p.blue, p.green == p.blue]
        }
        aurora_doc::Adjustment::Levels(p) if levels.is_some() => {
            [p.red == p.green, p.red == p.blue, p.green == p.blue]
        }
        _ => return None,
    };
    if let Ok(cached) = cache().lock()
        && let Some((_, tables)) = cached.iter().find(|(key, _)| key == adjustment)
    {
        return Some(tables.clone());
    }
    let map = |index: usize| -> Table {
        match (curves, levels) {
            (Some(lut), _) => tabulate(|value| lut.map_channel(index, value)),
            (None, Some(lut)) => tabulate(|value| lut.map_channel(index, value)),
            (None, None) => tabulate(|value| value),
        }
    };
    let red = map(0);
    let green = if same[0] { Arc::clone(&red) } else { map(1) };
    let blue = if same[1] {
        Arc::clone(&red)
    } else if same[2] {
        Arc::clone(&green)
    } else {
        map(2)
    };
    let tables = [red, green, blue];
    if let Ok(mut cached) = cache().lock()
        && !cached.iter().any(|(key, _)| key == adjustment)
    {
        if cached.len() >= CACHE_LEN {
            cached.remove(0);
        }
        cached.push((adjustment.clone(), tables.clone()));
    }
    Some(tables)
}

/// `value` through `table`, the computation the table holds.
#[inline]
pub(crate) fn lookup(table: &[u16], value: half::f16) -> half::f16 {
    match table.get(usize::from(value.to_bits())) {
        Some(&bits) => half::f16::from_bits(bits),
        // Unreachable: every table has 65,536 entries.
        None => value,
    }
}

#[cfg(test)]
mod tests {
    use super::{channel_tables, lookup};

    fn ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => unreachable!("{err:?}"),
        }
    }

    fn curve(points: &[(f32, f32)]) -> aurora_core::ToneCurve {
        let points: Vec<_> = points
            .iter()
            .map(|&(x, y)| aurora_core::CurvePoint::new(x, y))
            .collect();
        ok(aurora_core::ToneCurve::new(&points))
    }

    /// Every channel's table holds, for every `f16` bit pattern, exactly
    /// what `apply_adjustment_layer`'s per-texel path computes:
    /// `f16::from_f32(lut.apply(..)[i])`.
    #[test]
    fn a_table_is_the_per_texel_map_for_every_bit_pattern() {
        let s = curve(&[(0.0, 0.05), (0.3, 0.2), (0.7, 0.85), (1.0, 0.95)]);
        let lift = curve(&[(0.2, 0.0), (0.8, 1.0)]);
        let level = |b, w, g| ok(aurora_core::LevelsChannel::new(b, w, g, 0.02, 0.97));
        let cases = [
            aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
                composite: s.clone(),
                ..aurora_core::CurvesParams::default()
            }),
            aurora_doc::Adjustment::Curves(aurora_core::CurvesParams {
                composite: s.clone(),
                red: Some(lift.clone()),
                green: None,
                blue: Some(s),
            }),
            aurora_doc::Adjustment::Levels(aurora_core::LevelsParams {
                master: level(0.05, 0.9, 1.3),
                red: level(0.1, 0.95, 0.7),
                green: level(0.1, 0.95, 0.7),
                blue: level(0.0, 0.8, 2.0),
            }),
        ];
        for adjustment in &cases {
            let (curves, levels) = match adjustment {
                aurora_doc::Adjustment::Curves(p) => {
                    (Some(aurora_filters::CurvesLut::new(p)), None)
                }
                aurora_doc::Adjustment::Levels(p) => {
                    (None, Some(aurora_filters::LevelsLut::new(p)))
                }
                _ => unreachable!("Curves or Levels"),
            };
            let Some(tables) = channel_tables(adjustment, curves.as_ref(), levels.as_ref()) else {
                unreachable!("tabulated");
            };
            for bits in 0..=u16::MAX {
                let v = half::f16::from_bits(bits);
                let x = v.to_f32();
                let direct = match (&curves, &levels) {
                    (Some(lut), _) => lut.apply([x, x, x]),
                    (None, Some(lut)) => lut.apply([x, x, x]),
                    (None, None) => unreachable!("one of them"),
                };
                for (channel, (table, want)) in tables.iter().zip(direct).enumerate() {
                    assert_eq!(
                        lookup(table, v).to_bits(),
                        half::f16::from_f32(want).to_bits(),
                        "{adjustment:?} channel {channel} bits {bits:#06x}"
                    );
                }
            }
            // A second call answers from the cache, with the same tables.
            let Some(again) = channel_tables(adjustment, curves.as_ref(), levels.as_ref()) else {
                unreachable!("tabulated");
            };
            for (a, b) in again.iter().zip(&tables) {
                assert!(std::sync::Arc::ptr_eq(a, b));
            }
        }
    }

    /// Channels whose own parameters are equal share one table; others
    /// do not.
    #[test]
    fn equal_channels_share_a_table() {
        let s = curve(&[(0.0, 0.1), (1.0, 0.9)]);
        let params = aurora_core::CurvesParams {
            composite: s.clone(),
            red: Some(s),
            green: None,
            blue: None,
        };
        let lut = aurora_filters::CurvesLut::new(&params);
        let adjustment = aurora_doc::Adjustment::Curves(params);
        let Some([r, g, b]) = channel_tables(&adjustment, Some(&lut), None) else {
            unreachable!("tabulated");
        };
        assert!(!std::sync::Arc::ptr_eq(&r, &g));
        assert!(std::sync::Arc::ptr_eq(&g, &b));
        assert_eq!(r.len(), 65_536);
    }

    /// Only Curves and Levels are tabulated.
    #[test]
    fn other_adjustments_are_not_tabulated() {
        assert!(channel_tables(&aurora_doc::Adjustment::Invert, None, None).is_none());
    }

    /// The write-back's copy at alpha 1 equals the arithmetic it skips,
    /// for every non-NaN `f16`.
    #[test]
    fn the_opaque_write_back_copy_is_the_product() {
        for bits in 0..=u16::MAX {
            let v = half::f16::from_bits(bits);
            if v.is_nan() {
                continue;
            }
            let product = half::f16::from_f32(v.to_f32() * half::f16::ONE.to_f32());
            assert_eq!(product.to_bits(), bits, "{bits:#06x}");
        }
    }
}
