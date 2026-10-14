//! Fill Path, Stroke Path and Make Work Path from Selection (0.234.0): the
//! document half. The pixel work is `aurora_brush::{fill_with_coverage,
//! stamp_dab_amount}`; this module turns the active path, the active layer,
//! its locks, the selection and the tool settings into those calls and
//! returns what [`crate::commit_fill`] records as ONE undo step. It takes
//! the parts it needs (not `App`), so a test drives it without a window.
//!
//! **Fill Path** paints the path's interior (nonzero winding, antialiased,
//! [`aurora_doc::selection_from_path`]) on the active pixel layer with the
//! foreground colour at the Brush's Opacity (source-over). It composes with
//! the selection (the amount scales by its coverage), the pixels lock (the
//! Bucket's refusal), the transparency lock (recolour, alpha untouched) and
//! the Bucket's tile cap; a fill that changes nothing records nothing.
//!
//! **Stroke Path** walks the flattened path and stamps the Brush's dab
//! ([`aurora_brush::stamp_dab_amount`], the entry point the live Brush
//! uses, with the Brush's radius, spacing, Opacity and Flow) as ONE stroke
//! (one [`aurora_brush::StrokeSnapshot`], so the stroke compositing and the
//! Opacity ceiling are per stroke and the step is one). Dabs sit at the
//! Brush's spacing along the arc length ([`path_dabs`]); every subpath
//! starts afresh with a dab on its first point, a closed subpath includes
//! its closing segment, an open one does not. A path needing more dabs than
//! [`stroke_dab_cap`] is refused with a calm notice before anything is
//! painted.

use aurora_brush::{DEFAULT_SPACING, MAX_FILL_TILES, ShapeFillSpec, StrokeSnapshot, dab_step};
use aurora_doc::{LayerId, LayerTree, Path, PathSet, Selection, WORK_PATH_NAME};

use crate::fill_tools::{self, FillRefused, Filled};
use crate::paths_app::PathsCommand;
use crate::pen::PenStep;
use crate::{App, DabTool, LockRefusal, ToolSettings, selection_paint};

pub(crate) const FILL_PATH_LABEL: &str = "Fill Path";
pub(crate) const STROKE_PATH_LABEL: &str = "Stroke Path";
pub(crate) const MAKE_WORK_PATH_LABEL: &str = "Make Work Path";

pub(crate) const COMMAND_FILL: &str = "paths.fill";
pub(crate) const FILL_TITLE: &str = "Fill Path";
pub(crate) const COMMAND_STROKE: &str = "paths.stroke";
pub(crate) const STROKE_TITLE: &str = "Stroke Path";
pub(crate) const COMMAND_MAKE_FROM_SELECTION: &str = "paths.make_from_selection";
pub(crate) const MAKE_FROM_SELECTION_TITLE: &str = "Make Work Path from Selection";

pub(crate) const NEED_FILL_POINTS_NOTICE: &str =
    "Fill Path needs a path with a subpath of at least three points.";
pub(crate) const NEED_STROKE_POINTS_NOTICE: &str =
    "Stroke Path needs a path with a subpath of at least two points.";
pub(crate) const NOTHING_TO_FILL_NOTICE: &str = "This path has no area to fill.";
pub(crate) const FILL_FAILED_NOTICE: &str = "That path is too large to fill.";
pub(crate) const TOO_MANY_DABS_NOTICE: &str =
    "That stroke would need too many brush dabs. Use a larger brush or a shorter path.";
pub(crate) const NO_SELECTION_NOTICE: &str = "Make a selection first.";
pub(crate) const SELECTION_TOO_COMPLEX_NOTICE: &str =
    "That selection is too complex to turn into a path.";
pub(crate) const NOTHING_SELECTED_NOTICE: &str = "That selection covers no pixel.";

/// The most texel visits one Stroke Path may cost (an engineering default:
/// about three seconds on the dev profile at the default brush).
const STROKE_WORK_BUDGET: f32 = 40_000_000.0;
/// The most dabs a Stroke Path may place, however small the brush.
pub(crate) const MAX_STROKE_DABS: usize = 200_000;
/// The fewest dabs a Stroke Path may always place, however big the brush.
pub(crate) const MIN_STROKE_DABS: usize = 256;

/// The most dabs a stroke with a brush of `radius` may place: a work budget
/// divided by the dab's area, within [`MIN_STROKE_DABS`] and
/// [`MAX_STROKE_DABS`].
#[must_use]
pub(crate) fn stroke_dab_cap(radius: f32) -> usize {
    let radius = if radius.is_finite() {
        radius.max(0.5)
    } else {
        0.5
    };
    let area = std::f32::consts::PI * radius * radius;
    // Truncation is intended: a small positive count.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let raw = (STROKE_WORK_BUDGET / area) as usize;
    raw.clamp(MIN_STROKE_DABS, MAX_STROKE_DABS)
}

/// A flattened subpath: its points and whether it is closed.
pub(crate) type FlatSubpath = (Vec<(f32, f32)>, bool);

/// The dabs of a stroke and whether the cap cut it short.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PathDabs {
    pub(crate) dabs: Vec<(f32, f32)>,
    /// More dabs were due than `cap`; `dabs` holds the first `cap`.
    pub(crate) truncated: bool,
}

/// The arc length of a polyline, with the closing segment when `closed`.
fn polyline_length(points: &[(f32, f32)], closed: bool) -> f64 {
    let mut total = 0.0_f64;
    for pair in points.windows(2) {
        if let [a, b] = pair {
            total += f64::from(b.0 - a.0).hypot(f64::from(b.1 - a.1));
        }
    }
    if closed && let (Some(first), Some(last)) = (points.first(), points.last()) {
        total += f64::from(first.0 - last.0).hypot(f64::from(first.1 - last.1));
    }
    total
}

/// The dab centres of `subpaths`, `step` apart along the arc length
/// ([`aurora_brush::advance_segment`], the live Brush's own engine): each
/// subpath restarts with a dab on its first point and a fresh carry; the
/// spacing carries across the segments of one subpath; a closed subpath's
/// closing segment is walked and a last dab that would land within a
/// quarter step of its first is dropped; a subpath of one point, or of
/// coincident points, is one dab. A path whose length over `step` (plus a
/// dab per subpath) exceeds `cap` is refused BEFORE it is walked: no dabs
/// and `truncated` set (it never allocates a dab list it cannot use). A non-positive or non-finite `step`
/// gives none.
#[must_use]
pub(crate) fn path_dabs(subpaths: &[FlatSubpath], step: f32, cap: usize) -> PathDabs {
    let mut out = PathDabs::default();
    if !(step.is_finite() && step > 0.0) {
        return out;
    }
    // Refuse before walking anything: the arc length (closing segments
    // included) over the step, plus a dab per subpath, is the dab count.
    let mut length = 0.0_f64;
    for (points, closed) in subpaths {
        length += polyline_length(points, *closed);
    }
    #[allow(clippy::cast_precision_loss)]
    let wanted = length / f64::from(step) + subpaths.len() as f64;
    #[allow(clippy::cast_precision_loss)]
    if !wanted.is_finite() || wanted > cap as f64 {
        out.truncated = true;
        return out;
    }
    'subpaths: for (points, closed) in subpaths {
        let Some(&first) = points.first() else {
            continue;
        };
        if out.dabs.len() >= cap {
            out.truncated = true;
            break;
        }
        let begin = out.dabs.len();
        out.dabs.push(first);
        let mut previous = first;
        let mut carry = 0.0_f32;
        let closing = (*closed && points.len() > 1).then_some(first);
        for point in points.iter().skip(1).copied().chain(closing) {
            let (placed, next_carry) = aurora_brush::advance_segment(previous, point, carry, step);
            for dab in placed {
                if out.dabs.len() >= cap {
                    out.truncated = true;
                    break 'subpaths;
                }
                out.dabs.push(dab);
            }
            carry = next_carry;
            previous = point;
        }
        if closing.is_some() && out.dabs.len() > begin + 1 && carry < 0.25 * step {
            out.dabs.pop();
        }
    }
    out
}

/// Whether some subpath of `path` has at least `points` anchors.
fn has_subpath_of(path: &Path, points: usize) -> bool {
    path.subpaths.iter().any(|s| s.anchors.len() >= points)
}

/// Whether Fill Path has something to fill (a subpath of three anchors).
#[must_use]
pub(crate) fn path_can_fill(path: &Path) -> bool {
    has_subpath_of(path, 3)
}

/// Whether Stroke Path has something to stroke (a subpath of two anchors).
#[must_use]
pub(crate) fn path_can_stroke(path: &Path) -> bool {
    has_subpath_of(path, 2)
}

fn pixel_target(
    layers: &LayerTree,
    layer: LayerId,
) -> Result<(aurora_core::Rect, aurora_tile::SurfaceId), FillRefused> {
    let Some(aurora_doc::LayerKind::Pixel { bounds }) = layers.kind(layer).cloned() else {
        return Err(FillRefused::Notice(LockRefusal::FillNeedsPixelLayer));
    };
    let Some(surface) = layers.surface_id(layer) else {
        return Err(FillRefused::Notice(LockRefusal::FillNeedsPixelLayer));
    };
    Ok((bounds, surface))
}

/// Fills `path`'s interior on `layer`.
///
/// # Errors
///
/// [`FillRefused::Notice`] for a lock, an oversized fill, a path with no
/// area or one too large to rasterise.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_path(
    store: &mut aurora_tile::TileStore,
    layers: &LayerTree,
    layer: LayerId,
    canvas_size: (u32, u32),
    path: &Path,
    settings: &ToolSettings,
    foreground: [f32; 3],
    selection: Option<Selection>,
) -> Result<Filled, FillRefused> {
    let (bounds, surface) = pixel_target(layers, layer)?;
    let mode = fill_tools::mode_for(layers.lock(layer).unwrap_or_default())?;
    let shape = match aurora_doc::selection_from_path(path, canvas_size) {
        Ok(Some(shape)) => shape,
        Ok(None) => {
            return Err(FillRefused::Notice(LockRefusal::Path(
                NOTHING_TO_FILL_NOTICE,
            )));
        }
        Err(_) => return Err(FillRefused::Notice(LockRefusal::Path(FILL_FAILED_NOTICE))),
    };
    let shape_cover = selection_paint::layer_selection(Some(shape), bounds);
    let Some(shape_cover) = shape_cover else {
        return Err(FillRefused::Nothing);
    };
    let selection_cover = selection_paint::layer_selection(selection, bounds);
    let spec = ShapeFillSpec {
        colour: foreground,
        opacity: fill_tools::opacity_of(settings.brush_opacity),
        max_tiles: MAX_FILL_TILES,
    };
    let mut snapshot = StrokeSnapshot::new(surface);
    let outcome = aurora_brush::fill_with_coverage(
        store,
        surface,
        fill_tools::gradient_area(bounds, canvas_size),
        &shape_cover,
        &spec,
        mode,
        Some(&mut snapshot),
        selection_paint::as_coverage(selection_cover.as_ref()),
    )
    .map_err(|err| fill_tools::refusal(&err))?;
    for (tile, err) in outcome.failed() {
        tracing::warn!(?err, ?tile, "failed to fill part of a path");
    }
    Ok(Filled {
        snapshot,
        painted: outcome.painted().to_vec(),
    })
}

/// Strokes `path` on `layer` with the Brush's settings as one stroke.
///
/// # Errors
///
/// [`FillRefused::Notice`] for a lock or a path needing more dabs than
/// [`stroke_dab_cap`] allows (nothing is painted then).
pub(crate) fn stroke_path(
    store: &mut aurora_tile::TileStore,
    layers: &LayerTree,
    layer: LayerId,
    path: &Path,
    settings: &ToolSettings,
    foreground: [f32; 3],
    selection: Option<Selection>,
) -> Result<Filled, FillRefused> {
    let (bounds, surface) = pixel_target(layers, layer)?;
    let preserve_alpha =
        match crate::paint_permission(DabTool::Brush, layers.lock(layer).unwrap_or_default()) {
            crate::PaintPermission::Refused(why) => return Err(FillRefused::Notice(why)),
            crate::PaintPermission::PreserveAlpha => true,
            crate::PaintPermission::Allowed => false,
        };
    let radius = settings.dab_radius(DabTool::Brush);
    let flat: Vec<FlatSubpath> = path
        .subpaths
        .iter()
        .map(|s| (s.flatten(aurora_doc::FLATTEN_TOLERANCE), s.closed))
        .collect();
    let planned = path_dabs(
        &flat,
        dab_step(radius, DEFAULT_SPACING),
        stroke_dab_cap(radius),
    );
    if planned.truncated {
        return Err(FillRefused::Notice(LockRefusal::Path(TOO_MANY_DABS_NOTICE)));
    }
    let cover = selection_paint::layer_selection(selection, bounds);
    let amount = settings.paint_amount(DabTool::Brush);
    let mut snapshot = StrokeSnapshot::new(surface);
    let mut painted: Vec<aurora_tile::TileId> = Vec::new();
    let mut painted_seen: std::collections::HashSet<aurora_tile::TileId> =
        std::collections::HashSet::new();
    let mut warned: std::collections::HashSet<aurora_tile::TileId> =
        std::collections::HashSet::new();
    for dab in planned.dabs {
        let outcome = aurora_brush::stamp_dab_amount(
            store,
            surface,
            crate::layer_local_point(bounds, dab),
            radius,
            foreground,
            Some(&mut snapshot),
            selection_paint::as_coverage(cover.as_ref()),
            preserve_alpha,
            false,
            amount,
        );
        for tile in outcome.painted() {
            if painted_seen.insert(*tile) {
                painted.push(*tile);
            }
        }
        for (tile, err) in outcome.failed() {
            if warned.insert(*tile) {
                tracing::warn!(?err, ?tile, "failed to stamp part of a path stroke");
            }
        }
    }
    Ok(Filled { snapshot, painted })
}

/// Replaces the work path with the outline of `selection`, as the step to
/// record (`None` when the work path already is that outline).
///
/// # Errors
///
/// A calm sentence for a selection that covers nothing, one too complex for
/// the path caps, or a document that cannot hold another path.
pub(crate) fn make_work_path(
    paths: &mut PathSet,
    selection: &Selection,
    canvas: (u32, u32),
) -> Result<Option<PenStep>, &'static str> {
    let made = aurora_doc::path_from_selection(selection, canvas).map_err(|why| match why {
        aurora_doc::PathFromSelectionError::Empty => NOTHING_SELECTED_NOTICE,
        aurora_doc::PathFromSelectionError::TooComplex => SELECTION_TOO_COMPLEX_NOTICE,
    })?;
    crate::paths_app::edit_paths(paths, MAKE_WORK_PATH_LABEL, |set| {
        set.ensure_work()?;
        set.set_subpaths(WORK_PATH_NAME, made.subpaths)
    })
    .map_err(|_| SELECTION_TOO_COMPLEX_NOTICE)
}

impl App {
    /// Fill Path or Stroke Path with the active path on the active layer.
    pub(crate) fn run_path_paint(&mut self, command: &PathsCommand) {
        if self.quick_mask_refuses_command() {
            return;
        }
        let stroke = *command == PathsCommand::Stroke;
        let path = self
            .pen
            .active()
            .and_then(|name| self.doc.selection.paths().get(name))
            .cloned();
        let Some(path) = path else {
            self.lock_notice = Some((
                LockRefusal::Path(crate::paths_app::NO_PATH_NOTICE),
                self.doc.active_layer,
            ));
            self.sync_status_bar_now();
            return;
        };
        let (enough, notice) = if stroke {
            (path_can_stroke(&path), NEED_STROKE_POINTS_NOTICE)
        } else {
            (path_can_fill(&path), NEED_FILL_POINTS_NOTICE)
        };
        if !enough {
            self.lock_notice = Some((LockRefusal::Path(notice), self.doc.active_layer));
            self.sync_status_bar_now();
            return;
        }
        let Some(layer) = self.fill_target() else {
            return;
        };
        let selection = self.doc.selection.active();
        let foreground = crate::colour_state::fill_foreground(&self.colour.state);
        let canvas = self.doc.canvas_size;
        let Some(store) = self.doc.tile_store.as_mut() else {
            return;
        };
        let (result, label) = if stroke {
            (
                stroke_path(
                    store,
                    &self.doc.layers,
                    layer,
                    &path,
                    &self.tool_settings,
                    foreground,
                    selection,
                ),
                STROKE_PATH_LABEL,
            )
        } else {
            (
                fill_path(
                    store,
                    &self.doc.layers,
                    layer,
                    canvas,
                    &path,
                    &self.tool_settings,
                    foreground,
                    selection,
                ),
                FILL_PATH_LABEL,
            )
        };
        self.finish_fill(layer, result, label);
    }

    /// Make Work Path from Selection: ONE path step.
    pub(crate) fn run_make_work_path(&mut self) {
        if self.quick_mask_refuses_command() {
            return;
        }
        let Some(selection) = self.doc.selection.active() else {
            self.lock_notice = Some((
                LockRefusal::Path(NO_SELECTION_NOTICE),
                self.doc.active_layer,
            ));
            self.sync_status_bar_now();
            return;
        };
        let canvas = self.doc.canvas_size;
        match make_work_path(self.doc.selection.paths_mut(), &selection, canvas) {
            Ok(step) => {
                if let Some(step) = step {
                    crate::paths_app::record_step(&mut self.workspace, &mut self.doc, step);
                }
                self.pen.set_active(Some(WORK_PATH_NAME.to_owned()));
                self.pen.paths_replaced(self.doc.selection.paths());
            }
            Err(text) => {
                self.lock_notice = Some((LockRefusal::Path(text), self.doc.active_layer));
                self.sync_status_bar_now();
            }
        }
        self.needs_redraw = true;
    }
}

#[cfg(test)]
#[path = "path_paint_tests.rs"]
mod tests;
