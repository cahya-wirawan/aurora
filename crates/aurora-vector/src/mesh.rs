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

/// [`DEFAULT_TOLERANCE`], adjusted for a window's DPI `scale_factor`
/// (`winit::window::Window::scale_factor`, e.g. `2.0` on a Retina/HiDPI
/// display) so tessellation quality tracks *physical* pixel density
/// rather than staying fixed in logical units.
///
/// A [`Path`] built from a widget's layout bounds lives in logical
/// pixels (`aurora_core::Rect`, resolved by `taffy`), but `tolerance`
/// itself is a distance in that same logical-path space — the maximum
/// the tessellated polyline is allowed to deviate from the true curve.
/// At `scale_factor == 1.0` one logical pixel is one physical pixel, so
/// [`DEFAULT_TOLERANCE`] (`0.1`) is already a sub-physical-pixel
/// deviation, imperceptible. At `scale_factor == 2.0`, though, that same
/// `0.1`-logical-pixel deviation now spans `0.2` *physical* pixels — the
/// polyline is flattened as if the display were half as dense as it
/// actually is, so facets become visible that a `1.0`-scale window
/// wouldn't show. Dividing by `scale_factor` cancels that out: the
/// *physical*-pixel tolerance (`logical_tolerance * scale_factor`,
/// since one logical pixel covers `scale_factor` physical pixels) stays
/// exactly `DEFAULT_TOLERANCE` regardless of DPI, which is the actual
/// goal — a curve should look equally smooth at any density, not just
/// occupy equally many logical units.
///
/// Degenerate `scale_factor` values (`<= 0.0`, `NaN`, infinite — never
/// legitimate, unlike a fractional value below `1.0`, which some Linux
/// compositors report for real when scaling *down*) fall back to an
/// effective `1.0`, returning [`DEFAULT_TOLERANCE`] unchanged — the same
/// fallback `aurora-app`'s own `logical_size`/`logical_point` already
/// use for the identical class of value `winit` should never actually
/// report.
#[must_use]
pub fn tolerance_for_scale_factor(scale_factor: f32) -> f32 {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    DEFAULT_TOLERANCE / scale_factor
}

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
    use super::{DEFAULT_TOLERANCE, fill, stroke, tolerance_for_scale_factor};
    use crate::path::PathBuilder;
    use crate::point::Point;
    use crate::shapes::rounded_rect;

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

    #[test]
    // `tolerance_for_scale_factor(1.0)` divides `DEFAULT_TOLERANCE` by
    // exactly `1.0` -- bit-exact by construction, not accumulated float
    // noise, the same precedent `aurora_color`'s own round-trip tests
    // and `aurora-widgets::paint`'s tests already allow this lint for.
    #[allow(clippy::float_cmp)]
    fn tolerance_for_scale_factor_is_the_default_at_one() {
        assert_eq!(tolerance_for_scale_factor(1.0), DEFAULT_TOLERANCE);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn tolerance_for_scale_factor_shrinks_as_scale_factor_grows() {
        let doubled = tolerance_for_scale_factor(2.0);
        assert_eq!(doubled, DEFAULT_TOLERANCE / 2.0);
        assert!(doubled < DEFAULT_TOLERANCE);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn tolerance_for_scale_factor_falls_back_to_the_default_for_non_positive_or_non_finite_input() {
        for degenerate in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let tolerance = tolerance_for_scale_factor(degenerate);
            assert_eq!(
                tolerance, DEFAULT_TOLERANCE,
                "scale_factor {degenerate} must fall back to the default tolerance"
            );
            assert!(tolerance.is_finite() && tolerance > 0.0);
        }
    }

    /// The real effect a scale-factor-aware tolerance exists to produce:
    /// tessellating the *same curved shape* (a rounded rect with a real
    /// radius -- a plain rectangle has no curve for tolerance to affect
    /// at all, lyon flattens its straight edges identically regardless
    /// of tolerance) at a higher scale factor must yield strictly more
    /// geometry than at a lower one, not just a different number fed
    /// into the tessellator. This is the property the whole feature is
    /// for: a `HiDPI` window's curves must look as smooth as a 1x
    /// window's, which only holds if more triangles actually appear.
    #[test]
    fn a_higher_scale_factor_tessellates_a_rounded_rect_with_strictly_more_geometry() {
        let shape = rounded_rect(0.0, 0.0, 64.0, 64.0, 16.0);

        let at_1x = match fill(&shape, tolerance_for_scale_factor(1.0)) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let at_2x = match fill(&shape, tolerance_for_scale_factor(2.0)) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let at_4x = match fill(&shape, tolerance_for_scale_factor(4.0)) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };

        assert!(
            at_2x.vertices.len() > at_1x.vertices.len(),
            "2x ({} vertices) must tessellate finer than 1x ({} vertices)",
            at_2x.vertices.len(),
            at_1x.vertices.len()
        );
        assert!(
            at_2x.indices.len() > at_1x.indices.len(),
            "2x ({} indices) must produce more triangles than 1x ({} indices)",
            at_2x.indices.len(),
            at_1x.indices.len()
        );
        assert!(
            at_4x.vertices.len() > at_2x.vertices.len(),
            "the relationship must hold again going from 2x to 4x, not just once: \
             4x ({} vertices) vs 2x ({} vertices)",
            at_4x.vertices.len(),
            at_2x.vertices.len()
        );
    }

    /// The same effect, for [`stroke`] -- a rounded rect's curved
    /// outline, not just its fill, must gain vertices at a higher scale
    /// factor too.
    #[test]
    fn a_higher_scale_factor_strokes_a_rounded_rect_with_strictly_more_geometry() {
        let shape = rounded_rect(0.0, 0.0, 64.0, 64.0, 16.0);

        let at_1x = match stroke(&shape, 2.0, tolerance_for_scale_factor(1.0)) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };
        let at_2x = match stroke(&shape, 2.0, tolerance_for_scale_factor(2.0)) {
            Ok(mesh) => mesh,
            Err(err) => unreachable!("{err}"),
        };

        assert!(
            at_2x.vertices.len() > at_1x.vertices.len(),
            "2x ({} vertices) must stroke finer than 1x ({} vertices)",
            at_2x.vertices.len(),
            at_1x.vertices.len()
        );
    }
}
