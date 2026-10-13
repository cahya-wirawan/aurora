//! The Lasso, Polygonal Lasso and Magic Wand (0.214.0): the document half.
//! These are pure functions over points, a view and the live document
//! parts (never `App`), so a test drives them without a window; `lib.rs`
//! owns the pointer and key plumbing (`Drag::Lasso`, `App::select_wand`).
//!
//! **Lasso.** A press deselects (the "before" is kept, as the Marquee's),
//! the pointer's path is recorded in document space (a point closer than
//! [`LASSO_MIN_STEP`] to the last one is dropped, and at most
//! [`MAX_LASSO_POINTS`] are kept), and the live selection is the
//! antialiased mask of the closed path ([`aurora_doc::polygon_mask`],
//! nonzero winding, clipped to the document). The mask is rebuilt on a
//! cadence that thins out as the path's box grows ([`rebuild_period`]) so a
//! big lasso does not cost a whole-box rasterisation per pointer move; the
//! release always rebuilds. A drag ending any other way keeps the
//! selection as it was last built. One undo step, "Lasso".
//!
//! **Polygonal Lasso.** The same drag state, but each click adds a vertex
//! and the pointer's release does not end it. A click within
//! [`POLYGON_CLOSE_TOLERANCE_PX`] screen pixels of the first vertex (with at
//! least three) or of the last one (a double click lands there) closes the
//! polygon, as does Enter; Backspace removes the last vertex; Escape
//! cancels. The live selection is the polygon closed through the vertices
//! so far once there are three. One undo step, "Polygonal Lasso".
//!
//! **Magic Wand.** A click selects the pixels of the active pixel layer
//! similar to the clicked one ([`aurora_brush::wand_region`]: the Paint
//! Bucket's similarity rule, tolerance and caps), softened one texel
//! ([`aurora_doc::antialiased_mask`]). The composite is never sampled.

use aurora_core::Rect;
use aurora_doc::{LayerKind, LayerTree, Selection};
use aurora_tile::TileStore;

use aurora_widgets::shortcut::{Key, NamedKey};

use crate::selection_modes::ModeGesture;
use crate::{Drag, LockRefusal, PointerButton, ToolSettings};

/// The History row of a Lasso drag.
pub(crate) const LASSO_LABEL: &str = "Lasso";
/// The History row of a finished Polygonal Lasso.
pub(crate) const POLYGONAL_LASSO_LABEL: &str = "Polygonal Lasso";
/// The History row of a Magic Wand click.
pub(crate) const MAGIC_WAND_LABEL: &str = "Magic Wand";

/// A freehand point closer than this to the previous one (document pixels)
/// adds nothing.
pub(crate) const LASSO_MIN_STEP: f32 = 1.0;

/// The most points a freehand path keeps (further moves are ignored).
pub(crate) const MAX_LASSO_POINTS: usize = 50_000;

/// How close (in logical screen pixels) a click must be to the first or
/// last vertex to close a Polygonal Lasso: an interaction tolerance, the
/// same footing as the panel drag threshold, not a style value.
pub(crate) const POLYGON_CLOSE_TOLERANCE_PX: f32 = 6.0;

/// The path box area (document pixels) per extra pointer move between
/// rebuilds of the live mask.
const REBUILD_AREA_STEP: f64 = 262_144.0;

/// Points per extra pointer move between rebuilds: a rebuild costs
/// `O(points log points)` (the edges are sorted and every sub-scanline
/// sorts its crossings), so a long scribble inside a small box would
/// otherwise cost `O(points^2 log points)` over the drag.
const REBUILD_POINTS_STEP: usize = 512;

/// The History row of a finished lasso: `"Deselect"` when it selected
/// nothing (a click, a degenerate path).
pub(crate) fn lasso_step_label(polygonal: bool, selected: bool) -> &'static str {
    match (selected, polygonal) {
        (false, _) => "Deselect",
        (true, false) => LASSO_LABEL,
        (true, true) => POLYGONAL_LASSO_LABEL,
    }
}

/// The document as a clip rectangle.
pub(crate) fn document_clip(size: (u32, u32)) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    }
}

/// Adds `point` to a freehand path unless it is too close to the last one
/// or the path is full; whether it was added.
pub(crate) fn push_lasso_point(points: &mut Vec<(f32, f32)>, point: (f32, f32)) -> bool {
    if !(point.0.is_finite() && point.1.is_finite()) || points.len() >= MAX_LASSO_POINTS {
        return false;
    }
    if let Some(last) = points.last() {
        let (dx, dy) = (point.0 - last.0, point.1 - last.1);
        if dx * dx + dy * dy < LASSO_MIN_STEP * LASSO_MIN_STEP {
            return false;
        }
    }
    points.push(point);
    true
}

/// After how many points the live mask is rebuilt: every point for a small
/// path, every `1 + area / 262,144 + points / 512`th as the path's box
/// and point count grow.
pub(crate) fn rebuild_period(points: &[(f32, f32)]) -> usize {
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for &(x, y) in points {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    if lo.0 > hi.0 {
        return 1;
    }
    let area = f64::from(hi.0 - lo.0) * f64::from(hi.1 - lo.1);
    // A non-negative, finite quotient.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let extra = (area / REBUILD_AREA_STEP) as usize;
    1 + extra + points.len() / REBUILD_POINTS_STEP
}

/// A live rebuild that took longer than this makes the next ones wait.
pub(crate) const SLOW_REBUILD: std::time::Duration = std::time::Duration::from_millis(8);

/// After a slow rebuild, how many times its duration to wait before the
/// next (so rebuilding takes at most a quarter of the drag's time).
const SLOW_REBUILD_BACKOFF: u32 = 3;

/// When the next live rebuild may run after one that began at `now` and
/// took `took`: `None` (right away) for a quick one.
pub(crate) fn backoff_until(
    now: std::time::Instant,
    took: std::time::Duration,
) -> Option<std::time::Instant> {
    (took > SLOW_REBUILD)
        .then(|| now.checked_add(took.saturating_mul(SLOW_REBUILD_BACKOFF)))
        .flatten()
}

/// One pointer move's chance to rebuild the live mask of a freehand path:
/// when it is due by count ([`rebuild_due`]) and any back-off after a slow
/// rebuild has passed. The release never goes through this: it always
/// rebuilds.
pub(crate) fn live_rebuild_step(
    points: &[(f32, f32)],
    clip: &Rect,
    modal: &ModeGesture,
    selection: &mut aurora_doc::SelectionSet,
    current: &mut Option<Selection>,
    not_before: &mut Option<std::time::Instant>,
) {
    if !rebuild_due(points) {
        return;
    }
    let started = std::time::Instant::now();
    if not_before.is_some_and(|t| started < t) {
        return;
    }
    rebuild_live(points, clip, modal, selection, current);
    *not_before = backoff_until(started, started.elapsed());
}

/// Whether the live mask is due for a rebuild after the path grew to
/// `points`.
pub(crate) fn rebuild_due(points: &[(f32, f32)]) -> bool {
    points.len().is_multiple_of(rebuild_period(points))
}

/// The mask selection of the closed path through `points`, clipped to
/// `clip`: `Ok(None)` for nothing (fewer than three points, a degenerate
/// or off-document path).
///
/// # Errors
///
/// [`aurora_doc::DocError`] over the vertex or area caps.
pub(crate) fn path_selection(
    points: &[(f32, f32)],
    clip: &Rect,
) -> Result<Option<Selection>, aurora_doc::DocError> {
    if points.len() < 3 {
        return Ok(None);
    }
    let wide: Vec<(f64, f64)> = points
        .iter()
        .map(|&(x, y)| (f64::from(x), f64::from(y)))
        .collect();
    Ok(aurora_doc::polygon_mask(&wide, clip)?
        .map(|mask| Selection::from_mask(std::sync::Arc::new(mask))))
}

/// Rebuilds the live selection from `points` into `selection` and
/// `current`, combined by the gesture's mode (0.215.0). A path the caps
/// refuse, or a combination the mask cap refuses, leaves both unchanged.
pub(crate) fn rebuild_live(
    points: &[(f32, f32)],
    clip: &Rect,
    modal: &ModeGesture,
    selection: &mut aurora_doc::SelectionSet,
    current: &mut Option<Selection>,
) {
    let shape = match path_selection(points, clip) {
        Ok(shape) => shape,
        Err(err) => {
            tracing::debug!(
                ?err,
                "a lasso path is over a cap; the selection is unchanged"
            );
            return;
        }
    };
    if modal.place(selection, shape.as_ref()) {
        *current = selection.active();
    }
}

/// Whether a click at screen point `click` closes a polygon with
/// `vertices` (document points): it lands within the tolerance of the
/// first vertex (three or more) or of the last one (two or more; the second
/// click of a double click).
pub(crate) fn click_closes_polygon(
    vertices: &[(f32, f32)],
    click: (f32, f32),
    view: &aurora_ui::CanvasView,
) -> bool {
    let near = |v: (f32, f32)| {
        let s = view.to_screen(v);
        let (dx, dy) = (s.0 - click.0, s.1 - click.1);
        dx * dx + dy * dy <= POLYGON_CLOSE_TOLERANCE_PX * POLYGON_CLOSE_TOLERANCE_PX
    };
    let first = vertices.len() >= 3 && vertices.first().is_some_and(|v| near(*v));
    let last = vertices.len() >= 2 && vertices.last().is_some_and(|v| near(*v));
    first || last
}

/// Why a Magic Wand click selected nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WandRefused {
    /// A calm status-bar notice.
    Notice(LockRefusal),
    /// Nothing to say (the click is outside the layer, a tile is
    /// unreadable).
    Nothing,
}

/// One Magic Wand click at `doc_point` on `layer`: the selection of the
/// similar pixels (never empty), or why not.
///
/// # Errors
///
/// [`WandRefused`].
pub(crate) fn wand_select(
    store: &mut TileStore,
    layers: &LayerTree,
    layer: aurora_doc::LayerId,
    doc_point: (f32, f32),
    settings: &ToolSettings,
    document_size: (u32, u32),
) -> Result<Selection, WandRefused> {
    let Some(LayerKind::Pixel { bounds }) = layers.kind(layer).cloned() else {
        return Err(WandRefused::Notice(LockRefusal::WandNeedsPixelLayer));
    };
    let Some(surface) = layers.surface_id(layer) else {
        return Err(WandRefused::Notice(LockRefusal::WandNeedsPixelLayer));
    };
    let local = crate::layer_local_point(bounds, doc_point);
    if !(local.0.is_finite() && local.1.is_finite()) || local.0 < 0.0 || local.1 < 0.0 {
        return Err(WandRefused::Nothing);
    }
    // Non-negative and finite here.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let seed = (local.0.floor() as u32, local.1.floor() as u32);
    let spec = aurora_brush::FloodSpec {
        seed,
        tolerance: settings.tolerance,
        contiguous: settings.contiguous,
        max_tiles: aurora_brush::MAX_FILL_TILES,
        max_examined: aurora_brush::MAX_EXAMINED_TILES,
    };
    let region =
        match aurora_brush::wand_region(store, surface, (bounds.width, bounds.height), &spec) {
            Ok(Some(region)) => region,
            Ok(None) => return Err(WandRefused::Nothing),
            Err(aurora_brush::FillError::TooLarge { .. }) => {
                return Err(WandRefused::Notice(LockRefusal::SelectionTooLarge));
            }
            Err(aurora_brush::FillError::Tile(err)) => {
                tracing::warn!(?err, "the Magic Wand could not read a tile");
                return Err(WandRefused::Nothing);
            }
        };
    // Layer-local to document space.
    let doc_bounds = Rect {
        x: region.bounds.x.saturating_add(bounds.x),
        y: region.bounds.y.saturating_add(bounds.y),
        width: region.bounds.width,
        height: region.bounds.height,
    };
    // Clipped to the document, like the lassos (a layer may reach past the
    // canvas; nothing there can be painted or seen).
    let Some((doc_bounds, data)) =
        crop_binary(doc_bounds, &region.data, &document_clip(document_size))
    else {
        return Err(WandRefused::Nothing);
    };
    match aurora_doc::antialiased_mask(doc_bounds, &data) {
        Ok(Some(mask)) => Ok(Selection::from_mask(std::sync::Arc::new(mask))),
        Err(aurora_doc::DocError::SelectionMaskTooLarge(_)) => {
            Err(WandRefused::Notice(LockRefusal::SelectionTooLarge))
        }
        Ok(None) | Err(_) => Err(WandRefused::Nothing),
    }
}

/// A Lasso press as the app runs it: in New mode the press deselects at
/// once, in a combining mode the selection stays (either way the old
/// selection is kept as the gesture's "before"), and the mask will be
/// clipped to the document.
pub(crate) fn press_lasso(
    drag: &mut Option<crate::Drag>,
    selection: &mut aurora_doc::SelectionSet,
    document_size: (u32, u32),
    mode: aurora_doc::SelectionMode,
) {
    let (gesture, began_with) = crate::selection_modes::press_selection(selection, mode);
    if let Some(crate::Drag::Lasso {
        before,
        clip,
        current,
        modal,
        ..
    }) = drag.as_mut()
    {
        *before = began_with;
        *clip = document_clip(document_size);
        *current = crate::selection_modes::initial_current(&gesture);
        **modal = gesture;
    }
}

/// A pointer release: a Polygonal Lasso outlives it (the drag stays in
/// `slot`, nothing is returned); a freehand lasso gets its final exact
/// mask and is returned to be committed.
pub(crate) fn release_lasso(
    slot: &mut Option<crate::Drag>,
    mut ending: Option<crate::Drag>,
    selection: &mut aurora_doc::SelectionSet,
) -> Option<crate::Drag> {
    match ending.as_mut() {
        Some(crate::Drag::Lasso {
            polygonal: true, ..
        }) => {
            *slot = ending.take();
        }
        Some(crate::Drag::Lasso {
            points,
            clip,
            current,
            modal,
            ..
        }) => rebuild_live(points, clip, modal, selection, current),
        _ => {}
    }
    ending
}

/// What a click did to a Polygonal Lasso in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolygonClick {
    /// The click closes the polygon: the caller commits the drag.
    Closes,
    /// A vertex was added (or the click was too close to the last one).
    Added,
}

/// A click of the Polygonal Lasso at `canvas_point` on a polygon in
/// progress.
pub(crate) fn polygon_click(
    drag: &mut crate::Drag,
    canvas_point: (f32, f32),
    view: &aurora_ui::CanvasView,
    selection: &mut aurora_doc::SelectionSet,
) -> PolygonClick {
    let crate::Drag::Lasso {
        points,
        clip,
        current,
        modal,
        ..
    } = drag
    else {
        return PolygonClick::Added;
    };
    if click_closes_polygon(points, canvas_point, view) {
        return PolygonClick::Closes;
    }
    if push_lasso_point(points, view.to_document(canvas_point)) {
        rebuild_live(points, clip, modal, selection, current);
    }
    PolygonClick::Added
}

/// Backspace on a polygon in progress: removes the last vertex; `true`
/// when none is left (the caller cancels the gesture).
pub(crate) fn polygon_backspace(
    drag: &mut crate::Drag,
    selection: &mut aurora_doc::SelectionSet,
) -> bool {
    let crate::Drag::Lasso {
        points,
        clip,
        current,
        modal,
        ..
    } = drag
    else {
        return false;
    };
    points.pop();
    if points.is_empty() {
        return true;
    }
    rebuild_live(points, clip, modal, selection, current);
    false
}

/// One Magic Wand click as a recorded selection step ("Magic Wand"): the
/// selection of the similar pixels replaces the current one, the step is
/// recorded and the History panel refreshed. `Ok(true)` when it recorded
/// a step, `Ok(false)` when the click selected exactly what was already
/// selected.
///
/// # Errors
///
/// [`WandRefused`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn wand_step(
    workspace: &mut aurora_ui::Workspace,
    undo_order: &mut crate::UndoOrder,
    history: &mut aurora_doc::History,
    pixel_history: &mut aurora_brush::PixelHistory,
    selection: &mut aurora_doc::SelectionSet,
    store: &mut TileStore,
    layers: &LayerTree,
    layer: aurora_doc::LayerId,
    doc_point: (f32, f32),
    settings: &ToolSettings,
    document_size: (u32, u32),
    mode: aurora_doc::SelectionMode,
) -> Result<bool, WandRefused> {
    let picked = wand_select(store, layers, layer, doc_point, settings, document_size)?;
    let before = selection.active();
    // The click's pixels combine with the selection by the mode in force
    // at the press (0.215.0); New replaces.
    let combined = aurora_doc::combine_selections(before.as_ref(), &picked, mode)
        .map_err(|_| WandRefused::Notice(LockRefusal::SelectionTooLarge))?;
    match combined {
        Some(next) => selection.select(next),
        None => selection.deselect(),
    }
    Ok(crate::record_selection_step(
        workspace,
        undo_order,
        history,
        pixel_history,
        crate::selection_modes::step_label(mode, MAGIC_WAND_LABEL),
        before,
        selection.active(),
    ))
}

/// The part of the binary grid `data` (one byte per texel of `bounds`)
/// inside `clip`, with its bounds; `None` when they do not meet.
pub(crate) fn crop_binary(bounds: Rect, data: &[u8], clip: &Rect) -> Option<(Rect, Vec<u8>)> {
    let x0 = bounds.x.max(clip.x);
    let y0 = bounds.y.max(clip.y);
    let x1 = bounds.right().min(clip.right());
    let y1 = bounds.bottom().min(clip.bottom());
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let width = usize::try_from(bounds.width).ok()?;
    let (cw, ch) = (
        usize::try_from(x1 - x0).ok()?,
        usize::try_from(y1 - y0).ok()?,
    );
    let (ox, oy) = (
        usize::try_from(x0 - bounds.x).ok()?,
        usize::try_from(y0 - bounds.y).ok()?,
    );
    let mut out = Vec::with_capacity(cw.saturating_mul(ch));
    for row in oy..oy + ch {
        out.extend_from_slice(data.get(row * width + ox..row * width + ox + cw)?);
    }
    Some((
        Rect {
            x: x0,
            y: y0,
            width: u32::try_from(cw).ok()?,
            height: u32::try_from(ch).ok()?,
        },
        out,
    ))
}

/// Takes the live selection gesture (a Marquee or a Lasso) out of `drag`
/// without recording it and returns what was selected when it began
/// (`Some(None)` = nothing); `None` when no such gesture is live.
// `Some(None)` (began with nothing selected) differs from `None`.
#[allow(clippy::option_option)]
pub(crate) fn take_selection_drag(
    drag: &mut Option<Drag>,
) -> Option<Option<aurora_doc::Selection>> {
    if !matches!(drag, Some(Drag::Marquee { .. } | Drag::Lasso { .. })) {
        return None;
    }
    match drag.take() {
        Some(Drag::Marquee { before, .. } | Drag::Lasso { before, .. }) => Some(before),
        _ => None,
    }
}

/// Cancels a live selection gesture: the selection goes back to what it
/// was when the gesture began and nothing is recorded. Whether there was
/// one.
pub(crate) fn cancel_selection_gesture(
    drag: &mut Option<Drag>,
    selection: &mut aurora_doc::SelectionSet,
) -> bool {
    match take_selection_drag(drag) {
        Some(before) => {
            crate::selection_view::cancel_marquee(selection, before);
            true
        }
        None => false,
    }
}

/// Whether `drag` is a Polygonal Lasso in progress.
pub(crate) fn is_polygon(drag: Option<&Drag>) -> bool {
    matches!(
        drag,
        Some(Drag::Lasso {
            polygonal: true,
            ..
        })
    )
}

/// What a key did to a Polygonal Lasso in progress.
#[derive(Debug)]
pub(crate) enum PolygonKey {
    /// Not a polygon key (or no polygon, or another tool): the key goes on.
    Ignored,
    /// Used (Backspace, or Escape cancelling): nothing to commit.
    Handled,
    /// Enter: the polygon closes; the caller commits this drag.
    Close(Option<Drag>),
}

/// Enter closes, Backspace removes the last vertex (the only vertex
/// cancels) and Escape cancels a Polygonal Lasso in progress while the
/// Polygonal Lasso is the active tool. The caller offers keys to focused
/// widgets first, so a text field or a button keeps its own Enter and
/// Backspace.
pub(crate) fn polygon_key(
    drag: &mut Option<Drag>,
    tool: aurora_ui::Tool,
    key: Key,
    selection: &mut aurora_doc::SelectionSet,
) -> PolygonKey {
    if tool != aurora_ui::Tool::PolygonalLasso || !is_polygon(drag.as_ref()) {
        return PolygonKey::Ignored;
    }
    match key {
        Key::Named(NamedKey::Enter) => PolygonKey::Close(drag.take()),
        Key::Named(NamedKey::Escape) => {
            cancel_selection_gesture(drag, selection);
            PolygonKey::Handled
        }
        Key::Named(NamedKey::Backspace) => {
            let emptied = drag
                .as_mut()
                .is_some_and(|d| polygon_backspace(d, selection));
            if emptied {
                cancel_selection_gesture(drag, selection);
            }
            PolygonKey::Handled
        }
        _ => PolygonKey::Ignored,
    }
}

/// What a press did with a Polygonal Lasso in progress.
#[derive(Debug)]
pub(crate) enum PolygonPress {
    /// Not a click on a polygon in progress: the press goes on.
    NotApplicable,
    /// A vertex was added.
    Added,
    /// The click closed the polygon; the caller commits this drag.
    Closes(Option<Drag>),
}

/// A press while a Polygonal Lasso is in progress and its tool is active
/// adds a vertex or closes (primary button only).
pub(crate) fn polygon_press(
    drag: &mut Option<Drag>,
    tool: aurora_ui::Tool,
    button: PointerButton,
    canvas_point: (f32, f32),
    view: &aurora_ui::CanvasView,
    selection: &mut aurora_doc::SelectionSet,
) -> PolygonPress {
    if tool != aurora_ui::Tool::PolygonalLasso
        || button != PointerButton::Primary
        || !is_polygon(drag.as_ref())
    {
        return PolygonPress::NotApplicable;
    }
    let closes = drag
        .as_mut()
        .is_some_and(|d| polygon_click(d, canvas_point, view, selection) == PolygonClick::Closes);
    if closes {
        PolygonPress::Closes(drag.take())
    } else {
        PolygonPress::Added
    }
}

/// A tool switch ends a live Marquee or Lasso gesture by recording it
/// (the Marquee-drag rule): the drag to commit, when the tool really
/// changed.
pub(crate) fn take_on_tool_switch(
    drag: &mut Option<Drag>,
    from: aurora_ui::Tool,
    to: aurora_ui::Tool,
) -> Option<Drag> {
    if from != to && matches!(drag, Some(Drag::Marquee { .. } | Drag::Lasso { .. })) {
        drag.take()
    } else {
        None
    }
}

/// The pointer leaving the window ends a drag, except a Polygonal Lasso
/// (clicked vertices, no button held): that one stays in progress.
pub(crate) fn take_on_cursor_left(drag: &mut Option<Drag>) -> Option<Drag> {
    if is_polygon(drag.as_ref()) {
        None
    } else {
        drag.take()
    }
}
