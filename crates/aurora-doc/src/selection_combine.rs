//! Combining selections (0.215.0): Photoshop's New / Add / Subtract /
//! Intersect modes as a pure function over two [`Selection`]s.
//!
//! The arithmetic is on coverage **bytes** (`0..=255`, the unit a
//! [`SelectionMask`] stores; a rectangle is `0`/`255`, an ellipse
//! `round(k/16 * 255)`), on each operand's *effective* coverage (`255 -
//! shape` when it is inverted):
//!
//! * **Add** is `max(a, b)`: idempotent, commutative, never creates a dent
//!   where two soft edges overlap.
//! * **Intersect** is `min(a, b)`: idempotent and commutative.
//! * **Subtract** is `a - b` clamped at `0`: exactly `0` for a selection
//!   minus itself, and `255 - b` for a fully selected `a`.
//!
//! For hard (`0`/`255`) operands these are the set operations; for soft
//! edges they are the standard fuzzy-set choices, documented here because
//! any of several would do.
//!
//! **Inversion is exact, not approximated.** An inverted operand covers
//! everything outside its bounds, so there is no finite box to
//! materialise it over. Instead the result is computed over the box where
//! it can differ from its constant value outside every operand's bounds,
//! and is itself stored inverted when that constant is "covered" (Add with
//! an inverted operand, Intersect of two inverted ones, `inverted -
//! plain`): a mask holding `255 - effective` plus the `inverted` flag.
//! No document size is needed and nothing outside the document is lost.
//! The result is then trimmed to the tight bounds of its non-trivial
//! texels. A result that is nothing deselects (`Ok(None)`); one that is
//! everything is the inverted empty rectangle, which a selection set keeps.
//!
//! A result that is exactly a rectangle stays a plain
//! [`SelectionShape::Rect`]: always for rectangle-only operands where that
//! is cheap (disjoint, contained, intersecting), and whenever a computed
//! mask turns out to be solid. The result mask is bounded by
//! [`MAX_SELECTION_MASK_PIXELS`]; a larger one is refused with
//! [`DocError::SelectionMaskTooLarge`] before anything is allocated.

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection::{Selection, SelectionShape};
use crate::selection_mask::{MAX_SELECTION_MASK_PIXELS, SelectionMask};

/// How a new selection combines with the existing one (Photoshop's four
/// selection-mode buttons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// The new selection replaces the old one.
    #[default]
    Replace,
    /// The union: `max(old, new)`.
    Add,
    /// The old selection minus the new: `old - new` clamped at zero.
    Subtract,
    /// Only what both cover: `min(old, new)`.
    Intersect,
}

impl SelectionMode {
    /// Every mode, in the options bar's order.
    pub const ALL: [Self; 4] = [Self::Replace, Self::Add, Self::Subtract, Self::Intersect];
}

/// The elementwise operation behind a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Union,
    Intersect,
    Subtract,
}

impl Op {
    fn apply(self, a: u8, b: u8) -> u8 {
        match self {
            Self::Union => a.max(b),
            Self::Intersect => a.min(b),
            Self::Subtract => a.saturating_sub(b),
        }
    }

    /// Whether `a`'s value outside its bounds alone fixes the result there
    /// (so the result is confined to `a`'s bounds).
    fn absorbs_a(self, a: &Selection) -> bool {
        match self {
            Self::Union => a.inverted,
            Self::Intersect | Self::Subtract => !a.inverted,
        }
    }

    /// The same for `b`.
    fn absorbs_b(self, b: &Selection) -> bool {
        match self {
            Self::Union | Self::Subtract => b.inverted,
            Self::Intersect => !b.inverted,
        }
    }

    /// Whether the result covers everything outside both bounds.
    fn outside_is_covered(self, a: &Selection, b: &Selection) -> bool {
        match self {
            Self::Union => a.inverted || b.inverted,
            Self::Intersect => a.inverted && b.inverted,
            Self::Subtract => a.inverted && !b.inverted,
        }
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

/// `selection` as a selection set would store it: a non-inverted one with
/// no area is nothing.
fn normalised(selection: &Selection) -> Option<Selection> {
    (!selection.bounds.is_empty() || selection.inverted).then(|| selection.clone())
}

/// The part of `a` inside `b` (an empty rectangle when they do not meet).
fn intersection(a: &Rect, b: &Rect) -> Rect {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    if right <= x || bottom <= y {
        return Rect {
            x,
            y,
            width: 0,
            height: 0,
        };
    }
    Rect {
        x,
        y,
        width: u32::try_from(right - x).unwrap_or(u32::MAX),
        height: u32::try_from(bottom - y).unwrap_or(u32::MAX),
    }
}

/// Whether `outer` contains every texel of `inner` (both non-empty).
fn rect_contains(outer: &Rect, inner: &Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

/// The bounds of `selection` when it is a plain, non-inverted rectangle.
fn plain_rect(selection: &Selection) -> Option<Rect> {
    (selection.is_rect() && !selection.inverted && !selection.bounds.is_empty())
        .then_some(selection.bounds)
}

/// Combines `new` into `existing` by `mode`; see the module docs for the
/// arithmetic. `existing` of `None` is "nothing selected": Replace and Add
/// give `new`, Subtract and Intersect give nothing (as Photoshop's).
///
/// `Ok(None)` is "nothing selected" (a non-inverted selection with no
/// covered texel is never returned).
///
/// # Errors
///
/// [`DocError::SelectionMaskTooLarge`] when the result would need a mask of
/// more than [`MAX_SELECTION_MASK_PIXELS`] texels.
pub fn combine_selections(
    existing: Option<&Selection>,
    new: &Selection,
    mode: SelectionMode,
) -> Result<Option<Selection>, DocError> {
    let Some(old) = existing else {
        return Ok(match mode {
            SelectionMode::Replace | SelectionMode::Add => normalised(new),
            SelectionMode::Subtract | SelectionMode::Intersect => None,
        });
    };
    if mode == SelectionMode::Replace {
        return Ok(normalised(new));
    }
    if let Some(done) = cheap_result(old, new, mode) {
        return Ok(done);
    }
    let op = match mode {
        SelectionMode::Add => Op::Union,
        SelectionMode::Subtract => Op::Subtract,
        SelectionMode::Intersect | SelectionMode::Replace => Op::Intersect,
    };
    combine(old, new, op)
}

/// Exact answers that need no mask: disjoint bounds, and rectangles that
/// contain one another. `None` when the general path must run, `Some(r)`
/// when the answer is `r`.
// `Some(None)` (the answer is nothing) differs from `None` (no shortcut).
#[allow(clippy::option_option)]
fn cheap_result(
    old: &Selection,
    new: &Selection,
    mode: SelectionMode,
) -> Option<Option<Selection>> {
    if old.inverted || new.inverted {
        return None;
    }
    let disjoint = !old.bounds.intersects(&new.bounds);
    match mode {
        SelectionMode::Add => {
            if new.bounds.is_empty() {
                return Some(normalised(old));
            }
            let (a, b) = (plain_rect(old)?, plain_rect(new)?);
            if rect_contains(&a, &b) {
                Some(Some(old.clone()))
            } else if rect_contains(&b, &a) {
                Some(Some(new.clone()))
            } else {
                None
            }
        }
        SelectionMode::Subtract => {
            if disjoint {
                return Some(normalised(old));
            }
            let covering = plain_rect(new)?;
            rect_contains(&covering, &old.bounds).then_some(None)
        }
        SelectionMode::Intersect => {
            if disjoint {
                return Some(None);
            }
            let (a, b) = (plain_rect(old)?, plain_rect(new)?);
            Some(Some(Selection::new(intersection(&a, &b))))
        }
        SelectionMode::Replace => None,
    }
}

/// Fills `buf` with `selection`'s effective coverage on row `y` from
/// column `x0`.
fn fill_effective_row(selection: &Selection, y: i64, x0: i64, buf: &mut [u8]) {
    buf.fill(0);
    let b = &selection.bounds;
    if !b.is_empty() && y >= b.y && y < b.bottom() {
        let width = i64::try_from(buf.len()).unwrap_or(0);
        let lo = x0.max(b.x);
        let hi = x0.saturating_add(width).min(b.right());
        if lo < hi
            && let (Ok(s), Ok(e)) = (usize::try_from(lo - x0), usize::try_from(hi - x0))
            && let Some(dst) = buf.get_mut(s..e)
        {
            match &selection.shape {
                SelectionShape::Rect => dst.fill(u8::MAX),
                SelectionShape::Mask(mask) => {
                    if let Some(src) = mask.row_bytes(y, lo, hi) {
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = *s;
                        }
                    }
                }
                SelectionShape::Ellipse => {
                    for (d, x) in dst.iter_mut().zip(lo..hi) {
                        *d = selection.shape_byte_at(x, y);
                    }
                }
            }
        }
    }
    if selection.inverted {
        for v in buf.iter_mut() {
            *v = u8::MAX - *v;
        }
    }
}

/// The general path: the box where the result can differ from its constant
/// outside value, computed row by row.
// `a` and `b` are the two operands; `ra`/`rb` below are their row buffers.
#[allow(clippy::many_single_char_names)]
fn combine(a: &Selection, b: &Selection, op: Op) -> Result<Option<Selection>, DocError> {
    let region = match (op.absorbs_a(a), op.absorbs_b(b)) {
        (true, true) => intersection(&a.bounds, &b.bounds),
        (true, false) => a.bounds,
        (false, true) => b.bounds,
        (false, false) => a.bounds.union(&b.bounds),
    };
    let covered_outside = op.outside_is_covered(a, b);
    let nothing_inside = || {
        if covered_outside {
            Some(everything())
        } else {
            None
        }
    };
    if region.is_empty() {
        return Ok(nothing_inside());
    }
    let pixels = region.area_px();
    if pixels > MAX_SELECTION_MASK_PIXELS {
        return Err(DocError::SelectionMaskTooLarge(pixels));
    }
    let width = usize::try_from(region.width).unwrap_or(0);
    let mut data = vec![0_u8; usize::try_from(pixels).unwrap_or(0)];
    let (mut row_a, mut row_b) = (vec![0_u8; width], vec![0_u8; width]);
    // Tight bounds of the texels that differ from the outside value
    // (stored as `0`).
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0_usize, 0_usize);
    let mut any = false;
    for (ly, out) in data.chunks_exact_mut(width).enumerate() {
        let y = region.y.saturating_add(i64::try_from(ly).unwrap_or(0));
        fill_effective_row(a, y, region.x, &mut row_a);
        fill_effective_row(b, y, region.x, &mut row_b);
        for (lx, ((o, &ea), &eb)) in out.iter_mut().zip(&row_a).zip(&row_b).enumerate() {
            let e = op.apply(ea, eb);
            let g = if covered_outside { u8::MAX - e } else { e };
            *o = g;
            if g != 0 {
                any = true;
                min_x = min_x.min(lx);
                max_x = max_x.max(lx);
                min_y = min_y.min(ly);
                max_y = max_y.max(ly);
            }
        }
    }
    if !any {
        return Ok(nothing_inside());
    }
    let tight = Rect {
        x: region.x.saturating_add(i64::try_from(min_x).unwrap_or(0)),
        y: region.y.saturating_add(i64::try_from(min_y).unwrap_or(0)),
        width: u32::try_from(max_x - min_x + 1).unwrap_or(u32::MAX),
        height: u32::try_from(max_y - min_y + 1).unwrap_or(u32::MAX),
    };
    let data = if tight == region {
        data
    } else {
        let tw = usize::try_from(tight.width).unwrap_or(0);
        let mut cropped =
            Vec::with_capacity(tw.saturating_mul(usize::try_from(tight.height).unwrap_or(0)));
        for row in data.chunks_exact(width).skip(min_y).take(max_y - min_y + 1) {
            if let Some(part) = row.get(min_x..=max_x) {
                cropped.extend_from_slice(part);
            }
        }
        cropped
    };
    if data.iter().all(|&v| v == u8::MAX) {
        return Ok(Some(Selection {
            bounds: tight,
            inverted: covered_outside,
            shape: SelectionShape::Rect,
        }));
    }
    let mask = SelectionMask::new(tight, data)?;
    let mut selection = Selection::from_mask(std::sync::Arc::new(mask));
    selection.inverted = covered_outside;
    Ok(Some(selection))
}
