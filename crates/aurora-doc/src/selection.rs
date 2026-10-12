//! Selection representation: the current active selection plus any named
//! ones saved for later. PLAN.md M1.4's fourth piece.

use std::collections::HashMap;

use aurora_core::Rect;

use crate::error::DocError;

/// A selection: the region of the document an edit operation is
/// restricted to. Deliberately just a bounding [`Rect`] plus an inverted
/// flag, not a real raster mask — no selection tool (rectangle, ellipse,
/// lasso, magic wand, all FR-004, all Phase 2 scope) exists yet to
/// produce anything but a bounding box, and there is no real pixel
/// storage to hold antialiased/feathered coverage even if one did (the
/// same open question `LayerKind::Pixel`'s own `bounds` field and
/// [`crate::LayerMask`] already flagged). `inverted` answers FR-004's
/// "Inverse" command the same way [`crate::LayerMask::inverted`] answers
/// a mask's own invert toggle — well-defined even for a plain rectangle
/// (its complement is "everything outside it"), so it doesn't need real
/// pixel data either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub bounds: Rect,
    pub inverted: bool,
}

impl Selection {
    /// A non-inverted selection covering `bounds`.
    #[must_use]
    pub const fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            inverted: false,
        }
    }

    /// How much of document pixel `(x, y)` the selection covers, for
    /// painting through it (0.211.0): exactly `1.0` or `0.0` for a
    /// rectangle (`Rect`'s half-open convention: the left/top edge is in,
    /// the right/bottom edge is out), flipped by `inverted`. An empty
    /// rectangle contains no pixel, so inverted it covers everything. A
    /// future raster selection answers with anything in between; callers
    /// must not assume it is only ever `0` or `1`.
    #[must_use]
    pub fn coverage_at(&self, x: i64, y: i64) -> f32 {
        if self.bounds.contains_point(x, y) != self.inverted {
            1.0
        } else {
            0.0
        }
    }

    /// A rectangle outside which [`Self::coverage_at`] is `0`: the bounds
    /// of a plain selection, `None` for an inverted one (everything
    /// outside the rectangle is covered, so no useful bound exists).
    #[must_use]
    pub fn coverage_bounds(&self) -> Option<Rect> {
        (!self.inverted).then_some(self.bounds)
    }

    /// Whether every pixel of `rect` is covered. An empty `rect` has no
    /// pixel and is vacuously covered.
    #[must_use]
    pub fn fully_covers(&self, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if self.inverted {
            !self.bounds.intersects(rect)
        } else {
            self.bounds.x <= rect.x
                && self.bounds.y <= rect.y
                && self.bounds.right() >= rect.right()
                && self.bounds.bottom() >= rect.bottom()
        }
    }

    /// Whether no pixel of `rect` is covered. An empty `rect` has no
    /// pixel and is vacuously uncovered.
    #[must_use]
    pub fn fully_excludes(&self, rect: &Rect) -> bool {
        if rect.is_empty() {
            return true;
        }
        if self.inverted {
            self.bounds.x <= rect.x
                && self.bounds.y <= rect.y
                && self.bounds.right() >= rect.right()
                && self.bounds.bottom() >= rect.bottom()
        } else {
            !self.bounds.intersects(rect)
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
}

impl SelectionSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: None,
            saved: HashMap::new(),
        }
    }

    /// The current active selection, or `None` if nothing is selected
    /// (equivalent to the whole document/layer being eligible).
    #[must_use]
    pub fn active(&self) -> Option<Selection> {
        self.active
    }

    /// Replaces the active selection with `selection`.
    pub fn select(&mut self, selection: Selection) {
        self.active = Some(selection);
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
        let selection = self.active.ok_or(DocError::NoActiveSelection)?;
        self.saved.insert(name.into(), selection);
        Ok(())
    }

    /// Loads the selection saved under `name` back as the active
    /// selection.
    ///
    /// # Errors
    ///
    /// Returns [`DocError::UnknownSavedSelection`] if no selection was
    /// ever saved under `name`.
    pub fn load(&mut self, name: &str) -> Result<(), DocError> {
        let selection = *self
            .saved
            .get(name)
            .ok_or_else(|| DocError::UnknownSavedSelection(name.to_owned()))?;
        self.active = Some(selection);
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
            ..sel
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
            for s in [sel, inv] {
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
        set.select(selection);
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
        set.select(selection);
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
}
