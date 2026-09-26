//! Shaping, measurement, carets, and glyph rasterization for single-line
//! UI text.

use std::collections::HashMap;
use std::sync::Arc;

use harfrust::{Direction, ShaperData, UnicodeBuffer};
use swash::scale::{Render, ScaleContext, Source, image::Content};
use swash::zeno::{Format, Vector};
use unicode_segmentation::UnicodeSegmentation;

use crate::font::UI_FONT_REGULAR;

/// Why a [`TextEngine`] could not be built.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    /// The bundled font's bytes did not parse as an OpenType font.
    #[error("the bundled UI font did not parse: {0}")]
    FontParse(String),
}

/// How to set a run of text. All three values come from design tokens
/// (`type.size.*`, `type.line_height.*`, `type.weight.*`) in the caller;
/// this crate never picks a size of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Font size in logical pixels.
    pub size_px: f32,
    /// Line height as a multiple of `size_px`.
    pub line_height: f32,
    /// CSS-style weight (400 regular). Carried and cached on, but only the
    /// Regular face is bundled, so every weight currently renders Regular.
    pub weight: u16,
}

/// A line's size, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    /// Advance width of the whole line.
    pub width: f32,
    /// Distance from the line box's baseline up to the font's ascender.
    pub ascent: f32,
    /// Distance from the baseline down to the font's descender (positive).
    pub descent: f32,
    /// `size_px * line_height`.
    pub line_height: f32,
}

/// Which quarter-pixel horizontal offset a glyph was rasterized at.
///
/// Glyph quads are placed on whole physical pixels (so the atlas texel
/// grid lines up with the framebuffer's); the fractional part of a glyph's
/// pen position is carried here instead, and the glyph is rasterized
/// pre-shifted by it. Four bins is the same resolution `cosmic-text` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubpixelBin(u8);

impl SubpixelBin {
    /// No subpixel offset.
    pub const ZERO: Self = Self(0);

    /// Number of bins per physical pixel.
    pub const COUNT: u8 = 4;

    /// The horizontal offset, in physical pixels, this bin rasterizes at.
    #[must_use]
    pub fn offset(self) -> f32 {
        f32::from(self.0) / f32::from(Self::COUNT)
    }
}

/// Splits a physical x coordinate into the whole pixel a glyph quad is
/// placed at and the [`SubpixelBin`] its fractional part rounds to.
///
/// A fraction that rounds up to a whole pixel moves to the next pixel at
/// bin 0, so the result always satisfies
/// `pixel + bin.offset() ≈ x` to within an eighth of a pixel.
#[must_use]
pub fn snap_glyph_origin(x: f32) -> (i32, SubpixelBin) {
    if !x.is_finite() {
        return (0, SubpixelBin(0));
    }
    let floor = x.floor();
    let fraction = x - floor;
    let count = f32::from(SubpixelBin::COUNT);
    // Truncation is the point: `fraction * 4` is in `[0, 4]`, rounded.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bin = (fraction * count).round() as u8;
    #[allow(clippy::cast_possible_truncation)]
    let pixel = floor as i32;
    if bin >= SubpixelBin::COUNT {
        (pixel.saturating_add(1), SubpixelBin(0))
    } else {
        (pixel, SubpixelBin(bin))
    }
}

/// Identifies one rasterized glyph image: which glyph, at which physical
/// size, at which subpixel offset. The glyph atlas is keyed on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// The glyph's id in the bundled font.
    pub glyph_id: u16,
    /// Physical font size, as `f32::to_bits`.
    pub size_bits: u32,
    /// Horizontal subpixel offset.
    pub bin: SubpixelBin,
}

impl GlyphKey {
    /// The key for `glyph_id` at `size_phys` physical pixels, offset by `bin`.
    #[must_use]
    pub fn new(glyph_id: u16, size_phys: f32, bin: SubpixelBin) -> Self {
        Self {
            glyph_id,
            size_bits: size_phys.to_bits(),
            bin,
        }
    }

    /// The physical font size this key rasterizes at.
    #[must_use]
    pub fn size_phys(self) -> f32 {
        f32::from_bits(self.size_bits)
    }
}

/// One glyph of a [`ShapedLine`], positioned in *physical* pixels relative
/// to the line's pen origin (left edge, baseline).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    /// The glyph's id in the bundled font.
    pub glyph_id: u16,
    /// Pen x plus the shaper's x offset, physical pixels.
    pub x_phys: f32,
    /// The shaper's y offset, physical pixels, y-down.
    pub y_phys: f32,
}

/// A shaped single line of text.
///
/// Sizes are in logical pixels except [`ShapedLine::glyphs`], which are in
/// physical pixels because that is the grid they are rasterized on.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedLine {
    /// Advance width, logical pixels.
    pub width: f32,
    /// Ascender above the baseline, logical pixels.
    pub ascent: f32,
    /// Descender below the baseline (positive), logical pixels.
    pub descent: f32,
    /// `size_px * line_height`, logical pixels.
    pub line_height: f32,
    /// The physical font size glyphs were shaped at (`size_px * scale`).
    pub size_phys: f32,
    /// The scale factor the line was shaped for.
    pub scale_factor: f32,
    /// Glyphs in visual (left-to-right) order.
    pub glyphs: Vec<PlacedGlyph>,
    /// `(byte offset, logical x)` at every grapheme boundary of the text,
    /// including `0` and `text.len()`, in ascending byte order.
    pub carets: Vec<(usize, f32)>,
}

impl ShapedLine {
    /// This line's [`TextMetrics`].
    #[must_use]
    pub fn metrics(&self) -> TextMetrics {
        TextMetrics {
            width: self.width,
            ascent: self.ascent,
            descent: self.descent,
            line_height: self.line_height,
        }
    }

    /// The logical x of a caret placed *before* byte `byte`, or `None` if
    /// `byte` is not a grapheme boundary (a caret can never sit inside a
    /// grapheme cluster). Inside a ligature that spans several graphemes
    /// the position is interpolated across the ligature's advance.
    #[must_use]
    pub fn caret_x(&self, byte: usize) -> Option<f32> {
        self.carets
            .binary_search_by_key(&byte, |(b, _)| *b)
            .ok()
            .and_then(|index| self.carets.get(index))
            .map(|(_, x)| *x)
    }
}

/// A rasterized glyph: an 8-bit coverage mask and where it sits relative to
/// the glyph's (whole-pixel) origin on the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphMask {
    /// Mask width in physical pixels.
    pub width: u32,
    /// Mask height in physical pixels.
    pub height: u32,
    /// Offset from the origin to the mask's left edge.
    pub left: i32,
    /// Offset from the baseline *up* to the mask's top edge.
    pub top: i32,
    /// `width * height` coverage values, row-major, top row first.
    pub alpha: Vec<u8>,
}

/// Everything a shaped line depends on except its text. The cache is keyed
/// by this, then by the text (as a `String` looked up by `&str`), so a
/// cache hit allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ShapeKey {
    size_bits: u32,
    line_height_bits: u32,
    weight: u16,
    scale_bits: u32,
}

#[derive(Debug)]
struct ShapeEntry {
    line: Arc<ShapedLine>,
    last_used: u64,
}

/// Shapes, measures, and rasterizes single-line UI text in the bundled
/// font. Owns every cache; one per application.
pub struct TextEngine {
    hr_font: harfrust::FontRef<'static>,
    shaper_data: ShaperData,
    sw_font: swash::FontRef<'static>,
    units_per_em: f32,
    ascent_units: f32,
    descent_units: f32,
    scale_context: ScaleContext,
    shaped: HashMap<ShapeKey, HashMap<String, ShapeEntry>>,
    shaped_len: usize,
    glyphs: HashMap<GlyphKey, Option<GlyphMask>>,
    frame: u64,
}

impl std::fmt::Debug for TextEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEngine")
            .field("units_per_em", &self.units_per_em)
            .field("shaped_entries", &self.shaped_len)
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl TextEngine {
    /// The most shaped lines kept at once. Past it, entries not used in the
    /// current frame are dropped immediately; if every entry is current the
    /// cache is cleared (a pathological frame shapes more than this many
    /// distinct strings, and pays for it next frame, not with memory).
    pub const SHAPE_CACHE_CAP: usize = 4096;

    /// The most rasterized glyphs kept at once; past it the glyph cache is
    /// cleared and refilled on demand. UI text at one or two sizes and
    /// scale factors uses a few hundred.
    pub const GLYPH_CACHE_CAP: usize = 8192;

    /// How many frames an unused shaped line survives in the cache.
    pub const SHAPE_CACHE_MAX_IDLE_FRAMES: u64 = 2;

    /// Loads the bundled UI font.
    ///
    /// # Errors
    ///
    /// [`TextError::FontParse`] if the bundled bytes are not a font (they
    /// are pinned by a test, so this is a build defect, not a user error).
    pub fn new() -> Result<Self, TextError> {
        let hr_font = harfrust::FontRef::new(UI_FONT_REGULAR)
            .map_err(|e| TextError::FontParse(e.to_string()))?;
        let shaper_data = ShaperData::new(&hr_font);
        let sw_font = swash::FontRef::from_index(UI_FONT_REGULAR, 0)
            .ok_or_else(|| TextError::FontParse("swash rejected the font".to_owned()))?;
        let metrics = sw_font.metrics(&[]);
        if metrics.units_per_em == 0 {
            return Err(TextError::FontParse("units_per_em is 0".to_owned()));
        }
        Ok(Self {
            hr_font,
            shaper_data,
            sw_font,
            units_per_em: f32::from(metrics.units_per_em),
            ascent_units: metrics.ascent,
            descent_units: metrics.descent.abs(),
            scale_context: ScaleContext::new(),
            shaped: HashMap::new(),
            shaped_len: 0,
            glyphs: HashMap::new(),
            frame: 0,
        })
    }

    /// Starts a new frame: shaped lines unused for
    /// [`Self::SHAPE_CACHE_MAX_IDLE_FRAMES`] frames are dropped.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let frame = self.frame;
        self.retain_shaped(|entry| {
            frame.wrapping_sub(entry.last_used) < Self::SHAPE_CACHE_MAX_IDLE_FRAMES
        });
    }

    /// Keeps only the shaped lines `keep` accepts, dropping emptied style
    /// buckets and recounting.
    fn retain_shaped(&mut self, mut keep: impl FnMut(&ShapeEntry) -> bool) {
        let mut len = 0_usize;
        self.shaped.retain(|_, bucket| {
            bucket.retain(|_, entry| keep(entry));
            len = len.saturating_add(bucket.len());
            !bucket.is_empty()
        });
        self.shaped_len = len;
    }

    /// How many shaped lines are cached right now.
    #[must_use]
    pub fn cached_line_count(&self) -> usize {
        self.shaped_len
    }

    /// Shapes `text` as one left-to-right line in `style`, for a display at
    /// `scale_factor` physical pixels per logical pixel. Cached: the same
    /// arguments return the same `Arc` until the entry is swept.
    ///
    /// A non-finite or non-positive size, line-height multiplier, or scale
    /// shapes as an empty line of zero height rather than failing — a label is never worth an
    /// error path through a paint function.
    pub fn shape(&mut self, text: &str, style: &TextStyle, scale_factor: f32) -> Arc<ShapedLine> {
        let key = ShapeKey {
            size_bits: style.size_px.to_bits(),
            line_height_bits: style.line_height.to_bits(),
            weight: style.weight,
            scale_bits: scale_factor.to_bits(),
        };
        let frame = self.frame;
        if let Some(entry) = self
            .shaped
            .get_mut(&key)
            .and_then(|bucket| bucket.get_mut(text))
        {
            entry.last_used = frame;
            return Arc::clone(&entry.line);
        }
        let line = Arc::new(self.shape_uncached(text, style, scale_factor));
        if self.shaped_len >= Self::SHAPE_CACHE_CAP {
            self.retain_shaped(|entry| entry.last_used == frame);
            if self.shaped_len >= Self::SHAPE_CACHE_CAP {
                self.shaped.clear();
                self.shaped_len = 0;
            }
        }
        let previous = self.shaped.entry(key).or_default().insert(
            text.to_owned(),
            ShapeEntry {
                line: Arc::clone(&line),
                last_used: frame,
            },
        );
        if previous.is_none() {
            self.shaped_len = self.shaped_len.saturating_add(1);
        }
        line
    }

    /// [`Self::shape`]'s metrics alone.
    pub fn measure(&mut self, text: &str, style: &TextStyle, scale_factor: f32) -> TextMetrics {
        self.shape(text, style, scale_factor).metrics()
    }

    fn shape_uncached(&self, text: &str, style: &TextStyle, scale_factor: f32) -> ShapedLine {
        let valid = |v: f32| v.is_finite() && v > 0.0;
        if !valid(style.size_px) || !valid(scale_factor) || !valid(style.line_height) {
            return ShapedLine {
                width: 0.0,
                ascent: 0.0,
                descent: 0.0,
                line_height: 0.0,
                size_phys: 0.0,
                scale_factor: 1.0,
                glyphs: Vec::new(),
                carets: vec![(0, 0.0)],
            };
        }
        let size_phys = style.size_px * scale_factor;
        let units_to_phys = size_phys / self.units_per_em;

        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        // Left-to-right only (see the crate docs): a right-to-left script
        // is laid out in logical order rather than mirrored.
        buffer.set_direction(Direction::LeftToRight);
        let shaper = self
            .shaper_data
            .shaper(&self.hr_font)
            .point_size(Some(size_phys))
            .build();
        let output = shaper.shape(buffer, &[]);

        let mut glyphs = Vec::with_capacity(output.glyph_infos().len());
        // (cluster start byte, pen x at the cluster's start), physical.
        let mut clusters: Vec<(usize, f32)> = Vec::new();
        let mut pen = 0.0_f32;
        for (info, pos) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            let cluster = usize::try_from(info.cluster).unwrap_or(usize::MAX);
            if clusters.last().is_none_or(|(c, _)| *c != cluster) {
                clusters.push((cluster, pen));
            }
            #[allow(clippy::cast_precision_loss)]
            let (x_offset, y_offset, x_advance) = (
                pos.x_offset as f32 * units_to_phys,
                pos.y_offset as f32 * units_to_phys,
                pos.x_advance as f32 * units_to_phys,
            );
            glyphs.push(PlacedGlyph {
                glyph_id: u16::try_from(info.glyph_id).unwrap_or(0),
                x_phys: pen + x_offset,
                // Font units are y-up; the line's coordinates are y-down.
                y_phys: -y_offset,
            });
            pen += x_advance;
        }
        let width_phys = pen;

        let carets = caret_stops(text, &clusters, width_phys)
            .into_iter()
            .map(|(byte, x)| (byte, x / scale_factor))
            .collect();

        let size_to_logical = style.size_px / self.units_per_em;
        ShapedLine {
            width: width_phys / scale_factor,
            ascent: self.ascent_units * size_to_logical,
            descent: self.descent_units * size_to_logical,
            line_height: style.size_px * style.line_height,
            size_phys,
            scale_factor,
            glyphs,
            carets,
        }
    }

    /// [`Self::rasterize`], cached: the mask for `key` (or `None` for a
    /// glyph with no coverage), rasterized at most once per cache
    /// lifetime ([`Self::GLYPH_CACHE_CAP`]).
    pub fn glyph(&mut self, key: GlyphKey) -> Option<&GlyphMask> {
        if !self.glyphs.contains_key(&key) {
            if self.glyphs.len() >= Self::GLYPH_CACHE_CAP {
                self.glyphs.clear();
            }
            let mask = self.rasterize(key);
            self.glyphs.insert(key, mask);
        }
        self.glyphs.get(&key).and_then(Option::as_ref)
    }

    /// Rasterizes one glyph to an 8-bit coverage mask, unhinted, at the
    /// key's physical size and subpixel offset. `None` for a glyph with no
    /// outline (a space) or one the rasterizer cannot produce as a plain
    /// coverage mask (a colour glyph — Inter has none).
    pub fn rasterize(&mut self, key: GlyphKey) -> Option<GlyphMask> {
        let size = key.size_phys();
        if !(size.is_finite() && size > 0.0) {
            return None;
        }
        let mut scaler = self
            .scale_context
            .builder(self.sw_font)
            .size(size)
            .hint(false)
            .build();
        let image = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .offset(Vector::new(key.bin.offset(), 0.0))
            .render(&mut scaler, key.glyph_id)?;
        if image.content != Content::Mask
            || image.placement.width == 0
            || image.placement.height == 0
        {
            return None;
        }
        let expected = usize::try_from(image.placement.width)
            .ok()?
            .checked_mul(usize::try_from(image.placement.height).ok()?)?;
        if image.data.len() != expected {
            return None;
        }
        Some(GlyphMask {
            width: image.placement.width,
            height: image.placement.height,
            left: image.placement.left,
            top: image.placement.top,
            alpha: image.data,
        })
    }
}

/// Every grapheme boundary of `text` with its physical x, given the
/// shaper's clusters (ascending start byte, pen x at start) and the line's
/// total width. A boundary strictly inside a multi-grapheme cluster (a
/// ligature) is interpolated across that cluster's advance.
fn caret_stops(text: &str, clusters: &[(usize, f32)], width: f32) -> Vec<(usize, f32)> {
    let mut stops = Vec::new();
    let mut boundaries: Vec<usize> = text.grapheme_indices(true).map(|(b, _)| b).collect();
    boundaries.push(text.len());
    let mut cluster_index = 0_usize;
    let mut i = 0_usize;
    while let Some(&byte) = boundaries.get(i) {
        // Advance to the cluster containing `byte`.
        while clusters
            .get(cluster_index + 1)
            .is_some_and(|(start, _)| *start <= byte)
        {
            cluster_index += 1;
        }
        let (start, x0) = clusters.get(cluster_index).copied().unwrap_or((0, 0.0));
        let (end, x1) = clusters
            .get(cluster_index + 1)
            .copied()
            .unwrap_or((text.len(), width));
        if byte >= text.len() {
            stops.push((byte, width));
        } else if byte <= start {
            stops.push((byte, x0));
        } else {
            // Inside a ligature: interpolate by grapheme count.
            let inside: Vec<usize> = boundaries
                .iter()
                .copied()
                .filter(|b| *b >= start && *b < end)
                .collect();
            let n = inside.len().max(1);
            let k = inside.iter().position(|b| *b == byte).unwrap_or(0);
            #[allow(clippy::cast_precision_loss)]
            let t = k as f32 / n as f32;
            stops.push((byte, x0 + (x1 - x0) * t));
        }
        i += 1;
    }
    stops
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: TextStyle = TextStyle {
        size_px: 13.0,
        line_height: 1.4,
        weight: 400,
    };

    fn engine() -> Result<TextEngine, TextError> {
        TextEngine::new()
    }

    #[test]
    fn shaping_hello_at_13px_gives_the_pinned_advances() -> Result<(), TextError> {
        let mut engine = engine()?;
        let line = engine.shape("Hello", &MD, 1.0);
        assert_eq!(line.glyphs.len(), 5);
        let xs: Vec<f32> = line.glyphs.iter().map(|g| g.x_phys).collect();
        // Pinned against Inter 4.1 Regular: a font or shaper bump that
        // moves these is meant to fail here and be re-pinned on purpose.
        let expected = [0.0, 9.661_133, 17.240_234, 20.388_672, 23.537_11];
        for (x, e) in xs.iter().zip(expected) {
            assert!(
                (x - e).abs() < 1e-3,
                "advances drifted: {xs:?} width {}",
                line.width
            );
        }
        assert!(
            (line.width - 31.332_031).abs() < 1e-3,
            "width {}",
            line.width
        );
        Ok(())
    }

    #[test]
    fn width_scales_linearly_with_scale_factor() -> Result<(), TextError> {
        let mut engine = engine()?;
        let one = engine.shape("Layers", &MD, 1.0);
        let two = engine.shape("Layers", &MD, 2.0);
        assert!(one.width > 0.0);
        assert!(
            (one.width - two.width).abs() < 1e-3,
            "{} vs {}",
            one.width,
            two.width
        );
        assert!((two.size_phys - 26.0).abs() < f32::EPSILON);
        let last_one = one.glyphs.last().map(|g| g.x_phys).unwrap_or_default();
        let last_two = two.glyphs.last().map(|g| g.x_phys).unwrap_or_default();
        assert!((last_two - 2.0 * last_one).abs() < 1e-3);
        Ok(())
    }

    #[test]
    fn empty_string_has_zero_width_and_a_line_height() -> Result<(), TextError> {
        let mut engine = engine()?;
        let m = engine.measure("", &MD, 1.0);
        assert!(m.width.abs() < f32::EPSILON);
        assert!(m.line_height > 0.0);
        assert!(m.ascent > 0.0 && m.descent > 0.0);
        Ok(())
    }

    #[test]
    fn line_height_is_size_times_token_multiplier() -> Result<(), TextError> {
        let mut engine = engine()?;
        let m = engine.measure("Aa", &MD, 2.0);
        assert!((m.line_height - 13.0 * 1.4).abs() < 1e-4);
        let tight = TextStyle {
            line_height: 1.2,
            ..MD
        };
        assert!((engine.measure("Aa", &tight, 1.0).line_height - 15.6).abs() < 1e-4);
        Ok(())
    }

    #[test]
    fn caret_x_at_0_is_0_and_at_len_is_width() -> Result<(), TextError> {
        let mut engine = engine()?;
        let text = "Brush size";
        let line = engine.shape(text, &MD, 1.0);
        assert_eq!(line.caret_x(0), Some(0.0));
        assert_eq!(line.caret_x(text.len()), Some(line.width));
        Ok(())
    }

    #[test]
    fn caret_x_is_monotone_over_grapheme_boundaries() -> Result<(), TextError> {
        let mut engine = engine()?;
        // "ffi" is a ligature candidate; "é" as e + combining acute is one
        // grapheme of two code points.
        let text = "office cafe\u{301} ok";
        let line = engine.shape(text, &MD, 1.5);
        let boundaries: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(b, _)| b)
            .chain(std::iter::once(text.len()))
            .collect();
        let mut previous = -1.0_f32;
        for b in &boundaries {
            let x = line.caret_x(*b);
            assert!(x.is_some(), "no caret at grapheme boundary {b}");
            let x = x.unwrap_or_default();
            assert!(x > previous, "caret at {b} ({x}) not after {previous}");
            previous = x;
        }
        assert_eq!(line.carets.len(), boundaries.len());
        Ok(())
    }

    #[test]
    fn caret_x_rejects_a_non_boundary_byte() -> Result<(), TextError> {
        let mut engine = engine()?;
        let text = "cafe\u{301}!";
        let line = engine.shape(text, &MD, 1.0);
        // Byte 4 is the start of the combining mark: inside the "é" grapheme.
        assert_eq!(line.caret_x(4), None);
        assert!(line.caret_x(3).is_some());
        assert!(line.caret_x(6).is_some());
        assert_eq!(line.caret_x(text.len() + 1), None);
        Ok(())
    }

    #[test]
    fn rasterize_a_is_nonempty_mask_of_declared_size() -> Result<(), TextError> {
        let mut engine = engine()?;
        let line = engine.shape("A", &MD, 2.0);
        let glyph = line.glyphs.first().copied();
        assert!(glyph.is_some());
        let glyph = glyph.unwrap_or(PlacedGlyph {
            glyph_id: 0,
            x_phys: 0.0,
            y_phys: 0.0,
        });
        let mask = engine.rasterize(GlyphKey::new(
            glyph.glyph_id,
            line.size_phys,
            SubpixelBin(0),
        ));
        let Some(mask) = mask else {
            return Err(TextError::FontParse("no mask for A".to_owned()));
        };
        assert!(mask.width > 0 && mask.height > 0);
        assert_eq!(mask.alpha.len(), (mask.width * mask.height) as usize);
        assert!(mask.alpha.contains(&255), "A has solid interior coverage");
        // A capital sits on the baseline and rises about cap-height.
        assert!(mask.top > 10 && mask.top < 26, "top {}", mask.top);
        Ok(())
    }

    #[test]
    fn rasterize_space_is_none_or_empty() -> Result<(), TextError> {
        let mut engine = engine()?;
        let line = engine.shape(" ", &MD, 1.0);
        let id = line.glyphs.first().map(|g| g.glyph_id).unwrap_or_default();
        assert!(line.width > 0.0, "a space still advances");
        assert_eq!(
            engine.rasterize(GlyphKey::new(id, line.size_phys, SubpixelBin(0))),
            None
        );
        Ok(())
    }

    #[test]
    fn subpixel_bins_rasterize_differently() -> Result<(), TextError> {
        let mut engine = engine()?;
        let line = engine.shape("l", &MD, 1.0);
        let id = line.glyphs.first().map(|g| g.glyph_id).unwrap_or_default();
        let a = engine.rasterize(GlyphKey::new(id, 13.0, SubpixelBin(0)));
        let b = engine.rasterize(GlyphKey::new(id, 13.0, SubpixelBin(2)));
        assert!(a.is_some() && b.is_some());
        assert_ne!(a, b, "a half-pixel shift must change the coverage");
        Ok(())
    }

    #[test]
    fn snap_glyph_origin_rounds_the_fraction_to_quarter_bins() {
        assert_eq!(snap_glyph_origin(10.0), (10, SubpixelBin(0)));
        assert_eq!(snap_glyph_origin(10.26), (10, SubpixelBin(1)));
        assert_eq!(snap_glyph_origin(10.5), (10, SubpixelBin(2)));
        assert_eq!(snap_glyph_origin(10.9), (11, SubpixelBin(0)));
        assert_eq!(snap_glyph_origin(-0.3), (-1, SubpixelBin(3)));
        assert_eq!(snap_glyph_origin(f32::NAN), (0, SubpixelBin(0)));
    }

    #[test]
    fn glyph_cache_rasterizes_once_and_caches_empty_glyphs() -> Result<(), TextError> {
        let mut engine = engine()?;
        let line = engine.shape("A ", &MD, 1.0);
        let ids: Vec<u16> = line.glyphs.iter().map(|g| g.glyph_id).collect();
        let a = GlyphKey::new(
            ids.first().copied().unwrap_or_default(),
            13.0,
            SubpixelBin::ZERO,
        );
        let space = GlyphKey::new(
            ids.get(1).copied().unwrap_or_default(),
            13.0,
            SubpixelBin::ZERO,
        );
        let first = engine.glyph(a).cloned();
        assert!(first.is_some());
        assert_eq!(engine.glyph(a).cloned(), first);
        assert_eq!(engine.glyph(space), None);
        assert_eq!(
            engine.glyphs.len(),
            2,
            "the empty space glyph is cached too"
        );
        Ok(())
    }

    #[test]
    fn shape_cache_hits_on_second_call() -> Result<(), TextError> {
        let mut engine = engine()?;
        let a = engine.shape("Cached", &MD, 1.0);
        let b = engine.shape("Cached", &MD, 1.0);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(engine.cached_line_count(), 1);
        Ok(())
    }

    #[test]
    fn shape_cache_key_includes_scale() -> Result<(), TextError> {
        let mut engine = engine()?;
        let a = engine.shape("Scaled", &MD, 1.0);
        let b = engine.shape("Scaled", &MD, 2.0);
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(engine.cached_line_count(), 2);
        assert!((b.size_phys - 2.0 * a.size_phys).abs() < f32::EPSILON);
        Ok(())
    }

    #[test]
    fn shape_cache_sweeps_entries_unused_for_two_frames() -> Result<(), TextError> {
        let mut engine = engine()?;
        let _ = engine.shape("old", &MD, 1.0);
        let _ = engine.shape("kept", &MD, 1.0);
        engine.begin_frame();
        let _ = engine.shape("kept", &MD, 1.0);
        assert_eq!(engine.cached_line_count(), 2, "one idle frame is tolerated");
        engine.begin_frame();
        assert_eq!(engine.cached_line_count(), 1, "\"old\" idle for two frames");
        let _ = engine.shape("kept", &MD, 1.0);
        engine.begin_frame();
        assert_eq!(engine.cached_line_count(), 1);
        Ok(())
    }

    #[test]
    // Exact by construction: 30.0 split into thirds of whole numbers.
    #[allow(clippy::float_cmp)]
    fn caret_stops_interpolate_inside_a_multi_grapheme_cluster() {
        // One cluster covering three graphemes (a synthetic "ffi"
        // ligature) starting at pen 0, then a second cluster "x" at 30.
        let stops = caret_stops("ffix", &[(0, 0.0), (3, 30.0)], 40.0);
        assert_eq!(
            stops,
            vec![(0, 0.0), (1, 10.0), (2, 20.0), (3, 30.0), (4, 40.0)]
        );
        // The same ligature as the whole line interpolates to its width.
        let stops = caret_stops("ffi", &[(0, 0.0)], 30.0);
        assert_eq!(stops, vec![(0, 0.0), (1, 10.0), (2, 20.0), (3, 30.0)]);
        // A later ligature interpolates from its own start, not from 0.
        let stops = caret_stops("xffi", &[(0, 0.0), (1, 8.0)], 38.0);
        assert_eq!(
            stops,
            vec![(0, 0.0), (1, 8.0), (2, 18.0), (3, 28.0), (4, 38.0)]
        );
    }

    #[test]
    fn a_non_positive_line_height_shapes_as_an_empty_line() -> Result<(), TextError> {
        let mut engine = engine()?;
        for line_height in [0.0, -1.4, f32::NAN, f32::INFINITY] {
            let style = TextStyle { line_height, ..MD };
            let line = engine.shape("x", &style, 1.0);
            assert!(line.glyphs.is_empty(), "line_height {line_height}");
            assert!(line.line_height.abs() < f32::EPSILON);
        }
        Ok(())
    }

    #[test]
    fn a_shape_cache_hit_is_the_same_arc_across_distinct_styles() -> Result<(), TextError> {
        let mut engine = engine()?;
        let big = TextStyle {
            size_px: 20.0,
            ..MD
        };
        let a = engine.shape("same", &MD, 1.0);
        let b = engine.shape("same", &big, 1.0);
        assert!(
            !Arc::ptr_eq(&a, &b),
            "a different style is a different entry"
        );
        assert_eq!(engine.cached_line_count(), 2);
        assert!(Arc::ptr_eq(&a, &engine.shape("same", &MD, 1.0)));
        assert_eq!(engine.cached_line_count(), 2, "a hit adds nothing");
        Ok(())
    }

    #[test]
    fn invalid_sizes_shape_as_an_empty_line() -> Result<(), TextError> {
        let mut engine = engine()?;
        let zero = TextStyle { size_px: 0.0, ..MD };
        let line = engine.shape("x", &zero, 1.0);
        assert!(line.glyphs.is_empty() && line.width.abs() < f32::EPSILON);
        let nan_scale = engine.shape("x", &MD, f32::NAN);
        assert!(nan_scale.glyphs.is_empty());
        Ok(())
    }
}
