//! Header-level size checks for untrusted image files, made **before** a
//! byte is decoded or even copied: a 3.5 MB PNG whose header says
//! 30000 x 30000 decodes to ten gigabytes. Only the fixed-size header of
//! each format is read, with a bounded number of reads; nothing is
//! allocated from a number in the file.
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
