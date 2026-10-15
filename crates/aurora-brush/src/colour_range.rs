//! Colour Range (0.238.0): every pixel of a layer whose colour is near a
//! sampled one, as a soft selection mask. Pure over a tile store: it reads
//! the layer and writes nothing.
//!
//! **Distance.** CIE76 `Delta E`: the Euclidean distance between the two
//! colours in CIE L*a*b* (D65 white), after the layer's straight encoded
//! sRGB is linearised and taken to XYZ. A perceptual-ish space, so "near"
//! means about the same to the eye whatever the hue, which a per-channel
//! sRGB distance does not (it calls two greens and two blues the same
//! distance apart when the eye does not). The simplest Lab distance, not
//! `Delta E 2000`: it is cheap enough for 24 Mpx on the UI thread and the
//! ramp below hides its known unevenness. A choice, not compared with
//! Photoshop's own Fuzziness.
//!
//! **Fuzziness.** `0..=200` ([`MAX_FUZZINESS`]) maps to the threshold
//! `T = fuzziness * 0.5` `Delta E` ([`DELTA_E_PER_FUZZINESS`]): the default
//! 40 is `Delta E` 20, 200 is 100 (nearly everything but the opposite
//! colour). **Ramp.** A pixel at distance `d` is covered `1` up to `T / 2`
//! and falls linearly to `0` at `T`: `clamp((T - d) / (T / 2), 0, 1)`. At
//! fuzziness 0 only a colour equal to the sample (within `0.15` `Delta E`)
//! is covered.
//!
//! **Transparency.** The colour compared is the pixel's *straight* colour
//! (the premultiplied channels divided by alpha), and the coverage is
//! multiplied by the pixel's alpha, so a half-transparent pixel of the right
//! colour is half covered and a fully transparent one never is.

use std::sync::OnceLock;

use aurora_core::Rect;
use aurora_tile::{CHANNELS, SurfaceId, TILE, TileId, TileStore};
use half::f16;

use crate::FillError;

/// The largest Fuzziness.
pub const MAX_FUZZINESS: u16 = 200;

/// `Delta E` of threshold per unit of Fuzziness.
pub const DELTA_E_PER_FUZZINESS: f32 = 0.5;

/// At Fuzziness 0 a colour within this `Delta E` of the sample counts as
/// equal: more than the rounding of `f16` storage and of the conversions
/// (about 0.06 at worst), less than one 8-bit step of any channel (about
/// 0.2 or more), so "exactly this colour" holds for a colour picked in
/// `f32` and stored in `f16`.
const EXACT_DELTA_E: f32 = 0.15;

const TILE_SIDE: usize = TILE as usize;

/// The f16 bit pattern of `1.0`: an opaque pixel.
const ONE_BITS: u16 = 0x3C00;

/// What Colour Range was asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourRangeSpec {
    /// The sampled colour, straight encoded sRGB in `0..=1` per channel.
    pub colour: [f32; 3],
    /// `0..=`[`MAX_FUZZINESS`]; larger values read as the maximum.
    pub fuzziness: u16,
    /// The layer-local rectangle searched (clipped to the layer's area),
    /// usually the part of the layer inside the document.
    pub window: Rect,
    /// The cap on the window's area, in tiles of `TILE * TILE` texels: a
    /// larger window is refused before any read.
    pub max_tiles: usize,
}

/// The pixels Colour Range covered: a soft mask over their tight box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColourRangeMask {
    /// The tight bounding box of the non-zero coverage, layer-local.
    pub bounds: Rect,
    /// One coverage byte per texel of `bounds`, row-major.
    pub data: Vec<u8>,
}

/// sRGB (encoded) to linear light, for `c` in `0..=1`.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `srgb_to_linear` of every f16 bit pattern (a non-finite or negative
/// value reads as 0, above 1 as 1): 256 KiB, built once.
fn linear_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|bits| {
                let v = f16::from_bits(bits).to_f32();
                if v.is_finite() {
                    srgb_to_linear(v.clamp(0.0, 1.0))
                } else {
                    0.0
                }
            })
            .collect()
    })
}

/// The CIE L*a*b* `f` function.
fn lab_f(t: f32) -> f32 {
    const EPSILON: f32 = 216.0 / 24_389.0;
    const KAPPA: f32 = 24_389.0 / 27.0;
    if t > EPSILON {
        t.cbrt()
    } else {
        (KAPPA * t + 16.0) / 116.0
    }
}

/// L*a*b* (D65) of a linear-light colour.
fn lab_of_linear(r: f32, g: f32, b: f32) -> [f32; 3] {
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = (0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b) / 1.088_83;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// L*a*b* of a straight encoded sRGB colour (each channel clamped to
/// `0..=1`, a non-finite one read as 0).
#[must_use]
pub fn lab_of_srgb(rgb: [f32; 3]) -> [f32; 3] {
    let lin = |c: f32| {
        if c.is_finite() {
            srgb_to_linear(c.clamp(0.0, 1.0))
        } else {
            0.0
        }
    };
    lab_of_linear(lin(rgb[0]), lin(rgb[1]), lin(rgb[2]))
}

/// The CIE76 `Delta E` between two L*a*b* colours.
fn delta_e(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dl, da, db) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dl * dl + da * da + db * db).sqrt()
}

/// The `Delta E` between two straight encoded sRGB colours: the distance
/// Colour Range measures.
#[must_use]
pub fn colour_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    delta_e(lab_of_srgb(a), lab_of_srgb(b))
}

/// The coverage of a colour at `Delta E` distance `d` from the sample under
/// threshold `t` (`fuzziness * 0.5`): `1` up to `t / 2`, linear to `0` at
/// `t`; at `t == 0` only an exact match.
fn ramp(d: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return if d <= EXACT_DELTA_E { 1.0 } else { 0.0 };
    }
    ((t - d) / (t * 0.5)).clamp(0.0, 1.0)
}

/// The coverage a straight colour at distance `d` and alpha `alpha` gets,
/// as a byte (public for the model tests' sake: the whole rule in one
/// place).
#[must_use]
pub fn coverage_byte(d: f32, fuzziness: u16, alpha: f32) -> u8 {
    coverage_from(d, threshold(fuzziness), alpha)
}

/// The `Delta E` threshold of a Fuzziness (larger values read as the
/// maximum).
fn threshold(fuzziness: u16) -> f32 {
    f32::from(fuzziness.min(MAX_FUZZINESS)) * DELTA_E_PER_FUZZINESS
}

/// Finds the pixels of the layer `area` (its `(width, height)`) whose colour
/// is near `spec.colour`, as a soft mask. `Ok(None)` when no pixel is
/// covered at all. Reads only the tiles that exist (an absent tile is
/// transparent, so never covered); writes nothing.
///
/// # Errors
///
/// [`FillError::TooLarge`] when the searched window is over
/// `spec.max_tiles` tiles of area; [`FillError::Tile`] if a tile cannot be
/// read (nothing is returned then).
pub fn colour_range(
    store: &mut TileStore,
    surface: SurfaceId,
    area: (u32, u32),
    spec: &ColourRangeSpec,
) -> Result<Option<ColourRangeMask>, FillError> {
    // The window, clipped to the layer.
    let x0 = spec.window.x.max(0);
    let y0 = spec.window.y.max(0);
    let x1 = spec.window.right().min(i64::from(area.0));
    let y1 = spec.window.bottom().min(i64::from(area.1));
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    let (Ok(wx), Ok(wy), Ok(ww), Ok(wh)) = (
        u32::try_from(x0),
        u32::try_from(y0),
        u32::try_from(x1 - x0),
        u32::try_from(y1 - y0),
    ) else {
        return Ok(None);
    };
    let texels = u64::from(ww) * u64::from(wh);
    let cap = (spec.max_tiles as u64).saturating_mul(u64::from(TILE) * u64::from(TILE));
    if texels > cap {
        return Err(FillError::TooLarge {
            max: spec.max_tiles,
        });
    }
    let t = threshold(spec.fuzziness);
    let target = lab_of_srgb(spec.colour);
    let table = linear_table();
    let (width, height) = (ww as usize, wh as usize);
    let mut data = vec![0_u8; width.saturating_mul(height)];
    // The previous texel's bits and coverage: flat regions repeat.
    let mut previous: Option<([u16; CHANNELS], u8)> = None;
    let (mut lo, mut hi) = ((usize::MAX, usize::MAX), (0_usize, 0_usize));
    let mut any = false;
    for ty in (wy / TILE)..=((wy + wh - 1) / TILE) {
        for tx in (wx / TILE)..=((wx + ww - 1) / TILE) {
            let id = TileId { x: tx, y: ty };
            if !store.contains_tile(surface, id) {
                continue;
            }
            let tile = store.get(surface, id)?;
            let samples = tile.texels();
            let tile_x0 = tx * TILE;
            let tile_y0 = ty * TILE;
            let xs = wx.max(tile_x0)..(wx + ww).min(tile_x0 + TILE);
            let ys = wy.max(tile_y0)..(wy + wh).min(tile_y0 + TILE);
            for y in ys {
                for x in xs.clone() {
                    let lx = (x - tile_x0) as usize;
                    let ly = (y - tile_y0) as usize;
                    let start = (ly * TILE_SIDE + lx) * CHANNELS;
                    let Some(texel) = samples.get(start..start + CHANNELS) else {
                        continue;
                    };
                    let bits = [
                        texel.first().map_or(0, |v| v.to_bits()),
                        texel.get(1).map_or(0, |v| v.to_bits()),
                        texel.get(2).map_or(0, |v| v.to_bits()),
                        texel.get(3).map_or(0, |v| v.to_bits()),
                    ];
                    let byte = match previous {
                        Some((last, byte)) if last == bits => byte,
                        _ => {
                            let byte = texel_coverage(texel, bits, table, target, t);
                            previous = Some((bits, byte));
                            byte
                        }
                    };
                    if byte == 0 {
                        continue;
                    }
                    let (dx, dy) = ((x - wx) as usize, (y - wy) as usize);
                    if let Some(slot) = data.get_mut(dy * width + dx) {
                        *slot = byte;
                    }
                    lo = (lo.0.min(dx), lo.1.min(dy));
                    hi = (hi.0.max(dx), hi.1.max(dy));
                    any = true;
                }
            }
        }
    }
    if !any {
        return Ok(None);
    }
    let (bw, bh) = (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1);
    let mut out = Vec::with_capacity(bw.saturating_mul(bh));
    for row in lo.1..=hi.1 {
        let from = row * width + lo.0;
        if let Some(slice) = data.get(from..from + bw) {
            out.extend_from_slice(slice);
        }
    }
    let (Ok(bw32), Ok(bh32)) = (u32::try_from(bw), u32::try_from(bh)) else {
        return Ok(None);
    };
    Ok(Some(ColourRangeMask {
        bounds: Rect {
            x: i64::from(wx) + i64::try_from(lo.0).unwrap_or(0),
            y: i64::from(wy) + i64::try_from(lo.1).unwrap_or(0),
            width: bw32,
            height: bh32,
        },
        data: out,
    }))
}

/// The coverage byte of one premultiplied texel.
fn texel_coverage(
    texel: &[f16],
    bits: [u16; CHANNELS],
    table: &[f32],
    target: [f32; 3],
    t: f32,
) -> u8 {
    let alpha_bits = bits[3];
    let alpha = f16::from_bits(alpha_bits).to_f32();
    if !(alpha > 0.0) {
        return 0;
    }
    let lab = if alpha_bits == ONE_BITS {
        // Opaque: the stored channels are the straight colour.
        let lin = |i: usize| {
            bits.get(i)
                .and_then(|b| table.get(usize::from(*b)))
                .copied()
                .unwrap_or(0.0)
        };
        lab_of_linear(lin(0), lin(1), lin(2))
    } else {
        let straight = |i: usize| {
            let v = texel.get(i).map_or(0.0, |v| v.to_f32());
            if v.is_finite() {
                (v / alpha).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        lab_of_srgb([straight(0), straight(1), straight(2)])
    };
    coverage_from(delta_e(lab, target), t, alpha)
}

/// [`coverage_byte`] with the threshold already computed.
fn coverage_from(d: f32, t: f32, alpha: f32) -> u8 {
    let alpha = if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        0.0
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = (ramp(d, t) * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    byte
}
