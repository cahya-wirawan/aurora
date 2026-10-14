//! Selection representation: the current active selection plus any named
//! ones saved for later. PLAN.md M1.4's fourth piece.

use std::collections::HashMap;
use std::sync::Arc;

use aurora_core::Rect;

use crate::error::DocError;
use crate::selection_mask::SelectionMask;

/// The most selections a document keeps saved by name (0.218.0).
pub const MAX_SAVED_SELECTIONS: usize = 64;

/// The most raster coverage (bytes of [`SelectionMask`]) the saved
/// selections of one document may hold together (0.218.0): 128 MiB, two
/// full 64 Mpx masks. An engineering default, not a design decision.
pub const MAX_SAVED_SELECTION_BYTES: u64 = 128 * 1024 * 1024;

/// The most characters in a saved selection's name (0.218.0), like a
/// workspace name.
pub const SELECTION_NAME_MAX_CHARS: usize = 64;

/// Why a saved selection's name was refused (0.218.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionNameError {
    Empty,
    /// A control, separator or text-direction character.
    InvalidCharacter,
    TooLong,
}

/// A typed name, trimmed and checked: non-empty, free of the characters
/// `sanitize_display_name` strips (controls, line/paragraph separators,
/// bidi overrides and isolates), at most [`SELECTION_NAME_MAX_CHARS`].
/// A name read from a `.aur` goes through the same check.
///
/// # Errors
///
/// [`SelectionNameError`] naming the first problem found.
pub fn validate_selection_name(raw: &str) -> Result<String, SelectionNameError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(SelectionNameError::Empty);
    }
    let stripped = |c: char| {
        let one = c.to_string();
        crate::sanitize_display_name(&one).as_ref() != one.as_str()
    };
    if name.chars().any(stripped) {
        return Err(SelectionNameError::InvalidCharacter);
    }
    if name.chars().count() > SELECTION_NAME_MAX_CHARS {
        return Err(SelectionNameError::TooLong);
    }
    Ok(name.to_owned())
}

/// The shape of a [`Selection`]'s region (0.213.0), before inversion.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SelectionShape {
    /// The whole of [`Selection::bounds`], coverage exactly `0` or `1`.
    #[default]
    Rect,
    /// The ellipse inscribed in [`Selection::bounds`], antialiased: a
    /// texel's coverage is the share of 16 evenly spread sample points
    /// inside it (`0`, `1/16` .. `1`).
    Ellipse,
    /// A raster coverage grid ([`SelectionMask`]); its own bounds are the
    /// selection's.
    Mask(Arc<SelectionMask>),
}

/// A selection: the region of the document an edit operation is
/// restricted to. A bounding [`Rect`], a [`SelectionShape`] inside it
/// (a rectangle, its inscribed ellipse, or a raster [`SelectionMask`])
/// and an inverted flag. `inverted` answers FR-004's "Inverse" command
/// the same way [`crate::LayerMask::inverted`] answers a mask's own
/// invert toggle: the complement of the shape is "everything else".
///
/// Cloning is cheap (a mask is shared through an `Arc`). The rectangle
/// shape answers every question exactly as the pre-0.213.0 rectangle-only
/// selection did (differential-tested); the other shapes answer
/// fractional coverage, which the painting tools already consume.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// The rectangle outside which a non-inverted selection covers
    /// nothing (for a mask: the mask's own bounds).
    pub bounds: Rect,
    pub inverted: bool,
    pub shape: SelectionShape,
}

/// Samples per axis of the ellipse's edge supersampling (4 x 4 = 16).
const ELLIPSE_SAMPLES: u32 = 4;

/// Whether the point `(px, py)` lies in the closed ellipse inscribed in
/// `b`. Done in `f64` on values that are exact multiples of `1/8`, so
/// mirrored points get bit-identical answers.
fn in_ellipse(b: &Rect, px: f64, py: f64) -> bool {
    let a = f64::from(b.width) / 2.0;
    let c = f64::from(b.height) / 2.0;
    if a <= 0.0 || c <= 0.0 {
        return false;
    }
    #[allow(clippy::cast_precision_loss)]
    let (cx, cy) = (b.x as f64 + a, b.y as f64 + c);
    let (dx, dy) = ((px - cx) / a, (py - cy) / c);
    dx * dx + dy * dy <= 1.0
}

/// The nearest point of the continuous rectangle `rect` to the ellipse's
/// centre.
fn nearest_to_centre(b: &Rect, rect: &Rect) -> (f64, f64) {
    #[allow(clippy::cast_precision_loss)]
    let (cx, cy) = (
        b.x as f64 + f64::from(b.width) / 2.0,
        b.y as f64 + f64::from(b.height) / 2.0,
    );
    #[allow(clippy::cast_precision_loss)]
    let (x0, x1, y0, y1) = (
        rect.x as f64,
        rect.right() as f64,
        rect.y as f64,
        rect.bottom() as f64,
    );
    (cx.clamp(x0, x1), cy.clamp(y0, y1))
}

/// Coverage of texel `(x, y)` by the ellipse inscribed in `b`.
fn ellipse_coverage(b: &Rect, x: i64, y: i64) -> f32 {
    if !b.contains_point(x, y) {
        return 0.0;
    }
    let texel = Rect {
        x,
        y,
        width: 1,
        height: 1,
    };
    if ellipse_covers(b, &texel) {
        return 1.0;
    }
    let (nx, ny) = nearest_to_centre(b, &texel);
    if !in_ellipse(b, nx, ny) {
        return 0.0;
    }
    let step = 1.0 / f64::from(ELLIPSE_SAMPLES);
    let mut inside = 0_u32;
    for j in 0..ELLIPSE_SAMPLES {
        for i in 0..ELLIPSE_SAMPLES {
            #[allow(clippy::cast_precision_loss)]
            let (sx, sy) = (
                x as f64 + (f64::from(i) + 0.5) * step,
                y as f64 + (f64::from(j) + 0.5) * step,
            );
            if in_ellipse(b, sx, sy) {
                inside += 1;
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let total = (ELLIPSE_SAMPLES * ELLIPSE_SAMPLES) as f32;
    inside as f32 / total
}

/// Whether every texel of non-empty `rect` is fully inside the ellipse
/// (all four corners of the continuous rectangle are: the ellipse is
/// convex, and a texel whose corners are inside has all its samples in).
fn ellipse_covers(b: &Rect, rect: &Rect) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let (x0, x1, y0, y1) = (
        rect.x as f64,
        rect.right() as f64,
        rect.y as f64,
        rect.bottom() as f64,
    );
    [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
        .into_iter()
        .all(|(px, py)| in_ellipse(b, px, py))
}

/// Whether no texel of `rect` has any ellipse coverage: it misses the
/// bounds, or the point of its clipped area nearest the centre is outside.
fn ellipse_excludes(b: &Rect, rect: &Rect) -> bool {
    if !b.intersects(rect) {
        return true;
    }
    let clipped_x = rect.x.max(b.x);
    let clipped_y = rect.y.max(b.y);
    let clipped = Rect {
        x: clipped_x,
        y: clipped_y,
        width: u32::try_from(rect.right().min(b.right()) - clipped_x).unwrap_or(0),
        height: u32::try_from(rect.bottom().min(b.bottom()) - clipped_y).unwrap_or(0),
    };
    let (nx, ny) = nearest_to_centre(b, &clipped);
    !in_ellipse(b, nx, ny)
}

impl Selection {
    /// A non-inverted rectangular selection covering `bounds`.
    #[must_use]
    pub const fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            inverted: false,
            shape: SelectionShape::Rect,
        }
    }

    /// A non-inverted selection of the ellipse inscribed in `bounds`.
    #[must_use]
    pub const fn ellipse(bounds: Rect) -> Self {
        Self {
            bounds,
            inverted: false,
            shape: SelectionShape::Ellipse,
        }
    }

    /// A non-inverted selection of a raster `mask`; its bounds are the
    /// mask's.
    #[must_use]
    pub fn from_mask(mask: Arc<SelectionMask>) -> Self {
        Self {
            bounds: mask.bounds(),
            inverted: false,
            shape: SelectionShape::Mask(mask),
        }
    }

    /// The bytes of raster coverage this selection holds: the mask's
    /// texel count for a [`SelectionShape::Mask`], `0` for a rectangle or
    /// an ellipse (they are computed). What the saved-selection cap and
    /// the selection-history budget count.
    #[must_use]
    pub fn mask_bytes(&self) -> u64 {
        match &self.shape {
            SelectionShape::Mask(mask) => mask.bounds().area_px(),
            SelectionShape::Rect | SelectionShape::Ellipse => 0,
        }
    }

    /// This selection moved by `(dx, dy)` (0.226.0, the Crop): the same
    /// shape and coverage, bounds shifted, not clipped to anything, so
    /// shifting by the negation restores it exactly.
    ///
    /// # Errors
    ///
    /// [`DocError::SelectionMaskInvalid`] when the shifted origin leaves
    /// `i64`'s range.
    pub fn translated(&self, dx: i64, dy: i64) -> Result<Self, DocError> {
        let x = self
            .bounds
            .x
            .checked_add(dx)
            .ok_or(DocError::SelectionMaskInvalid)?;
        let y = self
            .bounds
            .y
            .checked_add(dy)
            .ok_or(DocError::SelectionMaskInvalid)?;
        let shape = match &self.shape {
            SelectionShape::Rect => SelectionShape::Rect,
            SelectionShape::Ellipse => SelectionShape::Ellipse,
            SelectionShape::Mask(mask) => SelectionShape::Mask(Arc::new(mask.translated(dx, dy)?)),
        };
        Ok(Self {
            bounds: Rect {
                x,
                y,
                ..self.bounds
            },
            inverted: self.inverted,
            shape,
        })
    }

    /// Whether this is a plain rectangle (the exact, 0-or-1 fast path).
    #[must_use]
    pub const fn is_rect(&self) -> bool {
        matches!(self.shape, SelectionShape::Rect)
    }

    /// How much of document pixel `(x, y)` the selection covers, for
    /// painting through it (0.211.0): exactly `1.0` or `0.0` for a
    /// rectangle (`Rect`'s half-open convention: the left/top edge is in,
    /// the right/bottom edge is out), flipped by `inverted`. An empty
    /// rectangle contains no pixel, so inverted it covers everything. An
    /// ellipse answers `k/16` along its edge and a mask `byte/255`
    /// (0.213.0); an inverted one answers the exact complement. Callers
    /// must not assume it is only ever `0` or `1`.
    #[must_use]
    pub fn coverage_at(&self, x: i64, y: i64) -> f32 {
        match &self.shape {
            SelectionShape::Rect => {
                if self.bounds.contains_point(x, y) ^ self.inverted {
                    1.0
                } else {
                    0.0
                }
            }
            SelectionShape::Ellipse => {
                let c = ellipse_coverage(&self.bounds, x, y);
                if self.inverted { 1.0 - c } else { c }
            }
            SelectionShape::Mask(mask) => {
                let byte = mask.byte_at(x, y);
                let byte = if self.inverted { u8::MAX - byte } else { byte };
                f32::from(byte) / 255.0
            }
        }
    }

    /// The coverage byte (`0..=255`) of the un-inverted shape at texel
    /// `(x, y)`: `255`/`0` for a rectangle, `round(k/16 * 255)` for an
    /// ellipse, the mask's own byte for a mask. The combination of
    /// selections (0.215.0) works on these bytes.
    #[must_use]
    pub(crate) fn shape_byte_at(&self, x: i64, y: i64) -> u8 {
        match &self.shape {
            SelectionShape::Rect => {
                if self.bounds.contains_point(x, y) {
                    u8::MAX
                } else {
                    0
                }
            }
            SelectionShape::Ellipse => {
                // In 0..=255 after rounding a value in 0..=1.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let byte = (ellipse_coverage(&self.bounds, x, y) * 255.0).round() as u8;
                byte
            }
            SelectionShape::Mask(mask) => mask.byte_at(x, y),
        }
    }

    /// A rectangle outside which [`Self::coverage_at`] is `0`: the bounds
    /// of a plain selection, `None` for an inverted one (everything
    /// outside the shape is covered, so no useful bound exists).
    #[must_use]
    pub fn coverage_bounds(&self) -> Option<Rect> {
        (!self.inverted).then_some(self.bounds)
    }

    /// Whether the (un-inverted) shape covers every texel of non-empty
    /// `rect` fully. Conservative: `false` when unsure.
    fn shape_covers(&self, rect: &Rect) -> bool {
        match &self.shape {
            SelectionShape::Rect => {
                self.bounds.x <= rect.x
                    && self.bounds.y <= rect.y
                    && self.bounds.right() >= rect.right()
                    && self.bounds.bottom() >= rect.bottom()
            }
            SelectionShape::Ellipse => ellipse_covers(&self.bounds, rect),
            SelectionShape::Mask(mask) => mask.fully_covers(rect),
        }
    }

    /// Whether the (un-inverted) shape covers no texel of non-empty
    /// `rect` at all. Conservative: `false` when unsure.
    fn shape_excludes(&self, rect: &Rect) -> bool {
        match &self.shape {
            SelectionShape::Rect => !self.bounds.intersects(rect),
            SelectionShape::Ellipse => ellipse_excludes(&self.bounds, rect),
            SelectionShape::Mask(mask) => mask.fully_excludes(rect),
        }
    }

    /// Whether every pixel of `rect` is covered (coverage exactly `1`). An
    /// empty `rect` has no pixel and is vacuously covered. Conservative
    /// for the non-rectangular shapes: it may say `false` for a `rect`
    /// that happens to be covered, never `true` for one that is not.
    #[must_use]
    pub fn fully_covers(&self, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if self.inverted {
            self.shape_excludes(rect)
        } else {
            self.shape_covers(rect)
        }
    }

    /// Whether no pixel of `rect` is covered. An empty `rect` has no
    /// pixel and is vacuously uncovered. Conservative like
    /// [`Self::fully_covers`]: never `true` for a `rect` with a covered
    /// pixel.
    #[must_use]
    pub fn fully_excludes(&self, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if self.inverted {
            self.shape_covers(rect)
        } else {
            self.shape_excludes(rect)
        }
    }
}

/// A document's full selection state: the current active selection (if
/// any) plus any named ones saved for later — FR-004's "Save
/// Selection"/"Load Selection" commands, matching Photoshop's own
/// saved-selection alpha channels. Lives in `aurora-doc` rather than on
/// [`crate::LayerTree`] since a selection is a document-level concept, not
/// a per-layer one.
///
/// **Deliberately out of scope**: Feather, Expand, Contract, and Border
/// (FR-004's other selection commands) all need to reshape a real raster
/// region — meaningless on a bounding box, and there's no raster
/// selection storage yet for the same reason [`Selection`] itself doesn't
/// have one. Saving over an existing name always replaces it outright;
/// Photoshop's own "combine with channel" add/subtract/intersect modes
/// need the same real boolean-region math this pass doesn't have.
#[derive(Debug, Clone)]
pub struct SelectionSet {
    active: Option<Selection>,
    saved: HashMap<String, Selection>,
    /// The document's paths (0.233.0). They ride in the selection state so
    /// every route the selections already take (save, autosave, recovery,
    /// the document shelf, the Crop's shift) carries them too; they are not
    /// selections and [`Self::is_empty`] does not count them.
    paths: crate::PathSet,
}

impl SelectionSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: None,
            saved: HashMap::new(),
            paths: crate::PathSet::new(),
        }
    }

    /// The document's paths (0.233.0).
    #[must_use]
    pub fn paths(&self) -> &crate::PathSet {
        &self.paths
    }

    /// Replaces the document's paths.
    pub fn set_paths(&mut self, paths: crate::PathSet) {
        self.paths = paths;
    }

    /// The current active selection, or `None` if nothing is selected
    /// (equivalent to the whole document/layer being eligible).
    #[must_use]
    pub fn active(&self) -> Option<Selection> {
        self.active.clone()
    }

    /// Replaces the active selection with `selection`. A non-inverted
    /// selection with no area covers nothing (painting through it would
    /// paint nothing and no outline could show it), so it is never stored:
    /// it deselects instead, which is what a Marquee click or a
    /// one-dimensional drag means. An inverted empty rectangle is
    /// everything and is kept.
    pub fn select(&mut self, selection: Selection) {
        if selection.bounds.is_empty() && !selection.inverted {
            self.active = None;
        } else {
            self.active = Some(selection);
        }
    }

    /// Clears the active selection (Photoshop's "Deselect").
    pub fn deselect(&mut self) {
        self.active = None;
    }

    /// Flips the active selection's `inverted` flag in place
    /// (Photoshop's "Inverse").
    ///
    /// # Errors
    ///
    /// Returns [`DocError::NoActiveSelection`] if nothing is selected.
    pub fn invert(&mut self) -> Result<(), DocError> {
        let selection = self.active.as_mut().ok_or(DocError::NoActiveSelection)?;
        selection.inverted = !selection.inverted;
        Ok(())
    }

    /// Saves the current active selection under `name`, replacing
    /// whatever was previously saved under that name.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::NoActiveSelection`] if nothing is selected.
    pub fn save_active(&mut self, name: impl Into<String>) -> Result<(), DocError> {
        let selection = self.active.clone().ok_or(DocError::NoActiveSelection)?;
        let name = name.into();
        // 0.218.0: bounded. A replaced name does not count twice.
        let replaced = self.saved.get(&name);
        if replaced.is_none() && self.saved.len() >= MAX_SAVED_SELECTIONS {
            return Err(DocError::SavedSelectionsFull);
        }
        let total = self.saved_mask_bytes() - replaced.map_or(0, Selection::mask_bytes)
            + selection.mask_bytes();
        if total > MAX_SAVED_SELECTION_BYTES {
            return Err(DocError::SavedSelectionsTooLarge);
        }
        self.saved.insert(name, selection);
        Ok(())
    }

    /// A set holding `active` and the `saved` selections (0.218.0), as a
    /// file restores them. Checked like [`Self::save_active`] checks one
    /// at a time.
    ///
    /// # Errors
    ///
    /// [`DocError::InvalidSelectionName`] for a name that fails
    /// [`validate_selection_name`] (as typed: not trimmed) or repeats,
    /// [`DocError::SavedSelectionsFull`] / [`DocError::SavedSelectionsTooLarge`]
    /// past the caps.
    pub fn from_parts(
        active: Option<Selection>,
        saved: Vec<(String, Selection)>,
    ) -> Result<Self, DocError> {
        if saved.len() > MAX_SAVED_SELECTIONS {
            return Err(DocError::SavedSelectionsFull);
        }
        let mut set = Self::new();
        set.select_stored(active);
        let mut bytes = 0_u64;
        for (name, selection) in saved {
            if selection.bounds.is_empty() && !selection.inverted {
                return Err(DocError::EmptySavedSelection);
            }
            if validate_selection_name(&name).as_deref() != Ok(name.as_str()) {
                return Err(DocError::InvalidSelectionName);
            }
            bytes = bytes.saturating_add(selection.mask_bytes());
            if bytes > MAX_SAVED_SELECTION_BYTES {
                return Err(DocError::SavedSelectionsTooLarge);
            }
            if set.saved.insert(name, selection).is_some() {
                return Err(DocError::InvalidSelectionName);
            }
        }
        Ok(set)
    }

    fn select_stored(&mut self, active: Option<Selection>) {
        match active {
            Some(selection) => self.select(selection),
            None => self.deselect(),
        }
    }

    /// Moves the active selection and every saved one by `(dx, dy)`
    /// (0.226.0, the Crop and its undo). All or nothing: on an error the
    /// set is unchanged. Not clipped, so the negation restores exactly.
    ///
    /// # Errors
    ///
    /// [`DocError::SelectionMaskInvalid`] when a shifted origin overflows.
    pub fn translate(&mut self, dx: i64, dy: i64) -> Result<(), DocError> {
        let active = self
            .active
            .as_ref()
            .map(|selection| selection.translated(dx, dy))
            .transpose()?;
        let mut saved = HashMap::with_capacity(self.saved.len());
        for (name, selection) in &self.saved {
            saved.insert(name.clone(), selection.translated(dx, dy)?);
        }
        // Paths follow, all or nothing with the rest (0.233.0).
        let mut paths = self.paths.clone();
        paths
            .translate(dx, dy)
            .map_err(|_| DocError::SelectionMaskInvalid)?;
        self.active = active;
        self.saved = saved;
        self.paths = paths;
        Ok(())
    }

    /// The selection saved under `name`, if any.
    #[must_use]
    pub fn saved(&self, name: &str) -> Option<&Selection> {
        self.saved.get(name)
    }

    /// How many selections are saved.
    #[must_use]
    pub fn saved_count(&self) -> usize {
        self.saved.len()
    }

    /// The raster coverage bytes the saved selections hold together.
    #[must_use]
    pub fn saved_mask_bytes(&self) -> u64 {
        self.saved.values().map(Selection::mask_bytes).sum()
    }

    /// Every saved selection, sorted by name (a stable order for files
    /// and menus).
    #[must_use]
    pub fn saved_sorted(&self) -> Vec<(&str, &Selection)> {
        let mut all: Vec<(&str, &Selection)> =
            self.saved.iter().map(|(n, s)| (n.as_str(), s)).collect();
        all.sort_by(|a, b| a.0.cmp(b.0));
        all
    }

    /// Whether the set holds nothing at all (no active selection, none
    /// saved).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_none() && self.saved.is_empty()
    }

    /// Loads the selection saved under `name` back as the active
    /// selection.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownSavedSelection`] if no selection was
    /// ever saved under `name`.
    pub fn load(&mut self, name: &str) -> Result<(), DocError> {
        let selection = self
            .saved
            .get(name)
            .cloned()
            .ok_or_else(|| DocError::UnknownSavedSelection(name.to_owned()))?;
        // Through the same invariant as every other way in: an empty,
        // non-inverted entry (a damaged file could hold one; `from_parts`
        // refuses it, but a set built by hand might) deselects.
        self.select(selection);
        Ok(())
    }

    /// Deletes the selection saved under `name`.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownSavedSelection`] if no selection was
    /// ever saved under `name`.
    pub fn delete_saved(&mut self, name: &str) -> Result<(), DocError> {
        self.saved
            .remove(name)
            .ok_or_else(|| DocError::UnknownSavedSelection(name.to_owned()))?;
        Ok(())
    }

    /// Names of every currently saved selection. No defined order.
    pub fn saved_names(&self) -> impl Iterator<Item = &str> {
        self.saved.keys().map(String::as_str)
    }
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::{Selection, SelectionSet};
    use crate::DocError;
    use aurora_core::Rect;

    fn bounds() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    fn other_bounds() -> Rect {
        Rect {
            x: 5,
            y: 5,
            width: 20,
            height: 20,
        }
    }

    fn r(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn coverage_at_is_one_inside_and_zero_outside_with_half_open_edges() {
        let s = Selection::new(r(2, 3, 4, 5));
        assert_eq!(s.coverage_at(2, 3), 1.0, "left/top edge is in");
        assert_eq!(s.coverage_at(5, 7), 1.0, "last column/row is in");
        assert_eq!(s.coverage_at(6, 7), 0.0, "right edge is out");
        assert_eq!(s.coverage_at(5, 8), 0.0, "bottom edge is out");
        assert_eq!(s.coverage_at(1, 3), 0.0);
        assert_eq!(s.coverage_at(2, 2), 0.0);
        assert_eq!(s.coverage_at(-100, -100), 0.0);
    }

    #[test]
    fn selecting_an_empty_plain_rectangle_deselects_but_an_inverted_one_is_kept() {
        let mut set = SelectionSet::new();
        set.select(Selection::new(r(0, 0, 10, 10)));
        set.select(Selection::new(r(3, 3, 0, 7)));
        assert_eq!(set.active(), None, "no area covers nothing: deselected");
        set.select(Selection::new(r(3, 3, 5, 0)));
        assert_eq!(set.active(), None);
        let everything = Selection {
            bounds: r(3, 3, 0, 7),
            inverted: true,
            shape: super::SelectionShape::Rect,
        };
        set.select(everything.clone());
        assert_eq!(set.active(), Some(everything));
    }

    #[test]
    fn inverting_flips_every_texel_and_drops_the_bound() {
        let mut s = Selection::new(r(2, 3, 4, 5));
        assert_eq!(s.coverage_bounds(), Some(r(2, 3, 4, 5)));
        s.inverted = true;
        assert_eq!(s.coverage_bounds(), None);
        assert_eq!(s.coverage_at(2, 3), 0.0);
        assert_eq!(s.coverage_at(6, 7), 1.0);
        assert_eq!(s.coverage_at(-5, 900), 1.0);
    }

    #[test]
    fn an_empty_selection_covers_nothing_and_inverted_everything() {
        let mut s = Selection::new(r(4, 4, 0, 10));
        assert_eq!(s.coverage_at(4, 4), 0.0);
        s.inverted = true;
        assert_eq!(s.coverage_at(4, 4), 1.0);
        assert!(s.fully_covers(&r(0, 0, 100, 100)));
    }

    #[test]
    fn region_queries_agree_with_per_pixel_coverage() {
        let sel = Selection::new(r(10, 10, 20, 20));
        let inv = Selection {
            inverted: true,
            ..sel.clone()
        };
        for rect in [
            r(10, 10, 20, 20),
            r(12, 12, 5, 5),
            r(0, 0, 10, 10),
            r(0, 0, 11, 11),
            r(25, 25, 10, 10),
            r(30, 30, 3, 3),
            r(9, 12, 22, 3),
        ] {
            for s in [sel.clone(), inv.clone()] {
                let (mut covered, mut missed) = (0_u32, 0_u32);
                for y in rect.y..rect.bottom() {
                    for x in rect.x..rect.right() {
                        if s.coverage_at(x, y) > 0.0 {
                            covered += 1;
                        } else {
                            missed += 1;
                        }
                    }
                }
                assert_eq!(s.fully_covers(&rect), missed == 0, "{s:?} {rect:?}");
                assert_eq!(s.fully_excludes(&rect), covered == 0, "{s:?} {rect:?}");
            }
        }
    }

    #[test]
    fn selection_new_is_not_inverted() {
        let s = Selection::new(bounds());
        assert_eq!(s.bounds, bounds());
        assert!(!s.inverted);
    }

    #[test]
    fn fresh_set_has_no_active_selection() {
        let set = SelectionSet::new();
        assert_eq!(set.active(), None);
        assert_eq!(set.saved_names().count(), 0);
    }

    #[test]
    fn default_is_the_same_as_new() {
        let set = SelectionSet::default();
        assert_eq!(set.active(), None);
    }

    #[test]
    fn select_and_deselect_round_trip() {
        let mut set = SelectionSet::new();
        let selection = Selection::new(bounds());
        set.select(selection.clone());
        assert_eq!(set.active(), Some(selection));

        set.deselect();
        assert_eq!(set.active(), None);
    }

    #[test]
    fn select_replaces_a_previous_active_selection() {
        let mut set = SelectionSet::new();
        set.select(Selection::new(bounds()));
        set.select(Selection::new(other_bounds()));
        assert_eq!(set.active(), Some(Selection::new(other_bounds())));
    }

    #[test]
    fn invert_flips_the_active_selection_and_rejects_when_absent() {
        let mut set = SelectionSet::new();
        match set.invert() {
            Err(DocError::NoActiveSelection) => {}
            other => unreachable!("expected NoActiveSelection, got {other:?}"),
        }

        set.select(Selection::new(bounds()));
        if let Err(err) = set.invert() {
            unreachable!("{err:?}");
        }
        assert_eq!(set.active().map(|s| s.inverted), Some(true));

        if let Err(err) = set.invert() {
            unreachable!("{err:?}");
        }
        assert_eq!(set.active().map(|s| s.inverted), Some(false));
    }

    #[test]
    fn save_active_rejects_when_nothing_is_selected() {
        let mut set = SelectionSet::new();
        match set.save_active("foo") {
            Err(DocError::NoActiveSelection) => {}
            other => unreachable!("expected NoActiveSelection, got {other:?}"),
        }
        assert_eq!(set.saved_names().count(), 0);
    }

    #[test]
    fn save_load_and_delete_round_trip() {
        let mut set = SelectionSet::new();
        let selection = Selection::new(bounds());
        set.select(selection.clone());
        if let Err(err) = set.save_active("mine") {
            unreachable!("{err:?}");
        }
        assert_eq!(set.saved_names().collect::<Vec<_>>(), ["mine"]);

        // Deselecting must not lose the saved copy.
        set.deselect();
        assert_eq!(set.active(), None);

        if let Err(err) = set.load("mine") {
            unreachable!("{err:?}");
        }
        assert_eq!(set.active(), Some(selection));

        if let Err(err) = set.delete_saved("mine") {
            unreachable!("{err:?}");
        }
        assert_eq!(set.saved_names().count(), 0);
    }

    #[test]
    fn save_active_replaces_an_existing_saved_name() {
        let mut set = SelectionSet::new();
        set.select(Selection::new(bounds()));
        if let Err(err) = set.save_active("mine") {
            unreachable!("{err:?}");
        }
        set.select(Selection::new(other_bounds()));
        if let Err(err) = set.save_active("mine") {
            unreachable!("{err:?}");
        }

        if let Err(err) = set.load("mine") {
            unreachable!("{err:?}");
        }
        assert_eq!(set.active(), Some(Selection::new(other_bounds())));
        assert_eq!(
            set.saved_names().count(),
            1,
            "must not have added a second entry"
        );
    }

    #[test]
    fn load_rejects_an_unknown_name() {
        let mut set = SelectionSet::new();
        match set.load("nope") {
            Err(DocError::UnknownSavedSelection(name)) => assert_eq!(name, "nope"),
            other => unreachable!("expected UnknownSavedSelection, got {other:?}"),
        }
    }

    #[test]
    fn delete_saved_rejects_an_unknown_name() {
        let mut set = SelectionSet::new();
        match set.delete_saved("nope") {
            Err(DocError::UnknownSavedSelection(name)) => assert_eq!(name, "nope"),
            other => unreachable!("expected UnknownSavedSelection, got {other:?}"),
        }
    }

    #[test]
    fn load_of_an_empty_plain_entry_deselects_and_never_stores_it() {
        let mut set = SelectionSet::new();
        set.select(Selection::new(bounds()));
        // The invariant keeps such an entry out through the public API
        // ([`SelectionSet::from_parts`] refuses it); plant one to prove
        // `load` applies the rule anyway.
        set.saved
            .insert("empty".to_owned(), Selection::new(r(3, 3, 0, 7)));
        if let Err(err) = set.load("empty") {
            unreachable!("{err:?}");
        }
        assert_eq!(set.active(), None, "an empty plain selection is not stored");
    }
}
