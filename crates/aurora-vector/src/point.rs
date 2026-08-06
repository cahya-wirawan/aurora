//! A resolution-independent 2D point.

/// A point in resolution-independent (fractional) space — unlike
/// `aurora_core::Rect`'s integer, document/tile-space coordinates,
/// vector geometry is inherently fractional (PRD's own "fractional DPI
/// and per-monitor scaling are correct by construction"). Deliberately
/// this crate's own type, not a re-export of `lyon`'s: downstream
/// crates (`aurora-widgets`, `aurora-text`) should never need to depend
/// on `lyon` directly to describe a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

pub(crate) fn to_lyon(point: Point) -> lyon::math::Point {
    lyon::math::point(point.x, point.y)
}

pub(crate) fn from_lyon(point: lyon::math::Point) -> Point {
    Point::new(point.x, point.y)
}

#[cfg(test)]
mod tests {
    use super::{Point, from_lyon, to_lyon};

    #[test]
    fn to_lyon_and_back_round_trips_the_same_coordinates() {
        let point = Point::new(1.5, -2.25);
        assert_eq!(from_lyon(to_lyon(point)), point);
    }
}
