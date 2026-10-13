//! Which dab spacing a stroke uses (0.226.1).
//!
//! The soft Brush paints with the max-alpha rule, so at the old `0.25`
//! spacing its centre-line alpha dipped 18 % between dabs (visible beading);
//! it now lays a dab every [`aurora_brush::BRUSH_SPACING`] of its radius.
//! **Every other dab tool keeps [`aurora_brush::DEFAULT_SPACING`] exactly**,
//! because their per-dab effects accumulate (the Eraser multiplies, the
//! Smudge, Blur, Sharpen, Dodge, Burn and Sponge mix per dab, the Clone
//! Stamp copies per dab) and the Pencil's hard dabs have no falloff to
//! bead; denser dabs would change their strength, not just smooth them.
//!
//! **Quick Mask stays on the old spacing too.** Its Brush edits the mask
//! with [`aurora_brush::stamp_toward`], which is *not* max-style: each dab
//! moves a texel toward the target by its falloff (`new = old + (target -
//! old) * a`), so overlapping dabs accumulate and the strength grows with
//! the dab count. There is no beading to fix there (accumulation smooths
//! it), and 8x denser dabs would make every Quick Mask stroke far heavier.
//! A Quick Mask stroke is told apart by having no stroke snapshot
//! (`quick_mask::drag_layer` hands `begin_drag` no layer), which is also the
//! only other case of a Brush drag with nothing to paint.

use crate::Drag;

/// Whether `drag` is a stroke of the soft Brush on a layer: a
/// `Drag::Brush` that is not the Pencil, Clone Stamp, Smudge, Blur,
/// Sharpen or a toning tool, and that holds a stroke snapshot (so it is not
/// a Quick Mask stroke).
#[must_use]
pub(crate) fn is_soft_layer_brush(drag: &Drag) -> bool {
    matches!(
        drag,
        Drag::Brush {
            hard: false,
            clone: None,
            smudge: None,
            filter: None,
            tone: None,
            stroke: Some(_),
            ..
        }
    )
}

/// How a drag's dabs are spaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DabSpacing {
    /// The soft Brush on a layer: [`aurora_brush::BRUSH_SPACING`], capped
    /// per pointer event.
    BrushDense,
    /// Every other dab drag: [`aurora_brush::DEFAULT_SPACING`], uncapped.
    Standard,
}

impl DabSpacing {
    /// The spacing as a fraction of the radius.
    #[must_use]
    pub(crate) const fn fraction(self) -> f32 {
        match self {
            Self::BrushDense => aurora_brush::BRUSH_SPACING,
            Self::Standard => aurora_brush::DEFAULT_SPACING,
        }
    }
}

/// The spacing `drag`'s dabs are laid at: [`DabSpacing::BrushDense`] for
/// [`is_soft_layer_brush`], [`DabSpacing::Standard`] for every other drag
/// (the Eraser, Pencil, Clone Stamp, Smudge, Blur, Sharpen, Dodge, Burn,
/// Sponge, a Quick Mask stroke, and drags that lay no dabs).
#[must_use]
pub(crate) fn spacing_of(drag: &Drag) -> DabSpacing {
    if is_soft_layer_brush(drag) {
        DabSpacing::BrushDense
    } else {
        DabSpacing::Standard
    }
}

/// The step for one pointer event moving from `from` to `to` (document
/// space) with `carry` already travelled, for a drag of the given
/// `spacing` (taken before the drag's fields are borrowed): the Brush gets
/// its dense step capped to [`aurora_brush::brush_dab_cap`] dabs for the
/// event ([`aurora_brush::brush_segment_step`]); every other drag the plain
/// [`aurora_brush::dab_step`] at its spacing, with no cap, exactly as
/// before 0.226.1.
#[must_use]
pub(crate) fn event_step(
    spacing: DabSpacing,
    radius: f32,
    from: (f32, f32),
    to: (f32, f32),
    carry: f32,
) -> f32 {
    match spacing {
        DabSpacing::BrushDense => {
            let len = (to.0 - from.0).hypot(to.1 - from.1);
            aurora_brush::brush_segment_step(radius, carry, len)
        }
        DabSpacing::Standard => aurora_brush::dab_step(radius, spacing.fraction()),
    }
}
