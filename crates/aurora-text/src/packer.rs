//! A shelf rectangle packer for glyph atlases.

/// One horizontal shelf: a strip of the atlas `height` pixels tall whose
/// next free column is `cursor_x`.
#[derive(Debug, Clone, Copy)]
struct Shelf {
    y: u32,
    height: u32,
    cursor_x: u32,
}

/// Packs rectangles into a fixed `width` × `height` area, shelf by shelf.
///
/// Every placed rectangle keeps [`ShelfPacker::PADDING`] pixels to its
/// right and below it that no *other* rectangle is placed in. That is a
/// guard band, not a guarantee of zeros: nothing here clears the owner's
/// texture, so after the glyph atlas resets those pixels may still hold an
/// earlier glyph's coverage. It is harmless today only because the glyph
/// renderer reads whole texels inside each image with `textureLoad` (no
/// sampler, no filtering) and never touches the band; a future filtered or
/// scaled path would need to clear it. There is no deletion: when the
/// area is full, [`ShelfPacker::insert`] returns `None` and the owner
/// decides what to do (the glyph atlas clears and starts over).
#[derive(Debug, Clone)]
pub struct ShelfPacker {
    width: u32,
    height: u32,
    shelves: Vec<Shelf>,
    next_y: u32,
}

impl ShelfPacker {
    /// Empty pixels kept to the right of and below every rectangle.
    pub const PADDING: u32 = 1;

    /// An empty packer over a `width` × `height` area.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    /// The packed area's width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The packed area's height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Places a `w` × `h` rectangle, returning its top-left corner, or
    /// `None` if it does not fit anywhere (including a zero-sized request
    /// wider or taller than the whole area).
    pub fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let padded_w = w.checked_add(Self::PADDING)?;
        let padded_h = h.checked_add(Self::PADDING)?;
        if w > self.width || h > self.height {
            return None;
        }
        // First fit on an existing shelf tall enough, preferring the
        // tightest one so short glyphs do not waste tall shelves.
        let mut best: Option<usize> = None;
        for (index, shelf) in self.shelves.iter().enumerate() {
            let fits_height = shelf.height >= padded_h;
            let fits_width = shelf.cursor_x.saturating_add(w) <= self.width;
            if fits_height && fits_width {
                let tighter = best
                    .and_then(|b| self.shelves.get(b))
                    .is_none_or(|b| shelf.height < b.height);
                if tighter {
                    best = Some(index);
                }
            }
        }
        if let Some(shelf) = best.and_then(|b| self.shelves.get_mut(b)) {
            let origin = (shelf.cursor_x, shelf.y);
            shelf.cursor_x = shelf.cursor_x.saturating_add(padded_w);
            return Some(origin);
        }
        // Open a new shelf below the last one.
        if self.next_y.saturating_add(h) > self.height {
            return None;
        }
        let y = self.next_y;
        self.shelves.push(Shelf {
            y,
            height: padded_h,
            cursor_x: padded_w,
        });
        self.next_y = self.next_y.saturating_add(padded_h);
        Some((0, y))
    }

    /// Forgets every placed rectangle; the whole area is free again.
    pub fn clear(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::ShelfPacker;

    fn overlaps(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn packer_places_nonoverlapping_within_bounds() {
        let mut packer = ShelfPacker::new(64, 64);
        let mut placed = Vec::new();
        for i in 0..40_u32 {
            let (w, h) = (3 + i % 7, 4 + i % 5);
            if let Some((x, y)) = packer.insert(w, h) {
                assert!(x + w <= 64 && y + h <= 64, "rect {i} out of bounds");
                placed.push((x, y, w, h));
            }
        }
        assert!(placed.len() > 20, "only {} of 40 placed", placed.len());
        for (i, a) in placed.iter().enumerate() {
            for b in placed.iter().skip(i + 1) {
                assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn packer_returns_none_when_full_and_clear_resets() {
        let mut packer = ShelfPacker::new(16, 16);
        let mut count = 0;
        while packer.insert(7, 7).is_some() {
            count += 1;
            assert!(
                count <= 4,
                "a 16x16 area cannot hold more than 4 padded 7x7s"
            );
        }
        assert_eq!(count, 4);
        assert_eq!(packer.insert(7, 7), None);
        assert_eq!(packer.insert(20, 1), None, "wider than the area");
        packer.clear();
        assert_eq!(packer.insert(7, 7), Some((0, 0)));
    }

    #[test]
    fn packer_respects_padding() {
        let mut packer = ShelfPacker::new(32, 32);
        let a = packer.insert(5, 5);
        let b = packer.insert(5, 5);
        assert_eq!(a, Some((0, 0)));
        assert_eq!(b, Some((5 + ShelfPacker::PADDING, 0)));
        // A taller rectangle opens a new shelf one padding below the first.
        let c = packer.insert(5, 9);
        assert_eq!(c, Some((0, 5 + ShelfPacker::PADDING)));
    }
}
