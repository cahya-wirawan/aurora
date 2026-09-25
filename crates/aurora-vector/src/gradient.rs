//! Vertex-coloured gradient geometry: a [`ColorMesh`] carries one
//! straight RGBA colour per vertex, and the GPU interpolates it across
//! each triangle.
//!
//! **What the colours are.** These are UI-chrome colours, like every
//! other colour `aurora-widgets` draws: straight (unpremultiplied)
//! sRGB-gamma-encoded RGBA of 8-bit origin, not colour-managed.
//! Invariant §7.3.6 ("every buffer carries its colour space") governs
//! document and render-graph buffers, and this is neither. A gradient's
//! stop colours are *content*, the same way a `ColorSwatch`'s own colour
//! is content (it shows a value the user picked), not a style value a
//! theme should be able to override. Callers that draw chrome must still
//! resolve chrome colours from design tokens (invariant §7.3.10).
//!
//! **Interpolation space.** Colours are interpolated componentwise on
//! the gamma-encoded values, never in linear light. The GPU consumer
//! (`aurora-widgets`' `GradientPipeline`) linearizes each *fragment*,
//! not each vertex, when its target is sRGB-aware, so an *opaque* mesh
//! stores the same bytes (to within 2 of 255, from the hardware sRGB
//! encode's own rounding) on a plain and an sRGB render target. A
//! *translucent* one does not: blending happens in the target's own
//! blend space (linear light on an sRGB target, gamma-encoded values on
//! a plain one), exactly as it does for a solid fill, so a half-alpha
//! gradient over a dark backdrop lands visibly brighter on an sRGB
//! target. That choice is
//! deliberate (it is exactly HSV's own space, so a colour picker's
//! saturation/value square and hue strip show the colours the picker
//! reports) and is a design decision recorded in PLAN.md for the design
//! owner. Alpha is straight too, so a gradient that varies RGB *and*
//! alpha at once shows the usual straight-alpha fringing toward a
//! transparent stop's own RGB; nothing uses one like that yet.
//!
//! **Degenerate input** — a non-finite coordinate, a non-positive width
//! or height, a width or height too small to move its edge at all in
//! `f32` (`x + width == x`), fewer than two or more than
//! [`MAX_GRADIENT_STOPS`] stops, or a non-finite colour component —
//! yields an empty [`ColorMesh`] rather than an error, the same "nothing
//! to draw" an empty [`crate::Mesh`] already means.
//!
//! **Out-of-range colours are accepted.** A *finite* component outside
//! `[0, 1]` (a negative channel, or one above one) is carried through
//! unchanged, the same way the solid-fill path passes a paint colour
//! through: the GPU consumer decodes it with the same sign-symmetric
//! sRGB curve the solid path uses, and the target clamps on store.

use crate::point::Point;

/// One vertex of a [`ColorMesh`]: a position in the same resolution-
/// independent space [`crate::Path`] uses, and the straight,
/// sRGB-gamma-encoded RGBA colour at that position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorVertex {
    pub position: Point,
    pub color: [f32; 4],
}

/// A triangle mesh with a colour per vertex — [`crate::Mesh`]'s
/// counterpart for a gradient. `indices` holds three entries per
/// triangle, each a valid index into `vertices`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorMesh {
    pub vertices: Vec<ColorVertex>,
    pub indices: Vec<u32>,
}

/// The four corner colours of a [`bilinear_rect`] gradient, each
/// straight sRGB-gamma-encoded RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientCorners {
    pub top_left: [f32; 4],
    pub top_right: [f32; 4],
    pub bottom_left: [f32; 4],
    pub bottom_right: [f32; 4],
}

/// The subdivision [`bilinear_rect`] callers should use unless they have
/// a reason not to: 16 cells per side keeps the worst-case deviation
/// from a true bilinear gradient under half an 8-bit step (see
/// [`bilinear_rect`]).
pub const DEFAULT_GRADIENT_CELLS: u32 = 16;

/// The most cells per side [`bilinear_rect`] builds; a larger request is
/// clamped to this. 256 cells is `257 * 257` vertices, far past the
/// point where subdividing further changes any 8-bit pixel.
const MAX_GRADIENT_CELLS: u32 = 256;

/// The most stops [`horizontal_strip`] accepts; a longer slice yields an
/// empty mesh rather than growing without bound (two vertices per
/// stop). 4096 stops is far past one per physical pixel of any strip a
/// UI draws, so no real caller comes near it.
pub const MAX_GRADIENT_STOPS: usize = 4096;

/// Interpolates `a` toward `b` as `a * (1 - t) + b * t`, which is
/// bit-exact at both ends (`t == 0.0` gives `a`, `t == 1.0` gives `b`,
/// for any finite inputs). The shorter `a + (b - a) * t` is not exact at
/// `t == 1.0`, which would leave a gradient's far corner or last stop a
/// rounding error away from the colour the caller asked for.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let [ar, ag, ab, aa] = a;
    let [br, bg, bb, ba] = b;
    [
        lerp(ar, br, t),
        lerp(ag, bg, t),
        lerp(ab, bb, t),
        lerp(aa, ba, t),
    ]
}

fn color_is_finite(color: [f32; 4]) -> bool {
    color.iter().all(|channel| channel.is_finite())
}

/// `true` when `(x, y, width, height)` describes a real, positive-area
/// rectangle whose far edges are themselves finite and actually move:
/// a width or height too small relative to its origin to change it in
/// `f32` (`x + width == x`) is absorbed, and the would-be rectangle has
/// zero area.
fn rect_is_drawable(x: f32, y: f32, width: f32, height: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
        && (x + width).is_finite()
        && (y + height).is_finite()
        && x + width > x
        && y + height > y
}

/// Pushes the two triangles of one quad whose corners are the vertex
/// indices `top_left`, `top_right`, `bottom_left`, `bottom_right`, split
/// along the `top_right`–`bottom_left` diagonal. Both triangles share
/// one winding, the same one [`crate::fill`] produces for an
/// axis-aligned rectangle.
fn push_quad(
    indices: &mut Vec<u32>,
    top_left: u32,
    top_right: u32,
    bottom_left: u32,
    bottom_right: u32,
) {
    indices.extend_from_slice(&[
        top_left,
        top_right,
        bottom_left,
        top_right,
        bottom_right,
        bottom_left,
    ]);
}

/// A rectangle whose colour is the bilinear blend of four corner
/// colours, subdivided into `cells` by `cells` quads.
///
/// Every vertex carries the exact bilinear colour at its own position
/// (interpolated along x first, then y). Within each quad the GPU
/// interpolates linearly across two triangles, which is *not* bilinear:
/// a single undivided quad would show its centre up to half the full
/// colour range away from the true value. Subdividing bounds that error.
/// Per channel, write `k = top_left - top_right - bottom_left +
/// bottom_right` (the bilinear cross term, `|k| <= 2` for colours in
/// `[0, 1]`); the piecewise-linear surface differs from the bilinear one
/// by at most `|k| / (4 * cells^2)`. At [`DEFAULT_GRADIENT_CELLS`] that
/// is at most `2 / 1024`, about `0.00195` — under half of one 8-bit
/// step. A channel with `k == 0` (a saturation/value square's red
/// channel, a plain one-axis ramp) is reproduced exactly.
///
/// `cells` is clamped to `1..=256`. The four corner vertices carry the
/// four corner colours bit-exactly, and the right and bottom vertex
/// columns sit exactly on `x + width` and `y + height`.
///
/// Returns an empty mesh for degenerate input (see this module's doc).
#[must_use]
pub fn bilinear_rect(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    corners: GradientCorners,
    cells: u32,
) -> ColorMesh {
    let GradientCorners {
        top_left,
        top_right,
        bottom_left,
        bottom_right,
    } = corners;
    if !rect_is_drawable(x, y, width, height)
        || ![top_left, top_right, bottom_left, bottom_right]
            .into_iter()
            .all(color_is_finite)
    {
        return ColorMesh::default();
    }
    let cells = cells.clamp(1, MAX_GRADIENT_CELLS);
    let side = cells + 1;
    let cells_f = cells as f32;

    let mut vertices = Vec::with_capacity((side * side) as usize);
    for row in 0..side {
        let ty = row as f32 / cells_f;
        let py = if row == cells {
            y + height
        } else {
            y + height * ty
        };
        for column in 0..side {
            let tx = column as f32 / cells_f;
            let px = if column == cells {
                x + width
            } else {
                x + width * tx
            };
            let top = lerp_color(top_left, top_right, tx);
            let bottom = lerp_color(bottom_left, bottom_right, tx);
            vertices.push(ColorVertex {
                position: Point::new(px, py),
                color: lerp_color(top, bottom, ty),
            });
        }
    }

    let mut indices = Vec::with_capacity((6 * cells * cells) as usize);
    for row in 0..cells {
        for column in 0..cells {
            let top_left = row * side + column;
            let bottom_left = top_left + side;
            push_quad(
                &mut indices,
                top_left,
                top_left + 1,
                bottom_left,
                bottom_left + 1,
            );
        }
    }
    ColorMesh { vertices, indices }
}

/// A horizontal multi-stop gradient: `stops` spaced evenly from `x` to
/// `x + width`, each stop's colour running the full `height`.
///
/// Stop `i` of `n` sits at `x + width * (i / (n - 1))`, the last exactly
/// at `x + width`. The top and bottom vertex of each stop share its
/// colour, so the result is exactly piecewise-linear between stops (no
/// subdivision needed, unlike [`bilinear_rect`]) and exact at each stop.
///
/// Returns an empty mesh for degenerate input (see this module's doc),
/// which includes fewer than two stops or more than
/// [`MAX_GRADIENT_STOPS`].
#[must_use]
pub fn horizontal_strip(x: f32, y: f32, width: f32, height: f32, stops: &[[f32; 4]]) -> ColorMesh {
    if stops.len() < 2
        || stops.len() > MAX_GRADIENT_STOPS
        || !rect_is_drawable(x, y, width, height)
        || !stops.iter().copied().all(color_is_finite)
    {
        return ColorMesh::default();
    }
    // Two vertices per stop must fit a `u32` index; a slice that long
    // could never be uploaded anyway.
    let Some(vertex_count) = stops
        .len()
        .checked_mul(2)
        .and_then(|count| u32::try_from(count).ok())
    else {
        return ColorMesh::default();
    };
    let last = (stops.len() - 1) as f32;
    let bottom = y + height;

    let mut vertices = Vec::with_capacity(vertex_count as usize);
    for (index, &color) in stops.iter().enumerate() {
        let edge = if index + 1 == stops.len() {
            x + width
        } else {
            x + width * (index as f32 / last)
        };
        vertices.push(ColorVertex {
            position: Point::new(edge, y),
            color,
        });
        vertices.push(ColorVertex {
            position: Point::new(edge, bottom),
            color,
        });
    }

    let segments = vertex_count / 2 - 1;
    let mut indices = Vec::with_capacity((6 * segments) as usize);
    for segment in 0..segments {
        let top_left = segment * 2;
        push_quad(
            &mut indices,
            top_left,
            top_left + 2,
            top_left + 1,
            top_left + 3,
        );
    }
    ColorMesh { vertices, indices }
}

#[cfg(test)]
// Exact float equality is the point of these tests: stop edges and
// corner colours are promised bit-exact, not approximately right.
#[allow(clippy::float_cmp)]
mod tests {
    use super::{
        ColorMesh, DEFAULT_GRADIENT_CELLS, GradientCorners, MAX_GRADIENT_CELLS, MAX_GRADIENT_STOPS,
        bilinear_rect, horizontal_strip,
    };

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    /// The four corners every bilinear test uses: every channel has a
    /// non-zero cross term, and the true bilinear centre is exactly
    /// `(0.5, 0.5, 0.5, 1.0)`.
    const CORNERS: GradientCorners = GradientCorners {
        top_left: RED,
        top_right: GREEN,
        bottom_left: BLUE,
        bottom_right: WHITE,
    };

    fn bits(color: [f32; 4]) -> [u32; 4] {
        color.map(f32::to_bits)
    }

    fn vertex_at(mesh: &ColorMesh, index: usize) -> super::ColorVertex {
        match mesh.vertices.get(index) {
            Some(vertex) => *vertex,
            None => unreachable!("vertex {index} out of range ({})", mesh.vertices.len()),
        }
    }

    /// Every index is in range and every triangle is non-degenerate with
    /// the same winding sign as the first.
    fn assert_triangles_valid(mesh: &ColorMesh) {
        assert_eq!(mesh.indices.len() % 3, 0);
        let mut first_sign = None;
        for triangle in mesh.indices.chunks_exact(3) {
            let &[a, b, c] = triangle else {
                unreachable!("chunks_exact(3)");
            };
            for index in [a, b, c] {
                assert!(
                    (index as usize) < mesh.vertices.len(),
                    "index {index} out of range ({} vertices)",
                    mesh.vertices.len()
                );
            }
            let (pa, pb, pc) = (
                vertex_at(mesh, a as usize).position,
                vertex_at(mesh, b as usize).position,
                vertex_at(mesh, c as usize).position,
            );
            let area = (pb.x - pa.x) * (pc.y - pa.y) - (pc.x - pa.x) * (pb.y - pa.y);
            assert!(area != 0.0, "degenerate triangle {a} {b} {c}");
            let sign = area > 0.0;
            match first_sign {
                None => first_sign = Some(sign),
                Some(first) => assert_eq!(sign, first, "triangle {a} {b} {c} winds the other way"),
            }
        }
    }

    // C1
    #[test]
    fn bilinear_rect_subdivides_and_keeps_corners_bit_exact() {
        let mesh = bilinear_rect(10.0, 20.0, 256.0, 128.0, CORNERS, DEFAULT_GRADIENT_CELLS);
        assert_eq!(mesh.vertices.len(), 17 * 17);
        assert_eq!(mesh.indices.len(), 6 * 16 * 16);
        assert_triangles_valid(&mesh);

        let top_left = vertex_at(&mesh, 0);
        let top_right = vertex_at(&mesh, 16);
        let bottom_left = vertex_at(&mesh, 16 * 17);
        let bottom_right = vertex_at(&mesh, 17 * 17 - 1);
        assert_eq!((top_left.position.x, top_left.position.y), (10.0, 20.0));
        assert_eq!((top_right.position.x, top_right.position.y), (266.0, 20.0));
        assert_eq!(
            (bottom_left.position.x, bottom_left.position.y),
            (10.0, 148.0)
        );
        assert_eq!(
            (bottom_right.position.x, bottom_right.position.y),
            (266.0, 148.0)
        );
        assert_eq!(bits(top_left.color), bits(RED));
        assert_eq!(bits(top_right.color), bits(GREEN));
        assert_eq!(bits(bottom_left.color), bits(BLUE));
        assert_eq!(bits(bottom_right.color), bits(WHITE));
    }

    /// Corners that are not `0`/`1`, so `a + (b - a) * t` and
    /// `a * (1 - t) + b * t` actually differ at `t == 1`.
    #[test]
    fn bilinear_rect_far_corner_is_bit_exact_for_awkward_colours() {
        let corners = GradientCorners {
            top_left: [0.1, 0.7, 0.3, 0.9],
            top_right: [0.3, 0.9, 0.7, 0.1],
            bottom_left: [0.7, 0.1, 0.9, 0.3],
            bottom_right: [0.9, 0.3, 0.1, 0.7],
        };
        for cells in [1, 3, 7, DEFAULT_GRADIENT_CELLS] {
            let mesh = bilinear_rect(0.0, 0.0, 1.0, 1.0, corners, cells);
            let side = (cells + 1) as usize;
            assert_eq!(bits(vertex_at(&mesh, 0).color), bits(corners.top_left));
            assert_eq!(
                bits(vertex_at(&mesh, side - 1).color),
                bits(corners.top_right)
            );
            assert_eq!(
                bits(vertex_at(&mesh, side * (side - 1)).color),
                bits(corners.bottom_left)
            );
            assert_eq!(
                bits(vertex_at(&mesh, side * side - 1).color),
                bits(corners.bottom_right),
                "cells = {cells}"
            );
        }
    }

    // C2
    #[test]
    fn bilinear_rect_centre_vertex_is_the_bilinear_mean() {
        let mesh = bilinear_rect(0.0, 0.0, 64.0, 64.0, CORNERS, DEFAULT_GRADIENT_CELLS);
        let centre = vertex_at(&mesh, 8 * 17 + 8);
        assert_eq!((centre.position.x, centre.position.y), (32.0, 32.0));
        for (channel, value) in centre.color.iter().zip([0.5, 0.5, 0.5, 1.0]) {
            assert!(
                (channel - value).abs() < 1e-6,
                "centre {:?} is not the bilinear mean",
                centre.color
            );
        }
    }

    /// Every vertex, not just the centre, carries the true bilinear
    /// colour at its own position.
    #[test]
    fn bilinear_rect_every_vertex_is_bilinear() {
        let mesh = bilinear_rect(0.0, 0.0, 1.0, 1.0, CORNERS, 5);
        for vertex in &mesh.vertices {
            let (u, v) = (vertex.position.x, vertex.position.y);
            let expected = [
                (1.0 - u) * (1.0 - v) + u * v,
                u * (1.0 - v) + u * v,
                (1.0 - u) * v + u * v,
                1.0,
            ];
            for (got, want) in vertex.color.iter().zip(expected) {
                assert!((got - want).abs() < 1e-6, "{vertex:?} vs {expected:?}");
            }
        }
    }

    // C3
    #[test]
    fn horizontal_strip_places_seven_stops_exactly() {
        let stops = [
            RED,
            [1.0, 1.0, 0.0, 1.0],
            GREEN,
            [0.0, 1.0, 1.0, 1.0],
            BLUE,
            [1.0, 0.0, 1.0, 1.0],
            RED,
        ];
        let (x, y, width, height) = (3.25, 4.5, 383.0, 8.0);
        let mesh = horizontal_strip(x, y, width, height, &stops);
        assert_eq!(mesh.vertices.len(), 14);
        assert_eq!(mesh.indices.len(), 36);
        assert_triangles_valid(&mesh);
        for (index, stop) in stops.iter().enumerate() {
            let top = vertex_at(&mesh, 2 * index);
            let bottom = vertex_at(&mesh, 2 * index + 1);
            let expected_x = if index == 6 {
                x + width
            } else {
                x + width * (index as f32 / 6.0)
            };
            assert_eq!(top.position.x.to_bits(), expected_x.to_bits());
            assert_eq!(bottom.position.x.to_bits(), expected_x.to_bits());
            assert_eq!(top.position.y, y);
            assert_eq!(bottom.position.y, y + height);
            assert_eq!(bits(top.color), bits(*stop));
            assert_eq!(bits(bottom.color), bits(*stop));
        }
        assert_eq!(vertex_at(&mesh, 12).position.x, x + width);
    }

    // C4
    #[test]
    fn gradients_produce_no_nan_across_a_sweep() {
        for cells in [0, 1, 2, 5, 16, 63, 256, 1000] {
            for &(width, height) in &[(0.001_f32, 0.001_f32), (1.0, 1.0), (1.0e6, 3.0)] {
                let mesh = bilinear_rect(-5.0, 7.0, width, height, CORNERS, cells);
                for vertex in &mesh.vertices {
                    assert!(vertex.position.x.is_finite() && vertex.position.y.is_finite());
                    assert!(vertex.color.iter().all(|c| c.is_finite()));
                }
            }
        }
        for count in 2..=12 {
            let stops: Vec<[f32; 4]> = (0..count)
                .map(|i| {
                    let t = i as f32 / (count - 1) as f32;
                    [t, 1.0 - t, 0.5, t]
                })
                .collect();
            let mesh = horizontal_strip(0.0, 0.0, 100.0, 10.0, &stops);
            assert_eq!(mesh.vertices.len(), 2 * count);
            for vertex in &mesh.vertices {
                assert!(vertex.position.x.is_finite() && vertex.position.y.is_finite());
                assert!(vertex.color.iter().all(|c| c.is_finite()));
            }
        }
    }

    // C5
    #[test]
    fn degenerate_input_yields_an_empty_mesh() {
        let empty = ColorMesh::default();
        for (x, y, width, height) in [
            (0.0, 0.0, 0.0, 10.0),
            (0.0, 0.0, 10.0, 0.0),
            (0.0, 0.0, -1.0, 10.0),
            (f32::NAN, 0.0, 10.0, 10.0),
            (0.0, f32::INFINITY, 10.0, 10.0),
            (0.0, 0.0, f32::NAN, 10.0),
            (0.0, 0.0, 10.0, f32::INFINITY),
            (f32::MAX, 0.0, f32::MAX, 10.0),
            // Absorbed: finite and positive, but too small to move the
            // edge (`1e8 + 1.0 == 1e8` in f32), so zero real area.
            (1.0e8, 0.0, 1.0, 10.0),
            (0.0, 1.0e8, 10.0, 1.0),
        ] {
            assert_eq!(bilinear_rect(x, y, width, height, CORNERS, 4), empty);
            assert_eq!(horizontal_strip(x, y, width, height, &[RED, BLUE]), empty);
        }
        let bad = GradientCorners {
            bottom_right: [1.0, f32::NAN, 1.0, 1.0],
            ..CORNERS
        };
        assert_eq!(bilinear_rect(0.0, 0.0, 10.0, 10.0, bad, 4), empty);
        assert_eq!(horizontal_strip(0.0, 0.0, 10.0, 10.0, &[]), empty);
        assert_eq!(horizontal_strip(0.0, 0.0, 10.0, 10.0, &[RED]), empty);
        // Stop count is capped: exactly the cap is drawn, one more is not.
        let at_cap = vec![RED; MAX_GRADIENT_STOPS];
        assert_eq!(
            horizontal_strip(0.0, 0.0, 10.0, 10.0, &at_cap)
                .vertices
                .len(),
            2 * MAX_GRADIENT_STOPS
        );
        let over_cap = vec![RED; MAX_GRADIENT_STOPS + 1];
        assert_eq!(horizontal_strip(0.0, 0.0, 10.0, 10.0, &over_cap), empty);
        assert_eq!(
            horizontal_strip(0.0, 0.0, 10.0, 10.0, &[RED, [0.0, 0.0, f32::INFINITY, 1.0]]),
            empty
        );
    }

    // C6
    #[test]
    fn cells_are_clamped_to_a_real_range() {
        let zero = bilinear_rect(0.0, 0.0, 10.0, 10.0, CORNERS, 0);
        assert_eq!(zero, bilinear_rect(0.0, 0.0, 10.0, 10.0, CORNERS, 1));
        assert_eq!(zero.vertices.len(), 4);
        assert_eq!(zero.indices.len(), 6);

        let huge = bilinear_rect(0.0, 0.0, 10.0, 10.0, CORNERS, u32::MAX);
        let side = (MAX_GRADIENT_CELLS + 1) as usize;
        assert_eq!(huge.vertices.len(), side * side);
        assert_eq!(
            huge.indices.len(),
            6 * (MAX_GRADIENT_CELLS * MAX_GRADIENT_CELLS) as usize
        );
    }

    // C7
    #[test]
    fn every_mesh_has_valid_indices_and_one_winding() {
        for cells in [1, 2, 3, 16] {
            assert_triangles_valid(&bilinear_rect(1.0, 2.0, 30.0, 40.0, CORNERS, cells));
        }
        assert_triangles_valid(&horizontal_strip(1.0, 2.0, 30.0, 40.0, &[RED, GREEN]));
        assert_triangles_valid(&horizontal_strip(1.0, 2.0, 30.0, 40.0, &[RED, GREEN, BLUE]));
    }
}
