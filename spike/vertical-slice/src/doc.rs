//! Document: a base layer plus a live stroke layer, and the brush that writes
//! into it.
//!
//! Tests PRD invariant §7.3.5 — the active stroke renders into a dedicated
//! scratch layer and merges into the base on stroke end, so a dab never
//! triggers a full graph re-evaluation. That separation is the whole reason the
//! 10 ms budget is reachable.

use crate::tiles::{CHANNELS, TILE, TileId, TileStore, texel_index};
use half::f16;
use std::path::PathBuf;

pub struct Document {
    pub width: u32,
    pub height: u32,
    /// Committed pixels.
    pub base: TileStore,
    /// The in-progress stroke, composited over `base` at draw time.
    pub stroke: TileStore,
    pub stroke_active: bool,
}

impl Document {
    pub fn new(width: u32, height: u32, scratch: &PathBuf, budget_bytes: usize) -> std::io::Result<Self> {
        Ok(Self {
            width,
            height,
            base: TileStore::new(scratch.join("base"), budget_bytes)?,
            // The stroke layer is small by construction: only what the current
            // stroke touches. It gets a fixed slice of the budget.
            stroke: TileStore::new(scratch.join("stroke"), budget_bytes / 8)?,
            stroke_active: false,
        })
    }

    pub fn tiles_across(&self) -> u32 {
        self.width.div_ceil(TILE)
    }

    pub fn tiles_down(&self) -> u32 {
        self.height.div_ceil(TILE)
    }

    pub fn begin_stroke(&mut self) {
        self.stroke_active = true;
    }

    /// Merge the stroke layer into the base. This is the only point where the
    /// committed document changes, and the only place a graph re-evaluation
    /// would be needed in a real implementation.
    pub fn end_stroke(&mut self) -> std::io::Result<usize> {
        let ids = self.stroke.known_ids();
        let merged = ids.len();
        for id in ids {
            let src: Vec<f16> = match self.stroke.get(id)? {
                Some(t) => t.texels.clone(),
                None => continue,
            };
            let dst = self.base.get_mut(id)?;
            for i in (0..src.len()).step_by(CHANNELS) {
                let sa = src[i + 3].to_f32();
                if sa <= 0.0 {
                    continue;
                }
                // Source-over, premultiplied.
                for c in 0..CHANNELS {
                    let s = src[i + c].to_f32();
                    let d = dst.texels[i + c].to_f32();
                    dst.texels[i + c] = f16::from_f32(s + d * (1.0 - sa));
                }
            }
        }
        self.stroke = TileStore::new(self.stroke_scratch(), self.stroke.budget_tiles() * crate::tiles::TILE_BYTES)?;
        self.stroke_active = false;
        Ok(merged)
    }

    fn stroke_scratch(&self) -> PathBuf {
        // Same directory the stroke store was created with; recreated fresh.
        PathBuf::from(std::env::temp_dir()).join("aurora-slice-stroke")
    }

    /// Stamp one brush dab into the stroke layer.
    ///
    /// Returns the number of tiles touched — a dab near a tile corner touches
    /// four, which is the interesting case for upload cost.
    pub fn dab(&mut self, cx: f32, cy: f32, radius: f32, colour: [f32; 3]) -> std::io::Result<usize> {
        if radius <= 0.0 {
            return Ok(0);
        }
        let min_x = ((cx - radius).floor().max(0.0)) as u32;
        let min_y = ((cy - radius).floor().max(0.0)) as u32;
        let max_x = ((cx + radius).ceil()).min(self.width as f32 - 1.0).max(0.0) as u32;
        let max_y = ((cy + radius).ceil()).min(self.height as f32 - 1.0).max(0.0) as u32;

        let t0 = TileId { x: min_x / TILE, y: min_y / TILE };
        let t1 = TileId { x: max_x / TILE, y: max_y / TILE };
        let mut touched = 0;

        for ty in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let id = TileId { x: tx, y: ty };
                let origin_x = tx * TILE;
                let origin_y = ty * TILE;
                let tile = self.stroke.get_mut(id)?;
                touched += 1;

                let lx0 = min_x.saturating_sub(origin_x).min(TILE - 1);
                let ly0 = min_y.saturating_sub(origin_y).min(TILE - 1);
                let lx1 = (max_x.saturating_sub(origin_x)).min(TILE - 1);
                let ly1 = (max_y.saturating_sub(origin_y)).min(TILE - 1);

                for ly in ly0..=ly1 {
                    for lx in lx0..=lx1 {
                        let px = (origin_x + lx) as f32 + 0.5;
                        let py = (origin_y + ly) as f32 + 0.5;
                        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                        if d > radius {
                            continue;
                        }
                        // Smooth falloff; hardness is not a variable here.
                        let a = (1.0 - (d / radius)).clamp(0.0, 1.0).powf(1.5);
                        if a <= 0.0 {
                            continue;
                        }
                        let i = texel_index(lx, ly);
                        let dst_a = tile.texels[i + 3].to_f32();
                        // Max-alpha accumulation within a stroke, so overlapping
                        // dabs do not darken along the spine.
                        if a > dst_a {
                            for c in 0..3 {
                                tile.texels[i + c] = f16::from_f32(colour[c] * a);
                            }
                            tile.texels[i + 3] = f16::from_f32(a);
                        }
                    }
                }
            }
        }
        Ok(touched)
    }

    /// Stamp dabs along a segment at fixed spacing, as a real brush engine would.
    pub fn stroke_segment(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        radius: f32,
        spacing: f32,
        colour: [f32; 3],
    ) -> std::io::Result<usize> {
        let dx = to.0 - from.0;
        let dy = to.1 - from.1;
        let len = (dx * dx + dy * dy).sqrt();
        let step = (radius * spacing).max(0.5);
        let n = (len / step).ceil().max(1.0) as usize;
        let mut touched = 0;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            touched += self.dab(from.0 + dx * t, from.1 + dy * t, radius, colour)?;
        }
        Ok(touched)
    }
}
