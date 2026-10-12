//! The Paint Bucket and the Gradient (0.209.0): the document half. The
//! pixel work is `aurora_brush::{flood_fill, gradient_fill}`; this module
//! turns a document point or drag, the active layer, its locks and the
//! tool settings into those calls, and returns the undo snapshot and the
//! tiles to invalidate. It takes the parts it needs (not `App`), so a test
//! drives it without a window.
//!
//! **What they act on.** The active pixel layer only (Photoshop's "Sample:
//! Current Layer"; the composite is never read). **Selections are
//! ignored**, exactly as the Brush and Eraser ignore them today: nothing
//! in this crate paints through a selection yet. The Paint Bucket is
//! confined to the layer's bounds; the Gradient to the layer's bounds
//! intersected with the canvas.
//!
//! **Locks.** Lock image pixels (or all) refuses both with the calm
//! status-bar notice, before anything is read. A transparency lock lets
//! both recolour without ever changing alpha.
//!
//! **Bounds.** More than [`aurora_brush::MAX_FILL_TILES`] tiles is refused
//! with [`aurora_brush::FillError::TooLarge`] and the layer is untouched.

use aurora_brush::{
    FillError, FillMode, FloodSpec, GradientKind, GradientSpec, MAX_FILL_TILES, StrokeSnapshot,
};

use crate::{DabTool, LockRefusal, PaintPermission, ToolSettings, paint_permission};

/// The History row of one Paint Bucket fill.
pub(crate) const PAINT_BUCKET_LABEL: &str = "Paint Bucket Fill";
/// The History row of one Gradient.
pub(crate) const GRADIENT_LABEL: &str = "Gradient";

/// What a fill wrote: the undo snapshot (empty when nothing changed) and
/// the layer tiles to invalidate in the composite.
#[derive(Debug)]
pub(crate) struct Filled {
    pub(crate) snapshot: StrokeSnapshot,
    pub(crate) painted: Vec<aurora_tile::TileId>,
}

/// Why a fill did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillRefused {
    /// A calm notice (a lock, or the fill is too large).
    Notice(LockRefusal),
    /// No usable pixel layer or store: nothing to say.
    Nothing,
}

fn mode_for(lock: aurora_doc::LayerLock) -> Result<FillMode, FillRefused> {
    match paint_permission(DabTool::Brush, lock) {
        PaintPermission::Refused(why) => Err(FillRefused::Notice(why)),
        PaintPermission::PreserveAlpha => Ok(FillMode::PreserveAlpha),
        PaintPermission::Allowed => Ok(FillMode::Normal),
    }
}

fn refusal(err: &FillError) -> FillRefused {
    match err {
        FillError::TooLarge { .. } => FillRefused::Notice(LockRefusal::FillTooLarge),
        FillError::Tile(_) => FillRefused::Nothing,
    }
}

/// Quantises a settings percentage to a `0..=1` opacity.
fn opacity_of(percent: u8) -> f32 {
    f32::from(percent.min(100)) / 100.0
}

/// One Paint Bucket click at `doc_point` on `layer`.
///
/// # Errors
///
/// [`FillRefused::Notice`] for a lock or an oversized region.
pub(crate) fn bucket_fill(
    store: &mut aurora_tile::TileStore,
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    doc_point: (f32, f32),
    settings: &ToolSettings,
    foreground: [f32; 3],
) -> Result<Filled, FillRefused> {
    let Some(aurora_doc::LayerKind::Pixel { bounds }) = layers.kind(layer).cloned() else {
        return Err(FillRefused::Nothing);
    };
    let Some(surface) = layers.surface_id(layer) else {
        return Err(FillRefused::Nothing);
    };
    let mode = mode_for(layers.lock(layer).unwrap_or_default())?;
    let local = crate::layer_local_point(bounds, doc_point);
    if !(local.0.is_finite() && local.1.is_finite()) || local.0 < 0.0 || local.1 < 0.0 {
        return Err(FillRefused::Nothing);
    }
    #[allow(clippy::cast_sign_loss)]
    let seed = (local.0.floor() as u32, local.1.floor() as u32);
    let spec = FloodSpec {
        seed,
        tolerance: settings.tolerance,
        contiguous: settings.contiguous,
        max_tiles: MAX_FILL_TILES,
    };
    let mut snapshot = StrokeSnapshot::new(surface);
    let outcome = aurora_brush::flood_fill(
        store,
        surface,
        (bounds.width, bounds.height),
        &spec,
        foreground,
        mode,
        Some(&mut snapshot),
    )
    .map_err(|err| refusal(&err))?;
    for (tile, err) in outcome.failed() {
        tracing::warn!(?err, ?tile, "failed to fill part of a Paint Bucket region");
    }
    Ok(Filled {
        snapshot,
        painted: outcome.painted().to_vec(),
    })
}

/// The layer-local area `[x0, x1) x [y0, y1)` a gradient on a layer with
/// `bounds` covers: the layer intersected with the canvas.
#[must_use]
pub(crate) fn gradient_area(
    bounds: aurora_core::Rect,
    canvas_size: (u32, u32),
) -> (u32, u32, u32, u32) {
    let clamp = |v: i64| u32::try_from(v.max(0)).unwrap_or(u32::MAX);
    let x0 = clamp(-bounds.x);
    let y0 = clamp(-bounds.y);
    let x1 = clamp((i64::from(canvas_size.0) - bounds.x).min(i64::from(bounds.width)));
    let y1 = clamp((i64::from(canvas_size.1) - bounds.y).min(i64::from(bounds.height)));
    (x0, y0, x1, y1)
}

/// One Gradient drag from `start_doc` to `end_doc` on `layer`.
///
/// # Errors
///
/// [`FillRefused::Notice`] for a lock or an oversized area.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gradient_drag(
    store: &mut aurora_tile::TileStore,
    layers: &aurora_doc::LayerTree,
    layer: aurora_doc::LayerId,
    canvas_size: (u32, u32),
    start_doc: (f32, f32),
    end_doc: (f32, f32),
    settings: &ToolSettings,
    foreground: [f32; 3],
    background: [f32; 3],
) -> Result<Filled, FillRefused> {
    let Some(aurora_doc::LayerKind::Pixel { bounds }) = layers.kind(layer).cloned() else {
        return Err(FillRefused::Nothing);
    };
    let Some(surface) = layers.surface_id(layer) else {
        return Err(FillRefused::Nothing);
    };
    let mode = mode_for(layers.lock(layer).unwrap_or_default())?;
    let (from, to) = if settings.gradient_reverse {
        (background, foreground)
    } else {
        (foreground, background)
    };
    let spec = GradientSpec {
        kind: if settings.gradient_radial {
            GradientKind::Radial
        } else {
            GradientKind::Linear
        },
        start: crate::layer_local_point(bounds, start_doc),
        end: crate::layer_local_point(bounds, end_doc),
        from,
        to,
        opacity: opacity_of(settings.gradient_opacity),
        max_tiles: MAX_FILL_TILES,
    };
    let mut snapshot = StrokeSnapshot::new(surface);
    let outcome = aurora_brush::gradient_fill(
        store,
        surface,
        gradient_area(bounds, canvas_size),
        &spec,
        mode,
        Some(&mut snapshot),
    )
    .map_err(|err| refusal(&err))?;
    for (tile, err) in outcome.failed() {
        tracing::warn!(?err, ?tile, "failed to paint part of a gradient");
    }
    Ok(Filled {
        snapshot,
        painted: outcome.painted().to_vec(),
    })
}
