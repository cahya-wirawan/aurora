//! Tessellating a [`crate::Path`] into GPU-ready triangles.

use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, FillVertexConstructor, StrokeOptions,
    StrokeTessellator, StrokeVertex, StrokeVertexConstructor, VertexBuffers,
};

use crate::error::VectorError;
use crate::path::Path;
use crate::point::{Point, from_lyon};

/// The tessellation tolerance [`fill`]/[`stroke`] use when a caller
/// doesn't need finer control — `lyon`'s own default, tight enough that
/// curve-flattening error is imperceptible at typical UI scales.
/// Resolution-independent rendering eventually wants this to scale with
/// the actual on-screen size (a widget rendered small needs less
/// precision than one zoomed in) — real, separate follow-on work, not
/// invented here without a real caller driving the tradeoff.
pub const DEFAULT_TOLERANCE: f32 = 0.1;

/// A tessellated mesh: flat vertex positions and the triangle indices
/// that connect them (three per triangle) — what a GPU consumer
/// (`aurora-widgets`' own renderer, once it exists — see this crate's
/// own doc comment) uploads directly as a vertex/index buffer pair.
/// Resolution-independent: `vertices` are in the same float space the
/// source [`Path`] was built in, not pre-scaled for any particular DPI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Point>,
    pub indices: Vec<u32>,
}

/// Extracts just the position from whichever vertex kind `lyon` hands
/// back — every real UI shape this crate tessellates today is a flat,
/// single-colour fill or stroke, so position is all a caller needs;
/// per-vertex colour/UV data is separate, still-open follow-on work for
/// whenever a real GPU consumer needs it.
struct PositionOnly;

impl FillVertexConstructor<Point> for PositionOnly {
    fn new_vertex(&mut self, vertex: FillVertex<'_>) -> Point {
        from_lyon(vertex.position())
    }
}

impl StrokeVertexConstructor<Point> for PositionOnly {
    fn new_vertex(&mut self, vertex: StrokeVertex<'_, '_>) -> Point {
        from_lyon(vertex.position())
    }
}

/// Fills `path`, producing the triangle mesh covering its interior
/// (nonzero winding rule — `lyon`'s own tessellator default, matching
/// SVG and every mainstream 2D API).
///
/// # Errors
///
/// See [`VectorError`].
pub fn fill(path: &Path, tolerance: f32) -> Result<Mesh, VectorError> {
    let mut buffers: VertexBuffers<Point, u32> = VertexBuffers::new();
    let mut tessellator = FillTessellator::new();
    let options = FillOptions::default().with_tolerance(tolerance);
    tessellator.tessellate_path(
        &path.inner,
        &options,
        &mut BuffersBuilder::new(&mut buffers, PositionOnly),
    )?;
    Ok(Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
    })
}

/// Strokes `path` at `width`, producing the triangle mesh that covers
/// the outline itself (not its interior) — a border, not a fill.
///
/// # Errors
///
/// See [`VectorError`].
pub fn stroke(path: &Path, width: f32, tolerance: f32) -> Result<Mesh, VectorError> {
    let mut buffers: VertexBuffers<Point, u32> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    let options = StrokeOptions::default()
        .with_line_width(width)
        .with_tolerance(tolerance);
    tessellator.tessellate_path(
        &path.inner,
        &options,
        &mut BuffersBuilder::new(&mut buffers, PositionOnly),
    )?;
    Ok(Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
    })
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TOLERANCE, fill, stroke};
    use crate::path::PathBuilder;
    use crate::point::Point;

    fn unit_square() -> crate::path::Path {
        let mut builder = PathBuilder::new();
        builder
            .move_to(Point::new(0.0, 0.0))
            .line_to(Point::new(10.0, 0.0))
            .line_to(Point::new(10.0, 10.0))
            .line_to(Point::new(0.0, 10.0))
            .close();
        builder.build()
    }

    #[test]
    fn filling_a_square_produces_two_triangles() {
        let mesh = match fill(&unit_square(), DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        assert_eq!(mesh.vertices.len(), 4, "a square has 4 distinct corners");
        assert_eq!(
            mesh.indices.len(),
            6,
            "two triangles, 3 indices each, cover a quad"
        );
        // Every index must actually name one of the 4 real vertices --
        // the one property a tessellator absolutely cannot get wrong.
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
    }

    #[test]
    fn filling_an_empty_path_produces_an_empty_mesh() {
        let path = PathBuilder::new().build();
        let mesh = match fill(&path, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn stroking_a_square_produces_a_nonempty_mesh_distinct_from_its_fill() {
        let square = unit_square();
        let filled = match fill(&square, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let stroked = match stroke(&square, 2.0, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        assert!(!stroked.vertices.is_empty());
        assert!(!stroked.indices.is_empty());
        // A stroke needs an inner and outer ring per corner to represent
        // the outline's own width -- structurally more geometry than a
        // flat fill of the same shape, the real, checkable difference
        // between the two rather than just "both non-empty."
        assert!(stroked.vertices.len() > filled.vertices.len());
    }

    #[test]
    fn a_wider_stroke_moves_vertices_further_from_the_path() {
        let square = unit_square();
        let narrow = match stroke(&square, 1.0, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let wide = match stroke(&square, 8.0, DEFAULT_TOLERANCE) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let extent = |mesh: &super::Mesh| -> f32 {
            mesh.vertices
                .iter()
                .fold(0.0_f32, |max, v| max.max(v.x.abs()).max(v.y.abs()))
        };
        assert!(
            extent(&wide) > extent(&narrow),
            "a wider stroke must reach further outward from the same path"
        );
    }
}
