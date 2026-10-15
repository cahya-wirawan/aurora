//! CIE Lab (D50) and encoded sRGB, for the Photo Filter colour (0.241.0).
//!
//! A Photo Filter (`phfl`, version 2) stores its colour as Lab: colour
//! space `7`, three components `(L * 100, a * 100, b * 100)` (verified on
//! the corpus's `layers/photo-filter.psd`, whose block holds
//! `(6706, 3200, 12000)`: converted here it gives
//! `(236.4, 138.3, <0)` which, clipped, is exactly Photoshop's preset
//! "Warming Filter (85)", `#EC8A00`).
//!
//! # The conversion
//!
//! Photoshop's Lab is relative to the D50 white point, and so is the
//! matrix used here (Bradford-adapted sRGB primaries, Bruce Lindbloom's
//! values): `Lab -> XYZ (D50) -> linear sRGB -> encoded sRGB`. **D50 versus
//! D65:** the file never says D65, and a D65 conversion of the same Lab
//! gives a visibly different orange, so D50 is the only reading that
//! reproduces the preset. An Aurora document is tagged sRGB (invariant 6),
//! so the colour is always taken to be in sRGB.
//!
//! A Lab colour can lie outside the sRGB gamut (the warming preset's
//! blue is `-0.054` linear): each linear channel is **clipped to `0..=1`**
//! before encoding, which is what turns that Lab value into `#EC8A00`.
//! The inverse ([`srgb_to_lab`]) is exact in `f64`; the file rounds the
//! components to hundredths, so a colour that goes out and back differs by
//! about `1e-4`, far below an 8-bit level.

/// The D50 reference white (Lindbloom).
const WHITE: [f64; 3] = [0.964_22, 1.0, 0.825_21];

/// Linear sRGB (D50-adapted) to XYZ.
const TO_XYZ: [[f64; 3]; 3] = [
    [0.436_074_7, 0.385_064_9, 0.143_080_4],
    [0.222_504_5, 0.716_878_6, 0.060_616_9],
    [0.013_932_2, 0.097_104_5, 0.714_173_3],
];

/// XYZ (D50) to linear sRGB.
const FROM_XYZ: [[f64; 3]; 3] = [
    [3.133_856_1, -1.616_866_7, -0.490_614_6],
    [-0.978_768_4, 1.916_141_5, 0.033_454_0],
    [0.071_945_3, -0.228_991_4, 1.405_242_7],
];

/// CIE `epsilon` and `kappa`.
const EPSILON: f64 = 216.0 / 24_389.0;
const KAPPA: f64 = 24_389.0 / 27.0;

#[allow(clippy::many_single_char_names)] // a dot product's own letters
fn dot([a, b, c]: [f64; 3], [x, y, z]: [f64; 3]) -> f64 {
    a.mul_add(x, b.mul_add(y, c * z))
}

fn mul([r0, r1, r2]: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [dot(*r0, v), dot(*r1, v), dot(*r2, v)]
}

fn decode(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055f64.mul_add(c.powf(1.0 / 2.4), -0.055)
    }
}

fn f_inverse(f: f64) -> f64 {
    let cubed = f * f * f;
    if cubed > EPSILON {
        cubed
    } else {
        116.0f64.mul_add(f, -16.0) / KAPPA
    }
}

fn f_forward(t: f64) -> f64 {
    if t > EPSILON {
        t.cbrt()
    } else {
        KAPPA.mul_add(t, 16.0) / 116.0
    }
}

/// Lab (D50) to encoded sRGB in `0.0..=1.0`, out-of-gamut channels
/// clipped. Non-finite input gives black.
#[must_use]
pub(super) fn lab_to_srgb(l: f64, a: f64, b: f64) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let [wx, wy, wz] = WHITE;
    let xyz = [f_inverse(fx) * wx, f_inverse(fy) * wy, f_inverse(fz) * wz];
    let linear = mul(&FROM_XYZ, xyz);
    let out = linear.map(|c| {
        let c = if c.is_finite() {
            c.clamp(0.0, 1.0)
        } else {
            0.0
        };
        #[allow(clippy::cast_possible_truncation)] // 0..=1
        {
            encode(c) as f32
        }
    });
    out.map(|c| c.clamp(0.0, 1.0))
}

/// Encoded sRGB (`0.0..=1.0` per channel) to Lab (D50) `(L, a, b)`.
#[must_use]
pub(super) fn srgb_to_lab(rgb: [f32; 3]) -> (f64, f64, f64) {
    let linear = rgb.map(|c| decode(f64::from(c).clamp(0.0, 1.0)));
    let [x, y, z] = mul(&TO_XYZ, linear);
    let [wx, wy, wz] = WHITE;
    let (fx, fy, fz) = (f_forward(x / wx), f_forward(y / wy), f_forward(z / wz));
    (
        116.0f64.mul_add(fy, -16.0),
        500.0 * (fx - fy),
        200.0 * (fy - fz),
    )
}
