//! A PSD's global pattern blocks (`Patt`, `Pat2`, `Pat3`; task 111).
//!
//! **Layout** (checked against the fixtures' bytes and psd-tools 1.17.4's
//! `psd/patterns.py`; all integers big-endian). The block is a list of
//! items, each a `u32` length, that many bytes, then zero padding to a
//! multiple of four. An item is:
//!
//! - `u32` version (1), `u32` image mode (1 Grayscale, 2 Indexed, 3 RGB,
//!   ...), two `i16` (the pattern's point);
//! - the name: `u32` UTF-16 unit count, then the units;
//! - the unique ID: a Pascal string (one length byte, no padding);
//! - for Indexed only a 768-byte colour table and 4 bytes (not read: an
//!   Indexed pattern is refused);
//! - the "virtual memory array list": `u32` version (3), `u32` length,
//!   then `u32 x 4` rectangle (top, left, bottom, right), `u32` channel
//!   count `n`, and `n + 2` channel records. A record is a `u32` written
//!   flag; when set, a `u32` length (0 = empty), `u32` depth, `u32 x 4`
//!   rectangle, `u16` pixel depth, `u8` compression (0 raw, 1 `PackBits`
//!   with a `u16` count per row, 2 zlib, 3 zlib with prediction) and the
//!   data (`length - 23` bytes).
//!
//! Only the patterns a layer's overlay names are decoded; an item is
//! examined (its header) at most once and every size is checked against
//! `aurora_core::pattern`'s caps **before** a buffer is allocated. RGB and
//! Grayscale patterns of 8 or 16 bits are read; another mode, depth,
//! compression or an over-cap size is a refused pattern (reported by the
//! caller), never an approximation.

#![allow(clippy::many_single_char_names)] // the format's own w, h, n, c, v

use std::collections::{BTreeMap, BTreeSet};

use aurora_core::pattern::{MAX_PATTERN_POOL_BYTES, MAX_PATTERNS, Pattern};

use super::{ByteSource, Window};
use crate::error::IoError;

/// The largest item read into memory (an RGBA 16-bit 4 Mpx pattern is
/// 32 MiB raw; the rest is the header).
const MAX_ITEM_BYTES: usize = 40 * 1024 * 1024;
/// Items examined across all of a file's pattern blocks.
const MAX_ITEMS: usize = 4096;
/// Item bytes read, in all, across a file's pattern blocks.
const MAX_BYTES_READ: u64 = 256 * 1024 * 1024;
/// Most channel records one pattern may claim (`n + 2`).
const MAX_CHANNEL_RECORDS: u32 = 64;
/// Most UTF-16 units in a pattern's name that are walked.
const MAX_NAME_UNITS: usize = 4096;

/// What the pattern blocks gave for the IDs asked for.
#[derive(Debug, Default)]
pub(super) struct PatternPool {
    /// Patterns decoded, by ID.
    pub ready: BTreeMap<String, Pattern>,
    /// IDs found but refused (unsupported mode, depth or compression,
    /// damaged data, over a cap).
    pub refused: BTreeSet<String>,
}

/// Reads the patterns in `wanted` from the `windows` (each a `Patt`,
/// `Pat2` or `Pat3` block's data).
///
/// # Errors
///
/// A read failure of the source itself. A damaged item never fails the
/// open: it ends that block's walk, or refuses that one pattern.
pub(super) fn read_pool(
    src: &mut dyn ByteSource,
    windows: &mut [Window],
    wanted: &BTreeSet<String>,
) -> Result<PatternPool, IoError> {
    let mut pool = PatternPool::default();
    let mut items = 0usize;
    let mut bytes = 0u64;
    let mut pixel_bytes = 0u64;
    if wanted.is_empty() {
        return Ok(pool);
    }
    for window in windows.iter_mut() {
        while window.remaining() >= 4 {
            if items >= MAX_ITEMS || bytes >= MAX_BYTES_READ {
                return Ok(pool);
            }
            let len = u32::from_be_bytes(window.take_array::<4>(src, "pattern length")?);
            let len = len as usize;
            let pad = (4 - len % 4) % 4;
            if !window.fits(len) {
                break;
            }
            items += 1;
            if len > MAX_ITEM_BYTES {
                // An over-size item cannot be named without reading it;
                // skipped, and any overlay that wanted it finds nothing.
                window.advance(len, "pattern")?;
            } else {
                let body = window.take(src, len, "pattern")?;
                bytes = bytes.saturating_add(len as u64);
                if let Some((id, fate)) = read_item(&body, wanted, &pool, pixel_bytes) {
                    match fate {
                        Some(pattern) => {
                            pixel_bytes = pixel_bytes.saturating_add(pattern.byte_len());
                            pool.ready.insert(id, pattern);
                        }
                        None => {
                            pool.refused.insert(id);
                        }
                    }
                }
            }
            if window.fits(pad) {
                window.advance(pad, "pattern padding")?;
            }
        }
    }
    Ok(pool)
}

/// A bounds-checked big-endian cursor.
struct Cur<'a>(&'a [u8]);

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let head = self.0.get(..n)?;
        self.0 = self.0.get(n..)?;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// One channel record that was written.
struct Channel<'a> {
    depth: u32,
    rect: [u32; 4],
    pixel_depth: u16,
    compression: u8,
    data: &'a [u8],
}

/// Parses one item. `None`: not an item (or one that names no usable ID);
/// `Some((id, None))`: a pattern that is wanted but refused;
/// `Some((id, Some(pattern)))`: decoded. Items not wanted (or already
/// decoded) return `None` after the header.
fn read_item(
    body: &[u8],
    wanted: &BTreeSet<String>,
    pool: &PatternPool,
    pixel_bytes: u64,
) -> Option<(String, Option<Pattern>)> {
    let mut c = Cur(body);
    if c.u32()? != 1 {
        return None;
    }
    let mode = c.u32()?;
    c.take(4)?; // the point
    let units = usize::try_from(c.u32()?).ok()?;
    if units > MAX_NAME_UNITS {
        return None;
    }
    let name_bytes = c.take(units.checked_mul(2)?)?;
    let id_len = usize::from(c.u8()?);
    let id_bytes = c.take(id_len)?;
    let id: String = String::from_utf8_lossy(id_bytes)
        .trim_matches(|ch: char| ch == '\0' || ch.is_whitespace())
        .to_owned();
    if !wanted.contains(&id) || pool.ready.contains_key(&id) {
        return None;
    }
    let refused = || Some((id.clone(), None));
    if mode != 1 && mode != 3 {
        return refused();
    }
    if pool.ready.len() >= MAX_PATTERNS {
        return refused();
    }
    let name: String = char::decode_utf16(name_bytes.chunks_exact(2).map(|p| match p {
        [hi, lo] => u16::from_be_bytes([*hi, *lo]),
        _ => 0,
    }))
    .map(|ch| ch.unwrap_or(char::REPLACEMENT_CHARACTER))
    .take_while(|ch| *ch != '\0')
    .collect();
    match decode(&mut c, mode == 3, id.clone(), name, pixel_bytes) {
        Some(pattern) => Some((id, Some(pattern))),
        None => refused(),
    }
}

fn decode(
    c: &mut Cur<'_>,
    rgb: bool,
    id: String,
    name: String,
    pixel_bytes: u64,
) -> Option<Pattern> {
    if c.u32()? != 3 {
        return None;
    }
    let length = usize::try_from(c.u32()?).ok()?;
    let mut v = Cur(c.take(length)?);
    let rect = [v.u32()?, v.u32()?, v.u32()?, v.u32()?];
    let [top, left, bottom, right] = rect;
    let width = right.checked_sub(left)?;
    let height = bottom.checked_sub(top)?;
    // Sizes are checked before any buffer exists.
    let area = Pattern::texel_count(width, height).ok()?;
    if pixel_bytes.saturating_add(area * 8) > MAX_PATTERN_POOL_BYTES {
        return None;
    }
    let records = v.u32()?.checked_add(2)?;
    if records > MAX_CHANNEL_RECORDS {
        return None;
    }
    let mut written: Vec<Channel<'_>> = Vec::new();
    for _ in 0..records {
        if v.u32()? == 0 {
            continue;
        }
        let len = usize::try_from(v.u32()?).ok()?;
        if len == 0 {
            continue;
        }
        let body_len = len.checked_sub(23)?;
        let depth = v.u32()?;
        let rect = [v.u32()?, v.u32()?, v.u32()?, v.u32()?];
        let pixel_depth = v.u16()?;
        let compression = v.u8()?;
        let data = v.take(body_len)?;
        written.push(Channel {
            depth,
            rect,
            pixel_depth,
            compression,
            data,
        });
    }
    let colour = if rgb { 3 } else { 1 };
    if written.len() < colour {
        return None;
    }
    let used = written.get(..(colour + usize::from(written.len() > colour)))?;
    let w = width as usize;
    let h = height as usize;
    let n = w.checked_mul(h)?;
    let mut planes: Vec<Vec<u16>> = Vec::new();
    for ch in used {
        if ch.rect != rect
            || u32::from(ch.pixel_depth) != ch.depth
            || !matches!(ch.pixel_depth, 8 | 16)
        {
            return None;
        }
        planes.push(plane(ch, w, h, n)?);
    }
    let mut texels: Vec<u16> = Vec::new();
    texels.try_reserve_exact(n.checked_mul(4)?).ok()?;
    let alpha = planes.get(colour);
    for i in 0..n {
        let at = |p: &Vec<u16>| p.get(i).copied();
        if rgb {
            texels.push(at(planes.first()?)?);
            texels.push(at(planes.get(1)?)?);
            texels.push(at(planes.get(2)?)?);
        } else {
            let g = at(planes.first()?)?;
            texels.extend_from_slice(&[g, g, g]);
        }
        texels.push(match alpha {
            Some(a) => at(a)?,
            None => u16::MAX,
        });
    }
    Pattern::new(id, name, width, height, texels).ok()
}

/// One channel's samples as `u16` (an 8-bit sample `v` becomes `v * 257`).
fn plane(ch: &Channel<'_>, w: usize, h: usize, n: usize) -> Option<Vec<u16>> {
    let bps = usize::from(ch.pixel_depth / 8);
    let row = w.checked_mul(bps)?;
    let total = n.checked_mul(bps)?;
    let raw: Vec<u8> = match ch.compression {
        0 => ch.data.get(..total)?.to_vec(),
        1 => {
            // One u16 compressed length per row, then the rows.
            let mut counts = Cur(ch.data);
            let mut lens = Vec::new();
            for _ in 0..h {
                lens.push(usize::from(counts.u16()?));
            }
            let mut out: Vec<u8> = Vec::new();
            out.try_reserve_exact(total).ok()?;
            for len in lens {
                super::unpack_bits(counts.take(len)?, &mut out, row).ok()?;
            }
            out
        }
        2 | 3 => {
            let mut out = super::inflate(ch.data, total).ok()?;
            if ch.compression == 3 {
                super::undo_prediction(&mut out, row, bps);
            }
            out
        }
        _ => return None,
    };
    if raw.len() != total {
        return None;
    }
    let mut out: Vec<u16> = Vec::new();
    out.try_reserve_exact(n).ok()?;
    if bps == 2 {
        out.extend(raw.chunks_exact(2).map(|p| match p {
            [hi, lo] => u16::from_be_bytes([*hi, *lo]),
            _ => 0,
        }));
    } else {
        out.extend(raw.iter().map(|v| u16::from(*v) * 257));
    }
    Some(out)
}
