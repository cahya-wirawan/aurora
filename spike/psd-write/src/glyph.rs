//! Standalone glyph rasterization proof — PLAN 0.6 / FINDINGS.md finding 8's
//! named gap: editing a text layer requires re-rendering glyphs into pixel
//! channels, or the file is internally inconsistent (descriptor vs. preview).
//!
//! **Scope: this file does not touch the PSD writer yet.** It answers a
//! narrower question first — can `cosmic-text` rasterize real text to an
//! RGBA8 buffer, headlessly, with a font Aurora controls rather than
//! whatever happens to be installed on the host — before wiring that output
//! into `psd.rs`'s layer-record writer (which doesn't have a `TySh` slot at
//! all yet; that's the next step, not this one). Same incremental pattern as
//! finding 7's Python proof-of-concept before findings 9/10's Rust port.
//!
//! The font is bundled (`reference/fonts/DejaVuSans.ttf`, Bitstream Vera
//! License — see `reference/fonts/DejaVu-LICENSE`), not read from the host's
//! installed fonts, so rendering is reproducible on any machine or in CI.
//! It will not visually match whatever font a given Photoshop file actually
//! names — that's not the question here. The question is narrower: can
//! Aurora rasterize *some* correct, legible glyphs for edited text at all.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};

const FONT_BYTES: &[u8] = include_bytes!("../reference/fonts/DejaVuSans.ttf");

/// An RGBA8, row-major, straight-alpha pixel buffer — the same convention
/// `psd.rs`'s `Layer::pixels` already uses, so this can be dropped straight
/// into a `Layer` once the writer grows a `TySh` slot.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Rasterizes `text` at `font_size_px`, filled with `color` (straight RGB,
/// full opacity — `FillColor` alpha compositing is a follow-on concern, not
/// this proof's), wrapped to `max_width_px` if given.
///
/// Canvas size is derived from the shaped layout itself (line count ×
/// line height, and the widest shaped line) — there is no fixed bounding
/// box to render into here, matching how *point* text has none either
/// (`reference/tysh.bin`'s own `bbox` is `(0,0,0,0)` — see FINDINGS.md).
/// Paragraph/area text's fixed-box case is a real difference (finding 11)
/// but out of scope for this proof.
pub fn rasterize(
    text: &str,
    font_size_px: f32,
    color: (u8, u8, u8),
    max_width_px: Option<f32>,
) -> Raster {
    let mut font_system = FontSystem::new_with_fonts([cosmic_text::fontdb::Source::Binary(
        std::sync::Arc::new(FONT_BYTES),
    )]);
    let mut swash_cache = SwashCache::new();

    let line_height_px = font_size_px * 1.2;
    let metrics = Metrics::new(font_size_px, line_height_px);
    let mut buffer = Buffer::new(&mut font_system, metrics);
    let mut buffer = buffer.borrow_with(&mut font_system);
    buffer.set_size(max_width_px, None);

    let attrs = Attrs::new().family(Family::Name("DejaVu Sans"));
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(true);

    let line_count = buffer.layout_runs().count().max(1);
    let height = (line_height_px * line_count as f32).ceil() as u32;
    let width = max_width_px.map_or_else(
        || {
            buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(0.0_f32, f32::max)
                .ceil() as u32
        },
        |w| w.ceil() as u32,
    );
    let width = width.max(1);
    let height = height.max(1);

    let mut pixels = vec![0u8; (width * height) as usize * 4];
    let text_color = Color::rgb(color.0, color.1, color.2);
    buffer.draw(&mut swash_cache, text_color, |x, y, w, h, glyph_color| {
        let a = glyph_color.a();
        if a == 0 || x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
            return;
        }
        // `draw` calls back per-subpixel-run; every real glyph run seen from
        // this font/shaper is 1x1 in practice, matching the terminal example
        // upstream. A run larger than that would silently under-paint if
        // ignored, so fail loudly rather than guess — that is exactly the
        // failure mode findings 1/2/9 warned about, one library deeper.
        assert_eq!(
            (w, h),
            (1, 1),
            "unexpected multi-pixel glyph run from cosmic-text"
        );
        let i = ((y as u32 * width + x as u32) * 4) as usize;
        pixels[i] = glyph_color.r();
        pixels[i + 1] = glyph_color.g();
        pixels[i + 2] = glyph_color.b();
        pixels[i + 3] = a;
    });

    Raster {
        width,
        height,
        pixels,
    }
}

/// Writes a binary PPM (P6) — trivial, dependency-free, and enough to
/// eyeball the result in any image viewer. Not a PSD channel yet (finding 8
/// still needs the writer-integration step); alpha is composited onto white
/// since PPM has no alpha channel.
pub fn write_ppm(raster: &Raster, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::fs::File::create(path)?;
    write!(out, "P6\n{} {}\n255\n", raster.width, raster.height)?;
    let mut rgb = Vec::with_capacity((raster.width * raster.height) as usize * 3);
    for px in raster.pixels.chunks_exact(4) {
        let a = f32::from(px[3]) / 255.0;
        for &channel in &px[..3] {
            let fg = f32::from(channel);
            let composited = fg * a + 255.0 * (1.0 - a);
            rgb.push(composited.round() as u8);
        }
    }
    out.write_all(&rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_nonempty_visible_text() {
        let raster = rasterize("Aurora spike", 24.0, (0, 0, 0), None);
        assert!(raster.width > 1 && raster.height > 1);
        let inked: usize = raster.pixels.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            inked > 20,
            "expected meaningful glyph coverage, got {inked} inked pixels out of {}",
            raster.width * raster.height
        );
    }

    #[test]
    fn empty_string_has_no_ink() {
        // Not a real editing case (an empty text layer), but a cheap check
        // that "no glyphs" and "some glyphs" are actually distinguishable —
        // if this failed it would mean the harness paints unconditionally.
        let raster = rasterize("", 24.0, (0, 0, 0), None);
        let inked: usize = raster.pixels.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert_eq!(inked, 0);
    }

    #[test]
    fn wider_text_produces_a_wider_canvas() {
        let short = rasterize("A", 24.0, (0, 0, 0), None);
        let long = rasterize("Aurora spike", 24.0, (0, 0, 0), None);
        assert!(
            long.width > short.width,
            "longer text should shape to a wider line: short={} long={}",
            short.width,
            long.width
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        // Same bundled font, same input, must produce byte-identical output
        // on every run — this is what makes a hash-based regression test
        // legitimate here rather than fragile. Confirms it before relying on
        // that property anywhere else.
        let a = rasterize("Aurora spike", 24.0, (10, 20, 30), None);
        let b = rasterize("Aurora spike", 24.0, (10, 20, 30), None);
        assert_eq!(a.width, b.width);
        assert_eq!(a.height, b.height);
        assert_eq!(a.pixels, b.pixels);
    }
}
