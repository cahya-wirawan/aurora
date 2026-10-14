//! Paths (0.233.0): Bezier outlines kept with the document, the data side
//! of the Pen tool and the Paths panel.
//!
//! A [`Path`] is a name and a list of [`Subpath`]s; a subpath is a list of
//! [`Anchor`]s (a point plus the offsets of its two control handles, zero
//! meaning "no handle") and whether it is closed. Coordinates are document
//! pixels in `f32`. A [`PathSet`] is a document's paths: named paths plus an
//! optional unsaved **work path** (named [`WORK_PATH_NAME`]).
//!
//! Everything here is pure data with bounds: finite coordinates within
//! [`MAX_PATH_COORDINATE`], at most [`MAX_ANCHORS_PER_SUBPATH`] anchors per
//! subpath, [`MAX_SUBPATHS`] subpaths per path, [`MAX_PATHS`] paths and
//! [`MAX_PATH_ANCHORS_TOTAL`] anchors per document; unique non-empty names
//! of at most [`PATH_NAME_MAX_CHARS`] characters. [`Path::flatten`] turns
//! the curves into polylines within [`FLATTEN_TOLERANCE`] and
//! [`selection_from_path`] rasterises a path with the Lasso's nonzero-winding
//! polygon filler.

use std::sync::Arc;

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection::{Selection, validate_selection_name};
use crate::selection_build::{MAX_POLYGON_VERTICES, polygon_mask};

/// Coordinates and handle offsets must lie within `±MAX_PATH_COORDINATE`.
pub const MAX_PATH_COORDINATE: f32 = 1.0e7;
/// The most anchors one subpath may hold.
pub const MAX_ANCHORS_PER_SUBPATH: usize = 10_000;
/// The most subpaths one path may hold.
pub const MAX_SUBPATHS: usize = 256;
/// The most paths (saved ones plus the work path) one document may hold.
pub const MAX_PATHS: usize = 64;
/// The most anchors all the paths of a document may hold together (an
/// engineering default that bounds memory and the `.aur` entry).
pub const MAX_PATH_ANCHORS_TOTAL: usize = 200_000;
/// The most characters in a path's name.
pub const PATH_NAME_MAX_CHARS: usize = 64;
/// The flattening tolerance in document pixels.
pub const FLATTEN_TOLERANCE: f32 = 0.1;
/// The most line segments one curve is flattened into.
pub const MAX_SEGMENTS_PER_CURVE: u32 = 512;
/// The name of the unsaved work path.
pub const WORK_PATH_NAME: &str = "Work Path";

/// A point of a [`Subpath`] with its two control handles as offsets from
/// the point (`handle_in` toward the previous anchor, `handle_out` toward the
/// next). A zero offset is no handle.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Anchor {
    pub point: (f32, f32),
    pub handle_in: (f32, f32),
    pub handle_out: (f32, f32),
}

impl Anchor {
    /// A corner anchor (no handles) at `(x, y)`.
    #[must_use]
    pub const fn corner(x: f32, y: f32) -> Self {
        Self {
            point: (x, y),
            handle_in: (0.0, 0.0),
            handle_out: (0.0, 0.0),
        }
    }

    /// A smooth anchor at `point` whose outgoing handle is `out` and whose
    /// incoming handle mirrors it.
    #[must_use]
    pub const fn smooth(point: (f32, f32), out: (f32, f32)) -> Self {
        Self {
            point,
            handle_in: (-out.0, -out.1),
            handle_out: out,
        }
    }

    /// Whether the anchor has no handle at all.
    #[must_use]
    pub fn is_corner(&self) -> bool {
        self.handle_in == (0.0, 0.0) && self.handle_out == (0.0, 0.0)
    }

    /// The absolute position of the incoming handle.
    #[must_use]
    pub fn in_position(&self) -> (f32, f32) {
        (
            self.point.0 + self.handle_in.0,
            self.point.1 + self.handle_in.1,
        )
    }

    /// The absolute position of the outgoing handle.
    #[must_use]
    pub fn out_position(&self) -> (f32, f32) {
        (
            self.point.0 + self.handle_out.0,
            self.point.1 + self.handle_out.1,
        )
    }

    fn is_valid(&self) -> bool {
        [
            self.point,
            self.handle_in,
            self.handle_out,
            self.in_position(),
            self.out_position(),
        ]
        .iter()
        .all(|&(x, y)| in_range(x) && in_range(y))
    }
}

fn in_range(v: f32) -> bool {
    v.is_finite() && v.abs() <= MAX_PATH_COORDINATE
}

/// A run of anchors joined by cubic segments; closed adds the segment from
/// the last anchor back to the first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Subpath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

/// A named Bezier path of one or more subpaths.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub name: String,
    pub subpaths: Vec<Subpath>,
}

/// One cubic segment's control polygon.
type Cubic = [(f32, f32); 4];

fn cubic_between(a: &Anchor, b: &Anchor) -> Cubic {
    [a.point, a.out_position(), b.in_position(), b.point]
}

/// Appends the flattening of `c` (excluding its first point) to `out`.
fn flatten_cubic(c: &Cubic, tolerance: f32, out: &mut Vec<(f32, f32)>) {
    let [p0, p1, p2, p3] = *c;
    let d1 = (p0.0 - 2.0 * p1.0 + p2.0, p0.1 - 2.0 * p1.1 + p2.1);
    let d2 = (p1.0 - 2.0 * p2.0 + p3.0, p1.1 - 2.0 * p2.1 + p3.1);
    let m = d1.0.hypot(d1.1).max(d2.0.hypot(d2.1));
    // Uniform subdivision error is at most (1/8) * max|B''| / n^2 with
    // max|B''| <= 6 * m.
    // A segment with no handle at either end is a straight line.
    let n = if m <= 0.0 || (p1 == p0 && p2 == p3) {
        1
    } else {
        let raw = (0.75 * m / tolerance).sqrt().ceil();
        // Truncation is intended: `raw` is a small positive whole number.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = raw.min(MAX_SEGMENTS_PER_CURVE as f32) as u32;
        n.max(1)
    };
    for i in 1..=n {
        #[allow(clippy::cast_precision_loss)]
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let x = b0 * p0.0 + b1 * p1.0 + b2 * p2.0 + b3 * p3.0;
        let y = b0 * p0.1 + b1 * p1.1 + b2 * p2.1 + b3 * p3.1;
        out.push(if i == n { p3 } else { (x, y) });
    }
}

impl Subpath {
    /// The polyline of this subpath within `tolerance`. A closed subpath's
    /// closing curve is included and its last point is not repeated.
    #[must_use]
    pub fn flatten(&self, tolerance: f32) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        let Some(first) = self.anchors.first() else {
            return out;
        };
        out.push(first.point);
        for pair in self.anchors.windows(2) {
            if let [a, b] = pair {
                flatten_cubic(&cubic_between(a, b), tolerance, &mut out);
            }
        }
        if self.closed
            && self.anchors.len() > 1
            && let Some(last) = self.anchors.last()
        {
            flatten_cubic(&cubic_between(last, first), tolerance, &mut out);
            // The closing curve ends on the first point again.
            out.pop();
        }
        out
    }
}

impl Path {
    /// An empty path called `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            subpaths: Vec::new(),
        }
    }

    /// How many anchors the path holds.
    #[must_use]
    pub fn anchor_count(&self) -> usize {
        self.subpaths.iter().map(|s| s.anchors.len()).sum()
    }

    /// Whether the path has no anchor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor_count() == 0
    }

    /// Checks every bound of the module doc (the name is checked by
    /// [`PathSet`], which owns uniqueness).
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] naming the first problem found.
    pub fn validate(&self) -> Result<(), DocError> {
        if self.subpaths.len() > MAX_SUBPATHS {
            return Err(DocError::InvalidPath("too many subpaths"));
        }
        for sub in &self.subpaths {
            if sub.anchors.len() > MAX_ANCHORS_PER_SUBPATH {
                return Err(DocError::InvalidPath("too many anchors in a subpath"));
            }
            if !sub.anchors.iter().all(Anchor::is_valid) {
                return Err(DocError::InvalidPath("a coordinate is not usable"));
            }
        }
        Ok(())
    }

    /// The polylines of every subpath within `tolerance` (see
    /// [`Subpath::flatten`]); empty subpaths give nothing.
    #[must_use]
    pub fn flatten(&self, tolerance: f32) -> Vec<Vec<(f32, f32)>> {
        self.subpaths
            .iter()
            .map(|s| s.flatten(tolerance))
            .filter(|p| !p.is_empty())
            .collect()
    }

    fn translate(&mut self, dx: f32, dy: f32) {
        for sub in &mut self.subpaths {
            for a in &mut sub.anchors {
                a.point = (a.point.0 + dx, a.point.1 + dy);
            }
        }
    }
}

/// A path name as typed, trimmed and checked like a saved selection's;
/// [`WORK_PATH_NAME`] is reserved for the work path (any case).
///
/// # Errors
///
/// [`DocError::InvalidPath`] for an empty, over-long, control-character or
/// reserved name.
pub fn validate_path_name(raw: &str) -> Result<String, DocError> {
    let name = validate_selection_name(raw)
        .map_err(|_| DocError::InvalidPath("that is not a usable path name"))?;
    if name.to_lowercase() == WORK_PATH_NAME.to_lowercase() {
        return Err(DocError::InvalidPath("that name is reserved"));
    }
    Ok(name)
}

/// A document's paths: saved ones in order plus an optional work path.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathSet {
    paths: Vec<Path>,
    work: Option<Path>,
}

impl PathSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a set from stored parts, checking every bound.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] for any violation (counts, names, coordinates).
    pub fn from_parts(paths: Vec<Path>, work: Option<Path>) -> Result<Self, DocError> {
        let set = Self { paths, work };
        set.check()?;
        Ok(set)
    }

    /// Mutable access to the subpaths of the path `name`, for a gesture that
    /// edits it live. The caller must call [`Self::validate`] before the
    /// result is kept; nothing is checked here.
    pub fn subpaths_mut(&mut self, name: &str) -> Option<&mut Vec<Subpath>> {
        if name == WORK_PATH_NAME {
            return self.work.as_mut().map(|p| &mut p.subpaths);
        }
        self.paths
            .iter_mut()
            .find(|p| p.name == name)
            .map(|p| &mut p.subpaths)
    }

    /// Checks every bound of the module doc over the whole set.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] naming the first problem found.
    pub fn validate(&self) -> Result<(), DocError> {
        self.check()
    }

    fn check(&self) -> Result<(), DocError> {
        let count = self.paths.len() + usize::from(self.work.is_some());
        if count > MAX_PATHS {
            return Err(DocError::InvalidPath("too many paths"));
        }
        let mut total = 0_usize;
        let mut seen: Vec<&str> = Vec::new();
        for path in &self.paths {
            if validate_path_name(&path.name).ok().as_deref() != Some(path.name.as_str())
                || seen.contains(&path.name.as_str())
            {
                return Err(DocError::InvalidPath("a path name is not usable"));
            }
            seen.push(&path.name);
        }
        for path in self.paths.iter().chain(self.work.iter()) {
            path.validate()?;
            total += path.anchor_count();
        }
        if let Some(work) = &self.work
            && work.name != WORK_PATH_NAME
        {
            return Err(DocError::InvalidPath("the work path has its own name"));
        }
        if total > MAX_PATH_ANCHORS_TOTAL {
            return Err(DocError::InvalidPath("too many anchors in the document"));
        }
        Ok(())
    }

    /// Whether the document has no path at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.work.is_none()
    }

    /// The saved paths in order.
    #[must_use]
    pub fn saved(&self) -> &[Path] {
        &self.paths
    }

    /// The work path, if any.
    #[must_use]
    pub fn work(&self) -> Option<&Path> {
        self.work.as_ref()
    }

    /// Every path's name in panel order: saved paths, then the work path.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.paths
            .iter()
            .chain(self.work.iter())
            .map(|p| p.name.as_str())
    }

    /// How many paths there are, the work path included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.paths.len() + usize::from(self.work.is_some())
    }

    /// The path called `name` (the work path answers to [`WORK_PATH_NAME`]).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Path> {
        self.paths
            .iter()
            .chain(self.work.iter())
            .find(|p| p.name == name)
    }

    /// The first name `Path N` not in use.
    #[must_use]
    pub fn next_free_name(&self) -> String {
        (1..)
            .map(|n| format!("Path {n}"))
            .find(|name| self.get(name).is_none())
            .unwrap_or_default()
    }

    /// Replaces the contents of the path `name` with `path`'s subpaths (the
    /// name stays); the set is unchanged on an error.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] for an unknown name or a bound violation.
    pub fn set_subpaths(&mut self, name: &str, subpaths: Vec<Subpath>) -> Result<(), DocError> {
        let mut next = self.clone();
        let target = if name == WORK_PATH_NAME {
            next.work.as_mut()
        } else {
            next.paths.iter_mut().find(|p| p.name == name)
        }
        .ok_or(DocError::InvalidPath("no such path"))?;
        target.subpaths = subpaths;
        next.check()?;
        *self = next;
        Ok(())
    }

    /// Adds an empty saved path called `name`.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] for a bad or taken name or a full document.
    pub fn add(&mut self, name: &str) -> Result<String, DocError> {
        let name = validate_path_name(name)?;
        if self.get(&name).is_some() {
            return Err(DocError::InvalidPath("that name is taken"));
        }
        if self.len() >= MAX_PATHS {
            return Err(DocError::InvalidPath("too many paths"));
        }
        self.paths.push(Path::new(name.clone()));
        Ok(name)
    }

    /// Creates the empty work path when there is none.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] when the document already holds [`MAX_PATHS`].
    pub fn ensure_work(&mut self) -> Result<(), DocError> {
        if self.work.is_none() {
            if self.len() >= MAX_PATHS {
                return Err(DocError::InvalidPath("too many paths"));
            }
            self.work = Some(Path::new(WORK_PATH_NAME));
        }
        Ok(())
    }

    /// Renames the path `old` to `new`. Renaming the work path saves it:
    /// it becomes an ordinary path at the end of the list.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] for an unknown `old` or a bad or taken `new`.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<String, DocError> {
        let new = validate_path_name(new)?;
        if old != new && self.get(&new).is_some() {
            return Err(DocError::InvalidPath("that name is taken"));
        }
        if old == WORK_PATH_NAME {
            let mut path = self
                .work
                .take()
                .ok_or(DocError::InvalidPath("no such path"))?;
            path.name.clone_from(&new);
            self.paths.push(path);
        } else {
            let path = self
                .paths
                .iter_mut()
                .find(|p| p.name == old)
                .ok_or(DocError::InvalidPath("no such path"))?;
            path.name.clone_from(&new);
        }
        Ok(new)
    }

    /// Removes the path `name`.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] for an unknown name.
    pub fn delete(&mut self, name: &str) -> Result<(), DocError> {
        if name == WORK_PATH_NAME && self.work.is_some() {
            self.work = None;
            return Ok(());
        }
        let before = self.paths.len();
        self.paths.retain(|p| p.name != name);
        if self.paths.len() == before {
            return Err(DocError::InvalidPath("no such path"));
        }
        Ok(())
    }

    /// Moves every anchor by `(dx, dy)` (the Crop). All or nothing.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidPath`] when a shifted coordinate leaves the range.
    pub fn translate(&mut self, dx: i64, dy: i64) -> Result<(), DocError> {
        #[allow(clippy::cast_precision_loss)]
        let (fx, fy) = (dx as f32, dy as f32);
        let mut next = self.clone();
        for path in next.paths.iter_mut().chain(next.work.iter_mut()) {
            path.translate(fx, fy);
        }
        next.check()?;
        *self = next;
        Ok(())
    }
}

/// The selection a path outlines: every subpath is flattened
/// ([`FLATTEN_TOLERANCE`]), an open one is closed with a straight edge, and
/// the polygons are filled together by [`polygon_mask`]'s **nonzero
/// winding** rule, clipped to the `size` document. `None` when the path
/// covers no pixel.
///
/// The polygons are chained into one polygon for the filler: each later
/// subpath is entered from, and left back to, the first subpath's first
/// point along the same line twice in opposite directions, which adds
/// nothing to any winding number.
///
/// # Errors
///
/// [`DocError::InvalidPath`] for an out-of-bounds path;
/// [`DocError::SelectionPolygonTooComplex`] when the flattening has more
/// vertices than the filler takes; [`DocError::SelectionMaskTooLarge`].
pub fn selection_from_path(path: &Path, size: (u32, u32)) -> Result<Option<Selection>, DocError> {
    path.validate()?;
    let mut hub: Option<(f64, f64)> = None;
    let mut points: Vec<(f64, f64)> = Vec::new();
    for poly in path.flatten(FLATTEN_TOLERANCE) {
        if poly.len() < 3 {
            continue;
        }
        let to64 = |&(x, y): &(f32, f32)| (f64::from(x), f64::from(y));
        let first = poly.first().map(to64);
        match (hub, first) {
            (None, Some(f)) => {
                hub = Some(f);
                points.extend(poly.iter().map(to64));
                points.push(f);
            }
            (Some(h), Some(f)) => {
                points.push(f);
                points.extend(poly.iter().map(to64));
                points.push(f);
                points.push(h);
            }
            _ => {}
        }
        if points.len() > MAX_POLYGON_VERTICES {
            return Err(DocError::SelectionPolygonTooComplex(points.len()));
        }
    }
    let clip = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    Ok(polygon_mask(&points, &clip)?.map(|mask| Selection::from_mask(Arc::new(mask))))
}
