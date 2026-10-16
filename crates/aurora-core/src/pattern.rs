//! Pattern pool entries (task 111): the pixels a Photoshop pattern overlay
//! tiles over a layer.
//!
//! A pattern is a small straight-alpha RGBA image held at 16 bits a
//! channel (`u16`, an 8-bit source is promoted exactly: `v * 257`), so the
//! pool is bounded by dimensions, not by what a file claims: every size is
//! checked before anything is allocated.

use std::sync::Arc;

/// Most texels one pattern may hold (4 Mpx, 32 MiB as RGBA `u16`).
pub const MAX_PATTERN_TEXELS: u64 = 4 * 1024 * 1024;
/// Longest side of a pattern.
pub const MAX_PATTERN_SIDE: u32 = 8192;
/// Most bytes of decoded pattern pixels a document's pool may hold.
pub const MAX_PATTERN_POOL_BYTES: u64 = 64 * 1024 * 1024;
/// Most patterns a document's pool may hold.
pub const MAX_PATTERNS: usize = 256;
/// Longest pattern ID (a Photoshop one is a 36-character UUID).
pub const MAX_PATTERN_ID_BYTES: usize = 128;
/// Longest pattern name kept, in characters.
pub const MAX_PATTERN_NAME_CHARS: usize = 256;

/// Why a pattern was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// An empty or over-long ID.
    #[error("a pattern ID must be 1 to {MAX_PATTERN_ID_BYTES} bytes")]
    BadId,
    /// A zero side, a side over [`MAX_PATTERN_SIDE`] or more than
    /// [`MAX_PATTERN_TEXELS`] texels.
    #[error("the pattern's size is out of range")]
    BadSize,
    /// The pixel buffer is not `width * height * 4` samples.
    #[error("the pattern's pixel buffer does not match its size")]
    BadPixels,
    /// More patterns, or more pixel bytes, than the pool allows.
    #[error("the document's pattern pool is full")]
    PoolTooLarge,
}

/// One pattern: straight RGBA, `u16` per channel, row-major, `width` by
/// `height` texels.
#[derive(Clone, PartialEq, Eq)]
pub struct Pattern {
    id: String,
    name: String,
    width: u32,
    height: u32,
    texels: Arc<Vec<u16>>,
}

impl std::fmt::Debug for Pattern {
    /// Without the pixels: a pattern can be 32 MiB.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pattern")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("size", &(self.width, self.height))
            .finish()
    }
}

impl Pattern {
    /// A pattern over `texels` (RGBA, `width * height * 4` samples).
    ///
    /// # Errors
    ///
    /// [`PatternError`] for an out-of-range ID or size, or a buffer of the
    /// wrong length. Nothing is allocated here.
    pub fn new(
        id: String,
        name: String,
        width: u32,
        height: u32,
        texels: Vec<u16>,
    ) -> Result<Self, PatternError> {
        Self::from_shared(id, name, width, height, Arc::new(texels))
    }

    /// [`Self::new`] over shared pixels.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn from_shared(
        id: String,
        name: String,
        width: u32,
        height: u32,
        texels: Arc<Vec<u16>>,
    ) -> Result<Self, PatternError> {
        if id.is_empty() || id.len() > MAX_PATTERN_ID_BYTES {
            return Err(PatternError::BadId);
        }
        let count = Self::texel_count(width, height)?;
        if u64::try_from(texels.len()).ok() != count.checked_mul(4) {
            return Err(PatternError::BadPixels);
        }
        let name: String = name.chars().take(MAX_PATTERN_NAME_CHARS).collect();
        Ok(Self {
            id,
            name,
            width,
            height,
            texels,
        })
    }

    /// `width * height`, or [`PatternError::BadSize`].
    ///
    /// # Errors
    ///
    /// [`PatternError::BadSize`] when a side is zero or over
    /// [`MAX_PATTERN_SIDE`], or the area over [`MAX_PATTERN_TEXELS`].
    pub fn texel_count(width: u32, height: u32) -> Result<u64, PatternError> {
        let area = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width > MAX_PATTERN_SIDE
            || height > MAX_PATTERN_SIDE
            || area > MAX_PATTERN_TEXELS
        {
            return Err(PatternError::BadSize);
        }
        Ok(area)
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, RGBA `u16`, row-major.
    #[must_use]
    pub fn texels(&self) -> &[u16] {
        &self.texels
    }

    /// The shared pixel buffer.
    #[must_use]
    pub fn shared_texels(&self) -> &Arc<Vec<u16>> {
        &self.texels
    }

    /// Bytes of pixel data this pattern holds.
    #[must_use]
    pub fn byte_len(&self) -> u64 {
        (self.texels.len() as u64).saturating_mul(2)
    }

    /// Texel `(x, y)` (wrapped into the pattern) as straight RGBA in
    /// `0.0..=1.0`.
    #[must_use]
    pub fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let w = i64::from(self.width);
        let h = i64::from(self.height);
        let (xi, yi) = (x.rem_euclid(w), y.rem_euclid(h));
        let index = usize::try_from(yi * w + xi).unwrap_or(0).saturating_mul(4);
        let at = |k: usize| {
            self.texels
                .get(index + k)
                .map_or(0.0, |v| f32::from(*v) / 65535.0)
        };
        [at(0), at(1), at(2), at(3)]
    }

    /// The pattern sampled at `(u, v)` in texel units, where the CENTRE of
    /// texel `(i, j)` is at `(i, j)`; wraps (the pattern tiles forever).
    /// Bilinear over the four surrounding texels, weighted in premultiplied
    /// alpha; a position on a texel centre returns that texel exactly.
    /// Non-finite coordinates read texel `(0, 0)`.
    #[must_use]
    pub fn sample(&self, u: f64, v: f64) -> [f32; 4] {
        if !(u.is_finite() && v.is_finite()) {
            return self.texel(0, 0);
        }
        let (u0, v0) = (u.floor(), v.floor());
        // Texel indices stay far inside i64 for any position the callers
        // produce (document coordinates over a scale >= 0.01); the casts
        // saturate otherwise and the wrap keeps the read in range.
        #[allow(clippy::cast_possible_truncation)]
        let (xi, yi) = (u0 as i64, v0 as i64);
        #[allow(clippy::cast_possible_truncation)]
        let (fu, fv) = ((u - u0) as f32, (v - v0) as f32);
        if fu == 0.0 && fv == 0.0 {
            return self.texel(xi, yi);
        }
        let taps = [
            (self.texel(xi, yi), (1.0 - fu) * (1.0 - fv)),
            (self.texel(xi.saturating_add(1), yi), fu * (1.0 - fv)),
            (self.texel(xi, yi.saturating_add(1)), (1.0 - fu) * fv),
            (
                self.texel(xi.saturating_add(1), yi.saturating_add(1)),
                fu * fv,
            ),
        ];
        let mut rgb = [0.0f32; 3];
        let mut alpha = 0.0f32;
        for (t, w) in taps {
            alpha += t[3] * w;
            for (c, s) in rgb.iter_mut().zip(t) {
                *c += s * t[3] * w;
            }
        }
        if alpha <= 0.0 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        [
            (rgb[0] / alpha).min(1.0),
            (rgb[1] / alpha).min(1.0),
            (rgb[2] / alpha).min(1.0),
            alpha.min(1.0),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: u32, h: u32) -> Pattern {
        let mut texels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                texels.extend_from_slice(&[
                    u16::try_from(x * 1000).unwrap_or(0),
                    u16::try_from(y * 1000).unwrap_or(0),
                    0,
                    65535,
                ]);
            }
        }
        Pattern::new("id".into(), "n".into(), w, h, texels)
            .unwrap_or_else(|e| unreachable!("{e:?}"))
    }

    #[test]
    fn construction_is_bounded_before_anything_is_allocated() {
        let id = || "x".to_owned();
        assert_eq!(
            Pattern::new(String::new(), String::new(), 1, 1, vec![0; 4]),
            Err(PatternError::BadId)
        );
        assert_eq!(
            Pattern::new(
                "x".repeat(MAX_PATTERN_ID_BYTES + 1),
                String::new(),
                1,
                1,
                vec![0; 4]
            ),
            Err(PatternError::BadId)
        );
        // Huge claimed sizes fail on the size alone, with an empty buffer.
        for (w, h) in [
            (0, 1),
            (1, 0),
            (MAX_PATTERN_SIDE + 1, 1),
            (3000, 3000),
            (u32::MAX, u32::MAX),
        ] {
            assert_eq!(
                Pattern::new(id(), String::new(), w, h, Vec::new()),
                Err(PatternError::BadSize),
                "{w}x{h}"
            );
        }
        assert_eq!(
            Pattern::new(id(), String::new(), 2, 2, vec![0; 15]),
            Err(PatternError::BadPixels)
        );
        assert!(Pattern::new(id(), String::new(), 2, 2, vec![0; 16]).is_ok());
        assert_eq!(pattern(2, 2).byte_len(), 32);
    }

    #[test]
    fn a_position_on_a_texel_centre_is_that_texel_and_tiling_wraps() {
        let p = pattern(4, 3);
        assert_eq!(p.sample(2.0, 1.0), p.texel(2, 1));
        assert_eq!(p.sample(2.0 + 4.0, 1.0 - 3.0), p.texel(2, 1));
        assert_eq!(p.sample(-1.0, -1.0), p.texel(3, 2));
        // Half way between texels 1 and 2 on x.
        let mid = p.sample(1.5, 0.0);
        let (a, b) = (p.texel(1, 0), p.texel(2, 0));
        assert!((mid[0] - (a[0] + b[0]) / 2.0).abs() < 1e-6);
        assert_eq!(mid[1], a[1]);
        // Half way across the wrap seam.
        let seam = p.sample(3.5, 0.0);
        let (a, b) = (p.texel(3, 0), p.texel(0, 0));
        assert!((seam[0] - (a[0] + b[0]) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn sampling_is_total_and_weights_by_alpha() {
        let p = pattern(2, 2);
        for (u, v) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (f64::MAX, f64::MIN),
            (1e300, -1e300),
        ] {
            let s = p.sample(u, v);
            assert!(s.iter().all(|c| (0.0..=1.0).contains(c)), "{u} {v}");
        }
        // A transparent texel next to an opaque red one: the colour stays
        // red (premultiplied weights), the alpha halves.
        let q = Pattern::new(
            "a".into(),
            String::new(),
            2,
            1,
            vec![65535, 0, 0, 65535, 0, 65535, 0, 0],
        )
        .unwrap_or_else(|e| unreachable!("{e:?}"));
        let s = q.sample(0.5, 0.0);
        assert!((s[3] - 0.5).abs() < 1e-6);
        assert!((s[0] - 1.0).abs() < 1e-6 && s[1].abs() < 1e-6);
    }
}
