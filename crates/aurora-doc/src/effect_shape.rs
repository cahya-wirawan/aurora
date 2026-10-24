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
//! travels as [`PackedEffectShape`] (positional `postcard`, the plane
//! lz4-compressed and expanded only when applied, with its area checked
//! against [`MAX_EFFECT_SHAPE_PIXELS`] first) in `.aur`'s optional
//! `layer-effect-shapes` entry and in the appended `LayerOp::SetEffectShape`
//! that [`crate::History::save_journal`] emits after a journaled `Restore`
//! of a subtree that carries one.

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
    /// A hash of the area and the plane, taken once at construction: what a
    /// cache of anything derived from the plane keys on.
    content_hash: u64,
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
        let content_hash = hash_plane(area, &coverage);
        Ok(Self {
            area,
            coverage,
            pixel_fingerprint,
            content_hash,
        })
    }

    /// A hash of the plane and its area (never `0`), for cache keys.
    #[must_use]
    pub fn content_hash(&self) -> u64 {
        self.content_hash
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

/// FNV-1a over the area and the plane, forced non-zero.
fn hash_plane(area: Rect, coverage: &[u8]) -> u64 {
    const PRIME: u64 = 0x0100_0000_01B3;
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut mix = |v: u64| {
        h = (h ^ v).wrapping_mul(PRIME);
    };
    mix(area.x.cast_unsigned());
    mix(area.y.cast_unsigned());
    mix(u64::from(area.width));
    mix(u64::from(area.height));
    for chunk in coverage.chunks(8) {
        let mut word = 0u64;
        for (i, b) in chunk.iter().enumerate() {
            word |= u64::from(*b) << (8 * i);
        }
        mix(word);
    }
    h | 1
}

/// An [`EffectShape`] as it travels (journal op, `.aur` entry): the plane
/// lz4-compressed, **not yet expanded**. Deserialising one allocates no more
/// than its input; [`Self::unpack`] checks the declared size against
/// [`MAX_EFFECT_SHAPE_PIXELS`] before it expands anything, and its caller
/// expands one shape at a time against the document's total bound, so a
/// hostile file cannot make a long list of small payloads each decompress to
/// a full plane at once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackedEffectShape {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    pixel_fingerprint: u64,
    packed: Vec<u8>,
}

impl PackedEffectShape {
    /// The packed form of `shape`.
    #[must_use]
    pub fn of(shape: &EffectShape) -> Self {
        Self {
            x: shape.area.x,
            y: shape.area.y,
            width: shape.area.width,
            height: shape.area.height,
            pixel_fingerprint: shape.pixel_fingerprint,
            packed: lz4_flex::block::compress(&shape.coverage),
        }
    }

    /// The shape, expanded.
    ///
    /// # Errors
    ///
    /// [`EffectShapeError::BadSize`] for an empty or oversized declared
    /// area (checked before anything is allocated);
    /// [`EffectShapeError::LengthMismatch`] when the payload does not
    /// expand to exactly `width * height` bytes.
    pub fn unpack(&self) -> Result<EffectShape, EffectShapeError> {
        let pixels = u64::from(self.width) * u64::from(self.height);
        if pixels == 0 || pixels > MAX_EFFECT_SHAPE_PIXELS {
            return Err(EffectShapeError::BadSize);
        }
        let size = usize::try_from(pixels).map_err(|_| EffectShapeError::BadSize)?;
        let coverage = lz4_flex::block::decompress(&self.packed, size)
            .map_err(|_| EffectShapeError::LengthMismatch)?;
        EffectShape::new(
            Rect {
                x: self.x,
                y: self.y,
                width: self.width,
                height: self.height,
            },
            coverage,
            self.pixel_fingerprint,
        )
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
        let bytes = postcard::to_allocvec(&PackedEffectShape::of(&shape)).unwrap_or_default();
        assert!(!bytes.is_empty());
        let back: Result<PackedEffectShape, _> = postcard::from_bytes(&bytes);
        assert_eq!(back.ok().and_then(|p| p.unpack().ok()), Some(shape));
    }

    #[test]
    fn a_hostile_size_is_refused_without_allocating_the_plane() {
        // A declared 4 billion pixel plane with a two-byte payload.
        let hostile = PackedEffectShape {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
            pixel_fingerprint: 0,
            packed: vec![0, 0],
        };
        assert_eq!(hostile.unpack(), Err(EffectShapeError::BadSize));
        // A plausible size whose payload decodes to fewer bytes.
        let short = PackedEffectShape {
            width: 8,
            height: 8,
            packed: vec![0x10, 7],
            ..hostile.clone()
        };
        assert_eq!(short.unpack(), Err(EffectShapeError::LengthMismatch));
        // A payload that would expand past the declared size is refused too.
        let over = PackedEffectShape {
            width: 2,
            height: 2,
            packed: lz4_flex::block::compress(&[1u8; 64]),
            ..hostile
        };
        assert_eq!(over.unpack(), Err(EffectShapeError::LengthMismatch));
    }
}
