//! Sparse tile store with LRU eviction to a scratch directory.
//!
//! Tests PRD invariant §7.3.1 (nothing assumes a document fits in memory) and
//! ADR 0003 (half-float storage, 8 bytes/px).
//!
//! Deliberately simple: fixed tile size, no compression, no background writer.
//! Compression is required by the real design; leaving it out here makes the
//! paging cost worse than production, which is the safe direction for a
//! measurement we want to trust.

use half::f16;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

/// 256×256 RGBA f16 = 512 KiB per tile.
pub const TILE: u32 = 256;
pub const TEXELS: usize = (TILE * TILE) as usize;
pub const CHANNELS: usize = 4;
pub const TILE_BYTES: usize = TEXELS * CHANNELS * 2;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
}

pub struct Tile {
    /// RGBA, premultiplied, linear light. Length is TEXELS * CHANNELS.
    pub texels: Vec<f16>,
    /// Needs re-upload to the GPU.
    pub dirty: bool,
    /// Differs from what is on the scratch disk.
    pub unsaved: bool,
}

impl Tile {
    fn blank() -> Self {
        Self { texels: vec![f16::ZERO; TEXELS * CHANNELS], dirty: true, unsaved: true }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub tiles_created: u64,
    pub evictions: u64,
    pub faults: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub peak_resident: usize,
}

pub struct TileStore {
    resident: HashMap<TileId, Tile>,
    /// Approximate LRU: most-recently-touched pushed to the back.
    recency: Vec<TileId>,
    /// Tiles that exist but currently live on disk.
    paged_out: HashMap<TileId, PathBuf>,
    budget: usize,
    scratch: PathBuf,
    pub stats: Stats,
}

impl TileStore {
    /// `budget_bytes` caps resident tile memory; beyond it, tiles page to disk.
    pub fn new(scratch: PathBuf, budget_bytes: usize) -> std::io::Result<Self> {
        fs::create_dir_all(&scratch)?;
        Ok(Self {
            resident: HashMap::new(),
            recency: Vec::new(),
            paged_out: HashMap::new(),
            budget: (budget_bytes / TILE_BYTES).max(4),
            scratch,
            stats: Stats::default(),
        })
    }

    pub fn budget_tiles(&self) -> usize {
        self.budget
    }

    #[allow(dead_code)] // Handy when instrumenting eviction behaviour.
    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// Every tile that exists, resident or paged out.
    pub fn known_ids(&self) -> Vec<TileId> {
        let mut v: Vec<TileId> = self.resident.keys().copied().collect();
        v.extend(self.paged_out.keys().copied());
        v.sort_unstable();
        v.dedup();
        v
    }

    fn touch(&mut self, id: TileId) {
        if let Some(pos) = self.recency.iter().rposition(|&t| t == id) {
            self.recency.remove(pos);
        }
        self.recency.push(id);
    }

    /// Fetch for writing, creating a blank tile if it does not exist yet.
    pub fn get_mut(&mut self, id: TileId) -> std::io::Result<&mut Tile> {
        self.ensure_resident(id)?;
        self.touch(id);
        self.evict_if_needed()?;
        let tile = self
            .resident
            .get_mut(&id)
            .expect("just ensured resident");
        tile.dirty = true;
        tile.unsaved = true;
        Ok(tile)
    }

    /// Fetch for reading. Returns None only if the tile has never been written.
    pub fn get(&mut self, id: TileId) -> std::io::Result<Option<&Tile>> {
        if !self.resident.contains_key(&id) && !self.paged_out.contains_key(&id) {
            return Ok(None);
        }
        self.ensure_resident(id)?;
        self.touch(id);
        self.evict_if_needed()?;
        Ok(self.resident.get(&id))
    }

    pub fn exists(&self, id: TileId) -> bool {
        self.resident.contains_key(&id) || self.paged_out.contains_key(&id)
    }

    fn ensure_resident(&mut self, id: TileId) -> std::io::Result<()> {
        if self.resident.contains_key(&id) {
            return Ok(());
        }
        let tile = if let Some(path) = self.paged_out.remove(&id) {
            // Page fault: this is the cost the 60 FPS budget has to absorb.
            self.stats.faults += 1;
            let mut buf = Vec::with_capacity(TILE_BYTES);
            fs::File::open(&path)?.read_to_end(&mut buf)?;
            self.stats.bytes_in += buf.len() as u64;
            let texels: Vec<f16> = bytemuck::cast_slice::<u8, u16>(&buf)
                .iter()
                .map(|&bits| f16::from_bits(bits))
                .collect();
            Tile { texels, dirty: true, unsaved: false }
        } else {
            self.stats.tiles_created += 1;
            Tile::blank()
        };
        self.resident.insert(id, tile);
        self.stats.peak_resident = self.stats.peak_resident.max(self.resident.len());
        Ok(())
    }

    fn evict_if_needed(&mut self) -> std::io::Result<()> {
        while self.resident.len() > self.budget {
            // Oldest entry that is still resident.
            let victim = self
                .recency
                .iter()
                .copied()
                .find(|id| self.resident.contains_key(id));
            let Some(victim) = victim else { break };
            self.recency.retain(|&t| t != victim);

            let Some(tile) = self.resident.remove(&victim) else { break };
            let path = self.scratch.join(format!("{}_{}.tile", victim.x, victim.y));
            let bits: Vec<u16> = tile.texels.iter().map(|t| t.to_bits()).collect();
            let bytes: &[u8] = bytemuck::cast_slice(&bits);
            fs::File::create(&path)?.write_all(bytes)?;
            self.stats.bytes_out += bytes.len() as u64;
            self.stats.evictions += 1;
            self.paged_out.insert(victim, path);
        }
        Ok(())
    }

    /// Resident tiles needing GPU upload, clearing their dirty flag.
    pub fn take_dirty(&mut self, limit: usize) -> Vec<TileId> {
        let mut out = Vec::new();
        for (id, tile) in &mut self.resident {
            if tile.dirty {
                tile.dirty = false;
                out.push(*id);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }
}

/// Index of the texel at (x, y) within a tile.
#[inline]
pub fn texel_index(x: u32, y: u32) -> usize {
    ((y * TILE + x) as usize) * CHANNELS
}
