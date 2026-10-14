//! The Pen tool's state machine (0.233.0): pure, no window, tested
//! headlessly. [`crate::paths_app`] is the `App` glue.
//!
//! **Gestures.** A press either grabs something that exists (a handle of
//! the selected anchor, then an anchor of the active path) or adds. Adding
//! to the subpath in progress: a click makes a corner anchor, a click-drag
//! a smooth one whose outgoing handle follows the pointer and whose
//! incoming handle mirrors it. A press within the close tolerance of the
//! in-progress subpath's first anchor closes it. A drag on an existing
//! anchor moves it (its handles with it); on a handle moves that handle (a
//! smooth anchor's other handle mirrors it, `Alt` breaks the mirror).
//! `Enter` ends an open subpath, `Escape` removes the one in progress,
//! `Backspace` its last anchor, `Delete` the selected anchor.
//!
//! **Steps.** Each completed gesture is one [`PenStep`] (the path set
//! before and after, with a label); a gesture that changed nothing gives
//! none. During a drag the path set is edited live and the step is made at
//! the release.

use aurora_doc::{
    Anchor, DocError, MAX_ANCHORS_PER_SUBPATH, MAX_PATH_ANCHORS_TOTAL, MAX_SUBPATHS, PathSet,
    Subpath, WORK_PATH_NAME,
};
use aurora_ui::{AnchorRef, PathPart};

use crate::document_session::DocumentId;

pub(crate) const ADD_POINT_LABEL: &str = "Pen: Add Point";
pub(crate) const MOVE_ANCHOR_LABEL: &str = "Move Anchor";
pub(crate) const MOVE_HANDLE_LABEL: &str = "Move Handle";
pub(crate) const CLOSE_PATH_LABEL: &str = "Close Path";
pub(crate) const DELETE_ANCHOR_LABEL: &str = "Delete Anchor";
pub(crate) const CANCEL_SUBPATH_LABEL: &str = "Cancel Subpath";
pub(crate) const REMOVE_POINT_LABEL: &str = "Remove Point";
/// A click on a segment inserts an anchor there (0.234.0).
pub(crate) const INSERT_POINT_LABEL: &str = "Add Point";
/// Alt-click on an anchor toggles corner and smooth (0.234.0).
pub(crate) const CONVERT_POINT_LABEL: &str = "Convert Point";

/// A completed gesture: what to record as one undo step.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PenStep {
    pub(crate) label: &'static str,
    pub(crate) before: PathSet,
    pub(crate) after: PathSet,
}

/// A path edit as [`crate::UndoOrder`] keeps it: the path set before and
/// after.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PathStep {
    pub(crate) before: PathSet,
    pub(crate) after: PathSet,
}

impl PenStep {
    /// The undo-stack form of this step and its History label.
    pub(crate) fn into_parts(self) -> (PathStep, &'static str) {
        (
            PathStep {
                before: self.before,
                after: self.after,
            },
            self.label,
        )
    }
}

/// What a Pen operation did.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PenOutcome {
    /// A finished gesture's step (none while a drag is live, or when the
    /// gesture changed nothing).
    pub(crate) step: Option<PenStep>,
    /// Why the operation was refused, as a calm notice.
    pub(crate) refused: Option<&'static str>,
    /// Whether anything on screen changed (the path, the selection mark).
    pub(crate) redraw: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// Dragging out the handles of the anchor just added.
    NewAnchor(AnchorRef),
    /// Moving an anchor; `grab` is the pointer's offset from the anchor.
    MoveAnchor { anchor: AnchorRef, grab: (f32, f32) },
    /// Moving a handle; `mirror` keeps the other handle opposite.
    MoveHandle {
        anchor: AnchorRef,
        out: bool,
        mirror: bool,
    },
}

/// The Pen tool's state.
#[derive(Debug, Clone, Default)]
pub(crate) struct PenState {
    /// The path the Pen edits (a saved path's name or [`WORK_PATH_NAME`]).
    active: Option<String>,
    /// The index of the open subpath still being drawn.
    drawing: Option<usize>,
    selected: Option<AnchorRef>,
    gesture: Option<Gesture>,
    /// The path set when the gesture began, and its label.
    before: Option<(PathSet, &'static str)>,
    /// The document the state belongs to.
    doc: Option<DocumentId>,
    /// `Enter` ended the last subpath: the next click starts a new one
    /// instead of continuing it (0.234.0). Cleared by a new anchor, an
    /// undo/redo and a change of active path.
    ended: bool,
}

/// The notice for a path set that reached the point limit.
pub(crate) const POINT_LIMIT_NOTICE: &str = "That path set has reached its point limit.";
/// The notice for a coordinate out of range.
pub(crate) const RANGE_NOTICE: &str = "That point is too far outside the document.";

/// Every anchor of every path of the set.
fn total_anchors(paths: &PathSet) -> usize {
    paths
        .saved()
        .iter()
        .chain(paths.work())
        .map(aurora_doc::Path::anchor_count)
        .sum()
}

/// The sentence for a failed release-time validation: a coordinate out of
/// range, or a cap.
pub(crate) fn release_notice(error: &DocError) -> &'static str {
    match error {
        DocError::InvalidPath(why) if why.contains("coordinate") => RANGE_NOTICE,
        _ => POINT_LIMIT_NOTICE,
    }
}

fn sub_mut<'a>(paths: &'a mut PathSet, name: &str) -> Option<&'a mut Vec<Subpath>> {
    paths.subpaths_mut(name)
}

fn anchor_mut(subs: &mut [Subpath], r: AnchorRef) -> Option<&mut Anchor> {
    subs.get_mut(r.subpath)?.anchors.get_mut(r.anchor)
}

fn sub(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (a.0 - b.0, a.1 - b.1)
}

impl PenState {
    /// The active path's name.
    pub(crate) fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// The selected anchor, for the overlay.
    pub(crate) fn selected(&self) -> Option<AnchorRef> {
        self.selected
    }

    /// Whether a gesture (a drag) is live.
    pub(crate) fn dragging(&self) -> bool {
        self.gesture.is_some()
    }

    /// The index of the open subpath still being drawn.
    pub(crate) fn drawing(&self) -> Option<usize> {
        self.drawing
    }

    /// Makes `name` (or nothing) the active path; the subpath in progress
    /// and the selected anchor are forgotten.
    pub(crate) fn set_active(&mut self, name: Option<String>) {
        if self.active != name {
            self.reset_editing();
            self.ended = false;
        }
        self.active = name;
    }

    /// Forgets the in-progress subpath, the selection and any live drag
    /// (a tool switch, an undo, another document), keeping the active path.
    pub(crate) fn reset_editing(&mut self) {
        self.drawing = None;
        self.selected = None;
        self.gesture = None;
        self.before = None;
    }

    /// Whether the state belongs to another document than `doc`; then it is
    /// cleared (the active path included).
    pub(crate) fn adopt_document(&mut self, doc: DocumentId) -> bool {
        if self.doc == Some(doc) {
            return false;
        }
        let had = self.active.is_some() || self.drawing.is_some() || self.gesture.is_some();
        *self = Self {
            doc: Some(doc),
            ..Self::default()
        };
        had
    }

    /// After the path set was replaced from outside (undo, redo): the
    /// active path stays if it still exists, else nothing is active.
    pub(crate) fn paths_replaced(&mut self, paths: &PathSet) {
        self.reset_editing();
        self.ended = false;
        if self
            .active
            .as_deref()
            .is_some_and(|n| paths.get(n).is_none())
        {
            self.active = None;
        }
    }

    fn begin(&mut self, paths: &PathSet, label: &'static str) {
        if self.before.is_none() {
            self.before = Some((paths.clone(), label));
        }
    }

    fn finish(&mut self, paths: &PathSet) -> Option<PenStep> {
        let (before, label) = self.before.take()?;
        (before != *paths).then(|| PenStep {
            label,
            before,
            after: paths.clone(),
        })
    }

    /// Makes sure there is a path to draw on: the active one, else the work
    /// path (created).
    fn ensure_active(&mut self, paths: &mut PathSet) -> Result<(), &'static str> {
        if self
            .active
            .as_deref()
            .is_some_and(|name| paths.get(name).is_some())
        {
            return Ok(());
        }
        paths
            .ensure_work()
            .map_err(|_| "This document already has the most paths it can hold.")?;
        self.active = Some(WORK_PATH_NAME.to_owned());
        self.drawing = None;
        Ok(())
    }

    /// A primary press at `point` (document px). `hit` is what
    /// `aurora_ui::path_hit` found under the pointer on the active path,
    /// `close_radius` the close tolerance in document px, `alt` whether Alt
    /// is held.
    #[allow(clippy::too_many_lines)]
    #[cfg(test)]
    pub(crate) fn press(
        &mut self,
        paths: &mut PathSet,
        point: (f32, f32),
        hit: Option<PathPart>,
        close_radius: f32,
        alt: bool,
    ) -> PenOutcome {
        self.press_at(paths, point, hit, None, close_radius, alt)
    }

    /// [`Self::press`] with `segment`, the segment of the active path under
    /// the pointer (not an anchor): a press on it, with no subpath in
    /// progress, inserts an anchor there.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn press_at(
        &mut self,
        paths: &mut PathSet,
        point: (f32, f32),
        hit: Option<PathPart>,
        segment: Option<aurora_doc::SegmentHit>,
        close_radius: f32,
        alt: bool,
    ) -> PenOutcome {
        let mut out = PenOutcome {
            redraw: true,
            ..PenOutcome::default()
        };
        self.gesture = None;
        self.before = None;
        // 1. A handle of the selected anchor, 2. closing, 3. an anchor.
        if let Some(PathPart::HandleIn(r) | PathPart::HandleOut(r)) = hit {
            let is_out = matches!(hit, Some(PathPart::HandleOut(_)));
            let Some(name) = self.active.clone() else {
                return out;
            };
            let mirror = !alt
                && sub_mut(paths, &name)
                    .and_then(|subs| anchor_mut(subs, r).map(|a| *a))
                    .is_some_and(|a| a.handle_in == (-a.handle_out.0, -a.handle_out.1));
            self.begin(paths, MOVE_HANDLE_LABEL);
            self.selected = Some(r);
            self.gesture = Some(Gesture::MoveHandle {
                anchor: r,
                out: is_out,
                mirror,
            });
            return out;
        }
        let closing = self
            .active
            .as_deref()
            .zip(self.drawing)
            .filter(|&(name, index)| {
                paths
                    .get(name)
                    .and_then(|path| path.subpaths.get(index))
                    .is_some_and(|open| {
                        open.anchors.len() >= 2
                            && open
                                .anchors
                                .first()
                                .is_some_and(|a| dist(a.point, point) <= close_radius)
                    })
            })
            .map(|(name, index)| (name.to_owned(), index));
        if let Some((name, index)) = closing {
            self.begin(paths, CLOSE_PATH_LABEL);
            if let Some(open) = sub_mut(paths, &name).and_then(|subs| subs.get_mut(index)) {
                open.closed = true;
            }
            self.drawing = None;
            self.selected = Some(AnchorRef {
                subpath: index,
                anchor: 0,
            });
            out.step = self.finish(paths);
            return out;
        }
        if let Some(PathPart::Anchor(r)) = hit {
            let Some(name) = self.active.clone() else {
                return out;
            };
            if alt {
                // Alt-click converts the anchor: corner <-> smooth.
                self.begin(paths, CONVERT_POINT_LABEL);
                if let Some(s) = sub_mut(paths, &name).and_then(|subs| subs.get_mut(r.subpath)) {
                    aurora_doc::toggle_corner_smooth(s, r.anchor);
                }
                self.selected = Some(r);
                out.step = self.finish(paths);
                return out;
            }
            if let Some(a) = sub_mut(paths, &name).and_then(|subs| anchor_mut(subs, r).map(|a| *a))
            {
                self.begin(paths, MOVE_ANCHOR_LABEL);
                self.selected = Some(r);
                self.gesture = Some(Gesture::MoveAnchor {
                    anchor: r,
                    grab: sub(point, a.point),
                });
                return out;
            }
        }
        // 4. Add an anchor.
        if total_anchors(paths) >= MAX_PATH_ANCHORS_TOTAL {
            out.refused = Some(POINT_LIMIT_NOTICE);
            return out;
        }
        // A press on a segment of the active path splits it.
        if let (Some(hit), None, None, Some(name)) =
            (segment, hit, self.drawing, self.active.clone())
        {
            self.begin(paths, INSERT_POINT_LABEL);
            let inserted = sub_mut(paths, &name)
                .and_then(|subs| aurora_doc::insert_anchor(subs, &hit).map(|at| (subs, at)))
                .map(|(_, at)| at);
            if let Some(at) = inserted {
                self.selected = Some(AnchorRef {
                    subpath: hit.subpath,
                    anchor: at,
                });
                out.step = self.finish(paths);
                return out;
            }
            self.before = None;
        }
        if let Err(why) = self.ensure_active_for_add(paths) {
            out.refused = Some(why);
            return out;
        }
        let Some(name) = self.active.clone() else {
            return out;
        };
        self.begin(paths, ADD_POINT_LABEL);
        let drawing = self.drawing;
        let Some(subs) = sub_mut(paths, &name) else {
            return out;
        };
        // The subpath in progress, else (unless Enter ended it) the path's
        // last subpath when it is still open: after an undo or a change of
        // path the next click carries on drawing it.
        let resume = (!self.ended)
            .then(|| subs.len().checked_sub(1))
            .flatten()
            .filter(|&i| subs.get(i).is_some_and(|s| !s.closed));
        let index = if let Some(i) = drawing
            .filter(|&i| subs.get(i).is_some_and(|s| !s.closed))
            .or(resume)
        {
            i
        } else {
            if subs.len() >= MAX_SUBPATHS {
                self.before = None;
                out.refused = Some("This path already has the most subpaths it can hold.");
                return out;
            }
            subs.push(Subpath::default());
            subs.len() - 1
        };
        let Some(open) = subs.get_mut(index) else {
            return out;
        };
        if open.anchors.len() >= MAX_ANCHORS_PER_SUBPATH {
            self.before = None;
            out.refused = Some("This subpath already has the most points it can hold.");
            return out;
        }
        open.anchors.push(Anchor::corner(point.0, point.1));
        let anchor = AnchorRef {
            subpath: index,
            anchor: open.anchors.len() - 1,
        };
        self.drawing = Some(index);
        self.ended = false;
        self.selected = Some(anchor);
        self.gesture = Some(Gesture::NewAnchor(anchor));
        out
    }

    fn ensure_active_for_add(&mut self, paths: &mut PathSet) -> Result<(), &'static str> {
        // The work path is created inside the same step: `before` is taken
        // after this, so capture the set first when it will change.
        let creates = self
            .active
            .as_deref()
            .is_none_or(|name| paths.get(name).is_none());
        if creates && self.before.is_none() {
            self.before = Some((paths.clone(), ADD_POINT_LABEL));
        }
        let result = self.ensure_active(paths);
        if result.is_err() {
            self.before = None;
        }
        result
    }

    /// The pointer moved with the button down at `point`.
    pub(crate) fn drag(&mut self, paths: &mut PathSet, point: (f32, f32)) -> bool {
        let (Some(gesture), Some(name)) = (self.gesture, self.active.clone()) else {
            return false;
        };
        let Some(subs) = sub_mut(paths, &name) else {
            return false;
        };
        match gesture {
            Gesture::NewAnchor(r) => {
                if let Some(a) = anchor_mut(subs, r) {
                    let out = sub(point, a.point);
                    a.handle_out = out;
                    a.handle_in = (-out.0, -out.1);
                }
            }
            Gesture::MoveAnchor { anchor, grab } => {
                if let Some(a) = anchor_mut(subs, anchor) {
                    a.point = (point.0 - grab.0, point.1 - grab.1);
                }
            }
            Gesture::MoveHandle {
                anchor,
                out,
                mirror,
            } => {
                if let Some(a) = anchor_mut(subs, anchor) {
                    let offset = sub(point, a.point);
                    if out {
                        a.handle_out = offset;
                        if mirror {
                            a.handle_in = (-offset.0, -offset.1);
                        }
                    } else {
                        a.handle_in = offset;
                        if mirror {
                            a.handle_out = (-offset.0, -offset.1);
                        }
                    }
                }
            }
        }
        true
    }

    /// The button was released: the gesture's step, validated. A path that
    /// no longer validates (a coordinate out of range) is put back to what
    /// it was and a notice is returned.
    pub(crate) fn release(&mut self, paths: &mut PathSet) -> PenOutcome {
        let mut out = PenOutcome {
            redraw: true,
            ..PenOutcome::default()
        };
        if self.gesture.take().is_none() {
            return out;
        }
        if let Err(error) = paths.validate() {
            if let Some((before, _)) = self.before.take() {
                *paths = before;
            }
            self.reset_editing();
            out.refused = Some(release_notice(&error));
            return out;
        }
        out.step = self.finish(paths);
        out
    }

    /// `Enter`: ends the open subpath, so the next click starts a new one.
    /// No document change, no step.
    pub(crate) fn end_subpath(&mut self) -> bool {
        let was = self.drawing.take().is_some();
        self.ended |= was;
        was
    }

    /// `Escape`: removes the subpath in progress (if any), as one step.
    pub(crate) fn cancel_subpath(&mut self, paths: &mut PathSet) -> PenOutcome {
        let mut out = PenOutcome {
            redraw: true,
            ..PenOutcome::default()
        };
        let (Some(index), Some(name)) = (self.drawing.take(), self.active.clone()) else {
            return out;
        };
        self.gesture = None;
        self.before = None;
        self.begin(paths, CANCEL_SUBPATH_LABEL);
        if let Some(subs) = sub_mut(paths, &name)
            && index < subs.len()
        {
            subs.remove(index);
        }
        self.selected = None;
        out.step = self.finish(paths);
        out
    }

    /// `Backspace`: removes the last anchor of the subpath in progress.
    pub(crate) fn remove_last_point(&mut self, paths: &mut PathSet) -> PenOutcome {
        let mut out = PenOutcome {
            redraw: true,
            ..PenOutcome::default()
        };
        let (Some(index), Some(name)) = (self.drawing, self.active.clone()) else {
            return out;
        };
        self.before = None;
        self.begin(paths, REMOVE_POINT_LABEL);
        if let Some(subs) = sub_mut(paths, &name)
            && let Some(open) = subs.get_mut(index)
        {
            open.anchors.pop();
            if open.anchors.is_empty() {
                subs.remove(index);
                self.drawing = None;
                self.selected = None;
            } else {
                self.selected = Some(AnchorRef {
                    subpath: index,
                    anchor: open.anchors.len() - 1,
                });
            }
        }
        out.step = self.finish(paths);
        out
    }

    /// `Delete`: removes the selected anchor.
    pub(crate) fn delete_selected(&mut self, paths: &mut PathSet) -> PenOutcome {
        match self.selected {
            Some(r) => self.delete_anchor(paths, r),
            None => PenOutcome::default(),
        }
    }

    /// `Delete` with no anchor selected: removes the anchor under the
    /// pointer (`hovered`), as one step.
    pub(crate) fn delete_anchor(&mut self, paths: &mut PathSet, r: AnchorRef) -> PenOutcome {
        let mut out = PenOutcome::default();
        let Some(name) = self.active.clone() else {
            return out;
        };
        out.redraw = true;
        self.gesture = None;
        self.before = None;
        self.begin(paths, DELETE_ANCHOR_LABEL);
        if let Some(subs) = sub_mut(paths, &name)
            && let Some(s) = subs.get_mut(r.subpath)
            && r.anchor < s.anchors.len()
        {
            s.anchors.remove(r.anchor);
            if s.anchors.len() < 2 {
                s.closed = false;
            }
            if s.anchors.is_empty() {
                subs.remove(r.subpath);
                match self.drawing {
                    Some(d) if d == r.subpath => self.drawing = None,
                    Some(d) if d > r.subpath => self.drawing = Some(d - 1),
                    _ => {}
                }
            }
        }
        self.selected = None;
        out.step = self.finish(paths);
        out
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

#[cfg(test)]
#[path = "pen_tests.rs"]
mod tests;
