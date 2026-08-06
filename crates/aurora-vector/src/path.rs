//! Building and holding a resolution-independent vector path.

use crate::point::{Point, to_lyon};

/// Builds a [`Path`] one segment at a time — [`Self::move_to`] starts a
/// new sub-path (a path can hold more than one — e.g. a letter "O"'s
/// inner and outer contours, or an icon made of several disjoint
/// shapes), [`Self::line_to`]/[`Self::quadratic_bezier_to`]/
/// [`Self::cubic_bezier_to`] extend the current one, and [`Self::close`]
/// connects it back to wherever it began. A thin wrapper over
/// `lyon_path`'s own builder — callers never see `lyon`'s own types,
/// only [`Point`].
pub struct PathBuilder {
    inner: lyon::path::Builder,
}

impl PathBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: lyon::path::Path::builder(),
        }
    }

    /// Starts a new sub-path at `at`. Must be called before any of
    /// [`Self::line_to`]/[`Self::quadratic_bezier_to`]/
    /// [`Self::cubic_bezier_to`]/[`Self::close`], and again for each
    /// additional sub-path a multi-contour shape needs.
    pub fn move_to(&mut self, at: Point) -> &mut Self {
        self.inner.begin(to_lyon(at));
        self
    }

    /// Adds a straight line segment to the current sub-path.
    pub fn line_to(&mut self, to: Point) -> &mut Self {
        self.inner.line_to(to_lyon(to));
        self
    }

    /// Adds a quadratic (one control point) Bézier curve to the current
    /// sub-path.
    pub fn quadratic_bezier_to(&mut self, ctrl: Point, to: Point) -> &mut Self {
        self.inner.quadratic_bezier_to(to_lyon(ctrl), to_lyon(to));
        self
    }

    /// Adds a cubic (two control point) Bézier curve to the current
    /// sub-path — most real icon/letterform outlines, and how
    /// [`crate::rounded_rect`] draws its own four corners.
    pub fn cubic_bezier_to(&mut self, ctrl1: Point, ctrl2: Point, to: Point) -> &mut Self {
        self.inner
            .cubic_bezier_to(to_lyon(ctrl1), to_lyon(ctrl2), to_lyon(to));
        self
    }

    /// Closes the current sub-path — connects its own last point back
    /// to wherever its [`Self::move_to`] began, so [`crate::fill`] sees
    /// a real, closed shape rather than an open one with no defined
    /// interior.
    pub fn close(&mut self) -> &mut Self {
        self.inner.close();
        self
    }

    /// Consumes the builder, producing an immutable, tessellation-ready
    /// [`Path`].
    #[must_use]
    pub fn build(self) -> Path {
        Path {
            inner: self.inner.build(),
        }
    }
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// `lyon_path::Builder` (`NoAttributes<BuilderImpl>`) doesn't itself
// implement `Debug` (its inner `BuilderImpl` doesn't), so this can't be
// derived — written by hand instead, matching `missing_debug_implementations`
// (workspace lint) without leaking `lyon`'s own types into the message.
impl std::fmt::Debug for PathBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathBuilder").finish_non_exhaustive()
    }
}

/// A resolution-independent vector path, ready to [`crate::fill`] or
/// [`crate::stroke`] — one or more sub-paths of line/Bézier segments,
/// built via [`PathBuilder`].
#[derive(Clone)]
pub struct Path {
    pub(crate) inner: lyon::path::Path,
}

// `lyon_path::Path` doesn't implement `Debug` either — see
// `PathBuilder`'s own impl above for why this is written by hand.
impl std::fmt::Debug for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Path").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{Path, PathBuilder};
    use crate::point::Point;

    #[test]
    fn a_fresh_builder_produces_an_empty_path() {
        let path: Path = PathBuilder::new().build();
        assert_eq!(path.inner.iter().count(), 0);
    }

    #[test]
    fn a_closed_triangle_has_the_expected_number_of_path_events() {
        let mut builder = PathBuilder::new();
        builder
            .move_to(Point::new(0.0, 0.0))
            .line_to(Point::new(10.0, 0.0))
            .line_to(Point::new(5.0, 10.0))
            .close();
        let path = builder.build();
        // Begin + 2 lines + close/end -- exactly the four events this
        // sub-path was built from, in order.
        assert_eq!(path.inner.iter().count(), 4);
    }

    #[test]
    fn debug_impls_do_not_panic() {
        let builder = PathBuilder::new();
        let path = PathBuilder::new().build();
        assert!(!format!("{builder:?}").is_empty());
        assert!(!format!("{path:?}").is_empty());
    }
}
