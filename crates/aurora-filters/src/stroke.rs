//! Stroke coverage (0.190.0): how much of each pixel a layer's Stroke
//! effect covers, from a window of the layer's shape alpha and the
//! Euclidean distance to the shape's edge.
//!
//! **Edge distance.** The shape's edge runs through every partly covered
//! pixel. A pixel `q` with alpha `a` has its edge about `a - 0.5` pixels
//! beyond its centre toward the empty side (`a = 1`: at its far side,
//! `a = 0.5`: through its centre, `a -> 0`: at its near side), so the
//! distance from a point `p` to the shape is `min over q of
//! (|p - q| - (a(q) - 0.5))`. The nearest `q` is found with an exact
//! squared Euclidean distance transform (Felzenszwalb and Huttenlocher's
//! lower envelope of parabolas, in integers, tracking which pixel won)
//! and its alpha corrects the distance by up to half a pixel, which is
//! what makes a stroke's edge smooth rather than stair-stepped. The
//! transform is exact for the pixel centres; the correction assumes the
//! edge is locally straight and perpendicular to the line to `p` (true
//! for straight edges and smooth curves, off by a fraction of a pixel at
//! corners). It is **an approximation of Photoshop's own, not measured
//! against it beyond the corpus fixtures** (PLAN.md, 0.190.0).
//!
//! **Coverage.** A stroke `width` pixels wide next to the edge covers a
//! pixel at distance `d` by `clamp(width + 0.5 - d, 0, 1)`: the box
//! filter of a one-pixel-wide pixel across the stroke's far edge.
//!
//! Deterministic and seamless: the transform is exact integer arithmetic
//! with a tie rule that depends only on the relative order of pixels, so a
//! pixel's value is a function of the window's content within its reach,
//! never of where the window starts. Two composite tiles whose windows
//! overlap agree bit for bit where they meet.

/// Which side of the shape's edge a coverage plane is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeSide {
    /// Outside the shape: the stroke of an Outside (or half a Center)
    /// stroke. Already scaled by `1 - alpha`, since a partly covered edge
    /// pixel is stroked only where the shape is not.
    Outside,
    /// Inside the shape: Inside (or half a Center). **Not** scaled by
    /// alpha — the caller lays it down inside the shape, which scales it.
    Inside,
}

/// The window margin, in pixels, a coverage of `width` needs to be exact:
/// every source within `width + 1` of an output pixel must be in the
/// window.
#[must_use]
pub fn stroke_reach(width: f32) -> usize {
    if width.is_finite() && width > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = width.ceil().min(f32::from(u16::MAX)) as usize;
        whole + 1
    } else {
        1
    }
}

/// No site: the distance is unbounded.
const NO_SITE: u32 = u32::MAX;

/// The stroke coverage of the centre `out_side`² of a `side`² alpha
/// window (row-major, straight alpha `0.0..=1.0`, outside the window
/// transparent), `width` pixels wide on `which` side. Exact wherever the
/// window's margin, `(side - out_side) / 2`, is at least
/// [`stroke_reach`]`(width)`.
///
/// Returns `out_side`² values in `0.0..=1.0`, or an empty vector when the
/// sizes do not fit (`alpha.len() != side²`, `out_side > side`) or the
/// window holds no edge the stroke could reach.
#[must_use]
pub fn stroke_coverage(
    alpha: &[f32],
    side: usize,
    out_side: usize,
    width: f32,
    which: StrokeSide,
) -> Vec<f32> {
    if side.checked_mul(side) != Some(alpha.len())
        || out_side > side
        || side == 0
        || !width.is_finite()
        || width <= 0.0
        || side > usize::from(u16::MAX)
    {
        return Vec::new();
    }
    let alphas: Vec<f32> = alpha
        .iter()
        .map(|&a| {
            if a.is_finite() {
                a.clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();
    // The sites the distance is measured to: pixels the shape touches
    // (outside) or does not fully cover (inside).
    let is_site = |a: f32| match which {
        StrokeSide::Outside => a > 0.0,
        StrokeSide::Inside => a < 1.0,
    };
    if !alphas.iter().any(|&a| is_site(a)) {
        return Vec::new();
    }
    let (d2, site) = nearest_sites(&alphas, side, is_site);
    let margin = (side - out_side) / 2;
    let mut out = Vec::with_capacity(out_side * out_side);
    let mut any = false;
    for y in margin..margin + out_side {
        for x in margin..margin + out_side {
            let i = y * side + x;
            let coverage = match (d2.get(i).copied(), site.get(i).copied()) {
                (Some(d2), Some(site)) if d2 != NO_SITE => {
                    let site_alpha = alphas.get(site as usize).copied().unwrap_or(0.0);
                    // How much of the site pixel lies on the shape's side
                    // of the edge, as the edge's offset from its centre.
                    let near = match which {
                        StrokeSide::Outside => site_alpha,
                        StrokeSide::Inside => 1.0 - site_alpha,
                    };
                    let distance = f64::from(d2).sqrt() - (f64::from(near) - 0.5);
                    let covered = (f64::from(width) + 0.5 - distance).clamp(0.0, 1.0);
                    let own = alphas.get(i).copied().unwrap_or(0.0);
                    #[allow(clippy::cast_possible_truncation)]
                    let covered = match which {
                        StrokeSide::Outside => (covered * f64::from(1.0 - own)) as f32,
                        StrokeSide::Inside => covered as f32,
                    };
                    covered
                }
                _ => 0.0,
            };
            any |= coverage > 0.0;
            out.push(coverage);
        }
    }
    if any { out } else { Vec::new() }
}

/// For every pixel, the squared distance to the nearest site and that
/// site's index (`NO_SITE` when none). Ties between equally near sites
/// resolve by position only (the rule of the parabola envelope).
fn nearest_sites(
    alphas: &[f32],
    side: usize,
    is_site: impl Fn(f32) -> bool,
) -> (Vec<u32>, Vec<u32>) {
    let n = side * side;
    // Pass 1, per column: the nearest site above or below in the column.
    let mut col_d2 = vec![NO_SITE; n];
    let mut col_y = vec![0_u32; n];
    let mut scratch = Envelope::default();
    let mut f = vec![NO_SITE; side];
    let mut win = vec![usize::MAX; side];
    for x in 0..side {
        for (y, slot) in f.iter_mut().enumerate() {
            let site = alphas.get(y * side + x).is_some_and(|&a| is_site(a));
            *slot = if site { 0 } else { NO_SITE };
        }
        scratch.run(&f, &mut win);
        for (y, &w) in win.iter().enumerate() {
            if w == usize::MAX {
                continue;
            }
            let dy = y.abs_diff(w) as u64;
            if let (Some(d), Some(sy)) = (col_d2.get_mut(y * side + x), col_y.get_mut(y * side + x))
            {
                *d = u32::try_from(dy * dy).unwrap_or(NO_SITE);
                *sy = u32::try_from(w).unwrap_or(0);
            }
        }
    }
    // Pass 2, per row: the best column's nearest site.
    let mut d2 = vec![NO_SITE; n];
    let mut site = vec![0_u32; n];
    for y in 0..side {
        let row = y * side;
        for (x, slot) in f.iter_mut().enumerate() {
            *slot = col_d2.get(row + x).copied().unwrap_or(NO_SITE);
        }
        scratch.run(&f, &mut win);
        for (x, &w) in win.iter().enumerate() {
            if w == usize::MAX {
                continue;
            }
            let dx = x.abs_diff(w) as u64;
            let down = u64::from(col_d2.get(row + w).copied().unwrap_or(NO_SITE));
            let sy = col_y.get(row + w).copied().unwrap_or(0) as usize;
            if let (Some(d), Some(s)) = (d2.get_mut(row + x), site.get_mut(row + x)) {
                *d = u32::try_from(dx * dx + down).unwrap_or(NO_SITE);
                *s = u32::try_from(sy * side + w).unwrap_or(0);
            }
        }
    }
    (d2, site)
}

/// One-dimensional lower envelope of parabolas `(i - q)² + f[q]`,
/// reusing its buffers.
#[derive(Default)]
struct Envelope {
    v: Vec<usize>,
    /// Where each parabola in `v` starts to win, as a fraction `(num,
    /// den)` with `den > 0`; `None` is minus infinity.
    z: Vec<Option<(i64, i64)>>,
}

impl Envelope {
    /// For each `i`, the `q` whose parabola is lowest at `i`
    /// (`usize::MAX` when no `f` is finite). Intersections are compared
    /// as exact fractions, so the result is a function of the finite
    /// sites' relative positions and costs only.
    #[allow(clippy::cast_possible_wrap)]
    fn run(&mut self, f: &[u32], win: &mut [usize]) {
        self.v.clear();
        self.z.clear();
        let cost = |q: usize| f.get(q).copied().map_or(0_i64, i64::from) + (q * q) as i64;
        for (q, &fq) in f.iter().enumerate() {
            if fq == NO_SITE {
                continue;
            }
            loop {
                let Some(&p) = self.v.last() else {
                    self.v.push(q);
                    self.z.push(None);
                    break;
                };
                let s = (cost(q) - cost(p), 2 * (q as i64 - p as i64));
                let beaten = match self.z.last().copied().flatten() {
                    None => false,
                    Some(z) => {
                        i128::from(s.0) * i128::from(z.1) <= i128::from(z.0) * i128::from(s.1)
                    }
                };
                if beaten {
                    self.v.pop();
                    self.z.pop();
                } else {
                    self.v.push(q);
                    self.z.push(Some(s));
                    break;
                }
            }
        }
        let mut k = 0;
        for (i, slot) in win.iter_mut().enumerate() {
            if self.v.is_empty() {
                *slot = usize::MAX;
                continue;
            }
            while let Some(Some(z)) = self.z.get(k + 1) {
                // z <= i
                if z.0 <= (i as i64) * z.1 {
                    k += 1;
                } else {
                    break;
                }
            }
            *slot = self.v.get(k).copied().unwrap_or(usize::MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `side`² window with a filled rectangle `[x0, x1) x [y0, y1)`.
    fn rect(side: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<f32> {
        let mut a = vec![0.0; side * side];
        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(v) = a.get_mut(y * side + x) {
                    *v = 1.0;
                }
            }
        }
        a
    }

    fn at(v: &[f32], side: usize, x: usize, y: usize) -> f32 {
        v.get(y * side + x).copied().unwrap_or(f32::NAN)
    }

    #[test]
    fn outside_width_three_covers_three_pixels_beside_an_edge() {
        // 24² window, shape x 8..16, y 8..16, out_side 24 (everything).
        let side = 24;
        let cov = stroke_coverage(
            &rect(side, 8, 8, 16, 16),
            side,
            side,
            3.0,
            StrokeSide::Outside,
        );
        // Straight edge: x = 7, 6, 5 fully, x = 4 not at all.
        for x in [7, 6, 5] {
            assert!((at(&cov, side, x, 12) - 1.0).abs() < 1e-6, "x {x}");
        }
        assert!(at(&cov, side, 4, 12).abs() < 1e-6);
        // Right side and the shape itself.
        assert!((at(&cov, side, 18, 12) - 1.0).abs() < 1e-6);
        assert!(at(&cov, side, 19, 12).abs() < 1e-6);
        assert!(at(&cov, side, 12, 12).abs() < 1e-6, "not over the shape");
    }

    #[test]
    fn outside_corner_is_round_not_square() {
        let side = 32;
        let cov = stroke_coverage(
            &rect(side, 12, 12, 20, 20),
            side,
            side,
            4.0,
            StrokeSide::Outside,
        );
        // Diagonal from the corner pixel (11, 11): distance sqrt(2)*k.
        assert!(at(&cov, side, 9, 9) > 0.0, "(2,2) away is inside the arc");
        assert!(at(&cov, side, 8, 8).abs() < 1e-6, "(3,3) away is past it");
        // Straight out from the same corner the stroke reaches 4 pixels.
        assert!((at(&cov, side, 8, 12) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn inside_width_two_covers_two_pixels_inside_the_edge() {
        let side = 24;
        let cov = stroke_coverage(
            &rect(side, 8, 8, 16, 16),
            side,
            side,
            2.0,
            StrokeSide::Inside,
        );
        for x in [8, 9] {
            assert!((at(&cov, side, x, 12) - 1.0).abs() < 1e-6, "x {x}");
        }
        assert!(at(&cov, side, 10, 12).abs() < 1e-6);
        assert!((at(&cov, side, 15, 12) - 1.0).abs() < 1e-6);
        assert!(at(&cov, side, 12, 12).abs() < 1e-6);
    }

    #[test]
    fn a_partly_covered_edge_pixel_gives_a_smooth_edge() {
        // A shape edge at x = 8.5: column 8 is half covered.
        let side = 24;
        let mut a = rect(side, 9, 4, 20, 20);
        for y in 4..20 {
            if let Some(v) = a.get_mut(y * side + 8) {
                *v = 0.5;
            }
        }
        let out = stroke_coverage(&a, side, side, 2.0, StrokeSide::Outside);
        // The half-covered pixel: stroked where the shape is not (0.5).
        assert!((at(&out, side, 8, 12) - 0.5).abs() < 1e-6);
        // The edge runs through column 8's centre, 1.0 from column 7's:
        // covered 2 + 0.5 - 1 -> 1; column 6 is 2.0 away: 0.5; column 5: 0.
        assert!((at(&out, side, 7, 12) - 1.0).abs() < 1e-6);
        assert!((at(&out, side, 6, 12) - 0.5).abs() < 1e-6);
        assert!(at(&out, side, 5, 12).abs() < 1e-6);
        // A column 8 at 0.25 puts the edge 0.25 further out: column 6
        // falls to 0.25.
        let mut b = a.clone();
        for y in 4..20 {
            if let Some(v) = b.get_mut(y * side + 8) {
                *v = 0.25;
            }
        }
        let out = stroke_coverage(&b, side, side, 2.0, StrokeSide::Outside);
        assert!((at(&out, side, 6, 12) - 0.25).abs() < 1e-6);
    }

    /// The window offset never matters: a cropped centre equals the same
    /// region of a bigger window, bit for bit.
    #[test]
    fn coverage_is_independent_of_the_window() {
        let big = 96;
        let mut a = vec![0.0_f32; big * big];
        // A smooth blob with soft edges.
        for y in 0..big {
            for x in 0..big {
                let dx = x as f32 - 47.3;
                let dy = y as f32 - 51.1;
                let d = (dx * dx + dy * dy).sqrt();
                if let Some(v) = a.get_mut(y * big + x) {
                    *v = (20.5 - d).clamp(0.0, 1.0);
                }
            }
        }
        let full = |which| stroke_coverage(&a, big, big, 5.0, which);
        let reach = stroke_reach(5.0);
        for which in [StrokeSide::Outside, StrokeSide::Inside] {
            let whole = full(which);
            // The window of the 32² centre at (30, 30) with margin `reach`.
            let out_side = 32;
            let side = out_side + 2 * reach;
            let (ox, oy) = (30 - reach, 30 - reach);
            let mut sub = Vec::new();
            for y in 0..side {
                for x in 0..side {
                    sub.push(at(&a, big, ox + x, oy + y));
                }
            }
            let part = stroke_coverage(&sub, side, out_side, 5.0, which);
            for y in 0..out_side {
                for x in 0..out_side {
                    let want = at(&whole, big, 30 + x, 30 + y);
                    let want = if want.is_nan() { 0.0 } else { want };
                    let got = if part.is_empty() {
                        0.0
                    } else {
                        at(&part, out_side, x, y)
                    };
                    assert_eq!(got.to_bits(), want.to_bits(), "{which:?} ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn degenerate_inputs_give_nothing() {
        assert!(stroke_coverage(&[0.0; 4], 2, 2, 3.0, StrokeSide::Outside).is_empty());
        assert!(stroke_coverage(&[1.0; 4], 2, 2, 0.0, StrokeSide::Outside).is_empty());
        assert!(stroke_coverage(&[1.0; 3], 2, 2, 3.0, StrokeSide::Outside).is_empty());
        assert!(stroke_coverage(&[1.0; 4], 2, 4, 3.0, StrokeSide::Outside).is_empty());
        assert!(stroke_coverage(&[1.0; 4], 2, 2, f32::NAN, StrokeSide::Inside).is_empty());
        // NaN alpha counts as transparent.
        let nan = [f32::NAN; 16];
        assert!(stroke_coverage(&nan, 4, 4, 2.0, StrokeSide::Outside).is_empty());
    }

    #[test]
    fn the_distance_transform_matches_brute_force() {
        // A pseudo-random binary window; compare squared distances.
        let side = 40;
        let mut state = 12345_u64;
        let mut a = vec![0.0_f32; side * side];
        for v in &mut a {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            if (state >> 33).is_multiple_of(23) {
                *v = 1.0;
            }
        }
        let (d2, site) = nearest_sites(&a, side, |v| v > 0.0);
        for y in 0..side {
            for x in 0..side {
                let mut best = u64::MAX;
                for sy in 0..side {
                    for sx in 0..side {
                        if at(&a, side, sx, sy) > 0.0 {
                            let dx = x.abs_diff(sx) as u64;
                            let dy = y.abs_diff(sy) as u64;
                            best = best.min(dx * dx + dy * dy);
                        }
                    }
                }
                let got = d2.get(y * side + x).copied().unwrap_or(NO_SITE);
                assert_eq!(u64::from(got), best, "({x}, {y})");
                // The site really is at that distance.
                let s = site.get(y * side + x).copied().unwrap_or(0) as usize;
                let (sx, sy) = (s % side, s / side);
                let dx = x.abs_diff(sx) as u64;
                let dy = y.abs_diff(sy) as u64;
                assert_eq!(dx * dx + dy * dy, best);
            }
        }
    }
}
