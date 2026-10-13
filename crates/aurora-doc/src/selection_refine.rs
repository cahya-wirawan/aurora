//! Refining a selection (0.216.0): Photoshop's Select > Modify commands as
//! pure functions over one [`Selection`]: [`feather`], [`expand`],
//! [`contract`], [`smooth`] and [`border`].
//!
//! Every result is `Ok(None)` when nothing is left (a selection set
//! deselects), and is built from the *effective* coverage in bytes
//! (`0..=255`, the unit a [`SelectionMask`] stores). A `Rect` or `Ellipse`
//! input is materialised to bytes first; the result is a [`SelectionMask`]
//! cut to the tight bounds of its non-zero texels (or a plain rectangle
//! when it turns out to be solid), never an empty non-inverted selection.
//!
//! **Expand / Contract** move the region's edge by `px` whole pixels,
//! measured as Euclidean distance (an exact distance transform, so the
//! grown shape is a true disk-offset, not a diamond or a square). The
//! *region* is the set of texels with coverage at least `128` (the 0.5
//! threshold the outline is drawn at). For a texel `p` outside it, `d` is
//! the Euclidean distance to the centre of the nearest region texel `q`
//! (ties broken by the transform's scan order) and `c` that texel's own
//! coverage in `0.5..=1`; then
//!
//! ```text
//! coverage(p) = clamp(px + c - d, 0, 1)
//! ```
//!
//! which is `1` for a neighbouring texel at `px = 1` and a straight
//! antialiased edge `0.4` outside its last texel stays a straight
//! antialiased edge `px` further out. Expand never lowers a texel
//! (`max` with the original). Contract is the **dual**: the complement of
//! the expansion of the complement, so an input exactly `255` away from
//! its complement erodes the same way. A texel below `128` is "outside"
//! for both, so a selection fainter than one half is untouched by Expand
//! and erased by Contract (stated, not hidden).
//!
//! **Feather** is a Gaussian blur of the coverage with sigma equal to
//! half the radius, realised as three successive box blurs (the standard
//! odd widths whose variances add up to sigma squared, in integer Q8 fixed
//! point, O(1) per texel, deterministic). The composite kernel is within
//! about 3 % of a Gaussian; the result grows by the sum of the three box
//! radii (about 1.5 times the radius, that is three sigma, at most 1.5
//! times the radius plus 3) on every side, zero outside the input. Total
//! coverage is conserved up to byte rounding. A radius below 1 px cannot be
//! told from none by a box pass and returns an equal selection.
//!
//! **Smooth** is a disk majority filter with a soft edge: for each texel
//! the mean coverage `m` over the digital disk of radius `px` around it,
//! then the coverage is `0.5 + (m - 0.5) * gain` clamped to `0..=1`, with
//! the gain the larger of 2 and 1.5 times `px` (the gain that keeps a
//! straight edge about one texel wide). Corners round off, specks and pin-holes smaller than the disk go;
//! a straight edge stays where it is. The result is confined to the input's
//! bounds.
//!
//! **Border** is the ring around the edge, `width` texels thick: the
//! expansion by `ceil(width / 2)` minus the contraction by `floor(width /
//! 2)` (the odd texel goes outside), subtracted per texel.
//!
//! **Inverted selections.** An inverted selection's effective coverage is
//! `255 - shape` everywhere, so no document size is needed: the operation
//! is applied to its *complement*. Expand of an inverted selection is the
//! inverted Contract of its shape (and vice versa); Feather and Smooth are
//! their own duals, so the shape is feathered or smoothed and the result
//! stays inverted. Border is a ring of the edge, which an inverted
//! selection shares with its shape, but "outside" and "inside" swap, so the
//! ring of `255 - shape` is `expand(shape, b) - contract(shape, a)` and the
//! result is a plain (non-inverted) ring. The "everything" selection (the
//! inverted empty rectangle) is unchanged by all but Border, which gives
//! nothing.
//!
//! **Bounded.** A result (including its growth) is at most
//! [`MAX_SELECTION_MASK_PIXELS`] texels, checked before anything is
//! allocated ([`DocError::SelectionMaskTooLarge`]); the distances are capped
//! at [`MAX_REFINE_PX`] (Feather at [`MAX_FEATHER_PX`], Smooth at
//! [`MAX_SMOOTH_PX`]) and the estimated work at [`MAX_REFINE_WORK`]
//! ([`DocError::SelectionRefineTooExpensive`]). Non-finite or out-of-range
//! parameters are [`DocError::SelectionRefineInvalid`]. A zero parameter
//! returns an equal selection.

// The transforms and filters name their coordinates and sizes by letter.
#![allow(clippy::many_single_char_names)]

use std::sync::Arc;

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection::{Selection, SelectionShape};
use crate::selection_mask::{MAX_SELECTION_MASK_PIXELS, SelectionMask};

/// The largest Expand, Contract or Border distance, in pixels.
pub const MAX_REFINE_PX: u32 = 500;

/// The largest Feather radius, in pixels.
pub const MAX_FEATHER_PX: f32 = 250.0;

/// The largest Smooth radius, in pixels.
pub const MAX_SMOOTH_PX: u32 = 100;

/// The most estimated work one refinement may take, in about nanoseconds
/// of the dev profile (so about 1.2 s there, less in release; 24 Mpx of
/// Expand, Contract or Feather): an engineering default that keeps the UI
/// thread responsive (measured: see PLAN.md 0.216.0), not a design
/// decision.
pub const MAX_REFINE_WORK: u64 = 1_200_000_000;

/// The coverage byte from which a texel counts as "in the region".
const REGION_THRESHOLD: u8 = 128;

/// A bounded coverage grid over a document rectangle.
pub(crate) struct Grid {
    pub(crate) rect: Rect,
    pub(crate) data: Vec<u8>,
}

fn us(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

fn to_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Whether a grid over `rect` is within the mask cap.
pub(crate) fn cap_ok(rect: &Rect) -> bool {
    rect.area_px() <= MAX_SELECTION_MASK_PIXELS
}

fn checked_area(rect: &Rect) -> Result<usize, DocError> {
    let pixels = rect.area_px();
    if !cap_ok(rect) {
        return Err(DocError::SelectionMaskTooLarge(pixels));
    }
    Ok(usize::try_from(pixels).unwrap_or(0))
}

fn inflate(rect: &Rect, by: u32) -> Rect {
    Rect {
        x: rect.x.saturating_sub(i64::from(by)),
        y: rect.y.saturating_sub(i64::from(by)),
        width: rect.width.saturating_add(by.saturating_mul(2)),
        height: rect.height.saturating_add(by.saturating_mul(2)),
    }
}

/// The inverted empty rectangle: everything.
fn everything() -> Selection {
    Selection {
        bounds: Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        inverted: true,
        shape: SelectionShape::Rect,
    }
}

/// `selection`'s un-inverted shape as bytes over its bounds.
fn materialise(selection: &Selection) -> Result<Grid, DocError> {
    let rect = selection.bounds;
    let n = checked_area(&rect)?;
    let mut data = Vec::with_capacity(n);
    match &selection.shape {
        SelectionShape::Rect => data.resize(n, u8::MAX),
        SelectionShape::Mask(mask) => {
            for y in rect.y..rect.bottom() {
                match mask.row_bytes(y, rect.x, rect.right()) {
                    Some(row) => data.extend_from_slice(row),
                    None => data.resize(data.len() + us(rect.width), 0),
                }
            }
        }
        SelectionShape::Ellipse => {
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    data.push(selection.shape_byte_at(x, y));
                }
            }
        }
    }
    Ok(Grid { rect, data })
}

/// A grid as a selection: cut to the tight bounds of its non-zero texels,
/// a plain rectangle when solid; nothing (or, `inverted`, everything) when
/// no texel is covered.
fn finish(grid: Grid, inverted: bool) -> Result<Option<Selection>, DocError> {
    let w = us(grid.rect.width);
    if w == 0 || grid.data.is_empty() {
        return Ok(inverted.then(everything));
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0_usize, 0_usize);
    let mut any = false;
    for (ly, row) in grid.data.chunks_exact(w).enumerate() {
        let first = row.iter().position(|&v| v != 0);
        let last = row.iter().rposition(|&v| v != 0);
        if let (Some(f), Some(l)) = (first, last) {
            any = true;
            min_x = min_x.min(f);
            max_x = max_x.max(l);
            min_y = min_y.min(ly);
            max_y = max_y.max(ly);
        }
    }
    if !any {
        return Ok(inverted.then(everything));
    }
    let tight = Rect {
        x: grid.rect.x.saturating_add(to_i64(min_x)),
        y: grid.rect.y.saturating_add(to_i64(min_y)),
        width: u32::try_from(max_x - min_x + 1).unwrap_or(u32::MAX),
        height: u32::try_from(max_y - min_y + 1).unwrap_or(u32::MAX),
    };
    let data = if tight == grid.rect {
        grid.data
    } else {
        let mut cropped = Vec::with_capacity(us(tight.width).saturating_mul(us(tight.height)));
        for row in grid
            .data
            .chunks_exact(w)
            .skip(min_y)
            .take(max_y - min_y + 1)
        {
            if let Some(part) = row.get(min_x..=max_x) {
                cropped.extend_from_slice(part);
            }
        }
        cropped
    };
    if data.iter().all(|&v| v == u8::MAX) {
        return Ok(Some(Selection {
            bounds: tight,
            inverted,
            shape: SelectionShape::Rect,
        }));
    }
    let mask = SelectionMask::new(tight, data)?;
    let mut selection = Selection::from_mask(Arc::new(mask));
    selection.inverted = inverted;
    Ok(Some(selection))
}

// ---- the exact Euclidean distance transform ----

/// A one-dimensional squared-distance transform with the nearest site's
/// coverage carried along (Felzenszwalb and Huttenlocher's lower envelope
/// of parabolas). `f[i]` is the squared distance already known at `i`
/// (`cap` for "no site within reach"); `out_d[i]` is the minimum over `j`
/// of `f[j] + (i - j)^2`, clamped to `cap`, and `out_c[i]` the coverage
/// carried by that `j`.
fn envelope(
    f: &[u32],
    c: &[u8],
    cap: u32,
    out_d: &mut [u32],
    out_c: &mut [u8],
    v: &mut Vec<usize>,
    z: &mut Vec<f64>,
) {
    let n = f.len();
    if n == 0 {
        return;
    }
    if f.iter().all(|&x| x >= cap) {
        out_d.fill(cap);
        out_c.fill(0);
        return;
    }
    v.clear();
    z.clear();
    v.push(0);
    z.push(f64::NEG_INFINITY);
    #[allow(clippy::cast_precision_loss)]
    let at = |i: usize| f64::from(f.get(i).copied().unwrap_or(cap)) + (i as f64) * (i as f64);
    for q in 1..n {
        let fq = at(q);
        while let Some(&p) = v.last() {
            #[allow(clippy::cast_precision_loss)]
            let s = (fq - at(p)) / (2.0 * (q as f64) - 2.0 * (p as f64));
            let zk = z.last().copied().unwrap_or(f64::NEG_INFINITY);
            if s <= zk && v.len() > 1 {
                v.pop();
                z.pop();
            } else {
                v.push(q);
                z.push(s);
                break;
            }
        }
    }
    let mut j = 0_usize;
    for q in 0..n {
        #[allow(clippy::cast_precision_loss)]
        while j + 1 < v.len() && z.get(j + 1).is_some_and(|&b| b < q as f64) {
            j += 1;
        }
        let p = v.get(j).copied().unwrap_or(0);
        let dq = q.abs_diff(p);
        let total = u64::from(f.get(p).copied().unwrap_or(cap)) + (dq as u64) * (dq as u64);
        if let Some(o) = out_d.get_mut(q) {
            *o = u32::try_from(total.min(u64::from(cap))).unwrap_or(cap);
        }
        if let Some(o) = out_c.get_mut(q) {
            *o = c.get(p).copied().unwrap_or(0);
        }
    }
}

/// For every texel of the `(w + 2 pad) x (h + 2 pad)` grid around the
/// `w x h` input `src`: the squared distance (clamped to `cap`) to the
/// nearest *site* texel and that site's coverage byte. A site is an input
/// texel with coverage at least [`REGION_THRESHOLD`]; with `outside_site`
/// every texel outside the input (all of the plane around it) is a site
/// of coverage `255` too (the transform of a complement).
pub(crate) fn nearest_sites(
    src: &[u8],
    w: usize,
    h: usize,
    pad: usize,
    outside_site: bool,
    cap: u32,
) -> (Vec<u32>, Vec<u8>) {
    let (ow, oh) = (w + 2 * pad, h + 2 * pad);
    let mut d2 = vec![cap; ow * oh];
    let mut cc = vec![0_u8; ow * oh];
    let end_f = if outside_site { 0 } else { cap };
    let end_c = if outside_site { u8::MAX } else { 0 };
    let site = |xo: usize, yo: usize| -> Option<u8> {
        if xo >= pad && xo < pad + w && yo >= pad && yo < pad + h {
            let b = src.get((yo - pad) * w + (xo - pad)).copied().unwrap_or(0);
            (b >= REGION_THRESHOLD).then_some(b)
        } else {
            outside_site.then_some(u8::MAX)
        }
    };
    let (mut v, mut z) = (Vec::new(), Vec::new());
    // Columns: the nearest site in the same column, virtual sites above
    // and below the grid when the outside is a site.
    let n = oh + 2;
    let (mut f, mut c) = (vec![cap; n], vec![0_u8; n]);
    let (mut od, mut oc) = (vec![cap; n], vec![0_u8; n]);
    for xo in 0..ow {
        if let Some(e) = f.first_mut() {
            *e = end_f;
        }
        if let Some(e) = f.last_mut() {
            *e = end_f;
        }
        if let Some(e) = c.first_mut() {
            *e = end_c;
        }
        if let Some(e) = c.last_mut() {
            *e = end_c;
        }
        for yo in 0..oh {
            let s = site(xo, yo);
            if let (Some(fe), Some(ce)) = (f.get_mut(yo + 1), c.get_mut(yo + 1)) {
                *fe = if s.is_some() { 0 } else { cap };
                *ce = s.unwrap_or(0);
            }
        }
        envelope(&f, &c, cap, &mut od, &mut oc, &mut v, &mut z);
        for yo in 0..oh {
            if let (Some(d), Some(cv), Some(&dv), Some(&cvv)) = (
                d2.get_mut(yo * ow + xo),
                cc.get_mut(yo * ow + xo),
                od.get(yo + 1),
                oc.get(yo + 1),
            ) {
                *d = dv;
                *cv = cvv;
            }
        }
    }
    // Rows: combine the columns, with virtual all-site columns left and
    // right of the grid when the outside is a site.
    let n = ow + 2;
    let (mut f, mut c) = (vec![cap; n], vec![0_u8; n]);
    let (mut od, mut oc) = (vec![cap; n], vec![0_u8; n]);
    for yo in 0..oh {
        if let Some(e) = f.first_mut() {
            *e = end_f;
        }
        if let Some(e) = f.last_mut() {
            *e = end_f;
        }
        if let Some(e) = c.first_mut() {
            *e = end_c;
        }
        if let Some(e) = c.last_mut() {
            *e = end_c;
        }
        let row = yo * ow;
        if let (Some(src_d), Some(src_c), Some(dst_f), Some(dst_c)) = (
            d2.get(row..row + ow),
            cc.get(row..row + ow),
            f.get_mut(1..=ow),
            c.get_mut(1..=ow),
        ) {
            dst_f.copy_from_slice(src_d);
            dst_c.copy_from_slice(src_c);
        }
        envelope(&f, &c, cap, &mut od, &mut oc, &mut v, &mut z);
        if let (Some(dst_d), Some(dst_c), Some(src_d), Some(src_c)) = (
            d2.get_mut(row..row + ow),
            cc.get_mut(row..row + ow),
            od.get(1..=ow),
            oc.get(1..=ow),
        ) {
            dst_d.copy_from_slice(src_d);
            dst_c.copy_from_slice(src_c);
        }
    }
    (d2, cc)
}

/// The dilation of the region of `src` (`w x h` bytes) by `px`, over a
/// grid `pad` texels larger on every side; see the module docs for the
/// coverage formula. With `outside_site` the plane outside `src` counts as
/// region too (used by Contract on the complement).
fn grow(src: &[u8], w: usize, h: usize, pad: usize, outside_site: bool, px: u32) -> Vec<u8> {
    let cap = (px + 1) * (px + 1) + 1;
    let (d2, cc) = nearest_sites(src, w, h, pad, outside_site, cap);
    let ow = w + 2 * pad;
    let mut out = vec![0_u8; d2.len()];
    let pxf = f32::from(u16::try_from(px).unwrap_or(u16::MAX));
    for (i, ((o, &d), &c)) in out.iter_mut().zip(&d2).zip(&cc).enumerate() {
        let (xo, yo) = (i % ow, i / ow);
        let own = if xo >= pad && xo < pad + w && yo >= pad && yo < pad + h {
            src.get((yo - pad) * w + (xo - pad)).copied().unwrap_or(0)
        } else if outside_site {
            u8::MAX
        } else {
            0
        };
        let grown = if d >= cap {
            0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let dist = (d as f32).sqrt();
            let cov = (pxf + f32::from(c) / 255.0 - dist).clamp(0.0, 1.0);
            // In 0..=255 after the clamp.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (cov * 255.0).round() as u8;
            byte
        };
        *o = grown.max(own);
    }
    out
}

fn expand_grid(g: &Grid, px: u32) -> Result<Grid, DocError> {
    let rect = inflate(&g.rect, px);
    checked_area(&rect)?;
    let data = grow(
        &g.data,
        us(g.rect.width),
        us(g.rect.height),
        us(px),
        false,
        px,
    );
    Ok(Grid { rect, data })
}

fn contract_grid(g: &Grid, px: u32) -> Grid {
    let comp: Vec<u8> = g.data.iter().map(|&b| u8::MAX - b).collect();
    let grown = grow(&comp, us(g.rect.width), us(g.rect.height), 0, true, px);
    Grid {
        rect: g.rect,
        data: grown.into_iter().map(|b| u8::MAX - b).collect(),
    }
}

// ---- Feather ----

/// The box radii (half widths) of the three passes for `radius`, or `None`
/// when a box cannot tell it from no blur.
pub(crate) fn feather_box_radii(radius: f32) -> Option<[u32; 3]> {
    // sigma = radius / 2; ideal width = sqrt(12 sigma^2 / 3 + 1).
    let r = f64::from(radius);
    let ideal = (r * r + 1.0).sqrt();
    // A non-negative, small value.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    #[allow(clippy::cast_precision_loss)]
    let m = ((3.0 * r * r - 3.0 * (wl * wl) as f64 - 12.0 * wl as f64 - 9.0)
        / (-4.0 * wl as f64 - 4.0))
        .round();
    // Between 0 and 3.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let m = m.clamp(0.0, 3.0) as usize;
    let half = |w: i64| u32::try_from((w - 1) / 2).unwrap_or(0);
    let mut radii = [half(wu); 3];
    for slot in radii.iter_mut().take(m) {
        *slot = half(wl);
    }
    if radii.iter().all(|&k| k == 0) {
        if radius < 1.0 {
            return None;
        }
        radii[2] = 1;
    }
    Some(radii)
}

/// One box blur of radius `k` (window `2k + 1`, zero outside) over `line`
/// into `out`, in Q8 with rounding.
fn box_line(line: &[u16], k: usize, out: &mut [u16]) {
    let n = line.len();
    let window = u32::try_from(2 * k + 1).unwrap_or(u32::MAX);
    let half = window / 2;
    let mut sum: u32 = line.iter().take(k + 1).map(|&v| u32::from(v)).sum();
    for i in 0..n {
        if i > 0 {
            if let Some(&add) = line.get(i + k) {
                sum += u32::from(add);
            }
            if i > k
                && let Some(&sub) = line.get(i - k - 1)
            {
                sum -= u32::from(sub);
            }
        }
        if let Some(o) = out.get_mut(i) {
            *o = u16::try_from((sum + half) / window).unwrap_or(u16::MAX);
        }
    }
}

fn feather_grid(g: &Grid, radii: [u32; 3]) -> Result<Grid, DocError> {
    let growth: u32 = radii.iter().sum();
    let rect = inflate(&g.rect, growth);
    checked_area(&rect)?;
    let (w, h) = (us(rect.width), us(rect.height));
    let (iw, ih, off) = (us(g.rect.width), us(g.rect.height), us(growth));
    let mut q = vec![0_u16; w * h];
    for (y, row) in g.data.chunks_exact(iw).enumerate().take(ih) {
        if let Some(dst) = q.get_mut((y + off) * w + off..(y + off) * w + off + iw) {
            for (d, &s) in dst.iter_mut().zip(row) {
                *d = u16::from(s) * 256;
            }
        }
    }
    let mut out = vec![0_u16; w.max(h)];
    for &k in &radii {
        let k = us(k);
        for row in q.chunks_exact_mut(w) {
            if let Some(o) = out.get_mut(..w) {
                box_line(row, k, o);
                row.copy_from_slice(o);
            }
        }
    }
    let mut col = vec![0_u16; h];
    for &k in &radii {
        let k = us(k);
        for x in 0..w {
            for (y, c) in col.iter_mut().enumerate() {
                *c = q.get(y * w + x).copied().unwrap_or(0);
            }
            if let Some(o) = out.get_mut(..h) {
                box_line(&col, k, o);
                for (y, &v) in o.iter().enumerate() {
                    if let Some(d) = q.get_mut(y * w + x) {
                        *d = v;
                    }
                }
            }
        }
    }
    let data = q
        .into_iter()
        .map(|v| u8::try_from((u32::from(v) + 128) >> 8).unwrap_or(u8::MAX))
        .collect();
    Ok(Grid { rect, data })
}

// ---- Smooth ----

/// `(dy, half-width)` of the digital disk of radius `r`, and its texel
/// count.
pub(crate) fn disk_rows(r: u32) -> (Vec<(i64, usize)>, u64) {
    let r = i64::from(r);
    let mut rows = Vec::new();
    let mut count = 0_u64;
    for dy in -r..=r {
        let sq = r * r - dy * dy;
        let mut hw = 0_i64;
        while (hw + 1) * (hw + 1) <= sq {
            hw += 1;
        }
        count += u64::try_from(2 * hw + 1).unwrap_or(0);
        rows.push((dy, usize::try_from(hw).unwrap_or(0)));
    }
    (rows, count)
}

fn smooth_grid(g: &Grid, r: u32) -> Grid {
    let (w, h) = (us(g.rect.width), us(g.rect.height));
    let (rows, count) = disk_rows(r);
    #[allow(clippy::cast_precision_loss)]
    let gain = (1.5 * f64::from(r)).max(2.0);
    #[allow(clippy::cast_precision_loss)]
    let scale = 1.0 / (255.0 * count as f64);
    let mut data = vec![0_u8; w * h];
    let mut sums = vec![0_u32; w];
    for (y, out) in data.chunks_exact_mut(w).enumerate() {
        sums.fill(0);
        for &(dy, hw) in &rows {
            let yy = to_i64(y) + dy;
            let Ok(yy) = usize::try_from(yy) else {
                continue;
            };
            if yy >= h {
                continue;
            }
            let Some(row) = g.data.get(yy * w..(yy + 1) * w) else {
                continue;
            };
            let mut s: u32 = row.iter().take(hw + 1).map(|&v| u32::from(v)).sum();
            for (x, acc) in sums.iter_mut().enumerate() {
                if x > 0 {
                    if let Some(&add) = row.get(x + hw) {
                        s += u32::from(add);
                    }
                    if x > hw
                        && let Some(&sub) = row.get(x - hw - 1)
                    {
                        s -= u32::from(sub);
                    }
                }
                *acc += s;
            }
        }
        for (o, &s) in out.iter_mut().zip(&sums) {
            let m = f64::from(s) * scale;
            let cov = (0.5 + (m - 0.5) * gain).clamp(0.0, 1.0);
            // In 0..=255 after the clamp.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (cov * 255.0).round() as u8;
            *o = byte;
        }
    }
    Grid { rect: g.rect, data }
}

// ---- Border ----

fn border_grid(g: &Grid, outer: u32, inner: u32) -> Result<Grid, DocError> {
    let grown = if outer == 0 {
        Grid {
            rect: g.rect,
            data: g.data.clone(),
        }
    } else {
        expand_grid(g, outer)?
    };
    let eroded = if inner == 0 {
        Grid {
            rect: g.rect,
            data: g.data.clone(),
        }
    } else {
        contract_grid(g, inner)
    };
    let mut data = grown.data;
    let gw = us(grown.rect.width);
    let (ew, eh) = (us(eroded.rect.width), us(eroded.rect.height));
    let dx = us(u32::try_from(eroded.rect.x - grown.rect.x).unwrap_or(0));
    let dy = us(u32::try_from(eroded.rect.y - grown.rect.y).unwrap_or(0));
    for y in 0..eh {
        for x in 0..ew {
            if let (Some(d), Some(&e)) = (
                data.get_mut((y + dy) * gw + x + dx),
                eroded.data.get(y * ew + x),
            ) {
                *d = d.saturating_sub(e);
            }
        }
    }
    Ok(Grid {
        rect: grown.rect,
        data,
    })
}

// ---- the public operations ----

/// What to do to the effective coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Refine {
    Expand(u32),
    Contract(u32),
    Feather([u32; 3]),
    Smooth(u32),
    Border { outer: u32, inner: u32 },
}

fn check_px(px: u32, max: u32) -> Result<(), DocError> {
    if px > max {
        return Err(DocError::SelectionRefineInvalid);
    }
    Ok(())
}

/// The estimated work of `op` over a grid of `pixels` texels, in about
/// nanoseconds on the dev profile (measured, PLAN.md 0.216.0): the
/// transforms and the blur cost about 50 ns a texel, a Border two of them,
/// Smooth about 3 ns a texel per row of its disk.
fn work(op: Refine, pixels: u64) -> u64 {
    match op {
        Refine::Expand(_) | Refine::Contract(_) | Refine::Feather(_) => pixels.saturating_mul(50),
        Refine::Border { .. } => pixels.saturating_mul(90),
        Refine::Smooth(r) => pixels.saturating_mul(3 * (2 * u64::from(r) + 1) + 4),
    }
}

fn run(selection: &Selection, op: Refine) -> Result<Option<Selection>, DocError> {
    let empty_shape = selection.bounds.is_empty();
    if empty_shape {
        let is_border = matches!(op, Refine::Border { .. });
        return Ok((selection.inverted && !is_border).then(everything));
    }
    // The operation on the shape that gives the operation on the
    // effective coverage (module docs: inverted selections).
    let (shape_op, inverted_out) = if selection.inverted {
        match op {
            Refine::Expand(px) => (Refine::Contract(px), true),
            Refine::Contract(px) => (Refine::Expand(px), true),
            Refine::Border { outer, inner } => (
                Refine::Border {
                    outer: inner,
                    inner: outer,
                },
                false,
            ),
            other => (other, true),
        }
    } else {
        (op, false)
    };
    checked_area(&selection.bounds)?;
    // The result's own size is checked before the input is read.
    let grown = match shape_op {
        Refine::Expand(px) => inflate(&selection.bounds, px),
        Refine::Border { outer, .. } => inflate(&selection.bounds, outer),
        Refine::Feather(radii) => inflate(&selection.bounds, radii.iter().sum()),
        Refine::Contract(_) | Refine::Smooth(_) => selection.bounds,
    };
    checked_area(&grown)?;
    let estimate = work(shape_op, grown.area_px());
    if estimate > MAX_REFINE_WORK {
        return Err(DocError::SelectionRefineTooExpensive(estimate));
    }
    let grid = materialise(selection)?;
    let out = match shape_op {
        Refine::Expand(px) => expand_grid(&grid, px)?,
        Refine::Contract(px) => contract_grid(&grid, px),
        Refine::Feather(radii) => feather_grid(&grid, radii)?,
        Refine::Smooth(r) => smooth_grid(&grid, r),
        Refine::Border { outer, inner } => border_grid(&grid, outer, inner)?,
    };
    finish(out, inverted_out)
}

/// `selection` as a set would store it when nothing changed.
fn unchanged(selection: &Selection) -> Option<Selection> {
    (!selection.bounds.is_empty() || selection.inverted).then(|| selection.clone())
}

/// Grows the selection by `px` pixels (Euclidean; module docs).
///
/// # Errors
///
/// [`DocError::SelectionRefineInvalid`] over [`MAX_REFINE_PX`],
/// [`DocError::SelectionMaskTooLarge`] when the result would be over the
/// mask cap, [`DocError::SelectionRefineTooExpensive`] over the work cap.
pub fn expand(selection: &Selection, px: u32) -> Result<Option<Selection>, DocError> {
    check_px(px, MAX_REFINE_PX)?;
    if px == 0 {
        return Ok(unchanged(selection));
    }
    run(selection, Refine::Expand(px))
}

/// Shrinks the selection by `px` pixels (Euclidean; module docs); `None`
/// when nothing is left.
///
/// # Errors
///
/// As [`expand`].
pub fn contract(selection: &Selection, px: u32) -> Result<Option<Selection>, DocError> {
    check_px(px, MAX_REFINE_PX)?;
    if px == 0 {
        return Ok(unchanged(selection));
    }
    run(selection, Refine::Contract(px))
}

/// Blurs the selection's edge with a Gaussian of `sigma = radius / 2`
/// (module docs).
///
/// # Errors
///
/// [`DocError::SelectionRefineInvalid`] for a non-finite, negative or
/// over-[`MAX_FEATHER_PX`] radius, and the other errors of [`expand`].
pub fn feather(selection: &Selection, radius_px: f32) -> Result<Option<Selection>, DocError> {
    if !radius_px.is_finite() || !(0.0..=MAX_FEATHER_PX).contains(&radius_px) {
        return Err(DocError::SelectionRefineInvalid);
    }
    let Some(radii) = feather_box_radii(radius_px) else {
        return Ok(unchanged(selection));
    };
    run(selection, Refine::Feather(radii))
}

/// Rounds off corners and removes specks (module docs).
///
/// # Errors
///
/// [`DocError::SelectionRefineInvalid`] over [`MAX_SMOOTH_PX`], and the
/// other errors of [`expand`].
pub fn smooth(selection: &Selection, radius_px: u32) -> Result<Option<Selection>, DocError> {
    check_px(radius_px, MAX_SMOOTH_PX)?;
    if radius_px == 0 {
        return Ok(unchanged(selection));
    }
    run(selection, Refine::Smooth(radius_px))
}

/// The ring of `width_px` around the selection's edge (module docs).
///
/// # Errors
///
/// As [`expand`]; width `0` is [`DocError::SelectionRefineInvalid`] (a
/// ring of no width is nothing, which is not what a user asked for).
pub fn border(selection: &Selection, width_px: u32) -> Result<Option<Selection>, DocError> {
    check_px(width_px, MAX_REFINE_PX)?;
    if width_px == 0 {
        return Err(DocError::SelectionRefineInvalid);
    }
    let inner = width_px / 2;
    let outer = width_px - inner;
    run(selection, Refine::Border { outer, inner })
}
