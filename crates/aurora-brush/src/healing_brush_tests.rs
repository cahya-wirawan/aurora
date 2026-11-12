//! The Healing Brush's engine (0.314.0) and the pin that the Spot Healing
//! refactor behind it changed no bit.

#![allow(
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names,
    clippy::needless_range_loop,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines
)]

use aurora_tile::{TILE, TileId};

use crate::spot_heal_tests::{Constant, RightOf, close, dump, getf, paint, store, surface, texel};
use crate::{
    HealMask, HealRefusal, PixelHistory, StrokeSnapshot, healing_brush_covered, spot_heal_covered,
};

use aurora_tile::TileStore;

fn heal(
    s: &mut TileStore,
    mask: &HealMask,
    preserve: bool,
    snap: Option<&mut StrokeSnapshot>,
    cov: Option<&dyn crate::Coverage>,
) -> Result<crate::DabOutcome, HealRefusal> {
    spot_heal_covered(s, surface(), mask, 8.0, preserve, snap, cov)
}

fn brush(
    s: &mut TileStore,
    mask: &HealMask,
    offset: (i64, i64),
    preserve: bool,
    snap: Option<&mut StrokeSnapshot>,
    cov: Option<&dyn crate::Coverage>,
) -> Result<crate::DabOutcome, HealRefusal> {
    healing_brush_covered(s, surface(), mask, 8.0, offset, preserve, snap, cov)
}

fn disc(cx: f32, cy: f32, r: f32) -> HealMask {
    let mut m = HealMask::new();
    m.add_dab((cx, cy), r);
    m
}

/// A deterministic textured image, straight RGB from a small LCG, opaque.
fn noise(x: u32, y: u32) -> [f32; 4] {
    let mut s = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ 0x9E37_79B9;
    let mut next = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((s >> 8) & 0xFFFF) as f32 / 65_535.0
    };
    let base = 0.3 + 0.002 * x as f32;
    texel(
        base + 0.2 * next(),
        0.5 * next() + 0.1,
        0.4 + 0.3 * next(),
        1.0,
    )
}

fn fnv(values: &[[u16; 4]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for v in values.iter().flatten() {
        h ^= u64::from(*v);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Hash of the Spot Healing output on a textured image, taken from the code
/// before `heal_window` learned a fixed source.
const SPOT_HEAL_PIN: u64 = 6_766_062_256_207_077_254;

#[test]
fn spot_healing_output_is_bit_for_bit_what_it_was_before_the_fixed_source_refactor() {
    let (_dir, mut s) = store();
    paint(&mut s, 96, 96, noise);
    let mut mask = crate::HealMask::new();
    mask.add_dab((40.5, 44.5), 6.0);
    mask.add_dab((46.5, 47.5), 5.0);
    assert!(heal(&mut s, &mask, false, None, None).is_ok());
    let got = fnv(&dump(&mut s, 96, 96));
    assert_eq!(got, SPOT_HEAL_PIN, "{got}");
}

// ------------------------------------------------------- the Healing Brush

/// Destination: x < 100, a linear ramp in x. Source: x >= 100, a flat 0.7.
fn ramp_and_flat(x: u32, _y: u32) -> [f32; 4] {
    if x < 100 {
        texel(0.2 + 0.004 * x as f32, 0.3, 0.4, 1.0)
    } else {
        texel(0.7, 0.3, 0.4, 1.0)
    }
}

#[test]
fn a_flat_source_healed_into_a_gradient_matches_the_boundary_with_no_seam() {
    let (_dir, mut s) = store();
    paint(&mut s, 160, 64, |x, y| {
        let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 32.0).powi(2);
        if d2 < 49.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            ramp_and_flat(x, y)
        }
    });
    let mask = disc(40.5, 32.5, 8.0);
    let out = brush(&mut s, &mask, (80, 0), false, None, None);
    assert!(out.is_ok_and(|o| !o.painted().is_empty()));
    // The flat source carries no texture, so the result is the ramp.
    let mut worst = 0.0_f32;
    let mut seam = 0.0_f32;
    for y in 20..45 {
        for x in 25..56 {
            let got = getf(&mut s, x, y);
            let want = ramp_and_flat(x, y);
            worst = worst.max((got[0] - want[0]).abs());
            let right = getf(&mut s, x + 1, y);
            seam = seam.max((right[0] - got[0] - 0.004).abs());
        }
    }
    assert!(worst < 0.01, "worst error against the ramp {worst}");
    assert!(seam < 0.01, "largest neighbour step error {seam}");
}

/// A texture with a distinct value in every column and row.
fn texture(x: u32, y: u32) -> f32 {
    0.15 * (((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5)
}

#[test]
fn the_texture_of_the_source_is_carried_over_a_smooth_correction() {
    let (_dir, mut s) = store();
    // Destination flat 0.3 (with a red blemish), source flat 0.6 plus texture.
    paint(&mut s, 160, 64, |x, y| {
        if x < 100 {
            let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 32.0).powi(2);
            if d2 < 49.0 {
                texel(1.0, 0.0, 0.0, 1.0)
            } else {
                texel(0.3, 0.3, 0.3, 1.0)
            }
        } else {
            let t = texture(x, y);
            texel(0.6 + t, 0.6 + t, 0.6 + t, 1.0)
        }
    });
    let mask = disc(40.5, 32.5, 8.0);
    assert!(brush(&mut s, &mask, (80, 0), false, None, None).is_ok());
    // result - source is the smooth correction (about -0.3, varying only
    // with the ring's own noise), whatever the texture: its spread is far
    // below the texture's.
    let (mut diff, mut tex) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
    for y in 28..37 {
        for x in 36..45 {
            if !mask.contains(i64::from(x), i64::from(y)) {
                continue;
            }
            let got = getf(&mut s, x, y)[0];
            let src = getf(&mut s, x + 80, y)[0];
            diff = (diff.0.min(got - src), diff.1.max(got - src));
            tex = (tex.0.min(src), tex.1.max(src));
        }
    }
    assert!(tex.1 - tex.0 > 0.1, "the source has texture: {tex:?}");
    assert!(
        diff.1 - diff.0 < 0.3 * (tex.1 - tex.0),
        "correction spread {diff:?} against texture {tex:?}"
    );
    assert!((diff.0 + 0.3).abs() < 0.06 && (diff.1 + 0.3).abs() < 0.06);
}

#[test]
fn the_offset_is_honoured_exactly() {
    let build = |s: &mut TileStore| {
        paint(s, 200, 64, |x, y| {
            if x < 100 {
                let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 32.0).powi(2);
                if d2 < 49.0 {
                    texel(1.0, 0.0, 0.0, 1.0)
                } else {
                    texel(0.3, 0.3, 0.3, 1.0)
                }
            } else {
                let t = texture(x, y);
                texel(0.6 + t, 0.6 + t, 0.6 + t, 1.0)
            }
        });
    };
    let mask = disc(40.5, 32.5, 8.0);
    let (_d1, mut a) = store();
    build(&mut a);
    assert!(brush(&mut a, &mask, (80, 0), false, None, None).is_ok());
    let (_d2, mut b) = store();
    build(&mut b);
    assert!(brush(&mut b, &mask, (83, 2), false, None, None).is_ok());
    let mut differing = 0;
    for y in 28..37 {
        for x in 36..45 {
            if !mask.contains(i64::from(x), i64::from(y)) {
                continue;
            }
            // Each result is its own source plus the same smooth -0.3.
            let ra = getf(&mut a, x, y)[0] - getf(&mut a, x + 80, y)[0];
            let rb = getf(&mut b, x, y)[0] - getf(&mut b, x + 83, y + 2)[0];
            assert!((ra + 0.3).abs() < 0.06 && (rb + 0.3).abs() < 0.06);
            if (getf(&mut a, x, y)[0] - getf(&mut b, x, y)[0]).abs() > 0.01 {
                differing += 1;
            }
        }
    }
    assert!(
        differing > 20,
        "a different offset must give a different texture"
    );
}

#[test]
fn the_stroke_never_reads_its_own_output() {
    // The source overlaps the healed area: offset (6, 0) with a radius-8 disc.
    let image = |x: u32, y: u32| {
        let d2 = (x as f32 - 60.0).powi(2) + (y as f32 - 60.0).powi(2);
        if d2 < 49.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            let t = texture(x, y);
            texel(0.4 + t, 0.5 + t, 0.6, 1.0)
        }
    };
    let mask = disc(60.5, 60.5, 8.0);
    let (_d1, mut overlapping) = store();
    paint(&mut overlapping, 400, 400, image);
    assert!(brush(&mut overlapping, &mask, (6, 0), false, None, None).is_ok());
    // The same heal with the source copied far away first: a copy of every
    // texel the overlapping source window can read, so the result must match.
    let (_d2, mut copied) = store();
    paint(&mut copied, 400, 400, image);
    for y in 0..120_u32 {
        for x in 0..120_u32 {
            let t = {
                let v = getf(&mut copied, x, y);
                [v[0], v[1], v[2], v[3]]
            };
            crate::spot_heal_tests::put(&mut copied, x + 200, y + 250, t);
        }
    }
    // Offset (6, 0) from p reads p + (6, 0); the copy at (+200, +250) reads
    // p + (206, 250) = copy of p + (6, 0) only if offset is (206 - 0...):
    // copy(q) = image(q - (200, 250)), so offset (206, 250) reads image(p + 6).
    assert!(brush(&mut copied, &mask, (206, 250), false, None, None).is_ok());
    for y in 40..80 {
        for x in 40..80 {
            assert_eq!(
                crate::spot_heal_tests::bits(crate::spot_heal_tests::get(&mut overlapping, x, y)),
                crate::spot_heal_tests::bits(crate::spot_heal_tests::get(&mut copied, x, y)),
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn undo_and_redo_are_exact_through_one_snapshot() {
    let (_dir, mut s) = store();
    paint(&mut s, 300, 80, |x, y| {
        let d2 = (x as f32 - 256.0).powi(2) + (y as f32 - 40.0).powi(2);
        if d2 < 100.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            let t = texture(x, y);
            texel(0.5 + t, 0.5, 0.5, 1.0)
        }
    });
    let before = dump(&mut s, 300, 80);
    let mut snap = StrokeSnapshot::new(surface());
    let mask = disc(256.0, 40.5, 9.0);
    let Ok(out) = brush(&mut s, &mask, (-60, 0), false, Some(&mut snap), None) else {
        unreachable!("heals");
    };
    assert_eq!(out.painted().len(), 2, "both tiles of the boundary");
    let healed = dump(&mut s, 300, 80);
    assert_ne!(healed, before);
    let mut history = PixelHistory::new();
    assert!(history.push(snap));
    assert!(matches!(history.undo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 80), before);
    assert!(matches!(history.redo(&mut s), Ok(true)));
    assert_eq!(dump(&mut s, 300, 80), healed);
}

#[test]
fn selection_coverage_and_the_transparency_lock_behave_as_for_spot_healing() {
    let image = |x: u32, y: u32| {
        let d2 = (x as f32 - 40.0).powi(2) + (y as f32 - 32.0).powi(2);
        if d2 < 49.0 {
            texel(1.0, 0.0, 0.0, 0.5)
        } else if x < 100 {
            texel(0.3, 0.3, 0.3, 0.5)
        } else {
            let t = texture(x, y);
            texel(0.6 + t, 0.6 + t, 0.6 + t, 0.5)
        }
    };
    let mask = disc(40.5, 32.5, 8.0);
    // Locked alpha: every alpha bit stays.
    let (_d, mut s) = store();
    paint(&mut s, 160, 64, image);
    let before = dump(&mut s, 160, 64);
    assert!(brush(&mut s, &mask, (80, 0), true, None, None).is_ok());
    let after = dump(&mut s, 160, 64);
    assert!(before.iter().zip(&after).all(|(b, a)| b[3] == a[3]));
    assert_ne!(before, after);
    // A selection that excludes the left half of the mask leaves it alone.
    let (_d2, mut t) = store();
    paint(&mut t, 160, 64, image);
    let before = dump(&mut t, 160, 64);
    assert!(brush(&mut t, &mask, (80, 0), false, None, Some(&RightOf(41))).is_ok());
    let after = dump(&mut t, 160, 64);
    for y in 0..64_usize {
        for x in 0..41_usize {
            assert_eq!(before[y * 160 + x], after[y * 160 + x], "({x}, {y})");
        }
    }
    assert_ne!(before, after);
    // Selection coverage zero everywhere changes nothing.
    let (_d3, mut u) = store();
    paint(&mut u, 160, 64, image);
    let before = dump(&mut u, 160, 64);
    assert!(brush(&mut u, &mask, (80, 0), false, None, Some(&Constant(0.0))).is_ok());
    assert_eq!(dump(&mut u, 160, 64), before);
}

#[test]
fn a_source_mostly_outside_the_layer_is_refused_and_writes_nothing() {
    let (_dir, mut s) = store();
    paint(&mut s, 160, 64, |x, y| {
        let t = texture(x, y);
        texel(0.5 + t, 0.5, 0.5, 1.0)
    });
    let before = dump(&mut s, 160, 64);
    let mask = disc(40.5, 32.5, 8.0);
    // Left of the layer, below it, and far away.
    for offset in [
        (-100, 0),
        (0, 500),
        (1 << 30, -(1 << 30)),
        (-300, -300),
        (-45, 0),
    ] {
        let out = brush(&mut s, &mask, offset, false, None, None);
        assert_eq!(out.err(), Some(HealRefusal::SourceOutside), "{offset:?}");
        assert_eq!(dump(&mut s, 160, 64), before);
    }
    // A source just over half inside heals: offset (-40, 0) puts the
    // mask's left part (x < 40) outside, the rest inside.
    let out = brush(&mut s, &mask, (-37, 0), false, None, None);
    assert!(out.is_ok(), "{out:?}");
}

#[test]
fn an_overflowed_or_empty_mask_is_handled_like_spot_healing() {
    let (_dir, mut s) = store();
    paint(&mut s, 64, 64, |_, _| texel(0.5, 0.5, 0.5, 1.0));
    let empty = HealMask::new();
    assert!(brush(&mut s, &empty, (3, 3), false, None, None).is_ok_and(|o| o.painted().is_empty()));
    let mut big = HealMask::new();
    big.add_dab((2000.0, 2000.0), 1000.0);
    big.add_dab((6000.0, 2000.0), 1000.0);
    assert_eq!(
        brush(&mut s, &big, (3, 3), false, None, None).err(),
        Some(HealRefusal::TooLarge)
    );
}

#[test]
fn it_is_deterministic_and_makes_no_phantom_tile() {
    let run = || {
        let (_dir, mut s) = store();
        paint(&mut s, 100, 100, |x, y| {
            let d2 = (x as f32 - 30.0).powi(2) + (y as f32 - 30.0).powi(2);
            if d2 < 36.0 {
                texel(1.0, 0.0, 0.0, 1.0)
            } else {
                let t = texture(x, y);
                texel(0.5 + t, 0.5, 0.5, 1.0)
            }
        });
        let out = brush(&mut s, &disc(30.5, 30.5, 6.0), (40, 40), false, None, None);
        assert!(out.is_ok());
        for id in [
            TileId { x: 1, y: 0 },
            TileId { x: 0, y: 1 },
            TileId { x: 1, y: 1 },
        ] {
            assert!(
                !s.contains_tile(surface(), id) || id.x * TILE < 100,
                "{id:?}"
            );
        }
        dump(&mut s, 100, 100)
    };
    assert_eq!(run(), run());
}

#[test]
fn a_flat_destination_and_flat_source_heal_to_the_flat_colour() {
    let (_dir, mut s) = store();
    let flat = texel(0.5, 0.25, 0.75, 1.0);
    paint(&mut s, 120, 60, |x, y| {
        let d2 = (x as f32 - 30.0).powi(2) + (y as f32 - 30.0).powi(2);
        if d2 < 25.0 {
            texel(1.0, 0.0, 0.0, 1.0)
        } else {
            flat
        }
    });
    assert!(brush(&mut s, &disc(30.5, 30.5, 7.0), (60, 0), false, None, None).is_ok());
    for y in 0..60 {
        for x in 0..120 {
            assert!(close(getf(&mut s, x, y), flat, 2.0e-3), "({x}, {y})");
        }
    }
}

/// Prints the Healing Brush's heal time for a disc of radius 50 and 200
/// (`cargo test -p aurora-brush --lib healing_brush_cost -- --ignored
/// --nocapture`).
#[test]
#[ignore = "timing report"]
fn healing_brush_cost_report() {
    for radius in [50.0_f32, 200.0] {
        let (_dir, mut s) = store();
        let size = (radius * 6.0) as u32;
        paint(&mut s, size, size, noise);
        let c = size as f32 / 2.0;
        let mask = disc(c, c, radius);
        let offset = ((radius * 2.5) as i64, 0);
        let start = std::time::Instant::now();
        let out = healing_brush_covered(&mut s, surface(), &mask, 24.0, offset, false, None, None);
        eprintln!(
            "radius {radius} ({} texels): {:?} ok={}",
            mask.count(),
            start.elapsed(),
            out.is_ok()
        );
    }
}
