//! Header-level size checks for untrusted image files, made **before** a
//! byte is decoded or even copied: a 3.5 MB PNG whose header says
//! 30000 x 30000 decodes to ten gigabytes. Only the fixed-size header of
//! each format is read, with a bounded number of reads; nothing is
//! allocated from a number in the file. A PSD or PSB also declares a
//! rectangle for each layer (and layer mask): [`psd_layers`] reads those
//! records, so a small canvas cannot hide huge layers.
//!
//! Recognised by content (magic bytes), not by extension, so a file named
//! `.png` that is really a PSB is checked as a PSB. A `.aur` (a zip) is
//! checked by the host, which can read its manifest
//! (the host's `open`). A file with no known magic is left to the
//! decoders, which refuse it cheaply.

use std::io::{Read, Seek, SeekFrom};

/// What the header says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u64,
    pub height: u64,
    pub format: &'static str,
}

impl Dimensions {
    /// Width times height.
    #[must_use]
    pub fn pixels(&self) -> u64 {
        self.width.saturating_mul(self.height)
    }
}

fn u16_at(b: &[u8], at: usize, big: bool) -> Option<u64> {
    let raw: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
    Some(u64::from(if big {
        u16::from_be_bytes(raw)
    } else {
        u16::from_le_bytes(raw)
    }))
}

fn u32_at(b: &[u8], at: usize, big: bool) -> Option<u64> {
    let raw: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(u64::from(if big {
        u32::from_be_bytes(raw)
    } else {
        u32::from_le_bytes(raw)
    }))
}

fn u64_at(b: &[u8], at: usize, big: bool) -> Option<u64> {
    let raw: [u8; 8] = b.get(at..at + 8)?.try_into().ok()?;
    Some(if big {
        u64::from_be_bytes(raw)
    } else {
        u64::from_le_bytes(raw)
    })
}

fn read_exact_at<R: Read + Seek>(r: &mut R, at: u64, len: usize) -> std::io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0_u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Reads the dimensions a PNG, JPEG, TIFF, `BigTIFF`, PSD or PSB header
/// declares. `Ok(None)`: no recognised magic. `Err`: recognised but the
/// header is cut or inconsistent (refuse it).
///
/// # Errors
/// A message when a recognised header cannot be read.
pub fn dimensions<R: Read + Seek>(r: &mut R) -> Result<Option<Dimensions>, String> {
    let cut = |what: &str| format!("the {what} header is damaged or cut short");
    let mut magic = [0_u8; 8];
    r.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let n = r.read(&mut magic).map_err(|e| e.to_string())?;
    let magic = magic.get(..n).unwrap_or_default();
    if magic.starts_with(b"\x89PNG\r\n\x1a\n") {
        let head = read_exact_at(r, 8, 16).map_err(|_| cut("PNG"))?;
        if head.get(4..8) != Some(b"IHDR") {
            return Err(cut("PNG"));
        }
        return Ok(Some(Dimensions {
            width: u32_at(&head, 8, true).ok_or_else(|| cut("PNG"))?,
            height: u32_at(&head, 12, true).ok_or_else(|| cut("PNG"))?,
            format: "PNG",
        }));
    }
    if magic.starts_with(b"8BPS") {
        let head = read_exact_at(r, 0, 26).map_err(|_| cut("PSD"))?;
        let version = u16_at(&head, 4, true).ok_or_else(|| cut("PSD"))?;
        if version != 1 && version != 2 {
            return Err(cut("PSD"));
        }
        return Ok(Some(Dimensions {
            height: u32_at(&head, 14, true).ok_or_else(|| cut("PSD"))?,
            width: u32_at(&head, 18, true).ok_or_else(|| cut("PSD"))?,
            format: if version == 2 { "PSB" } else { "PSD" },
        }));
    }
    if magic.starts_with(&[0xFF, 0xD8]) {
        return jpeg(r).map(Some).map_err(|()| cut("JPEG"));
    }
    if magic.starts_with(b"II*\0")
        || magic.starts_with(b"MM\0*")
        || magic.starts_with(b"II+\0")
        || magic.starts_with(b"MM\0+")
    {
        return tiff(r, magic).map(Some).map_err(|()| cut("TIFF"));
    }
    Ok(None)
}

fn jpeg<R: Read + Seek>(r: &mut R) -> Result<Dimensions, ()> {
    let mut at: u64 = 2;
    // A real file has a few dozen segments before its frame header.
    for _ in 0..512 {
        let head = read_exact_at(r, at, 4).map_err(|_| ())?;
        if head.first() != Some(&0xFF) {
            return Err(());
        }
        let marker = *head.get(1).ok_or(())?;
        let len = u16_at(&head, 2, true).ok_or(())?;
        match marker {
            // padding fill byte
            0xFF => at += 1,
            // standalone markers (no length)
            0x01 | 0xD0..=0xD8 => at += 2,
            // start of frame (not DHT 0xC4, JPG 0xC8, DAC 0xCC)
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                let sof = read_exact_at(r, at + 4, 5).map_err(|_| ())?;
                return Ok(Dimensions {
                    height: u16_at(&sof, 1, true).ok_or(())?,
                    width: u16_at(&sof, 3, true).ok_or(())?,
                    format: "JPEG",
                });
            }
            // start of scan or end of image before any frame header
            0xD9 | 0xDA => return Err(()),
            _ => {
                if len < 2 {
                    return Err(());
                }
                at += 2 + len;
            }
        }
    }
    Err(())
}

fn tiff<R: Read + Seek>(r: &mut R, magic: &[u8]) -> Result<Dimensions, ()> {
    let big = magic.starts_with(b"MM");
    let bigtiff = magic.get(2..4).is_some_and(|m| m == b"+\0" || m == b"\0+");
    let (ifd, entry_len, count_len): (u64, u64, usize) = if bigtiff {
        let head = read_exact_at(r, 0, 16).map_err(|_| ())?;
        (u64_at(&head, 8, big).ok_or(())?, 20, 8)
    } else {
        let head = read_exact_at(r, 0, 8).map_err(|_| ())?;
        (u32_at(&head, 4, big).ok_or(())?, 12, 2)
    };
    let counts = read_exact_at(r, ifd, count_len).map_err(|_| ())?;
    let entries = if bigtiff {
        u64_at(&counts, 0, big).ok_or(())?
    } else {
        u16_at(&counts, 0, big).ok_or(())?
    };
    // Tags are sorted; ImageWidth (256) and ImageLength (257) come first.
    let (mut width, mut height) = (None, None);
    for index in 0..entries.min(256) {
        let at = ifd + count_len as u64 + index * entry_len;
        let entry = read_exact_at(r, at, entry_len as usize).map_err(|_| ())?;
        let tag = u16_at(&entry, 0, big).ok_or(())?;
        let kind = u16_at(&entry, 2, big).ok_or(())?;
        let value_at = if bigtiff { 12 } else { 8 };
        let value = match kind {
            3 => u16_at(&entry, value_at, big),
            4 => u32_at(&entry, value_at, big),
            16 => u64_at(&entry, value_at, big),
            _ => None,
        };
        match tag {
            256 => width = value,
            257 => height = value,
            _ => {}
        }
        if width.is_some() && height.is_some() {
            break;
        }
    }
    Ok(Dimensions {
        width: width.ok_or(())?,
        height: height.ok_or(())?,
        format: "TIFF",
    })
}

// ---------------------------------------------------------------------------
// PSD / PSB layer rectangles
// ---------------------------------------------------------------------------

/// The most layer records [`psd_layers`] walks (a PSD stores the count in an
/// `i16`, so this is also the format's own ceiling).
const MAX_LAYER_RECORDS: u32 = 32_767;
/// A layer record has at most this many channels (the format allows 56).
const MAX_CHANNELS: u64 = 64;
/// How many global tagged blocks are examined looking for the `Lr16`,
/// `Lr32` or `Layr` block that holds a 16/32-bit file's layers.
const MAX_GLOBAL_BLOCKS: u32 = 4096;
/// Tagged-block keys whose length is 8 bytes in a PSB (the list the PSD reader
/// uses: psd-tools' `_BIG_KEYS`).
const PSB_WIDE_KEYS: [&[u8; 4]; 21] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"lnk3", b"lnkE", b"FXid", b"FEid", b"FELS", b"PxSD", b"pths", b"extd", b"extn", b"cinf",
    b"artd",
];

/// What a PSD's layer records declare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerExtent {
    /// Layer records seen.
    pub layers: u32,
    /// The largest single layer rectangle or layer mask rectangle, in pixels.
    pub largest: u64,
    /// The sum of all layer rectangles, in pixels.
    pub total: u64,
}

/// A bounds-checked cursor over a file; nothing is allocated from a number
/// in the file and a read past the end is an error.
struct Cursor<'a, R: Read + Seek> {
    file: &'a mut R,
    at: u64,
    len: u64,
}

impl<R: Read + Seek> Cursor<'_, R> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], ()> {
        let end = self.at.checked_add(N as u64).ok_or(())?;
        if end > self.len {
            return Err(());
        }
        self.file.seek(SeekFrom::Start(self.at)).map_err(|_| ())?;
        let mut buf = [0_u8; N];
        self.file.read_exact(&mut buf).map_err(|_| ())?;
        self.at = end;
        Ok(buf)
    }

    fn u16(&mut self) -> Result<u64, ()> {
        Ok(u64::from(u16::from_be_bytes(self.bytes::<2>()?)))
    }

    fn u32(&mut self) -> Result<u64, ()> {
        Ok(u64::from(u32::from_be_bytes(self.bytes::<4>()?)))
    }

    fn i32(&mut self) -> Result<i64, ()> {
        Ok(i64::from(i32::from_be_bytes(self.bytes::<4>()?)))
    }

    fn u64(&mut self) -> Result<u64, ()> {
        Ok(u64::from_be_bytes(self.bytes::<8>()?))
    }

    /// A 4-byte length, or an 8-byte one in a PSB.
    fn length(&mut self, wide: bool) -> Result<u64, ()> {
        if wide { self.u64() } else { self.u32() }
    }

    fn skip(&mut self, n: u64) -> Result<(), ()> {
        let end = self.at.checked_add(n).ok_or(())?;
        if end > self.len {
            return Err(());
        }
        self.at = end;
        Ok(())
    }
}

fn rect_pixels(top: i64, left: i64, bottom: i64, right: i64) -> u64 {
    let height = bottom.saturating_sub(top).max(0).unsigned_abs();
    let width = right.saturating_sub(left).max(0).unsigned_abs();
    width.saturating_mul(height)
}

/// Reads the layer records of a PSD or PSB (`psb`: version 2) and returns the
/// extent they declare, without decoding a pixel: only the fixed fields of
/// each record are read, with seeks over everything else. The records are
/// found in the layer-info section or, for a 16- or 32-bit file, in the
/// `Lr16`/`Lr32`/`Layr` block after it.
///
/// # Errors
/// A message when the structure is cut short or inconsistent: such a file
/// is refused rather than guessed at (the real reader refuses it too).
pub fn psd_layers<R: Read + Seek>(r: &mut R, psb: bool) -> Result<LayerExtent, String> {
    let cut = || "the PSD layer records are damaged or cut short".to_owned();
    let len = r.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let mut c = Cursor {
        file: r,
        at: 26,
        len,
    };
    // Colour mode data, image resources, then the layer and mask section.
    let colour = c.u32().map_err(|()| cut())?;
    c.skip(colour).map_err(|()| cut())?;
    let resources = c.u32().map_err(|()| cut())?;
    c.skip(resources).map_err(|()| cut())?;
    let section = c.length(psb).map_err(|()| cut())?;
    let section_end = c.at.checked_add(section).ok_or_else(cut)?;
    if section == 0 {
        return Ok(LayerExtent::default());
    }
    if section_end > len {
        return Err(cut());
    }
    let info = c.length(psb).map_err(|()| cut())?;
    let info_end = c.at.checked_add(info).ok_or_else(cut)?;
    if info_end > section_end {
        return Err(cut());
    }
    if info > 0 {
        return layer_info(&mut c, psb, info_end).map_err(|()| cut());
    }
    // The layers live in a global `Lr16` / `Lr32` / `Layr` block.
    let mask = c.u32().map_err(|()| cut())?;
    c.skip(mask).map_err(|()| cut())?;
    for _ in 0..MAX_GLOBAL_BLOCKS {
        if section_end.saturating_sub(c.at) < 12 {
            break;
        }
        let signature = c.bytes::<4>().map_err(|()| cut())?;
        if &signature != b"8BIM" && &signature != b"8B64" {
            break;
        }
        let key = c.bytes::<4>().map_err(|()| cut())?;
        let wide = psb && PSB_WIDE_KEYS.iter().any(|k| **k == key);
        let block = c.length(wide).map_err(|()| cut())?;
        let block_end = c.at.checked_add(block).ok_or_else(cut)?;
        if block_end > section_end {
            return Err(cut());
        }
        if &key == b"Lr16" || &key == b"Lr32" || &key == b"Layr" {
            return layer_info(&mut c, psb, block_end).map_err(|()| cut());
        }
        c.at = block_end;
        // Global blocks are padded to a multiple of four.
        let pad = (4 - block % 4) % 4;
        if section_end.saturating_sub(c.at) >= pad {
            c.at += pad;
        }
    }
    Ok(LayerExtent::default())
}

/// The layer count and records, ending at `end`.
fn layer_info<R: Read + Seek>(
    c: &mut Cursor<'_, R>,
    psb: bool,
    end: u64,
) -> Result<LayerExtent, ()> {
    // Negative: the first alpha channel is the merged result's transparency.
    let count = u32::from(i16::from_be_bytes(c.bytes::<2>()?).unsigned_abs());
    if count > MAX_LAYER_RECORDS {
        return Err(());
    }
    let mut out = LayerExtent::default();
    for _ in 0..count {
        let top = c.i32()?;
        let left = c.i32()?;
        let bottom = c.i32()?;
        let right = c.i32()?;
        let pixels = rect_pixels(top, left, bottom, right);
        out.layers += 1;
        out.largest = out.largest.max(pixels);
        out.total = out.total.saturating_add(pixels);
        let channels = c.u16()?;
        if channels > MAX_CHANNELS {
            return Err(());
        }
        c.skip(channels * if psb { 10 } else { 6 })?;
        // blend signature, key, opacity, clipping, flags, filler
        c.skip(12)?;
        let extra = c.u32()?;
        let extra_end = c.at.checked_add(extra).ok_or(())?;
        if extra_end > end {
            return Err(());
        }
        // The extra data opens with the layer mask: a length, then (when it
        // holds a rectangle) top, left, bottom, right.
        if extra >= 4 {
            let mask = c.u32()?;
            if mask >= 16 && extra >= 20 {
                let top = c.i32()?;
                let left = c.i32()?;
                let bottom = c.i32()?;
                let right = c.i32()?;
                out.largest = out.largest.max(rect_pixels(top, left, bottom, right));
            }
        }
        c.at = extra_end;
    }
    Ok(out)
}

/// Checks a PSD or PSB's layer rectangles against `max_pixels` (the cap the
/// canvas header is held to). `format` is [`Dimensions::format`]; other
/// formats pass. `Err` is the refusal text.
///
/// A single layer (or mask) rectangle over the cap is refused: that layer
/// would be decoded into memory whole. The sum of all layer rectangles is
/// refused past [`LAYER_TOTAL_FACTOR`] times the cap, which bounds the
/// total work of one open.
///
/// # Errors
/// The refusal message.
pub fn check_psd_layers<R: Read + Seek>(
    r: &mut R,
    format: &str,
    max_pixels: u64,
) -> Result<(), String> {
    if format != "PSD" && format != "PSB" {
        return Ok(());
    }
    let extent = psd_layers(r, format == "PSB")?;
    if extent.largest > max_pixels {
        return Err(format!(
            "a {format} layer declares a {} Mpx rectangle; at most {} Mpx may be opened, \
             layers included",
            extent.largest / 1_000_000,
            max_pixels / 1_000_000
        ));
    }
    if extent.total > max_pixels.saturating_mul(LAYER_TOTAL_FACTOR) {
        return Err(format!(
            "the {format} layers declare {} Mpx in all ({} layers); at most {} Mpx (\
             {LAYER_TOTAL_FACTOR} times the open limit) may be opened",
            extent.total / 1_000_000,
            extent.layers,
            max_pixels.saturating_mul(LAYER_TOTAL_FACTOR) / 1_000_000
        ));
    }
    Ok(())
}

/// How many times the open limit the layer rectangles may add up to.
pub const LAYER_TOTAL_FACTOR: u64 = 16;
