//! A shape layer's *effect shape* (task 151): the coverage of the vector
//! path that Photoshop draws a shape layer's strokes, glows and shadows
//! from, kept beside the layer's stored pixels.
//!
//! Why it exists. A shape layer's stored pixels are its fill times the
//! path's coverage: a gradient fill that fades to transparent leaves pixels
//! that fade to transparent, and the pixels' alpha is then nothing like the
//! shape's outline. Photoshop strokes, glows and shadows the *outline*
//! (`stroke-effects.psd`: a green frame around the whole square, the
//! interior red over white), whereas deriving them from the pixel alpha
//! draws the frame where the alpha is, trapezoids and all. Aurora keeps no
//! vector path after open (the path is rasterised once), so this is the
//! rasterised path, as coverage.
//!
//! Like a mask's coverage it is an 8-bit coverage plane (invariant 7.3.1b
//! concerns colour intermediates; a coverage mask is the same kind of data
//! as a layer mask), `0` outside, `255` inside, anti-aliased on the edge.
//!
//! It is **tied to the pixels it was taken beside**: [`EffectShape::pixel_fingerprint`]
//! is the layer's pixel fingerprint (the smart-object fingerprint,
//! [`crate::smart_object::surface_fingerprint`]) at the moment the shape was
//! recorded. The compositor uses the shape only while the layer's pixels
//! still have that fingerprint, so painting, erasing or filtering the layer
//! (Photoshop rasterises a shape layer when it is painted on) makes its
//! effects fall back to the pixel alpha, and undoing the edit brings the
//! shape back. A Move does not change the fingerprint (tiles are
//! layer-local) and the shape is positioned relative to the layer's own
//! bounds, so it travels with the layer.
//!
//! Persistence: `LayerEntry::effect_shape` is `#[serde(skip)]`; the shape
//! travels as `.aur`'s optional `layer-effect-shapes` entry and as the
//! appended `LayerOp::SetEffectShape` that [`crate::History::save_journal`]
//! emits after a journaled `Restore` of a subtree that carries one. The wire
//! is positional `postcard` of `(x, y, width, height, fingerprint,
//! lz4-compressed plane)`, decoded with the area checked against
//! [`MAX_EFFECT_SHAPE_PIXELS`] before anything is allocated.

use aurora_core::Rect;

/// The most pixels one effect shape may cover (a 2,048 x 2,048 box). An
/// engineering bound: a shape layer with effects is normally small, and the
/// plane is held uncompressed in memory.
pub const MAX_EFFECT_SHAPE_PIXELS: u64 = 4 * 1024 * 1024;

/// The most pixels the effect shapes of one document may cover together
/// (32 MiB of planes).
pub const MAX_EFFECT_SHAPE_TOTAL_PIXELS: u64 = 32 * 1024 * 1024;

/// A layer's effect shape, see the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectShape {
    /// Where the plane lies, in the layer's own coordinates: `x`/`y` are
    /// offsets from the layer's `bounds` origin (so a Move carries the shape
    /// along), `width`/`height` its size.
    area: Rect,
    /// Row-major over `area`, `0..=255`; `width * height` bytes.
    coverage: Vec<u8>,
    /// The layer's pixel fingerprint when this shape was recorded.
    pixel_fingerprint: u64,
}

/// Why an [`EffectShape`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EffectShapeError {
    /// The plane is empty or larger than [`MAX_EFFECT_SHAPE_PIXELS`].
    #[error("the effect shape's size is out of bounds")]
    BadSize,
    /// The coverage does not hold exactly `width * height` bytes.
    #[error("the effect shape's coverage does not match its size")]
    LengthMismatch,
}

impl EffectShape {
    /// A shape from its parts.
    ///
    /// # Errors
    ///
    /// [`EffectShapeError`] when the area is empty or over
    /// [`MAX_EFFECT_SHAPE_PIXELS`], or `coverage` is not `width * height`
    /// bytes.
    pub fn new(
        area: Rect,
        coverage: Vec<u8>,
        pixel_fingerprint: u64,
    ) -> Result<Self, EffectShapeError> {
        let pixels = u64::from(area.width) * u64::from(area.height);
        if pixels == 0 || pixels > MAX_EFFECT_SHAPE_PIXELS {
            return Err(EffectShapeError::BadSize);
        }
        if u64::try_from(coverage.len()) != Ok(pixels) {
            return Err(EffectShapeError::LengthMismatch);
        }
        Ok(Self {
            area,
            coverage,
            pixel_fingerprint,
        })
    }

    /// The plane's rectangle relative to the layer's bounds origin.
    #[must_use]
    pub fn area(&self) -> Rect {
        self.area
    }

    /// The plane, row-major over [`Self::area`].
    #[must_use]
    pub fn coverage(&self) -> &[u8] {
        &self.coverage
    }

    /// The layer's pixel fingerprint the shape was recorded beside.
    #[must_use]
    pub fn pixel_fingerprint(&self) -> u64 {
        self.pixel_fingerprint
    }

    /// The pixel count of the plane.
    #[must_use]
    pub fn pixels(&self) -> u64 {
        u64::from(self.area.width) * u64::from(self.area.height)
    }

    /// Coverage `0..=255` at layer-relative `(x, y)`; `0` outside the plane.
    #[must_use]
    pub fn at(&self, x: i64, y: i64) -> u8 {
        let (dx, dy) = (x - self.area.x, y - self.area.y);
        let (Ok(dx), Ok(dy)) = (u32::try_from(dx), u32::try_from(dy)) else {
            return 0;
        };
        if dx >= self.area.width || dy >= self.area.height {
            return 0;
        }
        let index = u64::from(dy) * u64::from(self.area.width) + u64::from(dx);
        usize::try_from(index)
            .ok()
            .and_then(|i| self.coverage.get(i))
            .copied()
            .unwrap_or(0)
    }
}

/// The wire form: `(x, y, width, height, fingerprint, lz4 plane)`.
type Wire<'a> = (i64, i64, u32, u32, u64, std::borrow::Cow<'a, [u8]>);

impl serde::Serialize for EffectShape {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let packed = lz4_flex::block::compress(&self.coverage);
        let wire: Wire<'_> = (
            self.area.x,
            self.area.y,
            self.area.width,
            self.area.height,
            self.pixel_fingerprint,
            std::borrow::Cow::Owned(packed),
        );
        wire.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for EffectShape {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let (x, y, width, height, fingerprint, packed) = <Wire<'static>>::deserialize(deserializer)?;
        // The size is checked before the plane is allocated: `decompress`
        // allocates exactly the length it is told.
        let pixels = u64::from(width) * u64::from(height);
        if pixels == 0 || pixels > MAX_EFFECT_SHAPE_PIXELS {
            return Err(D::Error::custom(EffectShapeError::BadSize));
        }
        let size = usize::try_from(pixels).map_err(|_| D::Error::custom("size"))?;
        let coverage = lz4_flex::block::decompress(&packed, size)
            .map_err(|err| D::Error::custom(format!("effect shape plane: {err}")))?;
        Self::new(
            Rect {
                x,
                y,
                width,
                height,
            },
            coverage,
            fingerprint,
        )
        .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i64, y: i64, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn square() -> EffectShape {
        let mut plane = vec![0u8; 16];
        for (i, v) in plane.iter_mut().enumerate() {
            *v = if (5..=6).contains(&i) || (9..=10).contains(&i) {
                255
            } else {
                u8::try_from(i * 3).unwrap_or(0)
            };
        }
        EffectShape::new(rect(-1, 2, 4, 4), plane, 77).unwrap_or_else(|_| unreachable!("valid"))
    }

    #[test]
    fn at_reads_inside_and_is_zero_outside() {
        let shape = square();
        assert_eq!(shape.at(0, 3), 255);
        assert_eq!(shape.at(-1, 2), 0);
        assert_eq!(shape.at(2, 5), 45);
        assert_eq!(shape.at(-2, 2), 0);
        assert_eq!(shape.at(3, 2), 0);
        assert_eq!(shape.at(0, 6), 0);
        assert_eq!(shape.at(0, 1), 0);
    }

    #[test]
    fn new_refuses_an_empty_an_oversized_and_a_mismatched_plane() {
        assert_eq!(
            EffectShape::new(rect(0, 0, 0, 4), Vec::new(), 0),
            Err(EffectShapeError::BadSize)
        );
        assert_eq!(
            EffectShape::new(rect(0, 0, 2049, 2048), Vec::new(), 0),
            Err(EffectShapeError::BadSize)
        );
        assert_eq!(
            EffectShape::new(rect(0, 0, 2, 2), vec![0; 3], 0),
            Err(EffectShapeError::LengthMismatch)
        );
    }

    #[test]
    fn the_wire_round_trips() {
        let shape = square();
        let bytes = postcard::to_allocvec(&shape).unwrap_or_default();
        assert!(!bytes.is_empty());
        let back: Result<EffectShape, _> = postcard::from_bytes(&bytes);
        assert_eq!(back.ok(), Some(shape));
    }

    #[test]
    fn a_hostile_size_is_refused_without_allocating_the_plane() {
        // A declared 4 billion pixel plane with a two-byte payload.
        let wire: Wire<'_> = (0, 0, u32::MAX, u32::MAX, 0, std::borrow::Cow::Borrowed(&[0, 0]));
        let bytes = postcard::to_allocvec(&wire).unwrap_or_default();
        assert!(postcard::from_bytes::<EffectShape>(&bytes).is_err());
        // A plausible size whose compressed payload decodes to fewer bytes.
        let wire: Wire<'_> = (0, 0, 8, 8, 0, std::borrow::Cow::Borrowed(&[0x10, 7]));
        let bytes = postcard::to_allocvec(&wire).unwrap_or_default();
        assert!(postcard::from_bytes::<EffectShape>(&bytes).is_err());
    }
}
