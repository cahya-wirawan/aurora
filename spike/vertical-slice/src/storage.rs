//! Save and reload, so the slice closes the loop rather than stopping at
//! "pixels appeared on screen".
//!
//! Format is deliberately trivial: header, then (tile id, raw f16 texels) for
//! every tile that exists. No compression, no layer tree — `.aur` is a separate
//! design problem (PRD §12 Q7).

use crate::doc::Document;
use crate::tiles::{CHANNELS, TEXELS, TileId};
use half::f16;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"AURSLICE";
const VERSION: u32 = 1;

pub fn save(doc: &mut Document, path: &Path) -> std::io::Result<u64> {
    let mut w = BufWriter::new(File::create(path)?);
    let ids = doc.base.known_ids();

    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&doc.width.to_le_bytes())?;
    w.write_all(&doc.height.to_le_bytes())?;
    w.write_all(&(ids.len() as u64).to_le_bytes())?;

    let mut bytes = 0u64;
    for id in ids {
        let Some(tile) = doc.base.get(id)? else { continue };
        let bits: Vec<u16> = tile.texels.iter().map(|t| t.to_bits()).collect();
        w.write_all(&id.x.to_le_bytes())?;
        w.write_all(&id.y.to_le_bytes())?;
        let raw: &[u8] = bytemuck::cast_slice(&bits);
        w.write_all(raw)?;
        bytes += raw.len() as u64 + 8;
    }
    w.flush()?;
    Ok(bytes)
}

pub fn load(doc: &mut Document, path: &Path) -> std::io::Result<usize> {
    let mut r = BufReader::new(File::open(path)?);

    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a slice file",
        ));
    }
    let mut u32buf = [0u8; 4];
    let mut u64buf = [0u8; 8];
    r.read_exact(&mut u32buf)?;
    if u32::from_le_bytes(u32buf) != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "version mismatch",
        ));
    }
    r.read_exact(&mut u32buf)?;
    let width = u32::from_le_bytes(u32buf);
    r.read_exact(&mut u32buf)?;
    let height = u32::from_le_bytes(u32buf);
    if width != doc.width || height != doc.height {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "document size mismatch",
        ));
    }
    r.read_exact(&mut u64buf)?;
    let count = u64::from_le_bytes(u64buf) as usize;

    let mut raw = vec![0u8; TEXELS * CHANNELS * 2];
    for _ in 0..count {
        r.read_exact(&mut u32buf)?;
        let x = u32::from_le_bytes(u32buf);
        r.read_exact(&mut u32buf)?;
        let y = u32::from_le_bytes(u32buf);
        r.read_exact(&mut raw)?;

        let tile = doc.base.get_mut(TileId { x, y })?;
        for (dst, &bits) in tile
            .texels
            .iter_mut()
            .zip(bytemuck::cast_slice::<u8, u16>(&raw))
        {
            *dst = f16::from_bits(bits);
        }
    }
    Ok(count)
}
