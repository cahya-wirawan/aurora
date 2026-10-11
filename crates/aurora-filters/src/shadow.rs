//! Drop-shadow coverage (0.182.0): a source alpha window, dilated by the
//! spread and blurred by three box passes, cropped to its centre.
//!
//! Exact integer arithmetic on purpose: alpha is held as Q24 fixed point
//! (`u32`, `1.0 = 2^24`, finer than `f16`'s 11-bit mantissa, so no
//! precision is lost against tile storage — invariant §7.3.1b), every box
//! sum is an exact `u64`, and each pass divides with round-half-up. A
//! pixel's result is therefore a function of the source values around it
//! only, never of where the window starts, so two composite tiles whose
//! windows overlap compute bit-identical shadow values where they meet:
//! a shadow is seamless across tile boundaries. A running `f32` sum would
//! not be (its rounding depends on the window's start).

/// `1.0` in the Q24 fixed point this module computes in.
const ONE: u32 = 1 << 24;

/// The drop-shadow coverage of the centre `out_side`² of a `side`² source
/// alpha window (row-major, straight alpha `0.0..=1.0`): a max filter of
/// radius `choke` (the spread, a square dilation applied row then column),
/// then three box blurs of radius `blur_radius`, each separable. Outside
/// the window counts as transparent. The result is exact wherever the
/// window's margin, `(side - out_side) / 2`, is at least `choke + 3 *
/// blur_radius`; a caller sizes the window that way.
///
/// Each pass computes only the region the passes after it still read
/// (the centre grown by their remaining radii), and every pass walks rows
/// (a vertical box pass keeps running column sums), so the work is about
/// `(out_side + 2·reach)²` per pass rather than `side²`, cache-friendly,
/// with two reused buffers and no per-line allocation (0.182.0 review).
///
/// Returns `out_side`² values in `0.0..=1.0`; an empty vector when the
/// sizes do not fit (`alpha.len() != side²` or `out_side > side`).
#[must_use]
pub fn shadow_coverage(
    alpha: &[f32],
    side: usize,
    choke: usize,
    blur_radius: usize,
    out_side: usize,
) -> Vec<f32> {
    if side.checked_mul(side) != Some(alpha.len()) || out_side > side {
        return Vec::new();
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut grid: Vec<u32> = alpha
        .iter()
        .map(|&a| {
            let a = if a.is_finite() {
                a.clamp(0.0, 1.0)
            } else {
                0.0
            };
            (f64::from(a) * f64::from(ONE)).round() as u32
        })
        .collect();
    let mut spare = vec![0_u32; grid.len()];
    let mut steps: Vec<Step> = Vec::new();
    if choke > 0 {
        steps.push(Step::Max(Axis::Rows, choke));
        steps.push(Step::Max(Axis::Cols, choke));
    }
    if blur_radius > 0 {
        for _ in 0..3 {
            steps.push(Step::Box(Axis::Rows, blur_radius));
            steps.push(Step::Box(Axis::Cols, blur_radius));
        }
    }
    // The region each step must produce, working back from the centre.
    let margin = (side - out_side) / 2;
    let centre = Region {
        x0: margin,
        x1: margin + out_side,
        y0: margin,
        y1: margin + out_side,
    };
    let mut regions = vec![centre; steps.len()];
    let mut need = centre;
    for (k, step) in steps.iter().enumerate().rev() {
        if let Some(slot) = regions.get_mut(k) {
            *slot = need;
        }
        need = need.grow(*step, side);
    }
    for (step, region) in steps.iter().zip(&regions) {
        match *step {
            Step::Box(Axis::Rows, r) => box_rows(&grid, &mut spare, side, r, *region),
            Step::Box(Axis::Cols, r) => box_cols(&grid, &mut spare, side, r, *region),
            Step::Max(Axis::Rows, r) => max_rows(&grid, &mut spare, side, r, *region),
            Step::Max(Axis::Cols, r) => max_cols(&grid, &mut spare, side, r, *region),
        }
        std::mem::swap(&mut grid, &mut spare);
    }
    let mut out = Vec::with_capacity(out_side * out_side);
    for row in grid.chunks_exact(side).skip(margin).take(out_side) {
        out.extend(
            row.iter()
                .skip(margin)
                .take(out_side)
                .map(|&v| (f64::from(v) / f64::from(ONE)) as f32),
        );
    }
    out
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    Rows,
    Cols,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Box(Axis, usize),
    Max(Axis, usize),
}

/// A half-open rectangle of the window.
#[derive(Debug, Clone, Copy)]
struct Region {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

impl Region {
    /// The input region `step` reads to produce `self`.
    fn grow(self, step: Step, side: usize) -> Self {
        let (axis, r) = match step {
            Step::Box(axis, r) | Step::Max(axis, r) => (axis, r),
        };
        match axis {
            Axis::Rows => Self {
                x0: self.x0.saturating_sub(r),
                x1: (self.x1 + r).min(side),
                ..self
            },
            Axis::Cols => Self {
                y0: self.y0.saturating_sub(r),
                y1: (self.y1 + r).min(side),
                ..self
            },
        }
    }
}

/// `src[y][x]`, zero outside the window.
fn at(src: &[u32], side: usize, x: isize, y: isize) -> u64 {
    if x < 0 || y < 0 {
        return 0;
    }
    let (x, y) = (x.unsigned_abs(), y.unsigned_abs());
    if x >= side || y >= side {
        return 0;
    }
    u64::from(src.get(y * side + x).copied().unwrap_or(0))
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn box_rows(src: &[u32], dst: &mut [u32], side: usize, r: usize, reg: Region) {
    let width = (2 * r + 1) as u64;
    let ri = r as isize;
    for y in reg.y0..reg.y1 {
        let yi = y as isize;
        let x0 = reg.x0 as isize;
        let mut sum: u64 = (x0 - ri..=x0 + ri).map(|x| at(src, side, x, yi)).sum();
        for x in reg.x0..reg.x1 {
            if let Some(d) = dst.get_mut(y * side + x) {
                *d = ((sum + width / 2) / width) as u32;
            }
            let xi = x as isize;
            sum += at(src, side, xi + ri + 1, yi);
            sum -= at(src, side, xi - ri, yi);
        }
    }
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn box_cols(src: &[u32], dst: &mut [u32], side: usize, r: usize, reg: Region) {
    let width = (2 * r + 1) as u64;
    let ri = r as isize;
    let y0 = reg.y0 as isize;
    let mut sums: Vec<u64> = (reg.x0..reg.x1)
        .map(|x| {
            (y0 - ri..=y0 + ri)
                .map(|y| at(src, side, x as isize, y))
                .sum()
        })
        .collect();
    for y in reg.y0..reg.y1 {
        let yi = y as isize;
        for (k, sum) in sums.iter_mut().enumerate() {
            let x = reg.x0 + k;
            if let Some(d) = dst.get_mut(y * side + x) {
                *d = ((*sum + width / 2) / width) as u32;
            }
            *sum += at(src, side, x as isize, yi + ri + 1);
            *sum -= at(src, side, x as isize, yi - ri);
        }
    }
}

#[allow(clippy::cast_possible_wrap)]
fn max_rows(src: &[u32], dst: &mut [u32], side: usize, r: usize, reg: Region) {
    let mut line = Vec::with_capacity(reg.x1 - reg.x0 + 2 * r);
    let mut out = Vec::new();
    for y in reg.y0..reg.y1 {
        line.clear();
        let lo = reg.x0 as isize - r as isize;
        let hi = (reg.x1 + r) as isize;
        line.extend((lo..hi).map(|x| at(src, side, x, y as isize)));
        sliding_max(&line, r, &mut out);
        for (k, v) in out.iter().skip(r).take(reg.x1 - reg.x0).enumerate() {
            if let Some(d) = dst.get_mut(y * side + reg.x0 + k) {
                *d = *v;
            }
        }
    }
}

#[allow(clippy::cast_possible_wrap)]
fn max_cols(src: &[u32], dst: &mut [u32], side: usize, r: usize, reg: Region) {
    let mut line = Vec::with_capacity(reg.y1 - reg.y0 + 2 * r);
    let mut out = Vec::new();
    for x in reg.x0..reg.x1 {
        line.clear();
        let lo = reg.y0 as isize - r as isize;
        let hi = (reg.y1 + r) as isize;
        line.extend((lo..hi).map(|y| at(src, side, x as isize, y)));
        sliding_max(&line, r, &mut out);
        for (k, v) in out.iter().skip(r).take(reg.y1 - reg.y0).enumerate() {
            if let Some(d) = dst.get_mut((reg.y0 + k) * side + x) {
                *d = *v;
            }
        }
    }
}

/// `out[i]` = the maximum of `line` over `[i - r, i + r]` (outside the
/// line counting as zero), by a monotone deque, linear in the length.
#[allow(clippy::cast_possible_truncation)]
fn sliding_max(line: &[u64], r: usize, out: &mut Vec<u32>) {
    let n = line.len();
    out.clear();
    out.resize(n, 0);
    let value = |k: usize| line.get(k).copied().unwrap_or(0);
    let mut deque: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for j in 0..n + r {
        if j < n {
            let v = value(j);
            while deque.back().is_some_and(|&b| value(b) <= v) {
                deque.pop_back();
            }
            deque.push_back(j);
        }
        let Some(i) = j.checked_sub(r) else {
            continue;
        };
        while deque.front().is_some_and(|&f| f + r < i) {
            deque.pop_front();
        }
        if let (Some(slot), Some(&f)) = (out.get_mut(i), deque.front()) {
            *slot = value(f) as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shadow_coverage;

    #[test]
    fn a_hard_edged_dilation_and_no_blur() {
        let side = 9;
        let mut alpha = vec![0.0; side * side];
        if let Some(c) = alpha.get_mut(4 * side + 4) {
            *c = 1.0;
        }
        let out = shadow_coverage(&alpha, side, 2, 0, side);
        for (i, v) in out.iter().enumerate() {
            let (x, y) = (i % side, i / side);
            let inside = (2..=6).contains(&x) && (2..=6).contains(&y);
            let want = if inside { 1.0 } else { 0.0 };
            assert!((*v - want).abs() < f32::EPSILON, "({x}, {y})");
        }
    }

    /// The region-trimmed passes give exactly what a naive full-window
    /// computation does.
    #[test]
    #[allow(
        clippy::many_single_char_names,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss
    )]
    fn trimmed_passes_match_a_naive_reference() {
        let side = 37;
        let alpha: Vec<f32> = (0..side * side)
            .map(|i| {
                if (i * 7919) % 13 < 4 {
                    ((i % 5) as f32) / 4.0
                } else {
                    0.0
                }
            })
            .collect();
        let (choke, r, out_side) = (2, 3, 9);
        let got = shadow_coverage(&alpha, side, choke, r, out_side);
        // Naive: every pass over the whole window.
        let one = f64::from(super::ONE);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mut g: Vec<u64> = alpha
            .iter()
            .map(|&a| (f64::from(a) * one).round() as u64)
            .collect();
        let get = |g: &[u64], x: isize, y: isize| -> u64 {
            if x < 0 || y < 0 || x as usize >= side || y as usize >= side {
                0
            } else {
                g.get(y as usize * side + x as usize).copied().unwrap_or(0)
            }
        };
        let pass = |g: &[u64], horizontal: bool, r: isize, max: bool| -> Vec<u64> {
            let mut o = vec![0_u64; side * side];
            for y in 0..side as isize {
                for x in 0..side as isize {
                    let vals = (-r..=r).map(|d| {
                        if horizontal {
                            get(g, x + d, y)
                        } else {
                            get(g, x, y + d)
                        }
                    });
                    let v = if max {
                        vals.max().unwrap_or(0)
                    } else {
                        let w = (2 * r + 1) as u64;
                        (vals.sum::<u64>() + w / 2) / w
                    };
                    if let Some(slot) = o.get_mut(y as usize * side + x as usize) {
                        *slot = v;
                    }
                }
            }
            o
        };
        g = pass(&g, true, 2, true);
        g = pass(&g, false, 2, true);
        for _ in 0..3 {
            g = pass(&g, true, 3, false);
            g = pass(&g, false, 3, false);
        }
        let m = (side - out_side) / 2;
        for (k, v) in got.iter().enumerate() {
            let (x, y) = (k % out_side + m, k / out_side + m);
            let want = g.get(y * side + x).copied().unwrap_or(0) as f64 / one;
            assert_eq!(
                f64::from(*v).to_bits(),
                (want as f32).into_f64_bits(),
                "({x}, {y})"
            );
        }
        let _ = choke;
        let _ = r;
    }

    trait IntoF64Bits {
        fn into_f64_bits(self) -> u64;
    }
    impl IntoF64Bits for f32 {
        fn into_f64_bits(self) -> u64 {
            f64::from(self).to_bits()
        }
    }

    #[test]
    fn box_blur_conserves_mass_and_is_window_independent() {
        let side = 41;
        let mut alpha = vec![0.0; side * side];
        if let Some(c) = alpha.get_mut(20 * side + 20) {
            *c = 1.0;
        }
        let full = shadow_coverage(&alpha, side, 0, 2, side);
        let mass: f32 = full.iter().sum();
        assert!((mass - 1.0).abs() < 1e-3, "{mass}");
        // The centre 21² of the same window, computed directly.
        let centre = shadow_coverage(&alpha, side, 0, 2, 21);
        for (k, v) in centre.iter().enumerate() {
            let (x, y) = (k % 21 + 10, k / 21 + 10);
            assert_eq!(Some(v), full.get(y * side + x));
        }
        assert!(shadow_coverage(&alpha, side + 1, 0, 2, 3).is_empty());
    }
}
